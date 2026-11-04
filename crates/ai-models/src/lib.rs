//! What the AI tools download the first time they are used.
//!
//! The program ships without models: they are large (tens of megabytes to a
//! gigabyte each) and carry their own licences. Each tool that needs some
//! names a [`Feature`]; [`needs`] lists the [`Pack`]s it requires and
//! [`missing`] which of them are not on disk yet, so the window (or the
//! command line) can offer to download them, with [`download`].
//!
//! Everything goes into one folder, [`models_dir`]: `$AKTIS_MODELS_DIR`, else
//! `models` in the editor's data folder (`%LOCALAPPDATA%\aktis`
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
    pub extract: Option<Extract>,
    /// For a file made on this computer from what is downloaded (the
    /// original weights): how to make it.
    pub convert: Option<Convert>,
}

/// The file taken out of a downloaded archive.
///
/// A NuGet package can be signed again by nuget.org, which changes its
/// bytes but not the files inside: for those the catalog pins the file
/// that is installed (and loaded), not the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extract {
    /// Its path inside the archive (`/` separated).
    pub inner: String,
    /// Its size, when known.
    pub bytes: Option<u64>,
    /// Its SHA-256 (lowercase hex), when known.
    pub sha256: Option<String>,
}

impl Extract {
    fn new(inner: impl Into<String>) -> Self {
        Self { inner: inner.into(), bytes: None, sha256: None }
    }

    fn pinned(inner: &str, bytes: u64, sha256: &str) -> Self {
        Self { inner: inner.to_owned(), bytes: Some(bytes), sha256: Some(sha256.to_owned()) }
    }
}

/// How an installed file is made from the downloaded original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Convert {
    /// The KAIR SCUNet checkpoint (`.pth`) becomes an ONNX model for tiles of
    /// `tile` pixels, with [`scunet_onnx::convert`].
    ScunetOnnx {
        /// Tile side in pixels.
        tile: u32,
        /// SHA-256 of the ONNX file the conversion makes (it is deterministic).
        sha256: &'static str,
    },
}

impl Download {
    /// SHA-256 of the installed file, when the catalog knows it: the
    /// download's for a plain file, the conversion's for a converted one,
    /// the inner file's for one taken from an archive.
    #[must_use]
    pub fn installed_sha256(&self) -> Option<&str> {
        match (&self.convert, &self.extract) {
            (Some(Convert::ScunetOnnx { sha256, .. }), _) => Some(sha256),
            (None, None) => self.sha256.as_deref(),
            (None, Some(inner)) => inner.sha256.as_deref(),
        }
    }

    /// Size of the installed file, when the catalog knows it (none for a
    /// converted file).
    #[must_use]
    pub fn installed_bytes(&self) -> Option<u64> {
        match (&self.convert, &self.extract) {
            (Some(_), _) => None,
            (None, None) => self.bytes,
            (None, Some(inner)) => inner.bytes,
        }
    }
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
    /// Home of the model or program: its authors' page, where every file
    /// is downloaded from (or, for the few models the authors publish only
    /// in another format, where the converted file is listed).
    pub source: &'static str,
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
    /// The AI denoise tool (SCUNet).
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

    /// Name used on the command line (`aktis-cli models --download denoise`).
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

/// The feature a tool needs, for the tools that need one.
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

/// The SCUNet weights as their authors publish them (KAIR, release v1.0).
/// The ONNX models are made from this file on the user's computer.
const SCUNET_WEIGHTS: &str = "https://github.com/cszn/KAIR/releases/download/v1.0/scunet_color_real_psnr.pth";
const SCUNET_WEIGHTS_BYTES: u64 = 71_982_841;
const SCUNET_WEIGHTS_SHA256: &str = "fa78899ba2caec9d235a900e91d96c689da71c42029230c2028b00f09f809c2e";
const SCUNET_HOME: &str = "https://github.com/cszn/SCUNet";

/// SigLIP 2 at a fixed revision of its repository, so the checksums below
/// keep matching when the repository changes.
const SIGLIP2_REPO: &str = "https://huggingface.co/onnx-community/siglip2-base-patch16-224-ONNX/resolve/ba1f3b0843f24bc5417d38e19c37b287d719b2f4";

/// The SigLIP 2 files: path in the repository, size, SHA-256.
const SIGLIP2_FILES: [(&str, u64, &str); 3] = [
    ("onnx/vision_model.onnx", 371_807_752, "c0573e3f4140c3a7c4e9cc5912bd6b26a033b46a6a8e8af26cbea262b163bcad"),
    ("onnx/text_model.onnx", 1_129_469_657, "baf12d941beabafafb14f7b4adb38dc15be18681b964a84410ec53d9d65e6293"),
    ("tokenizer.json", 34_363_039, "cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322"),
];

/// Every pack this build knows, ONNX Runtime first.
#[must_use]
pub fn catalog() -> Vec<Pack> {
    let mut packs = vec![runtime_pack()];
    packs.push(Pack {
        id: SCUNET_ID,
        name: "SCUNet (AI denoise)",
        purpose: "Removes noise; needed by the AI denoise tool",
        license: "Apache-2.0",
        source: SCUNET_HOME,
        optional: false,
        files: vec![scunet_file(448, "b39fe6660b7952ad078ddeab9eaa2f593d99b1aa4ea6c929ccd174f1099a9eae")],
    });
    packs.push(Pack {
        id: SCUNET_FAST_ID,
        name: "SCUNet, large tiles (faster AI denoise on a GPU)",
        purpose: "Optional: the GPU works on 960 px tiles instead of 448",
        license: "Apache-2.0",
        source: SCUNET_HOME,
        optional: true,
        files: vec![scunet_file(960, "9cf5d9ee24ebf1f466fd7d7893a0632529486485cc7622f07a4c648c90b366ae")],
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
            source: spec.homepage,
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
        source: "https://huggingface.co/google/siglip2-base-patch16-224",
        optional: false,
        files: SIGLIP2_FILES
            .into_iter()
            .map(|(f, bytes, sha256)| {
                let name = f.rsplit('/').next().unwrap_or(f);
                file(&format!("{SIGLIP2_DIR}/{name}"), format!("{SIGLIP2_REPO}/{f}"), Some(bytes), Some(sha256))
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
    Download { path: path.to_owned(), url, bytes, sha256: sha256.map(str::to_owned), extract: None, convert: None }
}

/// The SCUNet model for `tile` px tiles, made from the KAIR weights.
fn scunet_file(tile: u32, sha256: &'static str) -> Download {
    Download {
        convert: Some(Convert::ScunetOnnx { tile, sha256 }),
        ..file(
            &format!("scunet_color_real_{tile}.onnx"),
            SCUNET_WEIGHTS.to_owned(),
            Some(SCUNET_WEIGHTS_BYTES),
            Some(SCUNET_WEIGHTS_SHA256),
        )
    }
}

/// ONNX Runtime for this system: the DirectML build on Windows (any GPU
/// vendor) with a recent `DirectML.dll`, the CPU builds elsewhere.
fn runtime_pack() -> Pack {
    let files = match platform::Os::current() {
        platform::Os::Windows => vec![
            // Both DLLs carry Microsoft's Authenticode signature; the checksums
            // pin those signed files.
            Download {
                path: "onnxruntime.dll".to_owned(),
                url: format!("https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime.DirectML/{ORT_VERSION}"),
                bytes: None,
                sha256: None,
                extract: Some(Extract::pinned(
                    "runtimes/win-x64/native/onnxruntime.dll",
                    17_328_152,
                    "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
                )),
                convert: None,
            },
            Download {
                path: "DirectML.dll".to_owned(),
                url: "https://www.nuget.org/api/v2/package/Microsoft.AI.DirectML/1.15.4".to_owned(),
                bytes: None,
                sha256: None,
                extract: Some(Extract::pinned(
                    "bin/x64-win/DirectML.dll",
                    18_527_776,
                    "9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1",
                )),
                convert: None,
            },
        ],
        platform::Os::Linux => vec![Download {
            path: "libonnxruntime.so".to_owned(),
            url: format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-linux-x64-{ORT_VERSION}.tgz"
            ),
            bytes: Some(8_155_822),
            sha256: Some("3a211fbea252c1e66290658f1b735b772056149f28321e71c308942cdb54b747".to_owned()),
            extract: Some(Extract::new(format!(
                "onnxruntime-linux-x64-{ORT_VERSION}/lib/libonnxruntime.so.{ORT_VERSION}"
            ))),
            convert: None,
        }],
        platform::Os::MacOs => vec![Download {
            path: "libonnxruntime.dylib".to_owned(),
            url: format!(
                "https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/onnxruntime-osx-arm64-{ORT_VERSION}.tgz"
            ),
            bytes: Some(30_937_282),
            sha256: Some("93787795f47e1eee369182e43ed51b9e5da0878ab0346aecf4258979b8bba989".to_owned()),
            extract: Some(Extract::new(format!(
                "onnxruntime-osx-arm64-{ORT_VERSION}/lib/libonnxruntime.{ORT_VERSION}.dylib"
            ))),
            convert: None,
        }],
    };
    Pack {
        id: RUNTIME_ID,
        name: "ONNX Runtime",
        purpose: "Runs the AI models, on the GPU when it can; needed by every AI tool",
        license: "MIT",
        source: "https://github.com/microsoft/onnxruntime",
        optional: false,
        files,
    }
}

/// The editor's data folder ([`platform::editor_data_dir`], `.aktis` if the system has none).
#[must_use]
pub fn data_dir() -> PathBuf {
    platform::editor_data_dir().unwrap_or_else(|| PathBuf::from(".aktis"))
}

/// The models folder ([`platform::editor_models_dir`]).
#[must_use]
pub fn models_dir() -> PathBuf {
    platform::editor_models_dir().unwrap_or_else(|| data_dir().join("models"))
}

/// Where `file` is installed in `dir`.
#[must_use]
pub fn installed_path(dir: &Path, file: &Download) -> PathBuf {
    file.path.split('/').fold(dir.to_path_buf(), |p, part| p.join(part))
}

/// Whether `file` is installed in `dir`: present, and of the right size when
/// that is known (an interrupted download is not; a converted file is
/// written only once complete).
#[must_use]
pub fn is_installed(dir: &Path, file: &Download) -> bool {
    let Ok(meta) = std::fs::metadata(installed_path(dir, file)) else { return false };
    let size_ok = match file.installed_bytes() {
        Some(b) => meta.len() == b,
        None => meta.len() > 0,
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
        .filter(|f| is_installed(dir, f))
        .filter(|f| {
            f.installed_sha256().is_some_and(|want| {
                let got = std::fs::File::open(installed_path(dir, f)).and_then(ai_segment::sha256_hex);
                !got.is_ok_and(|h| h == want)
            })
        })
        .collect()
}

/// Where the original downloaded for `file` is kept while (and after) it is
/// converted, so a second model made from it does not download it again.
#[must_use]
pub fn source_path(dir: &Path, file: &Download) -> PathBuf {
    let name = file.url.rsplit('/').next().unwrap_or("source");
    dir.join("sources").join(name)
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
        let dir = std::env::temp_dir().join(format!("aktis-ai-models-{name}-{}", std::process::id()));
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
            assert!(p.source.starts_with("https://"), "{}", p.id);
            for f in &p.files {
                assert!(f.url.starts_with("https://"), "{}", f.url);
                assert!(!f.url.contains("MR1999develop"), "{} is not downloaded from its authors", f.path);
                assert!(!f.path.is_empty() && !f.path.starts_with('/') && !f.path.contains('\\'), "{}", f.path);
                for h in [f.sha256.as_deref(), f.installed_sha256()].into_iter().flatten() {
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
    fn every_file_is_checked_against_a_checksum() {
        // What the editor loads (models, the ONNX Runtime library) is pinned:
        // the download, or the file taken from it.
        for p in catalog() {
            for f in &p.files {
                assert!(f.sha256.is_some() || f.installed_sha256().is_some(), "{} has no checksum", f.path);
            }
        }
        let siglip = pack(SIGLIP2_ID).expect("siglip2");
        assert!(siglip.files.iter().all(|f| !f.url.contains("/resolve/main/")), "SigLIP 2 at a fixed revision");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(megabytes(76_725_198), "77 MB");
        assert_eq!(megabytes(1_200_000_000), "1.2 GB");
        let scunet = pack(SCUNET_ID).expect("scunet");
        assert_eq!(scunet.size_text(), "72 MB");
        assert_eq!(pack(SIGLIP2_ID).expect("siglip2").size_text(), "1.5 GB");
        let unknown = Pack { files: vec![file("x", "https://example.org/x".to_owned(), None, None)], ..scunet };
        assert_eq!(unknown.size_text(), "size unknown");
    }

    #[test]
    fn a_partial_file_is_not_installed_and_the_right_size_is() {
        let dir = temp_dir("status");
        let scunet = pack(SCUNET_ID).expect("scunet");
        assert_eq!(missing(&[SCUNET_ID], &dir).len(), 1);
        let path = installed_path(&dir, &scunet.files[0]);
        std::fs::write(&path, b"").expect("write");
        assert_eq!(missing_files(&scunet, &dir).len(), 1, "an empty converted file is not installed");
        std::fs::write(&path, b"not the model").expect("write");
        assert!(missing_files(&scunet, &dir).is_empty());
        assert_eq!(
            damaged_files(&scunet, &dir).len(),
            1,
            "a converted file is checked against the conversion's checksum"
        );
        let siglip = pack(SIGLIP2_ID).expect("siglip2");
        let tokenizer = siglip.files.iter().find(|f| f.path.ends_with("tokenizer.json")).expect("tokenizer");
        let tok_path = installed_path(&dir, tokenizer);
        std::fs::create_dir_all(tok_path.parent().expect("parent")).expect("dir");
        std::fs::write(&tok_path, b"{}").expect("write");
        assert!(!is_installed(&dir, tokenizer), "a file of the wrong size is not installed");
        assert_eq!(missing_files(&siglip, &dir).len(), 3);

        let inner = |e: Extract| Download {
            path: "lib.dll".to_owned(),
            url: "https://example.org/pkg.nupkg".to_owned(),
            bytes: None,
            sha256: None,
            extract: Some(e),
            convert: None,
        };
        std::fs::write(dir.join("lib.dll"), b"library").expect("write");
        assert!(is_installed(&dir, &inner(Extract::new("lib.dll"))), "unknown size: any non-empty file counts");
        let pinned = inner(Extract::pinned("lib.dll", 7, &"0".repeat(64)));
        assert!(is_installed(&dir, &pinned), "an extracted file of the pinned size");
        assert!(!is_installed(&dir, &inner(Extract::pinned("lib.dll", 8, &"0".repeat(64)))));
        let lib = Pack { files: vec![pinned], ..siglip };
        assert_eq!(damaged_files(&lib, &dir).len(), 1, "an extracted file is checked against its own checksum");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn both_scunet_models_are_made_from_one_download_of_the_original() {
        let (a, b) = (pack(SCUNET_ID).expect("448"), pack(SCUNET_FAST_ID).expect("960"));
        let (a, b) = (&a.files[0], &b.files[0]);
        assert_eq!(a.url, b.url);
        assert!(a.url.starts_with("https://github.com/cszn/KAIR/"));
        let dir = Path::new("m");
        assert_eq!(source_path(dir, a), source_path(dir, b));
        assert_eq!(source_path(dir, a), dir.join("sources").join("scunet_color_real_psnr.pth"));
        assert_ne!(a.installed_sha256(), a.sha256.as_deref());
    }
}
