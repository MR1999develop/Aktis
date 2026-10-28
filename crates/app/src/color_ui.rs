//! Inspector panels for the colour and detail tools, laid out like
//! Capture One's:
//!
//! - **Colour editor**: Basic (a chip per colour range, then the chosen
//!   range's hue, saturation and lightness), Advanced (three ranges, each
//!   a wedge on a colour wheel: drag inside it to turn it, near an edge to
//!   widen it, or pick a colour on the photo) and Skin Tone (the same, with
//!   the uniformity sliders).
//! - **Colour balance**: Master, 3-Way and a tab per tone band. A band is a
//!   colour wheel (drag the puck: its direction is the hue, its distance
//!   the amount) with the lightness on an arc on the right and, for the
//!   master, the saturation on an arc on the left; double-click resets.
//! - **Black & white**: the switch, the six colour sensitivities and the
//!   split tones.
//! - **Clarity** (method, clarity, structure), **Dehaze** (amount and
//!   shadow tone, Auto or a colour picked on the photo) and **Vignetting**
//!   (amount, method, midpoint, feather, roundness for the ellipse).
//!
//! The geometry of the wheels is in [`editor::color_wheel`], what the
//! pickers set in [`editor::adjust`]; this module draws and turns pointer
//! input into [`UiEdit`]s, one undo step per gesture.

use core_model::{NodeId, ParamId, ParamValue};
use editor::NodeRow;
use editor::adjust::{Pick, PickKind};
use editor::color_wheel::{self, Arc, WedgeGrab};
use eframe::egui::{
    self, Color32, CornerRadius, Id, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, pos2, vec2,
};
use tool_adjust::{black_white, clarity, color_balance, color_editor, dehaze, vignetting};
use tool_api::ParamKind;

use crate::adjust_ui::{
    PanelContext, choice, float, number, param, picker_button, pid, remember, remembered, tabs, toggle_pick,
};
use crate::inspector::{UiEdit, float_slider};
use crate::theme::{
    CHANNEL, FONT, HUE_VALUE, LIGHTNESS_STRIP, SPACE, Tokens, WHEEL_DIM, WHEEL_HANDLE, hue_color, linear_color,
};

/// Hues of the six colour ranges and sliders (red, yellow, green, cyan,
/// blue, magenta), every 60° from red as the tools place them.
const RANGE_HUES: [f32; 6] = [0.0, 60.0, 120.0, 180.0, 240.0, 300.0];

/// Segments around a hue wheel, and rings from its grey centre to the rim.
const SEGMENTS: u16 = 72;
const RINGS: u16 = 3;

// ---------------------------------------------------------------------------
// Shared pieces

fn boolean(row: &NodeRow, id: &str) -> bool {
    matches!(param(row, id).map(|p| p.value), Some(ParamValue::Bool(true)))
}

/// The lowest and highest value of float parameter `id`.
fn bounds(row: &NodeRow, id: &str) -> [f32; 2] {
    match param(row, id).map(|p| &p.descriptor.kind) {
        Some(ParamKind::Float { min, max, .. }) => [*min, *max],
        _ => [0.0, 100.0],
    }
}

/// A strip through `colors`, evenly spaced, filling `rect`.
fn gradient(painter: &egui::Painter, rect: Rect, colors: &[Color32]) {
    let Some(last) = colors.len().checked_sub(1).filter(|n| *n > 0) else { return };
    let mut mesh = egui::Mesh::default();
    for (i, c) in colors.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // a few stops
        let x = rect.left() + rect.width() * i as f32 / last as f32;
        mesh.colored_vertex(pos2(x, rect.top()), *c);
        mesh.colored_vertex(pos2(x, rect.bottom()), *c);
        if i > 0 {
            #[allow(clippy::cast_possible_truncation)] // a few vertices
            let k = mesh.vertices.len() as u32 - 4;
            mesh.add_triangle(k, k + 2, k + 3);
            mesh.add_triangle(k, k + 3, k + 1);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Colours through the hues `from` to `to` (degrees), for a strip.
fn hue_stops(from: f32, to: f32, saturation: f32) -> Vec<Color32> {
    (0..=12_u8).map(|i| hue_color(from + (to - from) * f32::from(i) / 12.0, saturation, HUE_VALUE)).collect()
}

/// A labelled slider for float parameter `id` (labelled `label`, else as
/// its tool names it), with a strip through `colors` under its track.
fn slider(
    ui: &mut egui::Ui,
    row: &NodeRow,
    id: &str,
    label: Option<&str>,
    colors: &[Color32],
    edits: &mut Vec<UiEdit>,
) {
    let t = Tokens::of(ui.ctx().theme());
    let Some(p) = param(row, id) else { return };
    let ParamValue::Float(v) = p.value else { return };
    ui.label(RichText::new(label.unwrap_or(p.descriptor.label)).color(t.text_weak));
    let (response, track) = float_slider(ui, row.node, &p.descriptor, v, edits);
    if colors.len() >= 2 {
        let r = response.rect;
        gradient(ui.painter(), Rect::from_min_size(pos2(r.left(), r.bottom() + 1.0), vec2(track, 3.0)), colors);
        ui.add_space(4.0);
    }
    ui.add_space(SPACE[0]);
}

/// A checkbox for bool parameter `id`.
fn check(ui: &mut egui::Ui, row: &NodeRow, id: &str, label: &str, edits: &mut Vec<UiEdit>) {
    let mut on = boolean(row, id);
    if ui.checkbox(&mut on, label).changed() {
        edits.push(UiEdit::Set { node: row.node, param: pid(id), value: ParamValue::Bool(on), in_drag: false });
    }
}

/// A labelled menu for choice parameter `id`.
fn combo(ui: &mut egui::Ui, row: &NodeRow, id: &str, options: &[&str], edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let current = choice(row, id);
    let label = param(row, id).map_or("", |p| p.descriptor.label);
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(t.text_weak));
        egui::ComboBox::from_id_salt((node, id))
            .selected_text(options.get(current as usize).copied().unwrap_or(""))
            .show_ui(ui, |ui| {
                for (i, option) in (0_u32..).zip(options) {
                    if ui.selectable_label(i == current, *option).clicked() && i != current {
                        edits.push(UiEdit::Set { node, param: pid(id), value: ParamValue::Choice(i), in_drag: false });
                    }
                }
            });
    });
}

/// Makes `edits` also turn on the switch `on` of `node` in the same step: a
/// range edited while off is turned on, as in Capture One.
fn turning_on(edits: &mut [UiEdit], node: NodeId, on: &str) {
    for e in edits {
        let with_on = match e {
            UiEdit::Set { node: n, param, value, in_drag } if *n == node => Some(UiEdit::SetMany {
                node,
                key: param.clone(),
                values: vec![(param.clone(), *value), (pid(on), ParamValue::Bool(true))],
                in_drag: *in_drag,
            }),
            _ => None,
        };
        if let Some(with_on) = with_on {
            *e = with_on;
        }
    }
}

/// A "Reset …" button for the parameters `ids`.
fn reset_button(ui: &mut egui::Ui, node: NodeId, text: &str, ids: &[String], edits: &mut Vec<UiEdit>) {
    if ui.small_button(text).on_hover_text("These settings back to their defaults").clicked() {
        edits.extend(ids.iter().map(|id| UiEdit::Reset { node, param: pid(id) }));
    }
}

/// The hint shown while picker `pick` waits for a click in the viewer.
fn pick_hint(ui: &mut egui::Ui, cx: PanelContext<'_>, pick: Pick, text: &str) {
    if cx.pick == Some(pick) {
        let t = Tokens::of(ui.ctx().theme());
        ui.label(RichText::new(text).size(FONT[0]).color(t.accent_text));
    }
}

// ---------------------------------------------------------------------------
// Colour wheels

/// The point at `offset` (wheel units) from `center` on a wheel of `radius`.
fn screen(center: Pos2, radius: f32, offset: [f32; 2]) -> Pos2 {
    center + vec2(offset[0], offset[1]) * radius
}

/// The offset (wheel units) of `p` from `center` on a wheel of `radius`.
fn offset(center: Pos2, radius: f32, p: Pos2) -> [f32; 2] {
    [(p.x - center.x) / radius, (p.y - center.y) / radius]
}

/// A hue wheel: the hues around, from grey at the centre to full colour at the rim.
fn hue_wheel(painter: &egui::Painter, center: Pos2, radius: f32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, hue_color(0.0, 0.0, HUE_VALUE));
    for k in 1..=RINGS {
        let s = f32::from(k) / f32::from(RINGS);
        for i in 0..SEGMENTS {
            let h = f32::from(i) * 360.0 / f32::from(SEGMENTS);
            mesh.colored_vertex(screen(center, radius * s, color_wheel::direction(h)), hue_color(h, s, HUE_VALUE));
        }
    }
    let at = |k: u16, i: u16| 1 + u32::from(k - 1) * u32::from(SEGMENTS) + u32::from(i % SEGMENTS);
    for i in 0..SEGMENTS {
        mesh.add_triangle(0, at(1, i), at(1, i + 1));
        for k in 2..=RINGS {
            mesh.add_triangle(at(k - 1, i), at(k, i), at(k, i + 1));
            mesh.add_triangle(at(k - 1, i), at(k, i + 1), at(k - 1, i + 1));
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// A slice of a wheel from the angle `from` turning by `sweep` degrees, filled with `fill`.
fn fan(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, sweep: f32, fill: Color32) {
    if sweep.abs() < 0.5 {
        return;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1 to 72 steps
    let steps = (sweep.abs() / 5.0).ceil().clamp(1.0, 72.0) as u16;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, fill);
    for i in 0..=steps {
        let a = from + sweep * f32::from(i) / f32::from(steps);
        mesh.colored_vertex(screen(center, radius, color_wheel::direction(a)), fill);
        if i > 0 {
            mesh.add_triangle(0, u32::from(i), u32::from(i) + 1);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// A handle: a light dot over a dark shadow, with `fill` inside when given.
fn knob(painter: &egui::Painter, at: Pos2, radius: f32, fill: Option<Color32>) {
    painter.circle_filled(at, radius + 1.0, WHEEL_HANDLE[1]);
    painter.circle_filled(at, radius, WHEEL_HANDLE[0]);
    if let Some(fill) = fill {
        painter.circle_filled(at, radius - 2.0, fill);
    }
}

/// An arc beside a wheel, coloured from `colors[0]` (lowest value) to
/// `colors[1]`, with a tick at the default and a knob at `value`.
#[allow(clippy::too_many_arguments)] // the arc's geometry, value and colours
fn value_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    arc: Arc,
    range: [f32; 2],
    value: f32,
    rest: f32,
    colors: [Color32; 2],
) {
    let at = |f: f32| screen(center, radius, color_wheel::direction(arc.start + arc.sweep * f));
    let steps = 24_u8;
    for i in 0..steps {
        let (a, b) = (f32::from(i) / f32::from(steps), f32::from(i + 1) / f32::from(steps));
        painter.line_segment([at(a), at(b)], Stroke::new(3.0, colors[0].lerp_to_gamma(colors[1], (a + b) / 2.0)));
    }
    let tick = color_wheel::direction(arc.angle(rest, range));
    painter.line_segment(
        [screen(center, radius - 4.0, tick), screen(center, radius + 4.0, tick)],
        Stroke::new(1.0, WHEEL_HANDLE[0]),
    );
    knob(painter, screen(center, radius, color_wheel::direction(arc.angle(value, range))), 4.5, None);
}

// ---------------------------------------------------------------------------
// Colour balance

/// The parameters of one colour-balance wheel.
struct Band {
    name: &'static str,
    hue: String,
    amount: String,
    lightness: String,
    /// Only the master has a saturation.
    saturation: Option<String>,
}

impl Band {
    fn master() -> Self {
        Self {
            name: "Master",
            hue: color_balance::MASTER_HUE.to_owned(),
            amount: color_balance::MASTER_AMOUNT.to_owned(),
            lightness: color_balance::MASTER_LIGHTNESS.to_owned(),
            saturation: Some(color_balance::MASTER_SATURATION.to_owned()),
        }
    }

    fn zone(z: usize) -> Self {
        Self {
            name: color_balance::ZONES[z].1,
            hue: color_balance::hue_id(z),
            amount: color_balance::amount_id(z),
            lightness: color_balance::lightness_id(z),
            saturation: None,
        }
    }

    fn ids(&self) -> Vec<String> {
        let mut ids = vec![self.hue.clone(), self.amount.clone(), self.lightness.clone()];
        ids.extend(self.saturation.clone());
        ids
    }
}

/// The part of a colour-balance wheel a drag holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WheelPart {
    #[default]
    None,
    Puck,
    Lightness,
    Saturation,
}

/// One colour-balance wheel `size` points wide: the puck on the hue wheel,
/// the lightness on an arc on the right and, when the band has one, the
/// saturation on an arc on the left.
fn balance_wheel(ui: &mut egui::Ui, row: &NodeRow, band: &Band, size: f32, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(2.0));
    let center = rect.center();
    let gap = (size * 0.06).max(6.0);
    let radius = size / 2.0 - gap - 5.0;
    let arc_radius = radius + gap;
    let (hue, amount, light) = (float(row, &band.hue), float(row, &band.amount), float(row, &band.lightness));
    let amount_max = bounds(row, &band.amount)[1].max(1.0);
    let light_range = bounds(row, &band.lightness);
    let saturation = band.saturation.as_deref().map(|id| (id, float(row, id), bounds(row, id)));

    // Drawing.
    hue_wheel(&painter, center, radius);
    painter.circle_stroke(center, radius, Stroke::new(1.0, t.stroke_strong));
    value_arc(&painter, center, arc_radius, Arc::RIGHT, light_range, light, 0.0, LIGHTNESS_STRIP);
    if let Some((_, s, range)) = saturation {
        let colors = [hue_color(hue, 0.0, HUE_VALUE), hue_color(hue, 1.0, HUE_VALUE)];
        value_arc(&painter, center, arc_radius, Arc::LEFT, range, s, 0.0, colors);
    }
    let puck = screen(center, radius, color_wheel::wheel_offset(hue, amount / amount_max));
    painter.line_segment([center, puck], Stroke::new(1.0, WHEEL_HANDLE[0]));
    knob(&painter, puck, 6.0, Some(hue_color(hue, amount / amount_max, HUE_VALUE)));

    // Pointer input: what a pointer at `p` holds, and the values it sets.
    let part_at = |p: Pos2| {
        let o = offset(center, radius, p);
        let d = o[0].hypot(o[1]);
        if d <= 1.05 {
            WheelPart::Puck
        } else if d * radius <= arc_radius + 10.0 && Arc::RIGHT.covers(o, 10.0) {
            WheelPart::Lightness
        } else if d * radius <= arc_radius + 10.0 && saturation.is_some() && Arc::LEFT.covers(o, 10.0) {
            WheelPart::Saturation
        } else {
            WheelPart::None
        }
    };
    let key_of = |part: WheelPart| match part {
        WheelPart::Puck => Some(pid(&band.hue)),
        WheelPart::Lightness => Some(pid(&band.lightness)),
        WheelPart::Saturation => saturation.map(|(id, ..)| pid(id)),
        WheelPart::None => None,
    };
    let values_at = |part: WheelPart, p: Pos2| -> Vec<(ParamId, ParamValue)> {
        let o = offset(center, radius, p);
        match part {
            WheelPart::Puck => {
                let (h, a) = color_wheel::wheel_hue_amount(o);
                vec![
                    (pid(&band.hue), ParamValue::Float(h.round().rem_euclid(360.0))),
                    (pid(&band.amount), ParamValue::Float((a * amount_max).round())),
                ]
            }
            WheelPart::Lightness => {
                vec![(pid(&band.lightness), ParamValue::Float(Arc::RIGHT.value_at(o, light_range).round()))]
            }
            WheelPart::Saturation => saturation
                .map(|(id, _, range)| vec![(pid(id), ParamValue::Float(Arc::LEFT.value_at(o, range).round()))])
                .unwrap_or_default(),
            WheelPart::None => Vec::new(),
        }
    };
    let drag_id = Id::unique(("rne-balance-drag", node, band.hue.as_str()));
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        let part = part_at(p);
        remember(ui, drag_id, part);
        if let Some(param) = key_of(part) {
            edits.push(UiEdit::BeginDrag { node, param });
        }
    }
    let held = remembered::<WheelPart>(ui, drag_id);
    if (response.dragged() || response.drag_started())
        && let Some(key) = key_of(held)
        && let Some(p) = response.interact_pointer_pos()
    {
        edits.push(UiEdit::SetMany { node, key, values: values_at(held, p), in_drag: true });
    }
    if response.drag_stopped() {
        remember(ui, drag_id, WheelPart::None);
        edits.push(UiEdit::EndDrag);
    }
    if let Some(p) = response.interact_pointer_pos() {
        let part = part_at(p);
        if response.double_clicked() {
            let ids: Vec<String> = match part {
                WheelPart::Puck => vec![band.hue.clone(), band.amount.clone()],
                WheelPart::Lightness => vec![band.lightness.clone()],
                WheelPart::Saturation => band.saturation.clone().into_iter().collect(),
                WheelPart::None => Vec::new(),
            };
            edits.extend(ids.iter().map(|id| UiEdit::Reset { node, param: pid(id) }));
        } else if response.clicked()
            && let Some(key) = key_of(part)
        {
            edits.push(UiEdit::SetMany { node, key, values: values_at(part, p), in_drag: false });
        }
    }
    let tip = if saturation.is_some() {
        "Drag the puck: its direction is the hue, its distance the amount. \
         Lightness on the right arc, saturation on the left. Double-click resets."
    } else {
        "Drag the puck: its direction is the hue, its distance the amount. \
         Lightness on the arc. Double-click resets."
    };
    response.on_hover_text(tip);
}

/// The values of a band in one line.
fn band_readout(row: &NodeRow, band: &Band) -> String {
    let (h, a, l) = (float(row, &band.hue), float(row, &band.amount), float(row, &band.lightness));
    format!("{h:.0}\u{b0}  {a:.0}  L {l:+.0}")
}

const BALANCE_TABS: [&str; 5] = ["Master", "3-Way", "Shadow", "Midtone", "Highlight"];

/// Master, 3-Way and one tab per tone band.
pub fn color_balance(ui: &mut egui::Ui, row: &NodeRow, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let tab_id = Id::unique(("rne-balance-tab", node));
    let tab = tabs(ui, &BALANCE_TABS, remembered::<usize>(ui, tab_id)).min(BALANCE_TABS.len() - 1);
    remember(ui, tab_id, tab);
    ui.add_space(SPACE[0]);
    if tab == 1 {
        let gap = SPACE[1];
        let w = ((ui.available_width() - 2.0 * gap) / 3.0).max(70.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for z in 0..color_balance::ZONES.len() {
                let band = Band::zone(z);
                ui.vertical_centered(|ui| {
                    ui.set_width(w);
                    ui.label(RichText::new(band.name).size(FONT[1]).strong());
                    balance_wheel(ui, row, &band, w, edits);
                    ui.label(RichText::new(band_readout(row, &band)).size(FONT[0]).color(t.text_weak));
                });
            }
        });
        let all: Vec<String> = (0..color_balance::ZONES.len()).flat_map(|z| Band::zone(z).ids()).collect();
        reset_button(ui, node, "Reset 3-Way", &all, edits);
        return;
    }
    let band = if tab == 0 { Band::master() } else { Band::zone(tab - 2) };
    let size = ui.available_width().clamp(160.0, 260.0);
    ui.vertical_centered(|ui| balance_wheel(ui, row, &band, size, edits));
    ui.add_space(SPACE[0]);
    let hue = float(row, &band.hue);
    slider(ui, row, &band.hue, None, &hue_stops(0.0, 360.0, 0.8), edits);
    let amount = [hue_color(hue, 0.0, HUE_VALUE), hue_color(hue, 1.0, HUE_VALUE)];
    slider(ui, row, &band.amount, None, &amount, edits);
    slider(ui, row, &band.lightness, None, &LIGHTNESS_STRIP, edits);
    if let Some(id) = &band.saturation {
        slider(ui, row, id, None, &amount, edits);
    }
    reset_button(ui, node, &format!("Reset {}", band.name), &band.ids(), edits);
}

// ---------------------------------------------------------------------------
// Colour editor

/// A colour chip: `fill`, outlined when chosen, with a dot when its range has changes.
fn chip(ui: &mut egui::Ui, fill: Color32, selected: bool, changed: bool, name: &str) -> egui::Response {
    let t = Tokens::of(ui.ctx().theme());
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect.shrink(3.0), CornerRadius::same(3), fill);
    if selected {
        painter.rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0, t.accent), StrokeKind::Inside);
    } else if response.hovered() {
        painter.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0, t.stroke_strong), StrokeKind::Inside);
    }
    if changed {
        knob(painter, pos2(rect.center().x, rect.bottom() - 7.0), 2.0, None);
    }
    let tip = if changed { format!("{name} (changed)") } else { name.to_owned() };
    response.on_hover_text(tip)
}

/// Basic, Advanced and Skin Tone tabs.
pub fn color_editor(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    let tab_id = Id::unique(("rne-color-editor-tab", row.node));
    let tab = tabs(ui, &["Basic", "Advanced", "Skin Tone"], remembered::<usize>(ui, tab_id)).min(2);
    remember(ui, tab_id, tab);
    ui.add_space(SPACE[0]);
    match tab {
        0 => color_editor_basic(ui, row, edits),
        1 => color_editor_advanced(ui, row, cx, edits),
        _ => color_editor_skin(ui, row, cx, edits),
    }
}

/// The six range chips and the chosen range's sliders.
fn color_editor_basic(ui: &mut egui::Ui, row: &NodeRow, edits: &mut Vec<UiEdit>) {
    use color_editor::{RANGES, hue_id, lightness_id, saturation_id};
    let node = row.node;
    let chosen_id = Id::unique(("rne-color-editor-range", node));
    let mut r = remembered::<usize>(ui, chosen_id).min(RANGES.len() - 1);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SPACE[0];
        for (i, (_, name)) in RANGES.iter().enumerate() {
            let changed = [hue_id(i), saturation_id(i), lightness_id(i)].iter().any(|id| float(row, id) != 0.0);
            if chip(ui, hue_color(RANGE_HUES[i], 0.8, HUE_VALUE), i == r, changed, name).clicked() {
                r = i;
            }
        }
    });
    remember(ui, chosen_id, r);
    ui.add_space(SPACE[0]);
    ui.label(RichText::new(RANGES[r].1).size(FONT[2]).strong());
    let h = RANGE_HUES[r];
    slider(ui, row, &hue_id(r), None, &hue_stops(h - 30.0, h + 30.0, 0.8), edits);
    slider(ui, row, &saturation_id(r), None, &[hue_color(h, 0.0, HUE_VALUE), hue_color(h, 1.0, HUE_VALUE)], edits);
    slider(ui, row, &lightness_id(r), None, &[hue_color(h, 0.8, 0.2), hue_color(h, 0.35, 1.0)], edits);
    let ids = [hue_id(r), saturation_id(r), lightness_id(r)];
    reset_button(ui, node, &format!("Reset {}", RANGES[r].1), &ids, edits);
}

/// The list of Advanced ranges (on/off, colour, choose) and the chosen one's controls.
fn color_editor_advanced(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let chosen_id = Id::unique(("rne-color-editor-advanced", node));
    let mut n = remembered::<usize>(ui, chosen_id).min(color_editor::ADVANCED - 1);
    for k in 0..color_editor::ADVANCED {
        let id = |v: &str| color_editor::advanced_id(k, v);
        ui.horizontal(|ui| {
            check(ui, row, &id("on"), "", edits);
            let (c, w) = (float(row, &id("center")), float(row, &id("width")));
            let (swatch, _) = ui.allocate_exact_size(vec2(18.0, 14.0), Sense::hover());
            ui.painter().rect_filled(swatch, CornerRadius::same(3), hue_color(c, 0.8, HUE_VALUE));
            let text = format!("Range {}", k + 1);
            if ui.selectable_label(k == n, text).clicked() {
                n = k;
            }
            ui.label(RichText::new(format!("{c:.0}\u{b0} \u{b1}{w:.0}\u{b0}")).size(FONT[0]).color(t.text_weak));
        });
    }
    remember(ui, chosen_id, n);
    ui.add_space(SPACE[0]);
    let pick = Pick { node, kind: PickKind::ColorRange, channel: n };
    range_controls(ui, row, cx, &|v: &str| color_editor::advanced_id(n, v), pick, edits);
}

/// The skin tone range: switch, controls and uniformity.
fn color_editor_skin(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    use color_editor::skin_id;
    let node = row.node;
    check(ui, row, &skin_id("on"), "Skin tone range", edits);
    let pick = Pick { node, kind: PickKind::ColorRange, channel: color_editor::ADVANCED };
    range_controls(ui, row, cx, &skin_id, pick, edits);
    ui.add_space(SPACE[1]);
    ui.label(RichText::new("Uniformity").size(FONT[2]).strong());
    let start = edits.len();
    let uniform = [("uniform_hue", "Hue"), ("uniform_saturation", "Saturation"), ("uniform_lightness", "Lightness")];
    for (v, label) in uniform {
        slider(ui, row, &skin_id(v), Some(label), &[], edits);
    }
    turning_on(&mut edits[start..], node, &skin_id("on"));
    let ids: Vec<String> = uniform.iter().map(|(v, _)| skin_id(v)).collect();
    reset_button(ui, node, "Reset uniformity", &ids, edits);
}

/// The wheel, picker, centre and width, and sliders of a picked range
/// whose parameter ids `id` gives.
fn range_controls(
    ui: &mut egui::Ui,
    row: &NodeRow,
    cx: PanelContext<'_>,
    id: &dyn Fn(&str) -> String,
    pick: Pick,
    edits: &mut Vec<UiEdit>,
) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let size = (ui.available_width() - 40.0).clamp(140.0, 220.0);
    ui.horizontal_top(|ui| {
        range_wheel(ui, row, cx, id, size, edits);
        if picker_button(ui, cx.pick == Some(pick), "Click a colour in the photo: the range is centred on its hue") {
            edits.push(toggle_pick(cx.pick, pick));
        }
    });
    pick_hint(ui, cx, pick, "Click a colour in the photo (Esc to stop).");
    let start = edits.len();
    ui.horizontal(|ui| {
        for (v, label) in [("center", "Centre"), ("width", "Width")] {
            ui.label(RichText::new(label).color(t.text_weak));
            number(ui, node, &id(v), float(row, &id(v)), bounds(row, &id(v)), edits);
        }
    });
    let c = float(row, &id("center"));
    slider(ui, row, &id("smoothness"), None, &[], edits);
    slider(ui, row, &id("hue"), None, &hue_stops(c - 30.0, c + 30.0, 0.8), edits);
    slider(ui, row, &id("saturation"), None, &[hue_color(c, 0.0, HUE_VALUE), hue_color(c, 1.0, HUE_VALUE)], edits);
    slider(ui, row, &id("lightness"), None, &[hue_color(c, 0.8, 0.2), hue_color(c, 0.35, 1.0)], edits);
    turning_on(&mut edits[start..], node, &id("on"));
    reset_button(ui, node, "Reset range", &color_editor::RANGE_VALUES.map(id), edits);
}

/// The colour wheel of a picked range: the range as a wedge (fading out
/// over its smoothness), turned by dragging inside it and widened by
/// dragging an edge; the colour under the pointer in the viewer as a ring.
fn range_wheel(
    ui: &mut egui::Ui,
    row: &NodeRow,
    cx: PanelContext<'_>,
    id: &dyn Fn(&str) -> String,
    size: f32,
    edits: &mut Vec<UiEdit>,
) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(2.0));
    let center = rect.center();
    let radius = size / 2.0 - 4.0;
    let (on, c, w, smooth) =
        (boolean(row, &id("on")), float(row, &id("center")), float(row, &id("width")), float(row, &id("smoothness")));
    // As the tool weighs a hue: fully in up to `inner`, out from `outer`.
    let inner = (w * (1.0 - 0.5 * smooth / 100.0)).min(180.0);
    let outer = (w * (1.0 + 0.5 * smooth / 100.0)).min(180.0);

    hue_wheel(&painter, center, radius);
    fan(&painter, center, radius, c + outer, 360.0 - 2.0 * outer, WHEEL_DIM);
    let half = WHEEL_DIM.gamma_multiply(0.5);
    fan(&painter, center, radius, c + inner, outer - inner, half);
    fan(&painter, center, radius, c - outer, outer - inner, half);
    painter.circle_stroke(center, radius, Stroke::new(1.0, t.stroke_strong));
    let line = if on { WHEEL_HANDLE[0] } else { WHEEL_HANDLE[0].gamma_multiply(0.5) };
    for edge in [c - w, c + w] {
        let rim = screen(center, radius, color_wheel::direction(edge));
        painter.line_segment([center, rim], Stroke::new(3.0, WHEEL_HANDLE[1]));
        painter.line_segment([center, rim], Stroke::new(1.5, line));
    }
    knob(&painter, screen(center, radius * 0.8, color_wheel::direction(c)), 5.0, Some(hue_color(c, 0.8, HUE_VALUE)));
    if let Some(rgb) = cx.probe {
        let (h, chroma) = tool_adjust::hue_chroma(rgb);
        if chroma > 0.0 {
            let at = screen(center, radius, color_wheel::wheel_offset(h, chroma));
            painter.circle_stroke(at, 5.0, Stroke::new(3.0, WHEEL_HANDLE[1]));
            painter.circle_stroke(at, 5.0, Stroke::new(1.5, WHEEL_HANDLE[0]));
        }
    }

    // Pointer input: a drag turns the range or moves an edge, a click centres it.
    let hue_at = |p: Pos2| {
        let o = offset(center, radius, p);
        (o[0].hypot(o[1]) > 0.08).then(|| color_wheel::angle_of(o))
    };
    let key = pid(&id("center"));
    let limits = bounds(row, &id("width"));
    let values = |grab: WedgeGrab, h: f32| {
        let set = match grab {
            WedgeGrab::Center => (key.clone(), h.round().rem_euclid(360.0)),
            WedgeGrab::Width => (pid(&id("width")), color_wheel::wedge_width(h, c, limits).round()),
        };
        vec![(set.0, ParamValue::Float(set.1)), (pid(&id("on")), ParamValue::Bool(true))]
    };
    let drag_id = Id::unique(("rne-range-drag", node, id("center")));
    if response.drag_started()
        && let Some(h) = response.interact_pointer_pos().and_then(hue_at)
    {
        remember(ui, drag_id, Some(color_wheel::wedge_grab(h, c, w, 8.0)));
        edits.push(UiEdit::BeginDrag { node, param: key.clone() });
    }
    if let Some(grab) = remembered::<Option<WedgeGrab>>(ui, drag_id)
        && (response.dragged() || response.drag_started())
        && let Some(h) = response.interact_pointer_pos().and_then(hue_at)
    {
        edits.push(UiEdit::SetMany { node, key: key.clone(), values: values(grab, h), in_drag: true });
    }
    if response.drag_stopped() {
        remember(ui, drag_id, None::<WedgeGrab>);
        edits.push(UiEdit::EndDrag);
    }
    if response.clicked()
        && let Some(h) = response.interact_pointer_pos().and_then(hue_at)
    {
        edits.push(UiEdit::SetMany { node, key: key.clone(), values: values(WedgeGrab::Center, h), in_drag: false });
    }
    response.on_hover_text("Drag inside the range to turn it, drag an edge to widen it; click a colour to centre it");
}

// ---------------------------------------------------------------------------
// Black & white

/// The switch, then the Color Sensitivity and Split Tones tabs.
pub fn black_white(ui: &mut egui::Ui, row: &NodeRow, edits: &mut Vec<UiEdit>) {
    use black_white::{
        BALANCE, COLORS, ENABLE, HIGHLIGHT_HUE, HIGHLIGHT_SATURATION, SHADOW_HUE, SHADOW_SATURATION, TONE_AMOUNT,
        TONE_HUE,
    };
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    check(ui, row, ENABLE, "Enable Black & White", edits);
    ui.add_space(SPACE[0]);
    ui.add_enabled_ui(boolean(row, ENABLE), |ui| {
        let tab_id = Id::unique(("rne-black-white-tab", node));
        let tab = tabs(ui, &["Color Sensitivity", "Split Tones"], remembered::<usize>(ui, tab_id)).min(1);
        remember(ui, tab_id, tab);
        ui.add_space(SPACE[0]);
        if tab == 0 {
            for (i, (id, _)) in COLORS.iter().enumerate() {
                let h = RANGE_HUES[i];
                slider(ui, row, id, None, &[hue_color(h, 0.9, 0.2), hue_color(h, 0.9, 1.0)], edits);
            }
            let ids: Vec<String> = COLORS.iter().map(|(id, _)| (*id).to_owned()).collect();
            reset_button(ui, node, "Reset sensitivity", &ids, edits);
            return;
        }
        let tone = |hue: &str, sat: &str| (float(row, hue), float(row, sat) / bounds(row, sat)[1].max(1.0));
        let (hh, hs) = tone(HIGHLIGHT_HUE, HIGHLIGHT_SATURATION);
        let (sh, ss) = tone(SHADOW_HUE, SHADOW_SATURATION);
        for (title, hue, sat, h) in
            [("Highlights", HIGHLIGHT_HUE, HIGHLIGHT_SATURATION, hh), ("Shadows", SHADOW_HUE, SHADOW_SATURATION, sh)]
        {
            ui.label(RichText::new(title).size(FONT[2]).strong());
            slider(ui, row, hue, Some("Hue"), &hue_stops(0.0, 360.0, 0.8), edits);
            let colors = [hue_color(h, 0.0, HUE_VALUE), hue_color(h, 1.0, HUE_VALUE)];
            slider(ui, row, sat, Some("Saturation"), &colors, edits);
        }
        slider(ui, row, BALANCE, None, &[hue_color(sh, ss, 0.35), hue_color(hh, hs, 1.0)], edits);
        // The single tint of version 1 recipes still renders: shown only when set.
        if float(row, TONE_AMOUNT) != 0.0 {
            ui.label(RichText::new("Overall tone (older recipe)").size(FONT[1]).color(t.text_weak));
            slider(ui, row, TONE_HUE, Some("Hue"), &hue_stops(0.0, 360.0, 0.8), edits);
            slider(ui, row, TONE_AMOUNT, Some("Amount"), &[], edits);
        }
        let ids: Vec<String> = [HIGHLIGHT_HUE, HIGHLIGHT_SATURATION, SHADOW_HUE, SHADOW_SATURATION, BALANCE]
            .iter()
            .map(|id| (*id).to_owned())
            .collect();
        reset_button(ui, node, "Reset split tones", &ids, edits);
    });
}

// ---------------------------------------------------------------------------
// Clarity, dehaze, vignetting

/// Method, clarity and structure.
pub fn clarity(ui: &mut egui::Ui, row: &NodeRow, edits: &mut Vec<UiEdit>) {
    combo(ui, row, clarity::METHOD, &clarity::METHODS, edits);
    ui.add_space(SPACE[0]);
    slider(ui, row, clarity::CLARITY, None, &[], edits);
    slider(ui, row, clarity::STRUCTURE, None, &[], edits);
}

/// Amount and shadow tone: Auto, or a colour typed or picked on the photo.
pub fn dehaze(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    slider(ui, row, dehaze::AMOUNT, None, &[], edits);
    let custom = choice(row, dehaze::SHADOW_TONE) == dehaze::CUSTOM_TONE;
    let pick = Pick { node, kind: PickKind::ShadowTone, channel: 0 };
    ui.horizontal(|ui| {
        combo(ui, row, dehaze::SHADOW_TONE, &dehaze::SHADOW_TONES, edits);
        let tone = dehaze::TONE_RGB.map(|id| float(row, id));
        let (swatch, response) = ui.allocate_exact_size(vec2(28.0, 20.0), Sense::hover());
        let fill = if custom { linear_color(tone) } else { linear_color(tone).gamma_multiply(0.35) };
        ui.painter().rect_filled(swatch, CornerRadius::same(4), fill);
        ui.painter().rect_stroke(swatch, CornerRadius::same(4), Stroke::new(1.0, t.stroke_strong), StrokeKind::Inside);
        let tip = if custom { "The colour the shadows are taken to" } else { "Auto: the shadows are made neutral" };
        response.on_hover_text(tip);
        if picker_button(ui, cx.pick == Some(pick), "Click a shadow in the photo: its colour becomes the shadow tone") {
            edits.push(toggle_pick(cx.pick, pick));
        }
    });
    pick_hint(ui, cx, pick, "Click a shadow in the photo (Esc to stop).");
    ui.add_enabled_ui(custom, |ui| {
        for (i, id) in dehaze::TONE_RGB.iter().enumerate() {
            slider(ui, row, id, None, &[LIGHTNESS_STRIP[0], CHANNEL[i + 2]], edits);
        }
    });
}

/// Amount, method, midpoint, feather, and roundness for the elliptic method.
pub fn vignetting(ui: &mut egui::Ui, row: &NodeRow, edits: &mut Vec<UiEdit>) {
    slider(ui, row, vignetting::AMOUNT, None, &LIGHTNESS_STRIP, edits);
    combo(ui, row, vignetting::METHOD, &vignetting::METHODS, edits);
    ui.add_space(SPACE[0]);
    slider(ui, row, vignetting::MIDPOINT, None, &[], edits);
    slider(ui, row, vignetting::FEATHER, None, &[], edits);
    // Roundness shapes the ellipse only; the circles are round already.
    ui.add_enabled_ui(choice(row, vignetting::METHOD) == 0, |ui| {
        slider(ui, row, vignetting::ROUNDNESS, None, &[], edits);
    });
}
