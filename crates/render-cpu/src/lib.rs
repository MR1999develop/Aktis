//! Reference renderer on the CPU.
//!
//! Renders a photo's [`Layers`] with each tool's [`Tool::apply`] (its CPU
//! reference, or its own engine for host tools) and turns the result into
//! display pixels. It favours clarity over speed: the GPU engine must
//! produce the same pixels (within a stated tolerance), and this crate is
//! what its tests compare against. It is also the fallback without a GPU.
//!
//! The Background's tools act on the developed image in the fixed order of
//! the tools; then each layer that is switched on, from the bottom, runs
//! its own tools on the result and mixes them in through its mask:
//! `out = in + mask · (tools(in) − in)`. Masks are built from the developed
//! image by the `masks` crate, the same code the GPU engine uses, so both
//! engines see identical masks.
//!
//! [`Tool::apply`]: tool_api::Tool::apply

mod display;
pub mod resize;
pub mod tone;

use core_model::{ImageF32, LayerId, Layers, MaskImage, Orientation, ToolId, ToolRef, Tools};
use masks::{NoRasters, RasterSource};
use tool_api::{ResolvedParams, ToolError, ToolRegistry};

/// The rendered image and what could not be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// Linear image in the source's working space.
    pub image: ImageF32,
    /// Tools used by the recipe but missing from this build. They passed the
    /// image through unchanged; their parameters are kept.
    pub missing_tools: Vec<ToolId>,
    /// Tools that could not run (for example a missing model file). They
    /// passed the image through unchanged.
    pub tool_errors: Vec<(ToolRef, ToolError)>,
    /// AI mask components whose raster is not computed: (layer, component
    /// index). They counted as empty, so the layer had no effect there.
    pub missing_masks: Vec<(LayerId, usize)>,
}

impl Rendered {
    /// `image` with nothing missing yet.
    #[must_use]
    pub fn passing(image: ImageF32) -> Self {
        Self { image, missing_tools: Vec::new(), tool_errors: Vec::new(), missing_masks: Vec::new() }
    }
}

/// Renders `layers` on `source` (the developed image), with no AI rasters:
/// every AI mask component counts as not computed.
#[must_use]
pub fn render(layers: &Layers, registry: &ToolRegistry, source: &ImageF32) -> Rendered {
    render_with_masks(layers, registry, source, &NoRasters)
}

/// Renders `layers` on `source` (the developed image), taking AI rasters from `rasters`.
#[must_use]
pub fn render_with_masks(
    layers: &Layers,
    registry: &ToolRegistry,
    source: &ImageF32,
    rasters: &dyn RasterSource,
) -> Rendered {
    let order = registry.pipeline();
    let mut out = Rendered::passing(source.clone());
    apply_tools(&layers.background, LayerId::BACKGROUND, &order, registry, &mut out);
    for layer in layers.layers().iter().filter(|l| l.enabled) {
        if layer.tools.steps(&order).is_empty() {
            continue;
        }
        let evaluated = masks::evaluate(&layer.mask, rasters, source);
        out.missing_masks.extend(evaluated.missing.iter().map(|c| (layer.id, *c)));
        let before = out.image.clone();
        apply_tools(&layer.tools, layer.id, &order, registry, &mut out);
        blend(&mut out.image, &before, &evaluated.mask);
    }
    out
}

/// Runs the tools of one layer on `out.image`, in `order`, noting the
/// missing tools and the failures in `out`.
pub fn apply_tools(tools: &Tools, layer: LayerId, order: &[ToolId], registry: &ToolRegistry, out: &mut Rendered) {
    for (tool_id, slot, params) in tools.steps(order) {
        let Some(tool) = registry.get(tool_id) else {
            if !out.missing_tools.contains(tool_id) {
                out.missing_tools.push(tool_id.clone());
            }
            continue;
        };
        let params = ResolvedParams::resolve(tool.descriptor(), params);
        if let Err(e) = tool.apply(&mut out.image, &params) {
            out.tool_errors.push((ToolRef::new(layer, tool_id.clone()).with_slot(slot), e));
        }
    }
}

/// `after ← before + mask · (after − before)`, every channel. Masks are built
/// at the source's size and every tool keeps that size.
///
/// # Panics
/// When the sizes differ (a bug): mixing only part of the image would show
/// and export a wrong result without a word. The render workers catch it.
pub fn blend(after: &mut ImageF32, before: &ImageF32, mask: &MaskImage) {
    let size = (after.width(), after.height());
    assert!(
        (mask.width(), mask.height()) == size && (before.width(), before.height()) == size,
        "a layer's mask or the image under it does not have the image's size"
    );
    for ((a, b), m) in after.pixels_mut().iter_mut().zip(before.pixels()).zip(mask.values()) {
        for (x, y) in a.iter_mut().zip(b) {
            *x = y + m * (*x - y);
        }
    }
}

/// Converts a linear image to 8-bit sRGB for display or JPEG export, with
/// the output render of [`tone`]: a film-like curve on stops (even exposure
/// changes, toe and shoulder instead of clipping), hues kept, colours
/// outside sRGB desaturated.
#[must_use]
pub fn to_srgb8(image: &ImageF32) -> Vec<u8> {
    display::to_srgb8(image)
}

/// Like [`to_srgb8`], turned upright by `orientation` in the same pass:
/// returns the upright width, height and pixels.
#[must_use]
pub fn to_srgb8_oriented(image: &ImageF32, orientation: Orientation) -> (u32, u32, Vec<u8>) {
    display::to_srgb8_oriented(image, orientation)
}

/// Like [`to_srgb8`] at 16 bits per channel (`R G B` per pixel, row by
/// row), for 16-bit PNG and TIFF export.
#[must_use]
pub fn to_srgb16(image: &ImageF32) -> Vec<u16> {
    display::to_srgb16(image)
}

/// [`to_srgb8_oriented`] through base curve `curve` instead of Film Standard.
#[must_use]
pub fn to_srgb8_oriented_with(
    image: &ImageF32,
    orientation: Orientation,
    curve: core_model::BaseCurve,
) -> (u32, u32, Vec<u8>) {
    display::to_srgb8_oriented_with(image, orientation, curve)
}

/// [`to_srgb16`] through base curve `curve` instead of Film Standard.
#[must_use]
pub fn to_srgb16_with(image: &ImageF32, curve: core_model::BaseCurve) -> Vec<u16> {
    display::to_srgb16_with(image, curve)
}

/// Rounds an encoded value in [0, 1] to 0–65 535 (never truncates).
fn quantize_u16(v: f32) -> u16 {
    let scaled = (v * 65_535.0).round().clamp(0.0, 65_535.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–65535 above
    let q = scaled as u16;
    q
}

/// Rounds an encoded value in [0, 1] to 0–255 (never truncates).
fn quantize_u8(v: f32) -> u8 {
    let scaled = (v * 255.0).round().clamp(0.0, 255.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–255 above
    let q = scaled as u8;
    q
}

/// The 8-bit sRGB code of a linear value, computed directly: what the
/// table-driven encoder of [`display`] must reproduce exactly.
fn encode_reference(linear: f32) -> u8 {
    quantize_u8(core_color::srgb_encode(linear.clamp(0.0, 1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::WorkingSpace;

    #[test]
    fn quantisation_rounds_and_saturates() {
        assert_eq!([0.0, 0.5, 0.998, 1.0, -0.2, 3.0].map(quantize_u8), [0, 128, 254, 255, 0, 255]);
    }

    #[test]
    fn middle_grey_maps_to_its_srgb_code_value() {
        let img =
            ImageF32::from_pixels(1, 1, WorkingSpace::LinearSrgb, vec![[0.18, 0.18, 0.18, 1.0]]).expect("1×1 image");
        assert_eq!(to_srgb8(&img), vec![128, 128, 128], "Film Standard shows middle grey at half");
    }

    #[test]
    fn white_rolls_off_towards_255_brighter_light_reaches_it_and_black_is_0() {
        let px = vec![[1.0, 1.0, 1.0, 1.0], [64.0, 64.0, 64.0, 1.0], [0.0, 0.0, 0.0, 1.0]];
        let img = ImageF32::from_pixels(3, 1, WorkingSpace::LinearRec2020, px).expect("3×1 image");
        let out = to_srgb8(&img);
        assert!(out[..3].iter().all(|v| (230..250).contains(v)), "white is just below the top: {out:?}");
        assert_eq!(&out[3..], &[255, 255, 255, 0, 0, 0]);
    }
}
