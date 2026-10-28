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
    /// The dark theme: the neutral whites, greys and blacks of a photo viewer
    /// (no tint, so colours are judged on true greys) with the copper accent
    /// of the Interface design; default for a photo editor.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            accent: Color32::from_rgb(0xe2, 0x89, 0x4f),
            on_accent: Color32::from_rgb(0x1a, 0x12, 0x10),
            accent_text: Color32::from_rgb(0xe2, 0x89, 0x4f),
            text_strong: Color32::from_rgb(0xff, 0xff, 0xff),
            text_weak: Color32::from_rgb(0xb0, 0xb0, 0xb0),
            stroke_strong: Color32::from_rgb(0x80, 0x80, 0x80),
            stroke_weak: Color32::from_rgb(0x2c, 0x2c, 0x2c),
            fill: Color32::from_rgb(0x24, 0x24, 0x24),
            bg: Color32::from_rgb(0x20, 0x20, 0x20),
            bar: Color32::from_rgb(0x16, 0x16, 0x16),
            bg_raised: Color32::from_rgb(0x25, 0x25, 0x26),
            bg_overlay: Color32::from_rgb(0x2e, 0x2e, 0x2e),
            canvas: Color32::from_rgb(0x12, 0x12, 0x12),
            graph: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            graph_grid: Color32::from_rgb(0x26, 0x26, 0x26),
            star: Color32::from_rgb(0xe5, 0xa0, 0x0d),
            port_image: Color32::from_rgb(0x7f, 0xbf, 0x6a),
            port_mask: Color32::from_rgb(0x5b, 0x8f, 0xd6),
            error: Color32::from_rgb(0xff, 0x8a, 0x7a),
            warning: Color32::from_rgb(0xe3, 0xb3, 0x41),
            success: Color32::from_rgb(0x5c, 0xbd, 0x50),
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

/// Red, green and blue of the histogram, translucent so overlaps mix; the
/// histogram sits on `canvas`, the same in both themes.
pub const HISTOGRAM: [Color32; 3] = [
    Color32::from_rgba_premultiplied(81, 28, 25, 90),
    Color32::from_rgba_premultiplied(32, 71, 32, 90),
    Color32::from_rgba_premultiplied(28, 46, 85, 90),
];

/// Curve lines and handles of the RGB (neutral), Luma (grey), red, green
/// and blue curves and levels; they sit on `canvas`, the same in both themes.
pub const CHANNEL: [Color32; 5] = [
    Color32::from_rgb(0xe8, 0xe8, 0xe8),
    Color32::from_rgb(0xa0, 0xa0, 0xa0),
    Color32::from_rgb(0xe0, 0x55, 0x4d),
    Color32::from_rgb(0x5c, 0xbd, 0x50),
    Color32::from_rgb(0x4f, 0x8c, 0xe8),
];

/// Ends of the strip under the kelvin slider: a cooler light (blue image
/// correction) on the left, a warmer one on the right.
pub const KELVIN_STRIP: [Color32; 2] = [Color32::from_rgb(0x3f, 0x76, 0xd6), Color32::from_rgb(0xe8, 0xb3, 0x3a)];

/// Ends of the strip under the tint slider: green on the left, magenta on the right.
pub const TINT_STRIP: [Color32; 2] = [Color32::from_rgb(0x4c, 0xb0, 0x4c), Color32::from_rgb(0xc8, 0x4c, 0xc8)];

/// Ends of the strips under lightness sliders and arcs: dark to light grey.
pub const LIGHTNESS_STRIP: [Color32; 2] = [Color32::from_rgb(0x1c, 0x1c, 0x1c), Color32::from_rgb(0xf0, 0xf0, 0xf0)];

/// Brightness (HSV value, 0–1) of the colours of the hue wheels, chips and strips.
pub const HUE_VALUE: f32 = 0.9;

/// Handles drawn on a colour wheel (puck, range edges, arc knobs): a light
/// line over a dark shadow, so they show on every hue (the same in both themes).
pub const WHEEL_HANDLE: [Color32; 2] =
    [Color32::from_rgb(0xf2, 0xf2, 0xf2), Color32::from_rgba_premultiplied(0, 0, 0, 160)];

/// Shade over the part of a colour wheel outside the chosen range.
pub const WHEEL_DIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 165);

/// The colour of `hue` (degrees) at `saturation` and `value` (0–1), for the
/// colour wheels, chips and strips: the hue is what they show, so it is the
/// same in both themes.
#[must_use]
pub fn hue_color(hue: f32, saturation: f32, value: f32) -> Color32 {
    screen_color(editor::color_wheel::hsv(hue, saturation, value))
}

/// A linear-light colour (a tool's colour value) as a swatch on screen.
#[must_use]
pub fn linear_color(rgb: [f32; 3]) -> Color32 {
    screen_color(rgb.map(editor::adjust::encode))
}

/// A colour from R, G and B already encoded for the screen (0–1).
fn screen_color(rgb: [f32; 3]) -> Color32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–255 after the clamp
    let [r, g, b] = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Color32::from_rgb(r, g, b)
}

/// The mask shown over the photo (red, as photo editors use), the same in
/// both themes.
pub const MASK_OVERLAY: Color32 = Color32::from_rgb(0xff, 0x30, 0x30);

/// Shade over the part of the photo a crop leaves out (on the photo, the
/// same in both themes).
pub const CROP_DIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 136);

/// Circles and lines of the Heal and Clone spots: a light line over a dark
/// shadow, so they show on any photo (the same in both themes).
pub const SPOT_LINE: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
/// The shadow under [`SPOT_LINE`].
pub const SPOT_SHADOW: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 150);

/// The viewer's background for the choice in the Preferences; `canvas` is
/// the interface's own (the [`Tokens::canvas`] of the current theme).
#[must_use]
pub fn viewer_background(choice: crate::settings::ViewerBackground, canvas: Color32) -> Color32 {
    use crate::settings::ViewerBackground as B;
    match choice {
        B::Interface => canvas,
        B::White => Color32::from_rgb(0xff, 0xff, 0xff),
        B::LightGrey => Color32::from_rgb(0xc8, 0xc8, 0xc8),
        B::MiddleGrey => Color32::from_rgb(0x77, 0x77, 0x77),
        B::Black => Color32::from_rgb(0x00, 0x00, 0x00),
    }
}

/// Star and border of the rating badge drawn over a thumbnail (on the photo,
/// the same in both themes).
pub const BADGE_STAR: Color32 = Color32::from_rgb(0xff, 0xb9, 0x00);

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
/// Width of the left panel (Explorer, with History and Metadata under it): default, minimum, maximum.
pub const LEFT_WIDTH: [f32; 3] = [250., 180., 480.];
/// Narrowest filmstrip (and its first width); it can be dragged wider.
pub const FILMSTRIP_WIDTH: f32 = 172.0;
/// Height of the node graph under the viewer: default, minimum, maximum.
pub const GRAPH_HEIGHT: [f32; 3] = [190., 120., 420.];
/// History/Metadata tabs height under the Explorer: default, min, max.
pub const TABS_HEIGHT: [f32; 3] = [280., 140., 560.];
/// Height of the histogram above the settings.
pub const HISTOGRAM_HEIGHT: f32 = 110.0;
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
    if theme == Theme::Dark {
        // a photo viewer's control greys: fields #1C1C1C, buttons #242424,
        // hovered #303030, pressed #3F3F3F.
        v.extreme_bg_color = Color32::from_rgb(0x1c, 0x1c, 0x1c);
        w.inactive.bg_fill = Color32::from_rgb(0x1c, 0x1c, 0x1c);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x30, 0x30, 0x30);
        w.hovered.bg_fill = Color32::from_rgb(0x30, 0x30, 0x30);
        w.active.weak_bg_fill = Color32::from_rgb(0x3f, 0x3f, 0x3f);
        w.active.bg_fill = Color32::from_rgb(0x3f, 0x3f, 0x3f);
        w.open.weak_bg_fill = Color32::from_rgb(0x2e, 0x2e, 0x2e);
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x99, 0x99, 0x99));
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
    fn hue_colours_follow_the_wheel() {
        assert_eq!(hue_color(0.0, 1.0, 1.0), Color32::from_rgb(255, 0, 0));
        assert_eq!(hue_color(240.0, 0.0, 1.0), Color32::from_rgb(255, 255, 255));
        assert_eq!(linear_color([1.0, 0.0, 2.0]), Color32::from_rgb(255, 0, 255));
    }

    #[test]
    fn the_canvas_is_a_neutral_grey() {
        for t in [Tokens::dark(), Tokens::light()] {
            assert!(t.canvas.r() == t.canvas.g() && t.canvas.g() == t.canvas.b());
        }
    }

    #[test]
    fn the_dark_greys_are_neutral_like_a_photo_viewer() {
        let t = Tokens::dark();
        for c in [t.text_strong, t.text_weak, t.stroke_strong, t.stroke_weak, t.fill, t.bg, t.bar, t.canvas, t.graph] {
            assert!(c.r() == c.g() && c.g() == c.b(), "{c:?} is tinted");
        }
    }
}
