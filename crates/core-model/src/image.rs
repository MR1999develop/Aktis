//! Linear floating-point images, the only pixel format inside the editor.

use std::fmt;

/// One pixel: red, green, blue, alpha. Linear light, no clamping: values
/// below 0 or above 1 are legitimate intermediate results.
pub type Rgba = [f32; 4];

/// Largest accepted image, in pixels (400 MP). Checked before allocating.
pub const MAX_PIXELS: u64 = 400_000_000;

/// Colour space of an [`ImageF32`]. All are linear (no transfer curve).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkingSpace {
    /// Rec.2020 primaries, D65 white, linear. The editor's working space.
    LinearRec2020,
    /// sRGB / Rec.709 primaries, D65 white, linear.
    LinearSrgb,
}

/// Why an image could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// Width or height is zero.
    Empty,
    /// More than [`MAX_PIXELS`] pixels.
    TooLarge {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// The pixel buffer length does not match `width × height`.
    LengthMismatch {
        /// Pixels expected from the dimensions.
        expected: usize,
        /// Pixels actually supplied.
        actual: usize,
    },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "image has zero width or height"),
            Self::TooLarge { width, height } => write!(
                f,
                "image of {width}\u{d7}{height} pixels exceeds the limit of {} megapixels",
                MAX_PIXELS / 1_000_000
            ),
            Self::LengthMismatch { expected, actual } => {
                write!(f, "pixel buffer holds {actual} pixels, expected {expected}")
            }
        }
    }
}

impl std::error::Error for ImageError {}

/// Checks dimensions and returns the pixel count.
fn pixel_count(width: u32, height: u32) -> Result<usize, ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::Empty);
    }
    let n = u64::from(width) * u64::from(height);
    if n > MAX_PIXELS {
        return Err(ImageError::TooLarge { width, height });
    }
    usize::try_from(n).map_err(|_| ImageError::TooLarge { width, height })
}

/// A linear RGBA image in `f32`, rows top to bottom, no padding.
#[derive(Clone, PartialEq)]
pub struct ImageF32 {
    width: u32,
    height: u32,
    space: WorkingSpace,
    pixels: Vec<Rgba>,
}

impl fmt::Debug for ImageF32 {
    // Never dump millions of pixels into a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageF32")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("space", &self.space)
            .finish_non_exhaustive()
    }
}

impl ImageF32 {
    /// A black, opaque image.
    ///
    /// # Errors
    /// [`ImageError::Empty`] or [`ImageError::TooLarge`].
    pub fn new(width: u32, height: u32, space: WorkingSpace) -> Result<Self, ImageError> {
        let n = pixel_count(width, height)?;
        Ok(Self { width, height, space, pixels: vec![[0.0, 0.0, 0.0, 1.0]; n] })
    }

    /// Wraps an existing buffer of `width × height` pixels.
    ///
    /// # Errors
    /// [`ImageError`] when the dimensions are invalid or the buffer has the wrong length.
    pub fn from_pixels(width: u32, height: u32, space: WorkingSpace, pixels: Vec<Rgba>) -> Result<Self, ImageError> {
        let expected = pixel_count(width, height)?;
        if pixels.len() != expected {
            return Err(ImageError::LengthMismatch { expected, actual: pixels.len() });
        }
        Ok(Self { width, height, space, pixels })
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Colour space of the pixel values.
    #[must_use]
    pub fn space(&self) -> WorkingSpace {
        self.space
    }

    /// All pixels, row by row.
    #[must_use]
    pub fn pixels(&self) -> &[Rgba] {
        &self.pixels
    }

    /// All pixels, mutable. The length cannot change.
    pub fn pixels_mut(&mut self) -> &mut [Rgba] {
        &mut self.pixels
    }

    /// The pixel at `(x, y)`, or `None` outside the image.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<Rgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.width as usize + x as usize;
        self.pixels.get(i).copied()
    }

    /// Takes the buffer out, consuming the image.
    #[must_use]
    pub fn into_pixels(self) -> Vec<Rgba> {
        self.pixels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_sized_images_are_refused() {
        assert_eq!(ImageF32::new(0, 10, WorkingSpace::LinearRec2020), Err(ImageError::Empty));
        assert_eq!(ImageF32::new(10, 0, WorkingSpace::LinearRec2020), Err(ImageError::Empty));
    }

    #[test]
    fn an_image_over_the_limit_is_refused_before_allocating() {
        let r = ImageF32::new(100_000, 100_000, WorkingSpace::LinearRec2020);
        assert_eq!(r, Err(ImageError::TooLarge { width: 100_000, height: 100_000 }));
    }

    #[test]
    fn a_buffer_of_the_wrong_length_is_refused() {
        let r = ImageF32::from_pixels(2, 2, WorkingSpace::LinearSrgb, vec![[0.0; 4]; 3]);
        assert_eq!(r, Err(ImageError::LengthMismatch { expected: 4, actual: 3 }));
    }

    #[test]
    fn get_returns_none_outside_the_image() {
        let img = ImageF32::new(2, 1, WorkingSpace::LinearSrgb).expect("valid size");
        assert!(img.get(1, 0).is_some());
        assert!(img.get(2, 0).is_none());
        assert!(img.get(0, 1).is_none());
    }
}
