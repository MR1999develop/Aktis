//! The tool panels on the right, as Capture One's tools: Layers, Base
//! Characteristics, Lens Correction, one panel per tool and the Spots of
//! Heal and Clone.
//!
//! - A click on a panel's header opens or closes it.
//! - Dragging the header moves the panel up or down the column; dropped
//!   outside the column, the panel floats in a window of its own there.
//! - The header's menu (…) floats or docks the panel, resets the tool and
//!   saves the camera's defaults; closing a floating window docks it again.
//!
//! The tool panels show the values of the layer chosen in the Layers
//! panel, and act on it. Their order, which are closed and which float are
//! kept in [`PanelLayout`] (in the settings and in each workspace).

use core_model::{LayerId, ToolId, ToolRef};
use editor::Document;
use eframe::egui::{self, Align, CornerRadius, Id, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, vec2};
use serde::{Deserialize, Serialize};
use tool_api::{Category, ToolRegistry};

use crate::inspector::{self, UiEdit};
use crate::retouch_ui::RetouchEdit;
use crate::theme::{FONT, RADIUS_CONTROL, SPACE, Tokens};

/// The panel showing the layers.
const LAYERS: &str = "layers";
/// Base Characteristics: curve, demosaic, profile.
const BASE: &str = "base";
/// Lens corrections.
const LENS: &str = "lens";
/// The spots of Heal and Clone (the retouch tool).
const SPOTS: &str = "spots";
/// Id of the retouch tool, whose panel is [`SPOTS`].
const RETOUCH: &str = "retouch";

/// Height of a panel's header.
const HEADER: f32 = 24.0;
/// Width a floating panel starts with.
const FLOAT_WIDTH: f32 = 300.0;
/// Height past which a floating panel's content scrolls.
const FLOAT_MAX_HEIGHT: f32 = 560.0;

/// What the panels asked for that changes the photo or the layout.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelAction {
    /// The panels and the viewer tools act on this layer.
    SelectLayer(LayerId),
    /// A new empty layer at the top.
    NewLayer,
    /// Delete a layer.
    RemoveLayer(LayerId),
    /// Rename a layer.
    RenameLayer(LayerId, String),
    /// Move a layer to this place in the list (0 = just above the Background).
    MoveLayer(LayerId, usize),
    /// Turn a layer on or off.
    LayerEnabled(LayerId, bool),
    /// A layer's opacity, 0–1.
    LayerOpacity(LayerId, f32),
    /// Every panel back in the column, in the standard order, open.
    ResetLayout,
}

/// The order of the panels, which are closed and which float (by key: a
/// tool id, or `layers`, `base`, `lens`, `spots`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelLayout {
    /// Panels in the order the user put them; the others follow in the
    /// standard order.
    pub order: Vec<String>,
    /// Closed panels (only their header shows).
    pub closed: Vec<String>,
    /// Panels in a window of their own.
    pub floating: Vec<String>,
}

impl PanelLayout {
    /// Every panel of this build, in order.
    #[must_use]
    pub fn ordered(&self, registry: &ToolRegistry) -> Vec<String> {
        let all = standard_order(registry);
        let mut out: Vec<String> = self.order.iter().filter(|k| all.contains(k)).cloned().collect();
        for k in all {
            if !out.contains(&k) {
                out.push(k);
            }
        }
        out
    }

    /// Moves panel `key` just before `before` (`None`: to the end).
    pub fn move_before(&mut self, registry: &ToolRegistry, key: &str, before: Option<&str>) {
        let mut order = self.ordered(registry);
        let Some(from) = order.iter().position(|k| k == key) else { return };
        let moved = order.remove(from);
        let to = before.and_then(|b| order.iter().position(|k| k == b)).unwrap_or(order.len());
        order.insert(to, moved);
        self.order = order;
    }

    fn toggle(list: &mut Vec<String>, key: &str, on: bool) {
        list.retain(|k| k != key);
        if on {
            list.push(key.to_owned());
        }
    }

    /// Opens or closes panel `key`.
    pub fn set_closed(&mut self, key: &str, closed: bool) {
        Self::toggle(&mut self.closed, key, closed);
    }

    /// Floats panel `key` in a window, or docks it in the column.
    pub fn set_floating(&mut self, key: &str, floating: bool) {
        Self::toggle(&mut self.floating, key, floating);
    }
}

/// The panels in the standard order: Layers, the development panels, then
/// the tools by kind (exposure and tone, colour, details, lens) in the
/// order they act, and the spots last.
fn standard_order(registry: &ToolRegistry) -> Vec<String> {
    let mut tools: Vec<(u8, usize, String)> = registry
        .pipeline()
        .iter()
        .enumerate()
        .filter(|(_, id)| id.as_str() != RETOUCH)
        .filter_map(|(i, id)| registry.get(id).map(|t| (category_rank(t.descriptor().category), i, id.to_string())))
        .collect();
    tools.sort();
    let mut out = vec![LAYERS.to_owned(), BASE.to_owned(), LENS.to_owned()];
    out.extend(tools.into_iter().map(|(_, _, id)| id));
    if registry.get(&ToolId::new(RETOUCH).expect("constant id is valid")).is_some() {
        out.push(SPOTS.to_owned());
    }
    out
}

/// The name of a kind of tool, for groups in lists.
pub(crate) fn category_name(c: Category) -> &'static str {
    match c {
        Category::Exposure => "Exposure and tone",
        Category::Color => "Colour",
        Category::Details => "Details",
        Category::Lens => "Lens",
        Category::Geometry => "Geometry",
        Category::Selection => "Selection",
    }
}

/// Where a kind of tool comes in lists.
pub(crate) fn category_rank(c: Category) -> u8 {
    match c {
        Category::Exposure => 0,
        Category::Color => 1,
        Category::Details => 2,
        Category::Lens => 3,
        Category::Geometry => 4,
        Category::Selection => 5,
    }
}

/// What the panels draw from.
pub struct Shown<'a> {
    /// The open photo.
    pub doc: &'a Document,
    /// What the panel bodies need.
    pub cx: inspector::Context<'a>,
    /// The layer the tool panels show.
    pub layer: LayerId,
}

/// What the panels collect while they are drawn.
struct Out<'o> {
    edits: &'o mut Vec<(Option<ToolRef>, UiEdit)>,
    spots: &'o mut Vec<RetouchEdit>,
    actions: &'o mut Vec<PanelAction>,
    /// Layout changes, applied after drawing.
    layout: Vec<LayoutChange>,
}

/// A change of the panels' layout asked for while drawing.
#[derive(Debug, Clone, PartialEq)]
enum LayoutChange {
    Closed(String, bool),
    Floating(String, bool, Option<Pos2>),
    MoveBefore(String, Option<String>),
}

/// The panels between frames: the header being dragged.
#[derive(Debug, Default)]
pub struct ToolPanels {
    /// The panel whose header is being dragged.
    dragging: Option<String>,
    /// Where floated panels open (the pointer when they were dropped).
    float_at: Vec<(String, Pos2)>,
}

impl ToolPanels {
    /// Draws the docked panels in `ui` (a scrolling column) and the floating
    /// ones in windows of their own; collects what they asked for.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        layout: &mut PanelLayout,
        shown: &Shown<'_>,
        edits: &mut Vec<(Option<ToolRef>, UiEdit)>,
        spots: &mut Vec<RetouchEdit>,
        actions: &mut Vec<PanelAction>,
    ) {
        let registry = shown.cx.registry;
        let order = layout.ordered(registry);
        let mut out = Out { edits, spots, actions, layout: Vec::new() };
        let column = ui.available_rect_before_wrap();
        let mut headers: Vec<(String, Rect)> = Vec::new();
        egui::ScrollArea::vertical().id_salt("aktis-tool-panels").auto_shrink([false, false]).show(ui, |ui| {
            for key in order.iter().filter(|k| !layout.floating.contains(k)) {
                let open = !layout.closed.contains(key);
                ui.push_id(("aktis-panel", key.as_str()), |ui| {
                    let header = self.header(ui, key, open, false, shown, &mut out);
                    headers.push((key.clone(), header));
                    if open {
                        egui::Frame::NONE.inner_margin(Margin { left: 8, right: 4, top: 4, bottom: 8 }).show(
                            ui,
                            |ui| {
                                body(ui, key, shown, &mut out);
                            },
                        );
                    }
                });
                ui.add_space(SPACE[0]);
            }
            if headers.is_empty() {
                ui.label(RichText::new("Every panel floats; View \u{25b8} Reset tool panels brings them back.").weak());
            }
        });
        self.drop_header(ui.ctx(), column, &headers, &mut out);

        for key in order.iter().filter(|k| layout.floating.contains(k)) {
            let mut open = true;
            let title = title(key, shown);
            let mut window = egui::Window::new(title)
                .id(Id::unique(("aktis-floating-panel", key.as_str())))
                .open(&mut open)
                .resizable(true)
                .collapsible(true)
                .default_width(FLOAT_WIDTH);
            if let Some(at) = self.float_at.iter().find(|(k, _)| k == key).map(|(_, p)| *p) {
                // Opened towards the middle of the screen, so it stays on it.
                let right = ui.ctx().content_rect().right();
                let x = if at.x + FLOAT_WIDTH > right { at.x - FLOAT_WIDTH } else { at.x };
                window = window.default_pos(egui::pos2(x.max(0.0), at.y));
            }
            window.show(ui.ctx(), |ui| {
                ui.push_id(("aktis-panel", key.as_str()), |ui| {
                    self.header(ui, key, true, true, shown, &mut out);
                    egui::ScrollArea::vertical()
                        .max_height(FLOAT_MAX_HEIGHT)
                        .show(ui, |ui| body(ui, key, shown, &mut out));
                });
            });
            if !open {
                out.layout.push(LayoutChange::Floating(key.clone(), false, None));
            }
        }

        for change in out.layout {
            match change {
                LayoutChange::Closed(key, closed) => layout.set_closed(&key, closed),
                LayoutChange::Floating(key, floating, at) => {
                    layout.set_floating(&key, floating);
                    self.float_at.retain(|(k, _)| *k != key);
                    if let Some(at) = at {
                        self.float_at.push((key.clone(), at));
                    }
                    if floating {
                        layout.set_closed(&key, false);
                    }
                }
                LayoutChange::MoveBefore(key, before) => layout.move_before(registry, &key, before.as_deref()),
            }
        }
    }

    /// A panel's header: open or closed, its name, on/off for a tool, and its
    /// menu. A floating panel's header (in its window) has no title or
    /// disclosure. Returns its rectangle.
    fn header(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        open: bool,
        floating: bool,
        shown: &Shown<'_>,
        out: &mut Out<'_>,
    ) -> Rect {
        let t = Tokens::of(ui.ctx().theme());
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), if floating { HEADER - SPACE[0] } else { HEADER }),
            Sense::click_and_drag(),
        );
        let name = title(key, shown);
        let tool = tool_of(key, shown);
        let row = tool.as_ref().and_then(|at| shown.doc.tool_row(shown.cx.registry, at));
        if !floating {
            let fill = if response.hovered() || self.dragging.as_deref() == Some(key) { t.fill } else { t.panel };
            ui.painter().rect_filled(rect, CornerRadius::same(RADIUS_CONTROL), fill);
            let arrow = Rect::from_center_size(rect.left_center() + vec2(10.0, 0.0), vec2(12.0, 12.0));
            crate::icons::disclosure(ui.painter(), arrow, open, t.text_weak);
            let used = row.as_ref().is_none_or(|r| r.used);
            let color = if used { t.text_strong } else { t.text_weak };
            ui.painter().text(
                rect.left_center() + vec2(22.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &name,
                egui::FontId::proportional(FONT[2]),
                color,
            );
        }
        let hint = if floating {
            "Close the window to put the panel back on the right"
        } else {
            "Click to open or close; drag to move it, or out of the column to float it"
        };
        let response = response.on_hover_text(hint);
        // Painted by hand: named for screen readers as the button it is.
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if !floating && response.clicked() {
            out.layout.push(LayoutChange::Closed(key.to_owned(), open));
        }
        if !floating && response.drag_started() {
            self.dragging = Some(key.to_owned());
        }
        if !floating && response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        // The controls on the right of the header.
        let mut controls = ui.new_child(
            egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 1.0))).layout(Layout::right_to_left(Align::Center)),
        );
        controls.menu_button("\u{2026}", |ui| {
            let float_label = if floating { "Dock panel" } else { "Float panel" };
            if ui.button(float_label).on_hover_text("In a window of its own, or back on the right").clicked() {
                let at = ui.ctx().pointer_latest_pos();
                out.layout.push(LayoutChange::Floating(key.to_owned(), !floating, at));
                ui.close();
            }
            if let Some(at) = &tool {
                ui.separator();
                if ui.button("Reset").on_hover_text("Every setting of this tool back to its default").clicked() {
                    out.edits.push((Some(at.clone()), UiEdit::ResetAll));
                    ui.close();
                }
                if at.layer.is_background() {
                    let camera = shown.doc.camera();
                    let saved = shown.cx.camera_defaults.params(camera, &at.tool).is_some();
                    let mut local = Vec::new();
                    inspector::defaults_items(ui, camera, saved, &mut local);
                    out.edits.extend(local.into_iter().map(|e| (Some(at.clone()), e)));
                }
            }
        });
        if let (Some(at), Some(row)) = (&tool, &row) {
            let mut on = row.enabled;
            let check = controls.add(egui::Checkbox::without_text(&mut on)).on_hover_text("Turn this tool on or off");
            check.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, format!("{name} on")));
            if check.changed() {
                out.edits.push((Some(at.clone()), UiEdit::Enabled { on }));
            }
        }
        rect
    }

    /// A header let go: dropped on the column, the panel moves there;
    /// dropped outside it, the panel floats where it was dropped.
    fn drop_header(&mut self, ctx: &egui::Context, column: Rect, headers: &[(String, Rect)], out: &mut Out<'_>) {
        let Some(key) = self.dragging.clone() else { return };
        let pointer = ctx.pointer_latest_pos();
        if let Some(p) = pointer
            && column.contains(p)
        {
            // A line where the panel would go.
            let before = headers.iter().find(|(_, r)| r.center().y > p.y);
            let y =
                before.map_or_else(|| headers.last().map_or(column.top(), |(_, r)| r.bottom()), |(_, r)| r.top() - 1.0);
            let t = Tokens::of(ctx.theme());
            ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("aktis-panel-drop"))).hline(
                column.x_range(),
                y,
                Stroke::new(2.0, t.accent),
            );
        }
        if ctx.input(|i| i.pointer.any_released()) {
            self.dragging = None;
            match pointer {
                Some(p) if column.contains(p) => {
                    let before = headers.iter().find(|(_, r)| r.center().y > p.y).map(|(k, _)| k.clone());
                    if before.as_deref() != Some(key.as_str()) {
                        out.layout.push(LayoutChange::MoveBefore(key, before));
                    }
                }
                Some(p) => out.layout.push(LayoutChange::Floating(key, true, Some(p))),
                None => {}
            }
        }
    }
}

/// The tool a panel shows, in the selected layer (`None` for the panels
/// that are not a tool's).
fn tool_of(key: &str, shown: &Shown<'_>) -> Option<ToolRef> {
    match key {
        LAYERS | BASE | LENS | SPOTS => None,
        id => ToolId::new(id).ok().map(|tool| ToolRef::new(shown.layer, tool)),
    }
}

/// A panel's name, with the layer it shows when that is not the Background.
fn title(key: &str, shown: &Shown<'_>) -> String {
    let base = match key {
        LAYERS => "Layers".to_owned(),
        BASE => "Base Characteristics".to_owned(),
        LENS => "Lens Correction".to_owned(),
        SPOTS => "Spots".to_owned(),
        id => ToolId::new(id)
            .ok()
            .and_then(|t| shown.cx.registry.get(&t).map(|t| t.descriptor().name.to_owned()))
            .unwrap_or_else(|| id.to_owned()),
    };
    let per_layer = !matches!(key, LAYERS | BASE | LENS);
    match shown.doc.recipe().layers.layer(shown.layer) {
        Some(layer) if per_layer => format!("{base} \u{b7} {}", layer_name(layer)),
        _ => base,
    }
}

/// A layer's name as shown ("Layer" when it has none).
fn layer_name(layer: &core_model::Layer) -> &str {
    if layer.name.is_empty() { "Layer" } else { layer.name.as_str() }
}

/// A panel's content.
fn body(ui: &mut egui::Ui, key: &str, shown: &Shown<'_>, out: &mut Out<'_>) {
    let mut local = Vec::new();
    match key {
        LAYERS => layers_body(ui, shown, out),
        BASE => {
            inspector::base_panel(ui, shown.doc, &mut local);
        }
        LENS => {
            inspector::lens_panel(ui, shown.doc, shown.cx, &mut local);
        }
        SPOTS => crate::retouch_ui::panel(ui, shown.doc, shown.cx.registry, shown.layer, out.spots),
        _ => {
            let Some(at) = tool_of(key, shown) else { return };
            let Some(row) = shown.doc.tool_row(shown.cx.registry, &at) else { return };
            inspector::tool_body(ui, &row, shown.cx, &mut local);
            out.edits.extend(local.drain(..).map(|e| (Some(at.clone()), e)));
            return;
        }
    }
    out.edits.extend(local.into_iter().map(|e| (None, e)));
}

/// The Layers panel: the layers from the top down to the Background, the
/// chosen one with its opacity and name; new, delete, up and down.
fn layers_body(ui: &mut egui::Ui, shown: &Shown<'_>, out: &mut Out<'_>) {
    let t = Tokens::of(ui.ctx().theme());
    let layers = &shown.doc.recipe().layers;
    let count = layers.layers().len();
    for (index, layer) in layers.layers().iter().enumerate().rev() {
        let selected = shown.layer == layer.id;
        ui.horizontal(|ui| {
            let mut on = layer.enabled;
            let check = ui.add(egui::Checkbox::without_text(&mut on)).on_hover_text("Turn this layer on or off");
            check.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, format!("{} on", layer_name(layer)))
            });
            if check.changed() {
                out.actions.push(PanelAction::LayerEnabled(layer.id, on));
            }
            let label = egui::Button::selectable(selected, layer_name(layer)).truncate();
            let r = ui.add(label).on_hover_text("Choose this layer: the panels and the mask tools act on it");
            if r.clicked() {
                out.actions.push(PanelAction::SelectLayer(layer.id));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let opacity = layer.mask.opacity;
                ui.label(RichText::new(format!("{:.0} %", opacity * 100.0)).size(FONT[1]).color(t.text_weak));
                if layer.mask.components.is_empty() {
                    ui.label(RichText::new("no mask").size(FONT[0]).color(t.text_weak))
                        .on_hover_text("Draw its mask with Masks (B) or AI Select (A): until then it changes nothing");
                }
            });
        });
        if selected {
            ui.indent(("aktis-layer-settings", layer.id.get()), |ui| {
                ui.horizontal(|ui| {
                    ui.label("Opacity");
                    let id = Id::unique(("aktis-layer-opacity", layer.id.get()));
                    if let Some(v) =
                        crate::inspector::released_slider(ui, id, layer.mask.opacity * 100.0, 0.0..=100.0, " %")
                    {
                        out.actions.push(PanelAction::LayerOpacity(layer.id, v / 100.0));
                    }
                });
                ui.horizontal(|ui| {
                    let label = ui.label("Name");
                    let text =
                        ui.ctx().data(|d| d.get_temp::<String>(Id::unique(("aktis-layer-name", layer.id.get()))));
                    let mut name = text.unwrap_or_else(|| layer.name.clone());
                    let field = ui
                        .add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY))
                        .labelled_by(label.id);
                    let key = Id::unique(("aktis-layer-name", layer.id.get()));
                    if field.changed() {
                        ui.ctx().data_mut(|d| d.insert_temp(key, name.clone()));
                    }
                    if field.lost_focus() {
                        ui.ctx().data_mut(|d| d.remove::<String>(key));
                        if name.trim() != layer.name && !name.trim().is_empty() {
                            out.actions.push(PanelAction::RenameLayer(layer.id, name));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(index + 1 < count, egui::Button::new("Up"))
                        .on_hover_text("Move the layer up")
                        .clicked()
                    {
                        out.actions.push(PanelAction::MoveLayer(layer.id, index + 1));
                    }
                    if ui
                        .add_enabled(index > 0, egui::Button::new("Down"))
                        .on_hover_text("Move the layer down")
                        .clicked()
                    {
                        out.actions.push(PanelAction::MoveLayer(layer.id, index - 1));
                    }
                    // At the far end, away from the moves.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("Delete")
                            .on_hover_text("Delete this layer with its mask and adjustments (Del)")
                            .clicked()
                        {
                            out.actions.push(PanelAction::RemoveLayer(layer.id));
                        }
                    });
                });
            });
            ui.add_space(SPACE[0]);
        }
    }
    let background = shown.layer.is_background();
    if ui
        .add(egui::Button::selectable(background, "Background"))
        .on_hover_text("The whole photo: its adjustments are under every layer")
        .clicked()
    {
        out.actions.push(PanelAction::SelectLayer(LayerId::BACKGROUND));
    }
    ui.add_space(SPACE[1]);
    ui.horizontal(|ui| {
        if ui.button("New layer").on_hover_text("An empty layer above the others").clicked() {
            out.actions.push(PanelAction::NewLayer);
        }
    });
    ui.label(
        RichText::new(
            "Draw a layer's mask with Masks (B) or AI Select (A); on the Background the first stroke adds a layer.",
        )
        .size(FONT[0])
        .color(t.text_weak),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn registry() -> ToolRegistry {
        engine::builtin_tools().unwrap()
    }

    #[test]
    fn every_tool_has_a_panel_after_layers_and_development_and_retouch_is_the_spots() {
        let reg = registry();
        let order = PanelLayout::default().ordered(&reg);
        assert_eq!(&order[..3], [LAYERS, BASE, LENS]);
        assert_eq!(order.last().map(String::as_str), Some(SPOTS));
        assert!(!order.iter().any(|k| k == RETOUCH));
        for tool in reg.iter() {
            let id = tool.descriptor().id.as_str();
            assert!(id == RETOUCH || order.iter().any(|k| k == id), "{id} has no panel");
        }
        let exposure = order.iter().position(|k| k == "exposure").unwrap();
        let sharpening = order.iter().position(|k| k == "sharpening").unwrap();
        assert!(exposure < sharpening, "exposure and tone before details");
    }

    /// A photo with a 6 × 4 developed image and no RAW data.
    fn document() -> Document {
        use std::sync::Arc;
        let img = Arc::new(core_model::ImageF32::new(6, 4, core_model::WorkingSpace::LinearRec2020).unwrap());
        let opened = editor::Opened {
            path: "a/b.CR3".into(),
            camera: "Canon EOS RP".into(),
            orientation: core_model::Orientation::Normal,
            has_camera_white_balance: true,
            raw: None,
            detected_lens: None,
            lens_choices: Vec::new(),
            lens_key: None,
            info: raw_io::PhotoInfo::default(),
            camera_curve: None,
            image: editor::DevelopedImage {
                settings: core_model::DevelopSettings::default(),
                developed: Arc::clone(&img),
                preview: Arc::clone(&img),
                draft: img,
                lens: None,
                notes: Vec::new(),
                profile: String::new(),
            },
        };
        Document::new(opened, 1)
    }

    /// Every panel open, drawn a few times without a click: nothing is
    /// asked for (a slider rounding its value to its step is not a change).
    #[test]
    fn drawing_the_panels_changes_nothing() {
        let reg = registry();
        let doc = document();
        let lenses = editor::custom_lenses::CustomLenses::load(None);
        let defaults = editor::camera_defaults::AllCameraDefaults::load(None);
        let mut panels = ToolPanels::default();
        let mut layout = PanelLayout::default();
        let edits = std::cell::RefCell::new(Vec::new());
        let actions = std::cell::RefCell::new(Vec::new());
        let mut harness = egui_kittest::Harness::builder()
            .with_size([340.0, 4000.0])
            // The panels' combo boxes and number fields are named by the label beside them.
            .with_accessibility_check(false)
            .with_context_setup(crate::theme::install)
            .build_ui(|ui| {
                let cx = inspector::Context {
                    registry: &reg,
                    developing: false,
                    histogram: None,
                    pick: None,
                    probe: None,
                    custom_lenses: &lenses,
                    camera_defaults: &defaults,
                };
                let shown = Shown { doc: &doc, cx, layer: LayerId::BACKGROUND };
                let mut spots = Vec::new();
                panels.show(ui, &mut layout, &shown, &mut edits.borrow_mut(), &mut spots, &mut actions.borrow_mut());
                assert!(spots.is_empty());
            });
        harness.run_steps(3);
        drop(harness);
        let edits: Vec<String> = edits.into_inner().iter().map(|(at, e)| format!("{at:?}: {e:?}")).collect();
        assert!(edits.is_empty(), "{edits:#?}");
        assert!(actions.into_inner().is_empty());
    }

    /// The panels' own controls (headers, switches, the Layers panel) carry
    /// the names a screen reader reads; the tools' bodies stay closed, their
    /// fields are named by the label beside them (see TIDY.md).
    #[test]
    fn the_headers_and_the_layers_panel_have_accessible_names() {
        use egui_kittest::kittest::Queryable;
        let reg = registry();
        let mut doc = document();
        doc.add_layer(&reg, "Sky", core_model::MaskDef::default(), Vec::new()).unwrap();
        let lenses = editor::custom_lenses::CustomLenses::load(None);
        let defaults = editor::camera_defaults::AllCameraDefaults::load(None);
        let mut panels = ToolPanels::default();
        let closed = PanelLayout::default().ordered(&reg).into_iter().filter(|k| k != LAYERS).collect();
        let mut layout = PanelLayout { closed, ..PanelLayout::default() };
        let mut harness = egui_kittest::Harness::builder()
            .with_size([340.0, 1200.0])
            .with_context_setup(crate::theme::install)
            .build_ui(|ui| {
                let cx = inspector::Context {
                    registry: &reg,
                    developing: false,
                    histogram: None,
                    pick: None,
                    probe: None,
                    custom_lenses: &lenses,
                    camera_defaults: &defaults,
                };
                let shown = Shown { doc: &doc, cx, layer: LayerId::BACKGROUND };
                let (mut edits, mut spots, mut actions) = (Vec::new(), Vec::new(), Vec::new());
                panels.show(ui, &mut layout, &shown, &mut edits, &mut spots, &mut actions);
            });
        harness.run_steps(2);
        harness.get_by_role_and_label(egui::Role::Button, "Exposure");
        harness.get_by_role_and_label(egui::Role::CheckBox, "Exposure on");
        harness.get_by_role_and_label(egui::Role::CheckBox, "Sky on");
    }

    /// The height of each slider tool's body at the inspector's usual width,
    /// drawn as its panel draws it (first tab, nothing picked).
    fn body_heights(tools: &[&str], width: f32) -> Vec<(String, f32)> {
        let reg = registry();
        let doc = document();
        let lenses = editor::custom_lenses::CustomLenses::load(None);
        let defaults = editor::camera_defaults::AllCameraDefaults::load(None);
        let heights = std::cell::RefCell::new(Vec::new());
        let mut harness = egui_kittest::Harness::builder()
            .with_size([width, 4000.0])
            .with_accessibility_check(false)
            .with_context_setup(crate::theme::install)
            .build_ui(|ui| {
                let cx = inspector::Context {
                    registry: &reg,
                    developing: false,
                    histogram: None,
                    pick: None,
                    probe: None,
                    custom_lenses: &lenses,
                    camera_defaults: &defaults,
                };
                let mut measured = Vec::new();
                for tool in tools {
                    let at = ToolRef::new(LayerId::BACKGROUND, core_model::ToolId::new(tool).unwrap());
                    let row = doc.tool_row(&reg, &at).unwrap();
                    let h = ui.scope(|ui| inspector::tool_body(ui, &row, cx, &mut Vec::new())).response.rect.height();
                    measured.push(((*tool).to_owned(), h));
                }
                *heights.borrow_mut() = measured;
            });
        harness.run_steps(2);
        drop(harness);
        heights.into_inner()
    }

    /// The slider tools stay compact: a slider takes one row, label on the
    /// left as in Capture One, so a tool is no taller than its budget (in
    /// points, at 300 wide; with the label above each slider they took
    /// 270 for four sliders, 474 for black & white).
    #[test]
    fn the_slider_tools_are_compact() {
        const BUDGETS: [(&str, f32); 12] = [
            ("exposure", 150.0),
            ("hdr", 150.0),
            ("sharpening", 150.0),
            ("noise_reduction", 150.0),
            ("denoise_ai", 90.0),
            ("clarity", 125.0),
            ("white_balance", 145.0),
            ("vignetting", 190.0),
            ("dehaze", 195.0),
            ("color_editor", 240.0),
            ("black_white", 315.0),
            ("color_balance", 380.0),
        ];
        let tools: Vec<&str> = BUDGETS.iter().map(|(t, _)| *t).collect();
        let heights = body_heights(&tools, 300.0);
        for ((tool, h), (_, budget)) in heights.iter().zip(BUDGETS) {
            assert!(*h <= budget, "{tool} is {h:.0} tall, more than {budget:.0}");
        }
    }

    /// A row too wide for the panel (a row of tabs) goes on to the next
    /// line instead of widening the column for the panels under it.
    #[test]
    fn no_tool_body_is_wider_than_its_panel() {
        let reg = registry();
        let doc = document();
        let lenses = editor::custom_lenses::CustomLenses::load(None);
        let defaults = editor::camera_defaults::AllCameraDefaults::load(None);
        let widths = std::cell::RefCell::new(Vec::new());
        let mut harness = egui_kittest::Harness::builder()
            .with_size([280.0, 4000.0])
            .with_accessibility_check(false)
            .with_context_setup(crate::theme::install)
            .build_ui(|ui| {
                let cx = inspector::Context {
                    registry: &reg,
                    developing: false,
                    histogram: None,
                    pick: None,
                    probe: None,
                    custom_lenses: &lenses,
                    camera_defaults: &defaults,
                };
                let room = ui.available_width();
                let mut measured = Vec::new();
                for tool in reg.pipeline() {
                    let row = doc.tool_row(&reg, &ToolRef::new(LayerId::BACKGROUND, tool.clone())).unwrap();
                    let w = ui.scope(|ui| inspector::tool_body(ui, &row, cx, &mut Vec::new())).response.rect.width();
                    measured.push((row.at.tool.to_string(), w, room));
                }
                *widths.borrow_mut() = measured;
            });
        harness.run_steps(2);
        drop(harness);
        let widths = widths.into_inner();
        assert!(!widths.is_empty());
        for (tool, w, room) in widths {
            assert!(w <= room + 0.5, "{tool} is {w:.0} wide in {room:.0}");
        }
    }

    #[test]
    fn panels_move_float_and_close_and_unknown_keys_are_dropped() {
        let reg = registry();
        let mut layout = PanelLayout { order: vec!["hologram".into(), "curve".into()], ..PanelLayout::default() };
        let order = layout.ordered(&reg);
        assert_eq!(order[0], "curve", "the user's order first, unknown panels left out");
        layout.move_before(&reg, "exposure", Some("curve"));
        let order = layout.ordered(&reg);
        assert_eq!(&order[..2], ["exposure", "curve"]);
        layout.move_before(&reg, "exposure", None);
        assert_eq!(layout.ordered(&reg).last().map(String::as_str), Some("exposure"));
        layout.set_floating("lens", true);
        layout.set_closed("lens", true);
        layout.set_floating("lens", true);
        assert_eq!(layout.floating, ["lens"], "once");
        layout.set_floating("lens", false);
        layout.set_closed("lens", false);
        assert!(layout.floating.is_empty() && layout.closed.is_empty());
    }
}
