//! Linear and radial gradients to mask values, smooth and identical at
//! every resolution.

use core_model::{LinearGradient, NormPoint, RadialGradient};

/// Hermite fade: 1 at `t ≤ 0`, 0 at `t ≥ 1`, smooth between.
fn fade(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// The pixel position of a normalised point in a `w × h` image.
#[allow(clippy::cast_precision_loss)] // image sizes
fn px(p: NormPoint, w: usize, h: usize) -> (f32, f32) {
    (p.x() * w as f32, p.y() * h as f32)
}

/// The values of a linear gradient at `w × h` (0–1): full before `start`,
/// nothing after `end`, measured along the line from one to the other.
#[must_use]
#[allow(clippy::cast_precision_loss)] // pixel coordinates
pub fn linear(g: &LinearGradient, w: usize, h: usize) -> Vec<f32> {
    let (s, e) = (px(g.start, w, h), px(g.end, w, h));
    let (dx, dy) = (e.0 - s.0, e.1 - s.1);
    let len2 = dx * dx + dy * dy;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            // A gradient of no length is a hard edge through `start`.
            let t = if len2 > 1e-6 { ((cx - s.0) * dx + (cy - s.1) * dy) / len2 } else { 0.0 };
            out.push(fade(t));
        }
    }
    out
}

/// The values of a radial gradient at `w × h` (0–1): full inside the
/// ellipse's unfeathered part, nothing outside it.
#[must_use]
#[allow(clippy::cast_precision_loss)] // pixel coordinates
pub fn radial(g: &RadialGradient, w: usize, h: usize) -> Vec<f32> {
    let g = g.sanitized();
    let c = px(g.centre, w, h);
    let long = w.max(h) as f32;
    let (rx, ry) = ((g.radius[0] * long).max(0.5), (g.radius[1] * long).max(0.5));
    let inner = 1.0 - g.feather;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = ((x as f32 + 0.5 - c.0) / rx, (y as f32 + 0.5 - c.1) / ry);
            let r = (dx * dx + dy * dy).sqrt();
            let v = if r <= inner {
                1.0
            } else if r >= 1.0 {
                0.0
            } else {
                fade((r - inner) / (1.0 - inner).max(1e-6))
            };
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> NormPoint {
        NormPoint::new(x, y).unwrap()
    }

    #[test]
    fn a_linear_gradient_is_full_before_its_start_and_empty_after_its_end() {
        let g = LinearGradient { start: p(0.25, 0.5), end: p(0.75, 0.5) };
        let v = linear(&g, 100, 10);
        let at = |x: usize| v[5 * 100 + x];
        assert!((at(10) - 1.0).abs() < 1e-6 && at(90).abs() < 1e-6);
        assert!((at(50) - 0.5).abs() < 0.05, "half way: {}", at(50));
        assert!(at(30) > at(50) && at(50) > at(70), "falls along the line");
        // Across the line, nothing changes.
        assert!((v[40] - v[9 * 100 + 40]).abs() < 1e-6);
    }

    #[test]
    fn a_radial_gradient_is_full_inside_and_empty_outside_a_true_circle() {
        let g = RadialGradient { centre: p(0.5, 0.5), radius: [0.2, 0.2], feather: 0.5 };
        let (w, h) = (200, 100);
        let v = radial(&g, w, h);
        let at = |x: usize, y: usize| v[y * w + x];
        assert!((at(100, 50) - 1.0).abs() < 1e-6, "centre");
        assert!(at(0, 0).abs() < 1e-6, "corner");
        // Radius 0.2 of the long side = 40 px in both directions (a circle, not an oval).
        assert!(at(100 + 39, 50) > 0.0 && at(100 + 41, 50) < 1e-6);
        assert!(at(100, 50 + 39) > 0.0 && at(100, 99) < 1e-6);
        assert!((at(100 + 15, 50) - 1.0).abs() < 1e-6, "inside the unfeathered half");
    }
}
