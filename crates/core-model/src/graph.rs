//! The free node graph that follows the fixed development stage.
//!
//! The graph always contains exactly one `Source` (the developed image) and
//! one `Output`. Every edit keeps it acyclic and keeps each input port fed by
//! at most one link, so evaluation never has to handle invalid graphs.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use crate::ids::{NodeId, ToolId};
use crate::mask::MaskDef;
use crate::params::Params;

/// What travels on a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    /// An RGBA image (green ports in the UI).
    Image,
    /// A single-channel mask (blue "key" ports in the UI).
    Mask,
}

/// The kinds of node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// The developed image, linear working space. Exactly one per graph.
    Source,
    /// The end of the graph. Exactly one per graph.
    Output,
    /// A node running one registered tool (exposure, curves, …).
    Tool(ToolId),
    /// Adds the effects of several branches that start from the same image.
    Parallel {
        /// Number of image inputs, at least 2.
        inputs: u8,
    },
    /// Builds a mask (AI selection, and later brush, gradients, colour range)
    /// and offers it on its key output. Its definition is the node's
    /// [`Node::mask`]. It reads the developed image itself, so it has no input.
    Mask,
}

impl NodeKind {
    /// Input ports in order.
    #[must_use]
    pub fn inputs(&self) -> Vec<PortKind> {
        match self {
            Self::Source | Self::Mask => vec![],
            Self::Output => vec![PortKind::Image],
            Self::Tool(_) => vec![PortKind::Image, PortKind::Mask],
            Self::Parallel { inputs } => vec![PortKind::Image; usize::from(*inputs)],
        }
    }

    /// Output ports in order.
    #[must_use]
    pub fn outputs(&self) -> Vec<PortKind> {
        match self {
            Self::Output => vec![],
            Self::Source | Self::Tool(_) | Self::Parallel { .. } => vec![PortKind::Image],
            Self::Mask => vec![PortKind::Mask],
        }
    }
}

/// One node of the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Identifier, unique in this graph.
    pub id: NodeId,
    /// What the node does.
    pub kind: NodeKind,
    /// Parameter values (tool nodes).
    pub params: Params,
    /// A disabled node passes its image input through unchanged.
    pub enabled: bool,
    /// Name shown above the node; empty means "use the tool's name".
    pub label: String,
    /// The mask definition; `Some` exactly for [`NodeKind::Mask`] nodes.
    pub mask: Option<MaskDef>,
}

/// A connection from an output port to an input port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Link {
    /// Node producing the value.
    pub from: NodeId,
    /// Output port index on `from`.
    pub from_port: u8,
    /// Node consuming the value.
    pub to: NodeId,
    /// Input port index on `to`.
    pub to_port: u8,
}

/// Why a graph edit was refused. The graph is unchanged when an error is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// No node with this id.
    UnknownNode(NodeId),
    /// The port index does not exist on that node.
    NoSuchPort(NodeId, u8),
    /// An image port connected to a mask port or vice versa.
    PortKindMismatch,
    /// The link would create a loop.
    Cycle,
    /// Source and Output cannot be removed or duplicated.
    ProtectedNode(NodeId),
    /// A parallel node needs at least two inputs.
    TooFewInputs,
    /// Output is not reachable from Source through image links.
    OutputNotConnected,
    /// A graph read back from storage breaks a rule (the text says which).
    Malformed(&'static str),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode(id) => write!(f, "node {id} does not exist"),
            Self::NoSuchPort(id, p) => write!(f, "node {id} has no port {p}"),
            Self::PortKindMismatch => {
                write!(f, "an image port can only connect to an image port, a mask port to a mask port")
            }
            Self::Cycle => write!(f, "this connection would create a loop"),
            Self::ProtectedNode(id) => write!(f, "node {id} is the source or the output and cannot change"),
            Self::TooFewInputs => write!(f, "a parallel node needs at least two inputs"),
            Self::OutputNotConnected => write!(f, "the output is not connected to the source"),
            Self::Malformed(why) => write!(f, "the stored graph is not valid: {why}"),
        }
    }
}

impl std::error::Error for GraphError {}

/// The node graph.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeGraph {
    nodes: BTreeMap<NodeId, Node>,
    links: BTreeSet<Link>,
    next_id: u32,
    source: NodeId,
    output: NodeId,
}

impl Default for NodeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeGraph {
    /// A graph holding only Source → Output.
    #[must_use]
    pub fn new() -> Self {
        let source = NodeId(0);
        let output = NodeId(1);
        let mut nodes = BTreeMap::new();
        for (id, kind) in [(source, NodeKind::Source), (output, NodeKind::Output)] {
            nodes.insert(id, Node { id, kind, params: Params::new(), enabled: true, label: String::new(), mask: None });
        }
        let mut links = BTreeSet::new();
        links.insert(Link { from: source, from_port: 0, to: output, to_port: 0 });
        Self { nodes, links, next_id: 2, source, output }
    }

    /// Rebuilds a graph read back from storage, keeping its node ids, after
    /// checking every rule the edits keep: one Source and one Output, unique
    /// ids below `next_id`, a mask definition exactly on mask nodes, links
    /// between existing ports of the same kind, one link per input, no loop.
    ///
    /// # Errors
    /// [`GraphError`] naming the first rule broken.
    pub fn from_parts(nodes: Vec<Node>, links: Vec<Link>, next_id: u32) -> Result<Self, GraphError> {
        let mut map = BTreeMap::new();
        let (mut source, mut output) = (None, None);
        for node in nodes {
            if node.id.0 >= next_id {
                return Err(GraphError::Malformed("a node id is not below the next free id"));
            }
            match node.kind {
                NodeKind::Source if source.replace(node.id).is_some() => {
                    return Err(GraphError::Malformed("more than one source"));
                }
                NodeKind::Output if output.replace(node.id).is_some() => {
                    return Err(GraphError::Malformed("more than one output"));
                }
                NodeKind::Parallel { inputs } if inputs < 2 => return Err(GraphError::TooFewInputs),
                _ => {}
            }
            if node.mask.is_some() != (node.kind == NodeKind::Mask) {
                return Err(GraphError::Malformed("a mask definition belongs to mask nodes only"));
            }
            if map.insert(node.id, node).is_some() {
                return Err(GraphError::Malformed("two nodes share an id"));
            }
        }
        let (Some(source), Some(output)) = (source, output) else {
            return Err(GraphError::Malformed("the source or the output is missing"));
        };
        let mut graph = Self { nodes: map, links: BTreeSet::new(), next_id, source, output };
        for link in links {
            if graph.input_link(link.to, link.to_port).is_some() {
                return Err(GraphError::Malformed("an input is fed by two links"));
            }
            // `connect` checks nodes, ports, kinds and loops.
            graph.connect(link)?;
        }
        Ok(graph)
    }

    /// The id the next added node will get (stored with the graph so ids
    /// are never reused).
    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// The Source node's id.
    #[must_use]
    pub fn source(&self) -> NodeId {
        self.source
    }

    /// The Output node's id.
    #[must_use]
    pub fn output(&self) -> NodeId {
        self.output
    }

    /// A node by id.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// A node by id, mutable. Only fields that cannot break the graph's
    /// invariants are reachable this way (kind is fixed after creation).
    pub fn node_mut(&mut self, id: NodeId) -> Option<NodeMut<'_>> {
        self.nodes.get_mut(&id).map(NodeMut)
    }

    /// All nodes in id order.
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// All links.
    pub fn links(&self) -> impl Iterator<Item = &Link> {
        self.links.iter()
    }

    /// The link feeding `to`'s input `port`, if any.
    #[must_use]
    pub fn input_link(&self, to: NodeId, port: u8) -> Option<Link> {
        self.links.iter().find(|l| l.to == to && l.to_port == port).copied()
    }

    /// Adds a node with no links and returns its id.
    ///
    /// # Errors
    /// [`GraphError::ProtectedNode`] for `Source`/`Output`,
    /// [`GraphError::TooFewInputs`] for a parallel node with fewer than 2 inputs.
    pub fn add_node(&mut self, kind: NodeKind, params: Params) -> Result<NodeId, GraphError> {
        match kind {
            NodeKind::Source | NodeKind::Output => return Err(GraphError::ProtectedNode(NodeId(self.next_id))),
            NodeKind::Parallel { inputs } if inputs < 2 => return Err(GraphError::TooFewInputs),
            NodeKind::Tool(_) | NodeKind::Parallel { .. } | NodeKind::Mask => {}
        }
        let mask = (kind == NodeKind::Mask).then(MaskDef::default);
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, Node { id, kind, params, enabled: true, label: String::new(), mask });
        Ok(id)
    }

    /// Adds a mask node with the given definition and no links; returns its id.
    pub fn add_mask_node(&mut self, def: MaskDef) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let node = Node {
            id,
            kind: NodeKind::Mask,
            params: Params::new(),
            enabled: true,
            label: String::new(),
            mask: Some(def),
        };
        self.nodes.insert(id, node);
        id
    }

    /// Adds a tool node right after `after` on its image output, the way
    /// "Add serial node" works: everything that `after` fed now receives the
    /// new node's output instead.
    ///
    /// # Errors
    /// [`GraphError::UnknownNode`], [`GraphError::ProtectedNode`] when `after`
    /// is the Output, [`GraphError::PortKindMismatch`] when `after` gives a mask.
    pub fn insert_serial(&mut self, after: NodeId, tool: ToolId, params: Params) -> Result<NodeId, GraphError> {
        let after_kind = &self.nodes.get(&after).ok_or(GraphError::UnknownNode(after))?.kind;
        match after_kind.outputs().first() {
            None => return Err(GraphError::ProtectedNode(after)),
            Some(PortKind::Mask) => return Err(GraphError::PortKindMismatch),
            Some(PortKind::Image) => {}
        }
        let id = self.add_node(NodeKind::Tool(tool), params)?;
        let downstream: Vec<Link> =
            self.links.iter().filter(|l| l.from == after && l.from_port == 0).copied().collect();
        for l in downstream {
            self.links.remove(&l);
            self.links.insert(Link { from: id, ..l });
        }
        self.links.insert(Link { from: after, from_port: 0, to: id, to_port: 0 });
        Ok(id)
    }

    /// Removes a node and its links. When the node had one image input and its
    /// image output fed other nodes, those are reconnected to its input, so a
    /// serial chain stays connected.
    ///
    /// # Errors
    /// [`GraphError::UnknownNode`] or [`GraphError::ProtectedNode`].
    pub fn remove_node(&mut self, id: NodeId) -> Result<Node, GraphError> {
        if id == self.source || id == self.output {
            return Err(GraphError::ProtectedNode(id));
        }
        if !self.nodes.contains_key(&id) {
            return Err(GraphError::UnknownNode(id));
        }
        let upstream = self.input_link(id, 0);
        let downstream: Vec<Link> = self.links.iter().filter(|l| l.from == id).copied().collect();
        self.links.retain(|l| l.from != id && l.to != id);
        if let Some(up) = upstream {
            for d in downstream.iter().filter(|d| d.from_port == 0) {
                self.links.insert(Link { from: up.from, from_port: up.from_port, ..*d });
            }
        }
        self.nodes.remove(&id).ok_or(GraphError::UnknownNode(id))
    }

    /// Connects two ports. A link already feeding the target input is replaced.
    ///
    /// # Errors
    /// [`GraphError`] when a node or port does not exist, the port kinds differ,
    /// or the link would create a cycle. The graph is unchanged on error.
    pub fn connect(&mut self, link: Link) -> Result<Option<Link>, GraphError> {
        let from = self.nodes.get(&link.from).ok_or(GraphError::UnknownNode(link.from))?;
        let to = self.nodes.get(&link.to).ok_or(GraphError::UnknownNode(link.to))?;
        let out_kind = *from
            .kind
            .outputs()
            .get(usize::from(link.from_port))
            .ok_or(GraphError::NoSuchPort(link.from, link.from_port))?;
        let in_kind =
            *to.kind.inputs().get(usize::from(link.to_port)).ok_or(GraphError::NoSuchPort(link.to, link.to_port))?;
        if out_kind != in_kind {
            return Err(GraphError::PortKindMismatch);
        }
        if link.from == link.to || self.reaches(link.to, link.from) {
            return Err(GraphError::Cycle);
        }
        let replaced = self.input_link(link.to, link.to_port);
        if let Some(old) = replaced {
            self.links.remove(&old);
        }
        self.links.insert(link);
        Ok(replaced)
    }

    /// Removes the link feeding `to`'s input `port`; returns it.
    pub fn disconnect(&mut self, to: NodeId, port: u8) -> Option<Link> {
        let link = self.input_link(to, port)?;
        self.links.remove(&link);
        Some(link)
    }

    /// Whether `from` can reach `to` following links downstream.
    fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([from]);
        while let Some(n) = queue.pop_front() {
            if n == to {
                return true;
            }
            if seen.insert(n) {
                queue.extend(self.links.iter().filter(|l| l.from == n).map(|l| l.to));
            }
        }
        false
    }

    /// Nodes that contribute to the Output, in an order where every node comes
    /// after the nodes feeding it. Source is first, Output last.
    ///
    /// # Errors
    /// [`GraphError::OutputNotConnected`] when no image path joins Source to Output.
    pub fn evaluation_order(&self) -> Result<Vec<NodeId>, GraphError> {
        // Nodes upstream of Output.
        let mut needed = BTreeSet::new();
        let mut stack = vec![self.output];
        while let Some(n) = stack.pop() {
            if needed.insert(n) {
                stack.extend(self.links.iter().filter(|l| l.to == n).map(|l| l.from));
            }
        }
        if !needed.contains(&self.source) {
            return Err(GraphError::OutputNotConnected);
        }
        // Kahn's algorithm restricted to the needed nodes; ties broken by id for a stable order.
        let mut indegree: BTreeMap<NodeId, usize> = needed.iter().map(|n| (*n, 0)).collect();
        for l in self.links.iter().filter(|l| needed.contains(&l.from) && needed.contains(&l.to)) {
            *indegree.entry(l.to).or_default() += 1;
        }
        let mut ready: BTreeSet<NodeId> = indegree.iter().filter(|(_, d)| **d == 0).map(|(n, _)| *n).collect();
        let mut order = Vec::with_capacity(needed.len());
        while let Some(n) = ready.pop_first() {
            order.push(n);
            for l in self.links.iter().filter(|l| l.from == n && needed.contains(&l.to)) {
                if let Some(d) = indegree.get_mut(&l.to) {
                    *d -= 1;
                    if *d == 0 {
                        ready.insert(l.to);
                    }
                }
            }
        }
        // Edits refuse cycles, so every needed node is ordered.
        assert_eq!(order.len(), needed.len(), "node graph invariant broken: cycle among needed nodes");
        Ok(order)
    }
}

/// Mutable access to the editable fields of a node.
#[derive(Debug)]
pub struct NodeMut<'a>(&'a mut Node);

impl NodeMut<'_> {
    /// The node's parameters.
    pub fn params(&mut self) -> &mut Params {
        &mut self.0.params
    }

    /// Enables or bypasses the node; returns the previous state.
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        std::mem::replace(&mut self.0.enabled, enabled)
    }

    /// Renames the node; returns the previous label.
    pub fn set_label(&mut self, label: String) -> String {
        std::mem::replace(&mut self.0.label, label)
    }

    /// The mask definition of a mask node; `None` for other kinds.
    pub fn mask(&mut self) -> Option<&mut MaskDef> {
        self.0.mask.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> NodeKind {
        NodeKind::Tool(ToolId::new(name).expect("valid id"))
    }

    fn exposure() -> ToolId {
        ToolId::new("exposure").expect("valid id")
    }

    #[test]
    fn a_new_graph_evaluates_source_then_output() {
        let g = NodeGraph::new();
        assert_eq!(g.evaluation_order(), Ok(vec![g.source(), g.output()]));
    }

    #[test]
    fn serial_nodes_are_evaluated_in_chain_order() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert a");
        let b = g.insert_serial(a, exposure(), Params::new()).expect("insert b");
        assert_eq!(g.evaluation_order(), Ok(vec![g.source(), a, b, g.output()]));
        assert_eq!(g.input_link(g.output(), 0).map(|l| l.from), Some(b));
    }

    #[test]
    fn removing_a_middle_node_keeps_the_chain_connected() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert a");
        let b = g.insert_serial(a, exposure(), Params::new()).expect("insert b");
        g.remove_node(a).expect("remove a");
        assert_eq!(g.evaluation_order(), Ok(vec![g.source(), b, g.output()]));
    }

    #[test]
    fn a_link_that_closes_a_loop_is_refused_and_changes_nothing() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert a");
        let b = g.insert_serial(a, exposure(), Params::new()).expect("insert b");
        let before = g.clone();
        let r = g.connect(Link { from: b, from_port: 0, to: a, to_port: 0 });
        assert_eq!(r, Err(GraphError::Cycle));
        assert_eq!(g, before);
    }

    #[test]
    fn an_image_output_cannot_feed_a_mask_input() {
        let mut g = NodeGraph::new();
        let a = g.add_node(tool("exposure"), Params::new()).expect("add");
        let r = g.connect(Link { from: g.source(), from_port: 0, to: a, to_port: 1 });
        assert_eq!(r, Err(GraphError::PortKindMismatch));
    }

    #[test]
    fn connecting_an_input_replaces_its_previous_link() {
        let mut g = NodeGraph::new();
        let a = g.add_node(tool("exposure"), Params::new()).expect("add");
        g.connect(Link { from: g.source(), from_port: 0, to: a, to_port: 0 }).expect("first link");
        let old = g.connect(Link { from: a, from_port: 0, to: g.output(), to_port: 0 }).expect("second link");
        assert_eq!(old.map(|l| l.from), Some(g.source()));
        assert_eq!(g.links().filter(|l| l.to == g.output()).count(), 1);
    }

    #[test]
    fn source_and_output_are_protected() {
        let mut g = NodeGraph::new();
        assert_eq!(g.remove_node(g.source()).map(|_| ()), Err(GraphError::ProtectedNode(g.source())));
        assert!(g.add_node(NodeKind::Output, Params::new()).is_err());
        assert_eq!(g.add_node(NodeKind::Parallel { inputs: 1 }, Params::new()), Err(GraphError::TooFewInputs));
    }

    #[test]
    fn a_disconnected_output_is_reported() {
        let mut g = NodeGraph::new();
        g.disconnect(g.output(), 0);
        assert_eq!(g.evaluation_order(), Err(GraphError::OutputNotConnected));
    }

    #[test]
    fn a_mask_node_feeds_the_key_input_of_a_tool_and_is_evaluated_before_it() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert");
        let m = g.add_mask_node(MaskDef::default());
        assert_eq!(g.node(m).and_then(|n| n.mask.clone()), Some(MaskDef::default()));
        assert_eq!(g.connect(Link { from: m, from_port: 0, to: a, to_port: 0 }), Err(GraphError::PortKindMismatch));
        g.connect(Link { from: m, from_port: 0, to: a, to_port: 1 }).expect("mask link");
        let order = g.evaluation_order().expect("order");
        let pos = |id| order.iter().position(|n| *n == id).expect("present");
        assert!(pos(m) < pos(a));
        assert_eq!(g.insert_serial(m, exposure(), Params::new()), Err(GraphError::PortKindMismatch));
    }

    #[test]
    fn removing_a_mask_node_frees_the_key_input_and_keeps_the_chain() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert");
        let m = g.add_node(NodeKind::Mask, Params::new()).expect("add mask");
        assert!(g.node(m).is_some_and(|n| n.mask.is_some()));
        g.connect(Link { from: m, from_port: 0, to: a, to_port: 1 }).expect("mask link");
        g.remove_node(m).expect("remove");
        assert_eq!(g.input_link(a, 1), None);
        assert_eq!(g.evaluation_order(), Ok(vec![g.source(), a, g.output()]));
    }

    #[test]
    fn only_mask_nodes_have_a_mask_definition() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert");
        assert!(g.node_mut(a).expect("exists").mask().is_none());
        let m = g.add_mask_node(MaskDef::default());
        g.node_mut(m).expect("exists").mask().expect("mask node").invert = true;
        assert!(g.node(m).and_then(|n| n.mask.as_ref()).is_some_and(|d| d.invert));
    }

    #[test]
    fn a_graph_rebuilt_from_its_parts_is_identical_and_keeps_its_next_id() {
        let mut g = NodeGraph::new();
        let a = g.insert_serial(g.source(), exposure(), Params::new()).expect("insert");
        let m = g.add_mask_node(MaskDef::default());
        g.connect(Link { from: m, from_port: 0, to: a, to_port: 1 }).expect("mask link");
        g.remove_node(m).expect("remove");
        let nodes: Vec<Node> = g.nodes().cloned().collect();
        let links: Vec<Link> = g.links().copied().collect();
        let back = NodeGraph::from_parts(nodes, links, g.next_id()).expect("valid");
        assert_eq!(back, g);
        assert_eq!(back.next_id(), 4, "the id of the removed mask is not reused");
    }

    #[test]
    fn broken_stored_graphs_are_refused() {
        let g = NodeGraph::new();
        let nodes: Vec<Node> = g.nodes().cloned().collect();
        let links: Vec<Link> = g.links().copied().collect();
        let only_source = vec![nodes[0].clone()];
        assert!(matches!(NodeGraph::from_parts(only_source, vec![], 2), Err(GraphError::Malformed(_))));
        let twice = vec![nodes[0].clone(), nodes[1].clone(), nodes[1].clone()];
        assert!(matches!(NodeGraph::from_parts(twice, vec![], 2), Err(GraphError::Malformed(_))));
        assert!(matches!(NodeGraph::from_parts(nodes.clone(), links.clone(), 1), Err(GraphError::Malformed(_))));
        let mut masked = nodes.clone();
        masked[0].mask = Some(MaskDef::default());
        assert!(matches!(NodeGraph::from_parts(masked, vec![], 2), Err(GraphError::Malformed(_))));
        let mut a = nodes.clone();
        a.push(Node {
            id: NodeId(2),
            kind: tool("exposure"),
            params: Params::new(),
            enabled: true,
            label: String::new(),
            mask: None,
        });
        let looped = vec![Link { from: NodeId(2), from_port: 0, to: NodeId(2), to_port: 0 }];
        assert_eq!(NodeGraph::from_parts(a.clone(), looped, 3), Err(GraphError::Cycle));
        let doubled = vec![
            Link { from: NodeId(0), from_port: 0, to: NodeId(1), to_port: 0 },
            Link { from: NodeId(2), from_port: 0, to: NodeId(1), to_port: 0 },
        ];
        assert!(matches!(NodeGraph::from_parts(a.clone(), doubled, 3), Err(GraphError::Malformed(_))));
        let wrong_port = vec![Link { from: NodeId(0), from_port: 0, to: NodeId(2), to_port: 1 }];
        assert_eq!(NodeGraph::from_parts(a, wrong_port, 3), Err(GraphError::PortKindMismatch));
    }

    #[test]
    fn nodes_not_feeding_the_output_are_not_evaluated() {
        let mut g = NodeGraph::new();
        let stray = g.add_node(tool("exposure"), Params::new()).expect("add");
        g.connect(Link { from: g.source(), from_port: 0, to: stray, to_port: 0 }).expect("link");
        assert!(!g.evaluation_order().expect("order").contains(&stray));
    }
}
