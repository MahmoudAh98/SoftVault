//! Password generator (CSPRNG) and a simple entropy-based strength estimate.

use rand::{rngs::OsRng, seq::SliceRandom, Rng};

#[derive(Clone)]
pub struct GenOptions {
    pub length: usize,
    pub upper: bool,
    pub lower: bool,
    pub digits: bool,
    pub symbols: bool,
    pub avoid_ambiguous: bool,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self { length: 24, upper: true, lower: true, digits: true, symbols: true, avoid_ambiguous: true }
    }
}

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:,.<>?/~";
const AMBIGUOUS: &str = "O0Il1|`'\"";

pub fn generate(o: &GenOptions) -> String {
    let mut sets: Vec<Vec<char>> = Vec::new();
    let filt = |s: &str| -> Vec<char> {
        s.chars().filter(|c| !(o.avoid_ambiguous && AMBIGUOUS.contains(*c))).collect()
    };
    if o.upper { sets.push(filt(UPPER)); }
    if o.lower { sets.push(filt(LOWER)); }
    if o.digits { sets.push(filt(DIGITS)); }
    if o.symbols { sets.push(filt(SYMBOLS)); }
    if sets.is_empty() {
        sets.push(filt(LOWER));
    }
    let all: Vec<char> = sets.iter().flatten().copied().collect();
    let len = o.length.clamp(8, 128).max(sets.len());
    let mut rng = OsRng;
    // one from every selected class, rest uniformly from the union
    let mut out: Vec<char> = sets.iter().map(|s| s[rng.gen_range(0..s.len())]).collect();
    while out.len() < len {
        out.push(all[rng.gen_range(0..all.len())]);
    }
    out.shuffle(&mut rng);
    out.into_iter().collect()
}

/// Returns (score 0..=4, label, estimated entropy bits).
pub fn strength(pw: &str) -> (u8, &'static str, f64) {
    if pw.is_empty() {
        return (0, "Empty", 0.0);
    }
    let mut pool = 0u32;
    if pw.chars().any(|c| c.is_ascii_lowercase()) { pool += 26; }
    if pw.chars().any(|c| c.is_ascii_uppercase()) { pool += 26; }
    if pw.chars().any(|c| c.is_ascii_digit()) { pool += 10; }
    if pw.chars().any(|c| c.is_ascii_punctuation() || c == ' ') { pool += 33; }
    if !pw.is_ascii() { pool += 100; }
    // penalise repeated characters a bit
    let mut uniq: Vec<char> = pw.chars().collect();
    uniq.sort();
    uniq.dedup();
    let eff_len = (uniq.len() as f64 * 0.6 + pw.chars().count() as f64 * 0.4).max(1.0);
    let bits = eff_len * (pool.max(1) as f64).log2();
    let (score, label) = match bits as u32 {
        0..=35 => (0, "Very weak"),
        36..=59 => (1, "Weak"),
        60..=79 => (2, "Fair"),
        80..=99 => (3, "Strong"),
        _ => (4, "Excellent"),
    };
    (score, label, bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gen_len_and_classes() {
        let o = GenOptions { length: 32, ..Default::default() };
        let p = generate(&o);
        assert_eq!(p.chars().count(), 32);
        assert!(p.chars().any(|c| c.is_ascii_uppercase()));
        assert!(p.chars().any(|c| c.is_ascii_digit()));
    }
}
