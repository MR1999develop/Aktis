//! The Heal and Clone tools without the window: where the spots are,
//! where a new spot goes, its automatic source, and which circle is under
//! the pointer.
//!
//! Spots live in the retouch tool of a layer ([`tool_adjust::retouch`]),
//! in slots of at most [`MAX_SPOTS`] each: a new spot goes into the last
//! slot while it has room, then a new slot starts. Retouch acts first in a
//! layer, so the spots are taken from the image under the layer and the
//! layer's other tools adjust the copied texture like the rest.

use commands::EditError;
use core_model::{LayerId, ToolId, ToolRef};
use tool_adjust::retouch::{self, MAX_SPOTS, Spot};
use tool_api::{ResolvedParams, ToolRegistry};

use crate::Document;

/// Slot `slot` of the retouch tool of `layer`.
#[must_use]
pub fn retouch_at(layer: LayerId, slot: u16) -> ToolRef {
    ToolRef::new(layer, ToolId::new(retouch::ID).expect("constant id is valid")).with_slot(slot)
}

/// How many slots of spots `layer` holds (0 when it has no spot yet).
#[must_use]
pub fn slots(doc: &Document, layer: LayerId) -> u16 {
    let tool = ToolId::new(retouch::ID).expect("constant id is valid");
    let n = doc.recipe().layers.tools(layer).and_then(|t| t.get(&tool)).map_or(0, |s| s.slots().len());
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// The spots of one slot by index (`None` for an empty index); `None`
/// when the slot does not exist or the tool is missing from this build.
#[must_use]
pub fn spots(doc: &Document, registry: &ToolRegistry, at: &ToolRef) -> Option<[Option<Spot>; MAX_SPOTS]> {
    if at.tool.as_str() != retouch::ID {
        return None;
    }
    let params = doc.recipe().layers.params(at)?;
    let tool = registry.get(&at.tool)?;
    Some(retouch::spots(&ResolvedParams::resolve(tool.descriptor(), params)))
}

/// Every spot of `layer`: its slot, its index there, and the spot.
#[must_use]
pub fn all_spots(doc: &Document, registry: &ToolRegistry, layer: LayerId) -> Vec<(ToolRef, usize, Spot)> {
    (0..slots(doc, layer))
        .map(|slot| retouch_at(layer, slot))
        .filter_map(|at| spots(doc, registry, &at).map(|s| (at, s)))
        .flat_map(|(at, s)| s.into_iter().enumerate().filter_map(move |(i, x)| x.map(|x| (at.clone(), i, x))))
        .collect()
}

/// Where a spot added now to `layer` goes: the slot and the index there.
#[must_use]
pub fn target(doc: &Document, registry: &ToolRegistry, layer: LayerId) -> (ToolRef, usize) {
    let n = slots(doc, layer);
    if n > 0 {
        let last = retouch_at(layer, n - 1);
        if let Some(index) = spots(doc, registry, &last).and_then(|s| retouch::free_slot(&s)) {
            return (last, index);
        }
    }
    (retouch_at(layer, n), 0)
}

/// Adds `spot` to `layer` where [`target`] says (one undo step); returns
/// its slot and index.
///
/// # Errors
/// [`EditError`] when the retouch tool is missing from this build or the
/// layer does not exist; nothing changes.
pub fn add_spot(
    doc: &mut Document,
    registry: &ToolRegistry,
    layer: LayerId,
    spot: &Spot,
) -> Result<(ToolRef, usize), EditError> {
    let (at, index) = target(doc, registry, layer);
    doc.set_params(registry, &at, retouch::all_values(index, spot), None)?;
    Ok((at, index))
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

    #[test]
    fn spots_fill_the_last_slot_then_start_a_new_one() {
        let (mut doc, reg) = doc();
        let bg = LayerId::BACKGROUND;
        let (at, index) = add_spot(&mut doc, &reg, bg, &spot(0.1)).unwrap();
        assert_eq!((at.slot, index), (0, 0));
        for i in 1..MAX_SPOTS {
            #[allow(clippy::cast_precision_loss)] // a few spots
            let x = 0.1 + 0.05 * i as f32;
            assert_eq!(add_spot(&mut doc, &reg, bg, &spot(x)).unwrap(), (retouch_at(bg, 0), i));
        }
        let s = spots(&doc, &reg, &retouch_at(bg, 0)).unwrap();
        assert!(s.iter().all(Option::is_some));
        assert!((s[3].unwrap().at[0] - 0.25).abs() < 1e-6);
        // A full slot: the next spot starts slot 1.
        assert_eq!(add_spot(&mut doc, &reg, bg, &spot(0.9)).unwrap(), (retouch_at(bg, 1), 0));
        assert_eq!(slots(&doc, bg), 2);
        assert_eq!(all_spots(&doc, &reg, bg).len(), MAX_SPOTS + 1);
        // Each spot is one undo step: undoing removes the second slot.
        doc.undo();
        assert_eq!(slots(&doc, bg), 1);
        doc.undo();
        assert!(spots(&doc, &reg, &retouch_at(bg, 0)).unwrap()[MAX_SPOTS - 1].is_none());
        // A layer has its own spots.
        let layer = doc.add_layer(&reg, "L", core_model::MaskDef::default(), Vec::new()).unwrap();
        assert_eq!(add_spot(&mut doc, &reg, layer, &spot(0.5)).unwrap(), (retouch_at(layer, 0), 0));
        assert_eq!(spots(&doc, &reg, &ToolRef::background(ToolId::new("exposure").unwrap())), None);
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
