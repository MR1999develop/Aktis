//! The export window: file format, size, names, folder, watermark and
//! metadata, for the open photo or the photos selected in the filmstrip.
//!
//! The window edits a copy of the remembered [`ExportSettings`]; Export
//! hands them back to be remembered and used, Cancel drops them.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, TextEdit};
use engine::export;

use crate::dialog::{Choice, dialog_buttons, dialog_with_width};
use crate::settings::{DestinationMode, ExportFormat, ExportSettings, MetadataMode, ResizeMode};
use crate::theme::{FONT, SPACE, Tokens};

/// Width of the export window.
const WIDTH: f32 = 560.0;

/// Labels of the nine watermark positions, row by row from the top left.
const POSITIONS: [&str; 9] =
    ["Top left", "Top", "Top right", "Left", "Centre", "Right", "Bottom left", "Bottom", "Bottom right"];

/// What the live example of the file name is made from: the first photo's
/// name, and its date and camera when known (the open photo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameExample {
    /// File name without extension.
    pub stem: String,
    /// Capture date as cameras write it.
    pub captured: Option<String>,
    /// Camera ("Nikon D3S").
    pub camera: Option<String>,
}

/// What the window asks for when it closes.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportRequest {
    /// Export `photos` with `settings`; `folder` is the folder chosen for
    /// [`DestinationMode::Folder`].
    Export {
        /// The photos, in export order.
        photos: Vec<PathBuf>,
        /// The choices made.
        settings: ExportSettings,
        /// The chosen folder.
        folder: Option<PathBuf>,
    },
    /// Closed without exporting.
    Cancel,
}

/// The export window while it is open.
#[derive(Debug, Clone)]
pub struct ExportDialog {
    photos: Vec<PathBuf>,
    draft: ExportSettings,
    folder: Option<PathBuf>,
    example: NameExample,
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn section(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.add_space(SPACE[2]);
    ui.label(RichText::new(title).size(FONT[2]).strong().color(t.text_strong));
    ui.add_space(SPACE[0]);
}

impl ExportDialog {
    /// The window for `photos` (at least one), starting from `settings`
    /// and the folder chosen last time.
    #[must_use]
    pub fn new(photos: Vec<PathBuf>, settings: &ExportSettings, folder: Option<PathBuf>, example: NameExample) -> Self {
        Self { photos, draft: settings.clone(), folder, example }
    }

    /// The file name the first photo would get.
    #[must_use]
    pub fn example_name(&self) -> String {
        let mut pattern = self.draft.name_pattern.clone();
        // Unknown until each photo is read: shown as placeholders.
        if self.example.captured.is_none() {
            pattern = pattern.replace("{date}", "YYYY-MM-DD");
        }
        if self.example.camera.is_none() {
            pattern = pattern.replace("{camera}", "Camera");
        }
        let options = ExportSettings { name_pattern: pattern, ..self.draft.clone() }.to_options(None);
        let fields = export::NameFields {
            stem: &self.example.stem,
            index: 0,
            captured: self.example.captured.as_deref(),
            camera: self.example.camera.as_deref(),
        };
        format!("{}.{}", options.name.stem(&fields), options.format.kind.extension())
    }

    /// Draws the window; returns what the user decided, if anything.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ExportRequest> {
        let t = Tokens::of(ctx.theme());
        let mut request = None;
        let max_height = (ctx.content_rect().height() - 180.0).max(240.0);
        let n = self.photos.len();
        dialog_with_width(ctx, "rne-export", WIDTH, |ui| {
            let title = match self.photos.as_slice() {
                [one] => format!("Export {}", file_name(one)),
                _ => format!("Export {n} selected photos"),
            };
            ui.label(RichText::new(title).size(FONT[4]).strong());
            egui::ScrollArea::vertical().id_salt("export-options").max_height(max_height).show(ui, |ui| {
                self.format_section(ui, &t);
                self.size_section(ui, &t);
                self.name_section(ui, &t);
                self.folder_section(ui, &t);
                self.watermark_section(ui, &t);
                self.metadata_section(ui, &t);
            });
            ui.add_space(SPACE[2]);
            let verb = if n == 1 { "Export".to_owned() } else { format!("Export {n} photos") };
            match dialog_buttons(ui, &t, Some(verb.as_str()), "Cancel", |_| {}) {
                Some(Choice::Action) => {
                    request = Some(ExportRequest::Export {
                        photos: self.photos.clone(),
                        settings: self.draft.clone(),
                        folder: self.folder.clone(),
                    });
                }
                Some(Choice::Cancel) => request = Some(ExportRequest::Cancel),
                None => {}
            }
        });
        request
    }

    fn format_section(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let d = &mut self.draft;
        section(ui, t, "Format");
        ui.horizontal(|ui| {
            for (format, label) in
                [(ExportFormat::Jpeg, "JPEG"), (ExportFormat::Png, "PNG"), (ExportFormat::Tiff, "TIFF")]
            {
                ui.selectable_value(&mut d.format, format, label);
            }
        });
        if d.format == ExportFormat::Jpeg {
            ui.add(egui::Slider::new(&mut d.jpeg_quality, 1..=100).text("Quality"));
        } else {
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.sixteen_bit, false, "8 bit");
                ui.radio_value(&mut d.sixteen_bit, true, "16 bit").on_hover_text("For further editing; larger files");
            });
        }
        ui.label(RichText::new("Colour space: sRGB, with its profile embedded").color(t.text_weak));
    }

    fn size_section(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let d = &mut self.draft;
        section(ui, t, "Size");
        let label = |m: ResizeMode| match m {
            ResizeMode::Original => "Full size",
            ResizeMode::LongEdge => "Long edge",
            ResizeMode::Width => "Width",
            ResizeMode::Height => "Height",
            ResizeMode::Percent => "Percentage",
        };
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("export-resize").selected_text(label(d.resize)).show_ui(ui, |ui| {
                for m in [
                    ResizeMode::Original,
                    ResizeMode::LongEdge,
                    ResizeMode::Width,
                    ResizeMode::Height,
                    ResizeMode::Percent,
                ] {
                    ui.selectable_value(&mut d.resize, m, label(m));
                }
            });
            match d.resize {
                ResizeMode::Original => {}
                ResizeMode::Percent => {
                    ui.add(egui::DragValue::new(&mut d.size_percent).range(1.0..=400.0).speed(1.0).suffix(" %"));
                }
                _ => {
                    ui.add(egui::DragValue::new(&mut d.size_pixels).range(16..=65_535).speed(10.0).suffix(" px"));
                }
            }
        });
        if d.resize != ResizeMode::Original {
            ui.checkbox(&mut d.upscale, "Allow enlarging")
                .on_hover_text("Off: photos smaller than this stay as they are");
        }
    }

    fn name_section(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "File name");
        let example = self.example_name();
        let several = self.photos.len() > 1;
        let d = &mut self.draft;
        ui.add(TextEdit::singleline(&mut d.name_pattern).hint_text("{name}").desired_width(f32::INFINITY));
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Insert").color(t.text_weak));
            for (token, meaning) in export::NAME_TOKENS {
                if ui.small_button(token).on_hover_text(meaning).clicked() {
                    d.name_pattern.push_str(token);
                }
            }
        });
        if d.name_pattern.contains("{counter}") {
            ui.horizontal(|ui| {
                ui.label("Counter from");
                ui.add(egui::DragValue::new(&mut d.counter_start).range(0..=999_999));
                ui.label("digits");
                ui.add(egui::DragValue::new(&mut d.counter_digits).range(1..=9));
            });
        }
        ui.label(RichText::new(format!("Example: {example}")).color(t.text_weak));
        if several && !d.name_pattern.contains("{name}") && !d.name_pattern.contains("{counter}") {
            ui.label(
                RichText::new("Photos with the same name get (2), (3)\u{2026} added; add {counter} or {name}.")
                    .color(t.warning),
            );
        }
    }

    fn folder_section(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "Folder");
        let d = &mut self.draft;
        ui.radio_value(&mut d.destination, DestinationMode::NextToOriginal, "Next to the original");
        ui.horizontal(|ui| {
            ui.radio_value(&mut d.destination, DestinationMode::Subfolder, "In a sub-folder named");
            ui.add_enabled(
                d.destination == DestinationMode::Subfolder,
                TextEdit::singleline(&mut d.subfolder).desired_width(160.0),
            );
        });
        let mut choose = false;
        ui.horizontal(|ui| {
            let picked = ui.radio_value(&mut d.destination, DestinationMode::Folder, "In this folder:");
            match &self.folder {
                Some(dir) => {
                    ui.label(RichText::new(file_name(dir)).color(t.text_strong))
                        .on_hover_text(dir.display().to_string());
                }
                None => {
                    ui.label(RichText::new("none chosen").color(t.text_weak));
                }
            }
            choose = ui.button("Choose\u{2026}").clicked() || (picked.clicked() && self.folder.is_none());
        });
        if choose {
            let mut picker = rfd::FileDialog::new().set_title("Export to folder");
            if let Some(dir) = &self.folder {
                picker = picker.set_directory(dir);
            }
            match picker.pick_folder() {
                Some(dir) => {
                    self.folder = Some(dir);
                    self.draft.destination = DestinationMode::Folder;
                }
                None if self.folder.is_none() => self.draft.destination = DestinationMode::NextToOriginal,
                None => {}
            }
        }
        ui.label(RichText::new("Existing files are never replaced: a number is added instead.").color(t.text_weak));
    }

    fn watermark_section(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "Watermark");
        let d = &mut self.draft;
        ui.checkbox(&mut d.watermark, "Text watermark");
        if !d.watermark {
            return;
        }
        ui.add(
            TextEdit::singleline(&mut d.watermark_text).hint_text("\u{a9} 2026 Your name").desired_width(f32::INFINITY),
        );
        ui.add(egui::Slider::new(&mut d.watermark_size, 0.5..=20.0).text("Size").suffix(" %"))
            .on_hover_text("Height of the capitals, as a share of the long edge");
        ui.add(egui::Slider::new(&mut d.watermark_opacity, 0.05..=1.0).text("Opacity"));
        ui.add(egui::Slider::new(&mut d.watermark_margin, 0.0..=20.0).text("Margin").suffix(" %"));
        egui::Grid::new("export-watermark-position").num_columns(3).show(ui, |ui| {
            for (i, label) in (0_u8..).zip(POSITIONS) {
                ui.selectable_value(&mut d.watermark_position, i, label);
                if i % 3 == 2 {
                    ui.end_row();
                }
            }
        });
        ui.label(
            RichText::new("Letters, digits, punctuation, \u{a9} and accented letters; other characters show as ?")
                .color(t.text_weak),
        );
    }

    fn metadata_section(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        section(ui, t, "Metadata");
        let d = &mut self.draft;
        let label = |m: MetadataMode| match m {
            MetadataMode::All => "All: camera, lens, exposure, date, GPS, rating, keywords",
            MetadataMode::AllButGps => "All except the GPS position",
            MetadataMode::CopyrightOnly => "Copyright only",
            MetadataMode::None => "None",
        };
        let combo = egui::ComboBox::from_id_salt("export-metadata").width(380.0);
        combo.selected_text(label(d.metadata)).show_ui(ui, |ui| {
            for m in [MetadataMode::All, MetadataMode::AllButGps, MetadataMode::CopyrightOnly, MetadataMode::None] {
                ui.selectable_value(&mut d.metadata, m, label(m));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Copyright");
            ui.add_enabled(
                d.metadata != MetadataMode::None,
                TextEdit::singleline(&mut d.copyright)
                    .hint_text("Empty: the camera's notice")
                    .desired_width(f32::INFINITY),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(pattern: &str, example: NameExample) -> ExportDialog {
        let settings = ExportSettings { name_pattern: pattern.into(), ..ExportSettings::default() };
        ExportDialog::new(vec![PathBuf::from("DSC_0042.NEF")], &settings, None, example)
    }

    #[test]
    fn the_example_name_follows_the_pattern_with_placeholders_for_unknown_values() {
        let unknown = NameExample { stem: "DSC_0042".into(), ..NameExample::default() };
        assert_eq!(dialog("{date}_{name}", unknown.clone()).example_name(), "YYYY-MM-DD_DSC_0042.jpg");
        assert_eq!(dialog("{camera}-{counter}", unknown).example_name(), "Camera-001.jpg");
        let known = NameExample {
            stem: "DSC_0042".into(),
            captured: Some("2026:08:14 18:42:07".into()),
            camera: Some("Nikon D3S".into()),
        };
        assert_eq!(dialog("{date} {camera}", known).example_name(), "2026-08-14 Nikon D3S.jpg");
    }
}
