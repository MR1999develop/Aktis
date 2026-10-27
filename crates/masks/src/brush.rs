//! Brush strokes to mask values.
//!
//! A stroke is drawn as the distance from each pixel to its path, so it is
//! smooth and identical at every resolution, however fast the brush moved.

use core_model::{BrushMask, NormPoint};

/// The painted values of `brush` at `w × h` (0–1), strokes combined by union.
#[must_use]
pub fn rasterize(brush: &BrushMask, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0_f32; w * h];
    let long = w.max(h) as f32;
    let px = |p: NormPoint| (p.x() * w as f32, p.y() * h as f32);
    for stroke in &brush.strokes {
        let r = (stroke.radius * long).max(0.5);
        let hard = stroke.hardness.clamp(0.0, 1.0);
        let opacity = stroke.opacity.clamp(0.0, 1.0);
        let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| px(*p)).collect();
        let segments: Vec<((f32, f32), (f32, f32))> = match pts.as_slice() {
            [] => continue,
            [one] => vec![(*one, *one)],
            many => many.windows(2).map(|s| (s[0], s[1])).collect(),
        };
        for (a, b) in segments {
            let span = |lo: f32, hi: f32, n: usize| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the image
                let r = ((lo - r).floor().max(0.0) as usize, ((hi + r).ceil().max(0.0) as usize).min(n));
                r
            };
            let (x0, x1) = span(a.0.min(b.0), a.0.max(b.0), w);
            let (y0, y1) = span(a.1.min(b.1), a.1.max(b.1), h);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = dx * dx + dy * dy;
            for y in y0..y1 {
                for x in x0..x1 {
                    let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let t = if len2 > 0.0 { (((cx - a.0) * dx + (cy - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    let (ex, ey) = (cx - (a.0 + t * dx), cy - (a.1 + t * dy));
                    let v = falloff((ex * ex + ey * ey).sqrt() / r, hard) * opacity;
                    let o = &mut out[y * w + x];
                    *o = o.max(v);
                }
            }
        }
    }
    out
}

/// Strength at `t` radii from the path: full inside the hard core, then a
/// smooth step down to 0 at the edge.
fn falloff(t: f32, hardness: f32) -> f32 {
    if t <= hardness {
        return 1.0;
    }
    if t >= 1.0 {
        return 0.0;
    }
    let u = (t - hardness) / (1.0 - hardness);
    1.0 - u * u * (3.0 - 2.0 * u)
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact values at the centre and outside
mod tests {
    use super::*;
    use core_model::BrushStroke;

    fn p(x: f32, y: f32) -> NormPoint {
        NormPoint::new(x, y).expect("finite")
    }

    fn brush(points: Vec<NormPoint>, radius: f32, hardness: f32, opacity: f32) -> BrushMask {
        BrushMask { strokes: vec![BrushStroke { points, radius, hardness, opacity }], edge_aware: false }
    }

    #[test]
    fn a_dab_is_full_at_the_centre_soft_at_the_edge_and_empty_outside() {
        let v = rasterize(&brush(vec![p(0.5, 0.5)], 0.1, 0.5, 1.0), 100, 100);
        assert_eq!(v[50 * 100 + 50], 1.0);
        assert!(v[50 * 100 + 58] > 0.0 && v[50 * 100 + 58] < 1.0, "soft edge: {}", v[50 * 100 + 58]);
        assert_eq!(v[50 * 100 + 62], 0.0);
    }

    #[test]
    fn a_stroke_covers_its_whole_path_at_its_opacity() {
        let v = rasterize(&brush(vec![p(0.1, 0.5), p(0.9, 0.5)], 0.05, 1.0, 0.6), 200, 100);
        assert!((10..190).all(|x| (v[50 * 200 + x] - 0.6).abs() < 1e-6));
        assert_eq!(v[10 * 200 + 100], 0.0);
    }

    #[test]
    fn the_same_stroke_looks_the_same_at_any_resolution() {
        let b = brush(vec![p(0.2, 0.3), p(0.7, 0.6)], 0.04, 0.3, 1.0);
        let small = rasterize(&b, 100, 50);
        let large = rasterize(&b, 400, 200);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean(&small) - mean(&large)).abs() < 0.01);
    }
}
