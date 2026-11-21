//! Themes modelled on Bitwarden's web-vault design language: a navy sidebar,
//! a blue primary color, white or light-gray content, Inter typography,
//! fully rounded (pill) buttons and flat bordered sections.

use egui::{
    epaint::Shadow, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke,
    TextStyle, Theme, Vec2,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// main content background
    pub bg: Color32,
    /// login / unlock page background ("background-alt")
    pub bg_alt: Color32,
    /// sidebar
    pub panel: Color32,
    pub nav_text: Color32,
    pub nav_weak: Color32,
    pub nav_hover: Color32,
    pub nav_active: Color32,
    pub nav_active_text: Color32,
    /// surfaces: cards, sections, dialogs, table
    pub card: Color32,
    pub card_hover: Color32,
    pub input: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub weak: Color32,
    /// primary
    pub accent: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warn: Color32,
    pub shadow: Color32,
}

fn hex(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            dark,
            bg: hex(0x1f242e),
            bg_alt: hex(0x161c26),
            panel: hex(0x161c26),
            nav_text: hex(0xffffff),
            nav_weak: hex(0xbac0ce),
            nav_hover: Color32::from_white_alpha(14),
            nav_active: hex(0x6a99f0),
            nav_active_text: hex(0x101827),
            card: hex(0x262b35),
            card_hover: hex(0x2f343d),
            input: hex(0x1f242e),
            border: hex(0x3a404c),
            border_strong: hex(0x6b7280),
            text: hex(0xffffff),
            weak: hex(0xbac0ce),
            accent: hex(0x6a99f0),
            on_accent: hex(0x101827),
            accent_soft: Color32::from_rgba_unmultiplied(0x6a, 0x99, 0xf0, 34),
            danger: hex(0xff8d85),
            success: hex(0x52e07c),
            warn: hex(0xffd560),
            shadow: Color32::from_black_alpha(90),
        }
    } else {
        Palette {
            dark,
            bg: hex(0xffffff),
            bg_alt: hex(0xf3f6f9),
            panel: hex(0x0e2a5e),
            nav_text: hex(0xffffff),
            nav_weak: hex(0xaebfe0),
            nav_hover: Color32::from_white_alpha(18),
            nav_active: hex(0x175ddc),
            nav_active_text: hex(0xffffff),
            card: hex(0xffffff),
            card_hover: hex(0xf3f6f9),
            input: hex(0xffffff),
            border: hex(0xdfe3ea),
            border_strong: hex(0x89929f),
            text: hex(0x1b2029),
            weak: hex(0x5a6271),
            accent: hex(0x175ddc),
            on_accent: hex(0xffffff),
            accent_soft: hex(0xebf1fd),
            danger: hex(0xc83522),
            success: hex(0x017e45),
            warn: hex(0x8b6609),
            shadow: Color32::from_rgba_unmultiplied(0x1b, 0x20, 0x29, 28),
        }
    }
}

/// Subtle elevation used only for dialogs, menus and toasts.
pub fn soft_shadow(p: &Palette) -> Shadow {
    Shadow { offset: [0, 4], blur: 16, spread: 0, color: p.shadow }
}

pub const PILL: CornerRadius = CornerRadius::same(18);
pub const R_INPUT: CornerRadius = CornerRadius::same(8);
pub const R_CARD: CornerRadius = CornerRadius::same(12);

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.ttf"))),
    );
    fonts.font_data.insert(
        "lucide".into(),
        Arc::new(
            FontData::from_static(include_bytes!("../assets/fonts/lucide-subset.ttf"))
                .tweak(egui::FontTweak { y_offset_factor: 0.08, ..Default::default() }),
        ),
    );
    // Font chains: Lucide icons, then Inter, then egui's built-ins as fallback
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    {
        let prop = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
        // Lucide first: it only contains private-use icon codepoints, and Inter has
        // its own private-use glyphs that would otherwise shadow the icons.
        prop.insert(0, "lucide".into());
        prop.insert(1, "inter".into());
    }
    fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "lucide".into());
    let mut semi = vec!["lucide".to_string(), "inter-semibold".to_string()];
    semi.extend(fallback);
    fonts.families.insert(semibold(), semi);
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
    let theme = if dark { Theme::Dark } else { Theme::Light };
    let mut style = (*ctx.global_style()).clone();
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };

    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = R_CARD;
    v.window_shadow = soft_shadow(&p);
    v.popup_shadow = soft_shadow(&p);
    v.menu_corner_radius = R_INPUT;
    v.extreme_bg_color = p.input;
    v.text_edit_bg_color = Some(p.input);
    v.faint_bg_color = p.card_hover;
    v.code_bg_color = p.bg_alt;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.override_text_color = Some(p.text);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = R_INPUT;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.weak_bg_fill = p.card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);

    // inputs: white field, gray border; buttons: soft gray
    v.widgets.inactive.bg_fill = p.input;
    v.widgets.inactive.weak_bg_fill = p.card_hover;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border_strong.gamma_multiply(0.7));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);

    v.widgets.hovered.bg_fill = p.input;
    v.widgets.hovered.weak_bg_fill = p.card_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);

    v.widgets.active.bg_fill = p.input;
    v.widgets.active.weak_bg_fill = p.accent_soft;
    v.widgets.active.bg_stroke = Stroke::new(2.0, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text); // also drives "strong" text color

    v.widgets.open.weak_bg_fill = p.accent_soft;
    v.widgets.open.bg_stroke = Stroke::new(1.0, p.accent);

    style.visuals = v;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 7.0);
    style.spacing.interact_size = Vec2::new(40.0, 30.0);
    style.spacing.slider_width = 220.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.scroll = egui::style::ScrollStyle::floating();
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.combo_height = 260.0;
    style.interaction.tooltip_delay = 0.35;

    style.text_styles = [
        (TextStyle::Heading, FontId::new(22.0, semibold())),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(14.0, FontFamily::Monospace)),
    ]
    .into();

    ctx.set_theme(theme);
    ctx.set_style_of(theme, style);
}
