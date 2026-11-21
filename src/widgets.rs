//! Bitwarden-style widgets: pill buttons, flat bordered sections, navy sidebar
//! entries, filter chips and table rows.

use crate::icons as ic;
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

pub fn icon_tile(ui: &mut Ui, p: &Palette, icon: &str, size: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same((size * 0.28) as u8), p.accent_soft);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(size * 0.5), p.accent);
}

pub fn section_title(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        icon_tile(ui, p, icon, 36.0);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(h(title, 16.0));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(12.5).color(p.weak));
            }
        });
    });
    ui.add_space(10.0);
}

/// iOS-style switch.
pub fn toggle(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let off_col = if p.dark { p.card_hover } else { Color32::from_rgb(0xcf, 0xd5, 0xdf) };
    let bg = crate::widgets::lerp(off_col, p.accent, t);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(11), bg);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    painter.circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Settings row: title + description on the left, control on the right, divider below.
pub fn setting_row(ui: &mut Ui, p: &Palette, title: &str, desc: &str, last: bool, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let w = ui.available_width();
        ui.allocate_ui_with_layout(Vec2::new((w * 0.55).max(260.0), 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(h(title, 14.0));
            if !desc.is_empty() {
                ui.add(egui::Label::new(RichText::new(desc).size(12.5).color(p.weak)).wrap());
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
    });
    if !last {
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::ZERO, p.border);
        ui.add_space(4.0);
    }
}

/// Underlined tab bar. Returns the clicked index.
pub fn tabs(ui: &mut Ui, p: &Palette, items: &[(&str, &str)], selected: usize) -> Option<usize> {
    let mut clicked = None;
    let start = ui.cursor().min;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, (icon, text)) in items.iter().enumerate() {
            let sel = i == selected;
            let col = if sel { p.accent } else { p.weak };
            let label = RichText::new(format!("{icon}  {text}")).size(14.0).color(col).family(if sel { semibold() } else { egui::FontFamily::Proportional });
            let r = ui.add(egui::Button::new(label).frame(false).min_size(Vec2::new(0.0, 38.0)));
            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
            if sel {
                let bar = Rect::from_min_max(egui::pos2(r.rect.left(), r.rect.bottom() - 1.0), egui::pos2(r.rect.right(), r.rect.bottom() + 2.0));
                ui.painter().rect_filled(bar, CornerRadius::same(1), p.accent);
            } else if r.hovered() {
                let bar = Rect::from_min_max(egui::pos2(r.rect.left(), r.rect.bottom() - 1.0), egui::pos2(r.rect.right(), r.rect.bottom() + 2.0));
                ui.painter().rect_filled(bar, CornerRadius::same(1), p.border_strong.gamma_multiply(0.5));
            }
            if r.clicked() {
                clicked = Some(i);
            }
            ui.add_space(14.0);
        }
    });
    let y = ui.cursor().min.y;
    let line = Rect::from_min_max(egui::pos2(start.x, y), egui::pos2(start.x + ui.available_width(), y + 1.0));
    ui.painter().rect_filled(line, CornerRadius::ZERO, p.border);
    ui.add_space(14.0);
    clicked
}

/// Dashboard number tile.
pub fn stat_tile(ui: &mut Ui, p: &Palette, icon: &str, value: &str, label: &str, tone: Color32, width: f32) -> Response {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(R_CARD)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(width - 38.0);
            ui.set_height(48.0);
            ui.horizontal_centered(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(11), tone.gamma_multiply(if p.dark { 0.22 } else { 0.12 }));
                ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(19.0), tone);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(h(value, 24.0));
                    ui.add(egui::Label::new(RichText::new(label).size(12.5).color(p.weak)).truncate());
                });
            });
        })
        .response
}

pub fn empty_state(ui: &mut Ui, p: &Palette, icon: &str, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 32.0, p.accent_soft);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(28.0), p.accent);
        ui.add_space(10.0);
        ui.label(h(title, 17.0));
        ui.label(RichText::new(body).size(13.5).color(p.weak));
        ui.add_space(8.0);
    });
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

/// Borderless round icon button (row quick actions, toolbars).
pub fn ghost_icon(ui: &mut Ui, p: &Palette, icon: &str, tip: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(icon).size(15.0).color(p.weak))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(PILL)
            .min_size(Vec2::splat(30.0)),
    )
    .on_hover_text(tip)
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn icon_btn(ui: &mut Ui, p: &Palette, icon: &str, tip: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(icon).size(15.0).color(p.weak))
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
    let ht = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let st = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, 0.18);
    let painter = ui.painter();
    let bg = lerp(lerp(Color32::TRANSPARENT, p.nav_hover, ht), p.nav_active, st);
    // selected pill grows in from the left
    let r = Rect::from_min_size(rect.min, Vec2::new(rect.width() * (0.6 + 0.4 * st.max(ht)), rect.height()));
    painter.rect_filled(if st > 0.0 { rect } else { r }, R_INPUT, bg);
    let col = if selected { p.nav_active_text } else { p.nav_text };
    painter.text(rect.left_center() + Vec2::new(14.0, 0.0), Align2::LEFT_CENTER, icon, FontId::proportional(16.0), col);
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

pub struct Cols {
    pub name: f32,
    pub folder: f32,
    pub kind: f32,
    pub views: f32,
    pub updated: f32,
    pub options: f32,
}

pub fn cols(rect: Rect) -> Cols {
    let w = rect.width();
    let x = rect.left();
    Cols {
        name: x + 52.0,
        folder: x + (w * 0.42).max(240.0),
        kind: x + (w * 0.59).max(360.0),
        views: x + (w * 0.73).max(450.0),
        updated: x + (w * 0.80).max(500.0),
        options: rect.right() - 56.0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    Name,
    Folder,
    Type,
    Views,
    Updated,
}

/// Sortable column header. `sort` = (column, ascending). Returns a clicked column.
pub fn table_header(ui: &mut Ui, p: &Palette, sort: (SortCol, bool)) -> Option<SortCol> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::hover());
    let c = cols(rect);
    let y = rect.center().y;
    let mut clicked = None;
    for (col, x, next, text) in [
        (SortCol::Name, c.name, c.folder, "Name"),
        (SortCol::Folder, c.folder, c.kind, "Folder"),
        (SortCol::Type, c.kind, c.views, "Type"),
        (SortCol::Views, c.views, c.updated, "Views"),
        (SortCol::Updated, c.updated, c.options, "Updated"),
    ] {
        let hit = Rect::from_min_size(egui::pos2(x - 6.0, rect.top()), Vec2::new((next - x - 4.0).max(40.0), rect.height()));
        let r = ui.interact(hit, egui::Id::unique("softvault-sort").with(text), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = sort.0 == col;
        let colr = if active || r.hovered() { p.text } else { p.weak };
        let g = ui.painter().layout_no_wrap(text.into(), FontId::new(12.5, semibold()), colr);
        let tw = g.size().x;
        ui.painter().galley(egui::pos2(x, y - g.size().y / 2.0), g, colr);
        if active {
            let arrow = if sort.1 { ic::ARROW_UP } else { ic::ARROW_DOWN };
            ui.painter().text(egui::pos2(x + tw + 6.0, y), Align2::LEFT_CENTER, arrow, FontId::proportional(12.0), p.accent);
        }
        if r.clicked() {
            clicked = Some(col);
        }
    }
    ui.painter().text(egui::pos2(c.options + 20.0, y), Align2::CENTER_CENTER, "Actions", FontId::new(12.5, semibold()), p.weak);
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));
    clicked
}

/// A small rounded tag pill; returns its width.
pub fn paint_tag(painter: &egui::Painter, pos: egui::Pos2, tag: &str, p: &Palette) -> f32 {
    let col = avatar_color(tag);
    let g = painter.layout_no_wrap(format!("#{tag}"), FontId::proportional(11.0), if p.dark { lerp(col, Color32::WHITE, 0.45) } else { col });
    let r = Rect::from_min_size(pos, g.size() + Vec2::new(12.0, 4.0));
    painter.rect_filled(r, PILL, col.gamma_multiply(if p.dark { 0.28 } else { 0.12 }));
    painter.galley(r.min + Vec2::new(6.0, 2.0), g, Color32::WHITE);
    r.width()
}

pub struct RowData<'a> {
    pub title: &'a str,
    pub subtitle: &'a str,
    pub note: bool,
    pub note_icon: &'a str,
    pub badge: &'a str,
    pub favorite: bool,
    pub folder: &'a str,
    pub tags: &'a [String],
    pub views: u64,
    pub updated: &'a str,
    pub selected: bool,
    pub drop_target: bool,
}

/// One table row; returns the row response and the rect reserved for the options button.
/// Rows are click-and-drag: drag onto a folder to move, or out of the window to export.
pub fn table_row(ui: &mut Ui, p: &Palette, d: &RowData) -> (Response, Rect) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), Sense::click_and_drag());
    let c = cols(rect);
    let hovered = ui.rect_contains_pointer(rect);
    let painter = ui.painter();
    let ht = ui.ctx().animate_bool_with_time(resp.id.with("hover"), hovered, 0.12);
    let bg = if d.selected { p.accent_soft } else { lerp(p.card, p.card_hover, ht) };
    if d.selected || ht > 0.0 {
        painter.rect_filled(rect, CornerRadius::same(8), bg);
    }
    if resp.dragged() {
        painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, p.border));

    // favicon-like tile
    let icp = egui::pos2(rect.left() + 28.0, rect.center().y);
    let col = if d.note { p.weak } else { avatar_color(d.title) };
    let tile = Rect::from_center_size(icp, Vec2::splat(30.0));
    painter.rect_filled(tile, CornerRadius::same(8), col.gamma_multiply(if p.dark { 0.30 } else { 0.12 }));
    let glyph = if d.note { d.note_icon.to_string() } else { d.title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into()) };
    painter.text(icp, Align2::CENTER_CENTER, glyph, FontId::new(if d.note { 15.0 } else { 13.5 }, semibold()), if p.dark { lerp(col, Color32::WHITE, 0.35) } else { col });

    // name + favorite + subtitle + tags
    let name_w = (c.folder - c.name - 16.0).max(60.0);
    let max_chars = (name_w / 7.2) as usize;
    let t = if d.title.is_empty() { "(untitled)" } else { d.title };
    let tg = painter.layout_no_wrap(truncate(t, max_chars.saturating_sub(if d.favorite { 2 } else { 0 })), FontId::new(14.0, semibold()), p.accent);
    let tw = tg.size().x;
    painter.galley(egui::pos2(c.name, rect.center().y - 17.0), tg, p.accent);
    if d.favorite {
        painter.text(egui::pos2(c.name + tw + 6.0, rect.center().y - 9.5), Align2::LEFT_CENTER, ic::STAR, FontId::proportional(12.0), p.warn);
    }
    let sub_g = painter.layout_no_wrap(truncate(d.subtitle, (max_chars / 2).max(12)), FontId::proportional(12.5), p.weak);
    let sub_w = if d.subtitle.is_empty() { 0.0 } else { sub_g.size().x + 8.0 };
    painter.galley(egui::pos2(c.name, rect.center().y + 2.0), sub_g, p.weak);
    let mut x = c.name + sub_w;
    let clip = painter.with_clip_rect(Rect::from_min_max(egui::pos2(c.name, rect.top()), egui::pos2(c.folder - 10.0, rect.bottom())));
    for tag in d.tags.iter().take(4) {
        x += paint_tag(&clip, egui::pos2(x, rect.center().y + 2.0), tag, p) + 4.0;
    }

    // folder
    if !d.folder.is_empty() {
        let fmax = ((c.kind - c.folder - 30.0) / 7.0).max(4.0) as usize;
        painter.text(egui::pos2(c.folder, rect.center().y), Align2::LEFT_CENTER, ic::FOLDER, FontId::proportional(13.0), p.weak);
        painter.text(egui::pos2(c.folder + 18.0, rect.center().y), Align2::LEFT_CENTER, truncate(d.folder, fmax), FontId::proportional(12.5), p.text);
    } else {
        painter.text(egui::pos2(c.folder, rect.center().y), Align2::LEFT_CENTER, "—", FontId::proportional(12.5), p.weak.gamma_multiply(0.6));
    }

    // type badge
    let g = painter.layout_no_wrap(d.badge.into(), FontId::proportional(12.0), p.text);
    let br = Rect::from_min_size(egui::pos2(c.kind, rect.center().y - 11.0), g.size() + Vec2::new(16.0, 8.0));
    painter.rect_filled(br, CornerRadius::same(6), if p.dark { p.card_hover } else { p.bg_alt });
    painter.rect_stroke(br, CornerRadius::same(6), Stroke::new(1.0, p.border), StrokeKind::Inside);
    painter.galley(br.min + Vec2::new(8.0, 4.0), g, p.text);

    if !hovered {
        // on hover the views/updated columns are replaced by quick actions
        painter.text(egui::pos2(c.views, rect.center().y), Align2::LEFT_CENTER, format!("{}  {}", ic::EYE, d.views), FontId::proportional(12.5), p.weak);
        painter.text(egui::pos2(c.updated, rect.center().y), Align2::LEFT_CENTER, d.updated, FontId::proportional(12.5), p.weak);
    }
    if d.drop_target {
        painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(2.0, p.accent), StrokeKind::Inside);
    }
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
    painter.text(rect.center(), Align2::CENTER_CENTER, ic::VAULT, FontId::proportional(size * 0.56), fg);
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
pub fn page_header(ui: &mut Ui, p: &Palette, icon: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(11), p.accent);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(20.0), p.on_accent);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(h(title, 22.0));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(12.5).color(p.weak));
            }
        });
    });
}

// ───────────────────────────── animated recycle bin ─────────────────────────────

fn rot(p: egui::Pos2, pivot: egui::Pos2, ang: f32) -> egui::Pos2 {
    let (s, c) = ang.sin_cos();
    let d = p - pivot;
    pivot + Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c)
}

/// Paint a vector trash can centered at `c`.
/// `lid` 0..1 = how far the lid is open, `drop` = Some(0..1) while a page falls in.
fn paint_bin(painter: &egui::Painter, c: egui::Pos2, scale: f32, col: Color32, lid: f32, drop: Option<f32>, paper: Color32) {
    let s = scale;
    let stroke = Stroke::new(1.8 * s, col);
    // falling page (drawn first so the bin body covers it as it lands)
    if let Some(d) = drop {
        let fall = ((d - 0.12) / 0.55).clamp(0.0, 1.0);
        if fall > 0.0 && fall < 1.0 {
            let eased = fall * fall;
            let y = c.y - 19.0 * s + eased * 18.0 * s;
            let tilt = (1.0 - fall) * 0.5;
            let r = Rect::from_center_size(egui::pos2(c.x - 1.0 * s, y), Vec2::new(7.0 * s, 8.5 * s));
            let pts: Vec<egui::Pos2> = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].iter().map(|q| rot(*q, r.center(), tilt)).collect();
            painter.add(egui::Shape::convex_polygon(pts.clone(), paper, Stroke::new(1.0 * s, col)));
            // text lines on the page
            for k in 0..2 {
                let a = rot(egui::pos2(r.left() + 1.6 * s, r.top() + (2.6 + k as f32 * 2.4) * s), r.center(), tilt);
                let b = rot(egui::pos2(r.right() - 1.6 * s, r.top() + (2.6 + k as f32 * 2.4) * s), r.center(), tilt);
                painter.line_segment([a, b], Stroke::new(0.9 * s, col));
            }
        }
    }
    // body (slightly tapered)
    let top = c.y - 3.0 * s;
    let bot = c.y + 8.5 * s;
    let body = vec![egui::pos2(c.x - 6.2 * s, top), egui::pos2(c.x + 6.2 * s, top), egui::pos2(c.x + 5.0 * s, bot), egui::pos2(c.x - 5.0 * s, bot)];
    painter.add(egui::Shape::closed_line(body, stroke));
    for dx in [-2.3f32, 0.0, 2.3] {
        painter.line_segment([egui::pos2(c.x + dx * s, top + 2.6 * s), egui::pos2(c.x + dx * 0.85 * s, bot - 2.4 * s)], Stroke::new(1.4 * s, col));
    }
    // lid hinged on the right edge, swinging up and back
    let pivot = egui::pos2(c.x + 7.6 * s, c.y - 5.6 * s);
    let ang = 0.75 * lid; // screen y points down: positive = counter-clockwise visually (left end lifts)
    let lift = Vec2::new(0.0, -1.6 * s * lid);
    let l1 = rot(egui::pos2(c.x - 7.6 * s, c.y - 5.6 * s), pivot, ang) + lift;
    let l2 = pivot + lift;
    painter.line_segment([l1, l2], Stroke::new(2.0 * s, col));
    let h1 = rot(egui::pos2(c.x - 2.4 * s, c.y - 5.6 * s), pivot, ang) + lift;
    let h2 = rot(egui::pos2(c.x - 2.4 * s, c.y - 8.2 * s), pivot, ang) + lift;
    let h3 = rot(egui::pos2(c.x + 2.4 * s, c.y - 8.2 * s), pivot, ang) + lift;
    let h4 = rot(egui::pos2(c.x + 2.4 * s, c.y - 5.6 * s), pivot, ang) + lift;
    painter.add(egui::Shape::line(vec![h1, h2, h3, h4], Stroke::new(1.6 * s, col)));
}

/// Animated delete button.
/// * idle: outlined red circle; on hover the lid lifts.
/// * `armed`: expands into a red pill "Click to delete" and the bin wiggles.
/// * `dropping = Some(t)`: a page falls into the bin, then the lid shuts.
pub fn trash_button(ui: &mut Ui, p: &Palette, id_salt: &str, armed: bool, dropping: Option<f32>) -> Response {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique(id_salt);
    let armed_t = ctx.animate_bool_with_time(id.with("armed"), armed || dropping.is_some(), 0.22);
    let width = 36.0 + armed_t * 132.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::click());
    let hover_t = ctx.animate_bool_with_time(id.with("hover"), resp.hovered() && !armed, 0.16);
    let painter = ui.painter();

    let on_red = if p.dark { Color32::from_rgb(0x1f, 0x24, 0x2e) } else { Color32::WHITE };
    let bg = lerp(Color32::TRANSPARENT, p.danger, armed_t);
    let bg = if resp.hovered() && !armed { lerp(bg, p.danger.gamma_multiply(0.12), hover_t) } else { bg };
    painter.rect_filled(rect, PILL, bg);
    painter.rect_stroke(rect, PILL, Stroke::new(1.0, p.danger), StrokeKind::Inside);

    let icon_col = lerp(p.danger, on_red, armed_t);
    let mut c = egui::pos2(rect.left() + 18.0, rect.center().y + 1.0);
    let mut lid = hover_t;
    if armed && dropping.is_none() {
        // nervous wiggle
        let time = ctx.input(|i| i.time) as f32;
        c.x += (time * 28.0).sin() * 1.1;
        lid = 0.25 + 0.15 * (time * 9.0).sin().abs();
        ctx.request_repaint();
    }
    if let Some(d) = dropping {
        lid = if d < 0.25 { d / 0.25 } else if d < 0.7 { 1.0 } else { 1.0 - (d - 0.7) / 0.3 };
        // small squash when the page lands
        if (0.62..0.8).contains(&d) {
            c.y += ((d - 0.62) / 0.18 * std::f32::consts::PI).sin() * 1.4;
        }
        ctx.request_repaint();
    }
    paint_bin(painter, c, 1.0, icon_col, lid, dropping, if p.dark { p.card } else { Color32::WHITE });

    if armed_t > 0.05 {
        let label = if dropping.is_some() { "Deleting…" } else { "Click to delete" };
        let g = painter.layout_no_wrap(label.into(), FontId::new(13.5, semibold()), on_red.gamma_multiply(armed_t));
        let pos = egui::pos2(rect.left() + 36.0, rect.center().y - g.size().y / 2.0);
        // clip text while the pill grows
        painter.with_clip_rect(rect.shrink(2.0)).galley(pos, g, on_red);
    }
    resp.on_hover_text(if armed { "Click again to permanently delete" } else { "Delete" }).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Dashed rounded rectangle; `offset` animates the dashes ("marching ants").
pub fn dashed_rounded_rect(painter: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke, offset: f32) {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut pts = Vec::new();
    let corners = [
        (egui::pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (egui::pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (egui::pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (egui::pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    pts.push(egui::pos2(rect.left() + r, rect.top()));
    for (c, start) in corners {
        for k in 0..=8 {
            let a = (start + k as f32 * 90.0 / 8.0).to_radians();
            pts.push(c + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    pts.push(egui::pos2(rect.left() + r, rect.top()));
    painter.extend(egui::Shape::dashed_line_with_offset(&pts, stroke, &[9.0], &[6.0], offset));
}

/// Card for one encrypted file. Returns (card response, save clicked).
#[allow(clippy::too_many_arguments)]
pub fn file_card(ui: &mut Ui, p: &Palette, id: egui::Id, icon: &str, tone: Color32, name: &str, info: &str, folder: &str, tags: &[String], size: Vec2) -> (Response, Rect) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let hovered = ui.rect_contains_pointer(rect);
    let ht = ui.ctx().animate_bool_with_time(resp.id.with("hover"), hovered, 0.14);
    let painter = ui.painter();
    let lift = ht * 2.0;
    let r = rect.translate(Vec2::new(0.0, -lift));
    if ht > 0.0 {
        painter.add(egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: p.shadow.gamma_multiply(ht) }.as_shape(r, R_CARD));
    }
    painter.rect_filled(r, R_CARD, p.card);
    painter.rect_stroke(r, R_CARD, Stroke::new(1.0, lerp(p.border, p.accent, ht * 0.8)), StrokeKind::Inside);
    // icon tile
    let tile = Rect::from_min_size(r.min + Vec2::new(16.0, 16.0), Vec2::splat(46.0));
    painter.rect_filled(tile, CornerRadius::same(12), tone.gamma_multiply(if p.dark { 0.22 } else { 0.12 }));
    painter.text(tile.center(), Align2::CENTER_CENTER, icon, FontId::proportional(23.0), tone);
    // grip hint
    painter.text(r.right_top() + Vec2::new(-16.0, 24.0), Align2::CENTER_CENTER, ic::GRIP_VERTICAL, FontId::proportional(14.0), p.weak.gamma_multiply(0.4 + 0.6 * ht));
    let max_chars = ((r.width() - 32.0) / 7.4) as usize;
    painter.text(egui::pos2(r.left() + 16.0, tile.bottom() + 18.0), Align2::LEFT_CENTER, truncate(name, max_chars), FontId::new(14.0, semibold()), p.text);
    painter.text(egui::pos2(r.left() + 16.0, tile.bottom() + 38.0), Align2::LEFT_CENTER, info, FontId::proportional(12.0), p.weak);
    // folder badge (top-right of the tile row)
    if !folder.is_empty() {
        let fg = painter.layout_no_wrap(format!("{}  {}", ic::FOLDER, truncate(folder, 18)), FontId::proportional(11.5), p.weak);
        painter.galley(egui::pos2(tile.right() + 10.0, tile.center().y - fg.size().y / 2.0), fg, p.weak);
    }
    // tags
    let clip = painter.with_clip_rect(Rect::from_min_max(egui::pos2(r.left() + 12.0, r.top()), egui::pos2(r.right() - 12.0, r.bottom())));
    let mut x = r.left() + 16.0;
    for t in tags.iter().take(5) {
        x += paint_tag(&clip, egui::pos2(x, tile.bottom() + 52.0), t, p) + 4.0;
    }
    let actions = Rect::from_min_max(egui::pos2(r.left() + 8.0, r.bottom() - 42.0), egui::pos2(r.right() - 8.0, r.bottom() - 6.0));
    (resp.on_hover_cursor(if hovered { egui::CursorIcon::Grab } else { egui::CursorIcon::Default }), actions)
}

/// Sidebar folder row with indentation and an expand chevron.
/// Returns (row response, chevron clicked).
#[allow(clippy::too_many_arguments)]
pub fn tree_item(ui: &mut Ui, p: &Palette, id: egui::Id, name: &str, depth: usize, expanded: Option<bool>, selected: bool, count: usize, drop_hover: bool) -> (Response, bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let ht = ui.ctx().animate_bool_with_time(id.with("hover"), resp.hovered() || drop_hover, 0.12);
    let painter = ui.painter();
    let mut bg = lerp(Color32::TRANSPARENT, p.nav_hover, ht);
    if selected {
        bg = p.nav_active;
    }
    painter.rect_filled(rect, R_INPUT, bg);
    if drop_hover {
        painter.rect_stroke(rect, R_INPUT, Stroke::new(1.5, p.nav_text), StrokeKind::Inside);
    }
    let col = if selected { p.nav_active_text } else { p.nav_text };
    let x0 = rect.left() + 8.0 + depth as f32 * 14.0;
    let mut chevron_clicked = false;
    if let Some(open) = expanded {
        let cr = Rect::from_center_size(egui::pos2(x0 + 6.0, rect.center().y), Vec2::splat(18.0));
        let cresp = ui.interact(cr, id.with("chev"), Sense::click());
        chevron_clicked = cresp.clicked();
        let painter = ui.painter();
        painter.text(cr.center(), Align2::CENTER_CENTER, if open { ic::CHEVRON_DOWN } else { ic::CHEVRON_RIGHT }, FontId::proportional(12.0), if selected { col } else { p.nav_weak });
    }
    let painter = ui.painter();
    painter.text(egui::pos2(x0 + 20.0, rect.center().y), Align2::LEFT_CENTER, if expanded == Some(true) { ic::FOLDER_OPEN } else { ic::FOLDER }, FontId::proportional(15.0), col);
    let max_chars = ((rect.right() - x0 - 70.0) / 7.2).max(4.0) as usize;
    painter.text(egui::pos2(x0 + 42.0, rect.center().y), Align2::LEFT_CENTER, truncate(name, max_chars), FontId::new(13.5, if selected { semibold() } else { egui::FontFamily::Proportional }), col);
    if count > 0 {
        painter.text(rect.right_center() - Vec2::new(12.0, 0.0), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(11.5), if selected { col } else { p.nav_weak });
    }
    (resp.on_hover_cursor(egui::CursorIcon::PointingHand), chevron_clicked)
}

/// Sidebar tag row (#tag with a colored dot).
pub fn tag_nav_item(ui: &mut Ui, p: &Palette, tag: &str, selected: bool, count: usize) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    let ht = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let painter = ui.painter();
    let bg = if selected { p.nav_active } else { lerp(Color32::TRANSPARENT, p.nav_hover, ht) };
    painter.rect_filled(rect, R_INPUT, bg);
    let col = if selected { p.nav_active_text } else { p.nav_text };
    painter.circle_filled(egui::pos2(rect.left() + 18.0, rect.center().y), 4.5, avatar_color(tag));
    painter.text(egui::pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, format!("#{tag}"), FontId::proportional(13.0), col);
    painter.text(rect.right_center() - Vec2::new(12.0, 0.0), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(11.5), if selected { col } else { p.nav_weak });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Tag editor: existing tags as removable pills + an input that adds on Enter / comma.
/// Returns true if tags changed.
pub fn tag_editor(ui: &mut Ui, p: &Palette, id_salt: &str, tags: &mut Vec<String>, suggestions: &[String]) -> bool {
    let mut changed = false;
    let input_id = egui::Id::unique(id_salt).with("input");
    let mut input: String = ui.ctx().data_mut(|d| d.get_temp::<String>(input_id).unwrap_or_default());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        let mut remove = None;
        for (i, t) in tags.iter().enumerate() {
            let col = avatar_color(t);
            let txt = RichText::new(format!("#{t}  {}", ic::X)).size(12.5).color(if p.dark { lerp(col, Color32::WHITE, 0.45) } else { col });
            if ui
                .add(egui::Button::new(txt).fill(col.gamma_multiply(if p.dark { 0.28 } else { 0.12 })).stroke(Stroke::NONE).corner_radius(PILL).min_size(Vec2::new(0.0, 26.0)))
                .on_hover_text("Remove tag")
                .clicked()
            {
                remove = Some(i);
            }
        }
        if let Some(i) = remove {
            tags.remove(i);
            changed = true;
        }
        let r = ui.add(egui::TextEdit::singleline(&mut input).id(input_id.with("te")).hint_text("Add tag…").desired_width(130.0).margin(Margin::symmetric(10, 5)));
        let commit = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || input.contains(',');
        if commit {
            for t in input.split(',') {
                let t = t.trim().trim_start_matches('#').to_lowercase().replace(char::is_whitespace, "-");
                if !t.is_empty() && !tags.contains(&t) {
                    tags.push(t);
                    changed = true;
                }
            }
            input.clear();
            r.request_focus();
        }
        // quick-pick existing tags matching the input
        if r.has_focus() || !input.is_empty() {
            let q = input.trim().to_lowercase();
            let picks: Vec<String> = suggestions.iter().filter(|s| !tags.contains(s) && (q.is_empty() || s.contains(&q))).take(6).cloned().collect();
            for s in picks {
                if ui.add(egui::Button::new(RichText::new(format!("+ #{s}")).size(12.0).color(p.weak)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, p.border)).corner_radius(PILL)).clicked() {
                    tags.push(s);
                    changed = true;
                    input.clear();
                }
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(input_id, input));
    changed
}
