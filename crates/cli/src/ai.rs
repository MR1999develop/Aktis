//! AI selection for the command line: where models and caches live, and
//! turning command-line selection options into a mask definition.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ai_segment::catalog::ModelStore;
use ai_segment::{OnnxRuntime, Segmenter};
use core_model::{
    BrushMask, BrushStroke, ImageF32, MaskComponent, MaskDef, MaskOp, NormPoint, NormRect, Orientation, PromptPoint,
};
use masks::DiskRasters;

use crate::BoxError;
use crate::args::{Op, Selection, Step};

/// Smallest detector confidence accepted from the command line.
const MIN_SCORE: f32 = 0.35;

/// Folder for models and caches: `$AKTIS_DATA_DIR`, else the system's data folder.
pub fn data_dir() -> PathBuf {
    ai_models::data_dir()
}

/// Folder of the model files: `$AKTIS_MODELS_DIR`, else `models` in the data folder.
pub fn models_dir() -> PathBuf {
    ai_models::models_dir()
}

/// The ONNX Runtime library, loaded.
fn runtime() -> Result<OnnxRuntime, BoxError> {
    let library = ai_segment::find_library(&[models_dir(), data_dir()]).ok_or_else(|| {
        format!(
            "ONNX Runtime ({}) not found: set ORT_DYLIB_PATH, or put it next to aktis-cli or in {}",
            ai_segment::library_name(),
            models_dir().display()
        )
    })?;
    Ok(OnnxRuntime::load(&library)?)
}

/// What `aktis-cli models` prints: the runtime in use, then every pack.
pub fn list_models(verify: bool) -> Result<(), BoxError> {
    let runtime_line = match runtime() {
        Ok(rt) => {
            let gpus = rt.available_gpu_providers();
            let gpus = if gpus.is_empty() { "CPU only".to_owned() } else { gpus.join(", ") };
            format!("{} ({gpus})", rt.library().display())
        }
        Err(e) => format!("not available: {e}"),
    };
    crate::models::list(verify, &runtime_line)
}

/// The engine and the raster cache.
#[derive(Debug)]
pub struct Ai {
    pub segmenter: Segmenter,
    pub rasters: DiskRasters,
}

impl Ai {
    /// Loads ONNX Runtime and opens the model folder and the raster cache.
    pub fn open() -> Result<Self, BoxError> {
        let rt = runtime()?;
        let segmenter = Segmenter::new(Arc::new(rt), ModelStore::new(models_dir()));
        let rasters = DiskRasters::open(data_dir().join("masks"))?;
        Ok(Self { segmenter, rasters })
    }

    /// Builds the mask definition for `sel` and computes its AI rasters.
    /// Coordinates on the command line are upright; masks are stored in the
    /// image's own orientation.
    pub fn build(
        &self,
        sel: &Selection,
        image_id: &str,
        image: &ImageF32,
        orientation: Orientation,
    ) -> Result<MaskDef, BoxError> {
        let stored = |x: f32, y: f32| -> Result<NormPoint, BoxError> {
            let p = NormPoint::new(x, y).ok_or("coordinates must be numbers")?;
            Ok(orientation.inverse().to_upright(p))
        };
        let mut people: Option<Vec<ai_segment::Detection>> = None;
        let mut components = Vec::new();
        for (op, step) in &sel.steps {
            let op = match op {
                Op::Add => MaskOp::Add,
                Op::Subtract => MaskOp::Subtract,
                Op::Intersect => MaskOp::Intersect,
            };
            match step {
                Step::Subject => components.push(ai_segment::subject(op)),
                Step::Detect(class) => {
                    let started = Instant::now();
                    let found = self.segmenter.detect(image, orientation, &[*class], MIN_SCORE)?;
                    eprintln!("aktis-cli: found {} in {:.2}s", describe(&found), started.elapsed().as_secs_f32());
                    components.extend(ai_segment::from_detections(&found, op));
                }
                Step::Parts(parts) => {
                    if people.is_none() {
                        let found = self.segmenter.detect(image, orientation, &[ai_segment::PERSON], MIN_SCORE)?;
                        eprintln!("aktis-cli: found {}", describe(&found));
                        people = Some(found);
                    }
                    for (n, person) in people.iter().flatten().enumerate() {
                        for part in parts {
                            let label = format!("Person {} \u{b7} {}", n + 1, part.label());
                            components.push(ai_segment::person_part(label, op, person.rect, *part));
                        }
                    }
                }
                Step::Object { points, rect } => {
                    let points = points
                        .iter()
                        .map(|(x, y, include)| Ok(PromptPoint { at: stored(*x, *y)?, include: *include }))
                        .collect::<Result<Vec<_>, BoxError>>()?;
                    let rect = match rect {
                        Some([x0, y0, x1, y1]) => Some(NormRect::new(stored(*x0, *y0)?, stored(*x1, *y1)?)),
                        None => None,
                    };
                    components.push(ai_segment::object("Object", op, points, rect));
                }
                Step::Brush { radius, path } => {
                    let points = path.iter().map(|(x, y)| stored(*x, *y)).collect::<Result<Vec<_>, BoxError>>()?;
                    let stroke = BrushStroke { points, radius: *radius, hardness: 0.5, opacity: 1.0 };
                    let label = if op == MaskOp::Subtract { "Erase" } else { "Brush" };
                    let brush = BrushMask { strokes: vec![stroke], edge_aware: sel.edge_aware };
                    components.push(MaskComponent::brush(label, op, brush));
                }
            }
        }
        if components.is_empty() {
            return Err("nothing to select: no object of the requested kind was found".into());
        }
        let mut def = MaskDef {
            components,
            refine_edges: !sel.no_refine,
            feather: sel.feather / 100.0,
            opacity: 1.0,
            invert: sel.invert,
        }
        .sanitized();

        let started = Instant::now();
        let failures =
            ai_segment::compute_pending(&self.segmenter, image_id, image, orientation, &mut def, &self.rasters);
        for (index, e) in &failures {
            eprintln!("aktis-cli: warning: '{}' could not be computed: {e}", def.components[*index].label);
        }
        let ai_parts = def.components.iter().filter(|c| matches!(c.source, core_model::MaskSource::Ai(_))).count();
        if let Some((_, first)) = failures.first()
            && failures.len() == ai_parts
        {
            return Err(first.clone().into());
        }
        eprintln!(
            "aktis-cli: AI masks ready in {:.2}s ({})",
            started.elapsed().as_secs_f32(),
            self.segmenter.devices().join(", ")
        );
        Ok(def)
    }
}

/// "3 persons, 1 dog".
fn describe(found: &[ai_segment::Detection]) -> String {
    if found.is_empty() {
        return "nothing".to_owned();
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for d in found {
        match counts.iter_mut().find(|(n, _)| *n == d.class_name()) {
            Some((_, c)) => *c += 1,
            None => counts.push((d.class_name(), 1)),
        }
    }
    counts.iter().map(|(n, c)| format!("{c} {n}")).collect::<Vec<_>>().join(", ")
}
