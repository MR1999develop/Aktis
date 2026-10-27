//! GPU engine on wgpu: Vulkan (NVIDIA, AMD, Intel), DirectX 12 and Metal from
//! one code base.
//!
//! [`GpuRenderer::render`] evaluates a node graph on the GPU and returns the
//! same pixels as `render-cpu` within a stated tolerance. Data stay on the
//! GPU for the whole chain of nodes and are read back once. Images larger
//! than the device's buffer limits are processed in chunks of rows; per-pixel
//! kernels need no overlap between chunks.
//!
//! Host tools ([`GpuKernel::Host`], such as the neural-network denoiser) split
//! the chain: the kernels before them run and are read back, the tool runs on
//! the whole image through [`Tool::apply`](tool_api::Tool::apply), and the
//! kernels after it start from its result.
//!
//! Masks are built on the CPU by the `masks` crate (the same code as the
//! reference renderer, so both see identical masks) and uploaded next to the
//! image; a masked kernel writes `c + mask · (tool(c) − c)`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use core_model::{GraphError, ImageError, ImageF32, MaskImage, NodeGraph, NodeId, NodeKind, Rgba, ToolId};
use masks::{NoRasters, RasterSource};
use tool_api::{GpuKernel, MAX_PARAMS, ResolvedParams, ToolError, ToolRegistry};

/// Threads per workgroup in every kernel.
const WORKGROUP: u32 = 256;
/// Bytes per pixel on the GPU (RGBA `f32`).
const PIXEL_BYTES: u64 = 16;
/// GPU memory a render may use at once, before chunking kicks in.
const MEMORY_BUDGET: u64 = 1 << 30;

/// Why the GPU could not be used or a render failed.
#[derive(Debug, Clone, PartialEq)]
pub enum GpuError {
    /// No GPU adapter with compute support was found.
    NoAdapter(String),
    /// The adapter refused to create a device.
    Device(String),
    /// The graph cannot be evaluated.
    Graph(GraphError),
    /// A node has no image input.
    MissingInput(NodeId),
    /// The node kind is not rendered yet in this phase (parallel mixer).
    NotYetSupported(NodeId),
    /// A tool's WGSL code was refused by the shader compiler (a bug in the tool).
    InvalidShader {
        /// The tool.
        tool: ToolId,
        /// Compiler message.
        message: String,
    },
    /// Reading the result back from the GPU failed (device lost or out of memory).
    Readback(String),
    /// The result could not be assembled into an image.
    Image(ImageError),
    /// A mask node has no mask definition (a corrupted recipe).
    NoMaskDefinition(NodeId),
    /// A mask does not have the image's size (a bug in mask evaluation).
    MaskSize,
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(why) => write!(f, "no usable GPU found: {why}"),
            Self::Device(why) => write!(f, "the GPU refused to start: {why}"),
            Self::Graph(e) => e.fmt(f),
            Self::MissingInput(id) => write!(f, "node {id} has no image input"),
            Self::NotYetSupported(id) => write!(f, "node {id} cannot be rendered on the GPU yet"),
            Self::InvalidShader { tool, message } => write!(f, "GPU code of tool '{tool}' is invalid: {message}"),
            Self::Readback(why) => write!(f, "reading the result from the GPU failed: {why}"),
            Self::Image(e) => e.fmt(f),
            Self::NoMaskDefinition(id) => write!(f, "mask node {id} has no mask definition"),
            Self::MaskSize => write!(f, "a mask does not have the size of the image"),
        }
    }
}

impl std::error::Error for GpuError {}

impl From<GraphError> for GpuError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}

/// The GPU device and its queue. Create once and share; creation blocks, so
/// do it on a worker thread, never on the UI thread.
pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
    limits: wgpu::Limits,
}

impl fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuContext").field("adapter", &self.adapter_name()).finish_non_exhaustive()
    }
}

impl GpuContext {
    /// Opens the most powerful GPU with compute support. The backend can be
    /// forced with the `WGPU_BACKEND` environment variable (`vulkan`, `dx12`, `metal`).
    ///
    /// # Errors
    /// [`GpuError::NoAdapter`] or [`GpuError::Device`].
    pub fn new() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        };
        let adapter =
            pollster::block_on(instance.request_adapter(&options)).map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        if !adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS) {
            return Err(GpuError::NoAdapter(format!("{} has no compute shaders", adapter.get_info().name)));
        }
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("raw-node-editor"),
            required_features: wgpu::Features::empty(),
            required_limits: limits.clone(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Self { device, queue, info: adapter.get_info(), limits })
    }

    /// Adapter name and backend, for the status bar and logs ("NVIDIA GeForce RTX 4070 · Vulkan").
    #[must_use]
    pub fn adapter_name(&self) -> String {
        format!("{} \u{b7} {:?}", self.info.name, self.info.backend)
    }

    /// Largest number of pixels per chunk when `buffers` images are alive at once.
    fn chunk_pixels(&self, total: usize, buffers: usize) -> usize {
        let by_binding = self.limits.max_storage_buffer_binding_size / PIXEL_BYTES;
        let by_buffer = self.limits.max_buffer_size / PIXEL_BYTES;
        let by_dispatch = u64::from(self.limits.max_compute_workgroups_per_dimension) * u64::from(WORKGROUP);
        let by_budget = MEMORY_BUDGET / (PIXEL_BYTES * buffers.max(1) as u64);
        let limit = by_binding.min(by_buffer).min(by_dispatch).min(by_budget).max(1);
        total.min(usize::try_from(limit).unwrap_or(usize::MAX))
    }
}

/// The WGSL wrapped around a tool's per-pixel body. `params.v[i]` holds the
/// tool's parameters in descriptor order; `params.n.x` is the pixel count and
/// `params.n.y` is 1 when `key_mask` holds the node's mask (0: no mask, `key_mask` is
/// a placeholder and is not read).
fn per_pixel_shader(body: &str) -> String {
    format!(
        "struct Params {{ v: array<vec4<f32>, {groups}>, n: vec4<u32>, }};\n\
         @group(0) @binding(0) var<storage, read> src: array<vec4<f32>>;\n\
         @group(0) @binding(1) var<storage, read_write> dst: array<vec4<f32>>;\n\
         @group(0) @binding(2) var<uniform> params: Params;\n\
         @group(0) @binding(3) var<storage, read> key_mask: array<f32>;\n\
         fn tool_apply(c: vec4<f32>) -> vec4<f32> {{\n{body}\n}}\n\
         @compute @workgroup_size({WORKGROUP})\n\
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             if (id.x >= params.n.x) {{ return; }}\n\
             let c = src[id.x];\n\
             var r = tool_apply(c);\n\
             if (params.n.y != 0u) {{ r = c + key_mask[id.x] * (r - c); }}\n\
             dst[id.x] = r;\n\
         }}\n",
        groups = MAX_PARAMS / 4,
    )
}

/// Uniform block: the tool's parameters, then the pixel count and the mask flag.
fn uniform_bytes(params: &[f32; MAX_PARAMS], pixels: u32, masked: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MAX_PARAMS * 4 + 16);
    bytes.extend_from_slice(bytemuck::cast_slice(params));
    bytes.extend_from_slice(bytemuck::cast_slice(&[pixels, u32::from(masked), 0, 0]));
    bytes
}

/// The rendered image and what could not be applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// Linear image in the source's working space.
    pub image: ImageF32,
    /// Tools used by the recipe but missing from this build (passed through).
    pub missing_tools: Vec<ToolId>,
    /// Nodes whose tool could not run (passed through), as in `render-cpu`.
    pub tool_errors: Vec<(NodeId, ToolError)>,
    /// AI mask components whose raster is not computed: (mask node, component
    /// index). They counted as empty.
    pub missing_masks: Vec<(NodeId, usize)>,
}

/// What one node does in a render.
enum Step {
    /// Run a kernel from buffer `input` into buffer `output`, through mask
    /// number `mask` if any.
    Kernel {
        pipeline: Arc<wgpu::ComputePipeline>,
        params: [f32; MAX_PARAMS],
        input: usize,
        output: usize,
        mask: Option<usize>,
    },
}

/// Renders node graphs on a [`GpuContext`], caching one pipeline per tool.
pub struct GpuRenderer {
    ctx: Arc<GpuContext>,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: Mutex<HashMap<ToolId, Arc<wgpu::ComputePipeline>>>,
}

impl fmt::Debug for GpuRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuRenderer").field("ctx", &self.ctx).finish_non_exhaustive()
    }
}

impl GpuRenderer {
    /// A renderer on `ctx`.
    #[must_use]
    pub fn new(ctx: Arc<GpuContext>) -> Self {
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("per-pixel kernel"),
            entries: &[
                storage(0, true),
                storage(1, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(3, true),
            ],
        });
        let pipeline_layout = ctx.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("per-pixel kernel"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        Self { ctx, layout, pipeline_layout, pipelines: Mutex::new(HashMap::new()) }
    }

    /// The context this renderer runs on.
    #[must_use]
    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    /// The compiled pipeline of a tool, built on first use.
    fn pipeline(&self, tool: &ToolId, body: &str) -> Result<Arc<wgpu::ComputePipeline>, GpuError> {
        let mut cache = self.pipelines.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(p) = cache.get(tool) {
            return Ok(Arc::clone(p));
        }
        let device = &self.ctx.device;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(tool.as_str()),
            source: wgpu::ShaderSource::Wgsl(per_pixel_shader(body).into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(tool.as_str()),
            layout: Some(&self.pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::InvalidShader { tool: tool.clone(), message: e.to_string() });
        }
        let pipeline = Arc::new(pipeline);
        cache.insert(tool.clone(), Arc::clone(&pipeline));
        Ok(pipeline)
    }

    /// Evaluates `graph` on `source` (the developed image), with no AI
    /// rasters: every AI mask component counts as not computed.
    ///
    /// # Errors
    /// [`GpuError`] when the graph cannot be evaluated, a shader is invalid, or the GPU fails.
    pub fn render(&self, graph: &NodeGraph, registry: &ToolRegistry, source: &ImageF32) -> Result<Rendered, GpuError> {
        self.render_with_masks(graph, registry, source, &NoRasters)
    }

    /// Evaluates `graph` on `source` (the developed image), taking AI rasters from `rasters`.
    ///
    /// # Errors
    /// [`GpuError`] when the graph cannot be evaluated, a shader is invalid, or the GPU fails.
    pub fn render_with_masks(
        &self,
        graph: &NodeGraph,
        registry: &ToolRegistry,
        source: &ImageF32,
        rasters: &dyn RasterSource,
    ) -> Result<Rendered, GpuError> {
        // Plan: buffer 0 holds the segment's input; each active kernel writes a new buffer.
        // Bypassed nodes, missing tools and the output just reuse their input's buffer.
        // A host tool ends the segment: its input is read back, the tool runs on
        // the CPU side, and its result becomes buffer 0 of the next segment.
        let mut segment_input = source.clone();
        let mut buffer_of: BTreeMap<NodeId, usize> = BTreeMap::new();
        // Nodes whose buffers belonged to a finished segment.
        let mut finished: BTreeSet<NodeId> = BTreeSet::new();
        // Masks are built once from the source and shared by every segment.
        let mut mask_of: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut mask_images: Vec<MaskImage> = Vec::new();
        let mut steps = Vec::new();
        let mut missing_tools = Vec::new();
        let mut tool_errors = Vec::new();
        let mut missing_masks = Vec::new();
        let mut buffers = 1;
        for id in graph.evaluation_order()? {
            let Some(node) = graph.node(id) else { continue };
            let input = || {
                let from = graph.input_link(id, 0).ok_or(GpuError::MissingInput(id))?.from;
                if finished.contains(&from) {
                    // A branch that skips over a host tool: not needed until
                    // the parallel mixer is rendered.
                    return Err(GpuError::NotYetSupported(id));
                }
                buffer_of.get(&from).copied().ok_or(GpuError::MissingInput(id))
            };
            let slot = match &node.kind {
                NodeKind::Source => 0,
                NodeKind::Output => input()?,
                NodeKind::Parallel { .. } => return Err(GpuError::NotYetSupported(id)),
                NodeKind::Mask => {
                    let def = node.mask.as_ref().ok_or(GpuError::NoMaskDefinition(id))?;
                    // A bypassed mask node leaves the key input as if unconnected.
                    if node.enabled {
                        let evaluated = masks::evaluate(def, rasters, source);
                        missing_masks.extend(evaluated.missing.iter().map(|c| (id, *c)));
                        mask_of.insert(id, mask_images.len());
                        mask_images.push(evaluated.mask);
                    }
                    continue;
                }
                NodeKind::Tool(tool_id) => match registry.get(tool_id) {
                    Some(tool) if node.enabled => {
                        let params = ResolvedParams::resolve(tool.descriptor(), &node.params);
                        let mask = graph.input_link(id, 1).and_then(|l| mask_of.get(&l.from).copied());
                        match tool.gpu_kernel() {
                            GpuKernel::PerPixel { wgsl } => {
                                let pipeline = self.pipeline(tool_id, wgsl)?;
                                let output = buffers;
                                buffers += 1;
                                steps.push(Step::Kernel {
                                    pipeline,
                                    params: params.gpu_uniforms(),
                                    input: input()?,
                                    output,
                                    mask,
                                });
                                output
                            }
                            GpuKernel::Host => {
                                let from = input()?;
                                let mut image =
                                    self.finish_segment(&segment_input, &steps, buffers, from, &mask_images)?;
                                let before = mask.map(|_| image.clone());
                                match tool.apply(&mut image, &params) {
                                    Err(e) => tool_errors.push((id, e)),
                                    Ok(()) => {
                                        if let (Some(m), Some(before)) = (mask, &before) {
                                            blend(&mut image, before, &mask_images[m]);
                                        }
                                    }
                                }
                                segment_input = image;
                                steps.clear();
                                buffers = 1;
                                finished.extend(buffer_of.keys().copied());
                                buffer_of.clear();
                                0
                            }
                        }
                    }
                    Some(_) => input()?,
                    None => {
                        if !missing_tools.contains(tool_id) {
                            missing_tools.push(tool_id.clone());
                        }
                        input()?
                    }
                },
            };
            buffer_of.insert(id, slot);
        }
        let output = graph.output();
        if finished.contains(&output) {
            return Err(GpuError::NotYetSupported(output));
        }
        let result_slot = *buffer_of.get(&output).ok_or(GpuError::MissingInput(output))?;
        let image = self.finish_segment(&segment_input, &steps, buffers, result_slot, &mask_images)?;
        Ok(Rendered { image, missing_tools, tool_errors, missing_masks })
    }

    /// Runs one segment's kernels over `input` and returns buffer `slot`.
    /// A segment without kernels returns its input as is.
    fn finish_segment(
        &self,
        input: &ImageF32,
        steps: &[Step],
        buffers: usize,
        slot: usize,
        masks: &[MaskImage],
    ) -> Result<ImageF32, GpuError> {
        if steps.is_empty() {
            return Ok(input.clone());
        }
        let pixels = self.run(input.pixels(), steps, buffers, slot, masks)?;
        ImageF32::from_pixels(input.width(), input.height(), input.space(), pixels).map_err(GpuError::Image)
    }

    /// Runs the planned steps over `src`, chunk by chunk, and reads back `result_slot`.
    fn run(
        &self,
        src: &[Rgba],
        steps: &[Step],
        buffers: usize,
        result_slot: usize,
        masks: &[MaskImage],
    ) -> Result<Vec<Rgba>, GpuError> {
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        // A mask takes a quarter of an image buffer; counting it as a whole one keeps a margin.
        let chunk = self.ctx.chunk_pixels(src.len(), buffers + masks.len());
        let chunk_bytes = chunk as u64 * PIXEL_BYTES;

        let mask_buffers: Vec<wgpu::Buffer> = masks
            .iter()
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("mask"),
                    size: chunk as u64 * 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        // Bound where a kernel has no mask; never read (the mask flag is 0).
        let no_mask = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("no mask"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let storage: Vec<wgpu::Buffer> = (0..buffers)
            .map(|i| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(if i == 0 { "source" } else { "node result" }),
                    size: chunk_bytes,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: chunk_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut out: Vec<Rgba> = Vec::with_capacity(src.len());
        for (index, part) in src.chunks(chunk).enumerate() {
            let n = u32::try_from(part.len()).map_err(|_| GpuError::Readback("chunk too large".to_owned()))?;
            let bytes = u64::from(n) * PIXEL_BYTES;
            queue.write_buffer(&storage[0], 0, bytemuck::cast_slice(part));
            let start = index * chunk;
            for (buffer, mask) in mask_buffers.iter().zip(masks) {
                let values = mask.values().get(start..start + part.len()).ok_or(GpuError::MaskSize)?;
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(values));
            }

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") });
            // Uniform buffers must outlive the submission; keep them until the chunk is done.
            let mut uniforms = Vec::with_capacity(steps.len());
            for step in steps {
                let Step::Kernel { pipeline, params, input, output, mask } = step;
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("params"),
                    size: (MAX_PARAMS * 4 + 16) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&uniform, 0, &uniform_bytes(params, n, mask.is_some()));
                let mask_buffer = mask.and_then(|m| mask_buffers.get(m)).unwrap_or(&no_mask);
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: storage[*input].as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: storage[*output].as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 3, resource: mask_buffer.as_entire_binding() },
                    ],
                });
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("per-pixel"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(n.div_ceil(WORKGROUP), 1, 1);
                drop(pass);
                uniforms.push(uniform);
            }
            encoder.copy_buffer_to_buffer(&storage[result_slot], 0, &readback, 0, bytes);
            queue.submit([encoder.finish()]);

            let (tx, rx) = mpsc::channel();
            readback.slice(..bytes).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Readback(e.to_string()))?;
            rx.recv()
                .map_err(|_| GpuError::Readback("no answer from the GPU".to_owned()))?
                .map_err(|e| GpuError::Readback(e.to_string()))?;
            {
                let view = readback.slice(..bytes).get_mapped_range().map_err(|e| GpuError::Readback(e.to_string()))?;
                out.extend_from_slice(bytemuck::cast_slice::<u8, Rgba>(&view));
            }
            readback.unmap();
            drop(uniforms);
        }
        Ok(out)
    }
}

/// `after ← before + mask · (after − before)` on the CPU, for a host tool on
/// a masked node (the same formula as the masked kernels and `render-cpu`).
fn blend(after: &mut ImageF32, before: &ImageF32, mask: &MaskImage) {
    debug_assert_eq!((mask.width(), mask.height()), (after.width(), after.height()));
    for ((a, b), m) in after.pixels_mut().iter_mut().zip(before.pixels()).zip(mask.values()) {
        for (x, y) in a.iter_mut().zip(b) {
            *x = y + m * (*x - y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_template_declares_the_four_bindings_and_the_tool_body() {
        let s = per_pixel_shader("return c;");
        assert!(s.contains("@binding(0) var<storage, read> src"));
        assert!(s.contains("@binding(1) var<storage, read_write> dst"));
        assert!(s.contains("@binding(2) var<uniform> params"));
        assert!(s.contains("@binding(3) var<storage, read> key_mask"));
        assert!(s.contains("r = c + key_mask[id.x] * (r - c);"));
        assert!(s.contains("array<vec4<f32>, 4>"));
        assert!(s.contains("return c;"));
        assert!(s.contains("@workgroup_size(256)"));
    }

    #[test]
    fn uniforms_are_sixteen_floats_then_the_pixel_count_and_the_mask_flag() {
        let mut p = [0.0; MAX_PARAMS];
        p[0] = 1.5;
        let b = uniform_bytes(&p, 7, true);
        assert_eq!(b.len(), 80);
        assert_eq!(&b[0..4], &1.5_f32.to_le_bytes());
        assert_eq!(&b[64..68], &7_u32.to_le_bytes());
        assert_eq!(&b[68..72], &1_u32.to_le_bytes());
        assert_eq!(&uniform_bytes(&p, 7, false)[68..72], &0_u32.to_le_bytes());
    }
}
