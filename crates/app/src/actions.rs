//! Every command of the window, with its menu label and shortcuts.
//!
//! Menus, tooltips and keyboard handling all read this table, so a shortcut
//! shown in a menu is always the one that works.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

/// A command the user can give from a menu or the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// File ▸ Open…
    Open,
    /// File ▸ Export…: the export window.
    Export,
    /// File ▸ Export with previous settings: no window.
    ExportAgain,
    /// File ▸ Quit.
    Quit,
    /// Edit ▸ Undo.
    Undo,
    /// Edit ▸ Redo.
    Redo,
    /// View ▸ Fit.
    ZoomFit,
    /// View ▸ 100 %.
    Zoom100,
    /// View ▸ Zoom in.
    ZoomIn,
    /// View ▸ Zoom out.
    ZoomOut,
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);

const fn k(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const OPEN: &[KeyboardShortcut] = &[k(CMD, Key::O)];
const EXPORT: &[KeyboardShortcut] = &[k(CMD, Key::E)];
const EXPORT_AGAIN: &[KeyboardShortcut] = &[k(CMD_SHIFT, Key::E)];
const QUIT: &[KeyboardShortcut] = &[k(CMD, Key::Q)];
const UNDO: &[KeyboardShortcut] = &[k(CMD, Key::Z)];
const REDO: &[KeyboardShortcut] = &[k(CMD, Key::Y), k(CMD_SHIFT, Key::Z)];
const ZOOM_FIT: &[KeyboardShortcut] = &[k(CMD, Key::Num0)];
const ZOOM_100: &[KeyboardShortcut] = &[k(CMD, Key::Num1)];
const ZOOM_IN: &[KeyboardShortcut] = &[k(CMD, Key::Plus), k(CMD, Key::Equals)];
const ZOOM_OUT: &[KeyboardShortcut] = &[k(CMD, Key::Minus)];

impl Command {
    /// Menu label, sentence case.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open\u{2026}",
            Self::Export => "Export\u{2026}",
            Self::ExportAgain => "Export with previous settings",
            Self::Quit => "Quit",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::ZoomFit => "Fit",
            Self::Zoom100 => "100 %",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
        }
    }

    /// Shortcuts, the first one shown in menus.
    #[must_use]
    pub fn shortcuts(self) -> &'static [KeyboardShortcut] {
        match self {
            Self::Open => OPEN,
            Self::Export => EXPORT,
            Self::ExportAgain => EXPORT_AGAIN,
            Self::Quit => QUIT,
            Self::Undo => UNDO,
            Self::Redo => REDO,
            Self::ZoomFit => ZOOM_FIT,
            Self::Zoom100 => ZOOM_100,
            Self::ZoomIn => ZOOM_IN,
            Self::ZoomOut => ZOOM_OUT,
        }
    }

    /// Every command.
    pub const ALL: [Self; 10] = [
        Self::Open,
        Self::Export,
        Self::ExportAgain,
        Self::Quit,
        Self::Undo,
        Self::Redo,
        Self::ZoomFit,
        Self::Zoom100,
        Self::ZoomIn,
        Self::ZoomOut,
    ];

    /// Every shortcut with its command, those with Shift first: egui's
    /// matching ignores an extra Shift, so Ctrl+Z must not see Shift+Ctrl+Z first.
    #[must_use]
    pub fn shortcut_table() -> Vec<(KeyboardShortcut, Self)> {
        let mut table: Vec<_> = Self::ALL.iter().flat_map(|c| c.shortcuts().iter().map(move |s| (*s, *c))).collect();
        table.sort_by_key(|(s, _)| !s.modifiers.shift);
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_commands_share_a_shortcut() {
        let table = Command::shortcut_table();
        for (i, (a, ca)) in table.iter().enumerate() {
            for (b, cb) in &table[i + 1..] {
                assert!(a != b || ca == cb, "{a:?} is used by {ca:?} and {cb:?}");
            }
        }
    }

    #[test]
    fn shift_shortcuts_are_checked_first() {
        let table = Command::shortcut_table();
        let first_plain = table.iter().position(|(s, _)| !s.modifiers.shift).unwrap_or(table.len());
        assert!(table[first_plain..].iter().all(|(s, _)| !s.modifiers.shift));
        assert!(table[..first_plain].iter().any(|(_, c)| *c == Command::Redo));
    }

    #[test]
    fn every_command_has_a_shortcut_and_a_label() {
        for c in Command::ALL {
            assert!(!c.shortcuts().is_empty(), "{c:?}");
            assert!(!c.label().is_empty());
        }
    }
}
