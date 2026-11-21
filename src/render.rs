//! Rich rendering for secure notes: Markdown, syntax-highlighted code, a JSON
//! tree and CSV tables. Everything renders locally: no network requests and no
//! remote images, so nothing in a note can phone home.

use crate::theme::{semibold, Palette, R_INPUT};
use egui::{
    text::LayoutJob, Color32, CornerRadius, FontFamily, FontId, Frame, Margin, RichText, ScrollArea,
    Stroke, TextWrapMode, Ui,
};
use egui_extras::syntax_highlighting::{highlight, CodeTheme};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NoteFormat {
    #[default]
    Plain,
    Markdown,
    Code,
    Json,
    Csv,
}

impl NoteFormat {
    pub const ALL: [NoteFormat; 5] = [Self::Plain, Self::Markdown, Self::Code, Self::Json, Self::Csv];

    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "Plain text",
            Self::Markdown => "Markdown",
            Self::Code => "Code",
            Self::Json => "JSON",
            Self::Csv => "CSV table",
        }
    }

    /// Icon used for the item tile in lists.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Plain => crate::icons::STICKY_NOTE,
            Self::Markdown => crate::icons::FILE_TEXT,
            Self::Code => crate::icons::CODE,
            Self::Json => crate::icons::BRACES,
            Self::Csv => crate::icons::TABLE,
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::Plain => "\u{E303}  Note",
            Self::Markdown => "\u{E0CC}  Markdown",
            Self::Code => "\u{E093}  Code",
            Self::Json => "\u{E36A}  JSON",
            Self::Csv => "\u{E17D}  CSV",
        }
    }

    /// Syntax used to highlight the *editor* (not the preview).
    pub fn editor_syntax(self, lang: &str) -> Option<String> {
        match self {
            Self::Markdown => Some("md".into()),
            Self::Code => Some(if lang.trim().is_empty() { "txt".into() } else { lang.trim().to_lowercase() }),
            Self::Json => Some("json".into()),
            _ => None,
        }
    }

    pub fn has_preview(self) -> bool {
        self != Self::Plain
    }
}

pub fn code_theme(p: &Palette, size: f32) -> CodeTheme {
    if p.dark { CodeTheme::dark(size) } else { CodeTheme::light(size) }
}

/// Render a note body in its format.
pub fn render(ui: &mut Ui, p: &Palette, format: NoteFormat, text: &str, lang: &str) {
    if text.trim().is_empty() {
        ui.label(RichText::new("Nothing to preview yet.").italics().color(p.weak));
        return;
    }
    ui.spacing_mut().interact_size.y = 18.0; // compact rows in previews
    match format {
        NoteFormat::Plain => {
            ui.add(egui::Label::new(text).selectable(true));
        }
        NoteFormat::Markdown => markdown(ui, p, text),
        NoteFormat::Code => code_block(ui, p, text, lang, true),
        NoteFormat::Json => json(ui, p, text),
        NoteFormat::Csv => csv_table(ui, p, text),
    }
}

// ───────────────────────────── code ─────────────────────────────

pub fn code_block(ui: &mut Ui, p: &Palette, code: &str, lang: &str, line_numbers: bool) {
    let lang = if lang.trim().is_empty() { "txt" } else { lang.trim() };
    let theme = code_theme(p, 13.0);
    let job = highlight(ui.ctx(), ui.style(), &theme, code.trim_end_matches('\n'), lang);
    Frame::new()
        .fill(p.bg_alt)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(R_INPUT)
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    if line_numbers {
                        let n = code.trim_end_matches('\n').lines().count().max(1);
                        let nums: String = (1..=n).map(|i| format!("{i}\n")).collect();
                        ui.add(
                            egui::Label::new(RichText::new(nums.trim_end()).font(FontId::monospace(13.0)).color(p.weak.gamma_multiply(0.7)))
                                .wrap_mode(TextWrapMode::Extend)
                                .selectable(false),
                        );
                        ui.add_space(6.0);
                    }
                    ui.add(egui::Label::new(job).wrap_mode(TextWrapMode::Extend).selectable(true));
                });
            });
        });
}

// ───────────────────────────── markdown ─────────────────────────────

#[derive(Clone, Default)]
struct Style {
    strong: bool,
    emph: bool,
    strike: bool,
    code: bool,
    link: Option<String>,
    heading: Option<HeadingLevel>,
}

#[derive(Clone)]
enum Run {
    Text(String, Style),
    Break,
}

enum ListKind {
    Bullet,
    Ordered(u64),
}

struct Md<'a> {
    p: &'a Palette,
    runs: Vec<Run>,
    style: Style,
    lists: Vec<ListKind>,
    quote: usize,
    marker: Option<String>,
    code: Option<(String, String)>, // (lang, text)
    table: Option<Vec<Vec<Vec<Run>>>>,
    cell: Vec<Run>,
    in_cell: bool,
    image: Option<(String, String)>,
    heading_idx: usize,
}

fn heading_size(l: HeadingLevel) -> f32 {
    match l {
        HeadingLevel::H1 => 24.0,
        HeadingLevel::H2 => 20.0,
        HeadingLevel::H3 => 17.5,
        HeadingLevel::H4 => 15.5,
        _ => 14.5,
    }
}

fn rich(p: &Palette, text: &str, s: &Style) -> RichText {
    let mut size = 14.0;
    let mut family = FontFamily::Proportional;
    if let Some(l) = s.heading {
        size = heading_size(l);
        family = semibold();
    }
    if s.strong {
        family = semibold();
    }
    let mut rt = RichText::new(text).size(size).family(family);
    if s.code {
        rt = RichText::new(text).font(FontId::monospace(13.0)).background_color(p.bg_alt).color(if p.dark { Color32::from_rgb(0xff, 0xa6, 0x9e) } else { Color32::from_rgb(0xb3, 0x26, 0x1e) });
    }
    if s.emph {
        rt = rt.italics();
    }
    if s.strike {
        rt = rt.strikethrough();
    }
    if s.link.is_some() {
        rt = rt.color(p.accent).underline();
    }
    rt
}

fn safe_url(u: &str) -> bool {
    let l = u.trim().to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://") || l.starts_with("mailto:")
}

fn draw_runs(ui: &mut Ui, p: &Palette, runs: &[Run]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
        for r in runs {
            match r {
                Run::Break => ui.end_row(),
                Run::Text(t, s) => {
                    let rt = rich(p, t, s);
                    match &s.link {
                        Some(url) if safe_url(url) => {
                            ui.hyperlink_to(rt, url).on_hover_text(url);
                        }
                        Some(url) => {
                            ui.label(rt).on_hover_text(format!("Link not opened (unsupported scheme): {url}"));
                        }
                        None => {
                            ui.add(egui::Label::new(rt).selectable(true));
                        }
                    }
                }
            }
        }
    });
}

impl<'a> Md<'a> {
    fn indent(&self) -> f32 {
        self.lists.len().saturating_sub(1) as f32 * 22.0
    }

    fn push_text(&mut self, t: &str) {
        let run = Run::Text(t.to_string(), self.style.clone());
        if self.in_cell {
            self.cell.push(run);
        } else {
            self.runs.push(run);
        }
    }

    fn flush(&mut self, ui: &mut Ui) {
        if self.runs.is_empty() && self.marker.is_none() {
            return;
        }
        let runs = std::mem::take(&mut self.runs);
        let marker = self.marker.take();
        let indent = self.indent();
        let p = self.p;
        let quote = self.quote;
        let body = |ui: &mut Ui| {
            ui.horizontal_top(|ui| {
                ui.add_space(indent);
                if let Some(m) = &marker {
                    let col = if m.starts_with('\u{E559}') { p.success } else if m.starts_with('\u{E167}') { p.weak } else { p.accent };
                    ui.add_sized([22.0, 16.0], egui::Label::new(RichText::new(m).color(col).family(semibold())));
                }
                ui.vertical(|ui| draw_runs(ui, p, &runs));
            });
        };
        if quote > 0 {
            let r = Frame::new()
                .fill(p.bg_alt)
                .inner_margin(Margin { left: 14, right: 10, top: 6, bottom: 6 })
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    body(ui)
                })
                .response
                .rect;
            ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(3.0, r.height())), CornerRadius::ZERO, p.accent);
        } else {
            body(ui);
        }
    }
}

pub fn markdown(ui: &mut Ui, p: &Palette, text: &str) {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut md = Md {
        p,
        runs: vec![],
        style: Style::default(),
        lists: vec![],
        quote: 0,
        marker: None,
        code: None,
        table: None,
        cell: vec![],
        in_cell: false,
        image: None,
        heading_idx: 0,
    };
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.spacing_mut().interact_size.y = 16.0; // compact rows for wrapped inline text

    for ev in Parser::new_ext(text, opts) {
        // inside fenced/indented code: collect raw text
        if let Some((_, buf)) = &mut md.code {
            match ev {
                Event::Text(t) => {
                    buf.push_str(&t);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, buf) = md.code.take().unwrap();
                    ui.horizontal_top(|ui| {
                        ui.add_space(md.indent());
                        ui.vertical(|ui| code_block(ui, p, &buf, &lang, false));
                    });
                    continue;
                }
                _ => continue,
            }
        }
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    md.flush(ui);
                    if level <= HeadingLevel::H2 {
                        ui.add_space(6.0);
                    }
                    md.style.heading = Some(level);
                }
                Tag::BlockQuote(_) => {
                    md.flush(ui);
                    md.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    md.flush(ui);
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    md.code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    md.flush(ui);
                    md.lists.push(match start {
                        Some(n) => ListKind::Ordered(n),
                        None => ListKind::Bullet,
                    });
                }
                Tag::Item => {
                    md.flush(ui);
                    let depth = md.lists.len();
                    md.marker = Some(match md.lists.last_mut() {
                        Some(ListKind::Ordered(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => if depth > 1 { "◦".into() } else { "•".into() },
                    });
                }
                Tag::Emphasis => md.style.emph = true,
                Tag::Strong => md.style.strong = true,
                Tag::Strikethrough => md.style.strike = true,
                Tag::Link { dest_url, .. } => md.style.link = Some(dest_url.to_string()),
                Tag::Image { dest_url, .. } => md.image = Some((dest_url.to_string(), String::new())),
                Tag::Table(_) => {
                    md.flush(ui);
                    md.table = Some(vec![]);
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = &mut md.table {
                        t.push(vec![]);
                    }
                }
                Tag::TableCell => {
                    md.in_cell = true;
                    md.cell.clear();
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => md.flush(ui),
                TagEnd::Heading(level) => {
                    let top = ui.cursor().top();
                    md.flush(ui);
                    // table-of-contents navigation: scroll to the requested heading
                    let target = ui.ctx().data_mut(|d| d.get_temp::<usize>(scroll_target_id()));
                    if target == Some(md.heading_idx) {
                        let r = egui::Rect::from_min_max(egui::pos2(ui.min_rect().left(), top - 8.0), egui::pos2(ui.min_rect().right(), top + 40.0));
                        ui.scroll_to_rect(r, Some(egui::Align::TOP));
                        ui.ctx().data_mut(|d| d.remove::<usize>(scroll_target_id()));
                    }
                    md.heading_idx += 1;
                    md.style.heading = None;
                    if level <= HeadingLevel::H2 {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, CornerRadius::ZERO, p.border);
                    }
                }
                TagEnd::BlockQuote(_) => {
                    md.flush(ui);
                    md.quote = md.quote.saturating_sub(1);
                }
                TagEnd::List(_) => {
                    md.flush(ui);
                    md.lists.pop();
                    if md.lists.is_empty() {
                        ui.add_space(2.0);
                    }
                }
                TagEnd::Item => md.flush(ui),
                TagEnd::Emphasis => md.style.emph = false,
                TagEnd::Strong => md.style.strong = false,
                TagEnd::Strikethrough => md.style.strike = false,
                TagEnd::Link => md.style.link = None,
                TagEnd::Image => {
                    if let Some((url, alt)) = md.image.take() {
                        // remote images are never loaded (privacy); show a placeholder
                        let label = if alt.is_empty() { "image".to_string() } else { alt };
                        md.push_text(&format!("\u{E0F6} {label}"));
                        let _ = url;
                    }
                }
                TagEnd::TableCell => {
                    md.in_cell = false;
                    let cell = std::mem::take(&mut md.cell);
                    if let Some(row) = md.table.as_mut().and_then(|t| t.last_mut()) {
                        row.push(cell);
                    }
                }
                TagEnd::Table => {
                    if let Some(rows) = md.table.take() {
                        let indent = md.indent();
                        ui.horizontal_top(|ui| {
                            ui.add_space(indent);
                            md_table(ui, p, &rows);
                        });
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, alt)) = &mut md.image {
                    alt.push_str(&t);
                } else {
                    md.push_text(&t);
                }
            }
            Event::Code(t) => {
                let prev = md.style.code;
                md.style.code = true;
                md.push_text(&format!(" {t} "));
                md.style.code = prev;
            }
            Event::SoftBreak => md.push_text(" "),
            Event::HardBreak => {
                if md.in_cell {
                    md.cell.push(Run::Break);
                } else {
                    md.runs.push(Run::Break);
                }
            }
            Event::Rule => {
                md.flush(ui);
                ui.add_space(4.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::ZERO, p.border);
                ui.add_space(4.0);
            }
            Event::TaskListMarker(checked) => {
                md.marker = Some(if checked { "\u{E559}".into() } else { "\u{E167}".into() });
                if checked {
                    md.style.strike = false;
                }
            }
            Event::Html(h) | Event::InlineHtml(h) => {
                // never interpret HTML; show it as literal code
                let prev = md.style.code;
                md.style.code = true;
                md.push_text(h.trim_end());
                md.style.code = prev;
            }
            _ => {}
        }
    }
    md.flush(ui);
}

fn md_table(ui: &mut Ui, p: &Palette, rows: &[Vec<Vec<Run>>]) {
    Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(R_INPUT).inner_margin(Margin::same(8)).show(ui, |ui| {
        ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id()).striped(true).spacing([18.0, 8.0]).show(ui, |ui| {
                for (ri, row) in rows.iter().enumerate() {
                    for cell in row {
                        if ri == 0 {
                            let bold: Vec<Run> = cell
                                .iter()
                                .map(|r| match r {
                                    Run::Text(t, s) => Run::Text(t.clone(), Style { strong: true, ..s.clone() }),
                                    Run::Break => Run::Break,
                                })
                                .collect();
                            draw_runs(ui, p, &bold);
                        } else {
                            draw_runs(ui, p, cell);
                        }
                    }
                    ui.end_row();
                }
            });
        });
    });
}

fn scroll_target_id() -> egui::Id {
    egui::Id::unique("softvault-md-scroll-target")
}

/// Ask the next Markdown render to scroll to heading number `idx`.
pub fn request_scroll_to_heading(ctx: &egui::Context, idx: usize) {
    ctx.data_mut(|d| d.insert_temp(scroll_target_id(), idx));
}

/// Headings of a Markdown document: (level 1-6, text), in order.
pub fn headings(text: &str) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    let mut cur: Option<(u8, String)> = None;
    for ev in Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => cur = Some((level as u8, String::new())),
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, s)) = &mut cur {
                    s.push_str(&t);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = cur.take() {
                    out.push(h);
                }
            }
            _ => {}
        }
    }
    out
}

// ───────────────────────────── JSON ─────────────────────────────

pub fn json(ui: &mut Ui, p: &Palette, text: &str) {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => {
            let summary = match &v {
                serde_json::Value::Object(m) => format!("Valid JSON · object with {} keys", m.len()),
                serde_json::Value::Array(a) => format!("Valid JSON · array with {} items", a.len()),
                _ => "Valid JSON".into(),
            };
            ui.label(RichText::new(format!("\u{E226} {summary}")).size(12.5).color(p.success));
            Frame::new()
                .fill(p.bg_alt)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(R_INPUT)
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 3.0;
                    json_node(ui, p, None, &v, "$", 0);
                });
        }
        Err(e) => {
            ui.label(RichText::new(format!("\u{E193} Invalid JSON: {e}")).color(p.danger));
            code_block(ui, p, text, "json", true);
        }
    }
}

fn json_scalar(p: &Palette, v: &serde_json::Value) -> RichText {
    let mono = |s: String, c: Color32| RichText::new(s).font(FontId::monospace(13.0)).color(c);
    match v {
        serde_json::Value::String(s) => mono(format!("\"{s}\""), p.success),
        serde_json::Value::Number(n) => mono(n.to_string(), p.accent),
        serde_json::Value::Bool(b) => mono(b.to_string(), p.warn),
        serde_json::Value::Null => mono("null".into(), p.weak),
        _ => mono(String::new(), p.text),
    }
}

fn json_node(ui: &mut Ui, p: &Palette, key: Option<&str>, v: &serde_json::Value, path: &str, depth: usize) {
    let key_txt = |k: &str| RichText::new(format!("{k}: ")).font(FontId::monospace(13.0)).color(p.text);
    match v {
        serde_json::Value::Object(map) => {
            let head = format!("{}{{…}} {} keys", key.map(|k| format!("{k}: ")).unwrap_or_default(), map.len());
            egui::CollapsingHeader::new(RichText::new(head).font(FontId::monospace(13.0)))
                .id_salt(path)
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (k, child) in map {
                        json_node(ui, p, Some(k), child, &format!("{path}.{k}"), depth + 1);
                    }
                });
        }
        serde_json::Value::Array(arr) => {
            let head = format!("{}[…] {} items", key.map(|k| format!("{k}: ")).unwrap_or_default(), arr.len());
            egui::CollapsingHeader::new(RichText::new(head).font(FontId::monospace(13.0)))
                .id_salt(path)
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (i, child) in arr.iter().enumerate() {
                        json_node(ui, p, Some(&i.to_string()), child, &format!("{path}[{i}]"), depth + 1);
                    }
                });
        }
        scalar => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if let Some(k) = key {
                    ui.label(key_txt(k));
                }
                ui.add(egui::Label::new(json_scalar(p, scalar)).selectable(true));
            });
        }
    }
}

pub fn pretty_json(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    serde_json::to_string_pretty(&v).ok()
}

// ───────────────────────────── CSV ─────────────────────────────

fn sniff_delimiter(text: &str) -> u8 {
    let first = text.lines().next().unwrap_or("");
    let counts = [(b',', first.matches(',').count()), (b';', first.matches(';').count()), (b'\t', first.matches('\t').count()), (b'|', first.matches('|').count())];
    counts.iter().max_by_key(|c| c.1).filter(|c| c.1 > 0).map(|c| c.0).unwrap_or(b',')
}

pub fn csv_table(ui: &mut Ui, p: &Palette, text: &str) {
    let delim = sniff_delimiter(text);
    let mut rdr = csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(delim).from_reader(text.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    for rec in rdr.records() {
        match rec {
            Ok(r) => rows.push(r.iter().map(|s| s.to_string()).collect()),
            Err(e) => {
                ui.label(RichText::new(format!("\u{E193} CSV error: {e}")).color(p.danger));
                break;
            }
        }
        if rows.len() > 5000 {
            break;
        }
    }
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let dname = match delim {
        b';' => "semicolon",
        b'\t' => "tab",
        b'|' => "pipe",
        _ => "comma",
    };
    ui.label(RichText::new(format!("{} rows × {} columns · {dname}-separated · first row used as header", rows.len().saturating_sub(1), cols)).size(12.5).color(p.weak));
    Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(R_INPUT).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ScrollArea::horizontal().id_salt(ui.next_auto_id()).show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id()).striped(true).spacing([20.0, 8.0]).min_col_width(40.0).show(ui, |ui| {
                ui.label(RichText::new("#").color(p.weak).family(semibold()));
                for c in 0..cols {
                    let h = rows[0].get(c).cloned().unwrap_or_default();
                    ui.label(RichText::new(h).family(semibold()));
                }
                ui.end_row();
                for (i, row) in rows.iter().enumerate().skip(1) {
                    ui.label(RichText::new(i.to_string()).color(p.weak).size(12.0));
                    for c in 0..cols {
                        let cell = row.get(c).map(|s| s.as_str()).unwrap_or("");
                        let rt = if cell.parse::<f64>().is_ok() { RichText::new(cell).font(FontId::monospace(13.0)) } else { RichText::new(cell) };
                        ui.add(egui::Label::new(rt).selectable(true).wrap_mode(TextWrapMode::Extend));
                    }
                    ui.end_row();
                }
            });
        });
    });
}

/// Build a highlighted layout for the editor TextEdit.
pub fn editor_layout(ui: &Ui, p: &Palette, text: &str, syntax: &str, wrap_width: f32) -> LayoutJob {
    let theme = code_theme(p, 13.5);
    let mut job = highlight(ui.ctx(), ui.style(), &theme, text, syntax);
    job.wrap.max_width = wrap_width;
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delimiter_sniffing() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3"), b';');
        assert_eq!(sniff_delimiter("a,b\n1,2"), b',');
        assert_eq!(sniff_delimiter("a\tb"), b'\t');
    }
    #[test]
    fn pretty() {
        assert!(pretty_json("{\"a\":1}").unwrap().contains("\n"));
        let out = pretty_json("{\"z\":1,\"a\":2}").unwrap();
        assert!(out.find("\"z\"").unwrap() < out.find("\"a\"").unwrap(), "key order must be preserved");
        assert!(pretty_json("{oops").is_none());
    }
    #[test]
    fn toc() {
        let h = headings("# A\ntext\n## B `x`\n### C");
        assert_eq!(h, vec![(1, "A".into()), (2, "B x".into()), (3, "C".into())]);
    }
    #[test]
    fn url_safety() {
        assert!(safe_url("https://x.y"));
        assert!(!safe_url("file:///C:/Windows/system32/calc.exe"));
        assert!(!safe_url("javascript:alert(1)"));
    }
}
