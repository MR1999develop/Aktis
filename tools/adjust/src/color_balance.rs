//! Colour balance, as in Capture One's three-way tool: for shadows,
//! midtones and highlights, a hue to tint them with, how much, and a
//! lightness.
//!
//! The three bands are measured in stops from middle grey (shadows below
//! about 3 stops under, highlights above about 3 stops over, midtones in
//! between, blending smoothly). A tint moves R, G and B by up to ±0.6 stop
//! along the hue's direction, which adds no grey; lightness moves the band
//! by up to ±0.8 stop.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ParamDescriptor, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{float, hue_direction, luma, slider, smoothstep, stops_from_grey};

/// The three bands, in parameter order: id prefix and name.
pub const ZONES: [(&str, &str); 3] = [("shadows", "Shadows"), ("midtones", "Midtones"), ("highlights", "Highlights")];

/// Id of a band's hue (degrees).
#[must_use]
pub fn hue_id(zone: usize) -> String {
    format!("{}_hue", ZONES[zone].0)
}

/// Id of a band's tint amount (0–100).
#[must_use]
pub fn amount_id(zone: usize) -> String {
    format!("{}_amount", ZONES[zone].0)
}

/// Id of a band's lightness (−100 to 100).
#[must_use]
pub fn lightness_id(zone: usize) -> String {
    format!("{}_lightness", ZONES[zone].0)
}

/// Id of the master tint's hue (degrees), applied to every tone.
pub const MASTER_HUE: &str = "master_hue";
/// Id of the master tint's amount (0–100).
pub const MASTER_AMOUNT: &str = "master_amount";
/// Id of the master lightness (−100 to 100).
pub const MASTER_LIGHTNESS: &str = "master_lightness";
/// Id of the master saturation (−100 to 100).
pub const MASTER_SATURATION: &str = "master_saturation";

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| {
    let mut params: Vec<ParamDescriptor> = (0..ZONES.len())
        .flat_map(|z| -> [ParamDescriptor; 3] {
            [
                float(&hue_id(z), "Hue", [0.0, 360.0], 0.0, 1.0, Unit::Degrees),
                float(&amount_id(z), "Amount", [0.0, 100.0], 0.0, 1.0, Unit::None),
                slider(&lightness_id(z), "Lightness"),
            ]
        })
        .collect();
    params.push(float(MASTER_HUE, "Hue", [0.0, 360.0], 0.0, 1.0, Unit::Degrees));
    params.push(float(MASTER_AMOUNT, "Amount", [0.0, 100.0], 0.0, 1.0, Unit::None));
    params.push(slider(MASTER_LIGHTNESS, "Lightness"));
    params.push(slider(MASTER_SATURATION, "Saturation"));
    ToolDescriptor {
        id: ToolId::new("color_balance").expect("constant id is valid"),
        version: 2,
        name: "Colour balance",
        category: Category::Color,
        order: 90,
        starts_on: true,
        params,
    }
});

/// Weights of the shadows, midtones and highlights for a tone `e` stops from grey.
#[must_use]
pub fn zone_weights(e: f32) -> [f32; 3] {
    let shadows = 1.0 - smoothstep(-3.5, 0.0, e);
    let highlights = smoothstep(0.0, 3.0, e);
    [shadows, (1.0 - shadows - highlights).clamp(0.0, 1.0), highlights]
}

/// The colour-balance tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct ColorBalance;

impl Tool for ColorBalance {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        // Shadows, midtones, highlights, then the master (weight 1).
        let zones: [([f32; 3], f32, f32); 4] = std::array::from_fn(|z| match z {
            3 => (hue_direction(params.float(MASTER_HUE)), params.float(MASTER_AMOUNT), params.float(MASTER_LIGHTNESS)),
            _ => (hue_direction(params.float(&hue_id(z))), params.float(&amount_id(z)), params.float(&lightness_id(z))),
        });
        let saturation = params.float(MASTER_SATURATION);
        if zones.iter().all(|(_, a, l)| *a == 0.0 && *l == 0.0) && saturation == 0.0 {
            return;
        }
        for p in image.pixels_mut() {
            let Some(e) = stops_from_grey(luma([p[0], p[1], p[2]])) else { continue };
            let zw = zone_weights(e);
            let w = [zw[0], zw[1], zw[2], 1.0];
            for (ch, v) in p[..3].iter_mut().enumerate() {
                let stops: f32 =
                    zones.iter().zip(w).map(|((dir, a, l), w)| w * (a / 100.0 * 0.6 * dir[ch] + l / 100.0 * 0.8)).sum();
                *v *= stops.exp2();
            }
            if saturation != 0.0 {
                let y = luma([p[0], p[1], p[2]]);
                let k = 1.0 + saturation / 100.0;
                for v in &mut p[..3] {
                    *v = y + (*v - y) * k;
                }
            }
        }
    }

    /// Version 1 had no master: its parameters are valid as they are.
    fn migrate(&self, from_version: u32, params: tool_api::Params) -> Result<tool_api::Params, tool_api::ToolError> {
        match from_version {
            1 | 2 => Ok(params),
            found => Err(tool_api::ToolError::UnsupportedVersion { found, current: 2 }),
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel {
            wgsl: "
    let y = dot(c.rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    if (y <= 1e-6) { return c; }
    let e = log2(y / 0.18);
    let ts = clamp((e + 3.5) / 3.5, 0.0, 1.0);
    let th = clamp(e / 3.0, 0.0, 1.0);
    let ws = 1.0 - ts * ts * (3.0 - 2.0 * ts);
    let wh = th * th * (3.0 - 2.0 * th);
    let w = vec4<f32>(ws, clamp(1.0 - ws - wh, 0.0, 1.0), wh, 1.0);
    var stops = vec3<f32>(0.0);
    for (var z = 0u; z < 4u; z++) {
        let b = z * 3u;
        let hue = radians(params.v[b / 4u][b % 4u]);
        let amount = params.v[(b + 1u) / 4u][(b + 1u) % 4u];
        let light = params.v[(b + 2u) / 4u][(b + 2u) % 4u];
        let third = 6.2831853 / 3.0;
        let dir = vec3<f32>(cos(hue), cos(hue - third), cos(hue - 2.0 * third));
        stops = stops + w[z] * (amount / 100.0 * 0.6 * dir + vec3<f32>(light / 100.0 * 0.8));
    }
    var rgb = c.rgb * exp2(stops);
    let sat = params.v[3].x;
    if (sat != 0.0) {
        let y2 = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
        rgb = vec3<f32>(y2) + (rgb - vec3<f32>(y2)) * (1.0 + sat / 100.0);
    }
    return vec4<f32>(rgb, c.a);",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    fn run(values: &[(String, f32)], rgb: [f32; 3]) -> [f32; 3] {
        let mut p = Params::new();
        for (id, v) in values {
            p.set(crate::common::pid(id), ParamValue::Float(*v));
        }
        let r = ResolvedParams::resolve(ColorBalance.descriptor(), &p);
        let mut img =
            ImageF32::from_pixels(1, 1, WorkingSpace::LinearRec2020, vec![[rgb[0], rgb[1], rgb[2], 1.0]]).expect("1×1");
        ColorBalance.apply_cpu(&mut img, &r);
        let o = img.pixels()[0];
        [o[0], o[1], o[2]]
    }

    #[test]
    fn the_bands_cover_every_tone_once() {
        for i in 0..=100 {
            let e = -8.0 + i as f32 * 0.16;
            let w = zone_weights(e);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{e}: {w:?}");
        }
        assert!(zone_weights(-6.0)[0] > 0.99 && zone_weights(0.0)[1] > 0.99 && zone_weights(5.0)[2] > 0.99);
    }

    #[test]
    fn a_blue_shadow_tint_leaves_the_highlights_alone() {
        let tint = [(hue_id(0), 240.0), (amount_id(0), 100.0)];
        let dark = run(&tint, [0.01, 0.01, 0.01]);
        assert!(dark[2] > dark[0] * 1.5, "{dark:?}");
        let bright = run(&tint, [2.0, 2.0, 2.0]);
        assert!((bright[2] - bright[0]).abs() < 1e-5, "{bright:?}");
    }

    #[test]
    fn the_master_tints_every_tone_and_sets_saturation() {
        let warm = [(MASTER_HUE.to_owned(), 30.0), (MASTER_AMOUNT.to_owned(), 100.0)];
        for grey in [0.01, 0.18, 2.0] {
            let o = run(&warm, [grey, grey, grey]);
            assert!(o[0] > o[2], "{grey}: {o:?}");
        }
        let flat = run(&[(MASTER_SATURATION.to_owned(), -100.0)], [0.4, 0.2, 0.1]);
        assert!((flat[0] - flat[2]).abs() < 1e-6, "{flat:?}");
    }

    #[test]
    fn lightness_moves_its_band() {
        let mid = run(&[(lightness_id(1), 100.0)], [0.18, 0.18, 0.18]);
        assert!((mid[0] / 0.18 - 0.8_f32.exp2()).abs() < 1e-4, "{mid:?}");
    }
}
