//! Where the nodes sit in the node graph panel, and what is under the
//! pointer.
//!
//! Positions are kept per node in graph coordinates (points, the panel's
//! top-left corner at the origin before panning). A node gets a position
//! the first time it is seen: right of the node feeding it, the nodes
//! after it moved right to make room; then the user drags it anywhere.
//! The Output node is not drawn: the end of the chain feeds it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use core_model::{NodeGraph, NodeId, NodeKind};

/// Size of a node box.
pub const NODE_SIZE: [f32; 2] = [132.0, 46.0];
/// From one column of nodes to the next, and from one row to the next.
pub const NODE_STEP: [f32; 2] = [NODE_SIZE[0] + 46.0, NODE_SIZE[1] + 24.0];
/// Where the Source starts.
pub const ORIGIN: [f32; 2] = [24.0, 24.0];
/// Radius (screen points) within which a click hits a port.
pub const PORT_HIT: f32 = 9.0;

/// Smallest and largest zoom of the node graph.
pub const ZOOM_RANGE: [f32; 2] = [0.3, 2.5];

/// Node positions, panning and zoom of the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphView {
    positions: BTreeMap<NodeId, [f32; 2]>,
    /// Offset of the graph origin from the panel's top-left corner, in
    /// screen points: screen = panel corner + pan + zoom × graph point.
    pub pan: [f32; 2],
    /// Screen points per graph point.
    pub zoom: f32,
}

impl Default for GraphView {
    fn default() -> Self {
        Self { positions: BTreeMap::new(), pan: [0.0, 0.0], zoom: 1.0 }
    }
}

/// Whether a node is drawn (the Output is not).
#[must_use]
pub fn is_drawn(graph: &NodeGraph, id: NodeId) -> bool {
    graph.node(id).is_some_and(|n| !matches!(n.kind, NodeKind::Output))
}

fn overlaps(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < NODE_SIZE[0] + 8.0 && (a[1] - b[1]).abs() < NODE_SIZE[1] + 8.0
}

impl GraphView {
    /// Forgets every position (another photo is open).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The positions, pan and zoom, to store with the edit.
    #[must_use]
    pub fn layout(&self) -> recipe_store::Layout {
        recipe_store::Layout { nodes: self.nodes().collect(), pan: self.pan, zoom: self.zoom }
    }

    /// A view with stored positions, pan and zoom. Positions that are not
    /// finite are dropped (the node is placed again), a pan that is not
    /// finite goes back to 0 and the zoom is kept in [`ZOOM_RANGE`].
    #[must_use]
    pub fn from_layout(layout: &recipe_store::Layout) -> Self {
        let finite = |p: [f32; 2]| p.iter().all(|v| v.is_finite());
        let positions = layout.nodes.iter().filter(|(_, p)| finite(*p)).copied().collect();
        let pan = if finite(layout.pan) { layout.pan } else { [0.0, 0.0] };
        let zoom = if layout.zoom.is_finite() { layout.zoom.clamp(ZOOM_RANGE[0], ZOOM_RANGE[1]) } else { 1.0 };
        Self { positions, pan, zoom }
    }

    /// Top-left corner of a node, in graph coordinates.
    #[must_use]
    pub fn position(&self, id: NodeId) -> Option<[f32; 2]> {
        self.positions.get(&id).copied()
    }

    /// Every placed node with its position.
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, [f32; 2])> + '_ {
        self.positions.iter().map(|(id, p)| (*id, *p))
    }

    /// Places the nodes seen for the first time and forgets removed ones.
    pub fn sync(&mut self, graph: &NodeGraph) {
        self.positions.retain(|id, _| is_drawn(graph, *id));
        let order = graph.evaluation_order().unwrap_or_else(|_| graph.nodes().map(|n| n.id).collect());
        let is_mask = |id: NodeId| graph.node(id).is_some_and(|n| matches!(n.kind, NodeKind::Mask));
        // Image nodes first (in processing order), then masks next to their users.
        for pass_masks in [false, true] {
            for &id in order.iter().filter(|&&id| is_mask(id) == pass_masks) {
                if !is_drawn(graph, id) || self.positions.contains_key(&id) {
                    continue;
                }
                let wanted = if pass_masks { self.mask_spot(graph, id) } else { self.chain_spot(graph, id) };
                if !pass_masks {
                    self.make_room(graph, id, wanted);
                }
                let spot = self.free_spot(wanted);
                self.positions.insert(id, spot);
            }
        }
    }

    /// Right of the node feeding `id`, or the origin.
    fn chain_spot(&self, graph: &NodeGraph, id: NodeId) -> [f32; 2] {
        graph
            .input_link(id, 0)
            .and_then(|l| self.positions.get(&l.from))
            .map_or(ORIGIN, |p| [p[0] + NODE_STEP[0], p[1]])
    }

    /// Below and a little left of the first node using the mask.
    fn mask_spot(&self, graph: &NodeGraph, id: NodeId) -> [f32; 2] {
        graph
            .links()
            .filter(|l| l.from == id)
            .find_map(|l| self.positions.get(&l.to))
            .map_or([ORIGIN[0], ORIGIN[1] + NODE_STEP[1]], |p| [p[0] - NODE_STEP[0] * 0.5, p[1] + NODE_STEP[1]])
    }

    /// Moves right, by one column, the placed nodes downstream of `id`
    /// when one of them sits where `id` goes.
    fn make_room(&mut self, graph: &NodeGraph, id: NodeId, spot: [f32; 2]) {
        let downstream = downstream_of(graph, id);
        let blocked = downstream.iter().filter_map(|d| self.positions.get(d)).any(|p| overlaps(*p, spot));
        if blocked {
            for d in downstream {
                if let Some(p) = self.positions.get_mut(&d) {
                    p[0] += NODE_STEP[0];
                }
            }
        }
    }

    /// `wanted`, or the first free place below it.
    fn free_spot(&self, wanted: [f32; 2]) -> [f32; 2] {
        let mut spot = wanted;
        for _ in 0..64 {
            if !self.positions.values().any(|p| overlaps(*p, spot)) {
                break;
            }
            spot[1] += NODE_STEP[1];
        }
        spot
    }

    /// Zooms by `factor` keeping the graph point under `anchor` (screen
    /// points from the panel's corner) where it is.
    pub fn zoom_at(&mut self, factor: f32, anchor: [f32; 2]) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let new = (self.zoom * factor).clamp(ZOOM_RANGE[0], ZOOM_RANGE[1]);
        let k = new / self.zoom;
        for (pan, a) in self.pan.iter_mut().zip(anchor) {
            *pan = a - (a - *pan) * k;
        }
        self.zoom = new;
    }

    /// Graph point under a screen point (from the panel's corner).
    #[must_use]
    pub fn to_graph(&self, screen: [f32; 2]) -> [f32; 2] {
        [(screen[0] - self.pan[0]) / self.zoom, (screen[1] - self.pan[1]) / self.zoom]
    }

    /// Screen point (from the panel's corner) of a graph point.
    #[must_use]
    pub fn to_screen(&self, graph: [f32; 2]) -> [f32; 2] {
        [self.pan[0] + graph[0] * self.zoom, self.pan[1] + graph[1] * self.zoom]
    }

    /// Moves a node by `delta` (graph coordinates).
    pub fn move_node(&mut self, id: NodeId, delta: [f32; 2]) {
        if let Some(p) = self.positions.get_mut(&id) {
            p[0] += delta[0];
            p[1] += delta[1];
        }
    }

    /// The node under graph point `p`, the one drawn last when they overlap.
    #[must_use]
    pub fn node_at(&self, p: [f32; 2]) -> Option<NodeId> {
        self.positions
            .iter()
            .filter(|(_, q)| p[0] >= q[0] && p[0] <= q[0] + NODE_SIZE[0] && p[1] >= q[1] && p[1] <= q[1] + NODE_SIZE[1])
            .map(|(id, _)| *id)
            .next_back()
    }

    /// The output port of a node: the middle of its right edge.
    #[must_use]
    pub fn output_port(&self, id: NodeId) -> Option<[f32; 2]> {
        self.positions.get(&id).map(|p| [p[0] + NODE_SIZE[0], p[1] + NODE_SIZE[1] / 2.0])
    }

    /// Input port `k` of `n` on the left edge of a node.
    #[must_use]
    pub fn input_port(&self, id: NodeId, k: usize, n: usize) -> Option<[f32; 2]> {
        #[allow(clippy::cast_precision_loss)] // two or three ports
        let y = |p: [f32; 2]| p[1] + NODE_SIZE[1] * (k + 1) as f32 / (n + 1) as f32;
        self.positions.get(&id).map(|p| [p[0], y(*p)])
    }

    /// The node whose output port is within `radius` (graph units) of graph point `p`.
    #[must_use]
    pub fn output_port_at(&self, graph: &NodeGraph, p: [f32; 2], radius: f32) -> Option<NodeId> {
        self.positions.keys().copied().find(|&id| {
            let has_output = graph.node(id).is_some_and(|n| !n.kind.outputs().is_empty());
            has_output
                && self
                    .output_port(id)
                    .is_some_and(|q| (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) <= radius * radius)
        })
    }

    /// The bottom-right corner of the drawing, for the scroll extent.
    #[must_use]
    pub fn extent(&self) -> [f32; 2] {
        self.positions.values().fold([0.0, 0.0], |acc, p| {
            [acc[0].max(p[0] + NODE_SIZE[0] + ORIGIN[0]), acc[1].max(p[1] + NODE_SIZE[1] + ORIGIN[1])]
        })
    }
}

/// Nodes reached from `id` through links, Output excluded.
fn downstream_of(graph: &NodeGraph, id: NodeId) -> Vec<NodeId> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([id]);
    while let Some(n) = queue.pop_front() {
        for l in graph.links().filter(|l| l.from == n) {
            if is_drawn(graph, l.to) && seen.insert(l.to) {
                queue.push_back(l.to);
            }
        }
    }
    seen.into_iter().collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use core_model::{Link, MaskDef, Params, ToolId};

    use super::*;

    fn tool() -> ToolId {
        ToolId::new("exposure").unwrap()
    }

    #[test]
    fn a_new_photo_shows_only_the_source() {
        let g = NodeGraph::new();
        let mut v = GraphView::default();
        v.sync(&g);
        let nodes: Vec<_> = v.nodes().collect();
        assert_eq!(nodes, vec![(g.source(), ORIGIN)]);
        assert_eq!(v.output_port(g.source()), Some([ORIGIN[0] + NODE_SIZE[0], ORIGIN[1] + NODE_SIZE[1] / 2.0]));
    }

    #[test]
    fn a_node_added_after_another_goes_to_its_right_and_pushes_the_rest() {
        let mut g = NodeGraph::new();
        let mut v = GraphView::default();
        let a = g.insert_serial(g.source(), tool(), Params::new()).unwrap();
        v.sync(&g);
        assert_eq!(v.position(a), Some([ORIGIN[0] + NODE_STEP[0], ORIGIN[1]]));
        // Inserted between the Source and a: a moves one column right.
        let b = g.insert_serial(g.source(), tool(), Params::new()).unwrap();
        v.sync(&g);
        assert_eq!(v.position(b), Some([ORIGIN[0] + NODE_STEP[0], ORIGIN[1]]));
        assert_eq!(v.position(a), Some([ORIGIN[0] + 2.0 * NODE_STEP[0], ORIGIN[1]]));
        // A node moved away by the user is not pushed.
        v.move_node(a, [0.0, 300.0]);
        let c = g.insert_serial(b, tool(), Params::new()).unwrap();
        v.sync(&g);
        assert_eq!(v.position(c), Some([ORIGIN[0] + 2.0 * NODE_STEP[0], ORIGIN[1]]));
        assert_eq!(v.position(a), Some([ORIGIN[0] + 2.0 * NODE_STEP[0], ORIGIN[1] + 300.0]));
        // Removed nodes are forgotten.
        g.remove_node(c).unwrap();
        v.sync(&g);
        assert!(v.position(c).is_none());
    }

    #[test]
    fn hits_find_nodes_and_output_ports() {
        let mut g = NodeGraph::new();
        let mut v = GraphView::default();
        let a = g.insert_serial(g.source(), tool(), Params::new()).unwrap();
        v.sync(&g);
        let pa = v.position(a).unwrap();
        assert_eq!(v.node_at([pa[0] + 5.0, pa[1] + 5.0]), Some(a));
        assert_eq!(v.node_at([pa[0] - 20.0, pa[1] - 20.0]), None);
        let port = v.output_port(g.source()).unwrap();
        assert_eq!(v.output_port_at(&g, [port[0] + 3.0, port[1] - 3.0], PORT_HIT), Some(g.source()));
        assert_eq!(v.output_port_at(&g, [port[0] + 30.0, port[1]], PORT_HIT), None);
        v.move_node(a, [10.0, 20.0]);
        assert_eq!(v.position(a), Some([pa[0] + 10.0, pa[1] + 20.0]));
        assert!(v.extent()[0] >= pa[0] + 10.0 + NODE_SIZE[0]);
    }

    #[test]
    fn masks_sit_below_their_user_without_overlapping() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), tool(), Params::new()).unwrap();
        let m = g.add_mask_node(MaskDef::default());
        g.connect(Link { from: m, from_port: 0, to: a, to_port: 1 }).unwrap();
        let mut v = GraphView::default();
        v.sync(&g);
        let (pa, pm) = (v.position(a).unwrap(), v.position(m).unwrap());
        assert!(pm[1] > pa[1]);
        assert!(!overlaps(pa, pm) && !overlaps(v.position(g.source()).unwrap(), pm));
    }

    #[test]
    fn a_view_round_trips_through_its_stored_layout_and_bad_values_are_fixed() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), tool(), Params::new()).unwrap();
        let mut v = GraphView::default();
        v.sync(&g);
        v.move_node(a, [5.0, 40.0]);
        v.pan = [12.0, -3.0];
        v.zoom = 1.5;
        assert_eq!(GraphView::from_layout(&v.layout()), v);

        let bad = recipe_store::Layout {
            nodes: vec![(a, [f32::NAN, 1.0]), (g.source(), [1.0, 2.0])],
            pan: [f32::INFINITY, 0.0],
            zoom: 40.0,
        };
        let fixed = GraphView::from_layout(&bad);
        assert_eq!(fixed.position(a), None);
        assert_eq!(fixed.position(g.source()), Some([1.0, 2.0]));
        assert_eq!((fixed.pan, fixed.zoom), ([0.0, 0.0], ZOOM_RANGE[1]));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let mut v = GraphView::default();
        let anchor = [200.0, 80.0];
        let before = v.to_graph(anchor);
        v.zoom_at(1.5, anchor);
        assert!((v.zoom - 1.5).abs() < 1e-6);
        let after = v.to_graph(anchor);
        assert!((before[0] - after[0]).abs() < 1e-4 && (before[1] - after[1]).abs() < 1e-4);
        let back = v.to_screen(after);
        assert!((back[0] - anchor[0]).abs() < 1e-3);
        v.zoom_at(100.0, anchor);
        assert!((v.zoom - ZOOM_RANGE[1]).abs() < 1e-6, "clamped");
        v.zoom_at(f32::NAN, anchor);
        assert!((v.zoom - ZOOM_RANGE[1]).abs() < 1e-6);
        v.reset();
        assert!((v.zoom - 1.0).abs() < 1e-6);
    }
}
