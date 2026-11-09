//! Background work: opening files, rendering previews, exporting.
//!
//! The UI thread never decodes, develops, renders or writes. It sends
//! requests to a [`Worker`] and reads [`Event`]s back once per frame with
//! [`Worker::poll`]. After each event the worker calls the `notify`
//! function given at start, so the UI wakes up and repaints.
//!
//! - Opening and exporting run on their own short-lived thread each.
//! - Previews run on one long-lived thread. Only the newest request is kept:
//!   while the user drags a slider, intermediate values that the renderer
//!   could not reach in time are skipped instead of queued.
//! - The engine (GPU start-up can take a second) is created on a thread of
//!   its own; renders wait for it.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use core_model::{ImageF32, Layers, Orientation, WorkingSpace};
use engine::{Backend, Engine, EngineChoice, Rgb8Image};
use tool_api::ToolRegistry;

use crate::document::{DevelopedImage, Opened, RawData};
use crate::export_job::{ExportHandle, ExportJob, ExportSummary};
use crate::histogram::Histogram;
use crate::library::{FolderListing, list_folder};

/// Longer side of the preview copy made when a file is opened.
pub const PREVIEW_SIDE: u32 = 1920;

/// Longest side of the draft copy rendered while a slider is dragged: about
/// the size of the viewer on a full-HD screen, under half the preview's pixels.
pub const DRAFT_SIDE: u32 = 1280;

/// Longer side of filmstrip thumbnails (twice the card width, for sharp HiDPI screens).
pub const THUMBNAIL_SIDE: u32 = 320;

/// The cache of AI mask results shared by the renders and the AI Select
/// tool: `masks` in the editor's data folder (as the command line uses).
/// `None` when the folder cannot be made; AI masks then render empty.
#[must_use]
pub fn mask_store() -> Option<&'static masks::DiskRasters> {
    static STORE: OnceLock<Option<masks::DiskRasters>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let dir = platform::editor_data_dir().unwrap_or_else(|| PathBuf::from(".aktis")).join("masks");
            masks::DiskRasters::open(dir).ok()
        })
        .as_ref()
}

/// The rasters renders read AI masks from.
pub(crate) fn rasters() -> &'static dyn masks::RasterSource {
    match mask_store() {
        Some(store) => store,
        None => &masks::NoRasters,
    }
}

/// Longest side of the large previews shown when several photos fill the viewer.
pub const TILE_PREVIEW_SIDE: u32 = 1600;

/// Threads making thumbnails at once (each reads a file and decodes a JPEG).
const THUMBNAIL_THREADS: usize = 3;

/// Called after every event, from a worker thread (e.g. `egui::Context::request_repaint`).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// Which source a render uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    /// The smallest copy: fastest, used while a control is dragged.
    Draft,
    /// The reduced preview copy: sharp when the whole photo is shown.
    Preview,
    /// The full-size image: sharp at 100 %.
    Full,
}

/// A render of a document's recipe for the screen.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    /// The document generation this render is for.
    pub generation: u64,
    /// Which source.
    pub quality: Quality,
    /// The layers to render.
    pub layers: Layers,
    /// The developed image to start from.
    pub source: Arc<ImageF32>,
    /// How to turn the result upright.
    pub orientation: Orientation,
    /// Largest texture side the screen accepts.
    pub max_side: u32,
    /// Perspective, straightening and crop of the finished image.
    pub geometry: core_model::Geometry,
    /// Apply the crop (off while the crop is being edited, so the whole
    /// frame shows around it).
    pub crop: bool,
    /// Tone curve of the output render.
    pub curve: core_model::BaseCurve,
}

/// What the worker reports back.
#[derive(Debug)]
pub enum Event {
    /// The engine is ready; `backend` says where renders run.
    EngineReady {
        /// Where renders run.
        backend: Backend,
        /// Why there is no GPU, when one was wanted.
        gpu_problem: Option<String>,
    },
    /// A file was opened (or could not be).
    Opened {
        /// The ticket given to [`Worker::open`].
        ticket: u64,
        /// The image, or a message for the user.
        result: Result<Box<Opened>, String>,
        /// How long the file took to open.
        timing: OpenTiming,
    },
    /// An image was developed again with new development settings.
    Developed {
        /// The ticket given to [`Worker::develop`].
        ticket: u64,
        /// The new image, or a message for the user.
        result: Result<Box<DevelopedImage>, String>,
    },
    /// A render for the screen is ready.
    Rendered {
        /// Document generation it was made for.
        generation: u64,
        /// Which source it used.
        quality: Quality,
        /// The longest side asked ([`RenderRequest::max_side`]).
        max_side: u32,
        /// Upright 8-bit pixels.
        image: Rgb8Image,
        /// Where it ran.
        backend: Backend,
        /// Why the GPU was skipped, if it failed and the CPU took over.
        gpu_failure: Option<String>,
        /// Tools that could not run and were passed through, as messages for
        /// the user ("AI denoise: no SCUNet model found …").
        tool_problems: Vec<String>,
        /// Levels of the displayed pixels.
        histogram: Box<Histogram>,
        /// How long it took.
        timing: RenderTiming,
    },
    /// A render for the screen failed.
    RenderFailed {
        /// Document generation it was made for.
        generation: u64,
        /// Message for the user.
        message: String,
    },
    /// An export moved on to its next photo.
    ExportProgress {
        /// Photos finished so far.
        done: usize,
        /// Photos in the export.
        total: usize,
        /// File name of the photo now being exported.
        current: String,
    },
    /// An export ended: what was written, what failed, whether it was stopped.
    ExportFinished(ExportSummary),
    /// A folder was listed for the Explorer.
    FolderListed {
        /// The folder.
        dir: PathBuf,
        /// Its sub-folders and photos, or a message for the user.
        result: Result<FolderListing, String>,
    },
    /// A filmstrip thumbnail is ready (or could not be made).
    Thumbnail {
        /// The photo.
        path: PathBuf,
        /// Upright 8-bit pixels, or why there is none.
        result: Result<Arc<raw_io::Thumbnail>, String>,
    },
    /// A large preview (several photos in the viewer) is ready (or could not be made).
    Preview {
        /// The photo.
        path: PathBuf,
        /// Upright 8-bit pixels, at most [`TILE_PREVIEW_SIDE`] on the longer side, or why there is none.
        result: Result<Arc<raw_io::Thumbnail>, String>,
    },
}

/// Time spent opening a file, shown in the status bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenTiming {
    /// Reading and decoding the RAW file.
    pub decode: Duration,
    /// Developing it (demosaic, lens, profile) and making the preview copies.
    pub develop: Duration,
    /// It had been opened ahead of time ([`Worker::prefetch`]): the times are
    /// those of the background work, the user waited for none of it.
    pub ahead: bool,
}

/// The one photo opened ahead of time: the next one in the filmstrip.
#[derive(Debug, Default)]
struct Prefetch {
    state: Mutex<PrefetchState>,
    finished: Condvar,
}

/// The photos asked to open: one is opened at a time, and only the newest
/// request matters. Moving through the filmstrip asks for photo after
/// photo; the ones passed over stop at the next step instead of taking the
/// processor from the one the user stopped on.
#[derive(Debug, Default)]
struct Opens {
    /// The ticket of the newest request.
    newest: AtomicU64,
    /// Held while a photo is being opened.
    one_at_a_time: Mutex<()>,
}

impl Opens {
    /// `true` while `ticket` is the newest request.
    fn wanted(&self, ticket: u64) -> bool {
        self.newest.load(Ordering::Acquire) == ticket
    }
}

#[derive(Debug, Default)]
struct PrefetchState {
    /// The photo being or already opened ahead; `None` when there is none.
    path: Option<PathBuf>,
    /// Its result once ready.
    result: Option<(Result<Opened, String>, OpenTiming)>,
}

impl Prefetch {
    /// The result for `path` when it is the photo opened ahead, waiting for
    /// it if still running; `None` (nothing waited for) for any other photo.
    fn take(&self, path: &std::path::Path) -> Option<(Result<Opened, String>, OpenTiming)> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.path.as_deref() != Some(path) {
            return None;
        }
        let mut state = self
            .finished
            .wait_while(state, |s| s.path.as_deref() == Some(path) && s.result.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        if state.path.as_deref() != Some(path) {
            return None; // replaced meanwhile by another photo
        }
        state.path = None;
        state.result.take().map(|(r, t)| (r, OpenTiming { ahead: true, ..t }))
    }
}

/// Time spent on one screen render, shown in the status bar.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RenderTiming {
    /// Rendering the layers (GPU or CPU).
    pub render: Duration,
    /// Turning the result into upright 8-bit pixels and the histogram.
    pub display: Duration,
}

#[derive(Debug, Default)]
struct Slot {
    request: Option<RenderRequest>,
    shutdown: bool,
}

#[derive(Debug, Default)]
struct RenderQueue {
    slot: Mutex<Slot>,
    wake: Condvar,
}

/// A photo whose thumbnail or large preview is wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbJob {
    /// The photo.
    pub path: PathBuf,
    /// The JPEG of the edited image the catalog keeps, shown instead of the
    /// camera's embedded preview (as in Capture One); `None` for a photo
    /// never edited.
    pub edited: Option<Arc<[u8]>>,
}

impl From<PathBuf> for ThumbJob {
    fn from(path: PathBuf) -> Self {
        Self { path, edited: None }
    }
}

#[derive(Debug, Default)]
struct ThumbState {
    queue: VecDeque<ThumbJob>,
    /// Large previews, served before thumbnails (they are on screen).
    previews: VecDeque<ThumbJob>,
    shutdown: bool,
}

/// Photos waiting for a thumbnail, served by a few threads.
#[derive(Debug, Default)]
struct ThumbQueue {
    state: Mutex<ThumbState>,
    wake: Condvar,
}

/// Owns the background threads. Dropping it stops the preview thread.
pub struct Worker {
    tx: Sender<Event>,
    rx: Receiver<Event>,
    notify: Notify,
    engine: Arc<OnceLock<Arc<Engine>>>,
    queue: Arc<RenderQueue>,
    render_thread: Option<JoinHandle<()>>,
    thumbs: Arc<ThumbQueue>,
    thumb_threads: Vec<JoinHandle<()>>,
    prefetch: Arc<Prefetch>,
    opens: Arc<Opens>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker").field("engine_ready", &self.engine.get().is_some()).finish_non_exhaustive()
    }
}

impl Worker {
    /// Starts the engine and the preview thread.
    ///
    /// # Errors
    /// The operating system refused to start a thread.
    pub fn start(registry: ToolRegistry, choice: EngineChoice, notify: Notify) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let engine: Arc<OnceLock<Arc<Engine>>> = Arc::new(OnceLock::new());
        let queue = Arc::new(RenderQueue::default());
        // Renders on the CPU while the GPU starts (seconds with some drivers),
        // so the first photo shows at once.
        let cpu = Arc::new(Engine::new(registry.clone(), EngineChoice::CpuOnly));

        {
            let (engine, tx, notify) = (Arc::clone(&engine), tx.clone(), Arc::clone(&notify));
            thread::Builder::new().name("engine-start".into()).spawn(move || {
                // A driver that panics while starting must not leave renders waiting forever.
                let made = catch_unwind(AssertUnwindSafe(|| Engine::new(registry.clone(), choice)))
                    .unwrap_or_else(|_| Engine::new(registry, EngineChoice::CpuOnly));
                let event =
                    Event::EngineReady { backend: made.backend(), gpu_problem: made.gpu_problem().map(str::to_owned) };
                let _ = engine.set(Arc::new(made));
                let _ = tx.send(event);
                notify();
            })?;
        }

        let render_thread = {
            let (engine, queue, tx, notify) =
                (Arc::clone(&engine), Arc::clone(&queue), tx.clone(), Arc::clone(&notify));
            thread::Builder::new()
                .name("preview".into())
                .spawn(move || preview_loop(&engine, &cpu, &queue, &tx, &*notify))?
        };

        let thumbs = Arc::new(ThumbQueue::default());
        let mut thumb_threads = Vec::with_capacity(THUMBNAIL_THREADS);
        for i in 0..THUMBNAIL_THREADS {
            let (thumbs, tx, notify) = (Arc::clone(&thumbs), tx.clone(), Arc::clone(&notify));
            thumb_threads.push(
                thread::Builder::new()
                    .name(format!("thumbnails-{i}"))
                    .spawn(move || thumbnail_loop(&thumbs, &tx, &*notify))?,
            );
        }

        Ok(Self {
            tx,
            rx,
            notify,
            engine,
            queue,
            render_thread: Some(render_thread),
            thumbs,
            thumb_threads,
            prefetch: Arc::new(Prefetch::default()),
            opens: Arc::new(Opens::default()),
        })
    }

    /// Events that arrived since the last call. Never blocks.
    pub fn poll(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    /// `true` once the engine has started.
    #[must_use]
    pub fn engine_ready(&self) -> bool {
        self.engine.get().is_some()
    }

    /// Opens `path` in the background: decode, develop, make the preview copy.
    /// The result comes back as [`Event::Opened`] with the same `ticket`.
    /// A photo opened ahead with [`Self::prefetch`] comes back at once (or as
    /// soon as that work ends).
    ///
    /// Tickets grow with each request. One photo is opened at a time; a
    /// request replaced by a newer one stops at its next step and reports
    /// nothing (the caller waits only for the newest).
    pub fn open(&self, ticket: u64, path: PathBuf) {
        let (tx, notify) = (self.tx.clone(), Arc::clone(&self.notify));
        let (prefetch, opens) = (Arc::clone(&self.prefetch), Arc::clone(&self.opens));
        opens.newest.fetch_max(ticket, Ordering::AcqRel);
        let fallback = tx.clone();
        let spawned = thread::Builder::new().name("open".into()).spawn(move || {
            let _turn = opens.one_at_a_time.lock().unwrap_or_else(PoisonError::into_inner);
            let wanted = || opens.wanted(ticket);
            if !wanted() {
                return;
            }
            let opened = match prefetch.take(&path) {
                Some(done) => Some(done),
                None => open_file_guarded(&path, &wanted),
            };
            let Some((result, timing)) = opened.filter(|_| wanted()) else { return };
            let _ = tx.send(Event::Opened { ticket, result: result.map(Box::new), timing });
            notify();
        });
        if let Err(e) = spawned {
            let _ = fallback.send(Event::Opened {
                ticket,
                result: Err(format!("cannot start opening: {e}")),
                timing: OpenTiming::default(),
            });
        }
    }

    /// Opens `path` ahead of time, in the background, so that [`Self::open`]
    /// of it is immediate (the next photo in the filmstrip). One photo is
    /// kept: asking for another replaces it; asking again for the same one
    /// does nothing.
    pub fn prefetch(&self, path: PathBuf) {
        {
            let mut state = self.prefetch.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.path.as_ref() == Some(&path) {
                return;
            }
            state.path = Some(path.clone());
            state.result = None; // frees the previous photo's memory now
        }
        self.prefetch.finished.notify_all();
        let prefetch = Arc::clone(&self.prefetch);
        let spawned = thread::Builder::new().name("prefetch".into()).spawn(move || {
            // Opened ahead whatever the user does meanwhile: never abandoned.
            let Some(done) = open_file_guarded(&path, &|| true) else { return };
            let mut state = prefetch.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.path.as_ref() == Some(&path) {
                state.result = Some(done);
            }
            drop(state);
            prefetch.finished.notify_all();
        });
        if spawned.is_err() {
            // Not opened ahead: a later open simply does the work itself.
            let mut state = self.prefetch.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.path = None;
            drop(state);
            self.prefetch.finished.notify_all();
        }
    }

    /// Develops `raw` again with `settings` in the background (lens
    /// corrections or profile changed). Reported as [`Event::Developed`].
    pub fn develop(&self, ticket: u64, raw: Arc<RawData>, settings: core_model::DevelopSettings) {
        let (tx, notify) = (self.tx.clone(), Arc::clone(&self.notify));
        let fallback = tx.clone();
        let spawned = thread::Builder::new().name("develop".into()).spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| develop_raw(&raw, &settings)))
                .unwrap_or_else(|_| Err("the image could not be developed (internal error)".to_owned()));
            let _ = tx.send(Event::Developed { ticket, result: result.map(Box::new) });
            notify();
        });
        if let Err(e) = spawned {
            let _ = fallback.send(Event::Developed { ticket, result: Err(format!("cannot start developing: {e}")) });
        }
    }

    /// Lists `dir` for the Explorer: sub-folders and RAW files. Reported as
    /// [`Event::FolderListed`].
    pub fn list_folder(&self, dir: PathBuf) {
        let (tx, notify) = (self.tx.clone(), Arc::clone(&self.notify));
        let fallback = tx.clone();
        let fallback_dir = dir.clone();
        let spawned = thread::Builder::new().name("list-folder".into()).spawn(move || {
            let result = list_folder(&dir).map_err(|e| format!("{} cannot be read: {e}", display_name(&dir)));
            let _ = tx.send(Event::FolderListed { dir, result });
            notify();
        });
        if let Err(e) = spawned {
            let _ = fallback.send(Event::FolderListed { dir: fallback_dir, result: Err(format!("cannot start: {e}")) });
        }
    }

    /// Asks for the thumbnails of `paths`, first ones first. Replaces the
    /// photos still waiting from an earlier request (another folder, or
    /// rows scrolled away). Each one comes back as [`Event::Thumbnail`].
    pub fn request_thumbnails(&self, jobs: Vec<ThumbJob>) {
        let mut state = self.thumbs.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.queue = jobs.into();
        drop(state);
        self.thumbs.wake.notify_all();
    }

    /// Asks for the large previews of `paths` (the photos filling the
    /// viewer), first ones first, before any thumbnail. Replaces the previews
    /// still waiting. Each one comes back as [`Event::Preview`].
    pub fn request_previews(&self, jobs: Vec<ThumbJob>) {
        let mut state = self.thumbs.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.previews = jobs.into();
        drop(state);
        self.thumbs.wake.notify_all();
    }

    /// Asks for a screen render. Replaces a request that has not started yet.
    pub fn render(&self, request: RenderRequest) {
        let mut slot = self.queue.slot.lock().unwrap_or_else(PoisonError::into_inner);
        slot.request = Some(request);
        drop(slot);
        self.queue.wake.notify_one();
    }

    /// Exports the photos of `job` one after another on a background
    /// thread: [`Event::ExportProgress`] before each photo, then
    /// [`Event::ExportFinished`]. The handle stops it.
    pub fn export(&self, job: ExportJob) -> ExportHandle {
        let handle = ExportHandle::default();
        let (tx, notify, engine) = (self.tx.clone(), Arc::clone(&self.notify), Arc::clone(&self.engine));
        let fallback = tx.clone();
        let total = job.photos.len();
        let stop = handle.clone();
        let spawned = thread::Builder::new().name("export".into()).spawn(move || {
            let engine = engine.wait();
            let mut progress = |done: usize, current: String| {
                let _ = tx.send(Event::ExportProgress { done, total, current });
                notify();
            };
            let summary = crate::export_job::run(engine, &job, stop.flag(), &mut progress);
            let _ = tx.send(Event::ExportFinished(summary));
            notify();
        });
        if let Err(e) = spawned {
            let failed = vec![format!("cannot start the export: {e}")];
            let _ = fallback.send(Event::ExportFinished(ExportSummary { total, failed, ..ExportSummary::default() }));
        }
        handle
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.queue.slot.lock().unwrap_or_else(PoisonError::into_inner).shutdown = true;
        self.queue.wake.notify_all();
        {
            let mut state = self.thumbs.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.shutdown = true;
            state.queue.clear();
        }
        self.thumbs.wake.notify_all();
        if let Some(handle) = self.render_thread.take() {
            let _ = handle.join();
        }
        for handle in self.thumb_threads.drain(..) {
            let _ = handle.join();
        }
    }
}

fn thumbnail_loop(thumbs: &ThumbQueue, tx: &Sender<Event>, notify: &(dyn Fn() + Send + Sync)) {
    loop {
        let next = {
            let guard = thumbs.state.lock().unwrap_or_else(PoisonError::into_inner);
            let mut state = thumbs
                .wake
                .wait_while(guard, |s| s.queue.is_empty() && s.previews.is_empty() && !s.shutdown)
                .unwrap_or_else(PoisonError::into_inner);
            if state.shutdown {
                return;
            }
            match state.previews.pop_front() {
                Some(p) => Some((p, true)),
                None => state.queue.pop_front().map(|p| (p, false)),
            }
        };
        let Some((ThumbJob { path, edited }, large)) = next else { continue };
        let side = if large { TILE_PREVIEW_SIDE } else { THUMBNAIL_SIDE };
        let result = catch_unwind(AssertUnwindSafe(|| {
            // The edited image when the catalog has one that decodes, else the camera's preview.
            edited
                .and_then(|jpeg| raw_io::decode_jpeg(&jpeg, side).ok())
                .map_or_else(|| raw_io::read_thumbnail(&path, side), Ok)
        }))
        .map_err(|_| "the preview could not be read (internal error)".to_owned())
        .and_then(|r| r.map(Arc::new).map_err(|e| e.to_string()));
        let event = if large { Event::Preview { path, result } } else { Event::Thumbnail { path, result } };
        if tx.send(event).is_err() {
            return; // the UI is gone
        }
        notify();
    }
}

/// Renders the screen requests of `queue`, with `engine` once it has
/// started and with `cpu` until then.
fn preview_loop(
    engine: &OnceLock<Arc<Engine>>,
    cpu: &Engine,
    queue: &RenderQueue,
    tx: &Sender<Event>,
    notify: &(dyn Fn() + Send + Sync),
) {
    loop {
        let request = {
            let guard = queue.slot.lock().unwrap_or_else(PoisonError::into_inner);
            let mut slot = queue
                .wake
                .wait_while(guard, |s| s.request.is_none() && !s.shutdown)
                .unwrap_or_else(PoisonError::into_inner);
            if slot.shutdown {
                return;
            }
            slot.request.take()
        };
        let Some(req) = request else { continue };
        let engine = engine.get().map_or(cpu, AsRef::as_ref);
        let event = catch_unwind(AssertUnwindSafe(|| render_for_screen(engine, &req))).unwrap_or_else(|_| {
            Event::RenderFailed {
                generation: req.generation,
                message: "the preview stopped because of an internal error".into(),
            }
        });
        if tx.send(event).is_err() {
            return; // the UI is gone
        }
        notify();
    }
}

fn render_for_screen(engine: &Engine, req: &RenderRequest) -> Event {
    let started = Instant::now();
    let rendered = engine.render_with_masks(&req.layers, &req.source, rasters());
    let rendered_at = Instant::now();
    match engine::to_display_geometry(
        &rendered.image,
        req.orientation,
        &req.geometry,
        req.crop,
        req.max_side,
        req.curve,
    ) {
        Ok(image) => Event::Rendered {
            generation: req.generation,
            quality: req.quality,
            max_side: req.max_side,
            histogram: Box::new(Histogram::of_image(&image.pixels, image.width as usize)),
            timing: RenderTiming { render: rendered_at - started, display: rendered_at.elapsed() },
            image,
            backend: rendered.backend,
            gpu_failure: rendered.gpu_failure,
            tool_problems: rendered.tool_errors.iter().map(|(_, e)| e.to_string()).collect(),
        },
        Err(e) => Event::RenderFailed { generation: req.generation, message: e.to_string() },
    }
}

fn display_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// [`open_file`], with a panic turned into a message for the user.
/// Opens `path` as [`open_file`] does, a panic becoming an error; `None`
/// when `wanted` turned false on the way.
fn open_file_guarded(
    path: &std::path::Path,
    wanted: &dyn Fn() -> bool,
) -> Option<(Result<Opened, String>, OpenTiming)> {
    let mut timing = OpenTiming::default();
    let result = catch_unwind(AssertUnwindSafe(|| open_file(path, &mut timing, wanted)))
        .unwrap_or_else(|_| Some(Err(format!("{} could not be opened (internal error)", display_name(path)))))?;
    Some((result, timing))
}

/// Decodes, develops (camera profile and lens corrections included) and
/// makes the preview copy, recording the time of each phase in `timing`.
/// Errors are user messages. `None` when `wanted` turned false between two
/// phases: the photo is no longer needed, the rest is not done.
fn open_file(
    path: &std::path::Path,
    timing: &mut OpenTiming,
    wanted: &dyn Fn() -> bool,
) -> Option<Result<Opened, String>> {
    let name = display_name(path);
    let started = Instant::now();
    let file = match raw_io::read_raw(path) {
        Ok(file) => file,
        Err(e) => return Some(Err(format!("{name} could not be opened: {e}"))),
    };
    let raw = Arc::new(RawData { sensor: file.sensor, capture: file.capture, file_notes: file.notes });
    timing.decode = started.elapsed();
    if !wanted() {
        return None;
    }
    let started = Instant::now();
    let image = match develop_raw(&raw, &core_model::DevelopSettings::default()) {
        Ok(image) => image,
        Err(e) => return Some(Err(format!("{name}: {e}"))),
    };
    timing.develop = started.elapsed();
    if !wanted() {
        return None;
    }

    // The lens as recognised, and the lenses that fit the camera, for the inspector.
    let (db, _) = engine::lens_database();
    let camera = develop::prepare::find_camera(&db, &raw.capture);
    let detected_lens = develop::prepare::find_lens(&db, camera, &raw.capture, None).map(lens_db::Lens::full_name);
    let lens_choices = crate::lenses::lens_choices(&db, camera);
    let lens_key = raw.capture.lens_key();
    let started = Instant::now();
    let camera_curve = if raw.capture.rendered { None } else { crate::camera_look::camera_curve(path, &image.preview) };
    timing.develop += started.elapsed();

    Some(Ok(Opened {
        path: path.to_owned(),
        camera: format!("{} {}", file.make, file.model).trim().to_owned(),
        orientation: file.orientation,
        has_camera_white_balance: file.has_camera_white_balance,
        raw: Some(raw),
        detected_lens,
        lens_choices,
        lens_key,
        info: file.info,
        image,
        camera_curve,
    }))
}

/// Runs the development stage on `raw` with `settings`: the developed
/// image and the lens database's notes. Errors are user messages.
pub(crate) fn develop_stage(
    raw: &RawData,
    settings: &core_model::DevelopSettings,
) -> Result<(develop::prepare::Developed, Vec<String>), String> {
    let (lens_db, notes) = engine::lens_database();
    let library = engine::profile_library();
    let result = develop::prepare::develop_capture(
        &raw.sensor,
        &raw.capture,
        settings,
        WorkingSpace::LinearRec2020,
        &lens_db,
        Some(&library),
    )
    .map_err(|e| format!("could not be developed: {e}"))?;
    Ok((result, notes))
}

/// Runs the development stage on `raw` with `settings` and makes the
/// preview copy. Errors are user messages.
fn develop_raw(raw: &RawData, settings: &core_model::DevelopSettings) -> Result<DevelopedImage, String> {
    let (result, mut notes) = develop_stage(raw, settings)?;
    let lens = result.lens.correction.as_ref().and(result.lens.lens.clone());
    if let Some(lens_name) = &lens {
        let a = result.lens.applied;
        let what: Vec<&str> =
            [(a.distortion, "distortion"), (a.tca, "chromatic aberration"), (a.vignetting, "vignetting")]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, n)| *n)
                .collect();
        notes.insert(0, format!("{lens_name}: {} corrected", what.join(", ")));
    }
    if settings.lens.enabled {
        notes.extend(result.lens.note.clone());
    }
    notes.extend(result.profile.note.clone());
    let profile = result.profile.describe();
    notes.extend(raw.file_notes.iter().cloned());
    let developed = result.image;
    let preview = engine::downscale(&developed, PREVIEW_SIDE).map_err(|e| e.to_string())?;
    let draft = engine::downscale(&preview, DRAFT_SIDE).map_err(|e| e.to_string())?;
    let developed = Arc::new(developed);
    let preview = if preview.width() == developed.width() { Arc::clone(&developed) } else { Arc::new(preview) };
    let draft = if draft.width() == preview.width() { Arc::clone(&preview) } else { Arc::new(draft) };
    Ok(DevelopedImage { settings: settings.clone(), developed, preview, draft, lens, notes, profile })
}

/// Decides which screen render to ask for next, so that editing stays
/// fast (previews) and the image ends up sharp (one full render once the
/// user stops).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderPlan {
    requested: Option<(u64, Quality)>,
    shown: Option<(u64, Quality)>,
    floor: u64,
    /// Longest side asked of the last request and of the render on screen
    /// (the size the viewer shows the photo at, or the full size).
    requested_side: u32,
    shown_side: u32,
}

impl RenderPlan {
    /// Starts over for another document whose generations begin at
    /// `first_generation`; late results of earlier documents are refused.
    pub fn reset(&mut self, first_generation: u64) {
        *self = Self { floor: first_generation, ..Self::default() };
    }

    /// What to request now for a document at `generation`, or `None`.
    ///
    /// While the user drags a control (`busy`) a change is rendered from the
    /// draft when that is smaller (`draft_helps`); once the drag ends the
    /// preview follows, then the full-size image when the zoom shows more
    /// than the preview has (`full_adds_detail`).
    #[must_use]
    pub fn next(&self, generation: u64, busy: bool, draft_helps: bool, full_adds_detail: bool) -> Option<Quality> {
        let shown = |q: Quality| self.shown == Some((generation, q));
        match self.requested {
            Some((g, q)) if g == generation => match q {
                Quality::Draft if !busy && shown(Quality::Draft) => Some(Quality::Preview),
                Quality::Preview if !busy && full_adds_detail && shown(Quality::Preview) => Some(Quality::Full),
                // A full render asked while previews were wanted (zoomed in) is not repeated.
                _ => None,
            },
            _ if busy && draft_helps => Some(Quality::Draft),
            _ => Some(Quality::Preview),
        }
    }

    /// Records a request that was sent.
    pub fn requested(&mut self, generation: u64, quality: Quality) {
        self.requested = Some((generation, quality));
    }

    /// Records a request that was sent for a picture of longest side `side`.
    pub fn requested_at(&mut self, generation: u64, quality: Quality, side: u32) {
        self.requested(generation, quality);
        self.requested_side = side;
    }

    /// Whether the render on screen is of the current recipe but for
    /// another size than `side` (the viewer zoomed or was resized), with
    /// nothing in flight: then the same image is asked again at that size.
    #[must_use]
    pub fn resize_needed(&self, generation: u64, side: u32) -> bool {
        let differs = |a: u32, b: u32| f64::from(a.max(b)) > f64::from(a.min(b)) * 1.02;
        self.requested == self.shown
            && self.shown.is_some_and(|(g, _)| g == generation)
            && differs(self.shown_side, side)
    }

    /// [`Self::accepts`], also taking the answer to the last request when
    /// it re-renders what is on screen at another size.
    #[must_use]
    pub fn accepts_at(&self, generation: u64, quality: Quality, side: u32) -> bool {
        let last_request = self.requested == Some((generation, quality)) && side == self.requested_side;
        let new_size = self.shown.is_none_or(|(g, _)| generation >= g) && side != self.shown_side;
        self.accepts(generation, quality) || (generation >= self.floor && last_request && new_size)
    }

    /// [`Self::shown`] for a picture of longest side `side`.
    pub fn shown_at(&mut self, generation: u64, quality: Quality, side: u32) {
        self.shown(generation, quality);
        self.shown_side = side;
    }

    /// Whether a render result should replace what is on screen: never an
    /// older generation, and never a preview over the full render of the same generation.
    #[must_use]
    pub fn accepts(&self, generation: u64, quality: Quality) -> bool {
        generation >= self.floor && self.shown.is_none_or(|shown| (generation, quality) > shown)
    }

    /// Records that a result is now on screen.
    pub fn shown(&mut self, generation: u64, quality: Quality) {
        self.shown = Some((generation, quality));
    }

    /// `true` while the image on screen is older than the recipe.
    #[must_use]
    pub fn is_stale(&self, generation: u64) -> bool {
        self.shown.is_none_or(|(g, _)| g != generation)
    }

    /// `true` while a requested render has not arrived yet.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        self.requested.is_some() && self.requested != self.shown
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_zoom_or_resize_asks_the_image_again_at_the_new_size() {
        let mut plan = RenderPlan::default();
        plan.requested_at(1, Quality::Full, 3000);
        assert!(!plan.resize_needed(1, 2000), "the render is still on its way");
        plan.shown_at(1, Quality::Full, 3000);
        assert!(!plan.resize_needed(1, 3030), "within 2 %");
        assert!(plan.resize_needed(1, 2000));
        assert!(!plan.resize_needed(2, 2000), "an edit renders anyway");
        // Back to fit: the preview at the new size replaces the full render.
        plan.requested_at(1, Quality::Preview, 1500);
        assert!(!plan.accepts_at(1, Quality::Preview, 3000), "an older answer");
        assert!(plan.accepts_at(1, Quality::Preview, 1500));
        plan.shown_at(1, Quality::Preview, 1500);
        assert!(!plan.accepts_at(1, Quality::Preview, 1500), "already on screen");
        assert!(!plan.resize_needed(1, 1500));
    }

    #[test]
    fn a_new_document_gets_a_preview_then_one_full_render_when_idle() {
        let mut plan = RenderPlan::default();
        assert_eq!(plan.next(1, false, true, true), Some(Quality::Preview));
        plan.requested(1, Quality::Preview);
        assert_eq!(plan.next(1, false, true, true), None, "wait for the preview");
        plan.shown(1, Quality::Preview);
        assert_eq!(plan.next(1, true, true, true), None, "no full render while dragging");
        assert_eq!(plan.next(1, false, true, true), Some(Quality::Full));
        plan.requested(1, Quality::Full);
        assert_eq!(plan.next(1, false, true, true), None);
        assert!(plan.is_waiting());
        plan.shown(1, Quality::Full);
        assert!(!plan.is_waiting());
        assert_eq!(plan.next(1, false, true, true), None);
    }

    #[test]
    fn a_change_asks_for_a_new_preview_and_small_images_skip_the_full_render() {
        let mut plan = RenderPlan::default();
        plan.requested(1, Quality::Preview);
        plan.shown(1, Quality::Preview);
        assert_eq!(plan.next(1, false, true, false), None);
        assert_eq!(plan.next(2, false, true, false), Some(Quality::Preview));
    }

    #[test]
    fn dragging_renders_drafts_then_the_preview_when_the_drag_ends() {
        let mut plan = RenderPlan::default();
        assert_eq!(plan.next(1, true, true, false), Some(Quality::Draft));
        plan.requested(1, Quality::Draft);
        assert_eq!(plan.next(1, true, true, false), None, "same recipe, still dragging");
        assert_eq!(plan.next(2, true, true, false), Some(Quality::Draft), "the slider moved");
        plan.requested(2, Quality::Draft);
        assert_eq!(plan.next(2, false, true, false), None, "drag over, the draft is not back yet");
        plan.shown(2, Quality::Draft);
        assert_eq!(plan.next(2, false, true, false), Some(Quality::Preview));
        plan.requested(2, Quality::Preview);
        plan.shown(2, Quality::Preview);
        assert_eq!(plan.next(2, false, true, false), None, "fit: the preview is enough");
        assert_eq!(plan.next(2, false, true, true), Some(Quality::Full), "zoomed in");
        assert_eq!(plan.next(3, true, false, false), Some(Quality::Preview), "small photo: no draft");
        assert!(plan.accepts(2, Quality::Full) && !plan.accepts(2, Quality::Draft));
    }

    #[test]
    fn late_and_lower_quality_results_do_not_replace_newer_ones() {
        let mut plan = RenderPlan::default();
        assert!(plan.accepts(1, Quality::Preview));
        plan.shown(2, Quality::Preview);
        assert!(!plan.accepts(1, Quality::Full));
        assert!(plan.accepts(2, Quality::Full));
        plan.shown(2, Quality::Full);
        assert!(!plan.accepts(2, Quality::Preview));
        assert!(plan.accepts(3, Quality::Preview));
        assert!(plan.is_stale(3));
        assert!(!plan.is_stale(2));
    }

    #[test]
    fn after_opening_another_document_late_results_of_the_old_one_are_refused() {
        let mut plan = RenderPlan::default();
        plan.requested(4, Quality::Preview);
        plan.reset(5);
        assert!(!plan.accepts(4, Quality::Full));
        assert!(plan.accepts(5, Quality::Preview));
        assert_eq!(plan.next(5, false, true, true), Some(Quality::Preview));
    }

    fn wait_for(worker: &Worker, mut done: impl FnMut(&Event) -> bool) -> Vec<Event> {
        let start = Instant::now();
        let mut seen = Vec::new();
        while start.elapsed() < Duration::from_secs(20) {
            for e in worker.poll() {
                let stop = done(&e);
                seen.push(e);
                if stop {
                    return seen;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out; events so far: {seen:?}");
    }

    /// The RAW files in `testdata/`, sorted.
    fn sample_raws() -> Vec<PathBuf> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).filter(|p| raw_io::has_raw_extension(p)).collect())
            .unwrap_or_default();
        files.sort();
        files
    }

    /// Waits up to two minutes for the photo opened as `ticket`.
    fn opened_within(worker: &Worker, ticket: u64) -> Duration {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(120) {
            if worker.poll().iter().any(|e| matches!(e, Event::Opened { ticket: t, .. } if *t == ticket)) {
                return start.elapsed();
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("photo {ticket} did not open in two minutes");
    }

    /// Moving through the filmstrip (a photo every 150 ms, as with the
    /// arrow keys) must not make the last photo wait for the ones passed
    /// over: it opens about as fast as a photo opened alone.
    #[test]
    #[ignore = "needs six RAW files in testdata/; a measurement, run in release"]
    fn moving_through_photos_opens_the_last_one_about_as_fast_as_one_alone() {
        let raws = sample_raws();
        assert!(raws.len() >= 6, "put six RAW files in testdata/");
        let worker = cpu_worker();
        worker.open(1, raws[0].clone());
        let alone = opened_within(&worker, 1);
        for (ticket, path) in (10..).zip(&raws[1..6]) {
            worker.open(ticket, path.clone());
            thread::sleep(Duration::from_millis(150));
        }
        let last = opened_within(&worker, 14);
        eprintln!(
            "one photo alone: {:.2} s; the last after moving through five: {:.2} s",
            alone.as_secs_f32(),
            last.as_secs_f32()
        );
        // Every photo opened at once took 2.41 s for 1.41 s alone; one at a
        // time, the ones passed over abandoned, 1.13 s for 1.64 s.
        assert!(last.as_secs_f32() < alone.as_secs_f32() * 1.3, "the last photo waited for the ones passed over");
    }

    fn cpu_worker() -> Worker {
        Worker::start(engine::builtin_tools().unwrap(), EngineChoice::CpuOnly, Arc::new(|| {})).unwrap()
    }

    #[test]
    fn the_worker_reports_the_engine_and_renders_for_the_screen() {
        let worker = cpu_worker();
        wait_for(&worker, |e| matches!(e, Event::EngineReady { backend: Backend::Cpu, .. }));
        let img = ImageF32::from_pixels(3, 2, WorkingSpace::LinearRec2020, vec![[0.18, 0.18, 0.18, 1.0]; 6]).unwrap();
        worker.render(RenderRequest {
            generation: 7,
            quality: Quality::Preview,
            layers: Layers::new(),
            source: Arc::new(img),
            orientation: Orientation::Rotate90,
            max_side: 4096,
            geometry: core_model::Geometry::default(),
            crop: true,
            curve: core_model::BaseCurve::Auto,
        });
        let events = wait_for(&worker, |e| matches!(e, Event::Rendered { .. } | Event::RenderFailed { .. }));
        let Some(Event::Rendered { generation, image, .. }) = events.last() else { panic!("{events:?}") };
        assert_eq!(*generation, 7);
        assert_eq!((image.width, image.height), (2, 3));
    }

    #[test]
    fn opening_a_missing_file_reports_a_message_with_the_file_name() {
        let worker = cpu_worker();
        worker.open(3, PathBuf::from("no/such/DSC_0042.NEF"));
        let events = wait_for(&worker, |e| matches!(e, Event::Opened { .. }));
        let Some(Event::Opened { ticket, result: Err(message), .. }) = events.last() else { panic!("{events:?}") };
        assert_eq!(*ticket, 3);
        assert!(message.contains("DSC_0042.NEF"), "{message}");
    }

    #[test]
    fn a_photo_opened_ahead_comes_back_from_the_prefetch_and_others_are_opened_as_usual() {
        let worker = cpu_worker();
        let ahead = PathBuf::from("no/such/AHEAD_1.NEF");
        worker.prefetch(ahead.clone());
        worker.prefetch(ahead.clone()); // asked twice: opened once
        worker.open(4, ahead);
        let events = wait_for(&worker, |e| matches!(e, Event::Opened { .. }));
        let Some(Event::Opened { ticket: 4, result: Err(message), timing }) = events.last() else {
            panic!("{events:?}")
        };
        assert!(timing.ahead, "served by the prefetch");
        assert!(message.contains("AHEAD_1.NEF"), "{message}");

        worker.prefetch(PathBuf::from("no/such/AHEAD_2.NEF"));
        worker.open(5, PathBuf::from("no/such/OTHER.NEF"));
        let events = wait_for(&worker, |e| matches!(e, Event::Opened { .. }));
        let Some(Event::Opened { ticket: 5, result: Err(message), timing }) = events.last() else {
            panic!("{events:?}")
        };
        assert!(!timing.ahead && message.contains("OTHER.NEF"), "{message}");
    }

    #[test]
    fn exports_report_progress_and_their_files() {
        let worker = cpu_worker();
        let dir = std::env::temp_dir().join(format!("aktis-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = ImageF32::from_pixels(4, 2, WorkingSpace::LinearRec2020, vec![[0.5, 0.2, 0.1, 1.0]; 8]).unwrap();
        let photo = crate::export_job::ExportPhoto {
            path: dir.join("a.NEF"),
            recipe: None,
            open: Some(crate::export_job::OpenPhoto {
                raw: None,
                developed: Some(Arc::new(img)),
                orientation: Orientation::Normal,
                info: raw_io::PhotoInfo::default(),
                camera_curve: None,
            }),
            rating: None,
            keywords: Vec::new(),
        };
        let options = engine::export::ExportOptions::default();
        let handle = worker.export(ExportJob {
            photos: vec![photo],
            options,
            software: "test".into(),
            custom_lenses: Default::default(),
            camera_defaults: Default::default(),
        });
        assert!(!handle.is_cancelled());
        let events = wait_for(&worker, |e| matches!(e, Event::ExportFinished(_)));
        let progress =
            |e: &Event| matches!(e, Event::ExportProgress { done: 0, total: 1, current } if current == "a.NEF");
        assert!(events.iter().any(progress), "{events:?}");
        let Some(Event::ExportFinished(summary)) = events.last() else { panic!("{events:?}") };
        assert_eq!(summary.written, vec![dir.join("a.jpg")]);
        assert!(dir.join("a.jpg").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const _: () = {
        const fn send<T: Send>() {}
        send::<Event>();
        send::<RenderRequest>();
    };
}
