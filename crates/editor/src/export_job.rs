//! Exports with the export window's options: one photo or many, one after
//! another on a background thread, with progress and cancel.
//!
//! Each photo is developed (the open one reuses its developed image),
//! rendered with its recipe, finished ([`engine::finish_for_export`]:
//! size, upright pixels, watermark), encoded with its metadata and written
//! under a name from the pattern, never over an existing file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use core_model::{CaptureInfo, ImageF32, Orientation, Recipe};
use engine::Engine;
use engine::export::{self, ExportOptions, NameFields, PhotoMetadata};

use crate::document::RawData;

/// What is already in memory for the photo open in the editor.
#[derive(Debug, Clone)]
pub struct OpenPhoto {
    /// The decoded RAW (for its camera data, and to develop again).
    pub raw: Option<Arc<RawData>>,
    /// The developed full-size image, when it matches the recipe's
    /// development settings; `None` develops `raw` again.
    pub developed: Option<Arc<ImageF32>>,
    /// How to turn it upright.
    pub orientation: Orientation,
    /// Shooting data.
    pub info: raw_io::PhotoInfo,
}

/// One photo to export.
#[derive(Debug, Clone)]
pub struct ExportPhoto {
    /// The RAW file.
    pub path: PathBuf,
    /// Its edits; `None` exports with the default recipe (a photo never
    /// edited). The window fills this with its `recipe_for(path)`.
    pub recipe: Option<Recipe>,
    /// Set for the photo open in the editor: nothing is read again.
    pub open: Option<OpenPhoto>,
    /// Rating from the catalog (0 = none); `None` keeps the camera's.
    pub rating: Option<u8>,
    /// Keywords from the catalog.
    pub keywords: Vec<String>,
}

/// An export of one or more photos.
#[derive(Debug, Clone)]
pub struct ExportJob {
    /// The photos, in export order (the `{counter}` order).
    pub photos: Vec<ExportPhoto>,
    /// The export window's choices.
    pub options: ExportOptions,
    /// Written as the EXIF "Software" ("RAW Node Editor 0.1.0").
    pub software: String,
}

/// How an export ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// Photos in the job.
    pub total: usize,
    /// Files written, in order.
    pub written: Vec<PathBuf>,
    /// Messages for the photos that failed ("DSC_0042.NEF: …").
    pub failed: Vec<String>,
    /// The user stopped it.
    pub cancelled: bool,
}

/// Stops a running export after the step in progress.
#[derive(Debug, Clone, Default)]
pub struct ExportHandle {
    cancel: Arc<AtomicBool>,
}

impl ExportHandle {
    /// Asks the export to stop; files already written stay.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// `true` once [`Self::cancel`] was called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The flag the export thread reads.
    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.cancel
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn some_text(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// What the exported file may say about the photo, before the user's
/// metadata choice is applied.
#[must_use]
pub fn photo_metadata(
    capture: Option<&CaptureInfo>,
    info: &raw_io::PhotoInfo,
    photo: &ExportPhoto,
    options: &ExportOptions,
    software: &str,
) -> PhotoMetadata {
    PhotoMetadata {
        make: capture.and_then(|c| some_text(&c.camera_make)),
        model: capture.and_then(|c| some_text(&c.camera_model)),
        lens: info.lens.clone(),
        exposure_time: info.exposure_time,
        aperture: info.aperture,
        iso: info.iso,
        focal_length: info.focal_length,
        exposure_bias: info.exposure_bias,
        captured: info.captured.clone(),
        gps: info.gps,
        rating: photo.rating.or(info.camera_rating),
        keywords: photo.keywords.clone(),
        artist: info.artist.clone(),
        copyright: some_text(&options.copyright).or_else(|| info.copyright.clone()),
        software: some_text(software),
    }
    .filtered(options.metadata)
}

/// The camera as file names show it ("Nikon D3S").
fn camera_name(capture: Option<&CaptureInfo>) -> Option<String> {
    capture.and_then(|c| some_text(&format!("{} {}", c.clean_make, c.clean_model)))
}

/// The file name (with extension) the export of `photo` asks for.
#[must_use]
pub fn wanted_name(
    options: &ExportOptions,
    path: &Path,
    index: usize,
    captured: Option<&str>,
    camera: Option<&str>,
) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let fields = NameFields { stem: &stem, index: u32::try_from(index).unwrap_or(u32::MAX), captured, camera };
    format!("{}.{}", options.name.stem(&fields), options.format.kind.extension())
}

/// Exports one photo; `Ok(None)` when cancelled on the way.
fn export_one(
    engine: &Engine,
    photo: &ExportPhoto,
    index: usize,
    job: &ExportJob,
    cancel: &AtomicBool,
) -> Result<Option<PathBuf>, String> {
    let name = display_name(&photo.path);
    let recipe = photo.recipe.clone().unwrap_or_default();
    let (raw, image, orientation, info) = match &photo.open {
        Some(open) => {
            let image = match (&open.developed, &open.raw) {
                (Some(image), _) => Arc::clone(image),
                (None, Some(raw)) => Arc::new(develop_full(raw, &recipe, &name)?),
                (None, None) => return Err(format!("{name}: the photo cannot be developed again")),
            };
            (open.raw.clone(), image, open.orientation, open.info.clone())
        }
        None => {
            let file = raw_io::read_raw(&photo.path).map_err(|e| format!("{name}: {e}"))?;
            let raw = Arc::new(RawData { sensor: file.sensor, capture: file.capture, file_notes: file.notes });
            let image = Arc::new(develop_full(&raw, &recipe, &name)?);
            (Some(raw), image, file.orientation, file.info)
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let rendered = engine
        .render_with_masks(&recipe.graph, &image, crate::worker::rasters())
        .map_err(|e| format!("{name}: {e}"))?;
    drop(image);
    if cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let options = &job.options;
    let finished = engine::finish_for_export(
        &rendered.image,
        orientation,
        &recipe.geometry,
        &options.size,
        options.watermark.as_ref(),
        recipe.curve,
    )
    .map_err(|e| format!("{name}: {e}"))?;
    drop(rendered);
    let capture = raw.as_deref().map(|r| &r.capture);
    let metadata = photo_metadata(capture, &info, photo, options, &job.software);
    let bytes = export::encode(&finished, &options.format, &metadata).map_err(|e| format!("{name}: {e}"))?;
    drop(finished);
    let folder = options.destination.folder_for(&photo.path);
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("{name}: the folder {} cannot be made: {e}", folder.display()))?;
    let camera = camera_name(capture);
    let file_name = wanted_name(options, &photo.path, index, info.captured.as_deref(), camera.as_deref());
    let target = export::non_destructive_path(&folder.join(file_name));
    export::write_atomic(&target, &bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok(Some(target))
}

fn develop_full(raw: &RawData, recipe: &Recipe, name: &str) -> Result<ImageF32, String> {
    crate::worker::develop_stage(raw, &recipe.develop).map(|(d, _)| d.image).map_err(|e| format!("{name} {e}"))
}

/// Runs `job` on this thread: `progress(done, name)` before each photo.
pub(crate) fn run(
    engine: &Engine,
    job: &ExportJob,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, String),
) -> ExportSummary {
    let mut summary = ExportSummary { total: job.photos.len(), ..ExportSummary::default() };
    for (index, photo) in job.photos.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let name = display_name(&photo.path);
        progress(index, name.clone());
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| export_one(engine, photo, index, job, cancel)))
                .unwrap_or_else(|_| Err(format!("{name}: the export stopped because of an internal error")));
        match result {
            Ok(Some(path)) => summary.written.push(path),
            Ok(None) => {
                summary.cancelled = true;
                break;
            }
            Err(message) => summary.failed.push(message),
        }
    }
    summary
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use core_model::WorkingSpace;
    use engine::EngineChoice;
    use engine::export::{Destination, FileFormat, Format, MetadataChoice, NamePattern};

    fn engine() -> Engine {
        Engine::new(engine::builtin_tools().unwrap(), EngineChoice::CpuOnly)
    }

    fn open_photo(path: &Path) -> ExportPhoto {
        let img = ImageF32::from_pixels(6, 4, WorkingSpace::LinearRec2020, vec![[0.4, 0.2, 0.1, 1.0]; 24]).unwrap();
        ExportPhoto {
            path: path.to_owned(),
            recipe: None,
            open: Some(OpenPhoto {
                raw: None,
                developed: Some(Arc::new(img)),
                orientation: Orientation::Rotate90,
                info: raw_io::PhotoInfo { captured: Some("2026:08:14 18:42:07".into()), ..Default::default() },
            }),
            rating: Some(3),
            keywords: vec!["lake".into()],
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rne-export-job-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn photos_are_named_by_the_pattern_and_never_overwrite() {
        let dir = temp_dir("names");
        let options = ExportOptions {
            format: Format { kind: FileFormat::Png, ..Format::default() },
            name: NamePattern { pattern: "{date}_{counter}".into(), counter_start: 7, counter_digits: 2 },
            destination: Destination::Subfolder("out".into()),
            ..ExportOptions::default()
        };
        let job = ExportJob {
            photos: vec![open_photo(&dir.join("a.NEF")), open_photo(&dir.join("b.NEF"))],
            options: ExportOptions {
                name: NamePattern { pattern: "same".into(), ..NamePattern::default() },
                ..options.clone()
            },
            software: "test".into(),
        };
        let mut seen = Vec::new();
        let summary = run(&engine(), &job, &AtomicBool::new(false), &mut |i, n| seen.push((i, n)));
        assert_eq!(seen, vec![(0, "a.NEF".to_owned()), (1, "b.NEF".to_owned())]);
        assert_eq!(summary.written, vec![dir.join("out").join("same.png"), dir.join("out").join("same (2).png")]);
        assert!(summary.failed.is_empty() && !summary.cancelled);

        assert_eq!(
            wanted_name(&options, &dir.join("a.NEF"), 1, Some("2026:08:14 18:42:07"), None),
            "2026-08-14_08.png"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_photo_is_reported_and_the_others_still_export() {
        let dir = temp_dir("fail");
        let job = ExportJob {
            photos: vec![
                ExportPhoto {
                    path: dir.join("missing.NEF"),
                    recipe: None,
                    open: None,
                    rating: None,
                    keywords: Vec::new(),
                },
                open_photo(&dir.join("ok.NEF")),
            ],
            options: ExportOptions::default(),
            software: String::new(),
        };
        let summary = run(&engine(), &job, &AtomicBool::new(false), &mut |_, _| {});
        assert_eq!(summary.written, vec![dir.join("ok.jpg")]);
        assert_eq!(summary.failed.len(), 1);
        assert!(summary.failed[0].starts_with("missing.NEF"), "{:?}", summary.failed);
        let jpeg = std::fs::read(dir.join("ok.jpg")).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_export_stops_before_the_next_photo() {
        let dir = temp_dir("cancel");
        let job = ExportJob {
            photos: vec![open_photo(&dir.join("a.NEF"))],
            options: ExportOptions::default(),
            software: String::new(),
        };
        let summary = run(&engine(), &job, &AtomicBool::new(true), &mut |_, _| {});
        assert!(summary.cancelled && summary.written.is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_takes_the_catalog_rating_and_the_typed_copyright() {
        let info = raw_io::PhotoInfo {
            camera_rating: Some(5),
            copyright: Some("camera".into()),
            gps: Some((1.0, 2.0)),
            ..Default::default()
        };
        let photo = open_photo(Path::new("a.NEF"));
        let mut options = ExportOptions { copyright: " \u{a9} Me ".into(), ..ExportOptions::default() };
        let m = photo_metadata(None, &info, &photo, &options, "app");
        assert_eq!(
            (m.rating, m.copyright.as_deref(), m.software.as_deref()),
            (Some(3), Some("\u{a9} Me"), Some("app"))
        );
        assert_eq!(m.keywords, vec!["lake".to_owned()]);
        options.copyright.clear();
        options.metadata = MetadataChoice::AllButGps;
        let unrated = ExportPhoto { rating: None, ..photo };
        let m = photo_metadata(None, &info, &unrated, &options, "app");
        assert_eq!((m.rating, m.copyright.as_deref(), m.gps), (Some(5), Some("camera"), None));
    }

    #[test]
    fn the_export_is_upright_and_sized() {
        let dir = temp_dir("size");
        let options = ExportOptions {
            format: Format { kind: FileFormat::Tiff, ..Format::default() },
            size: export::Size { resize: export::Resize::LongEdge(3), upscale: false },
            ..ExportOptions::default()
        };
        let job = ExportJob { photos: vec![open_photo(&dir.join("p.NEF"))], options, software: String::new() };
        let summary = run(&engine(), &job, &AtomicBool::new(false), &mut |_, _| {});
        let tiff = std::fs::read(&summary.written[0]).unwrap();
        // Width and height are the first two LONG entries of IFD0 (tags 256, 257).
        let entry = |k: usize| u32::from_le_bytes(tiff[8 + 2 + 12 * k + 8..8 + 2 + 12 * k + 12].try_into().unwrap());
        assert_eq!((entry(0), entry(1)), (2, 3), "6 × 4 turned 90° and fitted to 3 pixels");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
