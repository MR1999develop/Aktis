//! The scope above the tool panels: histogram, waveform or parade, chosen
//! with three small tabs. The waveform and the parade are drawn from a
//! texture made once per render ([`Scopes::update`]).

use editor::Histogram;
use editor::histogram::{WAVEFORM_COLUMNS, WAVEFORM_LEVELS, Waveform};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, TextureHandle, TextureOptions, pos2,
    vec2,
};
use serde::{Deserialize, Serialize};

use crate::theme::{CHANNEL, FONT, HISTOGRAM, HISTOGRAM_HEIGHT, Tokens};

/// Which scope is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeKind {
    /// Levels per channel.
    #[default]
    Histogram,
    /// Luminance per column of the image.
    Waveform,
    /// Red, green and blue per column, side by side.
    Parade,
}

impl ScopeKind {
    /// Every scope, in tab order.
    pub const ALL: [Self; 3] = [Self::Histogram, Self::Waveform, Self::Parade];

    /// Tab label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Histogram => "Histogram",
            Self::Waveform => "Waveform",
            Self::Parade => "Parade",
        }
    }
}

/// The texture of the waveform or parade, kept between frames.
#[derive(Default)]
pub struct Scopes {
    texture: Option<TextureHandle>,
    /// What the texture shows, to remake it only when needed.
    made_for: Option<ScopeKind>,
    /// The histogram changed since the texture was made.
    dirty: bool,
}

impl std::fmt::Debug for Scopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scopes").field("made_for", &self.made_for).finish_non_exhaustive()
    }
}

/// One plot's cells coloured: `color` at full strength where the counts are high.
fn colored(cells: &[f32], color: Color32) -> Vec<Color32> {
    cells
        .iter()
        .map(|&v| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–255
            let a = (v * 255.0).round().clamp(0.0, 255.0) as u8;
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
        })
        .collect()
}

/// The waveform (luminance) or the parade (red, green and blue side by side) as an image.
fn image(w: &Waveform, kind: ScopeKind) -> Option<egui::ColorImage> {
    if w.is_empty() {
        return None;
    }
    match kind {
        ScopeKind::Histogram => None,
        ScopeKind::Waveform => Some(egui::ColorImage::new(
            [WAVEFORM_COLUMNS, WAVEFORM_LEVELS],
            colored(&w.intensities(&w.luma), CHANNEL[0]),
        )),
        ScopeKind::Parade => {
            let parts = [(&w.red, CHANNEL[2]), (&w.green, CHANNEL[3]), (&w.blue, CHANNEL[4])]
                .map(|(counts, color)| colored(&w.intensities(counts), color));
            let mut pixels = Vec::with_capacity(3 * WAVEFORM_COLUMNS * WAVEFORM_LEVELS);
            for row in 0..WAVEFORM_LEVELS {
                for part in &parts {
                    pixels.extend_from_slice(&part[row * WAVEFORM_COLUMNS..(row + 1) * WAVEFORM_COLUMNS]);
                }
            }
            Some(egui::ColorImage::new([3 * WAVEFORM_COLUMNS, WAVEFORM_LEVELS], pixels))
        }
    }
}

impl Scopes {
    /// A new histogram arrived.
    pub fn changed(&mut self) {
        self.dirty = true;
    }

    /// Makes the texture for `kind` from `hist` when it is out of date.
    pub fn update(&mut self, ctx: &egui::Context, kind: ScopeKind, hist: Option<&Histogram>) {
        if !self.dirty && self.made_for == Some(kind) {
            return;
        }
        self.dirty = false;
        self.made_for = Some(kind);
        let Some(img) = hist.and_then(|h| image(&h.waveform, kind)) else {
            self.texture = None;
            return;
        };
        match &mut self.texture {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("aktis-scope", img, TextureOptions::LINEAR)),
        }
    }

    /// The tabs and the chosen scope, `height` points high. Returns the
    /// scope chosen with the tabs.
    pub fn show(&self, ui: &mut egui::Ui, kind: ScopeKind, hist: Option<&Histogram>, height: f32) -> ScopeKind {
        let mut chosen = kind;
        ui.horizontal(|ui| {
            for k in ScopeKind::ALL {
                if ui.selectable_label(k == kind, k.label()).clicked() {
                    chosen = k;
                }
            }
        });
        if kind == ScopeKind::Histogram {
            histogram(ui, hist, height);
            return chosen;
        }
        self.draw_scope(ui, kind, height);
        chosen
    }

    /// The bottom edge of the scope, dragged to change its height: the new
    /// height (within [`HISTOGRAM_HEIGHT`]) while dragged; a double click
    /// returns to the default.
    pub fn resize_handle(ui: &mut egui::Ui, height: f32) -> Option<f32> {
        let t = Tokens::of(ui.ctx().theme());
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::click_and_drag());
        let response = response.on_hover_text("Drag to resize; double-click for the default size");
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let colour = if response.hovered() || response.dragged() { t.stroke_strong } else { t.stroke_weak };
        let c = rect.center();
        ui.painter().line_segment([pos2(c.x - 16.0, c.y), pos2(c.x + 16.0, c.y)], Stroke::new(2.0, colour));
        if response.double_clicked() {
            return Some(HISTOGRAM_HEIGHT[0]);
        }
        response.dragged().then(|| (height + response.drag_delta().y).clamp(HISTOGRAM_HEIGHT[1], HISTOGRAM_HEIGHT[2]))
    }

    /// Waveform or parade, `height` points high.
    fn draw_scope(&self, ui: &mut egui::Ui, kind: ScopeKind, height: f32) {
        let t = Tokens::of(ui.ctx().theme());
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(4), t.canvas);
        let inner = rect.shrink(2.0);
        // Guides at 0, 25, 50, 75 and 100 %.
        for i in 0..=4 {
            #[allow(clippy::cast_precision_loss)] // five guides
            let y = inner.bottom() - i as f32 / 4.0 * inner.height();
            painter.line_segment([pos2(inner.left(), y), pos2(inner.right(), y)], Stroke::new(1.0, t.grid));
        }
        match &self.texture {
            Some(texture) => {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(texture.id(), inner, uv, Color32::WHITE);
                if kind == ScopeKind::Parade {
                    for i in 1..3 {
                        #[allow(clippy::cast_precision_loss)] // two separators
                        let x = inner.left() + i as f32 / 3.0 * inner.width();
                        painter.line_segment(
                            [pos2(x, inner.top()), pos2(x, inner.bottom())],
                            Stroke::new(1.0, t.stroke_weak),
                        );
                    }
                }
            }
            None => {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "No image",
                    FontId::proportional(FONT[0]),
                    t.text_weak,
                );
            }
        }
    }
}

/// The histogram of the displayed image, with clipping warnings.
pub fn histogram(ui: &mut egui::Ui, hist: Option<&Histogram>, height: f32) {
    let t = Tokens::of(ui.ctx().theme());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), t.canvas);
    let Some(h) = hist.filter(|h| h.total() > 0) else {
        painter.text(rect.center(), Align2::CENTER_CENTER, "No image", FontId::proportional(FONT[0]), t.text_weak);
        return;
    };
    #[allow(clippy::cast_precision_loss)] // counts, for drawing
    let peak = h.peak().max(1) as f32;
    let inner = rect.shrink(2.0);
    let bin_w = inner.width() / 256.0;
    let channels = [(&h.red, HISTOGRAM[0]), (&h.green, HISTOGRAM[1]), (&h.blue, HISTOGRAM[2])];
    for (counts, color) in channels {
        for (i, &c) in counts.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // counts, for drawing
            let level = (c as f32 / peak).min(1.0);
            if level <= 0.0 {
                continue;
            }
            #[allow(clippy::cast_precision_loss)] // 256 bins
            let x = inner.left() + i as f32 * bin_w;
            let bar = Rect::from_min_max(
                pos2(x, inner.bottom() - level * inner.height()),
                pos2(x + bin_w.max(1.0), inner.bottom()),
            );
            painter.rect_filled(bar, CornerRadius::ZERO, color);
        }
    }
    // Luminance as a line on top.
    #[allow(clippy::cast_precision_loss)] // 256 bins
    let points: Vec<Pos2> = h
        .luma
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let level = (c as f32 / peak).min(1.0);
            pos2(inner.left() + (i as f32 + 0.5) * bin_w, inner.bottom() - level * inner.height())
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.0, t.text_weak)));

    let (low, high) = h.clipped();
    let warn = |share: f32| share >= 0.001;
    let font = FontId::proportional(FONT[0]);
    if warn(low) {
        painter.text(
            rect.left_top() + vec2(4.0, 2.0),
            Align2::LEFT_TOP,
            format!("{:.1} % black", low * 100.0),
            font.clone(),
            t.warning,
        );
    }
    if warn(high) {
        painter.text(
            rect.right_top() + vec2(-4.0, 2.0),
            Align2::RIGHT_TOP,
            format!("{:.1} % white", high * 100.0),
            font,
            t.warning,
        );
    }
}
