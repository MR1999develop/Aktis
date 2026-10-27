//! Colours, sizes and durations of the interface, in one place.
//!
//! The rest of the UI uses only these tokens. Both themes are installed at
//! start-up; egui switches between them following the user's choice
//! (System, Light, Dark). Contrast ratios are checked by the tests below.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Theme, Visuals};

/// Semantic colours of one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// Fills of interactive elements (primary button, selection); one primary per view.
    pub accent: Color32,
    /// Links and accent-coloured text (readable on `bg`).
    pub accent_text: Color32,
    /// Titles, values, main labels.
    pub text_strong: Color32,
    /// Secondary labels and units.
    pub text_weak: Color32,
    /// Borders of fields, sliders and checkboxes.
    pub stroke_strong: Color32,
    /// Decorative dividers.
    pub stroke_weak: Color32,
    /// Secondary fills, hover.
    pub fill: Color32,
    /// Panels.
    pub bg: Color32,
    /// Windows, popups and menus.
    pub bg_raised: Color32,
    /// Toasts and overlays.
    pub bg_overlay: Color32,
    /// Neutral grey behind the photo, never tinted.
    pub canvas: Color32,
    /// Errors (always with text, never colour alone).
    pub error: Color32,
    /// Warnings.
    pub warning: Color32,
    /// Success.
    pub success: Color32,
}

impl Tokens {
    /// The dark theme, default for a photo editor.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            accent: Color32::from_rgb(0x2f, 0x6f, 0xe0),
            accent_text: Color32::from_rgb(0x7a, 0xa7, 0xff),
            text_strong: Color32::from_rgb(0xe6, 0xe6, 0xe6),
            text_weak: Color32::from_rgb(0xa8, 0xa8, 0xa8),
            stroke_strong: Color32::from_rgb(0x74, 0x74, 0x74),
            stroke_weak: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            fill: Color32::from_rgb(0x33, 0x33, 0x33),
            bg: Color32::from_rgb(0x23, 0x23, 0x23),
            bg_raised: Color32::from_rgb(0x2b, 0x2b, 0x2b),
            bg_overlay: Color32::from_rgb(0x36, 0x36, 0x36),
            canvas: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            error: Color32::from_rgb(0xff, 0x7b, 0x72),
            warning: Color32::from_rgb(0xe3, 0xb3, 0x41),
            success: Color32::from_rgb(0x5f, 0xc8, 0x6e),
        }
    }

    /// The light theme.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            accent: Color32::from_rgb(0x1f, 0x5f, 0xcc),
            accent_text: Color32::from_rgb(0x1f, 0x5f, 0xcc),
            text_strong: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            text_weak: Color32::from_rgb(0x55, 0x55, 0x55),
            stroke_strong: Color32::from_rgb(0x86, 0x86, 0x86),
            stroke_weak: Color32::from_rgb(0xd6, 0xd6, 0xd6),
            fill: Color32::from_rgb(0xe6, 0xe6, 0xe6),
            bg: Color32::from_rgb(0xf3, 0xf3, 0xf3),
            bg_raised: Color32::from_rgb(0xff, 0xff, 0xff),
            bg_overlay: Color32::from_rgb(0xff, 0xff, 0xff),
            canvas: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            error: Color32::from_rgb(0xb3, 0x26, 0x1e),
            warning: Color32::from_rgb(0x8a, 0x5a, 0x00),
            success: Color32::from_rgb(0x1e, 0x7a, 0x34),
        }
    }

    /// The tokens of `theme`.
    #[must_use]
    pub const fn of(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self::dark(),
            Theme::Light => Self::light(),
        }
    }
}

/// Spacing scale in points.
pub const SPACE: [f32; 8] = [4., 8., 12., 16., 24., 32., 48., 64.];
/// Font sizes: caption, small, body, large, title, display.
pub const FONT: [f32; 6] = [12., 13., 14., 16., 20., 24.];
/// Corner radius of controls.
pub const RADIUS_CONTROL: u8 = 4;
/// Corner radius of windows, menus and popups.
pub const RADIUS_WINDOW: u8 = 8;
/// Width of the inspector panel: default, minimum, maximum.
pub const INSPECTOR_WIDTH: [f32; 3] = [300., 260., 420.];
/// Seconds a toast stays (5 s like GNOME; errors a little longer).
pub const TOAST_SECONDS: f32 = 5.0;
/// Seconds an error toast stays.
pub const TOAST_ERROR_SECONDS: f32 = 8.0;
/// A job shows a spinner only after this many seconds.
pub const SPINNER_DELAY_SECONDS: f32 = 1.0;

/// Builds egui visuals from `t`.
#[must_use]
pub fn visuals(t: &Tokens, theme: Theme) -> Visuals {
    let mut v = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };
    v.panel_fill = t.bg;
    v.window_fill = t.bg_raised;
    v.faint_bg_color = t.fill;
    v.hyperlink_color = t.accent_text;
    v.error_fg_color = t.error;
    v.warn_fg_color = t.warning;
    v.weak_text_color = Some(t.text_weak);
    v.selection.bg_fill = t.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.window_corner_radius = CornerRadius::same(RADIUS_WINDOW);
    v.menu_corner_radius = CornerRadius::same(RADIUS_WINDOW);
    v.window_stroke = Stroke::new(1.0, t.stroke_weak);

    let w = &mut v.widgets;
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_strong);
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.stroke_strong);
    w.noninteractive.bg_fill = t.bg;
    w.inactive.fg_stroke = Stroke::new(1.0, t.text_strong);
    w.inactive.bg_stroke = Stroke::new(1.0, t.stroke_strong);
    w.inactive.weak_bg_fill = t.fill;
    w.hovered.fg_stroke = Stroke::new(1.0, t.text_strong);
    w.active.fg_stroke = Stroke::new(1.0, t.text_strong);
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::same(RADIUS_CONTROL);
    }
    v
}

/// Installs both themes, sizes and spacing on `ctx`.
pub fn install(ctx: &egui::Context) {
    for theme in [Theme::Dark, Theme::Light] {
        ctx.set_visuals_of(theme, visuals(&Tokens::of(theme), theme));
    }
    ctx.all_styles_mut(|style| {
        style.spacing.interact_size.y = 24.0;
        style.spacing.item_spacing = egui::vec2(SPACE[1], 6.0);
        style.spacing.button_padding = egui::vec2(SPACE[1], 4.0);
        for (text_style, size) in [
            (egui::TextStyle::Small, FONT[0]),
            (egui::TextStyle::Body, FONT[2]),
            (egui::TextStyle::Button, FONT[2]),
            (egui::TextStyle::Monospace, FONT[1]),
            (egui::TextStyle::Heading, FONT[4]),
        ] {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = size;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two colours (1 to 21).
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn channel(c: u8) -> f32 {
            let c = f32::from(c) / 255.0;
            if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        fn luminance(c: Color32) -> f32 {
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        }
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_and_controls_meet_wcag_contrast_in_both_themes() {
        for (name, t) in [("dark", Tokens::dark()), ("light", Tokens::light())] {
            for bg in [t.bg, t.bg_raised, t.bg_overlay] {
                assert!(contrast(t.text_strong, bg) >= 4.5, "{name}: strong text");
                assert!(contrast(t.text_weak, bg) >= 4.5, "{name}: weak text");
                assert!(contrast(t.accent_text, bg) >= 4.5, "{name}: links");
                assert!(contrast(t.error, bg) >= 4.5, "{name}: error text");
            }
            assert!(contrast(t.stroke_strong, t.bg) >= 3.0, "{name}: control borders");
            assert!(contrast(t.accent, t.bg) >= 3.0, "{name}: accent fills");
            assert!(contrast(Color32::WHITE, t.accent) >= 4.5, "{name}: text on accent");
        }
    }

    #[test]
    fn the_canvas_is_a_neutral_grey() {
        for t in [Tokens::dark(), Tokens::light()] {
            assert!(t.canvas.r() == t.canvas.g() && t.canvas.g() == t.canvas.b());
        }
    }
}
