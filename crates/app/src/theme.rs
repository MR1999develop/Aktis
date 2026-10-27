//! Colours, sizes and durations of the interface, in one place.
//!
//! The rest of the UI uses only these tokens. Both themes are installed at
//! start-up; egui switches between them following the user's choice
//! (System, Light, Dark). Contrast ratios are checked by the tests below.

use eframe::egui::{self, Color32, CornerRadius, Stroke, Theme, Visuals};

/// Semantic colours of one theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    /// Fills of interactive elements (primary button, selection, active tool); one primary per view.
    pub accent: Color32,
    /// Text and icons drawn on an `accent` fill.
    pub on_accent: Color32,
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
    /// Side panels.
    pub bg: Color32,
    /// Top bar and status bar.
    pub bar: Color32,
    /// Windows, popups and menus.
    pub bg_raised: Color32,
    /// Toasts, overlays and badges.
    pub bg_overlay: Color32,
    /// Neutral grey behind the photo, never tinted.
    pub canvas: Color32,
    /// Background of the node graph.
    pub graph: Color32,
    /// Grid lines of the node graph.
    pub graph_grid: Color32,
    /// Filled rating stars.
    pub star: Color32,
    /// Image ports and links of the node graph.
    pub port_image: Color32,
    /// Mask ports and links of the node graph.
    pub port_mask: Color32,
    /// Errors (always with text, never colour alone).
    pub error: Color32,
    /// Warnings.
    pub warning: Color32,
    /// Success.
    pub success: Color32,
}

impl Tokens {
    /// The dark theme (warm greys and a copper accent, from the Interface design),
    /// default for a photo editor.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            accent: Color32::from_rgb(0xe2, 0x89, 0x4f),
            on_accent: Color32::from_rgb(0x1a, 0x12, 0x10),
            accent_text: Color32::from_rgb(0xe2, 0x89, 0x4f),
            text_strong: Color32::from_rgb(0xe9, 0xe6, 0xe1),
            text_weak: Color32::from_rgb(0xa2, 0x9d, 0x95),
            stroke_strong: Color32::from_rgb(0x77, 0x71, 0x6a),
            stroke_weak: Color32::from_rgb(0x2a, 0x28, 0x26),
            fill: Color32::from_rgb(0x24, 0x22, 0x20),
            bg: Color32::from_rgb(0x18, 0x17, 0x16),
            bar: Color32::from_rgb(0x16, 0x15, 0x14),
            bg_raised: Color32::from_rgb(0x23, 0x21, 0x20),
            bg_overlay: Color32::from_rgb(0x2c, 0x29, 0x26),
            canvas: Color32::from_rgb(0x0e, 0x0e, 0x0e),
            graph: Color32::from_rgb(0x1b, 0x1a, 0x19),
            graph_grid: Color32::from_rgb(0x24, 0x22, 0x1f),
            star: Color32::from_rgb(0xe5, 0xa0, 0x0d),
            port_image: Color32::from_rgb(0x7f, 0xbf, 0x6a),
            port_mask: Color32::from_rgb(0x5b, 0x8f, 0xd6),
            error: Color32::from_rgb(0xff, 0x8a, 0x7a),
            warning: Color32::from_rgb(0xe3, 0xb3, 0x41),
            success: Color32::from_rgb(0x7f, 0xbf, 0x6a),
        }
    }

    /// The light theme.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            accent: Color32::from_rgb(0xa4, 0x52, 0x1d),
            on_accent: Color32::WHITE,
            accent_text: Color32::from_rgb(0xa4, 0x52, 0x1d),
            text_strong: Color32::from_rgb(0x1f, 0x1c, 0x1a),
            text_weak: Color32::from_rgb(0x5c, 0x57, 0x51),
            stroke_strong: Color32::from_rgb(0x86, 0x82, 0x7d),
            stroke_weak: Color32::from_rgb(0xdd, 0xd9, 0xd4),
            fill: Color32::from_rgb(0xe9, 0xe6, 0xe2),
            bg: Color32::from_rgb(0xf3, 0xf2, 0xf0),
            bar: Color32::from_rgb(0xec, 0xea, 0xe7),
            bg_raised: Color32::from_rgb(0xff, 0xff, 0xff),
            bg_overlay: Color32::from_rgb(0xff, 0xff, 0xff),
            canvas: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            graph: Color32::from_rgb(0xe9, 0xe6, 0xe2),
            graph_grid: Color32::from_rgb(0xdd, 0xd9, 0xd4),
            star: Color32::from_rgb(0xb7, 0x7c, 0x00),
            port_image: Color32::from_rgb(0x3f, 0x8a, 0x2a),
            port_mask: Color32::from_rgb(0x2f, 0x62, 0xb0),
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
/// Width of the right panel (histogram and node settings): default, minimum, maximum.
pub const INSPECTOR_WIDTH: [f32; 3] = [330., 280., 460.];
/// Width of the left panel (Explorer and filmstrip): default, minimum, maximum.
pub const LEFT_WIDTH: [f32; 3] = [420., 340., 640.];
/// Width of the filmstrip column inside the left panel.
pub const FILMSTRIP_WIDTH: f32 = 172.0;
/// Height of the node graph under the viewer: default, minimum, maximum.
pub const GRAPH_HEIGHT: [f32; 3] = [190., 120., 420.];
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
    v.selection.stroke = Stroke::new(1.0, t.on_accent);
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
            for bg in [t.bg, t.bar, t.bg_raised, t.bg_overlay] {
                assert!(contrast(t.text_strong, bg) >= 4.5, "{name}: strong text");
                assert!(contrast(t.text_weak, bg) >= 4.5, "{name}: weak text");
                assert!(contrast(t.accent_text, bg) >= 4.5, "{name}: links");
                assert!(contrast(t.error, bg) >= 4.5, "{name}: error text");
            }
            assert!(contrast(t.stroke_strong, t.bg) >= 3.0, "{name}: control borders");
            assert!(contrast(t.accent, t.bg) >= 3.0, "{name}: accent fills");
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "{name}: text on accent");
        }
    }

    #[test]
    fn the_canvas_is_a_neutral_grey() {
        for t in [Tokens::dark(), Tokens::light()] {
            assert!(t.canvas.r() == t.canvas.g() && t.canvas.g() == t.canvas.b());
        }
    }
}
