//! Dialogs: a centred box above the window and the buttons at its foot,
//! in the order of the system the editor runs on.

use eframe::egui::{self, Align, Button, Id, Key, Layout, Margin, RichText};

use crate::theme::{SPACE, Tokens};

/// Width of the dialogs.
const DIALOG_WIDTH: f32 = 460.0;

/// The button the user chose, or the key that stands for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The dialog's action (its own button, or Enter).
    Action,
    /// Cancel, Close, Not now, Stop (its own button, or Esc).
    Cancel,
}

/// A centred dialog above the window; the window behind it does not take
/// clicks or keys while it is open (a modal).
pub fn dialog(ctx: &egui::Context, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    dialog_with_width(ctx, id, DIALOG_WIDTH, add);
}

/// A centred modal dialog `width` points wide, for dialogs with many options.
/// Clicking outside does not close it: its buttons (or Enter and Esc) do.
pub fn dialog_with_width(ctx: &egui::Context, id: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let t = Tokens::of(ctx.theme());
    egui::Modal::new(Id::unique(id)).frame(egui::Frame::NONE).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(t.bg_raised).inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.set_width(width);
            add(ui);
        });
    });
}

/// The buttons at the foot of a dialog, on the right, in the system's order
/// (Windows `[Action] [Cancel]`, GNOME and macOS `[Cancel] [Action]`).
/// `action` is the verb of the main button, `None` when there is nothing to
/// do but cancel (a download running: `cancel` is then "Stop"). Enter runs
/// the action, Esc cancels. `extra` adds buttons on the left, such as
/// "Open folder".
pub fn dialog_buttons(
    ui: &mut egui::Ui,
    t: &Tokens,
    action: Option<&str>,
    cancel: &str,
    extra: impl FnOnce(&mut egui::Ui),
) -> Option<Choice> {
    let order = platform::button_order(platform::Os::current());
    let (acted, cancelled) = ui
        .horizontal(|ui| {
            extra(ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = SPACE[1];
                // Right to left: the first button added is the rightmost.
                let act = |ui: &mut egui::Ui| {
                    action.is_some_and(|label| {
                        ui.add(Button::new(RichText::new(label).color(t.on_accent)).fill(t.accent)).clicked()
                    })
                };
                let cancel_button = |ui: &mut egui::Ui| ui.button(cancel).clicked();
                match order {
                    platform::ButtonOrder::ActionFirst => {
                        let c = cancel_button(ui);
                        (act(ui), c)
                    }
                    platform::ButtonOrder::ActionLast => {
                        let a = act(ui);
                        (a, cancel_button(ui))
                    }
                }
            })
            .inner
        })
        .inner;
    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if cancelled || esc {
        Some(Choice::Cancel)
    } else if acted || (enter && action.is_some()) {
        Some(Choice::Action)
    } else {
        None
    }
}
