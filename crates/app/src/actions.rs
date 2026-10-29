//! Every command of the window, with its menu label and shortcuts.
//!
//! Menus, tooltips and keyboard handling all read this table, so a shortcut
//! shown in a menu is always the one that works.

use eframe::egui::{Key, KeyboardShortcut, Modifiers};

/// A command the user can give from a menu or the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// File ▸ Open Catalog…
    Open,
    /// File ▸ New Catalog…
    NewCatalog,
    /// File ▸ Import Images…: photos are imported into the catalog, never opened from the disk.
    Import,
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
    /// View ▸ Exposure warning (on/off), Ctrl+E as in Capture One.
    ExposureWarning,
    /// Image ▸ Variants ▸ New variant: the photo as it came from the
    /// camera, edited apart from its other variants.
    NewVariant,
    /// Image ▸ Variants ▸ Clone variant: a copy of the open variant.
    CloneVariant,
    /// Image ▸ Variants ▸ Delete variant…
    DeleteVariant,
}

const CMD: Modifiers = Modifiers::COMMAND;
const CMD_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);

const fn k(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const OPEN: &[KeyboardShortcut] = &[k(CMD, Key::O)];
const NEW_CATALOG: &[KeyboardShortcut] = &[k(CMD, Key::N)];
// Ctrl+Shift+I as Capture One's Import Images.
const IMPORT: &[KeyboardShortcut] = &[k(CMD_SHIFT, Key::I)];
// Ctrl+D as Capture One's Process; Ctrl+E is the exposure warning there and here.
const EXPORT: &[KeyboardShortcut] = &[k(CMD, Key::D)];
const EXPORT_AGAIN: &[KeyboardShortcut] = &[k(CMD_SHIFT, Key::D)];
const EXPOSURE_WARNING: &[KeyboardShortcut] = &[k(CMD, Key::E)];
const QUIT: &[KeyboardShortcut] = &[k(CMD, Key::Q)];
const UNDO: &[KeyboardShortcut] = &[k(CMD, Key::Z)];
const REDO: &[KeyboardShortcut] = &[k(CMD, Key::Y), k(CMD_SHIFT, Key::Z)];
// Comma and full stop as in Capture One, the Ctrl keys as before.
const ZOOM_FIT: &[KeyboardShortcut] = &[k(Modifiers::NONE, Key::Comma), k(CMD, Key::Num0)];
const ZOOM_100: &[KeyboardShortcut] = &[k(Modifiers::NONE, Key::Period), k(CMD, Key::Num1)];
const ZOOM_IN: &[KeyboardShortcut] = &[k(CMD, Key::Plus), k(CMD, Key::Equals)];
const ZOOM_OUT: &[KeyboardShortcut] = &[k(CMD, Key::Minus)];
const NEW_VARIANT: &[KeyboardShortcut] = &[k(Modifiers::NONE, Key::F7)];
const CLONE_VARIANT: &[KeyboardShortcut] = &[k(Modifiers::NONE, Key::F8)];
const DELETE_VARIANT: &[KeyboardShortcut] = &[k(CMD, Key::Delete)];

impl Command {
    /// Menu label, sentence case.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open Catalog\u{2026}",
            Self::NewCatalog => "New Catalog\u{2026}",
            Self::Import => "Import Images\u{2026}",
            Self::Export => "Export\u{2026}",
            Self::ExportAgain => "Export with previous settings",
            Self::Quit => "Quit",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::ZoomFit => "Fit",
            Self::Zoom100 => "100 %",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::ExposureWarning => "Exposure warning",
            Self::NewVariant => "New variant",
            Self::CloneVariant => "Clone variant",
            Self::DeleteVariant => "Delete variant\u{2026}",
        }
    }

    /// Shortcuts, the first one shown in menus.
    #[must_use]
    pub fn shortcuts(self) -> &'static [KeyboardShortcut] {
        match self {
            Self::Open => OPEN,
            Self::NewCatalog => NEW_CATALOG,
            Self::Import => IMPORT,
            Self::Export => EXPORT,
            Self::ExportAgain => EXPORT_AGAIN,
            Self::Quit => QUIT,
            Self::Undo => UNDO,
            Self::Redo => REDO,
            Self::ZoomFit => ZOOM_FIT,
            Self::Zoom100 => ZOOM_100,
            Self::ZoomIn => ZOOM_IN,
            Self::ZoomOut => ZOOM_OUT,
            Self::ExposureWarning => EXPOSURE_WARNING,
            Self::NewVariant => NEW_VARIANT,
            Self::CloneVariant => CLONE_VARIANT,
            Self::DeleteVariant => DELETE_VARIANT,
        }
    }

    /// Commands whose keys also edit text (Ctrl+Delete deletes a word, a
    /// comma or full stop is typed):
    /// not taken from a text field that has the keyboard.
    #[must_use]
    pub fn yields_to_text(self) -> bool {
        matches!(self, Self::NewVariant | Self::CloneVariant | Self::DeleteVariant | Self::ZoomFit | Self::Zoom100)
    }

    /// Every command.
    pub const ALL: [Self; 16] = [
        Self::Open,
        Self::NewCatalog,
        Self::Import,
        Self::Export,
        Self::ExportAgain,
        Self::Quit,
        Self::Undo,
        Self::Redo,
        Self::ZoomFit,
        Self::Zoom100,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ExposureWarning,
        Self::NewVariant,
        Self::CloneVariant,
        Self::DeleteVariant,
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
