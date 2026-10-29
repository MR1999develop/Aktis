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

/// How to move `path` (a folder when `is_dir`, else a file) to the
/// system's wastebasket, from where the user can restore it: the Recycle
/// Bin on Windows (through PowerShell and .NET's `FileSystem`, the path
/// handed over in Base64 so no character of it is read as a command), the
/// freedesktop Trash on Linux (`gio trash`), the Trash on macOS (Finder).
/// The program exits with a failure status when the move failed.
#[must_use]
pub fn move_to_trash(os: Os, path: &Path, is_dir: bool) -> Launch {
    match os {
        Os::Windows => {
            let utf16: Vec<u8> = path.as_os_str().to_string_lossy().encode_utf16().flat_map(u16::to_le_bytes).collect();
            let call = if is_dir { "DeleteDirectory" } else { "DeleteFile" };
            let script = format!(
                "$ErrorActionPreference='Stop'; Add-Type -AssemblyName Microsoft.VisualBasic; \
                 $p=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{}')); \
                 [Microsoft.VisualBasic.FileIO.FileSystem]::{call}($p,'OnlyErrorDialogs','SendToRecycleBin')",
                base64(&utf16)
            );
            Launch {
                program: "powershell",
                args: ["-NoProfile", "-NonInteractive", "-Command", script.as_str()].map(OsString::from).to_vec(),
            }
        }
        Os::Linux => Launch { program: "gio", args: vec![OsString::from("trash"), path.as_os_str().to_owned()] },
        Os::MacOs => {
            let text = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!("tell application \"Finder\" to delete (POSIX file \"{text}\")");
            Launch { program: "osascript", args: vec![OsString::from("-e"), OsString::from(script)] }
        }
    }
}

/// Standard Base64 (with padding) of `bytes`.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// How to open `file` with the application the system associates with it
/// (Explorer on Windows hands it to the default app).
#[must_use]
pub fn open_with_default_app(os: Os, file: &Path) -> Launch {
    let program = match os {
        Os::Windows => "explorer",
        Os::MacOs => "open",
        Os::Linux => "xdg-open",
    };
    Launch { program, args: vec![file.as_os_str().to_owned()] }
}

/// Top-level folders of the Explorer: the user's Pictures folder first,
/// then the drives (Windows), the home folder and mount points (Linux),
/// or the home folder and volumes (macOS). Only folders for which `exists`
/// is true are listed.
#[must_use]
pub fn library_roots(os: Os, env: impl Fn(&str) -> Option<OsString>, exists: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let mut roots = Vec::new();
    match os {
        Os::Windows => {
            if let Some(home) = env("USERPROFILE").and_then(absolute) {
                roots.push(home.join("Pictures"));
            }
            roots.extend((b'A'..=b'Z').map(|d| PathBuf::from(format!("{}:\\", char::from(d)))));
        }
        Os::Linux => {
            if let Some(home) = env("HOME").and_then(absolute) {
                roots.push(home.join("Pictures"));
                roots.push(home);
            }
            roots.extend(["/media", "/mnt", "/"].into_iter().map(PathBuf::from));
        }
        Os::MacOs => {
            if let Some(home) = env("HOME").and_then(absolute) {
                roots.push(home.join("Pictures"));
                roots.push(home);
            }
            roots.push(PathBuf::from("/Volumes"));
        }
    }
    let mut seen = Vec::new();
    roots.retain(|p| {
        let keep = exists(p) && !seen.contains(p);
        if keep {
            seen.push(p.clone());
        }
        keep
    });
    roots
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

/// Folder for large data that can be downloaded again (AI models, caches),
/// following each system's convention: `%LOCALAPPDATA%\<app>` on Windows (not
/// roamed between machines), `$XDG_DATA_HOME/<app>` or `~/.local/share/<app>`
/// on Linux, `~/Library/Application Support/<app>` on macOS.
///
/// `env` looks up environment variables, as in [`config_dir`]. Returns `None`
/// when the variables a system needs are missing or relative.
#[must_use]
pub fn data_dir(os: Os, app: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let base = match os {
        Os::Windows => env("LOCALAPPDATA").and_then(absolute)?,
        Os::Linux => env("XDG_DATA_HOME")
            .and_then(absolute)
            .or_else(|| env("HOME").and_then(absolute).map(|h| h.join(".local").join("share")))?,
        Os::MacOs => env("HOME").and_then(absolute)?.join("Library").join("Application Support"),
    };
    Some(base.join(app))
}

/// The editor's data folder (catalog, keyword list, masks, models):
/// `$RNE_DATA_DIR`, else [`data_dir`] of [`APP_DIR`] on this system.
/// `None` only when the system says nothing about its folders.
#[must_use]
pub fn editor_data_dir() -> Option<PathBuf> {
    std::env::var_os("RNE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| data_dir(Os::current(), APP_DIR, |k| std::env::var_os(k)))
}

/// The folder of the downloaded AI models: `$RNE_MODELS_DIR`, else
/// `models` inside [`editor_data_dir`].
#[must_use]
pub fn editor_models_dir() -> Option<PathBuf> {
    std::env::var_os("RNE_MODELS_DIR").map(PathBuf::from).or_else(|| editor_data_dir().map(|d| d.join("models")))
}

/// Folder for caches that can be rebuilt (compiled GPU engines):
/// `%LOCALAPPDATA%\<app>` on Windows, `$XDG_CACHE_HOME/<app>` or
/// `~/.cache/<app>` on Linux, `~/Library/Caches/<app>` on macOS. `env` as in
/// [`config_dir`].
#[must_use]
pub fn cache_dir(os: Os, app: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let base = match os {
        Os::Windows => env("LOCALAPPDATA").and_then(absolute)?,
        Os::Linux => env("XDG_CACHE_HOME")
            .and_then(absolute)
            .or_else(|| env("HOME").and_then(absolute).map(|h| h.join(".cache")))?,
        Os::MacOs => env("HOME").and_then(absolute)?.join("Library").join("Caches"),
    };
    Some(base.join(app))
}

/// Like [`data_dir`], but in the roaming `%APPDATA%\<app>` on Windows (the
/// same folder elsewhere). Only for finding data that other programs, such as
/// an earlier denoiser, keep there.
#[must_use]
pub fn roaming_data_dir(os: Os, app: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    match os {
        Os::Windows => {
            let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
            Some(env("APPDATA").and_then(absolute)?.join(app))
        }
        Os::Linux | Os::MacOs => data_dir(os, app, env),
    }
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
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_trash_never_reads_the_path_as_a_command() {
        let odd = Path::new("C:\\Photos\\it's \"x\"; rm");
        let w = move_to_trash(Os::Windows, odd, true);
        assert_eq!(w.program, "powershell");
        let script = w.args.last().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
        assert!(script.contains("DeleteDirectory") && !script.contains("rm"), "{script}");
        let l = move_to_trash(Os::Linux, Path::new("/home/a b"), false);
        assert_eq!(l.args, [OsString::from("trash"), OsString::from("/home/a b")]);
        let m = move_to_trash(Os::MacOs, Path::new("/x/\"q\""), true);
        assert!(m.args[1].to_string_lossy().contains("\\\"q\\\""));
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

    #[test]
    #[cfg(unix)]
    fn data_and_cache_dirs_follow_xdg_then_home_on_linux_and_library_on_macos() {
        let vars = [("XDG_DATA_HOME", "/data"), ("XDG_CACHE_HOME", "/cache"), ("HOME", "/home/m")];
        assert_eq!(data_dir(Os::Linux, "rne", env(&vars)), Some(PathBuf::from("/data/rne")));
        assert_eq!(cache_dir(Os::Linux, "rne", env(&vars)), Some(PathBuf::from("/cache/rne")));
        let home = [("HOME", "/home/m")];
        assert_eq!(data_dir(Os::Linux, "rne", env(&home)), Some(PathBuf::from("/home/m/.local/share/rne")));
        assert_eq!(cache_dir(Os::Linux, "rne", env(&home)), Some(PathBuf::from("/home/m/.cache/rne")));
        let mac = [("HOME", "/Users/m")];
        assert_eq!(
            data_dir(Os::MacOs, "rne", env(&mac)),
            Some(PathBuf::from("/Users/m/Library/Application Support/rne"))
        );
        assert_eq!(cache_dir(Os::MacOs, "rne", env(&mac)), Some(PathBuf::from("/Users/m/Library/Caches/rne")));
    }

    #[test]
    #[cfg(windows)]
    fn data_and_cache_dirs_are_local_and_only_the_roaming_one_roams_on_windows() {
        let vars = [("APPDATA", r"C:\Users\m\AppData\Roaming"), ("LOCALAPPDATA", r"C:\Users\m\AppData\Local")];
        assert_eq!(data_dir(Os::Windows, "rne", env(&vars)), Some(PathBuf::from(r"C:\Users\m\AppData\Local\rne")));
        assert_eq!(cache_dir(Os::Windows, "rne", env(&vars)), Some(PathBuf::from(r"C:\Users\m\AppData\Local\rne")));
        assert_eq!(
            roaming_data_dir(Os::Windows, "rne", env(&vars)),
            Some(PathBuf::from(r"C:\Users\m\AppData\Roaming\rne"))
        );
    }

    #[test]
    fn data_and_cache_dirs_are_none_without_the_needed_variables() {
        for os in [Os::Windows, Os::Linux, Os::MacOs] {
            assert_eq!(data_dir(os, "rne", env(&[])), None);
            assert_eq!(cache_dir(os, "rne", env(&[])), None);
            assert_eq!(roaming_data_dir(os, "rne", env(&[])), None);
        }
    }

    #[test]
    fn library_roots_start_with_pictures_and_keep_only_existing_folders() {
        let windows = library_roots(Os::Windows, env(&[("USERPROFILE", r"C:\Users\m")]), |p| {
            [r"C:\Users\m\Pictures", r"C:\", r"D:\"].iter().any(|e| p == Path::new(e))
        });
        if cfg!(windows) {
            assert_eq!(
                windows,
                vec![PathBuf::from(r"C:\Users\m\Pictures"), PathBuf::from(r"C:\"), PathBuf::from(r"D:\")]
            );
        } else {
            // A Windows path is not absolute here, so only the drives remain.
            assert_eq!(windows, vec![PathBuf::from(r"C:\"), PathBuf::from(r"D:\")]);
        }
        let linux = library_roots(Os::Linux, env(&[("HOME", "/home/m")]), |p| p != Path::new("/mnt"));
        if cfg!(unix) {
            assert_eq!(
                linux,
                ["/home/m/Pictures", "/home/m", "/media", "/"].iter().map(PathBuf::from).collect::<Vec<_>>()
            );
        }
        assert!(library_roots(Os::MacOs, env(&[]), |_| true).contains(&PathBuf::from("/Volumes")));
    }

    #[test]
    fn files_open_with_the_systems_default_app() {
        let f = Path::new("a b/c.jpg");
        assert_eq!(open_with_default_app(Os::Windows, f).program, "explorer");
        assert_eq!(open_with_default_app(Os::Linux, f).args, vec![OsString::from("a b/c.jpg")]);
        assert_eq!(open_with_default_app(Os::MacOs, f).program, "open");
    }
}
