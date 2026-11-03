//! The contract between the editor and its tools.
//!
//! A tool is a crate that implements [`Tool`] and is registered in a
//! [`ToolRegistry`]. The layers, the tool panels, the MCP server and the export
//! read tools only through the registry, so adding or removing a tool means
//! adding or removing one registration line (and a Cargo feature).
//!
//! The UI and the MCP schema are generated from [`ToolDescriptor`]; the GPU
//! engine builds its pipelines from [`GpuKernel`]; [`Tool::apply_cpu`] is the
//! simple reference every GPU kernel is tested against.
//!
//! A few tools cannot be a WGSL kernel: a neural network running on its own
//! inference engine, or an algorithm that needs the whole image. They declare
//! [`GpuKernel::Host`] and do their work in [`Tool::apply`], which may fail
//! (a missing model file): the renderers then pass the image through and
//! report the failure instead of stopping the render.

mod descriptor;
mod registry;

pub use core_model::{ParamId, ParamValue, Params, ToolId};
pub use descriptor::{
    Category, DescriptorError, MAX_PARAMS, ParamDescriptor, ParamKind, ResolvedParams, ToolDescriptor, Unit,
};
pub use registry::{RegistryError, ToolRegistry};

use core_model::ImageF32;

/// How the GPU engine runs a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuKernel {
    /// The output pixel depends only on the input pixel at the same place.
    ///
    /// `wgsl` is the body of a WGSL function
    /// `fn tool_apply(c: vec4<f32>, uv: vec2<f32>) -> vec4<f32>`, where `uv`
    /// is the pixel's position in the image (0 to 1, from the top-left
    /// corner, at the pixel's centre); it reads the tool's values from
    /// `params.v[i / 4][i % 4]`: by default the parameters in
    /// [`ToolDescriptor::params`] order (floats as is, booleans as 0/1,
    /// choices as their index), or what [`Tool::gpu_uniforms`] packs.
    PerPixel {
        /// WGSL function body.
        wgsl: &'static str,
    },
    /// The tool runs outside the GPU engine's kernels, through [`Tool::apply`]:
    /// the engine reads the image back before the tool, hands it to the tool and
    /// uploads the result. For tools with their own accelerator (a neural
    /// network on ONNX Runtime) or that need the whole image at once.
    Host,
}

/// Why a tool could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// Parameters saved by a newer or unknown version of the tool.
    UnsupportedVersion {
        /// Version found in the recipe.
        found: u32,
        /// Version this build understands.
        current: u32,
    },
    /// The tool cannot run here (for example its model file is missing or its
    /// engine failed). The image is left unchanged; the text is for the user.
    Unavailable(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { found, current } => {
                write!(f, "parameters are from tool version {found}, this build knows up to {current}")
            }
            Self::Unavailable(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ToolError {}

/// A tool: a named image operation with typed parameters.
pub trait Tool: Send + Sync + std::fmt::Debug {
    /// Name, category, stage and parameters. The UI and MCP are generated from it.
    fn descriptor(&self) -> &ToolDescriptor;

    /// Reference implementation on the CPU, written for clarity, not speed.
    /// GPU kernels are tested against it; it is also the fallback without a GPU.
    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams);

    /// What the renderers call: [`Tool::apply_cpu`] for ordinary tools.
    ///
    /// Tools with [`GpuKernel::Host`] override it to run their own engine.
    /// On error the image must be left unchanged; the renderers pass it on
    /// and report the error.
    ///
    /// # Errors
    /// [`ToolError::Unavailable`] when the tool cannot run here.
    fn apply(&self, image: &mut ImageF32, params: &ResolvedParams) -> Result<(), ToolError> {
        self.apply_cpu(image, params);
        Ok(())
    }

    /// How the GPU engine runs this tool.
    fn gpu_kernel(&self) -> GpuKernel;

    /// The values the GPU kernel reads from `params.v`. By default the
    /// parameters in descriptor order ([`ResolvedParams::gpu_uniforms`]);
    /// tools override it to hand the kernel derived values (a colour
    /// matrix, curve tangents) computed once on the CPU.
    fn gpu_uniforms(&self, params: &ResolvedParams) -> [f32; MAX_PARAMS] {
        params.gpu_uniforms()
    }

    /// Converts parameters saved by an older version of the tool.
    ///
    /// The default accepts only the current version.
    ///
    /// # Errors
    /// [`ToolError::UnsupportedVersion`] for versions this tool cannot read.
    fn migrate(&self, from_version: u32, params: Params) -> Result<Params, ToolError> {
        let current = self.descriptor().version;
        if from_version == current {
            Ok(params)
        } else {
            Err(ToolError::UnsupportedVersion { found: from_version, current })
        }
    }
}
