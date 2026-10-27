//! Where AI rasters are kept: in memory, and on disk so they survive a restart.
//!
//! A raster is what a model produced for one AI component, at the model's
//! working resolution (about a megapixel). It is stored with 8 bits per
//! value, which is far finer than any model's own precision.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use core_model::{MaskImage, RasterKey};

/// Read access to computed AI rasters.
pub trait RasterSource {
    /// The raster with this key, if it has been computed.
    fn raster(&self, key: &RasterKey) -> Option<Arc<MaskImage>>;
}

/// Write access: where newly computed rasters go.
pub trait RasterStore: RasterSource {
    /// Keeps `raster` under `key`.
    ///
    /// # Errors
    /// When a persistent store cannot write it (it may still be kept in memory).
    fn store(&self, key: RasterKey, raster: MaskImage) -> io::Result<()>;
}

/// No rasters at all: every AI component counts as not computed yet.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoRasters;

impl RasterSource for NoRasters {
    fn raster(&self, _key: &RasterKey) -> Option<Arc<MaskImage>> {
        None
    }
}

/// Rasters kept in memory.
#[derive(Debug, Default)]
pub struct MemoryRasters {
    map: Mutex<HashMap<RasterKey, Arc<MaskImage>>>,
}

impl MemoryRasters {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a raster under `key`, replacing any previous one.
    pub fn insert(&self, key: RasterKey, raster: MaskImage) -> Arc<MaskImage> {
        let raster = Arc::new(raster);
        self.map.lock().unwrap_or_else(PoisonError::into_inner).insert(key, Arc::clone(&raster));
        raster
    }

    /// Number of rasters held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl RasterStore for MemoryRasters {
    fn store(&self, key: RasterKey, raster: MaskImage) -> io::Result<()> {
        self.insert(key, raster);
        Ok(())
    }
}

impl RasterSource for MemoryRasters {
    fn raster(&self, key: &RasterKey) -> Option<Arc<MaskImage>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }
}

/// File signature and version of a stored raster.
const MAGIC: &[u8; 8] = b"RNEMASK1";
/// Largest raster side accepted from disk; models work far below this.
const MAX_SIDE: u32 = 8192;
/// File name extension of stored rasters.
const EXTENSION: &str = "rnemask";

/// Rasters in a folder (one small file each), with an in-memory cache.
#[derive(Debug)]
pub struct DiskRasters {
    dir: PathBuf,
    memory: MemoryRasters,
}

impl DiskRasters {
    /// Uses `dir`, creating it if needed.
    ///
    /// # Errors
    /// When the folder cannot be created.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, memory: MemoryRasters::new() })
    }

    /// The folder holding the rasters.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: &RasterKey) -> PathBuf {
        self.dir.join(format!("{key}.{EXTENSION}"))
    }

    /// Stores a raster in memory and on disk. The file is written to a
    /// temporary name and renamed, so a crash never leaves half a raster.
    ///
    /// # Errors
    /// When the file cannot be written; the raster is still kept in memory.
    pub fn insert(&self, key: RasterKey, raster: MaskImage) -> io::Result<Arc<MaskImage>> {
        let raster = self.memory.insert(key, raster);
        let target = self.path(&key);
        let tmp = self.dir.join(format!("{key}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&encode(&raster))?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map(|()| raster)
    }

    fn load(&self, key: &RasterKey) -> Option<MaskImage> {
        let mut f = fs::File::open(self.path(key)).ok()?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).ok()?;
        decode(&bytes)
    }
}

impl RasterSource for DiskRasters {
    fn raster(&self, key: &RasterKey) -> Option<Arc<MaskImage>> {
        if let Some(r) = self.memory.raster(key) {
            return Some(r);
        }
        let loaded = self.load(key)?;
        Some(self.memory.insert(*key, loaded))
    }
}

impl RasterStore for DiskRasters {
    fn store(&self, key: RasterKey, raster: MaskImage) -> io::Result<()> {
        self.insert(key, raster).map(|_| ())
    }
}

/// `MAGIC`, width and height (u32 little-endian), then one byte per value.
fn encode(m: &MaskImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + m.values().len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&m.width().to_le_bytes());
    out.extend_from_slice(&m.height().to_le_bytes());
    out.extend(m.values().iter().map(|v| quantize(*v)));
    out
}

fn quantize(v: f32) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0–255
    let q = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    q
}

/// Reads what [`encode`] wrote; `None` for anything else (wrong signature,
/// absurd size, truncated data).
fn decode(bytes: &[u8]) -> Option<MaskImage> {
    let header = bytes.get(..16)?;
    if &header[..8] != MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(header[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(header[12..16].try_into().ok()?);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let data = bytes.get(16..)?;
    if data.len() != width as usize * height as usize {
        return None;
    }
    MaskImage::from_values(width, height, data.iter().map(|b| f32::from(*b) / 255.0).collect()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> RasterKey {
        RasterKey::from_bytes([n; 16])
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rne-masks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn rasters_round_trip_through_disk_at_eight_bits() {
        let dir = temp_dir("roundtrip");
        let m = MaskImage::from_values(3, 2, vec![0.0, 0.25, 0.5, 0.75, 1.0, 1.7]).expect("valid");
        {
            let store = DiskRasters::open(&dir).expect("open");
            store.insert(key(1), m).expect("write");
        }
        let store = DiskRasters::open(&dir).expect("reopen");
        let back = store.raster(&key(1)).expect("found on disk");
        assert_eq!((back.width(), back.height()), (3, 2));
        let expected = [0.0, 0.25, 0.5, 0.75, 1.0, 1.0];
        assert!(back.values().iter().zip(expected).all(|(a, b)| (a - b).abs() <= 0.5 / 255.0 + 1e-6));
        assert!(store.raster(&key(2)).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_files_are_ignored() {
        let m = MaskImage::filled(4, 4, 0.5).expect("valid");
        let good = encode(&m);
        assert!(decode(&good).is_some());
        assert!(decode(&good[..good.len() - 1]).is_none());
        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        assert!(decode(&wrong_magic).is_none());
        let mut huge = good;
        huge[8..12].copy_from_slice(&100_000_u32.to_le_bytes());
        assert!(decode(&huge).is_none());
    }

    #[test]
    fn the_memory_store_shares_rasters() {
        let s = MemoryRasters::new();
        assert!(s.is_empty());
        s.insert(key(3), MaskImage::filled(1, 1, 1.0).expect("valid"));
        assert_eq!(s.len(), 1);
        assert!(s.raster(&key(3)).is_some());
        assert!(NoRasters.raster(&key(3)).is_none());
    }
}
