//! schr.ag design tokens (oklch converted to sRGB) mapped onto egui.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke};

use crate::config::{Accent, Theme};

pub struct Palette {
    pub background: Color32,
    pub card: Color32,
    pub popover: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub border: Color32,
    pub foreground: Color32,
    pub muted_fg: Color32,
    pub primary: Color32,
    pub destructive: Color32,
    pub success: Color32,
    accents: [Color32; 6],
    pub shadow_alpha: u8,
}

pub const DARK: Palette = Palette {
    background: Color32::from_rgb(18, 18, 20),
    card: Color32::from_rgb(25, 25, 28),
    popover: Color32::from_rgb(30, 30, 32),
    muted: Color32::from_rgb(36, 36, 38),
    accent: Color32::from_rgb(42, 42, 45),
    border: Color32::from_rgb(50, 50, 54),
    foreground: Color32::from_rgb(231, 231, 234),
    muted_fg: Color32::from_rgb(154, 154, 161),
    primary: Color32::from_rgb(217, 124, 80),
    destructive: Color32::from_rgb(202, 85, 81),
    success: Color32::from_rgb(111, 176, 125),
    accents: [
        Color32::from_rgb(217, 124, 80),  // orange
        Color32::from_rgb(154, 191, 115), // lime
        Color32::from_rgb(108, 184, 202), // cyan
        Color32::from_rgb(162, 138, 205), // violet
        Color32::from_rgb(206, 132, 167), // pink
        Color32::from_rgb(213, 179, 106), // amber
    ],
    shadow_alpha: 100,
};

pub const LIGHT: Palette = Palette {
    background: Color32::from_rgb(250, 250, 251),
    card: Color32::WHITE,
    popover: Color32::WHITE,
    muted: Color32::from_rgb(240, 240, 241),
    accent: Color32::from_rgb(231, 231, 234),
    border: Color32::from_rgb(218, 218, 221),
    foreground: Color32::from_rgb(24, 24, 27),
    muted_fg: Color32::from_rgb(98, 98, 105),
    primary: Color32::from_rgb(201, 103, 54),
    destructive: Color32::from_rgb(189, 65, 63),
    success: Color32::from_rgb(62, 134, 81),
    accents: [
        Color32::from_rgb(201, 103, 54),
        Color32::from_rgb(110, 148, 65),
        Color32::from_rgb(32, 130, 149),
        Color32::from_rgb(122, 93, 169),
        Color32::from_rgb(178, 87, 133),
        Color32::from_rgb(192, 142, 67),
    ],
    shadow_alpha: 34,
};

pub fn palette(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

impl Palette {
    pub fn accent_color(&self, a: Accent) -> Color32 {
        self.accents[a as usize]
    }
}

/// --font-label: 12px mono, medium, uppercase, wide tracking
pub fn font_label() -> FontId {
    FontId::new(10.0, FontFamily::Monospace)
}
/// --font-ui: 14px sans medium
pub fn font_ui() -> FontId {
    FontId::new(13.0, FontFamily::Proportional)
}
/// bubble / chat body text
pub fn font_body() -> FontId {
    FontId::new(14.0, FontFamily::Proportional)
}

pub fn apply_style(ctx: &egui::Context, pal: &Palette) {
    // the palette is our single source of truth, so overwrite both the light
    // and dark style that egui 0.35 keeps side by side
    ctx.all_styles_mut(|style| apply_to(style, pal));
}

fn apply_to(style: &mut egui::Style, pal: &Palette) {
    let v = &mut style.visuals;
    v.dark_mode = pal.background.r() < 128;
    v.override_text_color = Some(pal.foreground);
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = pal.card;
    v.window_stroke = Stroke::new(1.0_f32, pal.border);
    v.extreme_bg_color = pal.background; // text edit background
    v.faint_bg_color = pal.muted;
    v.selection.bg_fill = pal.primary.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, pal.primary);

    let radius = CornerRadius::same(8);
    let w = &mut v.widgets;
    for wv in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        wv.corner_radius = radius;
        wv.fg_stroke.color = pal.foreground;
    }
    w.noninteractive.bg_fill = pal.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, pal.border);
    w.noninteractive.fg_stroke.color = pal.muted_fg;
    w.inactive.bg_fill = pal.muted;
    w.inactive.weak_bg_fill = pal.muted;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, pal.border);
    w.hovered.bg_fill = pal.accent;
    w.hovered.weak_bg_fill = pal.accent;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, pal.border);
    w.active.bg_fill = pal.accent;
    w.active.weak_bg_fill = pal.accent;
    w.active.bg_stroke = Stroke::new(1.0_f32, pal.border);
    w.open.bg_fill = pal.popover;
    w.open.weak_bg_fill = pal.popover;

    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);

    use egui::TextStyle::*;
    style
        .text_styles
        .insert(Body, FontId::new(13.0, FontFamily::Proportional));
    style
        .text_styles
        .insert(Button, FontId::new(13.0, FontFamily::Proportional));
    style
        .text_styles
        .insert(Small, FontId::new(10.0, FontFamily::Monospace));
    style
        .text_styles
        .insert(Heading, FontId::new(15.0, FontFamily::Proportional));
    style
        .text_styles
        .insert(Monospace, FontId::new(11.0, FontFamily::Monospace));
}

/// Load the design-system fonts (Instrument Sans / JetBrains Mono) with
/// graceful fontconfig fallback to whatever the system resolves.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let add = |key: &str, queries: &[&str], fonts: &mut egui::FontDefinitions| -> bool {
        for q in queries {
            let Ok(out) = std::process::Command::new("fc-match")
                .args(["-f", "%{file}", q])
                .output()
            else {
                continue;
            };
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if path.is_empty() {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&path) {
                fonts.font_data.insert(
                    key.to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(bytes)),
                );
                return true;
            }
        }
        false
    };

    if add(
        "ds-sans",
        &["Instrument Sans", "Inter", "sans-serif"],
        &mut fonts,
    ) {
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ds-sans".to_owned());
    }
    if add(
        "ds-sans-semibold",
        &[
            "Instrument Sans:weight=semibold",
            "Inter:weight=semibold",
            "sans-serif:weight=semibold",
        ],
        &mut fonts,
    ) {
        fonts.families.insert(
            FontFamily::Name("semibold".into()),
            vec!["ds-sans-semibold".to_owned(), "ds-sans".to_owned()],
        );
    }
    // the "semibold" family must always resolve — fall back to the default
    // proportional chain if fontconfig gave us nothing
    if !fonts
        .families
        .contains_key(&FontFamily::Name("semibold".into()))
    {
        let fallback = fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        fonts
            .families
            .insert(FontFamily::Name("semibold".into()), fallback);
    }
    if add("ds-mono", &["JetBrains Mono", "monospace"], &mut fonts) {
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "ds-mono".to_owned());
    }
    ctx.set_fonts(fonts);
}
