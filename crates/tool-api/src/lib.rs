//! The contract between the editor and its tools.
//!
//! A tool is a crate that implements [`Tool`] and is registered in a
//! [`ToolRegistry`]. The graph, the inspector, the MCP server and the export
//! read tools only through the registry, so adding or removing a tool means
//! adding or removing one registration line (and a Cargo feature).
//!
//! The UI and the MCP schema are generated from [`ToolDescriptor`]; the GPU
//! engine builds its pipelines from [`GpuKernel`]; [`Tool::apply_cpu`] is the
//! simple reference every GPU kernel is tested against.

mod descriptor;
mod registry;

pub use core_model::{ParamId, ParamValue, Params, ToolId};
pub use descriptor::{
    Category, DescriptorError, MAX_PARAMS, ParamDescriptor, ParamKind, ResolvedParams, Stage, ToolDescriptor, Unit,
};
pub use registry::{RegistryError, ToolRegistry};

use core_model::ImageF32;

/// How the GPU engine runs a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuKernel {
    /// The output pixel depends only on the input pixel at the same place.
    ///
    /// `wgsl` is the body of a WGSL function
    /// `fn tool_apply(c: vec4<f32>) -> vec4<f32>`; it reads the tool's
    /// parameters from `params.v[i]`, where `i` is the parameter's position in
    /// [`ToolDescriptor::params`] (floats as is, booleans as 0/1, choices as
    /// their index). See [`ResolvedParams::gpu_uniforms`].
    PerPixel {
        /// WGSL function body.
        wgsl: &'static str,
    },
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
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { found, current } => {
                write!(f, "parameters are from tool version {found}, this build knows up to {current}")
            }
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

    /// How the GPU engine runs this tool.
    fn gpu_kernel(&self) -> GpuKernel;

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
