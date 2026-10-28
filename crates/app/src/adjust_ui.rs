//! Inspector panels for the tools that need more than sliders, laid out
//! like Capture One's:
//!
//! - **White balance**: the mode, a picker for something that should be
//!   grey, and kelvin and tint sliders over colour strips.
//! - **Levels**: RGB, red, green and blue tabs; the histogram with the
//!   input black, midtone and white handles; the output black and white on
//!   a strip below; the values as numbers; black and white pickers.
//! - **Curve**: RGB, Luma, red, green and blue tabs; the curve over the
//!   histogram, edited with the mouse (drag on empty space adds a point,
//!   drag a point to move it, double-click or right-click removes it); the
//!   input and output under the pointer; a picker that adds a point where
//!   the clicked colour sits.
//!
//! The logic (which values a handle or a point sets) is in
//! [`editor::adjust`]; this module draws and turns pointer input into
//! [`UiEdit`]s. The colour under the pointer in the viewer is marked on the
//! histogram and the curve.

use core_model::{NodeId, ParamId, ParamValue};
use editor::adjust::{self, Pick, PickKind};
use editor::{Histogram, NodeRow, ParamRow};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, RichText, Sense, Shape, Stroke, pos2, vec2,
};
use tool_adjust::curve::{self, CURVES, MAX_POINTS, Spline};
use tool_adjust::levels::{self, CHANNELS};
use tool_adjust::white_balance::{self, Mode};

use crate::icons;
use crate::inspector::{UiEdit, float_slider};
use crate::theme::{CHANNEL, FONT, HISTOGRAM, KELVIN_STRIP, SPACE, TINT_STRIP, Tokens};

/// What the panels need besides the node.
#[derive(Debug, Clone, Copy)]
pub struct PanelContext<'a> {
    /// Levels of the image on screen.
    pub histogram: Option<&'a Histogram>,
    /// The picker waiting for a click in the viewer.
    pub pick: Option<Pick>,
    /// The colour entering the node under the pointer in the viewer.
    pub probe: Option<[f32; 3]>,
}

fn pid(id: &str) -> ParamId {
    ParamId::new(id).expect("tool parameter ids are valid")
}

fn param<'r>(row: &'r NodeRow, id: &str) -> Option<&'r ParamRow> {
    row.params.iter().find(|p| p.descriptor.id.as_str() == id)
}

fn float(row: &NodeRow, id: &str) -> f32 {
    match param(row, id).map(|p| p.value) {
        Some(ParamValue::Float(v)) => v,
        _ => 0.0,
    }
}

fn choice(row: &NodeRow, id: &str) -> u32 {
    match param(row, id).map(|p| p.value) {
        Some(ParamValue::Choice(v)) => v,
        _ => 0,
    }
}

/// A small square button with an eyedropper, highlighted while its picker waits.
fn picker_button(ui: &mut egui::Ui, active: bool, tip: &str) -> bool {
    let t = Tokens::of(ui.ctx().theme());
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
    let fill = if active {
        t.accent
    } else if response.hovered() {
        t.fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    icons::eyedropper(ui.painter(), rect.shrink(5.0), if active { t.on_accent } else { t.text_strong });
    response.on_hover_text(tip).clicked()
}

/// The edit that turns picker `pick` on, or off when it is the one waiting.
fn toggle_pick(current: Option<Pick>, pick: Pick) -> UiEdit {
    UiEdit::Pick(if current == Some(pick) { None } else { Some(pick) })
}

/// A horizontal colour strip from `ends[0]` to `ends[1]`.
fn strip(painter: &egui::Painter, rect: Rect, ends: [Color32; 2]) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), ends[0]);
    mesh.colored_vertex(rect.right_top(), ends[1]);
    mesh.colored_vertex(rect.right_bottom(), ends[1]);
    mesh.colored_vertex(rect.left_bottom(), ends[0]);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// Remembers a small value per panel between frames (the chosen tab, the
/// handle being dragged).
fn remembered<T: Clone + Default + Send + Sync + 'static>(ui: &egui::Ui, id: Id) -> T {
    ui.ctx().data(|d| d.get_temp::<T>(id)).unwrap_or_default()
}

fn remember<T: Clone + Send + Sync + 'static>(ui: &egui::Ui, id: Id, value: T) {
    ui.ctx().data_mut(|d| d.insert_temp(id, value));
}

/// A row of tabs; returns the chosen index.
fn tabs(ui: &mut egui::Ui, names: &[&str], current: usize) -> usize {
    let mut chosen = current;
    ui.horizontal(|ui| {
        for (i, name) in names.iter().enumerate() {
            if ui.selectable_label(i == current, *name).clicked() {
                chosen = i;
            }
        }
    });
    chosen
}

// ---------------------------------------------------------------------------
// White balance

/// Mode, picker, kelvin and tint.
pub fn white_balance(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let mode = Mode::from_index(choice(row, white_balance::MODE));
    let neutral = Pick { node, kind: PickKind::Neutral, channel: 0 };
    ui.horizontal(|ui| {
        ui.label(RichText::new("Mode").color(t.text_weak));
        egui::ComboBox::from_id_salt((node, "wb-mode")).selected_text(mode.label()).show_ui(ui, |ui| {
            for m in Mode::ALL {
                // Choosing Auto again estimates again (the image may have changed).
                if ui.selectable_label(m == mode, m.label()).clicked() && (m != mode || m == Mode::Auto) {
                    edits.push(UiEdit::WbMode { node, mode: m });
                }
            }
        });
        let active = cx.pick == Some(neutral);
        if picker_button(ui, active, "Pick something that should be neutral grey in the photo") {
            edits.push(toggle_pick(cx.pick, neutral));
        }
    });
    ui.add_space(SPACE[1]);
    for (id, ends) in [(white_balance::KELVIN, KELVIN_STRIP), (white_balance::TINT, TINT_STRIP)] {
        let Some(p) = param(row, id) else { continue };
        let ParamValue::Float(v) = p.value else { continue };
        ui.label(RichText::new(p.descriptor.label).color(t.text_weak));
        let start = edits.len();
        let (response, track) = float_slider(ui, node, &p.descriptor, v, edits);
        let r = response.rect;
        let bar = Rect::from_min_size(pos2(r.left(), r.bottom() + 1.0), vec2(track, 3.0));
        strip(ui.painter(), bar, ends);
        ui.add_space(SPACE[1] + 4.0);
        // Moving kelvin or tint by hand makes the mode Custom, in the same step.
        if mode != Mode::Custom {
            for e in &mut edits[start..] {
                let custom = match e {
                    UiEdit::Set { node, param, value, in_drag } => Some(UiEdit::SetMany {
                        node: *node,
                        key: param.clone(),
                        values: vec![
                            (param.clone(), *value),
                            (pid(white_balance::MODE), ParamValue::Choice(Mode::Custom.index())),
                        ],
                        in_drag: *in_drag,
                    }),
                    _ => None,
                };
                if let Some(custom) = custom {
                    *e = custom;
                }
            }
        }
    }
    let (k, tint) = (float(row, white_balance::SHOT_KELVIN), float(row, white_balance::SHOT_TINT));
    ui.label(RichText::new(format!("As shot: {k:.0} K, tint {tint:+.1}")).size(FONT[0]).color(t.text_weak));
    if cx.pick == Some(neutral) {
        ui.label(
            RichText::new("Click a grey or white area in the photo (Esc to stop).").size(FONT[0]).color(t.accent_text),
        );
    }
}

// ---------------------------------------------------------------------------
// Levels

fn levels_set(row: &NodeRow, c: usize) -> levels::Set {
    let v = |i| float(row, &levels::param_id(c, i));
    levels::Set { in_black: v(0), in_white: v(1), midtone: v(2), out_black: v(3), out_white: v(4) }
}

/// The counts of a histogram for levels set or curve `c` (luminance for the
/// RGB set, the RGB and Luma curves).
fn counts(h: &Histogram, c: usize, curve: bool) -> &[u32; 256] {
    match (curve, c) {
        (false, 1) | (true, 2) => &h.red,
        (false, 2) | (true, 3) => &h.green,
        (false, 3) | (true, 4) => &h.blue,
        _ => &h.luma,
    }
}

/// Draws `counts` as bars filling `rect`, in `color`.
fn histogram_bars(painter: &egui::Painter, rect: Rect, counts: &[u32; 256], color: Color32) {
    let peak = counts.iter().copied().max().unwrap_or(0).max(1);
    #[allow(clippy::cast_precision_loss)] // counts, for drawing
    let peak = peak as f32;
    let bin = rect.width() / 256.0;
    for (i, &n) in counts.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // counts and bins, for drawing
        let (level, x) = ((n as f32 / peak).min(1.0), rect.left() + i as f32 * bin);
        if level > 0.0 {
            let bar = Rect::from_min_max(
                pos2(x, rect.bottom() - level * rect.height()),
                pos2(x + bin.max(1.0), rect.bottom()),
            );
            painter.rect_filled(bar, CornerRadius::ZERO, color);
        }
    }
}

/// A handle under a strip: a small triangle pointing up at `x`.
fn handle(painter: &egui::Painter, x: f32, top: f32, fill: Color32, stroke: Color32) {
    let points = vec![pos2(x, top), pos2(x + 5.0, top + 8.0), pos2(x - 5.0, top + 8.0)];
    painter.add(Shape::convex_polygon(points, fill, Stroke::new(1.0, stroke)));
}

/// A number field for a levels value.
fn number(ui: &mut egui::Ui, node: NodeId, id: &str, value: f32, range: [f32; 2], edits: &mut Vec<UiEdit>) {
    let mut v = value;
    let response = ui.add(egui::DragValue::new(&mut v).range(range[0]..=range[1]).speed(1.0).fixed_decimals(0));
    let param = pid(id);
    if response.drag_started() {
        edits.push(UiEdit::BeginDrag { node, param: param.clone() });
    }
    if response.changed() {
        edits.push(UiEdit::Set { node, param, value: ParamValue::Float(v), in_drag: response.dragged() });
    }
    if response.drag_stopped() {
        edits.push(UiEdit::EndDrag);
    }
}

/// Tabs, histogram with handles, output strip, numbers and pickers.
pub fn levels(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let tab_id = Id::unique(("rne-levels-tab", node));
    let names: Vec<&str> = CHANNELS.iter().map(|c| c.1).collect();
    let c = tabs(ui, &names, remembered::<usize>(ui, tab_id)).min(CHANNELS.len() - 1);
    remember(ui, tab_id, c);
    let set = levels_set(row, c);
    let color = CHANNEL[if c == 0 { 0 } else { c + 1 }];

    // Histogram and input handles.
    let w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(w, 110.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(6.0));
    let plot = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - 12.0));
    painter.rect_filled(plot, CornerRadius::same(4), t.canvas);
    if let Some(h) = cx.histogram {
        let fill = if c == 0 { t.stroke_strong.gamma_multiply(0.6) } else { HISTOGRAM[c - 1].gamma_multiply(2.0) };
        histogram_bars(&painter, plot.shrink(2.0), counts(h, c, false), fill);
    }
    let x_of = |v: f32| plot.left() + v / 255.0 * plot.width();
    let v_of = |x: f32| ((x - plot.left()) / plot.width() * 255.0).clamp(0.0, 255.0);
    // Shade what the input points clip.
    let shade = t.bg.gamma_multiply(0.55);
    painter.rect_filled(
        Rect::from_min_max(plot.min, pos2(x_of(set.in_black), plot.bottom())),
        CornerRadius::ZERO,
        shade,
    );
    painter.rect_filled(Rect::from_min_max(pos2(x_of(set.in_white), plot.top()), plot.max), CornerRadius::ZERO, shade);
    if let Some(rgb) = cx.probe {
        let x = x_of(adjust::channel_value(rgb, c, false).min(1.0) * 255.0);
        painter.extend(Shape::dashed_line(
            &[pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(1.0, t.text_strong),
            3.0,
            3.0,
        ));
    }
    let mid = adjust::midtone_position(&set);
    let top = plot.bottom() + 2.0;
    handle(&painter, x_of(set.in_black), top, Color32::BLACK, t.text_strong);
    handle(&painter, x_of(mid), top, t.text_weak, t.text_strong);
    handle(&painter, x_of(set.in_white), top, Color32::WHITE, t.stroke_strong);
    painter
        .line_segment([pos2(plot.left(), plot.bottom()), pos2(plot.right(), plot.bottom())], Stroke::new(1.0, color));

    // Dragging the handle nearest to where the drag started: 0 black, 1 white, 2 midtone.
    let drag_id = Id::unique(("rne-levels-handle", node));
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        let near = [(0_u8, set.in_black), (1, set.in_white), (2, mid)]
            .into_iter()
            .map(|(k, v)| (k, (x_of(v) - p.x).abs()))
            .filter(|(_, d)| *d <= 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k);
        remember(ui, drag_id, near);
        if let Some(k) = near {
            edits.push(UiEdit::BeginDrag { node, param: pid(&levels::param_id(c, usize::from(k))) });
        }
    }
    if let Some(k) = remembered::<Option<u8>>(ui, drag_id)
        && (response.dragged() || response.drag_started())
        && let Some(p) = response.interact_pointer_pos()
    {
        let x = v_of(p.x);
        let (which, value) = match k {
            0 => (0, x.round().min(set.in_white - 2.0).max(0.0)),
            1 => (1, x.round().max(set.in_black + 2.0).min(255.0)),
            _ => (2, adjust::midtone_for_position(&set, x).round()),
        };
        edits.push(UiEdit::Set {
            node,
            param: pid(&levels::param_id(c, which)),
            value: ParamValue::Float(value),
            in_drag: true,
        });
    }
    if response.drag_stopped() {
        remember::<Option<u8>>(ui, drag_id, None);
        edits.push(UiEdit::EndDrag);
    }

    // Output strip with its two handles.
    ui.add_space(SPACE[1]);
    let (out_rect, out_response) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click_and_drag());
    let bar = Rect::from_min_size(out_rect.min, vec2(out_rect.width(), 8.0));
    let out_painter = ui.painter_at(out_rect.expand(6.0));
    strip(&out_painter, bar, [Color32::BLACK, Color32::WHITE]);
    let ox = |v: f32| bar.left() + v / 255.0 * bar.width();
    handle(&out_painter, ox(set.out_black), bar.bottom() + 2.0, Color32::BLACK, t.text_strong);
    handle(&out_painter, ox(set.out_white), bar.bottom() + 2.0, Color32::WHITE, t.stroke_strong);
    let out_drag = Id::unique(("rne-levels-out", node));
    if out_response.drag_started()
        && let Some(p) = out_response.interact_pointer_pos()
    {
        let near = if (ox(set.out_black) - p.x).abs() <= (ox(set.out_white) - p.x).abs() { 3 } else { 4 };
        remember(ui, out_drag, Some(near));
        edits.push(UiEdit::BeginDrag { node, param: pid(&levels::param_id(c, near)) });
    }
    if let Some(which) = remembered::<Option<usize>>(ui, out_drag)
        && (out_response.dragged() || out_response.drag_started())
        && let Some(p) = out_response.interact_pointer_pos()
    {
        let v = ((p.x - bar.left()) / bar.width() * 255.0).clamp(0.0, 255.0).round();
        edits.push(UiEdit::Set {
            node,
            param: pid(&levels::param_id(c, which)),
            value: ParamValue::Float(v),
            in_drag: true,
        });
    }
    if out_response.drag_stopped() {
        remember::<Option<usize>>(ui, out_drag, None);
        edits.push(UiEdit::EndDrag);
    }

    // The values as numbers, and the pickers.
    ui.horizontal(|ui| {
        ui.label(RichText::new("Input").color(t.text_weak));
        number(ui, node, &levels::param_id(c, 0), set.in_black, [0.0, 254.0], edits);
        let mut m = set.midtone;
        let response = ui.add(egui::DragValue::new(&mut m).range(-100.0..=100.0).speed(0.5).fixed_decimals(0));
        let midtone = pid(&levels::param_id(c, 2));
        if response.drag_started() {
            edits.push(UiEdit::BeginDrag { node, param: midtone.clone() });
        }
        if response.changed() {
            edits.push(UiEdit::Set { node, param: midtone, value: ParamValue::Float(m), in_drag: response.dragged() });
        }
        if response.drag_stopped() {
            edits.push(UiEdit::EndDrag);
        }
        number(ui, node, &levels::param_id(c, 1), set.in_white, [1.0, 255.0], edits);
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Output").color(t.text_weak));
        number(ui, node, &levels::param_id(c, 3), set.out_black, [0.0, 255.0], edits);
        number(ui, node, &levels::param_id(c, 4), set.out_white, [0.0, 255.0], edits);
        ui.add_space(SPACE[2]);
        let black = Pick { node, kind: PickKind::LevelsBlack, channel: c };
        if picker_button(ui, cx.pick == Some(black), "Pick the darkest point: it becomes the input black") {
            edits.push(toggle_pick(cx.pick, black));
        }
        let white = Pick { node, kind: PickKind::LevelsWhite, channel: c };
        if picker_button(ui, cx.pick == Some(white), "Pick the brightest point: it becomes the input white") {
            edits.push(toggle_pick(cx.pick, white));
        }
    });
    if ui
        .small_button(format!("Reset {}", CHANNELS[c].1))
        .on_hover_text("This tab's levels back to their defaults")
        .clicked()
    {
        for i in 0..levels::VALUES.len() {
            edits.push(UiEdit::Reset { node, param: pid(&levels::param_id(c, i)) });
        }
    }
}

// ---------------------------------------------------------------------------
// Curve

fn curve_points(row: &NodeRow, c: usize) -> Vec<[f32; 2]> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a small count
    let n = (float(row, &curve::count_id(c)).round() as usize).clamp(2, MAX_POINTS);
    let mut pts: Vec<[f32; 2]> =
        (0..n).map(|i| [float(row, &curve::point_id(c, i, 'x')), float(row, &curve::point_id(c, i, 'y'))]).collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pts
}

/// The point being dragged in a curve panel.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct CurveDrag {
    /// Its index in the sorted points.
    index: Option<usize>,
}

/// Tabs, the curve to edit, the readout and the picker.
pub fn curve(ui: &mut egui::Ui, row: &NodeRow, cx: PanelContext<'_>, edits: &mut Vec<UiEdit>) {
    let t = Tokens::of(ui.ctx().theme());
    let node = row.node;
    let tab_id = Id::unique(("rne-curve-tab", node));
    let names: Vec<&str> = CURVES.iter().map(|c| c.1).collect();
    let c = tabs(ui, &names, remembered::<usize>(ui, tab_id)).min(CURVES.len() - 1);
    remember(ui, tab_id, c);
    let pts = curve_points(row, c);
    let spline = Spline::new(&pts);
    let color = CHANNEL[c];
    let key = pid(&curve::count_id(c));

    let side = ui.available_width().clamp(160.0, 320.0);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(4.0));
    painter.rect_filled(rect, CornerRadius::same(4), t.canvas);
    let inner = rect.shrink(6.0);
    if let Some(h) = cx.histogram {
        let fill = if c < 2 { t.stroke_strong.gamma_multiply(0.45) } else { HISTOGRAM[c - 2].gamma_multiply(1.6) };
        histogram_bars(&painter, inner, counts(h, c, true), fill);
    }
    for i in 1..4 {
        #[allow(clippy::cast_precision_loss)] // grid lines
        let f = i as f32 / 4.0;
        let grid = Stroke::new(1.0, t.graph_grid);
        painter.line_segment(
            [
                pos2(inner.left() + f * inner.width(), inner.top()),
                pos2(inner.left() + f * inner.width(), inner.bottom()),
            ],
            grid,
        );
        painter.line_segment(
            [
                pos2(inner.left(), inner.top() + f * inner.height()),
                pos2(inner.right(), inner.top() + f * inner.height()),
            ],
            grid,
        );
    }
    painter.line_segment([inner.left_bottom(), inner.right_top()], Stroke::new(1.0, t.stroke_weak));
    let to_screen = |p: [f32; 2]| pos2(inner.left() + p[0] * inner.width(), inner.bottom() - p[1] * inner.height());
    let to_curve = |p: Pos2| {
        [
            ((p.x - inner.left()) / inner.width()).clamp(0.0, 1.0),
            ((inner.bottom() - p.y) / inner.height()).clamp(0.0, 1.0),
        ]
    };
    #[allow(clippy::cast_precision_loss)] // samples along the curve
    let line: Vec<Pos2> = (0..=128)
        .map(|i| {
            let x = i as f32 / 128.0;
            to_screen([x, spline.eval(x).clamp(0.0, 1.0)])
        })
        .collect();
    painter.add(Shape::line(line, Stroke::new(1.5, color)));
    if let Some(rgb) = cx.probe {
        let x = adjust::channel_value(rgb, c, true).min(1.0);
        let at = to_screen([x, spline.eval(x).clamp(0.0, 1.0)]);
        painter.extend(Shape::dashed_line(
            &[pos2(at.x, inner.top()), pos2(at.x, inner.bottom())],
            Stroke::new(1.0, t.text_weak),
            3.0,
            3.0,
        ));
        painter.circle_stroke(at, 4.0, Stroke::new(1.5, t.text_strong));
    }

    let drag_id = Id::unique(("rne-curve-drag", node, c));
    let dragging = remembered::<CurveDrag>(ui, drag_id).index;
    let radius = 8.0 / inner.width();
    let hovered = response.hover_pos().map(to_curve).and_then(|p| adjust::nearest_point(&pts, p, radius));
    for (i, p) in pts.iter().enumerate() {
        let at = to_screen(*p);
        if Some(i) == dragging || Some(i) == hovered {
            painter.circle_filled(at, 5.0, color);
        } else {
            painter.circle_filled(at, 4.0, t.canvas);
            painter.circle_stroke(at, 4.0, Stroke::new(1.5, color));
        }
    }

    // Pointer input.
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos().map(to_curve)
    {
        // A drag on empty space adds a point there and moves it: one undo step.
        edits.push(UiEdit::BeginDrag { node, param: key.clone() });
        let index = match adjust::nearest_point(&pts, p, radius) {
            Some(i) => Some(i),
            None => adjust::insert_point(&pts, p).map(|(new, i)| {
                edits.push(UiEdit::SetMany {
                    node,
                    key: key.clone(),
                    values: adjust::curve_values(c, &new),
                    in_drag: true,
                });
                i
            }),
        };
        remember(ui, drag_id, CurveDrag { index });
    }
    if let Some(i) = dragging
        && response.dragged()
        && let Some(p) = response.interact_pointer_pos().map(to_curve)
    {
        let moved = adjust::move_point(&pts, i, p);
        edits.push(UiEdit::SetMany { node, key: key.clone(), values: adjust::curve_values(c, &moved), in_drag: true });
    }
    if response.drag_stopped() {
        remember(ui, drag_id, CurveDrag::default());
        edits.push(UiEdit::EndDrag);
    }
    if response.clicked()
        && let Some(p) = response.interact_pointer_pos().map(to_curve)
        && adjust::nearest_point(&pts, p, radius).is_none()
        && let Some((new, _)) = adjust::insert_point(&pts, p)
    {
        edits.push(UiEdit::SetMany { node, key: key.clone(), values: adjust::curve_values(c, &new), in_drag: false });
    }
    if (response.double_clicked() || response.secondary_clicked())
        && let Some(i) =
            response.interact_pointer_pos().map(to_curve).and_then(|p| adjust::nearest_point(&pts, p, radius))
        && let Some(fewer) = adjust::remove_point(&pts, i)
    {
        edits.push(UiEdit::SetMany { node, key: key.clone(), values: adjust::curve_values(c, &fewer), in_drag: false });
    }
    let response =
        response.on_hover_text("Drag to add or move a point; double-click or right-click a point to remove it");

    // Readout: the dragged point, else the pointer.
    let shown = dragging
        .and_then(|i| pts.get(i).copied())
        .or_else(|| response.hover_pos().map(to_curve).map(|p| [p[0], spline.eval(p[0]).clamp(0.0, 1.0)]));
    ui.horizontal(|ui| {
        let text = match shown {
            Some(p) => format!("Input {:.0}   Output {:.0}", p[0] * 255.0, p[1] * 255.0),
            None => "Input \u{2014}   Output \u{2014}".to_owned(),
        };
        ui.label(RichText::new(text).size(FONT[1]).color(t.text_weak));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let pick = Pick { node, kind: PickKind::CurvePoint, channel: c };
            if picker_button(
                ui,
                cx.pick == Some(pick),
                "Click a colour in the photo to add a point where it sits on the curve",
            ) {
                edits.push(toggle_pick(cx.pick, pick));
            }
        });
    });
    if ui.small_button(format!("Reset {}", CURVES[c].1)).on_hover_text("This curve back to a straight line").clicked() {
        edits.push(UiEdit::SetMany {
            node,
            key,
            values: adjust::curve_values(c, &[[0.0, 0.0], [1.0, 1.0]]),
            in_drag: false,
        });
    }
    let _ = painter.text(
        rect.left_top() + vec2(8.0, 6.0),
        Align2::LEFT_TOP,
        CURVES[c].1,
        FontId::proportional(FONT[0]),
        t.text_weak,
    );
}
