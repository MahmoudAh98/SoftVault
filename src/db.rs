//! SQLite storage. The database only ever holds ciphertext for vault content:
//! item IDs are random, and titles, usernames, URLs, timestamps and the type of
//! each entry are all inside the encrypted blob.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA secure_delete = ON;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS items (
                 id   TEXT PRIMARY KEY,
                 blob BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS files (
                 id   TEXT PRIMARY KEY,
                 meta BLOB NOT NULL,
                 data BLOB NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn del_meta(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM meta WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn load_items(&self) -> Result<Vec<(String, Vec<u8>)>> {
        let mut st = self.conn.prepare("SELECT id, blob FROM items")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn upsert_item(&self, id: &str, blob: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO items(id, blob) VALUES(?1, ?2)
             ON CONFLICT(id) DO UPDATE SET blob = excluded.blob",
            params![id, blob],
        )?;
        Ok(())
    }

    pub fn delete_item(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM items WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn load_file_metas(&self) -> Result<Vec<(String, Vec<u8>)>> {
        let mut st = self.conn.prepare("SELECT id, meta FROM files")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn load_file_data(&self, id: &str) -> Result<Vec<u8>> {
        Ok(self.conn.query_row("SELECT data FROM files WHERE id = ?1", params![id], |r| r.get(0))?)
    }

    pub fn insert_file(&self, id: &str, meta: &[u8], data: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO files(id, meta, data) VALUES(?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET meta = excluded.meta, data = excluded.data",
            params![id, meta, data],
        )?;
        Ok(())
    }

    pub fn delete_file(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// `VACUUM INTO` produces a consistent, compact copy even while the app is running.
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        self.conn.execute("VACUUM INTO ?1", params![dest.to_string_lossy()])?;
        Ok(())
    }

    /// Write several meta values atomically.
    pub fn set_meta_many(&mut self, kv: &[(&str, &[u8])]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (k, v) in kv {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
