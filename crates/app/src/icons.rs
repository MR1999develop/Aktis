//! Small symbols drawn with shapes instead of font glyphs: egui's default
//! font has no star, triangle or bullet, and a missing glyph shows as a box.

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// A five-pointed star of outer radius `r` centred on `c`.
pub fn star(painter: &Painter, c: Pos2, r: f32, fill: Color32) {
    let point = |radius: f32, degrees: f32| {
        let a = degrees.to_radians();
        pos2(c.x + radius * a.cos(), c.y + radius * a.sin())
    };
    let outer: Vec<Pos2> = (0..5).map(|k| point(r, -90.0 + 72.0 * k as f32)).collect();
    let inner: Vec<Pos2> = (0..5).map(|k| point(r * 0.42, -54.0 + 72.0 * k as f32)).collect();
    // A star is not convex: a pentagon plus one triangle per point.
    painter.add(Shape::convex_polygon(inner.clone(), fill, Stroke::NONE));
    for k in 0..5 {
        let before = inner[(k + 4) % 5];
        painter.add(Shape::convex_polygon(vec![outer[k], inner[k], before], fill, Stroke::NONE));
    }
}

/// A small triangle pointing right (closed folder) or down (open folder).
pub fn disclosure(painter: &Painter, rect: Rect, open: bool, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.3;
    let points = if open {
        vec![pos2(c.x - s, c.y - s * 0.6), pos2(c.x + s, c.y - s * 0.6), pos2(c.x, c.y + s * 0.8)]
    } else {
        vec![pos2(c.x - s * 0.6, c.y - s), pos2(c.x + s * 0.8, c.y), pos2(c.x - s * 0.6, c.y + s)]
    };
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// Six dots in two columns: "this row can be dragged".
pub fn grip(painter: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    for dx in [-2.5, 2.5] {
        for dy in [-5.0, 0.0, 5.0] {
            painter.circle_filled(c + vec2(dx, dy), 1.3, color);
        }
    }
}

/// A filled dot.
pub fn dot(painter: &Painter, c: Pos2, r: f32, color: Color32) {
    painter.circle_filled(c, r, color);
}

/// An eyedropper (colour picker): a slanted pipette with its bulb at the top right.
pub fn eyedropper(painter: &Painter, rect: Rect, color: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center() - vec2(s / 2.0, s / 2.0);
    let at = |x: f32, y: f32| o + vec2(x * s, y * s);
    // Glass tube from the tip (bottom left) up to the collar.
    painter.line_segment([at(0.08, 0.92), at(0.62, 0.38)], Stroke::new(s * 0.14, color));
    // Collar across the tube.
    painter.line_segment([at(0.48, 0.28), at(0.72, 0.52)], Stroke::new(s * 0.12, color));
    // Rubber bulb.
    painter.circle_filled(at(0.76, 0.24), s * 0.2, color);
}
