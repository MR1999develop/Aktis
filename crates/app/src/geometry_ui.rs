//! The geometry tools of the bar, drawn over the photo:
//!
//! - **Crop** (C), as in a photo viewer: a box with a rule-of-thirds grid
//!   and eight handles over the dimmed frame; drag inside to move it;
//!   aspect presets (Unconstrained, Original, 1x1 … 65x24, custom) and a
//!   portrait/landscape switch in the options bar. Enter or a double-click
//!   inside applies, Esc cancels; leaving the tool applies too.
//! - **Straighten** (R), as in Capture One: drag along something that
//!   should be level or plumb (the nearer of the two is used); on release
//!   the photo turns by that much and is enlarged to hide the empty corners.
//!   An angle slider in the options bar; ←/→ turn by 0.01°, with Shift 0.1°.
//! - **Perspective** (K): vertical and horizontal keystone sliders.
//!
//! Level and Perspective show a grid over the photo. Every change is one
//! undo step ([`GeoEdit`]); the maths is in [`editor::crop`] and
//! [`core_model::Geometry`].

use core_model::{Geometry, NormPoint, NormRect};
use editor::crop::{self, ASPECTS, Handle};
use eframe::egui::{self, Align2, CornerRadius, FontId, Id, Order, Pos2, Rect, Response, Stroke, vec2};

use crate::theme::{FONT, SPACE, Tokens};

/// Degrees one arrow key turns the photo in Straighten (Shift: ten times).
pub const FINE_TURN: f32 = 0.01;
use crate::toolbar::ViewerTool;

/// A change of the recipe's geometry asked for by a tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeoEdit {
    /// A slider drag started: the next changes form one undo step.
    Begin,
    /// New settings (`in_drag`: part of the drag started with `Begin`).
    Set {
        /// The settings.
        geometry: Geometry,
        /// Part of the current drag.
        in_drag: bool,
    },
    /// The slider drag ended.
    End,
}

/// A drag on the crop box.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BoxDrag {
    handle: Handle,
    from: Pos2,
    start: [f32; 4],
}

/// State of the geometry tools between frames.
#[derive(Debug, Default)]
pub struct GeometryUi {
    /// The crop box being edited (normalised `[x, y, w, h]` of the frame).
    crop: Option<[f32; 4]>,
    /// Index into [`ASPECTS`], or `None` for a custom ratio.
    aspect: Option<usize>,
    /// A ratio typed by the user (width / height).
    custom: Option<f32>,
    /// The text of the custom ratio field.
    custom_text: String,
    /// Portrait: the ratio is turned round.
    portrait: bool,
    drag: Option<BoxDrag>,
    /// Where the Level line started.
    level_from: Option<Pos2>,
}

fn rect_of(b: [f32; 4]) -> Option<NormRect> {
    Some(NormRect::new(NormPoint::new(b[0], b[1])?, NormPoint::new(b[0] + b[2], b[1] + b[3])?))
}

impl GeometryUi {
    /// The crop tool was entered: the box starts at the recipe's crop.
    pub fn start_crop(&mut self, g: &Geometry) {
        let c = g.crop_or_full();
        self.crop = Some([c.min().x(), c.min().y(), c.width(), c.height()]);
        self.drag = None;
        if self.aspect.is_none() && self.custom.is_none() {
            self.aspect = Some(0);
        }
    }

    /// The crop tool is left: the geometry with the edited box when
    /// `apply`, else nothing (the box is dropped).
    pub fn finish_crop(&mut self, g: &Geometry, apply: bool) -> Option<Geometry> {
        let b = self.crop.take()?;
        self.drag = None;
        apply.then(|| Geometry { crop: rect_of(b), ..*g }.sanitized())
    }

    /// The chosen ratio (width / height of the result) for a frame `aspect`.
    fn ratio(&self, frame_aspect: f32) -> Option<f32> {
        let r = match (self.aspect, self.custom) {
            (Some(i), _) => match ASPECTS.get(i)?.ratio? {
                0.0 => frame_aspect,
                r => r,
            },
            (None, Some(r)) => r,
            (None, None) => return None,
        };
        Some(if self.portrait { 1.0 / r } else { r })
    }

    fn aspect_label(&self) -> String {
        let base = match (self.aspect, self.custom) {
            (Some(i), _) => ASPECTS.get(i).map_or("Unconstrained", |a| a.label).to_owned(),
            (None, Some(r)) => format!("Custom {r:.3}"),
            (None, None) => "Unconstrained".to_owned(),
        };
        if self.portrait { format!("{base} \u{21c4}") } else { base }
    }

    /// Sets the box to the chosen ratio, centred.
    fn apply_ratio(&mut self, frame_aspect: f32) {
        if let Some(r) = self.ratio(frame_aspect) {
            self.crop = Some(crop::centred(crop::norm_ratio(r, frame_aspect)));
        }
    }

    /// The crop overlay over the photo shown in `screen` (the whole frame,
    /// `frame_aspect` = width / height). Returns whether the pointer drag
    /// belongs to the crop box (so the viewer does not pan), and the box
    /// to apply when the user double-clicked inside it.
    pub fn crop_overlay(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        screen: Rect,
        frame_aspect: f32,
    ) -> (bool, bool) {
        let t = Tokens::of(ui.ctx().theme());
        let Some(b) = self.crop else { return (false, false) };
        let to_screen = |x: f32, y: f32| screen.min + vec2(x * screen.width(), y * screen.height());
        let boxed = Rect::from_min_max(to_screen(b[0], b[1]), to_screen(b[0] + b[2], b[1] + b[3]));
        let painter = ui.painter_at(response.rect);
        // Dim the frame outside the box.
        let dim = crate::theme::CROP_DIM;
        let full = response.rect;
        for r in [
            Rect::from_min_max(full.min, egui::pos2(full.max.x, boxed.min.y)),
            Rect::from_min_max(egui::pos2(full.min.x, boxed.max.y), full.max),
            Rect::from_min_max(egui::pos2(full.min.x, boxed.min.y), egui::pos2(boxed.min.x, boxed.max.y)),
            Rect::from_min_max(egui::pos2(boxed.max.x, boxed.min.y), egui::pos2(full.max.x, boxed.max.y)),
        ] {
            painter.rect_filled(r, CornerRadius::ZERO, dim);
        }
        painter.rect_stroke(boxed, CornerRadius::ZERO, Stroke::new(1.5, t.text_strong), egui::StrokeKind::Inside);
        let grid = Stroke::new(1.0, t.text_strong.gamma_multiply(0.27));
        for k in 1..3 {
            #[allow(clippy::cast_precision_loss)] // two lines
            let f = k as f32 / 3.0;
            let x = boxed.min.x + f * boxed.width();
            let y = boxed.min.y + f * boxed.height();
            painter.line_segment([egui::pos2(x, boxed.min.y), egui::pos2(x, boxed.max.y)], grid);
            painter.line_segment([egui::pos2(boxed.min.x, y), egui::pos2(boxed.max.x, y)], grid);
        }
        let handle_at = |h: Handle| {
            let a = h.anchor();
            egui::pos2(boxed.min.x + a[0] * boxed.width(), boxed.min.y + a[1] * boxed.height())
        };
        for h in Handle::RESIZE {
            painter.rect_filled(
                Rect::from_center_size(handle_at(h), vec2(12.0, 12.0)),
                CornerRadius::same(1),
                t.text_strong,
            );
        }
        let label = format!("{:.0} \u{d7} {:.0} %", b[2] * 100.0, b[3] * 100.0);
        painter.text(
            boxed.left_top() + vec2(4.0, -4.0),
            Align2::LEFT_BOTTOM,
            label,
            FontId::proportional(FONT[0]),
            t.text_strong,
        );

        // Which part of the box the pointer is over.
        let over = |p: Pos2| {
            Handle::RESIZE
                .into_iter()
                .find(|h| handle_at(*h).distance(p) <= 10.0)
                .or_else(|| boxed.shrink(12.0).contains(p).then_some(Handle::Move))
        };
        if let Some(p) = response.hover_pos()
            && let Some(h) = self.drag.map(|d| d.handle).or_else(|| over(p))
        {
            let icon = match h {
                Handle::Move => egui::CursorIcon::Move,
                Handle::N | Handle::S => egui::CursorIcon::ResizeVertical,
                Handle::E | Handle::W => egui::CursorIcon::ResizeHorizontal,
                Handle::Ne | Handle::Sw => egui::CursorIcon::ResizeNeSw,
                Handle::Nw | Handle::Se => egui::CursorIcon::ResizeNwSe,
            };
            ui.ctx().set_cursor_icon(icon);
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = response.interact_pointer_pos()
            && let Some(handle) = over(p)
        {
            self.drag = Some(BoxDrag { handle, from: p, start: b });
        }
        let ratio = self.ratio(frame_aspect).map(|r| crop::norm_ratio(r, frame_aspect));
        if let Some(d) = self.drag
            && let Some(p) = response.interact_pointer_pos()
        {
            let delta = [(p.x - d.from.x) / screen.width().max(1.0), (p.y - d.from.y) / screen.height().max(1.0)];
            self.crop = Some(crop::drag(d.start, d.handle, delta, ratio));
        }
        let owned = self.drag.is_some();
        if response.drag_stopped() {
            self.drag = None;
        }
        let apply = response.double_clicked() && response.interact_pointer_pos().is_some_and(|p| boxed.contains(p));
        (owned, apply)
    }

    /// The options bar of the crop tool, at the top of the viewer.
    /// Returns `Some(true)` for Apply, `Some(false)` for Cancel.
    pub fn crop_options(&mut self, ctx: &egui::Context, viewer: Rect, frame_aspect: f32) -> Option<bool> {
        let mut result = None;
        options_bar(ctx, viewer, "rne-crop-options", |ui| {
            ui.label("Aspect");
            egui::ComboBox::from_id_salt("rne-crop-aspect").selected_text(self.aspect_label()).show_ui(ui, |ui| {
                for (i, a) in ASPECTS.iter().enumerate() {
                    if ui.selectable_label(self.aspect == Some(i), a.label).clicked() {
                        self.aspect = Some(i);
                        self.custom = None;
                        self.apply_ratio(frame_aspect);
                    }
                }
            });
            let field = ui.add(egui::TextEdit::singleline(&mut self.custom_text).hint_text("16x9").desired_width(56.0));
            if field.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some(r) = crop::parse_ratio(&self.custom_text)
            {
                self.aspect = None;
                self.custom = Some(r);
                self.apply_ratio(frame_aspect);
            }
            field.on_hover_text("A custom ratio, like 16x9 or 1.85; Enter to use it");
            if ui.button("\u{21c4}").on_hover_text("Portrait or landscape").clicked() {
                self.portrait = !self.portrait;
                if self.ratio(frame_aspect).is_some() {
                    self.apply_ratio(frame_aspect);
                } else if let Some(b) = self.crop {
                    // Free box: swap its sides around its centre, inside the frame.
                    let (cx, cy) = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
                    let (w, h) = (b[3].min(1.0), b[2].min(1.0));
                    self.crop = Some([(cx - w / 2.0).clamp(0.0, 1.0 - w), (cy - h / 2.0).clamp(0.0, 1.0 - h), w, h]);
                }
            }
            ui.separator();
            if ui.button("Reset").on_hover_text("The whole frame").clicked() {
                self.crop = Some([0.0, 0.0, 1.0, 1.0]);
            }
            if ui.button("Cancel").on_hover_text("Esc").clicked() {
                result = Some(false);
            }
            if ui
                .button(egui::RichText::new("Apply").strong())
                .on_hover_text("Enter, or double-click inside the box")
                .clicked()
            {
                result = Some(true);
            }
        });
        result
    }

    /// The Level tool over the photo shown in `screen` (`shown` = its size
    /// in pixels): a line follows the drag; on release, the new geometry
    /// that makes the line level. Returns whether the drag is the tool's.
    pub fn level_overlay(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        screen: Rect,
        shown: [f32; 2],
        g: &Geometry,
        edits: &mut Vec<GeoEdit>,
    ) -> bool {
        let t = Tokens::of(ui.ctx().theme());
        grid_overlay(ui, screen, &t);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.level_from = response.interact_pointer_pos();
        }
        let Some(from) = self.level_from else { return false };
        if let Some(to) = response.interact_pointer_pos().or_else(|| response.hover_pos()) {
            ui.painter_at(response.rect).line_segment([from, to], Stroke::new(2.0, t.accent));
            if response.drag_stopped() {
                let px = |p: Pos2| {
                    [
                        (p.x - screen.min.x) / screen.width() * shown[0],
                        (p.y - screen.min.y) / screen.height() * shown[1],
                    ]
                };
                let turn = core_model::level_angle(px(from), px(to));
                if from.distance(to) > 8.0 {
                    let geometry = Geometry { angle: g.angle + turn, ..*g }.sanitized();
                    edits.push(GeoEdit::Set { geometry, in_drag: false });
                }
            }
        }
        if response.drag_stopped() {
            self.level_from = None;
        }
        true
    }

    /// The arrow keys of Straighten, as in Capture One: ←/→ turn the photo by
    /// [`FINE_TURN`] degrees, with Shift ten times as much.
    pub fn level_keys(ctx: &egui::Context, g: &Geometry, edits: &mut Vec<GeoEdit>) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        let (left, right) = ctx.input_mut(|i| {
            let mut key =
                |k: egui::Key| i.consume_key(egui::Modifiers::NONE, k) || i.consume_key(egui::Modifiers::SHIFT, k);
            (key(egui::Key::ArrowLeft), key(egui::Key::ArrowRight))
        });
        let turns = f32::from(u8::from(right)) - f32::from(u8::from(left));
        if turns != 0.0 {
            let step = if shift { FINE_TURN * 10.0 } else { FINE_TURN };
            // Rounded to the step, so repeated presses stay on round values.
            let angle = ((g.angle + turns * step) / FINE_TURN).round() * FINE_TURN;
            edits.push(GeoEdit::Set { geometry: Geometry { angle, ..*g }.sanitized(), in_drag: false });
        }
    }

    /// The options bar of Level and Perspective: angle, or the two keystones.
    pub fn sliders(&self, ctx: &egui::Context, viewer: Rect, tool: ViewerTool, g: &Geometry, edits: &mut Vec<GeoEdit>) {
        options_bar(ctx, viewer, "rne-geometry-options", |ui| {
            let mut next = *g;
            let mut slider =
                |ui: &mut egui::Ui, label: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str| {
                    ui.label(label);
                    let r = ui
                        .add(egui::Slider::new(v, range).suffix(suffix).fixed_decimals(2).update_while_editing(false));
                    if r.drag_started() {
                        edits.push(GeoEdit::Begin);
                    }
                    (r.changed(), r.dragged() || r.drag_started(), r.drag_stopped())
                };
            let mut changes = Vec::new();
            match tool {
                ViewerTool::Level => {
                    changes.push(slider(
                        ui,
                        "Angle",
                        &mut next.angle,
                        -core_model::MAX_ANGLE..=core_model::MAX_ANGLE,
                        "\u{b0}",
                    ));
                    ui.label(
                        egui::RichText::new(
                            "or drag along a line that should be level or plumb; \u{2190}/\u{2192} fine-tune",
                        )
                        .weak(),
                    );
                }
                _ => {
                    changes.push(slider(ui, "Vertical", &mut next.vertical, -100.0..=100.0, ""));
                    changes.push(slider(ui, "Horizontal", &mut next.horizontal, -100.0..=100.0, ""));
                }
            }
            for (changed, in_drag, stopped) in changes {
                if changed {
                    edits.push(GeoEdit::Set { geometry: next.sanitized(), in_drag });
                }
                if stopped {
                    edits.push(GeoEdit::End);
                }
            }
            if ui.button("Reset").clicked() {
                let geometry = match tool {
                    ViewerTool::Level => Geometry { angle: 0.0, ..*g },
                    _ => Geometry { vertical: 0.0, horizontal: 0.0, ..*g },
                };
                edits.push(GeoEdit::Set { geometry, in_drag: false });
            }
        });
    }
}

/// A rule-of-thirds and fine grid over the photo, for judging lines.
pub fn grid_overlay(ui: &egui::Ui, screen: Rect, t: &Tokens) {
    let painter = ui.painter_at(screen);
    let fine = Stroke::new(1.0, t.text_strong.gamma_multiply(0.18));
    for k in 1..12 {
        #[allow(clippy::cast_precision_loss)] // grid lines
        let f = k as f32 / 12.0;
        let x = screen.min.x + f * screen.width();
        let y = screen.min.y + f * screen.height();
        painter.line_segment([egui::pos2(x, screen.min.y), egui::pos2(x, screen.max.y)], fine);
        painter.line_segment([egui::pos2(screen.min.x, y), egui::pos2(screen.max.x, y)], fine);
    }
}

/// A floating bar at the top centre of the viewer (also the Heal and Clone tools').
pub(crate) fn options_bar(ctx: &egui::Context, viewer: Rect, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::of(ctx.theme());
    egui::Area::new(Id::unique(id))
        .order(Order::Foreground)
        .fixed_pos(egui::pos2(viewer.center().x, viewer.top() + SPACE[1]))
        .pivot(Align2::CENTER_TOP)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.bg_overlay)
                .stroke(Stroke::new(1.0, t.stroke_weak))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(add);
                });
        });
}
