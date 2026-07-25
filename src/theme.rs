//! schr.ag design tokens (oklch converted to sRGB) mapped onto egui.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke};

use crate::config::Accent;

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
};

pub fn palette(theme: egui::Theme) -> &'static Palette {
    match theme {
        egui::Theme::Dark => &DARK,
        egui::Theme::Light => &LIGHT,
    }
}

/// Ask the desktop what color scheme it prefers.
///
/// winit reports no system theme on X11/Wayland, so on Linux we read the XDG
/// desktop portal's `color-scheme` setting over D-Bus (the same source GTK and
/// Firefox use) — shelling out like the fontconfig lookup below. Returns None
/// when the desktop expresses no preference or the lookup fails.
#[cfg(target_os = "linux")]
pub fn system_theme() -> Option<egui::Theme> {
    portal_color_scheme().or_else(gnome_color_scheme)
}

#[cfg(not(target_os = "linux"))]
pub fn system_theme() -> Option<egui::Theme> {
    None
}

/// org.freedesktop.appearance color-scheme: 0 = no preference, 1 = dark, 2 = light
#[cfg(target_os = "linux")]
fn portal_color_scheme() -> Option<egui::Theme> {
    let out = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply=literal",
            "--reply-timeout=1000",
            "--dest=org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings.Read",
            "string:org.freedesktop.appearance",
            "string:color-scheme",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_portal_reply(&String::from_utf8_lossy(&out.stdout))
}

/// Parse a `dbus-send --print-reply=literal` reply like
/// `   variant       variant          uint32 1`.
#[cfg(any(target_os = "linux", test))]
fn parse_portal_reply(reply: &str) -> Option<egui::Theme> {
    match reply.rsplit("uint32").next()?.trim().parse::<u32>().ok()? {
        1 => Some(egui::Theme::Dark),
        2 => Some(egui::Theme::Light),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn gnome_color_scheme() -> Option<egui::Theme> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    parse_gsettings_scheme(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `gsettings get org.gnome.desktop.interface color-scheme` output
/// (`'prefer-dark'`, `'prefer-light'`, or `'default'`).
#[cfg(any(target_os = "linux", test))]
fn parse_gsettings_scheme(s: &str) -> Option<egui::Theme> {
    if s.contains("prefer-dark") {
        Some(egui::Theme::Dark)
    } else if s.contains("prefer-light") {
        Some(egui::Theme::Light)
    } else {
        None
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
    // flat design: panels draw a single 1px border, no drop shadows
    v.window_shadow = egui::epaint::Shadow::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_reply_maps_the_color_scheme_values() {
        // 0 = no preference, 1 = dark, 2 = light
        let reply = |n: u32| format!("   variant       variant          uint32 {n}\n");
        assert_eq!(parse_portal_reply(&reply(1)), Some(egui::Theme::Dark));
        assert_eq!(parse_portal_reply(&reply(2)), Some(egui::Theme::Light));
        assert_eq!(parse_portal_reply(&reply(0)), None);
        assert_eq!(parse_portal_reply(&reply(7)), None);
    }

    #[test]
    fn portal_reply_garbage_is_no_preference() {
        assert_eq!(parse_portal_reply(""), None);
        assert_eq!(
            parse_portal_reply("Error org.freedesktop.portal.Error.NotFound: not found"),
            None
        );
        assert_eq!(parse_portal_reply("variant uint32 banana"), None);
    }

    #[test]
    fn gsettings_output_maps_the_color_scheme_values() {
        assert_eq!(
            parse_gsettings_scheme("'prefer-dark'\n"),
            Some(egui::Theme::Dark)
        );
        assert_eq!(
            parse_gsettings_scheme("'prefer-light'\n"),
            Some(egui::Theme::Light)
        );
        assert_eq!(parse_gsettings_scheme("'default'\n"), None);
        assert_eq!(parse_gsettings_scheme(""), None);
    }

    #[test]
    fn palette_matches_the_theme() {
        assert!(palette(egui::Theme::Dark).background.r() < 128);
        assert!(palette(egui::Theme::Light).background.r() >= 128);
    }

    #[test]
    fn apply_style_overwrites_both_side_by_side_styles() {
        // the palette is the single source of truth: egui keeps a light and a
        // dark style, and apply_style must overwrite both
        let ctx = egui::Context::default();
        for (pal, dark_mode) in [(&DARK, true), (&LIGHT, false)] {
            apply_style(&ctx, pal);
            for t in [egui::Theme::Dark, egui::Theme::Light] {
                let v = &ctx.style_of(t).visuals;
                assert_eq!(v.dark_mode, dark_mode);
                assert_eq!(v.window_fill, pal.card);
            }
        }
    }
}
