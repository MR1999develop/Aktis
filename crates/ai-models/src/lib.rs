//! What the AI tools download the first time they are used.
//!
//! The program ships without models: they are large (tens of megabytes to a
//! gigabyte each) and carry their own licences. Each tool that needs some
//! names a [`Feature`]; [`needs`] lists the [`Pack`]s it requires and
//! [`missing`] which of them are not on disk yet, so the window (or the
//! command line) can offer to download them, with [`download`].
//!
//! Everything goes into one folder, [`models_dir`]: `$RNE_MODELS_DIR`, else
//! `models` in the editor's data folder (`%LOCALAPPDATA%\raw-node-editor`
//! on Windows).

mod download;

use std::path::{Path, PathBuf};

pub use download::{DownloadError, Progress, download};

/// Version of ONNX Runtime installed by the `onnx-runtime` pack.
pub const ORT_VERSION: &str = "1.24.4";

/// Where SigLIP 2 (tagging) is installed, inside the models folder.
pub const SIGLIP2_DIR: &str = "siglip2-base-patch16-224";

/// One file to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    /// Path of the installed file, relative to the models folder (`/` separated).
    pub path: String,
    /// Where to download it (or the archive that contains it).
    pub url: String,
    /// Size of what is downloaded, when known.
    pub bytes: Option<u64>,
    /// SHA-256 of what is downloaded (lowercase hex), when known.
    pub sha256: Option<String>,
    /// For an archive (`.zip`, `.nupkg`, `.tgz`): the file to take from it.
    pub extract: Option<String>,
}

/// A set of files that is installed together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    /// Stable identifier (`"onnx-runtime"`, `"scunet"`, `"sam2.1_large"` …).
    pub id: &'static str,
    /// Name shown to the user.
    pub name: &'static str,
    /// What it is for, one line.
    pub purpose: &'static str,
    /// Licence (SPDX identifier).
    pub license: &'static str,
    /// Not needed by any tool, only makes one faster.
    pub optional: bool,
    /// Its files.
    pub files: Vec<Download>,
}

impl Pack {
    /// Total download size of the files whose size is known, and whether every size is known.
    #[must_use]
    pub fn bytes(&self) -> (u64, bool) {
        let known = self.files.iter().filter_map(|f| f.bytes).sum();
        (known, self.files.iter().all(|f| f.bytes.is_some()))
    }

    /// "77 MB", or "at least 77 MB" when some sizes are unknown, or "size unknown".
    #[must_use]
    pub fn size_text(&self) -> String {
        let (bytes, all) = self.bytes();
        match (bytes, all) {
            (0, false) => "size unknown".to_owned(),
            (b, true) => megabytes(b),
            (b, false) => format!("at least {}", megabytes(b)),
        }
    }
}

/// "77 MB" or "1.2 GB".
#[must_use]
pub fn megabytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // sizes for display
    let mb = bytes as f64 / 1e6;
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1000.0) } else { format!("{mb:.0} MB") }
}

/// A use of AI that needs downloaded files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// The AI denoise node (SCUNet).
    Denoise,
    /// Select the main subject (BiRefNet).
    Subject,
    /// Select an object with clicks or a rectangle (Segment Anything).
    Objects,
    /// Select every person or every object of a kind (YOLOX + Segment Anything).
    People,
    /// Select hair, skin, clothes or facial features (MediaPipe on top of people).
    PersonParts,
    /// Suggest keywords (SigLIP 2).
    Tagging,
}

impl Feature {
    /// Every feature, in menu order.
    pub const ALL: [Self; 6] =
        [Self::Denoise, Self::Subject, Self::Objects, Self::People, Self::PersonParts, Self::Tagging];

    /// Name shown to the user.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Denoise => "AI denoise",
            Self::Subject => "Subject selection",
            Self::Objects => "Object selection",
            Self::People => "People and object detection",
            Self::PersonParts => "Person parts (hair, skin, clothes)",
            Self::Tagging => "AI keywords",
        }
    }

    /// Name used on the command line (`rne models --download denoise`).
    #[must_use]
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Denoise => "denoise",
            Self::Subject => "subject",
            Self::Objects => "objects",
            Self::People => "people",
            Self::PersonParts => "parts",
            Self::Tagging => "tagging",
        }
    }

    /// The feature named `name` on the command line.
    #[must_use]
    pub fn from_cli_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.cli_name().eq_ignore_ascii_case(name))
    }
}

/// The feature a node's tool needs, for the tools that need one.
#[must_use]
pub fn feature_for_tool(tool_id: &str) -> Option<Feature> {
    match tool_id {
        "denoise_ai" => Some(Feature::Denoise),
        _ => None,
    }
}

/// The packs `feature` needs, ONNX Runtime first.
#[must_use]
pub fn needs(feature: Feature) -> Vec<&'static str> {
    use ai_segment::catalog::{BIREFNET_GENERAL, MEDIAPIPE_PARTS, SAM21_LARGE, YOLOX_S};
    let mut ids = vec![RUNTIME_ID];
    match feature {
        Feature::Denoise => ids.push(SCUNET_ID),
        Feature::Subject => ids.push(BIREFNET_GENERAL.id),
        Feature::Objects => ids.push(SAM21_LARGE.id),
        Feature::People => ids.extend([YOLOX_S.id, SAM21_LARGE.id]),
        Feature::PersonParts => ids.extend([YOLOX_S.id, SAM21_LARGE.id, MEDIAPIPE_PARTS.id]),
        Feature::Tagging => ids.push(SIGLIP2_ID),
    }
    ids
}

const RUNTIME_ID: &str = "onnx-runtime";
const SCUNET_ID: &str = "scunet";
const SCUNET_FAST_ID: &str = "scunet-960";
const SIGLIP2_ID: &str = "siglip2";

/// Where this project publishes the files it generates (the SCUNet models
/// are converted from the original KAIR weights by `scripts/scunet`).
const PROJECT_RELEASE: &str = "https://github.com/MR1999develop/RawNodeEditor/releases/download/models-v1";

const SIGLIP2_REPO: &str = "https://huggingface.co/onnx-community/siglip2-base-patch16-224-ONNX/resolve/main";

/// Every pack this build knows, ONNX Runtime first.
#[must_use]
pub fn catalog() -> Vec<Pack> {
    let mut packs = vec![runtime_pack()];
    packs.push(Pack {
        id: SCUNET_ID,
        name: "SCUNet (AI denoise)",
        purpose: "Removes noise; needed by the AI denoise node",
        license: "Apache-2.0",
        optional: false,
        files: vec![file(
            "scunet_color_real_448.onnx",
            format!("{PROJECT_RELEASE}/scunet_color_real_448.onnx"),
            Some(76_725_198),
            Some("b39fe6660b7952ad078ddeab9eaa2f593d99b1aa4ea6c929ccd174f1099a9eae"),
        )],
    });
    packs.push(Pack {
        id: SCUNET_FAST_ID,
        name: "SCUNet, large tiles (faster AI denoise on a GPU)",
        purpose: "Optional: the GPU works on 960 px tiles instead of 448",
        license: "Apache-2.0",
        optional: true,
        files: vec![file(
            "scunet_color_real_960.onnx",
            format!("{PROJECT_RELEASE}/scunet_color_real_960.onnx"),
            Some(80_657_358),
            Some("9cf5d9ee24ebf1f466fd7d7893a0632529486485cc7622f07a4c648c90b366ae"),
        )],
    });
    for spec in ai_segment::catalog::CATALOG {
        packs.push(Pack {
            id: spec.id,
            name: spec.name,
            purpose: match spec.task {
                ai_segment::catalog::Task::Subject => "Finds the main subject",
                ai_segment::catalog::Task::Promptable => "Selects what is under clicks or inside a rectangle",
                ai_segment::catalog::Task::Detector => "Finds people and 79 other kinds of object",
                ai_segment::catalog::Task::PersonParts => "Splits a person into hair, skin, clothes and features",
            },
            license: spec.license,
            // The small Segment Anything is an alternative no tool asks for by default.
            optional: spec.id == ai_segment::catalog::SAM21_SMALL.id,
            files: spec.files.iter().map(|f| file(f.name, f.url.to_owned(), Some(f.bytes), Some(f.sha256))).collect(),
        });
    }
    packs.push(Pack {
        id: SIGLIP2_ID,
        name: "SigLIP 2 (AI keywords)",
        purpose: "Recognises what a photo shows, to suggest keywords",
        license: "Apache-2.0",
        optional: false,
        files: ["onnx/vision_model.onnx", "onnx/text_model.onnx", "tokenizer.json"]
            .into_iter()
            .map(|f| {
                let name = f.rsplit('/').next().unwrap_or(f);
                file(&format!("{SIGLIP2_DIR}/{name}"), format!("{SIGLIP2_REPO}/{f}"), None, None)
            })
            .collect(),
    });
    packs
}

/// The pack with this id.
#[must_use]
pub fn pack(id: &str) -> Option<Pack> {
    catalog().into_iter().find(|p| p.id == id)
}

fn file(path: &str, url: String, bytes: Option<u64>, sha256: Option<&str>) -> Download {
    Download { path: path.to_owned(), url, bytes, sha256: sha256.map(str::to_owned), extract: None }
}

/// ONNX Runtime for this system: the DirectML build on Windows (any GPU
/// vendor) with a recent `DirectML.dll`, the CPU builds elsewhere.
fn runtime_pack() -> Pack {
    let files = match platform::Os::current() {
        platform::Os::Windows => vec![
            Download {
                path: "onnxruntime.dll".to_owned(),
                url: format!("https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime.DirectML/{ORT_VERSION}"),
                bytes: None,
                sha256: None,
                extract: Some("runtimes/win-x64/native/onnxruntime.dll".to_owned()),
            },
            Download {
                path: "DirectML.dll".to_owned(),
                url: "https://www.nuget.org/api/v2/package/Microsoft.AI.DirectML/1.15.4".to_owned(),
                bytes: None,
                sha256: None,
                extract: Some("bin/x64-win/DirectML.dll".to_owned()),
            },
        ],
        platform::Os::Linux => vec![Download {
            path: "libonnxruntime.so".to_owned(),
            url: format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-linux-x64-{ORT_VERSION}.tgz"
            ),
            bytes: Some(8_155_822),
            sha256: Some("3a211fbea252c1e66290658f1b735b772056149f28321e71c308942cdb54b747".to_owned()),
            extract: Some(format!("onnxruntime-linux-x64-{ORT_VERSION}/lib/libonnxruntime.so.{ORT_VERSION}")),
        }],
        platform::Os::MacOs => vec![Download {
            path: "libonnxruntime.dylib".to_owned(),
            url: format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-osx-arm64-{ORT_VERSION}.tgz"
            ),
            bytes: Some(30_937_282),
            sha256: Some("93787795f47e1eee369182e43ed51b9e5da0878ab0346aecf4258979b8bba989".to_owned()),
            extract: Some(format!("onnxruntime-osx-arm64-{ORT_VERSION}/lib/libonnxruntime.{ORT_VERSION}.dylib")),
        }],
    };
    Pack {
        id: RUNTIME_ID,
        name: "ONNX Runtime",
        purpose: "Runs the AI models, on the GPU when it can; needed by every AI tool",
        license: "MIT",
        optional: false,
        files,
    }
}

/// The editor's data folder: `$RNE_DATA_DIR`, else [`platform::data_dir`].
#[must_use]
pub fn data_dir() -> PathBuf {
    std::env::var_os("RNE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| platform::data_dir(platform::Os::current(), platform::APP_DIR, |k| std::env::var_os(k)))
        .unwrap_or_else(|| PathBuf::from(".rne"))
}

/// The models folder: `$RNE_MODELS_DIR`, else `models` in [`data_dir`].
#[must_use]
pub fn models_dir() -> PathBuf {
    std::env::var_os("RNE_MODELS_DIR").map(PathBuf::from).unwrap_or_else(|| data_dir().join("models"))
}

/// Where `file` is installed in `dir`.
#[must_use]
pub fn installed_path(dir: &Path, file: &Download) -> PathBuf {
    file.path.split('/').fold(dir.to_path_buf(), |p, part| p.join(part))
}

/// Whether `file` is installed in `dir`: present, and of the right size when
/// that is known (an interrupted download is not).
#[must_use]
pub fn is_installed(dir: &Path, file: &Download) -> bool {
    let Ok(meta) = std::fs::metadata(installed_path(dir, file)) else { return false };
    let size_ok = match (file.extract.is_none(), file.bytes) {
        (true, Some(b)) => meta.len() == b,
        _ => meta.len() > 0,
    };
    meta.is_file() && size_ok
}

/// The files of `pack` not installed in `dir`.
#[must_use]
pub fn missing_files<'a>(pack: &'a Pack, dir: &Path) -> Vec<&'a Download> {
    pack.files.iter().filter(|f| !is_installed(dir, f)).collect()
}

/// The installed files of `pack` whose SHA-256, when the catalog knows it,
/// is not the expected one (reads every byte: seconds for a large model).
#[must_use]
pub fn damaged_files<'a>(pack: &'a Pack, dir: &Path) -> Vec<&'a Download> {
    pack.files
        .iter()
        .filter(|f| f.extract.is_none() && is_installed(dir, f))
        .filter(|f| {
            f.sha256.as_ref().is_some_and(|want| {
                let got = std::fs::File::open(installed_path(dir, f)).and_then(ai_segment::sha256_hex);
                !got.is_ok_and(|h| &h == want)
            })
        })
        .collect()
}

/// The packs among `ids` with files missing in `dir`.
#[must_use]
pub fn missing(ids: &[&str], dir: &Path) -> Vec<Pack> {
    ids.iter().filter_map(|id| pack(id)).filter(|p| !missing_files(p, dir).is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rne-ai-models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn the_catalog_is_consistent() {
        let packs = catalog();
        let mut ids = std::collections::BTreeSet::new();
        for p in &packs {
            assert!(ids.insert(p.id), "duplicate pack {}", p.id);
            assert!(!p.files.is_empty(), "{}", p.id);
            for f in &p.files {
                assert!(f.url.starts_with("https://"), "{}", f.url);
                assert!(!f.path.is_empty() && !f.path.starts_with('/') && !f.path.contains('\\'), "{}", f.path);
                if let Some(h) = &f.sha256 {
                    assert_eq!(h.len(), 64, "{}", f.path);
                    assert!(h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{}", f.path);
                }
            }
        }
        assert_eq!(packs[0].id, RUNTIME_ID);
        for feature in Feature::ALL {
            let needed = needs(feature);
            assert_eq!(needed[0], RUNTIME_ID);
            for id in needed {
                assert!(pack(id).is_some_and(|p| !p.optional), "{feature:?} needs unknown or optional pack {id}");
            }
            assert_eq!(Feature::from_cli_name(feature.cli_name()), Some(feature));
        }
        assert_eq!(feature_for_tool("denoise_ai"), Some(Feature::Denoise));
        assert_eq!(feature_for_tool("exposure"), None);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(megabytes(76_725_198), "77 MB");
        assert_eq!(megabytes(1_200_000_000), "1.2 GB");
        let scunet = pack(SCUNET_ID).expect("scunet");
        assert_eq!(scunet.size_text(), "77 MB");
        assert_eq!(pack(SIGLIP2_ID).expect("siglip2").size_text(), "size unknown");
    }

    #[test]
    fn a_partial_file_is_not_installed_and_the_right_size_is() {
        let dir = temp_dir("status");
        let scunet = pack(SCUNET_ID).expect("scunet");
        assert_eq!(missing(&[SCUNET_ID], &dir).len(), 1);
        let path = installed_path(&dir, &scunet.files[0]);
        std::fs::write(&path, b"half a download").expect("write");
        assert_eq!(missing_files(&scunet, &dir).len(), 1);
        let siglip = pack(SIGLIP2_ID).expect("siglip2");
        let tokenizer = siglip.files.iter().find(|f| f.path.ends_with("tokenizer.json")).expect("tokenizer");
        let tok_path = installed_path(&dir, tokenizer);
        std::fs::create_dir_all(tok_path.parent().expect("parent")).expect("dir");
        std::fs::write(&tok_path, b"{}").expect("write");
        assert!(is_installed(&dir, tokenizer), "unknown size: any non-empty file counts");
        assert_eq!(missing_files(&siglip, &dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
