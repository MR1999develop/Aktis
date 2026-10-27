//! Reference renderer on the CPU.
//!
//! Evaluates a node graph with each tool's [`Tool::apply`] (its CPU reference,
//! or its own engine for host tools) and turns the result into display
//! pixels. It favours clarity over speed: the GPU engine must produce the
//! same pixels (within a stated tolerance), and this crate
//! is what its tests compare against. It is also the fallback without a GPU.
//!
//! [`Tool::apply`]: tool_api::Tool::apply

use std::collections::BTreeMap;
use std::fmt;

use core_model::{GraphError, ImageF32, NodeGraph, NodeId, NodeKind, ToolId, WorkingSpace};
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
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(e) => e.fmt(f),
            Self::MissingInput(id) => write!(f, "node {id} has no image input"),
            Self::NotYetSupported(id) => write!(f, "node {id} cannot be rendered yet"),
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
}

/// Evaluates `graph` on `source` (the developed image).
///
/// # Errors
/// [`RenderError`] when the graph cannot be evaluated.
pub fn render(graph: &NodeGraph, registry: &ToolRegistry, source: &ImageF32) -> Result<Rendered, RenderError> {
    let order = graph.evaluation_order()?;
    let mut results: BTreeMap<NodeId, ImageF32> = BTreeMap::new();
    let mut missing_tools = Vec::new();
    let mut tool_errors = Vec::new();

    for id in order {
        let Some(node) = graph.node(id) else { continue };
        let image_input =
            || graph.input_link(id, 0).and_then(|l| results.get(&l.from)).cloned().ok_or(RenderError::MissingInput(id));
        let out = match &node.kind {
            NodeKind::Source => source.clone(),
            NodeKind::Output => image_input()?,
            NodeKind::Tool(tool_id) => {
                let mut img = image_input()?;
                match registry.get(tool_id) {
                    Some(tool) if node.enabled => {
                        let params = ResolvedParams::resolve(tool.descriptor(), &node.params);
                        if let Err(e) = tool.apply(&mut img, &params) {
                            tool_errors.push((id, e));
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
    Ok(Rendered { image, missing_tools, tool_errors })
}

/// Converts a linear image to 8-bit sRGB for display or JPEG export.
///
/// Provisional output render for the tracer bullet: values outside the sRGB
/// gamut and above white are clipped per channel. The scene-referred tone
/// mapping and gamut mapping replace this in phase 4.
#[must_use]
pub fn to_srgb8(image: &ImageF32) -> Vec<u8> {
    let m = core_color::rgb_to_rgb(image.space(), WorkingSpace::LinearSrgb);
    let mut out = Vec::with_capacity(image.pixels().len() * 3);
    for p in image.pixels() {
        let rgb = m.apply([p[0], p[1], p[2]]);
        for v in rgb {
            out.push(quantize_u8(core_color::srgb_encode(v.clamp(0.0, 1.0))));
        }
    }
    out
}

/// Rounds an encoded value in [0, 1] to 0–255 (never truncates).
fn quantize_u8(v: f32) -> u8 {
    let scaled = (v * 255.0).round().clamp(0.0, 255.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–255 above
    let q = scaled as u8;
    q
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rec2020_white_and_black_map_to_255_and_0() {
        let img =
            ImageF32::from_pixels(2, 1, WorkingSpace::LinearRec2020, vec![[1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]])
                .expect("2×1 image");
        assert_eq!(to_srgb8(&img), vec![255, 255, 255, 0, 0, 0]);
    }
}
