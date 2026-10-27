//! The main window: menu, viewer, inspector, status bar and toasts.
//!
//! Each frame: read the worker's events, handle dropped files and
//! shortcuts, draw, then apply what the user did (collected as values while
//! drawing) and ask for the next render. Nothing slow runs here: decoding,
//! rendering and writing happen in [`editor::Worker`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use commands::GestureId;
use core_model::{DevelopSettings, LensSettings, NodeId, ParamId, ParamValue};
use editor::{Document, Event, Quality, RenderPlan, RenderRequest, ViewState, Viewport, Worker, Zoom};
use eframe::egui::{
    self, Align, Align2, Button, Color32, CursorIcon, Id, Layout, Margin, Order, RichText, Sense, TextureOptions,
};
use engine::{Backend, EngineChoice};
use tool_api::{ParamKind, ToolRegistry, Unit};

use crate::actions::Command;
use crate::settings::{STORAGE_KEY, Settings, ThemeChoice};
use crate::theme::{
    self, FONT, INSPECTOR_WIDTH, SPACE, SPINNER_DELAY_SECONDS, TOAST_ERROR_SECONDS, TOAST_SECONDS, Tokens,
};
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

/// An ongoing slider drag: its changes form one undo step.
#[derive(Debug, Clone)]
struct Drag {
    node: NodeId,
    param: ParamId,
    gesture: GestureId,
}

/// What the user did in the inspector, applied after drawing.
#[derive(Debug, Clone)]
enum UiEdit {
    BeginDrag { node: NodeId, param: ParamId },
    EndDrag,
    Set { node: NodeId, param: ParamId, value: ParamValue, in_drag: bool },
    Reset { node: NodeId, param: ParamId },
    Enabled { node: NodeId, on: bool },
    Lens(LensSettings),
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
    title: String,
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
            title: String::new(),
        };
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
        worker.export(doc.recipe().graph.clone(), Arc::clone(doc.developed()), doc.orientation(), wanted);
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
                let (w, h) = doc.upright_size();
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
                            match Document::new(*opened, &self.registry, first) {
                                Ok(doc) => {
                                    self.settings.opened(&path);
                                    self.plan.reset(first);
                                    self.view.fit();
                                    self.texture = None;
                                    self.drag = None;
                                    self.doc = Some(doc);
                                }
                                Err(e) => self.toast = Some(error_toast(format!("Cannot edit this image: {e}"))),
                            }
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
                Event::Rendered { generation, quality, image, backend, gpu_failure, node_problems } => {
                    if !self.plan.accepts(generation, quality) {
                        continue;
                    }
                    let size = [image.width as usize, image.height as usize];
                    let color = egui::ColorImage::from_rgb(size, &image.pixels);
                    let options = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
                    match &mut self.texture {
                        Some(t) => t.set(color, options),
                        None => self.texture = Some(ctx.load_texture("photo", color, options)),
                    }
                    self.plan.shown(generation, quality);
                    if let Some(why) = gpu_failure {
                        self.toast = Some(error_toast(format!("The GPU failed, the CPU took over: {why}")));
                    }
                    // Told once per message, not at every render.
                    if let Some(problem) = node_problems.into_iter().next()
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
                UiEdit::Enabled { node, on } => doc.set_enabled(&self.registry, node, on),
                UiEdit::Lens(lens) => doc.set_lens_correction(&self.registry, lens),
            };
            if let Err(e) = result {
                self.toast = Some(error_toast(format!("That change was refused: {e}")));
            }
        }
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
        let Some(quality) = self.plan.next(doc.generation(), busy, doc.preview_is_reduced()) else { return };
        let source = match quality {
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
            ui.menu_button("Help", |ui| {
                ui.label(format!("{APP_NAME} {VERSION}"));
                ui.weak("Non-destructive RAW editor with nodes");
            });
        });

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
            if let Some(doc) = &self.doc {
                ui.label(doc.camera());
                ui.separator();
                let (w, h) = doc.upright_size();
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
                        ui.weak(b.to_string());
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

    fn inspector(&self, ui: &mut egui::Ui, edits: &mut Vec<UiEdit>) {
        let Some(doc) = &self.doc else { return };
        ui.add_space(SPACE[2]);
        self.lens_section(ui, doc, edits);
        ui.add_space(SPACE[4]);
        ui.label(RichText::new("Adjustments").size(FONT[3]).strong());
        ui.add_space(SPACE[1]);
        for row in doc.node_rows(&self.registry) {
            let mut on = row.enabled;
            if ui
                .checkbox(&mut on, RichText::new(row.name).strong())
                .on_hover_text("Turn this adjustment on or off")
                .changed()
            {
                edits.push(UiEdit::Enabled { node: row.node, on });
            }
            // A one-parameter tool named like its parameter needs no second label.
            let hide_label = row.params.len() == 1 && row.params.iter().all(|p| p.descriptor.label == row.name);
            ui.add_enabled_ui(row.enabled, |ui| {
                for p in &row.params {
                    param_widget(ui, row.node, p, !hide_label, edits);
                }
            });
            ui.add_space(SPACE[4]);
        }
    }

    /// Lens corrections (lensfun data built into the editor): on/off, what to
    /// correct, and the lens when the file does not name it correctly.
    fn lens_section(&self, ui: &mut egui::Ui, doc: &Document, edits: &mut Vec<UiEdit>) {
        let current = doc.recipe().develop.lens.clone();
        let mut next = current.clone();
        ui.horizontal(|ui| {
            ui.checkbox(&mut next.enabled, RichText::new("Lens corrections").size(FONT[3]).strong())
                .on_hover_text("Correct distortion, chromatic aberration and vignetting from the lens database");
            if self.developing.is_some() {
                ui.spinner();
            }
        });
        ui.add_space(SPACE[1]);
        ui.add_enabled_ui(current.enabled, |ui| {
            let automatic = match doc.detected_lens() {
                Some(name) => format!("Automatic ({name})"),
                None => "Automatic (lens not recognised)".to_owned(),
            };
            let selected = current.lens.clone().unwrap_or_else(|| automatic.clone());
            egui::ComboBox::from_id_salt("lens-choice")
                .width(ui.available_width())
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut next.lens, None, automatic);
                    for name in doc.lens_choices() {
                        ui.selectable_value(&mut next.lens, Some(name.clone()), name.as_str());
                    }
                })
                .response
                .on_hover_text("The lens whose data is used; Automatic reads it from the file");
            ui.checkbox(&mut next.distortion, "Distortion");
            ui.checkbox(&mut next.tca, "Chromatic aberration");
            ui.checkbox(&mut next.vignetting, "Vignetting");
            let mut auto_zoom = next.zoom.is_none();
            if ui
                .checkbox(&mut auto_zoom, "Fill the frame")
                .on_hover_text("Enlarge slightly so that no empty borders remain after correcting the geometry")
                .changed()
            {
                next.zoom = if auto_zoom { None } else { Some(1.0) };
            }
            for note in doc.develop_notes() {
                ui.label(RichText::new(note).size(FONT[1]).weak());
            }
        });
        if next != current {
            edits.push(UiEdit::Lens(next));
        }
    }

    fn central(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let t = Tokens::of(ui.ctx().theme());
        let rect = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        let vp = Viewport { rect: to_view_rect(rect), pixels_per_point: ppp };
        self.viewport = Some(vp);

        if let Some(doc) = &self.doc {
            let response = ui.allocate_rect(rect, Sense::click_and_drag());
            let (w, h) = doc.upright_size();
            let image = [w as f32, h as f32];

            if response.hovered() {
                let (scroll, pinch, hover) =
                    ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
                let factor = pinch * (scroll * WHEEL_ZOOM_PER_POINT).exp();
                if (factor - 1.0).abs() > 1e-4 {
                    self.view.zoom_by(factor, hover.map(|p| [p.x, p.y]), image, &vp);
                }
            }
            if response.dragged() {
                let d = response.drag_delta();
                self.view.pan([d.x, d.y], image, &vp);
            }
            if response.double_clicked() {
                match self.view.zoom() {
                    Zoom::Fit => {
                        let at = response.interact_pointer_pos().map(|p| [p.x, p.y]);
                        self.view.actual_pixels(at, image, &vp);
                    }
                    Zoom::Pixels { .. } => self.view.fit(),
                }
            }
            if matches!(self.view.zoom(), Zoom::Pixels { .. }) {
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
            let button = Button::new(RichText::new("Open\u{2026}").color(Color32::WHITE)).fill(t.accent);
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

impl eframe::App for EditorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        self.handle_dropped_files(&ctx);
        let mut commands = Self::consume_shortcuts(&ctx);
        let mut edits = Vec::new();

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut commands));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.doc.is_some() {
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(INSPECTOR_WIDTH[0])
                .size_range(INSPECTOR_WIDTH[1]..=INSPECTOR_WIDTH[2])
                .show(ui, |ui| self.inspector(ui, &mut edits));
        }
        let canvas = Tokens::of(ctx.theme()).canvas;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(canvas))
            .show(ui, |ui| self.central(ui, &mut commands));

        self.apply_edits(edits);
        for command in commands {
            self.run(command, &ctx);
        }
        self.show_toast(&ctx);
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

fn param_widget(ui: &mut egui::Ui, node: NodeId, row: &editor::ParamRow, show_label: bool, edits: &mut Vec<UiEdit>) {
    let d = &row.descriptor;
    let param = d.id.clone();
    match (&d.kind, row.value) {
        (ParamKind::Float { min, max, step, unit, .. }, ParamValue::Float(mut v)) => {
            if show_label {
                ui.label(RichText::new(d.label).color(ui.visuals().weak_text_color()));
            }
            let places = decimals(*step);
            let signed = *min < 0.0;
            let response = ui
                .scope(|ui| {
                    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(100.0);
                    ui.add(
                        egui::Slider::new(&mut v, *min..=*max)
                            .step_by(f64::from(*step))
                            .custom_formatter(move |n, _| format_value(n, places, signed))
                            .suffix(unit_suffix(*unit))
                            .update_while_editing(false),
                    )
                })
                .inner
                .on_hover_text("Drag, or click the number to type a value. Right-click to reset.");
            if response.drag_started() {
                edits.push(UiEdit::BeginDrag { node, param: param.clone() });
            }
            if response.changed() {
                let in_drag = response.dragged() || response.drag_started();
                edits.push(UiEdit::Set { node, param: param.clone(), value: ParamValue::Float(v), in_drag });
            }
            if response.drag_stopped() {
                edits.push(UiEdit::EndDrag);
            }
            response.context_menu(|ui| {
                if ui.button("Reset to default").clicked() {
                    edits.push(UiEdit::Reset { node, param: param.clone() });
                    ui.close();
                }
            });
        }
        (ParamKind::Bool { .. }, ParamValue::Bool(mut b)) => {
            if ui.checkbox(&mut b, d.label).changed() {
                edits.push(UiEdit::Set { node, param, value: ParamValue::Bool(b), in_drag: false });
            }
        }
        (ParamKind::Choice { options, .. }, ParamValue::Choice(mut c)) => {
            if show_label {
                ui.label(RichText::new(d.label).color(ui.visuals().weak_text_color()));
            }
            let current = options.get(c as usize).copied().unwrap_or("");
            let before = c;
            egui::ComboBox::from_id_salt((node, d.id.as_str())).selected_text(current).show_ui(ui, |ui| {
                for (i, option) in (0_u32..).zip(options.iter()) {
                    ui.selectable_value(&mut c, i, *option);
                }
            });
            if c != before {
                edits.push(UiEdit::Set { node, param, value: ParamValue::Choice(c), in_drag: false });
            }
        }
        _ => {
            ui.weak(format!("{}: unsupported value", d.label));
        }
    }
    ui.add_space(SPACE[1]);
}

/// Text after a value: " EV", " %", "°", " K".
fn unit_suffix(unit: Unit) -> &'static str {
    match unit {
        Unit::None => "",
        Unit::Stops => " EV",
        Unit::Percent => " %",
        Unit::Degrees => "\u{b0}",
        Unit::Kelvin => " K",
    }
}

/// Decimals shown for a parameter whose step is `step`.
fn decimals(step: f32) -> usize {
    if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else if step >= 0.01 {
        2
    } else {
        3
    }
}

/// A value with `places` decimals; with an explicit sign when the range
/// has negative values ("+0.50", "-1.00"), and never "-0.00".
fn format_value(n: f64, places: usize, signed: bool) -> String {
    let text = format!("{n:.places$}");
    let is_zero = text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.');
    if is_zero {
        text.trim_start_matches('-').to_owned()
    } else if signed && n > 0.0 {
        format!("+{text}")
    } else {
        text
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

fn theme_preference(choice: ThemeChoice) -> egui::ThemePreference {
    match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    }
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
    fn units_and_decimals_follow_the_descriptor() {
        assert_eq!(unit_suffix(Unit::Stops), " EV");
        assert_eq!(unit_suffix(Unit::None), "");
        assert_eq!(decimals(0.01), 2);
        assert_eq!(decimals(1.0), 0);
        assert_eq!(decimals(0.5), 1);
        assert_eq!(decimals(0.001), 3);
    }

    #[test]
    fn values_show_a_sign_when_the_range_has_one_and_never_minus_zero() {
        assert_eq!(format_value(0.5, 2, true), "+0.50");
        assert_eq!(format_value(-1.0, 2, true), "-1.00");
        assert_eq!(format_value(-0.0, 2, true), "0.00");
        assert_eq!(format_value(-0.001, 2, true), "0.00");
        assert_eq!(format_value(42.0, 0, false), "42");
    }

    #[test]
    fn undo_labels_read_naturally_in_menus() {
        assert_eq!(lowercase_first("Change exposure"), "change exposure");
        assert_eq!(lowercase_first(""), "");
    }
}
