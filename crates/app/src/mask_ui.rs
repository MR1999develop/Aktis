//! The Masks (B) and AI Select (A) tools: they build the mask of the layer
//! selected in the Layers panel, so its tools act only where the mask is.
//! On the Background, the first stroke or click adds a new layer with it,
//! as in Capture One.
//!
//! - **Masks** (B), with the brush of Capture One: B paints, E erases, Alt
//!   held swaps them; Shift draws a straight line (Shift-click: from the
//!   last point). `[`/`]` change the size, Shift+`[`/`]` the hardness;
//!   Alt + right-drag changes the size (across) and hardness (up/down),
//!   Shift + right-drag the opacity; a right-click opens the brush settings
//!   (size, hardness, opacity, auto mask).
//!   L draws a **linear gradient** (drag from where the mask is full to
//!   where it ends) and T a **radial gradient** (drag from the centre
//!   outwards), as in Capture One; with Alt they take away from the mask.
//!   The last gradient shows handles to move it; the radial one has its
//!   own feather in the options bar.
//! - **AI Select**: click on an object to select it; Alt-click marks a
//!   part to leave out; Shift-click starts another object. "Select
//!   subject" finds the main subject. The models run in the background.
//!
//! Both show the mask as a red overlay (M toggles it, as in Capture One),
//! and share feather, invert and clear in the options bar.
//!
//! - **Style brushes** (as in Capture One): choosing one in the options bar
//!   (or Image \u{203a} Style brushes) arms the brush with its settings; the
//!   first stroke adds a layer with the brush's adjustments and the stroke
//!   as its mask, named after the brush; further strokes extend it.
//!
//! The editing logic is in [`editor::masking`]; this module draws and turns
//! pointer input into [`MaskEdit`]s.

use core_model::{LayerId, LinearGradient, MaskDef, MaskSource, NormPoint, RadialGradient, Style};
use editor::masking::Brush;
use eframe::egui::{self, Align2, CornerRadius, Id, Order, Pos2, Rect, Response, RichText, Stroke, TextureHandle};

use crate::theme::{MASK_OVERLAY, SPACE, Tokens};
use crate::toolbar::ViewerTool;

/// Hardness change of Shift+`[` / Shift+`]`.
const HARDNESS_STEP: f32 = 0.1;
/// Weakest brush opacity.
const MIN_OPACITY: f32 = 0.05;

/// A change of a mask asked for by the tools.
#[derive(Debug, Clone, PartialEq)]
pub enum MaskEdit {
    /// A painted stroke (developed-image points).
    Stroke {
        /// The layer whose mask it is.
        layer: LayerId,
        /// The path.
        points: Vec<NormPoint>,
        /// Remove instead of add.
        erase: bool,
    },
    /// An AI Select click.
    Click {
        /// The layer.
        layer: LayerId,
        /// Where (developed image).
        at: NormPoint,
        /// On the object (`false`: a part to leave out).
        include: bool,
        /// Starts another object.
        new_object: bool,
    },
    /// "Select subject".
    Subject {
        /// The layer.
        layer: LayerId,
    },
    /// Fill Mask: the whole image, to erase from.
    Fill {
        /// The layer.
        layer: LayerId,
    },
    /// Feather, invert or clear: the whole new definition.
    Def {
        /// The layer.
        layer: LayerId,
        /// The definition.
        def: MaskDef,
    },
    /// The first stroke of an armed style brush: a new layer with its
    /// adjustments and the stroke as its mask.
    StyleStroke {
        /// The brush's name.
        name: String,
        /// Its adjustments.
        style: Style,
        /// The path (developed-image points).
        points: Vec<NormPoint>,
    },
    /// A style brush was chosen in the options bar (an index into the
    /// brushes it was given).
    ChooseStyleBrush(usize),
}

/// What the Masks tool draws.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Shape {
    /// Brush strokes (or the eraser).
    #[default]
    Brush,
    /// A linear gradient (L).
    Linear,
    /// A radial gradient (T).
    Radial,
}

/// Which part of a gradient a drag moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
    /// Linear: where it starts. Radial: the centre.
    Start,
    /// Linear: where it ends.
    End,
    /// Radial: the horizontal radius.
    RadiusX,
    /// Radial: the vertical radius.
    RadiusY,
}

/// A gradient being drawn or moved, shown live and written when the drag ends.
#[derive(Debug, Clone, PartialEq)]
struct GradientDrag {
    /// The component moved; `None` for a new gradient.
    index: Option<usize>,
    /// The handle dragged.
    handle: Handle,
    /// The gradient as it is now.
    source: MaskSource,
    /// It takes away from the mask (Alt when it was started).
    subtract: bool,
}

/// Pointer distance, in points, within which a handle is grabbed.
const HANDLE_REACH: f32 = 10.0;

/// A style brush waiting for its first stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmedBrush {
    /// Its name.
    pub name: String,
    /// Its adjustments.
    pub style: Style,
}

/// State of the mask tools between frames.
#[derive(Default)]
pub struct MaskUi {
    /// The brush.
    pub brush: Brush,
    /// The Eraser is chosen.
    erase: bool,
    /// The mask overlay is shown.
    pub show: bool,
    /// The stroke being painted: developed points and screen points.
    stroke: Vec<NormPoint>,
    stroke_screen: Vec<Pos2>,
    /// The overlay texture and what it was made for (document generation, layer, cropped).
    overlay: Option<((u64, LayerId, bool), TextureHandle)>,
    /// The style brush the next stroke adds.
    armed: Option<ArmedBrush>,
    /// Where the brush settings popup is open (right-click), in points.
    settings_at: Option<Pos2>,
    /// A right-drag changing the brush: the brush and pointer at its start.
    adjusting: Option<(Brush, Pos2)>,
    /// The end of the last stroke, for Shift-click straight lines.
    last_point: Option<(NormPoint, Pos2)>,
    /// What the Masks tool draws.
    shape: Shape,
    /// A gradient being drawn or moved.
    gradient: Option<GradientDrag>,
}

impl std::fmt::Debug for MaskUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaskUi").field("brush", &self.brush).field("show", &self.show).finish_non_exhaustive()
    }
}

impl MaskUi {
    /// Arms style brush `name`: its settings become the brush's, and the
    /// next stroke adds `style` behind a new mask.
    pub fn arm(&mut self, name: String, style: Style, settings: &editor::presets::BrushSettings) {
        self.brush.radius = settings.radius;
        self.brush.hardness = settings.hardness;
        self.brush.opacity = settings.opacity;
        self.brush.edge_aware = settings.edge_aware;
        self.erase = false;
        self.armed = Some(ArmedBrush { name, style });
    }

    /// Forgets the armed style brush.
    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// Keys of the mask tools, as in Capture One: `[`/`]` brush size,
    /// Shift+`[`/`]` hardness, B brush, E eraser, L linear gradient, T radial
    /// gradient, M mask overlay. Not while typing.
    pub fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        ctx.input_mut(|i| {
            let shift = egui::Modifiers::SHIFT;
            if i.consume_key(shift, egui::Key::OpenBracket) {
                self.brush.hardness = (self.brush.hardness - HARDNESS_STEP).max(0.0);
            }
            if i.consume_key(shift, egui::Key::CloseBracket) {
                self.brush.hardness = (self.brush.hardness + HARDNESS_STEP).min(1.0);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket) {
                self.brush.radius = (self.brush.radius / 1.25).max(0.001);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket) {
                self.brush.radius = (self.brush.radius * 1.25).min(core_model::MAX_BRUSH_RADIUS);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::E) {
                self.erase = true;
                self.shape = Shape::Brush;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::L) {
                self.shape = Shape::Linear;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::T) {
                self.shape = Shape::Radial;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::M) {
                self.show = !self.show;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                self.settings_at = None;
            }
        });
    }

    /// The Masks tool was chosen again (B): back to the brush from the eraser.
    pub fn choose_brush(&mut self) {
        self.erase = false;
        self.shape = Shape::Brush;
    }

    /// A right-drag over the photo: with Alt, across changes the size and
    /// up/down the hardness; with Shift, across changes the opacity; a plain
    /// right-click opens the settings popup.
    fn right_button(&mut self, ui: &egui::Ui, response: &Response) {
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        if response.drag_started_by(egui::PointerButton::Secondary)
            && let Some(p) = response.interact_pointer_pos()
        {
            self.adjusting = Some((self.brush, p));
        }
        if let Some((start, from)) = self.adjusting
            && let Some(p) = response.interact_pointer_pos()
        {
            let (dx, dy) = (p.x - from.x, from.y - p.y);
            if alt {
                self.brush.radius = (start.radius * (dx / 200.0).exp2()).clamp(0.001, core_model::MAX_BRUSH_RADIUS);
                self.brush.hardness = (start.hardness + dy / 200.0).clamp(0.0, 1.0);
            } else if shift {
                self.brush.opacity = (start.opacity + dx / 200.0).clamp(MIN_OPACITY, 1.0);
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Secondary) {
            self.adjusting = None;
        }
        if response.secondary_clicked() && !alt && !shift {
            self.settings_at = response.interact_pointer_pos();
        }
    }

    /// The brush settings popup (right-click), as Capture One's.
    fn settings_popup(&mut self, ctx: &egui::Context) {
        let Some(at) = self.settings_at else { return };
        let t = Tokens::of(ctx.theme());
        let shown = egui::Area::new(Id::unique("aktis-brush-settings"))
            .order(Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(t.bg_raised).show(ui, |ui| {
                    ui.set_width(240.0);
                    ui.label(RichText::new("Brush").strong());
                    let mut size = self.brush.radius * 100.0;
                    if ui
                        .add(egui::Slider::new(&mut size, 0.1..=50.0).logarithmic(true).suffix(" %").text("Size"))
                        .on_hover_text("Radius, as a share of the photo's long side ([ and ])")
                        .changed()
                    {
                        self.brush.radius = size / 100.0;
                    }
                    let mut hardness = self.brush.hardness * 100.0;
                    if ui
                        .add(egui::Slider::new(&mut hardness, 0.0..=100.0).fixed_decimals(0).text("Hardness"))
                        .on_hover_text("100: a hard edge (Shift+[ and Shift+])")
                        .changed()
                    {
                        self.brush.hardness = hardness / 100.0;
                    }
                    let mut opacity = self.brush.opacity * 100.0;
                    if ui
                        .add(
                            egui::Slider::new(&mut opacity, MIN_OPACITY * 100.0..=100.0)
                                .fixed_decimals(0)
                                .suffix(" %")
                                .text("Opacity"),
                        )
                        .on_hover_text("How strong a stroke is (Shift + right-drag)")
                        .changed()
                    {
                        self.brush.opacity = opacity / 100.0;
                    }
                    ui.checkbox(&mut self.brush.edge_aware, "Auto mask")
                        .on_hover_text("The stroke stops at edges in the photo");
                })
            })
            .response;
        if shown.clicked_elsewhere() {
            self.settings_at = None;
        }
    }

    /// Pointer input over the photo shown in `screen`. `long_side` is the
    /// developed image's long side on screen, in points (for the brush
    /// circle); `to_developed` maps a screen point into the developed
    /// image. Returns whether the drag is the tool's (no panning).
    #[allow(clippy::too_many_arguments)]
    pub fn overlay(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        tool: ViewerTool,
        layer: Option<LayerId>,
        def: Option<&MaskDef>,
        long_side: f32,
        to_developed: &dyn Fn(Pos2) -> Option<NormPoint>,
        to_screen: &dyn Fn(NormPoint) -> Option<Pos2>,
        edits: &mut Vec<MaskEdit>,
    ) -> bool {
        let t = Tokens::of(ui.ctx().theme());
        let painting_style = tool == ViewerTool::Masks && self.armed.is_some();
        if layer.is_none() && !painting_style {
            return false;
        }
        let painter = ui.painter_at(response.rect);
        let alt = ui.input(|i| i.modifiers.alt) && !painting_style;
        match tool {
            ViewerTool::Masks if !painting_style && self.shape != Shape::Brush => {
                let Some(layer) = layer else { return false };
                let map = Mapping { long_side, to_developed, to_screen };
                self.gradient_input(ui, response, layer, def, &map, alt, edits);
                true
            }
            ViewerTool::Masks => {
                self.right_button(ui, response);
                self.settings_popup(ui.ctx());
                let shift = ui.input(|i| i.modifiers.shift);
                let r = self.brush.radius * long_side;
                if let Some(p) = response.hover_pos() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    let color = if (self.erase && !painting_style) || alt { t.error } else { t.text_strong };
                    painter.circle_stroke(p, r, Stroke::new(1.5, color));
                    painter.circle_stroke(p, r * self.brush.hardness, Stroke::new(1.0, color.gamma_multiply(0.5)));
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    self.stroke.clear();
                    self.stroke_screen.clear();
                }
                if response.dragged_by(egui::PointerButton::Primary)
                    && let Some(p) = response.interact_pointer_pos()
                    && let Some(d) = to_developed(p)
                {
                    if shift && !self.stroke.is_empty() {
                        // Shift: a straight line from where the stroke started.
                        self.stroke.truncate(1);
                        self.stroke_screen.truncate(1);
                        self.stroke.push(d);
                        self.stroke_screen.push(p);
                    } else if self.stroke_screen.last().is_none_or(|q| q.distance(p) >= (r * 0.25).max(1.0)) {
                        self.stroke.push(d);
                        self.stroke_screen.push(p);
                    }
                }
                if self.stroke_screen.len() > 1 {
                    painter.add(egui::Shape::line(
                        self.stroke_screen.clone(),
                        Stroke::new(2.0 * r, MASK_OVERLAY.gamma_multiply(0.8)),
                    ));
                }
                if (response.drag_stopped() || response.clicked())
                    && let Some(p) = response.interact_pointer_pos()
                {
                    if self.stroke.is_empty()
                        && let Some(d) = to_developed(p)
                    {
                        // Shift-click: a straight line from the end of the last stroke.
                        if shift && let Some((last, _)) = self.last_point {
                            self.stroke.push(last);
                        }
                        self.stroke.push(d);
                    }
                    if let Some(&end) = self.stroke.last() {
                        self.last_point = Some((end, p));
                    }
                    let points = std::mem::take(&mut self.stroke);
                    if !points.is_empty() {
                        if let Some(armed) = self.armed.take() {
                            edits.push(MaskEdit::StyleStroke { name: armed.name, style: armed.style, points });
                        } else if let Some(layer) = layer {
                            edits.push(MaskEdit::Stroke { layer, points, erase: self.erase || alt });
                        }
                    }
                    self.stroke_screen.clear();
                }
                true
            }
            ViewerTool::AiSelect => {
                if response.hovered() {
                    ui.ctx().set_cursor_icon(if alt {
                        egui::CursorIcon::NotAllowed
                    } else {
                        egui::CursorIcon::Crosshair
                    });
                }
                if response.clicked()
                    && let Some(layer) = layer
                    && let Some(at) = response.interact_pointer_pos().and_then(to_developed)
                {
                    let shift = ui.input(|i| i.modifiers.shift);
                    edits.push(MaskEdit::Click { layer, at, include: !alt, new_object: shift });
                }
                false
            }
            _ => false,
        }
    }

    /// Linear and radial gradients: a drag on a handle of the last gradient
    /// moves it, a drag elsewhere draws a new one; the mask is written when
    /// the drag ends (one undo step).
    #[allow(clippy::too_many_arguments)]
    fn gradient_input(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        layer: LayerId,
        def: Option<&MaskDef>,
        map: &Mapping<'_>,
        alt: bool,
        edits: &mut Vec<MaskEdit>,
    ) {
        let t = Tokens::of(ui.ctx().theme());
        let last = def.and_then(editor::masking::last_gradient);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
            && let Some(d) = (map.to_developed)(p)
        {
            // A handle of the last gradient of the kind being drawn, else a new gradient.
            let grabbed = last.as_ref().and_then(|(i, src)| {
                let kind_matches = matches!(
                    (src, self.shape),
                    (MaskSource::Linear(_), Shape::Linear) | (MaskSource::Radial(_), Shape::Radial)
                );
                if !kind_matches {
                    return None;
                }
                handles(src, map)
                    .into_iter()
                    .filter(|(_, at)| at.distance(p) <= HANDLE_REACH)
                    .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
                    .map(|(h, _)| GradientDrag { index: Some(*i), handle: h, source: src.clone(), subtract: false })
            });
            self.gradient = grabbed.or_else(|| {
                let source = match self.shape {
                    Shape::Radial => MaskSource::Radial(RadialGradient {
                        centre: d,
                        radius: [core_model::MIN_RADIUS; 2],
                        feather: editor::masking::RADIAL_FEATHER,
                    }),
                    _ => MaskSource::Linear(LinearGradient { start: d, end: d }),
                };
                let handle = if self.shape == Shape::Radial { Handle::RadiusX } else { Handle::End };
                Some(GradientDrag { index: None, handle, source, subtract: alt })
            });
        }
        if let Some(drag) = &mut self.gradient
            && response.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            move_handle(drag, p, map, ui.input(|i| i.modifiers.shift));
        }
        // What to draw: the gradient being dragged, else the last one.
        let shown = self.gradient.as_ref().map(|g| g.source.clone()).or_else(|| last.map(|(_, s)| s));
        if let Some(src) = &shown {
            draw_gradient(&ui.painter_at(response.rect), src, map, response.rect, &t);
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some(drag) = self.gradient.take()
        {
            let next = match (drag.index, &drag.source, def) {
                (Some(i), _, Some(d)) => Some(editor::masking::with_source(d, i, drag.source.clone())),
                (None, MaskSource::Linear(g), _) if g.start != g.end => {
                    Some(editor::masking::add_linear(def, g.start, g.end, drag.subtract))
                }
                (None, MaskSource::Radial(g), _) if g.radius[0] > core_model::MIN_RADIUS => {
                    let r = editor::masking::add_radial(def, g.centre, g.radius, drag.subtract);
                    Some(r)
                }
                _ => None,
            };
            if let Some(def) = next {
                edits.push(MaskEdit::Def { layer, def });
            }
        }
    }

    /// Draws the mask overlay over `screen`, making its texture when the
    /// mask changed (`key`: document generation, layer, cropped); `values`
    /// computes the mask as shown (width, height, values 0–1).
    pub fn draw_overlay(
        &mut self,
        ui: &egui::Ui,
        screen: Rect,
        key: (u64, LayerId, bool),
        values: impl FnOnce() -> Option<(u32, u32, Vec<f32>)>,
    ) {
        if !self.show {
            return;
        }
        if self.overlay.as_ref().is_none_or(|(k, _)| *k != key) {
            self.overlay = values().map(|(w, h, v)| {
                let pixels = v
                    .iter()
                    .map(|&m| {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–255
                        let a = (m.clamp(0.0, 1.0) * 128.0).round() as u8;
                        egui::Color32::from_rgba_unmultiplied(MASK_OVERLAY.r(), MASK_OVERLAY.g(), MASK_OVERLAY.b(), a)
                    })
                    .collect();
                let img = egui::ColorImage::new([w as usize, h as usize], pixels);
                (key, ui.ctx().load_texture("aktis-mask-overlay", img, egui::TextureOptions::LINEAR))
            });
        }
        if let Some((_, tex)) = &self.overlay {
            let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter_at(screen).image(tex.id(), screen, uv, egui::Color32::WHITE);
        }
    }

    /// The options bar at the top of the viewer.
    #[allow(clippy::too_many_arguments)]
    pub fn options(
        &mut self,
        ctx: &egui::Context,
        viewer: Rect,
        tool: ViewerTool,
        layer: Option<(LayerId, String)>,
        def: Option<&MaskDef>,
        busy: bool,
        brushes: &[String],
        edits: &mut Vec<MaskEdit>,
    ) {
        let t = Tokens::of(ctx.theme());
        egui::Area::new(Id::unique("aktis-mask-options"))
            .order(Order::Foreground)
            .fixed_pos(egui::pos2(viewer.center().x, viewer.top() + SPACE[1]))
            .pivot(Align2::CENTER_TOP)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(t.bg_overlay)
                    .stroke(Stroke::new(1.0, t.stroke_weak))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if tool == ViewerTool::Masks {
                                self.style_brush_picker(ui, &t, brushes, edits);
                                ui.separator();
                            }
                            if tool == ViewerTool::Masks
                                && let Some(armed) = &self.armed
                            {
                                ui.label(RichText::new(format!("Paint to add \u{201c}{}\u{201d}", armed.name)).strong());
                                ui.label("Size");
                                let mut size = self.brush.radius * 100.0;
                                if ui
                                    .add(egui::Slider::new(&mut size, 0.1..=25.0).logarithmic(true).suffix(" %").fixed_decimals(1))
                                    .changed()
                                {
                                    self.brush.radius = size / 100.0;
                                }
                                return;
                            }
                            let Some((layer, name)) = layer else {
                                ui.label(RichText::new("Select a layer: its mask is what you draw").color(t.text_weak));
                                return;
                            };
                            ui.label(RichText::new(format!("Mask of {name}")).strong());
                            ui.separator();
                            match tool {
                                ViewerTool::Masks => {
                                    let mut choice = match (self.shape, self.erase) {
                                        (Shape::Brush, false) => 0,
                                        (Shape::Brush, true) => 1,
                                        (Shape::Linear, _) => 2,
                                        (Shape::Radial, _) => 3,
                                    };
                                    let before = choice;
                                    ui.selectable_value(&mut choice, 0, "Brush").on_hover_text("B");
                                    ui.selectable_value(&mut choice, 1, "Eraser").on_hover_text("E, or hold Alt");
                                    ui.selectable_value(&mut choice, 2, "Linear")
                                        .on_hover_text("L: drag from where the mask is full to where it ends (Alt: take away)");
                                    ui.selectable_value(&mut choice, 3, "Radial")
                                        .on_hover_text("T: drag from the centre outwards; Shift for a circle (Alt: take away)");
                                    if choice != before {
                                        (self.shape, self.erase) = match choice {
                                            1 => (Shape::Brush, true),
                                            2 => (Shape::Linear, false),
                                            3 => (Shape::Radial, false),
                                            _ => (Shape::Brush, false),
                                        };
                                    }
                                    if self.shape != Shape::Brush {
                                        ui.separator();
                                        self.gradient_options(ui, layer, def, edits);
                                    } else {
                                    ui.label("Size");
                                    let mut size = self.brush.radius * 100.0;
                                    if ui
                                        .add(egui::Slider::new(&mut size, 0.1..=25.0).logarithmic(true).suffix(" %").fixed_decimals(1))
                                        .on_hover_text("Radius, as a share of the photo's long side ([ and ])")
                                        .changed()
                                    {
                                        self.brush.radius = size / 100.0;
                                    }
                                    ui.label("Hardness");
                                    ui.add(egui::Slider::new(&mut self.brush.hardness, 0.0..=1.0).fixed_decimals(2));
                                    ui.label("Opacity");
                                    ui.add(egui::Slider::new(&mut self.brush.opacity, MIN_OPACITY..=1.0).fixed_decimals(2));
                                    ui.checkbox(&mut self.brush.edge_aware, "Auto mask")
                                        .on_hover_text("The stroke stops at edges in the photo");
                                    }
                                }
                                _ => {
                                    ui.label(RichText::new("Click an object \u{b7} Alt-click: leave out \u{b7} Shift-click: another object").color(t.text_weak));
                                    if ui.button("Select subject").clicked() {
                                        edits.push(MaskEdit::Subject { layer });
                                    }
                                    if busy {
                                        ui.spinner();
                                    }
                                }
                            }
                            ui.separator();
                            if ui.button("Fill").on_hover_text("Select the whole image, then erase (E) what to leave out").clicked() {
                                edits.push(MaskEdit::Fill { layer });
                            }
                            ui.checkbox(&mut self.show, "Show").on_hover_text("Show the mask in red (M)");
                            if let Some(def) = def {
                                let mut next = def.clone();
                                let mut feather = next.feather * 100.0;
                                ui.label("Feather");
                                let changed_feather = ui
                                    .add(egui::Slider::new(&mut feather, 0.0..=core_model::MAX_FEATHER * 100.0).suffix(" %").fixed_decimals(1))
                                    .drag_stopped();
                                let inverted = ui.checkbox(&mut next.invert, "Invert").changed();
                                if changed_feather || inverted {
                                    next.feather = feather / 100.0;
                                    edits.push(MaskEdit::Def { layer, def: next.sanitized() });
                                }
                                if ui.button("Clear").on_hover_text("Remove everything from this mask").clicked() {
                                    edits.push(MaskEdit::Def { layer, def: MaskDef { components: Vec::new(), ..def.clone() } });
                                }
                            }
                        });
                    });
            });
    }
}

impl MaskUi {
    /// The options of the gradients: the radial one's feather (written when
    /// let go), and a hint.
    fn gradient_options(
        &mut self,
        ui: &mut egui::Ui,
        layer: LayerId,
        def: Option<&MaskDef>,
        edits: &mut Vec<MaskEdit>,
    ) {
        let last = def.and_then(editor::masking::last_gradient);
        match (self.shape, last) {
            (Shape::Radial, Some((i, MaskSource::Radial(mut g)))) => {
                ui.label("Feather");
                let mut v = g.feather * 100.0;
                let r = ui
                    .add(egui::Slider::new(&mut v, 0.0..=100.0).suffix(" %").fixed_decimals(0))
                    .on_hover_text("How much of the radius fades out");
                if (r.drag_stopped() || (r.changed() && !r.dragged()))
                    && let Some(d) = def
                {
                    g.feather = v / 100.0;
                    edits.push(MaskEdit::Def { layer, def: editor::masking::with_source(d, i, MaskSource::Radial(g)) });
                }
            }
            _ => {
                ui.label(
                    RichText::new("Drag on the photo; drag the handles to move the last gradient")
                        .color(Tokens::of(ui.ctx().theme()).text_weak),
                );
            }
        }
    }

    /// The Style brush menu of the options bar: the brushes, and Cancel
    /// while one is armed.
    fn style_brush_picker(&mut self, ui: &mut egui::Ui, t: &Tokens, brushes: &[String], edits: &mut Vec<MaskEdit>) {
        let current = self.armed.as_ref().map_or("Style brush", |a| a.name.as_str());
        egui::ComboBox::from_id_salt("aktis-style-brush")
            .selected_text(current)
            .width(150.0)
            .show_ui(ui, |ui| {
                if brushes.is_empty() {
                    ui.label(RichText::new("No style brushes").color(t.text_weak));
                }
                for (i, name) in brushes.iter().enumerate() {
                    if ui.selectable_label(false, name.as_str()).clicked() {
                        edits.push(MaskEdit::ChooseStyleBrush(i));
                    }
                }
            })
            .response
            .on_hover_text("Paint adjustments: the first stroke adds a new layer, as Capture One's style brushes");
        if self.armed.is_some()
            && ui.button("Cancel").on_hover_text("Paint the selected layer's mask instead").clicked()
        {
            self.armed = None;
        }
    }
}

/// Size of the developed image's long side on screen, for a photo shown in
/// `screen` whose shown part is `crop` (`[w, h]` of the frame) of a frame
/// enlarged `scale` times by the geometry.
#[must_use]
pub fn long_side_on_screen(screen: Rect, crop: [f32; 2], scale: f32) -> f32 {
    let w = screen.width() / crop[0].max(1e-3);
    let h = screen.height() / crop[1].max(1e-3);
    w.max(h) * scale
}

/// Between the screen and the developed image, for the gradients.
struct Mapping<'a> {
    /// The developed image's long side on screen, in points.
    long_side: f32,
    to_developed: &'a dyn Fn(Pos2) -> Option<NormPoint>,
    to_screen: &'a dyn Fn(NormPoint) -> Option<Pos2>,
}

/// The handles of a gradient on screen.
fn handles(src: &MaskSource, map: &Mapping<'_>) -> Vec<(Handle, Pos2)> {
    match src {
        MaskSource::Linear(g) => [(Handle::Start, g.start), (Handle::End, g.end)]
            .into_iter()
            .filter_map(|(h, p)| (map.to_screen)(p).map(|s| (h, s)))
            .collect(),
        MaskSource::Radial(g) => {
            let Some(c) = (map.to_screen)(g.centre) else { return Vec::new() };
            let (rx, ry) = (g.radius[0] * map.long_side, g.radius[1] * map.long_side);
            vec![
                (Handle::Start, c),
                (Handle::RadiusX, c + egui::vec2(rx, 0.0)),
                (Handle::RadiusY, c + egui::vec2(0.0, -ry)),
            ]
        }
        MaskSource::Ai(_) | MaskSource::Brush(_) => Vec::new(),
    }
}

/// Moves `drag`'s handle to screen point `p`; `circle` keeps a radial
/// gradient round.
fn move_handle(drag: &mut GradientDrag, p: Pos2, map: &Mapping<'_>, circle: bool) {
    match (&mut drag.source, drag.handle) {
        (MaskSource::Linear(g), Handle::Start) => {
            if let Some(d) = (map.to_developed)(p) {
                g.start = d;
            }
        }
        (MaskSource::Linear(g), _) => {
            if let Some(d) = (map.to_developed)(p) {
                g.end = d;
            }
        }
        (MaskSource::Radial(g), Handle::Start) => {
            if let Some(d) = (map.to_developed)(p) {
                g.centre = d;
            }
        }
        (MaskSource::Radial(g), handle) => {
            let Some(c) = (map.to_screen)(g.centre) else { return };
            let long = map.long_side.max(1.0);
            let clamp = |r: f32| r.clamp(core_model::MIN_RADIUS, core_model::MAX_RADIUS);
            match handle {
                Handle::RadiusY => g.radius[1] = clamp((c.y - p.y).abs() / long),
                _ if drag.index.is_none() || circle => {
                    // A new radial gradient is a circle through the pointer.
                    let r = clamp(c.distance(p) / long);
                    g.radius = [r, r];
                }
                _ => g.radius[0] = clamp((p.x - c.x).abs() / long),
            }
        }
        (MaskSource::Ai(_) | MaskSource::Brush(_), _) => {}
    }
}

/// Draws a gradient's guides and handles: for a linear one the lines where
/// it is full and where it ends, for a radial one its edge and where the
/// fade starts.
fn draw_gradient(painter: &egui::Painter, src: &MaskSource, map: &Mapping<'_>, area: Rect, t: &Tokens) {
    let line = Stroke::new(1.5, t.text_strong);
    let shadow = Stroke::new(3.0, egui::Color32::from_black_alpha(140));
    let dot = |c: Pos2| {
        painter.circle_filled(c, 5.0, egui::Color32::from_black_alpha(160));
        painter.circle_stroke(c, 5.0, line);
    };
    match src {
        MaskSource::Linear(g) => {
            let (Some(a), Some(b)) = ((map.to_screen)(g.start), (map.to_screen)(g.end)) else { return };
            let dir = b - a;
            let len = dir.length();
            if len > 0.5 {
                // Lines across the photo, square to the gradient, through both ends.
                let n = egui::vec2(-dir.y, dir.x) / len * (area.width() + area.height());
                for c in [a, b] {
                    painter.line_segment([c - n, c + n], shadow);
                    painter.line_segment([c - n, c + n], line);
                }
                painter.line_segment([a, b], Stroke::new(1.0, t.text_strong.gamma_multiply(0.6)));
            }
            dot(a);
            dot(b);
        }
        MaskSource::Radial(g) => {
            let Some(c) = (map.to_screen)(g.centre) else { return };
            let (rx, ry) = (g.radius[0] * map.long_side, g.radius[1] * map.long_side);
            let inner = 1.0 - g.feather.clamp(0.0, 1.0);
            let ellipse = |k: f32, stroke: Stroke| {
                let points: Vec<Pos2> = (0..=72)
                    .map(|i| {
                        #[allow(clippy::cast_precision_loss)] // 73 points
                        let a = i as f32 / 72.0 * std::f32::consts::TAU;
                        c + egui::vec2(a.cos() * rx * k, a.sin() * ry * k)
                    })
                    .collect();
                painter.add(egui::Shape::line(points, stroke));
            };
            ellipse(1.0, shadow);
            ellipse(1.0, line);
            if inner > 0.01 {
                ellipse(inner, Stroke::new(1.0, t.text_strong.gamma_multiply(0.6)));
            }
            for (_, at) in handles(src, map) {
                dot(at);
            }
        }
        MaskSource::Ai(_) | MaskSource::Brush(_) => {}
    }
}
