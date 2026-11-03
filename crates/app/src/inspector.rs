//! The bodies of the tool panels on the right: the parameters of a tool
//! (with panels of their own for white balance, levels and curve, in
//! [`crate::adjust_ui`], for the colour editor, colour balance, black &
//! white, clarity, dehaze and vignetting, in [`crate::color_ui`], and the
//! list of spots of the retouch tool, in [`crate::retouch_ui`]), and the
//! development panels: base characteristics and lens corrections.
//!
//! A body reports what the user did as [`UiEdit`]s without saying which
//! tool: the panel around it knows ([`crate::tool_panels`]).

use commands::GestureId;
use core_model::{BaseCurve, CustomLens, LensSettings, ParamId, ParamValue, SensorKind, ToolRef};
use editor::adjust::Pick;
use editor::custom_lenses::CustomLenses;
use editor::{Document, Histogram, ToolRow};
use eframe::egui::{self, Align, Layout, RichText};
use tool_api::{ParamDescriptor, ParamKind, ToolRegistry, Unit};

use crate::theme::{FONT, SPACE, Tokens};

/// An ongoing slider drag: its changes form one undo step.
#[derive(Debug, Clone)]
pub struct Drag {
    /// The tool.
    pub at: ToolRef,
    /// The parameter.
    pub param: ParamId,
    /// The gesture its changes join.
    pub gesture: GestureId,
}

/// What the user did in a panel, applied after drawing.
#[derive(Debug, Clone)]
pub enum UiEdit {
    /// A slider drag started.
    BeginDrag {
        /// The parameter.
        param: ParamId,
    },
    /// The drag ended.
    EndDrag,
    /// A new value.
    Set {
        /// The parameter.
        param: ParamId,
        /// The value.
        value: ParamValue,
        /// Part of the current drag.
        in_drag: bool,
    },
    /// Back to the default value.
    Reset {
        /// The parameter.
        param: ParamId,
    },
    /// Every value of the tool back to its default.
    ResetAll,
    /// Turn the tool on or off.
    Enabled {
        /// On.
        on: bool,
    },
    /// New lens-correction settings.
    Lens(LensSettings),
    /// Save the lens corrections as a custom lens (opens the window).
    SaveCustomLens,
    /// Save the tool's values as its defaults for the photo's camera.
    SaveDefaults,
    /// Forget the saved defaults of the tool for the photo's camera.
    ForgetDefaults,
    /// Delete the custom lens with this name (asks first).
    DeleteCustomLens(String),
    /// Another base curve (Base Characteristics).
    Curve(BaseCurve),
    /// Several values of the tool at once (one undo step; while dragging,
    /// merged with the drag started with `key`).
    SetMany {
        /// The parameter the drag was started with.
        key: ParamId,
        /// The values.
        values: Vec<(ParamId, ParamValue)>,
        /// Part of the current drag.
        in_drag: bool,
    },
    /// A white-balance mode was chosen: the editor sets its kelvin and tint.
    WbMode {
        /// The mode.
        mode: tool_adjust::white_balance::Mode,
    },
    /// A picker waits for a click in the viewer (`None`: no picker).
    Pick(Option<Pick>),
}

/// What the panels need besides the document.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The tools of this build.
    pub registry: &'a ToolRegistry,
    /// Lens corrections are being applied.
    pub developing: bool,
    /// Levels of the image on screen, for the levels and curve panels.
    pub histogram: Option<&'a Histogram>,
    /// The picker waiting for a click in the viewer.
    pub pick: Option<&'a Pick>,
    /// The colour entering the tool under the pointer in the viewer, and the tool.
    pub probe: Option<(&'a ToolRef, [f32; 3])>,
    /// The custom lenses saved on this computer.
    pub custom_lenses: &'a CustomLenses,
    /// The tool defaults saved per camera.
    pub camera_defaults: &'a editor::camera_defaults::AllCameraDefaults,
}

/// The body of a tool's panel: its own controls, or a slider, tick or
/// menu per parameter, then Reset all.
pub fn tool_body(ui: &mut egui::Ui, row: &ToolRow, cx: Context<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    if row.params.is_empty() {
        ui.label(RichText::new("This tool is missing from this build.").color(t.text_weak));
        return;
    }
    // A one-parameter tool named like its parameter needs no second label.
    let hide_label = row.params.len() == 1 && row.params.iter().all(|p| p.descriptor.label == row.name);
    let probe = cx.probe.filter(|(at, _)| **at == row.at).map(|(_, rgb)| rgb);
    let pick = cx.pick.filter(|p| p.at == row.at);
    let panel = crate::adjust_ui::PanelContext { histogram: cx.histogram, pick, probe };
    ui.add_enabled_ui(row.enabled, |ui| match row.at.tool.as_str() {
        "white_balance" => crate::adjust_ui::white_balance(ui, row, panel, edits),
        "levels" => crate::adjust_ui::levels(ui, row, panel, edits),
        "curve" => crate::adjust_ui::curve(ui, row, panel, edits),
        "color_editor" => crate::color_ui::color_editor(ui, row, panel, edits),
        "color_balance" => crate::color_ui::color_balance(ui, row, edits),
        "black_white" => crate::color_ui::black_white(ui, row, edits),
        "clarity" => crate::color_ui::clarity(ui, row, edits),
        "dehaze" => crate::color_ui::dehaze(ui, row, panel, edits),
        "vignetting" => crate::color_ui::vignetting(ui, row, edits),
        _ => {
            for p in &row.params {
                param_widget(ui, p, !hide_label, edits);
            }
        }
    });
    ui.add_space(SPACE[1]);
    if ui
        .small_button("Reset all")
        .on_hover_text("Every setting of this tool back to its default (the camera's, when saved)")
        .clicked()
    {
        edits.push(UiEdit::ResetAll);
    }
}

/// The tool's action menu items, as in Capture One: Save as Defaults for
/// the photo's camera, and Reset Defaults when some are saved.
pub fn defaults_items(ui: &mut egui::Ui, camera: &str, saved: bool, edits: &mut Vec<UiEdit>) {
    let save = ui.button(format!("Save as defaults for {camera}")).on_hover_text(
        "New photos of this camera start with this tool and these values, and resetting goes back to them",
    );
    if save.clicked() {
        edits.push(UiEdit::SaveDefaults);
        ui.close();
    }
    let reset = ui
        .add_enabled(saved, egui::Button::new(format!("Reset defaults for {camera}")))
        .on_hover_text("Back to the editor's own defaults for this tool; photos keep their values")
        .on_disabled_hover_text("No defaults are saved for this tool and camera");
    if reset.clicked() {
        edits.push(UiEdit::ForgetDefaults);
        ui.close();
    }
}

/// The Base Characteristics panel: the curve under every adjustment, the
/// demosaic method and the camera profile.
pub fn base_panel(ui: &mut egui::Ui, doc: &Document, edits: &mut Vec<UiEdit>) {
    base_characteristics(ui, doc, edits);
}

/// The Lens Correction panel.
pub fn lens_panel(ui: &mut egui::Ui, doc: &Document, cx: Context<'_>, edits: &mut Vec<UiEdit>) {
    lens_section(ui, doc, cx, edits);
}

/// Lens corrections (lensfun data built into the editor): on/off, what to
/// correct, and the lens when the file does not name it correctly.
/// Base Characteristics, as in Capture One: the curve that gives the photo
/// its basic look, under every adjustment.
fn base_characteristics(ui: &mut egui::Ui, doc: &Document, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let current = doc.recipe().curve;
    ui.horizontal(|ui| {
        ui.label("Curve");
        egui::ComboBox::from_id_salt("aktis-base-curve").selected_text(current.label()).show_ui(ui, |ui| {
            for curve in BaseCurve::ALL {
                if ui.selectable_label(curve == current, curve.label()).on_hover_text(curve_hint(curve)).clicked()
                    && curve != current
                {
                    edits.push(UiEdit::Curve(curve));
                }
            }
        });
    });
    ui.label(RichText::new(curve_hint(current)).size(FONT[0]).color(t.text_weak));
    ui.add_space(SPACE[1]);
    demosaic_line(ui, doc);
    // Not in a horizontal row: a long description must wrap, not widen the panel.
    ui.label(RichText::new(format!("Profile: {}", doc.profile_description())).size(FONT[1]).color(t.text_weak))
        .on_hover_text("The camera profile applied when the photo is developed, before every tool");
    ui.label(
        RichText::new("Developed into linear Rec. 2020; shown and exported in sRGB through the curve above.")
            .size(FONT[0])
            .color(t.text_weak),
    );
}

/// The demosaic method, chosen for the photo's sensor and ISO (nothing for
/// a picture that is already RGB).
fn demosaic_line(ui: &mut egui::Ui, doc: &Document) {
    let high_iso = doc.info().iso.is_some_and(|iso| iso >= develop::LMMSE_FROM_ISO);
    let method = match doc.sensor_kind() {
        Some(SensorKind::Bayer) if high_iso => "LMMSE (high ISO)",
        Some(SensorKind::Bayer) => "RCD",
        Some(SensorKind::XTrans) => "Markesteijn 3-pass + VNG",
        Some(SensorKind::Rgb) | None => return,
    };
    let t = Tokens::of(ui.ctx().theme());
    ui.label(RichText::new(format!("Demosaic: {method}")).size(FONT[1]).color(t.text_weak))
        .on_hover_text(
            "Chosen for the sensor: RCD for Bayer, LMMSE for Bayer from ISO 3200, Markesteijn with VNG in flat areas for X-Trans",
        );
}

/// What a base curve does, in one line.
fn curve_hint(curve: BaseCurve) -> &'static str {
    match curve {
        BaseCurve::Auto | BaseCurve::FilmStandard | BaseCurve::CameraMatch(_) => {
            "The camera's own look for RAW files (a film curve when the file has no picture); JPEG and TIFF files as they are"
        }
        BaseCurve::Linear => "No curve: the picture as it is, as JPEG and TIFF files look",
    }
}

fn lens_section(ui: &mut egui::Ui, doc: &Document, cx: Context<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let current = doc.recipe().develop.lens.clone();
    let mut next = current.clone();
    ui.horizontal(|ui| {
        ui.checkbox(&mut next.enabled, RichText::new("Lens corrections").size(FONT[2]).strong()).on_hover_text(
            "Correct distortion, chromatic aberration and vignetting from the lens database, and purple fringing",
        );
        if cx.developing {
            ui.spinner();
        }
    });
    ui.add_space(SPACE[1]);
    let exif_lens = doc.opened().lens_key.as_deref();
    ui.add_enabled_ui(current.enabled, |ui| {
        let automatic = match doc.detected_lens() {
            Some(name) => format!("Automatic ({name})"),
            None => "Automatic (lens not recognised)".to_owned(),
        };
        // A custom lens in use shows by its name.
        let custom = current.custom.as_deref().and_then(|n| cx.custom_lenses.named(n));
        let selected = match custom {
            Some(c) if c.settings == current => format!("\u{2605} {}", c.name),
            Some(c) => format!("\u{2605} {} (modified)", c.name),
            None => current.lens.clone().unwrap_or_else(|| automatic.clone()),
        };
        let customs = cx.custom_lenses.by_name();
        let custom_name = custom.map(|c| c.name.as_str());
        let database = current.lens.as_deref();
        let menu = LensMenu {
            selected: &selected,
            automatic: &automatic,
            chosen: database,
            custom_chosen: custom_name,
            customs: &customs,
            exif_lens,
        };
        match lens_chooser(ui, doc, &menu) {
            Some(LensPick::Database(lens)) => next = std::mem::take(&mut next).with_database_lens(lens),
            Some(LensPick::Custom(settings)) => next = settings,
            None => {}
        }
        ui.add_space(SPACE[0]);
        let id = egui::Id::unique("aktis-lens-sliders");
        amount_row(ui, id.with("d"), &mut next.distortion, &mut next.amount.distortion, "Distortion");
        amount_row(ui, id.with("t"), &mut next.tca, &mut next.amount.tca, "Chromatic aberration");
        amount_row(ui, id.with("v"), &mut next.vignetting, &mut next.amount.vignetting, "Vignetting");
        manual_row(
            ui,
            id.with("pf"),
            &mut next.purple_fringing,
            0.0..=100.0,
            "Purple fringing",
            "Removes the violet halo along bright edges, giving it the colour around it; also for lenses the \
             database does not know",
        );
        ui.add_space(SPACE[0]);
        ui.label(RichText::new("Manual").size(FONT[1]).strong().color(t.text_weak));
        manual_row(
            ui,
            id.with("md"),
            &mut next.manual.distortion,
            -100.0..=100.0,
            "Distortion",
            "Above 0 straightens lines bowed outwards (barrel), below 0 lines bowed inwards (pincushion); \
             also for lenses the database does not know",
        );
        manual_row(
            ui,
            id.with("mv"),
            &mut next.manual.vignetting,
            -100.0..=100.0,
            "Light falloff",
            "Above 0 brightens the corners (up to one stop), below 0 darkens them",
        );
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
    ui.add_space(SPACE[1]);
    custom_lens_row(ui, &t, &current, cx.custom_lenses, exif_lens, edits);
    if next != current {
        edits.push(UiEdit::Lens(next));
    }
}

/// The custom lens the settings came from (and whether they were changed
/// since), with Save and Delete.
fn custom_lens_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    current: &LensSettings,
    customs: &CustomLenses,
    exif_lens: Option<&str>,
    edits: &mut Vec<UiEdit>,
) {
    let custom = current.custom.as_deref().and_then(|n| customs.named(n));
    if let Some(c) = custom {
        ui.label(RichText::new(format!("Custom lens for {}", c.exif_lens)).size(FONT[1]).color(t.text_weak));
    }
    ui.horizontal(|ui| {
        let why = if exif_lens.is_none() { Some("The file does not name its lens") } else { None };
        let save = ui
            .add_enabled(why.is_none(), egui::Button::new("Save custom lens\u{2026}"))
            .on_hover_text(
                "Keep these lens corrections under a name: photos taken with this lens and never edited start \
                 with them",
            )
            .on_disabled_hover_text(why.unwrap_or_default());
        if save.clicked() {
            edits.push(UiEdit::SaveCustomLens);
        }
        if let Some(c) = custom
            && ui
                .button("Delete\u{2026}")
                .on_hover_text("Delete this custom lens; photos keep their settings")
                .clicked()
        {
            edits.push(UiEdit::DeleteCustomLens(c.name.clone()));
        }
    });
}

/// A correction's tick with its amount (0–200 %), applied when the slider
/// is let go so that the photo is developed once per change.
fn amount_row(ui: &mut egui::Ui, id: egui::Id, on: &mut bool, amount: &mut f32, label: &str) {
    ui.horizontal(|ui| {
        ui.checkbox(on, label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let max = core_model::LensAmounts::MAX * 100.0;
            if let Some(v) = released(ui, id, *amount * 100.0, *on, 0.0..=max, " %") {
                *amount = v / 100.0;
            }
        });
    });
}

/// A correction over `range` (shown as −100 to 100 or 0 to 100 for a
/// stored −1 to 1 or 0 to 1), 0 by default, applied when the slider is let go.
fn manual_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    label: &str,
    hint: &str,
) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(hint);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(v) = released(ui, id, *value * 100.0, true, range, "") {
                *value = v / 100.0;
            }
        });
    });
}

/// [`released`] for a slider that is always enabled, for other panels.
pub(crate) fn released_slider(
    ui: &mut egui::Ui,
    id: egui::Id,
    stored: f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> Option<f32> {
    released(ui, id, stored, true, range, suffix)
}

/// A whole-number slider over `range` starting at `stored`: while it is
/// dragged the value shown is kept under `id`; the new value is returned
/// once the drag ends, or at once for a click or a typed value.
fn released(
    ui: &mut egui::Ui,
    id: egui::Id,
    stored: f32,
    enabled: bool,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) -> Option<f32> {
    let mut shown = ui.ctx().data(|d| d.get_temp::<f32>(id)).unwrap_or(stored);
    let default = if suffix == " %" { 100.0 } else { 0.0 };
    let slider = egui::Slider::new(&mut shown, range).suffix(suffix).step_by(1.0).fixed_decimals(0);
    let response = ui.add_enabled(enabled, slider).on_hover_text("Double-click to reset");
    if double_clicked(ui, &response) {
        ui.ctx().data_mut(|d| d.remove::<f32>(id));
        return Some(default);
    }
    if response.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<f32>(id));
        return Some(shown);
    }
    if response.dragged() {
        ui.ctx().data_mut(|d| d.insert_temp(id, shown));
        return None;
    }
    response.changed().then_some(shown)
}

/// The lens chooser: a button with the chosen lens opening a list of every
/// lens of the database, the ones that fit the camera first and the others
/// by maker, with a search field that filters as you type and a tick to
/// show only the fitting ones; the custom lenses come first, those for the
/// photo's lens (`exif_lens`) at the top. `chosen` is the database lens
/// chosen now (`None` = Automatic).
fn lens_chooser(ui: &mut egui::Ui, doc: &Document, menu: &LensMenu<'_>) -> Option<LensPick> {
    let LensMenu { selected, automatic, chosen, custom_chosen, customs, exif_lens } = *menu;
    let mut pick = None;
    let t = Tokens::of(ui.ctx().theme());
    let search_id = egui::Id::unique("aktis-lens-search");
    let only_id = egui::Id::unique("aktis-lens-compatible-only");
    let mut search: String = ui.ctx().data(|d| d.get_temp(search_id)).unwrap_or_default();
    let mut only: bool = ui.ctx().data(|d| d.get_temp::<bool>(only_id)).unwrap_or(true);
    let popup = egui::ComboBox::from_id_salt("lens-choice")
        .width(ui.available_width())
        .height(420.0)
        .selected_text(selected)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut search)
                    .hint_text("Type the first letters of the name, e.g. Can")
                    .desired_width(f32::INFINITY),
            );
            // Ready to type as soon as the list opens.
            if ui.ctx().memory(|m| m.focused().is_none()) {
                field.request_focus();
            }
            ui.checkbox(&mut only, "Only lenses that fit this camera");
            ui.separator();
            let mut mine: Vec<&&CustomLens> = customs
                .iter()
                .filter(|c| editor::lenses::starts_with(&c.name, &search))
                .filter(|c| !only || exif_lens.is_some_and(|k| c.fits(k)))
                .collect();
            mine.sort_by_key(|c| !exif_lens.is_some_and(|k| c.fits(k)));
            if !mine.is_empty() {
                ui.label(RichText::new("Custom lenses").size(FONT[0]).strong().color(t.text_weak));
                for c in mine {
                    let label = format!("\u{2605} {}", c.name);
                    let is = custom_chosen.is_some_and(|n| n.eq_ignore_ascii_case(&c.name));
                    if ui.selectable_label(is, label).on_hover_text(format!("For {}", c.exif_lens)).clicked() {
                        pick = Some(LensPick::Custom(c.settings.clone()));
                        ui.close();
                    }
                }
                ui.add_space(SPACE[0]);
            }
            if ui.selectable_label(chosen.is_none(), automatic).clicked() {
                pick = Some(LensPick::Database(None));
                ui.close();
            }
            let found = editor::lenses::filter(doc.lens_choices(), &search, only);
            if found.is_empty() {
                ui.label(RichText::new("No lens matches.").color(t.text_weak));
            }
            for (title, lenses) in editor::lenses::groups(&found) {
                ui.add_space(SPACE[0]);
                ui.label(RichText::new(title).size(FONT[0]).strong().color(t.text_weak));
                for lens in lenses {
                    let is = chosen == Some(lens.name.as_str());
                    let response = ui.selectable_label(is, lens.name.as_str());
                    let response = if lens.compatible {
                        response
                    } else {
                        response.on_hover_text(format!("Mount: {}", lens.mounts))
                    };
                    if response.clicked() {
                        pick = Some(LensPick::Database(Some(lens.name.clone())));
                        ui.close();
                    }
                }
            }
        });
    popup.response.on_hover_text("The lens whose data is used; Automatic reads it from the file");
    ui.ctx().data_mut(|d| {
        d.insert_temp(search_id, search);
        d.insert_temp(only_id, only);
    });
    pick
}

/// What the lens chooser shows.
#[derive(Clone, Copy)]
struct LensMenu<'a> {
    /// The text of the closed chooser.
    selected: &'a str,
    /// The entry that reads the lens from the file.
    automatic: &'a str,
    /// The database lens chosen now (`None` = Automatic).
    chosen: Option<&'a str>,
    /// The custom lens in use, by name.
    custom_chosen: Option<&'a str>,
    /// Every custom lens.
    customs: &'a [&'a CustomLens],
    /// The photo's lens as the file names it.
    exif_lens: Option<&'a str>,
}

/// What was chosen in the lens chooser.
enum LensPick {
    /// A lens of the database (`None` = Automatic).
    Database(Option<String>),
    /// A custom lens: all its settings.
    Custom(LensSettings),
}

/// `true` when the primary button was double-clicked on `response`'s
/// widget (sliders sense drags only, so their responses never report it).
pub fn double_clicked(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.hovered() && ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
}

/// A slider for a float parameter, with its edits; returns the slider's
/// response and the width of its track (where it starts on the left).
pub fn float_slider(
    ui: &mut egui::Ui,
    d: &ParamDescriptor,
    mut v: f32,
    edits: &mut Vec<UiEdit>,
) -> (egui::Response, f32) {
    let ParamKind::Float { min, max, step, unit, .. } = d.kind else {
        return (ui.label(""), 0.0);
    };
    let param = d.id.clone();
    let places = decimals(step);
    let signed = min < 0.0;
    let track = (ui.available_width() - 90.0).max(100.0);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = track;
            ui.add(
                egui::Slider::new(&mut v, min..=max)
                    .step_by(f64::from(step))
                    .custom_formatter(move |n, _| format_value(n, places, signed))
                    .suffix(unit_suffix(unit))
                    .update_while_editing(false),
            )
        })
        .inner
        .on_hover_text("Drag, or click the number to type a value. Double-click or right-click to reset.");
    if response.drag_started() {
        edits.push(UiEdit::BeginDrag { param: param.clone() });
    }
    // Only what the user did: a slider drawn with a value off its step
    // rounds it and says it changed, which is not an edit.
    let touched = response.dragged()
        || response.drag_started()
        || response.drag_stopped()
        || response.clicked()
        || response.has_focus()
        || response.lost_focus();
    if response.changed() && touched {
        let in_drag = response.dragged() || response.drag_started();
        edits.push(UiEdit::Set { param: param.clone(), value: ParamValue::Float(v), in_drag });
    }
    if response.drag_stopped() {
        edits.push(UiEdit::EndDrag);
    }
    // As in Capture One, a double-click goes back to the default (the
    // slider itself only senses drags, so the pointer is asked).
    if double_clicked(ui, &response) {
        edits.push(UiEdit::Reset { param: param.clone() });
    }
    response.context_menu(|ui| {
        if ui.button("Reset to default").clicked() {
            edits.push(UiEdit::Reset { param: param.clone() });
            ui.close();
        }
    });
    (response, track)
}

/// The widget of one parameter: a labelled slider, a checkbox or a menu.
pub fn param_widget(ui: &mut egui::Ui, row: &editor::ParamRow, show_label: bool, edits: &mut Vec<UiEdit>) {
    let d = &row.descriptor;
    let param = d.id.clone();
    match (&d.kind, row.value) {
        (ParamKind::Float { .. }, ParamValue::Float(v)) => {
            if show_label {
                ui.label(RichText::new(d.label).color(ui.visuals().weak_text_color()));
            }
            float_slider(ui, d, v, edits);
        }
        (ParamKind::Bool { .. }, ParamValue::Bool(mut b)) => {
            if ui.checkbox(&mut b, d.label).changed() {
                edits.push(UiEdit::Set { param, value: ParamValue::Bool(b), in_drag: false });
            }
        }
        (ParamKind::Choice { options, .. }, ParamValue::Choice(mut c)) => {
            if show_label {
                ui.label(RichText::new(d.label).color(ui.visuals().weak_text_color()));
            }
            let current = options.get(c as usize).copied().unwrap_or("");
            let before = c;
            egui::ComboBox::from_id_salt(d.id.as_str()).selected_text(current).show_ui(ui, |ui| {
                for (i, option) in (0_u32..).zip(options.iter()) {
                    ui.selectable_value(&mut c, i, *option);
                }
            });
            if c != before {
                edits.push(UiEdit::Set { param, value: ParamValue::Choice(c), in_drag: false });
            }
        }
        _ => {
            ui.weak(format!("{}: unsupported value", d.label));
        }
    }
    ui.add_space(SPACE[1]);
}

/// Text after a value: " EV", " %", "°", " K", " px".
fn unit_suffix(unit: Unit) -> &'static str {
    match unit {
        Unit::None => "",
        Unit::Stops => " EV",
        Unit::Percent => " %",
        Unit::Degrees => "\u{b0}",
        Unit::Kelvin => " K",
        Unit::Pixels => " px",
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
}
