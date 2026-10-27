//! `rne`: develops RAW files from the command line.
//!
//! ```text
//! rne develop <file> [-o <out.jpg>] [--exposure <stops>] [--denoise <grain %>]
//!             [--denoise-color <colour %>] [--quality <1-100>] [--cpu]
//!             [--profile <name>] [--lens <name>] [--lens-zoom <factor>] [--no-lens] [selection]
//! rne mask <file> <selection> [-o <overlay.jpg>] [--mask-out <mask.jpg>]
//! rne models [--verify]
//! rne info <file>
//! rne lenses [<words>...]
//! rne profiles <file>
//! rne gpu
//! rne tag <files...> --model <dir> --keywords <file> [--template <text>] [--search <text>]
//! ```
//!
//! Results go to stdout, diagnostics and errors to stderr. Exit codes:
//! 0 success, 1 the operation failed, 2 wrong arguments.

mod ai;
mod args;
mod ppm;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use args::{Command, DevelopArgs, MaskArgs, TagArgs};
use commands::{Edit, EditSession};
use core_model::{
    DevelopSettings, ImageF32, LensSettings, MaskImage, NodeId, Orientation, ParamId, ParamValue, ProfileChoice,
    Recipe, ToolId, WorkingSpace,
};
use develop::prepare;
use engine::{Engine, EngineChoice};
use lens_db::{Applied, Lens, LensDatabase};
use tagging::{ImageEncoder, ImageTags, SuggestPolicy, TextEncoder};
use tool_api::ToolRegistry;
use masks::RasterSource;

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
        Command::Lenses(words) => lenses(&words),
        Command::Profiles(path) => profiles(&path),
        Command::Models { verify } => ai::list_models(verify),
        Command::Develop(a) => develop(&a),
        Command::Tag(a) => tag(&a),
        Command::Mask(a) => mask(&a),
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
) -> Result<NodeId, BoxError> {
    let output = session.recipe().graph.output();
    let last = session.recipe().graph.input_link(output, 0).ok_or("the output is not connected")?.from;
    session.apply(registry, Edit::InsertSerial { after: last, tool: ToolId::new(tool)? })?;
    let node = session.recipe().graph.input_link(output, 0).ok_or("new node not connected")?.from;
    for (param, value) in values {
        session
            .apply(registry, Edit::SetParam { node, param: ParamId::new(param)?, value: ParamValue::Float(*value) })?;
    }
    Ok(node)
}

fn gpu_info() -> Result<(), BoxError> {
    let ctx = gpu::GpuContext::new()?;
    println!("{}", ctx.adapter_name());
    Ok(())
}

/// The lens database, reporting user files that could not be read.
fn lens_database() -> std::borrow::Cow<'static, LensDatabase> {
    let (db, problems) = engine::lens_database();
    for problem in problems {
        eprintln!("rne: warning: lens database: {problem}");
    }
    db
}

/// "distortion, chromatic aberration, vignetting".
fn describe(applied: Applied) -> String {
    let parts: Vec<&str> =
        [(applied.distortion, "distortion"), (applied.tca, "chromatic aberration"), (applied.vignetting, "vignetting")]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, name)| *name)
            .collect();
    if parts.is_empty() { "nothing".to_owned() } else { parts.join(", ") }
}

fn info(path: &Path) -> Result<(), BoxError> {
    let raw = raw_io::read_raw(path)?;
    let s = &raw.sensor;
    let c = &raw.capture;
    println!("{}", path.display());
    println!("  camera       {} {}", raw.make, raw.model);
    println!(
        "  lens         {}",
        match (&c.lens_make, &c.lens_model) {
            (Some(make), Some(model)) => format!("{make} {model}"),
            (None, Some(model)) => model.clone(),
            _ => "(not recorded)".to_owned(),
        }
    );
    let mut shot = Vec::new();
    if let Some(f) = c.focal_length {
        shot.push(format!("{f} mm"));
    }
    if let Some(a) = c.aperture {
        shot.push(format!("f/{a:.1}"));
    }
    if let Some(d) = c.focus_distance {
        shot.push(if d >= 1000.0 { "focus at infinity".to_owned() } else { format!("focus {d} m") });
    }
    println!("  shot         {}", if shot.is_empty() { "(not recorded)".to_owned() } else { shot.join(" \u{b7} ") });
    println!("  size         {}\u{d7}{}", s.width(), s.height());
    println!("  layout       {:?}", s.cfa());
    println!(
        "  white bal.   {:?}{}",
        s.as_shot_wb(),
        if raw.has_camera_white_balance { "" } else { " (none in file)" }
    );
    println!("  orientation  {:?}", raw.orientation);
    let illuminants: Vec<String> = c.color_matrices.iter().map(|m| format!("{:?}", m.illuminant)).collect();
    println!(
        "  profile      built-in matrices ({})",
        if illuminants.is_empty() { "from the file".to_owned() } else { illuminants.join(", ") }
    );

    let db = lens_database();
    let lens = prepare::prepare_lens(&db, c, &LensSettings::default(), s.width(), s.height());
    println!("  lens data    {}", lens.lens.as_deref().unwrap_or("(lens not found)"));
    println!("  corrects     {}", describe(lens.applied));
    if let Some(note) = &lens.note {
        println!("               ({note})");
    }
    Ok(())
}

/// What a database lens can correct, for listings.
fn lens_data(l: &Lens) -> String {
    describe(Applied { distortion: l.has_distortion(), tca: l.has_tca(), vignetting: l.has_vignetting() })
}

fn lenses(words: &[String]) -> Result<(), BoxError> {
    let db = lens_database();
    let found = db.search_lenses(&words.join(" "));
    for l in &found {
        println!("{}  [{}]  {}", l.full_name(), l.mounts.join(", "), lens_data(l));
    }
    eprintln!("rne: {} of {} lenses", found.len(), db.lenses().len());
    Ok(())
}

fn profiles(path: &Path) -> Result<(), BoxError> {
    let raw = raw_io::read_raw(path)?;
    let library = engine::profile_library();
    let (found, problems) = library.for_camera(&prepare::camera_names(&raw.capture));
    println!("{} {}", raw.make, raw.model);
    println!("  Built-in  (the camera's own matrices; the default)");
    for entry in &found {
        println!("  {}  ({})", entry.name, entry.path.display());
    }
    for problem in problems {
        eprintln!("rne: warning: {problem}");
    }
    if found.is_empty() {
        eprintln!("rne: no DCP profiles installed for this camera; searched:");
        for dir in library.dirs() {
            eprintln!("rne:   {}", dir.display());
        }
    }
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
    if let Some(name) = &a.profile {
        session.apply(engine.registry(), Edit::SetCameraProfile(ProfileChoice::Named(name.clone())))?;
    }
    if a.no_lens || a.lens.is_some() || a.lens_zoom.is_some() {
        let lens = if a.no_lens {
            LensSettings::off()
        } else {
            LensSettings { lens: a.lens.clone(), zoom: a.lens_zoom, ..LensSettings::default() }
        };
        session.apply(engine.registry(), Edit::SetLensCorrection(lens))?;
    }
    let mut exposure_node = None;
    if let Some(stops) = a.exposure {
        exposure_node =
            Some(append_node(&mut session, engine.registry(), "exposure", &[(tool_exposure::EXPOSURE, stops)])?);
    }
    if let Some((grain, color)) = a.denoise {
        // After the exposure: the network judges noise at the final brightness.
        append_node(&mut session, engine.registry(), "denoise_ai", &[("grain", grain), ("color", color)])?;
    }
    let develop_settings = session.recipe().develop.clone();
    let recipe = session.recipe();

    // The development stage with the camera profile and lens data available here.
    let db = lens_database();
    let library = engine::profile_library();
    let developed = prepare::develop_capture(
        &raw.sensor,
        &raw.capture,
        &recipe.develop,
        WorkingSpace::LinearRec2020,
        &db,
        Some(&library),
    )?;
    if let Some(note) = &developed.profile.note {
        eprintln!("rne: warning: {note}");
    }
    if recipe.develop.lens.enabled
        && let Some(note) = &developed.lens.note
    {
        eprintln!("rne: note: {note}");
    }
    let developed_at = started.elapsed();

    // An AI or brush selection restricts the exposure to it.
    let ai = if a.selection.is_empty() {
        None
    } else {
        let node = exposure_node.ok_or("a selection needs an adjustment to restrict: add --exposure")?;
        let ai = ai::Ai::open()?;
        let id = image_id(&a.input, &develop_settings)?;
        let def = ai.build(&a.selection, &id, &developed.image, raw.orientation)?;
        session.apply(engine.registry(), Edit::AddMask { to: node, def })?;
        Some(ai)
    };
    let masked_at = started.elapsed();
    let no_rasters = masks::NoRasters;
    let rasters: &dyn RasterSource = match &ai {
        Some(ai) => &ai.rasters,
        None => &no_rasters,
    };

    let rendered = engine.render_with_masks(&session.recipe().graph, &developed.image, rasters)?;
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
    for (node, component) in &rendered.missing_masks {
        eprintln!("rne: warning: mask {node} part {component} is not computed; it selected nothing");
    }
    let rendered_at = started.elapsed();

    let target = engine::export_jpeg(&rendered.image, raw.orientation, a.quality, &output_path(a))?;

    println!("{}", target.display());
    let (w, h) = if raw.orientation.swaps_axes() {
        (rendered.image.height(), rendered.image.width())
    } else {
        (rendered.image.width(), rendered.image.height())
    };
    let profile_name = developed.profile.profile.as_ref().map_or("file matrix", |p| p.name.as_str());
    let lens_summary = match (&developed.lens.lens, developed.lens.correction.is_some()) {
        (Some(name), true) => format!("{name}: {}", describe(developed.lens.applied)),
        _ => "no lens corrections".to_owned(),
    };
    eprintln!("rne: profile {profile_name} \u{b7} {lens_summary}");
    eprintln!(
        "rne: {} {} {w}\u{d7}{h} \u{b7} decode {:.2}s \u{b7} develop {:.2}s \u{b7} masks {:.2}s \u{b7} render {:.2}s ({}) \u{b7} total {:.2}s",
        raw.make,
        raw.model,
        decoded.as_secs_f32(),
        (developed_at - decoded).as_secs_f32(),
        (masked_at - developed_at).as_secs_f32(),
        (rendered_at - masked_at).as_secs_f32(),
        rendered.backend,
        started.elapsed().as_secs_f32(),
    );
    Ok(())
}

/// `-o` if given, else the input's name with `.jpg`, in the same folder.
fn output_path(a: &DevelopArgs) -> PathBuf {
    a.output.clone().unwrap_or_else(|| a.input.with_extension("jpg"))
}

/// Suggests keywords for each file, or ranks the files against a description.
fn tag(a: &TagArgs) -> Result<(), BoxError> {
    let started = Instant::now();
    let spec = &tagging_onnx::SIGLIP2_BASE_224;
    let opts = tagging_onnx::LoadOptions { prefer_quantized: a.quantized, threads: None };
    let (mut images, mut texts) = tagging_onnx::load(&a.model, spec, &opts)?;
    eprintln!("rne: model {} loaded in {:.2}s", spec.name, started.elapsed().as_secs_f32());

    let mut embeddings = Vec::new();
    for path in &a.inputs {
        let t = Instant::now();
        let image = load_image(path)?;
        let e = images.encode_images(&[&image])?.pop().ok_or("the model returned no embedding")?;
        eprintln!("rne: {} encoded in {:.2}s", path.display(), t.elapsed().as_secs_f32());
        embeddings.push(e);
    }

    if let Some(query) = &a.search {
        let q = texts.encode_texts(&[query.as_str()])?.pop().ok_or("the model returned no embedding")?;
        let items: Vec<(&Path, &tagging::Embedding)> =
            a.inputs.iter().map(PathBuf::as_path).zip(embeddings.iter()).collect();
        println!("search: {query}");
        for r in tagging::nearest(&q, &items, items.len()) {
            println!("  {:+.3}  {}", r.similarity, r.item.display());
        }
    }

    if let Some(kw_path) = &a.keywords {
        let text = std::fs::read_to_string(kw_path).map_err(|e| format!("{}: {e}", kw_path.display()))?;
        let vocab = tagging::Vocabulary::parse_lightroom(&text).map_err(|e| format!("{}: {e}", kw_path.display()))?;
        let head = spec.head().ok_or("model specification has an invalid logit scale")?;
        let mut tagger = tagging::Tagger::new(spec.model_id(), head, vocab);
        let t = Instant::now();
        tagger.encode_prompts(&mut texts, &a.template)?;
        eprintln!("rne: {} keywords encoded in {:.2}s", tagger.vocabulary().len(), t.elapsed().as_secs_f32());

        let policy =
            SuggestPolicy { min_probability: a.min_probability, max_suggestions: a.top, ..SuggestPolicy::default() };
        for (path, e) in a.inputs.iter().zip(&embeddings) {
            println!("{}", path.display());
            let suggestions = tagger.suggest(e, &ImageTags::new(), &policy)?;
            if suggestions.is_empty() {
                println!("  (no keyword above {:.2})", a.min_probability);
            }
            for s in suggestions {
                println!("  {:.3}  {}", s.probability, tagger.vocabulary().path(s.keyword));
            }
            // The most likely keywords whatever the threshold, to judge calibration.
            let mut all = tagger.probabilities(e)?;
            all.sort_by(|x, y| y.1.total_cmp(&x.1));
            let best: Vec<String> = all
                .iter()
                .take(5)
                .map(|(k, p)| format!("{} {p:.3}", tagger.vocabulary().get(*k).map_or("?", tagging::Keyword::name)))
                .collect();
            println!("  best: {}", best.join(" · "));
        }
    }
    eprintln!("rne: total {:.2}s", started.elapsed().as_secs_f32());
    Ok(())
}

/// A file as an editor image, upright: JPEG decoded directly, anything
/// else developed as a RAW file with default settings.
fn load_image(path: &Path) -> Result<ImageF32, BoxError> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("jpg" | "jpeg")) {
        return load_jpeg(path);
    }
    let raw = raw_io::read_raw(path)?;
    let developed = develop::develop(&raw.sensor, &Recipe::default().develop, WorkingSpace::LinearRec2020)?;
    Ok(developed.oriented(raw.orientation)?)
}

/// An sRGB JPEG as linear sRGB.
fn load_jpeg(path: &Path) -> Result<ImageF32, BoxError> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    let data = decoder.decode().map_err(|e| format!("{}: {e}", path.display()))?;
    let info = decoder.info().ok_or("JPEG without header")?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let lin = |v: u8| core_color::srgb_decode(f32::from(v) / 255.0);
    let pixels: Vec<core_model::Rgba> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => {
            data.chunks_exact(3).map(|p| [lin(p[0]), lin(p[1]), lin(p[2]), 1.0]).collect()
        }
        jpeg_decoder::PixelFormat::L8 => data.iter().map(|&v| [lin(v), lin(v), lin(v), 1.0]).collect(),
        other => return Err(format!("{}: unsupported JPEG pixel format {other:?}", path.display()).into()),
    };
    Ok(ImageF32::from_pixels(w, h, WorkingSpace::LinearSrgb, pixels)?)
}

/// A developed image for AI selection.
struct Loaded {
    /// Linear, working space, in the stored orientation.
    image: ImageF32,
    orientation: Orientation,
    /// "Nikon D3S", or "PPM".
    camera: String,
    /// Names the image and its development, for the AI raster cache.
    id: String,
}

fn is_ppm(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ppm") || e.eq_ignore_ascii_case("pnm"))
}

/// Names a file and its development settings, for the AI raster cache.
fn image_id(path: &Path, settings: &DevelopSettings) -> Result<String, BoxError> {
    let bytes = std::fs::read(path)?;
    Ok(masks::fingerprint(&[&bytes, format!("{settings:?}").as_bytes()]))
}

/// Decodes and develops `path` (a RAW file, or a PPM for testing) with default settings.
fn load(path: &Path) -> Result<Loaded, BoxError> {
    let settings = DevelopSettings::default();
    let id = image_id(path, &settings)?;
    if is_ppm(path) {
        let image = ppm::decode(&std::fs::read(path)?)?;
        return Ok(Loaded { image, orientation: Orientation::Normal, camera: "PPM".to_owned(), id });
    }
    let raw = raw_io::read_raw(path)?;
    let db = lens_database();
    let library = engine::profile_library();
    let developed = prepare::develop_capture(
        &raw.sensor,
        &raw.capture,
        &settings,
        WorkingSpace::LinearRec2020,
        &db,
        Some(&library),
    )?;
    Ok(Loaded {
        image: developed.image,
        orientation: raw.orientation,
        camera: format!("{} {}", raw.make, raw.model),
        id,
    })
}

fn mask(a: &MaskArgs) -> Result<(), BoxError> {
    let started = Instant::now();
    let loaded = load(&a.input)?;
    let ai = ai::Ai::open()?;
    let def = ai.build(&a.selection, &loaded.id, &loaded.image, loaded.orientation)?;
    for c in &def.components {
        eprintln!("rne:   {} ({:?})", c.label, c.op);
    }

    let evaluated = masks::evaluate(&def, &ai.rasters, &loaded.image);
    let upright_image = loaded.image.oriented(loaded.orientation)?;
    let upright_mask = evaluated.mask.oriented(loaded.orientation)?;
    #[allow(clippy::cast_precision_loss)] // a coverage percentage for display
    let coverage = upright_mask.values().iter().sum::<f32>() / upright_mask.values().len() as f32 * 100.0;

    let overlay = overlay(&render_cpu::to_srgb8(&upright_image), &upright_mask);
    let (w, h) = (upright_mask.width(), upright_mask.height());
    let target = export::non_destructive_path(&a.output.clone().unwrap_or_else(|| suffixed(&a.input, "-mask")));
    export::write_atomic(&target, &export::encode_jpeg(&overlay, w, h, 90)?)?;
    println!("{}", target.display());
    if let Some(path) = &a.mask_out {
        let grey: Vec<u8> = upright_mask.values().iter().flat_map(|v| [quantize(*v); 3]).collect();
        let path = export::non_destructive_path(path);
        export::write_atomic(&path, &export::encode_jpeg(&grey, w, h, 95)?)?;
        println!("{}", path.display());
    }
    eprintln!(
        "rne: {} {w}\u{d7}{h} · {} part(s) · {coverage:.1}% selected · total {:.2}s",
        loaded.camera,
        def.components.len(),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

/// `photo.NEF` → `photo-mask.jpg`, in the same folder.
fn suffixed(input: &Path, suffix: &str) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    input.with_file_name(format!("{stem}{suffix}.jpg"))
}

fn quantize(v: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–255
    let q = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    q
}

/// The photo with the selection tinted red and the rest dimmed.
fn overlay(rgb: &[u8], mask: &MaskImage) -> Vec<u8> {
    let tint = [255.0, 48.0, 48.0];
    rgb.chunks_exact(3)
        .zip(mask.values())
        .flat_map(|(px, m)| {
            let m = m.clamp(0.0, 1.0);
            [0, 1, 2].map(|c| {
                let base = f32::from(px[c]) * (0.4 + 0.6 * m);
                quantize((base * (1.0 - 0.35 * m) + tint[c] * 0.35 * m) / 255.0)
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_dims_outside_and_tints_inside() {
        let mask = MaskImage::from_values(2, 1, vec![0.0, 1.0]).expect("2×1");
        let out = overlay(&[200, 200, 200, 200, 200, 200], &mask);
        assert_eq!(&out[..3], &[80, 80, 80]);
        assert!(out[3] > out[4] && out[4] == out[5], "red tint: {:?}", &out[3..]);
    }

    #[test]
    fn mask_previews_go_next_to_the_input() {
        assert_eq!(suffixed(Path::new("/p/DSC_1.NEF"), "-mask"), PathBuf::from("/p/DSC_1-mask.jpg"));
    }
}
