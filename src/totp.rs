//! RFC 6238 TOTP (SHA-1, 6 digits, 30 s) – compatible with Google Authenticator,
//! Microsoft Authenticator, Authy, 2FAS, Aegis, …

use hmac::{Hmac, Mac};
use sha1::Sha1;

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
pub const PERIOD: u64 = 30;

pub fn generate_secret() -> Vec<u8> {
    crate::crypto::random_bytes::<20>().to_vec()
}

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let v = ALPHABET.iter().position(|&a| a as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((buffer >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

pub fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[19] & 0x0f) as usize;
    let bin = ((h[off] as u32 & 0x7f) << 24)
        | ((h[off + 1] as u32) << 16)
        | ((h[off + 2] as u32) << 8)
        | (h[off + 3] as u32);
    bin % 1_000_000
}

pub fn now_step() -> u64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs / PERIOD
}

/// Verify a code (±1 step for clock drift). Returns the matched step, rejecting
/// re-use of a step that was already accepted (`last_step`).
pub fn verify(secret_b32: &str, code: &str, last_step: u64) -> Option<u64> {
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != 6 {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let secret = base32_decode(secret_b32)?;
    let step = now_step();
    [step.saturating_sub(1), step, step + 1].into_iter().find(|&s| s > last_step && hotp(&secret, s) == want)
}

pub fn otpauth_uri(secret_b32: &str, account: &str) -> String {
    format!(
        "otpauth://totp/SoftVault:{account}?secret={secret_b32}&issuer=SoftVault&algorithm=SHA1&digits=6&period=30"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4226_vectors() {
        let secret = b"12345678901234567890";
        assert_eq!(hotp(secret, 0), 755224);
        assert_eq!(hotp(secret, 1), 287082);
        assert_eq!(hotp(secret, 9), 520489);
    }

    #[test]
    fn base32_roundtrip() {
        let d = generate_secret();
        assert_eq!(base32_decode(&base32_encode(&d)).unwrap(), d);
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
    }
}
