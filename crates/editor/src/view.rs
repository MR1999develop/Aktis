//! Where the image sits in the viewer: zoom, pan, fit and 100 %.
//!
//! Plain geometry in screen points, independent of the UI toolkit, so it is
//! tested without a window. Image coordinates are pixels of the full-size
//! upright image, whatever resolution the texture on screen has.

/// A rectangle in screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// Centre point.
    #[must_use]
    pub fn center(&self) -> [f32; 2] {
        [self.x + self.w / 2.0, self.y + self.h / 2.0]
    }
}

/// The area the image is drawn in, and the screen's density.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Area in points.
    pub rect: Rect,
    /// Physical pixels per point (1.0 at 100 % system scaling, 1.5 at 150 %).
    pub pixels_per_point: f32,
}

/// Largest magnification: 32 screen pixels per image pixel.
pub const MAX_PIXEL_ZOOM: f32 = 32.0;

/// Space left around the image at [`Zoom::Fit`], in points on each side, so
/// it does not touch the viewer's edges (as in Capture One).
pub const FIT_MARGIN: f32 = 16.0;

/// Zoom step of one wheel notch or Ctrl+Plus.
pub const ZOOM_STEP: f32 = 1.25;

/// How the image is scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// Whole image visible, centred; never enlarged beyond 100 %.
    Fit,
    /// Fixed scale: screen pixels per image pixel (1.0 = 100 %), with the
    /// image point shown at the viewport's centre.
    Pixels {
        /// Screen pixels per image pixel.
        scale: f32,
        /// Image point at the centre of the viewport.
        center: [f32; 2],
    },
}

/// Zoom and pan of the viewer. Starts at [`Zoom::Fit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    zoom: Zoom,
}

impl Default for ViewState {
    fn default() -> Self {
        Self { zoom: Zoom::Fit }
    }
}

impl ViewState {
    /// The current zoom.
    #[must_use]
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// Returns to fit (also used when another image is opened).
    pub fn fit(&mut self) {
        self.zoom = Zoom::Fit;
    }

    /// Screen pixels per image pixel that fit `image` in the viewport with
    /// [`FIT_MARGIN`] around it, at most 1. A viewport too small for the
    /// margin is used whole.
    #[must_use]
    pub fn fit_scale(image: [f32; 2], vp: &Viewport) -> f32 {
        if image[0] <= 0.0 || image[1] <= 0.0 {
            return 1.0;
        }
        let inner = |side: f32| if side > 8.0 * FIT_MARGIN { side - 2.0 * FIT_MARGIN } else { side };
        let px_w = inner(vp.rect.w) * vp.pixels_per_point;
        let px_h = inner(vp.rect.h) * vp.pixels_per_point;
        (px_w / image[0]).min(px_h / image[1]).clamp(f32::MIN_POSITIVE, 1.0)
    }

    /// Screen pixels per image pixel now (`1.0` is 100 %).
    #[must_use]
    pub fn scale(&self, image: [f32; 2], vp: &Viewport) -> f32 {
        match self.zoom {
            Zoom::Fit => Self::fit_scale(image, vp),
            Zoom::Pixels { scale, .. } => scale,
        }
    }

    /// Where the image is drawn, in points. It may extend past the viewport.
    #[must_use]
    pub fn image_rect(&self, image: [f32; 2], vp: &Viewport) -> Rect {
        let points_per_px = self.scale(image, vp) / vp.pixels_per_point;
        let (w, h) = (image[0] * points_per_px, image[1] * points_per_px);
        let [cx, cy] = vp.rect.center();
        let center = match self.zoom {
            Zoom::Fit => [image[0] / 2.0, image[1] / 2.0],
            Zoom::Pixels { center, .. } => center,
        };
        Rect { x: cx - center[0] * points_per_px, y: cy - center[1] * points_per_px, w, h }
    }

    /// Shows the image at 100 %, keeping the point at `anchor` (points) still,
    /// or the centre when `anchor` is `None`.
    pub fn actual_pixels(&mut self, anchor: Option<[f32; 2]>, image: [f32; 2], vp: &Viewport) {
        let factor = 1.0 / self.scale(image, vp);
        self.zoom_by(factor, anchor, image, vp);
    }

    /// Multiplies the zoom by `factor`, keeping the image point under
    /// `anchor` (points) still, or the centre when `anchor` is `None`.
    /// Zooming out below fit returns to [`Zoom::Fit`].
    pub fn zoom_by(&mut self, factor: f32, anchor: Option<[f32; 2]>, image: [f32; 2], vp: &Viewport) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let old = self.scale(image, vp);
        let fit = Self::fit_scale(image, vp);
        let new = (old * factor).min(MAX_PIXEL_ZOOM);
        if factor < 1.0 && new <= fit * 1.0001 {
            self.zoom = Zoom::Fit;
            return;
        }
        let vc = vp.rect.center();
        let a = anchor.unwrap_or(vc);
        let old_ppx = old / vp.pixels_per_point;
        let new_ppx = new / vp.pixels_per_point;
        let r = self.image_rect(image, vp);
        // Image point under the anchor, before and (by construction) after.
        let p = [(a[0] - r.x) / old_ppx, (a[1] - r.y) / old_ppx];
        let center = [p[0] - (a[0] - vc[0]) / new_ppx, p[1] - (a[1] - vc[1]) / new_ppx];
        self.zoom = Zoom::Pixels { scale: new, center };
        self.clamp(image, vp);
    }

    /// Moves the image by `delta` points (drag to pan). Does nothing at fit.
    pub fn pan(&mut self, delta: [f32; 2], image: [f32; 2], vp: &Viewport) {
        if let Zoom::Pixels { scale, center } = &mut self.zoom {
            let ppx = *scale / vp.pixels_per_point;
            center[0] -= delta[0] / ppx;
            center[1] -= delta[1] / ppx;
        }
        self.clamp(image, vp);
    }

    /// Keeps the image covering the viewport where it can, and centred on
    /// an axis where it is smaller than the viewport.
    fn clamp(&mut self, image: [f32; 2], vp: &Viewport) {
        if let Zoom::Pixels { scale, center } = &mut self.zoom {
            let ppx = *scale / vp.pixels_per_point;
            for (axis, size) in [(0, vp.rect.w), (1, vp.rect.h)] {
                let half = size / 2.0 / ppx;
                center[axis] = if image[axis] <= 2.0 * half {
                    image[axis] / 2.0
                } else {
                    center[axis].clamp(half, image[axis] - half)
                };
            }
        }
    }

    /// Zoom in percent for the status bar.
    #[must_use]
    pub fn percent(&self, image: [f32; 2], vp: &Viewport) -> f32 {
        self.scale(image, vp) * 100.0
    }
}

/// Whether the full-size image adds detail over the preview at this
/// zoom: when one preview pixel would cover more than one screen pixel.
/// `screen_scale` is screen pixels per full-size pixel ([`ViewState::scale`]),
/// `preview_scale` the preview's size over the full size.
#[must_use]
pub fn needs_full_resolution(screen_scale: f32, preview_scale: f32) -> bool {
    // A little slack: a preview shown at 103 % looks as sharp.
    screen_scale > preview_scale * 1.05
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const IMAGE: [f32; 2] = [4000.0, 3000.0];

    fn vp(ppp: f32) -> Viewport {
        Viewport { rect: Rect { x: 100.0, y: 50.0, w: 800.0, h: 400.0 }, pixels_per_point: ppp }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3 * a.abs().max(1.0)
    }

    #[test]
    fn fit_shows_the_whole_image_centred() {
        let v = ViewState::default();
        let r = v.image_rect(IMAGE, &vp(1.0));
        // Height limits: (400 − 2 × 16) / 3000, a margin above and below.
        assert!(close(r.h, 368.0) && close(r.w, 490.667), "{r:?}");
        assert!(close(r.y, 50.0 + FIT_MARGIN), "{r:?}");
        assert!(close(r.x + r.w / 2.0, 500.0));
        // A viewport too small for a margin is used whole.
        let tiny = Viewport { rect: Rect { x: 0.0, y: 0.0, w: 100.0, h: 75.0 }, pixels_per_point: 1.0 };
        let r = v.image_rect(IMAGE, &tiny);
        assert!(close(r.w, 100.0) && close(r.h, 75.0), "{r:?}");
    }

    #[test]
    fn fit_never_enlarges_a_small_image() {
        let v = ViewState::default();
        let r = v.image_rect([200.0, 100.0], &vp(1.0));
        assert_eq!((r.w, r.h), (200.0, 100.0));
        assert_eq!(v.percent([200.0, 100.0], &vp(1.0)), 100.0);
    }

    #[test]
    fn one_hundred_percent_means_one_screen_pixel_per_image_pixel_on_hidpi_too() {
        let mut v = ViewState::default();
        let vp2 = vp(2.0);
        v.actual_pixels(None, IMAGE, &vp2);
        let r = v.image_rect(IMAGE, &vp2);
        // 4000 image pixels = 4000 physical pixels = 2000 points.
        assert!(close(r.w, 2000.0));
        assert!(close(v.percent(IMAGE, &vp2), 100.0));
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let mut v = ViewState::default();
        let vp1 = vp(1.0);
        v.actual_pixels(None, IMAGE, &vp1);
        let cursor = [300.0, 120.0];
        let before = v.image_rect(IMAGE, &vp1);
        let p_before = [(cursor[0] - before.x) / 1.0, (cursor[1] - before.y) / 1.0];
        v.zoom_by(2.0, Some(cursor), IMAGE, &vp1);
        let after = v.image_rect(IMAGE, &vp1);
        let p_after = [(cursor[0] - after.x) / 2.0, (cursor[1] - after.y) / 2.0];
        assert!(close(p_before[0], p_after[0]) && close(p_before[1], p_after[1]), "{p_before:?} {p_after:?}");
    }

    #[test]
    fn zooming_out_past_fit_returns_to_fit_and_zoom_is_capped() {
        let mut v = ViewState::default();
        let vp1 = vp(1.0);
        v.zoom_by(ZOOM_STEP, None, IMAGE, &vp1);
        assert!(matches!(v.zoom(), Zoom::Pixels { .. }));
        for _ in 0..5 {
            v.zoom_by(1.0 / ZOOM_STEP, None, IMAGE, &vp1);
        }
        assert_eq!(v.zoom(), Zoom::Fit);
        for _ in 0..100 {
            v.zoom_by(ZOOM_STEP, None, IMAGE, &vp1);
        }
        assert_eq!(v.scale(IMAGE, &vp1), MAX_PIXEL_ZOOM);
        v.zoom_by(f32::NAN, None, IMAGE, &vp1);
        v.zoom_by(0.0, None, IMAGE, &vp1);
        assert_eq!(v.scale(IMAGE, &vp1), MAX_PIXEL_ZOOM);
    }

    #[test]
    fn panning_stops_at_the_image_edges() {
        let mut v = ViewState::default();
        let vp1 = vp(1.0);
        v.actual_pixels(None, IMAGE, &vp1);
        v.pan([1.0e6, 1.0e6], IMAGE, &vp1);
        let r = v.image_rect(IMAGE, &vp1);
        assert!(close(r.x, 100.0) && close(r.y, 50.0), "{r:?}");
        v.pan([-1.0e6, -1.0e6], IMAGE, &vp1);
        let r = v.image_rect(IMAGE, &vp1);
        assert!(close(r.x + r.w, 900.0) && close(r.y + r.h, 450.0), "{r:?}");
    }

    #[test]
    fn an_axis_smaller_than_the_viewport_stays_centred() {
        let mut v = ViewState::default();
        let vp1 = vp(1.0);
        let wide = [3000.0, 300.0];
        v.actual_pixels(None, wide, &vp1);
        v.pan([0.0, 500.0], wide, &vp1);
        let r = v.image_rect(wide, &vp1);
        assert!(close(r.y + r.h / 2.0, 250.0), "{r:?}");
    }

    #[test]
    fn panning_at_fit_does_nothing() {
        let mut v = ViewState::default();
        v.pan([30.0, 30.0], IMAGE, &vp(1.0));
        assert_eq!(v.zoom(), Zoom::Fit);
    }

    #[test]
    fn the_full_image_is_rendered_only_when_the_preview_would_be_enlarged() {
        // A 6000 px photo with a 2560 px preview.
        let preview = 2560.0 / 6000.0;
        assert!(!needs_full_resolution(0.2, preview), "fit on a laptop screen");
        assert!(!needs_full_resolution(0.44, preview), "a hair above the preview size");
        assert!(needs_full_resolution(1.0, preview), "100 %");
        assert!(!needs_full_resolution(1.0, 1.0), "the preview is the full image");
    }
}
