//! Computing AI masks in the background for the AI Select tool: the
//! segmentation models (ONNX Runtime) run on a thread of their own and fill
//! in the rasters of a mask's AI components; the window then puts the
//! completed definition into the recipe.
//!
//! The models stay loaded between jobs, and Segment Anything keeps the last
//! photo's encoding, so further clicks on the same photo take a fraction of
//! a second.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use ai_segment::catalog::ModelStore;
use ai_segment::{OnnxRuntime, Segmenter};
use core_model::{DevelopSettings, ImageF32, MaskDef, NodeId, Orientation};

/// The segmenter, loaded once.
static SEGMENTER: Mutex<Option<Arc<Segmenter>>> = Mutex::new(None);

/// File fingerprints already computed, by path and settings (reading a RAW
/// file to fingerprint it takes a moment).
static IDS: Mutex<Vec<(PathBuf, String, String)>> = Mutex::new(Vec::new());

/// A mask being computed.
#[derive(Debug)]
pub struct AiMaskJob {
    rx: Receiver<Result<MaskDef, String>>,
    /// The mask node it is for.
    pub mask: NodeId,
}

/// What the job needs about the photo.
#[derive(Debug, Clone)]
pub struct Photo {
    /// The RAW file.
    pub path: PathBuf,
    /// Its development settings (they change the pixels the model sees).
    pub settings: DevelopSettings,
    /// The developed image.
    pub image: Arc<ImageF32>,
    /// How it is turned upright.
    pub orientation: Orientation,
}

impl AiMaskJob {
    /// Computes the missing rasters of `def` (the mask of node `mask`).
    ///
    /// # Errors
    /// The thread could not be started.
    pub fn start(
        photo: Photo,
        mask: NodeId,
        mut def: MaskDef,
        notify: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        thread::Builder::new().name("rne-ai-mask".into()).spawn(move || {
            let result = run(&photo, &mut def).map(|()| def);
            let _ = tx.send(result);
            notify();
        })?;
        Ok(Self { rx, mask })
    }

    /// The completed definition, or why it failed, once the job is done.
    pub fn poll(&self) -> Option<Result<MaskDef, String>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("the AI selection stopped unexpectedly".to_owned())),
        }
    }
}

fn segmenter() -> Result<Arc<Segmenter>, String> {
    let mut slot = SEGMENTER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(s) = slot.as_ref() {
        return Ok(Arc::clone(s));
    }
    let dirs = [ai_models::models_dir(), ai_models::data_dir()];
    let library = ai_segment::find_library(&dirs).ok_or_else(|| {
        format!(
            "ONNX Runtime ({}) is not installed: download it from Tools \u{25b8} AI models",
            ai_segment::library_name()
        )
    })?;
    let runtime = OnnxRuntime::load(&library).map_err(|e| e.to_string())?;
    let s = Arc::new(Segmenter::new(Arc::new(runtime), ModelStore::new(ai_models::models_dir())));
    *slot = Some(Arc::clone(&s));
    Ok(s)
}

/// Names a file and its development settings for the raster cache (as the
/// command line does, so both share results).
fn image_id(path: &Path, settings: &DevelopSettings) -> Result<String, String> {
    let settings_text = format!("{settings:?}");
    let mut ids = IDS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, _, id)) = ids.iter().find(|(p, s, _)| p.as_path() == path && *s == settings_text) {
        return Ok(id.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let id = masks::fingerprint(&[&bytes, settings_text.as_bytes()]);
    ids.push((path.to_owned(), settings_text, id.clone()));
    if ids.len() > 32 {
        ids.remove(0);
    }
    Ok(id)
}

fn run(photo: &Photo, def: &mut MaskDef) -> Result<(), String> {
    let store = editor::worker::mask_store().ok_or("the mask cache folder cannot be created")?;
    let segmenter = segmenter()?;
    let id = image_id(&photo.path, &photo.settings)?;
    let failures = ai_segment::compute_pending(&segmenter, &id, &photo.image, photo.orientation, def, store);
    match failures.into_iter().next() {
        Some((index, e)) => {
            let label = def.components.get(index).map_or("A part of the mask", |c| c.label.as_str());
            Err(format!("{label} could not be computed: {e}"))
        }
        None => Ok(()),
    }
}
