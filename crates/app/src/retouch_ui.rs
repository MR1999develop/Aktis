//! The Heal (H) and Clone (S) tools of the bar, drawn over the photo, like
//! the spot tools of Capture One and Lightroom:
//!
//! - click on the photo to put a spot there: a circle of the brush size,
//!   with a source picked automatically nearby, shown as a second circle
//!   joined to it by a line;
//! - click a spot to select it, drag either circle to move it;
//! - Size (also `[` and `]`), Feather and Opacity in the options bar apply
//!   to the selected spot and to the next ones; Heal and Clone switch the
//!   tool and the selected spot's kind;
//! - Delete or Backspace (or the Delete button) removes the selected spot.
//!
//! The spots are parameters of the retouch tool of the selected layer
//! ([`tool_adjust::retouch`], [`editor::retouch`] says where a new one
//! goes): the spots of that layer are shown and edited. Every change is one
//! undo step; a drag is one step.

use commands::GestureId;
use core_model::{Geometry, LayerId, NormPoint, ParamId, ParamValue, ToolRef};
use editor::Document;
use editor::retouch::{self as spots_of, Circles, Part};
use eframe::egui::{self, Pos2, Rect, Response, RichText, Stroke};
use tool_adjust::retouch::{self, Field, MAX_RADIUS, Spot, SpotKind};
use tool_api::ToolRegistry;

use crate::theme::{FONT, SPACE, SPOT_LINE, SPOT_SHADOW, Tokens};
use crate::toolbar::ViewerTool;

/// Smallest brush radius, as a fraction of the long side.
const MIN_RADIUS: f32 = 0.0005;

/// Points around a small circle that still catch the pointer.
const SLACK: f32 = 6.0;

/// Size change of one `[` or `]` press.
const SIZE_STEP: f32 = 1.15;

/// A change asked for by the Heal and Clone tools.
#[derive(Debug, Clone, PartialEq)]
pub enum RetouchEdit {
    /// A drag started: the next changes form one undo step.
    Begin,
    /// The drag ended.
    End,
    /// A new spot (it becomes the selected one).
    Add(Spot),
    /// New values for a spot of the slot `at` (`in_drag`: part of the drag).
    Change {
        /// The slot of the retouch tool.
        at: ToolRef,
        /// The values.
        values: Vec<(ParamId, ParamValue)>,
        /// Part of the current drag.
        in_drag: bool,
    },
}

/// A spot of the layer: its slot, its index there, the spot.
type Placed = (ToolRef, usize, Spot);

/// A circle being dragged.
#[derive(Debug, Clone, PartialEq)]
struct SpotDrag {
    at: ToolRef,
    index: usize,
    part: Part,
    /// Centre minus pointer, in the developed image (0–1).
    offset: [f32; 2],
}

/// State of the Heal and Clone tools between frames.
#[derive(Debug)]
pub struct RetouchUi {
    /// Brush radius for new spots (fraction of the long side).
    radius: f32,
    /// Feather of new spots (0–1).
    feather: f32,
    /// Opacity of new spots (0–1).
    opacity: f32,
    /// The selected spot: its slot and index there.
    selected: Option<(ToolRef, usize)>,
    drag: Option<SpotDrag>,
    /// The undo step of the current drag.
    gesture: Option<GestureId>,
}

impl Default for RetouchUi {
    fn default() -> Self {
        Self { radius: 0.01, feather: 0.5, opacity: 1.0, selected: None, drag: None, gesture: None }
    }
}

/// From the screen to the developed image and back, through the geometry.
struct Mapping<'a> {
    doc: &'a Document,
    geometry: Geometry,
    aspect: f32,
    screen: Rect,
    /// Width and height of the developed image in pixels.
    size: [f32; 2],
}

impl<'a> Mapping<'a> {
    #[allow(clippy::cast_precision_loss)] // image sizes
    fn new(doc: &'a Document, screen: Rect) -> Self {
        let (uw, uh) = doc.upright_size();
        let developed = doc.developed();
        Self {
            doc,
            geometry: doc.recipe().geometry,
            aspect: uw as f32 / uh.max(1) as f32,
            screen,
            size: [developed.width().max(1) as f32, developed.height().max(1) as f32],
        }
    }

    /// The developed-image point under screen point `p`.
    fn to_dev(&self, p: Pos2) -> Option<[f32; 2]> {
        let s = self.screen;
        let shown = [(p.x - s.min.x) / s.width().max(1.0), (p.y - s.min.y) / s.height().max(1.0)];
        let upright = self.geometry.to_source(shown, self.aspect, true)?;
        let d = self.doc.developed_point(upright)?;
        Some([d.x(), d.y()])
    }

    /// Where developed-image point `d` is on screen, if it is shown.
    fn to_screen(&self, d: [f32; 2]) -> Option<Pos2> {
        let upright = self.doc.upright_point(NormPoint::new(d[0], d[1])?);
        let shown = self.geometry.to_shown(upright, self.aspect, true)?;
        let s = self.screen;
        Some(egui::pos2(s.min.x + shown[0] * s.width(), s.min.y + shown[1] * s.height()))
    }

    /// The radius on screen of a circle at `at` with stored radius `r`.
    #[allow(clippy::cast_precision_loss)] // image sizes
    fn radius(&self, at: [f32; 2], r: f32) -> f32 {
        let px = r * self.size[0].max(self.size[1]);
        let (dx, dy) = (px / self.size[0], px / self.size[1]);
        let mut total = 0.0;
        let mut n = 0.0;
        if let Some(c) = self.to_screen(at) {
            for o in [[dx, 0.0], [-dx, 0.0], [0.0, dy], [0.0, -dy]] {
                if let Some(q) = self.to_screen([at[0] + o[0], at[1] + o[1]]) {
                    total += c.distance(q);
                    n += 1.0;
                }
            }
        }
        if n > 0.0 {
            total / n
        } else {
            let (w, _) = self.doc.shown_size(true);
            px * self.screen.width() / w.max(1) as f32
        }
    }
}

/// The kind of spot a tool makes.
fn kind_of(tool: ViewerTool) -> SpotKind {
    if tool == ViewerTool::Clone { SpotKind::Clone } else { SpotKind::Heal }
}

impl RetouchUi {
    /// The selected spot, if it still exists in `current`.
    fn selected_spot(&mut self, current: &[Placed]) -> Option<Spot> {
        let found = self
            .selected
            .as_ref()
            .and_then(|(at, i)| current.iter().find(|(a, j, _)| a == at && j == i).map(|(_, _, s)| *s));
        if found.is_none() {
            self.selected = None;
        }
        found
    }

    /// Forgets the selected spot (another photo or layer).
    pub fn deselect(&mut self) {
        self.selected = None;
        self.drag = None;
    }

    /// Applies what the tools asked for on `layer`; returns a message for
    /// the user when a change was refused. A new spot becomes the selected one.
    pub fn apply(
        &mut self,
        doc: &mut Document,
        registry: &ToolRegistry,
        layer: LayerId,
        edits: Vec<RetouchEdit>,
    ) -> Option<String> {
        let mut problem = None;
        for edit in edits {
            let result = match edit {
                RetouchEdit::Begin => {
                    self.gesture = Some(doc.begin_gesture());
                    Ok(())
                }
                RetouchEdit::End => {
                    self.gesture = None;
                    Ok(())
                }
                RetouchEdit::Add(spot) => {
                    spots_of::add_spot(doc, registry, layer, &spot).map(|placed| self.selected = Some(placed))
                }
                RetouchEdit::Change { at, values, in_drag } => {
                    doc.set_params(registry, &at, values, self.gesture.filter(|_| in_drag))
                }
            };
            if let Err(e) = result {
                problem = Some(format!("That change was refused: {e}"));
            }
        }
        problem
    }

    /// The tool over the photo shown in `screen`: draws the spots of
    /// `layer`, places, selects, moves, resizes and deletes them. Returns whether the pointer drag is the tool's (so the
    /// viewer does not pan).
    #[allow(clippy::too_many_arguments)] // the viewer's state, as the other tools
    pub fn overlay(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        screen: Rect,
        doc: &Document,
        registry: &ToolRegistry,
        layer: LayerId,
        tool: ViewerTool,
        edits: &mut Vec<RetouchEdit>,
    ) -> bool {
        let t = Tokens::of(ui.ctx().theme());
        let map = Mapping::new(doc, screen);
        let current = spots_of::all_spots(doc, registry, layer);
        let selected = self.selected_spot(&current);
        // A circle's `slot` is the spot's place in `current`.
        let circles: Vec<Circles> = current
            .iter()
            .enumerate()
            .map(|(k, (_, _, s))| Circles {
                slot: k,
                spot: map.to_screen(s.at).map(|p| [p.x, p.y]),
                source: map.to_screen(s.source).map(|p| [p.x, p.y]),
                radius: map.radius(s.at, s.radius),
            })
            .collect();
        let hit = |p: Pos2| spots_of::hit(&circles, [p.x, p.y], SLACK);

        self.keys(ui.ctx(), selected, edits);

        // Start a drag on a circle; the drag moves it.
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            && let Some((k, part)) = hit(origin)
            && let Some((at, index, spot)) = current.get(k)
            && let Some(pointer) = map.to_dev(origin)
        {
            let centre = if part == Part::Spot { spot.at } else { spot.source };
            let offset = [centre[0] - pointer[0], centre[1] - pointer[1]];
            self.drag = Some(SpotDrag { at: at.clone(), index: *index, part, offset });
            self.selected = Some((at.clone(), *index));
            edits.push(RetouchEdit::Begin);
        }
        if let Some(d) = self.drag.clone() {
            let spot = current.iter().find(|(a, i, _)| *a == d.at && *i == d.index).map(|(_, _, s)| *s);
            if response.dragged()
                && let Some(mut spot) = spot
                && let Some(at) = response.interact_pointer_pos().and_then(|p| map.to_dev(p))
            {
                let centre = [(at[0] + d.offset[0]).clamp(0.0, 1.0), (at[1] + d.offset[1]).clamp(0.0, 1.0)];
                let fields = if d.part == Part::Spot {
                    spot.at = centre;
                    [Field::X, Field::Y]
                } else {
                    spot.source = centre;
                    [Field::SourceX, Field::SourceY]
                };
                edits.push(RetouchEdit::Change {
                    at: d.at.clone(),
                    values: retouch::values(d.index, &spot, &fields),
                    in_drag: true,
                });
            }
            if !response.dragged() || spot.is_none() {
                self.drag = None;
                edits.push(RetouchEdit::End);
            }
        }

        // A click selects the spot under it, or puts a new one there.
        if response.clicked_by(egui::PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
        {
            match hit(p).and_then(|(k, _)| current.get(k)) {
                Some((at, index, _)) => self.selected = Some((at.clone(), *index)),
                None => {
                    if let Some(at) = map.to_dev(p) {
                        let spot = Spot {
                            kind: kind_of(tool),
                            at,
                            radius: self.radius,
                            source: spots_of::auto_source(doc, at, self.radius),
                            feather: self.feather,
                            opacity: self.opacity,
                        };
                        edits.push(RetouchEdit::Add(spot));
                    }
                }
            }
        }

        // Cursor, and the brush under the pointer.
        let hovered = response.hover_pos().and_then(|p| hit(p).map(|h| (p, h)));
        let painter = ui.painter_at(screen.intersect(response.rect));
        if self.drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if hovered.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if let Some(p) = response.hover_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if let Some(at) = map.to_dev(p) {
                ring(&painter, p, map.radius(at, self.radius), None);
            }
        }

        // The spots: each spot's circle; the source and the line too for
        // the selected one, the one under the pointer and the one dragged.
        let place = |at: &ToolRef, index: usize| current.iter().position(|(a, i, _)| a == at && *i == index);
        let shown_slot = self.selected.as_ref().and_then(|(at, i)| place(at, *i));
        let hover_slot = hovered.map(|(_, (k, _))| k);
        let drag_slot = self.drag.as_ref().and_then(|d| place(&d.at, d.index));
        for c in &circles {
            let active = Some(c.slot) == shown_slot;
            let accent = active.then_some(t.accent);
            let spot = c.spot.map(|p| egui::pos2(p[0], p[1]));
            let source = c.source.map(|p| egui::pos2(p[0], p[1]));
            if let Some(p) = spot {
                ring(&painter, p, c.radius, accent);
            }
            if active || Some(c.slot) == hover_slot || Some(c.slot) == drag_slot {
                if let Some(q) = source {
                    ring(&painter, q, c.radius, accent);
                }
                if let (Some(p), Some(q)) = (spot, source)
                    && p.distance(q) > 2.0 * c.radius
                {
                    // From edge to edge.
                    let dir = (q - p).normalized();
                    link(&painter, p + dir * c.radius, q - dir * c.radius, accent);
                }
            }
        }
        self.drag.is_some()
    }

    /// `[` and `]` change the size, Delete and Backspace remove the
    /// selected spot. Not while typing.
    fn keys(&mut self, ctx: &egui::Context, selected: Option<Spot>, edits: &mut Vec<RetouchEdit>) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (smaller, larger) = ctx.input(|i| {
            let typed = |c: &str| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == c));
            (i.key_pressed(egui::Key::OpenBracket) || typed("["), i.key_pressed(egui::Key::CloseBracket) || typed("]"))
        });
        if smaller != larger {
            let base = selected.map_or(self.radius, |s| s.radius);
            let next = if larger { base * SIZE_STEP } else { base / SIZE_STEP };
            self.set_radius(next.clamp(MIN_RADIUS, MAX_RADIUS), selected, false, edits);
        }
        let delete = self.selected.is_some()
            && ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
            });
        if delete {
            self.delete(edits);
        }
    }

    /// Removes the selected spot.
    fn delete(&mut self, edits: &mut Vec<RetouchEdit>) {
        if let Some((at, index)) = self.selected.take() {
            self.drag = None;
            edits.push(RetouchEdit::Change { at, values: retouch::cleared(index), in_drag: false });
        }
    }

    /// A new brush size, given to the selected spot too.
    fn set_radius(&mut self, radius: f32, selected: Option<Spot>, in_drag: bool, edits: &mut Vec<RetouchEdit>) {
        self.radius = radius;
        self.change_selected(selected, |s| s.radius = radius, Field::Radius, in_drag, edits);
    }

    /// Changes one field of the selected spot.
    fn change_selected(
        &self,
        selected: Option<Spot>,
        change: impl FnOnce(&mut Spot),
        field: Field,
        in_drag: bool,
        edits: &mut Vec<RetouchEdit>,
    ) {
        if let (Some((at, index)), Some(mut spot)) = (&self.selected, selected) {
            change(&mut spot);
            let values = retouch::values(*index, &spot, &[field]);
            edits.push(RetouchEdit::Change { at: at.clone(), values, in_drag });
        }
    }

    /// The options bar at the top of the viewer: Heal or Clone, Size,
    /// Feather, Opacity and Delete. Returns the tool to switch to when the
    /// other kind was chosen.
    #[allow(clippy::too_many_arguments)] // the viewer's state, as the other tools
    pub fn options(
        &mut self,
        ctx: &egui::Context,
        viewer: Rect,
        tool: ViewerTool,
        doc: &Document,
        registry: &ToolRegistry,
        layer: LayerId,
        edits: &mut Vec<RetouchEdit>,
    ) -> Option<ViewerTool> {
        let current = spots_of::all_spots(doc, registry, layer);
        let selected = self.selected_spot(&current);
        let developed = doc.developed();
        #[allow(clippy::cast_precision_loss)] // image sizes
        let long = developed.width().max(developed.height()).max(1) as f32;
        let mut switch = None;
        crate::geometry_ui::options_bar(ctx, viewer, "aktis-retouch-options", |ui| {
            for (kind_tool, label, hint) in [
                (ViewerTool::Heal, "Heal", "Copy texture from the source, matched to the spot's surroundings (H)"),
                (ViewerTool::Clone, "Clone", "Copy the source as it is (S)"),
            ] {
                if ui.add(egui::Button::selectable(tool == kind_tool, label)).on_hover_text(hint).clicked()
                    && tool != kind_tool
                {
                    switch = Some(kind_tool);
                    let kind = kind_of(kind_tool);
                    self.change_selected(selected, |s| s.kind = kind, Field::Kind, false, edits);
                }
            }
            ui.separator();

            let shown = selected.unwrap_or(Spot {
                kind: kind_of(tool),
                at: [0.5, 0.5],
                radius: self.radius,
                source: [0.5, 0.5],
                feather: self.feather,
                opacity: self.opacity,
            });
            let mut ends = Vec::new();
            // A drag on a slider changes the selected spot in one undo step.
            let mut track = |r: &Response, edits: &mut Vec<RetouchEdit>| {
                if r.drag_started() && selected.is_some() {
                    edits.push(RetouchEdit::Begin);
                }
                ends.push(r.drag_stopped() && selected.is_some());
                r.dragged() || r.drag_started()
            };
            let mut size = shown.radius * long;
            let r = slider(ui, "Size", &mut size, 1.0..=MAX_RADIUS * long, " px", true);
            let in_drag = track(&r, edits);
            if r.changed() {
                self.set_radius((size / long).clamp(MIN_RADIUS, MAX_RADIUS), selected, in_drag, edits);
            }
            let mut feather = shown.feather * 100.0;
            let r = slider(ui, "Feather", &mut feather, 0.0..=100.0, " %", false);
            let in_drag = track(&r, edits);
            if r.changed() {
                self.feather = feather / 100.0;
                let f = self.feather;
                self.change_selected(selected, |s| s.feather = f, Field::Feather, in_drag, edits);
            }
            let mut opacity = shown.opacity * 100.0;
            let r = slider(ui, "Opacity", &mut opacity, 0.0..=100.0, " %", false);
            let in_drag = track(&r, edits);
            if r.changed() {
                self.opacity = opacity / 100.0;
                let o = self.opacity;
                self.change_selected(selected, |s| s.opacity = o, Field::Opacity, in_drag, edits);
            }
            if ends.into_iter().any(|e| e) {
                edits.push(RetouchEdit::End);
            }
            ui.separator();
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Delete"))
                .on_hover_text("Remove the selected spot (Delete or Backspace)")
                .on_disabled_hover_text("Click a spot to select it first")
                .clicked()
            {
                self.delete(edits);
            }
            let hint = if selected.is_some() {
                "Drag a circle to move it; [ and ] change the size"
            } else {
                "Click a spot on the photo; [ and ] change the size"
            };
            ui.label(RichText::new(hint).weak());
        });
        switch
    }
}

/// A labelled slider of the options bar.
fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    logarithmic: bool,
) -> Response {
    ui.label(label);
    ui.add(
        egui::Slider::new(value, range)
            .suffix(suffix)
            .fixed_decimals(0)
            .logarithmic(logarithmic)
            .update_while_editing(false),
    )
}

/// A circle of a spot: a light line on a dark shadow (or the accent for
/// the selected spot), visible on any photo.
fn ring(painter: &egui::Painter, centre: Pos2, radius: f32, accent: Option<egui::Color32>) {
    painter.circle_stroke(centre, radius, Stroke::new(3.0, SPOT_SHADOW));
    painter.circle_stroke(centre, radius, Stroke::new(1.5, accent.unwrap_or(SPOT_LINE)));
}

/// The line from a spot to its source.
fn link(painter: &egui::Painter, from: Pos2, to: Pos2, accent: Option<egui::Color32>) {
    painter.line_segment([from, to], Stroke::new(2.5, SPOT_SHADOW));
    painter.line_segment([from, to], Stroke::new(1.0, accent.unwrap_or(SPOT_LINE)));
}

/// The Spots panel: the spots of `layer`, each with a Remove button, and
/// how to add them.
pub fn panel(ui: &mut egui::Ui, doc: &Document, registry: &ToolRegistry, layer: LayerId, edits: &mut Vec<RetouchEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let spots = spots_of::all_spots(doc, registry, layer);
    let count = spots.len();
    let text = if count == 0 {
        "No spots. Add them with Heal (H) or Clone (S) in the bar.".to_owned()
    } else {
        format!("{count} spots. Add more with Heal (H) or Clone (S) in the bar.")
    };
    ui.label(RichText::new(text).color(t.text_weak));
    ui.add_space(SPACE[0]);
    for (n, (at, index, spot)) in spots.iter().enumerate() {
        ui.horizontal(|ui| {
            let kind = match spot.kind {
                SpotKind::Heal => "Heal",
                SpotKind::Clone => "Clone",
            };
            ui.label(format!("Spot {}", n + 1));
            ui.label(
                RichText::new(format!(
                    "{kind}, size {:.1} %, feather {:.0} %, opacity {:.0} %",
                    spot.radius * 100.0,
                    spot.feather * 100.0,
                    spot.opacity * 100.0
                ))
                .size(FONT[1])
                .color(t.text_weak),
            );
            if ui.small_button("Remove").on_hover_text("Remove this spot").clicked() {
                edits.push(RetouchEdit::Change { at: at.clone(), values: retouch::cleared(*index), in_drag: false });
            }
        });
    }
}
