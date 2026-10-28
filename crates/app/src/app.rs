//! The main window.
//!
//! Layout: menu and tool bar on top; on the left the Explorer and the
//! filmstrip side by side, with the History and Metadata tabs below; in the
//! middle the photo above the node graph; on the right the histogram above
//! the settings of the node selected in the graph; a status line at the
//! bottom.
//!
//! Each frame: read the worker's events, handle dropped files and
//! shortcuts, draw, then apply what the user did (collected as values while
//! drawing) and ask for the next render. Nothing slow runs here: decoding,
//! rendering, folder listings and thumbnails happen in [`editor::Worker`],
//! AI keywords in a [`TaggingJob`].
//!
//! Edits are saved in the catalog, per variant, as Capture One does: when
//! another photo or variant is opened, when the window closes, and
//! [`variants::AUTOSAVE_SECONDS`] after a change (see [`editor::variants`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use catalog::{VariantInfo, next_variant_number, variant_name};
use core_model::{DevelopSettings, NodeId, NodeKind, Recipe, ToolId};
use editor::adjust::Pick;
use editor::graph_view::GraphView;
use editor::variants::{self, Reopened, VariantState};
use editor::{
    Document, Event, ExportHandle, ExportJob, ExportPhoto, ExportSummary, Histogram, OpenPhoto, OpenTiming, Quality,
    RenderPlan, RenderRequest, RenderTiming, ViewState, Viewport, Worker, Zoom,
};
use eframe::egui::{
    self, Align, Align2, Button, Color32, CursorIcon, Frame, Id, Layout, Margin, Order, RichText, Sense, TextureOptions,
};
use engine::{Backend, EngineChoice};
use tool_api::ToolRegistry;

use crate::actions::Command;
use crate::ai_mask_job::AiMaskJob;
use crate::dialog::{Choice, dialog, dialog_buttons};
use crate::export_ui::{ExportDialog, ExportRequest, NameExample};
use crate::geometry_ui::{GeoEdit, GeometryUi};
use crate::graph_ui::{GraphAction, GraphUi};
use crate::history_ui::{LeftTab, TabAction, Tabs};
use crate::inspector::{self, Drag, UiEdit};
use crate::library_ui::{Library, LibraryAction, TaggingProgress, VariantBlocks};
use crate::mask_ui::{MaskEdit, MaskUi};
use crate::models_ui::ModelsUi;
use crate::prefs_ui::Preferences;
use crate::presets_ui::{PresetAction, PresetsUi};
use crate::scopes::{ScopeKind, Scopes};
use crate::settings::{STORAGE_KEY, Settings, ThemeChoice};
use crate::tagging_job::{Progress, TaggingJob};
use crate::theme::{
    self, FILMSTRIP_WIDTH, FONT, GRAPH_HEIGHT, HISTOGRAM_HEIGHT, INSPECTOR_WIDTH, LEFT_WIDTH, SPACE,
    SPINNER_DELAY_SECONDS, TABS_HEIGHT, TOAST_ERROR_SECONDS, TOAST_SECONDS, Tokens,
};
use crate::toolbar::{self, BarAction, BarState, TOOL_GROUPS, ViewerTool};
use crate::{APP_NAME, VERSION};

/// Zoom factor per point of mouse-wheel scrolling (one notch ≈ 50 points ≈ ×1.25).
const WHEEL_ZOOM_PER_POINT: f32 = 0.004_5;

/// Largest texture side requested, even if the GPU allows more.
const MAX_TEXTURE_SIDE: u32 = 16_384;

/// A background job the user is waiting for.
#[derive(Debug, Clone)]
struct Job {
    ticket: u64,
    name: String,
    started: Instant,
}

/// A running export, for the status bar.
#[derive(Debug)]
struct ExportRun {
    handle: ExportHandle,
    done: usize,
    total: usize,
    current: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastKind {
    Info,
    Error,
}

#[derive(Debug, Clone)]
enum ToastAction {
    ShowInFolder(PathBuf),
}

impl ToastAction {
    fn label(&self) -> &'static str {
        match self {
            Self::ShowInFolder(_) => "Show in folder",
        }
    }
}

#[derive(Debug, Clone)]
struct Toast {
    text: String,
    kind: ToastKind,
    action: Option<ToastAction>,
    deadline: Instant,
}

/// What a test (or a screenshot) needs to know about the window's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// An image is open.
    pub has_document: bool,
    /// The image on screen matches the recipe and no render is pending.
    pub image_current: bool,
    /// Opening or exporting is running.
    pub busy: bool,
}

/// The editor window.
pub struct EditorApp {
    registry: ToolRegistry,
    worker: Option<Worker>,
    settings: Settings,
    doc: Option<Document>,
    next_ticket: u64,
    opening: Option<Job>,
    /// File being opened, to drop it from the recent list if it has gone.
    opening_path: Option<PathBuf>,
    exporting: Option<ExportRun>,
    /// The export window, while it is open.
    export_dialog: Option<ExportDialog>,
    /// A development in flight (lens corrections changed): ticket and settings.
    developing: Option<(u64, DevelopSettings)>,
    texture: Option<egui::TextureHandle>,
    /// The exposure warning over `texture`, made from `shown` when first
    /// needed after each render.
    warning_texture: Option<egui::TextureHandle>,
    plan: RenderPlan,
    view: ViewState,
    viewport: Option<Viewport>,
    drag: Option<Drag>,
    backend: Option<Backend>,
    gpu_problem: Option<String>,
    toast: Option<Toast>,
    /// The last "node skipped" message shown, so it is not repeated at every render.
    node_problem: Option<String>,
    /// Downloads of the AI models: the dialog and the "AI models" window.
    models: ModelsUi,
    title: String,
    /// Explorer, filmstrip and catalog.
    library: Library,
    /// History and Metadata.
    tabs: Tabs,
    /// The node whose settings are shown on the right.
    selected_node: Option<NodeId>,
    /// The last nodes selected, newest first (the selected one included):
    /// their settings are shown on the right, up to [`RECENT_NODES`].
    recent_nodes: Vec<NodeId>,
    /// Node positions, panning and menus of the node graph.
    graph: GraphUi,
    /// The tool of the bar in use in the viewer.
    tool: Option<ViewerTool>,
    /// A picker of the settings panel waiting for a click in the viewer.
    pick: Option<Pick>,
    /// The viewer tool at the last frame, to notice a change.
    prev_tool: Option<ViewerTool>,
    /// Crop box, aspect and Level line of the geometry tools.
    geometry_ui: GeometryUi,
    /// The Masks and AI Select tools.
    mask_ui: MaskUi,
    /// AI masks being computed.
    ai_job: Option<AiMaskJob>,
    /// Retouch (T4): brush size and the selected spot of the Heal and Clone tools.
    retouch_ui: crate::retouch_ui::RetouchUi,
    /// The Preferences window, while open.
    prefs: Preferences,
    /// The slider drag of a geometry tool, one undo step.
    geometry_gesture: Option<commands::GestureId>,
    /// The colour entering the selected node under the pointer in the viewer.
    probe: Option<[f32; 3]>,
    /// Levels of the image on screen.
    histogram: Option<Box<Histogram>>,
    /// The waveform or parade texture.
    scopes: Scopes,
    /// The 8-bit image on screen, for the eyedropper and the loupe.
    shown: Option<engine::Rgb8Image>,
    /// The loupe's pixels while the middle button is held.
    loupe: Option<egui::TextureHandle>,
    /// "AI-tag this folder", while it runs.
    tagging: Option<TaggingJob>,
    /// Time, kind and size of the last render shown.
    last_render: Option<(RenderTiming, Quality, [u32; 2])>,
    /// How long the open photo took to open.
    last_open: Option<OpenTiming>,
    /// Direction of the last move in the filmstrip (+1 next, −1 previous)
    /// once a photo has opened and the next one should be opened ahead,
    /// after its first render so the two do not compete.
    prefetch_step: Option<isize>,
    /// What the catalog has for the open variant; `None` without a photo.
    variant: Option<VariantState>,
    /// The open photo's stored variants, for the menus and the status bar.
    variants: Vec<VariantInfo>,
    /// When the open variant first differed from what is saved.
    dirty_since: Option<Instant>,
    /// The variant whose deletion waits for confirmation.
    confirm_delete: Option<VariantInfo>,
    /// Presets and style brushes, and the Save Preset window.
    presets: PresetsUi,
    /// Closing was refused once because the edit could not be saved.
    close_refused: bool,
}

impl std::fmt::Debug for EditorApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorApp").field("status", &self.status()).finish_non_exhaustive()
    }
}

impl EditorApp {
    /// Builds the window state, starts the background worker and, if
    /// given, starts opening `open`.
    #[must_use]
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        // Ctrl+Plus/Minus/0 zoom the photo, not the interface.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);

        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, STORAGE_KEY)).unwrap_or_default();
        ctx.set_theme(theme_preference(settings.theme));

        let mut toast = None;
        let registry = engine::builtin_tools().unwrap_or_else(|e| {
            toast = Some(error_toast(format!("Some tools could not be loaded: {e}")));
            ToolRegistry::new()
        });
        let repaint = ctx.clone();
        let worker =
            match Worker::start(registry.clone(), EngineChoice::Auto, Arc::new(move || repaint.request_repaint())) {
                Ok(w) => Some(w),
                Err(e) => {
                    toast = Some(error_toast(format!("The background worker could not start: {e}")));
                    None
                }
            };

        let mut app = Self {
            registry,
            worker,
            settings,
            doc: None,
            next_ticket: 0,
            opening: None,
            opening_path: None,
            exporting: None,
            export_dialog: None,
            developing: None,
            texture: None,
            warning_texture: None,
            plan: RenderPlan::default(),
            view: ViewState::default(),
            viewport: None,
            drag: None,
            backend: None,
            gpu_problem: None,
            toast,
            node_problem: None,
            models: ModelsUi::new(),
            title: String::new(),
            library: Library::new(),
            tabs: Tabs::default(),
            selected_node: None,
            recent_nodes: Vec::new(),
            graph: GraphUi::default(),
            tool: None,
            pick: None,
            prev_tool: None,
            geometry_ui: GeometryUi::default(),
            retouch_ui: crate::retouch_ui::RetouchUi::default(), // Retouch (T4)
            geometry_gesture: None,
            prefs: Preferences::default(),
            mask_ui: MaskUi::default(),
            ai_job: None,
            probe: None,
            histogram: None,
            scopes: Scopes::default(),
            shown: None,
            loupe: None,
            tagging: None,
            last_render: None,
            last_open: None,
            prefetch_step: None,
            variant: None,
            variants: Vec::new(),
            dirty_since: None,
            confirm_delete: None,
            presets: PresetsUi::default(),
            close_refused: false,
        };
        app.reload_presets();
        if let Some(problem) = app.library.catalog_problem.take() {
            app.toast = Some(error_toast(problem));
        }
        if let Some(dir) = app.settings.library_dir.clone().filter(|d| d.is_dir()) {
            app.library.show_folder(dir, app.worker.as_ref());
        }
        if let Some(path) = open {
            app.open(path);
        }
        app
    }

    /// The window's state, for tests.
    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            has_document: self.doc.is_some(),
            image_current: self.doc.as_ref().is_some_and(|d| !self.plan.is_stale(d.generation()))
                && !self.plan.is_waiting(),
            busy: self.opening.is_some() || self.exporting.is_some(),
        }
    }

    fn open(&mut self, path: PathBuf) {
        let Some(worker) = &self.worker else { return };
        self.next_ticket += 1;
        let name = file_name(&path);
        self.opening_path = Some(path.clone());
        worker.open(self.next_ticket, path);
        self.opening = Some(Job { ticket: self.next_ticket, name, started: Instant::now() });
    }

    fn pick_and_open(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open a RAW file")
            .add_filter("RAW files", raw_io::RAW_EXTENSIONS)
            .add_filter("All files", &["*"]);
        if let Some(dir) = &self.settings.open_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open(path);
        }
    }

    /// The photos File ▸ Export and the Export button act on: the photos
    /// selected in the filmstrip when there are several, else the open
    /// photo (or the one selected photo).
    fn export_targets(&self) -> Vec<PathBuf> {
        let selection = self.library.selection();
        if selection.len() > 1 {
            return selection;
        }
        match &self.doc {
            Some(doc) => vec![doc.path().to_owned()],
            None => selection,
        }
    }

    /// Opens the export window for `photos`.
    fn open_export_dialog(&mut self, photos: Vec<PathBuf>) {
        let Some(first) = photos.first() else { return };
        if self.exporting.is_some() {
            self.toast = Some(info_toast("An export is running; wait for it to finish or stop it".to_owned()));
            return;
        }
        let example = self.name_example(first);
        self.export_dialog =
            Some(ExportDialog::new(photos, &self.settings.export, self.settings.export_dir.clone(), example));
    }

    /// The live example of the export window: the photo's name, and its date
    /// and camera when it is the open photo.
    fn name_example(&self, path: &Path) -> NameExample {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match self.doc.as_ref().filter(|d| d.path() == path) {
            Some(doc) => NameExample {
                stem,
                captured: doc.info().captured.clone(),
                camera: Some(doc.camera().trim().to_owned()).filter(|c| !c.is_empty()),
            },
            None => NameExample { stem, ..NameExample::default() },
        }
    }

    /// The edits of `path` for an export: the open photo's recipe, else
    /// the variant the catalog shows for it; `None` (the default recipe)
    /// for a photo never edited.
    ///
    /// The one place exports learn a photo's edits.
    fn recipe_for(&self, path: &Path) -> Option<Recipe> {
        if let Some(doc) = self.doc.as_ref().filter(|d| d.path() == path) {
            return Some(doc.recipe().clone());
        }
        let (_, stored) = self.library.shown_variant(path).ok()?;
        variants::stored_recipe(&stored?)
    }

    /// What the export job needs about `path`: its recipe, what is in memory
    /// when it is the open photo, and its catalog rating and keywords.
    fn export_photo(&self, path: &Path) -> ExportPhoto {
        let open = self.doc.as_ref().filter(|d| d.path() == path).map(|d| OpenPhoto {
            raw: d.raw().cloned(),
            // While lens corrections are being applied the image on screen is
            // out of date: the export develops the RAW again instead.
            developed: (!d.needs_develop()).then(|| Arc::clone(d.developed())),
            orientation: d.orientation(),
            info: d.info().clone(),
        });
        let meta = self.library.catalog_meta(path);
        ExportPhoto {
            path: path.to_owned(),
            recipe: self.recipe_for(path),
            open,
            rating: Some(meta.rating).filter(|r| *r > 0),
            keywords: meta.tags.iter().map(|t| t.name.clone()).collect(),
        }
    }

    /// Exports `photos` with the remembered export settings.
    fn start_export(&mut self, photos: Vec<PathBuf>, ctx: &egui::Context) {
        if photos.is_empty() {
            return;
        }
        if self.exporting.is_some() {
            self.toast = Some(info_toast("An export is running; wait for it to finish or stop it".to_owned()));
            return;
        }
        let Some(worker) = &self.worker else { return };
        let job = ExportJob {
            photos: photos.iter().map(|p| self.export_photo(p)).collect(),
            options: self.settings.export_options(),
            software: format!("{APP_NAME} {VERSION}"),
        };
        let total = job.photos.len();
        let current = photos.first().map(|p| file_name(p.as_path())).unwrap_or_default();
        let handle = worker.export(job);
        self.exporting = Some(ExportRun { handle, done: 0, total, current });
        ctx.request_repaint();
    }

    fn run(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::Open => self.pick_and_open(),
            Command::Export => self.open_export_dialog(self.export_targets()),
            Command::ExportAgain => self.start_export(self.export_targets(), ctx),
            Command::Quit => {
                if self.save_before_closing() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Command::NewVariant => self.add_variant(false),
            Command::CloneVariant => self.add_variant(true),
            Command::DeleteVariant => self.ask_delete_variant(),
            Command::Undo => {
                if let Some(doc) = &mut self.doc {
                    doc.undo();
                }
            }
            Command::Redo => {
                if let Some(doc) = &mut self.doc {
                    doc.redo();
                }
            }
            Command::ZoomFit => self.view.fit(),
            Command::Zoom100 | Command::ZoomIn | Command::ZoomOut => {
                let (Some(doc), Some(vp)) = (&self.doc, &self.viewport) else { return };
                let (w, h) = doc.shown_size(self.tool != Some(ViewerTool::Crop));
                let image = [w as f32, h as f32];
                match command {
                    Command::Zoom100 => self.view.actual_pixels(None, image, vp),
                    Command::ZoomIn => self.view.zoom_by(editor::view::ZOOM_STEP, None, image, vp),
                    _ => self.view.zoom_by(1.0 / editor::view::ZOOM_STEP, None, image, vp),
                }
            }
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context) {
        let events = self.worker.as_ref().map(Worker::poll).unwrap_or_default();
        for event in events {
            match event {
                Event::EngineReady { backend, gpu_problem } => {
                    self.backend = Some(backend);
                    self.gpu_problem = gpu_problem;
                }
                Event::Opened { ticket, result, timing } => {
                    if self.opening.as_ref().is_none_or(|j| j.ticket != ticket) {
                        continue; // a newer open replaced this one
                    }
                    self.opening = None;
                    match result {
                        Ok(opened) => {
                            // The photo being left is saved first; if it cannot be, it stays open.
                            if !self.save_edit(true) {
                                self.opening_path = None;
                                continue;
                            }
                            let path = opened.path.clone();
                            // Moving backwards through the filmstrip opens the previous photo ahead.
                            let left = self.doc.as_ref().and_then(|d| self.library.strip.index_of(d.path()));
                            let step = match (left, self.library.strip.index_of(&path)) {
                                (Some(a), Some(b)) if b < a => -1,
                                _ => 1,
                            };
                            self.last_open = Some(timing);
                            self.prefetch_step = Some(step);
                            let first = self.doc.as_ref().map_or(1, |d| d.generation() + 1);
                            let reopened = match self.library.shown_variant(&path) {
                                Ok((number, stored)) => {
                                    // An edit changed from the filmstrip (a preset) has no preview yet.
                                    let missing = stored.is_some() && !self.library.has_edited_preview(&path);
                                    let mut r = variants::reopen(*opened, first, &self.registry, number, stored);
                                    if missing {
                                        r.state.preview_missing();
                                    }
                                    r
                                }
                                Err(e) => {
                                    // Never save over an edit that could not be read.
                                    let mut r =
                                        variants::reopen(*opened, first, &self.registry, catalog::FIRST_VARIANT, None);
                                    let why =
                                        format!("Read-only: the saved edit of this photo could not be read ({e}).");
                                    r.doc.set_read_only(true);
                                    r.state.read_only = Some(why.clone());
                                    r.notice = Some(why);
                                    r
                                }
                            };
                            self.settings.opened(&path);
                            self.view.fit();
                            // A development in flight is for the photo being left.
                            self.developing = None;
                            self.install(reopened, false, false);
                            // The filmstrip shows the photo's folder.
                            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
                                && self.library.folder() != Some(dir)
                            {
                                self.library.show_folder(dir.to_owned(), self.worker.as_ref());
                            }
                            self.library.reveal_photo(&path);
                        }
                        Err(message) => {
                            if let Some(path) = &self.opening_path
                                && !path.exists()
                            {
                                self.settings.forget(path);
                            }
                            self.toast = Some(error_toast(message));
                        }
                    }
                    self.opening_path = None;
                }
                Event::Rendered {
                    generation,
                    quality,
                    image,
                    backend,
                    gpu_failure,
                    node_problems,
                    histogram,
                    timing,
                } => {
                    if !self.plan.accepts(generation, quality) {
                        continue;
                    }
                    self.last_render = Some((timing, quality, [image.width, image.height]));
                    self.prefetch_neighbour();
                    self.histogram = Some(histogram);
                    self.scopes.changed();
                    let size = [image.width as usize, image.height as usize];
                    let color = egui::ColorImage::from_rgb(size, &image.pixels);
                    let options = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
                    match &mut self.texture {
                        Some(t) => t.set(color, options),
                        None => self.texture = Some(ctx.load_texture("photo", color, options)),
                    }
                    self.shown = Some(image);
                    self.warning_texture = None;
                    self.plan.shown(generation, quality);
                    if let Some(why) = gpu_failure {
                        self.toast = Some(error_toast(format!("The GPU failed, the CPU took over: {why}")));
                    }
                    // A tool short of its models offers the download instead of an error.
                    let offering = !node_problems.is_empty() && self.offer_missing_models();
                    // Told once per message, not at every render.
                    if let Some(problem) = node_problems.into_iter().next()
                        && !offering
                        && self.node_problem.as_ref() != Some(&problem)
                    {
                        self.toast = Some(error_toast(problem.clone()));
                        self.node_problem = Some(problem);
                    }
                    self.backend = Some(backend);
                }
                Event::Developed { ticket, result } => {
                    if self.developing.as_ref().is_none_or(|(t, _)| *t != ticket) {
                        continue; // a newer development replaced this one
                    }
                    self.developing = None;
                    match (result, &mut self.doc) {
                        (Ok(image), Some(doc)) => {
                            // Refused (and asked again) when the settings changed meanwhile.
                            doc.replace_developed(*image);
                        }
                        (Err(message), _) => self.toast = Some(error_toast(message)),
                        (Ok(_), None) => {}
                    }
                }
                Event::RenderFailed { message, .. } => {
                    self.toast = Some(error_toast(format!("The preview could not be made: {message}")));
                }
                Event::ExportProgress { done, total, current } => {
                    if let Some(run) = &mut self.exporting {
                        run.done = done;
                        run.total = total;
                        run.current = current;
                    }
                }
                Event::ExportFinished(summary) => {
                    self.exporting = None;
                    self.toast = Some(export_toast(&summary));
                }
                Event::FolderListed { dir, result } => {
                    if let Some(message) = self.library.on_listed(dir, result) {
                        self.toast = Some(error_toast(message));
                    }
                }
                Event::Thumbnail { path, result } => self.library.on_thumbnail(ctx, path, result),
                Event::Preview { path, result } => self.library.on_preview(ctx, path, result),
            }
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_owned()).collect());
        if let Some(first) = dropped.into_iter().next() {
            self.open(first);
        }
    }

    fn consume_shortcuts(ctx: &egui::Context) -> Vec<Command> {
        // Keys that also edit text stay with a text field that has the keyboard.
        let typing = ctx.egui_wants_keyboard_input();
        let table: Vec<_> =
            Command::shortcut_table().into_iter().filter(|(_, c)| !(typing && c.yields_to_text())).collect();
        ctx.input_mut(|i| table.iter().filter(|(s, _)| i.consume_shortcut(s)).map(|(_, c)| *c).collect())
    }

    fn apply_edits(&mut self, edits: Vec<UiEdit>) {
        let Some(doc) = &mut self.doc else { return };
        for edit in edits {
            let result = match edit {
                UiEdit::BeginDrag { node, param } => {
                    self.drag = Some(Drag { node, param, gesture: doc.begin_gesture() });
                    Ok(())
                }
                UiEdit::EndDrag => {
                    self.drag = None;
                    Ok(())
                }
                UiEdit::Set { node, param, value, in_drag } => {
                    let gesture =
                        self.drag.as_ref().filter(|d| in_drag && d.node == node && d.param == param).map(|d| d.gesture);
                    doc.set_param(&self.registry, node, param, value, gesture)
                }
                UiEdit::Reset { node, param } => doc.reset_param(&self.registry, node, param),
                UiEdit::Enabled { node, on } => {
                    let result = doc.set_enabled(&self.registry, node, on);
                    // Turning on a tool that needs AI models offers to download them.
                    if on
                        && result.is_ok()
                        && let Some(NodeKind::Tool(tool)) = doc.recipe().graph.node(node).map(|n| &n.kind)
                        && let Some(feature) = ai_models::feature_for_tool(tool.as_str())
                    {
                        self.models.require(feature);
                    }
                    result
                }
                UiEdit::Lens(lens) => doc.set_lens_correction(&self.registry, lens),
                UiEdit::Curve(curve) => doc.set_base_curve(&self.registry, curve),
                UiEdit::SetMany { node, key, values, in_drag } => {
                    let gesture =
                        self.drag.as_ref().filter(|d| in_drag && d.node == node && d.param == key).map(|d| d.gesture);
                    doc.set_params(&self.registry, node, values, gesture)
                }
                UiEdit::WbMode { node, mode } => {
                    match editor::adjust::wb_mode_values(doc, &self.registry, node, mode) {
                        Some(values) => doc.set_params(&self.registry, node, values, None),
                        None => Ok(()),
                    }
                }
                UiEdit::Pick(pick) => {
                    self.pick = pick;
                    Ok(())
                }
            };
            if let Err(e) = result {
                self.toast = Some(error_toast(format!("That change was refused: {e}")));
            }
        }
    }

    /// Applies what the Explorer and the filmstrip asked for.
    fn apply_library(&mut self, actions: Vec<LibraryAction>, ctx: &egui::Context) {
        for action in actions {
            match action {
                LibraryAction::Open(path) => {
                    let already = self.doc.as_ref().is_some_and(|d| d.path() == path);
                    let pending = self.opening_path.as_ref() == Some(&path);
                    if !already && !pending {
                        self.open(path);
                    }
                }
                LibraryAction::Export(photos) => self.open_export_dialog(photos),
                LibraryAction::ExportAgain(photos) => self.start_export(photos, ctx),
                LibraryAction::Renamed { from, to } => {
                    if let Some(doc) = &mut self.doc
                        && doc.path() == from
                    {
                        doc.file_renamed(to.clone());
                        // The catalog moved the edits with the file.
                        self.variants = self.library.variants(&to);
                    }
                    follow_rename(&mut self.settings.recent, &from, &to);
                }
                LibraryAction::Command(command) => self.run(command, ctx),
                LibraryAction::ApplyPreset(photos, index) => self.apply_preset(index, &photos),
                LibraryAction::SavePreset(photo) => match self.recipe_for(&photo) {
                    Some(recipe) => {
                        let settings = self.brush_settings();
                        self.presets.open_save(&recipe, &self.registry, false, None, settings);
                    }
                    None => {
                        self.toast = Some(info_toast(format!("{} has no adjustments to save yet", file_name(&photo))));
                    }
                },
                LibraryAction::TagFolder => self.start_tagging(ctx),
                LibraryAction::StopTagging => {
                    if let Some(job) = &self.tagging {
                        job.cancel();
                    }
                }
                LibraryAction::Error(message) => self.toast = Some(error_toast(message)),
            }
        }
    }

    /// Applies what the History and Metadata tabs asked for.
    fn apply_tabs(&mut self, actions: Vec<TabAction>) {
        let Some(path) = self.doc.as_ref().map(|d| d.path().to_owned()) else { return };
        for action in actions {
            let result = match action {
                TabAction::Rewrite(order) => {
                    let Some(doc) = &mut self.doc else { continue };
                    self.drag = None;
                    match doc.rewrite_history(&self.registry, &order) {
                        Ok(report) if !report.skipped.is_empty() => {
                            let n = report.skipped.len();
                            let steps = if n == 1 {
                                "1 later step no longer applies"
                            } else {
                                "Some later steps no longer apply"
                            };
                            self.toast = Some(info_toast(format!("{steps}; shown greyed in the History")));
                            Ok(())
                        }
                        Ok(_) => Ok(()),
                        Err(e) => Err(format!("The history could not be changed: {e}")),
                    }
                }
                TabAction::Rate(r) => self.library.set_rating(std::slice::from_ref(&path), r),
                TabAction::Label(l) => self.library.set_label(std::slice::from_ref(&path), l),
                TabAction::AddTag(name) => self.library.add_user_tag(&path, &name),
                TabAction::RemoveTag(name) => self.library.remove_tag(&path, &name),
                TabAction::Rename => {
                    self.library.begin_rename(path.clone());
                    Ok(())
                }
            };
            if let Err(message) = result {
                self.toast = Some(error_toast(message));
            }
        }
    }

    /// Starts "AI-tag this folder", after the model download if needed.
    fn start_tagging(&mut self, ctx: &egui::Context) {
        if self.tagging.is_some() || !self.models.require(ai_models::Feature::Tagging) {
            return; // running, or the download dialog is up
        }
        let photos = self.library.strip.tagging_targets();
        if photos.is_empty() {
            return;
        }
        let repaint = ctx.clone();
        match TaggingJob::start(photos, move || repaint.request_repaint()) {
            Ok(job) => self.tagging = Some(job),
            Err(e) => self.toast = Some(error_toast(format!("AI keywords could not start: {e}"))),
        }
    }

    /// Stores the keywords the tagging job found; tells when it ends.
    fn poll_tagging(&mut self) {
        let Some(job) = &mut self.tagging else { return };
        let mut finished = false;
        let mut failure = None;
        for progress in job.poll() {
            match progress {
                Progress::Tagged { path, tags } => {
                    if let Err(e) = self.library.set_ai_tags(&path, &tags) {
                        failure = Some(format!("Keywords could not be saved: {e}"));
                    }
                }
                Progress::Skipped { .. } => {}
                Progress::Failed(message) => failure = Some(message),
                Progress::Finished => finished = true,
            }
        }
        let (tagged, skipped, first_skip) = (job.tagged, job.done - job.tagged, job.first_skip.clone());
        if finished {
            self.tagging = None;
            if failure.is_none() {
                let photos = if tagged == 1 { "1 photo".to_owned() } else { format!("{tagged} photos") };
                self.toast = Some(match first_skip {
                    Some(first) if skipped > 0 => {
                        error_toast(format!("AI keywords added to {photos}; {skipped} could not be read ({first})"))
                    }
                    _ => info_toast(format!("AI keywords added to {photos}")),
                });
            }
        }
        if let Some(message) = failure {
            self.toast = Some(error_toast(message));
        }
    }

    /// Forgets the selected node when it is gone (history rewritten, undo).
    fn check_selection(&mut self) {
        let exists = |id: NodeId| self.doc.as_ref().is_some_and(|d| d.recipe().graph.node(id).is_some());
        if self.selected_node.is_some_and(|id| !exists(id)) {
            self.selected_node = None;
        }
        let kept: Vec<NodeId> = self.recent_nodes.iter().copied().filter(|id| exists(*id)).collect();
        self.recent_nodes = remember_recent(kept, self.selected_node);
    }

    /// Offers the download for the first enabled node whose tool lacks its
    /// AI models; `true` when the dialog is up.
    fn offer_missing_models(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        let features: Vec<ai_models::Feature> = doc
            .recipe()
            .graph
            .nodes()
            .filter(|n| n.enabled)
            .filter_map(|n| match &n.kind {
                NodeKind::Tool(tool) => ai_models::feature_for_tool(tool.as_str()),
                _ => None,
            })
            .collect();
        for feature in features {
            if !self.models.require(feature) {
                return self.models.is_prompting();
            }
        }
        false
    }

    /// Develops the image again when its development settings changed (lens
    /// corrections edited, or undone), one job at a time.
    fn request_develop(&mut self) {
        let (Some(doc), Some(worker)) = (&self.doc, &self.worker) else { return };
        if !doc.needs_develop() {
            return;
        }
        let wanted = &doc.recipe().develop;
        if self.developing.as_ref().is_some_and(|(_, s)| s == wanted) {
            return;
        }
        let Some(raw) = doc.raw() else { return };
        self.next_ticket += 1;
        worker.develop(self.next_ticket, Arc::clone(raw), wanted.clone());
        self.developing = Some((self.next_ticket, wanted.clone()));
    }

    fn request_render(&mut self, ctx: &egui::Context) {
        let (Some(doc), Some(worker)) = (&self.doc, &self.worker) else { return };
        let busy = self.drag.is_some();
        // The full-size render only when the zoom shows more than the preview has.
        let (w, h) = doc.shown_size(self.tool != Some(ViewerTool::Crop));
        let full_adds_detail = doc.preview_is_reduced()
            && self.viewport.as_ref().is_some_and(|vp| {
                editor::needs_full_resolution(self.view.scale([w as f32, h as f32], vp), doc.preview_scale())
            });
        let Some(quality) = self.plan.next(doc.generation(), busy, doc.draft_is_reduced(), full_adds_detail) else {
            return;
        };
        let source = match quality {
            Quality::Draft => doc.draft(),
            Quality::Preview => doc.preview(),
            Quality::Full => doc.developed(),
        };
        let max_side = u32::try_from(ctx.input(|i| i.max_texture_side)).unwrap_or(u32::MAX).min(MAX_TEXTURE_SIDE);
        worker.render(RenderRequest {
            generation: doc.generation(),
            quality,
            graph: doc.recipe().graph.clone(),
            source: Arc::clone(source),
            orientation: doc.orientation(),
            max_side,
            geometry: doc.recipe().geometry,
            // While the crop is edited the whole frame shows around it.
            crop: self.tool != Some(ViewerTool::Crop),
            curve: doc.recipe().curve,
        });
        self.plan.requested(doc.generation(), quality);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title =
            self.doc.as_ref().map_or_else(|| APP_NAME.to_owned(), |d| format!("{} \u{2014} {APP_NAME}", d.file_name()));
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    // ----- edits in the catalog -----

    /// Shows a document: another photo (`same_photo` false), or another
    /// variant of the open one. `keep_shown`: the image on screen shows the
    /// new document too (a clone), so it can make its preview.
    fn install(&mut self, reopened: Reopened, same_photo: bool, keep_shown: bool) {
        let Reopened { doc, view, state, notice } = reopened;
        self.plan.reset(doc.generation());
        self.drag = None;
        self.pick = None;
        if !same_photo {
            self.texture = None;
            self.histogram = None;
        }
        if !keep_shown {
            self.shown = None;
        }
        // Show the Source's settings, as for a new photo.
        self.selected_node = Some(doc.recipe().graph.source());
        self.recent_nodes.clear();
        self.graph.restore(view);
        // A style brush armed on the photo being left is not carried over.
        self.mask_ui.disarm();
        self.variants = self.library.variants(doc.path());
        self.doc = Some(doc);
        self.variant = Some(state);
        self.dirty_since = None;
        self.confirm_delete = None;
        if let Some(text) = notice {
            self.toast = Some(info_toast(text));
        }
    }

    /// Saves the open variant before the window closes. When the catalog
    /// refuses, the window stays open the first time (the error is shown)
    /// and closes without saving if the user asks again.
    fn save_before_closing(&mut self) -> bool {
        if self.save_edit(true) || self.close_refused {
            return true;
        }
        self.close_refused = true;
        self.toast = Some(error_toast(
            "The edit could not be saved, so the window stays open. Close it again to quit without saving.".to_owned(),
        ));
        false
    }

    /// Writes the open variant to the catalog when it changed, with a
    /// preview of the image on screen: the current render, or when
    /// `closing` (the variant is being left) the last one shown. Returns
    /// `false` when the catalog refused (told to the user).
    fn save_edit(&mut self, closing: bool) -> bool {
        let (Some(doc), Some(state)) = (&self.doc, &mut self.variant) else { return true };
        let view = self.graph.view();
        if !state.is_dirty(doc, view) && !state.preview_due() {
            self.dirty_since = None;
            return true;
        }
        let current = !self.plan.is_stale(doc.generation()) && !self.plan.is_waiting() && !doc.needs_develop();
        let shown = self.shown.as_ref().filter(|_| closing || current);
        let save = state.to_save(doc, view, shown);
        let wrote_edit = save.data.is_some();
        if !save.is_empty()
            && let Err(e) = self.library.save_variant(doc.path(), &save)
        {
            self.toast = Some(error_toast(format!("The edit could not be saved: {e}")));
            // Try again at the next autosave.
            self.dirty_since = Some(Instant::now());
            return false;
        }
        state.saved(doc, view, save);
        self.dirty_since = None;
        if wrote_edit {
            self.variants = self.library.variants(doc.path());
        }
        true
    }

    /// Saves the open variant [`variants::AUTOSAVE_SECONDS`] after it
    /// changed, and its preview once the screen shows the saved edit.
    fn autosave(&mut self, ctx: &egui::Context) {
        let (Some(doc), Some(state)) = (&self.doc, &self.variant) else {
            self.dirty_since = None;
            return;
        };
        if state.is_dirty(doc, self.graph.view()) {
            let since = *self.dirty_since.get_or_insert_with(Instant::now);
            let wait = Duration::from_secs(variants::AUTOSAVE_SECONDS);
            match wait.checked_sub(since.elapsed()) {
                Some(left) if !left.is_zero() => ctx.request_repaint_after(left),
                _ => {
                    self.save_edit(false);
                }
            }
        } else {
            self.dirty_since = None;
            let current = !self.plan.is_stale(doc.generation()) && !self.plan.is_waiting() && !doc.needs_develop();
            if state.preview_due() && current && self.shown.is_some() {
                self.save_edit(false);
            }
        }
    }

    /// The open photo's variants: those stored and the open one.
    fn variant_list(&self) -> Vec<VariantInfo> {
        variants::variant_list(&self.variants, self.variant.as_ref().map(|s| &s.info))
    }

    /// Why each variant command is unavailable now.
    fn variant_blocks(&self) -> VariantBlocks {
        let Some(state) = &self.variant else {
            let why = Some("Open a photo first");
            return VariantBlocks { new: why, clone: why, delete: why };
        };
        let read_only = state.read_only.is_some();
        VariantBlocks {
            new: None,
            clone: read_only.then_some("A read-only variant cannot be cloned"),
            delete: if read_only {
                Some("A read-only variant cannot be deleted")
            } else if self.variant_list().len() < 2 {
                Some("A photo keeps at least one variant")
            } else {
                None
            },
        }
    }

    /// New variant (the photo as it came from the camera) or Clone variant
    /// (a copy of the open one, with its history and node positions).
    fn add_variant(&mut self, clone: bool) {
        let blocks = self.variant_blocks();
        if let Some(why) = if clone { blocks.clone } else { blocks.new } {
            self.toast = Some(info_toast(why.to_owned()));
            return;
        }
        // The open variant is stored first, so it stays in the list.
        if let Some(state) = &mut self.variant {
            state.ensure_stored();
        }
        if !self.save_edit(true) {
            return;
        }
        // Numbered after what the catalog really holds, so no stored variant is replaced.
        let Some(path) = self.doc.as_ref().map(|d| d.path().to_owned()) else { return };
        let stored = match self.library.try_variants(&path) {
            Ok(list) => list,
            Err(e) => {
                self.toast = Some(error_toast(format!("No variant can be added: the catalog cannot be read ({e})")));
                return;
            }
        };
        let open = self.variant.as_ref().map(|s| s.info.clone());
        let number = next_variant_number(&variants::variant_list(&stored, open.as_ref()));
        let Some(doc) = &self.doc else { return };
        let path = doc.path().to_owned();
        let first = doc.generation() + 1;
        let (doc, view) = if clone {
            let history = Some(doc.saved_history());
            let copy = Document::restored(doc.opened().clone(), first, &self.registry, doc.recipe().clone(), history);
            (copy.0, self.graph.view().clone())
        } else {
            (Document::new(doc.opened().clone(), first), GraphView::default())
        };
        let state = VariantState::unsaved(number, variant_name(number));
        self.install(Reopened { doc, view, state, notice: None }, true, clone);
        // Written now; its preview follows once the screen shows it.
        if !self.save_edit(true) {
            return;
        }
        if let Err(e) = self.library.set_current_variant(&path, number) {
            self.toast = Some(error_toast(format!("The variant could not be chosen: {e}")));
            return;
        }
        let verb = if clone { "Cloned as" } else { "Created" };
        self.toast = Some(info_toast(format!("{verb} {}", variant_name(number))));
    }

    /// Opens variant `number` of the open photo, after saving the one open.
    fn switch_variant(&mut self, number: u32) {
        let Some(doc) = &self.doc else { return };
        if self.variant.as_ref().is_some_and(|s| s.info.number == number) {
            return;
        }
        let (path, opened, first) = (doc.path().to_owned(), doc.opened().clone(), doc.generation() + 1);
        if !self.save_edit(true) {
            return;
        }
        let stored = match self.library.load_variant(&path, number) {
            Ok(stored) => stored,
            Err(e) => {
                self.toast = Some(error_toast(format!("The variant could not be read: {e}")));
                return;
            }
        };
        let reopened = variants::reopen(opened, first, &self.registry, number, stored);
        self.install(reopened, true, false);
        if let Err(e) = self.library.set_current_variant(&path, number) {
            self.toast = Some(error_toast(format!("The variant could not be chosen: {e}")));
        }
    }

    /// Delete variant: asks for confirmation first.
    fn ask_delete_variant(&mut self) {
        if let Some(why) = self.variant_blocks().delete {
            self.toast = Some(info_toast(why.to_owned()));
            return;
        }
        self.confirm_delete = self.variant.as_ref().map(|s| s.info.clone());
    }

    /// Deletes the open variant (confirmed) and opens a neighbour.
    fn delete_variant(&mut self, deleted: &VariantInfo) {
        let Some(path) = self.doc.as_ref().map(|d| d.path().to_owned()) else { return };
        if self.variant.as_ref().is_none_or(|s| s.info.number != deleted.number) {
            return; // another variant is open now
        }
        let Some(next) = variants::after_delete(&self.variant_list(), deleted.number) else { return };
        // The neighbour is read first: if it cannot be, nothing is deleted.
        let stored = match self.library.load_variant(&path, next) {
            Ok(stored) => stored,
            Err(e) => {
                self.toast = Some(error_toast(format!("The variant could not be deleted: {e}")));
                return;
            }
        };
        if let Err(e) = self.library.delete_variant(&path, deleted.number) {
            self.toast = Some(error_toast(format!("The variant could not be deleted: {e}")));
            return;
        }
        let Some(doc) = &self.doc else { return };
        let (opened, first) = (doc.opened().clone(), doc.generation() + 1);
        let reopened = variants::reopen(opened, first, &self.registry, next, stored);
        self.install(reopened, true, false);
        if let Err(e) = self.library.set_current_variant(&path, next) {
            self.toast = Some(error_toast(format!("The variant could not be chosen: {e}")));
        } else {
            self.toast = Some(info_toast(format!("Deleted {}", deleted.name)));
        }
    }

    /// The "Delete variant" confirmation, when asked.
    fn delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(target) = self.confirm_delete.clone() else { return };
        let t = Tokens::of(ctx.theme());
        let mut choice = None;
        dialog(ctx, "rne-delete-variant", |ui| {
            ui.label(RichText::new(format!("Delete {}?", target.name)).size(FONT[4]).strong());
            ui.add_space(SPACE[1]);
            ui.label(
                RichText::new(
                    "Its edits, history and preview are removed from the catalog. The RAW file and the other \
                     variants are not touched. This cannot be undone.",
                )
                .color(t.text_weak),
            );
            ui.add_space(SPACE[2]);
            choice = dialog_buttons(ui, &t, Some("Delete"), "Cancel", |_| {});
        });
        match choice {
            Some(Choice::Action) => {
                self.confirm_delete = None;
                self.delete_variant(&target);
            }
            Some(Choice::Cancel) => self.confirm_delete = None,
            None => {}
        }
    }

    // ----- drawing -----

    fn menu_bar(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let has_doc = self.doc.is_some();
        let undo = self.doc.as_ref().and_then(Document::undo_label);
        let redo = self.doc.as_ref().and_then(Document::redo_label);
        let exporting = self.exporting.is_some();
        let export_count = self.export_targets().len();
        let mut open_recent = None;
        let mut theme_choice = self.settings.theme;
        let mut exposure_warning = self.settings.exposure_warning;
        let mut open_models = false;
        let mut open_prefs = false;
        let mut open_folder = false;
        let mut graph_actions = Vec::new();
        let blocks = self.variant_blocks();
        let variant_list = self.variant_list();
        let current_variant = self.variant.as_ref().map(|s| s.info.number);
        let mut switch_to = None;
        let mut preset_actions = Vec::new();
        let apply_why = (export_count == 0).then_some("Open or select a photo first");
        let save_why = if has_doc { None } else { Some("Open a photo first") };
        // The selected node, when it is one that can be bypassed or deleted
        // (the same commands as its right-click menu in the graph).
        let editable_node = self.editable_node();

        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                menu_item(ui, Command::Open, None, None, commands);
                ui.menu_button("Open recent", |ui| {
                    if self.settings.recent.is_empty() {
                        ui.add_enabled(false, Button::new("No recent files"));
                    }
                    for path in &self.settings.recent {
                        if ui.button(file_name(path)).on_hover_text(path.display().to_string()).clicked() {
                            open_recent = Some(path.clone());
                            ui.close();
                        }
                    }
                });
                if ui.button("Show folder\u{2026}").on_hover_text("Show a folder's photos in the filmstrip").clicked() {
                    open_folder = true;
                    ui.close();
                }
                ui.separator();
                let why = if export_count == 0 {
                    Some("Open or select a photo first")
                } else {
                    exporting.then_some("An export is running")
                };
                let export_label = (export_count > 1).then(|| format!("Export {export_count} selected photos\u{2026}"));
                menu_item(ui, Command::Export, export_label, why, commands);
                menu_item(ui, Command::ExportAgain, None, why, commands);
                ui.separator();
                menu_item(ui, Command::Quit, None, None, commands);
            });
            ui.menu_button("Edit", |ui| {
                let undo_label = undo.as_ref().map(|l| format!("Undo {}", lowercase_first(l)));
                menu_item(ui, Command::Undo, undo_label, undo.is_none().then_some("Nothing to undo"), commands);
                let redo_label = redo.as_ref().map(|l| format!("Redo {}", lowercase_first(l)));
                menu_item(ui, Command::Redo, redo_label, redo.is_none().then_some("Nothing to redo"), commands);
                ui.separator();
                let why = "Select a node in the graph first";
                let bypass_label = if editable_node.is_some_and(|(_, on)| !on) { "Enable node" } else { "Bypass node" };
                if ui
                    .add_enabled(editable_node.is_some(), Button::new(bypass_label))
                    .on_disabled_hover_text(why)
                    .clicked()
                    && let Some((id, on)) = editable_node
                {
                    graph_actions.push(GraphAction::Enable(id, !on));
                    ui.close();
                }
                if ui
                    .add_enabled(editable_node.is_some(), Button::new("Delete node").shortcut_text("Del"))
                    .on_disabled_hover_text(why)
                    .clicked()
                    && let Some((id, _)) = editable_node
                {
                    graph_actions.push(GraphAction::Remove(id));
                    ui.close();
                }
                ui.separator();
                if ui.button("Preferences\u{2026}").on_hover_text("Viewer background, theme, presets folder").clicked()
                {
                    open_prefs = true;
                    ui.close();
                }
            });
            ui.menu_button("Image", |ui| {
                ui.menu_button("Variants", |ui| {
                    menu_item(ui, Command::NewVariant, None, blocks.new, commands);
                    menu_item(ui, Command::CloneVariant, None, blocks.clone, commands);
                    menu_item(ui, Command::DeleteVariant, None, blocks.delete, commands);
                    ui.separator();
                    if variant_list.is_empty() {
                        ui.add_enabled(false, Button::new("No photo open"));
                    }
                    for v in &variant_list {
                        if ui.radio(current_variant == Some(v.number), v.name.as_str()).clicked() {
                            switch_to = Some(v.number);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Presets", |ui| {
                    self.presets.presets_menu(ui, apply_why, save_why, &mut preset_actions);
                });
                ui.menu_button("Style brushes", |ui| {
                    self.presets.brushes_menu(ui, save_why, &mut preset_actions);
                });
            });
            ui.menu_button("View", |ui| {
                let why = (!has_doc).then_some("Open an image first");
                for c in [Command::ZoomFit, Command::Zoom100, Command::ZoomIn, Command::ZoomOut] {
                    menu_item(ui, c, None, why, commands);
                }
                ui.separator();
                if ui
                    .checkbox(&mut exposure_warning, "Exposure warning")
                    .on_hover_text("Burnt highlights in red; blocked shadows in blue when turned on in Preferences")
                    .clicked()
                {
                    ui.close();
                }
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    for (choice, label) in
                        [(ThemeChoice::System, "System"), (ThemeChoice::Light, "Light"), (ThemeChoice::Dark, "Dark")]
                    {
                        if ui.radio_value(&mut theme_choice, choice, label).clicked() {
                            ui.close();
                        }
                    }
                });
            });
            ui.menu_button("Tools", |ui| {
                if ui.button("AI models\u{2026}").on_hover_text("Download or check the AI models").clicked() {
                    open_models = true;
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                ui.label(format!("{APP_NAME} {VERSION}"));
                ui.weak("Non-destructive RAW editor with nodes");
            });
        });

        if !graph_actions.is_empty() {
            self.apply_graph(graph_actions);
        }
        for action in preset_actions {
            self.preset_action(action);
        }
        if open_prefs {
            self.prefs.open(&self.settings);
        }
        if open_models {
            self.models.open_manager();
        }
        if open_folder {
            let mut dialog = rfd::FileDialog::new().set_title("Show a folder");
            if let Some(dir) = self.library.folder().or(self.settings.open_dir.as_deref()) {
                dialog = dialog.set_directory(dir);
            }
            if let Some(dir) = dialog.pick_folder() {
                self.library.show_folder(dir, self.worker.as_ref());
            }
        }
        self.settings.exposure_warning = exposure_warning;
        if theme_choice != self.settings.theme {
            self.settings.theme = theme_choice;
            ui.ctx().set_theme(theme_preference(theme_choice));
        }
        if let Some(path) = open_recent {
            self.open(path);
        }
        if let Some(number) = switch_to {
            self.switch_variant(number);
        }
    }

    /// The status line; a variant chosen in its variant menu goes to `switch_to`.
    fn status_bar(&self, ui: &mut egui::Ui, switch_to: &mut Option<u32>) {
        let t = Tokens::of(ui.ctx().theme());
        ui.horizontal(|ui| {
            if let Some(folder) = self.library.folder() {
                let shown = self.library.strip.visible().len();
                let total = self.library.strip.photos.len();
                let mut count =
                    if shown == total { format!("{total} photos") } else { format!("{shown} of {total} photos") };
                let selected = self.library.selection().len();
                if selected > 1 {
                    count.push_str(&format!(", {selected} selected"));
                }
                ui.label(
                    RichText::new(format!("{} \u{b7} {count}", editor::library::folder_name(folder)))
                        .color(t.text_weak),
                )
                .on_hover_text(folder.display().to_string());
                ui.separator();
            }
            if let Some(doc) = &self.doc {
                ui.label(doc.camera());
                ui.separator();
                let (w, h) = doc.shown_size(self.tool != Some(ViewerTool::Crop));
                ui.label(format!("{w} \u{d7} {h}"));
                if let Some(vp) = &self.viewport {
                    ui.separator();
                    ui.label(format!("{:.0} %", self.view.percent([w as f32, h as f32], vp)));
                }
                if !doc.has_camera_white_balance() {
                    ui.separator();
                    ui.label(RichText::new("No white balance in file").color(t.warning));
                }
                let notes = doc.develop_notes().join("\n");
                match doc.lens() {
                    Some(lens) => {
                        ui.separator();
                        ui.label(lens).on_hover_text(notes);
                    }
                    None if !notes.is_empty() => {
                        ui.separator();
                        ui.weak("No lens corrections").on_hover_text(notes);
                    }
                    None => {}
                }
            }
            if let Some(state) = &self.variant {
                // Which variant is open, with the others a click away.
                let list = self.variant_list();
                let place = list.iter().position(|v| v.number == state.info.number).map_or(1, |i| i + 1);
                ui.separator();
                ui.menu_button(format!("{} \u{b7} {place}/{}", state.info.name, list.len()), |ui| {
                    for v in &list {
                        if ui.radio(v.number == state.info.number, v.name.as_str()).clicked() {
                            *switch_to = Some(v.number);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("Variants of this photo (also in Image \u{25b8} Variants)");
                if let Some(why) = &state.read_only {
                    ui.separator();
                    ui.label(RichText::new("Read-only").strong().color(t.warning)).on_hover_text(why.as_str());
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match (&self.backend, &self.gpu_problem) {
                    (Some(Backend::Cpu), Some(why)) => {
                        ui.weak("CPU").on_hover_text(format!("No GPU in use: {why}"));
                    }
                    (Some(b), _) => {
                        let label = ui.weak(b.to_string());
                        if let Some((timing, quality, [w, h])) = &self.last_render {
                            let kind = match quality {
                                Quality::Draft => "draft",
                                Quality::Preview => "preview",
                                Quality::Full => "full-size image",
                            };
                            let mut text = format!(
                                "Last {kind} ({w} \u{d7} {h}): render {} ms, display {} ms",
                                timing.render.as_millis(),
                                timing.display.as_millis()
                            );
                            if let Some(open) = &self.last_open {
                                text.push_str(&open_timing_text(open));
                            }
                            label.on_hover_text(text);
                        }
                    }
                    (None, _) => {
                        ui.weak("Starting the GPU\u{2026}");
                    }
                }
                if self.developing.is_some() {
                    ui.separator();
                    ui.weak("Applying lens corrections\u{2026}");
                }
                if let Some(job) = &self.opening {
                    ui.separator();
                    ui.label(format!("Opening {}\u{2026}", job.name));
                    if job.started.elapsed().as_secs_f32() > SPINNER_DELAY_SECONDS {
                        ui.add(egui::Spinner::new());
                    } else {
                        ui.ctx().request_repaint_after(Duration::from_secs_f32(SPINNER_DELAY_SECONDS));
                    }
                }
                if let Some(run) = &self.exporting {
                    // Right to left: Cancel at the end, the text first.
                    ui.separator();
                    let stopping = run.handle.is_cancelled();
                    if ui
                        .add_enabled(!stopping, Button::new("Cancel"))
                        .on_hover_text("Stop after the photo being exported; files written stay")
                        .clicked()
                    {
                        run.handle.cancel();
                    }
                    #[allow(clippy::cast_precision_loss)] // photo counts
                    let fraction = if run.total == 0 { 0.0 } else { run.done as f32 / run.total as f32 };
                    ui.add(egui::ProgressBar::new(fraction).desired_width(120.0));
                    let text = if stopping {
                        "Stopping the export\u{2026}".to_owned()
                    } else if run.total > 1 {
                        format!("Exporting {} of {}: {}\u{2026}", (run.done + 1).min(run.total), run.total, run.current)
                    } else {
                        format!("Exporting {}\u{2026}", run.current)
                    };
                    ui.label(text);
                }
            });
        });
    }

    fn central(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>, library: &mut Vec<LibraryAction>) {
        let t = Tokens::of(ui.ctx().theme());
        let rect = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        let vp = Viewport { rect: to_view_rect(rect), pixels_per_point: ppp };
        self.viewport = Some(vp);

        // Several photos selected in the filmstrip: their tiles instead of the photo.
        if self.library.selection().len() > 1 {
            let current = self.doc.as_ref().map(|d| d.path().to_owned());
            self.library.overview(ui, current.as_deref(), library);
            return;
        }

        // Made before the photo is borrowed for drawing.
        let warning = if self.settings.exposure_warning {
            self.exposure_warning_texture(ui.ctx()).map(egui::TextureHandle::id)
        } else {
            None
        };
        let mut picked = None;
        let mut geo_edits = Vec::new();
        let mut mask_edits = Vec::new();
        let mut retouch_edits = Vec::new(); // Retouch (T4)
        let mut crop_apply = false;
        if let Some(doc) = &self.doc {
            let response = ui.allocate_rect(rect, Sense::click_and_drag());
            let cropped = self.tool != Some(ViewerTool::Crop);
            let (w, h) = doc.shown_size(cropped);
            let image = [w as f32, h as f32];
            let geometry = doc.recipe().geometry;
            let (uw, uh) = doc.upright_size();
            let frame_aspect = uw as f32 / uh.max(1) as f32;

            if response.hovered() {
                let (scroll, pinch, hover) =
                    ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
                let factor = pinch * (scroll * WHEEL_ZOOM_PER_POINT).exp();
                if (factor - 1.0).abs() > 1e-4 {
                    self.view.zoom_by(factor, hover.map(|p| [p.x, p.y]), image, &vp);
                }
            }
            // The colour entering the selected node under the pointer (marked
            // on the levels and curve), and a click for a waiting picker.
            let r = self.view.image_rect(image, &vp);
            let screen = to_egui_rect(r);
            // A point of the photo on screen, in the upright image (0–1), through the geometry.
            let at = |p: egui::Pos2| {
                geometry.to_source([(p.x - r.x) / r.w.max(1.0), (p.y - r.y) / r.h.max(1.0)], frame_aspect, cropped)
            };
            self.probe = match (self.selected_node, response.hover_pos()) {
                (Some(node), Some(p)) => at(p).and_then(|a| doc.probe(&self.registry, node, a)),
                _ => None,
            };
            if let Some(pick) = self.pick {
                if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                if response.clicked()
                    && let Some(p) = response.interact_pointer_pos()
                {
                    picked = at(p).map(|a| (pick, a));
                }
            }
            if self.pick.is_none() && self.tool.is_none() && matches!(self.view.zoom(), Zoom::Pixels { .. }) {
                if response.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                } else if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grab);
                }
            }

            let painter = ui.painter_at(rect);
            if let Some(texture) = &self.texture {
                let r = self.view.image_rect(image, &vp);
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(texture.id(), to_egui_rect(r), uv, Color32::WHITE);
                if let Some(warning) = warning {
                    painter.image(warning, to_egui_rect(r), uv, Color32::WHITE);
                }
                let (right, middle) = ui.input(|i| {
                    (
                        i.pointer.button_down(egui::PointerButton::Secondary),
                        i.pointer.button_down(egui::PointerButton::Middle),
                    )
                });
                match (response.hover_pos(), &self.shown) {
                    (Some(p), Some(shown)) if right || middle => {
                        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                        let screen = to_egui_rect(r);
                        let (sw, sh) = (shown.width as f32, shown.height as f32);
                        // The pointer in pixels of the image on screen, and of the full image.
                        let at =
                            [(p.x - screen.min.x) / screen.width() * sw, (p.y - screen.min.y) / screen.height() * sh];
                        let full = [
                            ((p.x - screen.min.x) / screen.width() * w as f32).floor(),
                            ((p.y - screen.min.y) / screen.height() * h as f32).floor(),
                        ];
                        if right {
                            eyedropper_hud(&painter, &t, rect, p, shown, at, full);
                        } else {
                            let mag = screen.width() / sw.max(1.0) * editor::inspect::LOUPE_FACTOR;
                            let patch = editor::inspect::loupe_patch(
                                &shown.pixels,
                                shown.width as usize,
                                shown.height as usize,
                                at,
                                mag,
                            );
                            let side = editor::inspect::LOUPE_SIDE;
                            let img = egui::ColorImage::from_rgba_unmultiplied([side, side], &patch);
                            match &mut self.loupe {
                                Some(tex) => tex.set(img, TextureOptions::NEAREST),
                                None => {
                                    self.loupe = Some(ui.ctx().load_texture("rne-loupe", img, TextureOptions::NEAREST));
                                }
                            }
                            if let Some(tex) = &self.loupe {
                                let zoom = self.view.percent(image, &vp) * editor::inspect::LOUPE_FACTOR;
                                loupe(&painter, &t, rect, p, tex, zoom, full);
                            }
                        }
                    }
                    _ => {}
                }
                // The geometry tools draw over the photo and may own the drag.
                let mut tool_drag = false;
                match self.tool {
                    Some(ViewerTool::Crop) => {
                        let (owned, apply) = self.geometry_ui.crop_overlay(ui, &response, screen, frame_aspect);
                        tool_drag = owned;
                        crop_apply = apply;
                    }
                    Some(ViewerTool::Level) => {
                        tool_drag =
                            self.geometry_ui.level_overlay(ui, &response, screen, image, &geometry, &mut geo_edits);
                    }
                    Some(ViewerTool::Perspective) => crate::geometry_ui::grid_overlay(ui, screen, &t),
                    Some(tool @ (ViewerTool::Masks | ViewerTool::AiSelect)) => {
                        let node = self.selected_node.filter(|n| {
                            matches!(doc.recipe().graph.node(*n).map(|x| &x.kind), Some(NodeKind::Tool(_)))
                        });
                        // The mask in red over the photo.
                        if let Some(n) = node {
                            let key = (doc.generation(), n, cropped);
                            self.mask_ui.draw_overlay(ui, screen, key, || {
                                let (_, def) = doc.mask_of(n)?;
                                let guide = engine::downscale(doc.preview(), 1024).ok()?;
                                let rasters: &dyn masks::RasterSource = match editor::worker::mask_store() {
                                    Some(s) => s,
                                    None => &masks::NoRasters,
                                };
                                let m = masks::evaluate(def, rasters, &guide).mask;
                                editor::masking::shown_mask(&m, doc.orientation(), &geometry, cropped).ok()
                            });
                        }
                        let crop = match geometry.crop.filter(|_| cropped) {
                            Some(c) => [c.width(), c.height()],
                            None => [1.0, 1.0],
                        };
                        let scale = if geometry.warps() { geometry.warp(frame_aspect).scale() } else { 1.0 };
                        let long_side = crate::mask_ui::long_side_on_screen(screen, crop, scale);
                        let to_developed = |p: egui::Pos2| at(p).and_then(|u| doc.developed_point(u));
                        tool_drag =
                            self.mask_ui.overlay(ui, &response, tool, node, long_side, &to_developed, &mut mask_edits);
                    }
                    // Retouch (T4): the Heal and Clone spots.
                    Some(tool @ (ViewerTool::Heal | ViewerTool::Clone)) => {
                        let (reg, node) = (&self.registry, self.selected_node);
                        tool_drag =
                            self.retouch_ui.overlay(ui, &response, screen, doc, reg, node, tool, &mut retouch_edits);
                    }
                    _ => {}
                }
                // The left button pans; the right one is the eyedropper, the middle one the loupe.
                if !tool_drag && response.dragged_by(egui::PointerButton::Primary) {
                    let d = response.drag_delta();
                    self.view.pan([d.x, d.y], image, &vp);
                }
                // Retouch (T4): no double-click zoom while clicks place spots.
                if response.double_clicked()
                    && !matches!(self.tool, Some(ViewerTool::Crop | ViewerTool::Heal | ViewerTool::Clone))
                {
                    match self.view.zoom() {
                        Zoom::Fit => {
                            let at = response.interact_pointer_pos().map(|p| [p.x, p.y]);
                            self.view.actual_pixels(at, image, &vp);
                        }
                        Zoom::Pixels { .. } => self.view.fit(),
                    }
                }
            } else {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Developing\u{2026}",
                    egui::FontId::proportional(FONT[3]),
                    t.text_weak,
                );
            }
        } else {
            self.empty_state(ui, commands, &t);
        }
        if let Some((pick, at)) = picked {
            self.apply_pick(pick, at);
        }
        // The options bar of a geometry tool, at the top of the viewer.
        if let Some(doc) = &self.doc {
            let geometry = doc.recipe().geometry;
            let (uw, uh) = doc.upright_size();
            let frame_aspect = uw as f32 / uh.max(1) as f32;
            match self.tool {
                Some(ViewerTool::Crop) => {
                    if let Some(apply) = self.geometry_ui.crop_options(ui.ctx(), rect, frame_aspect) {
                        self.leave_crop(apply);
                    }
                }
                Some(tool @ (ViewerTool::Level | ViewerTool::Perspective)) => {
                    self.geometry_ui.sliders(ui.ctx(), rect, tool, &geometry, &mut geo_edits);
                    if tool == ViewerTool::Level {
                        GeometryUi::level_keys(ui.ctx(), &geometry, &mut geo_edits);
                    }
                }
                Some(tool @ (ViewerTool::Masks | ViewerTool::AiSelect)) => {
                    let node = self.selected_node.and_then(|n| {
                        let node = doc.recipe().graph.node(n)?;
                        matches!(node.kind, NodeKind::Tool(_))
                            .then(|| (n, crate::graph_ui::node_title(node, &self.registry)))
                    });
                    let def = node.as_ref().and_then(|(n, _)| doc.mask_of(*n)).map(|(_, d)| d.clone());
                    let busy = self.ai_job.is_some();
                    let brushes = self.presets.brush_names();
                    self.mask_ui.options(ui.ctx(), rect, tool, node, def.as_ref(), busy, &brushes, &mut mask_edits);
                }
                // Retouch (T4): the Heal and Clone options bar.
                Some(tool @ (ViewerTool::Heal | ViewerTool::Clone)) => {
                    let (reg, node) = (&self.registry, self.selected_node);
                    if let Some(next) =
                        self.retouch_ui.options(ui.ctx(), rect, tool, doc, reg, node, &mut retouch_edits)
                    {
                        self.tool = Some(next);
                    }
                }
                _ => {}
            }
        }
        if crop_apply {
            self.leave_crop(true);
        }
        self.apply_geometry(geo_edits);
        self.apply_mask_edits(mask_edits, ui.ctx());
        // Retouch (T4): apply the Heal and Clone changes.
        if let Some(doc) = &mut self.doc
            && let Some(problem) = self.retouch_ui.apply(doc, &self.registry, &mut self.selected_node, retouch_edits)
        {
            self.toast = Some(error_toast(problem));
        }

        if ui.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect.shrink(SPACE[3]), egui::CornerRadius::same(8), t.bg_overlay.gamma_multiply(0.85));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Drop to open",
                egui::FontId::proportional(FONT[4]),
                t.text_strong,
            );
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>, t: &Tokens) {
        let mut open = None;
        let shortcut = ui.ctx().format_shortcut(&Command::Open.shortcuts()[0]);
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.3).max(SPACE[5]));
            if let Some(job) = &self.opening {
                ui.label(RichText::new(format!("Opening {}\u{2026}", job.name)).size(FONT[4]).color(t.text_strong));
                if job.started.elapsed().as_secs_f32() > SPINNER_DELAY_SECONDS {
                    ui.add_space(SPACE[2]);
                    ui.add(egui::Spinner::new().size(24.0));
                } else {
                    ui.ctx().request_repaint_after(Duration::from_secs_f32(SPINNER_DELAY_SECONDS));
                }
                return;
            }
            ui.label(RichText::new("Open a RAW file").size(FONT[4]).strong().color(t.text_strong));
            ui.add_space(SPACE[0]);
            ui.label(RichText::new(format!("Drag a file here, or press {shortcut}")).color(t.text_weak));
            ui.add_space(SPACE[3]);
            let button = Button::new(RichText::new("Open\u{2026}").color(t.on_accent)).fill(t.accent);
            if ui.add(button).clicked() {
                commands.push(Command::Open);
            }
            if !self.settings.recent.is_empty() {
                ui.add_space(SPACE[4]);
                ui.label(RichText::new("Recent").color(t.text_weak));
                for path in self.settings.recent.iter().take(5) {
                    if ui.link(file_name(path)).on_hover_text(path.display().to_string()).clicked() {
                        open = Some(path.clone());
                    }
                }
            }
        });
        if let Some(path) = open {
            self.open(path);
        }
    }

    fn show_toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = &mut self.toast else { return };
        let t = Tokens::of(ctx.theme());
        let mut close = false;
        let mut action = None;
        let shown = egui::Area::new(Id::unique("rne-toast"))
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -48.0))
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(t.bg_overlay).inner_margin(Margin::symmetric(12, 8)).show(
                    ui,
                    |ui| {
                        ui.horizontal(|ui| {
                            if toast.kind == ToastKind::Error {
                                ui.label(RichText::new("Error").strong().color(t.error));
                            }
                            ui.label(RichText::new(&toast.text).color(t.text_strong));
                            if let Some(a) = &toast.action
                                && ui.button(a.label()).clicked()
                            {
                                action = Some(a.clone());
                            }
                            if ui.small_button("\u{d7}").on_hover_text("Close").clicked() {
                                close = true;
                            }
                        });
                    },
                );
            });
        let now = Instant::now();
        if shown.response.contains_pointer() {
            toast.deadline = toast.deadline.max(now + Duration::from_secs(1));
        }
        if close || action.is_some() || now >= toast.deadline {
            self.toast = None;
        } else {
            ctx.request_repaint_after(toast.deadline - now);
        }
        if let Some(ToastAction::ShowInFolder(path)) = action {
            let launch = platform::reveal_in_file_manager(platform::Os::current(), &path);
            if let Err(e) = std::process::Command::new(launch.program).args(&launch.args).spawn() {
                self.toast = Some(error_toast(format!("Cannot open the folder: {e}")));
            }
        }
    }
}

impl EditorApp {
    /// Explorer and filmstrip side by side, History and Metadata below.
    fn left_panel(&mut self, ui: &mut egui::Ui, library: &mut Vec<LibraryAction>, tabs: &mut Vec<TabAction>) {
        let t = Tokens::of(ui.ctx().theme());
        egui::Panel::bottom("left-tabs")
            .resizable(true)
            .default_size(TABS_HEIGHT[0])
            .size_range(TABS_HEIGHT[1]..=TABS_HEIGHT[2])
            .frame(Frame::NONE.fill(t.bg).inner_margin(Margin::symmetric(8, 4)))
            .show(ui, |ui| {
                let path = self.doc.as_ref().map(|d| d.path().to_owned());
                let meta = (self.tabs.tab == LeftTab::Metadata)
                    .then(|| path.as_deref().map(|p| self.library.catalog_meta(p)))
                    .flatten();
                // The panel keeps the height the user dragged it to: the tabs
                // draw inside that rectangle (clipped), and it is reserved whole,
                // so neither a short nor a long tab resizes the panel.
                let rect = ui.available_rect_before_wrap();
                let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                inside.set_clip_rect(rect.intersect(ui.clip_rect()));
                tabs.extend(self.tabs.show(&mut inside, self.doc.as_ref(), meta.as_ref()));
                ui.allocate_rect(rect, Sense::hover());
            });
        egui::CentralPanel::default().frame(Frame::NONE.fill(t.bg).inner_margin(Margin::symmetric(8, 4))).show(
            ui,
            |ui| {
                let progress = self.tagging.as_ref().map(|j| TaggingProgress { done: j.done, total: j.total });
                self.library.explorer(ui, self.worker.as_ref(), progress, library);
            },
        );
    }

    /// The histogram (or waveform, or parade) above the settings of the
    /// selected node; returns the scope chosen with its tabs.
    fn right_panel(&self, ui: &mut egui::Ui, edits: &mut Vec<UiEdit>) -> (ScopeKind, Option<f32>) {
        let t = Tokens::of(ui.ctx().theme());
        let height = self.settings.scope_height.unwrap_or(HISTOGRAM_HEIGHT[0]);
        let scope = self.scopes.show(ui, self.settings.scope, self.histogram.as_deref(), height);
        let resized = crate::scopes::Scopes::resize_handle(ui, height);
        ui.add_space(SPACE[0]);
        egui::ScrollArea::vertical().id_salt("node-settings").auto_shrink([false, false]).show(ui, |ui| {
            match &self.doc {
                None => {
                    ui.add_space(SPACE[4]);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No photo open").size(FONT[3]).strong());
                        ui.label(
                            RichText::new("Choose one in the filmstrip, or drop a RAW file here.").color(t.text_weak),
                        );
                    });
                }
                Some(doc) => {
                    // Saved by a newer editor (or unreadable): say so above the settings.
                    if let Some(why) = self.variant.as_ref().and_then(|s| s.read_only.as_deref()) {
                        ui.add_space(SPACE[1]);
                        Frame::NONE
                            .fill(t.bg_raised)
                            .stroke(egui::Stroke::new(1.0, t.warning))
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(why).color(t.warning));
                            });
                        ui.add_space(SPACE[1]);
                    }
                    let export_summary = self.settings.export.summary();
                    let cx = inspector::Context {
                        registry: &self.registry,
                        selected: self.selected_node,
                        developing: self.developing.is_some(),
                        export_summary: &export_summary,
                        histogram: self.histogram.as_deref(),
                        pick: self.pick,
                        probe: self.probe,
                    };
                    // The selected node first, then the ones used just before it.
                    let shown: Vec<NodeId> = if self.recent_nodes.is_empty() {
                        self.selected_node.into_iter().collect()
                    } else {
                        self.recent_nodes.clone()
                    };
                    if shown.is_empty() {
                        inspector::settings(ui, doc, cx, edits);
                    }
                    for (i, id) in shown.into_iter().enumerate() {
                        if i > 0 {
                            ui.add_space(SPACE[2]);
                            ui.separator();
                            ui.add_space(SPACE[1]);
                        }
                        ui.push_id(("rne-recent-node", id), |ui| {
                            inspector::settings(ui, doc, inspector::Context { selected: Some(id), ..cx }, edits);
                        });
                    }
                }
            }
        });
        (scope, resized)
    }

    /// The photo, with the node graph under it when a photo is open.
    fn center(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>, library: &mut Vec<LibraryAction>) {
        let t = Tokens::of(ui.ctx().theme());
        let mut actions = Vec::new();
        if let Some(doc) = &self.doc {
            egui::Panel::bottom("node-graph")
                .resizable(true)
                .default_size(GRAPH_HEIGHT[0])
                .size_range(GRAPH_HEIGHT[1]..=GRAPH_HEIGHT[2])
                .frame(Frame::NONE.fill(t.graph))
                .show(ui, |ui| {
                    actions = self.graph.show(ui, &doc.recipe().graph, &self.registry, self.selected_node);
                });
        }
        self.apply_graph(actions);
        let background = theme::viewer_background(self.settings.viewer_background, t.canvas);
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(background))
            .show(ui, |ui| self.central(ui, commands, library));
    }

    /// Leaves the crop tool, applying the edited box (one undo step) or not.
    fn leave_crop(&mut self, apply: bool) {
        if let Some(doc) = &mut self.doc {
            let g = doc.recipe().geometry;
            if let Some(next) = self.geometry_ui.finish_crop(&g, apply)
                && let Err(e) = doc.set_geometry(&self.registry, next, None)
            {
                self.toast = Some(error_toast(format!("That change was refused: {e}")));
            }
        }
        if self.tool == Some(ViewerTool::Crop) {
            self.tool = None;
        }
    }

    /// Applies what the Level and Perspective tools asked for.
    fn apply_geometry(&mut self, edits: Vec<GeoEdit>) {
        let Some(doc) = &mut self.doc else { return };
        for edit in edits {
            match edit {
                GeoEdit::Begin => self.geometry_gesture = Some(doc.begin_gesture()),
                GeoEdit::End => self.geometry_gesture = None,
                GeoEdit::Set { geometry, in_drag } => {
                    let gesture = self.geometry_gesture.filter(|_| in_drag);
                    if let Err(e) = doc.set_geometry(&self.registry, geometry, gesture) {
                        self.toast = Some(error_toast(format!("That change was refused: {e}")));
                    }
                }
            }
        }
    }

    /// Applies what the Masks and AI Select tools asked for: the new mask
    /// definition (one undo step; a new mask node the first time), then the
    /// AI models in the background when the mask has new AI parts. A style
    /// brush's first stroke adds its nodes behind a new mask and selects
    /// the first, so further strokes extend that mask.
    fn apply_mask_edits(&mut self, edits: Vec<MaskEdit>, ctx: &egui::Context) {
        for edit in edits {
            let brush = self.mask_ui.brush;
            let node = match edit {
                MaskEdit::ChooseStyleBrush(index) => {
                    self.preset_action(PresetAction::Arm(index));
                    continue;
                }
                MaskEdit::StyleStroke { name, style, points } => {
                    let Some(doc) = &mut self.doc else { return };
                    let def = editor::masking::add_stroke(None, points, brush, false);
                    match doc.add_style_brush(&self.registry, &name, style, def) {
                        Ok(node) => self.selected_node = Some(node),
                        Err(e) => self.toast = Some(error_toast(format!("The style brush could not be added: {e}"))),
                    }
                    continue;
                }
                MaskEdit::Stroke { node, .. }
                | MaskEdit::Click { node, .. }
                | MaskEdit::Subject { node }
                | MaskEdit::Def { node, .. } => node,
            };
            let Some(doc) = &mut self.doc else { return };
            let current = doc.mask_of(node).map(|(_, d)| d.clone());
            let def = match edit {
                MaskEdit::Stroke { points, erase, .. } => {
                    editor::masking::add_stroke(current.as_ref(), points, brush, erase)
                }
                MaskEdit::Click { at, include, new_object, .. } => {
                    editor::masking::add_click(current.as_ref(), at, include, new_object)
                }
                MaskEdit::Subject { .. } => editor::masking::add_subject(current.as_ref()),
                MaskEdit::Def { def, .. } => def,
                MaskEdit::ChooseStyleBrush(_) | MaskEdit::StyleStroke { .. } => continue,
            };
            let result = match doc.mask_of(node).map(|(id, _)| id) {
                Some(mask) => doc.set_mask(&self.registry, mask, def, None).map(|()| mask),
                None => doc.add_mask(&self.registry, node, def),
            };
            match result {
                Ok(mask) => self.start_ai_mask(mask, ctx),
                Err(e) => self.toast = Some(error_toast(format!("That change was refused: {e}"))),
            }
        }
    }

    /// Reads the presets folder again and tells the filmstrip the presets.
    fn reload_presets(&mut self) {
        let problems = self.presets.reload(self.settings.presets_folder().as_deref());
        self.library.set_presets(self.presets.preset_names());
        if let Some(first) = problems.first() {
            let more = if problems.len() > 1 { format!(" (and {} more)", problems.len() - 1) } else { String::new() };
            self.toast = Some(error_toast(format!("A preset could not be read: {first}{more}")));
        }
    }

    /// The Masks tool's current brush, as a style brush would keep it.
    fn brush_settings(&self) -> editor::presets::BrushSettings {
        let b = self.mask_ui.brush;
        editor::presets::BrushSettings {
            radius: b.radius,
            hardness: b.hardness,
            opacity: b.opacity,
            edge_aware: b.edge_aware,
        }
    }

    /// Does what a preset menu asked for.
    fn preset_action(&mut self, action: PresetAction) {
        match action {
            PresetAction::Apply(index) => {
                let targets = self.export_targets();
                self.apply_preset(index, &targets);
            }
            PresetAction::Arm(index) => self.arm_style_brush(index),
            PresetAction::Save(brush) => {
                let settings = self.brush_settings();
                let Some(doc) = &self.doc else { return };
                self.presets.open_save(doc.recipe(), &self.registry, brush, self.selected_node, settings);
            }
            PresetAction::ShowFolder => match crate::presets_ui::ensure_folder(self.settings.presets_folder()) {
                Ok(folder) => {
                    let open = platform::open_with_default_app(platform::Os::current(), &folder);
                    if let Err(e) = crate::library_ui::launch(&open) {
                        self.toast = Some(error_toast(format!("The presets folder could not be shown: {e}")));
                    }
                }
                Err(e) => self.toast = Some(error_toast(e)),
            },
            PresetAction::Reload => self.reload_presets(),
        }
    }

    /// Applies preset number `index` to `photos`: the open photo through
    /// its document (one undo step), the others in the catalog (their
    /// thumbnails go back to the camera's until they are opened again).
    fn apply_preset(&mut self, index: usize, photos: &[PathBuf]) {
        let Some(entry) = self.presets.presets().get(index).map(|e| (*e).clone()) else { return };
        let (style, missing) = editor::presets::usable(&entry.preset.style, &self.registry);
        let name = entry.preset.name.as_str();
        let mut done = 0;
        let mut failed = Vec::new();
        for path in photos {
            let result = match self.doc.as_mut().filter(|d| d.path() == path.as_path()) {
                Some(doc) => doc.apply_style(&self.registry, name, style.clone()).map_err(|e| e.to_string()),
                None => self.library.shown_variant(path).and_then(|(number, stored)| {
                    let save =
                        variants::apply_style_to_stored(&self.registry, number, stored.as_ref(), name, style.clone())?;
                    self.library.save_restyled(path, &save)
                }),
            };
            match result {
                Ok(()) => done += 1,
                Err(e) => failed.push(format!("{}: {e}", file_name(path))),
            }
        }
        let photos_text = |n: usize| if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
        let mut text = format!("Applied \u{201c}{name}\u{201d} to {}", photos_text(done));
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(ToolId::as_str).collect();
            text.push_str(&format!("; left out tools this version lacks: {}", names.join(", ")));
        }
        self.toast = Some(match failed.first() {
            Some(first) => error_toast(format!("{text}; {} failed: {first}", failed.len())),
            None => info_toast(text),
        });
    }

    /// Arms style brush number `index` in the Masks tool.
    fn arm_style_brush(&mut self, index: usize) {
        if self.doc.is_none() {
            self.toast = Some(info_toast("Open a photo first".to_owned()));
            return;
        }
        let Some(entry) = self.presets.brushes().get(index).map(|e| (*e).clone()) else { return };
        let (style, missing) = editor::presets::usable(&entry.preset.style, &self.registry);
        if style.nodes.is_empty() {
            let names: Vec<&str> = missing.iter().map(ToolId::as_str).collect();
            self.toast = Some(error_toast(format!(
                "\u{201c}{}\u{201d} paints tools this version lacks: {}",
                entry.preset.name,
                names.join(", ")
            )));
            return;
        }
        let settings = entry.preset.brush.unwrap_or_default();
        self.mask_ui.arm(entry.preset.name, style, &settings);
        self.tool = Some(ViewerTool::Masks);
        self.mask_ui.show = false;
    }

    /// Writes a preset from the Save window into the presets folder.
    fn save_preset(&mut self, preset: &editor::presets::Preset) {
        let result = self
            .settings
            .presets_folder()
            .ok_or_else(|| "this system names no data folder; choose one in Preferences".to_owned())
            .and_then(|folder| editor::presets::save(&folder, preset));
        match result {
            Ok(path) => {
                self.reload_presets();
                let kind = if preset.brush.is_some() { "Style brush" } else { "Preset" };
                self.toast = Some(info_toast(format!("{kind} saved as {}", file_name(&path))));
            }
            Err(e) => self.toast = Some(error_toast(format!("The preset could not be saved: {e}"))),
        }
    }

    /// Runs the AI models for the parts of mask node `mask` that have no
    /// result yet (one job at a time; the next starts when it ends).
    fn start_ai_mask(&mut self, mask: NodeId, ctx: &egui::Context) {
        if self.ai_job.is_some() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let graph = &doc.recipe().graph;
        let Some(def) = graph.node(mask).and_then(|n| n.mask.clone()) else { return };
        let mut features = Vec::new();
        for (_, ai) in def.pending_ai() {
            features.push(match ai.target {
                core_model::AiTarget::Subject => ai_models::Feature::Subject,
                core_model::AiTarget::Object { .. } => ai_models::Feature::Objects,
                core_model::AiTarget::PersonPart { .. } => ai_models::Feature::PersonParts,
            });
        }
        if features.is_empty() {
            return;
        }
        for feature in features {
            if !self.models.require(feature) {
                return; // the download is offered; click again once it is done
            }
        }
        let photo = crate::ai_mask_job::Photo {
            path: doc.path().to_owned(),
            settings: doc.recipe().develop.clone(),
            image: Arc::clone(doc.developed()),
            orientation: doc.orientation(),
        };
        let repaint = ctx.clone();
        match AiMaskJob::start(photo, mask, def, move || repaint.request_repaint()) {
            Ok(job) => self.ai_job = Some(job),
            Err(e) => self.toast = Some(error_toast(format!("The AI selection could not start: {e}"))),
        }
    }

    /// A finished AI mask job: its rasters go into the mask as it is now.
    fn poll_ai_mask(&mut self, ctx: &egui::Context) {
        let Some(result) = self.ai_job.as_ref().and_then(AiMaskJob::poll) else { return };
        let Some(job) = self.ai_job.take() else { return };
        let Some(doc) = &mut self.doc else { return };
        match result {
            Ok(computed) => {
                let Some(current) = doc.recipe().graph.node(job.mask).and_then(|n| n.mask.clone()) else { return };
                let filled = editor::masking::fill_rasters(&current, &computed);
                if filled != current
                    && let Err(e) = doc.set_mask(&self.registry, job.mask, filled, None)
                {
                    self.toast = Some(error_toast(format!("The AI mask could not be kept: {e}")));
                }
                // Clicks made while the models ran.
                self.start_ai_mask(job.mask, ctx);
            }
            Err(message) => self.toast = Some(error_toast(message)),
        }
    }

    /// Reacts to the viewer tool changing: entering the crop tool starts
    /// the box at the recipe's crop, leaving it (for another tool) applies
    /// the box; the photo is shown again with or without its crop.
    fn tool_changed(&mut self) {
        if self.tool == self.prev_tool {
            return;
        }
        if self.tool != Some(ViewerTool::Masks) {
            self.mask_ui.disarm();
        }
        if self.prev_tool == Some(ViewerTool::Crop) && self.tool != Some(ViewerTool::Crop) {
            let next = self.tool;
            self.leave_crop(true);
            self.tool = next;
        }
        if self.tool == Some(ViewerTool::Crop)
            && let Some(doc) = &self.doc
        {
            self.geometry_ui.start_crop(&doc.recipe().geometry);
        }
        if let Some(doc) = &self.doc {
            self.plan.reset(doc.generation());
        }
        self.prev_tool = self.tool;
    }

    /// A click with a picker of the settings panel at the upright point `at`
    /// (0–1): the colour entering its node there sets its values (one undo
    /// step). The picker stops after one click.
    fn apply_pick(&mut self, pick: Pick, at: [f32; 2]) {
        self.pick = None;
        let Some(doc) = &mut self.doc else { return };
        let Some(rgb) = doc.probe(&self.registry, pick.node, at) else { return };
        let Some(values) = editor::adjust::pick_values(doc, &self.registry, pick, rgb) else {
            self.toast =
                Some(info_toast("Nothing to pick there: the colour is black, or a grey with no hue.".to_owned()));
            return;
        };
        if let Err(e) = doc.set_params(&self.registry, pick.node, values, None) {
            self.toast = Some(error_toast(format!("That change was refused: {e}")));
        }
    }

    /// The tool keys (C, K, L, M, A, H, S) switch a tool on and off; Esc
    /// stops a picker, then leaves the tool. Not while typing, nor without a
    /// photo.
    fn tool_keys(&mut self, ctx: &egui::Context) {
        if self.doc.is_none() {
            self.tool = None;
            self.pick = None;
            return;
        }
        if self.pick.is_some_and(|p| Some(p.node) != self.selected_node) {
            self.pick = None;
        }
        if ctx.egui_wants_keyboard_input() || ctx.any_popup_open() {
            return;
        }
        if self.pick.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.pick = None;
            return;
        }
        // In the crop tool Enter applies the box and Esc drops it.
        if self.tool == Some(ViewerTool::Crop) {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                self.leave_crop(true);
                return;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.leave_crop(false);
                return;
            }
        }
        // Delete removes the selected node; Heal and Clone keep the key for their spots.
        if !matches!(self.tool, Some(ViewerTool::Heal | ViewerTool::Clone))
            && let Some((id, _)) = self.editable_node()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
        {
            self.apply_graph(vec![GraphAction::Remove(id)]);
            if self.selected_node.is_none() {
                let undo = ctx.format_shortcut(&Command::Undo.shortcuts()[0]);
                self.toast = Some(info_toast(format!("Node deleted. Undo with {undo}")));
            }
            return;
        }
        for &tool in TOOL_GROUPS.iter().flat_map(|g| g.iter()) {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, tool.key())) {
                self.tool = if self.tool == Some(tool) { None } else { Some(tool) };
            }
        }
        if self.tool.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.tool = None;
        }
    }

    /// The exposure warning for the image on screen, made once per render.
    fn exposure_warning_texture(&mut self, ctx: &egui::Context) -> Option<&egui::TextureHandle> {
        if self.warning_texture.is_none() {
            let shown = self.shown.as_ref()?;
            let rgba = editor::clipping::overlay(
                &shown.pixels,
                self.settings.warning.thresholds(),
                theme::WARNING_HIGHLIGHT,
                theme::WARNING_SHADOW,
            );
            let size = [shown.width as usize, shown.height as usize];
            let image = egui::ColorImage::from_rgba_premultiplied(size, &rgba);
            self.warning_texture = Some(ctx.load_texture("exposure-warning", image, TextureOptions::NEAREST));
        }
        self.warning_texture.as_ref()
    }

    /// Once the open photo is on screen, opens the next one in the
    /// direction of travel ahead of time, so moving to it is immediate.
    fn prefetch_neighbour(&mut self) {
        let Some(step) = self.prefetch_step.take() else { return };
        let (Some(worker), Some(doc)) = (&self.worker, &self.doc) else { return };
        if self.library.strip.index_of(doc.path()).is_none() {
            return; // not in the filmstrip: there is no next photo
        }
        if let Some(next) = self.library.strip.neighbour(Some(doc.path()), step) {
            worker.prefetch(next.to_owned());
        }
    }

    /// The selected node and whether it is on, when it is one that can be
    /// bypassed or deleted (a tool or a mask, not the Source or the Output).
    fn editable_node(&self) -> Option<(NodeId, bool)> {
        let id = self.selected_node?;
        let node = self.doc.as_ref()?.recipe().graph.node(id)?;
        matches!(node.kind, NodeKind::Tool(_) | NodeKind::Mask).then_some((id, node.enabled))
    }

    /// Applies what the user did in the node graph.
    fn apply_graph(&mut self, actions: Vec<GraphAction>) {
        for action in actions {
            let result = match action {
                GraphAction::Select(id) => {
                    self.selected_node = Some(id);
                    Ok(())
                }
                GraphAction::Deselect => {
                    self.selected_node = None;
                    Ok(())
                }
                GraphAction::Add { after, tool } => {
                    let Some(doc) = &mut self.doc else { continue };
                    let feature = ai_models::feature_for_tool(tool.as_str());
                    let first = editor::adjust::initial_params(doc, tool.as_str());
                    match doc.add_node_with(&self.registry, after, tool, first) {
                        Ok(id) => {
                            self.selected_node = Some(id);
                            // A tool that needs AI models offers to download them.
                            if let Some(feature) = feature {
                                self.models.require(feature);
                            }
                            Ok(())
                        }
                        Err(e) => Err(format!("The node could not be added: {e}")),
                    }
                }
                GraphAction::Remove(id) => {
                    let Some(doc) = &mut self.doc else { continue };
                    if self.selected_node == Some(id) {
                        self.selected_node = None;
                    }
                    doc.remove_node(&self.registry, id).map_err(|e| format!("The node could not be deleted: {e}"))
                }
                GraphAction::Enable(node, on) => {
                    self.apply_edits(vec![UiEdit::Enabled { node, on }]);
                    Ok(())
                }
            };
            if let Err(message) = result {
                self.toast = Some(error_toast(message));
            }
        }
    }
}

impl eframe::App for EditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t = Tokens::of(ctx.theme());
        self.handle_events(&ctx);
        self.handle_dropped_files(&ctx);
        self.poll_tagging();
        self.poll_ai_mask(&ctx);
        let mut commands = Self::consume_shortcuts(&ctx);
        let mut edits = Vec::new();
        let mut library = Vec::new();
        let mut tabs = Vec::new();
        let current = self.doc.as_ref().map(|d| d.path().to_owned());
        // Straighten turns the photo with the arrows instead of moving through the filmstrip.
        if self.tool == Some(ViewerTool::Level) {
            crate::color_ui::take_arrows(&ctx);
        }
        self.library.keyboard(&ctx, current.as_deref(), &mut library);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut commands));
        self.tool_keys(&ctx);
        let bar = BarState {
            has_doc: self.doc.is_some(),
            undo: self.doc.as_ref().and_then(Document::undo_label).map(|l| lowercase_first(&l)),
            redo: self.doc.as_ref().and_then(Document::redo_label).map(|l| lowercase_first(&l)),
            exporting: self.exporting.is_some(),
            export_count: self.export_targets().len(),
            active: self.tool,
            exposure_warning: self.settings.exposure_warning,
        };
        let bar_actions = egui::Panel::top("toolbar")
            .frame(Frame::NONE.fill(t.bar).inner_margin(Margin::symmetric(8, 4)))
            .show(ui, |ui| toolbar::toolbar(ui, &bar))
            .inner;
        for action in bar_actions {
            match action {
                BarAction::Command(c) => commands.push(c),
                BarAction::Tool(tool) => self.tool = tool,
                BarAction::ExposureWarning(on) => self.settings.exposure_warning = on,
            }
        }
        self.tool_changed();
        if matches!(self.tool, Some(ViewerTool::Masks | ViewerTool::AiSelect)) {
            self.mask_ui.keys(&ctx);
        }
        let mut switch_variant = None;
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui, &mut switch_variant));
        egui::Panel::left("library")
            .resizable(true)
            .default_size(LEFT_WIDTH[0])
            .size_range(LEFT_WIDTH[1]..=LEFT_WIDTH[2])
            .frame(Frame::NONE.fill(t.bg))
            .show(ui, |ui| self.left_panel(ui, &mut library, &mut tabs));
        let blocks = self.variant_blocks();
        // The filmstrip, right of the Explorer: drag its right edge to widen
        // it; the thumbnails grow, then line up in columns.
        let max_strip = (ctx.content_rect().width() * 0.75).max(FILMSTRIP_WIDTH);
        egui::Panel::left("filmstrip")
            .resizable(true)
            .default_size(FILMSTRIP_WIDTH)
            .size_range(FILMSTRIP_WIDTH..=max_strip)
            .frame(Frame::NONE.fill(t.bar).inner_margin(Margin::symmetric(6, 6)))
            .show(ui, |ui| {
                let current = self.doc.as_ref().map(|d| d.path().to_owned());
                self.library.filmstrip(ui, current.as_deref(), blocks, &mut library);
            });
        let (scope, scope_height) = egui::Panel::right("inspector")
            .resizable(true)
            .default_size(INSPECTOR_WIDTH[0])
            .size_range(INSPECTOR_WIDTH[1]..=INSPECTOR_WIDTH[2])
            .frame(Frame::NONE.fill(t.bg).inner_margin(Margin::symmetric(10, 4)))
            .show(ui, |ui| self.right_panel(ui, &mut edits))
            .inner;
        if scope != self.settings.scope {
            self.settings.scope = scope;
        }
        if let Some(h) = scope_height {
            // The default height is stored as "none", so a later default change applies.
            self.settings.scope_height = (h != HISTOGRAM_HEIGHT[0]).then_some(h);
        }
        self.scopes.update(&ctx, self.settings.scope, self.histogram.as_deref());
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.canvas))
            .show(ui, |ui| self.center(ui, &mut commands, &mut library));
        self.library.rename_dialog(&ctx, &mut library);
        let presets_folder = self.settings.presets_folder();
        if self.prefs.show(&ctx, &mut self.settings) {
            ctx.set_theme(theme_preference(self.settings.theme));
            self.warning_texture = None; // its thresholds may have changed
            if self.settings.presets_folder() != presets_folder {
                self.reload_presets();
            }
        }
        if let Some(preset) = self.presets.show_save(&ctx) {
            self.save_preset(&preset);
        }
        self.delete_dialog(&ctx);
        if let Some(dialog) = &mut self.export_dialog
            && let Some(request) = dialog.show(&ctx)
        {
            self.export_dialog = None;
            if let ExportRequest::Export { photos, settings, folder } = request {
                self.settings.export = settings;
                if folder.is_some() {
                    self.settings.export_dir = folder;
                }
                self.start_export(photos, &ctx);
            }
        }

        self.apply_edits(edits);
        self.apply_tabs(tabs);
        self.apply_library(library, &ctx);
        for command in commands {
            self.run(command, &ctx);
        }
        if let Some(number) = switch_variant {
            self.switch_variant(number);
        }
        self.check_selection();
        // Closing the window: save the open variant while the screen still shows it.
        if ctx.input(|i| i.viewport().close_requested()) {
            if !self.save_before_closing() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        } else {
            self.autosave(&ctx);
        }
        self.library.request_thumbnails(self.worker.as_ref());
        if let Some(dir) = self.library.folder() {
            self.settings.library_dir = Some(dir.to_owned());
        }
        self.show_toast(&ctx);
        if self.models.show(&ctx) {
            // A tool got its models: render again, and say it if it still fails.
            if let Some(doc) = &self.doc {
                self.plan.reset(doc.generation());
            }
            self.node_problem = None;
        }
        self.request_develop();
        self.request_render(&ctx);
        self.update_title(&ctx);
    }

    /// Called every 30 seconds and when the window closes: the settings,
    /// and the open variant when it changed.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.save_edit(false);
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }
}

fn menu_item(
    ui: &mut egui::Ui,
    command: Command,
    label: Option<String>,
    disabled_because: Option<&str>,
    commands: &mut Vec<Command>,
) {
    let shortcut = ui.ctx().format_shortcut(&command.shortcuts()[0]);
    let text = label.unwrap_or_else(|| command.label().to_owned());
    let response = ui
        .add_enabled(disabled_because.is_none(), Button::new(text).shortcut_text(shortcut))
        .on_disabled_hover_text(disabled_because.unwrap_or_default());
    if response.clicked() {
        commands.push(command);
        ui.close();
    }
}

fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| c.to_lowercase().chain(chars).collect())
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// What the user is told when an export ends, with "Show in folder" for the
/// last file written.
fn export_toast(summary: &ExportSummary) -> Toast {
    let written = summary.written.len();
    let photos = |n: usize| if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
    let action = summary.written.last().cloned().map(ToastAction::ShowInFolder);
    let (text, kind) = match (summary.failed.first(), summary.cancelled) {
        (Some(first), _) if written == 0 => (format!("Export failed: {first}"), ToastKind::Error),
        (Some(first), _) => {
            (format!("Exported {}; {} failed: {first}", photos(written), summary.failed.len()), ToastKind::Error)
        }
        (None, true) => (format!("Export stopped after {}", photos(written)), ToastKind::Info),
        (None, false) => match summary.written.as_slice() {
            [one] => (format!("Exported {}", file_name(one)), ToastKind::Info),
            _ => (format!("Exported {}", photos(written)), ToastKind::Info),
        },
    };
    let seconds = if kind == ToastKind::Error { TOAST_ERROR_SECONDS } else { TOAST_SECONDS };
    Toast { text, kind, action, deadline: Instant::now() + Duration::from_secs_f32(seconds) }
}

fn error_toast(text: String) -> Toast {
    Toast {
        text,
        kind: ToastKind::Error,
        action: None,
        deadline: Instant::now() + Duration::from_secs_f32(TOAST_ERROR_SECONDS),
    }
}

/// Settings panels shown on the right: the selected node and the ones used before it.
const RECENT_NODES: usize = 3;

/// `recent` (newest first) with `selected` moved or added to the front,
/// at most [`RECENT_NODES`] long.
fn remember_recent(mut recent: Vec<NodeId>, selected: Option<NodeId>) -> Vec<NodeId> {
    if let Some(id) = selected {
        recent.retain(|&n| n != id);
        recent.insert(0, id);
    }
    recent.truncate(RECENT_NODES);
    recent
}

/// The second line of the GPU tooltip: how the open photo was opened.
fn open_timing_text(t: &OpenTiming) -> String {
    let how = if t.ahead { "opened ahead in the background" } else { "opened" };
    format!("\nPhoto {how}: decode {} ms, develop {} ms", t.decode.as_millis(), t.develop.as_millis())
}

fn info_toast(text: String) -> Toast {
    Toast {
        text,
        kind: ToastKind::Info,
        action: None,
        deadline: Instant::now() + Duration::from_secs_f32(TOAST_SECONDS),
    }
}

/// Replaces a renamed file in a list of paths, keeping its place.
fn follow_rename(paths: &mut [PathBuf], from: &Path, to: &Path) {
    for p in paths.iter_mut().filter(|p| p.as_path() == from) {
        to.clone_into(p);
    }
}

fn theme_preference(choice: ThemeChoice) -> egui::ThemePreference {
    match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    }
}

/// The eyedropper: next to the pointer, a large swatch of the colour on
/// screen with its position in the full image, RGB and HSL.
fn eyedropper_hud(
    painter: &egui::Painter,
    t: &Tokens,
    viewer: egui::Rect,
    p: egui::Pos2,
    shown: &engine::Rgb8Image,
    at: [f32; 2],
    full: [f32; 2],
) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // inside the image, checked by `pixel`
    let (x, y) = (at[0].max(0.0) as usize, at[1].max(0.0) as usize);
    let Some(rgb) = editor::inspect::pixel(&shown.pixels, shown.width as usize, x, y) else { return };
    let (hue, sat, light) = editor::inspect::hsl(rgb);
    let size = egui::vec2(288.0, 118.0);
    let min = egui::pos2(
        (p.x + 22.0).clamp(viewer.left() + 8.0, (viewer.right() - size.x - 8.0).max(viewer.left() + 8.0)),
        (p.y + 22.0).clamp(viewer.top() + 8.0, (viewer.bottom() - size.y - 8.0).max(viewer.top() + 8.0)),
    );
    let card = egui::Rect::from_min_size(min, size);
    painter.rect_filled(card, egui::CornerRadius::same(4), t.bg_overlay);
    painter.rect_stroke(
        card,
        egui::CornerRadius::same(4),
        egui::Stroke::new(1.0, t.stroke_strong),
        egui::StrokeKind::Inside,
    );
    let swatch = egui::Rect::from_min_size(min + egui::vec2(14.0, 15.0), egui::vec2(88.0, 88.0));
    painter.rect_filled(swatch, egui::CornerRadius::ZERO, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    painter.rect_stroke(
        swatch,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.5, t.text_strong),
        egui::StrokeKind::Outside,
    );
    let mono = egui::FontId::monospace(FONT[1]);
    let lines = [
        (format!("XY: ({:.0}, {:.0})", full[0], full[1]), t.text_weak),
        (format!("RGB: {:3}, {:3}, {:3}", rgb[0], rgb[1], rgb[2]), t.text_strong),
        (format!("HSL: {hue:3}\u{b0}, {sat:3}%, {light:3}%"), t.text_strong),
    ];
    for (i, (text, color)) in lines.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // three lines
        let y = min.y + 26.0 + i as f32 * 30.0;
        painter.text(egui::pos2(min.x + 114.0, y), Align2::LEFT_CENTER, text, mono.clone(), color);
    }
}

/// The loupe: a magnified circle centred on the pointer, with a crosshair
/// and a badge saying the magnification and the position in the full image.
fn loupe(
    painter: &egui::Painter,
    t: &Tokens,
    viewer: egui::Rect,
    p: egui::Pos2,
    texture: &egui::TextureHandle,
    zoom_percent: f32,
    full: [f32; 2],
) {
    #[allow(clippy::cast_precision_loss)] // a small size
    let side = editor::inspect::LOUPE_SIDE as f32;
    let r = side / 2.0;
    let c = egui::pos2(
        p.x.clamp(viewer.left() + r + 4.0, (viewer.right() - r - 4.0).max(viewer.left() + r + 4.0)),
        p.y.clamp(viewer.top() + r + 4.0, (viewer.bottom() - r - 30.0).max(viewer.top() + r + 4.0)),
    );
    // A disc of the texture: a fan of triangles with matching texture coordinates.
    let mut mesh = egui::Mesh::with_texture(texture.id());
    let vertex = |pos: egui::Pos2| egui::epaint::Vertex {
        pos,
        uv: egui::pos2((pos.x - c.x) / side + 0.5, (pos.y - c.y) / side + 0.5),
        color: Color32::WHITE,
    };
    mesh.vertices.push(vertex(c));
    const SEGMENTS: u32 = 96;
    for k in 0..=SEGMENTS {
        #[allow(clippy::cast_precision_loss)] // segment angles
        let a = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        mesh.vertices.push(vertex(c + egui::vec2(a.cos(), a.sin()) * r));
    }
    for k in 1..=SEGMENTS {
        mesh.add_triangle(0, k, k + 1);
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.circle_stroke(c, r, egui::Stroke::new(2.5, t.stroke_strong));
    let cross = egui::Stroke::new(1.0, t.text_strong);
    painter.line_segment([c - egui::vec2(20.0, 0.0), c + egui::vec2(20.0, 0.0)], cross);
    painter.line_segment([c - egui::vec2(0.0, 20.0), c + egui::vec2(0.0, 20.0)], cross);
    let badge = egui::Rect::from_center_size(c + egui::vec2(0.0, r + 14.0), egui::vec2(200.0, 22.0));
    painter.rect_filled(badge, egui::CornerRadius::same(3), t.bg_overlay);
    painter.rect_stroke(
        badge,
        egui::CornerRadius::same(3),
        egui::Stroke::new(1.0, t.stroke_strong),
        egui::StrokeKind::Inside,
    );
    painter.text(
        badge.center(),
        Align2::CENTER_CENTER,
        format!("4\u{d7} ({zoom_percent:.0}%) [{:.0}, {:.0}]", full[0], full[1]),
        egui::FontId::monospace(FONT[0]),
        t.text_strong,
    );
}

fn to_view_rect(r: egui::Rect) -> editor::Rect {
    editor::Rect { x: r.min.x, y: r.min.y, w: r.width(), h: r.height() }
}

fn to_egui_rect(r: editor::Rect) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(r.x, r.y), egui::vec2(r.w, r.h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_right_panel_keeps_the_last_three_nodes_newest_first() {
        let n = NodeId::from_raw;
        let mut recent = Vec::new();
        for id in [1, 2, 3, 4] {
            recent = remember_recent(recent, Some(n(id)));
        }
        assert_eq!(recent, vec![n(4), n(3), n(2)]);
        // Selecting one already shown moves it to the top, nothing is lost.
        recent = remember_recent(recent, Some(n(2)));
        assert_eq!(recent, vec![n(2), n(4), n(3)]);
        // Nothing selected: the panels stay.
        assert_eq!(remember_recent(recent.clone(), None), recent);
    }

    #[test]
    fn undo_labels_read_naturally_in_menus() {
        assert_eq!(lowercase_first("Change exposure"), "change exposure");
        assert_eq!(lowercase_first(""), "");
    }

    #[test]
    fn a_renamed_file_keeps_its_place_in_the_recent_list() {
        let mut recent = vec![PathBuf::from("a/1.NEF"), PathBuf::from("a/2.NEF")];
        follow_rename(&mut recent, Path::new("a/2.NEF"), Path::new("a/Como.NEF"));
        assert_eq!(recent, vec![PathBuf::from("a/1.NEF"), PathBuf::from("a/Como.NEF")]);
    }

    #[test]
    fn the_export_toast_says_what_happened() {
        let written = vec![PathBuf::from("out/a.jpg"), PathBuf::from("out/b.jpg")];
        let done = ExportSummary { total: 2, written: written.clone(), ..ExportSummary::default() };
        let t = export_toast(&done);
        assert_eq!((t.text.as_str(), t.kind), ("Exported 2 photos", ToastKind::Info));
        assert!(matches!(&t.action, Some(ToastAction::ShowInFolder(p)) if p.as_path() == Path::new("out/b.jpg")));
        let one = ExportSummary { total: 1, written: written[..1].to_vec(), ..ExportSummary::default() };
        assert_eq!(export_toast(&one).text, "Exported a.jpg");
        let failed = ExportSummary { total: 3, written, failed: vec!["c.NEF: broken".into()], cancelled: false };
        let t = export_toast(&failed);
        assert_eq!((t.text.as_str(), t.kind), ("Exported 2 photos; 1 failed: c.NEF: broken", ToastKind::Error));
        let stopped = ExportSummary { total: 3, cancelled: true, ..ExportSummary::default() };
        let t = export_toast(&stopped);
        assert_eq!(t.text, "Export stopped after 0 photos");
        assert!(t.action.is_none());
    }
}
