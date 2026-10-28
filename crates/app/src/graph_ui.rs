//! The node graph under the viewer, and the histogram above the settings.
//!
//! A new photo shows only the Source. Clicking the port on the right of a
//! node opens the list of tools; the chosen one becomes a new node after
//! it. Nodes are dragged anywhere; dragging the background (or with the
//! middle button anywhere) pans, the wheel zooms. Click a node to edit it on the right,
//! right-click it to bypass or delete it.

use core_model::{Node, NodeGraph, NodeId, NodeKind, PortKind, ToolId};
use editor::Histogram;
use editor::graph_view::{GraphView, NODE_SIZE, PORT_HIT};
use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, CursorIcon, FontId, Id, Key, Order, PointerButton, Pos2, Rect,
    RichText, Sense, Shape, Stroke, StrokeKind, epaint::CubicBezierShape, pos2, vec2,
};
use tool_api::{Category, ToolRegistry};

use crate::theme::{FONT, HISTOGRAM, SPACE, Tokens};

/// Spacing of the background grid.
const GRID: f32 = 20.0;
/// Radius of a port.
const PORT: f32 = 4.5;
/// Zoom factor per point of mouse-wheel scrolling (as in the viewer).
const WHEEL_ZOOM_PER_POINT: f32 = 0.004_5;
/// Width of the "add a node" menu.
const ADD_MENU_WIDTH: f32 = 200.0;

/// What the user did in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphAction {
    /// Show this node's settings.
    Select(NodeId),
    /// Click on the background: show none.
    Deselect,
    /// Add a node running `tool` after `after`.
    Add {
        /// Node whose output the new node takes.
        after: NodeId,
        /// The tool.
        tool: ToolId,
    },
    /// Delete a node.
    Remove(NodeId),
    /// Turn a node on or off.
    Enable(NodeId, bool),
}

/// The open "add a node" menu.
#[derive(Debug, Clone, Copy)]
struct AddMenu {
    after: NodeId,
    at: Pos2,
    /// Opened by this frame's click: not closed by it.
    fresh: bool,
}

/// State of the node graph panel.
#[derive(Debug, Default)]
pub struct GraphUi {
    view: GraphView,
    dragging: Option<NodeId>,
    panning: bool,
    add_menu: Option<AddMenu>,
    /// The node right-clicked, for the context menu.
    menu_node: Option<NodeId>,
}

/// The name shown on a node: its label, else the tool's name.
#[must_use]
pub fn node_title(node: &Node, registry: &ToolRegistry) -> String {
    if !node.label.is_empty() {
        return node.label.clone();
    }
    match &node.kind {
        NodeKind::Source => "Source".to_owned(),
        NodeKind::Output => "Output".to_owned(),
        NodeKind::Tool(id) => {
            registry.get(id).map_or_else(|| id.as_str().to_owned(), |t| t.descriptor().name.to_owned())
        }
        NodeKind::Parallel { .. } => "Parallel".to_owned(),
        NodeKind::Mask => "Mask".to_owned(),
    }
}

/// The line under a node's name.
fn node_subtitle(node: &Node) -> &'static str {
    match &node.kind {
        NodeKind::Source => "RAW develop",
        NodeKind::Output => "Export",
        NodeKind::Mask => "Mask",
        NodeKind::Parallel { .. } => "Adds branches",
        NodeKind::Tool(_) if !node.enabled => "Off",
        NodeKind::Tool(_) => "On",
    }
}

fn category_name(c: Category) -> &'static str {
    match c {
        Category::Exposure => "Exposure and tone",
        Category::Color => "Colour",
        Category::Details => "Details",
        Category::Lens => "Lens",
        Category::Geometry => "Geometry",
        Category::Selection => "Selection",
    }
}

fn category_rank(c: Category) -> u8 {
    match c {
        Category::Exposure => 0,
        Category::Color => 1,
        Category::Details => 2,
        Category::Lens => 3,
        Category::Geometry => 4,
        Category::Selection => 5,
    }
}

/// The tools of this build that can be a graph node, by category then name.
fn node_tools(registry: &ToolRegistry) -> Vec<(Category, ToolId, &'static str)> {
    let mut tools: Vec<_> =
        registry.iter().map(|t| t.descriptor()).map(|d| (d.category, d.id.clone(), d.name)).collect();
    tools.sort_by(|a, b| category_rank(a.0).cmp(&category_rank(b.0)).then_with(|| a.2.cmp(b.2)));
    tools
}

impl GraphUi {
    /// Node positions, pan and zoom (saved with the edit).
    #[must_use]
    pub fn view(&self) -> &GraphView {
        &self.view
    }

    /// Shows another photo or variant: its saved node positions, pan and
    /// zoom, no drag or menu in progress.
    pub fn restore(&mut self, view: GraphView) {
        *self = Self { view, ..Self::default() };
    }

    /// Draws the graph and the menus; returns what the user did.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        graph: &NodeGraph,
        registry: &ToolRegistry,
        selected: Option<NodeId>,
    ) -> Vec<GraphAction> {
        let t = Tokens::of(ui.ctx().theme());
        let mut actions = Vec::new();
        self.view.sync(graph);
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let area = response.rect;
        // Wheel: zoom around the pointer.
        if let Some(pointer) = response.hover_pos() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            let factor = ui.input(egui::InputState::zoom_delta) * (scroll * WHEEL_ZOOM_PER_POINT).exp();
            if (factor - 1.0).abs() > 1e-4 {
                self.view.zoom_at(factor, [pointer.x - area.min.x, pointer.y - area.min.y]);
            }
        }
        let z = self.view.zoom;
        let origin = area.min + vec2(self.view.pan[0], self.view.pan[1]);
        let to_screen = |p: [f32; 2]| origin + vec2(p[0], p[1]) * z;
        let to_graph = |q: Pos2| [(q.x - origin.x) / z, (q.y - origin.y) / z];
        let port_hit = PORT_HIT / z;

        // Background grid, moving with the pan.
        let grid = Stroke::new(1.0, t.graph_grid);
        let step = GRID * z;
        let mut x = area.left() + self.view.pan[0].rem_euclid(step);
        while x < area.right() {
            painter.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], grid);
            x += step;
        }
        let mut y = area.top() + self.view.pan[1].rem_euclid(step);
        while y < area.bottom() {
            painter.line_segment([pos2(area.left(), y), pos2(area.right(), y)], grid);
            y += step;
        }

        // Links behind the nodes (the hidden Output's link is not drawn).
        for link in graph.links() {
            let (Some(from), Some(to_node)) = (self.view.output_port(link.from), graph.node(link.to)) else {
                continue;
            };
            let inputs = to_node.kind.inputs();
            let Some(to) = self.view.input_port(link.to, usize::from(link.to_port), inputs.len()) else { continue };
            let kind = inputs.get(usize::from(link.to_port)).copied().unwrap_or(PortKind::Image);
            let (start, end) = (to_screen(from), to_screen(to));
            let bend = ((end.x - start.x).abs() * 0.5).max(30.0 * z);
            painter.add(CubicBezierShape::from_points_stroke(
                [start, start + vec2(bend, 0.0), end - vec2(bend, 0.0), end],
                false,
                Color32::TRANSPARENT,
                Stroke::new(2.0 * z.sqrt(), port_color(&t, kind)),
            ));
        }

        // Nodes.
        let hover = response.hover_pos().map(to_graph);
        let hovered_node = hover.and_then(|p| self.view.node_at(p));
        let hovered_port = hover.and_then(|p| self.view.output_port_at(graph, p, port_hit));
        let feeds_output = graph.input_link(graph.output(), 0).map(|l| l.from);
        for (id, pos) in self.view.nodes() {
            let Some(node) = graph.node(id) else { continue };
            let rect = Rect::from_min_size(to_screen(pos), vec2(NODE_SIZE[0], NODE_SIZE[1]) * z);
            if !rect.expand(PORT_HIT).intersects(area) {
                continue;
            }
            let is_selected = selected == Some(id);
            let hovered = hovered_node == Some(id) || self.dragging == Some(id);
            let off = matches!(node.kind, NodeKind::Tool(_) | NodeKind::Mask) && !node.enabled;
            let fill = if hovered { t.bg_overlay } else { t.bg_raised };
            let stroke = if is_selected {
                Stroke::new(2.0, t.accent)
            } else {
                Stroke::new(1.0, if hovered { t.stroke_strong } else { t.stroke_weak })
            };
            painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
            let strip = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(rect.width() - 2.0, 3.0 * z));
            painter.rect_filled(strip, CornerRadius::same(2), kind_color(&t, &node.kind));
            let title_color = if off { t.text_weak } else { t.text_strong };
            painter.with_clip_rect(rect.shrink(4.0)).text(
                rect.left_top() + vec2(10.0, 10.0) * z,
                Align2::LEFT_TOP,
                node_title(node, registry),
                FontId::proportional(FONT[1] * z),
                title_color,
            );
            painter.text(
                rect.left_top() + vec2(10.0, 28.0) * z,
                Align2::LEFT_TOP,
                node_subtitle(node),
                FontId::proportional(FONT[0] * z),
                t.text_weak,
            );
            let inputs = node.kind.inputs();
            for (k, kind) in inputs.iter().enumerate() {
                if let Some(p) = self.view.input_port(id, k, inputs.len()) {
                    painter.circle_filled(to_screen(p), PORT * z, port_color(&t, *kind));
                }
            }
            if let (Some(kind), Some(p)) = (node.kind.outputs().first(), self.view.output_port(id)) {
                let c = to_screen(p);
                if hovered_port == Some(id) {
                    // "+": click to add a node here.
                    painter.circle_filled(c, (PORT + 4.0) * z, t.accent);
                    let s = 4.0 * z;
                    painter.line_segment([c - vec2(s, 0.0), c + vec2(s, 0.0)], Stroke::new(2.0, t.on_accent));
                    painter.line_segment([c - vec2(0.0, s), c + vec2(0.0, s)], Stroke::new(2.0, t.on_accent));
                } else {
                    painter.circle_filled(c, PORT * z, port_color(&t, *kind));
                    if feeds_output == Some(id) {
                        // The end of the chain: what is exported.
                        painter.circle_stroke(c, (PORT + 3.0) * z, Stroke::new(1.5, t.accent));
                    }
                }
            }
        }

        // Hints and cursors.
        if hovered_port.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        } else if self.dragging.is_some() || self.panning {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if hovered_node.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        if self.view.nodes().count() == 1
            && let Some(p) = self.view.output_port(graph.source())
        {
            painter.text(
                to_screen(p) + vec2(16.0, 0.0),
                Align2::LEFT_CENTER,
                "Click the port to add a node",
                FontId::proportional(FONT[0]),
                t.text_weak,
            );
        }

        // Dragging nodes, panning.
        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started_by(PointerButton::Primary) {
            let at = press.or(response.interact_pointer_pos()).map(to_graph);
            match at.and_then(|p| self.view.node_at(p)) {
                Some(id) => {
                    self.dragging = Some(id);
                    actions.push(GraphAction::Select(id));
                }
                None => self.panning = true,
            }
        }
        if response.drag_started_by(PointerButton::Middle) {
            self.panning = true;
        }
        if response.dragged() {
            let d = response.drag_delta();
            if let Some(id) = self.dragging {
                self.view.move_node(id, [d.x / z, d.y / z]);
            } else if self.panning {
                self.view.pan[0] += d.x;
                self.view.pan[1] += d.y;
            }
        }
        if response.drag_stopped() {
            self.dragging = None;
            self.panning = false;
        }

        // Clicks: a port opens the tool list, a node is selected.
        if response.clicked() {
            let at = response.interact_pointer_pos();
            let gp = at.map(to_graph);
            if let (Some(p), Some(id)) = (at, gp.and_then(|p| self.view.output_port_at(graph, p, port_hit))) {
                self.add_menu = Some(AddMenu { after: id, at: p, fresh: true });
            } else if let Some(id) = gp.and_then(|p| self.view.node_at(p)) {
                actions.push(GraphAction::Select(id));
            } else {
                actions.push(GraphAction::Deselect);
            }
        }
        if response.secondary_clicked() {
            self.menu_node = response.interact_pointer_pos().map(to_graph).and_then(|p| self.view.node_at(p));
        }
        let menu_node = self.menu_node;
        response.context_menu(|ui| {
            let Some(id) = menu_node else {
                if ui.button("Show the Source").clicked() {
                    self.view.pan = [0.0, 0.0];
                    ui.close();
                }
                if ui.button("Zoom 100 %").clicked() {
                    self.view.zoom = 1.0;
                    ui.close();
                }
                return;
            };
            let Some(node) = graph.node(id) else { return };
            ui.label(RichText::new(node_title(node, registry)).strong());
            ui.separator();
            if node.kind.outputs().first() == Some(&PortKind::Image) {
                ui.menu_button("Add node after", |ui| {
                    for (_, tool, name) in node_tools(registry) {
                        if ui.button(name).clicked() {
                            actions.push(GraphAction::Add { after: id, tool });
                            ui.close();
                        }
                    }
                });
            }
            if matches!(node.kind, NodeKind::Tool(_) | NodeKind::Mask) {
                let label = if node.enabled { "Bypass" } else { "Enable" };
                if ui.button(label).clicked() {
                    actions.push(GraphAction::Enable(id, !node.enabled));
                    ui.close();
                }
                if ui.add(Button::new("Delete node")).clicked() {
                    actions.push(GraphAction::Remove(id));
                    ui.close();
                }
            }
        });

        self.add_menu(ui.ctx(), graph, registry, &mut actions);
        actions
    }

    /// The list of tools that opens on an output port.
    fn add_menu(
        &mut self,
        ctx: &egui::Context,
        graph: &NodeGraph,
        registry: &ToolRegistry,
        actions: &mut Vec<GraphAction>,
    ) {
        let Some(menu) = self.add_menu else { return };
        let Some(node) = graph.node(menu.after) else {
            self.add_menu = None;
            return;
        };
        let t = Tokens::of(ctx.theme());
        let mut chosen = None;
        let shown = egui::Area::new(Id::unique("rne-add-node"))
            .fixed_pos(menu.at + vec2(10.0, -10.0))
            .order(Order::Foreground)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(ADD_MENU_WIDTH);
                    ui.label(
                        RichText::new(format!("Add after {}", node_title(node, registry)))
                            .size(FONT[0])
                            .color(t.text_weak),
                    );
                    ui.add_space(SPACE[0]);
                    let mut last = None;
                    for (category, tool, name) in node_tools(registry) {
                        if last != Some(category) {
                            if last.is_some() {
                                ui.separator();
                            }
                            ui.label(RichText::new(category_name(category)).size(FONT[0]).color(t.text_weak));
                            last = Some(category);
                        }
                        let button = Button::new(name).frame(false).min_size(vec2(ADD_MENU_WIDTH, 0.0));
                        if ui.add(button).clicked() {
                            chosen = Some(tool);
                        }
                    }
                });
            });
        if let Some(tool) = chosen {
            actions.push(GraphAction::Add { after: menu.after, tool });
            self.add_menu = None;
        } else if ctx.input(|i| i.key_pressed(Key::Escape)) || (!menu.fresh && shown.response.clicked_elsewhere()) {
            self.add_menu = None;
        } else if let Some(m) = &mut self.add_menu {
            m.fresh = false;
        }
    }
}

fn port_color(t: &Tokens, kind: PortKind) -> Color32 {
    match kind {
        PortKind::Image => t.port_image,
        PortKind::Mask => t.port_mask,
    }
}

fn kind_color(t: &Tokens, kind: &NodeKind) -> Color32 {
    match kind {
        NodeKind::Source | NodeKind::Output => t.accent,
        NodeKind::Tool(_) | NodeKind::Parallel { .. } => t.port_image,
        NodeKind::Mask => t.port_mask,
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

/// A heading for a panel section.
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.add_space(SPACE[1]);
    ui.label(RichText::new(text).size(FONT[3]).strong());
    ui.add_space(SPACE[0]);
}
