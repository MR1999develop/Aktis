//! Overlapping tiles for a network with a fixed input size (ported from an earlier
//! denoiser's `core::tiling`).
//!
//! Every useful tile measures `input_px − 2 × margin_px` per side, except at
//! the right and bottom edges where it is clipped. The network receives the
//! useful tile plus `margin_px` of context per side; outside the image the
//! context is the mirrored image, so the result does not depend on the grid.

use std::fmt;

/// An RGB image in the network's domain: interleaved pixels, rows top to bottom.
#[derive(Clone, PartialEq, Default)]
pub struct RgbImage {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 3]>,
}

impl fmt::Debug for RgbImage {
    // Never dump millions of pixels into a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbImage").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

impl RgbImage {
    /// Wraps `pixels`; `None` if their number is not `width × height`.
    #[must_use]
    pub fn new(width: usize, height: usize, pixels: Vec<[f32; 3]>) -> Option<Self> {
        (width.checked_mul(height) == Some(pixels.len())).then_some(Self { width, height, pixels })
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// All pixels, row by row.
    #[must_use]
    pub fn pixels(&self) -> &[[f32; 3]] {
        &self.pixels
    }

    fn pixel(&self, x: usize, y: usize) -> [f32; 3] {
        self.pixels.get(y * self.width + x).copied().unwrap_or_default()
    }
}

/// The inference engine failed; the text is for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelError(pub String);

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the neural network could not process the image: {}", self.0)
    }
}

impl std::error::Error for ModelError {}

/// A neural network that processes square tiles of a fixed size.
///
/// The input tile is planar (all red, then green, then blue),
/// `3 × input_px × input_px` values in 0..=1; the output has the same layout.
/// The `margin_px` pixels along each edge of the output are discarded,
/// because the network computes them without enough context.
pub trait TileModel: Send {
    /// Side of the input tile, in pixels.
    fn input_px(&self) -> usize;

    /// Pixels discarded along each edge of the output.
    fn margin_px(&self) -> usize;

    /// Processes one tile.
    ///
    /// # Errors
    /// Any error of the inference engine.
    fn run(&mut self, planar_in: &[f32], planar_out: &mut [f32]) -> Result<(), ModelError>;
}

/// How an image is split into tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilePlan {
    width: usize,
    height: usize,
    input_px: usize,
    margin_px: usize,
    core_px: usize,
    cols: usize,
    rows: usize,
}

impl TilePlan {
    /// A plan, or `None` if the image is empty or the margin leaves no useful area.
    #[must_use]
    pub fn new(width: usize, height: usize, input_px: usize, margin_px: usize) -> Option<Self> {
        let core_px = input_px.saturating_sub(margin_px.saturating_mul(2));
        if width == 0 || height == 0 || core_px == 0 {
            return None;
        }
        Some(Self {
            width,
            height,
            input_px,
            margin_px,
            core_px,
            cols: width.div_ceil(core_px),
            rows: height.div_ceil(core_px),
        })
    }

    /// Number of tiles.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cols * self.rows
    }

    /// `true` if the plan has no tiles (never, for a valid plan).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Useful rectangle `(x, y, width, height)` of tile `index`, in reading order.
    #[must_use]
    pub fn rect(&self, index: usize) -> (usize, usize, usize, usize) {
        let (row, col) = (index / self.cols, index % self.cols);
        let (x, y) = (col * self.core_px, row * self.core_px);
        (x, y, self.core_px.min(self.width.saturating_sub(x)), self.core_px.min(self.height.saturating_sub(y)))
    }
}

/// Runs `model` over `img` tile by tile and returns its output for the whole
/// image. `progress` receives `(done, total)` after each tile.
///
/// Tiles run one at a time: the engine already uses every core (or the GPU)
/// for each tile, and peak memory stays that of a single tile.
///
/// # Errors
/// The first [`ModelError`]; also when the model's sizes leave no useful tile.
pub fn run_tiles(
    img: &RgbImage,
    model: &mut dyn TileModel,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<RgbImage, ModelError> {
    let (n, margin) = (model.input_px(), model.margin_px());
    let plan = TilePlan::new(img.width, img.height, n, margin)
        .ok_or_else(|| ModelError(format!("a {n} px tile with a {margin} px margin has no useful area")))?;
    let mut out = img.clone();
    let mut input = vec![0.0_f32; 3 * n * n];
    let mut output = vec![0.0_f32; 3 * n * n];
    let total = plan.len();
    for index in 0..total {
        let rect = plan.rect(index);
        gather_planar(img, rect, margin, n, &mut input);
        model.run(&input, &mut output)?;
        scatter(&output, n, margin, rect, &mut out);
        progress(index + 1, total);
    }
    Ok(out)
}

/// Symmetrically mirrored index (…, 1, 0 | 0, 1, …, n−1 | n−1, n−2, …), valid
/// even when the distance exceeds the size.
fn reflect(i: isize, n: usize) -> usize {
    let n = isize::try_from(n.max(1)).unwrap_or(isize::MAX);
    let period = 2 * n;
    let m = i.rem_euclid(period);
    let r = if m >= n { period - 1 - m } else { m };
    r.unsigned_abs()
}

/// Signed coordinate; image sides are far below `isize::MAX`.
fn signed(v: usize) -> isize {
    isize::try_from(v).unwrap_or(isize::MAX)
}

/// Copies the useful rectangle plus `margin` per side into the planar buffer,
/// mirroring at the image edges and clamping values to 0..=1 (the range the
/// network was trained on).
fn gather_planar(img: &RgbImage, rect: (usize, usize, usize, usize), margin: usize, n: usize, out: &mut [f32]) {
    let x0 = signed(rect.0) - signed(margin);
    let y0 = signed(rect.1) - signed(margin);
    let xs: Vec<usize> = (0..n).map(|dx| reflect(x0 + signed(dx), img.width)).collect();
    let plane = n * n;
    for dy in 0..n {
        let sy = reflect(y0 + signed(dy), img.height);
        for (dx, &sx) in xs.iter().enumerate() {
            let p = img.pixel(sx, sy);
            for (c, v) in p.iter().enumerate() {
                if let Some(slot) = out.get_mut(c * plane + dy * n + dx) {
                    *slot = v.clamp(0.0, 1.0);
                }
            }
        }
    }
}

/// Writes the useful part of the planar output into `out`.
fn scatter(planar: &[f32], n: usize, margin: usize, rect: (usize, usize, usize, usize), out: &mut RgbImage) {
    let plane = n * n;
    let (x, y, w, h) = rect;
    for dy in 0..h {
        for dx in 0..w {
            let o = (margin + dy) * n + margin + dx;
            let value = |c: usize| planar.get(c * plane + o).copied().unwrap_or(0.0);
            if let Some(p) = out.pixels.get_mut((y + dy) * out.width + x + dx) {
                *p = [value(0), value(1), value(2)];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test "network": 3×3 mean with replicated border inside the tile. With a
    /// margin ≥ 1 the tiled result must equal the mean over the mirrored image.
    struct BoxBlur {
        n: usize,
        margin: usize,
        calls: usize,
    }

    impl TileModel for BoxBlur {
        fn input_px(&self) -> usize {
            self.n
        }
        fn margin_px(&self) -> usize {
            self.margin
        }
        fn run(&mut self, src: &[f32], dst: &mut [f32]) -> Result<(), ModelError> {
            self.calls += 1;
            let n = signed(self.n);
            let at = |c: isize, y: isize, x: isize| {
                let (y, x) = (y.clamp(0, n - 1), x.clamp(0, n - 1));
                src[usize::try_from((c * n + y) * n + x).expect("index")]
            };
            for (i, d) in dst.iter_mut().enumerate() {
                let i = signed(i);
                let (c, y, x) = (i / (n * n), (i / n) % n, i % n);
                let mut s = 0.0;
                for (yy, xx) in (-1..=1).flat_map(|a| (-1..=1).map(move |b| (a, b))) {
                    s += at(c, y + yy, x + xx);
                }
                *d = s / 9.0;
            }
            Ok(())
        }
    }

    fn reference_blur(img: &RgbImage) -> RgbImage {
        let (w, h) = (img.width, img.height);
        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut s = [0.0_f32; 3];
                for yy in -1..=1 {
                    for xx in -1..=1 {
                        let p = img.pixel(reflect(signed(x) + xx, w), reflect(signed(y) + yy, h));
                        for (acc, v) in s.iter_mut().zip(p) {
                            *acc += v;
                        }
                    }
                }
                pixels.push(s.map(|v| v / 9.0));
            }
        }
        RgbImage::new(w, h, pixels).expect("same size")
    }

    fn pseudo_random(w: usize, h: usize) -> RgbImage {
        let mut state = 12345_u32;
        let mut next = || {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            #[allow(clippy::cast_precision_loss)] // only test values
            let v = (state >> 8) as f32 / (1_u32 << 24) as f32;
            v
        };
        RgbImage::new(w, h, (0..w * h).map(|_| [next(), next(), next()]).collect()).expect("size")
    }

    #[test]
    fn reflect_matches_symmetric_padding() {
        let got: Vec<usize> = (-4..8).map(|i| reflect(i, 3)).collect();
        assert_eq!(got, vec![2, 2, 1, 0, 0, 1, 2, 2, 1, 0, 0, 1]);
        assert_eq!(reflect(5, 1), 0);
    }

    #[test]
    fn the_tiled_result_equals_the_whole_image_result() {
        // sizes that are not multiples of the tile, smaller and larger
        for (w, h) in [(37, 23), (5, 3), (64, 40)] {
            let img = pseudo_random(w, h);
            let mut model = BoxBlur { n: 16, margin: 2, calls: 0 };
            let mut last = (0, 0);
            let out = run_tiles(&img, &mut model, &mut |d, t| last = (d, t)).expect("runs");
            let reference = reference_blur(&img);
            let max_err = out
                .pixels()
                .iter()
                .flatten()
                .zip(reference.pixels().iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(max_err < 1e-5, "{w}\u{d7}{h}: error {max_err}");
            let tiles = TilePlan::new(w, h, 16, 2).expect("plan").len();
            assert_eq!(model.calls, tiles);
            assert_eq!(last, (tiles, tiles));
        }
    }

    #[test]
    fn values_outside_zero_to_one_reach_the_network_clamped() {
        let img = RgbImage::new(2, 1, vec![[-0.5, 0.5, 3.0], [0.2, 1.0, 0.0]]).expect("size");
        let mut planar = vec![0.0; 3 * 4 * 4];
        gather_planar(&img, (0, 0, 2, 1), 1, 4, &mut planar);
        assert!(planar.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn a_margin_without_useful_area_is_refused() {
        assert!(TilePlan::new(10, 10, 8, 4).is_none());
        assert!(TilePlan::new(0, 10, 8, 1).is_none());
        let img = pseudo_random(4, 4);
        assert!(run_tiles(&img, &mut BoxBlur { n: 8, margin: 4, calls: 0 }, &mut |_, _| {}).is_err());
    }
}
