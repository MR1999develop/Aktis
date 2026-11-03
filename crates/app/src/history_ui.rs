//! The tabs under the Explorer and the filmstrip: History (every edit of
//! the open photo; drag a step elsewhere, or tick steps and delete them)
//! and Metadata (what the camera recorded, and the catalog data).
//!
//! Reordering and deleting replay the later edits: a step that no longer
//! applies stays in the list, greyed, with the reason.

use std::collections::BTreeSet;

use catalog::{ColorLabel, MAX_RATING, PhotoMeta, TagSource};
use commands::{EditKind, HistoryItem};
use editor::Document;
use eframe::egui::{
    self, Align, Button, Checkbox, Color32, CornerRadius, FontId, Id, Label, Layout, RichText, Sense, Stroke,
    StrokeKind, TextEdit, vec2,
};

use crate::icons;
use crate::library_ui::label_color;
use crate::theme::{FONT, SPACE, Tokens};

/// Height of a history row.
const ROW_HEIGHT: f32 = 22.0;
/// Room kept on the right of a row for the value ("+0.50").
const DETAIL_WIDTH: f32 = 64.0;

/// Which tab is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftTab {
    /// The edit history.
    #[default]
    History,
    /// Camera and catalog data.
    Metadata,
}

/// What the user did in the tabs.
#[derive(Debug, Clone, PartialEq)]
pub enum TabAction {
    /// Keep the history steps at these positions, in this order.
    Rewrite(Vec<usize>),
    /// Rate the open photo.
    Rate(u8),
    /// Set the colour label of the open photo.
    Label(Option<ColorLabel>),
    /// Add a keyword to the open photo.
    AddTag(String),
    /// Remove a keyword from the open photo.
    RemoveTag(String),
    /// Open the Rename dialog for the open photo.
    Rename,
}

/// State of the tabs.
#[derive(Debug, Default)]
pub struct Tabs {
    /// The tab shown.
    pub tab: LeftTab,
    /// Ticked history steps.
    checked: BTreeSet<usize>,
    /// History length when the ticks were made; they are cleared when it changes.
    len: usize,
    /// Keyword being typed in the Metadata tab.
    new_tag: String,
}

impl Tabs {
    /// Draws the tab bar and the tab.
    pub fn show(&mut self, ui: &mut egui::Ui, doc: Option<&Document>, meta: Option<&PhotoMeta>) -> Vec<TabAction> {
        let t = Tokens::of(ui.ctx().theme());
        let mut actions = Vec::new();
        ui.add_space(SPACE[1]);
        ui.horizontal(|ui| {
            for (tab, name) in [(LeftTab::History, "History"), (LeftTab::Metadata, "Metadata")] {
                if ui.add(Button::selectable(self.tab == tab, RichText::new(name).strong())).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.separator();
        let Some(doc) = doc else {
            ui.label(RichText::new("Open a photo to see its history and metadata").color(t.text_weak));
            return actions;
        };
        match self.tab {
            LeftTab::History => self.history(ui, &t, &doc.history(), &mut actions),
            LeftTab::Metadata => self.metadata(ui, &t, doc, meta, &mut actions),
        }
        actions
    }

    fn history(&mut self, ui: &mut egui::Ui, t: &Tokens, items: &[HistoryItem], actions: &mut Vec<TabAction>) {
        if items.len() != self.len {
            self.checked.clear();
            self.len = items.len();
        }
        let len = items.len();
        let list_height = (ui.available_height() - 34.0).max(ROW_HEIGHT * 2.0);
        egui::ScrollArea::vertical()
            .id_salt("history-list")
            .max_height(list_height)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                // "Original": dropping a step here moves it first.
                let original = ui.horizontal(|ui| {
                    ui.add_space(12.0 + 4.0 + ui.spacing().interact_size.y);
                    ui.label(RichText::new("Original").color(t.text_weak).italics());
                });
                if let Some(from) = original.response.dnd_release_payload::<usize>()
                    && let Some(order) = editor::history_order::moved(len, *from, 0)
                {
                    actions.push(TabAction::Rewrite(order));
                }
                if original.response.dnd_hover_payload::<usize>().is_some() {
                    drop_line(ui, t, original.response.rect.bottom());
                }
                for (i, item) in items.iter().enumerate() {
                    self.row(ui, t, i, item, len, actions);
                }
                if items.is_empty() {
                    ui.label(RichText::new("No edits yet").color(t.text_weak));
                }
            });
        ui.horizontal(|ui| {
            let n = self.checked.len();
            let label = if n == 0 { "Delete".to_owned() } else { format!("Delete {n}") };
            if ui
                .add_enabled(n > 0, Button::new(label))
                .on_hover_text("Remove the ticked steps; the later ones are applied again")
                .on_disabled_hover_text("Tick the steps to delete")
                .clicked()
                && let Some(order) = editor::history_order::without(len, &self.checked)
            {
                actions.push(TabAction::Rewrite(order));
                self.checked.clear();
            }
            if n > 0 && ui.button("Untick all").clicked() {
                self.checked.clear();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let skipped = items.iter().filter(|i| i.skipped.is_some()).count();
                let text = match (len, skipped) {
                    (1, 0) => "1 step".to_owned(),
                    (n, 0) => format!("{n} steps"),
                    (n, s) => format!("{n} steps, {s} skipped"),
                };
                ui.label(RichText::new(text).size(FONT[0]).color(t.text_weak));
            });
        });
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        i: usize,
        item: &HistoryItem,
        len: usize,
        actions: &mut Vec<TabAction>,
    ) {
        let row = ui.horizontal(|ui| {
            ui.set_min_height(ROW_HEIGHT);
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.dnd_drag_source(Id::unique(("aktis-history-step", i)), i, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(12.0, ROW_HEIGHT), Sense::hover());
                icons::grip(ui.painter(), rect, t.text_weak);
            })
            .response
            .on_hover_text("Drag to move this step");
            let mut ticked = self.checked.contains(&i);
            if ui.add(Checkbox::without_text(&mut ticked)).on_hover_text("Tick to delete").changed() {
                if ticked {
                    self.checked.insert(i);
                } else {
                    self.checked.remove(&i);
                }
            }
            let (bar, _) = ui.allocate_exact_size(vec2(3.0, ROW_HEIGHT - 6.0), Sense::hover());
            ui.painter().rect_filled(bar, CornerRadius::same(1), kind_color(t, item.kind));
            let mut text = RichText::new(item.label.as_str());
            if item.skipped.is_some() {
                text = text.color(t.text_weak).strikethrough();
            }
            let width = (ui.available_width() - DETAIL_WIDTH).max(40.0);
            ui.allocate_ui_with_layout(vec2(width, ROW_HEIGHT), Layout::left_to_right(Align::Center), |ui| {
                ui.set_width(width);
                let label = ui.add(Label::new(text).truncate());
                if let Some(why) = &item.skipped {
                    label.on_hover_text(format!("Skipped: {why}"));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if item.skipped.is_some() {
                    ui.label(RichText::new("skipped").size(FONT[0]).color(t.warning));
                } else {
                    ui.label(RichText::new(item.detail.as_str()).monospace().color(t.text_weak));
                }
            });
        });
        let response = row.response;
        let above = ui.ctx().pointer_interact_pos().is_some_and(|p| p.y < response.rect.center().y);
        if response.dnd_hover_payload::<usize>().is_some() {
            drop_line(ui, t, if above { response.rect.top() } else { response.rect.bottom() });
        }
        if let Some(from) = response.dnd_release_payload::<usize>() {
            let before = if above { i } else { i + 1 };
            if let Some(order) = editor::history_order::moved(len, *from, before) {
                actions.push(TabAction::Rewrite(order));
            }
        }
    }

    fn metadata(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        doc: &Document,
        meta: Option<&PhotoMeta>,
        actions: &mut Vec<TabAction>,
    ) {
        let info = doc.info();
        let (w, h) = doc.upright_size();
        let folder = doc.path().parent().map(|p| p.display().to_string()).unwrap_or_default();
        let lens = doc.lens().map(str::to_owned).or_else(|| info.lens.clone());
        let rows: Vec<(&str, Option<String>)> = vec![
            ("File", Some(doc.file_name())),
            ("Folder", Some(folder)),
            ("Camera", Some(doc.camera().to_owned())),
            ("Lens", lens),
            ("Exposure", info.exposure_text()),
            ("Taken", info.captured_text()),
            ("Size", Some(format!("{w} \u{d7} {h}"))),
            ("GPS", info.gps_text()),
            ("Artist", info.artist.clone()),
            ("Copyright", info.copyright.clone()),
        ];
        egui::ScrollArea::vertical().id_salt("metadata").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("metadata-grid").num_columns(2).spacing(vec2(SPACE[2], 4.0)).show(ui, |ui| {
                for (name, value) in rows {
                    let Some(value) = value.filter(|v| !v.is_empty()) else { continue };
                    ui.label(RichText::new(name).color(t.text_weak));
                    ui.add(Label::new(value).truncate());
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button("Rename\u{2026}").on_hover_text("Rename the file (F2)").clicked() {
                    actions.push(TabAction::Rename);
                }
            });
            ui.add_space(SPACE[2]);
            let Some(meta) = meta else {
                ui.label(RichText::new("This photo is not in the filmstrip's folder").color(t.text_weak));
                return;
            };
            ui.label(RichText::new("Rating").color(t.text_weak));
            if let Some(r) = rating_stars(ui, t, meta.rating) {
                actions.push(TabAction::Rate(r));
            }
            ui.add_space(SPACE[1]);
            ui.label(RichText::new("Colour label").color(t.text_weak));
            ui.horizontal(|ui| {
                for label in ColorLabel::ALL {
                    let on = meta.label == Some(label);
                    if label_square(ui, t, on, label_color(label)).on_hover_text(label.name()).clicked() {
                        actions.push(TabAction::Label(if on { None } else { Some(label) }));
                    }
                }
            });
            ui.add_space(SPACE[1]);
            ui.label(RichText::new("Keywords").color(t.text_weak));
            ui.horizontal_wrapped(|ui| {
                for tag in &meta.tags {
                    let prefix = if tag.source == TagSource::Ai { "AI \u{b7} " } else { "" };
                    let text = format!("{prefix}{} \u{d7}", tag.name);
                    let hover = match (tag.source, tag.confidence) {
                        (TagSource::Ai, Some(p)) => {
                            format!("Suggested by the AI ({:.1} %). Click to remove.", p * 100.0)
                        }
                        _ => "Click to remove".to_owned(),
                    };
                    if ui.small_button(text).on_hover_text(hover).clicked() {
                        actions.push(TabAction::RemoveTag(tag.name.clone()));
                    }
                }
                if meta.tags.is_empty() {
                    ui.label(RichText::new("None").color(t.text_weak));
                }
            });
            ui.horizontal(|ui| {
                let edit =
                    ui.add(TextEdit::singleline(&mut self.new_tag).hint_text("Add a keyword").desired_width(160.0));
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.add_enabled(!self.new_tag.trim().is_empty(), Button::new("Add")).clicked() || enter)
                    && !self.new_tag.trim().is_empty()
                {
                    actions.push(TabAction::AddTag(self.new_tag.trim().to_owned()));
                    self.new_tag.clear();
                }
            });
        });
    }
}

/// Colour of the bar of a history row.
fn kind_color(t: &Tokens, kind: EditKind) -> Color32 {
    match kind {
        EditKind::Develop => t.accent,
        EditKind::Adjust => t.adjust_mark,
        EditKind::Layer => t.text_weak,
        EditKind::Mask => t.mask_mark,
    }
}

/// Where a dragged step would land.
fn drop_line(ui: &egui::Ui, t: &Tokens, y: f32) {
    let r = ui.max_rect();
    ui.painter().line_segment([egui::pos2(r.left(), y), egui::pos2(r.right(), y)], Stroke::new(2.0, t.accent));
}

/// Five clickable stars; returns the new rating when one was clicked
/// (the active star clears the rating).
pub fn rating_stars(ui: &mut egui::Ui, t: &Tokens, rating: u8) -> Option<u8> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for i in 1..=MAX_RATING {
            let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            let response = response.on_hover_text(format!("{i} star{}", if i == 1 { "" } else { "s" }));
            if i <= rating {
                icons::star(ui.painter(), rect.center(), 8.0, t.star);
            } else {
                let color = if response.hovered() { t.star } else { t.text_weak };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "\u{b7}",
                    FontId::proportional(FONT[4]),
                    color,
                );
            }
            if response.clicked() {
                clicked = Some(if rating == i { 0 } else { i });
            }
        }
    });
    clicked
}

/// A colour square that shows whether it is the photo's label.
fn label_square(ui: &mut egui::Ui, t: &Tokens, on: bool, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    ui.painter().rect_filled(rect.shrink(4.0), CornerRadius::same(3), color);
    if on || response.hovered() {
        let stroke = Stroke::new(if on { 2.0 } else { 1.0 }, if on { t.text_strong } else { t.stroke_strong });
        ui.painter().rect_stroke(rect.shrink(1.0), CornerRadius::same(5), stroke, StrokeKind::Inside);
    }
    response
}
