//! Geometry of the finished image: perspective (keystone), straightening
//! and crop, applied after the node graph to the upright image.
//!
//! Coordinates are normalised to the upright image (0–1 from the top-left
//! corner), so the same settings work at every resolution. The perspective
//! and the rotation keep the frame's size and are enlarged just enough that
//! no empty corner shows ("fill"); the crop is a rectangle of that frame.

use crate::mask::{NormPoint, NormRect};

/// Most straightening, in degrees either way.
pub const MAX_ANGLE: f32 = 45.0;

/// Strength of the keystone at ±100: the far edge is magnified by up to
/// this share of the image's half size.
const KEYSTONE: f32 = 0.6;

/// Perspective, straightening and crop of a recipe.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Geometry {
    /// Rotation, degrees, positive clockwise (−[`MAX_ANGLE`] to [`MAX_ANGLE`]).
    pub angle: f32,
    /// Vertical keystone, −100 to 100: positive widens the top (for
    /// verticals that converge upwards, as when looking up at a building).
    pub vertical: f32,
    /// Horizontal keystone, −100 to 100: positive widens the left side.
    pub horizontal: f32,
    /// The part of the frame kept; `None` keeps everything.
    pub crop: Option<NormRect>,
}

impl Geometry {
    /// The same settings inside their ranges (non-finite values become 0).
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let fix = |v: f32, max: f32| if v.is_finite() { v.clamp(-max, max) } else { 0.0 };
        self.angle = fix(self.angle, MAX_ANGLE);
        self.vertical = fix(self.vertical, 100.0);
        self.horizontal = fix(self.horizontal, 100.0);
        // A crop covering (almost) everything is no crop.
        self.crop = self.crop.filter(|c| c.width() < 0.9999 || c.height() < 0.9999);
        self
    }

    /// Whether pixels must be resampled (rotation or perspective).
    #[must_use]
    pub fn warps(&self) -> bool {
        self.angle != 0.0 || self.vertical != 0.0 || self.horizontal != 0.0
    }

    /// Whether the geometry changes nothing.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        !self.warps() && self.crop.is_none()
    }

    /// The warp of these settings for a frame `aspect` = width / height.
    #[must_use]
    pub fn warp(&self, aspect: f32) -> Warp {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let mut w = Warp {
            aspect,
            cos: self.angle.to_radians().cos(),
            sin: self.angle.to_radians().sin(),
            a: self.vertical / 100.0 * KEYSTONE,
            b: self.horizontal / 100.0 * KEYSTONE,
            scale: 1.0,
        };
        w.scale = w.fill_scale();
        w
    }

    /// Where a point of the shown image (0–1 within the crop when `cropped`,
    /// else within the frame) comes from in the upright image; `None` when
    /// it falls outside it.
    #[must_use]
    pub fn to_source(&self, p: [f32; 2], aspect: f32, cropped: bool) -> Option<[f32; 2]> {
        let frame = match (cropped, self.crop) {
            (true, Some(c)) => [c.min().x() + p[0] * c.width(), c.min().y() + p[1] * c.height()],
            _ => p,
        };
        let s = if self.warps() { self.warp(aspect).source(frame) } else { frame };
        ((0.0..=1.0).contains(&s[0]) && (0.0..=1.0).contains(&s[1])).then_some(s)
    }

    /// Where a point of the upright image (0–1) is shown: the inverse of
    /// [`Self::to_source`] (0–1 within the crop when `cropped`). `None` when
    /// the point is not shown (outside the crop or the frame).
    #[must_use]
    pub fn to_shown(&self, upright: [f32; 2], aspect: f32, cropped: bool) -> Option<[f32; 2]> {
        let frame = if self.warps() { self.warp(aspect).frame(upright) } else { upright };
        let p = match (cropped, self.crop) {
            (true, Some(c)) => [(frame[0] - c.min().x()) / c.width(), (frame[1] - c.min().y()) / c.height()],
            _ => frame,
        };
        (p.iter().all(|v| v.is_finite() && (-1e-4..=1.0 + 1e-4).contains(v))).then_some(p)
    }

    /// The crop as a rectangle of the frame (the whole frame when there is none).
    #[must_use]
    pub fn crop_or_full(&self) -> NormRect {
        self.crop.unwrap_or_else(full_frame)
    }
}

/// The whole frame, (0, 0)–(1, 1).
#[must_use]
pub fn full_frame() -> NormRect {
    let corner = |v: f32| NormPoint::new(v, v).unwrap_or_else(|| unreachable!("0 and 1 are valid"));
    NormRect::new(corner(0.0), corner(1.0))
}

/// A perspective and rotation ready to map points: from the output frame
/// back to the upright image (what a renderer needs for every pixel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Warp {
    aspect: f32,
    cos: f32,
    sin: f32,
    a: f32,
    b: f32,
    scale: f32,
}

impl Warp {
    /// Centred coordinates, both axes in units of half the shorter side.
    fn centred(&self, p: [f32; 2]) -> [f32; 2] {
        let (hx, hy) = self.half();
        [(p[0] * 2.0 - 1.0) * hx, (p[1] * 2.0 - 1.0) * hy]
    }

    fn uncentred(&self, c: [f32; 2]) -> [f32; 2] {
        let (hx, hy) = self.half();
        [(c[0] / hx + 1.0) / 2.0, (c[1] / hy + 1.0) / 2.0]
    }

    /// Half the frame's width and height, the shorter one being 1.
    fn half(&self) -> (f32, f32) {
        if self.aspect >= 1.0 { (self.aspect, 1.0) } else { (1.0, 1.0 / self.aspect) }
    }

    /// The upright-image point an output-frame point (0–1) shows, before
    /// the fill enlargement is undone; `p` in centred units.
    fn source_centred(&self, c: [f32; 2]) -> [f32; 2] {
        // Undo the enlargement, then the rotation (rotate by −angle).
        let (x, y) = (c[0] / self.scale, c[1] / self.scale);
        let (x, y) = (x * self.cos + y * self.sin, -x * self.sin + y * self.cos);
        // Undo the keystone: the forward map divides by w = 1 + a·y' + b·x'
        // (primes in units of the half size along each axis); its inverse
        // divides by 1 − a·Y − b·X.
        let (hx, hy) = self.half();
        let (nx, ny) = (x / hx, y / hy);
        let d = 1.0 - self.a * ny - self.b * nx;
        if d.abs() < 1e-6 {
            return [f32::INFINITY, f32::INFINITY];
        }
        [nx / d * hx, ny / d * hy]
    }

    /// The output-frame point (0–1) where upright-image point `p` (0–1)
    /// shows: the inverse of [`Self::source`].
    #[must_use]
    pub fn frame(&self, p: [f32; 2]) -> [f32; 2] {
        let c = self.centred(p);
        let (hx, hy) = self.half();
        let (nx, ny) = (c[0] / hx, c[1] / hy);
        // Keystone forward: divide by 1 + a·y + b·x.
        let d = 1.0 + self.a * ny + self.b * nx;
        if d.abs() < 1e-6 {
            return [f32::INFINITY, f32::INFINITY];
        }
        let (x, y) = (nx / d * hx, ny / d * hy);
        // Rotate by +angle, then enlarge.
        let (x, y) = (x * self.cos - y * self.sin, x * self.sin + y * self.cos);
        self.uncentred([x * self.scale, y * self.scale])
    }

    /// The upright-image point (0–1) shown at output-frame point `p` (0–1).
    #[must_use]
    pub fn source(&self, p: [f32; 2]) -> [f32; 2] {
        self.uncentred(self.source_centred(self.centred(p)))
    }

    /// The smallest enlargement (≥ 1) for which the whole frame shows image.
    fn fill_scale(&self) -> f32 {
        let inside = |w: &Self| {
            (0..=16).all(|i| {
                let t = i as f32 / 16.0;
                [[t, 0.0], [t, 1.0], [0.0, t], [1.0, t]].iter().all(|&p| {
                    let s = w.source(p);
                    (-1e-4..=1.0 + 1e-4).contains(&s[0]) && (-1e-4..=1.0 + 1e-4).contains(&s[1])
                })
            })
        };
        let mut w = *self;
        w.scale = 1.0;
        if inside(&w) {
            return 1.0;
        }
        let (mut lo, mut hi) = (1.0_f32, 8.0_f32);
        w.scale = hi;
        if !inside(&w) {
            return hi;
        }
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            w.scale = mid;
            if inside(&w) { hi = mid } else { lo = mid }
        }
        hi
    }

    /// The enlargement that hides the empty corners.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }
}

/// The angle (degrees, for [`Geometry::angle`]) that makes the line from
/// `a` to `b` (points of the shown upright image, in pixels) level, or
/// plumb when it is closer to vertical.
#[must_use]
pub fn level_angle(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
        return 0.0;
    }
    let deg = dy.atan2(dx).to_degrees();
    // Turn the line to the nearest multiple of 90°.
    let nearest = (deg / 90.0).round() * 90.0;
    (nearest - deg).clamp(-MAX_ANGLE, MAX_ANGLE)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> NormRect {
        NormRect::new(
            NormPoint::new(x0, y0).unwrap_or_else(|| panic!()),
            NormPoint::new(x1, y1).unwrap_or_else(|| panic!()),
        )
    }

    #[test]
    fn no_geometry_maps_every_point_to_itself() {
        let g = Geometry::default();
        assert!(g.is_identity());
        assert_eq!(g.to_source([0.25, 0.75], 1.5, true), Some([0.25, 0.75]));
    }

    #[test]
    fn a_crop_maps_the_shown_image_into_its_rectangle() {
        let g = Geometry { crop: Some(rect(0.2, 0.1, 0.6, 0.5)), ..Geometry::default() };
        let s = g.to_source([0.5, 0.5], 1.5, true).unwrap_or_default();
        assert!((s[0] - 0.4).abs() < 1e-6 && (s[1] - 0.3).abs() < 1e-6, "{s:?}");
        assert_eq!(g.to_source([0.5, 0.5], 1.5, false), Some([0.5, 0.5]), "while the crop is being edited");
        assert!(Geometry { crop: Some(full_frame()), ..g }.sanitized().crop.is_none());
    }

    #[test]
    fn rotation_turns_around_the_centre_and_fills_the_frame() {
        let g = Geometry { angle: 10.0, ..Geometry::default() };
        let w = g.warp(1.5);
        assert!(w.scale() > 1.1 && w.scale() < 1.5, "{}", w.scale());
        let c = w.source([0.5, 0.5]);
        assert!((c[0] - 0.5).abs() < 1e-5 && (c[1] - 0.5).abs() < 1e-5);
        // Every corner of the frame shows image.
        for p in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            let s = w.source(p);
            assert!((-1e-3..=1.001).contains(&s[0]) && (-1e-3..=1.001).contains(&s[1]), "{p:?} → {s:?}");
        }
        // Clockwise: a point right of the centre comes from above-right... of the source, below the axis after turning back.
        let right = w.source([0.9, 0.5]);
        assert!(right[1] < 0.5, "{right:?}");
    }

    #[test]
    fn a_positive_vertical_keystone_widens_the_top() {
        let w = Geometry { vertical: 60.0, ..Geometry::default() }.warp(1.0);
        // The same output width covers less of the source at the top than at the bottom.
        let top = w.source([1.0, 0.1])[0] - w.source([0.0, 0.1])[0];
        let bottom = w.source([1.0, 0.9])[0] - w.source([0.0, 0.9])[0];
        assert!(top < bottom, "{top} {bottom}");
    }

    #[test]
    fn showing_a_point_inverts_finding_its_source() {
        let g = Geometry { angle: -7.0, vertical: 35.0, horizontal: -20.0, crop: Some(rect(0.1, 0.2, 0.8, 0.9)) };
        for p in [[0.5, 0.5], [0.3, 0.4], [0.7, 0.65], [0.2, 0.8]] {
            let src = g.to_source(p, 1.5, true).unwrap_or_else(|| panic!("{p:?}"));
            let back = g.to_shown(src, 1.5, true).unwrap_or_else(|| panic!("{src:?}"));
            assert!((back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4, "{p:?} → {src:?} → {back:?}");
        }
    }

    #[test]
    fn levelling_a_line_turns_it_to_the_nearest_axis() {
        assert!((level_angle([0.0, 0.0], [100.0, 5.0]) + 2.862).abs() < 1e-2);
        assert!((level_angle([0.0, 0.0], [3.0, 100.0]) - 1.718).abs() < 1e-2);
        assert_eq!(level_angle([1.0, 1.0], [1.0, 1.0]), 0.0);
    }

    #[test]
    fn out_of_range_settings_are_clamped() {
        let g = Geometry { angle: 99.0, vertical: f32::NAN, horizontal: -300.0, crop: None }.sanitized();
        assert_eq!((g.angle, g.vertical, g.horizontal), (MAX_ANGLE, 0.0, -100.0));
    }
}
