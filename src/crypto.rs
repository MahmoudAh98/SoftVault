//! Cryptographic core.
//!
//! * Key derivation: **Argon2id** (RFC 9106), 256 MiB memory, 3 passes, 4 lanes.
//! * Encryption:     **XChaCha20-Poly1305** AEAD (256-bit key, 192-bit random nonce).
//!
//! Key hierarchy (envelope encryption):
//!
//! ```text
//! master password ──Argon2id(salt)──▶ KEK (key-encryption key, never stored)
//! random 256-bit Vault Key ──XChaCha20-Poly1305(KEK)──▶ wrapped_vk (stored)
//! every item / setting ──XChaCha20-Poly1305(Vault Key, AAD=context)──▶ blob (stored)
//! ```
//!
//! The master password and the KEK never touch the disk. The only "password
//! check" is whether the AEAD tag of `wrapped_vk` verifies.

use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    Key as AeadKey, XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;

pub const AAD_VAULT_KEY: &[u8] = b"softvault/v1/vault-key";
pub const AAD_SETTINGS: &[u8] = b"softvault/v1/settings";
pub const AAD_EXPORT: &[u8] = b"softvault/v1/export";

pub type Key = Zeroizing<[u8; KEY_LEN]>;

/// Argon2id cost parameters (stored next to the salt so they can be raised later).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        // 256 MiB, 3 iterations, 4 lanes – well above OWASP's Argon2id minimums.
        Self { m_cost_kib: 256 * 1024, t_cost: 3, p_cost: 4 }
    }
}

impl KdfParams {
    /// Refuse suspiciously weak parameters (defends against a tampered DB/export
    /// trying to make offline guessing cheap).
    pub fn validate(&self) -> Result<()> {
        if self.m_cost_kib < 64 * 1024 || self.t_cost < 2 || self.p_cost < 1 {
            bail!("KDF parameters are below the security minimum");
        }
        if self.m_cost_kib > 4 * 1024 * 1024 || self.t_cost > 64 || self.p_cost > 64 {
            bail!("KDF parameters are unreasonably large");
        }
        Ok(())
    }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

/// Derive a 256-bit key from a password with Argon2id.
pub fn derive_key(password: &[u8], salt: &[u8], p: &KdfParams) -> Result<Key> {
    p.validate()?;
    let params = Params::new(p.m_cost_kib, p.t_cost, p.p_cost, Some(KEY_LEN))
        .map_err(|e| anyhow!("argon2 params: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out: Key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password, salt, &mut *out)
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(out)
}

/// Encrypt → `nonce || ciphertext+tag`.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(AeadKey::from_slice(key));
    let nonce: [u8; NONCE_LEN] = random_bytes();
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| anyhow!("encryption failed"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt a blob produced by [`seal`]. Fails if anything was tampered with.
pub fn open(key: &[u8; KEY_LEN], blob: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < NONCE_LEN + 16 {
        bail!("ciphertext too short");
    }
    let (nonce, ct) = blob.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(AeadKey::from_slice(key));
    let pt = cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| anyhow!("decryption failed (wrong password or corrupted data)"))?;
    Ok(Zeroizing::new(pt))
}

/// Everything needed to persist a brand-new vault.
pub struct NewVaultMaterial {
    pub params: KdfParams,
    pub salt: [u8; SALT_LEN],
    pub wrapped: Vec<u8>,
    pub key: Key,
}

/// Generate a random vault key and wrap it with a key derived from `password`.
pub fn build_new_vault(password: &[u8]) -> Result<NewVaultMaterial> {
    let params = KdfParams::default();
    let salt: [u8; SALT_LEN] = random_bytes();
    let kek = derive_key(password, &salt, &params)?;
    let key: Key = Zeroizing::new(random_bytes());
    let wrapped = seal(&kek, &*key, AAD_VAULT_KEY)?;
    Ok(NewVaultMaterial { params, salt, wrapped, key })
}

/// Recover the vault key from the master password.
pub fn unwrap_vault_key(password: &[u8], params: &KdfParams, salt: &[u8], wrapped: &[u8]) -> Result<Key> {
    let kek = derive_key(password, salt, params)?;
    let pt = open(&kek, wrapped, AAD_VAULT_KEY).map_err(|_| anyhow!("Wrong master password"))?;
    if pt.len() != KEY_LEN {
        bail!("corrupted vault key");
    }
    let mut key: Key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&pt);
    Ok(key)
}

/// Re-wrap an existing vault key under a new password (fresh salt).
pub fn rewrap(key: &Key, new_password: &[u8]) -> Result<NewVaultMaterial> {
    let params = KdfParams::default();
    let salt: [u8; SALT_LEN] = random_bytes();
    let kek = derive_key(new_password, &salt, &params)?;
    let wrapped = seal(&kek, &**key, AAD_VAULT_KEY)?;
    Ok(NewVaultMaterial { params, salt, wrapped, key: key.clone() })
}

// ───────────────────────── encrypted export container ─────────────────────────

use base64::{engine::general_purpose::STANDARD as B64, Engine};

#[derive(Serialize, Deserialize)]
pub struct EncryptedExport {
    pub format: String,
    pub version: u32,
    pub encrypted: bool,
    pub cipher: String,
    pub kdf: KdfParams,
    pub salt: String,
    pub data: String,
}

pub fn encrypt_export(plaintext: &[u8], password: &[u8]) -> Result<Vec<u8>> {
    let params = KdfParams::default();
    let salt: [u8; SALT_LEN] = random_bytes();
    let key = derive_key(password, &salt, &params)?;
    let blob = seal(&key, plaintext, AAD_EXPORT)?;
    let c = EncryptedExport {
        format: "softvault".into(),
        version: 1,
        encrypted: true,
        cipher: "argon2id+xchacha20poly1305".into(),
        kdf: params,
        salt: B64.encode(salt),
        data: B64.encode(blob),
    };
    Ok(serde_json::to_vec_pretty(&c)?)
}

pub fn decrypt_export(file: &[u8], password: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let c: EncryptedExport = serde_json::from_slice(file)?;
    if c.format != "softvault" || !c.encrypted {
        bail!("not an encrypted SoftVault export");
    }
    let salt = B64.decode(c.salt)?;
    let blob = B64.decode(c.data)?;
    let key = derive_key(password, &salt, &c.kdf)?;
    open(&key, &blob, AAD_EXPORT).map_err(|_| anyhow!("Wrong export password or corrupted file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> KdfParams {
        KdfParams { m_cost_kib: 64 * 1024, t_cost: 2, p_cost: 1 }
    }

    #[test]
    fn seal_open_roundtrip_and_tamper() {
        let k: [u8; 32] = random_bytes();
        let mut b = seal(&k, b"hello", b"ctx").unwrap();
        assert_eq!(&*open(&k, &b, b"ctx").unwrap(), b"hello");
        assert!(open(&k, &b, b"other").is_err());
        let last = b.len() - 1;
        b[last] ^= 1;
        assert!(open(&k, &b, b"ctx").is_err());
    }

    #[test]
    fn wrap_unwrap() {
        let p = fast();
        let salt: [u8; 32] = random_bytes();
        let kek = derive_key(b"pw", &salt, &p).unwrap();
        let vk: [u8; 32] = random_bytes();
        let w = seal(&kek, &vk, AAD_VAULT_KEY).unwrap();
        assert_eq!(*unwrap_vault_key(b"pw", &p, &salt, &w).unwrap(), vk);
        assert!(unwrap_vault_key(b"bad", &p, &salt, &w).is_err());
    }

    #[test]
    fn weak_params_rejected() {
        assert!(KdfParams { m_cost_kib: 1024, t_cost: 1, p_cost: 1 }.validate().is_err());
    }
}
