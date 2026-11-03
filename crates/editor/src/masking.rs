//! The editing logic of the Masks (brush) and AI Select tools: how a
//! painted stroke or a click changes a layer's mask, and the mask as an
//! overlay on the photo. The drawing and the pointer are in the app.

use core_model::{
    AiTarget, BrushMask, BrushStroke, Geometry, ImageF32, LinearGradient, MaskComponent, MaskDef, MaskOp, MaskSource,
    NormPoint, Orientation, PromptPoint, RadialGradient,
};

/// The brush of the Masks tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    /// Radius, as a fraction of the image's long side.
    pub radius: f32,
    /// Share of the radius painted at full strength.
    pub hardness: f32,
    /// Strength of a stroke, 0–1.
    pub opacity: f32,
    /// Strokes follow the photo's edges.
    pub edge_aware: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Self { radius: 0.03, hardness: 0.5, opacity: 1.0, edge_aware: false }
    }
}

/// `def` (or a new mask) with a stroke along `points` added: painting adds
/// to the mask, erasing subtracts. Consecutive strokes of the same kind go
/// into the same component.
#[must_use]
pub fn add_stroke(def: Option<&MaskDef>, points: Vec<NormPoint>, brush: Brush, erase: bool) -> MaskDef {
    let mut def = def.cloned().unwrap_or_default();
    let stroke =
        BrushStroke { points, radius: brush.radius, hardness: brush.hardness, opacity: brush.opacity }.sanitized();
    let op = if erase { MaskOp::Subtract } else { MaskOp::Add };
    match def.components.last_mut() {
        Some(MaskComponent { op: last_op, source: MaskSource::Brush(b), enabled: true, .. })
            if *last_op == op && b.edge_aware == brush.edge_aware =>
        {
            b.strokes.push(stroke);
        }
        _ => {
            let label = if erase { "Eraser" } else { "Brush" };
            def.components.push(MaskComponent::brush(
                label,
                op,
                BrushMask { strokes: vec![stroke], edge_aware: brush.edge_aware },
            ));
        }
    }
    def.sanitized()
}

/// Share of a new radial gradient's radius over which it fades.
pub const RADIAL_FEATHER: f32 = 0.5;

/// `def` (or a new mask) with a linear gradient added: full at `start`,
/// nothing at `end`; `subtract` takes it away from the mask instead.
#[must_use]
pub fn add_linear(def: Option<&MaskDef>, start: NormPoint, end: NormPoint, subtract: bool) -> MaskDef {
    let mut def = def.cloned().unwrap_or_default();
    let op = if subtract { MaskOp::Subtract } else { MaskOp::Add };
    def.components.push(MaskComponent::linear("Linear gradient", op, LinearGradient { start, end }));
    def.sanitized()
}

/// `def` (or a new mask) with a radial gradient added at `centre` with
/// `radius` (fractions of the long side); `subtract` takes it away.
#[must_use]
pub fn add_radial(def: Option<&MaskDef>, centre: NormPoint, radius: [f32; 2], subtract: bool) -> MaskDef {
    let mut def = def.cloned().unwrap_or_default();
    let op = if subtract { MaskOp::Subtract } else { MaskOp::Add };
    let g = RadialGradient { centre, radius, feather: RADIAL_FEATHER };
    def.components.push(MaskComponent::radial("Radial gradient", op, g));
    def.sanitized()
}

/// The last enabled gradient of `def` (the one the handles move), with its index.
#[must_use]
pub fn last_gradient(def: &MaskDef) -> Option<(usize, MaskSource)> {
    def.components
        .iter()
        .enumerate()
        .rev()
        .find(|(_, c)| c.enabled && matches!(c.source, MaskSource::Linear(_) | MaskSource::Radial(_)))
        .map(|(i, c)| (i, c.source.clone()))
}

/// `def` with component `index` taking its values from `source` (a moved
/// gradient); unchanged when there is no such component.
#[must_use]
pub fn with_source(def: &MaskDef, index: usize, source: MaskSource) -> MaskDef {
    let mut def = def.clone();
    if let Some(c) = def.components.get_mut(index) {
        c.source = source;
    }
    def.sanitized()
}

/// The stroke that covers a whole image: along the top and bottom edges
/// with the largest hard brush (half the long side), which reaches the
/// middle from both edges whatever the shape of the photo.
#[must_use]
pub fn fill_stroke() -> BrushStroke {
    let p = |x, y| NormPoint::new(x, y).unwrap_or_else(|| unreachable!("the corners are inside the image"));
    BrushStroke {
        points: vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)],
        radius: core_model::MAX_BRUSH_RADIUS,
        hardness: 1.0,
        opacity: 1.0,
    }
}

/// Fill Mask, as in Capture One: `def` (or a new mask) selecting the whole
/// image, ready to be erased from; feather and opacity are kept, what was
/// painted or selected is replaced, invert is cleared.
#[must_use]
pub fn fill(def: Option<&MaskDef>) -> MaskDef {
    let def = def.cloned().unwrap_or_default();
    let brush = BrushMask { strokes: vec![fill_stroke()], edge_aware: false };
    MaskDef { components: vec![MaskComponent::brush("Fill", MaskOp::Add, brush)], invert: false, ..def }.sanitized()
}

/// `def` (or a new mask) with a click of AI Select at `at`: added to the
/// last object's clicks (`include` false marks something to leave out), or
/// a new object when `new_object` or the last component is not one. The
/// object's raster is cleared, to be computed again.
#[must_use]
pub fn add_click(def: Option<&MaskDef>, at: NormPoint, include: bool, new_object: bool) -> MaskDef {
    let mut def = def.cloned().unwrap_or_default();
    let click = PromptPoint { at, include };
    if !new_object
        && let Some(MaskComponent { source: MaskSource::Ai(ai), .. }) = def.components.last_mut()
        && let AiTarget::Object { points, .. } = &mut ai.target
    {
        points.push(click);
        ai.raster = None;
        return def;
    }
    let n = def
        .components
        .iter()
        .filter(|c| matches!(&c.source, MaskSource::Ai(a) if matches!(a.target, AiTarget::Object { .. })))
        .count();
    let label = format!("Object {}", n + 1);
    def.components.push(ai_segment::object(label, MaskOp::Add, vec![click], None));
    def
}

/// `def` (or a new mask) with "Select subject" added.
#[must_use]
pub fn add_subject(def: Option<&MaskDef>) -> MaskDef {
    let mut def = def.cloned().unwrap_or_default();
    def.components.push(ai_segment::subject(MaskOp::Add));
    def
}

/// `current` with the rasters `computed` found for the same targets filled
/// in (the user may have changed the mask while the model ran).
#[must_use]
pub fn fill_rasters(current: &MaskDef, computed: &MaskDef) -> MaskDef {
    let mut out = current.clone();
    for c in &mut out.components {
        let MaskSource::Ai(ai) = &mut c.source else { continue };
        if ai.raster.is_some() {
            continue;
        }
        let found = computed.components.iter().find_map(|d| match &d.source {
            MaskSource::Ai(x) if x.target == ai.target && x.model == ai.model => x.raster,
            _ => None,
        });
        ai.raster = found;
    }
    out
}

/// The mask over the photo as shown: values (0–1) `width × height`, upright,
/// with the recipe's geometry. `mask` is in the developed image's orientation.
///
/// # Errors
/// A message when the sizes are impossible (never in practice).
pub fn shown_mask(
    mask: &core_model::MaskImage,
    orientation: Orientation,
    geometry: &Geometry,
    cropped: bool,
) -> Result<(u32, u32, Vec<f32>), String> {
    let upright = mask.oriented(orientation).map_err(|e| e.to_string())?;
    if geometry.is_identity() || (!geometry.warps() && !cropped) {
        let (w, h) = (upright.width(), upright.height());
        return Ok((w, h, upright.into_values()));
    }
    let px = upright.values().iter().map(|&v| [v, v, v, 1.0]).collect();
    let img = ImageF32::from_pixels(upright.width(), upright.height(), core_model::WorkingSpace::LinearRec2020, px)
        .map_err(|e| e.to_string())?;
    let shown = engine::geometry::apply(&img, geometry, cropped).map_err(|e| e.to_string())?;
    Ok((shown.width(), shown.height(), shown.pixels().iter().map(|p| p[0]).collect()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> NormPoint {
        NormPoint::new(x, y).unwrap()
    }

    #[test]
    fn gradients_are_added_found_and_moved() {
        let def = add_linear(None, p(0.5, 0.0), p(0.5, 0.5), false);
        let def = add_stroke(Some(&def), vec![p(0.2, 0.2)], Brush::default(), false);
        let def = add_radial(Some(&def), p(0.3, 0.3), [0.1, 0.1], true);
        assert_eq!(def.components.len(), 3);
        assert_eq!(def.components[2].op, MaskOp::Subtract);
        let (i, src) = last_gradient(&def).unwrap();
        assert_eq!(i, 2);
        let MaskSource::Radial(mut g) = src else { panic!("radial") };
        g.radius = [0.3, 0.2];
        let moved = with_source(&def, i, MaskSource::Radial(g));
        assert!(matches!(&moved.components[2].source, MaskSource::Radial(r) if r.radius == [0.3, 0.2]));
        assert_eq!(with_source(&def, 9, MaskSource::Radial(g)), def, "no such component");
    }

    #[test]
    fn fill_replaces_the_mask_with_the_whole_image_and_keeps_its_softness() {
        let painted = add_stroke(None, vec![p(0.5, 0.5)], Brush::default(), false);
        let soft = MaskDef { feather: 0.01, invert: true, ..painted };
        let filled = fill(Some(&soft));
        assert_eq!(filled.components.len(), 1);
        assert!(!filled.invert && (filled.feather - 0.01).abs() < 1e-6);
        let MaskSource::Brush(b) = &filled.components[0].source else { panic!("a brush component") };
        assert_eq!(b.strokes, vec![fill_stroke()]);
        // Erasing afterwards goes into its own component.
        let erased = add_stroke(Some(&filled), vec![p(0.2, 0.2)], Brush::default(), true);
        assert_eq!(erased.components.len(), 2);
        assert_eq!(erased.components[1].op, MaskOp::Subtract);
    }

    #[test]
    fn strokes_of_one_kind_share_a_component_and_erasing_starts_another() {
        let brush = Brush::default();
        let a = add_stroke(None, vec![p(0.1, 0.1), p(0.2, 0.2)], brush, false);
        let b = add_stroke(Some(&a), vec![p(0.3, 0.3)], brush, false);
        assert_eq!(b.components.len(), 1);
        let c = add_stroke(Some(&b), vec![p(0.3, 0.3)], brush, true);
        assert_eq!(c.components.len(), 2);
        assert_eq!(c.components[1].op, MaskOp::Subtract);
        let MaskSource::Brush(first) = &c.components[0].source else { panic!() };
        assert_eq!(first.strokes.len(), 2);
    }

    #[test]
    fn clicks_refine_the_last_object_until_a_new_one_is_started() {
        let a = add_click(None, p(0.5, 0.5), true, false);
        let b = add_click(Some(&a), p(0.6, 0.5), false, false);
        assert_eq!(b.components.len(), 1);
        let MaskSource::Ai(ai) = &b.components[0].source else { panic!() };
        let AiTarget::Object { points, .. } = &ai.target else { panic!() };
        assert_eq!(points.len(), 2);
        assert!(!points[1].include);
        let c = add_click(Some(&b), p(0.1, 0.1), true, true);
        assert_eq!(c.components.len(), 2);
        assert_eq!(c.components[1].label, "Object 2");
        assert_eq!(add_subject(Some(&c)).components.len(), 3);
    }

    #[test]
    fn computed_rasters_fill_only_the_same_targets() {
        let def = add_click(None, p(0.5, 0.5), true, false);
        let mut computed = def.clone();
        let MaskSource::Ai(ai) = &mut computed.components[0].source else { panic!() };
        ai.raster = Some(masks::raster_key("img", &ai.model, &ai.target));
        assert!(matches!(&fill_rasters(&def, &computed).components[0].source, MaskSource::Ai(a) if a.raster.is_some()));
        // The user clicked again meanwhile: the old raster does not fit any more.
        let moved = add_click(Some(&def), p(0.7, 0.7), true, false);
        assert!(
            matches!(&fill_rasters(&moved, &computed).components[0].source, MaskSource::Ai(a) if a.raster.is_none())
        );
    }

    #[test]
    fn the_shown_mask_is_upright_and_cropped() {
        let m = core_model::MaskImage::from_values(4, 2, vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]).unwrap();
        let (w, h, v) = shown_mask(&m, Orientation::Rotate90, &Geometry::default(), true).unwrap();
        assert_eq!((w, h), (2, 4));
        assert_eq!(v[1], 1.0, "the stored top-left corner is at the upright top-right");
    }
}
