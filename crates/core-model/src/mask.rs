//! Masks: single-channel images, and the description of how a mask is built.
//!
//! A layer's mask is a [`MaskDef`]: an ordered list of components combined
//! with add, subtract and intersect, plus edge refinement, feather, opacity
//! and invert. AI components keep their *prompt* (subject, clicks, a
//! rectangle) and the [`RasterKey`] of the result the model produced for it.
//! The raster itself lives outside the recipe, in a cache: a model's output
//! can differ slightly between GPUs and runtime versions, so the recipe never
//! re-runs a model behind the user's back. Everything after the model
//! (scaling to the image, snapping to its edges, feather) is deterministic
//! and recomputed at the resolution being rendered.

use std::fmt;

use crate::ids::ModelId;
use crate::image::{ImageError, pixel_count};

/// A single-channel image of mask values: 0 = not selected, 1 = selected.
/// Rows top to bottom, no padding.
#[derive(Clone, PartialEq)]
pub struct MaskImage {
    width: u32,
    height: u32,
    values: Vec<f32>,
}

impl fmt::Debug for MaskImage {
    // Never dump millions of values into a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaskImage").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

impl MaskImage {
    /// A mask with every value set to `value`.
    ///
    /// # Errors
    /// [`ImageError::Empty`] or [`ImageError::TooLarge`].
    pub fn filled(width: u32, height: u32, value: f32) -> Result<Self, ImageError> {
        let n = pixel_count(width, height)?;
        Ok(Self { width, height, values: vec![value; n] })
    }

    /// Wraps `width × height` values.
    ///
    /// # Errors
    /// [`ImageError`] when the dimensions are invalid or the buffer has the wrong length.
    pub fn from_values(width: u32, height: u32, values: Vec<f32>) -> Result<Self, ImageError> {
        let expected = pixel_count(width, height)?;
        if values.len() != expected {
            return Err(ImageError::LengthMismatch { expected, actual: values.len() });
        }
        Ok(Self { width, height, values })
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// All values, row by row.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// All values, mutable. The length cannot change.
    pub fn values_mut(&mut self) -> &mut [f32] {
        &mut self.values
    }

    /// The value at `(x, y)`, or `None` outside the mask.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<f32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.values.get(y as usize * self.width as usize + x as usize).copied()
    }

    /// Takes the buffer out, consuming the mask.
    #[must_use]
    pub fn into_values(self) -> Vec<f32> {
        self.values
    }
}

/// A position in the developed image, as fractions of its width and height
/// (0 = left/top edge, 1 = right/bottom edge). Independent of resolution, so
/// the same prompt works on the preview and on the full-size image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormPoint {
    x: f32,
    y: f32,
}

impl NormPoint {
    /// A point, clamped into the image. `None` when a coordinate is not finite.
    #[must_use]
    pub fn new(x: f32, y: f32) -> Option<Self> {
        (x.is_finite() && y.is_finite()).then(|| Self { x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0) })
    }

    /// Horizontal position, 0–1.
    #[must_use]
    pub fn x(self) -> f32 {
        self.x
    }

    /// Vertical position, 0–1.
    #[must_use]
    pub fn y(self) -> f32 {
        self.y
    }
}

/// A rectangle in normalised coordinates; corners always ordered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormRect {
    min: NormPoint,
    max: NormPoint,
}

impl NormRect {
    /// The rectangle spanned by two opposite corners, in any order.
    #[must_use]
    pub fn new(a: NormPoint, b: NormPoint) -> Self {
        Self {
            min: NormPoint { x: a.x.min(b.x), y: a.y.min(b.y) },
            max: NormPoint { x: a.x.max(b.x), y: a.y.max(b.y) },
        }
    }

    /// Top-left corner.
    #[must_use]
    pub fn min(self) -> NormPoint {
        self.min
    }

    /// Bottom-right corner.
    #[must_use]
    pub fn max(self) -> NormPoint {
        self.max
    }

    /// Width, 0–1.
    #[must_use]
    pub fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    /// Height, 0–1.
    #[must_use]
    pub fn height(self) -> f32 {
        self.max.y - self.min.y
    }
}

/// One click on the image: on the object (`include`) or on something to leave out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PromptPoint {
    /// Where the click was.
    pub at: NormPoint,
    /// `true` adds the region under the click, `false` removes it.
    pub include: bool,
}

/// What an AI component selects.
#[derive(Debug, Clone, PartialEq)]
pub enum AiTarget {
    /// The main subject of the photo, found without any prompt.
    Subject,
    /// An object, a person or a part of one, chosen with clicks and/or a
    /// rectangle. "Select people" creates one of these per detected person,
    /// with the person's box as the rectangle, so each can be refined by
    /// clicking like any other object.
    Object {
        /// Clicks, in the order they were made.
        points: Vec<PromptPoint>,
        /// A rectangle around the object.
        rect: Option<NormRect>,
    },
    /// A part of one person (hair, face skin, lips …). The person is named by
    /// their box, as found by "Select people".
    PersonPart {
        /// Box around the person.
        person: NormRect,
        /// Which part.
        part: BodyPart,
    },
}

/// Parts of a person that can be selected on their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BodyPart {
    /// Hair (and beard).
    Hair,
    /// Skin of the face, without eyes, eyebrows, lips and mouth.
    FaceSkin,
    /// Skin of the body: neck, arms, hands, legs.
    BodySkin,
    /// Clothes.
    Clothes,
    /// Glasses, hats, jewellery, bags.
    Accessories,
    /// Eyebrows.
    Eyebrows,
    /// The white of the eyes.
    EyeSclera,
    /// Irises and pupils.
    Irises,
    /// Lips.
    Lips,
    /// Teeth (the inside of the mouth).
    Teeth,
}

impl BodyPart {
    /// Every part, in the order of the UI.
    pub const ALL: [Self; 10] = [
        Self::Hair,
        Self::FaceSkin,
        Self::BodySkin,
        Self::Clothes,
        Self::Accessories,
        Self::Eyebrows,
        Self::EyeSclera,
        Self::Irises,
        Self::Lips,
        Self::Teeth,
    ];

    /// Name shown in the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Hair => "Hair",
            Self::FaceSkin => "Face skin",
            Self::BodySkin => "Body skin",
            Self::Clothes => "Clothes",
            Self::Accessories => "Accessories",
            Self::Eyebrows => "Eyebrows",
            Self::EyeSclera => "Eye sclera",
            Self::Irises => "Iris and pupil",
            Self::Lips => "Lips",
            Self::Teeth => "Teeth",
        }
    }

    /// Stable identifier for files and the command line (`"face-skin"`).
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Hair => "hair",
            Self::FaceSkin => "face-skin",
            Self::BodySkin => "body-skin",
            Self::Clothes => "clothes",
            Self::Accessories => "accessories",
            Self::Eyebrows => "eyebrows",
            Self::EyeSclera => "eye-sclera",
            Self::Irises => "irises",
            Self::Lips => "lips",
            Self::Teeth => "teeth",
        }
    }

    /// The part with this [`Self::id`].
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
}

impl AiTarget {
    /// Whether the target says what to select. An object needs at least one
    /// including click or a rectangle.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        match self {
            Self::Subject => true,
            Self::Object { points, rect } => rect.is_some() || points.iter().any(|p| p.include),
            Self::PersonPart { .. } => true,
        }
    }
}

/// Identifies the raster a model produced for one AI component: a 128-bit
/// hash of the image, the model and the prompt. Equal keys mean equal inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RasterKey([u8; 16]);

impl RasterKey {
    /// Wraps 16 bytes of hash.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The hash bytes.
    #[must_use]
    pub fn bytes(self) -> [u8; 16] {
        self.0
    }

    /// Parses the 32 lowercase hex digits written by `Display`.
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return None;
        }
        let mut out = [0_u8; 16];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Display for RasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// An AI-generated component of a mask.
#[derive(Debug, Clone, PartialEq)]
pub struct AiMask {
    /// What to select.
    pub target: AiTarget,
    /// The model that computed (or will compute) the raster.
    pub model: ModelId,
    /// The raster computed for this target; `None` until the model has run.
    pub raster: Option<RasterKey>,
}

/// One brush stroke: dabs along a path. Sizes are fractions of the image's
/// long side, so a stroke covers the same area at every resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct BrushStroke {
    /// The path, in order.
    pub points: Vec<NormPoint>,
    /// Radius of the brush.
    pub radius: f32,
    /// Share of the radius painted at full strength (0 = soft, 1 = hard edge).
    pub hardness: f32,
    /// Strength of the stroke, 0–1.
    pub opacity: f32,
}

/// Largest brush radius, as a fraction of the image's long side.
pub const MAX_BRUSH_RADIUS: f32 = 0.5;

impl BrushStroke {
    /// The stroke with radius, hardness and opacity in range (non-finite
    /// values become the defaults).
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let fix = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
        self.radius = fix(self.radius, 0.0005, MAX_BRUSH_RADIUS, 0.02);
        self.hardness = fix(self.hardness, 0.0, 1.0, 0.5);
        self.opacity = fix(self.opacity, 0.0, 1.0, 1.0);
        self
    }
}

/// Painted strokes. Painting with a component set to subtract erases.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BrushMask {
    /// Strokes in the order they were painted.
    pub strokes: Vec<BrushStroke>,
    /// Let the strokes follow the photo's edges (like an auto-mask brush):
    /// the painted area is refined against the image's colours.
    pub edge_aware: bool,
}

/// A linear gradient, as Capture One's Linear Gradient Mask: the mask is
/// full on the side of `start`, nothing beyond `end`, and fades smoothly
/// along the line between them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearGradient {
    /// Where the fade starts (full mask up to here).
    pub start: NormPoint,
    /// Where it ends (no mask from here on).
    pub end: NormPoint,
}

/// A radial gradient, as Capture One's Radial Gradient Mask: an ellipse,
/// full inside and fading to nothing at its edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadialGradient {
    /// Centre of the ellipse.
    pub centre: NormPoint,
    /// Horizontal and vertical radius, as fractions of the image's long side.
    pub radius: [f32; 2],
    /// Share of the radius over which the mask fades out (0 = hard edge, 1 =
    /// fading from the centre).
    pub feather: f32,
}

/// Smallest gradient radius, as a fraction of the image's long side.
pub const MIN_RADIUS: f32 = 0.005;
/// Largest gradient radius, as a fraction of the image's long side.
pub const MAX_RADIUS: f32 = 2.0;

impl RadialGradient {
    /// Radii and feather inside their ranges (non-finite values become defaults).
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let fix = |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
        self.radius = self.radius.map(|r| fix(r, MIN_RADIUS, MAX_RADIUS, 0.2));
        self.feather = fix(self.feather, 0.0, 1.0, 0.5);
        self
    }
}

/// Where a component's values come from.
#[derive(Debug, Clone, PartialEq)]
pub enum MaskSource {
    /// Computed by a segmentation model.
    Ai(AiMask),
    /// Painted with a brush.
    Brush(BrushMask),
    /// A linear gradient.
    Linear(LinearGradient),
    /// A radial gradient.
    Radial(RadialGradient),
}

/// How a component combines with the components before it. Masks are soft,
/// so these are the fuzzy-set operations: union is the maximum, intersection
/// the minimum, and subtraction keeps what is not in the component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskOp {
    /// Union: selected if in the mask so far or in this component.
    Add,
    /// Difference: removes this component from the mask so far.
    Subtract,
    /// Intersection: keeps only what is in both.
    Intersect,
}

/// One layer of a mask.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskComponent {
    /// Name shown in the mask panel ("Subject", "Person 2", "Sky").
    pub label: String,
    /// How it combines with the components above it. The first enabled
    /// component always counts as [`MaskOp::Add`].
    pub op: MaskOp,
    /// A disabled component is kept but ignored.
    pub enabled: bool,
    /// Where the values come from.
    pub source: MaskSource,
}

impl MaskComponent {
    /// An enabled AI component that still has to be computed.
    #[must_use]
    pub fn ai(label: impl Into<String>, op: MaskOp, target: AiTarget, model: ModelId) -> Self {
        Self { label: label.into(), op, enabled: true, source: MaskSource::Ai(AiMask { target, model, raster: None }) }
    }

    /// An enabled linear gradient component.
    #[must_use]
    pub fn linear(label: impl Into<String>, op: MaskOp, gradient: LinearGradient) -> Self {
        Self { label: label.into(), op, enabled: true, source: MaskSource::Linear(gradient) }
    }

    /// An enabled radial gradient component.
    #[must_use]
    pub fn radial(label: impl Into<String>, op: MaskOp, gradient: RadialGradient) -> Self {
        Self { label: label.into(), op, enabled: true, source: MaskSource::Radial(gradient.sanitized()) }
    }

    /// An enabled brush component. With [`MaskOp::Subtract`] it erases.
    #[must_use]
    pub fn brush(label: impl Into<String>, op: MaskOp, brush: BrushMask) -> Self {
        Self { label: label.into(), op, enabled: true, source: MaskSource::Brush(brush) }
    }
}

/// Largest feather radius, as a fraction of the image's long side.
pub const MAX_FEATHER: f32 = 0.1;

/// How a layer builds its mask.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskDef {
    /// Components in order, top to bottom.
    pub components: Vec<MaskComponent>,
    /// Snap the edges of AI components to the edges in the image (hair, fur,
    /// leaves), instead of only scaling the model's output up.
    pub refine_edges: bool,
    /// Softness of the edge, as a fraction of the image's long side (0–[`MAX_FEATHER`]).
    pub feather: f32,
    /// Strength of the whole mask, 0–1.
    pub opacity: f32,
    /// Selects everything except the combined components.
    pub invert: bool,
}

impl Default for MaskDef {
    fn default() -> Self {
        Self { components: Vec::new(), refine_edges: true, feather: 0.0, opacity: 1.0, invert: false }
    }
}

impl MaskDef {
    /// The same definition with feather and opacity inside their ranges
    /// (non-finite values become the defaults).
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.feather = if self.feather.is_finite() { self.feather.clamp(0.0, MAX_FEATHER) } else { 0.0 };
        self.opacity = if self.opacity.is_finite() { self.opacity.clamp(0.0, 1.0) } else { 1.0 };
        for c in &mut self.components {
            match &mut c.source {
                MaskSource::Brush(b) => {
                    b.strokes = std::mem::take(&mut b.strokes).into_iter().map(BrushStroke::sanitized).collect();
                }
                MaskSource::Radial(r) => *r = r.sanitized(),
                MaskSource::Ai(_) | MaskSource::Linear(_) => {}
            }
        }
        self
    }

    /// AI components that have no raster yet, with their index.
    pub fn pending_ai(&self) -> impl Iterator<Item = (usize, &AiMask)> {
        self.components.iter().enumerate().filter_map(|(i, c)| match &c.source {
            MaskSource::Ai(ai) if ai.raster.is_none() => Some((i, ai)),
            MaskSource::Ai(_) | MaskSource::Brush(_) | MaskSource::Linear(_) | MaskSource::Radial(_) => None,
        })
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact values set by the tests
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> NormPoint {
        NormPoint::new(x, y).expect("finite")
    }

    #[test]
    fn mask_images_check_their_size() {
        assert_eq!(MaskImage::filled(0, 3, 0.0), Err(ImageError::Empty));
        let r = MaskImage::from_values(2, 2, vec![0.0; 3]);
        assert_eq!(r, Err(ImageError::LengthMismatch { expected: 4, actual: 3 }));
        let m = MaskImage::filled(2, 1, 0.5).expect("valid");
        assert_eq!(m.get(1, 0), Some(0.5));
        assert_eq!(m.get(2, 0), None);
    }

    #[test]
    fn points_are_clamped_into_the_image_and_must_be_finite() {
        assert_eq!(p(-0.5, 1.5), p(0.0, 1.0));
        assert_eq!(NormPoint::new(f32::NAN, 0.5), None);
        assert_eq!(NormPoint::new(0.5, f32::INFINITY), None);
    }

    #[test]
    fn rectangles_order_their_corners() {
        let r = NormRect::new(p(0.8, 0.1), p(0.2, 0.9));
        assert_eq!((r.min(), r.max()), (p(0.2, 0.1), p(0.8, 0.9)));
    }

    #[test]
    fn an_object_needs_an_including_click_or_a_rectangle() {
        let exclude_only =
            AiTarget::Object { points: vec![PromptPoint { at: p(0.5, 0.5), include: false }], rect: None };
        assert!(!exclude_only.is_complete());
        let click = AiTarget::Object { points: vec![PromptPoint { at: p(0.5, 0.5), include: true }], rect: None };
        assert!(click.is_complete());
        let rect = AiTarget::Object { points: vec![], rect: Some(NormRect::new(p(0.1, 0.1), p(0.2, 0.2))) };
        assert!(rect.is_complete());
        assert!(AiTarget::Subject.is_complete());
    }

    #[test]
    fn raster_keys_round_trip_through_hex() {
        let k = RasterKey::from_bytes([0x01, 0xab, 0, 0xff, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 0xfe]);
        let s = k.to_string();
        assert_eq!(s.len(), 32);
        assert!(s.starts_with("01ab00ff"));
        assert_eq!(RasterKey::from_hex(&s), Some(k));
        assert_eq!(RasterKey::from_hex("01AB"), None);
        assert_eq!(RasterKey::from_hex(&s.to_uppercase()), None);
    }

    #[test]
    fn sanitizing_clamps_feather_and_opacity() {
        let d = MaskDef { feather: 3.0, opacity: -1.0, ..MaskDef::default() }.sanitized();
        assert_eq!((d.feather, d.opacity), (MAX_FEATHER, 0.0));
        let d = MaskDef { feather: f32::NAN, opacity: f32::NAN, ..MaskDef::default() }.sanitized();
        assert_eq!((d.feather, d.opacity), (0.0, 1.0));
    }

    #[test]
    fn body_parts_have_unique_ids_that_round_trip() {
        for p in BodyPart::ALL {
            assert_eq!(BodyPart::from_id(p.id()), Some(p));
        }
        assert_eq!(BodyPart::from_id("nose"), None);
        let ids: std::collections::BTreeSet<&str> = BodyPart::ALL.iter().map(|p| p.id()).collect();
        assert_eq!(ids.len(), BodyPart::ALL.len());
    }

    #[test]
    fn brush_strokes_are_sanitized_with_the_definition() {
        let stroke = BrushStroke { points: vec![p(0.5, 0.5)], radius: 9.0, hardness: f32::NAN, opacity: -1.0 };
        let brush = BrushMask { strokes: vec![stroke], edge_aware: false };
        let d = MaskDef { components: vec![MaskComponent::brush("B", MaskOp::Add, brush)], ..MaskDef::default() };
        let MaskSource::Brush(b) = &d.sanitized().components[0].source else { panic!("brush") };
        assert_eq!((b.strokes[0].radius, b.strokes[0].hardness, b.strokes[0].opacity), (MAX_BRUSH_RADIUS, 0.5, 0.0));
    }

    #[test]
    fn pending_components_are_those_without_a_raster() {
        let model = ModelId::new("sam2.1_small").expect("valid");
        let mut d = MaskDef::default();
        d.components.push(MaskComponent::ai("Subject", MaskOp::Add, AiTarget::Subject, model.clone()));
        d.components.push(MaskComponent::ai("Person 1", MaskOp::Add, AiTarget::Subject, model));
        if let MaskSource::Ai(ai) = &mut d.components[0].source {
            ai.raster = Some(RasterKey::from_bytes([1; 16]));
        }
        d.components.push(MaskComponent::brush("Brush", MaskOp::Add, BrushMask::default()));
        assert_eq!(d.pending_ai().map(|(i, _)| i).collect::<Vec<_>>(), vec![1]);
    }
}
