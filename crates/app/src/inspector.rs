//! The settings of the node selected in the graph, on the right under the
//! histogram: the lens corrections for the Source, the parameters of a
//! tool (with panels of their own for white balance, levels and curve, in
//! [`crate::adjust_ui`]), a note for the other nodes.

use std::path::Path;

use commands::GestureId;
use core_model::{LensSettings, NodeId, NodeKind, ParamId, ParamValue};
use editor::adjust::Pick;
use editor::{Document, Histogram};
use eframe::egui::{self, Align, Layout, RichText};
use tool_api::{ParamDescriptor, ParamKind, ToolRegistry, Unit};

use crate::graph_ui::{node_title, section_title};
use crate::theme::{FONT, SPACE, Tokens};

/// An ongoing slider drag: its changes form one undo step.
#[derive(Debug, Clone)]
pub struct Drag {
    /// The node.
    pub node: NodeId,
    /// The parameter.
    pub param: ParamId,
    /// The gesture its changes join.
    pub gesture: GestureId,
}

/// What the user did in the settings, applied after drawing.
#[derive(Debug, Clone)]
pub enum UiEdit {
    /// A slider drag started.
    BeginDrag {
        /// The node.
        node: NodeId,
        /// The parameter.
        param: ParamId,
    },
    /// The drag ended.
    EndDrag,
    /// A new value.
    Set {
        /// The node.
        node: NodeId,
        /// The parameter.
        param: ParamId,
        /// The value.
        value: ParamValue,
        /// Part of the current drag.
        in_drag: bool,
    },
    /// Back to the default value.
    Reset {
        /// The node.
        node: NodeId,
        /// The parameter.
        param: ParamId,
    },
    /// Turn a node on or off.
    Enabled {
        /// The node.
        node: NodeId,
        /// On.
        on: bool,
    },
    /// New lens-correction settings.
    Lens(LensSettings),
    /// Several values of one node at once (one undo step; while dragging,
    /// merged with the drag started with `key`).
    SetMany {
        /// The node.
        node: NodeId,
        /// The parameter the drag was started with.
        key: ParamId,
        /// The values.
        values: Vec<(ParamId, ParamValue)>,
        /// Part of the current drag.
        in_drag: bool,
    },
    /// A white-balance mode was chosen: the editor sets its kelvin and tint.
    WbMode {
        /// The white-balance node.
        node: NodeId,
        /// The mode.
        mode: tool_adjust::white_balance::Mode,
    },
    /// A picker waits for a click in the viewer (`None`: no picker).
    Pick(Option<Pick>),
}

/// What the settings panel needs besides the document.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The tools of this build.
    pub registry: &'a ToolRegistry,
    /// The node whose settings are shown.
    pub selected: Option<NodeId>,
    /// Lens corrections are being applied.
    pub developing: bool,
    /// Where exports go.
    pub export_folder: &'a Path,
    /// Levels of the image on screen, for the levels and curve panels.
    pub histogram: Option<&'a Histogram>,
    /// The picker waiting for a click in the viewer.
    pub pick: Option<Pick>,
    /// The colour entering the selected node under the pointer in the viewer.
    pub probe: Option<[f32; 3]>,
}

/// The settings of the selected node.
pub fn settings(ui: &mut egui::Ui, doc: &Document, cx: Context<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = cx.selected.and_then(|id| doc.recipe().graph.node(id));
    let Some(node) = node else {
        ui.add_space(SPACE[4]);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No node selected").size(FONT[3]).strong());
            ui.add_space(SPACE[0]);
            ui.label(RichText::new("Select a node in the graph to edit its settings here.").color(t.text_weak));
        });
        return;
    };
    match &node.kind {
        NodeKind::Source => {
            section_title(ui, "Source");
            ui.label(
                RichText::new(
                    "RAW development: demosaicing, camera profile and lens corrections, \
                     with the camera's white balance.",
                )
                .color(t.text_weak),
            );
            ui.label(
                RichText::new("To change the white balance, add a White balance node after the Source.")
                    .size(FONT[1])
                    .color(t.text_weak),
            );
            ui.add_space(SPACE[2]);
            lens_section(ui, doc, cx.developing, edits);
        }
        NodeKind::Tool(_) => {
            let Some(row) = doc.node_rows(cx.registry).into_iter().find(|r| r.node == node.id) else { return };
            ui.horizontal(|ui| {
                ui.label(RichText::new(node_title(node, cx.registry)).size(FONT[3]).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut on = row.enabled;
                    if ui.checkbox(&mut on, "Enabled").on_hover_text("Turn this node on or off").changed() {
                        edits.push(UiEdit::Enabled { node: row.node, on });
                    }
                });
            });
            ui.add_space(SPACE[1]);
            if row.params.is_empty() {
                ui.label(RichText::new("This tool is missing from this build.").color(t.text_weak));
            }
            // A one-parameter tool named like its parameter needs no second label.
            let hide_label = row.params.len() == 1 && row.params.iter().all(|p| p.descriptor.label == row.name);
            let panel = crate::adjust_ui::PanelContext { histogram: cx.histogram, pick: cx.pick, probe: cx.probe };
            let tool = match &node.kind {
                NodeKind::Tool(id) => id.as_str(),
                _ => "",
            };
            ui.add_enabled_ui(row.enabled, |ui| match tool {
                "white_balance" => crate::adjust_ui::white_balance(ui, &row, panel, edits),
                "levels" => crate::adjust_ui::levels(ui, &row, panel, edits),
                "curve" => crate::adjust_ui::curve(ui, &row, panel, edits),
                _ => {
                    for p in &row.params {
                        param_widget(ui, row.node, p, !hide_label, edits);
                    }
                }
            });
            if !row.params.is_empty() {
                ui.add_space(SPACE[1]);
                if ui
                    .small_button("Reset all")
                    .on_hover_text("Every setting of this node back to its default")
                    .clicked()
                {
                    for p in &row.params {
                        edits.push(UiEdit::Reset { node: row.node, param: p.descriptor.id.clone() });
                    }
                }
            }
        }
        NodeKind::Output => {
            section_title(ui, "Output");
            ui.label(
                RichText::new(format!("Exported as JPEG, quality {}.", editor::worker::EXPORT_QUALITY))
                    .color(t.text_weak),
            );
            ui.label(RichText::new(format!("Folder: {}", cx.export_folder.display())).color(t.text_weak));
        }
        NodeKind::Mask => {
            section_title(ui, "Mask");
            ui.label(RichText::new("Masks are drawn with the Masks tool (M), coming soon.").color(t.text_weak));
        }
        NodeKind::Parallel { .. } => {
            section_title(ui, "Parallel");
            ui.label(RichText::new("Adds the effects of its branches.").color(t.text_weak));
        }
    }
}

/// Lens corrections (lensfun data built into the editor): on/off, what to
/// correct, and the lens when the file does not name it correctly.
fn lens_section(ui: &mut egui::Ui, doc: &Document, developing: bool, edits: &mut Vec<UiEdit>) {
    let current = doc.recipe().develop.lens.clone();
    let mut next = current.clone();
    ui.horizontal(|ui| {
        ui.checkbox(&mut next.enabled, RichText::new("Lens corrections").size(FONT[2]).strong())
            .on_hover_text("Correct distortion, chromatic aberration and vignetting from the lens database");
        if developing {
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
        lens_chooser(ui, doc, &selected, &automatic, &mut next.lens);
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

/// The lens chooser: a button with the chosen lens opening a list of every
/// lens of the database, the ones that fit the camera first and the others
/// by maker, with a search field that filters as you type and a tick to
/// show only the fitting ones. Sets `chosen` (`None` = Automatic).
fn lens_chooser(ui: &mut egui::Ui, doc: &Document, selected: &str, automatic: &str, chosen: &mut Option<String>) {
    let t = Tokens::of(ui.ctx().theme());
    let search_id = egui::Id::unique("rne-lens-search");
    let only_id = egui::Id::unique("rne-lens-compatible-only");
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
                    .hint_text("Type to search, e.g. 24-70 2.8")
                    .desired_width(f32::INFINITY),
            );
            // Ready to type as soon as the list opens.
            if ui.ctx().memory(|m| m.focused().is_none()) {
                field.request_focus();
            }
            ui.checkbox(&mut only, "Only lenses that fit this camera");
            ui.separator();
            if ui.selectable_label(chosen.is_none(), automatic).clicked() {
                *chosen = None;
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
                    let is = chosen.as_deref() == Some(lens.name.as_str());
                    let response = ui.selectable_label(is, lens.name.as_str());
                    let response = if lens.compatible {
                        response
                    } else {
                        response.on_hover_text(format!("Mount: {}", lens.mounts))
                    };
                    if response.clicked() {
                        *chosen = Some(lens.name.clone());
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
}

/// A slider for a float parameter, with its edits; returns the slider's
/// response and the width of its track (where it starts on the left).
pub fn float_slider(
    ui: &mut egui::Ui,
    node: NodeId,
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
    (response, track)
}

/// The widget of one parameter: a labelled slider, a checkbox or a menu.
pub fn param_widget(
    ui: &mut egui::Ui,
    node: NodeId,
    row: &editor::ParamRow,
    show_label: bool,
    edits: &mut Vec<UiEdit>,
) {
    let d = &row.descriptor;
    let param = d.id.clone();
    match (&d.kind, row.value) {
        (ParamKind::Float { .. }, ParamValue::Float(v)) => {
            if show_label {
                ui.label(RichText::new(d.label).color(ui.visuals().weak_text_color()));
            }
            float_slider(ui, node, d, v, edits);
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
