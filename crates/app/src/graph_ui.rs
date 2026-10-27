//! The node graph under the viewer (click a node to edit it on the right)
//! and the histogram above the settings.

use core_model::{Node, NodeGraph, NodeId, NodeKind, PortKind};
use editor::Histogram;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    epaint::CubicBezierShape, pos2, vec2,
};
use tool_api::ToolRegistry;

use crate::theme::{FONT, SPACE, Tokens};

/// Size of a node box.
const NODE_W: f32 = 132.0;
const NODE_H: f32 = 46.0;
/// Space between columns and rows of nodes.
const GAP_X: f32 = 46.0;
const GAP_Y: f32 = 22.0;
/// Space around the nodes.
const MARGIN: f32 = 18.0;
/// Spacing of the background grid.
const GRID: f32 = 20.0;
/// Radius of a port.
const PORT: f32 = 4.0;

/// What a click in the graph asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphClick {
    /// Show this node's settings.
    Select(NodeId),
    /// Click on the background: show none.
    Deselect,
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

/// Draws the graph; returns what a click asked for.
pub fn graph(
    ui: &mut egui::Ui,
    graph: &NodeGraph,
    registry: &ToolRegistry,
    selected: Option<NodeId>,
) -> Option<GraphClick> {
    let t = Tokens::of(ui.ctx().theme());
    let places = editor::layout(graph);
    let columns = places.iter().map(|p| p.column + 1).max().unwrap_or(1);
    let rows = places.iter().map(|p| p.row + 1).max().unwrap_or(1);
    #[allow(clippy::cast_precision_loss)] // a handful of columns and rows
    let content = vec2(
        2.0 * MARGIN + columns as f32 * (NODE_W + GAP_X) - GAP_X,
        2.0 * MARGIN + rows as f32 * (NODE_H + GAP_Y) - GAP_Y,
    );
    let mut click = None;
    egui::ScrollArea::both().id_salt("node-graph").auto_shrink([false, false]).show(ui, |ui| {
        let size = content.max(ui.available_size());
        let (response, painter) = ui.allocate_painter(size, Sense::click());
        let area = response.rect;

        // Background grid.
        let grid = Stroke::new(1.0, t.graph_grid);
        let mut x = area.left() + GRID;
        while x < area.right() {
            painter.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], grid);
            x += GRID;
        }
        let mut y = area.top() + GRID;
        while y < area.bottom() {
            painter.line_segment([pos2(area.left(), y), pos2(area.right(), y)], grid);
            y += GRID;
        }

        #[allow(clippy::cast_precision_loss)] // a handful of columns and rows
        let rect_of = |column: usize, row: usize| {
            Rect::from_min_size(
                area.min + vec2(MARGIN + column as f32 * (NODE_W + GAP_X), MARGIN + row as f32 * (NODE_H + GAP_Y)),
                vec2(NODE_W, NODE_H),
            )
        };
        let rect_for = |id: NodeId| places.iter().find(|p| p.node == id).map(|p| rect_of(p.column, p.row));

        // Links behind the nodes.
        for link in graph.links() {
            let (Some(from), Some(to), Some(to_node)) = (rect_for(link.from), rect_for(link.to), graph.node(link.to))
            else {
                continue;
            };
            let inputs = to_node.kind.inputs();
            let kind = inputs.get(usize::from(link.to_port)).copied().unwrap_or(PortKind::Image);
            let start = pos2(from.right(), from.center().y);
            let end = port_pos(to, usize::from(link.to_port), inputs.len());
            let bend = ((end.x - start.x).abs() * 0.5).max(30.0);
            let color = port_color(&t, kind);
            painter.add(CubicBezierShape::from_points_stroke(
                [start, start + vec2(bend, 0.0), end - vec2(bend, 0.0), end],
                false,
                Color32::TRANSPARENT,
                Stroke::new(2.0, color),
            ));
        }

        // Nodes.
        let hover = response.hover_pos();
        let mut over_node = None;
        for place in &places {
            let Some(node) = graph.node(place.node) else { continue };
            let rect = rect_of(place.column, place.row);
            let is_selected = selected == Some(node.id);
            let hovered = hover.is_some_and(|p| rect.contains(p));
            if hovered {
                over_node = Some(node.id);
            }
            let off = matches!(node.kind, NodeKind::Tool(_)) && !node.enabled;
            let fill = if hovered { t.bg_overlay } else { t.bg_raised };
            let stroke = if is_selected {
                Stroke::new(2.0, t.accent)
            } else {
                Stroke::new(1.0, if hovered { t.stroke_strong } else { t.stroke_weak })
            };
            painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
            // A coloured strip on top: what kind of node it is.
            let strip = Rect::from_min_size(rect.min + vec2(1.0, 1.0), vec2(rect.width() - 2.0, 3.0));
            painter.rect_filled(strip, CornerRadius::same(2), kind_color(&t, &node.kind));
            let title_color = if off { t.text_weak } else { t.text_strong };
            let title = node_title(node, registry);
            painter.with_clip_rect(rect.shrink(4.0)).text(
                rect.left_top() + vec2(10.0, 10.0),
                Align2::LEFT_TOP,
                title,
                FontId::proportional(FONT[1]),
                title_color,
            );
            painter.text(
                rect.left_top() + vec2(10.0, 28.0),
                Align2::LEFT_TOP,
                node_subtitle(node),
                FontId::proportional(FONT[0]),
                t.text_weak,
            );
            // Ports.
            let inputs = node.kind.inputs();
            for (k, kind) in inputs.iter().enumerate() {
                painter.circle_filled(port_pos(rect, k, inputs.len()), PORT, port_color(&t, *kind));
            }
            if let Some(kind) = node.kind.outputs().first() {
                painter.circle_filled(pos2(rect.right(), rect.center().y), PORT, port_color(&t, *kind));
            }
        }
        if over_node.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if response.clicked() {
            click = Some(over_node.map_or(GraphClick::Deselect, GraphClick::Select));
        }
    });
    click
}

/// Where input port `k` of `n` sits on the left edge of a node.
fn port_pos(rect: Rect, k: usize, n: usize) -> Pos2 {
    #[allow(clippy::cast_precision_loss)] // two or three ports
    let y = rect.top() + rect.height() * (k + 1) as f32 / (n + 1) as f32;
    pos2(rect.left(), y)
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
    let channels = [
        (&h.red, Color32::from_rgba_unmultiplied(230, 80, 70, 90)),
        (&h.green, Color32::from_rgba_unmultiplied(90, 200, 90, 90)),
        (&h.blue, Color32::from_rgba_unmultiplied(80, 130, 240, 90)),
    ];
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
