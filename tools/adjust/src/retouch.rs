//! Retouch: heal and clone spots, like the spot tools of Capture One and
//! Lightroom.
//!
//! A node holds up to [`MAX_SPOTS`] spots. Each is a circle on the image
//! and a source circle of the same size somewhere else:
//!
//! - **Clone** copies the source over the spot.
//! - **Heal** copies the source's texture and keeps the spot's
//!   surroundings: the difference between the spot's border and the
//!   source's border is spread smoothly over the inside (a harmonic, or
//!   membrane, interpolation: the Poisson integral of a disc), so the
//!   patch takes the spot's brightness and colour and leaves no seam.
//!
//! Both fade out towards the edge of the circle (the feather) and mix with
//! the image by their opacity. Spots apply one after another, in slot
//! order: a later spot copies what the earlier ones made.
//!
//! The spots are stored as plain parameters, eight per slot ([`Field`]):
//! the kind, the centre, the radius, the source centre, the feather and
//! the opacity. A slot with radius 0 is empty. Positions are in the
//! developed image (the image the graph processes, 0–1 from the top-left
//! corner) and the radius is a fraction of its long side, so a preview and
//! the full image look alike.
//!
//! It copies pixels from elsewhere in the image, so it runs on the CPU.

use std::f32::consts::TAU;
use std::sync::LazyLock;

use core_model::{ImageF32, ParamId, ParamValue};
use tool_api::{
    Category, GpuKernel, MAX_PARAMS, ParamDescriptor, ParamKind, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId,
    Unit,
};

use crate::common::{float, pid, smoothstep};

/// Id of the tool.
pub const ID: &str = "retouch";

/// Most spots in one node (eight parameters each).
pub const MAX_SPOTS: usize = MAX_PARAMS / Field::ALL.len();

/// Largest radius, as a fraction of the image's long side.
pub const MAX_RADIUS: f32 = 0.25;

/// What a spot does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotKind {
    /// Copy the source's texture, matched to the spot's surroundings.
    Heal,
    /// Copy the source as it is.
    Clone,
}

/// One spot, in the developed image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    /// Heal or clone.
    pub kind: SpotKind,
    /// Centre (0–1 from the top-left corner).
    pub at: [f32; 2],
    /// Radius as a fraction of the image's long side (0 to [`MAX_RADIUS`]).
    pub radius: f32,
    /// Centre of the source circle (0–1).
    pub source: [f32; 2],
    /// Part of the radius that fades out (0 = hard edge, 1 = from the centre).
    pub feather: f32,
    /// Strength (0–1).
    pub opacity: f32,
}

/// One stored value of a spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Heal or clone (a choice).
    Kind,
    /// Centre, across.
    X,
    /// Centre, down.
    Y,
    /// Radius.
    Radius,
    /// Source centre, across.
    SourceX,
    /// Source centre, down.
    SourceY,
    /// Feather, in percent.
    Feather,
    /// Opacity, in percent.
    Opacity,
}

impl Field {
    /// Every field, in parameter order.
    pub const ALL: [Self; 8] =
        [Self::Kind, Self::X, Self::Y, Self::Radius, Self::SourceX, Self::SourceY, Self::Feather, Self::Opacity];

    fn key(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::X => "x",
            Self::Y => "y",
            Self::Radius => "r",
            Self::SourceX => "sx",
            Self::SourceY => "sy",
            Self::Feather => "feather",
            Self::Opacity => "opacity",
        }
    }

    fn descriptor(self, slot: usize) -> ParamDescriptor {
        let id = param_id(slot, self);
        match self {
            Self::Kind => ParamDescriptor {
                id,
                label: "Kind",
                kind: ParamKind::Choice { options: vec!["Heal", "Clone"], default: 0 },
            },
            Self::X => float(id.as_str(), "X", [0.0, 1.0], 0.5, 0.001, Unit::None),
            Self::Y => float(id.as_str(), "Y", [0.0, 1.0], 0.5, 0.001, Unit::None),
            Self::Radius => float(id.as_str(), "Size", [0.0, MAX_RADIUS], 0.0, 0.001, Unit::None),
            Self::SourceX => float(id.as_str(), "Source X", [0.0, 1.0], 0.5, 0.001, Unit::None),
            Self::SourceY => float(id.as_str(), "Source Y", [0.0, 1.0], 0.5, 0.001, Unit::None),
            Self::Feather => float(id.as_str(), "Feather", [0.0, 100.0], 50.0, 1.0, Unit::Percent),
            Self::Opacity => float(id.as_str(), "Opacity", [0.0, 100.0], 100.0, 1.0, Unit::Percent),
        }
    }

    /// The value of this field of `spot` as stored.
    fn value(self, spot: &Spot) -> ParamValue {
        match self {
            Self::Kind => ParamValue::Choice(match spot.kind {
                SpotKind::Heal => 0,
                SpotKind::Clone => 1,
            }),
            Self::X => ParamValue::Float(spot.at[0].clamp(0.0, 1.0)),
            Self::Y => ParamValue::Float(spot.at[1].clamp(0.0, 1.0)),
            Self::Radius => ParamValue::Float(spot.radius.clamp(0.0, MAX_RADIUS)),
            Self::SourceX => ParamValue::Float(spot.source[0].clamp(0.0, 1.0)),
            Self::SourceY => ParamValue::Float(spot.source[1].clamp(0.0, 1.0)),
            Self::Feather => ParamValue::Float((spot.feather * 100.0).clamp(0.0, 100.0)),
            Self::Opacity => ParamValue::Float((spot.opacity * 100.0).clamp(0.0, 100.0)),
        }
    }
}

/// The parameter holding `field` of slot `slot` (`spot3_x`).
#[must_use]
pub fn param_id(slot: usize, field: Field) -> ParamId {
    pid(&format!("spot{slot}_{}", field.key()))
}

/// The stored values of `fields` of `spot` in slot `slot`, for one edit.
#[must_use]
pub fn values(slot: usize, spot: &Spot, fields: &[Field]) -> Vec<(ParamId, ParamValue)> {
    fields.iter().map(|&f| (param_id(slot, f), f.value(spot))).collect()
}

/// Every stored value of `spot` in slot `slot`.
#[must_use]
pub fn all_values(slot: usize, spot: &Spot) -> Vec<(ParamId, ParamValue)> {
    values(slot, spot, &Field::ALL)
}

/// The values that empty slot `slot` (its radius back to 0).
#[must_use]
pub fn cleared(slot: usize) -> Vec<(ParamId, ParamValue)> {
    vec![(param_id(slot, Field::Radius), ParamValue::Float(0.0))]
}

/// The spots of a node, by slot (`None` for an empty slot).
#[must_use]
pub fn spots(params: &ResolvedParams) -> [Option<Spot>; MAX_SPOTS] {
    std::array::from_fn(|slot| {
        let f = |field| params.float(param_id(slot, field).as_str());
        let radius = f(Field::Radius);
        (radius > 0.0).then(|| Spot {
            kind: if params.choice(param_id(slot, Field::Kind).as_str()) == 1 {
                SpotKind::Clone
            } else {
                SpotKind::Heal
            },
            at: [f(Field::X), f(Field::Y)],
            radius,
            source: [f(Field::SourceX), f(Field::SourceY)],
            feather: f(Field::Feather) / 100.0,
            opacity: f(Field::Opacity) / 100.0,
        })
    })
}

/// The first empty slot, if any.
#[must_use]
pub fn free_slot(spots: &[Option<Spot>]) -> Option<usize> {
    spots.iter().position(Option::is_none)
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new(ID).expect("constant id is valid"),
    version: 1,
    name: "Retouch",
    category: Category::Details,
    stage: Stage::Graph,
    params: (0..MAX_SPOTS).flat_map(|slot| Field::ALL.map(|f| f.descriptor(slot))).collect(),
});

/// The retouch tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Retouch;

impl Tool for Retouch {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        for spot in spots(params).iter().flatten() {
            apply_spot(image, spot);
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::Host
    }
}

/// A copy of a rectangle of an image's RGB, read with clamping at its edges.
struct Patch {
    x0: i64,
    y0: i64,
    w: i64,
    h: i64,
    px: Vec<[f32; 3]>,
}

impl Patch {
    /// The pixels within `pad` of `centre` (pixel units), inside the image.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)] // pixel positions
    fn copy(image: &ImageF32, centre: [f32; 2], pad: f32) -> Self {
        let (iw, ih) = (i64::from(image.width()), i64::from(image.height()));
        let x0 = ((centre[0] - pad).floor() as i64).clamp(0, iw - 1);
        let y0 = ((centre[1] - pad).floor() as i64).clamp(0, ih - 1);
        let x1 = ((centre[0] + pad).ceil() as i64 + 1).clamp(x0 + 1, iw);
        let y1 = ((centre[1] + pad).ceil() as i64 + 1).clamp(y0 + 1, ih);
        let pixels = image.pixels();
        let mut px = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
        for y in y0..y1 {
            let row = (y * iw) as usize;
            px.extend(pixels[row + x0 as usize..row + x1 as usize].iter().map(|p| [p[0], p[1], p[2]]));
        }
        Self { x0, y0, w: x1 - x0, h: y1 - y0, px }
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)] // clamped to the patch
    fn at(&self, x: i64, y: i64) -> [f32; 3] {
        let (x, y) = ((x - self.x0).clamp(0, self.w - 1), (y - self.y0).clamp(0, self.h - 1));
        self.px[(y * self.w + x) as usize]
    }

    /// Bilinear sample at image position (`x`, `y`); pixel `i` has its
    /// centre at `i + 0.5`.
    #[allow(clippy::cast_possible_truncation)] // pixel positions
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - ix, fy - iy);
        let (ix, iy) = (ix as i64, iy as i64);
        let (a, b, c, d) = (self.at(ix, iy), self.at(ix + 1, iy), self.at(ix, iy + 1), self.at(ix + 1, iy + 1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// Grid step of the heal correction, at most this many cells per radius.
const CELLS_PER_RADIUS: f32 = 24.0;

/// The heal correction: the smooth (harmonic) function inside the spot
/// that equals the spot's border minus the source's border on the circle,
/// computed on a coarse grid and read bilinearly.
struct Correction {
    origin: [f32; 2],
    step: f32,
    n: usize,
    grid: Vec<[f32; 3]>,
}

impl Correction {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // small counts
    fn new(dest: &Patch, src: &Patch, c: [f32; 2], d: [f32; 2], r: f32) -> Self {
        // Border values: the difference averaged over a small window centred
        // just outside the circle, on a circle of radius `rb` (so the defect
        // itself is left out and noise is smoothed away), every 1.5 pixels
        // or so around it. The smooth function is that of the disc of
        // radius `rb`; the spot lies inside it.
        let samples = ((TAU * r / 1.5).round() as usize).clamp(12, 192);
        let half = ((TAU * r / samples as f32 / 2.0).round() as i64).clamp(1, 6);
        let rb = r + half as f32;
        let mut border = Vec::with_capacity(samples);
        for k in 0..samples {
            let phi = TAU * k as f32 / samples as f32;
            let at = [c[0] + rb * phi.cos(), c[1] + rb * phi.sin()];
            let mut sum = [0.0_f32; 3];
            let mut count = 0.0_f32;
            for oy in -half..=half {
                for ox in -half..=half {
                    let q = [at[0] + ox as f32, at[1] + oy as f32];
                    let a = dest.sample(q[0], q[1]);
                    let b = src.sample(q[0] + d[0], q[1] + d[1]);
                    for i in 0..3 {
                        sum[i] += a[i] - b[i];
                    }
                    count += 1.0;
                }
            }
            border.push((at, sum.map(|s| s / count)));
        }
        // The Poisson integral of the disc at each grid node. Normalised,
        // its kernel is 1 / |border point − node|²; nodes within half a
        // sample spacing of the circle are pulled in, where the sum over
        // the samples is still smooth.
        let step = (r / CELLS_PER_RADIUS).max(1.0);
        let n = (2.0 * r / step).ceil() as usize + 2;
        let origin = [c[0] - r, c[1] - r];
        let reach = (rb - 0.5 * TAU * rb / samples as f32).max(0.0);
        let mut grid = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let mut v = [origin[0] + i as f32 * step - c[0], origin[1] + j as f32 * step - c[1]];
                let len = v[0].hypot(v[1]);
                if len > reach {
                    let k = if len > 0.0 { reach / len } else { 0.0 };
                    v = [v[0] * k, v[1] * k];
                }
                let q = [c[0] + v[0], c[1] + v[1]];
                let (mut sum, mut total) = ([0.0_f32; 3], 0.0_f32);
                for (b, e) in &border {
                    let w = 1.0 / ((b[0] - q[0]).powi(2) + (b[1] - q[1]).powi(2)).max(1e-6);
                    for ch in 0..3 {
                        sum[ch] += w * e[ch];
                    }
                    total += w;
                }
                grid.push(sum.map(|s| s / total));
            }
        }
        Self { origin, step, n, grid }
    }

    /// The correction at image position `q`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // grid cells
    fn at(&self, q: [f32; 2]) -> [f32; 3] {
        let last = (self.n - 1) as f32;
        let gx = ((q[0] - self.origin[0]) / self.step).clamp(0.0, last);
        let gy = ((q[1] - self.origin[1]) / self.step).clamp(0.0, last);
        let (ix, iy) = ((gx.floor() as usize).min(self.n - 2), (gy.floor() as usize).min(self.n - 2));
        let (tx, ty) = (gx - ix as f32, gy - iy as f32);
        let g = |x: usize, y: usize| self.grid[y * self.n + x];
        let (a, b, c, d) = (g(ix, iy), g(ix + 1, iy), g(ix, iy + 1), g(ix + 1, iy + 1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// Applies one spot to `image`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // pixel positions
fn apply_spot(image: &mut ImageF32, spot: &Spot) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (wf, hf) = (w as f32, h as f32);
    let r = spot.radius * wf.max(hf);
    if w == 0 || h == 0 || r < 0.5 || spot.opacity <= 0.0 {
        return;
    }
    let c = [spot.at[0] * wf, spot.at[1] * hf];
    let d = [(spot.source[0] - spot.at[0]) * wf, (spot.source[1] - spot.at[1]) * hf];
    // Room for the border windows of the heal correction (at most 2 × 6 pixels).
    let pad = r + 16.0;
    let dest = Patch::copy(image, c, pad);
    let src = Patch::copy(image, [c[0] + d[0], c[1] + d[1]], pad);
    let correction = (spot.kind == SpotKind::Heal).then(|| Correction::new(&dest, &src, c, d, r));
    // At least a pixel of fade, so even a hard edge is smooth.
    let inner = (r * (1.0 - spot.feather.clamp(0.0, 1.0))).min(r - 1.0).max(0.0);
    let x0 = (c[0] - r).floor().max(0.0) as usize;
    let y0 = (c[1] - r).floor().max(0.0) as usize;
    let x1 = ((c[0] + r).ceil().max(0.0) as usize).min(w);
    let y1 = ((c[1] + r).ceil().max(0.0) as usize).min(h);
    let pixels = image.pixels_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let q = [x as f32 + 0.5, y as f32 + 0.5];
            let dist = (q[0] - c[0]).hypot(q[1] - c[1]);
            if dist >= r {
                continue;
            }
            let a = (1.0 - smoothstep(inner, r, dist)) * spot.opacity.min(1.0);
            if a <= 0.0 {
                continue;
            }
            let mut v = src.sample(q[0] + d[0], q[1] + d[1]);
            if let Some(k) = &correction {
                let e = k.at(q);
                for i in 0..3 {
                    v[i] += e[i];
                }
            }
            let p = &mut pixels[y * w + x];
            for i in 0..3 {
                p[i] += a * (v[i] - p[i]);
            }
        }
    }
}

/// A source for a spot at `at` with `radius` (as stored), picked in
/// `image` (the developed image, or a smaller copy of it): the nearby
/// circle, two to five radii away, whose surroundings look most like the
/// spot's surroundings and whose inside is as even as they are. The whole
/// source circle stays inside the image when it fits.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // pixel positions
pub fn find_source(image: &ImageF32, at: [f32; 2], radius: f32) -> [f32; 2] {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (wf, hf) = (w as f32, h as f32);
    let r = (radius * wf.max(hf)).max(1.0);
    let fallback = || {
        let x = if at[0] < 0.5 { at[0] + 2.5 * radius } else { at[0] - 2.5 * radius };
        [x.clamp(0.0, 1.0), at[1].clamp(0.0, 1.0)]
    };
    if w == 0 || h == 0 {
        return fallback();
    }
    let c = [at[0] * wf, at[1] * hf];
    // Values in a roughly perceptual scale, nearest pixel.
    let pixels = image.pixels();
    let value = |p: [f32; 2]| -> [f32; 3] {
        let x = (p[0].max(0.0) as usize).min(w - 1);
        let y = (p[1].max(0.0) as usize).min(h - 1);
        let px = pixels[y * w + x];
        [px[0].max(0.0).sqrt(), px[1].max(0.0).sqrt(), px[2].max(0.0).sqrt()]
    };
    let ring = |n: usize, radii: &[f32]| -> Vec<[f32; 2]> {
        radii
            .iter()
            .flat_map(|&k| {
                (0..n).map(move |i| {
                    let a = TAU * (i as f32 + 0.5 * k) / n as f32;
                    [k * r * a.cos(), k * r * a.sin()]
                })
            })
            .collect()
    };
    let around = ring(24, &[1.2, 1.5]);
    let mut inside = ring(12, &[0.35, 0.7]);
    inside.push([0.0, 0.0]);
    let luma = |v: [f32; 3]| (v[0] + v[1] + v[2]) / 3.0;
    let stats = |values: &[f32]| {
        let n = values.len().max(1) as f32;
        let mean = values.iter().sum::<f32>() / n;
        let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n;
        (mean, var.sqrt())
    };
    let near: Vec<[f32; 3]> = around.iter().map(|o| value([c[0] + o[0], c[1] + o[1]])).collect();
    let (near_mean, near_std) = stats(&near.iter().map(|v| luma(*v)).collect::<Vec<_>>());
    let fits = |v: f32, size: f32| if size > 2.0 * r { (r..=size - r).contains(&v) } else { (0.0..=size).contains(&v) };

    let mut best: Option<(f32, [f32; 2])> = None;
    for (ring_index, factor) in [2.2_f32, 2.8, 3.6, 4.6].into_iter().enumerate() {
        for k in 0..24 {
            let a = TAU * (k as f32 + 0.5 * (ring_index % 2) as f32) / 24.0;
            let cand = [c[0] + factor * r * a.cos(), c[1] + factor * r * a.sin()];
            if !fits(cand[0], wf) || !fits(cand[1], hf) {
                continue;
            }
            // Surroundings against surroundings, leaving out the spot itself.
            let (mut ssd, mut count) = (0.0_f32, 0_usize);
            for (o, dv) in around.iter().zip(&near) {
                let q = [cand[0] + o[0], cand[1] + o[1]];
                if (q[0] - c[0]).hypot(q[1] - c[1]) < 1.05 * r {
                    continue;
                }
                let v = value(q);
                ssd += (0..3).map(|i| (v[i] - dv[i]).powi(2)).sum::<f32>();
                count += 1;
            }
            if count < around.len() / 2 {
                continue;
            }
            let ssd = ssd / count as f32;
            // The inside should be like the spot's surroundings: no edge, no other spot.
            let (mean, std) =
                stats(&inside.iter().map(|o| luma(value([cand[0] + o[0], cand[1] + o[1]]))).collect::<Vec<_>>());
            let score = (ssd + 3.0 * ((mean - near_mean).powi(2) + (std - near_std).powi(2))) * (1.0 + 0.05 * factor);
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, cand));
            }
        }
    }
    best.map_or_else(fallback, |(_, p)| [(p[0] / wf).clamp(0.0, 1.0), (p[1] / hf).clamp(0.0, 1.0)])
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::float_cmp)]
mod tests {
    use super::*;
    use core_model::{Params, WorkingSpace};

    const SIZE: usize = 128;

    fn image(f: impl Fn(f32, f32) -> f32) -> ImageF32 {
        let px = (0..SIZE * SIZE)
            .map(|i| {
                let (x, y) = ((i % SIZE) as f32 / SIZE as f32, (i / SIZE) as f32 / SIZE as f32);
                let v = f(x, y);
                [v, v * 0.8, v * 0.6, 1.0]
            })
            .collect();
        ImageF32::from_pixels(128, 128, WorkingSpace::LinearRec2020, px).expect("128×128")
    }

    /// A smooth, curved gradient (not harmonic, so heal has work to do).
    fn smooth(x: f32, y: f32) -> f32 {
        0.15 + 0.4 * x * x + 0.2 * (3.0 * y).sin() + 0.1 * x * y
    }

    fn run(img: &mut ImageF32, slot_spots: &[(usize, Spot)]) {
        let mut p = Params::new();
        for (slot, spot) in slot_spots {
            for (id, v) in all_values(*slot, spot) {
                p.set(id, v);
            }
        }
        let r = ResolvedParams::resolve(Retouch.descriptor(), &p);
        Retouch.apply_cpu(img, &r);
    }

    fn spot(kind: SpotKind) -> Spot {
        Spot { kind, at: [0.5, 0.5], radius: 10.0 / 128.0, source: [0.75, 0.3], feather: 0.3, opacity: 1.0 }
    }

    fn max_error(a: &ImageF32, b: &ImageF32) -> f32 {
        a.pixels()
            .iter()
            .zip(b.pixels())
            .flat_map(|(p, q)| (0..3).map(move |i| (p[i] - q[i]).abs()))
            .fold(0.0, f32::max)
    }

    #[test]
    fn the_descriptor_fills_the_parameters_and_spots_round_trip() {
        let d = Retouch.descriptor();
        assert_eq!(d.validate(), Ok(()));
        assert_eq!(d.params.len(), MAX_SPOTS * Field::ALL.len());
        assert!(d.params.len() <= MAX_PARAMS);
        let s = Spot {
            kind: SpotKind::Clone,
            at: [0.25, 0.75],
            radius: 0.02,
            source: [0.1, 0.9],
            feather: 0.4,
            opacity: 0.6,
        };
        let mut p = Params::new();
        for (id, v) in all_values(5, &s) {
            p.set(id, v);
        }
        let got = spots(&ResolvedParams::resolve(d, &p));
        assert_eq!(got.iter().flatten().count(), 1);
        let back = got[5].expect("slot 5");
        assert_eq!(back.kind, SpotKind::Clone);
        assert!((back.at[0] - 0.25).abs() < 1e-6 && (back.source[1] - 0.9).abs() < 1e-6);
        assert!((back.feather - 0.4).abs() < 1e-6 && (back.opacity - 0.6).abs() < 1e-6);
        assert_eq!(free_slot(&got), Some(0));
        // Clearing empties the slot.
        for (id, v) in cleared(5) {
            p.set(id, v);
        }
        assert!(spots(&ResolvedParams::resolve(d, &p)).iter().all(Option::is_none));
        assert_eq!(param_id(3, Field::SourceX).as_str(), "spot3_sx");
    }

    #[test]
    fn heal_leaves_no_seam_on_a_smooth_gradient() {
        let clean = image(smooth);
        let mut healed = clean.clone();
        run(&mut healed, &[(0, spot(SpotKind::Heal))]);
        // The source is far away and brighter: heal matches it to the spot.
        let err = max_error(&healed, &clean);
        assert!(err < 1.5e-3, "heal error {err}");
        // A large spot (its correction is computed on a coarser grid).
        let mut healed = clean.clone();
        let big = Spot { radius: MAX_RADIUS, source: [0.5 + 12.0 / 128.0, 0.5], ..spot(SpotKind::Heal) };
        run(&mut healed, &[(0, big)]);
        let err = max_error(&healed, &clean);
        assert!(err < 1.5e-3, "large heal error {err}");
        // Clone of the same source shows a visible step.
        let mut cloned = clean.clone();
        run(&mut cloned, &[(0, Spot { feather: 0.0, ..spot(SpotKind::Clone) })]);
        assert!(max_error(&cloned, &clean) > 0.05);
    }

    #[test]
    fn heal_removes_a_blemish_and_keeps_the_source_texture() {
        let clean = image(|x, y| smooth(x, y) + 0.03 * ((x * 128.0).floor() as i32 % 2) as f32);
        let mut img = clean.clone();
        // A dark blemish of radius 5 at the centre.
        for (i, p) in img.pixels_mut().iter_mut().enumerate() {
            let (x, y) = ((i % SIZE) as f32 + 0.5 - 64.0, (i / SIZE) as f32 + 0.5 - 64.0);
            if x.hypot(y) < 5.0 {
                p[0] = 0.01;
                p[1] = 0.01;
                p[2] = 0.01;
            }
        }
        // Source shifted by a whole even number of pixels, so the stripes line up.
        let s = Spot { source: [0.5 + 30.0 / 128.0, 0.5 + 10.0 / 128.0], ..spot(SpotKind::Heal) };
        run(&mut img, &[(0, s)]);
        let err = max_error(&img, &clean);
        assert!(err < 1e-3, "{err}");
        // The stripes are still there in the middle.
        let at = |x: usize| img.pixels()[64 * SIZE + x][0];
        assert!((at(64) - at(63)).abs() > 0.02);
    }

    #[test]
    fn clone_copies_the_source_and_respects_opacity_and_size() {
        let clean = image(smooth);
        let s = Spot { feather: 0.0, ..spot(SpotKind::Clone) };
        let mut img = clean.clone();
        run(&mut img, &[(2, s)]);
        let px = |im: &ImageF32, x: usize, y: usize| im.pixels()[y * SIZE + x][0];
        // The centre pixel (64, 64) takes the source's: (96, 38.4) between
        // pixel centres, so 0.6 of row 38 and 0.4 of row 39.
        let expected = px(&clean, 96, 38) * 0.6 + px(&clean, 96, 39) * 0.4;
        assert!((px(&img, 64, 64) - expected).abs() < 1e-3, "{} {expected}", px(&img, 64, 64));
        // Outside the circle nothing changes.
        assert_eq!(px(&img, 64, 80), px(&clean, 64, 80));
        // Half opacity goes half way.
        let mut half = clean.clone();
        run(&mut half, &[(0, Spot { opacity: 0.5, ..s })]);
        let mid = (px(&clean, 64, 64) + px(&img, 64, 64)) / 2.0;
        assert!((px(&half, 64, 64) - mid).abs() < 1e-4);
        // The same spot on a half-size image gives the same picture.
        let small = ImageF32::from_pixels(
            64,
            64,
            WorkingSpace::LinearRec2020,
            (0..64 * 64)
                .map(|i| {
                    let v = smooth((i % 64) as f32 / 64.0, (i / 64) as f32 / 64.0);
                    [v, v * 0.8, v * 0.6, 1.0]
                })
                .collect(),
        )
        .expect("64×64");
        let mut small = small;
        run(&mut small, &[(0, s)]);
        assert!((small.pixels()[32 * 64 + 32][0] - px(&img, 64, 64)).abs() < 0.01);
    }

    #[test]
    fn spots_overlapping_their_source_and_at_the_border_do_not_panic() {
        let clean = image(smooth);
        let mut img = clean.clone();
        let edge = Spot { at: [0.0, 1.0], source: [1.0, 0.0], radius: MAX_RADIUS, ..spot(SpotKind::Heal) };
        let overlap = Spot { source: [0.52, 0.5], ..spot(SpotKind::Clone) };
        run(&mut img, &[(0, edge), (1, overlap)]);
        assert!(img.pixels().iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn the_source_is_found_nearby_in_similar_texture() {
        // Left half: fine stripes; right half: flat. A spot on the stripes
        // near the middle line takes its source from the stripes.
        let img = image(|x, _| if x < 0.5 { 0.2 + 0.1 * ((x * 128.0).floor() as i32 % 2) as f32 } else { 0.25 });
        let r = 6.0 / 128.0;
        let at = [0.40, 0.5];
        let s = find_source(&img, at, r);
        let dist = (s[0] - at[0]).hypot(s[1] - at[1]);
        assert!(dist >= 2.0 * r && dist <= 5.0 * r, "{s:?}");
        assert!(s[0] + r < 0.5 + 1e-3, "{s:?} reaches into the flat half");
        // On the flat half it stays there.
        let s = find_source(&img, [0.6, 0.5], r);
        assert!(s[0] - r >= 0.5 - 1e-3, "{s:?}");
        // Near a corner the source circle stays inside the image.
        let s = find_source(&img, [0.02, 0.02], r);
        assert!(s[0] >= r - 1e-4 && s[1] >= r - 1e-4, "{s:?}");
    }
}
