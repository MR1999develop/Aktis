//! Where the SCUNet model files are and what their names say.
//!
//! The models are named `scunet_color_real_<N>.onnx`,
//! with a fixed `1 × 3 × N × N` input (N = 448 is the normal one; 704, 960,
//! 1216 and 1600 are optional, faster on a GPU) and the half-precision
//! variants `scunet_color_real_<N>_fp16.onnx` for TensorRT for RTX. They are
//! generated from the original KAIR weights by `scripts/scunet` and are not
//! in the repository.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use platform::Os;

/// Environment variable naming a model folder (or one model file).
pub const MODELS_ENV: &str = "AKTIS_DENOISE_MODELS";
/// Side of the input tile of the normal model.
pub const DEFAULT_TILE_PX: usize = 448;
/// Pixels discarded along each edge of the network's output.
pub const TILE_MARGIN_PX: usize = 64;

const PREFIX: &str = "scunet_color_real_";
const FP16_SUFFIX: &str = "_fp16";

/// Side of the input tile of a model file, read from its name; 448 when the
/// name does not say (or says something that is not a multiple of 64).
#[must_use]
pub fn tile_px_for(path: &Path) -> usize {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix(PREFIX))
        .map(|s| s.strip_suffix(FP16_SUFFIX).unwrap_or(s))
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|&n| n.is_multiple_of(64) && n > 2 * TILE_MARGIN_PX)
        .unwrap_or(DEFAULT_TILE_PX)
}

/// `true` for a normal (FP32) model file `scunet_color_real_<px>.onnx`.
/// The FP16 variants are found from these with [`fp16_variant`].
#[must_use]
pub fn is_model_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "onnx")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix(PREFIX))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The half-precision file next to a model (`…_448.onnx` → `…_448_fp16.onnx`).
#[must_use]
pub fn fp16_variant(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}{FP16_SUFFIX}.onnx"))
}

/// The process environment, as [`search_dirs`] and `platform` want it.
pub(crate) fn process_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Folders searched for models, in order:
///
/// 1. [`MODELS_ENV`] (a folder, or a model file whose folder is used);
/// 2. a `models` folder next to the executable or up to three folders above
///    it (handy during development: `target/release/../../models`);
/// 3. the editor's data folder, `…/aktis/models` (shared with
///    the AI selection models).
#[must_use]
pub fn search_dirs(os: Os, exe: Option<&Path>, env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(v) = env(MODELS_ENV).map(PathBuf::from) {
        dirs.push(if v.extension().is_some_and(|e| e == "onnx") {
            v.parent().map(Path::to_path_buf).unwrap_or(v)
        } else {
            v
        });
    }
    if let Some(exe) = exe {
        dirs.extend(exe.ancestors().skip(1).take(4).map(|d| d.join("models")));
    }
    if let Some(d) = platform::data_dir(os, platform::APP_DIR, &env) {
        dirs.push(d.join("models"));
    }
    dirs
}

/// The model files of the first folder in `dirs` that has any, so models of
/// different origins are never mixed.
#[must_use]
pub fn find_models(dirs: &[PathBuf]) -> Vec<PathBuf> {
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut models: Vec<PathBuf> =
            entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| is_model_file(p) && p.is_file()).collect();
        if !models.is_empty() {
            models.sort_by_key(|p| tile_px_for(p));
            return models;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tile_size_is_read_from_the_file_name() {
        assert_eq!(tile_px_for(Path::new("models/scunet_color_real_448.onnx")), 448);
        assert_eq!(tile_px_for(Path::new("scunet_color_real_960.onnx")), 960);
        assert_eq!(tile_px_for(Path::new("scunet_color_real_1000.onnx")), 448, "not a multiple of 64");
        assert_eq!(tile_px_for(Path::new("other.onnx")), 448);
        assert_eq!(tile_px_for(Path::new("scunet_color_real_960_fp16.onnx")), 960);
    }

    #[test]
    fn fp16_files_are_variants_not_models() {
        assert!(is_model_file(Path::new("m/scunet_color_real_704.onnx")));
        assert!(!is_model_file(Path::new("m/scunet_color_real_704_fp16.onnx")));
        assert!(!is_model_file(Path::new("m/scunet_color_real_.onnx")));
        assert!(!is_model_file(Path::new("m/scunet_color_real_704.onnx.tmp")));
        assert_eq!(
            fp16_variant(Path::new("m/scunet_color_real_704.onnx")),
            Path::new("m/scunet_color_real_704_fp16.onnx")
        );
    }

    #[test]
    #[cfg(unix)]
    fn folders_are_searched_from_the_variable_to_the_data_folder() {
        let env = |k: &str| match k {
            MODELS_ENV => Some(OsString::from("/opt/m/scunet_color_real_448.onnx")),
            "HOME" => Some(OsString::from("/home/u")),
            _ => None,
        };
        let dirs = search_dirs(Os::Linux, Some(Path::new("/w/target/release/aktis-cli")), env);
        let expected: Vec<PathBuf> = [
            "/opt/m",
            "/w/target/release/models",
            "/w/target/models",
            "/w/models",
            "/models",
            "/home/u/.local/share/aktis/models",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(dirs, expected);
    }

    #[test]
    fn models_come_from_the_first_folder_that_has_any() {
        let root = std::env::temp_dir().join(format!("aktis-models-{}", std::process::id()));
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).expect("temp folder");
        }
        std::fs::write(b.join("notes.txt"), "x").expect("write");
        std::fs::write(b.join("scunet_color_real_960.onnx"), "x").expect("write");
        std::fs::write(b.join("scunet_color_real_448.onnx"), "x").expect("write");
        std::fs::write(b.join("scunet_color_real_448_fp16.onnx"), "x").expect("write");
        std::fs::write(c.join("scunet_color_real_704.onnx"), "x").expect("write");
        let found = find_models(&[root.join("missing"), a, b.clone(), c]);
        assert_eq!(found, vec![b.join("scunet_color_real_448.onnx"), b.join("scunet_color_real_960.onnx")]);
        let _ = std::fs::remove_dir_all(root);
    }
}
