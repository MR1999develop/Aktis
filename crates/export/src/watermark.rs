//! A text watermark drawn into the exported pixels with the built-in
//! stroke font: white, anti-aliased, with a soft dark shadow so it reads on
//! light and dark areas alike.

use crate::RgbImage16;
use crate::font;

/// Where the watermark sits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Position {
    /// Top left corner.
    TopLeft,
    /// Top edge, centred.
    Top,
    /// Top right corner.
    TopRight,
    /// Left edge, centred.
    Left,
    /// The middle of the image.
    Center,
    /// Right edge, centred.
    Right,
    /// Bottom left corner.
    BottomLeft,
    /// Bottom edge, centred.
    Bottom,
    /// Bottom right corner.
    #[default]
    BottomRight,
}

impl Position {
    /// The nine positions, row by row from the top left.
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// Horizontal and vertical placement: 0 = start, 1 = centre, 2 = end.
    fn grid(self) -> (u8, u8) {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(8);
        #[allow(clippy::cast_possible_truncation)] // 0–8
        let i = i as u8;
        (i % 3, i / 3)
    }
}

/// A text watermark.
#[derive(Debug, Clone, PartialEq)]
pub struct Watermark {
    /// The text ("© 2026 Name").
    pub text: String,
    /// Capital height as a percentage of the image's longer side.
    pub size_percent: f32,
    /// 0 (invisible) to 1 (opaque).
    pub opacity: f32,
    /// Where it sits.
    pub position: Position,
    /// Distance from the edges as a percentage of the longer side.
    pub margin_percent: f32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            text: String::new(),
            size_percent: 3.0,
            opacity: 0.6,
            position: Position::BottomRight,
            margin_percent: 2.0,
        }
    }
}

/// A point in pixels.
type Point = (f32, f32);

/// Distance from `p` to the segment `a`–`b`.
fn distance(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    (qx * qx + qy * qy).sqrt()
}

/// Coverage (0–1) of a `width × height` mask by `segments` (pixel
/// coordinates, y down) drawn `radius` pixels thick on each side.
fn coverage(segments: &[(Point, Point)], radius: f32, width: usize, height: usize) -> Vec<f32> {
    let mut mask = vec![0.0_f32; width * height];
    let reach = radius + 1.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let span = |lo: f32, hi: f32, max: usize| {
        let a = (lo - reach).floor().max(0.0) as usize;
        let b = ((hi + reach).ceil().max(0.0) as usize).min(max);
        a..b
    };
    for &(a, b) in segments {
        for y in span(a.1.min(b.1), a.1.max(b.1), height) {
            for x in span(a.0.min(b.0), a.0.max(b.0), width) {
                #[allow(clippy::cast_precision_loss)] // pixel indices
                let d = distance((x as f32 + 0.5, y as f32 + 0.5), a, b);
                let c = (radius + 0.5 - d).clamp(0.0, 1.0);
                let m = &mut mask[y * width + x];
                *m = m.max(c);
            }
        }
    }
    mask
}

/// Draws `mark` into `image`. Text the font lacks is drawn as `?`; text
/// wider than the image is made smaller to fit. Nothing happens for empty
/// text or zero opacity.
pub fn draw_watermark(image: &mut RgbImage16, mark: &Watermark) {
    let opacity = mark.opacity.clamp(0.0, 1.0);
    let text = mark.text.trim();
    if text.is_empty() || opacity <= 0.0 || image.width == 0 || image.height == 0 {
        return;
    }
    let (strokes, _) = font::layout(text);
    if strokes.is_empty() {
        return;
    }
    #[allow(clippy::cast_precision_loss)] // image sides are far below 2^24
    let (w, h) = (image.width as f32, image.height as f32);
    let long = w.max(h);
    let margin = (mark.margin_percent.max(0.0) / 100.0 * long).min(w.min(h) / 4.0);
    // Ink box in font units.
    let half = font::STROKE / 2.0;
    let points = strokes.iter().flatten();
    let (min_x, max_x, min_y, max_y) = points.fold((f32::MAX, f32::MIN, f32::MAX, f32::MIN), |b, p| {
        (b.0.min(p.0 - half), b.1.max(p.0 + half), b.2.min(p.1 - half), b.3.max(p.1 + half))
    });
    let mut scale = mark.size_percent.max(0.1) / 100.0 * long / font::CAP_HEIGHT;
    let room = (w - 2.0 * margin).max(1.0);
    if (max_x - min_x) * scale > room {
        scale = room / (max_x - min_x);
    }
    let (box_w, box_h) = ((max_x - min_x) * scale, (max_y - min_y) * scale);
    let (gx, gy) = mark.position.grid();
    let place = |g: u8, size: f32, total: f32| match g {
        0 => margin,
        1 => (total - size) / 2.0,
        _ => total - margin - size,
    };
    let (left, top) = (place(gx, box_w, w), place(gy, box_h, h));
    let shadow = (0.06 * font::CAP_HEIGHT * scale).max(1.0);
    // The mask covers the box, the shadow and one pixel of anti-aliasing.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (ox, oy) = (left.floor().max(0.0) as usize, top.floor().max(0.0) as usize);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (mw, mh) = (
        ((left + box_w + shadow + 2.0).ceil().max(0.0) as usize).min(image.width as usize).saturating_sub(ox),
        ((top + box_h + shadow + 2.0).ceil().max(0.0) as usize).min(image.height as usize).saturating_sub(oy),
    );
    if mw == 0 || mh == 0 {
        return;
    }
    #[allow(clippy::cast_precision_loss)] // pixel offsets
    let (fx, fy) = (ox as f32, oy as f32);
    // Font units (y up) to mask pixels (y down).
    let to_px = |p: &(f32, f32), shift: f32| {
        ((p.0 - min_x) * scale + left - fx + shift, (max_y - p.1) * scale + top - fy + shift)
    };
    let segments = |shift: f32| -> Vec<_> {
        strokes
            .iter()
            .flat_map(|line| line.windows(2).map(move |s| (to_px(&s[0], shift), to_px(&s[1], shift))))
            .collect()
    };
    let radius = half * scale;
    let text_mask = coverage(&segments(0.0), radius, mw, mh);
    let shadow_mask = coverage(&segments(shadow), radius * 1.15, mw, mh);
    let row = image.width as usize * 3;
    for y in 0..mh {
        for x in 0..mw {
            let (t, s) = (text_mask[y * mw + x] * opacity, shadow_mask[y * mw + x] * opacity * 0.45);
            if t <= 0.0 && s <= 0.0 {
                continue;
            }
            let at = (oy + y) * row + (ox + x) * 3;
            for v in &mut image.pixels[at..at + 3] {
                let darkened = f32::from(*v) * (1.0 - s);
                let lit = darkened * (1.0 - t) + 65_535.0 * t;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 16 bits
                let out = lit.round().clamp(0.0, 65_535.0) as u16;
                *v = out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: u32, h: u32) -> RgbImage16 {
        RgbImage16 { width: w, height: h, pixels: vec![30_000; (w * h * 3) as usize] }
    }

    /// Bounding box of the pixels made lighter: (left, top, right, bottom).
    fn lit_box(img: &RgbImage16) -> Option<(usize, usize, usize, usize)> {
        let w = img.width as usize;
        let mut b: Option<(usize, usize, usize, usize)> = None;
        for (i, px) in img.pixels.chunks(3).enumerate() {
            if px[0] > 40_000 {
                let (x, y) = (i % w, i / w);
                b = Some(b.map_or((x, y, x, y), |(l, t, r, bt)| (l.min(x), t.min(y), r.max(x), bt.max(y))));
            }
        }
        b
    }

    #[test]
    fn the_watermark_lands_where_asked_with_its_margin() {
        let mark = Watermark { text: "TEST".into(), size_percent: 10.0, opacity: 1.0, ..Watermark::default() };
        let mut img = grey(400, 200);
        draw_watermark(&mut img, &Watermark { position: Position::BottomRight, ..mark.clone() });
        let (l, t, r, b) = lit_box(&img).expect("drawn");
        // Margin: 2 % of 400 = 8 px; capital height: 10 % of 400 = 40 px.
        assert!((388..=392).contains(&r) && (188..=192).contains(&b), "{l} {t} {r} {b}");
        assert!((38..=46).contains(&(b - t)), "height {}", b - t);
        assert!(l > 200);

        let mut img = grey(400, 200);
        draw_watermark(&mut img, &Watermark { position: Position::TopLeft, ..mark.clone() });
        let (l, t, _, _) = lit_box(&img).expect("drawn");
        assert!((7..=10).contains(&l) && (7..=10).contains(&t), "{l} {t}");

        let mut img = grey(400, 200);
        draw_watermark(&mut img, &Watermark { position: Position::Center, ..mark });
        let (l, t, r, b) = lit_box(&img).expect("drawn");
        assert!(((l + r) as i64 - 400).abs() <= 3 && ((t + b) as i64 - 200).abs() <= 3, "{l} {t} {r} {b}");
    }

    #[test]
    fn opacity_blends_and_edges_are_anti_aliased() {
        let mut img = grey(300, 120);
        let mark = Watermark { text: "O".into(), size_percent: 20.0, opacity: 0.5, ..Watermark::default() };
        draw_watermark(&mut img, &mark);
        let max = img.pixels.iter().copied().max().unwrap_or(0);
        // Half-way from 30 000 to white, a little less where the shadow darkens.
        assert!((45_000..=48_000).contains(&max), "{max}");
        let levels: std::collections::BTreeSet<u16> = img.pixels.iter().copied().collect();
        assert!(levels.len() > 10, "soft edges, not two levels");
    }

    #[test]
    fn text_too_wide_is_shrunk_and_empty_text_draws_nothing() {
        let mut img = grey(200, 100);
        let mark = Watermark { text: "WWWWWWWWWWWW".into(), size_percent: 20.0, opacity: 1.0, ..Watermark::default() };
        draw_watermark(&mut img, &mark);
        let (l, _, r, _) = lit_box(&img).expect("drawn");
        assert!(l >= 3 && r <= 197, "{l} {r}");

        let mut img = grey(50, 50);
        let before = img.clone();
        draw_watermark(&mut img, &Watermark { text: "  ".into(), ..Watermark::default() });
        draw_watermark(&mut img, &Watermark { text: "x".into(), opacity: 0.0, ..Watermark::default() });
        assert_eq!(img, before);
    }
}
