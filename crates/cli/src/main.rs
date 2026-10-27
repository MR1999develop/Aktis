//! `rne`: develops RAW files from the command line.
//!
//! ```text
//! rne develop <file> [-o <out.jpg>] [--exposure <stops>] [--denoise <grain %>]
//!             [--denoise-color <colour %>] [--quality <1-100>] [--cpu]
//! rne info <file>
//! rne gpu
//! ```
//!
//! Results go to stdout, diagnostics and errors to stderr. Exit codes:
//! 0 success, 1 the operation failed, 2 wrong arguments.

mod args;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use args::{Command, DevelopArgs};
use commands::{Edit, EditSession};
use core_model::{ParamId, ParamValue, Recipe, ToolId, WorkingSpace};
use engine::{Engine, EngineChoice};
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

/// Inserts `tool` after the node currently feeding the Output and sets its
/// float parameters.
fn append_node(
    session: &mut EditSession,
    registry: &ToolRegistry,
    tool: &str,
    values: &[(&str, f32)],
) -> Result<(), BoxError> {
    let output = session.recipe().graph.output();
    let last = session.recipe().graph.input_link(output, 0).ok_or("the output is not connected")?.from;
    session.apply(registry, Edit::InsertSerial { after: last, tool: ToolId::new(tool)? })?;
    let node = session.recipe().graph.input_link(output, 0).ok_or("new node not connected")?.from;
    for (param, value) in values {
        session
            .apply(registry, Edit::SetParam { node, param: ParamId::new(param)?, value: ParamValue::Float(*value) })?;
    }
    Ok(())
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
    let choice = if a.cpu_only { EngineChoice::CpuOnly } else { EngineChoice::Auto };
    let engine = Engine::new(engine::builtin_tools()?, choice);
    if let Some(why) = engine.gpu_problem() {
        eprintln!("rne: {why}; rendering on the CPU");
    }

    let raw = raw_io::read_raw(&a.input)?;
    let decoded = started.elapsed();

    // Build the recipe through commands, exactly as the UI and MCP will.
    let mut session = EditSession::new(Recipe::default());
    if let Some(stops) = a.exposure {
        append_node(&mut session, engine.registry(), "exposure", &[(tool_exposure::EXPOSURE, stops)])?;
    }
    if let Some((grain, color)) = a.denoise {
        // After the exposure: the network judges noise at the final brightness.
        append_node(&mut session, engine.registry(), "denoise_ai", &[("grain", grain), ("color", color)])?;
    }
    let recipe = session.recipe();

    let developed = develop::develop(&raw.sensor, &recipe.develop, WorkingSpace::LinearRec2020)?;
    let developed_at = started.elapsed();

    let rendered = engine.render(&recipe.graph, &developed)?;
    if let Some(why) = &rendered.gpu_failure {
        eprintln!("rne: warning: the GPU failed ({why}); rendered on the CPU");
    }
    for t in &rendered.missing_tools {
        eprintln!("rne: warning: tool '{t}' is not in this build; its node was skipped");
    }
    for (node, e) in &rendered.tool_errors {
        eprintln!("rne: warning: node {node} was skipped: {e}");
    }
    if a.denoise.is_some()
        && let Some(status) = engine::ai_denoise_status()
    {
        eprintln!("rne: AI denoise: {status}");
    }
    let rendered_at = started.elapsed();

    let target = engine::export_jpeg(&rendered.image, raw.orientation, a.quality, &output_path(a))?;

    println!("{}", target.display());
    let (w, h) = if raw.orientation.swaps_axes() {
        (rendered.image.height(), rendered.image.width())
    } else {
        (rendered.image.width(), rendered.image.height())
    };
    eprintln!(
        "rne: {} {} {w}\u{d7}{h} \u{b7} decode {:.2}s \u{b7} develop {:.2}s \u{b7} render {:.2}s ({}) \u{b7} total {:.2}s",
        raw.make,
        raw.model,
        decoded.as_secs_f32(),
        (developed_at - decoded).as_secs_f32(),
        (rendered_at - developed_at).as_secs_f32(),
        rendered.backend,
        started.elapsed().as_secs_f32(),
    );
    Ok(())
}

/// `-o` if given, else the input's name with `.jpg`, in the same folder.
fn output_path(a: &DevelopArgs) -> PathBuf {
    a.output.clone().unwrap_or_else(|| a.input.with_extension("jpg"))
}
