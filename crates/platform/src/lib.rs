//! Platform differences, in one place.
//!
//! Every `cfg!(windows)` / `cfg!(target_os = …)` of the project lives here.
//! Each decision is a pure function of an [`Os`] value, so all platforms are
//! tested on every CI runner; [`Os::current`] picks the one we run on.

use std::path::PathBuf;

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
pub fn config_dir(os: Os, app: &str, env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let base = match os {
        Os::Windows => env("APPDATA").and_then(absolute)?,
        Os::Linux => env("XDG_CONFIG_HOME")
            .and_then(absolute)
            .or_else(|| env("HOME").and_then(absolute).map(|h| h.join(".config")))?,
        Os::MacOs => env("HOME").and_then(absolute)?.join("Library").join("Application Support"),
    };
    Some(base.join(app))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

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
    fn config_dir_is_none_without_the_needed_variables() {
        assert_eq!(config_dir(Os::Windows, "rne", env(&[])), None);
        assert_eq!(config_dir(Os::Linux, "rne", env(&[])), None);
        assert_eq!(config_dir(Os::MacOs, "rne", env(&[])), None);
    }
}
