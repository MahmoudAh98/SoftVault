use crate::crypto::{self, Key, NewVaultMaterial};
use crate::generator::{self, GenOptions};
use crate::mailer;
use crate::theme::{self, Palette};
use crate::render::{self, NoteFormat};
use crate::totp;
use crate::vault::{self, ItemData, Kind, Recovery, Vault};
use crate::widgets::{self as w, card};
use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Vec2};
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
    ImportDecrypted(anyhow::Result<Vec<ItemData>>),
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
    Settings,
}

struct Editor {
    id: Option<String>,
    data: ItemData,
    show_pw: bool,
    show_gen: bool,
    confirm_delete: bool,
    note_mode: NoteMode,
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
}

struct Toast {
    text: String,
    ok: bool,
    until: Instant,
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
}

struct FullView {
    item_id: Option<String>,
    zoom: f32,
    os_fullscreen: bool,
    show_toc: bool,
}

impl SoftVaultApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let path = vault::default_db_path();
        let vault = Vault::open(&path).expect("could not open vault database");
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
        }
    }

    fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toast = Some(Toast { text: text.into(), ok, until: Instant::now() + Duration::from_secs(4) });
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
                    match r.and_then(|items| self.vault.import_items(items)) {
                        Ok(n) => {
                            self.su.import_bytes = None;
                            self.su.import_pw.zeroize();
                            self.toast(format!("Imported {n} entries"), true);
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
        if let Some(t) = &self.toast
            && Instant::now() > t.until {
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
                    ui.label(RichText::new("🔒 Argon2id · XChaCha20-Poly1305 · stored locally in SQLite").size(12.0).color(p.weak));
                    ui.add_space(16.0);
                });
            });
        });
    }

    fn error_line(&self, ui: &mut egui::Ui, p: &Palette) {
        if let Some(e) = &self.error {
            ui.label(RichText::new(format!("⚠ {e}")).color(p.danger).size(13.0));
        }
    }

    fn ui_setup(&mut self, ui: &mut egui::Ui, p: &Palette) {
        self.auth_frame(ui, p, |s, ui| {
            ui.vertical_centered(|ui| ui.label(w::h("Create your vault", 20.0)));
            ui.label(RichText::new("Your master password is never stored or sent anywhere. If you forget it, your data cannot be recovered.").size(12.5).color(p.weak));
            ui.add_space(8.0);
            w::label(ui, p, "Master password (required)");
            w::text_input(ui, &mut s.pw, "At least 12 characters", !s.show_pw);
            let (sc, lb, bits) = generator::strength(&s.pw);
            w::strength_bar(ui, p, sc, lb, bits);
            w::label(ui, p, "Confirm master password (required)");
            let r = w::text_input(ui, &mut s.pw2, "Repeat master password", !s.show_pw);
            ui.checkbox(&mut s.show_pw, "Show password");
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
            let r = w::text_input(ui, &mut s.pw, "Master password", !s.show_pw);
            if s.want_focus {
                s.want_focus = false;
                r.request_focus();
            }
            ui.checkbox(&mut s.show_pw, "Show password");
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
                    } else if w::ghost_wide(ui, p, &format!("📧  Email my hint to {}", mailer::mask_email(&rec.email))).clicked() {
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
            .show(ui, |ui| match self.view {
                View::Settings => self.ui_settings(ui, p),
                View::Generator => self.ui_generator(ui, p),
                _ => self.ui_vault_page(ui, p),
            });

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
            w::logo(ui, p, 30.0, true);
            ui.label(w::h("SoftVault", 19.0).color(p.nav_text));
        });
        ui.add_space(16.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let n_pw = self.vault.items.iter().filter(|i| i.data.kind == Kind::Password).count();
        let n_notes = self.vault.items.iter().filter(|i| i.data.kind == Kind::Note).count();
        let n_fav = self.vault.items.iter().filter(|i| i.data.favorite).count();
        w::nav_section(ui, p, "VAULT");
        for (v, icon, text, count) in [
            (View::All, "🗂", "All items", self.vault.items.len()),
            (View::Passwords, "🔑", "Logins", n_pw),
            (View::Notes, "📝", "Secure notes", n_notes),
            (View::Favorites, "★", "Favorites", n_fav),
        ] {
            if w::nav_item(ui, p, icon, text, self.view == v, Some(count)).clicked() {
                self.view = v;
            }
        }
        w::nav_section(ui, p, "TOOLS");
        if w::nav_item(ui, p, "🎲", "Generator", self.view == View::Generator, None).clicked() {
            self.view = View::Generator;
        }
        w::nav_section(ui, p, "MANAGE");
        if w::nav_item(ui, p, "⇅", "Import / export", false, None).clicked() {
            self.view = View::Settings;
            self.su.loaded = false;
        }
        if w::nav_item(ui, p, "⚙", "Settings", self.view == View::Settings, None).clicked() {
            self.view = View::Settings;
            self.su.loaded = false;
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            // account-style footer
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 17.0, if p.dark { p.accent } else { Color32::from_rgb(0x17, 0x5d, 0xdc) });
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "SV", egui::FontId::new(12.5, theme::semibold()), if p.dark { p.on_accent } else { Color32::WHITE });
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(w::h("Local vault", 13.5).color(p.nav_text));
                    let mfa = if self.vault.settings.mfa_secret.is_some() { "2FA enabled" } else { "2FA off" };
                    ui.label(RichText::new(mfa).size(12.0).color(p.nav_weak));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("🔒").size(15.0).color(p.nav_text)).fill(p.nav_hover).stroke(Stroke::NONE).corner_radius(theme::PILL).min_size(Vec2::splat(34.0)))
                        .on_hover_text("Lock vault")
                        .clicked()
                    {
                        self.lock(ui.ctx());
                    }
                });
            });
            ui.add_space(8.0);
            let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(line, CornerRadius::ZERO, p.nav_hover);
            ui.add_space(8.0);
            // theme switch
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let half = (ui.available_width() - 6.0) / 2.0;
                for (dark, txt) in [(false, "☀ Light"), (true, "🌙 Dark")] {
                    let sel = self.dark == dark;
                    let b = egui::Button::new((egui::Atom::grow(), RichText::new(txt).size(13.0).color(if sel { p.nav_active_text } else { p.nav_text }), egui::Atom::grow()))
                        .fill(if sel { p.nav_active } else { p.nav_hover })
                        .stroke(Stroke::NONE)
                        .corner_radius(theme::PILL)
                        .min_size(Vec2::new(half, 30.0));
                    if ui.add(b).clicked() {
                        self.set_theme(dark);
                    }
                }
            });
        });
    }

    fn visible_items(&self) -> Vec<(String, String, String, bool, bool, i64, &'static str)> {
        let q = self.search.to_lowercase();
        let mut v: Vec<_> = self
            .vault
            .items
            .iter()
            .filter(|i| match self.view {
                View::Passwords => i.data.kind == Kind::Password,
                View::Notes => i.data.kind == Kind::Note,
                View::Favorites => i.data.favorite,
                _ => true,
            })
            .filter(|i| {
                q.is_empty()
                    || i.data.title.to_lowercase().contains(&q)
                    || i.data.username.to_lowercase().contains(&q)
                    || i.data.url.to_lowercase().contains(&q)
                    || (i.data.kind == Kind::Note && i.data.notes.to_lowercase().contains(&q))
            })
            .map(|i| {
                let sub = match i.data.kind {
                    Kind::Password => {
                        if !i.data.username.is_empty() { i.data.username.clone() } else { i.data.url.clone() }
                    }
                    Kind::Note => match i.data.format {
                        NoteFormat::Csv => format!("{} rows", i.data.notes.lines().count().saturating_sub(1)),
                        NoteFormat::Code if !i.data.lang.is_empty() => format!("{} · {} lines", i.data.lang, i.data.notes.lines().count()),
                        _ => i.data
                            .notes
                            .lines()
                            .map(|l| l.trim_start_matches(['#', '>', '-', '*', ' ', '`', '{', '[']).trim())
                            .find(|l| !l.is_empty())
                            .unwrap_or("")
                            .to_string(),
                    },
                };
                (i.id.clone(), i.data.title.clone(), sub, i.data.favorite, i.data.kind == Kind::Note, i.data.updated, if i.data.kind == Kind::Note { i.data.format.badge() } else { "🔑 Login" })
            })
            .collect();
        v.sort_by(|a, b| b.3.cmp(&a.3).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        v
    }

    fn new_item(&mut self, kind: Kind) {
        self.selected = None;
        self.editor = Some(Editor {
            id: None,
            data: ItemData { kind, format: if kind == Kind::Note { NoteFormat::Markdown } else { NoteFormat::Plain }, ..Default::default() },
            show_pw: kind == Kind::Password,
            show_gen: false,
            confirm_delete: false,
            note_mode: if kind == Kind::Note { NoteMode::Split } else { NoteMode::Edit },
        });
        if kind == Kind::Password
            && let Some(e) = &mut self.editor
        {
            e.data.password = generator::generate(&self.gen_opts);
        }
    }

    fn select(&mut self, id: &str) {
        if let Some(it) = self.vault.items.iter().find(|i| i.id == id) {
            self.selected = Some(id.to_string());
            self.editor = Some(Editor { id: Some(id.to_string()), data: it.data.clone(), show_pw: false, show_gen: false, confirm_delete: false, note_mode: NoteMode::Preview });
        }
    }

    fn new_item_menu(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let r = w::primary(ui, p, "＋  New");
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(190.0);
            if ui.button("🔑   Login").clicked() {
                self.new_item(Kind::Password);
                ui.close();
            }
            if ui.button("📝   Secure note").clicked() {
                self.new_item(Kind::Note);
                ui.close();
            }
        });
    }

    fn ui_vault_page(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let title = match self.view {
            View::Passwords => "Logins",
            View::Notes => "Secure notes",
            View::Favorites => "Favorites",
            _ => "All items",
        };
        w::page_header(ui, p, "🗂", title);
        ui.add_space(14.0);

        // toolbar: search (left) · import + new (right)
        w::row_rtl(ui, |ui| {
            self.new_item_menu(ui, p);
            if w::ghost(ui, p, "⇅  Import").clicked() {
                self.view = View::Settings;
                self.su.loaded = false;
            }
            ui.add_space((ui.available_width() - 380.0).max(0.0));
            ui.add_sized(
                [ui.available_width(), 36.0],
                egui::TextEdit::singleline(&mut self.search).hint_text("🔍  Search vault").margin(Margin::symmetric(12, 9)),
            );
        });
        ui.add_space(6.0);

        let items = self.visible_items();
        // filter chips + count
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (v, label) in [(View::All, "Type: All"), (View::Passwords, "🔑 Logins"), (View::Notes, "📝 Notes"), (View::Favorites, "★ Favorites")] {
                if w::chip(ui, p, label, self.view == v).clicked() {
                    self.view = v;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("{} item{}", items.len(), if items.len() == 1 { "" } else { "s" })).size(12.5).color(p.weak));
            });
        });
        ui.add_space(10.0);

        let fmt = |t: i64| {
            chrono::DateTime::from_timestamp(t, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%b %d, %Y").to_string())
                .unwrap_or_default()
        };
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(theme::R_CARD)
            .inner_margin(Margin::symmetric(4, 2))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                w::table_header(ui, p);
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if items.is_empty() {
                        ui.add_space(48.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("🔍").size(30.0).color(p.weak));
                            ui.add_space(4.0);
                            ui.label(w::h(if self.search.is_empty() { "No items to list" } else { "No matching items" }, 16.0));
                            ui.label(RichText::new("Items you add are encrypted on this device before they're saved.").color(p.weak));
                            ui.add_space(8.0);
                            if self.search.is_empty() && w::primary(ui, p, "＋  New login").clicked() {
                                self.new_item(Kind::Password);
                            }
                        });
                    }
                    for (id, title, sub, fav, note, updated, badge) in items {
                        let sel = self.selected.as_deref() == Some(&id);
                        let (resp, opt_rect) = w::table_row(ui, p, &title, &sub, note, badge, fav, &fmt(updated), sel);
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(opt_rect).layout(Layout::left_to_right(Align::Center)));
                        let ob = child.add(
                            egui::Button::new(RichText::new("⋮").size(16.0).color(p.weak))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .corner_radius(theme::PILL)
                                .min_size(Vec2::splat(30.0)),
                        );
                        let menu = |ui: &mut egui::Ui, s: &mut Self| {
                            ui.set_min_width(200.0);
                            let data = s.vault.items.iter().find(|i| i.id == id).map(|i| i.data.clone());
                            let Some(data) = data else { return };
                            if data.kind == Kind::Password {
                                if ui.button("📋   Copy username").clicked() {
                                    s.copy(&ctx, &data.username, "Username");
                                    ui.close();
                                }
                                if ui.button("📋   Copy password").clicked() {
                                    s.copy(&ctx, &data.password, "Password");
                                    ui.close();
                                }
                            } else if ui.button("📋   Copy note").clicked() {
                                s.copy(&ctx, &data.notes, "Note");
                                ui.close();
                            }
                            if data.kind == Kind::Note && ui.button("⛶   Open full screen").clicked() {
                                s.open_fullscreen(Some(id.clone()));
                                ui.close();
                            }
                            if ui.button("✏   Edit").clicked() {
                                s.select(&id);
                                ui.close();
                            }
                            if ui.button(if data.favorite { "☆   Remove favorite" } else { "★   Add to favorites" }).clicked() {
                                let mut d = data.clone();
                                d.favorite = !d.favorite;
                                let _ = s.vault.save_item(Some(id.clone()), d);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(RichText::new("🗑   Delete").color(p.danger)).clicked() {
                                s.select(&id);
                                if let Some(e) = &mut s.editor {
                                    e.confirm_delete = true;
                                }
                                ui.close();
                            }
                        };
                        egui::Popup::menu(&ob).show(|ui| menu(ui, self));
                        egui::Popup::context_menu(&resp).show(|ui| menu(ui, self));
                        if resp.clicked() {
                            self.select(&id);
                        }
                    }
                });
            });
    }

    fn ui_generator(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        w::page_header(ui, p, "🎲", "Generator");
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
                    if w::primary(ui, p, "↻  Regenerate").clicked() {
                        self.gen_preview = Zeroizing::new(generator::generate(&self.gen_opts));
                    }
                    if w::ghost(ui, p, "📋  Copy").clicked() {
                        let s = self.gen_preview.to_string();
                        self.copy(&ctx, &s, "Password");
                    }
                });
            });
            card(ui, p, |ui| {
                w::section_title(ui, p, "⚙", "Options", "Uses your operating system's cryptographically secure random generator");
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
                ui.label(w::h("Note", 15.0));
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
                    if w::primary(ui, p, "⛶  Full screen").on_hover_text("Open a distraction-free, full-window preview").clicked() {
                        self.open_fullscreen(ed.id.clone());
                    }
                    ui.add_space(6.0);
                    if ed.data.format.has_preview() {
                        for (m, label) in [(NoteMode::Preview, "👁 Preview"), (NoteMode::Split, "◫ Split"), (NoteMode::Edit, "✏ Edit")] {
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
                if w::ghost(ui, p, "📋  Copy").clicked() {
                    let n = ed.data.notes.clone();
                    self.copy(ctx, &n, "Note");
                }
                if fmt == NoteFormat::Json && w::ghost(ui, p, "✨  Format JSON").clicked() {
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
            ("B", "Bold", A::Wrap("**", "**", "bold")),
            ("I", "Italic", A::Wrap("*", "*", "italic")),
            ("S̶", "Strikethrough", A::Wrap("~~", "~~", "text")),
            ("H", "Heading", A::Prefix("## ")),
            ("•", "Bulleted list", A::Prefix("- ")),
            ("1.", "Numbered list", A::Prefix("1. ")),
            ("☑", "Task list", A::Prefix("- [ ] ")),
            ("❝", "Quote", A::Prefix("> ")),
            ("</>", "Inline code", A::Wrap("`", "`", "code")),
            ("🔗", "Link", A::Wrap("[", "](https://)", "link text")),
            ("▦", "Table", A::Insert("\n| Column | Value |\n|--------|-------|\n| a      | b     |\n")),
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
                let code_block = ui.add(egui::Button::new(RichText::new("{ }").size(13.0)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::new(30.0, 26.0))).on_hover_text("Code block");
                let rule = ui.add(egui::Button::new(RichText::new("—").size(13.0)).fill(Color32::TRANSPARENT).stroke(Stroke::NONE).min_size(Vec2::new(30.0, 26.0))).on_hover_text("Divider");
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
                            if w::primary(ui, p, "✖  Close").on_hover_text("Esc").clicked() {
                                close = true;
                            }
                            let os_label = if fv.os_fullscreen { "🗗  Exit full screen" } else { "⛶  Full screen" };
                            if w::ghost(ui, p, os_label).on_hover_text("F11 — fill the whole monitor").clicked() {
                                toggle_os = true;
                            }
                            if w::ghost(ui, p, "📋  Copy").clicked() {
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
                            if !toc.is_empty() && w::chip(ui, p, "☰ Contents", fv.show_toc).clicked() {
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
        let is_note = ed.data.kind == Kind::Note;

        egui::Modal::new(egui::Id::unique("softvault-item-dialog"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 150 } else { 90 }))
            .frame(Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(theme::R_CARD).shadow(theme::soft_shadow(p)))
            .show(ctx, |ui| {
                ui.set_width(if is_note { (ctx.content_rect().width() - 120.0).clamp(620.0, 1120.0) } else { 620.0 });
                ui.spacing_mut().item_spacing.y = 0.0;
                // header
                Frame::new().inner_margin(Margin::symmetric(24, 16)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let kind = if is_note { "note" } else { "login" };
                        let title = if ed.id.is_none() { format!("New {kind}") } else { format!("Edit {kind}") };
                        ui.label(w::h(title, 19.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new(RichText::new("✖").size(14.0).color(p.weak)).frame(false)).on_hover_text("Close").clicked() {
                                close = true;
                            }
                            ui.add_space(6.0);
                            let star = if ed.data.favorite { "★" } else { "☆" };
                            if ui.add(egui::Button::new(RichText::new(star).size(17.0).color(p.warn)).frame(false)).on_hover_text("Favorite").clicked() {
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
                            ui.label(w::h("Item details", 15.0));
                            w::label(ui, p, "Item name (required)");
                            w::text_input(ui, &mut ed.data.title, if is_note { "e.g. Recovery codes" } else { "e.g. GitHub" }, false);
                        });

                        if !is_note {
                            card(ui, p, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label(w::h("Login credentials", 15.0));
                                w::label(ui, p, "Username");
                                w::row_rtl(ui, |ui| {
                                    if w::icon_btn(ui, p, "📋", "Copy username").clicked() {
                                        let u = ed.data.username.clone();
                                        self.copy(ctx, &u, "Username");
                                    }
                                    ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut ed.data.username).hint_text("you@example.com").margin(Margin::symmetric(12, 9)));
                                });
                                w::label(ui, p, "Password");
                                w::row_rtl(ui, |ui| {
                                    if w::icon_btn(ui, p, "↻", "Generate password").clicked() {
                                        ed.show_gen = !ed.show_gen;
                                    }
                                    if w::icon_btn(ui, p, "📋", "Copy password").clicked() {
                                        let pw = ed.data.password.clone();
                                        self.copy(ctx, &pw, "Password");
                                    }
                                    if w::icon_btn(ui, p, if ed.show_pw { "🙈" } else { "👁" }, "Toggle visibility").clicked() {
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
                                        if w::icon_btn(ui, p, "📋", "Copy verification code").clicked() {
                                            self.copy(ctx, &code, "Verification code");
                                        }
                                    }
                                    ui.add_sized([ui.available_width(), 36.0], egui::TextEdit::singleline(&mut ed.data.totp).hint_text("Base32 key, e.g. JBSWY3DPEHPK3PXP").password(true).margin(Margin::symmetric(12, 9)));
                                });
                                if let Some(sec) = totp::base32_decode(&ed.data.totp).filter(|s| s.len() >= 10) {
                                    let code = format!("{:06}", totp::hotp(&sec, totp::now_step()));
                                    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                                    let left = totp::PERIOD - secs % totp::PERIOD;
                                    ui.label(RichText::new(format!("{} {}   ⏱ {left}s", &code[..3], &code[3..])).monospace().size(19.0).color(p.accent));
                                }
                            });
                            card(ui, p, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label(w::h("Autofill options", 15.0));
                                w::label(ui, p, "Website (URI)");
                                w::text_input(ui, &mut ed.data.url, "https://", false);
                            });
                        }

                        if is_note {
                            self.note_card(ui, p, ctx, &mut ed, (max_body - 300.0).max(220.0));
                        } else {
                            card(ui, p, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                ui.label(w::h("Additional options", 15.0));
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
                            ui.label(RichText::new(format!("Created: {}    Last edited: {}", fmt(ed.data.created), fmt(ed.data.updated))).size(12.0).color(p.weak));
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
                        if ed.id.is_some() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ed.confirm_delete {
                                    if w::danger(ui, p, "Delete permanently").clicked() {
                                        delete = true;
                                    }
                                    if ui.add(egui::Button::new(RichText::new("Keep").color(p.weak)).frame(false)).clicked() {
                                        ed.confirm_delete = false;
                                    }
                                } else if ui
                                    .add(egui::Button::new(RichText::new("🗑").size(15.0).color(p.danger)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, p.danger)).corner_radius(theme::PILL).min_size(Vec2::splat(36.0)))
                                    .on_hover_text("Delete")
                                    .clicked()
                                {
                                    ed.confirm_delete = true;
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
            match self.vault.delete_item(&id) {
                Ok(()) => {
                    self.toast("Item deleted", true);
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
        w::page_header(ui, p, "⚙", "Settings");
        ui.add_space(8.0);

        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(820.0));
            ui.spacing_mut().item_spacing.y = 10.0;

            // ── Appearance & behaviour
            card(ui, p, |ui| {
                w::section_title(ui, p, "🎨", "Appearance & behaviour", "Theme, auto-lock and clipboard");
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    if ui.selectable_label(self.dark, "🌙 Dark").clicked() {
                        self.set_theme(true);
                    }
                    if ui.selectable_label(!self.dark, "☀ Light").clicked() {
                        self.set_theme(false);
                    }
                });
                let mut changed = false;
                changed |= ui.add(egui::Slider::new(&mut self.vault.settings.auto_lock_minutes, 1..=60).text("min · auto-lock after inactivity")).changed();
                changed |= ui.add(egui::Slider::new(&mut self.vault.settings.clipboard_clear_secs, 5..=120).text("s · clear clipboard after copy")).changed();
                if changed {
                    let _ = self.vault.save_settings();
                }
            });

            // ── MFA
            card(ui, p, |ui| {
                w::section_title(ui, p, "📱", "Two-factor authentication (optional)", "Require a code from your phone's authenticator app on every unlock");
                if self.vault.settings.mfa_secret.is_some() {
                    ui.label(RichText::new("✅ Enabled — a 6-digit code is required after your master password.").color(p.success));
                    ui.horizontal(|ui| {
                        ui.add_sized([160.0, 32.0], egui::TextEdit::singleline(&mut self.su.mfa_code).hint_text("Current code").margin(Margin::symmetric(10, 8)));
                        if w::danger(ui, p, "Disable 2FA").clicked() {
                            let sec = self.vault.settings.mfa_secret.clone().unwrap_or_default();
                            if let Some(step) = totp::verify(&sec, &self.su.mfa_code, self.vault.settings.mfa_last_step) {
                                self.vault.settings.mfa_secret = None;
                                self.vault.settings.mfa_last_step = step;
                                let _ = self.vault.save_settings();
                                self.toast("Two-factor authentication disabled", true);
                            } else {
                                self.toast("Invalid code", false);
                            }
                            self.su.mfa_code.clear();
                        }
                    });
                } else if let Some(secret) = self.su.mfa_setup.clone() {
                    ui.label("1. Scan this QR code with Google Authenticator, Microsoft Authenticator, Authy, 2FAS or Aegis:");
                    ui.horizontal(|ui| {
                        w::draw_qr(ui, &totp::otpauth_uri(&secret, "vault"), 190.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Or enter the key manually:").color(p.weak).size(12.5));
                            let grouped: String = secret.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join(" ");
                            ui.label(RichText::new(grouped).monospace().size(13.0));
                            if w::ghost(ui, p, "📋 Copy key").clicked() {
                                self.copy(&ctx, &secret, "Secret key");
                            }
                            ui.add_space(8.0);
                            ui.label("2. Enter the 6-digit code shown in the app:");
                            ui.horizontal(|ui| {
                                ui.add_sized([140.0, 32.0], egui::TextEdit::singleline(&mut self.su.mfa_code).hint_text("123456").margin(Margin::symmetric(10, 8)));
                                if w::primary(ui, p, "Enable").clicked() {
                                    if let Some(step) = totp::verify(&secret, &self.su.mfa_code, 0) {
                                        self.vault.settings.mfa_secret = Some(secret.clone());
                                        self.vault.settings.mfa_last_step = step;
                                        match self.vault.save_settings() {
                                            Ok(()) => self.toast("Two-factor authentication enabled", true),
                                            Err(e) => self.toast(e.to_string(), false),
                                        }
                                        self.su.mfa_setup = None;
                                    } else {
                                        self.toast("Code doesn't match — check your phone's clock", false);
                                    }
                                    self.su.mfa_code.clear();
                                }
                            });
                            if w::ghost(ui, p, "Cancel").clicked() {
                                self.su.mfa_setup = None;
                            }
                        });
                    });
                } else {
                    ui.label(RichText::new("Off").color(p.weak));
                    if w::primary(ui, p, "Set up 2FA").clicked() {
                        self.su.mfa_setup = Some(totp::base32_encode(&totp::generate_secret()));
                    }
                }
            });

            // ── Recovery email + hint
            card(ui, p, |ui| {
                w::section_title(ui, p, "📧", "Password hint & recovery Gmail", "Your hint is emailed only to a Gmail address you've verified");
                let rec = self.vault.recovery();
                match &rec {
                    Some(r) if r.verified => {
                        ui.label(RichText::new(format!("✅ Verified: {}", r.email)).color(p.success));
                        if w::ghost(ui, p, "Remove recovery email").clicked() {
                            let _ = self.vault.delete_meta("recovery");
                            self.su.email.clear();
                            self.toast("Recovery email removed", true);
                        }
                    }
                    _ => {
                        w::label(ui, p, "Gmail address");
                        w::text_input(ui, &mut self.su.email, "you@gmail.com", false);
                        w::label(ui, p, "Google app password");
                        w::text_input(ui, &mut self.su.app_pw, "16-character app password", true);
                        ui.label(RichText::new("Create one at myaccount.google.com → Security → 2-Step Verification → App passwords. It's stored encrypted with Windows DPAPI (bound to your Windows account) so the hint can be sent while the vault is locked.").size(11.5).color(p.weak));
                        if self.su.verify_sent && self.su.pending_verify.is_some() {
                            ui.horizontal(|ui| {
                                ui.add_sized([150.0, 32.0], egui::TextEdit::singleline(&mut self.su.verify_input).hint_text("Code from email").margin(Margin::symmetric(10, 8)));
                                if w::primary(ui, p, "Verify").clicked() {
                                    let (email, app_pw, code, sent) = self.su.pending_verify.as_ref().unwrap();
                                    if sent.elapsed() > Duration::from_secs(600) {
                                        self.toast("Code expired — send a new one", false);
                                    } else if self.su.verify_input.trim() == code.as_str() {
                                        let r = Recovery { email: email.clone(), app_password: app_pw.to_string(), verified: true };
                                        match self.vault.set_recovery(&r) {
                                            Ok(()) => self.toast("Gmail verified ✓", true),
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
                            });
                        }
                        if self.mail_busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Sending…");
                            });
                        } else if w::ghost(ui, p, "✉ Send verification code").clicked() {
                            if !mailer::is_gmail(&self.su.email) {
                                self.toast("Please enter a @gmail.com address", false);
                            } else if self.su.app_pw.replace(' ', "").len() < 16 {
                                self.toast("Enter your 16-character Google App Password", false);
                            } else {
                                use rand::Rng;
                                let code = Zeroizing::new(format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32)));
                                let email = self.su.email.trim().to_string();
                                let app_pw = self.su.app_pw.clone();
                                self.su.pending_verify = Some((email.clone(), app_pw.clone(), code.clone(), Instant::now()));
                                self.su.verify_sent = false;
                                self.mail_busy = true;
                                self.spawn(&ctx, None, move || {
                                    let body = format!("Your SoftVault verification code is: {}\n\nIt expires in 10 minutes.", code.as_str());
                                    JobResult::VerifyMailSent(mailer::send_to_self(&email, &app_pw, "SoftVault verification code", body))
                                });
                            }
                        }
                    }
                }
                ui.separator();
                w::label(ui, p, "Master password hint");
                w::row_rtl(ui, |ui| {
                    if w::primary(ui, p, "Save hint").clicked() {
                        match self.vault.set_hint(&self.su.hint) {
                            Ok(()) => self.toast("Hint saved", true),
                            Err(e) => self.toast(e.to_string(), false),
                        }
                    }
                    ui.add_sized([ui.available_width(), 34.0], egui::TextEdit::singleline(&mut *self.su.hint).hint_text("No hint set").margin(Margin::symmetric(12, 9)));
                });
                if rec.map(|r| r.verified) != Some(true) {
                    ui.label(RichText::new("⚠ The hint can't be retrieved until a Gmail address is verified.").size(12.0).color(p.warn));
                }
            });

            // ── Change master password
            card(ui, p, |ui| {
                w::section_title(ui, p, "🔑", "Change master password", "Re-wraps your vault key with a new Argon2id-derived key");
                w::text_input(ui, &mut self.su.old_pw, "Current master password", true);
                w::text_input(ui, &mut self.su.new_pw, "New master password (12+ characters)", true);
                let (sc, lb, bits) = generator::strength(&self.su.new_pw);
                w::strength_bar(ui, p, sc, lb, bits);
                w::text_input(ui, &mut self.su.new_pw2, "Confirm new master password", true);
                if self.busy.is_some() {
                    ui.spinner();
                } else if w::primary(ui, p, "Change password").clicked() {
                    if self.su.new_pw.chars().count() < 12 {
                        self.toast("New password must be at least 12 characters", false);
                    } else if *self.su.new_pw != *self.su.new_pw2 {
                        self.toast("New passwords don't match", false);
                    } else if let Ok(m) = self.vault.unlock_material() {
                        let old = Zeroizing::new(self.su.old_pw.as_bytes().to_vec());
                        let new = Zeroizing::new(self.su.new_pw.as_bytes().to_vec());
                        self.spawn(&ctx, Some("Re-encrypting vault key…"), move || {
                            JobResult::Rewrapped(
                                crypto::unwrap_vault_key(&old, &m.params, &m.salt, &m.wrapped).and_then(|k| crypto::rewrap(&k, &new)),
                            )
                        });
                    }
                }
            });

            // ── Export / Import
            card(ui, p, |ui| {
                w::section_title(ui, p, "📦", "Export & import", "Back up every password and note");
                ui.label(RichText::new("🔐 Encrypted export (.svx)").strong());
                ui.label(RichText::new("Argon2id + XChaCha20-Poly1305 with a password you choose (you can reuse your master password).").size(12.0).color(p.weak));
                w::text_input(ui, &mut self.su.export_pw, "Export password (12+ characters)", true);
                w::text_input(ui, &mut self.su.export_pw2, "Confirm export password", true);
                if let Some(b) = &self.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(b.as_str());
                    });
                } else if w::primary(ui, p, "Export encrypted…").clicked() {
                    if self.su.export_pw.chars().count() < 12 {
                        self.toast("Export password must be at least 12 characters", false);
                    } else if *self.su.export_pw != *self.su.export_pw2 {
                        self.toast("Export passwords don't match", false);
                    } else if let Some(path) = rfd::FileDialog::new()
                        .set_file_name(format!("softvault-backup-{}.svx", chrono::Local::now().format("%Y-%m-%d")))
                        .add_filter("SoftVault encrypted backup", &["svx"])
                        .save_file()
                    {
                        match self.vault.export_json() {
                            Ok(json) => {
                                let pw = Zeroizing::new(self.su.export_pw.as_bytes().to_vec());
                                self.spawn(&ctx, Some("Encrypting export…"), move || {
                                    JobResult::Exported(crypto::encrypt_export(&json, &pw), path)
                                });
                            }
                            Err(e) => self.toast(e.to_string(), false),
                        }
                    }
                }
                ui.add_space(6.0);
                ui.separator();
                ui.label(RichText::new("⚠ Unencrypted export (.json)").strong().color(p.warn));
                ui.checkbox(&mut self.su.plain_ack, "I understand this file contains ALL my passwords and notes in plain text");
                ui.add_enabled_ui(self.su.plain_ack, |ui| {
                    if w::ghost(ui, p, "Export unencrypted…").clicked()
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
                ui.add_space(6.0);
                ui.separator();
                ui.label(RichText::new("📥 Import a SoftVault export").strong());
                if self.su.import_bytes.is_some() {
                    ui.label("This file is encrypted — enter its export password:");
                    w::row_rtl(ui, |ui| {
                        if w::ghost(ui, p, "Cancel").clicked() {
                            self.su.import_bytes = None;
                        }
                        if self.busy.is_none() && w::primary(ui, p, "Decrypt & import").clicked() {
                            let bytes = self.su.import_bytes.clone().unwrap();
                            let pw = Zeroizing::new(self.su.import_pw.as_bytes().to_vec());
                            self.spawn(&ctx, Some("Decrypting import…"), move || {
                                JobResult::ImportDecrypted(crypto::decrypt_export(&bytes, &pw).and_then(|pt| vault::parse_plain_export(&pt)))
                            });
                        }
                        ui.add_sized([ui.available_width(), 34.0], egui::TextEdit::singleline(&mut *self.su.import_pw).password(true).hint_text("Export password").margin(Margin::symmetric(12, 9)));
                    });
                } else if w::ghost(ui, p, "Choose file…").clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter("SoftVault export", &["svx", "json"]).pick_file() {
                        match std::fs::read(&path) {
                            Ok(bytes) if vault::is_encrypted_export(&bytes) => self.su.import_bytes = Some(bytes),
                            Ok(bytes) => match vault::parse_plain_export(&bytes).and_then(|items| self.vault.import_items(items)) {
                                Ok(n) => self.toast(format!("Imported {n} entries"), true),
                                Err(e) => self.toast(format!("Import failed: {e}"), false),
                            },
                            Err(e) => self.toast(e.to_string(), false),
                        }
                    }
            });

            // ── About
            card(ui, p, |ui| {
                w::section_title(ui, p, "🛡", "Security model", "");
                for line in [
                    "• Zero-knowledge: your master password is never stored; only an authenticated ciphertext of a random vault key is kept.",
                    "• Key derivation: Argon2id, 256 MiB memory, 3 iterations, 4 lanes, 256-bit random salt.",
                    "• Encryption: XChaCha20-Poly1305 with a fresh 192-bit random nonce per record, bound to its record ID (AAD).",
                    "• Titles, usernames, URLs, timestamps and item types are all encrypted — SQLite only sees random IDs and ciphertext.",
                    "• Decrypted data is wiped from memory (zeroize) when the vault locks.",
                ] {
                    ui.label(RichText::new(line).size(12.5).color(p.weak));
                }
                ui.label(RichText::new(format!("Database: {}", self.vault.path.display())).size(11.5).color(p.weak));
            });
            ui.add_space(20.0);
        });
    }

    fn ui_toast(&mut self, ctx: &egui::Context, p: &Palette) {
        if let Some(t) = &self.toast {
            egui::Area::new(egui::Id::unique("softvault-toast"))
                .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-22.0, -22.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0, if t.ok { p.success } else { p.danger }))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(16, 12))
                        .shadow(theme::soft_shadow(p))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.label(RichText::new(format!("{}  {}", if t.ok { "✔" } else { "✖" }, t.text)).color(if t.ok { p.success } else { p.danger }));
                        });
                });
        }
    }
}

impl eframe::App for SoftVaultApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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
        self.ui_toast(&ctx, &p);
    }
}
