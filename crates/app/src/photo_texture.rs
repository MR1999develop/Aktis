//! The photo on screen as one or more GPU textures.
//!
//! A graphics card takes textures up to a largest side (often 8192 pixels),
//! smaller than the photos of medium-format cameras (a Fujifilm GFX 100 is
//! 11648 pixels wide). Instead of shrinking the photo, which would make
//! 100 % show half the detail, it is cut into tiles that each fit, drawn
//! side by side.

use eframe::egui::{self, Color32, TextureHandle, TextureOptions};

/// Texture options for pictures shown smaller than they are (thumbnails,
/// previews): averaged through mipmaps, so they stay smooth and sharp
/// instead of shimmering.
#[must_use]
pub fn smooth() -> TextureOptions {
    TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear))
}

/// A picture uploaded in tiles no larger than the GPU allows.
pub struct PhotoTexture {
    tiles: Vec<(TextureHandle, Tile)>,
    size: [usize; 2],
}

/// Where a tile sits in the picture, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// Left column.
    pub x: usize,
    /// Top row.
    pub y: usize,
    /// Width.
    pub w: usize,
    /// Height.
    pub h: usize,
}

impl PhotoTexture {
    /// Uploads `image` in tiles of at most `max_side` on each side.
    pub fn new(
        ctx: &egui::Context,
        name: &str,
        image: &egui::ColorImage,
        max_side: usize,
        options: TextureOptions,
    ) -> Self {
        let size = image.size;
        let tiles = tiles(size, max_side)
            .into_iter()
            .map(|t| {
                let part = if t.w == size[0] && t.h == size[1] { image.clone() } else { crop(image, t) };
                (ctx.load_texture(format!("{name}:{}:{}", t.x, t.y), part, options), t)
            })
            .collect();
        Self { tiles, size }
    }

    /// Draws the whole picture into `screen`.
    pub fn paint(&self, painter: &egui::Painter, screen: egui::Rect) {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for (texture, t) in &self.tiles {
            painter.image(texture.id(), tile_on_screen(*t, self.size, screen), uv, Color32::WHITE);
        }
    }
}

/// The tiles covering a picture of `size`, as few and as even as fit in
/// `max_side`, row by row.
#[must_use]
pub fn tiles(size: [usize; 2], max_side: usize) -> Vec<Tile> {
    let max_side = max_side.max(1);
    let cuts = |n: usize| -> Vec<(usize, usize)> {
        let parts = n.div_ceil(max_side).max(1);
        (0..parts).map(|k| (k * n / parts, (k + 1) * n / parts - k * n / parts)).collect()
    };
    let mut out = Vec::new();
    for (y, h) in cuts(size[1]) {
        for &(x, w) in &cuts(size[0]) {
            out.push(Tile { x, y, w, h });
        }
    }
    out
}

/// Where tile `t` of a picture of `size` drawn into `screen` goes.
#[allow(clippy::cast_precision_loss)] // pixel counts far below 2^24
fn tile_on_screen(t: Tile, size: [usize; 2], screen: egui::Rect) -> egui::Rect {
    let (sx, sy) = (screen.width() / size[0].max(1) as f32, screen.height() / size[1].max(1) as f32);
    egui::Rect::from_min_size(
        egui::pos2(screen.min.x + t.x as f32 * sx, screen.min.y + t.y as f32 * sy),
        egui::vec2(t.w as f32 * sx, t.h as f32 * sy),
    )
}

fn crop(image: &egui::ColorImage, t: Tile) -> egui::ColorImage {
    let mut bytes = Vec::with_capacity(t.w * t.h * 4);
    for y in t.y..t.y + t.h {
        let start = y * image.size[0] + t.x;
        bytes.extend(image.pixels[start..start + t.w].iter().flat_map(|c| c.to_array()));
    }
    // Colours are stored premultiplied, so they go back unchanged.
    egui::ColorImage::from_rgba_premultiplied([t.w, t.h], &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_that_fits_is_one_tile() {
        assert_eq!(tiles([6000, 4000], 8192), [Tile { x: 0, y: 0, w: 6000, h: 4000 }]);
    }

    #[test]
    fn a_larger_picture_is_cut_into_even_tiles_that_fit_and_cover_it() {
        let t = tiles([11648, 8736], 8192);
        assert_eq!(t.len(), 4);
        assert!(t.iter().all(|t| t.w <= 8192 && t.h <= 8192));
        assert_eq!(t.iter().map(|t| t.w * t.h).sum::<usize>(), 11648 * 8736);
        assert_eq!(t[1], Tile { x: 5824, y: 0, w: 5824, h: 4368 });
    }

    #[test]
    fn tiles_land_where_they_are_in_the_picture() {
        let screen = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0));
        let r = tile_on_screen(Tile { x: 50, y: 0, w: 50, h: 40 }, [100, 40], screen);
        assert_eq!(r, egui::Rect::from_min_size(egui::pos2(110.0, 20.0), egui::vec2(100.0, 100.0)));
    }

    #[test]
    fn a_crop_keeps_the_pixels_of_its_tile() {
        let rgb: Vec<u8> = (0..12_u8).flat_map(|v| [v, v, v]).collect();
        let image = egui::ColorImage::from_rgb([4, 3], &rgb);
        let part = crop(&image, Tile { x: 1, y: 1, w: 2, h: 2 });
        let grey: Vec<u8> = part.pixels.iter().map(|c| c.r()).collect();
        assert_eq!(grey, [5, 6, 9, 10]);
    }
}
