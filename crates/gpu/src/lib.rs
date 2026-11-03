//! GPU engine on wgpu: Vulkan (NVIDIA, AMD, Intel), DirectX 12 and Metal from
//! one code base.
//!
//! [`GpuRenderer::render`] renders a photo's layers on the GPU and returns
//! the same pixels as `render-cpu` within a stated tolerance. Data stay on
//! the GPU for the whole chain of tools and are read back once. Images
//! larger than the device's buffer limits are processed in chunks of rows;
//! per-pixel kernels need no overlap between chunks.
//!
//! Host tools ([`GpuKernel::Host`], such as the neural-network denoiser) split
//! the chain: the kernels before them run and are read back, the tool runs on
//! the whole image through [`Tool::apply`](tool_api::Tool::apply), and the
//! kernels after it start from its result.
//!
//! Masks are built on the CPU by the `masks` crate (the same code as the
//! reference renderer, so both see identical masks) and uploaded next to the
//! image. A layer's kernels run on a copy of the image, and a blend kernel
//! mixes their result in: `c + mask · (tools(c) − c)`. A layer holding a
//! host tool runs on the CPU, as in `render-cpu`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use core_model::{ImageError, ImageF32, LayerId, Layers, MaskImage, Rgba, ToolId, ToolRef, Tools};
use masks::{NoRasters, RasterSource};
pub use render_cpu::Rendered;
use tool_api::{GpuKernel, MAX_PARAMS, ResolvedParams, ToolRegistry};

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
    /// WGSL code was refused by the shader compiler (a bug in a tool or here).
    InvalidShader {
        /// The tool, or the editor's own kernel.
        tool: String,
        /// Compiler message.
        message: String,
    },
    /// Reading the result back from the GPU failed (device lost or out of memory).
    Readback(String),
    /// The result could not be assembled into an image.
    Image(ImageError),
    /// A mask does not have the image's size (a bug in mask evaluation).
    MaskSize,
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(why) => write!(f, "no usable GPU found: {why}"),
            Self::Device(why) => write!(f, "the GPU refused to start: {why}"),
            Self::InvalidShader { tool, message } => write!(f, "GPU code of tool '{tool}' is invalid: {message}"),
            Self::Readback(why) => write!(f, "reading the result from the GPU failed: {why}"),
            Self::Image(e) => e.fmt(f),
            Self::MaskSize => write!(f, "a mask does not have the size of the image"),
        }
    }
}

impl std::error::Error for GpuError {}

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
    /// Software renderers (Microsoft Basic Render Driver, llvmpipe: the only
    /// "GPU" of a virtual machine) are refused: they are slower than the CPU
    /// engine and give wrong results or fail to read back. `AKTIS_SOFTWARE_GPU=1`
    /// accepts them, for debugging.
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
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu && std::env::var_os("AKTIS_SOFTWARE_GPU").is_none() {
            return Err(GpuError::NoAdapter(format!("{} is a software renderer", info.name)));
        }
        if !adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS) {
            return Err(GpuError::NoAdapter(format!("{} has no compute shaders", adapter.get_info().name)));
        }
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("aktis"),
            required_features: wgpu::Features::empty(),
            required_limits: limits.clone(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Self { device, queue, info, limits })
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

/// The WGSL wrapped around a tool's per-pixel body. `params.v` holds what
/// `Tool::gpu_uniforms` packed (by default the parameters in descriptor
/// order); `params.n` is (pixel count of the chunk, 1 when `key_mask` holds
/// a mask — 0: no mask, `key_mask` is a placeholder and is not read
/// —, image width, image height) and `params.m.x` the index of the chunk's
/// first pixel, from which the body's `uv` (position in the image) is made.
fn per_pixel_shader(body: &str) -> String {
    format!(
        "struct Params {{ v: array<vec4<f32>, {groups}>, n: vec4<u32>, m: vec4<u32>, }};\n\
         @group(0) @binding(0) var<storage, read> src: array<vec4<f32>>;\n\
         @group(0) @binding(1) var<storage, read_write> dst: array<vec4<f32>>;\n\
         @group(0) @binding(2) var<uniform> params: Params;\n\
         @group(0) @binding(3) var<storage, read> key_mask: array<f32>;\n\
         fn tool_apply(c: vec4<f32>, uv: vec2<f32>) -> vec4<f32> {{\n{body}\n}}\n\
         @compute @workgroup_size({WORKGROUP})\n\
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             if (id.x >= params.n.x) {{ return; }}\n\
             let c = src[id.x];\n\
             let gi = params.m.x + id.x;\n\
             let size = vec2<f32>(f32(params.n.z), f32(params.n.w));\n\
             let uv = (vec2<f32>(f32(gi % params.n.z), f32(gi / params.n.z)) + 0.5) / size;\n\
             var r = tool_apply(c, uv);\n\
             if (params.n.y != 0u) {{ r = c + key_mask[id.x] * (r - c); }}\n\
             dst[id.x] = r;\n\
         }}\n",
        groups = MAX_PARAMS / 4,
    )
}

/// Where a chunk of pixels sits in the image, for kernels that use the
/// pixel's position (vignetting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChunkPlace {
    /// Pixels in the chunk.
    pixels: u32,
    /// Index of the chunk's first pixel in the image.
    start: u32,
    /// Image width and height.
    width: u32,
    height: u32,
}

/// Bytes of the uniform block: the parameters, then the pixel count, the mask
/// flag and the image size, then the chunk's first pixel.
const UNIFORM_BYTES: usize = MAX_PARAMS * 4 + 32;

/// Uniform block: the tool's parameters, then `n` = (pixel count, mask flag,
/// width, height) and `m` = (index of the chunk's first pixel, 0, 0, 0).
fn uniform_bytes(params: &[f32; MAX_PARAMS], place: ChunkPlace, masked: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(UNIFORM_BYTES);
    bytes.extend_from_slice(bytemuck::cast_slice(params));
    bytes.extend_from_slice(bytemuck::cast_slice(&[place.pixels, u32::from(masked), place.width, place.height]));
    bytes.extend_from_slice(bytemuck::cast_slice(&[place.start, 0, 0, 0]));
    bytes
}

/// The editor's own kernel that mixes a layer in: `dst` holds the layer's
/// result, `src` the image under the layer; same bindings as the tools'.
fn blend_shader() -> String {
    format!(
        "struct Params {{ v: array<vec4<f32>, {groups}>, n: vec4<u32>, m: vec4<u32>, }};\n\
         @group(0) @binding(0) var<storage, read> src: array<vec4<f32>>;\n\
         @group(0) @binding(1) var<storage, read_write> dst: array<vec4<f32>>;\n\
         @group(0) @binding(2) var<uniform> params: Params;\n\
         @group(0) @binding(3) var<storage, read> key_mask: array<f32>;\n\
         @compute @workgroup_size({WORKGROUP})\n\
         fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             if (id.x >= params.n.x) {{ return; }}\n\
             let c = src[id.x];\n\
             dst[id.x] = c + key_mask[id.x] * (dst[id.x] - c);\n\
         }}\n",
        groups = MAX_PARAMS / 4,
    )
}

/// Name under which the blend kernel is cached.
const BLEND: &str = "layer blend";

/// What one kernel does in a render.
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

/// Renders layers on a [`GpuContext`], caching one pipeline per tool.
pub struct GpuRenderer {
    ctx: Arc<GpuContext>,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: Mutex<HashMap<String, Arc<wgpu::ComputePipeline>>>,
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

    /// The compiled pipeline of a tool (or of [`BLEND`]), built on first use
    /// from the shader `source` makes.
    fn pipeline(&self, name: &str, source: impl FnOnce() -> String) -> Result<Arc<wgpu::ComputePipeline>, GpuError> {
        let mut cache = self.pipelines.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(p) = cache.get(name) {
            return Ok(Arc::clone(p));
        }
        let device = &self.ctx.device;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Wgsl(source().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: Some(&self.pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(GpuError::InvalidShader { tool: name.to_owned(), message: e.to_string() });
        }
        let pipeline = Arc::new(pipeline);
        cache.insert(name.to_owned(), Arc::clone(&pipeline));
        Ok(pipeline)
    }

    /// Renders `layers` on `source` (the developed image), with no AI
    /// rasters: every AI mask component counts as not computed.
    ///
    /// # Errors
    /// [`GpuError`] when a shader is invalid or the GPU fails.
    pub fn render(&self, layers: &Layers, registry: &ToolRegistry, source: &ImageF32) -> Result<Rendered, GpuError> {
        self.render_with_masks(layers, registry, source, &NoRasters)
    }

    /// Renders `layers` on `source` (the developed image), taking AI rasters from `rasters`.
    ///
    /// # Errors
    /// [`GpuError`] when a shader is invalid or the GPU fails.
    pub fn render_with_masks(
        &self,
        layers: &Layers,
        registry: &ToolRegistry,
        source: &ImageF32,
        rasters: &dyn RasterSource,
    ) -> Result<Rendered, GpuError> {
        let order = registry.pipeline();
        // Borrowed until a host tool produces a new image: no copy of the source per render.
        let mut plan = Plan::new(Cow::Borrowed(source));
        let mut report = Issues::default();
        self.plan_tools(&mut plan, &layers.background, LayerId::BACKGROUND, &order, registry, &mut report)?;
        for layer in layers.layers().iter().filter(|l| l.enabled) {
            let steps = layer.tools.steps(&order);
            if steps.is_empty() {
                continue;
            }
            let evaluated = masks::evaluate(&layer.mask, rasters, source);
            report.missing_masks.extend(evaluated.missing.iter().map(|c| (layer.id, *c)));
            let host = steps.iter().any(|(t, _, _)| registry.get(t).is_some_and(|t| t.gpu_kernel() == GpuKernel::Host));
            if host {
                // As in `render-cpu`, on the image read back so far.
                let mut cpu = Rendered::passing(self.flush(&mut plan)?);
                let before = cpu.image.clone();
                render_cpu::apply_tools(&layer.tools, layer.id, &order, registry, &mut cpu);
                render_cpu::blend(&mut cpu.image, &before, &evaluated.mask);
                report.merge(cpu.missing_tools, cpu.tool_errors);
                plan.input = Cow::Owned(cpu.image);
            } else {
                let under = plan.current;
                self.plan_tools(&mut plan, &layer.tools, layer.id, &order, registry, &mut report)?;
                if plan.current != under {
                    let mask = plan.masks.len();
                    plan.masks.push(evaluated.mask);
                    plan.steps.push(Step::Kernel {
                        pipeline: self.pipeline(BLEND, blend_shader)?,
                        params: [0.0; MAX_PARAMS],
                        input: under,
                        output: plan.current,
                        mask: Some(mask),
                    });
                }
            }
        }
        let image = self.flush(&mut plan)?;
        Ok(Rendered {
            image,
            missing_tools: report.missing_tools,
            tool_errors: report.tool_errors,
            missing_masks: report.missing_masks,
        })
    }

    /// Adds the kernels of one layer's tools to `plan`; a host tool ends the
    /// segment and runs on the image read back.
    fn plan_tools(
        &self,
        plan: &mut Plan<'_>,
        tools: &Tools,
        layer: LayerId,
        order: &[ToolId],
        registry: &ToolRegistry,
        report: &mut Issues,
    ) -> Result<(), GpuError> {
        for (tool_id, slot, params) in tools.steps(order) {
            let Some(tool) = registry.get(tool_id) else {
                report.merge(vec![tool_id.clone()], Vec::new());
                continue;
            };
            let params = ResolvedParams::resolve(tool.descriptor(), params);
            match tool.gpu_kernel() {
                GpuKernel::PerPixel { wgsl } => {
                    let pipeline = self.pipeline(tool_id.as_str(), || per_pixel_shader(wgsl))?;
                    let output = plan.buffers;
                    plan.buffers += 1;
                    plan.steps.push(Step::Kernel {
                        pipeline,
                        params: tool.gpu_uniforms(&params),
                        input: plan.current,
                        output,
                        mask: None,
                    });
                    plan.current = output;
                }
                GpuKernel::Host => {
                    let mut image = self.flush(plan)?;
                    if let Err(e) = tool.apply(&mut image, &params) {
                        report.tool_errors.push((ToolRef::new(layer, tool_id.clone()).with_slot(slot), e));
                    }
                    plan.input = Cow::Owned(image);
                }
            }
        }
        Ok(())
    }

    /// Runs the kernels planned so far and starts a new segment; returns
    /// the image they made.
    fn flush(&self, plan: &mut Plan<'_>) -> Result<ImageF32, GpuError> {
        let image = self.finish_segment(&plan.input, &plan.steps, plan.buffers, plan.current, &plan.masks)?;
        plan.steps.clear();
        plan.masks.clear();
        plan.buffers = 1;
        plan.current = 0;
        Ok(image)
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
        let pixels = self.run(input.pixels(), [input.width(), input.height()], steps, buffers, slot, masks)?;
        ImageF32::from_pixels(input.width(), input.height(), input.space(), pixels)
            .map(|i| i.with_pixel_size(input.pixel_size()))
            .map_err(GpuError::Image)
    }

    /// Runs the planned steps over `src`, chunk by chunk, and reads back `result_slot`.
    fn run(
        &self,
        src: &[Rgba],
        size: [u32; 2],
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
                    label: Some(if i == 0 { "source" } else { "tool result" }),
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

            let first = u32::try_from(start).map_err(|_| GpuError::Readback("image too large".to_owned()))?;
            let place = ChunkPlace { pixels: n, start: first, width: size[0], height: size[1] };
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render") });
            // Uniform buffers must outlive the submission; keep them until the chunk is done.
            let mut uniforms = Vec::with_capacity(steps.len());
            for step in steps {
                let Step::Kernel { pipeline, params, input, output, mask } = step;
                let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("params"),
                    size: UNIFORM_BYTES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&uniform, 0, &uniform_bytes(params, place, mask.is_some()));
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

/// The kernels of one segment: buffer 0 holds `input`, each kernel writes a
/// new buffer (a blend writes into its layer's last one), `current` holds
/// the result so far.
struct Plan<'a> {
    input: Cow<'a, ImageF32>,
    steps: Vec<Step>,
    buffers: usize,
    current: usize,
    masks: Vec<MaskImage>,
}

impl<'a> Plan<'a> {
    fn new(input: Cow<'a, ImageF32>) -> Self {
        Self { input, steps: Vec::new(), buffers: 1, current: 0, masks: Vec::new() }
    }
}

/// What the render could not apply so far ([`Rendered`]'s lists).
#[derive(Default)]
struct Issues {
    missing_tools: Vec<ToolId>,
    tool_errors: Vec<(ToolRef, tool_api::ToolError)>,
    missing_masks: Vec<(LayerId, usize)>,
}

impl Issues {
    fn merge(&mut self, missing: Vec<ToolId>, errors: Vec<(ToolRef, tool_api::ToolError)>) {
        for tool in missing {
            if !self.missing_tools.contains(&tool) {
                self.missing_tools.push(tool);
            }
        }
        self.tool_errors.extend(errors);
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
        assert!(s.contains(&format!("array<vec4<f32>, {}>", MAX_PARAMS / 4)));
        assert!(s.contains("fn tool_apply(c: vec4<f32>, uv: vec2<f32>)"));
        assert!(s.contains("return c;"));
        assert!(s.contains("@workgroup_size(256)"));
    }

    #[test]
    fn the_blend_kernel_mixes_the_layer_over_the_image_under_it() {
        let s = blend_shader();
        assert!(s.contains("dst[id.x] = c + key_mask[id.x] * (dst[id.x] - c);"));
        assert!(s.contains("@binding(3) var<storage, read> key_mask"));
    }

    #[test]
    fn uniforms_are_the_parameters_then_count_mask_size_and_chunk_start() {
        let mut p = [0.0; MAX_PARAMS];
        p[0] = 1.5;
        let place = ChunkPlace { pixels: 7, start: 40, width: 10, height: 6 };
        let b = uniform_bytes(&p, place, true);
        let n = MAX_PARAMS * 4;
        assert_eq!(b.len(), UNIFORM_BYTES);
        assert_eq!(&b[0..4], &1.5_f32.to_le_bytes());
        assert_eq!(&b[n..n + 4], &7_u32.to_le_bytes());
        assert_eq!(&b[n + 4..n + 8], &1_u32.to_le_bytes());
        assert_eq!(&b[n + 8..n + 12], &10_u32.to_le_bytes());
        assert_eq!(&b[n + 12..n + 16], &6_u32.to_le_bytes());
        assert_eq!(&b[n + 16..n + 20], &40_u32.to_le_bytes());
        assert_eq!(&uniform_bytes(&p, place, false)[n + 4..n + 8], &0_u32.to_le_bytes());
    }
}
