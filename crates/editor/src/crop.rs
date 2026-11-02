//! The crop box being edited: a rectangle of the
//! frame (normalised, `[x, y, w, h]`) moved by its inside and resized by
//! eight handles, optionally held to an aspect ratio; and the aspect-ratio
//! presets.

/// Smallest side of the crop box, as a share of the frame.
pub const MIN_SIDE: f32 = 0.02;

/// What part of the box a drag holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// The inside: moves the box.
    Move,
    /// Top edge.
    N,
    /// Bottom edge.
    S,
    /// Right edge.
    E,
    /// Left edge.
    W,
    /// Top-right corner.
    Ne,
    /// Top-left corner.
    Nw,
    /// Bottom-right corner.
    Se,
    /// Bottom-left corner.
    Sw,
}

impl Handle {
    /// Every resize handle, corners first.
    pub const RESIZE: [Self; 8] = [Self::Nw, Self::Ne, Self::Sw, Self::Se, Self::N, Self::S, Self::W, Self::E];

    /// Where the handle sits on a box `[x, y, w, h]`, as fractions of it.
    #[must_use]
    pub fn anchor(self) -> [f32; 2] {
        match self {
            Self::Move => [0.5, 0.5],
            Self::N => [0.5, 0.0],
            Self::S => [0.5, 1.0],
            Self::E => [1.0, 0.5],
            Self::W => [0.0, 0.5],
            Self::Ne => [1.0, 0.0],
            Self::Nw => [0.0, 0.0],
            Self::Se => [1.0, 1.0],
            Self::Sw => [0.0, 1.0],
        }
    }

    fn moves(self) -> (i8, i8) {
        // (-1 left/top edge, +1 right/bottom edge, 0 none) for x and y.
        match self {
            Self::Move => (0, 0),
            Self::N => (0, -1),
            Self::S => (0, 1),
            Self::E => (1, 0),
            Self::W => (-1, 0),
            Self::Ne => (1, -1),
            Self::Nw => (-1, -1),
            Self::Se => (1, 1),
            Self::Sw => (-1, 1),
        }
    }
}

/// An aspect-ratio preset of the crop tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aspect {
    /// Name in the menu.
    pub label: &'static str,
    /// Width / height; `None` = free, `Some(0.0)` = the photo's own.
    pub ratio: Option<f32>,
}

/// The presets, in menu order.
pub const ASPECTS: [Aspect; 9] = [
    Aspect { label: "Unconstrained", ratio: None },
    Aspect { label: "Original", ratio: Some(0.0) },
    Aspect { label: "1x1", ratio: Some(1.0) },
    Aspect { label: "3x2", ratio: Some(3.0 / 2.0) },
    Aspect { label: "4x3", ratio: Some(4.0 / 3.0) },
    Aspect { label: "5x4", ratio: Some(5.0 / 4.0) },
    Aspect { label: "16x9", ratio: Some(16.0 / 9.0) },
    Aspect { label: "21x9", ratio: Some(21.0 / 9.0) },
    Aspect { label: "65x24", ratio: Some(65.0 / 24.0) },
];

/// "16x9", "16:9" or "1.85" as width / height.
#[must_use]
pub fn parse_ratio(text: &str) -> Option<f32> {
    let t = text.trim().to_lowercase();
    let parts: Vec<&str> = t.split(['x', ':', '/']).map(str::trim).collect();
    let r = match parts.as_slice() {
        [w, h] => w.parse::<f32>().ok()? / h.parse::<f32>().ok()?,
        [r] => r.parse::<f32>().ok()?,
        _ => return None,
    };
    (r.is_finite() && (0.05..=20.0).contains(&r)).then_some(r)
}

/// The ratio in normalised units: a box `w × h` of the frame has the pixel
/// ratio `ratio` when `w / h` is this (the frame is `frame_aspect` = width / height).
#[must_use]
pub fn norm_ratio(ratio: f32, frame_aspect: f32) -> f32 {
    ratio / frame_aspect.max(1e-6)
}

/// The largest box of normalised ratio `r` centred in the frame, 85 % of
/// the way to its edges (where a newly chosen ratio starts).
#[must_use]
pub fn centred(r: f32) -> [f32; 4] {
    let (w, h) = if r >= 1.0 { (0.85, 0.85 / r) } else { (0.85 * r, 0.85) };
    let (w, h) = (w.min(1.0), h.min(1.0));
    [(1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h]
}

/// `start` (the box when the drag began) dragged by `delta` (normalised)
/// with `handle`, kept inside the frame, at least [`MIN_SIDE`], and at the
/// normalised ratio `ratio` when given.
#[must_use]
pub fn drag(start: [f32; 4], handle: Handle, delta: [f32; 2], ratio: Option<f32>) -> [f32; 4] {
    let [x, y, w, h] = start;
    if handle == Handle::Move {
        return [(x + delta[0]).clamp(0.0, 1.0 - w), (y + delta[1]).clamp(0.0, 1.0 - h), w, h];
    }
    let (mx, my) = handle.moves();
    let (mut left, mut top, mut right, mut bottom) = (x, y, x + w, y + h);
    match mx {
        -1 => left = (left + delta[0]).clamp(0.0, right - MIN_SIDE),
        1 => right = (right + delta[0]).clamp(left + MIN_SIDE, 1.0),
        _ => {}
    }
    match my {
        -1 => top = (top + delta[1]).clamp(0.0, bottom - MIN_SIDE),
        1 => bottom = (bottom + delta[1]).clamp(top + MIN_SIDE, 1.0),
        _ => {}
    }
    let Some(r) = ratio.filter(|r| *r > 0.0) else { return [left, top, right - left, bottom - top] };
    // Keep the ratio: the dragged dimension decides, the other follows,
    // growing away from the fixed side (centred for an edge handle).
    let (mut nw, mut nh) = (right - left, bottom - top);
    if mx != 0 && (my == 0 || nw / r >= nh) {
        nh = nw / r;
    } else {
        nw = nh * r;
    }
    // Room available from the fixed corner or edge.
    let room_w = match mx {
        -1 => right,
        1 => 1.0 - left,
        _ => 1.0,
    };
    let room_h = match my {
        -1 => bottom,
        1 => 1.0 - top,
        _ => 1.0,
    };
    let k = (room_w / nw).min(room_h / nh).min(1.0);
    let (nw, nh) = ((nw * k).max(MIN_SIDE), (nh * k).max(MIN_SIDE));
    let nx = match mx {
        -1 => right - nw,
        1 => left,
        _ => (x + w / 2.0 - nw / 2.0).clamp(0.0, 1.0 - nw),
    };
    let ny = match my {
        -1 => bottom - nh,
        1 => top,
        _ => (y + h / 2.0 - nh / 2.0).clamp(0.0, 1.0 - nh),
    };
    [nx, ny, nw, nh]
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn moving_keeps_the_box_in_the_frame() {
        assert!(close(drag([0.1, 0.1, 0.5, 0.5], Handle::Move, [0.2, -0.3], None), [0.3, 0.0, 0.5, 0.5]));
        assert!(close(drag([0.1, 0.1, 0.5, 0.5], Handle::Move, [0.9, 0.0], None), [0.5, 0.1, 0.5, 0.5]));
    }

    #[test]
    fn free_handles_move_their_edges_only() {
        assert!(close(drag([0.1, 0.1, 0.5, 0.5], Handle::Se, [0.1, 0.2], None), [0.1, 0.1, 0.6, 0.7]));
        assert!(close(drag([0.1, 0.1, 0.5, 0.5], Handle::W, [-0.5, 0.3], None), [0.0, 0.1, 0.6, 0.5]));
        // Never thinner than the minimum, never inverted.
        let b = drag([0.1, 0.1, 0.5, 0.5], Handle::E, [-0.9, 0.0], None);
        assert!((b[2] - MIN_SIDE).abs() < 1e-6);
    }

    #[test]
    fn a_ratio_is_kept_and_the_box_stays_inside() {
        let r = 2.0;
        let b = drag([0.1, 0.1, 0.4, 0.2], Handle::Se, [0.2, 0.0], Some(r));
        assert!((b[2] / b[3] - r).abs() < 1e-4, "{b:?}");
        assert!(b[0] + b[2] <= 1.0 + 1e-6 && b[1] + b[3] <= 1.0 + 1e-6);
        let big = drag([0.1, 0.1, 0.4, 0.2], Handle::Se, [5.0, 5.0], Some(r));
        assert!(big[0] + big[2] <= 1.0 + 1e-6 && big[1] + big[3] <= 1.0 + 1e-6 && (big[2] / big[3] - r).abs() < 1e-4);
        let top = drag([0.3, 0.3, 0.4, 0.2], Handle::N, [0.0, -0.1], Some(r));
        assert!(
            (top[2] / top[3] - r).abs() < 1e-4 && (top[1] + top[3] - 0.5).abs() < 1e-5,
            "the bottom stays: {top:?}"
        );
    }

    #[test]
    fn ratios_are_parsed_and_centred() {
        assert_eq!(parse_ratio("16x9"), Some(16.0 / 9.0));
        assert_eq!(parse_ratio(" 4:5 "), Some(0.8));
        assert_eq!(parse_ratio("1.85"), Some(1.85));
        assert_eq!(parse_ratio("abc"), None);
        assert_eq!(parse_ratio("1x0"), None);
        let c = centred(2.0);
        assert!((c[2] / c[3] - 2.0).abs() < 1e-6 && (c[0] + c[2] / 2.0 - 0.5).abs() < 1e-6);
        assert!((norm_ratio(1.5, 1.5) - 1.0).abs() < 1e-6);
    }
}
