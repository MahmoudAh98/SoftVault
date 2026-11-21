//! High level vault: ties crypto + SQLite together and keeps decrypted data in
//! memory only while unlocked (wiped with `zeroize` on lock).

use crate::crypto::{self, Key, KdfParams, NewVaultMaterial, AAD_SETTINGS};
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
}

impl Default for SecureSettings {
    fn default() -> Self {
        Self { mfa_secret: None, mfa_last_step: 0, auto_lock_minutes: 5, clipboard_clear_secs: 30 }
    }
}

/// Stored DPAPI-protected so the hint can be mailed while the vault is locked.
#[derive(Clone, Serialize, Deserialize, Zeroize)]
pub struct Recovery {
    pub email: String,
    pub app_password: String,
    pub verified: bool,
}

#[derive(Serialize, Deserialize)]
pub struct PlainExport {
    pub format: String,
    pub version: u32,
    pub encrypted: bool,
    pub exported_at: String,
    pub items: Vec<ItemData>,
}

pub struct UnlockMaterial {
    pub params: KdfParams,
    pub salt: Vec<u8>,
    pub wrapped: Vec<u8>,
}

pub struct Vault {
    db: Db,
    pub path: PathBuf,
    key: Option<Key>,
    pub items: Vec<Item>,
    pub settings: SecureSettings,
}

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

fn item_aad(id: &str) -> Vec<u8> {
    format!("softvault/v1/item/{id}").into_bytes()
}

fn new_id() -> String {
    crate::crypto::random_bytes::<16>().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn default_db_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("SoftVault").join("vault.db")
}

impl Vault {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            db: Db::open(path)?,
            path: path.to_path_buf(),
            key: None,
            items: Vec::new(),
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
        self.settings = settings;
        self.items = items;
        self.key = Some(key);
        Ok(())
    }

    pub fn lock(&mut self) {
        for it in self.items.iter_mut() {
            it.data.zeroize();
        }
        self.items.clear();
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

    pub fn save_item(&mut self, id: Option<String>, mut data: ItemData) -> Result<String> {
        let id = id.unwrap_or_else(new_id);
        let now = now_ts();
        if data.created == 0 {
            data.created = now;
        }
        data.updated = now;
        let pt = Zeroizing::new(serde_json::to_vec(&data)?);
        let blob = crypto::seal(self.key()?, &pt, &item_aad(&id))?;
        self.db.upsert_item(&id, &blob)?;
        match self.items.iter_mut().find(|i| i.id == id) {
            Some(it) => it.data = data,
            None => self.items.push(Item { id: id.clone(), data }),
        }
        Ok(id)
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
        };
        Ok(Zeroizing::new(serde_json::to_vec_pretty(&e)?))
    }

    pub fn import_items(&mut self, items: Vec<ItemData>) -> Result<usize> {
        let n = items.len();
        for mut d in items {
            d.created = if d.created == 0 { now_ts() } else { d.created };
            self.save_item(None, d)?;
        }
        Ok(n)
    }
}

/// Parse a plaintext export (or the decrypted inner payload of an encrypted one).
pub fn parse_plain_export(bytes: &[u8]) -> Result<Vec<ItemData>> {
    let e: PlainExport = serde_json::from_slice(bytes).context("not a SoftVault export file")?;
    if e.format != "softvault" {
        return Err(anyhow!("not a SoftVault export file"));
    }
    Ok(e.items)
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
        assert_eq!(back.len(), 2);
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
}
