//! What the export window sets: file format, size, name, folder,
//! watermark and metadata.

use crate::metadata::MetadataChoice;
use crate::naming::{Destination, NamePattern};
use crate::watermark::Watermark;

/// The kind of file written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileFormat {
    /// JPEG, 8 bits, quality 1–100.
    #[default]
    Jpeg,
    /// PNG, 8 or 16 bits, lossless.
    Png,
    /// TIFF, 8 or 16 bits, uncompressed.
    Tiff,
}

impl FileFormat {
    /// Every format, in menu order.
    pub const ALL: [Self; 3] = [Self::Jpeg, Self::Png, Self::Tiff];

    /// File name extension, without the dot.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Tiff => "tif",
        }
    }

    /// Label for menus.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Tiff => "TIFF",
        }
    }
}

/// Bits per channel of PNG and TIFF files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BitDepth {
    /// 8 bits: smaller files.
    #[default]
    Eight,
    /// 16 bits: for further editing.
    Sixteen,
}

/// File format and its settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// The kind of file.
    pub kind: FileFormat,
    /// JPEG quality, 1–100.
    pub jpeg_quality: u8,
    /// PNG and TIFF bits per channel (JPEG is always 8).
    pub depth: BitDepth,
}

impl Default for Format {
    fn default() -> Self {
        Self { kind: FileFormat::Jpeg, jpeg_quality: 92, depth: BitDepth::Eight }
    }
}

/// How the exported image is sized.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Resize {
    /// The full size of the edited photo.
    #[default]
    Original,
    /// The longer side becomes this many pixels.
    LongEdge(u32),
    /// The width becomes this many pixels.
    Width(u32),
    /// The height becomes this many pixels.
    Height(u32),
    /// Both sides scaled by this percentage.
    Percent(f32),
}

/// Size of the exported image.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    /// The rule.
    pub resize: Resize,
    /// Make the image larger when the rule asks for it (else it never grows).
    pub upscale: bool,
}

/// `num × a / b`, rounded, at least 1.
fn scaled(a: u32, num: u32, b: u32) -> u32 {
    let v = (u64::from(a) * u64::from(num) + u64::from(b) / 2) / u64::from(b.max(1));
    u32::try_from(v.max(1)).unwrap_or(u32::MAX)
}

impl Size {
    /// The exported size of an upright `width × height` image.
    #[must_use]
    pub fn target(&self, width: u32, height: u32) -> (u32, u32) {
        if width == 0 || height == 0 {
            return (width, height);
        }
        let grow = |n: u32, side: u32| n > side;
        match self.resize {
            Resize::Original => (width, height),
            Resize::LongEdge(n) | Resize::Width(n) | Resize::Height(n) if n == 0 => (width, height),
            Resize::LongEdge(n) => {
                let long = width.max(height);
                if grow(n, long) && !self.upscale {
                    (width, height)
                } else if width >= height {
                    (n, scaled(height, n, width))
                } else {
                    (scaled(width, n, height), n)
                }
            }
            Resize::Width(n) => {
                if grow(n, width) && !self.upscale {
                    (width, height)
                } else {
                    (n, scaled(height, n, width))
                }
            }
            Resize::Height(n) => {
                if grow(n, height) && !self.upscale {
                    (width, height)
                } else {
                    (scaled(width, n, height), n)
                }
            }
            Resize::Percent(p) => {
                let p = if p.is_finite() && p > 0.0 { p } else { 100.0 };
                let p = if p > 100.0 && !self.upscale { 100.0 } else { p };
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to sensible sides
                let side = |s: u32| (f64::from(s) * f64::from(p) / 100.0).round().clamp(1.0, 1e6) as u32;
                (side(width), side(height))
            }
        }
    }
}

/// Every choice of the export window.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExportOptions {
    /// File format.
    pub format: Format,
    /// Size.
    pub size: Size,
    /// File names.
    pub name: NamePattern,
    /// Folder.
    pub destination: Destination,
    /// A text watermark, or none.
    pub watermark: Option<Watermark>,
    /// Which metadata to write.
    pub metadata: MetadataChoice,
    /// Copyright notice written instead of the camera's; empty keeps the camera's.
    pub copyright: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(resize: Resize, upscale: bool) -> Size {
        Size { resize, upscale }
    }

    #[test]
    fn long_edge_width_and_height_keep_the_aspect_ratio() {
        assert_eq!(size(Resize::LongEdge(2048), false).target(6000, 4000), (2048, 1365));
        assert_eq!(size(Resize::LongEdge(2048), false).target(4000, 6000), (1365, 2048));
        assert_eq!(size(Resize::Width(1000), false).target(6000, 4000), (1000, 667));
        assert_eq!(size(Resize::Height(1000), false).target(6000, 4000), (1500, 1000));
        assert_eq!(size(Resize::Percent(50.0), false).target(6001, 4001), (3001, 2001));
        assert_eq!(size(Resize::Original, false).target(6000, 4000), (6000, 4000));
    }

    #[test]
    fn images_never_grow_unless_asked() {
        assert_eq!(size(Resize::LongEdge(8000), false).target(6000, 4000), (6000, 4000));
        assert_eq!(size(Resize::LongEdge(8000), true).target(6000, 4000), (8000, 5333));
        assert_eq!(size(Resize::Width(9000), false).target(6000, 4000), (6000, 4000));
        assert_eq!(size(Resize::Percent(200.0), false).target(100, 50), (100, 50));
        assert_eq!(size(Resize::Percent(200.0), true).target(100, 50), (200, 100));
    }

    #[test]
    fn degenerate_rules_leave_the_size_alone_and_sides_stay_positive() {
        assert_eq!(size(Resize::LongEdge(0), true).target(600, 400), (600, 400));
        assert_eq!(size(Resize::Percent(f32::NAN), true).target(600, 400), (600, 400));
        assert_eq!(size(Resize::LongEdge(10), false).target(6000, 10), (10, 1));
    }
}
