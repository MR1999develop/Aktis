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
    /// File ▸ Export (to the remembered folder).
    Export,
    /// File ▸ Export to…
    ExportTo,
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

impl Command {
    /// Menu label, sentence case.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open\u{2026}",
            Self::Export => "Export",
            Self::ExportTo => "Export to\u{2026}",
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
        const fn k(m: Modifiers, key: Key) -> KeyboardShortcut {
            KeyboardShortcut::new(m, key)
        }
        match self {
            Self::Open => &[k(CMD, Key::O)],
            Self::Export => &[k(CMD, Key::E)],
            Self::ExportTo => &[k(CMD_SHIFT, Key::E)],
            Self::Quit => &[k(CMD, Key::Q)],
            Self::Undo => &[k(CMD, Key::Z)],
            Self::Redo => &[k(CMD, Key::Y), k(CMD_SHIFT, Key::Z)],
            Self::ZoomFit => &[k(CMD, Key::Num0)],
            Self::Zoom100 => &[k(CMD, Key::Num1)],
            Self::ZoomIn => &[k(CMD, Key::Plus), k(CMD, Key::Equals)],
            Self::ZoomOut => &[k(CMD, Key::Minus)],
        }
    }

    /// Every command.
    pub const ALL: [Self; 10] = [
        Self::Open,
        Self::Export,
        Self::ExportTo,
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
