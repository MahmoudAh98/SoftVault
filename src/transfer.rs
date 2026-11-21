//! Drag & drop in/out of the vault.
//!
//! * **In:** files dropped on the window are encrypted into the vault. Text-like
//!   files become secure notes (format detected from the extension); everything
//!   else is stored as an encrypted file.
//! * **Out:** dragging a note or file out of the window writes a decrypted copy
//!   to a private temp folder and hands it to Windows' native drag & drop. The
//!   temp copy is overwritten with zeros and deleted shortly afterwards.

use crate::render::NoteFormat;
use crate::vault::{ItemData, Kind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a decrypted drag-out copy may live before it is shredded.
pub const TEMP_TTL: Duration = Duration::from_secs(120);

pub enum Classified {
    Note { format: NoteFormat, lang: String },
    File,
}

/// Decide whether a dropped file becomes a secure note or an encrypted file.
pub fn classify(path: &Path) -> Classified {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let code = |l: &str| Classified::Note { format: NoteFormat::Code, lang: l.into() };
    match ext.as_str() {
        "md" | "markdown" | "mdown" => Classified::Note { format: NoteFormat::Markdown, lang: String::new() },
        "txt" | "text" | "log" | "ini" | "cfg" | "conf" | "env" => Classified::Note { format: NoteFormat::Plain, lang: String::new() },
        "json" => Classified::Note { format: NoteFormat::Json, lang: String::new() },
        "csv" | "tsv" => Classified::Note { format: NoteFormat::Csv, lang: String::new() },
        "rs" => code("rust"),
        "py" => code("python"),
        "js" | "mjs" | "cjs" => code("js"),
        "ts" | "tsx" => code("ts"),
        "sh" | "bash" | "zsh" => code("sh"),
        "ps1" => code("ps1"),
        "bat" | "cmd" => code("bat"),
        "sql" => code("sql"),
        "yaml" | "yml" => code("yaml"),
        "toml" => code("toml"),
        "xml" => code("xml"),
        "html" | "htm" => code("html"),
        "css" => code("css"),
        "go" => code("go"),
        "c" | "h" => code("c"),
        "cpp" | "cc" | "hpp" => code("cpp"),
        "cs" => code("cs"),
        "java" => code("java"),
        "php" => code("php"),
        "rb" => code("ruby"),
        "pem" | "key" | "pub" | "crt" => code("txt"),
        _ => Classified::File,
    }
}

/// Build a secure note from a dropped text file (None if it isn't valid UTF-8).
pub fn note_from_file(path: &Path, bytes: &[u8], format: NoteFormat, lang: String) -> Option<ItemData> {
    let text = std::str::from_utf8(bytes).ok()?;
    let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported note").to_string();
    Some(ItemData { kind: Kind::Note, title, notes: text.to_string(), format, lang, ..Default::default() })
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() { "untitled".into() } else { trimmed.chars().take(120).collect() }
}

fn lang_ext(lang: &str) -> &str {
    match lang.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "js" | "javascript" => "js",
        "ts" | "typescript" => "ts",
        "sh" | "bash" | "shell" => "sh",
        "sql" => "sql",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "json" => "json",
        "html" => "html",
        "css" => "css",
        "go" => "go",
        "c" => "c",
        "cpp" | "c++" => "cpp",
        "cs" | "c#" => "cs",
        "java" => "java",
        "ps1" | "powershell" => "ps1",
        "bat" => "bat",
        "xml" => "xml",
        _ => "txt",
    }
}

/// File name used when a note is dragged/saved out of the vault.
pub fn note_file_name(d: &ItemData) -> String {
    let ext = match d.format {
        NoteFormat::Markdown => "md",
        NoteFormat::Json => "json",
        NoteFormat::Csv => "csv",
        NoteFormat::Code => lang_ext(&d.lang),
        NoteFormat::Plain => "txt",
    };
    format!("{}.{ext}", sanitize(&d.title))
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

/// Overwrite a file with zeros, then delete it (best effort; SSDs and some
/// file systems may still keep old copies of the data).
pub fn shred(path: &Path) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    if let Ok(meta) = std::fs::metadata(path)
        && let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path)
    {
        {
            let zeros = vec![0u8; 64 * 1024];
            let mut left = meta.len();
            let _ = f.seek(SeekFrom::Start(0));
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                if f.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = f.sync_all();
        }
    }
    std::fs::remove_file(path)
}

/// Private folder for short-lived decrypted drag-out copies.
pub struct TempArea {
    root: PathBuf,
    pending: Vec<(PathBuf, Instant)>,
}

impl TempArea {
    pub fn new() -> Self {
        let base = std::env::temp_dir();
        // shred leftovers from a previous session that didn't exit cleanly
        if let Ok(rd) = std::fs::read_dir(&base) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("SoftVault-out-") {
                    shred_dir(&e.path());
                }
            }
        }
        let tag: String = crate::crypto::random_bytes::<8>().iter().map(|b| format!("{b:02x}")).collect();
        let root = base.join(format!("SoftVault-out-{tag}"));
        let _ = std::fs::create_dir_all(&root);
        Self { root, pending: Vec::new() }
    }

    /// Write a decrypted copy named `name` into its own sub-folder and return its path.
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let tag: String = crate::crypto::random_bytes::<6>().iter().map(|b| format!("{b:02x}")).collect();
        let dir = self.root.join(tag);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(sanitize(name));
        std::fs::write(&path, bytes)?;
        self.pending.push((path.clone(), Instant::now() + TEMP_TTL));
        Ok(path)
    }

    /// True if `p` is one of our own decrypted copies (so dropping it back onto
    /// the window doesn't re-import it). Compares canonical, case-folded paths
    /// because Windows may report short (8.3) or `\\?\` prefixed forms.
    pub fn contains(&self, p: &Path) -> bool {
        fn norm(p: &Path) -> String {
            let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            let s = c.to_string_lossy().replace('\\', "/");
            let s = s.strip_prefix("//?/").unwrap_or(&s).to_string();
            if cfg!(windows) { s.to_lowercase() } else { s }
        }
        p.starts_with(&self.root) || norm(p).starts_with(&norm(&self.root))
    }

    /// Shred copies whose time is up.
    pub fn sweep(&mut self) {
        let now = Instant::now();
        self.pending.retain(|(p, until)| {
            if now >= *until {
                let _ = shred(p);
                if let Some(dir) = p.parent() {
                    let _ = std::fs::remove_dir(dir);
                }
                false
            } else {
                true
            }
        });
    }

    /// Shred everything (on lock / exit).
    pub fn clear(&mut self) {
        for (p, _) in self.pending.drain(..) {
            let _ = shred(&p);
        }
        shred_dir(&self.root);
    }
}

impl Drop for TempArea {
    fn drop(&mut self) {
        self.clear();
    }
}

fn shred_dir(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                shred_dir(&p);
            } else {
                let _ = shred(&p);
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Collect files from dropped paths (folders are walked, max `limit` files).
pub fn collect_paths(paths: &[PathBuf], limit: usize) -> Vec<PathBuf> {
    fn walk(p: &Path, out: &mut Vec<PathBuf>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                for e in rd.flatten() {
                    walk(&e.path(), out, limit);
                }
            }
        } else if p.is_file() {
            out.push(p.to_path_buf());
        }
    }
    let mut out = Vec::new();
    for p in paths {
        walk(p, &mut out, limit);
    }
    out
}

// ───────────────────────────── native drag-out ─────────────────────────────

pub const DRAG_OUT_SUPPORTED: bool = cfg!(windows);

/// Start an OS drag of `paths` from the app window. Returns Ok(true) if they
/// were dropped somewhere, Ok(false) if the drag was cancelled.
#[cfg(windows)]
pub fn start_drag_out(frame: &eframe::Frame, paths: Vec<PathBuf>) -> Result<bool, String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const DRAG_IMAGE: &[u8] = include_bytes!("../assets/drag-file.png");
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let result = drag::start_drag(
        frame,
        drag::DragItem::Files(paths),
        drag::Image::Raw(DRAG_IMAGE.to_vec()),
        move |r: drag::DragResult, _pos: drag::CursorPosition| {
            flag.store(matches!(r, drag::DragResult::Dropped), Ordering::SeqCst);
        },
        drag::Options::default(),
    );

    // Windows' drag loop swallows the mouse-up, so egui would think the button
    // is still held. Post a synthetic release to our own window.
    if let Ok(handle) = frame.window_handle()
        && let RawWindowHandle::Win32(w) = handle.as_raw()
    {
        use windows_sys::Win32::Foundation::{HWND, POINT};
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW, WM_LBUTTONUP};
        // SAFETY: plain Win32 calls on our own, still-alive window handle.
        unsafe {
            let hwnd = w.hwnd.get() as HWND;
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            ScreenToClient(hwnd, &mut pt);
            let lparam = (((pt.y as u16) as isize) << 16) | ((pt.x as u16) as isize);
            PostMessageW(hwnd, WM_LBUTTONUP, 0, lparam);
        }
    }

    result.map_err(|e| e.to_string())?;
    Ok(dropped.load(Ordering::SeqCst))
}

#[cfg(not(windows))]
pub fn start_drag_out(_frame: &eframe::Frame, _paths: Vec<PathBuf>) -> Result<bool, String> {
    Err("Dragging out of the app is available on Windows. Use “Save decrypted…” instead.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classify_and_names() {
        assert!(matches!(classify(Path::new("a/notes.MD")), Classified::Note { format: NoteFormat::Markdown, .. }));
        assert!(matches!(classify(Path::new("x.csv")), Classified::Note { format: NoteFormat::Csv, .. }));
        assert!(matches!(classify(Path::new("photo.jpg")), Classified::File));
        let d = ItemData { title: "My: plan?".into(), format: NoteFormat::Markdown, ..Default::default() };
        assert_eq!(note_file_name(&d), "My_ plan_.md");
        assert_eq!(human_size(1536), "1.5 KB");
    }

    #[test]
    fn temp_area_shreds() {
        let mut t = TempArea::new();
        let p = t.write("secret.txt", b"top secret").unwrap();
        assert!(p.exists() && t.contains(&p));
        t.clear();
        assert!(!p.exists());
    }
}
