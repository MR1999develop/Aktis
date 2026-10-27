//! Platform differences, in one place.
//!
//! Every `cfg!(windows)` / `cfg!(target_os = …)` of the project lives here.
//! Each decision is a pure function of an [`Os`] value, so all platforms are
//! tested on every CI runner; [`Os::current`] picks the one we run on.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Name of the editor's folder for profiles and lens data inside
/// [`config_dir`]; the same as the window's application id (`app::APP_ID`).
pub const APP_DIR: &str = "raw-node-editor";

/// Operating systems the editor knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows 10/11.
    Windows,
    /// Linux desktops (Ubuntu/GNOME is the reference).
    Linux,
    /// macOS (prepared, not built in version 1).
    MacOs,
}

impl Os {
    /// The system this build runs on.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// Order of the affirmative and cancel buttons at the bottom of a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonOrder {
    /// `[Action] [Cancel]`, as on Windows.
    ActionFirst,
    /// `[Cancel] [Action]`, action rightmost, as on GNOME and macOS.
    ActionLast,
}

/// The dialog button order of `os`.
#[must_use]
pub fn button_order(os: Os) -> ButtonOrder {
    match os {
        Os::Windows => ButtonOrder::ActionFirst,
        Os::Linux | Os::MacOs => ButtonOrder::ActionLast,
    }
}

/// Folder for settings and the catalog, following each system's convention:
/// `%APPDATA%\<app>` on Windows, `$XDG_CONFIG_HOME/<app>` or `~/.config/<app>`
/// on Linux, `~/Library/Application Support/<app>` on macOS.
///
/// `env` looks up environment variables; pass `|k| std::env::var_os(k)` in
/// the application and a table in tests. Returns `None` when the variables a
/// system needs are missing or relative.
#[must_use]
pub fn config_dir(os: Os, app: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let base = match os {
        Os::Windows => env("APPDATA").and_then(absolute)?,
        Os::Linux => env("XDG_CONFIG_HOME")
            .and_then(absolute)
            .or_else(|| env("HOME").and_then(absolute).map(|h| h.join(".config")))?,
        Os::MacOs => env("HOME").and_then(absolute)?.join("Library").join("Application Support"),
    };
    Some(base.join(app))
}

/// A program to start, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// Program name or path.
    pub program: &'static str,
    /// Arguments, passed as they are (no shell).
    pub args: Vec<OsString>,
}

/// How to show `file` in the system's file manager: selected in Explorer on
/// Windows and in Finder on macOS; on Linux the containing folder is opened
/// with the desktop's default handler (selection is not portable there).
#[must_use]
pub fn reveal_in_file_manager(os: Os, file: &Path) -> Launch {
    match os {
        Os::Windows => {
            // Explorer wants "/select," and the path as one argument.
            let mut arg = OsString::from("/select,");
            arg.push(file.as_os_str());
            Launch { program: "explorer", args: vec![arg] }
        }
        Os::MacOs => Launch { program: "open", args: vec![OsString::from("-R"), file.as_os_str().to_owned()] },
        Os::Linux => {
            let folder = file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            Launch { program: "xdg-open", args: vec![folder.as_os_str().to_owned()] }
        }
    }
}

/// Primary modifier name shown in menus and tooltips ("Ctrl" or "Cmd").
#[must_use]
pub fn command_key_name(os: Os) -> &'static str {
    match os {
        Os::MacOs => "Cmd",
        Os::Windows | Os::Linux => "Ctrl",
    }
}

/// Folders searched for DCP camera profiles: only the editor's own
/// (`<config_dir>/profiles`). Profiles of other programs are deliberately
/// not looked for; copy a `.dcp` file there to use it.
#[must_use]
pub fn camera_profile_dirs(os: Os, app: &str, env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    config_dir(os, app, env).map(|d| d.join("profiles")).into_iter().collect()
}

/// Folder for newer lensfun database files that override the built-in ones
/// (`<config_dir>/lensfun`).
#[must_use]
pub fn lensfun_dir(os: Os, app: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    config_dir(os, app, env).map(|d| d.join("lensfun"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| vars.iter().find(|(name, _)| *name == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn dialog_buttons_follow_each_platform() {
        assert_eq!(button_order(Os::Windows), ButtonOrder::ActionFirst);
        assert_eq!(button_order(Os::Linux), ButtonOrder::ActionLast);
        assert_eq!(button_order(Os::MacOs), ButtonOrder::ActionLast);
    }

    // What counts as an absolute path depends on the machine running the test,
    // so each system's rules are checked on a runner of that system.

    #[test]
    #[cfg(unix)]
    fn config_dir_follows_xdg_then_home_on_linux_and_library_on_macos() {
        let with_xdg = config_dir(Os::Linux, "rne", env(&[("XDG_CONFIG_HOME", "/cfg"), ("HOME", "/home/m")]));
        assert_eq!(with_xdg, Some(PathBuf::from("/cfg/rne")));
        let home_only = config_dir(Os::Linux, "rne", env(&[("HOME", "/home/m")]));
        assert_eq!(home_only, Some(PathBuf::from("/home/m/.config/rne")));
        let relative_xdg = config_dir(Os::Linux, "rne", env(&[("XDG_CONFIG_HOME", "cfg"), ("HOME", "/home/m")]));
        assert_eq!(relative_xdg, Some(PathBuf::from("/home/m/.config/rne")));
        let mac = config_dir(Os::MacOs, "rne", env(&[("HOME", "/Users/m")]));
        assert_eq!(mac, Some(PathBuf::from("/Users/m/Library/Application Support/rne")));
    }

    #[test]
    #[cfg(windows)]
    fn config_dir_uses_appdata_on_windows_and_refuses_a_relative_one() {
        let roaming = r"C:\Users\m\AppData\Roaming";
        assert_eq!(
            config_dir(Os::Windows, "rne", env(&[("APPDATA", roaming)])),
            Some(PathBuf::from(r"C:\Users\m\AppData\Roaming\rne"))
        );
        assert_eq!(config_dir(Os::Windows, "rne", env(&[("APPDATA", "Roaming")])), None);
    }

    #[test]
    #[cfg(unix)]
    fn profiles_and_lens_data_live_in_the_editors_own_folder_only() {
        let linux = camera_profile_dirs(Os::Linux, "rne", env(&[("HOME", "/home/m")]));
        assert_eq!(linux, vec![PathBuf::from("/home/m/.config/rne/profiles")]);
        assert!(camera_profile_dirs(Os::Linux, "rne", env(&[])).is_empty());
        assert_eq!(
            lensfun_dir(Os::Linux, "rne", env(&[("HOME", "/home/m")])),
            Some(PathBuf::from("/home/m/.config/rne/lensfun"))
        );
    }

    #[test]
    fn config_dir_is_none_without_the_needed_variables() {
        assert_eq!(config_dir(Os::Windows, "rne", env(&[])), None);
        assert_eq!(config_dir(Os::Linux, "rne", env(&[])), None);
        assert_eq!(config_dir(Os::MacOs, "rne", env(&[])), None);
    }

    #[test]
    fn reveal_selects_the_file_on_windows_and_macos_and_opens_its_folder_on_linux() {
        let file = Path::new("photos/with space/a.jpg");
        assert_eq!(
            reveal_in_file_manager(Os::Windows, file),
            Launch { program: "explorer", args: vec![OsString::from("/select,photos/with space/a.jpg")] }
        );
        assert_eq!(
            reveal_in_file_manager(Os::MacOs, file),
            Launch { program: "open", args: vec![OsString::from("-R"), OsString::from("photos/with space/a.jpg")] }
        );
        assert_eq!(
            reveal_in_file_manager(Os::Linux, file),
            Launch { program: "xdg-open", args: vec![OsString::from("photos/with space")] }
        );
        assert_eq!(reveal_in_file_manager(Os::Linux, Path::new("a.jpg")).args, vec![OsString::from(".")]);
    }

    #[test]
    fn the_command_key_is_cmd_only_on_macos() {
        assert_eq!(command_key_name(Os::Windows), "Ctrl");
        assert_eq!(command_key_name(Os::Linux), "Ctrl");
        assert_eq!(command_key_name(Os::MacOs), "Cmd");
    }
}
