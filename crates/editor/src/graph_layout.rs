//! Where each node of the graph is drawn: in columns from left to right
//! following the links, the image chain on the first row and mask nodes
//! below, just before the node that uses them.

use std::collections::BTreeMap;

use core_model::{NodeGraph, NodeId, NodeKind};

/// The cell of one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodePlace {
    /// The node.
    pub node: NodeId,
    /// Column, 0 = leftmost.
    pub column: usize,
    /// Row within the column, 0 = top.
    pub row: usize,
}

/// Places every node of `graph`. A graph with a cycle (refused by the
/// graph, so a bug) is laid out in id order on one row.
#[must_use]
pub fn layout(graph: &NodeGraph) -> Vec<NodePlace> {
    let Ok(order) = graph.evaluation_order() else {
        return graph.nodes().enumerate().map(|(column, n)| NodePlace { node: n.id, column, row: 0 }).collect();
    };
    let is_mask = |id: NodeId| graph.node(id).is_some_and(|n| matches!(n.kind, NodeKind::Mask));

    // Image nodes: one column after the deepest image input.
    let mut column: BTreeMap<NodeId, usize> = BTreeMap::new();
    for &id in order.iter().filter(|&&id| !is_mask(id)) {
        let c = graph
            .links()
            .filter(|l| l.to == id && !is_mask(l.from))
            .filter_map(|l| column.get(&l.from).map(|c| c + 1))
            .max()
            .unwrap_or(0);
        column.insert(id, c);
    }
    // Mask nodes: the column before their first user (or the first one).
    for &id in order.iter().filter(|&&id| is_mask(id)) {
        let c = graph
            .links()
            .filter(|l| l.from == id)
            .filter_map(|l| column.get(&l.to).copied())
            .min()
            .map_or(0, |c| c.saturating_sub(1));
        column.insert(id, c);
    }
    // Rows: image nodes first, in evaluation order, then masks.
    let mut used: BTreeMap<usize, usize> = BTreeMap::new();
    let mut places = Vec::with_capacity(order.len());
    for pass_masks in [false, true] {
        for &id in order.iter().filter(|&&id| is_mask(id) == pass_masks) {
            let c = column.get(&id).copied().unwrap_or(0);
            let row = used.entry(c).or_insert(0);
            // Masks never share the image row.
            if pass_masks && *row == 0 {
                *row = 1;
            }
            places.push(NodePlace { node: id, column: c, row: *row });
            *row += 1;
        }
    }
    places
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use core_model::{Link, MaskDef, Params, ToolId};

    use super::*;

    fn tool(name: &str) -> ToolId {
        ToolId::new(name).unwrap()
    }

    fn place(places: &[NodePlace], id: NodeId) -> (usize, usize) {
        places.iter().find(|p| p.node == id).map(|p| (p.column, p.row)).unwrap()
    }

    #[test]
    fn a_serial_chain_is_one_row_left_to_right() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), tool("exposure"), Params::new()).unwrap();
        let b = g.insert_serial(a, tool("denoise_ai"), Params::new()).unwrap();
        let places = layout(&g);
        assert_eq!(places.len(), 4);
        assert_eq!(place(&places, g.source()), (0, 0));
        assert_eq!(place(&places, a), (1, 0));
        assert_eq!(place(&places, b), (2, 0));
        assert_eq!(place(&places, g.output()), (3, 0));
    }

    #[test]
    fn a_mask_sits_below_just_before_its_user() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), tool("exposure"), Params::new()).unwrap();
        let b = g.insert_serial(a, tool("exposure"), Params::new()).unwrap();
        let m = g.add_mask_node(MaskDef::default());
        g.connect(Link { from: m, from_port: 0, to: b, to_port: 1 }).unwrap();
        let places = layout(&g);
        assert_eq!(place(&places, b), (2, 0));
        assert_eq!(place(&places, m), (1, 1));
        // No two nodes share a cell.
        for (i, p) in places.iter().enumerate() {
            assert!(places[i + 1..].iter().all(|q| (q.column, q.row) != (p.column, p.row)));
        }
    }
}
