//! Design tokens: colors, type, radii and the egui style derived from them.
//! Values are taken from the Kiln design artifact (dark and light palettes).

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};
use std::sync::{Arc, RwLock};

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    // Color32 stores premultiplied alpha.
    let a8 = (a * 255.0 + 0.5) as u8;
    Color32::from_rgba_premultiplied(pm(r, a8), pm(g, a8), pm(b, a8), a8)
}
const fn pm(c: u8, a8: u8) -> u8 {
    ((c as u16 * a8 as u16 + 127) / 255) as u8
}
pub const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}
/// `tint(#51a2ff, .12)` from the design: the color at reduced opacity.
pub fn tint(c: Color32, a: f32) -> Color32 {
    rgba(c.r(), c.g(), c.b(), a)
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub fg: Color32,
    pub fg_soft: Color32,
    pub muted: Color32,
    pub muted80: Color32,
    pub subtle: Color32,
    pub icon: Color32,
    pub faint: Color32,
    pub section: Color32,
    pub border: Color32,
    pub border_soft: Color32,
    pub border_faint: Color32,
    pub input_border: Color32,
    pub ctrl_bg: Color32,
    pub card: Color32,
    pub raised: Color32,
    pub popover: Color32,
    pub bubble: Color32,
    pub sidebar: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub row_sel: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub info: Color32,
    pub danger: Color32,
    pub stop: Color32,
    pub add: Color32,
    pub del: Color32,
    pub add_bg: Color32,
    pub del_bg: Color32,
    pub hunk_bg: Color32,
    pub line_no: Color32,
    pub ctx_fg: Color32,
    pub changed_fg: Color32,
    pub code_bg: Color32,
    pub kbd_bg: Color32,
    pub track: Color32,
    pub thumb_off: Color32,
    pub backdrop: Color32,
    pub diff_header: Color32,
}

impl Palette {
    pub fn dark(accent: Color32, blue_orange: bool) -> Self {
        let (add, del) = if blue_orange { (hex(0x51a2ff), hex(0xff8904)) } else { (hex(0x00d492), hex(0xff6467)) };
        Self {
            dark: true,
            bg: hex(0x000000),
            fg: hex(0xf5f5f5),
            fg_soft: rgba(245, 245, 245, 0.9),
            muted: hex(0xa1a1a1),
            muted80: rgba(161, 161, 161, 0.8),
            subtle: hex(0x8f8f8f),
            icon: hex(0x8b8b8b),
            faint: rgba(161, 161, 161, 0.6),
            section: hex(0xa3a3a3),
            border: rgba(255, 255, 255, 0.08),
            border_soft: rgba(255, 255, 255, 0.048),
            border_faint: rgba(255, 255, 255, 0.04),
            input_border: rgba(255, 255, 255, 0.11),
            ctrl_bg: rgba(255, 255, 255, 0.035),
            card: hex(0x070707),
            raised: hex(0x0e0e0e),
            popover: hex(0x101010),
            bubble: hex(0x1e1e1e),
            sidebar: hex(0x111111),
            hover: rgba(241, 243, 247, 0.08),
            selected: rgba(241, 243, 247, 0.11),
            row_sel: rgba(255, 255, 255, 0.05),
            accent,
            success: hex(0x00d492),
            warning: hex(0xffb900),
            info: hex(0x51a2ff),
            danger: hex(0xff6467),
            stop: rgba(251, 92, 92, 0.9),
            add,
            del,
            add_bg: tint(add, 0.1),
            del_bg: tint(del, 0.1),
            hunk_bg: rgba(81, 162, 255, 0.06),
            line_no: hex(0x7a7a7a),
            ctx_fg: hex(0xc4c4c4),
            changed_fg: hex(0xe5e5e5),
            code_bg: rgba(255, 255, 255, 0.06),
            kbd_bg: rgba(245, 245, 245, 0.07),
            track: rgba(255, 255, 255, 0.11),
            thumb_off: hex(0xa1a1a1),
            backdrop: rgba(0, 0, 0, 0.64),
            diff_header: hex(0x0a0a0a),
        }
    }

    pub fn light(accent: Color32, blue_orange: bool) -> Self {
        let (add, del) = if blue_orange { (hex(0x1447e6), hex(0xbb4d00)) } else { (hex(0x007a55), hex(0xc10007)) };
        Self {
            dark: false,
            bg: hex(0xfcfcfc),
            fg: hex(0x18181b),
            fg_soft: rgba(24, 24, 27, 0.9),
            muted: hex(0x71717a),
            muted80: hex(0x71717a),
            subtle: hex(0x71717a),
            icon: hex(0x86868d),
            faint: rgba(113, 113, 122, 0.7),
            section: hex(0x71717a),
            border: hex(0xe4e4e7),
            border_soft: rgba(228, 228, 231, 0.6),
            border_faint: rgba(228, 228, 231, 0.5),
            input_border: hex(0xd4d4d8),
            ctrl_bg: hex(0xffffff),
            card: hex(0xffffff),
            raised: hex(0xffffff),
            popover: hex(0xffffff),
            bubble: hex(0xf0f0f2),
            sidebar: hex(0xfafafa),
            hover: hex(0xf4f4f5),
            selected: hex(0xebebee),
            row_sel: hex(0xffffff),
            accent,
            success: hex(0x007a55),
            warning: hex(0xbb4d00),
            info: hex(0x1447e6),
            danger: hex(0xc10007),
            stop: hex(0xdc2626),
            add,
            del,
            add_bg: tint(add, 0.1),
            del_bg: tint(del, 0.1),
            hunk_bg: rgba(20, 71, 230, 0.06),
            line_no: hex(0x9f9fa9),
            ctx_fg: hex(0x3f3f46),
            changed_fg: hex(0x18181b),
            code_bg: rgba(24, 24, 27, 0.06),
            kbd_bg: rgba(24, 24, 27, 0.07),
            track: hex(0xd4d4d8),
            thumb_off: hex(0xffffff),
            backdrop: rgba(252, 252, 252, 0.6),
            diff_header: hex(0xf4f4f5),
        }
    }

    /// Contrast slider ≥ 50% swaps muted text and soft borders for the strong variants.
    pub fn with_contrast(mut self, contrast: u32) -> Self {
        if contrast >= 50 {
            self.muted = self.fg;
            self.muted80 = self.fg;
            self.subtle = self.fg;
            self.border_soft = self.border;
            self.border_faint = self.border;
        }
        self
    }
}

pub fn accent_for_theme(name: &str, dark: bool) -> Color32 {
    match name {
        "Ember" => hex(0xc2410c),
        "Forest" => hex(0x15803d),
        "Violet" => hex(0x7c3aed),
        _ if dark => hex(0x346bf1),
        _ => hex(0x1b4ed8),
    }
}

static PALETTE: RwLock<Option<Palette>> = RwLock::new(None);

/// The palette currently in effect (set once per frame by the app).
pub fn p() -> Palette {
    PALETTE.read().ok().and_then(|g| *g).unwrap_or_else(|| Palette::dark(hex(0x346bf1), false))
}
pub fn set_palette(pal: Palette) {
    if let Ok(mut g) = PALETTE.write() {
        *g = Some(pal);
    }
}

// ---- spacing / radii / sizes -------------------------------------------------

pub const R_SM: u8 = 4;
pub const R_MD: u8 = 6;
pub const R_LG: u8 = 8;
pub const R_XL: u8 = 12;
pub const R_2XL: u8 = 16;
pub const HEADER_H: f32 = 52.0;
pub const LANE_MAX: f32 = 736.0;

pub fn radius(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

// ---- type -------------------------------------------------------------------

pub fn medium() -> FontFamily {
    FontFamily::Name("medium".into())
}
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}
pub fn ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn ui_medium(size: f32) -> FontId {
    FontId::new(size, medium())
}
pub fn ui_semibold(size: f32) -> FontId {
    FontId::new(size, semibold())
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Font scale relative to the 14px design baseline (settings → interface font size).
static UI_SCALE: RwLock<(f32, f32)> = RwLock::new((14.0, 13.0));
pub fn set_font_sizes(ui_px: f32, mono_px: f32) {
    if let Ok(mut g) = UI_SCALE.write() {
        *g = (ui_px, mono_px);
    }
}
/// Scale a design pixel size by the user's interface font preference.
pub fn fs(px: f32) -> f32 {
    let base = UI_SCALE.read().map(|g| g.0).unwrap_or(14.0);
    (px * base / 14.0).round()
}
pub fn mono_fs(px: f32) -> f32 {
    let base = UI_SCALE.read().map(|g| g.1).unwrap_or(13.0);
    (px * base / 13.0 * 2.0).round() / 2.0
}

fn read_font(paths: &[&str]) -> Option<&'static [u8]> {
    for p in paths {
        if let Ok(bytes) = std::fs::read(p) {
            return Some(Box::leak(bytes.into_boxed_slice()));
        }
    }
    None
}

fn system_font_dir() -> String {
    std::env::var("WINDIR").map(|w| format!("{w}\\Fonts")).unwrap_or_else(|_| "C:\\Windows\\Fonts".into())
}

/// Installs Segoe UI Variable (400/500/600 via its `wght` axis) and Consolas on Windows,
/// common system fonts elsewhere, or the user's chosen font files. Falls back to egui's bundled fonts.
pub fn install_fonts(ctx: &egui::Context, ui_font_path: &str, mono_font_path: &str) {
    let mut defs = FontDefinitions::default();
    let fonts = system_font_dir();
    let var = format!("{fonts}\\SegUIVar.ttf");
    let regular = format!("{fonts}\\segoeui.ttf");
    let semib = format!("{fonts}\\seguisb.ttf");
    let consolas = format!("{fonts}\\consola.ttf");

    let custom_ui = (!ui_font_path.trim().is_empty()).then(|| read_font(&[ui_font_path.trim()])).flatten();
    let mut ui_names: [Option<String>; 3] = [None, None, None];
    if let Some(bytes) = custom_ui {
        defs.font_data.insert("ui".into(), Arc::new(FontData::from_static(bytes)));
        ui_names = [Some("ui".into()), Some("ui".into()), Some("ui".into())];
    } else if let Some(bytes) = read_font(&[&var]) {
        for (i, w) in [400.0f32, 500.0, 600.0].into_iter().enumerate() {
            let name = format!("ui{}", w as u32);
            let tweak = egui::FontTweak { coords: egui::epaint::text::VariationCoords::new([("wght", w)]), ..Default::default() };
            defs.font_data.insert(name.clone(), Arc::new(FontData::from_static(bytes).tweak(tweak)));
            ui_names[i] = Some(name);
        }
    } else {
        let mac_linux = [
            "/System/Library/Fonts/SFNS.ttf",
            "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        if let Some(bytes) = read_font(&[regular.as_str()]).or_else(|| read_font(&mac_linux)) {
            defs.font_data.insert("ui400".into(), Arc::new(FontData::from_static(bytes)));
            ui_names[0] = Some("ui400".into());
            ui_names[1] = Some("ui400".into());
        }
        if let Some(bytes) = read_font(&[semib.as_str()]) {
            defs.font_data.insert("ui600".into(), Arc::new(FontData::from_static(bytes)));
            ui_names[2] = Some("ui600".into());
        } else {
            ui_names[2] = ui_names[0].clone();
        }
    }
    let custom_mono = (!mono_font_path.trim().is_empty()).then(|| read_font(&[mono_font_path.trim()])).flatten();
    let mono_bytes = custom_mono.or_else(|| {
        read_font(&[
            consolas.as_str(),
            "/System/Library/Fonts/SFNSMono.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        ])
    });
    if let Some(bytes) = mono_bytes {
        defs.font_data.insert("mono".into(), Arc::new(FontData::from_static(bytes)));
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    }
    let fallback = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    // Symbols the UI fonts may lack (arrows, ellipsis) come from Segoe UI Symbol when present.
    let mut extra = Vec::new();
    if let Some(bytes) = read_font(&[&format!("{fonts}\\seguisym.ttf")]) {
        defs.font_data.insert("symbols".into(), Arc::new(FontData::from_static(bytes)));
        extra.push("symbols".to_string());
    }
    let family = |name: &Option<String>| -> Vec<String> {
        let mut v: Vec<String> = name.iter().cloned().collect();
        v.extend(fallback.iter().cloned());
        v.extend(extra.iter().cloned());
        v
    };
    defs.families.insert(FontFamily::Proportional, family(&ui_names[0]));
    defs.families.insert(medium(), family(&ui_names[1]));
    defs.families.insert(semibold(), family(&ui_names[2]));
    if let Some(m) = defs.families.get_mut(&FontFamily::Monospace) {
        m.extend(extra.iter().cloned());
    }
    ctx.set_fonts(defs);
}

/// Maps the palette onto egui's own widget visuals so stock widgets (text edits, scroll bars,
/// combo popups, selection) blend with the custom ones.
pub fn apply_style(ctx: &egui::Context, pal: &Palette) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, ui(fs(12.0))),
            (TextStyle::Body, ui(fs(14.0))),
            (TextStyle::Button, ui(fs(14.0))),
            (TextStyle::Heading, ui_semibold(fs(16.0))),
            (TextStyle::Monospace, mono(mono_fs(13.0))),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.interact_size = egui::vec2(24.0, 24.0);
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.menu_margin = egui::Margin::same(4);
        style.interaction.selectable_labels = true;
        style.url_in_tooltip = true;

        let v = &mut style.visuals;
        *v = if pal.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.override_text_color = Some(pal.fg);
        v.panel_fill = pal.bg;
        v.window_fill = pal.popover;
        v.extreme_bg_color = pal.ctrl_bg;
        v.faint_bg_color = pal.card;
        v.code_bg_color = pal.code_bg;
        v.window_stroke = Stroke::new(1.0, pal.border);
        v.window_corner_radius = radius(R_XL);
        v.menu_corner_radius = radius(10);
        v.window_shadow = egui::Shadow { offset: [0, 16], blur: 40, spread: 0, color: rgba(0, 0, 0, if pal.dark { 0.6 } else { 0.12 }) };
        v.popup_shadow = v.window_shadow;
        v.hyperlink_color = pal.info;
        v.warn_fg_color = pal.warning;
        v.error_fg_color = pal.danger;
        v.selection.bg_fill = tint(pal.accent, 0.45);
        v.selection.stroke = Stroke::new(1.0, pal.fg);
        v.text_cursor.stroke = Stroke::new(1.5, pal.fg);
        v.striped = false;
        v.indent_has_left_vline = false;
        let w = &mut v.widgets;
        for (state, fill, stroke) in [
            (&mut w.noninteractive, Color32::TRANSPARENT, pal.border),
            (&mut w.inactive, pal.ctrl_bg, pal.input_border),
            (&mut w.hovered, pal.hover, pal.input_border),
            (&mut w.active, pal.selected, pal.input_border),
            (&mut w.open, pal.selected, pal.input_border),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.corner_radius = radius(R_LG);
            state.fg_stroke = Stroke::new(1.0, pal.fg);
            state.expansion = 0.0;
        }
        w.noninteractive.fg_stroke = Stroke::new(1.0, pal.muted);
        w.inactive.fg_stroke = Stroke::new(1.0, pal.fg);
    });
}
