//! "AI-tag this folder": SigLIP 2 run on this computer over each photo's
//! embedded preview, on a background thread. The window stores the
//! keywords it reports in the catalog.
//!
//! The keyword list is `keywords.txt` in the editor's data folder when the
//! user has one (Lightroom keyword-list format), else the built-in
//! English list.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;

use core_model::{ImageF32, Rgba, WorkingSpace};
use tagging::{ImageEncoder, ImageTags, SuggestPolicy, Tagger, Vocabulary};

/// The built-in keyword list.
const DEFAULT_KEYWORDS: &str = include_str!("../../../data/keywords/default.txt");

/// File name of the user's own keyword list in the data folder.
pub const USER_KEYWORDS_FILE: &str = "keywords.txt";

/// How each keyword is put to the text model.
const PROMPT_TEMPLATE: &str = "a photo of {}";

/// Long side of the preview given to the model (it looks at 224 × 224).
const PREVIEW_SIDE: u32 = 448;

/// What the job reports.
#[derive(Debug, Clone)]
pub enum Progress {
    /// A photo's keywords, most likely first, with their probability.
    Tagged {
        /// The photo.
        path: PathBuf,
        /// Keyword names and probabilities.
        tags: Vec<(String, f32)>,
    },
    /// A photo that could not be read; the others go on.
    Skipped {
        /// The photo.
        path: PathBuf,
        /// Why.
        why: String,
    },
    /// The job could not start or stopped on an error.
    Failed(String),
    /// Every photo was done, or the job was stopped.
    Finished,
}

/// A running "AI-tag this folder".
#[derive(Debug)]
pub struct TaggingJob {
    rx: Receiver<Progress>,
    cancel: Arc<AtomicBool>,
    /// Photos to tag.
    pub total: usize,
    /// Photos done (tagged or skipped).
    pub done: usize,
    /// Photos tagged.
    pub tagged: usize,
    /// The first photo that could not be read, and why.
    pub first_skip: Option<String>,
    /// [`Progress::Finished`] was reported.
    finished: bool,
}

impl TaggingJob {
    /// Starts tagging `photos`; `notify` is called after each photo so the
    /// window repaints.
    ///
    /// # Errors
    /// The thread could not be started.
    pub fn start(photos: Vec<PathBuf>, notify: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let total = photos.len();
        let stop = Arc::clone(&cancel);
        thread::Builder::new().name("aktis-tagging".into()).spawn(move || {
            if let Err(message) = run(&photos, &stop, &tx, &notify) {
                let _ = tx.send(Progress::Failed(message));
            }
            let _ = tx.send(Progress::Finished);
            notify();
        })?;
        Ok(Self { rx, cancel, total, done: 0, tagged: 0, first_skip: None, finished: false })
    }

    /// What happened since the last call. A job whose thread ended without
    /// saying so (it panicked) reports [`Progress::Failed`] and
    /// [`Progress::Finished`], so the window never waits for it forever.
    pub fn poll(&mut self) -> Vec<Progress> {
        let mut events = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => events.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.finished {
                        events.push(Progress::Failed("The keyword job stopped unexpectedly".to_owned()));
                        events.push(Progress::Finished);
                    }
                    break;
                }
            }
        }
        if events.iter().any(|e| matches!(e, Progress::Finished)) {
            self.finished = true;
        }
        for e in &events {
            match e {
                Progress::Tagged { .. } => {
                    self.done += 1;
                    self.tagged += 1;
                }
                Progress::Skipped { path, why } => {
                    self.done += 1;
                    if self.first_skip.is_none() {
                        self.first_skip = Some(format!("{}: {why}", editor::library::folder_name(path)));
                    }
                }
                Progress::Failed(_) | Progress::Finished => {}
            }
        }
        events
    }

    /// Asks the job to stop after the current photo.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for TaggingJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Loads the model and keywords, then tags each photo.
fn run(photos: &[PathBuf], stop: &AtomicBool, tx: &Sender<Progress>, notify: &dyn Fn()) -> Result<(), String> {
    let spec = &tagging_onnx::SIGLIP2_BASE_224;
    // Leave half the cores to the editor.
    let threads = thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1));
    let opts = tagging_onnx::LoadOptions { prefer_quantized: false, threads: Some(threads) };
    let dir = ai_models::models_dir().join(ai_models::SIGLIP2_DIR);
    let (mut images, mut texts) =
        tagging_onnx::load(&dir, spec, &opts).map_err(|e| format!("The keyword model could not be loaded: {e}"))?;
    let head = spec.head().ok_or("The keyword model has an invalid scale")?;
    let mut tagger = Tagger::new(spec.model_id(), head, vocabulary()?);
    tagger
        .encode_prompts(&mut texts, PROMPT_TEMPLATE)
        .map_err(|e| format!("The keywords could not be prepared: {e}"))?;
    let policy = SuggestPolicy::default();
    for path in photos {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let result = preview(path).and_then(|image| {
            let embedding = images
                .encode_images(&[&image])
                .map_err(|e| e.to_string())?
                .pop()
                .ok_or_else(|| "the model returned nothing".to_owned())?;
            tagger.suggest(&embedding, &ImageTags::new(), &policy).map_err(|e| e.to_string())
        });
        let progress = match result {
            Ok(suggestions) => {
                let vocab = tagger.vocabulary();
                let tags = suggestions
                    .iter()
                    .filter_map(|s| vocab.get(s.keyword).map(|k| (k.name().to_owned(), s.probability)))
                    .collect();
                Progress::Tagged { path: path.clone(), tags }
            }
            Err(why) => Progress::Skipped { path: path.clone(), why },
        };
        if tx.send(progress).is_err() {
            break; // the window is gone
        }
        notify();
    }
    Ok(())
}

/// The user's keyword list if there is one, else the built-in one.
fn vocabulary() -> Result<Vocabulary, String> {
    let user = ai_models::data_dir().join(USER_KEYWORDS_FILE);
    match std::fs::read_to_string(&user) {
        Ok(text) => Vocabulary::parse_lightroom(&text).map_err(|e| format!("{}: {e}", user.display())),
        Err(_) => Vocabulary::parse_lightroom(DEFAULT_KEYWORDS).map_err(|e| format!("built-in keywords: {e}")),
    }
}

/// The embedded preview of a RAW file as a linear image for the model.
fn preview(path: &Path) -> Result<ImageF32, String> {
    let thumb = raw_io::read_thumbnail(path, PREVIEW_SIDE).map_err(|e| e.to_string())?;
    let lin = |v: u8| core_color::srgb_decode(f32::from(v) / 255.0);
    let pixels: Vec<Rgba> =
        thumb.rgb.as_chunks::<3>().0.iter().map(|p| [lin(p[0]), lin(p[1]), lin(p[2]), 1.0]).collect();
    ImageF32::from_pixels(thumb.width, thumb.height, WorkingSpace::LinearSrgb, pixels).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_keywords_parse() {
        let vocab = Vocabulary::parse_lightroom(DEFAULT_KEYWORDS).expect("valid list");
        assert!(vocab.len() > 100);
        assert!(!vocab.find_name("landscape").is_empty());
    }
}
