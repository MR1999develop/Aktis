//! White balance, as in Capture One: a mode (as shot, automatic, a light
//! source or custom), a colour temperature in kelvin and a tint.
//!
//! The Source develops the RAW file for the light the camera measured ("as
//! shot"). This tool says the light was another one, `kelvin` and `tint`:
//! colours are taken back to the scene under the as-shot light and adapted
//! from the new light to the working space's white (Bradford), one 3×3
//! matrix per image ([`core_color::rebalance`]).
//!
//! The as-shot light is stored with the tool (`shot_kelvin`, `shot_tint`),
//! written by the editor when a layer starts using it, so the recipe alone
//! says what the tool does. The mode only records how the values were chosen;
//! the editor sets `kelvin` and `tint` when it changes.

use std::sync::LazyLock;

use core_color::{REC2020_TO_XYZ, kelvin_tint_to_xy, rebalance, white_of, xy_to_kelvin_tint};
use core_model::{ImageF32, Matrix3};
use tool_api::{
    Category, GpuKernel, MAX_PARAMS, ParamDescriptor, ParamKind, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit,
};

use crate::common::{float, pid};

/// Id of the mode parameter (a [`Mode`] index).
pub const MODE: &str = "mode";
/// Id of the temperature of the light, in kelvin.
pub const KELVIN: &str = "kelvin";
/// Id of the tint of the light (positive = greener light, corrected
/// towards magenta).
pub const TINT: &str = "tint";
/// Id of the as-shot temperature (set by the editor).
pub const SHOT_KELVIN: &str = "shot_kelvin";
/// Id of the as-shot tint (set by the editor).
pub const SHOT_TINT: &str = "shot_tint";

/// Range of the temperature slider.
pub const KELVIN_RANGE: [f32; 2] = [2000.0, 15000.0];
/// Range of the tint slider.
pub const TINT_RANGE: [f32; 2] = [-150.0, 150.0];

/// How the light was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The camera's measurement.
    Shot,
    /// Estimated from the image (the average colour made grey).
    Auto,
    /// Sunlight.
    Daylight,
    /// Overcast sky.
    Cloudy,
    /// Open shade.
    Shade,
    /// Incandescent bulbs.
    Tungsten,
    /// Fluorescent tubes.
    Fluorescent,
    /// Electronic flash.
    Flash,
    /// Set by hand or with the picker.
    Custom,
}

impl Mode {
    /// Every mode, in menu order (the parameter's choice index).
    pub const ALL: [Self; 9] = [
        Self::Shot,
        Self::Auto,
        Self::Daylight,
        Self::Cloudy,
        Self::Shade,
        Self::Tungsten,
        Self::Fluorescent,
        Self::Flash,
        Self::Custom,
    ];

    /// Name in the menu.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Shot => "Shot",
            Self::Auto => "Auto",
            Self::Daylight => "Daylight",
            Self::Cloudy => "Cloudy",
            Self::Shade => "Shade",
            Self::Tungsten => "Tungsten",
            Self::Fluorescent => "Fluorescent",
            Self::Flash => "Flash",
            Self::Custom => "Custom",
        }
    }

    /// The choice index.
    #[must_use]
    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|m| *m == self).and_then(|i| u32::try_from(i).ok()).unwrap_or(0)
    }

    /// The mode of a choice index (Custom for unknown ones).
    #[must_use]
    pub fn from_index(i: u32) -> Self {
        usize::try_from(i).ok().and_then(|i| Self::ALL.get(i)).copied().unwrap_or(Self::Custom)
    }

    /// Kelvin and tint of a light-source preset; `None` for Shot, Auto and Custom.
    #[must_use]
    pub fn preset(self) -> Option<(f32, f32)> {
        match self {
            Self::Daylight => Some((5500.0, 0.0)),
            Self::Cloudy => Some((6500.0, 0.0)),
            Self::Shade => Some((7500.0, 0.0)),
            Self::Tungsten => Some((2850.0, 0.0)),
            Self::Fluorescent => Some((3800.0, 15.0)),
            Self::Flash => Some((5900.0, 0.0)),
            Self::Shot | Self::Auto | Self::Custom => None,
        }
    }
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("white_balance").expect("constant id is valid"),
    version: 1,
    name: "White balance",
    category: Category::Color,
    order: 40,
    starts_on: true,
    params: vec![
        ParamDescriptor {
            id: pid(MODE),
            label: "Mode",
            kind: ParamKind::Choice { options: Mode::ALL.iter().map(|m| m.label()).collect(), default: 0 },
        },
        float(KELVIN, "Kelvin", KELVIN_RANGE, 5500.0, 10.0, Unit::Kelvin),
        float(TINT, "Tint", TINT_RANGE, 0.0, 0.1, Unit::None),
        float(SHOT_KELVIN, "As-shot kelvin", [1500.0, 25000.0], 5500.0, 1.0, Unit::Kelvin),
        float(SHOT_TINT, "As-shot tint", [-200.0, 200.0], 0.0, 0.1, Unit::None),
    ],
});

/// The white-balance tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct WhiteBalance;

/// The re-balancing matrix for these parameters (identity when the light is the as-shot one).
#[must_use]
pub fn matrix(params: &ResolvedParams) -> Matrix3 {
    let shot = kelvin_tint_to_xy(f64::from(params.float(SHOT_KELVIN)), f64::from(params.float(SHOT_TINT)));
    let target = kelvin_tint_to_xy(f64::from(params.float(KELVIN)), f64::from(params.float(TINT)));
    rebalance(REC2020_TO_XYZ, shot, target)
}

/// Kelvin and tint of the light whose white is `xy`, limited to the sliders' ranges.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // slider values
pub fn light_of(xy: [f64; 2]) -> (f32, f32) {
    let (k, t) = xy_to_kelvin_tint(xy);
    ((k as f32).clamp(KELVIN_RANGE[0], KELVIN_RANGE[1]), (t as f32).clamp(TINT_RANGE[0], TINT_RANGE[1]))
}

/// The light (kelvin, tint) that makes `rgb` neutral, for an image
/// developed for the as-shot light `shot` (kelvin, tint): what the picker
/// sets when the user clicks something that should be grey. `None` for
/// black.
#[must_use]
pub fn neutral_light(rgb: [f32; 3], shot: (f32, f32)) -> Option<(f32, f32)> {
    let shot = kelvin_tint_to_xy(f64::from(shot.0), f64::from(shot.1));
    white_of(REC2020_TO_XYZ, rgb, shot).map(light_of)
}

impl Tool for WhiteBalance {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let m = matrix(params);
        for p in image.pixels_mut() {
            let rgb = m.apply([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&rgb);
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel {
            wgsl: "
    let rgb = vec3<f32>(dot(params.v[0].xyz, c.rgb), dot(params.v[1].xyz, c.rgb), dot(params.v[2].xyz, c.rgb));
    return vec4<f32>(rgb, c.a);",
        }
    }

    /// The matrix's rows in `v[0..3].xyz`.
    fn gpu_uniforms(&self, params: &ResolvedParams) -> [f32; MAX_PARAMS] {
        let m = matrix(params);
        let mut out = [0.0; MAX_PARAMS];
        for (i, row) in m.0.iter().enumerate() {
            out[i * 4..i * 4 + 3].copy_from_slice(row);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    fn params(values: &[(&str, f32)]) -> ResolvedParams {
        let mut p = Params::new();
        for (id, v) in values {
            p.set(pid(id), ParamValue::Float(*v));
        }
        ResolvedParams::resolve(WhiteBalance.descriptor(), &p)
    }

    fn run(values: &[(&str, f32)], rgb: [f32; 3]) -> [f32; 3] {
        let mut img =
            ImageF32::from_pixels(1, 1, WorkingSpace::LinearRec2020, vec![[rgb[0], rgb[1], rgb[2], 1.0]]).expect("1×1");
        WhiteBalance.apply_cpu(&mut img, &params(values));
        let p = img.pixels()[0];
        [p[0], p[1], p[2]]
    }

    #[test]
    fn the_as_shot_light_changes_nothing() {
        let shot = [(SHOT_KELVIN, 4300.0), (SHOT_TINT, 7.0), (KELVIN, 4300.0), (TINT, 7.0)];
        let out = run(&shot, [0.3, 0.2, 0.1]);
        for (a, b) in out.iter().zip([0.3, 0.2, 0.1]) {
            assert!((a - b).abs() < 1e-4, "{out:?}");
        }
    }

    #[test]
    fn a_lower_kelvin_cools_and_a_higher_one_warms() {
        let cooler = run(&[(KELVIN, 3000.0)], [0.5, 0.5, 0.5]);
        assert!(cooler[2] > cooler[0], "{cooler:?}");
        let warmer = run(&[(KELVIN, 9000.0)], [0.5, 0.5, 0.5]);
        assert!(warmer[0] > warmer[2], "{warmer:?}");
        let magenta = run(&[(TINT, 40.0)], [0.5, 0.5, 0.5]);
        assert!(magenta[1] < magenta[0] && magenta[1] < magenta[2], "{magenta:?}");
    }

    #[test]
    fn the_gpu_gets_the_matrix_rows() {
        let p = params(&[(KELVIN, 3000.0)]);
        let u = WhiteBalance.gpu_uniforms(&p);
        let m = matrix(&p);
        assert_eq!(&u[0..3], &m.0[0]);
        assert_eq!(&u[4..7], &m.0[1]);
        assert_eq!(&u[8..11], &m.0[2]);
    }

    #[test]
    fn the_picked_light_makes_the_picked_colour_grey() {
        let shot = (5200.0, 3.0);
        let warm = [0.42_f32, 0.35, 0.22];
        let (k, t) = neutral_light(warm, shot).expect("a light");
        assert!(k < 5200.0, "a warm cast means a warmer (lower) light: {k}");
        let out = run(&[(SHOT_KELVIN, shot.0), (SHOT_TINT, shot.1), (KELVIN, k), (TINT, t)], warm);
        assert!((out[0] - out[1]).abs() < 3e-3 && (out[1] - out[2]).abs() < 3e-3, "{out:?}");
        assert_eq!(neutral_light([0.0; 3], shot), None);
    }

    #[test]
    fn modes_round_trip_and_presets_are_in_range() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_index(m.index()), m);
            if let Some((k, t)) = m.preset() {
                assert!(
                    (KELVIN_RANGE[0]..=KELVIN_RANGE[1]).contains(&k) && (TINT_RANGE[0]..=TINT_RANGE[1]).contains(&t)
                );
            }
        }
        assert_eq!(Mode::from_index(99), Mode::Custom);
    }
}
