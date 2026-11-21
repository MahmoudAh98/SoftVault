use crate::crypto::{self, Key, NewVaultMaterial};
use crate::generator::{self, GenOptions};
use crate::mailer;
use crate::theme::{self, Palette};
use crate::render::{self, NoteFormat};
use crate::icons as ic;
use crate::totp;
use crate::vault::{self, ItemData, Kind, Recovery, Vault};
use crate::widgets::{self as w, card};
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Vec2};
use crate::transfer;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use zeroize::{Zeroize, Zeroizing};

// ───────────────────────────── background jobs ─────────────────────────────

enum JobResult {
    Created(anyhow::Result<NewVaultMaterial>),
    Unlocked(anyhow::Result<Key>),
    Rewrapped(anyhow::Result<NewVaultMaterial>),
    Exported(anyhow::Result<Vec<u8>>, PathBuf),
    ImportDecrypted(anyhow::Result<vault::PlainExport>),
    VerifyMailSent(Result<(), String>),
    HintMailSent(Result<(), String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Setup,
    Unlock,
    Mfa,
    Main,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    All,
    Passwords,
    Notes,
    Favorites,
    Generator,
    Health,
    Files,
    Settings,
}

/// Something being dragged out of the window.
enum DragSource {
    Note(String),
    File(String),
}

struct Row {
    id: String,
    title: String,
    sub: String,
    favorite: bool,
    note: bool,
    updated: i64,
    badge: &'static str,
    note_icon: &'static str,
    folder: String,
    tags: Vec<String>,
    views: u64,
}

/// Internal drag & drop payload (rows/cards/folders dragged onto sidebar folders).
#[derive(Clone)]
enum DragRef {
    Item(String),
    File(String),
    Folder(String),
}

struct FolderDialog {
    /// Some(id) = renaming that folder
    rename: Option<String>,
    parent: Option<String>,
    name: String,
}

#[derive(Default)]
struct Health {
    logins: usize,
    weak: Vec<String>,
    reused: Vec<String>,
    old: Vec<String>,
    no_2fa: usize,
}

impl Health {
    fn issue_count(&self) -> usize {
        let mut ids: Vec<&String> = self.weak.iter().chain(&self.reused).collect();
        ids.sort();
        ids.dedup();
        ids.len()
    }
}

struct Editor {
    id: Option<String>,
    data: ItemData,
    show_pw: bool,
    show_gen: bool,
    confirm_delete: bool,
    note_mode: NoteMode,
    opened: Instant,
}

#[derive(PartialEq, Clone, Copy)]
enum NoteMode {
    Edit,
    Split,
    Preview,
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

#[derive(Default)]
struct SettingsUi {
    mfa_setup: Option<String>, // base32 secret being enrolled
    mfa_code: String,
    email: String,
    app_pw: Zeroizing<String>,
    pending_verify: Option<(String, Zeroizing<String>, Zeroizing<String>, Instant)>, // email, app pw, code, sent
    verify_sent: bool,
    verify_input: String,
    hint: Zeroizing<String>,
    old_pw: Zeroizing<String>,
    new_pw: Zeroizing<String>,
    new_pw2: Zeroizing<String>,
    export_pw: Zeroizing<String>,
    export_pw2: Zeroizing<String>,
    plain_ack: bool,
    import_bytes: Option<Vec<u8>>,
    import_pw: Zeroizing<String>,
    loaded: bool,
    tab: usize,
}

struct Toast {
    text: String,
    ok: bool,
    until: Instant,
    born: Instant,
}

pub struct SoftVaultApp {
    vault: Vault,
    dark: bool,
    screen: Screen,
    view: View,
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    busy: Option<String>,
    mail_busy: bool,

    pw: Zeroizing<String>,
    pw2: Zeroizing<String>,
    hint_input: Zeroizing<String>,
    mfa_input: String,
    show_pw: bool,
    error: Option<String>,
    failed: u32,
    lockout_until: Option<Instant>,
    mfa_failed: u32,
    hint_sent_at: Option<Instant>,

    search: String,
    selected: Option<String>,
    editor: Option<Editor>,
    gen_opts: GenOptions,
    su: SettingsUi,

    last_activity: Instant,
    clip_clear_at: Option<Instant>,
    toast: Option<Toast>,
    applied_theme: Option<bool>,
    want_focus: bool,
    gen_preview: Zeroizing<String>,
    full: Option<FullView>,
    sort: (w::SortCol, bool),
    db_location: vault::DbLocation,
    deleting: Option<(String, Instant)>,
    undo: Option<UndoDelete>,
    view_changed: (View, Instant),
    temp: transfer::TempArea,
    drag_out: Option<DragSource>,
    file_confirm: Option<String>,
    file_deleting: Option<(String, Instant)>,
    folder_filter: Option<String>,
    tag_filter: std::collections::BTreeSet<String>,
    expanded: std::collections::HashSet<String>,
    folder_dialog: Option<FolderDialog>,
    confirm_folder_delete: Option<String>,
}

const DELETE_ANIM_SECS: f32 = 0.85;
const UNDO_SECS: u64 = 10;

/// A just-deleted item kept in memory for a few seconds so it can be restored.
struct UndoDelete {
    id: String,
    data: ItemData,
    title: String,
    until: Instant,
}

impl Drop for UndoDelete {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

struct FullView {
    item_id: Option<String>,
    zoom: f32,
    os_fullscreen: bool,
    show_toc: bool,
}

impl SoftVaultApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let location = vault::resolve_db_location();
        let vault = Vault::open(&location.path).expect("could not open vault database");
        let dark = vault.get_plain("theme").map(|t| t != "light").unwrap_or(true);
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, dark);
        let screen = if vault.is_initialized() { Screen::Unlock } else { Screen::Setup };
        let (tx, rx) = channel();
        Self {
            vault,
            dark,
            screen,
            view: View::All,
            tx,
            rx,
            busy: None,
            mail_busy: false,
            pw: Default::default(),
            pw2: Default::default(),
            hint_input: Default::default(),
            mfa_input: String::new(),
            show_pw: false,
            error: None,
            failed: 0,
            lockout_until: None,
            mfa_failed: 0,
            hint_sent_at: None,
            search: String::new(),
            selected: None,
            editor: None,
            gen_opts: GenOptions::default(),
            su: SettingsUi::default(),
            last_activity: Instant::now(),
            clip_clear_at: None,
            toast: None,
            applied_theme: Some(dark),
            want_focus: true,
            gen_preview: Default::default(),
            full: None,
            sort: (w::SortCol::Name, true),
            db_location: location,
            deleting: None,
            undo: None,
            view_changed: (View::All, Instant::now()),
            temp: transfer::TempArea::new(),
            drag_out: None,
            file_confirm: None,
            file_deleting: None,
            folder_filter: None,
            tag_filter: Default::default(),
            expanded: Default::default(),
            folder_dialog: None,
            confirm_folder_delete: None,
        }
    }

    fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toast = Some(Toast { text: text.into(), ok, until: Instant::now() + Duration::from_secs(4), born: Instant::now() });
    }

    fn spawn(&mut self, ctx: &egui::Context, label: Option<&str>, f: impl FnOnce() -> JobResult + Send + 'static) {
        if let Some(l) = label {
            self.busy = Some(l.to_string());
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
    }

    fn copy(&mut self, ctx: &egui::Context, text: &str, what: &str) {
        ctx.copy_text(text.to_string());
        let secs = self.vault.settings.clipboard_clear_secs.max(5);
        self.clip_clear_at = Some(Instant::now() + Duration::from_secs(secs as u64));
        self.toast(format!("{what} copied · clears in {secs}s"), true);
    }

    fn lock(&mut self, ctx: &egui::Context) {
        self.close_fullscreen(ctx);
        self.temp.clear();
        self.drag_out = None;
        self.file_confirm = None;
        self.file_deleting = None;
        self.undo = None;
        self.deleting = None;
        self.editor = None;
        self.selected = None;
        self.su = SettingsUi::default();
        self.vault.lock();
        self.pw.zeroize();
        self.mfa_input.clear();
        self.search.clear();
        self.error = None;
        if self.clip_clear_at.take().is_some() {
            ctx.copy_text(String::new());
        }
        self.screen = Screen::Unlock;
        self.want_focus = true;
    }

    fn set_theme(&mut self, dark: bool) {
        self.dark = dark;
        let _ = self.vault.set_plain("theme", if dark { "dark" } else { "light" });
    }

    // ─────────────────────────── job results ───────────────────────────

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.rx.try_recv() {
            match res {
                JobResult::Created(r) => {
                    self.busy = None;
                    match r.and_then(|m| self.vault.finish_create(m)) {
                        Ok(()) => {
                            let _ = self.vault.set_hint(&self.hint_input);
                            self.hint_input.zeroize();
                            self.pw.zeroize();
                            self.pw2.zeroize();
                            self.screen = Screen::Main;
                            self.toast("Vault created — welcome!", true);
                        }
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                JobResult::Unlocked(r) => {
                    self.busy = None;
                    match r.and_then(|k| self.vault.finish_unlock(k)) {
                        Ok(()) => {
                            self.pw.zeroize();
                            self.failed = 0;
                            self.error = None;
                            self.mfa_failed = 0;
                            self.screen = if self.vault.settings.mfa_secret.is_some() { Screen::Mfa } else { Screen::Main };
                            self.want_focus = true;
                            self.last_activity = Instant::now();
                        }
                        Err(e) => {
                            self.failed += 1;
                            if self.failed >= 3 {
                                let secs = (1u64 << (self.failed - 2).min(6)).min(60);
                                self.lockout_until = Some(Instant::now() + Duration::from_secs(secs));
                            }
                            self.error = Some(e.to_string());
                        }
                    }
                }
                JobResult::Rewrapped(r) => {
                    self.busy = None;
                    match r.and_then(|m| self.vault.store_material(&m)) {
                        Ok(()) => {
                            self.su.old_pw.zeroize();
                            self.su.new_pw.zeroize();
                            self.su.new_pw2.zeroize();
                            self.toast("Master password changed", true);
                        }
                        Err(e) => self.toast(e.to_string(), false),
                    }
                }
                JobResult::Exported(r, path) => {
                    self.busy = None;
                    match r.and_then(|b| std::fs::write(&path, b).map_err(Into::into)) {
                        Ok(()) => {
                            self.su.export_pw.zeroize();
                            self.su.export_pw2.zeroize();
                            self.toast(format!("Encrypted export saved to {}", path.display()), true);
                        }
                        Err(e) => self.toast(format!("Export failed: {e}"), false),
                    }
                }
                JobResult::ImportDecrypted(r) => {
                    self.busy = None;
                    match r.and_then(|e| self.vault.import_export(e)) {
                        Ok((n, f)) => {
                            self.su.import_bytes = None;
                            self.su.import_pw.zeroize();
                            self.toast(format!("Imported {n} items and {f} files"), true);
                        }
                        Err(e) => self.toast(format!("Import failed: {e}"), false),
                    }
                }
                JobResult::VerifyMailSent(r) => {
                    self.mail_busy = false;
                    match r {
                        Ok(()) => {
                            self.su.verify_sent = true;
                            self.toast("Verification code sent — check your Gmail", true);
                        }
                        Err(e) => {
                            self.su.pending_verify = None;
                            self.toast(e, false);
                        }
                    }
                }
                JobResult::HintMailSent(r) => {
                    self.mail_busy = false;
                    match r {
                        Ok(()) => self.toast("Password hint sent to your Gmail", true),
                        Err(e) => {
                            self.hint_sent_at = None;
                            self.toast(e, false)
                        }
                    }
                }
            }
        }
        let _ = ctx;
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let active = ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving() || i.pointer.any_down());
        if active {
            self.last_activity = Instant::now();
        }
        if let Some(t) = self.clip_clear_at
            && Instant::now() >= t {
                ctx.copy_text(String::new());
                self.clip_clear_at = None;
            }
        if matches!(self.screen, Screen::Main | Screen::Mfa) {
            let mins = self.vault.settings.auto_lock_minutes.max(1) as u64;
            if self.last_activity.elapsed() >= Duration::from_secs(mins * 60) {
                self.lock(ctx);
                self.toast("Locked after inactivity", true);
            }
        }
        self.temp.sweep();
        if self.undo.as_ref().is_some_and(|u| Instant::now() > u.until) {
            self.undo = None;
        }
        if let Some(t) = &self.toast
            && Instant::now() > t.until
        {
            self.toast = None;
        }
        ctx.request_repaint_after(Duration::from_millis(1000));
    }

    // ─────────────────────────── auth screens ───────────────────────────

    fn auth_frame(&mut self, ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut Self, &mut egui::Ui)) {
        egui::CentralPanel::default().frame(Frame::new().fill(p.bg_alt)).show(ui, |ui| {
            let avail = ui.available_size();
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space((avail.y * 0.5 - 320.0).max(20.0));
                ui.vertical_centered(|ui| {
                    ui.set_max_width(460.0);
                    w::logo(ui, p, 48.0, false);
                    ui.add_space(10.0);
                    ui.label(w::h("SoftVault", 26.0));
                    ui.add_space(20.0);
                    Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(theme::R_CARD)
                        .inner_margin(Margin::symmetric(32, 28))
                        .show(ui, |ui| {
                            ui.set_width(396.0);
                            ui.with_layout(Layout::top_down(Align::Min), |ui| add(self, ui));
                        });
                    ui.add_space(16.0);
                    ui.label(RichText::new("\u{E10B} Argon2id · XChaCha20-Poly1305 · stored locally in SQLite").size(12.0).color(p.weak));
                    ui.add_space(16.0);
                });
            });
        });
    }

    fn error_line(&self, ui: &mut egui::Ui, p: &Palette) {
        if let Some(e) = &self.error {
            ui.label(RichText::new(format!("\u{E193} {e}")).color(p.danger).size(13.0));
        }
    }

    fn ui_setup(&mut self, ui: &mut egui::Ui, p: &Palette) {
        self.auth_frame(ui, p, |s, ui| {
            ui.vertical_centered(|ui| ui.label(w::h("Create your vault", 20.0)));
            ui.label(RichText::new("Your master password is never stored or sent anywhere. If you forget it, your data cannot be recovered.").size(12.5).color(p.weak));
            ui.add_space(8.0);
            w::label(ui, p, "Master password (required)");
            w::row_rtl(ui, |ui| {
                if w::icon_btn(ui, p, if s.show_pw { ic::EYE_OFF } else { ic::EYE }, "Show / hide").clicked() {
                    s.show_pw = !s.show_pw;
                }
                ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut *s.pw).password(!s.show_pw).hint_text("At least 12 characters").margin(Margin::symmetric(12, 9)));
            });
            let (sc, lb, bits) = generator::strength(&s.pw);
            w::strength_bar(ui, p, sc, lb, bits);
            w::label(ui, p, "Confirm master password (required)");
            let r = w::text_input(ui, &mut s.pw2, "Repeat master password", !s.show_pw);
            w::label(ui, p, "Master password hint");
            w::text_input(ui, &mut s.hint_input, "Something only you understand", false);
            ui.label(RichText::new("The hint is never shown on screen. It can only be emailed to a verified Gmail you add later in Settings.").size(11.5).color(p.weak));
            ui.add_space(6.0);
            s.error_line(ui, p);
            if let Some(b) = &s.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b.as_str());
                });
            } else if w::primary_wide(ui, p, "Create vault").clicked()
                || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                s.try_create(ui.ctx());
            }
        });
    }

    fn try_create(&mut self, ctx: &egui::Context) {
        self.error = None;
        if self.pw.chars().count() < 12 {
            self.error = Some("Use at least 12 characters".into());
            return;
        }
        if *self.pw != *self.pw2 {
            self.error = Some("Passwords do not match".into());
            return;
        }
        if !self.hint_input.is_empty() && self.hint_input.to_lowercase().contains(&self.pw.to_lowercase()) {
            self.error = Some("The hint must not contain your password".into());
            return;
        }
        let pw = Zeroizing::new(self.pw.as_bytes().to_vec());
        self.spawn(ctx, Some("Deriving key with Argon2id…"), move || {
            JobResult::Created(crypto::build_new_vault(&pw))
        });
    }

    fn ui_unlock(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let recovery = self.vault.recovery().filter(|r| r.verified);
        let has_hint = self.vault.hint().is_some();
        self.auth_frame(ui, p, |s, ui| {
            ui.vertical_centered(|ui| ui.label(w::h("Your vault is locked", 20.0)));
            ui.vertical_centered(|ui| ui.label(RichText::new("Verify your identity to continue.").size(13.5).color(p.weak)));
            ui.add_space(8.0);
            w::label(ui, p, "Master password (required)");
            let r = w::row_rtl(ui, |ui| {
                if w::icon_btn(ui, p, if s.show_pw { ic::EYE_OFF } else { ic::EYE }, "Show / hide").clicked() {
                    s.show_pw = !s.show_pw;
                }
                ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut *s.pw).password(!s.show_pw).hint_text("Master password").margin(Margin::symmetric(12, 9)))
            });
            if s.want_focus {
                s.want_focus = false;
                r.request_focus();
            }
            s.error_line(ui, p);
            let locked_out = s.lockout_until.map(|t| Instant::now() < t).unwrap_or(false);
            if let Some(b) = &s.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b.as_str());
                });
            } else if locked_out {
                let left = s.lockout_until.unwrap().saturating_duration_since(Instant::now()).as_secs() + 1;
                ui.label(RichText::new(format!("Too many attempts — try again in {left}s")).color(p.warn));
            } else if w::primary_wide(ui, p, "Unlock").clicked()
                || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                s.try_unlock(ui.ctx());
            }
            ui.add_space(6.0);
            ui.add_space(4.0);
            ui.separator();
            ui.label(w::h("Forgot your master password?", 13.5));
            match (&recovery, has_hint) {
                (Some(rec), true) => {
                    let wait = s.hint_sent_at.map(|t| 60u64.saturating_sub(t.elapsed().as_secs())).unwrap_or(0);
                    if s.mail_busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Sending hint…");
                        });
                    } else if wait > 0 {
                        ui.label(RichText::new(format!("Hint sent. You can resend in {wait}s.")).color(p.weak).size(12.5));
                    } else if w::ghost_wide(ui, p, &format!("\u{E10F}  Email my hint to {}", mailer::mask_email(&rec.email))).clicked() {
                        let rec = rec.clone();
                        let hint = s.vault.hint().unwrap();
                        s.mail_busy = true;
                        s.hint_sent_at = Some(Instant::now());
                        s.spawn(ui.ctx(), None, move || {
                            let body = format!(
                                "Hello,\n\nYou requested your SoftVault master password hint.\n\nHint: {}\n\nIf you didn't request this, someone has access to your computer.\n\n— SoftVault",
                                hint.as_str()
                            );
                            JobResult::HintMailSent(mailer::send_to_self(&rec.email, &rec.app_password, "Your SoftVault password hint", body))
                        });
                    }
                }
                _ => {
                    ui.label(RichText::new("Add a password hint and verify a Gmail address in Settings to receive your hint by email.").size(12.0).color(p.weak));
                }
            }
        });
    }

    fn try_unlock(&mut self, ctx: &egui::Context) {
        self.error = None;
        let m = match self.vault.unlock_material() {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let pw = Zeroizing::new(self.pw.as_bytes().to_vec());
        self.spawn(ctx, Some("Unlocking…"), move || {
            JobResult::Unlocked(crypto::unwrap_vault_key(&pw, &m.params, &m.salt, &m.wrapped))
        });
    }

    fn ui_mfa(&mut self, ui: &mut egui::Ui, p: &Palette) {
        self.auth_frame(ui, p, |s, ui| {
            ui.vertical_centered(|ui| ui.label(w::h("Two-step verification", 20.0)));
            ui.label(RichText::new("Open your authenticator app and enter the 6-digit SoftVault code.").size(12.5).color(p.weak));
            ui.add_space(8.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut s.mfa_input)
                    .hint_text("123 456")
                    .font(egui::FontId::monospace(22.0))
                    .align(egui::Align2::CENTER_CENTER)
                    .desired_width(f32::INFINITY)
                    .char_limit(7)
                    .margin(Margin::symmetric(12, 10)),
            );
            if s.want_focus {
                s.want_focus = false;
                r.request_focus();
            }
            s.error_line(ui, p);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if w::primary_wide(ui, p, "Verify").clicked() || enter || s.mfa_input.chars().filter(|c| c.is_ascii_digit()).count() == 6 {
                let secret = s.vault.settings.mfa_secret.clone().unwrap_or_default();
                match totp::verify(&secret, &s.mfa_input, s.vault.settings.mfa_last_step) {
                    Some(step) => {
                        s.vault.settings.mfa_last_step = step;
                        let _ = s.vault.save_settings();
                        s.mfa_input.clear();
                        s.error = None;
                        s.screen = Screen::Main;
                    }
                    None => {
                        s.mfa_input.clear();
                        s.mfa_failed += 1;
                        s.error = Some("Invalid code".into());
                        if s.mfa_failed >= 5 {
                            s.lock(ui.ctx());
                            s.error = Some("Too many invalid codes — vault locked".into());
                        }
                    }
                }
            }
            if w::ghost_wide(ui, p, "Cancel").clicked() {
                s.lock(ui.ctx());
            }
        });
    }

    // ─────────────────────────── main screen ───────────────────────────

    fn ui_main(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        egui::Panel::left("nav")
            .exact_size(244.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.panel).inner_margin(Margin::symmetric(14, 18)))
            .show(ui, |ui| self.ui_nav(ui, p));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin::symmetric(32, 24)))
            .show(ui, |ui| {
                // gentle fade + rise when switching pages
                if self.view_changed.0 != self.view {
                    self.view_changed = (self.view, Instant::now());
                }
                let t = (self.view_changed.1.elapsed().as_secs_f32() / 0.2).min(1.0);
                let ease = 1.0 - (1.0 - t).powi(3);
                if t < 1.0 {
                    ui.ctx().request_repaint();
                }
                ui.multiply_opacity(ease);
                ui.add_space((1.0 - ease) * 10.0);
                match self.view {
                    View::Settings => self.ui_settings(ui, p),
                    View::Generator => self.ui_generator(ui, p),
                    View::Health => self.ui_health(ui, p),
                    View::Files => self.ui_files(ui, p),
                    _ => self.ui_vault_page(ui, p),
                }
            });

        self.shortcuts(&ctx);
        if self.folder_dialog.is_some() {
            self.ui_folder_dialog(&ctx, p);
        }
        if self.editor.is_some() {
            self.ui_editor_modal(&ctx, p);
        }
        if self.full.is_some() {
            self.ui_fullscreen(&ctx, p);
        }
    }

    fn ui_nav(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            w::logo(ui, p, 32.0, true);
            ui.add_space(2.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(w::h("SoftVault", 17.0).color(p.nav_text));
                ui.label(RichText::new("Password Manager").size(11.5).color(p.nav_weak));
            });
        });
        ui.add_space(14.0);
        let footer_h = 118.0;
        let body_h = (ui.available_height() - footer_h).max(120.0);
        ScrollArea::vertical().id_salt("softvault-nav-scroll").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let n_pw = self.vault.items.iter().filter(|i| i.data.kind == Kind::Password).count();
            let n_notes = self.vault.items.iter().filter(|i| i.data.kind == Kind::Note).count();
            let n_fav = self.vault.items.iter().filter(|i| i.data.favorite).count();
            w::nav_section(ui, p, "VAULT");
            for (v, icon, text, count) in [
                (View::All, ic::LAYERS, "All items", self.vault.items.len()),
                (View::Passwords, ic::KEY_ROUND, "Logins", n_pw),
                (View::Notes, ic::STICKY_NOTE, "Secure notes", n_notes),
                (View::Favorites, ic::STAR, "Favorites", n_fav),
                (View::Files, ic::FOLDER_LOCK, "Secure files", self.vault.files.len()),
            ] {
                let sel = self.view == v && self.folder_filter.is_none() && self.tag_filter.is_empty();
                let r = w::nav_item(ui, p, icon, text, sel, Some(count));
                // dropping on "All items" takes things out of their folder
                if v == View::All {
                    if r.dnd_hover_payload::<DragRef>().is_some() {
                        ui.painter().rect_stroke(r.rect, theme::R_INPUT, Stroke::new(1.5, p.nav_text), egui::StrokeKind::Inside);
                    }
                    if let Some(pl) = r.dnd_release_payload::<DragRef>() {
                        self.move_to_folder(&pl, None);
                    }
                }
                if r.clicked() {
                    self.view = v;
                    self.folder_filter = None;
                    self.tag_filter.clear();
                }
            }

            // ── folder tree
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(w::h("FOLDERS", 11.5).color(p.nav_weak));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(ic::FOLDER_PLUS).size(14.0).color(p.nav_weak)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::splat(24.0)))
                        .on_hover_text("New folder")
                        .clicked()
                    {
                        self.folder_dialog = Some(FolderDialog { rename: None, parent: self.folder_filter.clone(), name: String::new() });
                    }
                });
            });
            if self.vault.folders.is_empty() {
                ui.label(RichText::new("Create folders to organize your work. Drag items onto a folder to move them.").size(11.5).color(p.nav_weak));
            }
            for (folder, depth) in self.folder_rows() {
                let has_kids = self.vault.folders.iter().any(|f| f.parent.as_deref() == Some(folder.id.as_str()));
                let open = self.expanded.contains(&folder.id);
                let count = self.folder_count(&folder.id);
                let fid = egui::Id::unique("softvault-folder").with(&folder.id);
                let hover_payload = ui.ctx().dragged_id().is_some() && egui::DragAndDrop::has_payload_of_type::<DragRef>(ui.ctx());
                let (r, chev) = w::tree_item(
                    ui,
                    p,
                    fid,
                    &folder.name,
                    depth,
                    has_kids.then_some(open),
                    self.folder_filter.as_deref() == Some(folder.id.as_str()),
                    count,
                    false,
                );
                // drag folders onto other folders to nest them (same response as the click)
                r.dnd_set_drag_payload(DragRef::Folder(folder.id.clone()));
                let target = hover_payload && r.contains_pointer();
                if target {
                    ui.painter().rect_stroke(r.rect, theme::R_INPUT, Stroke::new(1.5, p.nav_text), egui::StrokeKind::Inside);
                    // hovering a collapsed folder while dragging opens it after a moment
                    if has_kids && !open {
                        let t = ui.ctx().animate_bool_with_time(fid.with("spring"), true, 0.8);
                        if t >= 1.0 {
                            self.expanded.insert(folder.id.clone());
                        }
                    }
                }
                if let Some(pl) = r.dnd_release_payload::<DragRef>() {
                    self.move_to_folder(&pl, Some(folder.id.clone()));
                }
                if chev {
                    if open {
                        self.expanded.remove(&folder.id);
                    } else {
                        self.expanded.insert(folder.id.clone());
                    }
                } else if r.clicked() {
                    self.folder_filter = Some(folder.id.clone());
                    if self.view != View::Files {
                        self.view = View::All;
                    }
                    if has_kids {
                        self.expanded.insert(folder.id.clone());
                    }
                }
                let fid_s = folder.id.clone();
                egui::Popup::context_menu(&r).show(|ui| self.folder_menu(ui, p, &fid_s));
            }

            // ── tags
            let tags = self.vault.all_tags();
            if !tags.is_empty() {
                ui.add_space(6.0);
                w::nav_section(ui, p, "TAGS");
                for (tag, count) in tags {
                    let sel = self.tag_filter.contains(&tag);
                    if w::tag_nav_item(ui, p, &tag, sel, count).clicked() {
                        if sel {
                            self.tag_filter.remove(&tag);
                        } else {
                            self.tag_filter.insert(tag.clone());
                        }
                        if !matches!(self.view, View::All | View::Passwords | View::Notes | View::Favorites | View::Files) {
                            self.view = View::All;
                        }
                    }
                }
            }

            w::nav_section(ui, p, "TOOLS");
            if w::nav_item(ui, p, ic::DICES, "Generator", self.view == View::Generator, None).clicked() {
                self.view = View::Generator;
            }
            let issues = self.health().issue_count();
            if w::nav_item(ui, p, ic::SHIELD_CHECK, "Vault health", self.view == View::Health, (issues > 0).then_some(issues)).clicked() {
                self.view = View::Health;
            }
            w::nav_section(ui, p, "MANAGE");
            if w::nav_item(ui, p, ic::ARROW_DOWN_UP, "Import & export", self.view == View::Settings && self.su.tab == 3, None).clicked() {
                self.view = View::Settings;
                self.su.loaded = false;
                self.su.tab = 3;
            }
            if w::nav_item(ui, p, ic::SETTINGS, "Settings", self.view == View::Settings && self.su.tab != 3, None).clicked() {
                self.view = View::Settings;
                self.su.loaded = false;
                if self.su.tab == 3 {
                    self.su.tab = 0;
                }
            }
            ui.add_space(8.0);
        });

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            // account-style footer
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 17.0, if p.dark { p.accent } else { Color32::from_rgb(0x17, 0x5d, 0xdc) });
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, ic::USER, egui::FontId::proportional(16.0), if p.dark { p.on_accent } else { Color32::WHITE });
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(w::h("Local vault", 13.5).color(p.nav_text));
                    let mins = self.vault.settings.auto_lock_minutes.max(1) as u64 * 60;
                    let left = mins.saturating_sub(self.last_activity.elapsed().as_secs());
                    ui.label(RichText::new(format!("{}  Locks in {}:{:02}", ic::TIMER, left / 60, left % 60)).size(11.5).color(p.nav_weak));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(ic::LOCK).size(15.0).color(p.nav_text)).fill(p.nav_hover).stroke(Stroke::NONE).corner_radius(theme::PILL).min_size(Vec2::splat(34.0)))
                        .on_hover_text("Lock now (Ctrl+L)")
                        .clicked()
                    {
                        self.lock(ui.ctx());
                    }
                });
            });
            ui.add_space(10.0);
            let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(line, CornerRadius::ZERO, p.nav_hover);
            ui.add_space(10.0);
            Frame::new().fill(p.nav_hover).corner_radius(theme::PILL).inner_margin(Margin::same(3)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let half = (ui.available_width()) / 2.0;
                    for (dark, icon, txt) in [(false, ic::SUN, "Light"), (true, ic::MOON, "Dark")] {
                        let sel = self.dark == dark;
                        let b = egui::Button::new((egui::Atom::grow(), RichText::new(format!("{icon}  {txt}")).size(13.0).color(if sel { p.nav_active_text } else { p.nav_weak }), egui::Atom::grow()))
                            .fill(if sel { p.nav_active } else { Color32::TRANSPARENT })
                            .stroke(Stroke::NONE)
                            .corner_radius(theme::PILL)
                            .min_size(Vec2::new(half, 28.0));
                        if ui.add(b).clicked() {
                            self.set_theme(dark);
                        }
                    }
                });
            });
        });
    }

    // ───── folder helpers ─────

    /// Visible folder rows in tree order (children only when the parent is expanded).
    fn folder_rows(&self) -> Vec<(vault::Folder, usize)> {
        fn walk(all: &[vault::Folder], parent: Option<&str>, depth: usize, expanded: &std::collections::HashSet<String>, out: &mut Vec<(vault::Folder, usize)>) {
            let mut kids: Vec<&vault::Folder> = all.iter().filter(|f| f.parent.as_deref() == parent).collect();
            kids.sort_by_key(|f| f.name.to_lowercase());
            for f in kids {
                out.push((f.clone(), depth));
                if expanded.contains(&f.id) && depth < 12 {
                    walk(all, Some(&f.id), depth + 1, expanded, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.vault.folders, None, 0, &self.expanded, &mut out);
        out
    }

    /// Every folder in tree order (for pickers), regardless of expansion.
    fn folder_options(&self) -> Vec<(String, String, usize)> {
        fn walk(all: &[vault::Folder], parent: Option<&str>, depth: usize, out: &mut Vec<(String, String, usize)>) {
            let mut kids: Vec<&vault::Folder> = all.iter().filter(|f| f.parent.as_deref() == parent).collect();
            kids.sort_by_key(|f| f.name.to_lowercase());
            for f in kids {
                out.push((f.id.clone(), f.name.clone(), depth));
                if depth < 12 {
                    walk(all, Some(&f.id), depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.vault.folders, None, 0, &mut out);
        out
    }

    /// Items + files in a folder and all its sub-folders.
    fn folder_count(&self, id: &str) -> usize {
        let mut ids = self.vault.folder_descendants(id);
        ids.push(id.to_string());
        let inside = |f: &Option<String>| f.as_ref().is_some_and(|x| ids.contains(x));
        self.vault.items.iter().filter(|i| inside(&i.data.folder)).count() + self.vault.files.iter().filter(|f| inside(&f.meta.folder)).count()
    }

    fn in_folder_filter(&self, folder: &Option<String>) -> bool {
        match &self.folder_filter {
            None => true,
            Some(sel) => folder.as_ref().is_some_and(|f| f == sel || self.vault.folder_descendants(sel).contains(f)),
        }
    }

    fn has_tags(&self, tags: &[String]) -> bool {
        self.tag_filter.iter().all(|t| tags.contains(t))
    }

    fn move_to_folder(&mut self, what: &DragRef, folder: Option<String>) {
        let dest = folder.as_ref().map(|f| self.vault.folder_path(f)).unwrap_or_else(|| "top level".into());
        let res = match what {
            DragRef::Item(id) => self.vault.move_item(id, folder.clone()).map(|_| self.vault.items.iter().find(|i| &i.id == id).map(|i| i.data.title.clone()).unwrap_or_default()),
            DragRef::File(id) => {
                let f = folder.clone();
                self.vault.update_file(id, |m| m.folder = f).map(|_| self.vault.files.iter().find(|x| &x.id == id).map(|x| x.meta.name.clone()).unwrap_or_default())
            }
            DragRef::Folder(id) => {
                if folder.as_deref() == Some(id.as_str()) {
                    return;
                }
                self.vault.move_folder(id, folder.clone()).map(|_| self.vault.folders.iter().find(|f| &f.id == id).map(|f| f.name.clone()).unwrap_or_default())
            }
        };
        match res {
            Ok(name) => {
                if let Some(f) = &folder {
                    self.expanded.insert(f.clone());
                }
                self.toast(format!("Moved “{name}” to {dest}"), true)
            }
            Err(e) => self.toast(e.to_string(), false),
        }
    }

    fn folder_menu(&mut self, ui: &mut egui::Ui, p: &Palette, id: &str) {
        ui.set_min_width(210.0);
        ui.spacing_mut().button_padding = Vec2::new(10.0, 7.0);
        if ui.button(format!("{}   New subfolder", ic::FOLDER_PLUS)).clicked() {
            self.folder_dialog = Some(FolderDialog { rename: None, parent: Some(id.to_string()), name: String::new() });
            ui.close();
        }
        if ui.button(format!("{}   Rename", ic::PENCIL)).clicked() {
            let name = self.vault.folders.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_default();
            self.folder_dialog = Some(FolderDialog { rename: Some(id.to_string()), parent: None, name });
            ui.close();
        }
        let is_nested = self.vault.folders.iter().find(|f| f.id == id).is_some_and(|f| f.parent.is_some());
        if is_nested && ui.button(format!("{}   Move to top level", ic::CORNER_DOWN_RIGHT)).clicked() {
            self.move_to_folder(&DragRef::Folder(id.to_string()), None);
            ui.close();
        }
        ui.separator();
        let armed = self.confirm_folder_delete.as_deref() == Some(id);
        let label = if armed { format!("{}   Click again to delete", ic::TRASH_2) } else { format!("{}   Delete folder", ic::TRASH_2) };
        if ui.button(RichText::new(label).color(p.danger)).on_hover_text("Its contents move to the parent folder").clicked() {
            if armed {
                let name = self.vault.folders.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_default();
                match self.vault.delete_folder(id) {
                    Ok(()) => {
                        if self.folder_filter.as_deref() == Some(id) {
                            self.folder_filter = None;
                        }
                        self.toast(format!("Folder “{name}” deleted. Its contents moved up."), true);
                    }
                    Err(e) => self.toast(e.to_string(), false),
                }
                self.confirm_folder_delete = None;
                ui.close();
            } else {
                self.confirm_folder_delete = Some(id.to_string());
            }
        }
    }

    fn ui_folder_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut d) = self.folder_dialog.take() else { return };
        let mut close = false;
        let mut submit = false;
        let options = self.folder_options();
        egui::Modal::new(egui::Id::unique("softvault-folder-dialog"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 150 } else { 90 }))
            .frame(Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(theme::R_CARD).inner_margin(Margin::same(24)).shadow(theme::soft_shadow(p)))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.horizontal(|ui| {
                    w::icon_tile(ui, p, if d.rename.is_some() { ic::PENCIL } else { ic::FOLDER_PLUS }, 36.0);
                    ui.label(w::h(if d.rename.is_some() { "Rename folder" } else { "New folder" }, 18.0));
                });
                ui.add_space(12.0);
                w::label(ui, p, "Name");
                let r = ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("e.g. Work, Clients/ACME").desired_width(f32::INFINITY).margin(Margin::symmetric(12, 9)));
                if !r.has_focus() && d.name.is_empty() {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if d.rename.is_none() {
                    ui.add_space(8.0);
                    w::label(ui, p, "Inside");
                    let current = d.parent.as_ref().map(|id| self.vault.folder_path(id)).unwrap_or_else(|| "Top level".into());
                    egui::ComboBox::from_id_salt("softvault-folder-parent").width(ui.available_width()).selected_text(current).show_ui(ui, |ui| {
                        ui.selectable_value(&mut d.parent, None, "Top level");
                        for (id, name, depth) in &options {
                            ui.selectable_value(&mut d.parent, Some(id.clone()), format!("{}{} {name}", "    ".repeat(*depth), ic::FOLDER));
                        }
                    });
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if w::primary(ui, p, if d.rename.is_some() { "Rename" } else { "Create" }).clicked() {
                        submit = true;
                    }
                    if w::ghost(ui, p, "Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if submit {
            if d.name.trim().is_empty() {
                self.toast("Folder name can't be empty", false);
            } else {
                let res = match &d.rename {
                    Some(id) => self.vault.rename_folder(id, &d.name).map(|_| id.clone()),
                    None => {
                        // "A/B/C" creates nested folders in one go
                        let mut parent = d.parent.clone();
                        let mut last = Ok(String::new());
                        for part in d.name.split('/').map(str::trim).filter(|s| !s.is_empty()) {
                            let existing = self.vault.folders.iter().find(|f| f.parent == parent && f.name.eq_ignore_ascii_case(part)).map(|f| f.id.clone());
                            let id = match existing {
                                Some(id) => Ok(id),
                                None => self.vault.create_folder(part, parent.clone()),
                            };
                            match id {
                                Ok(id) => {
                                    if let Some(pp) = &parent {
                                        self.expanded.insert(pp.clone());
                                    }
                                    parent = Some(id.clone());
                                    last = Ok(id);
                                }
                                Err(e) => {
                                    last = Err(e);
                                    break;
                                }
                            }
                        }
                        last
                    }
                };
                match res {
                    Ok(_) => self.toast(if d.rename.is_some() { "Folder renamed" } else { "Folder created" }, true),
                    Err(e) => self.toast(e.to_string(), false),
                }
                close = true;
            }
        }
        if !close {
            self.folder_dialog = Some(d);
        }
    }

    fn visible_items(&self) -> Vec<Row> {
        let q = self.search.to_lowercase();
        let mut v: Vec<Row> = self
            .vault
            .items
            .iter()
            .filter(|i| match self.view {
                View::Passwords => i.data.kind == Kind::Password,
                View::Notes => i.data.kind == Kind::Note,
                View::Favorites => i.data.favorite,
                _ => true,
            })
            .filter(|i| self.in_folder_filter(&i.data.folder) && self.has_tags(&i.data.tags))
            .filter(|i| {
                q.is_empty()
                    || i.data.title.to_lowercase().contains(&q)
                    || i.data.username.to_lowercase().contains(&q)
                    || i.data.url.to_lowercase().contains(&q)
                    || i.data.tags.iter().any(|t| t.contains(q.trim_start_matches('#')))
                    || (i.data.kind == Kind::Note && i.data.notes.to_lowercase().contains(&q))
            })
            .map(|i| {
                let note = i.data.kind == Kind::Note;
                let sub = if note {
                    match i.data.format {
                        NoteFormat::Csv => format!("{} rows", i.data.notes.lines().count().saturating_sub(1)),
                        NoteFormat::Code if !i.data.lang.is_empty() => format!("{} · {} lines", i.data.lang, i.data.notes.lines().count()),
                        _ => i
                            .data
                            .notes
                            .lines()
                            .map(|l| l.trim_start_matches(['#', '>', '-', '*', ' ', '`', '{', '[']).trim())
                            .find(|l| !l.is_empty())
                            .unwrap_or("")
                            .to_string(),
                    }
                } else if !i.data.username.is_empty() {
                    i.data.username.clone()
                } else {
                    i.data.url.clone()
                };
                Row {
                    id: i.id.clone(),
                    title: i.data.title.clone(),
                    sub,
                    favorite: i.data.favorite,
                    note,
                    updated: i.data.updated,
                    badge: if note { i.data.format.badge() } else { "\u{E4A3}  Login" },
                    note_icon: i.data.format.icon(),
                    folder: i.data.folder.as_ref().map(|f| self.vault.folder_path(f)).unwrap_or_default(),
                    tags: i.data.tags.clone(),
                    views: i.data.views,
                }
            })
            .collect();
        let (col, asc) = self.sort;
        v.sort_by(|a, b| {
            let name = || a.title.to_lowercase().cmp(&b.title.to_lowercase());
            let o = match col {
                w::SortCol::Name => name(),
                w::SortCol::Folder => a.folder.to_lowercase().cmp(&b.folder.to_lowercase()).then_with(name),
                w::SortCol::Type => a.badge.cmp(b.badge).then_with(name),
                w::SortCol::Views => a.views.cmp(&b.views).then_with(name),
                w::SortCol::Updated => a.updated.cmp(&b.updated),
            };
            if asc { o } else { o.reverse() }
        });
        v
    }

    fn new_item(&mut self, kind: Kind) {
        self.selected = None;
        self.editor = Some(Editor {
            id: None,
            data: ItemData {
                kind,
                format: if kind == Kind::Note { NoteFormat::Markdown } else { NoteFormat::Plain },
                folder: self.folder_filter.clone(),
                tags: self.tag_filter.iter().cloned().collect(),
                ..Default::default()
            },
            show_pw: kind == Kind::Password,
            show_gen: false,
            confirm_delete: false,
            note_mode: if kind == Kind::Note { NoteMode::Split } else { NoteMode::Edit },
            opened: Instant::now(),
        });
        if kind == Kind::Password
            && let Some(e) = &mut self.editor
        {
            e.data.password = generator::generate(&self.gen_opts);
        }
    }

    fn select(&mut self, id: &str) {
        let _ = self.vault.record_item_view(id);
        if let Some(it) = self.vault.items.iter().find(|i| i.id == id) {
            self.selected = Some(id.to_string());
            self.editor = Some(Editor { id: Some(id.to_string()), data: it.data.clone(), show_pw: false, show_gen: false, confirm_delete: false, note_mode: NoteMode::Preview, opened: Instant::now() });
        }
    }

    fn new_item_menu(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let r = w::primary(ui, p, &format!("{}  New", ic::PLUS));
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(230.0);
            ui.spacing_mut().button_padding = Vec2::new(10.0, 8.0);
            if ui.button(format!("{}   Login            Ctrl+N", ic::KEY_ROUND)).clicked() {
                self.new_item(Kind::Password);
                ui.close();
            }
            if ui.button(format!("{}   Secure note   Ctrl+Shift+N", ic::STICKY_NOTE)).clicked() {
                self.new_item(Kind::Note);
                ui.close();
            }
            ui.separator();
            if ui.button(format!("{}   Folder", ic::FOLDER_PLUS)).clicked() {
                self.folder_dialog = Some(FolderDialog { rename: None, parent: self.folder_filter.clone(), name: String::new() });
                ui.close();
            }
        });
    }

    fn search_id() -> egui::Id {
        egui::Id::unique("softvault-search")
    }

    /// Global keyboard shortcuts while the vault is open.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.editor.is_some() || self.full.is_some() || self.folder_dialog.is_some() {
            return;
        }
        let (f, n, sn, l) = ctx.input(|i| {
            let c = i.modifiers.command;
            (
                c && i.key_pressed(egui::Key::F),
                c && !i.modifiers.shift && i.key_pressed(egui::Key::N),
                c && i.modifiers.shift && i.key_pressed(egui::Key::N),
                c && i.key_pressed(egui::Key::L),
            )
        });
        if f {
            if !matches!(self.view, View::All | View::Passwords | View::Notes | View::Favorites) {
                self.view = View::All;
            }
            ctx.memory_mut(|m| m.request_focus(Self::search_id()));
        }
        if n {
            self.new_item(Kind::Password);
        }
        if sn {
            self.new_item(Kind::Note);
        }
        if l {
            self.lock(ctx);
        }
    }

    /// Chips for the active folder / tag filters (shared by the vault and files pages).
    fn active_filter_chips(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if let Some(f) = self.folder_filter.clone() {
            let path = self.vault.folder_path(&f);
            if w::chip(ui, p, &format!("{}  {path}   {}", ic::FOLDER, ic::X), true).on_hover_text("Clear folder filter").clicked() {
                self.folder_filter = None;
            }
        }
        for t in self.tag_filter.clone() {
            if w::chip(ui, p, &format!("#{t}   {}", ic::X), true).on_hover_text("Remove tag filter").clicked() {
                self.tag_filter.remove(&t);
            }
        }
        let all = self.vault.all_tags();
        if !all.is_empty() {
            let r = w::chip(ui, p, &format!("{}  Tags  {}", ic::TAG, ic::CHEVRON_DOWN), false);
            egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(200.0);
                ui.label(RichText::new("Show items with all selected tags").size(12.0).color(p.weak));
                ui.separator();
                for (t, n) in all {
                    let mut on = self.tag_filter.contains(&t);
                    if ui.checkbox(&mut on, format!("#{t}  ({n})")).changed() {
                        if on {
                            self.tag_filter.insert(t.clone());
                        } else {
                            self.tag_filter.remove(&t);
                        }
                    }
                }
                if !self.tag_filter.is_empty() {
                    ui.separator();
                    if ui.button("Clear tag filters").clicked() {
                        self.tag_filter.clear();
                        ui.close();
                    }
                }
            });
        }
    }

    fn ui_vault_page(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let (title, icon) = match (&self.folder_filter, self.view) {
            (Some(f), _) => (self.vault.folder_path(f), ic::FOLDER_OPEN),
            (None, View::Passwords) => ("Logins".into(), ic::KEY_ROUND),
            (None, View::Notes) => ("Secure notes".into(), ic::STICKY_NOTE),
            (None, View::Favorites) => ("Favorites".into(), ic::STAR),
            _ => ("All items".into(), ic::LAYERS),
        };
        let items = self.visible_items();
        let subtitle = format!(
            "{} item{} · drag rows onto a folder to move them",
            items.len(),
            if items.len() == 1 { "" } else { "s" }
        );
        ui.horizontal(|ui| {
            w::page_header(ui, p, icon, &title, &subtitle);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.new_item_menu(ui, p);
                if w::ghost(ui, p, &format!("{}  Import", ic::UPLOAD)).clicked() {
                    self.view = View::Settings;
                    self.su.loaded = false;
                    self.su.tab = 3;
                }
            });
        });
        ui.add_space(16.0);

        // toolbar: search + type chips + folder/tag filters
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
            ui.add_sized(
                [300.0, 36.0],
                egui::TextEdit::singleline(&mut self.search)
                    .id(Self::search_id())
                    .hint_text(format!("{}   Search name, user, #tag…", ic::SEARCH))
                    .margin(Margin::symmetric(12, 9)),
            );
            if !self.search.is_empty() && w::ghost_icon(ui, p, ic::X, "Clear search").clicked() {
                self.search.clear();
            }
            ui.add_space(6.0);
            for (v, label) in [
                (View::All, format!("{}  All", ic::LAYERS)),
                (View::Passwords, format!("{}  Logins", ic::KEY_ROUND)),
                (View::Notes, format!("{}  Notes", ic::STICKY_NOTE)),
                (View::Favorites, format!("{}  Favorites", ic::STAR)),
            ] {
                if w::chip(ui, p, &label, self.view == v).clicked() {
                    self.view = v;
                }
            }
            ui.add_space(6.0);
            self.active_filter_chips(ui, p);
        });
        ui.add_space(12.0);

        let fmt = |t: i64| {
            chrono::DateTime::from_timestamp(t, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%b %d, %Y").to_string())
                .unwrap_or_default()
        };
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(theme::R_CARD)
            .inner_margin(Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                if let Some(col) = w::table_header(ui, p, self.sort) {
                    self.sort = if self.sort.0 == col { (col, !self.sort.1) } else { (col, !matches!(col, w::SortCol::Updated | w::SortCol::Views)) };
                }
                ScrollArea::vertical().id_salt("softvault-table").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if items.is_empty() {
                        ui.add_space(64.0);
                        if self.search.is_empty() && self.folder_filter.is_none() && self.tag_filter.is_empty() {
                            w::empty_state(ui, p, ic::VAULT, "No items yet", "Add a login or a secure note. Everything is encrypted on this device.");
                            ui.vertical_centered(|ui| {
                                ui.horizontal(|ui| {
                                    let wdt = 290.0;
                                    ui.add_space((ui.available_width() - wdt) / 2.0);
                                    if w::primary(ui, p, &format!("{}  New login", ic::KEY_ROUND)).clicked() {
                                        self.new_item(Kind::Password);
                                    }
                                    if w::ghost(ui, p, &format!("{}  New note", ic::STICKY_NOTE)).clicked() {
                                        self.new_item(Kind::Note);
                                    }
                                });
                            });
                        } else if self.folder_filter.is_some() && self.search.is_empty() {
                            w::empty_state(ui, p, ic::FOLDER_OPEN, "This folder is empty", "Drag items from “All items” onto the folder in the sidebar, or create a new one here.");
                        } else {
                            w::empty_state(ui, p, ic::SEARCH, "No matching items", "Try a different search, folder or tag.");
                        }
                    }
                    for row in items {
                        let id = row.id.clone();
                        let sel = self.selected.as_deref() == Some(&id);
                        let date = fmt(row.updated);
                        let (resp, opt_rect) = w::table_row(
                            ui,
                            p,
                            &w::RowData {
                                title: &row.title,
                                subtitle: &row.sub,
                                note: row.note,
                                note_icon: row.note_icon,
                                badge: row.badge,
                                favorite: row.favorite,
                                folder: &row.folder,
                                tags: &row.tags,
                                views: row.views,
                                updated: &date,
                                selected: sel,
                                drop_target: false,
                                query: &self.search,
                            },
                        );
                        // drag onto a sidebar folder to move, or out of the window to export (notes)
                        resp.dnd_set_drag_payload(DragRef::Item(id.clone()));
                        let hovered = ui.rect_contains_pointer(resp.rect) && !egui::DragAndDrop::has_any_payload(&ctx);

                        // Children always exist with stable ids (only their content depends on
                        // hover), so widget ids — and open menus — don't shift when the mouse moves.
                        let qa = egui::Rect::from_min_max(egui::pos2(opt_rect.left() - 150.0, opt_rect.top()), egui::pos2(opt_rect.left() - 4.0, opt_rect.bottom()));
                        let mut q = ui.new_child(egui::UiBuilder::new().id_salt(("softvault-row-qa", &id)).max_rect(qa).layout(Layout::right_to_left(Align::Center)));
                        q.spacing_mut().item_spacing.x = 2.0;
                        if hovered {
                            let data = self.vault.items.iter().find(|i| i.id == id).map(|i| (i.data.kind, i.data.username.clone(), i.data.password.clone(), i.data.url.clone()));
                            if let Some((kind, user, pw, url)) = data {
                                if kind == Kind::Password {
                                    if !url.is_empty() && w::ghost_icon(&mut q, p, ic::EXTERNAL_LINK, "Open website").clicked() {
                                        let u = if url.contains("://") { url.clone() } else { format!("https://{url}") };
                                        if u.starts_with("https://") || u.starts_with("http://") {
                                            ctx.open_url(egui::OpenUrl::new_tab(u));
                                        }
                                    }
                                    if w::ghost_icon(&mut q, p, ic::KEY_ROUND, "Copy password").clicked() {
                                        self.copy(&ctx, &pw, "Password");
                                    }
                                    if !user.is_empty() && w::ghost_icon(&mut q, p, ic::USER, "Copy username").clicked() {
                                        self.copy(&ctx, &user, "Username");
                                    }
                                } else if w::ghost_icon(&mut q, p, ic::MAXIMIZE_2, "Open full screen").clicked() {
                                    self.open_fullscreen(Some(id.clone()));
                                }
                            }
                        }

                        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("softvault-row-opt", &id)).max_rect(opt_rect).layout(Layout::left_to_right(Align::Center)));
                        let ob = w::ghost_icon(&mut child, p, ic::ELLIPSIS_VERTICAL, "More options");
                        let menu = |ui: &mut egui::Ui, s: &mut Self| s.item_menu(ui, p, &ctx, &id);
                        egui::Popup::menu(&ob).show(|ui| menu(ui, self));
                        egui::Popup::context_menu(&resp).show(|ui| menu(ui, self));
                        if resp.clicked() {
                            self.select(&id);
                        }
                    }
                });
            });
    }

    fn item_menu(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context, id: &str) {
        ui.set_min_width(220.0);
        ui.spacing_mut().button_padding = Vec2::new(10.0, 7.0);
        let Some(data) = self.vault.items.iter().find(|i| i.id == id).map(|i| i.data.clone()) else { return };
        if data.kind == Kind::Password {
            if ui.button(format!("{}   Copy username", ic::USER)).clicked() {
                self.copy(ctx, &data.username, "Username");
                ui.close();
            }
            if ui.button(format!("{}   Copy password", ic::KEY_ROUND)).clicked() {
                self.copy(ctx, &data.password, "Password");
                ui.close();
            }
        } else {
            if ui.button(format!("{}   Copy note", ic::COPY)).clicked() {
                self.copy(ctx, &data.notes, "Note");
                ui.close();
            }
            if ui.button(format!("{}   Open full screen", ic::MAXIMIZE_2)).clicked() {
                self.open_fullscreen(Some(id.to_string()));
                ui.close();
            }
            if ui.button(format!("{}   Save decrypted copy…", ic::DOWNLOAD)).clicked() {
                ui.close();
                self.save_decrypted(DragSource::Note(id.to_string()));
            }
        }
        if ui.button(format!("{}   Edit", ic::PENCIL)).clicked() {
            self.select(id);
            ui.close();
        }
        if ui.button(format!("{}   {}", ic::STAR, if data.favorite { "Remove from favorites" } else { "Add to favorites" })).clicked() {
            let mut d = data.clone();
            d.favorite = !d.favorite;
            let _ = self.vault.store_item(Some(id.to_string()), d, false);
            ui.close();
        }
        // Move to folder
        let options = self.folder_options();
        ui.menu_button(format!("{}   Move to folder", ic::FOLDER_INPUT), |ui| {
            ui.set_min_width(200.0);
            if ui.add_enabled(data.folder.is_some(), egui::Button::new(format!("{}  No folder", ic::LAYERS))).clicked() {
                self.move_to_folder(&DragRef::Item(id.to_string()), None);
                ui.close();
            }
            for (fid, name, depth) in &options {
                let here = data.folder.as_deref() == Some(fid.as_str());
                if ui.add_enabled(!here, egui::Button::new(format!("{}{}  {name}", "    ".repeat(*depth), ic::FOLDER))).clicked() {
                    self.move_to_folder(&DragRef::Item(id.to_string()), Some(fid.clone()));
                    ui.close();
                }
            }
            if options.is_empty() {
                ui.label(RichText::new("No folders yet").color(p.weak));
            }
        });
        ui.separator();
        if ui.button(RichText::new(format!("{}   Delete", ic::TRASH_2)).color(p.danger)).clicked() {
            self.select(id);
            if let Some(e) = &mut self.editor {
                e.confirm_delete = true;
            }
            ui.close();
        }
    }

    // ─────────────────────────── secure files & drag-and-drop ───────────────────────────

    fn file_icon(name: &str) -> (&'static str, fn(&Palette) -> Color32) {
        let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "heic" => (ic::FILE_IMAGE, |p| p.success),
            "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" => (ic::FILE_ARCHIVE, |p| p.warn),
            "pdf" | "doc" | "docx" | "odt" | "rtf" => (ic::FILE_TEXT, |p| p.danger),
            "xls" | "xlsx" | "ods" => (ic::FILE_SPREADSHEET, |p| p.success),
            "mp3" | "wav" | "flac" | "ogg" | "m4a" => (ic::MUSIC, |p| p.accent),
            "mp4" | "mkv" | "mov" | "avi" | "webm" => (ic::VIDEO, |p| p.accent),
            "key" | "pem" | "pfx" | "p12" | "kdbx" | "asc" | "gpg" => (ic::FILE_KEY, |p| p.warn),
            "exe" | "msi" | "dll" | "bin" | "iso" => (ic::HARD_DRIVE, |p| p.weak),
            _ => (ic::FILE, |p| p.accent),
        }
    }

    /// Encrypt dropped/picked files into the vault.
    fn import_paths(&mut self, paths: Vec<PathBuf>) {
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| !self.temp.contains(p)).collect();
        if paths.is_empty() {
            return;
        }
        let files = transfer::collect_paths(&paths, 500);
        let (mut notes, mut stored, mut shredded) = (0usize, 0usize, 0usize);
        let mut errors: Vec<String> = Vec::new();
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
            if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > vault::MAX_FILE_BYTES {
                errors.push(format!("{name}: larger than {} MB", vault::MAX_FILE_BYTES / 1024 / 1024));
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => Zeroizing::new(b),
                Err(e) => {
                    errors.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let as_note = if self.vault.settings.import_text_as_notes {
                match transfer::classify(&path) {
                    transfer::Classified::Note { format, lang } => transfer::note_from_file(&path, &bytes, format, lang),
                    transfer::Classified::File => None,
                }
            } else {
                None
            };
            let folder = self.folder_filter.clone();
            let tags: Vec<String> = self.tag_filter.iter().cloned().collect();
            let result = match as_note {
                Some(mut note) => {
                    note.folder = folder;
                    note.tags = tags;
                    self.vault.save_item(None, note).map(|_| notes += 1)
                }
                None => self.vault.add_file(&name, &bytes, None).and_then(|id| {
                    stored += 1;
                    self.vault.update_file(&id, |m| {
                        m.folder = folder;
                        m.tags = tags;
                    })
                }),
            };
            match result {
                Ok(()) => {
                    if self.vault.settings.move_originals && transfer::shred(&path).is_ok() {
                        shredded += 1;
                    }
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        let mut parts = Vec::new();
        if stored > 0 {
            parts.push(format!("{stored} file{}", if stored == 1 { "" } else { "s" }));
        }
        if notes > 0 {
            parts.push(format!("{notes} note{}", if notes == 1 { "" } else { "s" }));
        }
        if !parts.is_empty() {
            let moved = if shredded > 0 { format!(" · {shredded} original{} securely deleted", if shredded == 1 { "" } else { "s" }) } else { String::new() };
            self.toast(format!("Encrypted {} into your vault{moved}", parts.join(" and ")), true);
        }
        if !errors.is_empty() {
            self.toast(format!("Couldn't import: {}", errors.join("; ")), false);
        }
    }

    /// Runs after the UI with access to the native window (needed for OS drag & drop).
    fn process_drag_out(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(src) = self.drag_out.take() else { return };
        if !transfer::DRAG_OUT_SUPPORTED {
            self.toast("Dragging out of the app is available on Windows. Use “Save decrypted…” instead.", false);
            return;
        }
        let payload: anyhow::Result<(String, Zeroizing<Vec<u8>>)> = match &src {
            DragSource::Note(id) => self
                .vault
                .items
                .iter()
                .find(|i| &i.id == id)
                .map(|i| (transfer::note_file_name(&i.data), Zeroizing::new(i.data.notes.as_bytes().to_vec())))
                .ok_or_else(|| anyhow::anyhow!("note not found")),
            DragSource::File(id) => match self.vault.files.iter().find(|f| &f.id == id).map(|f| f.meta.name.clone()) {
                Some(name) => self.vault.read_file(id).map(|b| (name, b)),
                None => Err(anyhow::anyhow!("file not found")),
            },
        };
        let (name, bytes) = match payload {
            Ok(v) => v,
            Err(e) => {
                self.toast(e.to_string(), false);
                return;
            }
        };
        let path = match self.temp.write(&name, &bytes) {
            Ok(p) => p,
            Err(e) => {
                self.toast(format!("Couldn't prepare the file: {e}"), false);
                return;
            }
        };
        if let DragSource::File(id) = &src {
            let _ = self.vault.record_file_view(id);
        }
        match transfer::start_drag_out(frame, vec![path]) {
            Ok(true) => self.toast(format!("Decrypted copy of “{name}” saved"), true),
            Ok(false) => {}
            Err(e) => self.toast(e, false),
        }
        ctx.stop_dragging();
        self.last_activity = Instant::now();
    }

    fn save_decrypted(&mut self, src: DragSource) {
        let payload = match &src {
            DragSource::Note(id) => self.vault.items.iter().find(|i| &i.id == id).map(|i| Ok((transfer::note_file_name(&i.data), Zeroizing::new(i.data.notes.as_bytes().to_vec())))),
            DragSource::File(id) => self.vault.files.iter().find(|f| &f.id == id).map(|f| f.meta.name.clone()).map(|n| self.vault.read_file(id).map(|b| (n, b))),
        };
        match payload {
            Some(Ok((name, bytes))) => {
                if let DragSource::File(id) = &src {
                    let _ = self.vault.record_file_view(id);
                }
                if let Some(path) = rfd::FileDialog::new().set_file_name(&name).save_file() {
                    match std::fs::write(&path, &*bytes) {
                        Ok(()) => self.toast(format!("Decrypted copy saved to {}", path.display()), true),
                        Err(e) => self.toast(format!("Save failed: {e}"), false),
                    }
                }
            }
            Some(Err(e)) => self.toast(e.to_string(), false),
            None => {}
        }
    }

    fn ui_files(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let total: u64 = self.vault.files.iter().map(|f| f.meta.size).sum();
        let n = self.vault.files.len();
        let title = match &self.folder_filter {
            Some(f) => format!("Secure files · {}", self.vault.folder_path(f)),
            None => "Secure files".into(),
        };
        ui.horizontal(|ui| {
            w::page_header(ui, p, ic::FOLDER_LOCK, &title, &format!("{n} file{} · {} encrypted", if n == 1 { "" } else { "s" }, transfer::human_size(total)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::primary(ui, p, &format!("{}  Add files", ic::PLUS)).clicked()
                    && let Some(paths) = rfd::FileDialog::new().pick_files()
                {
                    self.import_paths(paths);
                }
            });
        });
        ui.add_space(16.0);

        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // ── drop zone
            let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
            let ht = ui.ctx().animate_bool_with_time(egui::Id::unique("softvault-dropzone"), hovering, 0.2);
            let (zone, zresp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 168.0), egui::Sense::click());
            let zhover = zresp.hovered();
            let painter = ui.painter();
            painter.rect_filled(zone, theme::R_CARD, w::lerp(if zhover { p.card_hover } else { p.card }, p.accent_soft, ht));
            let time = ui.ctx().input(|i| i.time) as f32;
            let offset = if hovering { time * 30.0 } else { 0.0 };
            w::dashed_rounded_rect(painter, zone.shrink(1.0), 12.0, Stroke::new(1.6, w::lerp(p.border_strong.gamma_multiply(0.6), p.accent, ht.max(if zhover { 0.5 } else { 0.0 }))), offset);
            if hovering {
                ui.ctx().request_repaint();
            }
            let c = zone.center() - Vec2::new(0.0, 26.0);
            let bob = (time * 4.0).sin() * 3.0 * ht;
            painter.circle_filled(c + Vec2::new(0.0, bob), 26.0, p.accent_soft);
            painter.text(c + Vec2::new(0.0, bob), egui::Align2::CENTER_CENTER, ic::FILE_UP, egui::FontId::proportional(24.0), p.accent);
            painter.text(zone.center() + Vec2::new(0.0, 16.0), egui::Align2::CENTER_CENTER, if hovering { "Release to encrypt" } else { "Drag files or folders here to encrypt them" }, egui::FontId::new(15.5, theme::semibold()), p.text);
            painter.text(
                zone.center() + Vec2::new(0.0, 40.0),
                egui::Align2::CENTER_CENTER,
                "or click to browse  ·  drag any card back out to get a decrypted copy",
                egui::FontId::proportional(12.5),
                p.weak,
            );
            if zresp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                && let Some(paths) = rfd::FileDialog::new().pick_files()
            {
                self.import_paths(paths);
            }
            ui.add_space(10.0);

            // ── options
            let mut changed = false;
            ui.horizontal(|ui| {
                changed |= w::toggle(ui, p, &mut self.vault.settings.import_text_as_notes).changed();
                ui.label(RichText::new("Turn text files (.md .txt .json .csv, code) into secure notes").size(13.0));
                ui.add_space(18.0);
                changed |= w::toggle(ui, p, &mut self.vault.settings.move_originals).changed();
                ui.label(RichText::new("Move: securely delete the originals after encrypting").size(13.0));
            });
            if changed {
                let _ = self.vault.save_settings();
            }
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                self.active_filter_chips(ui, p);
            });
            ui.add_space(10.0);

            // ── grid of encrypted files
            if self.vault.files.is_empty() {
                ui.add_space(30.0);
                w::empty_state(ui, p, ic::FOLDER_LOCK, "No files yet", "Anything you drop above is encrypted with your vault key before it's saved.");
                return;
            }
            let mut files: Vec<vault::VaultFile> = self.vault.files.iter().filter(|f| self.in_folder_filter(&f.meta.folder) && self.has_tags(&f.meta.tags)).cloned().collect();
            files.sort_by_key(|f| std::cmp::Reverse(f.meta.created));
            if files.is_empty() {
                ui.add_space(20.0);
                w::empty_state(ui, p, ic::FOLDER_OPEN, "No files here", "Drop files above. They'll be added to the selected folder.");
                return;
            }
            let tag_suggestions: Vec<String> = self.vault.all_tags().into_iter().map(|(t, _)| t).collect();
            let folder_options = self.folder_options();
            let gap = 14.0;
            let avail = ui.available_width();
            let cols = ((avail + gap) / (220.0 + gap)).floor().max(1.0) as usize;
            let card_w = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
            let card = Vec2::new(card_w, 196.0);
            for chunk in files.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for f in chunk {
                        let (icon, tone) = Self::file_icon(&f.meta.name);
                        let date = chrono::DateTime::from_timestamp(f.meta.created, 0)
                            .map(|d| d.with_timezone(&chrono::Local).format("%b %d, %Y").to_string())
                            .unwrap_or_default();
                        let info = format!("{} · {date} · {} {}", transfer::human_size(f.meta.size), ic::EYE, f.meta.views);
                        let folder = f.meta.folder.as_ref().map(|x| self.vault.folder_path(x)).unwrap_or_default();
                        let cid = egui::Id::unique("softvault-file-card").with(&f.id);
                        let (resp, actions) = w::file_card(ui, p, cid, icon, tone(p), &f.meta.name, &info, &folder, &f.meta.tags, card);
                        resp.dnd_set_drag_payload(DragRef::File(f.id.clone()));
                        let resp = resp.on_hover_text("Drag onto a folder to move · drag outside the window for a decrypted copy · right-click for more");
                        // right-click: move to folder / edit tags
                        let fid = f.id.clone();
                        egui::Popup::context_menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                            ui.set_min_width(240.0);
                            ui.spacing_mut().button_padding = Vec2::new(10.0, 7.0);
                            ui.menu_button(format!("{}   Move to folder", ic::FOLDER_INPUT), |ui| {
                                ui.set_min_width(200.0);
                                if ui.button(format!("{}  No folder", ic::LAYERS)).clicked() {
                                    self.move_to_folder(&DragRef::File(fid.clone()), None);
                                    ui.close();
                                }
                                for (id, name, depth) in &folder_options {
                                    if ui.button(format!("{}{}  {name}", "    ".repeat(*depth), ic::FOLDER)).clicked() {
                                        self.move_to_folder(&DragRef::File(fid.clone()), Some(id.clone()));
                                        ui.close();
                                    }
                                }
                            });
                            ui.separator();
                            ui.label(RichText::new(format!("{}  Tags", ic::TAG)).size(12.5).color(p.weak));
                            let mut tags = self.vault.files.iter().find(|x| x.id == fid).map(|x| x.meta.tags.clone()).unwrap_or_default();
                            if w::tag_editor(ui, p, &format!("softvault-file-tags-{fid}"), &mut tags, &tag_suggestions) {
                                let _ = self.vault.update_file(&fid, |m| m.tags = tags);
                            }
                        });
                        let mut a = ui.new_child(egui::UiBuilder::new().id_salt(("softvault-file-actions", &f.id)).max_rect(actions).layout(Layout::left_to_right(Align::Center)));
                        a.spacing_mut().item_spacing.x = 6.0;
                        if w::ghost(&mut a, p, &format!("{}  Save", ic::DOWNLOAD)).on_hover_text("Save a decrypted copy…").clicked() {
                            self.save_decrypted(DragSource::File(f.id.clone()));
                        }
                        a.with_layout(Layout::right_to_left(Align::Center), |a| {
                            let dropping = self
                                .file_deleting
                                .as_ref()
                                .filter(|(d, _)| *d == f.id)
                                .map(|(_, t)| (t.elapsed().as_secs_f32() / DELETE_ANIM_SECS).min(1.0));
                            let armed = self.file_confirm.as_deref() == Some(&f.id);
                            let r = w::trash_button(a, p, &format!("softvault-file-del-{}", f.id), armed, dropping);
                            if dropping.is_none() && r.clicked() {
                                if armed {
                                    self.file_deleting = Some((f.id.clone(), Instant::now()));
                                } else {
                                    self.file_confirm = Some(f.id.clone());
                                }
                            }
                            if dropping == Some(1.0) {
                                match self.vault.delete_file(&f.id) {
                                    Ok(()) => self.toast(format!("“{}” deleted", f.meta.name), true),
                                    Err(e) => self.toast(e.to_string(), false),
                                }
                                self.file_deleting = None;
                                self.file_confirm = None;
                            }
                        });
                        let _ = resp;
                    }
                });
                ui.add_space(gap);
            }
            ui.add_space(6.0);
            ui.label(RichText::new(format!(
                "{}  Dragging out writes a decrypted copy where you drop it. The temporary copy SoftVault hands to Windows is overwritten and deleted after 2 minutes.",
                ic::INFO
            )).size(12.5).color(p.weak));
        });
    }

    /// Label that follows the cursor during an internal drag; dragging past the
    /// window edge turns it into a native "drag out" (decrypted copy).
    fn ui_drag_ghost(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(payload) = egui::DragAndDrop::payload::<DragRef>(ctx) else { return };
        let (pos, down) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        let screen = ctx.content_rect();
        let outside = down && pos.is_none_or(|p| !screen.shrink(2.0).contains(p));
        if outside {
            let src = match &*payload {
                DragRef::Item(id) => self.vault.items.iter().find(|i| &i.id == id).filter(|i| i.data.kind == Kind::Note).map(|_| DragSource::Note(id.clone())),
                DragRef::File(id) => Some(DragSource::File(id.clone())),
                DragRef::Folder(_) => None,
            };
            egui::DragAndDrop::clear_payload(ctx);
            match src {
                Some(src) => self.drag_out = Some(src),
                None => self.toast("Only notes and files can be dragged out of the vault.", false),
            }
            return;
        }
        let Some(pos) = pos else { return };
        let (icon, name) = match &*payload {
            DragRef::Item(id) => self
                .vault
                .items
                .iter()
                .find(|i| &i.id == id)
                .map(|i| (if i.data.kind == Kind::Note { i.data.format.icon() } else { ic::KEY_ROUND }, i.data.title.clone()))
                .unwrap_or((ic::FILE, String::new())),
            DragRef::File(id) => self.vault.files.iter().find(|f| &f.id == id).map(|f| (ic::FILE, f.meta.name.clone())).unwrap_or((ic::FILE, String::new())),
            DragRef::Folder(id) => (ic::FOLDER, self.vault.folders.iter().find(|f| &f.id == id).map(|f| f.name.clone()).unwrap_or_default()),
        };
        let hint = match &*payload {
            DragRef::Folder(_) => "Drop on a folder to nest it",
            DragRef::Item(id) if self.vault.items.iter().any(|i| &i.id == id && i.data.kind == Kind::Password) => "Drop on a folder to move",
            _ => "Drop on a folder · drag outside to export",
        };
        egui::Area::new(egui::Id::unique("softvault-drag-ghost"))
            .fixed_pos(pos + Vec2::new(14.0, 10.0))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.accent))
                    .corner_radius(theme::R_INPUT)
                    .inner_margin(Margin::symmetric(10, 6))
                    .shadow(theme::soft_shadow(p))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).color(p.accent).size(14.0));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(w::h(name, 13.0));
                                ui.label(RichText::new(hint).size(11.5).color(p.weak));
                            });
                        });
                    });
            });
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    /// Full-window overlay while files are dragged over SoftVault.
    fn ui_drop_overlay(&mut self, ctx: &egui::Context, p: &Palette) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let t = ctx.animate_bool_with_time(egui::Id::unique("softvault-drop-overlay"), hovering, 0.18);
        if t <= 0.0 || self.view == View::Files {
            return;
        }
        let n = ctx.input(|i| i.raw.hovered_files.len());
        let screen = ctx.content_rect();
        {
            {
                let painter = &ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("softvault-drop-overlay-layer")));
                painter.rect_filled(screen, CornerRadius::ZERO, p.bg.gamma_multiply(0.86 * t));
                let zone = screen.shrink(28.0);
                let time = ctx.input(|i| i.time) as f32;
                painter.rect_filled(zone, theme::R_CARD, p.accent_soft.gamma_multiply(t));
                w::dashed_rounded_rect(painter, zone, 16.0, Stroke::new(2.0, p.accent.gamma_multiply(t)), time * 30.0);
                let c = zone.center() - Vec2::new(0.0, 30.0);
                let bob = (time * 4.0).sin() * 4.0;
                painter.circle_filled(c + Vec2::new(0.0, bob), 40.0, p.accent.gamma_multiply(t));
                painter.text(c + Vec2::new(0.0, bob), egui::Align2::CENTER_CENTER, ic::LOCK_KEYHOLE, egui::FontId::proportional(36.0), p.on_accent.gamma_multiply(t));
                let title = format!("Drop to encrypt {} item{} into your vault", n.max(1), if n == 1 { "" } else { "s" });
                painter.text(zone.center() + Vec2::new(0.0, 34.0), egui::Align2::CENTER_CENTER, title, egui::FontId::new(20.0, theme::semibold()), p.text.gamma_multiply(t));
                let sub = if self.vault.settings.import_text_as_notes {
                    "Text files become secure notes · other files go to Secure files"
                } else {
                    "Files are stored encrypted in Secure files"
                };
                let sub = if self.vault.settings.move_originals { format!("{sub} · originals will be securely deleted") } else { sub.to_string() };
                painter.text(zone.center() + Vec2::new(0.0, 62.0), egui::Align2::CENTER_CENTER, sub, egui::FontId::proportional(13.5), p.weak.gamma_multiply(t));
            }
        }
        ctx.request_repaint();
    }

    // ─────────────────────────── vault health ───────────────────────────

    fn health(&self) -> Health {
        let mut h = Health::default();
        let mut by_pw: std::collections::HashMap<&str, Vec<String>> = std::collections::HashMap::new();
        let year_ago = vault::now_ts() - 365 * 24 * 3600;
        for it in self.vault.items.iter().filter(|i| i.data.kind == Kind::Password) {
            h.logins += 1;
            if it.data.password.is_empty() {
                continue;
            }
            let (score, _, _) = generator::strength(&it.data.password);
            if score <= 1 {
                h.weak.push(it.id.clone());
            }
            by_pw.entry(it.data.password.as_str()).or_default().push(it.id.clone());
            if it.data.updated > 0 && it.data.updated < year_ago {
                h.old.push(it.id.clone());
            }
            if it.data.totp.trim().is_empty() {
                h.no_2fa += 1;
            }
        }
        for ids in by_pw.into_values() {
            if ids.len() > 1 {
                h.reused.extend(ids);
            }
        }
        h
    }

    fn ui_health(&mut self, ui: &mut egui::Ui, p: &Palette) {
        w::page_header(ui, p, ic::SHIELD_CHECK, "Vault health", "Find weak, reused and old passwords");
        ui.add_space(18.0);
        let h = self.health();
        let score = if h.logins == 0 {
            100
        } else {
            let bad = (h.weak.len() + h.reused.len() + h.old.len()).min(h.logins * 3) as f32;
            (100.0 - bad / (h.logins as f32 * 3.0) * 100.0).round() as i32
        };
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(1000.0));
            let tw = (ui.available_width() - 3.0 * 12.0) / 4.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let tone = if score >= 80 { p.success } else if score >= 50 { p.warn } else { p.danger };
                w::stat_tile(ui, p, ic::GAUGE, &format!("{score}%"), "Health score", tone, tw);
                w::stat_tile(ui, p, ic::SHIELD_ALERT, &h.weak.len().to_string(), "Weak passwords", if h.weak.is_empty() { p.success } else { p.danger }, tw);
                w::stat_tile(ui, p, ic::REPEAT, &h.reused.len().to_string(), "Reused passwords", if h.reused.is_empty() { p.success } else { p.warn }, tw);
                w::stat_tile(ui, p, ic::HISTORY, &h.old.len().to_string(), "Unchanged 1 yr+", if h.old.is_empty() { p.success } else { p.warn }, tw);
            });
            ui.add_space(16.0);
            let sections: [(&str, &str, &str, &Vec<String>); 3] = [
                (ic::SHIELD_ALERT, "Weak passwords", "Easy to guess. Replace them with generated passwords.", &h.weak),
                (ic::REPEAT, "Reused passwords", "One breach exposes every account that shares a password.", &h.reused),
                (ic::HISTORY, "Old passwords", "Not updated in more than a year.", &h.old),
            ];
            for (icon, title, desc, ids) in sections {
                card(ui, p, |ui| {
                    w::section_title(ui, p, icon, title, desc);
                    if ids.is_empty() {
                        ui.label(RichText::new(format!("{}  Nothing to fix here", ic::CIRCLE_CHECK)).color(p.success));
                        return;
                    }
                    for (i, id) in ids.iter().enumerate() {
                        let Some(it) = self.vault.items.iter().find(|x| &x.id == id) else { continue };
                        let (t, u) = (it.data.title.clone(), it.data.username.clone());
                        let (sc, lb, _) = generator::strength(&it.data.password);
                        w::setting_row(ui, p, &t, &u, i + 1 == ids.len(), |ui| {
                            if w::ghost(ui, p, &format!("{}  Fix", ic::PENCIL)).clicked() {
                                self.select(id);
                            }
                            let col = if sc <= 1 { p.danger } else if sc == 2 { p.warn } else { p.success };
                            ui.label(RichText::new(lb).size(12.5).color(col).family(theme::semibold()));
                        });
                    }
                });
                ui.add_space(12.0);
            }
            ui.label(RichText::new(format!("{}  {} of {} logins have no authenticator key saved.", ic::INFO, h.no_2fa, h.logins)).size(12.5).color(p.weak));
        });
    }

    fn ui_generator(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        w::page_header(ui, p, ic::DICES, "Generator", "Create strong, random passwords");
        ui.add_space(14.0);
        if self.gen_preview.is_empty() {
            self.gen_preview = Zeroizing::new(generator::generate(&self.gen_opts));
        }
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(760.0));
            ui.spacing_mut().item_spacing.y = 12.0;
            card(ui, p, |ui| {
                Frame::new().fill(p.bg_alt).corner_radius(theme::R_INPUT).inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let mut job = egui::text::LayoutJob::default();
                    for ch in self.gen_preview.chars() {
                        let color = if ch.is_ascii_digit() {
                            p.accent
                        } else if ch.is_ascii_alphabetic() {
                            p.text
                        } else {
                            p.danger
                        };
                        job.append(&ch.to_string(), 0.0, egui::TextFormat { font_id: egui::FontId::monospace(20.0), color, ..Default::default() });
                    }
                    job.wrap.max_width = ui.available_width();
                    ui.label(job);
                });
                let (sc, lb, bits) = generator::strength(&self.gen_preview);
                w::strength_bar(ui, p, sc, lb, bits);
                ui.horizontal(|ui| {
                    if w::primary(ui, p, "\u{E145}  Regenerate").clicked() {
                        self.gen_preview = Zeroizing::new(generator::generate(&self.gen_opts));
                    }
                    if w::ghost(ui, p, "\u{E09E}  Copy").clicked() {
                        let s = self.gen_preview.to_string();
                        self.copy(&ctx, &s, "Password");
                    }
                });
            });
            card(ui, p, |ui| {
                w::section_title(ui, p, "\u{E154}", "Options", "Uses your operating system's cryptographically secure random generator");
                let mut changed = ui.add(egui::Slider::new(&mut self.gen_opts.length, 8..=128).text("Length")).changed();
                ui.horizontal_wrapped(|ui| {
                    changed |= ui.checkbox(&mut self.gen_opts.upper, "A-Z").changed();
                    changed |= ui.checkbox(&mut self.gen_opts.lower, "a-z").changed();
                    changed |= ui.checkbox(&mut self.gen_opts.digits, "0-9").changed();
                    changed |= ui.checkbox(&mut self.gen_opts.symbols, "!@#$%^&*").changed();
                    changed |= ui.checkbox(&mut self.gen_opts.avoid_ambiguous, "Avoid ambiguous characters").changed();
                });
                if changed {
                    self.gen_preview = Zeroizing::new(generator::generate(&self.gen_opts));
                }
            });
        });
    }

    // ───────────────────── secure note editor + rich preview ─────────────────────

    fn note_editor_id() -> egui::Id {
        egui::Id::unique("softvault-note-editor")
    }

    fn note_card(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context, ed: &mut Editor, h: f32) {
        card(ui, p, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(w::h(format!("{}  Note", ic::STICKY_NOTE), 15.0));
                ui.add_space(8.0);
                egui::ComboBox::from_id_salt("softvault-note-format")
                    .selected_text(ed.data.format.label())
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for f in NoteFormat::ALL {
                            ui.selectable_value(&mut ed.data.format, f, f.label());
                        }
                    });
                if ed.data.format == NoteFormat::Code {
                    ui.add(egui::TextEdit::singleline(&mut ed.data.lang).hint_text("rust, py, js, sql, sh…").desired_width(150.0).margin(Margin::symmetric(8, 6)));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if w::primary(ui, p, "\u{E113}  Full screen").on_hover_text("Open a distraction-free, full-window preview").clicked() {
                        self.open_fullscreen(ed.id.clone());
                    }
                    ui.add_space(6.0);
                    if ed.data.format.has_preview() {
                        for (m, label) in [(NoteMode::Preview, "\u{E05F} Preview"), (NoteMode::Split, "\u{E098} Split"), (NoteMode::Edit, "\u{E1F9} Edit")] {
                            if w::chip(ui, p, label, ed.note_mode == m).clicked() {
                                ed.note_mode = m;
                            }
                        }
                    }
                });
            });

            let fmt = ed.data.format;
            let mode = if fmt.has_preview() { ed.note_mode } else { NoteMode::Edit };
            if mode != NoteMode::Preview && fmt == NoteFormat::Markdown {
                Self::md_toolbar(ui, p, ctx, ed);
            }
            match mode {
                NoteMode::Edit => Self::note_editor(ui, p, ed, h),
                NoteMode::Preview => Self::note_preview(ui, p, ed, h + 60.0),
                NoteMode::Split => {
                    ui.columns(2, |cols| {
                        Self::note_editor(&mut cols[0], p, ed, h);
                        Self::note_preview(&mut cols[1], p, ed, h);
                    });
                }
            }

            ui.horizontal(|ui| {
                if w::ghost(ui, p, "\u{E09E}  Copy").clicked() {
                    let n = ed.data.notes.clone();
                    self.copy(ctx, &n, "Note");
                }
                if fmt == NoteFormat::Json && w::ghost(ui, p, "\u{E412}  Format JSON").clicked() {
                    match render::pretty_json(&ed.data.notes) {
                        Some(pretty) => ed.data.notes = pretty,
                        None => self.toast("Can't format: the JSON is invalid", false),
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let words = ed.data.notes.split_whitespace().count();
                    let lines = ed.data.notes.lines().count();
                    ui.label(RichText::new(format!("{lines} lines · {words} words · {} chars", ed.data.notes.chars().count())).size(12.0).color(p.weak));
                });
            });
        });
    }

    fn note_editor(ui: &mut egui::Ui, p: &Palette, ed: &mut Editor, h: f32) {
        let fmt = ed.data.format;
        let syntax = fmt.editor_syntax(&ed.data.lang);
        let pal = *p;
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
            let job = render::editor_layout(ui, &pal, buf.as_str(), syntax.as_deref().unwrap_or("txt"), wrap);
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };
        let hint = match fmt {
            NoteFormat::Markdown => "# Title\n\nWrite **Markdown**: lists, - [ ] tasks, `code`, tables…",
            NoteFormat::Code => "Paste or write code…",
            NoteFormat::Json => "{\n  \"key\": \"value\"\n}",
            NoteFormat::Csv => "name,email,role\nAlice,alice@example.com,admin",
            NoteFormat::Plain => "Write anything; it's encrypted before it's saved.",
        };
        ScrollArea::vertical().id_salt("softvault-note-editor-scroll").max_height(h).auto_shrink([false, true]).show(ui, |ui| {
            let rows = ((h - 20.0) / 18.0).max(6.0) as usize;
            let mut te = egui::TextEdit::multiline(&mut ed.data.notes)
                .id(Self::note_editor_id())
                .desired_width(f32::INFINITY)
                .desired_rows(rows)
                .hint_text(hint)
                .lock_focus(true)
                .margin(Margin::symmetric(12, 10));
            if syntax.is_some() {
                te = te.code_editor().layouter(&mut layouter);
            } else if fmt == NoteFormat::Csv {
                te = te.code_editor();
            }
            ui.add(te);
        });
    }

    fn note_preview(ui: &mut egui::Ui, p: &Palette, ed: &Editor, h: f32) {
        Frame::new()
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(theme::R_INPUT)
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ScrollArea::vertical().id_salt("softvault-note-preview-scroll").max_height(h - 24.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.set_min_height((h - 40.0).min(200.0));
                    render::render(ui, p, ed.data.format, &ed.data.notes, &ed.data.lang);
                });
            });
    }

    fn md_toolbar(ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context, ed: &mut Editor) {
        enum A {
            Wrap(&'static str, &'static str, &'static str),
            Prefix(&'static str),
            Insert(&'static str),
        }
        let tools: [(&str, &str, A); 11] = [
            ("\u{E05D}", "Bold", A::Wrap("**", "**", "bold")),
            ("\u{E0FB}", "Italic", A::Wrap("*", "*", "italic")),
            ("\u{E177}", "Strikethrough", A::Wrap("~~", "~~", "text")),
            ("\u{E384}", "Heading", A::Prefix("## ")),
            ("\u{E106}", "Bulleted list", A::Prefix("- ")),
            ("\u{E1D1}", "Numbered list", A::Prefix("1. ")),
            ("\u{E559}", "Task list", A::Prefix("- [ ] ")),
            ("\u{E239}", "Quote", A::Prefix("> ")),
            ("\u{E093}", "Inline code", A::Wrap("`", "`", "code")),
            ("\u{E102}", "Link", A::Wrap("[", "](https://)", "link text")),
            ("\u{E17D}", "Table", A::Insert("\n| Column | Value |\n|--------|-------|\n| a      | b     |\n")),
        ];
        Frame::new().fill(p.bg_alt).corner_radius(theme::R_INPUT).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let mut act: Option<&A> = None;
                for (icon, tip, a) in &tools {
                    let b = egui::Button::new(RichText::new(*icon).size(13.5).family(theme::semibold()))
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::NONE)
                        .min_size(Vec2::new(30.0, 26.0));
                    if ui.add(b).on_hover_text(*tip).clicked() {
                        act = Some(a);
                    }
                }
                let code_block = ui.add(egui::Button::new(RichText::new("\u{E36A}").size(14.0)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::new(30.0, 26.0))).on_hover_text("Code block");
                let rule = ui.add(egui::Button::new(RichText::new("\u{E11C}").size(14.0)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::new(30.0, 26.0))).on_hover_text("Divider");
                let a = if code_block.clicked() {
                    Some(&A::Insert("\n```\n\n```\n"))
                } else if rule.clicked() {
                    Some(&A::Insert("\n---\n"))
                } else {
                    act
                };
                let Some(a) = a else { return };

                let id = Self::note_editor_id();
                let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                let text = &mut ed.data.notes;
                let n = text.chars().count();
                let (s, e) = state
                    .cursor
                    .char_range()
                    .map(|r| {
                        let (x, y) = (r.primary.index.0, r.secondary.index.0);
                        (x.min(y).min(n), x.max(y).min(n))
                    })
                    .unwrap_or((n, n));
                let byte = |t: &str, ci: usize| t.char_indices().nth(ci).map(|(i, _)| i).unwrap_or(t.len());
                let (bs, be) = (byte(text, s), byte(text, e));
                let (sel_start, sel_end) = match a {
                    A::Wrap(l, r, placeholder) => {
                        let inner = if bs == be { placeholder.to_string() } else { text[bs..be].to_string() };
                        text.replace_range(bs..be, &format!("{l}{inner}{r}"));
                        let st = s + l.chars().count();
                        (st, st + inner.chars().count())
                    }
                    A::Prefix(pre) => {
                        let ls = text[..bs].rfind('\n').map(|i| i + 1).unwrap_or(0);
                        text.insert_str(ls, pre);
                        let c = e + pre.chars().count();
                        (c, c)
                    }
                    A::Insert(snip) => {
                        text.replace_range(bs..be, snip);
                        let c = s + snip.chars().count();
                        (c, c)
                    }
                };
                state.cursor.set_char_range(Some(egui::text_selection::CCursorRange::two(
                    egui::text::CCursor::new(sel_start),
                    egui::text::CCursor::new(sel_end),
                )));
                state.store(ctx, id);
                ctx.memory_mut(|m| m.request_focus(id));
            });
        });
    }

    // ─────────────────────────── full-screen note reader ───────────────────────────

    fn open_fullscreen(&mut self, item_id: Option<String>) {
        if self.editor.is_none()
            && let Some(id) = &item_id
        {
            let _ = self.vault.record_item_view(id);
        }
        self.full = Some(FullView { item_id, zoom: 1.0, os_fullscreen: false, show_toc: true });
    }

    fn close_fullscreen(&mut self, ctx: &egui::Context) {
        if let Some(fv) = self.full.take() {
            if fv.os_fullscreen {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            if fv.zoom != 1.0 {
                ctx.set_zoom_factor(1.0);
            }
        }
    }

    fn ui_fullscreen(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut fv) = self.full.take() else { return };
        // source: the open editor (shows unsaved edits) or the saved item
        let data = match &self.editor {
            Some(ed) if ed.data.kind == Kind::Note => Some(&ed.data),
            _ => fv.item_id.as_ref().and_then(|id| self.vault.items.iter().find(|i| &i.id == id)).map(|i| &i.data),
        };
        let Some(data) = data else {
            self.full = Some(fv);
            self.close_fullscreen(ctx);
            return;
        };

        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let mut copy = false;
        let mut toggle_os = ctx.input(|i| i.key_pressed(egui::Key::F11));
        let mut new_zoom = fv.zoom;
        let screen = ctx.content_rect();
        let is_md = data.format == NoteFormat::Markdown;
        let toc = if is_md { render::headings(&data.notes) } else { vec![] };
        let words = data.notes.split_whitespace().count();

        let fs_id = egui::Id::unique("softvault-fullscreen");
        egui::Modal::new(fs_id)
            .area(egui::Modal::default_area(fs_id).anchor(egui::Align2::LEFT_TOP, Vec2::ZERO).constrain(false))
            .backdrop_color(p.bg)
            .frame(Frame::new().fill(p.bg).inner_margin(Margin::ZERO).corner_radius(CornerRadius::ZERO))
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                ui.set_max_size(screen.size());
                ui.spacing_mut().item_spacing.y = 0.0;

                // ── top bar
                Frame::new().fill(p.card).inner_margin(Margin::symmetric(24, 12)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let g = ui.painter().layout_no_wrap(data.format.badge().into(), egui::FontId::proportional(12.0), p.text);
                        let (r, _) = ui.allocate_exact_size(g.size() + Vec2::new(16.0, 8.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, CornerRadius::same(6), p.bg_alt);
                        ui.painter().galley(r.min + Vec2::new(8.0, 4.0), g, p.text);
                        ui.label(w::h(if data.title.is_empty() { "(untitled)" } else { data.title.as_str() }, 18.0));
                        ui.label(RichText::new(format!("· {words} words · ≈{} min read", (words / 200).max(1))).size(12.5).color(p.weak));

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if w::primary(ui, p, "\u{E1B2}  Close").on_hover_text("Esc").clicked() {
                                close = true;
                            }
                            let os_label = if fv.os_fullscreen { "\u{E11B}  Exit full screen" } else { "\u{E113}  Full screen" };
                            if w::ghost(ui, p, os_label).on_hover_text("F11 — fill the whole monitor").clicked() {
                                toggle_os = true;
                            }
                            if w::ghost(ui, p, "\u{E09E}  Copy").clicked() {
                                copy = true;
                            }
                            ui.add_space(6.0);
                            if w::icon_btn(ui, p, "A+", "Larger text").clicked() {
                                new_zoom = (fv.zoom + 0.1).min(2.0);
                            }
                            ui.label(RichText::new(format!("{:.0}%", fv.zoom * 100.0)).size(12.5).color(p.weak));
                            if w::icon_btn(ui, p, "A−", "Smaller text").clicked() {
                                new_zoom = (fv.zoom - 0.1).max(0.7);
                            }
                            if !toc.is_empty() && w::chip(ui, p, "\u{E408} Contents", fv.show_toc).clicked() {
                                fv.show_toc = !fv.show_toc;
                            }
                        });
                    });
                });
                let (l, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(l, CornerRadius::ZERO, p.border);

                // ── body: optional contents sidebar + centered "paper"
                let body_h = ui.available_height();
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if fv.show_toc && toc.len() >= 2 {
                        Frame::new().fill(p.card).inner_margin(Margin::symmetric(16, 18)).show(ui, |ui| {
                            ui.set_width(240.0);
                            ui.set_min_height(body_h - 36.0);
                            ui.set_max_height(body_h - 36.0);
                            ui.vertical(|ui| ScrollArea::vertical().id_salt("softvault-fs-toc").show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(w::h("CONTENTS", 11.5).color(p.weak));
                                ui.add_space(6.0);
                                let min_level = toc.iter().map(|t| t.0).min().unwrap_or(1);
                                for (i, (lvl, text)) in toc.iter().enumerate() {
                                    let indent = (lvl - min_level) as f32 * 14.0;
                                    ui.horizontal(|ui| {
                                        ui.add_space(indent);
                                        let rt = if *lvl == min_level { w::h(text.as_str(), 13.5) } else { RichText::new(text.as_str()).size(13.0) };
                                        if ui.add(egui::Button::new(rt.color(p.text)).frame(false).wrap()).clicked() {
                                            render::request_scroll_to_heading(ui.ctx(), i);
                                        }
                                    });
                                }
                            }));
                        });
                        let (l, _) = ui.allocate_exact_size(Vec2::new(1.0, body_h), egui::Sense::hover());
                        ui.painter().rect_filled(l, CornerRadius::ZERO, p.border);
                    }
                    ui.vertical(|ui| {
                        ui.set_min_height(body_h);
                        ui.set_max_height(body_h);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ScrollArea::vertical().id_salt("softvault-fs-body").auto_shrink([false, false]).show(ui, |ui| {
                            let avail = ui.available_width();
                            let paper_w = (avail - 48.0).clamp(320.0, 900.0);
                            ui.add_space(28.0);
                            ui.horizontal(|ui| {
                                ui.add_space(((avail - paper_w) / 2.0).max(0.0));
                                Frame::new()
                                    .fill(p.card)
                                    .stroke(Stroke::new(1.0, p.border))
                                    .corner_radius(theme::R_CARD)
                                    .inner_margin(Margin::symmetric(48, 36))
                                    .show(ui, |ui| {
                                        ui.set_width(paper_w - 98.0);
                                        ui.vertical(|ui| {
                                            ui.spacing_mut().item_spacing.y = 8.0;
                                            render::render(ui, p, data.format, &data.notes, &data.lang);
                                        });
                                    });
                            });
                            ui.add_space(40.0);
                        });
                    });
                });
            });

        if toggle_os {
            fv.os_fullscreen = !fv.os_fullscreen;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(fv.os_fullscreen));
        }
        if (new_zoom - fv.zoom).abs() > f32::EPSILON {
            fv.zoom = new_zoom;
            ctx.set_zoom_factor(new_zoom);
        }
        let text = if copy { Some(data.notes.clone()) } else { None };
        self.full = Some(fv);
        if let Some(t) = text {
            self.copy(ctx, &t, "Note");
        }
        if close {
            self.close_fullscreen(ctx);
        }
    }

    /// Bitwarden-style item dialog.
    fn ui_editor_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut ed) = self.editor.take() else { return };
        let mut close = false;
        let mut save = false;
        let mut delete = false;
        let max_body = (ctx.content_rect().height() - 220.0).max(240.0);
        let folder_options = self.folder_options();
        let tag_suggestions: Vec<String> = self.vault.all_tags().into_iter().map(|(t, _)| t).collect();
        let is_note = ed.data.kind == Kind::Note;

        let fade = (ed.opened.elapsed().as_secs_f32() / 0.18).min(1.0);
        let fade = 1.0 - (1.0 - fade).powi(3);
        if fade < 1.0 {
            ctx.request_repaint();
        }
        egui::Modal::new(egui::Id::unique("softvault-item-dialog"))
            .backdrop_color(Color32::from_black_alpha(((if p.dark { 150.0 } else { 90.0 }) * fade) as u8))
            .frame(Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(theme::R_CARD).shadow(theme::soft_shadow(p)))
            .show(ctx, |ui| {
                ui.multiply_opacity(fade);
                ui.set_width(if is_note { (ctx.content_rect().width() - 120.0).clamp(620.0, 1120.0) } else { 620.0 });
                ui.spacing_mut().item_spacing.y = 0.0;
                // header
                Frame::new().inner_margin(Margin::symmetric(24, 16)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let kind = if is_note { "note" } else { "login" };
                        let title = if ed.id.is_none() { format!("New {kind}") } else { format!("Edit {kind}") };
                        w::icon_tile(ui, p, if is_note { ed.data.format.icon() } else { ic::KEY_ROUND }, 38.0);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(w::h(title, 18.0));
                            let sub = if is_note { format!("Secure note · {}", ed.data.format.label()) } else { "Login".to_string() };
                            ui.label(RichText::new(sub).size(12.5).color(p.weak));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new("\u{E1B2}").size(14.0).color(p.weak)).frame(false)).on_hover_text("Close").clicked() {
                                close = true;
                            }
                            ui.add_space(6.0);
                            let star = ic::STAR;
                            if ui.add(egui::Button::new(RichText::new(star).size(17.0).color(if ed.data.favorite { p.warn } else { p.weak })).frame(false)).on_hover_text("Favorite").clicked() {
                                ed.data.favorite = !ed.data.favorite;
                            }
                        });
                    });
                });
                let (l, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(l, CornerRadius::ZERO, p.border);

                // body (bg-alt with white sections)
                Frame::new().fill(p.bg_alt).inner_margin(Margin::symmetric(20, 16)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ScrollArea::vertical().max_height(max_body).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 12.0;
                        card(ui, p, |ui| {
                            ui.spacing_mut().item_spacing.y = 6.0;
                            ui.label(w::h(format!("{}  Item details", ic::INFO), 15.0));
                            w::label(ui, p, "Item name (required)");
                            w::text_input(ui, &mut ed.data.title, if is_note { "e.g. Recovery codes" } else { "e.g. GitHub" }, false);
                            ui.add_space(2.0);
                            ui.columns(2, |c| {
                                w::label(&mut c[0], p, "Folder");
                                let current = ed.data.folder.as_ref().map(|f| self.vault.folder_path(f)).unwrap_or_else(|| "No folder".into());
                                egui::ComboBox::from_id_salt("softvault-item-folder")
                                    .width(c[0].available_width())
                                    .selected_text(format!("{}  {current}", ic::FOLDER))
                                    .show_ui(&mut c[0], |ui| {
                                        ui.selectable_value(&mut ed.data.folder, None, "No folder");
                                        for (id, name, depth) in &folder_options {
                                            ui.selectable_value(&mut ed.data.folder, Some(id.clone()), format!("{}{}  {name}", "    ".repeat(*depth), ic::FOLDER));
                                        }
                                    });
                                w::label(&mut c[1], p, "Tags");
                                w::tag_editor(&mut c[1], p, "softvault-item-tags", &mut ed.data.tags, &tag_suggestions);
                            });
                        });

                        if !is_note {
                            card(ui, p, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label(w::h(format!("{}  Login credentials", ic::KEY_ROUND), 15.0));
                                w::label(ui, p, "Username");
                                w::row_rtl(ui, |ui| {
                                    if w::icon_btn(ui, p, "\u{E09E}", "Copy username").clicked() {
                                        let u = ed.data.username.clone();
                                        self.copy(ctx, &u, "Username");
                                    }
                                    ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut ed.data.username).hint_text("you@example.com").margin(Margin::symmetric(12, 9)));
                                });
                                w::label(ui, p, "Password");
                                w::row_rtl(ui, |ui| {
                                    if w::icon_btn(ui, p, "\u{E145}", "Generate password").clicked() {
                                        ed.show_gen = !ed.show_gen;
                                    }
                                    if w::icon_btn(ui, p, "\u{E09E}", "Copy password").clicked() {
                                        let pw = ed.data.password.clone();
                                        self.copy(ctx, &pw, "Password");
                                    }
                                    if w::icon_btn(ui, p, if ed.show_pw { "\u{E0BB}" } else { "\u{E0BA}" }, "Toggle visibility").clicked() {
                                        ed.show_pw = !ed.show_pw;
                                    }
                                    ui.add_sized(
                                        [ui.available_width(), 36.0],
                                        egui::TextEdit::singleline(&mut ed.data.password).password(!ed.show_pw).font(egui::TextStyle::Monospace).margin(Margin::symmetric(12, 9)),
                                    );
                                });
                                let (sc, lb, bits) = generator::strength(&ed.data.password);
                                w::strength_bar(ui, p, sc, lb, bits);
                                if ed.show_gen {
                                    Frame::new().fill(p.bg_alt).corner_radius(theme::R_INPUT).inner_margin(Margin::same(12)).show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.add(egui::Slider::new(&mut self.gen_opts.length, 8..=96).text("Length"));
                                        ui.horizontal_wrapped(|ui| {
                                            ui.checkbox(&mut self.gen_opts.upper, "A-Z");
                                            ui.checkbox(&mut self.gen_opts.lower, "a-z");
                                            ui.checkbox(&mut self.gen_opts.digits, "0-9");
                                            ui.checkbox(&mut self.gen_opts.symbols, "!@#");
                                            ui.checkbox(&mut self.gen_opts.avoid_ambiguous, "Avoid ambiguous");
                                        });
                                        if w::primary(ui, p, "Use new password").clicked() {
                                            ed.data.password = generator::generate(&self.gen_opts);
                                            ed.show_pw = true;
                                        }
                                    });
                                }
                                w::label(ui, p, "Authenticator key (TOTP)");
                                w::row_rtl(ui, |ui| {
                                    if let Some(sec) = totp::base32_decode(&ed.data.totp).filter(|s| s.len() >= 10) {
                                        let code = format!("{:06}", totp::hotp(&sec, totp::now_step()));
                                        if w::icon_btn(ui, p, "\u{E09E}", "Copy verification code").clicked() {
                                            self.copy(ctx, &code, "Verification code");
                                        }
                                    }
                                    ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut ed.data.totp).hint_text("Base32 key, e.g. JBSWY3DPEHPK3PXP").password(true).margin(Margin::symmetric(12, 9)));
                                });
                                if let Some(sec) = totp::base32_decode(&ed.data.totp).filter(|s| s.len() >= 10) {
                                    let code = format!("{:06}", totp::hotp(&sec, totp::now_step()));
                                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                                    let left = totp::PERIOD - secs % totp::PERIOD;
                                    ui.label(RichText::new(format!("{} {}   \u{E1E0} {left}s", &code[..3], &code[3..])).monospace().size(19.0).color(p.accent));
                                }
                            });
                            card(ui, p, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label(w::h(format!("{}  Autofill options", ic::GLOBE), 15.0));
                                w::label(ui, p, "Website (URI)");
                                w::text_input(ui, &mut ed.data.url, "https://", false);
                            });
                        }

                        if is_note {
                            self.note_card(ui, p, ctx, &mut ed, (max_body - 300.0).max(220.0));
                        } else {
                            card(ui, p, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label(w::h(format!("{}  Additional options", ic::STICKY_NOTE), 15.0));
                                w::label(ui, p, "Notes");
                                ui.add(egui::TextEdit::multiline(&mut ed.data.notes).desired_rows(4).desired_width(f32::INFINITY).margin(Margin::symmetric(12, 10)));
                            });
                        }

                        if ed.data.created > 0 {
                            let fmt = |t: i64| {
                                chrono::DateTime::from_timestamp(t, 0)
                                    .map(|d| d.with_timezone(&chrono::Local).format("%b %d, %Y, %H:%M").to_string())
                                    .unwrap_or_default()
                            };
                            let viewed = if ed.data.last_viewed > 0 { format!("    {}  Viewed {} time{} · last {}", ic::EYE, ed.data.views, if ed.data.views == 1 { "" } else { "s" }, fmt(ed.data.last_viewed)) } else { String::new() };
                            ui.label(RichText::new(format!("Created: {}    Last edited: {}{viewed}", fmt(ed.data.created), fmt(ed.data.updated))).size(12.0).color(p.weak));
                        }
                    });
                });

                let (l, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(l, CornerRadius::ZERO, p.border);
                // footer
                Frame::new().inner_margin(Margin::symmetric(24, 14)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if w::primary(ui, p, "Save").clicked() || ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
                            save = true;
                        }
                        if w::ghost(ui, p, "Cancel").clicked() {
                            close = true;
                        }
                        if let Some(item_id) = ed.id.clone() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let dropping = self
                                    .deleting
                                    .as_ref()
                                    .filter(|(d, _)| *d == item_id)
                                    .map(|(_, t)| (t.elapsed().as_secs_f32() / DELETE_ANIM_SECS).min(1.0));
                                let r = w::trash_button(ui, p, "softvault-delete", ed.confirm_delete, dropping);
                                if dropping.is_none() {
                                    if r.clicked() {
                                        if ed.confirm_delete {
                                            self.deleting = Some((item_id.clone(), Instant::now()));
                                        } else {
                                            ed.confirm_delete = true;
                                        }
                                    }
                                    if ed.confirm_delete && ui.add(egui::Button::new(RichText::new("Keep").color(p.weak)).frame(false)).clicked() {
                                        ed.confirm_delete = false;
                                    }
                                } else if dropping == Some(1.0) {
                                    delete = true;
                                }
                            });
                        }
                    });
                });
            });
        if self.full.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }

        if save {
            if ed.data.title.trim().is_empty() {
                self.toast("Item name is required", false);
            } else {
                match self.vault.save_item(ed.id.clone(), ed.data.clone()) {
                    Ok(id) => {
                        self.selected = Some(id);
                        self.toast("Item saved", true);
                        close = true;
                    }
                    Err(e) => self.toast(format!("Save failed: {e}"), false),
                }
            }
        }
        if delete && let Some(id) = ed.id.clone() {
            self.deleting = None;
            let backup = self.vault.items.iter().find(|i| i.id == id).map(|i| i.data.clone());
            match self.vault.delete_item(&id) {
                Ok(()) => {
                    if let Some(data) = backup {
                        let title = if data.title.is_empty() { "Item".to_string() } else { data.title.clone() };
                        self.undo = Some(UndoDelete { id, data, title, until: Instant::now() + Duration::from_secs(UNDO_SECS) });
                        self.toast = None;
                    }
                    self.selected = None;
                    return;
                }
                Err(e) => self.toast(format!("Delete failed: {e}"), false),
            }
        }
        if close {
            self.selected = None;
        } else {
            self.editor = Some(ed);
        }
    }

    // ─────────────────────────── settings ───────────────────────────

    fn ui_settings(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        if !self.su.loaded {
            self.su.loaded = true;
            self.su.hint = self.vault.hint().unwrap_or_default();
            if let Some(r) = self.vault.recovery() {
                self.su.email = r.email.clone();
            }
        }
        w::page_header(ui, p, ic::SETTINGS, "Settings", "Security, recovery, appearance and backups");
        ui.add_space(14.0);
        let tabs = [
            (ic::SHIELD_CHECK, "Security"),
            (ic::MAIL, "Recovery"),
            (ic::PALETTE, "Appearance"),
            (ic::HARD_DRIVE, "Backup & storage"),
            (ic::INFO, "About"),
        ];
        if let Some(t) = w::tabs(ui, p, &tabs, self.su.tab) {
            self.su.tab = t;
        }

        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(880.0));
            ui.spacing_mut().item_spacing.y = 14.0;
            match self.su.tab {
                0 => self.settings_security(ui, p, &ctx),
                1 => self.settings_recovery(ui, p, &ctx),
                2 => self.settings_appearance(ui, p),
                3 => self.settings_backup(ui, p, &ctx),
                _ => self.settings_about(ui, p),
            }
            ui.add_space(24.0);
        });
    }

    fn settings_security(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context) {
        // ── 2FA
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::SMARTPHONE, "Two-step login", "Ask for a code from your phone's authenticator app on every unlock (optional).");
            if self.vault.settings.mfa_secret.is_some() {
                w::setting_row(ui, p, "Status", "A 6-digit code is required after your master password.", false, |ui| {
                    ui.label(RichText::new(format!("{}  Enabled", ic::CIRCLE_CHECK)).color(p.success).family(theme::semibold()));
                });
                w::setting_row(ui, p, "Turn off two-step login", "Enter a current code from your authenticator app to confirm.", true, |ui| {
                    if w::danger(ui, p, "Turn off").clicked() {
                        let sec = self.vault.settings.mfa_secret.clone().unwrap_or_default();
                        if let Some(step) = totp::verify(&sec, &self.su.mfa_code, self.vault.settings.mfa_last_step) {
                            self.vault.settings.mfa_secret = None;
                            self.vault.settings.mfa_last_step = step;
                            let _ = self.vault.save_settings();
                            self.toast("Two-step login turned off", true);
                        } else {
                            self.toast("Invalid code", false);
                        }
                        self.su.mfa_code.clear();
                    }
                    ui.add_sized([130.0, 36.0], egui::TextEdit::singleline(&mut self.su.mfa_code).hint_text("123456").margin(Margin::symmetric(10, 9)));
                });
            } else if let Some(secret) = self.su.mfa_setup.clone() {
                ui.horizontal_top(|ui| {
                    Frame::new().fill(Color32::WHITE).corner_radius(theme::R_INPUT).inner_margin(Margin::same(6)).show(ui, |ui| {
                        w::draw_qr(ui, &totp::otpauth_uri(&secret, "vault"), 176.0);
                    });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.label(w::h("1.  Scan the QR code", 14.0));
                        ui.label(RichText::new("Google Authenticator, Microsoft Authenticator, Authy, 2FAS or Aegis.").size(12.5).color(p.weak));
                        ui.label(RichText::new("Can't scan? Enter this key manually:").size(12.5).color(p.weak));
                        let grouped: String = secret.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join(" ");
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(grouped).monospace().size(13.0));
                            if w::ghost_icon(ui, p, ic::COPY, "Copy key").clicked() {
                                self.copy(ctx, &secret, "Setup key");
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(w::h("2.  Enter the 6-digit code", 14.0));
                        ui.horizontal(|ui| {
                            ui.add_sized([130.0, 36.0], egui::TextEdit::singleline(&mut self.su.mfa_code).hint_text("123456").margin(Margin::symmetric(10, 9)));
                            if w::primary(ui, p, "Turn on").clicked() {
                                if let Some(step) = totp::verify(&secret, &self.su.mfa_code, 0) {
                                    self.vault.settings.mfa_secret = Some(secret.clone());
                                    self.vault.settings.mfa_last_step = step;
                                    match self.vault.save_settings() {
                                        Ok(()) => self.toast("Two-step login turned on", true),
                                        Err(e) => self.toast(e.to_string(), false),
                                    }
                                    self.su.mfa_setup = None;
                                } else {
                                    self.toast("Code doesn't match. Check your phone's clock.", false);
                                }
                                self.su.mfa_code.clear();
                            }
                            if w::ghost(ui, p, "Cancel").clicked() {
                                self.su.mfa_setup = None;
                            }
                        });
                    });
                });
            } else {
                w::setting_row(ui, p, "Authenticator app", "Not set up. Your vault is protected by your master password only.", true, |ui| {
                    if w::primary(ui, p, "Set up").clicked() {
                        self.su.mfa_setup = Some(totp::base32_encode(&totp::generate_secret()));
                    }
                });
            }
        });

        // ── session
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::TIMER, "Session", "Limit how long your vault stays open.");
            let mut changed = false;
            w::setting_row(ui, p, "Vault timeout", "Lock automatically after this many minutes without activity.", false, |ui| {
                let cur = self.vault.settings.auto_lock_minutes;
                egui::ComboBox::from_id_salt("softvault-timeout")
                    .width(150.0)
                    .selected_text(if cur >= 60 { format!("{} hour", cur / 60) } else { format!("{cur} minutes") })
                    .show_ui(ui, |ui| {
                        for (m, l) in [(1, "1 minute"), (5, "5 minutes"), (15, "15 minutes"), (30, "30 minutes"), (60, "1 hour")] {
                            changed |= ui.selectable_value(&mut self.vault.settings.auto_lock_minutes, m, l).changed();
                        }
                    });
            });
            w::setting_row(ui, p, "Clear clipboard", "Remove copied passwords from the clipboard after this many seconds.", true, |ui| {
                let cur = self.vault.settings.clipboard_clear_secs;
                egui::ComboBox::from_id_salt("softvault-clipboard")
                    .width(150.0)
                    .selected_text(if cur >= 60 { format!("{} minute{}", cur / 60, if cur >= 120 { "s" } else { "" }) } else { format!("{cur} seconds") })
                    .show_ui(ui, |ui| {
                        for (sec, l) in [(10, "10 seconds"), (20, "20 seconds"), (30, "30 seconds"), (60, "1 minute"), (120, "2 minutes")] {
                            changed |= ui.selectable_value(&mut self.vault.settings.clipboard_clear_secs, sec, l).changed();
                        }
                    });
            });
            if changed {
                let _ = self.vault.save_settings();
            }
        });

        // ── master password
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::KEY_ROUND, "Change master password", "Your vault key is re-wrapped with a new Argon2id-derived key. Items don't need re-encrypting.");
            ui.spacing_mut().item_spacing.y = 6.0;
            w::label(ui, p, "Current master password");
            w::text_input(ui, &mut self.su.old_pw, "", true);
            ui.columns(2, |c| {
                w::label(&mut c[0], p, "New master password");
                w::text_input(&mut c[0], &mut self.su.new_pw, "12+ characters", true);
                w::label(&mut c[1], p, "Confirm new master password");
                w::text_input(&mut c[1], &mut self.su.new_pw2, "", true);
            });
            let (sc, lb, bits) = generator::strength(&self.su.new_pw);
            w::strength_bar(ui, p, sc, lb, bits);
            ui.add_space(4.0);
            if let Some(b) = &self.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b.as_str());
                });
            } else if w::primary(ui, p, "Change master password").clicked() {
                if self.su.new_pw.chars().count() < 12 {
                    self.toast("New password must be at least 12 characters", false);
                } else if *self.su.new_pw != *self.su.new_pw2 {
                    self.toast("New passwords don't match", false);
                } else if let Ok(m) = self.vault.unlock_material() {
                    let old = Zeroizing::new(self.su.old_pw.as_bytes().to_vec());
                    let new = Zeroizing::new(self.su.new_pw.as_bytes().to_vec());
                    self.spawn(ctx, Some("Re-encrypting vault key…"), move || {
                        JobResult::Rewrapped(crypto::unwrap_vault_key(&old, &m.params, &m.salt, &m.wrapped).and_then(|k| crypto::rewrap(&k, &new)))
                    });
                }
            }
        });
    }

    fn settings_recovery(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context) {
        let rec = self.vault.recovery();
        let verified = rec.as_ref().map(|r| r.verified) == Some(true);
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::MAIL, "Recovery Gmail", "Your master password hint can be emailed only to a Gmail address you've verified.");
            match &rec {
                Some(r) if r.verified => {
                    w::setting_row(ui, p, "Verified address", &r.email, true, |ui| {
                        if w::ghost(ui, p, "Remove").clicked() {
                            let _ = self.vault.delete_meta("recovery");
                            self.su.email.clear();
                            self.toast("Recovery email removed", true);
                        }
                        ui.label(RichText::new(format!("{}  Verified", ic::BADGE_CHECK)).color(p.success).family(theme::semibold()));
                    });
                }
                _ => {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.columns(2, |c| {
                        w::label(&mut c[0], p, "Gmail address");
                        w::text_input(&mut c[0], &mut self.su.email, "you@gmail.com", false);
                        w::label(&mut c[1], p, "Google app password");
                        w::text_input(&mut c[1], &mut self.su.app_pw, "xxxx xxxx xxxx xxxx", true);
                    });
                    Frame::new().fill(p.bg_alt).corner_radius(theme::R_INPUT).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(format!("{}  Create an app password at myaccount.google.com → Security → 2-Step Verification → App passwords. It's protected with Windows DPAPI so your hint can be sent while the vault is locked.", ic::INFO)).size(12.5).color(p.weak));
                    });
                    ui.horizontal(|ui| {
                        if self.mail_busy {
                            ui.spinner();
                            ui.label("Sending…");
                        } else if w::ghost(ui, p, &format!("{}  Send verification code", ic::MAIL)).clicked() {
                            if !mailer::is_gmail(&self.su.email) {
                                self.toast("Please enter a @gmail.com address", false);
                            } else if self.su.app_pw.replace(' ', "").len() < 16 {
                                self.toast("Enter your 16-character Google app password", false);
                            } else {
                                use rand::Rng;
                                let code = Zeroizing::new(format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32)));
                                let email = self.su.email.trim().to_string();
                                let app_pw = self.su.app_pw.clone();
                                self.su.pending_verify = Some((email.clone(), app_pw.clone(), code.clone(), Instant::now()));
                                self.su.verify_sent = false;
                                self.mail_busy = true;
                                self.spawn(ctx, None, move || {
                                    let body = format!("Your SoftVault verification code is: {}\n\nIt expires in 10 minutes.", code.as_str());
                                    JobResult::VerifyMailSent(mailer::send_to_self(&email, &app_pw, "SoftVault verification code", body))
                                });
                            }
                        }
                        if self.su.verify_sent && self.su.pending_verify.is_some() {
                            ui.add_space(8.0);
                            ui.add_sized([130.0, 36.0], egui::TextEdit::singleline(&mut self.su.verify_input).hint_text("Code").margin(Margin::symmetric(10, 9)));
                            if w::primary(ui, p, "Verify").clicked() {
                                let (email, app_pw, code, sent) = self.su.pending_verify.as_ref().unwrap();
                                if sent.elapsed() > Duration::from_secs(600) {
                                    self.toast("Code expired. Send a new one.", false);
                                } else if self.su.verify_input.trim() == code.as_str() {
                                    let r = Recovery { email: email.clone(), app_password: app_pw.to_string(), verified: true };
                                    match self.vault.set_recovery(&r) {
                                        Ok(()) => self.toast("Gmail verified", true),
                                        Err(e) => self.toast(e.to_string(), false),
                                    }
                                    self.su.pending_verify = None;
                                    self.su.verify_sent = false;
                                    self.su.app_pw.zeroize();
                                } else {
                                    self.toast("Wrong verification code", false);
                                }
                                self.su.verify_input.clear();
                            }
                        }
                    });
                }
            }
        });
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::CIRCLE_HELP, "Master password hint", "Never shown on screen; only emailed to your verified Gmail from the lock screen.");
            w::row_rtl(ui, |ui| {
                if w::primary(ui, p, "Save hint").clicked() {
                    match self.vault.set_hint(&self.su.hint) {
                        Ok(()) => self.toast("Hint saved", true),
                        Err(e) => self.toast(e.to_string(), false),
                    }
                }
                ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut *self.su.hint).hint_text("No hint set").margin(Margin::symmetric(12, 9)));
            });
            if !verified {
                ui.label(RichText::new(format!("{}  Verify a Gmail address above, otherwise the hint can't be retrieved.", ic::TRIANGLE_ALERT)).size(12.5).color(p.warn));
            }
        });
    }

    fn settings_appearance(&mut self, ui: &mut egui::Ui, p: &Palette) {
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::PALETTE, "Appearance", "Choose how SoftVault looks on this computer.");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                for (dark, name, icon) in [(false, "Light", ic::SUN), (true, "Dark", ic::MOON)] {
                    let sel = self.dark == dark;
                    let preview = theme::palette(dark);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(210.0, 150.0), egui::Sense::click());
                    let painter = ui.painter();
                    painter.rect_filled(rect, theme::R_CARD, preview.bg);
                    painter.rect_stroke(rect, theme::R_CARD, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { p.accent } else { p.border }), egui::StrokeKind::Inside);
                    // mini mock-up of the app
                    let inner = rect.shrink(10.0);
                    let side = egui::Rect::from_min_size(inner.min, Vec2::new(48.0, inner.height() - 30.0));
                    painter.rect_filled(side, CornerRadius::same(6), preview.panel);
                    painter.rect_filled(egui::Rect::from_min_size(side.min + Vec2::new(6.0, 10.0), Vec2::new(36.0, 8.0)), CornerRadius::same(3), preview.nav_active);
                    for i in 0..3 {
                        let y = inner.top() + 10.0 + i as f32 * 22.0;
                        painter.rect_filled(egui::Rect::from_min_size(egui::pos2(side.right() + 10.0, y), Vec2::new(inner.right() - side.right() - 10.0, 14.0)), CornerRadius::same(4), preview.card_hover);
                    }
                    painter.text(egui::pos2(inner.left() + 4.0, inner.bottom() - 6.0), egui::Align2::LEFT_BOTTOM, format!("{icon}  {name}"), egui::FontId::new(13.5, theme::semibold()), preview.text);
                    if sel {
                        painter.text(egui::pos2(inner.right() - 4.0, inner.bottom() - 6.0), egui::Align2::RIGHT_BOTTOM, ic::CIRCLE_CHECK, egui::FontId::proportional(16.0), preview.accent);
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.set_theme(dark);
                    }
                }
            });
        });
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::KEYBOARD, "Keyboard shortcuts", "Work faster without the mouse.");
            egui::Grid::new("softvault-shortcuts").num_columns(2).spacing([40.0, 10.0]).striped(false).show(ui, |ui| {
                for (k, d) in [
                    ("Ctrl + F", "Search the vault"),
                    ("Ctrl + N", "New login"),
                    ("Ctrl + Shift + N", "New secure note"),
                    ("Ctrl + L", "Lock the vault"),
                    ("Ctrl + S", "Save the open item"),
                    ("Esc", "Close dialog or full-screen preview"),
                    ("F11", "Full screen (in note preview)"),
                ] {
                    Frame::new().fill(p.bg_alt).stroke(Stroke::new(1.0, p.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
                        ui.label(RichText::new(k).monospace().size(12.5));
                    });
                    ui.label(RichText::new(d).size(13.5));
                    ui.end_row();
                }
            });
        });
    }

    fn settings_backup(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &egui::Context) {
        // ── storage location
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::DATABASE, "Storage location", "Where your encrypted vault database lives.");
            let loc = self.db_location.clone();
            let mode = if loc.portable { "Portable: stored next to SoftVault.exe" } else { "Stored in your Windows profile (the app's folder is read-only)" };
            w::setting_row(ui, p, "Database file", mode, false, |ui| {
                if w::ghost(ui, p, &format!("{}  Open folder", ic::FOLDER_OPEN)).clicked()
                    && let Some(dir) = loc.path.parent()
                {
                    #[cfg(windows)]
                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                    #[cfg(not(windows))]
                    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                }
            });
            Frame::new().fill(p.bg_alt).corner_radius(theme::R_INPUT).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(loc.path.display().to_string()).monospace().size(12.5)).selectable(true));
            });
            if loc.portable {
                ui.label(RichText::new(format!("{}  Back up the whole folder to back up everything. The file is fully encrypted. Your Gmail recovery settings are tied to this Windows account; on another PC, just set them up again.", ic::INFO)).size(12.5).color(p.weak));
            }
            let (size, free) = self.vault.db_stats().unwrap_or((0, 0));
            let desc = format!(
                "vault.db is {}. Space from deleted items is returned automatically{}.",
                transfer::human_size(size),
                if free > 0 { format!("; {} can be reclaimed now", transfer::human_size(free)) } else { String::new() }
            );
            w::setting_row(ui, p, "Database size", &desc, false, |ui| {
                if w::ghost(ui, p, &format!("{}  Compact now", ic::MINIMIZE_2)).on_hover_text("Rebuild vault.db as small as possible").clicked() {
                    match self.vault.compact() {
                        Ok(()) => {
                            let after = self.vault.db_stats().map(|s| s.0).unwrap_or(size);
                            self.toast(format!("Database compacted: {} → {}", transfer::human_size(size), transfer::human_size(after)), true);
                        }
                        Err(e) => self.toast(format!("Compact failed: {e}"), false),
                    }
                }
            });
            w::setting_row(ui, p, "Back up database now", "Save an encrypted copy of vault.db. Restore it by placing it next to SoftVault.exe.", true, |ui| {
                if w::primary(ui, p, &format!("{}  Back up…", ic::SAVE)).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("vault-backup-{}.db", chrono::Local::now().format("%Y-%m-%d")))
                        .add_filter("SoftVault database", &["db"])
                        .save_file()
                {
                    match self.vault.backup_to(&path) {
                        Ok(()) => self.toast(format!("Backup saved to {}", path.display()), true),
                        Err(e) => self.toast(format!("Backup failed: {e}"), false),
                    }
                }
            });
        });

        // ── export
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::DOWNLOAD, "Export vault", "Save all logins and notes to a file.");
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(w::h(format!("{}  Encrypted (.svx), recommended", ic::LOCK), 14.0));
            ui.label(RichText::new("Protected with Argon2id + XChaCha20-Poly1305 using a password you choose.").size(12.5).color(p.weak));
            ui.columns(2, |c| {
                w::text_input(&mut c[0], &mut self.su.export_pw, "File password (12+ characters)", true);
                w::text_input(&mut c[1], &mut self.su.export_pw2, "Confirm file password", true);
            });
            if let Some(b) = &self.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(b.as_str());
                });
            } else if w::primary(ui, p, &format!("{}  Export encrypted…", ic::DOWNLOAD)).clicked() {
                if self.su.export_pw.chars().count() < 12 {
                    self.toast("File password must be at least 12 characters", false);
                } else if *self.su.export_pw != *self.su.export_pw2 {
                    self.toast("File passwords don't match", false);
                } else if let Some(path) = rfd::FileDialog::new()
                    .set_file_name(format!("softvault-export-{}.svx", chrono::Local::now().format("%Y-%m-%d")))
                    .add_filter("SoftVault encrypted export", &["svx"])
                    .save_file()
                {
                    match self.vault.export_json() {
                        Ok(json) => {
                            let pw = Zeroizing::new(self.su.export_pw.as_bytes().to_vec());
                            self.spawn(ctx, Some("Encrypting export…"), move || JobResult::Exported(crypto::encrypt_export(&json, &pw), path));
                        }
                        Err(e) => self.toast(e.to_string(), false),
                    }
                }
            }
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::ZERO, p.border);
            ui.add_space(8.0);
            ui.label(w::h(format!("{}  Unencrypted (.json)", ic::TRIANGLE_ALERT), 14.0).color(p.warn));
            ui.horizontal(|ui| {
                w::toggle(ui, p, &mut self.su.plain_ack);
                ui.label(RichText::new("I understand this file will contain all my passwords and notes in plain text").size(13.0));
            });
            ui.add_enabled_ui(self.su.plain_ack, |ui| {
                if w::ghost(ui, p, &format!("{}  Export unencrypted…", ic::DOWNLOAD)).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("softvault-plaintext-{}.json", chrono::Local::now().format("%Y-%m-%d")))
                        .add_filter("JSON", &["json"])
                        .save_file()
                {
                    match self.vault.export_json().and_then(|j| std::fs::write(&path, &*j).map_err(Into::into)) {
                        Ok(()) => self.toast(format!("Plain export saved to {}", path.display()), true),
                        Err(e) => self.toast(format!("Export failed: {e}"), false),
                    }
                    self.su.plain_ack = false;
                }
            });
        });

        // ── import
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::UPLOAD, "Import", "Add items from a SoftVault export (.svx or .json). Existing items are kept.");
            if self.su.import_bytes.is_some() {
                ui.label(RichText::new("This file is encrypted. Enter its file password:").size(13.0));
                w::row_rtl(ui, |ui| {
                    if w::ghost(ui, p, "Cancel").clicked() {
                        self.su.import_bytes = None;
                    }
                    if self.busy.is_none() && w::primary(ui, p, "Decrypt & import").clicked() {
                        let bytes = self.su.import_bytes.clone().unwrap();
                        let pw = Zeroizing::new(self.su.import_pw.as_bytes().to_vec());
                        self.spawn(ctx, Some("Decrypting import…"), move || {
                            JobResult::ImportDecrypted(crypto::decrypt_export(&bytes, &pw).and_then(|pt| vault::parse_plain_export(&pt)))
                        });
                    }
                    ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut *self.su.import_pw).password(true).hint_text("File password").margin(Margin::symmetric(12, 9)));
                });
            } else if w::ghost(ui, p, &format!("{}  Choose file…", ic::FOLDER_OPEN)).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("SoftVault export", &["svx", "json"]).pick_file()
            {
                match std::fs::read(&path) {
                    Ok(bytes) if vault::is_encrypted_export(&bytes) => self.su.import_bytes = Some(bytes),
                    Ok(bytes) => match vault::parse_plain_export(&bytes).and_then(|e| self.vault.import_export(e)) {
                        Ok((n, f)) => self.toast(format!("Imported {n} items and {f} files"), true),
                        Err(e) => self.toast(format!("Import failed: {e}"), false),
                    },
                    Err(e) => self.toast(e.to_string(), false),
                }
            }
        });
    }

    fn settings_about(&mut self, ui: &mut egui::Ui, p: &Palette) {
        card(ui, p, |ui| {
            ui.horizontal(|ui| {
                w::logo(ui, p, 48.0, false);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(w::h("SoftVault", 20.0));
                    ui.label(RichText::new(format!("Version {} · zero-knowledge password & notes vault", env!("CARGO_PKG_VERSION"))).size(12.5).color(p.weak));
                });
            });
        });
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::SHIELD_CHECK, "Security model", "");
            let rows = [
                ("Zero-knowledge", "Your master password is never stored. Only an authenticated ciphertext of a random vault key is kept."),
                ("Key derivation", "Argon2id: 256 MiB memory, 3 iterations, 4 lanes, 256-bit random salt."),
                ("Encryption", "XChaCha20-Poly1305 with a fresh 192-bit nonce per record, bound to its record ID."),
                ("Metadata", "Titles, usernames, URLs, timestamps and item types are all encrypted."),
                ("Memory", "Decrypted data is wiped (zeroize) whenever the vault locks."),
            ];
            for (i, (t, d)) in rows.iter().enumerate() {
                w::setting_row(ui, p, t, d, i == rows.len() - 1, |ui| {
                    ui.label(RichText::new(ic::CIRCLE_CHECK).color(p.success).size(16.0));
                });
            }
        });
        card(ui, p, |ui| {
            w::section_title(ui, p, ic::BOOK_OPEN, "Open-source components", "");
            ui.label(RichText::new("Inter typeface (SIL OFL 1.1) · Lucide icons (ISC) · egui · SQLite · RustCrypto (Argon2, ChaCha20-Poly1305) · pulldown-cmark · syntect").size(12.5).color(p.weak));
        });
    }

    fn ui_toast(&mut self, ctx: &egui::Context, p: &Palette) {
        // slide-in + fade
        let born = self.undo.as_ref().map(|u| u.until - Duration::from_secs(UNDO_SECS)).or(self.toast.as_ref().map(|t| t.born));
        let Some(born) = born else { return };
        let t = (born.elapsed().as_secs_f32() / 0.22).min(1.0);
        let ease = 1.0 - (1.0 - t).powi(3);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let offset = Vec2::new(-24.0, -24.0 + (1.0 - ease) * 28.0);

        let mut restore = false;
        let mut dismiss_undo = false;
        egui::Area::new(egui::Id::unique("softvault-toast"))
            .anchor(egui::Align2::RIGHT_BOTTOM, offset)
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                ui.multiply_opacity(ease);
                let frame = Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(theme::R_CARD)
                    .inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 12 })
                    .shadow(theme::soft_shadow(p));
                if let Some(u) = &self.undo {
                    let resp = frame.show(ui, |ui| {
                        ui.set_width(380.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 15.0, p.danger.gamma_multiply(if p.dark { 0.25 } else { 0.12 }));
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, ic::TRASH_2, egui::FontId::proportional(15.0), p.danger);
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.add(egui::Label::new(w::h(format!("\u{201C}{}\u{201D} deleted", u.title), 13.5)).truncate());
                                ui.label(RichText::new("It's gone from your vault.").size(12.5).color(p.weak));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if w::ghost_icon(ui, p, ic::X, "Dismiss").clicked() {
                                    dismiss_undo = true;
                                }
                                if w::ghost(ui, p, &format!("{}  Undo", ic::ROTATE_CCW)).clicked() {
                                    restore = true;
                                }
                            });
                        });
                    });
                    // countdown bar along the bottom edge
                    let left = u.until.saturating_duration_since(Instant::now()).as_secs_f32() / UNDO_SECS as f32;
                    let r = resp.response.rect;
                    let bar = egui::Rect::from_min_size(egui::pos2(r.left() + 12.0, r.bottom() - 4.0), Vec2::new((r.width() - 24.0) * left, 2.5));
                    ui.painter().rect_filled(bar, CornerRadius::same(2), p.danger.gamma_multiply(0.7));
                    ctx.request_repaint_after(Duration::from_millis(30));
                } else if let Some(t) = &self.toast {
                    let tone = if t.ok { p.success } else { p.danger };
                    frame.show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
                            ui.painter().circle_filled(r.center(), 15.0, tone.gamma_multiply(if p.dark { 0.25 } else { 0.14 }));
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, if t.ok { ic::CHECK } else { ic::X }, egui::FontId::proportional(15.0), tone);
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(w::h(if t.ok { "Done" } else { "Something went wrong" }, 13.5));
                                ui.add(egui::Label::new(RichText::new(&t.text).size(13.0).color(p.weak)).wrap());
                            });
                        });
                    });
                }
            });
        if restore && let Some(u) = self.undo.take() {
            match self.vault.save_item(Some(u.id.clone()), u.data.clone()) {
                Ok(id) => {
                    self.selected = Some(id);
                    self.toast(format!("\u{201C}{}\u{201D} restored", u.title), true);
                }
                Err(e) => self.toast(format!("Restore failed: {e}"), false),
            }
        }
        if dismiss_undo {
            self.undo = None;
        }
    }
}

impl eframe::App for SoftVaultApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.applied_theme != Some(self.dark) {
            theme::apply(&ctx, self.dark);
            self.applied_theme = Some(self.dark);
        }
        self.poll_jobs(&ctx);
        self.tick(&ctx);
        let p = theme::palette(self.dark);
        match self.screen {
            Screen::Setup => self.ui_setup(ui, &p),
            Screen::Unlock => self.ui_unlock(ui, &p),
            Screen::Mfa => self.ui_mfa(ui, &p),
            Screen::Main => self.ui_main(ui, &p),
        }

        // ── drag & drop in: encrypt dropped files
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            if self.screen == Screen::Main {
                self.last_activity = Instant::now();
                self.import_paths(dropped);
            } else {
                self.toast("Unlock your vault first, then drop the files again.", false);
            }
        }
        if self.screen == Screen::Main {
            self.ui_drag_ghost(&ctx, &p);
            self.ui_drop_overlay(&ctx, &p);
            // ── drag & drop out: decrypted copy handed to the OS
            self.process_drag_out(&ctx, frame);
        }
        self.ui_toast(&ctx, &p);
    }

    fn on_exit(&mut self) {
        self.temp.clear();
    }
}
