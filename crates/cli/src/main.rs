//! `rne`: develops RAW files from the command line.
//!
//! ```text
//! rne develop <file> [-o <out.jpg>] [--exposure <stops>] [--quality <1-100>] [--cpu]
//! rne info <file>
//! rne gpu
//! ```
//!
//! Results go to stdout, diagnostics and errors to stderr. Exit codes:
//! 0 success, 1 the operation failed, 2 wrong arguments.

mod args;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use args::{Command, DevelopArgs};
use commands::{Edit, EditSession};
use core_model::{ParamId, ParamValue, Recipe, ToolId, WorkingSpace};
use tool_api::ToolRegistry;

type BoxError = Box<dyn std::error::Error>;

fn main() -> ExitCode {
    let command = match args::parse(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("rne: {msg}\n\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Help => {
            println!("{}", args::USAGE);
            Ok(())
        }
        Command::Gpu => gpu_info(),
        Command::Info(path) => info(&path),
        Command::Develop(a) => develop(&a),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rne: {}", chain(e.as_ref()));
            ExitCode::FAILURE
        }
    }
}

/// "outer: inner: innermost", one line.
fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        let msg = inner.to_string();
        if !s.contains(&msg) {
            s.push_str(": ");
            s.push_str(&msg);
        }
        src = inner.source();
    }
    s
}

/// The composition root: every tool of this build, registered once.
fn registry() -> Result<ToolRegistry, BoxError> {
    let mut r = ToolRegistry::new();
    r.register(tool_exposure::Exposure::shared())?;
    Ok(r)
}

fn gpu_info() -> Result<(), BoxError> {
    let ctx = gpu::GpuContext::new()?;
    println!("{}", ctx.adapter_name());
    Ok(())
}

fn info(path: &Path) -> Result<(), BoxError> {
    let raw = raw_io::read_raw(path)?;
    let s = &raw.sensor;
    println!("{}", path.display());
    println!("  camera       {} {}", raw.make, raw.model);
    println!("  size         {}\u{d7}{}", s.width(), s.height());
    println!("  layout       {:?}", s.cfa());
    println!(
        "  white bal.   {:?}{}",
        s.as_shot_wb(),
        if raw.has_camera_white_balance { "" } else { " (none in file)" }
    );
    println!("  orientation  {:?}", raw.orientation);
    Ok(())
}

fn develop(a: &DevelopArgs) -> Result<(), BoxError> {
    let started = Instant::now();
    let registry = registry()?;

    let raw = raw_io::read_raw(&a.input)?;
    let decoded = started.elapsed();

    // Build the recipe through commands, exactly as the UI and MCP will.
    let mut session = EditSession::new(Recipe::default());
    if let Some(stops) = a.exposure {
        let source = session.recipe().graph.source();
        session.apply(&registry, Edit::InsertSerial { after: source, tool: ToolId::new("exposure")? })?;
        let output = session.recipe().graph.output();
        let node = session.recipe().graph.input_link(output, 0).ok_or("exposure node not connected")?.from;
        let param = ParamId::new(tool_exposure::EXPOSURE)?;
        session.apply(&registry, Edit::SetParam { node, param, value: ParamValue::Float(stops) })?;
    }
    let recipe = session.recipe();

    let developed = develop::develop(&raw.sensor, &recipe.develop, WorkingSpace::LinearRec2020)?;
    let developed_at = started.elapsed();

    let (rendered, engine) = render(&recipe.graph, &registry, &developed, a.cpu_only)?;
    for t in &rendered.1 {
        eprintln!("rne: warning: tool '{t}' is not in this build; its node was skipped");
    }
    let rendered_at = started.elapsed();

    let upright = rendered.0.oriented(raw.orientation)?;
    let rgb = render_cpu::to_srgb8(&upright);
    let jpeg = export::encode_jpeg(&rgb, upright.width(), upright.height(), a.quality)?;
    let target = export::non_destructive_path(&output_path(a));
    export::write_atomic(&target, &jpeg)?;

    println!("{}", target.display());
    eprintln!(
        "rne: {} {} {}\u{d7}{} · decode {:.2}s · develop {:.2}s · render {:.2}s ({engine}) · total {:.2}s",
        raw.make,
        raw.model,
        upright.width(),
        upright.height(),
        decoded.as_secs_f32(),
        (developed_at - decoded).as_secs_f32(),
        (rendered_at - developed_at).as_secs_f32(),
        started.elapsed().as_secs_f32(),
    );
    Ok(())
}

/// Renders on the GPU when possible, else on the CPU; returns the image,
/// missing tools and the name of the engine used.
fn render(
    graph: &core_model::NodeGraph,
    registry: &ToolRegistry,
    source: &core_model::ImageF32,
    cpu_only: bool,
) -> Result<((core_model::ImageF32, Vec<ToolId>), String), BoxError> {
    if !cpu_only {
        match gpu::GpuContext::new() {
            Ok(ctx) => {
                let name = ctx.adapter_name();
                let r = gpu::GpuRenderer::new(Arc::new(ctx)).render(graph, registry, source)?;
                return Ok(((r.image, r.missing_tools), name));
            }
            Err(e) => eprintln!("rne: {e}; rendering on the CPU"),
        }
    }
    let r = render_cpu::render(graph, registry, source)?;
    Ok(((r.image, r.missing_tools), "CPU".to_owned()))
}

/// `-o` if given, else the input's name with `.jpg`, in the same folder.
fn output_path(a: &DevelopArgs) -> PathBuf {
    a.output.clone().unwrap_or_else(|| a.input.with_extension("jpg"))
}
