//! The one ONNX Runtime of the process.
//!
//! `ort` allows a single environment per process, and the runtime library
//! (`onnxruntime.dll`, `libonnxruntime.so`) is loaded at run time, not linked:
//! the editor starts without it, and the library can be swapped for a build
//! with the GPU support of the machine. Every AI module (denoise, selection,
//! tagging) takes its environment from here, so they all run on the same
//! library.
//!
//! The library is looked for in order: `ORT_DYLIB_PATH`, next to the
//! program, the folders a caller passes, then the editor's models folder
//! (`$RNE_MODELS_DIR`, else `models` in [`platform::data_dir`]) and the data
//! folder itself.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::environment::Environment;

/// The loaded runtime.
pub struct Runtime {
    /// The process's ONNX Runtime environment; every session is built on it.
    pub env: Environment,
    /// The library in use.
    pub library: PathBuf,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").field("library", &self.library).finish_non_exhaustive()
    }
}

/// The runtime library's file name on this system.
#[must_use]
pub fn library_name() -> String {
    format!("{}onnxruntime{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX)
}

/// The editor's models folder: `$RNE_MODELS_DIR`, else `models` in the data folder.
#[must_use]
pub fn models_dir() -> Option<PathBuf> {
    std::env::var_os("RNE_MODELS_DIR").map(PathBuf::from).or_else(|| data_dir().map(|d| d.join("models")))
}

/// The editor's data folder: `$RNE_DATA_DIR`, else [`platform::data_dir`].
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("RNE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| platform::data_dir(platform::Os::current(), platform::APP_DIR, |k| std::env::var_os(k)))
}

/// Where the runtime library is: `ORT_DYLIB_PATH`, next to the program,
/// each of `extra_dirs`, then the models and data folders.
#[must_use]
pub fn find_library(extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let name = library_name();
    let beside_exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(&name)));
    beside_exe
        .into_iter()
        .chain(extra_dirs.iter().map(|d| d.join(&name)))
        .chain(models_dir().into_iter().chain(data_dir()).map(|d| d.join(&name)))
        .find(|p| p.is_file())
}

static RUNTIME: OnceLock<Runtime> = OnceLock::new();
/// Held while a library is being loaded, so two threads do not load two.
static LOADING: Mutex<()> = Mutex::new(());

/// The runtime, loaded on first use from [`find_library`] with no extra folders.
/// A failure is not remembered: once the library has been installed (for
/// example downloaded by the editor), the next call loads it.
///
/// # Errors
/// Why the library could not be found or loaded (text for the user).
pub fn runtime() -> Result<&'static Runtime, String> {
    if let Some(r) = RUNTIME.get() {
        return Ok(r);
    }
    let library = find_library(&[]).ok_or_else(not_found)?;
    load(&library)
}

/// Loads the runtime from `library`. Only one library can be loaded per
/// process: once one is loaded, later calls return it, whatever they ask for.
///
/// # Errors
/// Why the library could not be loaded or is too old (text for the user).
pub fn load(library: &Path) -> Result<&'static Runtime, String> {
    let _guard = LOADING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(r) = RUNTIME.get() {
        return Ok(r);
    }
    let rt = init(library)?;
    Ok(RUNTIME.get_or_init(|| rt))
}

fn init(library: &Path) -> Result<Runtime, String> {
    // The DirectML build needs a recent DirectML.dll; Windows' own copy in
    // System32 is often too old. One next to the runtime is loaded first.
    if let Some(dml) = library.parent().map(|d| d.join("DirectML.dll")).filter(|p| p.is_file()) {
        ort::util::preload_dylib(dml.as_path()).map_err(|e| format!("cannot load {}: {e}", dml.display()))?;
    }
    let builder = ort::init_from(library).map_err(|e| format!("cannot load {}: {e}", library.display()))?;
    let env = builder.with_name(platform::APP_DIR).build().map_err(|e| e.to_string())?;
    Ok(Runtime { env, library: library.to_path_buf() })
}

fn not_found() -> String {
    let place = models_dir().map_or_else(|| "the models folder".to_owned(), |d| d.display().to_string());
    format!(
        "ONNX Runtime ({}) not found: set ORT_DYLIB_PATH, or put it next to the program or in {place}",
        library_name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_name_follows_the_system() {
        let name = library_name();
        assert!(name.contains("onnxruntime"));
        #[cfg(windows)]
        assert_eq!(name, "onnxruntime.dll");
        #[cfg(target_os = "linux")]
        assert_eq!(name, "libonnxruntime.so");
    }

    #[test]
    fn a_library_in_an_extra_folder_is_found() {
        let dir = std::env::temp_dir().join(format!("onnx-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(library_name()), b"not really a library").expect("write");
        if std::env::var_os("ORT_DYLIB_PATH").is_none() {
            assert_eq!(find_library(std::slice::from_ref(&dir)), Some(dir.join(library_name())));
        }
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
