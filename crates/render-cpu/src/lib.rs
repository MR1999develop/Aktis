//! Reference renderer on the CPU.
//!
//! Evaluates a node graph with each tool's [`Tool::apply`] (its CPU reference,
//! or its own engine for host tools) and turns the result into display
//! pixels. It favours clarity over speed: the GPU engine must produce the
//! same pixels (within a stated tolerance), and this crate
//! is what its tests compare against. It is also the fallback without a GPU.
//!
//! A tool node with something on its key input applies its effect only
//! where the mask is: `out = in + mask · (tool(in) − in)`. Masks are built by
//! the `masks` crate, the same code the GPU engine uses, so both engines see
//! identical masks.
//!
//! [`Tool::apply`]: tool_api::Tool::apply

use std::collections::BTreeMap;
use std::fmt;

mod display;
pub mod tone;

use core_model::{GraphError, ImageF32, MaskImage, NodeGraph, NodeId, NodeKind, Orientation, ToolId};
use masks::{NoRasters, RasterSource};
use tool_api::{ResolvedParams, ToolError, ToolRegistry};

/// Why the graph could not be rendered.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// The graph is not connected from Source to Output.
    Graph(GraphError),
    /// A node needed for the output has no image input.
    MissingInput(NodeId),
    /// The node kind is not rendered yet in this phase (parallel mixer).
    NotYetSupported(NodeId),
    /// A mask node has no mask definition (a corrupted recipe).
    NoMaskDefinition(NodeId),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(e) => e.fmt(f),
            Self::MissingInput(id) => write!(f, "node {id} has no image input"),
            Self::NotYetSupported(id) => write!(f, "node {id} cannot be rendered yet"),
            Self::NoMaskDefinition(id) => write!(f, "mask node {id} has no mask definition"),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Graph(e) => Some(e),
            _ => None,
        }
    }
}

impl From<GraphError> for RenderError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}

/// The rendered image and what could not be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// Linear image in the source's working space.
    pub image: ImageF32,
    /// Tools used by the recipe but missing from this build. Their nodes
    /// passed the image through unchanged; their parameters are kept.
    pub missing_tools: Vec<ToolId>,
    /// Nodes whose tool could not run (for example a missing model file).
    /// They passed the image through unchanged.
    pub tool_errors: Vec<(NodeId, ToolError)>,
    /// AI mask components whose raster is not computed: (mask node, component
    /// index). They counted as empty, so their tools had no effect there.
    pub missing_masks: Vec<(NodeId, usize)>,
}

/// Evaluates `graph` on `source` (the developed image), with no AI rasters:
/// every AI mask component counts as not computed.
///
/// # Errors
/// [`RenderError`] when the graph cannot be evaluated.
pub fn render(graph: &NodeGraph, registry: &ToolRegistry, source: &ImageF32) -> Result<Rendered, RenderError> {
    render_with_masks(graph, registry, source, &NoRasters)
}

/// Evaluates `graph` on `source` (the developed image), taking AI rasters from `rasters`.
///
/// # Errors
/// [`RenderError`] when the graph cannot be evaluated.
pub fn render_with_masks(
    graph: &NodeGraph,
    registry: &ToolRegistry,
    source: &ImageF32,
    rasters: &dyn RasterSource,
) -> Result<Rendered, RenderError> {
    let order = graph.evaluation_order()?;
    let mut results: BTreeMap<NodeId, ImageF32> = BTreeMap::new();
    let mut mask_results: BTreeMap<NodeId, MaskImage> = BTreeMap::new();
    let mut missing_tools = Vec::new();
    let mut tool_errors = Vec::new();
    let mut missing_masks = Vec::new();

    for id in order {
        let Some(node) = graph.node(id) else { continue };
        let image_input =
            || graph.input_link(id, 0).and_then(|l| results.get(&l.from)).cloned().ok_or(RenderError::MissingInput(id));
        let out = match &node.kind {
            NodeKind::Source => source.clone(),
            NodeKind::Output => image_input()?,
            NodeKind::Mask => {
                let def = node.mask.as_ref().ok_or(RenderError::NoMaskDefinition(id))?;
                // A bypassed mask node leaves the key input as if unconnected.
                if node.enabled {
                    let evaluated = masks::evaluate(def, rasters, source);
                    missing_masks.extend(evaluated.missing.iter().map(|c| (id, *c)));
                    mask_results.insert(id, evaluated.mask);
                }
                continue;
            }
            NodeKind::Tool(tool_id) => {
                let mut img = image_input()?;
                match registry.get(tool_id) {
                    Some(tool) if node.enabled => {
                        let params = ResolvedParams::resolve(tool.descriptor(), &node.params);
                        let key = graph.input_link(id, 1).and_then(|l| mask_results.get(&l.from));
                        let before = key.map(|_| img.clone());
                        match tool.apply(&mut img, &params) {
                            Err(e) => tool_errors.push((id, e)),
                            Ok(()) => {
                                if let (Some(mask), Some(before)) = (key, &before) {
                                    blend(&mut img, before, mask);
                                }
                            }
                        }
                    }
                    Some(_) => {}
                    None => {
                        if !missing_tools.contains(tool_id) {
                            missing_tools.push(tool_id.clone());
                        }
                    }
                }
                img
            }
            NodeKind::Parallel { .. } => return Err(RenderError::NotYetSupported(id)),
        };
        results.insert(id, out);
    }

    let image = results.remove(&graph.output()).ok_or(RenderError::MissingInput(graph.output()))?;
    Ok(Rendered { image, missing_tools, tool_errors, missing_masks })
}

/// `after ← before + mask · (after − before)`, every channel. Masks are built
/// at the source's size and every node keeps that size.
fn blend(after: &mut ImageF32, before: &ImageF32, mask: &MaskImage) {
    debug_assert_eq!((mask.width(), mask.height()), (after.width(), after.height()));
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
        assert_eq!(to_srgb8(&img), vec![118, 118, 118]);
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
