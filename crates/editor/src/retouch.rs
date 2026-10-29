//! The Heal and Clone tools without the window: which node holds the
//! spots, where a new spot goes, its automatic source, and which circle
//! is under the pointer.
//!
//! Spots live in retouch nodes ([`tool_adjust::retouch`]), at most
//! [`MAX_SPOTS`] each. A new spot goes into the selected retouch node when
//! it has room. Otherwise a new retouch node is made: right after the
//! selected retouch node when that one is full (so the order of the spots
//! is kept), else right after the Source, so the spots are taken from the
//! developed image and every later node adjusts the copied texture like
//! the rest of the photo.

use commands::EditError;
use core_model::{NodeId, NodeKind, Params, ToolId};
use tool_adjust::retouch::{self, MAX_SPOTS, Spot};
use tool_api::{ResolvedParams, ToolRegistry};

use crate::Document;

/// Whether `node` is a retouch node.
#[must_use]
pub fn is_retouch(doc: &Document, node: NodeId) -> bool {
    doc.recipe().graph.node(node).is_some_and(|n| matches!(&n.kind, NodeKind::Tool(t) if t.as_str() == retouch::ID))
}

/// The spots of `node` by slot (`None` for an empty slot); `None` when it
/// is not a retouch node or the tool is missing from this build.
#[must_use]
pub fn spots(doc: &Document, registry: &ToolRegistry, node: NodeId) -> Option<[Option<Spot>; MAX_SPOTS]> {
    if !is_retouch(doc, node) {
        return None;
    }
    let n = doc.recipe().graph.node(node)?;
    let tool = registry.get(&ToolId::new(retouch::ID).ok()?)?;
    Some(retouch::spots(&ResolvedParams::resolve(tool.descriptor(), &n.params)))
}

/// Where a new spot goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// An empty slot of an existing retouch node.
    Slot {
        /// The node.
        node: NodeId,
        /// The slot.
        slot: usize,
    },
    /// The first slot of a new retouch node, made right after `after`.
    NewNode {
        /// The node the new one follows.
        after: NodeId,
    },
}

/// Where a spot added now goes, with `selected` the selected node (see
/// the module notes).
#[must_use]
pub fn target(doc: &Document, registry: &ToolRegistry, selected: Option<NodeId>) -> Target {
    if let Some(node) = selected
        && let Some(s) = spots(doc, registry, node)
    {
        return match retouch::free_slot(&s) {
            Some(slot) => Target::Slot { node, slot },
            None => Target::NewNode { after: node },
        };
    }
    Target::NewNode { after: doc.recipe().graph.source() }
}

/// Adds `spot` where [`target`] says (one undo step); returns its node and slot.
///
/// # Errors
/// [`EditError`] when the retouch tool is missing from this build; nothing changes.
pub fn add_spot(
    doc: &mut Document,
    registry: &ToolRegistry,
    selected: Option<NodeId>,
    spot: &Spot,
) -> Result<(NodeId, usize), EditError> {
    match target(doc, registry, selected) {
        Target::Slot { node, slot } => {
            doc.set_params(registry, node, retouch::all_values(slot, spot), None)?;
            Ok((node, slot))
        }
        Target::NewNode { after } => {
            let tool = ToolId::new(retouch::ID).expect("constant id is valid");
            let mut params = Params::new();
            for (id, v) in retouch::all_values(0, spot) {
                params.set(id, v);
            }
            let node = doc.add_node_with(registry, after, tool, params)?;
            Ok((node, 0))
        }
    }
}

/// The source for a new spot at `at` (developed image, 0–1) with
/// `radius` (fraction of the long side), searched in the developed preview.
#[must_use]
pub fn auto_source(doc: &Document, at: [f32; 2], radius: f32) -> [f32; 2] {
    retouch::find_source(doc.preview(), at, radius)
}

/// Which circle of a spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The spot itself.
    Spot,
    /// Its source.
    Source,
}

/// A spot as drawn on screen: centres and radius in screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circles {
    /// The spot's slot.
    pub slot: usize,
    /// Centre of the spot, or `None` when it is not shown.
    pub spot: Option<[f32; 2]>,
    /// Centre of the source, or `None` when it is not shown.
    pub source: Option<[f32; 2]>,
    /// Radius of both.
    pub radius: f32,
}

/// The circle under `p`: within its radius (at least `slack` points, so
/// small circles can be caught), the one whose centre is nearest; on a tie
/// the later spot, drawn on top.
#[must_use]
pub fn hit(circles: &[Circles], p: [f32; 2], slack: f32) -> Option<(usize, Part)> {
    let mut best: Option<(f32, usize, Part)> = None;
    for c in circles {
        for (centre, part) in [(c.spot, Part::Spot), (c.source, Part::Source)] {
            let Some(centre) = centre else { continue };
            let d = (p[0] - centre[0]).hypot(p[1] - centre[1]);
            if d <= c.radius.max(slack) && best.is_none_or(|(b, _, _)| d <= b) {
                best = Some((d, c.slot, part));
            }
        }
    }
    best.map(|(_, slot, part)| (slot, part))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tool_adjust::retouch::SpotKind;

    fn doc() -> (Document, ToolRegistry) {
        let img = Arc::new(core_model::ImageF32::new(32, 24, core_model::WorkingSpace::LinearRec2020).unwrap());
        let opened = crate::Opened {
            path: "a/b.NEF".into(),
            camera: "Nikon D3S".into(),
            orientation: core_model::Orientation::Normal,
            has_camera_white_balance: true,
            raw: None,
            detected_lens: None,
            lens_choices: Vec::new(),
            lens_key: None,
            info: raw_io::PhotoInfo::default(),
            camera_curve: None,
            image: crate::DevelopedImage {
                settings: core_model::DevelopSettings::default(),
                developed: Arc::clone(&img),
                preview: Arc::clone(&img),
                draft: img,
                lens: None,
                notes: Vec::new(),
                profile: String::new(),
            },
        };
        (Document::new(opened, 1), engine::builtin_tools().unwrap())
    }

    fn spot(x: f32) -> Spot {
        Spot { kind: SpotKind::Heal, at: [x, 0.5], radius: 0.02, source: [x, 0.2], feather: 0.5, opacity: 1.0 }
    }

    /// Tool ids of the chain from the Source to the Output.
    fn chain(doc: &Document) -> Vec<String> {
        let graph = &doc.recipe().graph;
        graph
            .evaluation_order()
            .unwrap()
            .into_iter()
            .filter_map(|id| match &graph.node(id)?.kind {
                NodeKind::Tool(t) => Some(t.as_str().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_first_spot_makes_a_retouch_node_after_the_source_and_later_ones_fill_it() {
        let (mut doc, reg) = doc();
        let source = doc.recipe().graph.source();
        let exposure = doc.add_node(&reg, source, ToolId::new("exposure").unwrap()).unwrap();
        // With another node selected, the retouch node goes right after the Source.
        let (node, slot) = add_spot(&mut doc, &reg, Some(exposure), &spot(0.1)).unwrap();
        assert_eq!(slot, 0);
        assert_eq!(chain(&doc), ["retouch", "exposure"]);
        assert!(is_retouch(&doc, node) && !is_retouch(&doc, exposure));
        // With the retouch node selected, the next spots go into it.
        for i in 1..MAX_SPOTS {
            #[allow(clippy::cast_precision_loss)] // a few spots
            let x = 0.1 + 0.05 * i as f32;
            assert_eq!(add_spot(&mut doc, &reg, Some(node), &spot(x)).unwrap(), (node, i));
        }
        let s = spots(&doc, &reg, node).unwrap();
        assert!(s.iter().all(Option::is_some));
        assert!((s[3].unwrap().at[0] - 0.25).abs() < 1e-6);
        // A full node gets a new one right after it.
        let (next, slot) = add_spot(&mut doc, &reg, Some(node), &spot(0.9)).unwrap();
        assert_ne!(next, node);
        assert_eq!(slot, 0);
        assert_eq!(chain(&doc), ["retouch", "retouch", "exposure"]);
        assert_eq!(doc.recipe().graph.input_link(next, 0).map(|l| l.from), Some(node));
        // Each spot is one undo step: undoing removes the second node.
        doc.undo();
        assert_eq!(chain(&doc), ["retouch", "exposure"]);
        doc.undo();
        assert!(spots(&doc, &reg, node).unwrap()[MAX_SPOTS - 1].is_none());
        assert_eq!(spots(&doc, &reg, exposure), None);
    }

    #[test]
    fn the_nearest_circle_under_the_pointer_is_hit() {
        let circles = [
            Circles { slot: 0, spot: Some([100.0, 100.0]), source: Some([160.0, 100.0]), radius: 20.0 },
            Circles { slot: 4, spot: Some([120.0, 100.0]), source: None, radius: 20.0 },
            Circles { slot: 5, spot: Some([300.0, 300.0]), source: Some([310.0, 300.0]), radius: 2.0 },
        ];
        assert_eq!(hit(&circles, [104.0, 100.0], 6.0), Some((0, Part::Spot)));
        assert_eq!(hit(&circles, [116.0, 100.0], 6.0), Some((4, Part::Spot)));
        assert_eq!(hit(&circles, [150.0, 100.0], 6.0), Some((0, Part::Source)));
        assert_eq!(hit(&circles, [200.0, 200.0], 6.0), None);
        // Small circles are caught within the slack.
        assert_eq!(hit(&circles, [304.0, 300.0], 6.0), Some((5, Part::Spot)));
        assert_eq!(hit(&circles, [309.0, 303.0], 6.0), Some((5, Part::Source)));
    }

    #[test]
    fn the_automatic_source_is_away_from_the_spot() {
        let (doc, _) = doc();
        let s = auto_source(&doc, [0.5, 0.5], 0.05);
        let d = (s[0] - 0.5).hypot(s[1] - 0.5);
        assert!(d >= 0.1, "{s:?}");
    }
}
