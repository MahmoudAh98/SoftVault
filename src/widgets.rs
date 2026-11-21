//! Bitwarden-style widgets: pill buttons, flat bordered sections, navy sidebar
//! entries, filter chips and table rows.

use crate::theme::{semibold, Palette, PILL, R_CARD, R_INPUT};
use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Margin, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

pub fn h(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).size(size).family(semibold())
}

/// Flat bordered section ("bit-section" look).
pub fn card<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(R_CARD)
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn section_title(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(17.0).color(p.accent));
        ui.label(h(title, 16.5));
    });
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).size(13.0).color(p.weak));
    }
    ui.add_space(6.0);
}

/// Field label (Bitwarden uses sentence-case semibold labels above inputs).
pub fn label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(2.0);
    ui.label(h(text, 13.0).color(p.text));
}

fn pill_button(text: RichText, fill: Color32, stroke: Stroke, min: Vec2) -> egui::Button<'static> {
    egui::Button::new((egui::Atom::grow(), text, egui::Atom::grow()))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(PILL)
        .min_size(min)
}

pub fn primary(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(pill_button(h(text, 14.0).color(p.on_accent), p.accent, Stroke::NONE, Vec2::new(96.0, 36.0)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn primary_wide(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let w = ui.available_width();
    ui.add(pill_button(h(text, 14.5).color(p.on_accent), p.accent, Stroke::NONE, Vec2::new(w, 40.0)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Secondary (outlined) pill button.
pub fn ghost(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.add(pill_button(h(text, 14.0).color(p.accent), Color32::TRANSPARENT, Stroke::new(1.0, p.accent), Vec2::new(0.0, 36.0)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn ghost_wide(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let w = ui.available_width();
    ui.add(pill_button(h(text, 14.0).color(p.accent), Color32::TRANSPARENT, Stroke::new(1.0, p.accent), Vec2::new(w, 38.0)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn danger(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let on = if p.dark { Color32::from_rgb(0x1f, 0x24, 0x2e) } else { Color32::WHITE };
    ui.add(pill_button(h(text, 14.0).color(on), p.danger, Stroke::NONE, Vec2::new(0.0, 36.0)))
}

pub fn icon_btn(ui: &mut Ui, p: &Palette, icon: &str, tip: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(icon).size(14.0).color(p.weak))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(R_INPUT)
            .min_size(Vec2::new(34.0, 34.0)),
    )
    .on_hover_text(tip)
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row whose widgets are added right-to-left, so a final text field can take the remaining width.
pub fn row_rtl<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(Vec2::new(w, 36.0), egui::Layout::right_to_left(egui::Align::Center), add).inner
}

pub fn text_input(ui: &mut Ui, s: &mut String, hint: &str, password: bool) -> Response {
    ui.add(
        egui::TextEdit::singleline(s)
            .hint_text(hint)
            .password(password)
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(12, 9)),
    )
}

pub fn nav_section(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(8.0);
    ui.label(h(text, 11.5).color(p.nav_weak));
}

/// Sidebar navigation entry (navy sidebar, solid blue active pill).
pub fn nav_item(ui: &mut Ui, p: &Palette, icon: &str, text: &str, selected: bool, count: Option<usize>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let painter = ui.painter();
    let bg = if selected {
        p.nav_active
    } else if resp.hovered() {
        p.nav_hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, R_INPUT, bg);
    let col = if selected { p.nav_active_text } else { p.nav_text };
    painter.text(rect.left_center() + Vec2::new(14.0, 0.0), Align2::LEFT_CENTER, icon, FontId::proportional(14.0), col);
    painter.text(
        rect.left_center() + Vec2::new(40.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        FontId::new(14.0, if selected { semibold() } else { egui::FontFamily::Proportional }),
        col,
    );
    if let Some(c) = count {
        painter.text(
            rect.right_center() - Vec2::new(14.0, 0.0),
            Align2::RIGHT_CENTER,
            c.to_string(),
            FontId::proportional(12.0),
            if selected { p.nav_active_text } else { p.nav_weak },
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded filter chip ("Type: All", "Favorites", …).
pub fn chip(ui: &mut Ui, p: &Palette, text: &str, selected: bool) -> Response {
    let (fill, stroke, col) = if selected {
        (p.accent_soft, Stroke::new(1.0, p.accent), p.accent)
    } else {
        (Color32::TRANSPARENT, Stroke::new(1.0, p.border_strong.gamma_multiply(0.6)), p.text)
    };
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).color(col))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(PILL)
            .min_size(Vec2::new(0.0, 28.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn avatar_color(seed: &str) -> Color32 {
    const COLS: [Color32; 8] = [
        Color32::from_rgb(0x17, 0x5d, 0xdc),
        Color32::from_rgb(0x01, 0x7e, 0x45),
        Color32::from_rgb(0xd9, 0x4f, 0x16),
        Color32::from_rgb(0x7c, 0x3a, 0xed),
        Color32::from_rgb(0xc0, 0x26, 0x5d),
        Color32::from_rgb(0x0e, 0x7c, 0x86),
        Color32::from_rgb(0xb4, 0x6b, 0x05),
        Color32::from_rgb(0x4b, 0x55, 0x63),
    ];
    let h = seed.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    COLS[(h % 8) as usize]
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Column x-offsets for the vault table: icon | name | type | updated | options
pub struct Cols {
    pub name: f32,
    pub kind: f32,
    pub updated: f32,
    pub options: f32,
}

pub fn cols(rect: Rect) -> Cols {
    let w = rect.width();
    Cols {
        name: rect.left() + 52.0,
        kind: rect.left() + (w * 0.52).max(260.0),
        updated: rect.left() + (w * 0.72).max(400.0),
        options: rect.right() - 56.0,
    }
}

pub fn table_header(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
    let c = cols(rect);
    let painter = ui.painter();
    let f = FontId::new(12.5, semibold());
    let y = rect.center().y;
    painter.text(egui::pos2(c.name, y), Align2::LEFT_CENTER, "Name", f.clone(), p.weak);
    painter.text(egui::pos2(c.kind, y), Align2::LEFT_CENTER, "Type", f.clone(), p.weak);
    painter.text(egui::pos2(c.updated, y), Align2::LEFT_CENTER, "Updated", f.clone(), p.weak);
    painter.text(egui::pos2(c.options + 20.0, y), Align2::CENTER_CENTER, "Options", f, p.weak);
    painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));
}

/// One table row; returns the row response and the rect reserved for the options button.
#[allow(clippy::too_many_arguments)]
pub fn table_row(
    ui: &mut Ui,
    p: &Palette,
    title: &str,
    subtitle: &str,
    note: bool,
    badge: &str,
    favorite: bool,
    updated: &str,
    selected: bool,
) -> (Response, Rect) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
    let c = cols(rect);
    let painter = ui.painter();
    if selected || resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, if selected { p.accent_soft } else { p.card_hover });
    }
    painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));

    // favicon-like tile
    let ic = egui::pos2(rect.left() + 26.0, rect.center().y);
    let col = if note { p.weak } else { avatar_color(title) };
    let tile = Rect::from_center_size(ic, Vec2::splat(24.0));
    painter.rect_filled(tile, CornerRadius::same(6), col.gamma_multiply(if p.dark { 0.35 } else { 0.14 }));
    let glyph = if note {
        "📝".to_string()
    } else {
        title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into())
    };
    painter.text(ic, Align2::CENTER_CENTER, glyph, FontId::new(12.5, semibold()), if p.dark { col.gamma_multiply(1.6) } else { col });

    let name_w = (c.kind - c.name - 16.0).max(60.0);
    let max_chars = (name_w / 7.2) as usize;
    let t = if title.is_empty() { "(untitled)" } else { title };
    let title_txt = truncate(t, max_chars.saturating_sub(if favorite { 2 } else { 0 }));
    let tg = painter.layout_no_wrap(title_txt, FontId::new(14.0, semibold()), p.accent);
    let tw = tg.size().x;
    painter.galley(egui::pos2(c.name, rect.center().y - 17.0), tg, p.accent);
    if favorite {
        painter.text(egui::pos2(c.name + tw + 6.0, rect.center().y - 9.5), Align2::LEFT_CENTER, "★", FontId::proportional(12.0), p.warn);
    }
    painter.text(egui::pos2(c.name, rect.center().y + 9.0), Align2::LEFT_CENTER, truncate(subtitle, max_chars + 4), FontId::proportional(12.5), p.weak);

    // type badge
    let g = painter.layout_no_wrap(badge.into(), FontId::proportional(12.0), p.text);
    let br = Rect::from_min_size(egui::pos2(c.kind, rect.center().y - 11.0), g.size() + Vec2::new(16.0, 8.0));
    painter.rect_filled(br, CornerRadius::same(6), if p.dark { p.card_hover } else { p.bg_alt });
    painter.rect_stroke(br, CornerRadius::same(6), Stroke::new(1.0, p.border), StrokeKind::Inside);
    painter.galley(br.min + Vec2::new(8.0, 4.0), g, p.text);

    painter.text(egui::pos2(c.updated, rect.center().y), Align2::LEFT_CENTER, updated, FontId::proportional(12.5), p.weak);

    let opt = Rect::from_center_size(egui::pos2(c.options + 20.0, rect.center().y), Vec2::new(32.0, 32.0));
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), opt)
}

pub fn strength_bar(ui: &mut Ui, p: &Palette, score: u8, label: &str, bits: f64) {
    ui.horizontal(|ui| {
        let colors = [p.danger, p.danger, p.warn, p.success, p.success];
        let w = ((ui.available_width() - 160.0) / 4.0).max(20.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        for i in 0..4u8 {
            let (r, _) = ui.allocate_exact_size(Vec2::new(w, 5.0), Sense::hover());
            let c = if i < score.max(1) && bits > 0.0 { colors[score as usize] } else { p.border };
            ui.painter().rect_filled(r, CornerRadius::same(3), c);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{label} · {:.0} bits", bits)).size(12.0).color(colors[score as usize]));
    });
}

/// SoftVault logo: solid primary tile with a lock glyph.
pub fn logo(ui: &mut Ui, p: &Palette, size: f32, on_dark_bg: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let (fill, fg) = if on_dark_bg { (Color32::WHITE, Color32::from_rgb(0x17, 0x5d, 0xdc)) } else { (p.accent, p.on_accent) };
    painter.rect_filled(rect, CornerRadius::same((size * 0.24) as u8), fill);
    painter.text(rect.center(), Align2::CENTER_CENTER, "🔐", FontId::proportional(size * 0.52), fg);
}

pub fn draw_qr(ui: &mut Ui, data: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(data.as_bytes()) else {
        ui.label("QR generation failed");
        return;
    };
    let w = code.width();
    let colors = code.to_colors();
    let quiet = 3usize;
    let total = (w + quiet * 2) as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, R_INPUT, Color32::WHITE);
    let cell = size / total;
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                let min = rect.min + Vec2::new((x + quiet) as f32 * cell, (y + quiet) as f32 * cell);
                painter.rect_filled(Rect::from_min_size(min, Vec2::splat(cell + 0.6)), CornerRadius::ZERO, Color32::BLACK);
            }
        }
    }
}

/// Page title with a small primary-colored icon tile (as in the Bitwarden web vault).
pub fn page_header(ui: &mut Ui, p: &Palette, icon: &str, title: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(6), p.accent);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(13.0), p.on_accent);
        ui.add_space(4.0);
        ui.label(h(title, 23.0));
    });
}
