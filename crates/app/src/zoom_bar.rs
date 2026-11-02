//! The zoom controls at the right of the status bar: Zoom out, the zoom in
//! percent (typed or dragged), the zoom levels of Capture One's zoom menu,
//! Zoom in, Fit, 100 % and Keep zoom. Plain text only: the app's font has no
//! arrows or icons.

use eframe::egui::{self, Button};

use crate::actions::Command;

/// Range of the zoom field, in percent, up to [`editor::view::MAX_PIXEL_ZOOM`];
/// a value at or below fit shows the photo fitted.
const PERCENT: std::ops::RangeInclusive<f32> = 1.0..=editor::view::MAX_PIXEL_ZOOM * 100.0;

/// What the zoom controls asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomAction {
    /// The whole photo.
    Fit,
    /// Screen pixels per image pixel (1.0 = 100 %).
    Scale(f32),
    /// One zoom level up (`true`) or down.
    Step(bool),
    /// Keep zoom switched on (`true`) or off: another photo opens at the same
    /// zoom and place.
    Keep(bool),
}

/// Draws the controls in a right-to-left layout (the status bar's right
/// side), so they read left to right as listed above. `percent` is the zoom
/// now, `keep` whether Keep zoom is on.
pub fn zoom_controls(ui: &mut egui::Ui, percent: f32, keep: bool) -> Option<ZoomAction> {
    let mut action = None;
    let keys = |command: Command| ui.ctx().format_shortcut(&command.shortcuts()[0]);
    let (fit_keys, actual_keys, in_keys, out_keys) =
        (keys(Command::ZoomFit), keys(Command::Zoom100), keys(Command::ZoomIn), keys(Command::ZoomOut));

    let mut kept = keep;
    if ui
        .toggle_value(&mut kept, "Keep zoom")
        .on_hover_text("Keep the zoom and position when another photo opens, to compare the same detail")
        .changed()
    {
        action = Some(ZoomAction::Keep(kept));
    }
    if ui.button("100%").on_hover_text(format!("One screen pixel per image pixel ({actual_keys})")).clicked() {
        action = Some(ZoomAction::Scale(1.0));
    }
    if ui.button("Fit").on_hover_text(format!("The whole photo in the viewer ({fit_keys})")).clicked() {
        action = Some(ZoomAction::Fit);
    }
    if ui.button("+").on_hover_text(format!("Zoom in ({in_keys}, or the wheel)")).clicked() {
        action = Some(ZoomAction::Step(true));
    }
    // The zoom levels, as in Capture One's zoom menu.
    ui.menu_button("Levels", |ui| {
        if ui.add(Button::new("Fit").shortcut_text(fit_keys.as_str())).clicked() {
            action = Some(ZoomAction::Fit);
            ui.close();
        }
        for level in editor::view::ZOOM_LEVELS {
            let label = format!("{:.0} %", level * 100.0);
            let button = if (level - 1.0).abs() < 1e-6 {
                Button::new(label).shortcut_text(actual_keys.as_str())
            } else {
                Button::new(label)
            };
            if ui.add(button).clicked() {
                action = Some(ZoomAction::Scale(level));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("The zoom levels of Capture One's zoom menu");
    let mut typed = percent;
    if ui
        .add(egui::DragValue::new(&mut typed).range(PERCENT).speed(1.0).max_decimals(0).suffix(" %"))
        .on_hover_text("Zoom: type a value or drag; at or below fit the photo is fitted")
        .changed()
    {
        action = Some(ZoomAction::Scale(typed / 100.0));
    }
    if ui.button("\u{2212}").on_hover_text(format!("Zoom out ({out_keys}, or the wheel)")).clicked() {
        action = Some(ZoomAction::Step(false));
    }
    action
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Align, Layout};
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// Draws the controls with the app's theme as the status bar does:
    /// every label has its glyphs (the harness fails on a missing one) and
    /// they read left to right in the order of the module's description.
    #[test]
    fn the_controls_are_drawn_left_to_right_with_the_app_fonts() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size([600.0, 40.0])
            .with_context_setup(crate::theme::install)
            .build_ui(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = zoom_controls(ui, 12.5, false);
                });
            });
        harness.run_steps(2);
        let left = |label: &str| harness.get_by_label(label).rect().left();
        let order = ["\u{2212}", "Levels", "+", "Fit", "100%", "Keep zoom"].map(left);
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    }
}
