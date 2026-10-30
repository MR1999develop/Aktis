//! The Import Images window, as Capture One's: the source on the left
//! (folder, sub-folders, duplicates, a name filter), its photos in the
//! middle (a grid, or a large preview over a strip of thumbnails), the
//! options on the right (where the photos go, backup, preset, metadata).
//! Each photo has a tick; the ticked ones are imported.
//!
//! [`Picker`] holds what the window shows and decides, without egui; the
//! source is read off the interface thread by [`editor::import_scan`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use catalog::{Day, FolderPattern, ShotKey};
use editor::import_scan::{ScanEvent, ScanJob};
use editor::{ThumbCache, ThumbState};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Key, Layout, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    pos2, vec2,
};

use crate::catalog_ui::Outcome;
use crate::dialog::{Choice, dialog_buttons, dialog_with_width};
use crate::icons;
use crate::settings::ImportSettings;
use crate::theme::{FONT, SPACE, Tokens};

/// Width of the source column.
const SOURCE_WIDTH: f32 = 230.0;
/// Width of the options column.
const OPTIONS_WIDTH: f32 = 320.0;
/// Width of a grid cell.
const CELL_WIDTH: f32 = 150.0;
/// Height of the strip under the large preview.
const STRIP_HEIGHT: f32 = 96.0;
/// What the window needs besides the three columns: its margins, the
/// title, the line under the columns and the buttons.
const CHROME_HEIGHT: f32 = 150.0;
/// The columns' height on the smallest screens.
const MIN_BODY: f32 = 200.0;
/// The photos column's width on the smallest screens.
const MIN_MIDDLE: f32 = 180.0;
/// Room left around the window, each way.
const SCREEN_MARGIN: f32 = 48.0;

/// The window's width and height for an app window of `screen_w` × `screen_h`
/// points: 92 % × 86 %, always leaving [`SCREEN_MARGIN`] around it, and
/// never smaller than the three columns need when the screen has room.
fn fitted(screen_w: f32, screen_h: f32) -> (f32, f32) {
    let wanted_w = SOURCE_WIDTH + OPTIONS_WIDTH + 4.0 * CELL_WIDTH + 40.0;
    let wanted_h = CHROME_HEIGHT + 3.0 * STRIP_HEIGHT + 120.0;
    let room_w = (screen_w - SCREEN_MARGIN).max(1.0);
    let room_h = (screen_h - SCREEN_MARGIN).max(1.0);
    ((screen_w * 0.92).max(wanted_w).min(room_w), (screen_h * 0.86).max(wanted_h).min(room_h))
}
/// Thumbnails kept on the GPU.
const THUMB_CACHE: usize = 400;
/// Thumbnails asked for around the current photo in the strip.
const STRIP_AROUND: usize = 30;

/// One photo of the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePhoto {
    /// The file.
    pub path: PathBuf,
    /// Its name.
    pub name: String,
    /// Its capture time once read.
    pub captured: Option<String>,
    /// The capture time was read (or found missing).
    pub read: bool,
    /// The same shot is in the catalog already (file name and capture
    /// time, or the same file).
    pub duplicate: bool,
}

/// What the window shows and which photos it imports.
#[derive(Debug, Clone, Default)]
pub struct Picker {
    photos: Vec<SourcePhoto>,
    unticked: HashSet<PathBuf>,
    known: HashSet<ShotKey>,
    imported: HashSet<PathBuf>,
    /// Shown photos: names holding this text (case apart).
    pub filter: String,
    /// The photo of the large preview.
    pub current: Option<PathBuf>,
}

impl Picker {
    /// A picker that tells duplicates by `known` (shots in the catalog) and
    /// `imported` (files in the catalog).
    #[must_use]
    pub fn new(known: HashSet<ShotKey>, imported: HashSet<PathBuf>) -> Self {
        Self { known, imported, ..Self::default() }
    }

    /// The source's photos, before their capture times are known.
    pub fn listed(&mut self, paths: Vec<PathBuf>) {
        self.photos = paths
            .into_iter()
            .map(|path| {
                let name = editor::library::folder_name(&path);
                let duplicate = self.imported.contains(&path);
                SourcePhoto { path, name, captured: None, read: false, duplicate }
            })
            .collect();
        self.unticked.clear();
        self.current = self.photos.first().map(|p| p.path.clone());
    }

    /// The capture time of a photo was read.
    pub fn captured(&mut self, path: &Path, captured: Option<String>) {
        let Some(p) = self.photos.iter_mut().find(|p| p.path == path) else { return };
        let shot = ShotKey::new(path, captured.as_deref());
        p.duplicate = self.imported.contains(path) || shot.is_some_and(|k| self.known.contains(&k));
        p.captured = captured;
        p.read = true;
    }

    /// Every photo of the source.
    #[must_use]
    pub fn photos(&self) -> &[SourcePhoto] {
        &self.photos
    }

    /// How many capture times are still being read.
    #[must_use]
    pub fn reading(&self) -> usize {
        self.photos.iter().filter(|p| !p.read).count()
    }

    /// How many photos are duplicates (so far).
    #[must_use]
    pub fn duplicates(&self) -> usize {
        self.photos.iter().filter(|p| p.duplicate).count()
    }

    /// The photos shown: those whose name holds the filter, in order.
    #[must_use]
    pub fn shown(&self) -> Vec<&SourcePhoto> {
        let text = self.filter.trim().to_lowercase();
        self.photos.iter().filter(|p| text.is_empty() || p.name.to_lowercase().contains(&text)).collect()
    }

    /// Whether `path` is ticked.
    #[must_use]
    pub fn is_ticked(&self, path: &Path) -> bool {
        !self.unticked.contains(path)
    }

    /// Ticks or unticks `path`.
    pub fn toggle(&mut self, path: &Path) {
        if !self.unticked.remove(path) {
            self.unticked.insert(path.to_owned());
        }
    }

    /// Ticks (or unticks) every photo shown.
    pub fn tick_shown(&mut self, on: bool) {
        let shown: Vec<PathBuf> = self.shown().into_iter().map(|p| p.path.clone()).collect();
        for p in shown {
            if on {
                self.unticked.remove(&p);
            } else {
                self.unticked.insert(p);
            }
        }
    }

    /// Whether `photo` can be imported: ticked, and not a duplicate left out.
    #[must_use]
    pub fn importable(&self, photo: &SourcePhoto, exclude_duplicates: bool) -> bool {
        self.is_ticked(&photo.path) && !(exclude_duplicates && photo.duplicate)
    }

    /// The photos to import: shown, ticked, duplicates left out when
    /// `exclude_duplicates`.
    #[must_use]
    pub fn to_import(&self, exclude_duplicates: bool) -> Vec<PathBuf> {
        self.shown().into_iter().filter(|p| self.importable(p, exclude_duplicates)).map(|p| p.path.clone()).collect()
    }

    /// The photo `step` places after (or before) the current one among
    /// those shown.
    pub fn step(&mut self, step: isize) {
        let shown: Vec<PathBuf> = self.shown().into_iter().map(|p| p.path.clone()).collect();
        let at = self.current.as_ref().and_then(|c| shown.iter().position(|p| p == c)).unwrap_or(0);
        let next = at.saturating_add_signed(step).min(shown.len().saturating_sub(1));
        self.current = shown.get(next).cloned();
    }
}

/// The open Import Images window.
pub struct ImportWindow {
    folder: PathBuf,
    subfolders: bool,
    /// What is shown and ticked.
    pub picker: Picker,
    scan: Option<ScanJob>,
    scanning: bool,
    thumbs: ThumbCache<TextureHandle>,
    wanted: Vec<PathBuf>,
    preview: Option<(PathBuf, ThumbState<TextureHandle>)>,
    asked_preview: Option<PathBuf>,
    problem: Option<String>,
}

impl std::fmt::Debug for ImportWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportWindow").field("folder", &self.folder).finish_non_exhaustive()
    }
}

impl ImportWindow {
    /// The window on `folder` (with its sub-folders when `subfolders`);
    /// duplicates are told by `known` and `imported` (see [`Picker::new`]).
    pub fn new(
        ctx: &egui::Context,
        folder: PathBuf,
        subfolders: bool,
        known: HashSet<ShotKey>,
        imported: HashSet<PathBuf>,
    ) -> Self {
        let mut w = Self {
            folder,
            subfolders,
            picker: Picker::new(known, imported),
            scan: None,
            scanning: false,
            thumbs: ThumbCache::new(THUMB_CACHE),
            wanted: Vec::new(),
            preview: None,
            asked_preview: None,
            problem: None,
        };
        w.rescan(ctx);
        w
    }

    /// The folder the photos come from.
    #[must_use]
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Reads the source again (another folder, or the sub-folder choice changed).
    fn rescan(&mut self, ctx: &egui::Context) {
        let repaint = ctx.clone();
        self.thumbs.clear();
        self.preview = None;
        self.asked_preview = None;
        self.picker.listed(Vec::new());
        match ScanJob::start(self.folder.clone(), self.subfolders, move || repaint.request_repaint()) {
            Ok(job) => {
                self.scan = Some(job);
                self.scanning = true;
                self.problem = None;
            }
            Err(e) => self.problem = Some(format!("The folder cannot be read: {e}")),
        }
    }

    /// Takes in what the scan reported.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(scan) = &self.scan else { return };
        for event in scan.poll() {
            match event {
                ScanEvent::Listed(paths) => self.picker.listed(paths),
                ScanEvent::Captured { path, captured } => self.picker.captured(&path, captured),
                ScanEvent::Done => self.scanning = false,
                ScanEvent::Thumbnail { path, result } => {
                    let state = texture(ctx, &path, result, "import-thumb");
                    self.thumbs.insert(path, state);
                }
                ScanEvent::Preview { path, result } => {
                    let state = texture(ctx, &path, result, "import-preview");
                    self.preview = Some((path, state));
                }
            }
        }
    }

    /// Asks for the thumbnails wanted at this frame and the current preview.
    fn request(&mut self, large: bool) {
        let Some(scan) = &self.scan else { return };
        if let Some(list) = self.thumbs.to_request(&self.wanted) {
            scan.request_thumbnails(list);
        }
        if large
            && let Some(current) = self.picker.current.clone()
            && self.asked_preview.as_ref() != Some(&current)
        {
            scan.request_preview(current.clone());
            self.asked_preview = Some(current);
        }
    }

    /// Draws the window; `options` are the import options (kept for the
    /// next import), `presets` the names offered, `catalog` the catalog's name.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        options: &mut ImportSettings,
        presets: &[String],
        catalog: &str,
    ) -> Outcome {
        self.poll(ctx);
        self.wanted.clear();
        let t = Tokens::of(ctx.theme());
        // As large as the window allows, never larger: on a small screen
        // the columns shrink and scroll instead of going off the edges.
        let screen = ctx.content_rect();
        let (width, height) = fitted(screen.width(), screen.height());
        let mut choice = None;
        let mut rescan = false;
        let mut files = Vec::new();
        dialog_with_width(ctx, "rne-import", width, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Import Images").size(FONT[4]).strong());
                ui.label(RichText::new(format!("into the catalog \u{201c}{catalog}\u{201d}")).color(t.text_weak));
            });
            ui.add_space(SPACE[1]);
            let body = (height - CHROME_HEIGHT).max(MIN_BODY);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(vec2(SOURCE_WIDTH, body), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(SOURCE_WIDTH);
                    egui::ScrollArea::vertical().id_salt("rne-import-source").max_height(body).show(ui, |ui| {
                        rescan = self.source_column(ui, &t, options);
                    });
                });
                ui.separator();
                let middle = (ui.available_width() - OPTIONS_WIDTH - 20.0).max(MIN_MIDDLE);
                ui.allocate_ui_with_layout(vec2(middle, body), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(middle);
                    ui.set_height(body);
                    self.photos_column(ui, &t, options);
                });
                ui.separator();
                ui.allocate_ui_with_layout(vec2(OPTIONS_WIDTH, body), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(OPTIONS_WIDTH);
                    egui::ScrollArea::vertical().id_salt("rne-import-options").max_height(body).show(ui, |ui| {
                        destination_section(ui, &t, options);
                        options_section(ui, &t, options, presets);
                    });
                });
            });
            ui.separator();
            files = options.files(&self.picker.to_import(options.skip_duplicates));
            let n = files.len();
            let problem = self
                .problem
                .clone()
                .or_else(|| if n == 0 { Some("No photos to import".to_owned()) } else { options.problem() });
            if let Some(why) = &problem {
                ui.label(RichText::new(why).color(t.warning));
            }
            let action = if n == 1 { "Import 1 photo".to_owned() } else { format!("Import {n} photos") };
            choice = dialog_buttons(ui, &t, problem.is_none().then_some(action.as_str()), "Cancel", |_| {});
        });
        if rescan {
            self.rescan(ctx);
        }
        self.request(options.large_preview);
        match choice {
            Some(Choice::Action) => Outcome::Import(files),
            Some(Choice::Cancel) => Outcome::Cancelled,
            None => Outcome::Open,
        }
    }

    /// The source: folder, sub-folders, duplicates, the name filter, counts.
    /// Returns whether the source must be read again.
    fn source_column(&mut self, ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings) -> bool {
        let mut rescan = false;
        heading(ui, t, "Import From");
        if let Some(dir) = folder_row(ui, t, Some(&self.folder), "Import images from a folder") {
            self.folder = dir;
            rescan = true;
        }
        if ui.checkbox(&mut self.subfolders, "Include sub-folders").changed() {
            rescan = true;
        }
        ui.checkbox(&mut o.skip_duplicates, "Exclude duplicates")
            .on_hover_text("Leave out photos already in the catalog: the same file name and capture time");
        ui.add_space(SPACE[1]);
        ui.label(RichText::new("Name").size(FONT[0]).color(t.text_weak));
        ui.add(
            egui::TextEdit::singleline(&mut self.picker.filter)
                .hint_text("Filter by name")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(SPACE[1]);
        let total = self.picker.photos().len();
        let shown = self.picker.shown().len();
        let lines = [
            if self.scanning && total == 0 {
                "Reading the folder\u{2026}".to_owned()
            } else if total == 0 {
                "No photos here (RAW, DNG, JPEG, TIFF)".to_owned()
            } else if shown == total {
                count(total)
            } else {
                format!("{} of {}", shown, count(total))
            },
            match (self.picker.duplicates(), self.picker.reading()) {
                (0, 0) => String::new(),
                (d, 0) => format!("{d} already in the catalog"),
                (d, r) => format!("{d} already in the catalog so far ({r} to check)"),
            },
        ];
        for line in lines.iter().filter(|l| !l.is_empty()) {
            ui.label(RichText::new(line).size(FONT[0]).color(t.text_weak));
        }
        ui.add_space(SPACE[1]);
        ui.horizontal(|ui| {
            if ui.small_button("Check All").clicked() {
                self.picker.tick_shown(true);
            }
            if ui.small_button("Uncheck All").clicked() {
                self.picker.tick_shown(false);
            }
        });
        rescan
    }

    /// The photos: a grid, or a large preview over a strip.
    fn photos_column(&mut self, ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut o.large_preview, false, "Grid").on_hover_text("Every photo as a thumbnail");
            ui.selectable_value(&mut o.large_preview, true, "Preview")
                .on_hover_text("One photo large, the others in a strip below (\u{2190} \u{2192} move, Space ticks)");
        });
        ui.add_space(SPACE[0]);
        if o.large_preview {
            self.preview_view(ui, t, o.skip_duplicates);
        } else {
            self.grid_view(ui, t, o);
        }
    }

    fn grid_view(&mut self, ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings) {
        let shown: Vec<SourcePhoto> = self.picker.shown().into_iter().cloned().collect();
        let gap = ui.spacing().item_spacing.x;
        let columns = ((ui.available_width() + gap) / (CELL_WIDTH + gap)).floor().max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few columns
        let columns = columns as usize;
        let cell_h = CELL_WIDTH / 1.5 + 22.0;
        let rows = shown.len().div_ceil(columns);
        let mut open = None;
        egui::ScrollArea::vertical().id_salt("rne-import-grid").auto_shrink([false, false]).show_rows(
            ui,
            cell_h,
            rows,
            |ui, range| {
                let first = range.start * columns;
                let last = (range.end * columns).min(shown.len());
                self.wanted.extend(shown[first..last].iter().map(|p| p.path.clone()));
                for row in range {
                    ui.horizontal(|ui| {
                        for photo in shown.iter().skip(row * columns).take(columns) {
                            if self.cell(ui, t, photo, vec2(CELL_WIDTH, cell_h), o.skip_duplicates) {
                                open = Some(photo.path.clone());
                            }
                        }
                    });
                }
            },
        );
        // A double-click shows the photo large.
        if let Some(path) = open {
            self.picker.current = Some(path);
            o.large_preview = true;
        }
    }

    fn preview_view(&mut self, ui: &mut egui::Ui, t: &Tokens, exclude: bool) {
        if !ui.ctx().egui_wants_keyboard_input() {
            let (left, right, space) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowRight),
                    i.consume_key(egui::Modifiers::NONE, Key::Space),
                )
            });
            if left {
                self.picker.step(-1);
            }
            if right {
                self.picker.step(1);
            }
            if space && let Some(c) = self.picker.current.clone() {
                self.picker.toggle(&c);
            }
        }
        let shown: Vec<SourcePhoto> = self.picker.shown().into_iter().cloned().collect();
        if self.picker.current.as_ref().is_none_or(|c| !shown.iter().any(|p| &p.path == c)) {
            self.picker.current = shown.first().map(|p| p.path.clone());
        }
        let large = (ui.available_height() - STRIP_HEIGHT - SPACE[1]).max(120.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), large), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(4), t.canvas);
        let current = self.picker.current.clone();
        match (&self.preview, &current) {
            (Some((path, ThumbState::Ready(tex))), Some(c)) if path == c => draw_fitted(&painter, rect, tex),
            (Some((path, ThumbState::Failed(why))), Some(c)) if path == c => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    why,
                    FontId::proportional(FONT[1]),
                    t.text_weak,
                );
            }
            (_, Some(_)) => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "\u{2026}",
                    FontId::proportional(FONT[4]),
                    t.text_weak,
                );
            }
            (_, None) => {}
        }
        if let Some(p) = current.as_ref().and_then(|c| shown.iter().find(|p| &p.path == c)) {
            let text = if p.duplicate { format!("{} \u{b7} already in the catalog", p.name) } else { p.name.clone() };
            painter.text(
                rect.left_bottom() + vec2(8.0, -8.0),
                egui::Align2::LEFT_BOTTOM,
                text,
                FontId::proportional(FONT[1]),
                t.text_strong,
            );
        }
        ui.add_space(SPACE[1]);
        let at = current.as_ref().and_then(|c| shown.iter().position(|p| &p.path == c)).unwrap_or(0);
        let around = at.saturating_sub(STRIP_AROUND)..(at + STRIP_AROUND).min(shown.len());
        self.wanted.extend(shown[around].iter().map(|p| p.path.clone()));
        let mut chosen = None;
        egui::ScrollArea::horizontal().id_salt("rne-import-strip").max_height(STRIP_HEIGHT).show(ui, |ui| {
            ui.horizontal(|ui| {
                for photo in &shown {
                    let size = vec2((STRIP_HEIGHT - 8.0) * 1.5, STRIP_HEIGHT - 8.0);
                    let is_current = current.as_ref() == Some(&photo.path);
                    let r = self.small_cell(ui, t, photo, size, is_current, exclude);
                    if r {
                        chosen = Some(photo.path.clone());
                    }
                }
            });
        });
        if let Some(path) = chosen {
            self.picker.current = Some(path);
        }
    }

    /// One grid cell; returns whether it was double-clicked.
    fn cell(&mut self, ui: &mut egui::Ui, t: &Tokens, photo: &SourcePhoto, size: egui::Vec2, exclude: bool) -> bool {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);
        let current = self.picker.current.as_ref() == Some(&photo.path);
        painter.rect_filled(rect, CornerRadius::same(4), if response.hovered() { t.bg_raised } else { t.bg });
        if current {
            painter.rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(4),
                Stroke::new(1.0, t.accent),
                StrokeKind::Inside,
            );
        }
        let frame = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(size.x - 8.0, size.y - 26.0));
        self.thumb(&painter, t, frame, &photo.path);
        let blocked = exclude && photo.duplicate;
        if blocked {
            painter.rect_filled(frame, CornerRadius::same(3), Color32::from_black_alpha(150));
        }
        let name = if photo.duplicate { format!("{} \u{b7} in catalog", photo.name) } else { photo.name.clone() };
        painter.text(
            pos2(frame.left(), frame.bottom() + 4.0),
            egui::Align2::LEFT_TOP,
            name,
            FontId::proportional(FONT[0]),
            if blocked { t.text_weak } else { t.text_strong },
        );
        let tick = Rect::from_min_size(frame.min + vec2(4.0, 4.0), vec2(16.0, 16.0));
        draw_tick(&painter, t, tick, self.picker.importable(photo, exclude), blocked);
        if response.clicked() {
            let on_tick = response.interact_pointer_pos().is_some_and(|p| tick.expand(4.0).contains(p));
            if on_tick && !blocked {
                self.picker.toggle(&photo.path);
            } else {
                self.picker.current = Some(photo.path.clone());
            }
        }
        response.double_clicked()
    }

    /// One thumbnail of the strip; returns whether it was clicked.
    fn small_cell(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        photo: &SourcePhoto,
        size: egui::Vec2,
        current: bool,
        exclude: bool,
    ) -> bool {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(3), t.bg);
        self.thumb(&painter, t, rect.shrink(2.0), &photo.path);
        let blocked = exclude && photo.duplicate;
        if blocked {
            painter.rect_filled(rect, CornerRadius::same(3), Color32::from_black_alpha(150));
        }
        if current {
            painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(2.0, t.accent), StrokeKind::Inside);
        }
        let tick = Rect::from_min_size(rect.min + vec2(3.0, 3.0), vec2(14.0, 14.0));
        draw_tick(&painter, t, tick, self.picker.importable(photo, exclude), blocked);
        if response.clicked() {
            let on_tick = response.interact_pointer_pos().is_some_and(|p| tick.expand(3.0).contains(p));
            if on_tick && !blocked {
                self.picker.toggle(&photo.path);
                return false;
            }
            return true;
        }
        false
    }

    fn thumb(&mut self, painter: &egui::Painter, t: &Tokens, frame: Rect, path: &Path) {
        painter.rect_filled(frame, CornerRadius::same(3), t.canvas);
        match self.thumbs.get(path) {
            Some(ThumbState::Ready(tex)) => draw_fitted(painter, frame, tex),
            Some(ThumbState::Failed(_)) => {
                painter.text(
                    frame.center(),
                    egui::Align2::CENTER_CENTER,
                    "No preview",
                    FontId::proportional(FONT[0]),
                    t.text_weak,
                );
            }
            None => {
                painter.text(
                    frame.center(),
                    egui::Align2::CENTER_CENTER,
                    "\u{2026}",
                    FontId::proportional(FONT[2]),
                    t.text_weak,
                );
            }
        }
    }
}

fn count(n: usize) -> String {
    if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") }
}

fn texture(
    ctx: &egui::Context,
    path: &Path,
    result: Result<Arc<raw_io::Thumbnail>, String>,
    kind: &str,
) -> ThumbState<TextureHandle> {
    match result {
        Ok(t) => {
            let image = egui::ColorImage::from_rgb([t.width as usize, t.height as usize], &t.rgb);
            ThumbState::Ready(ctx.load_texture(
                format!("{kind}:{}", path.display()),
                image,
                crate::photo_texture::smooth(),
            ))
        }
        Err(why) => ThumbState::Failed(why),
    }
}

/// `tex` letterboxed in `frame`.
fn draw_fitted(painter: &egui::Painter, frame: Rect, tex: &TextureHandle) {
    let size = tex.size_vec2();
    let scale = (frame.width() / size.x.max(1.0)).min(frame.height() / size.y.max(1.0));
    let image = Rect::from_center_size(frame.center(), size * scale);
    painter.image(tex.id(), image, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// The tick of a photo: filled with a check when it will be imported.
fn draw_tick(painter: &egui::Painter, t: &Tokens, rect: Rect, on: bool, blocked: bool) {
    let fill = if on { t.accent } else { Color32::from_black_alpha(120) };
    painter.rect_filled(rect, CornerRadius::same(3), fill);
    painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
    if on {
        let a = rect.left_center() + vec2(3.0, 0.0);
        let b = rect.center_bottom() + vec2(-1.0, -4.0);
        let c = rect.right_top() + vec2(-3.0, 4.0);
        painter.line(vec![a, b, c], Stroke::new(2.0, t.on_accent));
    } else if blocked {
        painter.line_segment(
            [rect.left_top() + vec2(4.0, 4.0), rect.right_bottom() - vec2(4.0, 4.0)],
            Stroke::new(1.5, Color32::WHITE),
        );
    }
}

fn heading(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.add_space(SPACE[1]);
    ui.label(RichText::new(title).strong().color(t.text_strong));
}

/// A folder and its Choose… button; returns the folder chosen.
fn folder_row(ui: &mut egui::Ui, t: &Tokens, folder: Option<&Path>, title: &str) -> Option<PathBuf> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        if ui.button("Choose\u{2026}").clicked() {
            let mut pick = rfd::FileDialog::new().set_title(title);
            if let Some(dir) = folder.filter(|d| d.is_dir()) {
                pick = pick.set_directory(dir);
            }
            chosen = pick.pick_folder();
        }
        let shown = folder.map_or_else(|| "No folder chosen".to_owned(), |d| d.display().to_string());
        ui.add(egui::Label::new(RichText::new(shown).color(t.text_weak)).truncate());
    });
    chosen
}

fn destination_section(ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings) {
    heading(ui, t, "Import To");
    ui.radio_value(&mut o.copy, false, "Add to the catalog: the files stay where they are");
    ui.radio_value(&mut o.copy, true, "Copy to a folder");
    if o.copy {
        ui.indent("rne-import-copy", |ui| {
            if let Some(dir) = folder_row(ui, t, o.copy_dir.as_deref(), "Folder for the copies") {
                o.copy_dir = Some(dir);
            }
            ui.horizontal(|ui| {
                ui.label("Sub-folders");
                ui.add(egui::TextEdit::singleline(&mut o.pattern).desired_width(150.0))
                    .on_hover_text("{yyyy} year, {yy} short year, {mm} month, {dd} day; / starts a sub-folder");
                if ui.small_button("Default").clicked() {
                    o.pattern = catalog::import::DEFAULT_PATTERN.to_owned();
                }
            });
            let example = FolderPattern::new(&o.pattern).folder_for(Day::from_exif("2026:10:29"));
            let example = if example.as_os_str().is_empty() {
                "Every copy straight in the folder".to_owned()
            } else {
                format!("A photo of 29 October 2026 goes in {}", example.display())
            };
            ui.label(RichText::new(example).size(FONT[0]).color(t.text_weak));
        });
    }
    ui.checkbox(&mut o.backup, "Back up the originals to a second folder");
    if o.backup {
        ui.indent("rne-import-backup", |ui| {
            if let Some(dir) = folder_row(ui, t, o.backup_dir.as_deref(), "Backup folder") {
                o.backup_dir = Some(dir);
            }
        });
    }
}

fn options_section(ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings, presets: &[String]) {
    heading(ui, t, "Options");
    ui.horizontal(|ui| {
        ui.label("RAW + JPEG");
        ui.radio_value(&mut o.jpeg_twins, true, "Both");
        ui.radio_value(&mut o.jpeg_twins, false, "RAW only");
    })
    .response
    .on_hover_text("A RAW file and a JPEG with the same name in the same folder");
    ui.horizontal(|ui| {
        ui.label("Preset");
        let shown = o.preset.clone().unwrap_or_else(|| "None".to_owned());
        egui::ComboBox::from_id_salt("rne-import-preset").selected_text(shown).width(220.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut o.preset, None, "None");
            for name in presets {
                ui.selectable_value(&mut o.preset, Some(name.clone()), name.as_str());
            }
        });
    });
    heading(ui, t, "Metadata");
    egui::Grid::new("rne-import-metadata").num_columns(2).spacing([SPACE[2], SPACE[1]]).show(ui, |ui| {
        ui.label("Photographer");
        ui.add(egui::TextEdit::singleline(&mut o.artist).desired_width(200.0));
        ui.end_row();
        ui.label("Copyright");
        ui.add(egui::TextEdit::singleline(&mut o.copyright).desired_width(200.0).hint_text("\u{a9} 2026 Your Name"));
        ui.end_row();
        ui.label("Keywords");
        ui.add(egui::TextEdit::singleline(&mut o.keywords).desired_width(200.0).hint_text("sea, holiday"))
            .on_hover_text("Separated by commas");
        ui.end_row();
        ui.label("Rating");
        stars(ui, t, &mut o.rating);
        ui.end_row();
    });
}

/// Five stars drawn as in the filmstrip: a click sets the rating, a click
/// on the rating's own star takes it off.
fn stars(ui: &mut egui::Ui, t: &Tokens, rating: &mut u8) {
    const CELL: f32 = 18.0;
    let (rect, response) = ui.allocate_exact_size(vec2(CELL * f32::from(catalog::MAX_RATING), CELL), Sense::click());
    let painter = ui.painter_at(rect);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–4 after the clamp
    let hovered =
        response.hover_pos().map(|p| ((p.x - rect.left()) / CELL).floor().clamp(0.0, 4.0)).map(|i| i as u8 + 1);
    let shown = hovered.unwrap_or(*rating);
    for i in 0..catalog::MAX_RATING {
        let c = rect.left_center() + vec2(CELL * (f32::from(i) + 0.5), 0.0);
        if i < shown {
            icons::star(&painter, c, 6.5, if hovered.is_some() { t.accent } else { t.star });
        } else {
            icons::dot(&painter, c, 2.0, t.text_weak);
        }
    }
    if response.clicked()
        && let Some(r) = hovered
    {
        *rating = if *rating == r { 0 } else { r };
    }
    let text = if *rating == 0 { "None".to_owned() } else { format!("{rating} of 5") };
    response.on_hover_text(format!("Rating given to every photo: {text}; click the same star to take it off"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_never_leaves_the_screen() {
        for (w, h) in [(1269.0, 951.0), (1024.0, 700.0), (800.0, 500.0), (2560.0, 1440.0), (300.0, 200.0)] {
            let (fw, fh) = fitted(w, h);
            assert!(fw <= w - SCREEN_MARGIN + 1.0 && fh <= h - SCREEN_MARGIN + 1.0, "{w}×{h} → {fw}×{fh}");
            assert!(fw > 0.0 && fh > 0.0);
        }
        // With room, it keeps the size of the earlier window.
        let (fw, fh) = fitted(2560.0, 1440.0);
        assert!((fw - 2560.0 * 0.92).abs() < 0.5 && (fh - 1440.0 * 0.86).abs() < 0.5);
    }

    fn picker() -> Picker {
        let known: HashSet<ShotKey> =
            [ShotKey::new(Path::new("old/a.NEF"), Some("2026:10:29 10:00:00"))].into_iter().flatten().collect();
        let imported: HashSet<PathBuf> = [PathBuf::from("card/c.NEF")].into_iter().collect();
        let mut p = Picker::new(known, imported);
        p.listed(["card/a.NEF", "card/b.NEF", "card/c.NEF", "card/IMG_9.JPG"].iter().map(PathBuf::from).collect());
        p
    }

    #[test]
    fn duplicates_are_told_by_shot_and_by_file() {
        let mut p = picker();
        assert_eq!(p.duplicates(), 1, "c is the same file");
        assert_eq!(p.reading(), 4);
        p.captured(Path::new("card/a.NEF"), Some("2026:10:29 10:00:00".into()));
        p.captured(Path::new("card/b.NEF"), None);
        assert_eq!(p.duplicates(), 2);
        assert_eq!(p.to_import(true), [PathBuf::from("card/b.NEF"), PathBuf::from("card/IMG_9.JPG")]);
        assert_eq!(p.to_import(false).len(), 4, "without the exclusion every ticked photo comes in");
    }

    #[test]
    fn the_name_filter_and_the_ticks_choose_the_photos() {
        let mut p = picker();
        p.filter = "img".into();
        assert_eq!(p.shown().len(), 1);
        assert_eq!(p.to_import(true), [PathBuf::from("card/IMG_9.JPG")]);
        p.tick_shown(false);
        assert!(p.to_import(true).is_empty());
        p.filter.clear();
        p.toggle(Path::new("card/b.NEF"));
        assert_eq!(p.to_import(true), [PathBuf::from("card/a.NEF")]);
        p.tick_shown(true);
        assert_eq!(p.to_import(true).len(), 3);
    }

    #[test]
    fn the_preview_steps_through_the_photos_shown() {
        let mut p = picker();
        assert_eq!(p.current.as_deref(), Some(Path::new("card/a.NEF")));
        p.step(1);
        p.step(5);
        assert_eq!(p.current.as_deref(), Some(Path::new("card/IMG_9.JPG")), "stops at the last");
        p.step(-10);
        assert_eq!(p.current.as_deref(), Some(Path::new("card/a.NEF")));
    }
}
