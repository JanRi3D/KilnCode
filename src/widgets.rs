//! Reusable, custom-painted widgets that follow the Kiln design tokens.

use crate::icons;
use crate::theme::{self, Palette, fs, mono_fs, p, radius, tint};
use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Margin, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextWrapMode, Ui, Vec2,
    pos2, vec2,
};

pub fn galley_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
}

/// Paints an icon glyph centered in `rect`.
pub fn paint_icon(ui: &Ui, rect: Rect, glyph: &str, size: f32, color: Color32) {
    ui.painter().text(rect.center() + vec2(0.0, size * 0.04), Align2::CENTER_CENTER, glyph, theme::ui(size), color);
}

/// A non-interactive icon taking `size`×`size`.
pub fn icon(ui: &mut Ui, glyph: &str, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui, rect, glyph, size, color);
    resp
}

/// Rotating dashed circle used for "working" states.
pub fn spinner(ui: &mut Ui, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_spinner(ui, rect, color);
    resp
}

pub fn paint_spinner(ui: &Ui, rect: Rect, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let c = rect.center();
    let r = rect.width() * 0.42;
    let stroke = Stroke::new((rect.width() / 12.0).max(1.2), color);
    let base = t * std::f32::consts::TAU / 3.0;
    for k in 0..8 {
        let a0 = base + k as f32 * std::f32::consts::TAU / 8.0;
        let a1 = a0 + std::f32::consts::TAU / 8.0 * 0.55;
        let pts: Vec<Pos2> = (0..=4)
            .map(|s| {
                let a = a0 + (a1 - a0) * s as f32 / 4.0;
                c + vec2(a.cos(), a.sin()) * r
            })
            .collect();
        ui.painter().add(Shape::line(pts, stroke));
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
    Danger,
    Subtle,
}

pub struct Btn<'a> {
    label: &'a str,
    icon: Option<&'a str>,
    trailing: Option<&'a str>,
    kind: Kind,
    height: f32,
    font: f32,
    enabled: bool,
    min_width: f32,
    selected: bool,
    tooltip: Option<&'a str>,
}

impl<'a> Btn<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            icon: None,
            trailing: None,
            kind: Kind::Secondary,
            height: 24.0,
            font: 12.0,
            enabled: true,
            min_width: 0.0,
            selected: false,
            tooltip: None,
        }
    }
    pub fn primary(label: &'a str) -> Self {
        Self::new(label).kind(Kind::Primary)
    }
    pub fn ghost(label: &'a str) -> Self {
        Self::new(label).kind(Kind::Ghost)
    }
    pub fn kind(mut self, k: Kind) -> Self {
        self.kind = k;
        self
    }
    pub fn icon(mut self, g: &'a str) -> Self {
        self.icon = Some(g);
        self
    }
    pub fn trailing(mut self, g: &'a str) -> Self {
        self.trailing = Some(g);
        self
    }
    pub fn medium(mut self) -> Self {
        self.height = 28.0;
        self.font = 14.0;
        self
    }
    pub fn large(mut self) -> Self {
        self.height = 32.0;
        self.font = 13.0;
        self
    }
    pub fn font(mut self, f: f32) -> Self {
        self.font = f;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    pub fn tooltip(mut self, t: &'a str) -> Self {
        self.tooltip = Some(t);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let pal = p();
        let font = theme::ui_medium(fs(self.font));
        let icon_sz = if self.height <= 24.0 { 14.0 } else { 16.0 };
        let pad = if self.height <= 24.0 { 7.0 } else { 9.0 };
        let text_w = if self.label.is_empty() { 0.0 } else { galley_width(ui, self.label, &font) };
        let mut w = pad * 2.0 + text_w;
        if self.icon.is_some() {
            w += icon_sz + if self.label.is_empty() { 0.0 } else { 4.0 };
        }
        if self.trailing.is_some() {
            w += icon_sz + 2.0;
        }
        let w = w.max(self.min_width);
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, self.height), sense);
        if ui.is_rect_visible(rect) {
            let hovered = resp.hovered() && self.enabled;
            let pressed = resp.is_pointer_button_down_on();
            let (mut fill, mut stroke, mut fg) = match self.kind {
                Kind::Primary => (pal.accent, pal.accent, Color32::WHITE),
                Kind::Secondary => (pal.ctrl_bg, pal.input_border, pal.fg),
                Kind::Ghost => (Color32::TRANSPARENT, Color32::TRANSPARENT, pal.muted),
                Kind::Subtle => (Color32::TRANSPARENT, Color32::TRANSPARENT, pal.fg),
                Kind::Danger => (pal.ctrl_bg, pal.input_border, pal.danger),
            };
            if self.selected {
                fill = pal.hover;
                fg = pal.fg;
                stroke = if self.kind == Kind::Secondary { theme::rgba(255, 255, 255, 0.16) } else { stroke };
            }
            if hovered {
                fill = match self.kind {
                    Kind::Primary => pal.accent.gamma_multiply(1.12),
                    _ => pal.hover,
                };
                if self.kind == Kind::Ghost {
                    fg = pal.fg;
                }
            }
            if pressed && self.enabled {
                fill = if self.kind == Kind::Primary { pal.accent.gamma_multiply(0.9) } else { pal.selected };
            }
            let r = radius(theme::R_LG);
            ui.painter().rect(rect, r, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
            if self.kind == Kind::Primary {
                ui.painter().hline(rect.x_range().shrink(4.0), rect.top() + 1.0, Stroke::new(1.0, theme::rgba(255, 255, 255, 0.16)));
            }
            if !self.enabled {
                fg = fg.gamma_multiply(0.5);
            }
            if resp.has_focus() {
                focus_ring(ui, rect, r);
            }
            let mut x = rect.left() + (rect.width() - (w - (w - self.min_width).min(0.0))).max(0.0) / 2.0 + pad;
            if self.min_width > 0.0 {
                let content = w - 2.0 * pad;
                let natural =
                    text_w + self.icon.map(|_| icon_sz + 4.0).unwrap_or(0.0) + self.trailing.map(|_| icon_sz + 2.0).unwrap_or(0.0);
                x = rect.left() + pad + (content - natural).max(0.0) / 2.0;
            }
            if let Some(g) = self.icon {
                let ir = Rect::from_min_size(pos2(x, rect.center().y - icon_sz / 2.0), Vec2::splat(icon_sz));
                let ic = if matches!(self.kind, Kind::Secondary | Kind::Subtle) && self.enabled { pal.muted } else { fg };
                paint_icon(ui, ir, g, icon_sz, ic);
                x += icon_sz + if self.label.is_empty() { 0.0 } else { 4.0 };
            }
            if !self.label.is_empty() {
                ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, self.label, font, fg);
                x += text_w + 2.0;
            }
            if let Some(g) = self.trailing {
                let ir = Rect::from_min_size(pos2(x, rect.center().y - icon_sz / 2.0), Vec2::splat(icon_sz));
                paint_icon(ui, ir, g, icon_sz, fg.gamma_multiply(0.7));
            }
        }
        let resp = if let Some(t) = self.tooltip { resp.on_hover_text(t) } else { resp };
        if self.enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
    }
}

pub fn focus_ring(ui: &Ui, rect: Rect, r: CornerRadius) {
    ui.painter().rect_stroke(rect.expand(1.5), r, Stroke::new(1.5, tint(p().accent, 0.8)), StrokeKind::Outside);
}

/// Square icon-only button (28px by default) with hover/pressed/active states.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str, active: bool) -> Response {
    icon_button_sized(ui, glyph, tooltip, active, 28.0, 16.0, p().icon)
}

pub fn icon_button_sized(ui: &mut Ui, glyph: &str, tooltip: &str, active: bool, size: f32, icon_size: f32, color: Color32) -> Response {
    let pal = p();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if resp.is_pointer_button_down_on() || active {
            pal.selected
        } else if resp.hovered() {
            pal.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, radius(theme::R_LG), bg);
        let c = if active || resp.hovered() { pal.fg } else { color };
        paint_icon(ui, rect, glyph, icon_size, c);
        if resp.has_focus() {
            focus_ring(ui, rect, radius(theme::R_LG));
        }
    }
    let resp = if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rounded pill with icon + label (status chips in the top bar, compare cards).
pub fn pill(ui: &mut Ui, glyph: Option<&str>, label: &str, color: Color32, height: f32, spin: bool) -> Response {
    let font = theme::ui_medium(fs(12.0));
    let tw = galley_width(ui, label, &font);
    let isz = 12.0;
    let w = 16.0 + tw + if glyph.is_some() { isz + 5.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(255), tint(color, 0.12));
    let mut x = rect.left() + 8.0;
    if let Some(g) = glyph {
        let ir = Rect::from_min_size(pos2(x, rect.center().y - isz / 2.0), Vec2::splat(isz));
        if spin {
            paint_spinner(ui, ir, color)
        } else {
            paint_icon(ui, ir, g, isz, color)
        }
        x += isz + 5.0;
    }
    ui.painter().text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, color);
    resp
}

/// Small square-ish badge (`New`, `Low risk`, `Allow`…).
pub fn badge(ui: &mut Ui, label: &str, fg: Color32, bg: Color32) -> Response {
    let font = theme::ui_medium(fs(12.0));
    let tw = galley_width(ui, label, &font);
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + 8.0, 18.0), Sense::hover());
    ui.painter().rect_filled(rect, radius(theme::R_SM), bg);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    resp
}

pub fn tone_badge(ui: &mut Ui, label: &str, tone: Color32) -> Response {
    badge(ui, label, tone, tint(tone, if p().dark { 0.16 } else { 0.08 }))
}

/// The 16px rounded project favicon square with its initial.
pub fn fav(ui: &mut Ui, letter: &str, fg: Color32, bg: Color32, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_fav(ui, rect, letter, fg, bg);
    resp
}

pub fn paint_fav(ui: &Ui, rect: Rect, letter: &str, fg: Color32, bg: Color32) {
    let r = if rect.width() >= 24.0 { theme::R_LG } else { theme::R_SM };
    ui.painter().rect_filled(rect, radius(r), bg);
    let size = (rect.width() * 0.62).round().max(9.0);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, letter, theme::ui_semibold(size), fg);
}

/// Deterministic project colors (blue, amber, green, violet, rose…) keyed by name.
pub fn project_colors(name: &str) -> (String, Color32, Color32) {
    let tones = [
        theme::hex(0x51a2ff),
        theme::hex(0xffb900),
        theme::hex(0x00d492),
        theme::hex(0xa684ff),
        theme::hex(0xff6467),
        theme::hex(0x00d3f2),
    ];
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let c = tones[(h as usize) % tones.len()];
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_string()).unwrap_or_else(|| "·".into());
    let a = if p().dark { 0.15 } else { 0.12 };
    (letter, c, tint(c, a))
}

/// `+24 −4` in monospace with add/del colors. Zero deletions are hidden when `hide_zero_del`.
pub fn diff_counts(ui: &mut Ui, add: u32, del: u32, size: f32) -> Response {
    let pal = p();
    let mut job = egui::text::LayoutJob::default();
    let f = theme::mono(mono_fs(size));
    job.append(&format!("+{add}"), 0.0, egui::TextFormat::simple(f.clone(), pal.add));
    job.append(&format!("−{del}"), 6.0, egui::TextFormat::simple(f, pal.del));
    ui.add(egui::Label::new(job).selectable(false).wrap_mode(TextWrapMode::Extend))
}

/// iOS-style switch drawn at 30×18 inside a 40×28 hit area.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 28.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    paint_switch(ui, rect, *on, resp.has_focus());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn paint_switch(ui: &Ui, rect: Rect, on: bool, focused: bool) {
    let pal = p();
    let t = ui.ctx().animate_bool_with_time(ui.scope_id().with(rect.min.x as i32).with(rect.min.y as i32), on, 0.15);
    let track = Rect::from_center_size(rect.center(), vec2(30.0, 18.0));
    let fill = lerp_color(pal.track, pal.accent, t);
    ui.painter().rect_filled(track, CornerRadius::same(255), fill);
    let x = track.left() + 2.0 + 7.0 + t * 12.0;
    let knob = lerp_color(pal.thumb_off, Color32::WHITE, t);
    ui.painter().circle_filled(pos2(x, track.center().y), 7.0, knob);
    if focused {
        focus_ring(ui, track, CornerRadius::same(255));
    }
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Keyboard key cap.
pub fn kbd(ui: &mut Ui, key: &str) -> Response {
    let pal = p();
    let font = theme::ui_medium(fs(12.0));
    let tw = galley_width(ui, key, &font);
    let (rect, resp) = ui.allocate_exact_size(vec2((tw + 8.0).max(20.0), 20.0), Sense::hover());
    ui.painter().rect_filled(rect, radius(theme::R_SM), pal.kbd_bg);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, key, font, pal.muted);
    resp
}

/// Thin progress bar (plan progress, viewed files).
pub fn progress(ui: &mut Ui, width: f32, frac: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    paint_progress(ui, rect, frac, color);
    resp
}
pub fn paint_progress(ui: &Ui, rect: Rect, frac: f32, color: Color32) {
    ui.painter().rect_filled(rect, CornerRadius::same(255), p().track);
    let mut r = rect;
    r.set_width(rect.width() * frac.clamp(0.0, 1.0));
    if r.width() > 0.5 {
        ui.painter().rect_filled(r, CornerRadius::same(255), color);
    }
}

/// Card surface: near-black, soft border, 12px radius.
pub fn card() -> Frame {
    let pal = p();
    Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border_soft)).corner_radius(radius(theme::R_XL))
}

/// Floating surface (menus, popovers, thread details).
pub fn popover(radius_px: u8) -> Frame {
    let pal = p();
    Frame::new()
        .fill(pal.popover)
        .stroke(Stroke::new(1.0, if pal.dark { theme::rgba(245, 245, 245, 0.1) } else { pal.border }))
        .corner_radius(radius(radius_px))
        .shadow(egui::Shadow { offset: [0, 18], blur: 44, spread: 0, color: theme::rgba(0, 0, 0, if pal.dark { 0.8 } else { 0.15 }) })
}

/// Styled single-line text input. Returns the TextEdit response.
pub fn text_input(ui: &mut Ui, value: &mut String, hint: &str, width: f32, monospace: bool) -> Response {
    let pal = p();
    let font = if monospace { theme::mono(mono_fs(13.0)) } else { theme::ui(fs(14.0)) };
    let mut out = None;
    Frame::new()
        .fill(pal.ctrl_bg)
        .stroke(Stroke::new(1.0, pal.input_border))
        .corner_radius(radius(theme::R_LG))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(width - 22.0);
            out = Some(
                ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text(egui::RichText::new(hint).color(pal.faint))
                        .font(font)
                        .frame(egui::Frame::NONE)
                        .desired_width(width - 22.0),
                ),
            );
        });
    out.expect("text edit")
}

/// A styled dropdown with string options. Returns true when the selection changed.
pub fn select(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, current: &mut String, options: &[&str], width: f32) -> bool {
    let mut changed = false;
    let pal = p();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    let open_id = ui.make_persistent_id(&id);
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() { pal.hover } else { pal.ctrl_bg };
        ui.painter().rect(rect, radius(theme::R_LG), fill, Stroke::new(1.0, pal.input_border), StrokeKind::Inside);
        let text_rect = rect.shrink2(vec2(9.0, 0.0));
        let font = theme::ui(fs(14.0));
        let painter = ui.painter().with_clip_rect(Rect::from_min_max(text_rect.min, pos2(rect.right() - 26.0, text_rect.max.y)));
        painter.text(pos2(text_rect.left(), rect.center().y), Align2::LEFT_CENTER, current.as_str(), font, pal.fg);
        paint_icon(
            ui,
            Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(14.0)),
            icons::CHEVRON_DOWN,
            14.0,
            pal.muted.gamma_multiply(0.85),
        );
        if resp.has_focus() {
            focus_ring(ui, rect, radius(theme::R_LG));
        }
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    egui::Popup::menu(&resp).id(open_id).width(width.max(160.0)).frame(popover(10).inner_margin(4)).show(|ui| {
        ui.set_min_width(width.max(160.0) - 8.0);
        for opt in options {
            let on = current == opt;
            if menu_item(ui, if on { Some(icons::CHECK) } else { None }, opt, None, on).clicked() {
                *current = (*opt).to_owned();
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

/// One row inside a popover menu: optional leading icon, label, optional trailing hint.
pub fn menu_item(ui: &mut Ui, glyph: Option<&str>, label: &str, hint: Option<&str>, checked_space: bool) -> Response {
    let pal = p();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() || resp.has_focus() {
            ui.painter().rect_filled(rect, radius(theme::R_SM), pal.hover);
        }
        let mut x = rect.left() + 8.0;
        if glyph.is_some() || checked_space {
            if let Some(g) = glyph {
                paint_icon(ui, Rect::from_min_size(pos2(x, rect.center().y - 7.0), Vec2::splat(14.0)), g, 14.0, pal.fg);
            }
            x += 22.0;
        }
        let hint_w = hint.map(|h| galley_width(ui, h, &theme::ui(fs(12.0))) + 16.0).unwrap_or(0.0);
        paint_truncated(
            ui,
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::ui(fs(14.0)),
            pal.fg,
            rect.right() - 8.0 - hint_w - x,
        );
        if let Some(h) = hint {
            ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, h, theme::ui(fs(12.0)), pal.muted);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn menu_heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(text).font(theme::ui_medium(fs(12.0))).color(p().muted));
    });
}

pub fn menu_separator(ui: &mut Ui) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 9.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p().border));
}

/// Horizontal hairline across the available width.
pub fn hairline(ui: &mut Ui, color: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, color));
}

// Text helpers -------------------------------------------------------------------

pub fn text(ui: &mut Ui, s: impl Into<String>, size: f32, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s).font(theme::ui(fs(size))).color(color)).wrap_mode(TextWrapMode::Truncate))
}
pub fn text_medium(ui: &mut Ui, s: impl Into<String>, size: f32, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s).font(theme::ui_medium(fs(size))).color(color)).wrap_mode(TextWrapMode::Truncate))
}
pub fn text_semibold(ui: &mut Ui, s: impl Into<String>, size: f32, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s).font(theme::ui_semibold(fs(size))).color(color)).wrap_mode(TextWrapMode::Truncate))
}
pub fn text_wrap(ui: &mut Ui, s: impl Into<String>, size: f32, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s).font(theme::ui(fs(size))).color(color)).wrap_mode(TextWrapMode::Wrap))
}
pub fn mono_text(ui: &mut Ui, s: impl Into<String>, size: f32, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(s).font(theme::mono(mono_fs(size))).color(color)).wrap_mode(TextWrapMode::Truncate))
}

/// `dir/` muted + `name` regular, as used for file paths in rows.
pub fn path_job(path: &str, size: f32, pal: &Palette) -> egui::text::LayoutJob {
    let cut = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let mut job = egui::text::LayoutJob::default();
    let f = theme::mono(mono_fs(size));
    job.append(&path[..cut], 0.0, egui::TextFormat::simple(f.clone(), pal.muted));
    job.append(&path[cut..], 0.0, egui::TextFormat::simple(f, pal.fg));
    job.wrap = egui::text::TextWrapping::truncate_at_width(f32::INFINITY);
    job
}

/// A clickable full-width row with hover background and custom content.
pub fn row_button(ui: &mut Ui, height: f32, selected: bool, r: u8) -> (Rect, Response) {
    let pal = p();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, height), Sense::click());
    let bg = if selected {
        pal.selected
    } else if resp.hovered() {
        pal.hover
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, radius(r), bg);
    }
    if resp.has_focus() {
        focus_ring(ui, rect, radius(r));
    }
    (rect, resp.on_hover_cursor(egui::CursorIcon::PointingHand))
}

/// Paints `text` truncated with an ellipsis to fit `max_w`.
pub fn paint_truncated(ui: &Ui, pos: Pos2, anchor: Align2, text: &str, font: FontId, color: Color32, max_w: f32) -> Rect {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let rect = anchor.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// Human "2m", "1h", "3d" style relative time from unix seconds.
pub fn ago(ts: i64) -> String {
    let now = crate::model::now();
    let d = (now - ts).max(0);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", d / 60),
        3600..=86_399 => format!("{}h", d / 3600),
        86_400..=2_591_999 => format!("{}d", d / 86_400),
        _ => format!("{}mo", d / 2_592_000),
    }
}

pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 { format!("{}h {:02}m", s / 3600, (s % 3600) / 60) } else { format!("{}m {:02}s", s / 60, s % 60) }
}

/// Left-to-right row aligned to the top. Unlike `Ui::horizontal_top` it never asks for the
/// (possibly infinite) available height, so it is safe inside scroll areas.
pub fn row_top<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::left_to_right(egui::Align::Min), add)
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations_format_like_the_design() {
        assert_eq!(super::duration(112), "1m 52s");
        assert_eq!(super::duration(30), "0m 30s");
        assert_eq!(super::duration(3720), "1h 02m");
    }
}
