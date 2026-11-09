//! Curve, as in Capture One: an RGB curve, a Luma curve and a curve for
//! each of red, green and blue, each with up to [`MAX_POINTS`] points.
//!
//! Values are encoded with a 2.2 gamma (as the curve's histogram shows
//! them). The curve through the points is a monotone cubic (Fritsch and
//! Carlson): it never overshoots between points, so a rising set of points
//! gives a rising curve. Left of the first point the curve is flat; right
//! of the last one it is flat too, unless that point is at the right edge,
//! where it continues in a straight line so highlights above white
//! (scene-referred data) are not clipped here.
//!
//! The RGB curve applies to each channel, then the Luma curve to the
//! luminance only (scaling R, G and B together, so hues keep), then each
//! channel's own curve.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, MAX_PARAMS, ParamDescriptor, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{float, luma};

/// Most points per curve.
pub const MAX_POINTS: usize = 8;

/// The five curves, in parameter order: their id prefix and name.
pub const CURVES: [(&str, &str); 5] =
    [("rgb", "RGB"), ("luma", "Luma"), ("red", "Red"), ("green", "Green"), ("blue", "Blue")];

/// Values per curve in the GPU uniforms: count, then x, y and the tangents.
const GPU_STRIDE: usize = 1 + 3 * MAX_POINTS;

/// Id of the number of points of curve `curve` (an index into [`CURVES`]).
#[must_use]
pub fn count_id(curve: usize) -> String {
    format!("{}_points", CURVES[curve].0)
}

/// Id of the input (`x`) or output (`y`) of point `i` of curve `curve`.
#[must_use]
pub fn point_id(curve: usize, i: usize, axis: char) -> String {
    format!("{}_{axis}{i}", CURVES[curve].0)
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| {
    let mut params: Vec<ParamDescriptor> = Vec::new();
    for c in 0..CURVES.len() {
        #[allow(clippy::cast_precision_loss)] // a small count
        let max = MAX_POINTS as f32;
        params.push(float(&count_id(c), "Points", [2.0, max], 2.0, 1.0, Unit::None));
        for i in 0..MAX_POINTS {
            let end = if i == 0 { 0.0 } else { 1.0 };
            params.push(float(&point_id(c, i, 'x'), "Input", [0.0, 1.0], end, 0.001, Unit::None));
            params.push(float(&point_id(c, i, 'y'), "Output", [0.0, 1.0], end, 0.001, Unit::None));
        }
    }
    ToolDescriptor {
        id: ToolId::new("curve").expect("constant id is valid"),
        version: 1,
        name: "Curve",
        category: Category::Exposure,
        order: 80,
        starts_on: true,
        params,
    }
});

/// A curve ready to evaluate: points sorted by input, with their tangents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spline {
    /// Points used (0 for a straight identity line, skipped).
    pub n: usize,
    /// Inputs, increasing.
    pub x: [f32; MAX_POINTS],
    /// Outputs.
    pub y: [f32; MAX_POINTS],
    /// Slopes at the points.
    pub m: [f32; MAX_POINTS],
}

impl Spline {
    /// The curve through `points` (any order; points closer than 0.001 in
    /// input to an earlier one are dropped). Fewer than two points, or the
    /// straight line from (0, 0) to (1, 1), give the identity (`n` = 0).
    #[must_use]
    pub fn new(points: &[[f32; 2]]) -> Self {
        let mut sorted: Vec<[f32; 2]> =
            points.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
        sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let mut pts: Vec<[f32; 2]> = Vec::with_capacity(MAX_POINTS);
        for p in sorted {
            if pts.last().is_none_or(|q| p[0] - q[0] >= 0.001) && pts.len() < MAX_POINTS {
                pts.push(p);
            }
        }
        let mut s = Self { n: 0, x: [0.0; MAX_POINTS], y: [0.0; MAX_POINTS], m: [0.0; MAX_POINTS] };
        let straight = pts.len() == 2
            && pts[0][0].abs() < 1e-6
            && pts[0][1].abs() < 1e-6
            && (pts[1][0] - 1.0).abs() < 1e-6
            && (pts[1][1] - 1.0).abs() < 1e-6;
        if pts.len() < 2 || straight {
            return s;
        }
        let n = pts.len();
        s.n = n;
        for (i, p) in pts.iter().enumerate() {
            s.x[i] = p[0];
            s.y[i] = p[1];
        }
        // Fritsch–Carlson: secant slopes, averaged, zero at extrema, limited.
        let mut delta = [0.0_f32; MAX_POINTS];
        for (i, d) in delta.iter_mut().enumerate().take(n - 1) {
            *d = (s.y[i + 1] - s.y[i]) / (s.x[i + 1] - s.x[i]);
        }
        s.m[0] = delta[0];
        s.m[n - 1] = delta[n - 2];
        for (i, d) in delta[..n - 1].windows(2).enumerate() {
            s.m[i + 1] = if d[0] * d[1] <= 0.0 { 0.0 } else { (d[0] + d[1]) / 2.0 };
        }
        for (i, d) in delta.iter().enumerate().take(n - 1) {
            if d.abs() < 1e-9 {
                s.m[i] = 0.0;
                s.m[i + 1] = 0.0;
                continue;
            }
            let a = s.m[i] / d;
            let b = s.m[i + 1] / d;
            let r = a * a + b * b;
            if r > 9.0 {
                let t = 3.0 / r.sqrt();
                s.m[i] = t * a * d;
                s.m[i + 1] = t * b * d;
            }
        }
        s
    }

    /// The curve of `curve` (an index into [`CURVES`]) in a tool's parameters.
    #[must_use]
    pub fn of(params: &ResolvedParams, curve: usize) -> Self {
        Self::new(&points(params, curve))
    }

    /// Whether this is the identity.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.n == 0
    }

    /// The output for input `x`.
    #[must_use]
    pub fn eval(&self, x: f32) -> f32 {
        let n = self.n;
        if n == 0 {
            return x;
        }
        if x <= self.x[0] {
            return self.y[0];
        }
        if x >= self.x[n - 1] {
            return if self.x[n - 1] >= 0.999 {
                self.y[n - 1] + self.m[n - 1] * (x - self.x[n - 1])
            } else {
                self.y[n - 1]
            };
        }
        let mut i = 0;
        while i + 2 < n && x >= self.x[i + 1] {
            i += 1;
        }
        let h = self.x[i + 1] - self.x[i];
        let t = (x - self.x[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.y[i]
            + (t3 - 2.0 * t2 + t) * h * self.m[i]
            + (-2.0 * t3 + 3.0 * t2) * self.y[i + 1]
            + (t3 - t2) * h * self.m[i + 1]
    }
}

/// The points of curve `curve` as stored in a tool's parameters.
#[must_use]
pub fn points(params: &ResolvedParams, curve: usize) -> Vec<[f32; 2]> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a small count, clamped by the descriptor
    let n = (params.float(&count_id(curve)).round() as usize).clamp(2, MAX_POINTS);
    (0..n).map(|i| [params.float(&point_id(curve, i, 'x')), params.float(&point_id(curve, i, 'y'))]).collect()
}

/// The curve tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Curve;

fn encode(v: f32) -> f32 {
    v.max(0.0).powf(1.0 / 2.2)
}

fn decode(e: f32) -> f32 {
    e.max(0.0).powf(2.2)
}

impl Tool for Curve {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let s: [Spline; 5] = std::array::from_fn(|c| Spline::of(params, c));
        if s.iter().all(Spline::is_identity) {
            return;
        }
        for p in image.pixels_mut() {
            let mut rgb = [p[0], p[1], p[2]];
            if !s[0].is_identity() {
                rgb = rgb.map(|v| decode(s[0].eval(encode(v))));
            }
            if !s[1].is_identity() {
                let y = luma(rgb);
                if y > 1e-6 {
                    let k = decode(s[1].eval(encode(y))) / y;
                    rgb = rgb.map(|v| v * k);
                }
            }
            for (k, v) in rgb.iter_mut().enumerate() {
                if !s[k + 2].is_identity() {
                    *v = decode(s[k + 2].eval(encode(*v)));
                }
            }
            p[..3].copy_from_slice(&rgb);
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: WGSL }
    }

    /// Per curve, 25 values: the number of points (0 = identity), then the
    /// inputs, outputs and slopes.
    fn gpu_uniforms(&self, params: &ResolvedParams) -> [f32; MAX_PARAMS] {
        let mut out = [0.0; MAX_PARAMS];
        for c in 0..CURVES.len() {
            let s = Spline::of(params, c);
            let base = c * GPU_STRIDE;
            #[allow(clippy::cast_precision_loss)] // a small count
            let n = s.n as f32;
            out[base] = n;
            out[base + 1..base + 1 + MAX_POINTS].copy_from_slice(&s.x);
            out[base + 1 + MAX_POINTS..base + 1 + 2 * MAX_POINTS].copy_from_slice(&s.y);
            out[base + 1 + 2 * MAX_POINTS..base + GPU_STRIDE].copy_from_slice(&s.m);
        }
        out
    }
}

/// The kernel: seven evaluations (RGB curve on each channel, the Luma
/// curve, each channel's curve), one loop because WGSL bodies cannot
/// declare functions.
const WGSL: &str = "
    var lin = c.rgb;
    for (var step_i = 0u; step_i < 7u; step_i++) {
        var curve = 0u;
        var k = step_i;
        if (step_i == 3u) { curve = 1u; k = 0u; }
        if (step_i > 3u) { k = step_i - 4u; curve = k + 2u; }
        let b = curve * 25u;
        let n = u32(params.v[b / 4u][b % 4u] + 0.5);
        if (n < 2u) { continue; }
        var v = 0.0;
        if (step_i == 3u) { v = dot(lin, vec3<f32>(0.2627, 0.6780, 0.0593)); if (v <= 1e-6) { continue; } }
        else { v = lin[k]; }
        let x = pow(max(v, 0.0), 1.0 / 2.2);
        let xi0 = b + 1u;
        let yi0 = b + 9u;
        let mi0 = b + 17u;
        let x_first = params.v[xi0 / 4u][xi0 % 4u];
        let xl_i = xi0 + n - 1u;
        let yl_i = yi0 + n - 1u;
        let ml_i = mi0 + n - 1u;
        let x_last = params.v[xl_i / 4u][xl_i % 4u];
        var y = 0.0;
        if (x <= x_first) {
            y = params.v[yi0 / 4u][yi0 % 4u];
        } else if (x >= x_last) {
            y = params.v[yl_i / 4u][yl_i % 4u];
            if (x_last >= 0.999) { y = y + params.v[ml_i / 4u][ml_i % 4u] * (x - x_last); }
        } else {
            var i = 0u;
            loop {
                if (i + 2u >= n) { break; }
                let nx = xi0 + i + 1u;
                if (x < params.v[nx / 4u][nx % 4u]) { break; }
                i = i + 1u;
            }
            let a_x = xi0 + i;
            let a_y = yi0 + i;
            let a_m = mi0 + i;
            let x0 = params.v[a_x / 4u][a_x % 4u];
            let x1 = params.v[(a_x + 1u) / 4u][(a_x + 1u) % 4u];
            let y0 = params.v[a_y / 4u][a_y % 4u];
            let y1 = params.v[(a_y + 1u) / 4u][(a_y + 1u) % 4u];
            let m0 = params.v[a_m / 4u][a_m % 4u];
            let m1 = params.v[(a_m + 1u) / 4u][(a_m + 1u) % 4u];
            let h = x1 - x0;
            let t = (x - x0) / h;
            let t2 = t * t;
            let t3 = t2 * t;
            y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * m0
                + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * h * m1;
        }
        let out = pow(max(y, 0.0), 2.2);
        if (step_i == 3u) { lin = lin * (out / v); } else { lin[k] = out; }
    }
    return vec4<f32>(lin, c.a);";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_curves_are_the_identity() {
        let r = ResolvedParams::resolve(Curve.descriptor(), &tool_api::Params::new());
        for c in 0..CURVES.len() {
            assert!(Spline::of(&r, c).is_identity());
        }
        assert!(Curve.gpu_uniforms(&r).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_curve_passes_through_its_points_and_does_not_overshoot() {
        let s = Spline::new(&[[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]]);
        for (x, y) in [(0.0, 0.0), (0.25, 0.15), (0.75, 0.85), (1.0, 1.0)] {
            assert!((s.eval(x) - y).abs() < 1e-6, "{x}");
        }
        let mut last = -1.0;
        for i in 0..=1000 {
            let y = s.eval(i as f32 / 1000.0);
            assert!(y >= last - 1e-6 && (-1e-6..=1.0 + 1e-6).contains(&y), "{i}: {y}");
            last = y;
        }
        // A peak stays a peak: no value above it.
        let peak = Spline::new(&[[0.0, 0.0], [0.5, 0.8], [1.0, 0.2]]);
        assert!((0..=100).all(|i| peak.eval(i as f32 / 100.0) <= 0.8 + 1e-6));
    }

    #[test]
    fn beyond_the_ends_the_curve_is_flat_or_continues_at_white() {
        let s = Spline::new(&[[0.1, 0.05], [0.9, 0.95]]);
        assert!((s.eval(0.0) - 0.05).abs() < 1e-6 && (s.eval(1.5) - 0.95).abs() < 1e-6);
        let s = Spline::new(&[[0.0, 0.1], [1.0, 0.9]]);
        assert!((s.eval(2.0) - (0.9 + 0.8)).abs() < 1e-5);
    }

    #[test]
    fn points_in_any_order_and_duplicates_are_handled() {
        let a = Spline::new(&[[1.0, 1.0], [0.0, 0.0], [0.5, 0.6]]);
        let b = Spline::new(&[[0.0, 0.0], [0.5, 0.6], [0.5004, 0.1], [1.0, 1.0]]);
        assert_eq!(a, b);
        assert!(Spline::new(&[[0.3, 0.3]]).is_identity());
    }

    #[test]
    fn the_luma_curve_keeps_the_hue() {
        let mut p = tool_api::Params::new();
        p.set(crate::common::pid(&point_id(1, 1, 'y')), core_model::ParamValue::Float(1.0));
        p.set(crate::common::pid(&count_id(1)), core_model::ParamValue::Float(3.0));
        p.set(crate::common::pid(&point_id(1, 1, 'x')), core_model::ParamValue::Float(0.5));
        p.set(crate::common::pid(&point_id(1, 1, 'y')), core_model::ParamValue::Float(0.7));
        p.set(crate::common::pid(&point_id(1, 2, 'x')), core_model::ParamValue::Float(1.0));
        p.set(crate::common::pid(&point_id(1, 2, 'y')), core_model::ParamValue::Float(1.0));
        let r = ResolvedParams::resolve(Curve.descriptor(), &p);
        let mut img = ImageF32::from_pixels(1, 1, core_model::WorkingSpace::LinearRec2020, vec![[0.2, 0.1, 0.05, 1.0]])
            .expect("1×1");
        Curve.apply_cpu(&mut img, &r);
        let o = img.pixels()[0];
        assert!(o[1] > 0.1, "brighter: {o:?}");
        assert!((o[0] / o[1] - 2.0).abs() < 1e-4 && (o[1] / o[2] - 2.0).abs() < 1e-4, "{o:?}");
    }
}
