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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_model::{DevelopSettings, NodeId, NodeKind};
use editor::adjust::Pick;
use editor::{
    Document, Event, Histogram, Quality, RenderPlan, RenderRequest, RenderTiming, ViewState, Viewport, Worker, Zoom,
};
use eframe::egui::{
    self, Align, Align2, Button, Color32, CursorIcon, Frame, Id, Layout, Margin, Order, RichText, Sense, TextureOptions,
};
use engine::{Backend, EngineChoice};
use tool_api::ToolRegistry;

use crate::actions::Command;
use crate::geometry_ui::{GeoEdit, GeometryUi};
use crate::graph_ui::{GraphAction, GraphUi};
use crate::history_ui::{LeftTab, TabAction, Tabs};
use crate::inspector::{self, Drag, UiEdit};
use crate::library_ui::{Library, LibraryAction, TaggingProgress};
use crate::models_ui::ModelsUi;
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
    exporting: Option<Job>,
    /// A development in flight (lens corrections changed): ticket and settings.
    developing: Option<(u64, DevelopSettings)>,
    texture: Option<egui::TextureHandle>,
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
            developing: None,
            texture: None,
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
            graph: GraphUi::default(),
            tool: None,
            pick: None,
            prev_tool: None,
            geometry_ui: GeometryUi::default(),
            geometry_gesture: None,
            probe: None,
            histogram: None,
            scopes: Scopes::default(),
            shown: None,
            loupe: None,
            tagging: None,
            last_render: None,
        };
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

    fn export(&mut self, ctx: &egui::Context) {
        let (Some(doc), Some(worker)) = (&self.doc, &self.worker) else { return };
        if self.exporting.is_some() {
            return;
        }
        if doc.needs_develop() {
            // The shown image is about to change: export what the recipe says, not the old pixels.
            self.toast =
                Some(error_toast("Lens corrections are still being applied; export again in a moment".to_owned()));
            return;
        }
        let wanted = doc.export_path(&self.settings.export_folder(doc.path()));
        worker.export(
            doc.recipe().graph.clone(),
            Arc::clone(doc.developed()),
            doc.orientation(),
            doc.recipe().geometry,
            wanted,
        );
        self.exporting = Some(Job { ticket: 0, name: doc.file_name(), started: Instant::now() });
        ctx.request_repaint();
    }

    fn export_to(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Export to folder");
        if let Some(dir) = self.settings.export_dir.as_ref().or(self.settings.open_dir.as_ref()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(folder) = dialog.pick_folder() {
            self.settings.export_dir = Some(folder);
            self.export(ctx);
        }
    }

    fn run(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::Open => self.pick_and_open(),
            Command::Export => self.export(ctx),
            Command::ExportTo => self.export_to(ctx),
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
                Event::Opened { ticket, result } => {
                    if self.opening.as_ref().is_none_or(|j| j.ticket != ticket) {
                        continue; // a newer open replaced this one
                    }
                    self.opening = None;
                    match result {
                        Ok(opened) => {
                            let path = opened.path.clone();
                            let first = self.doc.as_ref().map_or(1, |d| d.generation() + 1);
                            let doc = Document::new(*opened, first);
                            self.settings.opened(&path);
                            self.plan.reset(first);
                            self.view.fit();
                            self.texture = None;
                            self.histogram = None;
                            self.drag = None;
                            // A new photo starts from the Source alone: show its settings.
                            self.selected_node = Some(doc.recipe().graph.source());
                            self.graph.reset();
                            self.doc = Some(doc);
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
                Event::Exported { result } => {
                    self.exporting = None;
                    self.toast = Some(match result {
                        Ok(path) => Toast {
                            text: format!("Exported {}", file_name(&path)),
                            kind: ToastKind::Info,
                            action: Some(ToastAction::ShowInFolder(path)),
                            deadline: Instant::now() + Duration::from_secs_f32(TOAST_SECONDS),
                        },
                        Err(message) => error_toast(format!("Export failed: {message}")),
                    });
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
        let table = Command::shortcut_table();
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
                LibraryAction::Export => self.export(ctx),
                LibraryAction::ExportTo => self.export_to(ctx),
                LibraryAction::Renamed { from, to } => {
                    if let Some(doc) = &mut self.doc
                        && doc.path() == from
                    {
                        doc.file_renamed(to.clone());
                    }
                    follow_rename(&mut self.settings.recent, &from, &to);
                }
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

    // ----- drawing -----

    fn menu_bar(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let has_doc = self.doc.is_some();
        let undo = self.doc.as_ref().and_then(Document::undo_label);
        let redo = self.doc.as_ref().and_then(Document::redo_label);
        let exporting = self.exporting.is_some();
        let mut open_recent = None;
        let mut theme_choice = self.settings.theme;
        let mut open_models = false;
        let mut open_folder = false;
        let mut graph_actions = Vec::new();
        // The selected node, when it is one that can be bypassed or deleted
        // (the same commands as its right-click menu in the graph).
        let editable_node = self.selected_node.and_then(|id| {
            let node = self.doc.as_ref()?.recipe().graph.node(id)?;
            matches!(node.kind, NodeKind::Tool(_) | NodeKind::Mask).then_some((id, node.enabled))
        });

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
                let why =
                    if has_doc { exporting.then_some("An export is running") } else { Some("Open an image first") };
                menu_item(ui, Command::Export, None, why, commands);
                menu_item(ui, Command::ExportTo, None, why, commands);
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
                    .add_enabled(editable_node.is_some(), Button::new("Delete node"))
                    .on_disabled_hover_text(why)
                    .clicked()
                    && let Some((id, _)) = editable_node
                {
                    graph_actions.push(GraphAction::Remove(id));
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                let why = (!has_doc).then_some("Open an image first");
                for c in [Command::ZoomFit, Command::Zoom100, Command::ZoomIn, Command::ZoomOut] {
                    menu_item(ui, c, None, why, commands);
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
        if theme_choice != self.settings.theme {
            self.settings.theme = theme_choice;
            ui.ctx().set_theme(theme_preference(theme_choice));
        }
        if let Some(path) = open_recent {
            self.open(path);
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
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
                            label.on_hover_text(format!(
                                "Last {kind} ({w} \u{d7} {h}): render {} ms, display {} ms",
                                timing.render.as_millis(),
                                timing.display.as_millis()
                            ));
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
                for (job, verb) in [(&self.opening, "Opening"), (&self.exporting, "Exporting")] {
                    if let Some(job) = job {
                        ui.separator();
                        ui.label(format!("{verb} {}\u{2026}", job.name));
                        if job.started.elapsed().as_secs_f32() > SPINNER_DELAY_SECONDS {
                            ui.add(egui::Spinner::new());
                        } else {
                            ui.ctx().request_repaint_after(Duration::from_secs_f32(SPINNER_DELAY_SECONDS));
                        }
                    }
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

        let mut picked = None;
        let mut geo_edits = Vec::new();
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
                    _ => {}
                }
                // The left button pans; the right one is the eyedropper, the middle one the loupe.
                if !tool_drag && response.dragged_by(egui::PointerButton::Primary) {
                    let d = response.drag_delta();
                    self.view.pan([d.x, d.y], image, &vp);
                }
                if response.double_clicked() && self.tool != Some(ViewerTool::Crop) {
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
                }
                _ => {}
            }
        }
        if crop_apply {
            self.leave_crop(true);
        }
        self.apply_geometry(geo_edits);

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
        egui::Panel::right("filmstrip")
            .resizable(false)
            .exact_size(FILMSTRIP_WIDTH)
            .frame(Frame::NONE.fill(t.bar).inner_margin(Margin::symmetric(6, 6)))
            .show(ui, |ui| {
                let current = self.doc.as_ref().map(|d| d.path().to_owned());
                self.library.filmstrip(ui, current.as_deref(), library);
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
    fn right_panel(&self, ui: &mut egui::Ui, edits: &mut Vec<UiEdit>) -> ScopeKind {
        let t = Tokens::of(ui.ctx().theme());
        let scope = self.scopes.show(ui, self.settings.scope, self.histogram.as_deref(), HISTOGRAM_HEIGHT);
        ui.add_space(SPACE[1]);
        ui.separator();
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
                    let folder = self.settings.export_folder(doc.path());
                    let cx = inspector::Context {
                        registry: &self.registry,
                        selected: self.selected_node,
                        developing: self.developing.is_some(),
                        export_folder: &folder,
                        histogram: self.histogram.as_deref(),
                        pick: self.pick,
                        probe: self.probe,
                    };
                    inspector::settings(ui, doc, cx, edits);
                }
            }
        });
        scope
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
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.canvas))
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

    /// Reacts to the viewer tool changing: entering the crop tool starts
    /// the box at the recipe's crop, leaving it (for another tool) applies
    /// the box; the photo is shown again with or without its crop.
    fn tool_changed(&mut self) {
        if self.tool == self.prev_tool {
            return;
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
            self.toast = Some(info_toast("Nothing to pick there: the colour is black.".to_owned()));
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
        for &tool in TOOL_GROUPS.iter().flat_map(|g| g.iter()) {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, tool.key())) {
                self.tool = if self.tool == Some(tool) { None } else { Some(tool) };
            }
        }
        if self.tool.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.tool = None;
        }
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
        let mut commands = Self::consume_shortcuts(&ctx);
        let mut edits = Vec::new();
        let mut library = Vec::new();
        let mut tabs = Vec::new();
        let current = self.doc.as_ref().map(|d| d.path().to_owned());
        self.library.keyboard(&ctx, current.as_deref(), &mut library);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut commands));
        self.tool_keys(&ctx);
        let bar = BarState {
            has_doc: self.doc.is_some(),
            undo: self.doc.as_ref().and_then(Document::undo_label).map(|l| lowercase_first(&l)),
            redo: self.doc.as_ref().and_then(Document::redo_label).map(|l| lowercase_first(&l)),
            exporting: self.exporting.is_some(),
            active: self.tool,
        };
        let bar_actions = egui::Panel::top("toolbar")
            .frame(Frame::NONE.fill(t.bar).inner_margin(Margin::symmetric(8, 4)))
            .show(ui, |ui| toolbar::toolbar(ui, &bar))
            .inner;
        for action in bar_actions {
            match action {
                BarAction::Command(c) => commands.push(c),
                BarAction::Tool(tool) => self.tool = tool,
            }
        }
        self.tool_changed();
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("library")
            .resizable(true)
            .default_size(LEFT_WIDTH[0])
            .size_range(LEFT_WIDTH[1]..=LEFT_WIDTH[2])
            .frame(Frame::NONE.fill(t.bg))
            .show(ui, |ui| self.left_panel(ui, &mut library, &mut tabs));
        let scope = egui::Panel::right("inspector")
            .resizable(true)
            .default_size(INSPECTOR_WIDTH[0])
            .size_range(INSPECTOR_WIDTH[1]..=INSPECTOR_WIDTH[2])
            .frame(Frame::NONE.fill(t.bg).inner_margin(Margin::symmetric(10, 4)))
            .show(ui, |ui| self.right_panel(ui, &mut edits))
            .inner;
        if scope != self.settings.scope {
            self.settings.scope = scope;
        }
        self.scopes.update(&ctx, self.settings.scope, self.histogram.as_deref());
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.canvas))
            .show(ui, |ui| self.center(ui, &mut commands, &mut library));
        self.library.rename_dialog(&ctx, &mut library);

        self.apply_edits(edits);
        self.apply_tabs(tabs);
        self.apply_library(library, &ctx);
        for command in commands {
            self.run(command, &ctx);
        }
        self.check_selection();
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

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
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

fn error_toast(text: String) -> Toast {
    Toast {
        text,
        kind: ToastKind::Error,
        action: None,
        deadline: Instant::now() + Duration::from_secs_f32(TOAST_ERROR_SECONDS),
    }
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
}
