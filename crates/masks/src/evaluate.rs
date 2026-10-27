//! From a [`MaskDef`] to a mask image at the resolution being rendered.

use core_model::{ImageF32, MaskDef, MaskImage, MaskOp, MaskSource};

use crate::brush::rasterize;
use crate::filter::soft_blur;
use crate::refine::{Guide, refine};
use crate::resample::resize_plane;
use crate::store::RasterSource;

/// A mask ready to use, and what could not be built.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedMask {
    /// The mask, at the size of the guide image.
    pub mask: MaskImage,
    /// Indices of AI components whose raster is not computed (or not in the
    /// store). They counted as empty; the UI offers to compute them.
    pub missing: Vec<usize>,
}

/// The image as edge refinement sees it: sRGB-encoded colour, clipped to
/// 0–1, so edges are weighed as the eye sees them.
#[must_use]
pub fn guide_of(image: &ImageF32) -> Guide {
    let to_srgb = core_color::rgb_to_rgb(image.space(), core_model::WorkingSpace::LinearSrgb);
    let n = image.pixels().len();
    let mut rgb: [Vec<f32>; 3] = std::array::from_fn(|_| Vec::with_capacity(n));
    for p in image.pixels() {
        let v = to_srgb.apply([p[0], p[1], p[2]]);
        for (plane, x) in rgb.iter_mut().zip(v) {
            plane.push(core_color::srgb_encode(x.clamp(0.0, 1.0)));
        }
    }
    Guide::new(image.width() as usize, image.height() as usize, rgb)
}

/// Builds the mask of `def` at the size of `guide` (the developed image, or
/// its preview). Rasters come from `rasters`; missing ones count as empty and
/// are reported. The result is deterministic: the same inputs give the same
/// values on every machine, whichever renderer then uses them.
#[must_use]
pub fn evaluate(def: &MaskDef, rasters: &dyn RasterSource, guide: &ImageF32) -> EvaluatedMask {
    let (w, h) = (guide.width() as usize, guide.height() as usize);
    let mut missing = Vec::new();
    let mut guide_rgb: Option<Guide> = None;
    let mut acc: Option<Vec<f32>> = None;

    for (index, component) in def.components.iter().enumerate().filter(|(_, c)| c.enabled) {
        let values = match &component.source {
            MaskSource::Ai(ai) => match ai.raster.and_then(|k| rasters.raster(&k)) {
                None => {
                    missing.push(index);
                    vec![0.0; w * h]
                }
                Some(raster) => {
                    let (rw, rh) = (raster.width() as usize, raster.height() as usize);
                    let coarse = resize_plane(raster.values(), rw, rh, w, h);
                    if def.refine_edges {
                        refine(&coarse, guide_rgb.get_or_insert_with(|| guide_of(guide)))
                    } else {
                        coarse
                    }
                }
            },
            MaskSource::Brush(brush) => {
                let painted = rasterize(brush, w, h);
                if brush.edge_aware {
                    refine(&painted, guide_rgb.get_or_insert_with(|| guide_of(guide)))
                } else {
                    painted
                }
            }
        };
        acc = Some(match acc {
            None => values,
            Some(mut m) => {
                combine(&mut m, &values, component.op);
                m
            }
        });
    }

    let mut values = acc.unwrap_or_else(|| vec![0.0; w * h]);
    if def.feather > 0.0 {
        // The edge spans about ±2σ; σ is half the feather radius.
        let sigma = def.feather.clamp(0.0, core_model::MAX_FEATHER) * w.max(h) as f32 / 2.0;
        values = soft_blur(&values, w, h, sigma);
    }
    let opacity = def.opacity.clamp(0.0, 1.0);
    for v in &mut values {
        let x = if def.invert { 1.0 - *v } else { *v };
        *v = (x * opacity).clamp(0.0, 1.0);
    }
    // `w × h` comes from a valid image, so this cannot fail.
    let mask = MaskImage::from_values(guide.width(), guide.height(), values)
        .unwrap_or_else(|_| unreachable!("mask has the guide's size"));
    EvaluatedMask { mask, missing }
}

/// Fuzzy-set combination of `m` (the mask so far) with component `c`.
fn combine(m: &mut [f32], c: &[f32], op: MaskOp) {
    for (a, b) in m.iter_mut().zip(c) {
        *a = match op {
            MaskOp::Add => a.max(*b),
            MaskOp::Subtract => a.min(1.0 - b),
            MaskOp::Intersect => a.min(*b),
        };
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact values in tests
mod tests {
    use super::*;
    use crate::store::{MemoryRasters, NoRasters};
    use core_model::{AiTarget, MaskComponent, MaskSource, ModelId, RasterKey, WorkingSpace};

    fn grey(w: u32, h: u32) -> ImageF32 {
        ImageF32::from_pixels(w, h, WorkingSpace::LinearRec2020, vec![[0.18, 0.18, 0.18, 1.0]; (w * h) as usize])
            .expect("valid image")
    }

    /// A component whose raster is `left, right` on a 2×1 grid.
    fn component(store: &MemoryRasters, n: u8, op: MaskOp, left: f32, right: f32) -> MaskComponent {
        component_with(store, n, op, vec![left, right])
    }

    /// A component whose raster is one row of `values`.
    fn component_with(store: &MemoryRasters, n: u8, op: MaskOp, values: Vec<f32>) -> MaskComponent {
        let key = RasterKey::from_bytes([n; 16]);
        let w = u32::try_from(values.len()).expect("small");
        store.insert(key, MaskImage::from_values(w, 1, values).expect("valid"));
        let mut c = MaskComponent::ai(format!("c{n}"), op, AiTarget::Subject, ModelId::new("m").expect("id"));
        if let MaskSource::Ai(ai) = &mut c.source {
            ai.raster = Some(key);
        }
        c
    }

    fn def(components: Vec<MaskComponent>) -> MaskDef {
        MaskDef { components, refine_edges: false, ..MaskDef::default() }
    }

    #[test]
    fn an_empty_definition_selects_nothing() {
        let e = evaluate(&MaskDef::default(), &NoRasters, &grey(4, 2));
        assert!(e.mask.values().iter().all(|v| *v == 0.0));
        assert!(e.missing.is_empty());
    }

    #[test]
    fn components_combine_as_fuzzy_sets() {
        let s = MemoryRasters::new();
        let a = component(&s, 1, MaskOp::Add, 1.0, 0.0);
        let b = component(&s, 2, MaskOp::Add, 0.0, 0.6);
        let sub = component(&s, 3, MaskOp::Subtract, 0.25, 0.0);
        let int = component(&s, 4, MaskOp::Intersect, 1.0, 0.5);
        let img = grey(2, 1);
        assert_eq!(evaluate(&def(vec![a.clone(), b.clone()]), &s, &img).mask.values(), &[1.0, 0.6]);
        assert_eq!(evaluate(&def(vec![a.clone(), b.clone(), sub]), &s, &img).mask.values(), &[0.75, 0.6]);
        assert_eq!(evaluate(&def(vec![a, b, int]), &s, &img).mask.values(), &[1.0, 0.5]);
    }

    #[test]
    fn the_first_component_counts_as_add_and_disabled_ones_are_skipped() {
        let s = MemoryRasters::new();
        let first = component(&s, 1, MaskOp::Subtract, 0.4, 0.8);
        let mut off = component(&s, 2, MaskOp::Add, 1.0, 1.0);
        off.enabled = false;
        assert_eq!(evaluate(&def(vec![off, first]), &s, &grey(2, 1)).mask.values(), &[0.4, 0.8]);
    }

    #[test]
    fn invert_and_opacity_apply_to_the_whole_mask() {
        let s = MemoryRasters::new();
        let a = component(&s, 1, MaskOp::Add, 1.0, 0.0);
        let d = MaskDef { invert: true, opacity: 0.5, ..def(vec![a]) };
        assert_eq!(evaluate(&d, &s, &grey(2, 1)).mask.values(), &[0.0, 0.5]);
    }

    #[test]
    fn a_missing_raster_counts_as_empty_and_is_reported() {
        let s = MemoryRasters::new();
        let present = component(&s, 1, MaskOp::Add, 1.0, 1.0);
        let absent = MaskComponent::ai("later", MaskOp::Add, AiTarget::Subject, ModelId::new("m").expect("id"));
        let e = evaluate(&def(vec![absent, present]), &s, &grey(2, 1));
        assert_eq!(e.missing, vec![0]);
        assert_eq!(e.mask.values(), &[1.0, 1.0]);
    }

    #[test]
    fn rasters_are_scaled_to_the_rendered_size() {
        let s = MemoryRasters::new();
        let a = component(&s, 1, MaskOp::Add, 0.0, 1.0);
        let e = evaluate(&def(vec![a]), &s, &grey(8, 4));
        assert_eq!((e.mask.width(), e.mask.height()), (8, 4));
        let row = &e.mask.values()[..8];
        assert_eq!((row[0], row[7]), (0.0, 1.0));
        assert!(row.windows(2).all(|p| p[0] <= p[1]), "left to right ramp: {row:?}");
    }

    #[test]
    fn a_brush_adds_to_an_ai_selection_and_an_eraser_takes_away() {
        use core_model::{BrushMask, BrushStroke, NormPoint};
        let s = MemoryRasters::new();
        let ai = component_with(&s, 1, MaskOp::Add, (0..20).map(|i| if i < 10 { 1.0 } else { 0.0 }).collect());
        let stroke = |x: f32| BrushStroke {
            points: vec![NormPoint::new(x, 0.5).expect("finite")],
            radius: 0.1,
            hardness: 1.0,
            opacity: 1.0,
        };
        let paint =
            MaskComponent::brush("Paint", MaskOp::Add, BrushMask { strokes: vec![stroke(0.9)], edge_aware: false });
        let erase = MaskComponent::brush(
            "Erase",
            MaskOp::Subtract,
            BrushMask { strokes: vec![stroke(0.1)], edge_aware: false },
        );
        let e = evaluate(&def(vec![ai, paint, erase]), &s, &grey(20, 10));
        let row = &e.mask.values()[5 * 20..6 * 20];
        assert_eq!((row[1], row[8], row[18], row[12]), (0.0, 1.0, 1.0, 0.0), "{row:?}");
        assert!(e.missing.is_empty());
    }

    #[test]
    fn feather_softens_a_hard_edge() {
        let s = MemoryRasters::new();
        let a = component_with(&s, 1, MaskOp::Add, (0..20).map(|i| if i < 10 { 0.0 } else { 1.0 }).collect());
        let img = grey(200, 2);
        let sharp = evaluate(&def(vec![a.clone()]), &s, &img).mask;
        let soft = evaluate(&MaskDef { feather: 0.05, ..def(vec![a]) }, &s, &img).mask;
        let spread = |m: &MaskImage| m.values()[..200].iter().filter(|v| **v > 0.02 && **v < 0.98).count();
        assert!(spread(&soft) > spread(&sharp) + 10, "{} vs {}", spread(&soft), spread(&sharp));
    }
}
