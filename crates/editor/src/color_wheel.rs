//! The geometry of the colour wheels of the colour balance and colour
//! editor panels, kept apart from the drawing so it is tested without a
//! window.
//!
//! A wheel has radius 1 and its centre at the origin; offsets have `y`
//! pointing down, as on screen. Hue 0 (red) is on the right and the hues
//! turn counterclockwise (yellow up-right, green up-left, cyan on the left,
//! blue down-left, magenta down-right); the distance from the centre is an
//! amount, 0 (grey) at the centre to 1 at the rim.

/// `a − b` in degrees, wrapped into (−180, 180].
#[must_use]
pub fn hue_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// A colour of `hue` (degrees), `saturation` and `value` (0–1) as HSV
/// defines them: R, G and B, 0–1.
#[must_use]
pub fn hsv(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let (s, v) = (saturation.clamp(0.0, 1.0), value.clamp(0.0, 1.0));
    let h = hue.rem_euclid(360.0) / 60.0;
    let f = |n: f32| {
        let k = (n + h).rem_euclid(6.0);
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

/// The unit offset pointing at `angle` degrees (counterclockwise from the
/// right, `y` down).
#[must_use]
pub fn direction(angle: f32) -> [f32; 2] {
    let (s, c) = angle.to_radians().sin_cos();
    [c, -s]
}

/// The angle (degrees, 0–360) an offset points at; 0 for the centre itself.
#[must_use]
pub fn angle_of(offset: [f32; 2]) -> f32 {
    (-offset[1]).atan2(offset[0]).to_degrees().rem_euclid(360.0)
}

/// Where `hue` (degrees) at `amount` (0–1) sits on the wheel.
#[must_use]
pub fn wheel_offset(hue: f32, amount: f32) -> [f32; 2] {
    let d = direction(hue);
    let a = amount.clamp(0.0, 1.0);
    [d[0] * a, d[1] * a]
}

/// The hue (degrees, 0–360) and amount (0–1, held at the rim) at an offset.
#[must_use]
pub fn wheel_hue_amount(offset: [f32; 2]) -> (f32, f32) {
    (angle_of(offset), offset[0].hypot(offset[1]).min(1.0))
}

/// An arc beside a wheel on which a value is set by dragging: it starts
/// at `start` degrees (the lowest value) and turns by `sweep` degrees
/// (positive counterclockwise) to the highest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    /// Angle of the lowest value, degrees.
    pub start: f32,
    /// Turn to the highest value, degrees (positive counterclockwise).
    pub sweep: f32,
}

impl Arc {
    /// The arc on the right of a wheel: lowest at the bottom, highest at the top.
    pub const RIGHT: Self = Self { start: -50.0, sweep: 100.0 };
    /// The arc on the left of a wheel: lowest at the bottom, highest at the top.
    pub const LEFT: Self = Self { start: 230.0, sweep: -100.0 };

    /// The angle (degrees) where `value` of `range` sits.
    #[must_use]
    pub fn angle(self, value: f32, range: [f32; 2]) -> f32 {
        let span = range[1] - range[0];
        let t = if span > 0.0 { ((value - range[0]) / span).clamp(0.0, 1.0) } else { 0.0 };
        self.start + self.sweep * t
    }

    /// The value of `range` at an offset from the wheel's centre (held at
    /// the ends of the arc).
    #[must_use]
    pub fn value_at(self, offset: [f32; 2], range: [f32; 2]) -> f32 {
        let middle = self.start + self.sweep / 2.0;
        let t = (0.5 + hue_diff(angle_of(offset), middle) / self.sweep).clamp(0.0, 1.0);
        range[0] + t * (range[1] - range[0])
    }

    /// Whether an offset points within the arc, with `slack` degrees to spare at each end.
    #[must_use]
    pub fn covers(self, offset: [f32; 2], slack: f32) -> bool {
        let middle = self.start + self.sweep / 2.0;
        hue_diff(angle_of(offset), middle).abs() <= self.sweep.abs() / 2.0 + slack
    }
}

/// What a drag on a colour range's wedge changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WedgeGrab {
    /// Its centre hue (the drag turns the wedge).
    #[default]
    Center,
    /// Its width (the drag moves an edge).
    Width,
}

/// What a pointer at `hue` grabs on the wedge of a range centred on
/// `center` reaching `width` degrees each way: an edge when within `grab`
/// degrees of it, else the centre.
#[must_use]
pub fn wedge_grab(hue: f32, center: f32, width: f32, grab: f32) -> WedgeGrab {
    if (hue_diff(hue, center).abs() - width).abs() <= grab { WedgeGrab::Width } else { WedgeGrab::Center }
}

/// The width (degrees each way, within `limits`) that puts an edge of a
/// range centred on `center` at `hue`.
#[must_use]
pub fn wedge_width(hue: f32, center: f32, limits: [f32; 2]) -> f32 {
    hue_diff(hue, center).abs().clamp(limits[0], limits[1])
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact primaries
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn hsv_gives_the_primaries_and_greys() {
        assert_eq!(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]);
        assert_eq!(hsv(120.0, 1.0, 1.0), [0.0, 1.0, 0.0]);
        assert_eq!(hsv(240.0, 1.0, 0.5), [0.0, 0.0, 0.5]);
        assert_eq!(hsv(60.0, 1.0, 1.0), [1.0, 1.0, 0.0]);
        assert_eq!(hsv(300.0, 0.0, 0.25), [0.25, 0.25, 0.25]);
        let wrapped = hsv(-120.0, 1.0, 1.0);
        assert_eq!(wrapped, hsv(240.0, 1.0, 1.0));
    }

    #[test]
    fn wheel_positions_and_hues_agree() {
        // Red on the right, green up-left (y points down), blue down-left.
        let red = wheel_offset(0.0, 1.0);
        assert!(close(red[0], 1.0) && close(red[1], 0.0));
        let green = wheel_offset(120.0, 1.0);
        assert!(green[0] < 0.0 && green[1] < 0.0, "{green:?}");
        let blue = wheel_offset(240.0, 0.5);
        assert!(blue[0] < 0.0 && blue[1] > 0.0, "{blue:?}");
        for hue in [0.0, 37.0, 120.0, 200.0, 359.0] {
            for amount in [0.1, 0.5, 1.0] {
                let (h, a) = wheel_hue_amount(wheel_offset(hue, amount));
                assert!(hue_diff(h, hue).abs() < 1e-3 && close(a, amount), "{hue} {amount}: {h} {a}");
            }
        }
        assert!(close(wheel_hue_amount([3.0, 0.0]).1, 1.0), "held at the rim");
    }

    #[test]
    fn arcs_map_values_to_angles_and_back() {
        let range = [-100.0, 100.0];
        assert!(close(Arc::RIGHT.angle(-100.0, range), -50.0));
        assert!(close(Arc::RIGHT.angle(100.0, range), 50.0));
        assert!(close(Arc::LEFT.angle(100.0, range), 130.0), "the left arc's top");
        for arc in [Arc::RIGHT, Arc::LEFT] {
            for v in [-100.0, -40.0, 0.0, 65.0, 100.0] {
                let at = direction(arc.angle(v, range));
                assert!((arc.value_at(at, range) - v).abs() < 1e-2, "{arc:?} {v}");
                assert!(arc.covers(at, 1e-3));
            }
        }
        // Past the ends the value holds; the far side is not on the arc.
        assert!(close(Arc::RIGHT.value_at(direction(80.0), range), 100.0));
        assert!(close(Arc::RIGHT.value_at(direction(-80.0), range), -100.0));
        assert!(!Arc::RIGHT.covers(direction(180.0), 10.0));
        assert!(Arc::LEFT.covers(direction(180.0), 0.0));
    }

    #[test]
    fn wedges_are_turned_from_inside_and_widened_from_their_edges() {
        assert_eq!(wedge_grab(120.0, 120.0, 30.0, 6.0), WedgeGrab::Center);
        assert_eq!(wedge_grab(148.0, 120.0, 30.0, 6.0), WedgeGrab::Width);
        assert_eq!(wedge_grab(93.0, 120.0, 30.0, 6.0), WedgeGrab::Width);
        assert_eq!(wedge_grab(355.0, 5.0, 10.0, 6.0), WedgeGrab::Width, "across red");
        assert!(close(wedge_width(100.0, 120.0, [5.0, 90.0]), 20.0));
        assert!(close(wedge_width(122.0, 120.0, [5.0, 90.0]), 5.0));
        assert!(close(wedge_width(300.0, 120.0, [5.0, 90.0]), 90.0));
        assert!(close(hue_diff(10.0, 350.0), 20.0) && close(hue_diff(350.0, 10.0), -20.0));
    }
}
