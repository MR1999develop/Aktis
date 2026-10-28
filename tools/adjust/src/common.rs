//! What the tools share: parameter shorthands and the maths the CPU
//! references and the WGSL kernels both follow.

use tool_api::{ParamDescriptor, ParamId, ParamKind, Unit};

/// Rec. 2020 luminance weights (the working space).
pub const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

/// Middle grey, the pivot of the tone tools.
pub const GREY: f32 = 0.18;

/// Luminance of linear Rec. 2020 RGB.
pub fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

/// A parameter id from a constant.
pub fn pid(id: &str) -> ParamId {
    ParamId::new(id).expect("constant id is valid")
}

/// A float parameter.
pub fn float(id: &str, label: &'static str, range: [f32; 2], default: f32, step: f32, unit: Unit) -> ParamDescriptor {
    ParamDescriptor { id: pid(id), label, kind: ParamKind::Float { min: range[0], max: range[1], default, step, unit } }
}

/// An on/off parameter.
pub fn toggle(id: &str, label: &'static str, default: bool) -> ParamDescriptor {
    ParamDescriptor { id: pid(id), label, kind: tool_api::ParamKind::Bool { default } }
}

/// A choice among `options`.
pub fn choice(id: &str, label: &'static str, options: &[&'static str], default: u32) -> ParamDescriptor {
    ParamDescriptor { id: pid(id), label, kind: tool_api::ParamKind::Choice { options: options.to_vec(), default } }
}

/// Signed difference `a − b` of two hues, in degrees, −180 to 180.
pub fn hue_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// A −100 … 100 slider at 0.
pub fn slider(id: &str, label: &'static str) -> ParamDescriptor {
    float(id, label, [-100.0, 100.0], 0.0, 1.0, Unit::None)
}

/// WGSL's `smoothstep` for `low < high`: 0 below `low`, 1 above `high`.
pub fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Stops from middle grey of luminance `y` (`None` for black).
pub fn stops_from_grey(y: f32) -> Option<f32> {
    (y > 1e-6).then(|| (y / GREY).log2())
}

/// Hue (degrees, 0 = red, 120 = green, 240 = blue) and chroma (0 for a
/// grey, 1 for a primary) of an RGB colour, as HSV defines them.
pub fn hue_chroma(c: [f32; 3]) -> (f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    if max <= 1e-6 || d <= 1e-6 {
        return (0.0, 0.0);
    }
    let h = if c[0] >= c[1] && c[0] >= c[2] {
        let h = (c[1] - c[2]) / d;
        h - 6.0 * (h / 6.0).floor()
    } else if c[1] >= c[2] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    (h * 60.0, (d / max).clamp(0.0, 1.0))
}

/// The value at `hue` of six values placed every 60° from red (red,
/// yellow, green, cyan, blue, magenta), interpolated linearly around the
/// circle.
pub fn six_way(values: [f32; 6], hue: f32) -> f32 {
    let h = hue.rem_euclid(360.0) / 60.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // h is in [0, 6)
    let i = (h.floor() as usize).min(5);
    let f = h - h.floor();
    values[i] * (1.0 - f) + values[(i + 1) % 6] * f
}

/// The zero-mean RGB direction of `hue` (degrees): red is (1, −½, −½).
pub fn hue_direction(hue: f32) -> [f32; 3] {
    let h = hue.to_radians();
    let third = std::f32::consts::TAU / 3.0;
    [h.cos(), (h - third).cos(), (h - 2.0 * third).cos()]
}

/// A Gaussian-like blur of a `w` × `h` plane with standard deviation
/// `sigma` pixels: three box blurs each way, on every core. Edges repeat
/// the border value.
pub fn blur(plane: &mut [f32], w: usize, h: usize, sigma: f32) {
    if w == 0 || h == 0 || sigma < 0.5 {
        return;
    }
    // Three boxes of width b have variance 3 (b² − 1) / 12 = sigma².
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a small positive radius
    let radius = ((((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round() as usize).max(1);
    for _ in 0..3 {
        rows(plane, w, |row, scratch| box_line(row, scratch, radius));
    }
    let mut t = transpose(plane, w, h);
    for _ in 0..3 {
        rows(&mut t, h, |row, scratch| box_line(row, scratch, radius));
    }
    plane.copy_from_slice(&transpose(&t, h, w));
}

/// Runs `f` on every row of `plane` (rows of `w` values), in parallel.
fn rows(plane: &mut [f32], w: usize, f: impl Fn(&mut [f32], &mut Vec<f32>) + Sync) {
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let n_rows = plane.len() / w;
    let per = n_rows.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for chunk in plane.chunks_mut(per * w) {
            let f = &f;
            s.spawn(move || {
                let mut scratch = Vec::with_capacity(w);
                for row in chunk.chunks_mut(w) {
                    f(row, &mut scratch);
                }
            });
        }
    });
}

/// A running-mean box blur of one line, radius `r`, edges repeated.
#[allow(clippy::cast_precision_loss)] // small widths
fn box_line(row: &mut [f32], scratch: &mut Vec<f32>, r: usize) {
    let n = row.len();
    scratch.clear();
    scratch.extend_from_slice(row);
    let at = |i: isize| -> f32 {
        let i = i.clamp(0, n as isize - 1);
        #[allow(clippy::cast_sign_loss)] // clamped to 0..n
        scratch[i as usize]
    };
    let r = r as isize;
    let width = (2 * r + 1) as f32;
    let mut sum: f32 = (-r..=r).map(at).sum();
    for (i, out) in row.iter_mut().enumerate() {
        *out = sum / width;
        let i = i as isize;
        sum += at(i + r + 1) - at(i - r);
    }
}

/// `plane` (`w` × `h`) with rows and columns swapped.
fn transpose(plane: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; plane.len()];
    for y in 0..h {
        for x in 0..w {
            out[x * h + y] = plane[y * w + x];
        }
    }
    out
}

/// WGSL for [`hue_chroma`]: declares `hue` and `chroma` from `rgb`.
pub const WGSL_HUE_CHROMA: &str = "
    let mx = max(rgb.r, max(rgb.g, rgb.b));
    let mn = min(rgb.r, min(rgb.g, rgb.b));
    let dd = mx - mn;
    var hue = 0.0;
    var chroma = 0.0;
    if (mx > 1e-6 && dd > 1e-6) {
        var h = 0.0;
        if (rgb.r >= rgb.g && rgb.r >= rgb.b) {
            h = (rgb.g - rgb.b) / dd;
            h = h - 6.0 * floor(h / 6.0);
        } else if (rgb.g >= rgb.b) {
            h = (rgb.b - rgb.r) / dd + 2.0;
        } else {
            h = (rgb.r - rgb.g) / dd + 4.0;
        }
        hue = h * 60.0;
        chroma = clamp(dd / mx, 0.0, 1.0);
    }
";

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn hues_of_the_primaries_and_greys() {
        assert_eq!(hue_chroma([1.0, 0.0, 0.0]), (0.0, 1.0));
        assert_eq!(hue_chroma([0.0, 1.0, 0.0]), (120.0, 1.0));
        assert_eq!(hue_chroma([0.0, 0.0, 1.0]), (240.0, 1.0));
        assert_eq!(hue_chroma([1.0, 0.0, 1.0]), (300.0, 1.0));
        assert_eq!(hue_chroma([0.5, 0.5, 0.5]), (0.0, 0.0));
    }

    #[test]
    fn six_way_interpolates_around_the_circle() {
        let v = [0.0, 6.0, 12.0, 18.0, 24.0, 30.0];
        assert_eq!(six_way(v, 0.0), 0.0);
        assert_eq!(six_way(v, 90.0), 9.0);
        assert_eq!(six_way(v, 330.0), 15.0);
        assert_eq!(six_way(v, 360.0), 0.0);
    }

    #[test]
    fn blurring_keeps_a_flat_plane_and_the_mean_and_spreads_a_spike() {
        let mut flat = vec![0.25_f32; 40 * 30];
        blur(&mut flat, 40, 30, 3.0);
        assert!(flat.iter().all(|v| (v - 0.25).abs() < 1e-6));
        let mut spike = vec![0.0_f32; 41 * 41];
        spike[20 * 41 + 20] = 1.0;
        blur(&mut spike, 41, 41, 3.0);
        let total: f32 = spike.iter().sum();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
        assert!(spike[20 * 41 + 20] < 0.05 && spike[20 * 41 + 23] > 0.0);
        // Symmetric.
        assert!((spike[20 * 41 + 17] - spike[20 * 41 + 23]).abs() < 1e-6);
        assert!((spike[17 * 41 + 20] - spike[20 * 41 + 17]).abs() < 1e-6);
    }

    #[test]
    fn hue_directions_have_no_grey_in_them() {
        for h in [0.0, 37.0, 120.0, 300.0] {
            let d = hue_direction(h);
            assert!((d[0] + d[1] + d[2]).abs() < 1e-5);
        }
        assert!(hue_direction(0.0)[0] > 0.99);
    }
}
