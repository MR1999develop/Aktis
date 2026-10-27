//! Downloading packs: `curl` for the transfer (part of Windows 10 and later,
//! macOS and most Linux systems, so no TLS stack is built into the editor),
//! `tar` to take a file out of an archive, SHA-256 when the catalog knows it.
//!
//! Files arrive as `<name>.part` and are renamed only when complete and
//! checked, so an interrupted download never looks installed.

use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{Download, Pack, installed_path, missing_files};

/// How a download is going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// Name of the pack being installed.
    pub pack: &'static str,
    /// The file being downloaded (path in the models folder).
    pub file: String,
    /// Its number among the files still to download, from 1.
    pub index: usize,
    /// How many files are still to download in all.
    pub count: usize,
    /// Bytes received for this file.
    pub done: u64,
    /// Its size, when known.
    pub total: Option<u64>,
}

impl Progress {
    /// Fraction of this file received, when its size is known.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // a fraction for a progress bar
        self.total.filter(|t| *t > 0).map(|t| (self.done as f64 / t as f64).clamp(0.0, 1.0) as f32)
    }
}

/// Why a download did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// `curl` or `tar` could not be started.
    Tool(String),
    /// The server or the network failed.
    Network {
        /// What was asked for.
        url: String,
        /// What curl said.
        message: String,
    },
    /// The file arrived but is not the expected one.
    Checksum {
        /// The file.
        file: String,
    },
    /// The archive did not contain the expected file.
    Archive {
        /// The file looked for.
        file: String,
        /// What tar said.
        message: String,
    },
    /// The user stopped it.
    Cancelled,
    /// Writing to the models folder failed.
    Io(String),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tool(m) => write!(f, "cannot start the download: {m}"),
            Self::Network { url, message } => write!(f, "download of {url} failed: {message}"),
            Self::Checksum { file } => write!(f, "{file} arrived damaged (wrong checksum); try again"),
            Self::Archive { file, message } => write!(f, "{file} could not be taken from its archive: {message}"),
            Self::Cancelled => f.write_str("download cancelled"),
            Self::Io(m) => write!(f, "cannot write to the models folder: {m}"),
        }
    }
}

impl std::error::Error for DownloadError {}

fn io(e: impl fmt::Display) -> DownloadError {
    DownloadError::Io(e.to_string())
}

/// Downloads the missing files of `packs` into `dir`, telling `on_progress`
/// several times a second. Stops early when `cancel` becomes true.
///
/// # Errors
/// [`DownloadError`]; what was completed before the error stays installed.
pub fn download(
    packs: &[Pack],
    dir: &Path,
    on_progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let todo: Vec<(&Pack, &Download)> =
        packs.iter().flat_map(|p| missing_files(p, dir).into_iter().map(move |f| (p, f))).collect();
    let count = todo.len();
    for (i, (pack, file)) in todo.into_iter().enumerate() {
        let mut progress =
            Progress { pack: pack.name, file: file.path.clone(), index: i + 1, count, done: 0, total: file.bytes };
        on_progress(&progress);
        install(
            file,
            dir,
            &mut |done| {
                progress.done = done;
                on_progress(&progress);
            },
            cancel,
        )?;
    }
    Ok(())
}

/// Downloads one file, checks it and puts it in place.
fn install(
    file: &Download,
    dir: &Path,
    on_bytes: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let target = installed_path(dir, file);
    let folder = target.parent().map_or_else(|| dir.to_path_buf(), Path::to_path_buf);
    fs::create_dir_all(&folder).map_err(io)?;
    let part = with_suffix(&target, ".part");
    fetch(&file.url, &part, on_bytes, cancel)?;

    let checked = check(file, &part);
    if let Err(e) = checked {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    // A damaged earlier copy is replaced (Windows cannot rename over a file).
    let _ = fs::remove_file(&target);
    match &file.extract {
        None => fs::rename(&part, &target).map_err(io),
        Some(inner) => {
            let result = extract(&part, inner, &target, &file.path);
            let _ = fs::remove_file(&part);
            result
        }
    }
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

/// Size and checksum of a downloaded file, when the catalog knows them.
fn check(file: &Download, part: &Path) -> Result<(), DownloadError> {
    if let Some(bytes) = file.bytes {
        let len = fs::metadata(part).map_err(io)?.len();
        if len != bytes {
            return Err(DownloadError::Checksum { file: file.path.clone() });
        }
    }
    if let Some(expected) = &file.sha256 {
        let got = fs::File::open(part).and_then(ai_segment::sha256_hex).map_err(io)?;
        if &got != expected {
            return Err(DownloadError::Checksum { file: file.path.clone() });
        }
    }
    Ok(())
}

/// A system tool: the one in `System32` on Windows (curl and tar ship with
/// Windows; a `tar` from another package may not read zip files).
fn system_tool(name: &str) -> Command {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let path = PathBuf::from(root).join("System32").join(format!("{name}.exe"));
            if path.is_file() {
                return quiet(Command::new(path));
            }
        }
    }
    quiet(Command::new(name))
}

/// No console window flashing up when the editor (a window program) runs a tool.
fn quiet(mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// Downloads `url` to `out` with curl, reporting the bytes received.
pub(crate) fn fetch(
    url: &str,
    out: &Path,
    on_bytes: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let _ = fs::remove_file(out);
    let mut cmd = system_tool("curl");
    cmd.args(["--location", "--fail", "--silent", "--show-error", "--retry", "3", "--output"])
        .arg(out)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| DownloadError::Tool(format!("curl: {e}")))?;
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(out);
            return Err(DownloadError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(DownloadError::Tool(format!("curl: {e}"))),
        }
        on_bytes(fs::metadata(out).map_or(0, |m| m.len()));
        std::thread::sleep(Duration::from_millis(200));
    };
    on_bytes(fs::metadata(out).map_or(0, |m| m.len()));
    if status.success() {
        return Ok(());
    }
    let message = stderr_text(&mut child);
    let _ = fs::remove_file(out);
    Err(DownloadError::Network {
        url: url.to_owned(),
        message: if message.is_empty() { format!("curl ended with {status}") } else { message },
    })
}

fn stderr_text(child: &mut Child) -> String {
    let mut text = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut text);
    }
    text.trim().to_owned()
}

/// Takes `inner` out of `archive` (zip, nupkg or tgz) and puts it at `target`.
fn extract(archive: &Path, inner: &str, target: &Path, name: &str) -> Result<(), DownloadError> {
    let scratch = with_suffix(target, ".unpack");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(io)?;
    let output = system_tool("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(&scratch)
        .arg(inner)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| DownloadError::Tool(format!("tar: {e}")))?;
    let found = inner.split('/').fold(scratch.clone(), |p, part| p.join(part));
    let result = if output.status.success() && found.is_file() {
        fs::rename(&found, target).or_else(|_| fs::copy(&found, target).map(|_| ())).map_err(io)
    } else {
        Err(DownloadError::Archive {
            file: name.to_owned(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    };
    let _ = fs::remove_dir_all(&scratch);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_url(path: &Path) -> String {
        let p = path.display().to_string().replace('\\', "/");
        if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") }
    }

    #[test]
    fn a_file_is_fetched_checked_and_installed_and_a_bad_checksum_is_refused() {
        let dir = std::env::temp_dir().join(format!("rne-ai-dl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        let source = dir.join("source.bin");
        fs::write(&source, b"abc").expect("write");
        let models = dir.join("models");
        let good = Download {
            path: "sub/model.onnx".to_owned(),
            url: file_url(&source),
            bytes: Some(3),
            // SHA-256 of "abc" (FIPS 180-2 test vector).
            sha256: Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned()),
            extract: None,
        };
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let curl_works = system_tool("curl").arg("--version").stdout(Stdio::null()).status().is_ok_and(|s| s.success());
        if !curl_works {
            return; // no curl on this machine: nothing to test
        }
        install(&good, &models, &mut |b| seen.push(b), &cancel).expect("installed");
        assert_eq!(fs::read(installed_path(&models, &good)).expect("read"), b"abc");
        assert_eq!(seen.last(), Some(&3));
        assert!(!with_suffix(&installed_path(&models, &good), ".part").exists());

        let bad = Download { path: "bad.onnx".to_owned(), sha256: Some("0".repeat(64)), ..good.clone() };
        let err = install(&bad, &models, &mut |_| {}, &cancel).expect_err("checksum");
        assert_eq!(err, DownloadError::Checksum { file: "bad.onnx".to_owned() });
        assert!(!installed_path(&models, &bad).exists());

        let missing = Download { path: "none.onnx".to_owned(), url: file_url(&dir.join("absent")), ..good };
        assert!(matches!(install(&missing, &models, &mut |_| {}, &cancel), Err(DownloadError::Network { .. })));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_is_a_fraction_only_when_the_size_is_known() {
        let mut p = Progress { pack: "x", file: "f".to_owned(), index: 1, count: 1, done: 50, total: Some(200) };
        assert_eq!(p.fraction(), Some(0.25));
        p.total = None;
        assert_eq!(p.fraction(), None);
    }

    #[test]
    fn errors_say_what_to_do() {
        assert!(DownloadError::Checksum { file: "a.onnx".to_owned() }.to_string().contains("try again"));
        assert_eq!(DownloadError::Cancelled.to_string(), "download cancelled");
    }
}
