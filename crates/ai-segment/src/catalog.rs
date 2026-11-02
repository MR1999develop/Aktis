//! The models this build knows, and where they are on disk.
//!
//! Models are not bundled with the program: they are large (35 MB to 1 GB)
//! and each comes with its own licence. Each entry names the files, their
//! size and SHA-256, where to download them and the licence, so the app can
//! offer a download and check what it got. Only models whose licence allows
//! commercial use are listed.

use std::fs;
use std::path::{Path, PathBuf};

use core_model::ModelId;

use crate::sha256::sha256_hex;

/// What a model does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    /// Finds the main subject of a photo, no prompt needed.
    Subject,
    /// Segments the object under clicks or inside a rectangle.
    Promptable,
    /// Finds people and 79 other kinds of object, with a box each.
    Detector,
    /// Splits a person into hair, skin, clothes and facial features.
    PersonParts,
}

/// One file of a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    /// Its part in the model (`"model"`, `"encoder"`, `"decoder"`).
    pub role: &'static str,
    /// File name in the models folder.
    pub name: &'static str,
    /// Where to download it.
    pub url: &'static str,
    /// Size in bytes.
    pub bytes: u64,
    /// SHA-256, lowercase hex.
    pub sha256: &'static str,
}

/// A model of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// Stable identifier, stored in recipes.
    pub id: &'static str,
    /// Name shown in the UI.
    pub name: &'static str,
    /// What it does.
    pub task: Task,
    /// Its files.
    pub files: &'static [ModelFile],
    /// Licence of the weights (SPDX identifier).
    pub license: &'static str,
    /// Project page.
    pub homepage: &'static str,
}

impl ModelSpec {
    /// The id as a [`ModelId`].
    #[must_use]
    pub fn model_id(&self) -> ModelId {
        // Catalog ids are constants checked by a test.
        ModelId::new(self.id).unwrap_or_else(|e| unreachable!("catalog id '{}' is invalid: {e}", self.id))
    }

    /// The file playing `role`.
    #[must_use]
    pub fn file(&self, role: &str) -> Option<&'static ModelFile> {
        self.files.iter().find(|f| f.role == role)
    }

    /// Total download size in bytes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

/// Subject model: BiRefNet, general-purpose weights (Zheng Peng et al., 2024).
pub const BIREFNET_GENERAL: ModelSpec = ModelSpec {
    id: "birefnet_general",
    name: "BiRefNet (subject)",
    task: Task::Subject,
    files: &[ModelFile {
        role: "model",
        name: "BiRefNet-general-epoch_244.onnx",
        url: "https://github.com/ZhengPeng7/BiRefNet/releases/download/v1/BiRefNet-general-epoch_244.onnx",
        bytes: 972_666_916,
        sha256: "58f621f00f5d756097615970a88a791584600dcf7c45b18a0a6267535a1ebd3c",
    }],
    license: "MIT",
    homepage: "https://github.com/ZhengPeng7/BiRefNet",
};

/// Promptable model: Segment Anything 2.1, Hiera-Small (Meta, 2024), in the
/// encoder/decoder split used by AnyLabeling-style exports.
pub const SAM21_SMALL: ModelSpec = ModelSpec {
    id: "sam2.1_small",
    name: "Segment Anything 2.1 Small (objects and people)",
    task: Task::Promptable,
    files: &[
        ModelFile {
            role: "encoder",
            name: "sam21-s-encoder.onnx",
            url: "https://github.com/Genichirou1/prepress-models/releases/download/sam2.1-onnx/sam21-s-encoder.onnx",
            bytes: 137_931_433,
            sha256: "10c37a22401316c3238fff11f3d9695c80081b060235282b908b187af90097ad",
        },
        ModelFile {
            role: "decoder",
            name: "sam21-s-decoder.onnx",
            url: "https://github.com/Genichirou1/prepress-models/releases/download/sam2.1-onnx/sam21-s-decoder.onnx",
            bytes: 16_505_916,
            sha256: "15961312d9dad00748d50f2f67ef80dff88e4dff201073301864c74ad10037c5",
        },
    ],
    license: "Apache-2.0",
    homepage: "https://github.com/facebookresearch/sam2",
};

/// Promptable model, large: Segment Anything 2.1, Hiera-Large. Much cleaner
/// edges and fewer holes than the small one; the default for objects and people.
pub const SAM21_LARGE: ModelSpec = ModelSpec {
    id: "sam2.1_large",
    name: "Segment Anything 2.1 Large (objects and people)",
    task: Task::Promptable,
    files: &[
        ModelFile {
            role: "encoder",
            name: "sam21-l-encoder.onnx",
            url: "https://github.com/Genichirou1/prepress-models/releases/download/sam2.1-onnx/sam21-l-encoder.onnx",
            bytes: 852_363_591,
            sha256: "3fc8f095c9c6376d209c41ed7a97db08277fb98c5581ab2ed66ff25254cbe281",
        },
        ModelFile {
            role: "decoder",
            name: "sam21-l-decoder.onnx",
            url: "https://github.com/Genichirou1/prepress-models/releases/download/sam2.1-onnx/sam21-l-decoder.onnx",
            bytes: 16_505_916,
            sha256: "ab85d0a9a3b75c402ee3cc4156fdcca09e01b9036938ba4ec49fc3d825300a98",
        },
    ],
    license: "Apache-2.0",
    homepage: "https://github.com/facebookresearch/sam2",
};

/// Person parts: MediaPipe selfie multiclass segmenter (hair, face skin, body
/// skin, clothes, accessories), BlazeFace face detector and the face
/// landmarker's 478-point mesh (eyes, irises, eyebrows, lips), Google.
pub const MEDIAPIPE_PARTS: ModelSpec = ModelSpec {
    id: "mediapipe_parts",
    name: "MediaPipe (hair, skin, clothes, face features)",
    task: Task::PersonParts,
    files: &[
        ModelFile {
            role: "segmenter",
            name: "selfie_multiclass_256x256.onnx",
            url: "https://github.com/fernandolisboa/bardo/releases/download/vision-models-1/selfie_multiclass_256x256.onnx",
            bytes: 8_357_313,
            sha256: "abb7f1ba4acf288608865f974b41ab77062d4fde837cd12da37275751be8b985",
        },
        ModelFile {
            role: "faces",
            name: "face_detector.onnx",
            url: "https://github.com/fernandolisboa/bardo/releases/download/vision-models-1/face_detector.onnx",
            bytes: 224_033,
            sha256: "d316b66c081e4c3559799f36d867634071ae45fb372690bc509bfe9a55b3e84c",
        },
        ModelFile {
            role: "landmarks",
            name: "face_landmarks_detector.onnx",
            url: "https://github.com/fernandolisboa/bardo/releases/download/vision-models-1/face_landmarks_detector.onnx",
            bytes: 2_550_548,
            sha256: "e245948ae228f075740bc1f136ce1558d1a94dd876f50a70ccd4978ad066c951",
        },
    ],
    license: "Apache-2.0",
    homepage: "https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter",
};

/// Detector: YOLOX-S, COCO classes (Megvii, 2021).
pub const YOLOX_S: ModelSpec = ModelSpec {
    id: "yolox_s",
    name: "YOLOX-S (find people and objects)",
    task: Task::Detector,
    files: &[ModelFile {
        role: "model",
        name: "yolox_s.onnx",
        url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_s.onnx",
        bytes: 35_858_002,
        sha256: "c5c2d13e59ae883e6af3b45daea64af4833a4951c92d116ec270d9ddbe998063",
    }],
    license: "Apache-2.0",
    homepage: "https://github.com/Megvii-BaseDetection/YOLOX",
};

/// Every model of this build.
pub const CATALOG: &[ModelSpec] = &[BIREFNET_GENERAL, SAM21_LARGE, SAM21_SMALL, YOLOX_S, MEDIAPIPE_PARTS];

/// The catalog entry of `id`.
#[must_use]
pub fn spec(id: &ModelId) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|s| s.id == id.as_str())
}

/// The state of a model's files on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    /// Every file is present with the expected size.
    Ready,
    /// Files that are absent.
    Missing(Vec<&'static ModelFile>),
    /// Files whose size (or, after [`ModelStore::verify`], checksum) is wrong:
    /// an interrupted download or a different version.
    Damaged(Vec<&'static ModelFile>),
}

/// The folder holding the model files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStore {
    dir: PathBuf,
}

impl ModelStore {
    /// Models in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where `file` is (or should be).
    #[must_use]
    pub fn path(&self, file: &ModelFile) -> PathBuf {
        self.dir.join(file.name)
    }

    /// Quick check: presence and size of every file.
    #[must_use]
    pub fn status(&self, spec: &'static ModelSpec) -> ModelStatus {
        let mut missing = Vec::new();
        let mut damaged = Vec::new();
        for f in spec.files {
            match fs::metadata(self.path(f)) {
                Err(_) => missing.push(f),
                Ok(m) if m.len() != f.bytes => damaged.push(f),
                Ok(_) => {}
            }
        }
        if !damaged.is_empty() {
            ModelStatus::Damaged(damaged)
        } else if !missing.is_empty() {
            ModelStatus::Missing(missing)
        } else {
            ModelStatus::Ready
        }
    }

    /// Full check: like [`Self::status`], then the SHA-256 of every file
    /// (reads every byte, so seconds for a large model).
    #[must_use]
    pub fn verify(&self, spec: &'static ModelSpec) -> ModelStatus {
        match self.status(spec) {
            ModelStatus::Ready => {}
            other => return other,
        }
        let damaged: Vec<&'static ModelFile> = spec
            .files
            .iter()
            .filter(|f| {
                let hash = fs::File::open(self.path(f)).and_then(sha256_hex);
                !hash.is_ok_and(|h| h == f.sha256)
            })
            .collect();
        if damaged.is_empty() { ModelStatus::Ready } else { ModelStatus::Damaged(damaged) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_are_valid_and_unique() {
        let mut ids = std::collections::BTreeSet::new();
        for s in CATALOG {
            assert!(ModelId::new(s.id).is_ok(), "{}", s.id);
            assert!(ids.insert(s.id), "duplicate {}", s.id);
            assert!(!s.files.is_empty());
            for f in s.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
                assert!(f.url.starts_with("https://") && f.url.ends_with(f.name), "{}", f.url);
                assert!(f.bytes > 0);
            }
        }
        assert!(SAM21_SMALL.file("encoder").is_some() && SAM21_SMALL.file("decoder").is_some());
        assert_eq!(spec(&YOLOX_S.model_id()), Some(&YOLOX_S));
    }

    #[test]
    fn status_reports_missing_and_damaged_files() {
        let dir = std::env::temp_dir().join(format!("aktis-models-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        let store = ModelStore::new(&dir);
        assert_eq!(store.status(&YOLOX_S), ModelStatus::Missing(vec![&YOLOX_S.files[0]]));
        fs::write(store.path(&YOLOX_S.files[0]), b"partial download").expect("write");
        assert_eq!(store.status(&YOLOX_S), ModelStatus::Damaged(vec![&YOLOX_S.files[0]]));
        assert_eq!(store.verify(&YOLOX_S), ModelStatus::Damaged(vec![&YOLOX_S.files[0]]));
        let _ = fs::remove_dir_all(&dir);
    }
}
