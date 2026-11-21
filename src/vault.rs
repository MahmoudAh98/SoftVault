//! High level vault: ties crypto + SQLite together and keeps decrypted data in
//! memory only while unlocked (wiped with `zeroize` on lock).

use crate::crypto::{self, Key, KdfParams, NewVaultMaterial, AAD_FOLDERS, AAD_SETTINGS};
use crate::db::Db;
use crate::protect;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Password,
    Note,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct ItemData {
    #[zeroize(skip)]
    pub kind: Kind,
    pub title: String,
    pub username: String,
    pub password: String,
    pub url: String,
    pub notes: String,
    /// Optional TOTP secret stored *for this login* (base32)
    pub totp: String,
    /// How a secure note is rendered (plain / markdown / code / json / csv)
    #[zeroize(skip)]
    pub format: crate::render::NoteFormat,
    /// Language for `Code` notes (e.g. "rust", "py", "sql")
    pub lang: String,
    pub favorite: bool,
    pub created: i64,
    pub updated: i64,
    /// Folder id (None = not in a folder)
    pub folder: Option<String>,
    pub tags: Vec<String>,
    /// How many times the item was opened, and when last
    pub views: u64,
    pub last_viewed: i64,
}

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub data: ItemData,
}

/// Settings that live *inside* the encryption (tamper-proof).
#[derive(Clone, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct SecureSettings {
    pub mfa_secret: Option<String>,
    pub mfa_last_step: u64,
    pub auto_lock_minutes: u32,
    pub clipboard_clear_secs: u32,
    /// Drop zone: turn dropped .md/.txt/.json/.csv/code files into secure notes
    pub import_text_as_notes: bool,
    /// Drop zone: shred the original files after they were encrypted ("move")
    pub move_originals: bool,
}

impl Default for SecureSettings {
    fn default() -> Self {
        Self {
            mfa_secret: None,
            mfa_last_step: 0,
            auto_lock_minutes: 5,
            clipboard_clear_secs: 30,
            import_text_as_notes: true,
            move_originals: false,
        }
    }
}

/// Stored DPAPI-protected so the hint can be mailed while the vault is locked.
#[derive(Clone, Serialize, Deserialize, Zeroize)]
pub struct Recovery {
    pub email: String,
    pub app_password: String,
    pub verified: bool,
}

/// Metadata of an encrypted file kept in the vault (content is stored separately).
#[derive(Clone, Debug, Default, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
    pub created: i64,
    pub folder: Option<String>,
    pub tags: Vec<String>,
    pub views: u64,
    pub last_viewed: i64,
}

/// A folder in the user's hierarchy (stored encrypted as one blob).
#[derive(Clone, Debug, Default, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
}

#[derive(Clone)]
pub struct VaultFile {
    pub id: String,
    pub meta: FileMeta,
}

#[derive(Serialize, Deserialize)]
pub struct ExportFile {
    pub name: String,
    pub created: i64,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// base64 content
    pub data: String,
}

#[derive(Serialize, Deserialize)]
pub struct PlainExport {
    pub format: String,
    pub version: u32,
    pub encrypted: bool,
    pub exported_at: String,
    pub items: Vec<ItemData>,
    #[serde(default)]
    pub files: Vec<ExportFile>,
    #[serde(default)]
    pub folders: Vec<Folder>,
}

/// Largest single file accepted into the vault.
pub const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;

pub struct UnlockMaterial {
    pub params: KdfParams,
    pub salt: Vec<u8>,
    pub wrapped: Vec<u8>,
}

pub struct Vault {
    db: Db,
    #[allow(dead_code)]
    pub path: PathBuf,
    key: Option<Key>,
    pub items: Vec<Item>,
    pub files: Vec<VaultFile>,
    pub folders: Vec<Folder>,
    pub settings: SecureSettings,
}

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn file_meta_aad(id: &str) -> Vec<u8> {
    format!("softvault/v1/file-meta/{id}").into_bytes()
}

fn file_data_aad(id: &str) -> Vec<u8> {
    format!("softvault/v1/file-data/{id}").into_bytes()
}

fn item_aad(id: &str) -> Vec<u8> {
    format!("softvault/v1/item/{id}").into_bytes()
}

fn new_id() -> String {
    crate::crypto::random_bytes::<16>().iter().map(|b| format!("{b:02x}")).collect()
}

/// Where the database lives and how it got there.
#[derive(Clone, Debug)]
pub struct DbLocation {
    pub path: PathBuf,
    /// true = stored next to SoftVault.exe (portable; back up the whole folder)
    pub portable: bool,
}

fn legacy_db_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("SoftVault").join("vault.db")
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".softvault-write-test");
    let ok = std::fs::write(&probe, b"ok").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Portable-first: `vault.db` sits in the same folder as `SoftVault.exe`, so
/// backing up that folder backs up everything. Falls back to %APPDATA% only if
/// the exe folder is read-only (e.g. installed under Program Files).
pub fn resolve_db_location() -> DbLocation {
    let legacy = legacy_db_path();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let Some(dir) = exe_dir else {
        return DbLocation { path: legacy, portable: false };
    };
    let portable = dir.join("vault.db");
    if portable.exists() || dir_is_writable(&dir) {
        return DbLocation { path: portable, portable: true };
    }
    DbLocation { path: legacy, portable: false }
}

impl Vault {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            db: Db::open(path)?,
            path: path.to_path_buf(),
            key: None,
            items: Vec::new(),
            files: Vec::new(),
            folders: Vec::new(),
            settings: SecureSettings::default(),
        })
    }

    pub fn is_initialized(&self) -> bool {
        matches!(self.db.get_meta("wrapped_vk"), Ok(Some(_)))
    }

    #[allow(dead_code)]
    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    pub fn unlock_material(&self) -> Result<UnlockMaterial> {
        let params: KdfParams =
            serde_json::from_slice(&self.db.get_meta("kdf")?.context("missing kdf params")?)?;
        Ok(UnlockMaterial {
            params,
            salt: self.db.get_meta("salt")?.context("missing salt")?,
            wrapped: self.db.get_meta("wrapped_vk")?.context("missing vault key")?,
        })
    }

    /// Persist a new key-wrapping (first creation or master password change).
    pub fn store_material(&mut self, m: &NewVaultMaterial) -> Result<()> {
        let kdf = serde_json::to_vec(&m.params)?;
        self.db.set_meta_many(&[
            ("version", b"1"),
            ("kdf", &kdf),
            ("salt", &m.salt),
            ("wrapped_vk", &m.wrapped),
        ])
    }

    pub fn finish_create(&mut self, m: NewVaultMaterial) -> Result<()> {
        self.store_material(&m)?;
        self.key = Some(m.key);
        self.settings = SecureSettings::default();
        self.save_settings()?;
        self.items.clear();
        Ok(())
    }

    pub fn finish_unlock(&mut self, key: Key) -> Result<()> {
        let settings = match self.db.get_meta("settings")? {
            Some(blob) => serde_json::from_slice(&crypto::open(&key, &blob, AAD_SETTINGS)?)?,
            None => SecureSettings::default(),
        };
        let mut items = Vec::new();
        for (id, blob) in self.db.load_items()? {
            let pt = crypto::open(&key, &blob, &item_aad(&id))
                .map_err(|_| anyhow!("item {id} failed authentication (tampered?)"))?;
            let data: ItemData = serde_json::from_slice(&pt)?;
            items.push(Item { id, data });
        }
        let mut files = Vec::new();
        for (id, blob) in self.db.load_file_metas()? {
            let pt = crypto::open(&key, &blob, &file_meta_aad(&id)).map_err(|_| anyhow!("file {id} failed authentication (tampered?)"))?;
            let meta: FileMeta = serde_json::from_slice(&pt)?;
            files.push(VaultFile { id, meta });
        }
        let folders: Vec<Folder> = match self.db.get_meta("folders")? {
            Some(blob) => serde_json::from_slice(&crypto::open(&key, &blob, AAD_FOLDERS)?)?,
            None => Vec::new(),
        };
        self.settings = settings;
        self.items = items;
        self.files = files;
        self.folders = folders;
        self.key = Some(key);
        Ok(())
    }

    pub fn lock(&mut self) {
        for it in self.items.iter_mut() {
            it.data.zeroize();
        }
        self.items.clear();
        for f in self.files.iter_mut() {
            f.meta.zeroize();
        }
        self.files.clear();
        for f in self.folders.iter_mut() {
            f.zeroize();
        }
        self.folders.clear();
        self.settings.zeroize();
        self.settings = SecureSettings::default();
        self.key = None; // Zeroizing wipes the key on drop
    }

    fn key(&self) -> Result<&Key> {
        self.key.as_ref().ok_or_else(|| anyhow!("vault is locked"))
    }

    pub fn save_settings(&mut self) -> Result<()> {
        let pt = Zeroizing::new(serde_json::to_vec(&self.settings)?);
        let blob = crypto::seal(self.key()?, &pt, AAD_SETTINGS)?;
        self.db.set_meta("settings", &blob)
    }

    pub fn save_item(&mut self, id: Option<String>, data: ItemData) -> Result<String> {
        self.store_item(id, data, true)
    }

    /// `touch` = bump the "updated" timestamp (false for view counts / moving).
    pub fn store_item(&mut self, id: Option<String>, mut data: ItemData, touch: bool) -> Result<String> {
        let id = id.unwrap_or_else(new_id);
        let now = now_ts();
        if data.created == 0 {
            data.created = now;
        }
        if touch || data.updated == 0 {
            data.updated = now;
        }
        data.tags = normalize_tags(&data.tags);
        let pt = Zeroizing::new(serde_json::to_vec(&data)?);
        let blob = crypto::seal(self.key()?, &pt, &item_aad(&id))?;
        self.db.upsert_item(&id, &blob)?;
        match self.items.iter_mut().find(|i| i.id == id) {
            Some(it) => it.data = data,
            None => self.items.push(Item { id: id.clone(), data }),
        }
        Ok(id)
    }

    /// Count an opening of an item (doesn't change "updated").
    pub fn record_item_view(&mut self, id: &str) -> Result<()> {
        let Some(mut d) = self.items.iter().find(|i| i.id == id).map(|i| i.data.clone()) else { return Ok(()) };
        d.views += 1;
        d.last_viewed = now_ts();
        self.store_item(Some(id.to_string()), d, false).map(|_| ())
    }

    /// Move an item into a folder (None = top level).
    pub fn move_item(&mut self, id: &str, folder: Option<String>) -> Result<()> {
        let Some(mut d) = self.items.iter().find(|i| i.id == id).map(|i| i.data.clone()) else { return Ok(()) };
        d.folder = folder;
        self.store_item(Some(id.to_string()), d, false).map(|_| ())
    }

    fn write_file_meta(&mut self, id: &str, meta: FileMeta) -> Result<()> {
        let blob = crypto::seal(self.key()?, &Zeroizing::new(serde_json::to_vec(&meta)?), &file_meta_aad(id))?;
        self.db.update_file_meta(id, &blob)?;
        if let Some(f) = self.files.iter_mut().find(|f| f.id == id) {
            f.meta = meta;
        }
        Ok(())
    }

    pub fn update_file(&mut self, id: &str, edit: impl FnOnce(&mut FileMeta)) -> Result<()> {
        let Some(mut meta) = self.files.iter().find(|f| f.id == id).map(|f| f.meta.clone()) else { return Ok(()) };
        edit(&mut meta);
        meta.tags = normalize_tags(&meta.tags);
        self.write_file_meta(id, meta)
    }

    pub fn record_file_view(&mut self, id: &str) -> Result<()> {
        self.update_file(id, |m| {
            m.views += 1;
            m.last_viewed = now_ts();
        })
    }

    // ───── folders ─────

    fn save_folders(&mut self) -> Result<()> {
        let pt = Zeroizing::new(serde_json::to_vec(&self.folders)?);
        let blob = crypto::seal(self.key()?, &pt, AAD_FOLDERS)?;
        self.db.set_meta("folders", &blob)
    }

    pub fn create_folder(&mut self, name: &str, parent: Option<String>) -> Result<String> {
        let id = new_id();
        self.folders.push(Folder { id: id.clone(), name: name.trim().to_string(), parent });
        self.save_folders()?;
        Ok(id)
    }

    pub fn rename_folder(&mut self, id: &str, name: &str) -> Result<()> {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.name = name.trim().to_string();
        }
        self.save_folders()
    }

    /// Re-parent a folder (refuses to create cycles).
    pub fn move_folder(&mut self, id: &str, parent: Option<String>) -> Result<()> {
        if let Some(p) = &parent
            && (p == id || self.folder_descendants(id).contains(p))
        {
            return Err(anyhow!("a folder can't be moved into itself"));
        }
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.parent = parent;
        }
        self.save_folders()
    }

    /// Delete a folder; its items, files and sub-folders move up to its parent.
    pub fn delete_folder(&mut self, id: &str) -> Result<()> {
        let parent = self.folders.iter().find(|f| f.id == id).and_then(|f| f.parent.clone());
        let item_ids: Vec<String> = self.items.iter().filter(|i| i.data.folder.as_deref() == Some(id)).map(|i| i.id.clone()).collect();
        for iid in item_ids {
            self.move_item(&iid, parent.clone())?;
        }
        let file_ids: Vec<String> = self.files.iter().filter(|f| f.meta.folder.as_deref() == Some(id)).map(|f| f.id.clone()).collect();
        for fid in file_ids {
            let p = parent.clone();
            self.update_file(&fid, |m| m.folder = p)?;
        }
        for f in self.folders.iter_mut().filter(|f| f.parent.as_deref() == Some(id)) {
            f.parent = parent.clone();
        }
        self.folders.retain(|f| f.id != id);
        self.save_folders()
    }

    /// All folder ids below `id` (not including `id`).
    pub fn folder_descendants(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![id.to_string()];
        while let Some(cur) = stack.pop() {
            for f in self.folders.iter().filter(|f| f.parent.as_deref() == Some(cur.as_str())) {
                out.push(f.id.clone());
                stack.push(f.id.clone());
            }
        }
        out
    }

    /// "Parent / Child / Grandchild"
    pub fn folder_path(&self, id: &str) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            guard += 1;
            if guard > 64 {
                break;
            }
            match self.folders.iter().find(|f| f.id == c) {
                Some(f) => {
                    parts.push(f.name.clone());
                    cur = f.parent.clone();
                }
                None => break,
            }
        }
        parts.reverse();
        parts.join(" / ")
    }

    /// All tags in use with counts, sorted by name.
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        let mut map: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for t in self.items.iter().flat_map(|i| i.data.tags.iter()).chain(self.files.iter().flat_map(|f| f.meta.tags.iter())) {
            *map.entry(t.clone()).or_default() += 1;
        }
        map.into_iter().collect()
    }

    /// Encrypt `bytes` and store them as a vault file.
    pub fn add_file(&mut self, name: &str, bytes: &[u8], created: Option<i64>) -> Result<String> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(anyhow!("{name} is larger than {} MB", MAX_FILE_BYTES / 1024 / 1024));
        }
        let id = new_id();
        let meta = FileMeta { name: name.to_string(), size: bytes.len() as u64, created: created.unwrap_or_else(now_ts), ..Default::default() };
        let key = self.key()?;
        let meta_blob = crypto::seal(key, &Zeroizing::new(serde_json::to_vec(&meta)?), &file_meta_aad(&id))?;
        let data_blob = crypto::seal(key, bytes, &file_data_aad(&id))?;
        self.db.insert_file(&id, &meta_blob, &data_blob)?;
        self.files.push(VaultFile { id: id.clone(), meta });
        Ok(id)
    }

    /// Decrypt a vault file's content.
    pub fn read_file(&self, id: &str) -> Result<Zeroizing<Vec<u8>>> {
        let blob = self.db.load_file_data(id)?;
        crypto::open(self.key()?, &blob, &file_data_aad(id)).map_err(|_| anyhow!("file failed authentication (tampered?)"))
    }

    pub fn delete_file(&mut self, id: &str) -> Result<()> {
        self.key()?;
        self.db.delete_file(id)?;
        if let Some(pos) = self.files.iter().position(|f| f.id == id) {
            let mut f = self.files.remove(pos);
            f.meta.zeroize();
        }
        Ok(())
    }

    pub fn delete_item(&mut self, id: &str) -> Result<()> {
        self.key()?;
        self.db.delete_item(id)?;
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            let mut it = self.items.remove(pos);
            it.data.zeroize();
        }
        Ok(())
    }

    // ───── plain + DPAPI-protected metadata (available while locked) ─────

    pub fn get_plain(&self, key: &str) -> Option<String> {
        self.db.get_meta(key).ok().flatten().and_then(|v| String::from_utf8(v).ok())
    }

    pub fn set_plain(&self, key: &str, value: &str) -> Result<()> {
        self.db.set_meta(key, value.as_bytes())
    }

    pub fn get_protected(&self, key: &str) -> Option<Zeroizing<Vec<u8>>> {
        let blob = self.db.get_meta(key).ok().flatten()?;
        protect::unprotect(&blob).ok().map(Zeroizing::new)
    }

    pub fn set_protected(&self, key: &str, value: &[u8]) -> Result<()> {
        self.db.set_meta(key, &protect::protect(value)?)
    }

    /// (database size, reclaimable bytes)
    pub fn db_stats(&self) -> Result<(u64, u64)> {
        self.db.stats()
    }

    pub fn compact(&self) -> Result<()> {
        self.db.compact()
    }

    /// Consistent snapshot of the whole (encrypted) database into `dest`.
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        self.db.backup_to(dest)
    }

    pub fn delete_meta(&self, key: &str) -> Result<()> {
        self.db.del_meta(key)
    }

    pub fn hint(&self) -> Option<Zeroizing<String>> {
        let v = self.get_protected("hint")?;
        let s = String::from_utf8(v.to_vec()).ok()?;
        if s.trim().is_empty() { None } else { Some(Zeroizing::new(s)) }
    }

    pub fn set_hint(&self, hint: &str) -> Result<()> {
        if hint.trim().is_empty() {
            self.delete_meta("hint")
        } else {
            self.set_protected("hint", hint.as_bytes())
        }
    }

    pub fn recovery(&self) -> Option<Recovery> {
        let v = self.get_protected("recovery")?;
        serde_json::from_slice(&v).ok()
    }

    pub fn set_recovery(&self, r: &Recovery) -> Result<()> {
        let v = Zeroizing::new(serde_json::to_vec(r)?);
        self.set_protected("recovery", &v)
    }

    // ───── export ─────

    pub fn export_json(&self) -> Result<Zeroizing<Vec<u8>>> {
        self.key()?;
        let e = PlainExport {
            format: "softvault".into(),
            version: 1,
            encrypted: false,
            exported_at: chrono::Local::now().to_rfc3339(),
            items: self.items.iter().map(|i| i.data.clone()).collect(),
            files: self
                .files
                .iter()
                .map(|f| -> Result<ExportFile> {
                    use base64::{engine::general_purpose::STANDARD, Engine};
                    Ok(ExportFile {
                        name: f.meta.name.clone(),
                        created: f.meta.created,
                        folder: f.meta.folder.clone(),
                        tags: f.meta.tags.clone(),
                        data: STANDARD.encode(&*self.read_file(&f.id)?),
                    })
                })
                .collect::<Result<_>>()?,
            folders: self.folders.clone(),
        };
        Ok(Zeroizing::new(serde_json::to_vec_pretty(&e)?))
    }

    /// Import an export: returns (items, files) added.
    pub fn import_export(&mut self, e: PlainExport) -> Result<(usize, usize)> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let (n_items, n_files) = (e.items.len(), e.files.len());
        // recreate folders with fresh ids, keeping the hierarchy
        let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for f in &e.folders {
            map.insert(f.id.clone(), new_id());
        }
        for f in &e.folders {
            self.folders.push(Folder {
                id: map[&f.id].clone(),
                name: f.name.clone(),
                parent: f.parent.as_ref().and_then(|p| map.get(p).cloned()),
            });
        }
        if !e.folders.is_empty() {
            self.save_folders()?;
        }
        let remap = |f: &Option<String>| f.as_ref().and_then(|id| map.get(id).cloned());
        for mut d in e.items {
            d.created = if d.created == 0 { now_ts() } else { d.created };
            d.folder = remap(&d.folder);
            self.save_item(None, d)?;
        }
        for f in e.files {
            let bytes = Zeroizing::new(STANDARD.decode(f.data.as_bytes())?);
            let id = self.add_file(&f.name, &bytes, Some(f.created))?;
            let (folder, tags) = (remap(&f.folder), f.tags.clone());
            self.update_file(&id, |m| {
                m.folder = folder;
                m.tags = tags;
            })?;
        }
        Ok((n_items, n_files))
    }
}

/// Trim, lowercase, de-duplicate and sort tags.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut v: Vec<String> = tags
        .iter()
        .map(|t| t.trim().trim_start_matches('#').to_lowercase().replace(char::is_whitespace, "-"))
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(32).collect())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Parse a plaintext export (or the decrypted inner payload of an encrypted one).
pub fn parse_plain_export(bytes: &[u8]) -> Result<PlainExport> {
    let e: PlainExport = serde_json::from_slice(bytes).context("not a SoftVault export file")?;
    if e.format != "softvault" {
        return Err(anyhow!("not a SoftVault export file"));
    }
    Ok(e)
}

pub fn is_encrypted_export(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|v| v.get("encrypted").and_then(|e| e.as_bool()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_create_save_lock_unlock_export() {
        let dir = std::env::temp_dir().join(format!("sv-test-{}", now_ts()));
        let path = dir.join("vault.db");
        let mut v = Vault::open(&path).unwrap();
        assert!(!v.is_initialized());
        v.finish_create(crypto::build_new_vault(b"correct horse battery").unwrap()).unwrap();
        let id = v
            .save_item(None, ItemData { title: "GitHub".into(), password: "s3cr3t!".into(), ..Default::default() })
            .unwrap();
        v.save_item(None, ItemData { kind: Kind::Note, title: "Note".into(), notes: "hello".into(), ..Default::default() }).unwrap();

        // nothing readable on disk
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"GitHub"));
        assert!(!raw.windows(7).any(|w| w == b"s3cr3t!"));

        let plain = v.export_json().unwrap();
        let enc = crypto::encrypt_export(&plain, b"export-password-123").unwrap();
        assert!(is_encrypted_export(&enc));
        let back = parse_plain_export(&crypto::decrypt_export(&enc, b"export-password-123").unwrap()).unwrap();
        assert_eq!(back.items.len(), 2);
        assert!(crypto::decrypt_export(&enc, b"wrong").is_err());

        v.lock();
        assert!(v.items.is_empty());
        let m = v.unlock_material().unwrap();
        assert!(crypto::unwrap_vault_key(b"wrong password", &m.params, &m.salt, &m.wrapped).is_err());
        let k = crypto::unwrap_vault_key(b"correct horse battery", &m.params, &m.salt, &m.wrapped).unwrap();
        v.finish_unlock(k).unwrap();
        assert_eq!(v.items.len(), 2);
        assert_eq!(v.items.iter().find(|i| i.id == id).unwrap().data.password, "s3cr3t!");

        // swapping ciphertext between rows must be detected (AAD binds blob to its id)
        let other = v.items.iter().find(|i| i.id != id).unwrap().id.clone();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("UPDATE items SET blob = (SELECT blob FROM items WHERE id = ?1) WHERE id = ?2", [&id, &other]).unwrap();
        v.lock();
        let m = v.unlock_material().unwrap();
        let k = crypto::unwrap_vault_key(b"correct horse battery", &m.params, &m.salt, &m.wrapped).unwrap();
        assert!(v.finish_unlock(k).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_tags_views() {
        let dir = std::env::temp_dir().join(format!("sv-folders-{}-{}", now_ts(), std::process::id()));
        let mut v = Vault::open(&dir.join("vault.db")).unwrap();
        v.finish_create(crypto::build_new_vault(b"correct horse battery").unwrap()).unwrap();
        let work = v.create_folder("Work", None).unwrap();
        let client = v.create_folder("ACME", Some(work.clone())).unwrap();
        assert_eq!(v.folder_path(&client), "Work / ACME");
        assert!(v.move_folder(&work, Some(client.clone())).is_err(), "cycles must be refused");

        let id = v
            .save_item(None, ItemData { title: "API".into(), tags: vec![" Prod ".into(), "#prod".into(), "Client A".into()], ..Default::default() })
            .unwrap();
        let it = v.items.iter().find(|i| i.id == id).unwrap();
        assert_eq!(it.data.tags, vec!["client-a".to_string(), "prod".to_string()]);
        v.move_item(&id, Some(client.clone())).unwrap();
        let updated = v.items.iter().find(|i| i.id == id).unwrap().data.updated;
        v.record_item_view(&id).unwrap();
        v.record_item_view(&id).unwrap();
        let it = v.items.iter().find(|i| i.id == id).unwrap();
        assert_eq!(it.data.views, 2);
        assert_eq!(it.data.updated, updated, "views must not change 'updated'");

        // folders survive lock/unlock (encrypted blob)
        v.lock();
        let m = v.unlock_material().unwrap();
        let k = crypto::unwrap_vault_key(b"correct horse battery", &m.params, &m.salt, &m.wrapped).unwrap();
        v.finish_unlock(k).unwrap();
        assert_eq!(v.folders.len(), 2);

        // export → import keeps the hierarchy with fresh ids
        let e = parse_plain_export(&v.export_json().unwrap()).unwrap();
        v.import_export(e).unwrap();
        assert_eq!(v.folders.len(), 4);
        let copy = v.items.iter().find(|i| i.id != id).unwrap();
        let cf = copy.data.folder.clone().unwrap();
        assert_ne!(cf, client);
        assert_eq!(v.folder_path(&cf), "Work / ACME");

        // deleting a folder moves its content to the parent
        v.delete_folder(&client).unwrap();
        assert_eq!(v.items.iter().find(|i| i.id == id).unwrap().data.folder.as_deref(), Some(work.as_str()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_roundtrip_lock_and_tamper() {
        let dir = std::env::temp_dir().join(format!("sv-files-{}", now_ts()));
        let path = dir.join("vault.db");
        let mut v = Vault::open(&path).unwrap();
        v.finish_create(crypto::build_new_vault(b"correct horse battery").unwrap()).unwrap();
        let payload = b"%PDF-1.7 secret contract".to_vec();
        let id = v.add_file("contract.pdf", &payload, None).unwrap();
        assert_eq!(&*v.read_file(&id).unwrap(), &payload[..]);
        // name and content are not visible on disk
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(8).any(|w| w == b"contract"));
        // survives lock/unlock
        v.lock();
        assert!(v.files.is_empty());
        let m = v.unlock_material().unwrap();
        let k = crypto::unwrap_vault_key(b"correct horse battery", &m.params, &m.salt, &m.wrapped).unwrap();
        v.finish_unlock(k).unwrap();
        assert_eq!(v.files.len(), 1);
        assert_eq!(v.files[0].meta.name, "contract.pdf");
        // exports carry files
        let e = parse_plain_export(&v.export_json().unwrap()).unwrap();
        assert_eq!(e.files.len(), 1);
        // tampering with the ciphertext is detected
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("UPDATE files SET data = meta WHERE id = ?1", [&id]).unwrap();
        assert!(v.read_file(&id).is_err());
        v.delete_file(&id).unwrap();
        assert!(v.files.is_empty());

        // auto-vacuum: deleting a large file gives the space back
        let big = v.add_file("big.bin", &vec![1u8; 3_000_000], None).unwrap();
        let with = std::fs::metadata(&path).unwrap().len();
        v.delete_file(&big).unwrap();
        let without = std::fs::metadata(&path).unwrap().len();
        assert!(without + 2_500_000 < with, "db should shrink: {with} -> {without}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
