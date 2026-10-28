//! The Masks (B) and AI Select (A) tools: they build the mask of the tool
//! node selected in the graph, so that node acts only where the mask is.
//!
//! - **Masks** (B), with the brush of Capture One: B paints, E erases, Alt
//!   held swaps them; Shift draws a straight line (Shift-click: from the
//!   last point). `[`/`]` change the size, Shift+`[`/`]` the hardness;
//!   Alt + right-drag changes the size (across) and hardness (up/down),
//!   Shift + right-drag the opacity; a right-click opens the brush settings
//!   (size, hardness, opacity, auto mask).
//! - **AI Select**: click on an object to select it; Alt-click marks a
//!   part to leave out; Shift-click starts another object. "Select
//!   subject" finds the main subject. The models run in the background.
//!
//! Both show the mask as a red overlay (M toggles it, as in Capture One),
//! and share feather, invert and clear in the options bar.
//!
//! - **Style brushes** (as in Capture One): choosing one in the options bar
//!   (or Image \u{25b8} Style brushes) arms the brush with its settings; the
//!   first stroke adds the brush's adjustments behind a new mask, named
//!   after the brush, which further strokes extend.
//!
//! The editing logic is in [`editor::masking`]; this module draws and turns
//! pointer input into [`MaskEdit`]s.

use core_model::{MaskDef, NodeId, NormPoint, Style};
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
        /// The tool node the mask restricts.
        node: NodeId,
        /// The path.
        points: Vec<NormPoint>,
        /// Remove instead of add.
        erase: bool,
    },
    /// An AI Select click.
    Click {
        /// The tool node.
        node: NodeId,
        /// Where (developed image).
        at: NormPoint,
        /// On the object (`false`: a part to leave out).
        include: bool,
        /// Starts another object.
        new_object: bool,
    },
    /// "Select subject".
    Subject {
        /// The tool node.
        node: NodeId,
    },
    /// Feather, invert or clear: the whole new definition.
    Def {
        /// The tool node.
        node: NodeId,
        /// The definition.
        def: MaskDef,
    },
    /// The first stroke of an armed style brush: its adjustments are added
    /// behind a new mask holding the stroke.
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
    /// The overlay texture and what it was made for (document generation, node, cropped).
    overlay: Option<((u64, NodeId, bool), TextureHandle)>,
    /// The style brush the next stroke adds.
    armed: Option<ArmedBrush>,
    /// Where the brush settings popup is open (right-click), in points.
    settings_at: Option<Pos2>,
    /// A right-drag changing the brush: the brush and pointer at its start.
    adjusting: Option<(Brush, Pos2)>,
    /// The end of the last stroke, for Shift-click straight lines.
    last_point: Option<(NormPoint, Pos2)>,
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
    /// Shift+`[`/`]` hardness, B brush, E eraser, M mask overlay. Not while typing.
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
        let shown = egui::Area::new(Id::unique("rne-brush-settings"))
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
        node: Option<NodeId>,
        long_side: f32,
        to_developed: &dyn Fn(Pos2) -> Option<NormPoint>,
        edits: &mut Vec<MaskEdit>,
    ) -> bool {
        let t = Tokens::of(ui.ctx().theme());
        let painting_style = tool == ViewerTool::Masks && self.armed.is_some();
        if node.is_none() && !painting_style {
            return false;
        }
        let painter = ui.painter_at(response.rect);
        let alt = ui.input(|i| i.modifiers.alt) && !painting_style;
        match tool {
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
                        } else if let Some(node) = node {
                            edits.push(MaskEdit::Stroke { node, points, erase: self.erase || alt });
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
                    && let Some(node) = node
                    && let Some(at) = response.interact_pointer_pos().and_then(to_developed)
                {
                    let shift = ui.input(|i| i.modifiers.shift);
                    edits.push(MaskEdit::Click { node, at, include: !alt, new_object: shift });
                }
                false
            }
            _ => false,
        }
    }

    /// Draws the mask overlay over `screen`, making its texture when the
    /// mask changed (`key`: document generation, node, cropped); `values`
    /// computes the mask as shown (width, height, values 0–1).
    pub fn draw_overlay(
        &mut self,
        ui: &egui::Ui,
        screen: Rect,
        key: (u64, NodeId, bool),
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
                (key, ui.ctx().load_texture("rne-mask-overlay", img, egui::TextureOptions::LINEAR))
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
        node: Option<(NodeId, String)>,
        def: Option<&MaskDef>,
        busy: bool,
        brushes: &[String],
        edits: &mut Vec<MaskEdit>,
    ) {
        let t = Tokens::of(ctx.theme());
        egui::Area::new(Id::unique("rne-mask-options"))
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
                            let Some((node, name)) = node else {
                                ui.label(RichText::new("Select a node in the graph: its mask is what you draw").color(t.text_weak));
                                return;
                            };
                            ui.label(RichText::new(format!("Mask of {name}")).strong());
                            ui.separator();
                            match tool {
                                ViewerTool::Masks => {
                                    ui.selectable_value(&mut self.erase, false, "Brush").on_hover_text("B");
                                    ui.selectable_value(&mut self.erase, true, "Eraser").on_hover_text("E, or hold Alt");
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
                                _ => {
                                    ui.label(RichText::new("Click an object \u{b7} Alt-click: leave out \u{b7} Shift-click: another object").color(t.text_weak));
                                    if ui.button("Select subject").clicked() {
                                        edits.push(MaskEdit::Subject { node });
                                    }
                                    if busy {
                                        ui.spinner();
                                    }
                                }
                            }
                            ui.separator();
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
                                    edits.push(MaskEdit::Def { node, def: next.sanitized() });
                                }
                                if ui.button("Clear").on_hover_text("Remove everything from this mask").clicked() {
                                    edits.push(MaskEdit::Def { node, def: MaskDef { components: Vec::new(), ..def.clone() } });
                                }
                            }
                        });
                    });
            });
    }
}

impl MaskUi {
    /// The Style brush menu of the options bar: the brushes, and Cancel
    /// while one is armed.
    fn style_brush_picker(&mut self, ui: &mut egui::Ui, t: &Tokens, brushes: &[String], edits: &mut Vec<MaskEdit>) {
        let current = self.armed.as_ref().map_or("Style brush", |a| a.name.as_str());
        egui::ComboBox::from_id_salt("rne-style-brush")
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
            .on_hover_text(
                "Paint adjustments: the first stroke adds a new masked node, as Capture One's style brushes",
            );
        if self.armed.is_some() && ui.button("Cancel").on_hover_text("Paint the selected node's mask instead").clicked()
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
