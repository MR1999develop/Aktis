//! Parts of a person: hair, face skin, body skin, clothes, accessories from
//! the MediaPipe multiclass segmenter; eyebrows, eyes, irises, lips and teeth
//! from the face mesh.
//!
//! Each person is looked at three times: the whole body in one square, the
//! head in a second square at a much higher scale (hair and face need the
//! detail), and the face, levelled, for the mesh. Everything is then
//! multiplied by the person's own mask, so a part never spills onto the
//! person next to them.

use crate::face::{self, Face, MESH_POINTS, outline};
use crate::inference::{AiError, Model, output};
use crate::prep::Planes;
use crate::region::Plane;
use crate::warp::{Square, nhwc, sample};
use core_model::BodyPart;

/// Side of the segmenter's input.
const SEG_SIDE: usize = 256;
/// Classes of the segmenter, in its output order (0 is the background).
const HAIR: usize = 1;
const BODY_SKIN: usize = 2;
const FACE_SKIN: usize = 3;
const CLOTHES: usize = 4;
const OTHERS: usize = 5;
const CLASSES: usize = 6;

/// Class probabilities of one square of the photo.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClassMaps {
    pub square: Square,
    /// One `SEG_SIDE × SEG_SIDE` plane per class, summing to 1.
    pub probs: Vec<Vec<f32>>,
}

/// What was learnt about one person, in pixels of the working image.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PersonAnalysis {
    pub body: ClassMaps,
    pub head: Option<ClassMaps>,
    pub mesh: Option<Vec<(f32, f32)>>,
}

/// Runs the segmenter on square `sq`.
fn classes(segmenter: &mut dyn Model, image: &Planes, sq: Square) -> Result<ClassMaps, AiError> {
    let input = nhwc(image, sq, SEG_SIDE, 1.0, 0.0);
    let outputs = segmenter.run(vec![("input_29", input)])?;
    let logits = output(&outputs, "Identity", 0)?;
    let n = SEG_SIDE * SEG_SIDE;
    if logits.data.len() != n * CLASSES {
        return Err(AiError::UnexpectedOutput(format!("segmenter output of shape {:?}", logits.shape)));
    }
    let mut probs = vec![vec![0.0_f32; n]; CLASSES];
    for (i, px) in logits.data.chunks_exact(CLASSES).enumerate() {
        let max = px.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let e: Vec<f32> = px.iter().map(|v| (v - max).exp()).collect();
        let sum: f32 = e.iter().sum();
        for c in 0..CLASSES {
            probs[c][i] = e[c] / sum;
        }
    }
    Ok(ClassMaps { square: sq, probs })
}

/// Looks at the person inside `(x0, y0, x1, y1)` (pixels of `image`).
pub(crate) fn analyse(
    segmenter: &mut dyn Model,
    faces: &mut dyn Model,
    mesher: &mut dyn Model,
    image: &Planes,
    person: (f32, f32, f32, f32),
) -> Result<PersonAnalysis, AiError> {
    let (x0, y0, x1, y1) = person;
    let (pw, ph) = (x1 - x0, y1 - y0);
    let cx = (x0 + x1) / 2.0;
    let body = classes(segmenter, image, Square::upright(cx, (y0 + y1) / 2.0, pw.max(ph) * 1.05))?;

    // The head is at the top of the person: look for a face there first,
    // then in the whole person.
    let top = Square::upright(cx, y0 + pw.min(ph) * 0.45, pw.min(ph));
    let whole = Square::upright(cx, (y0 + y1) / 2.0, pw.max(ph));
    let inside = |f: &Face| f.cx >= x0 && f.cx <= x1 && f.cy >= y0 && f.cy <= y1;
    let mut face = face::detect(faces, image, top)?.into_iter().find(|f| inside(f));
    if face.is_none() {
        face = face::detect(faces, image, whole)?.into_iter().find(|f| inside(f));
    }
    let Some(face) = face else { return Ok(PersonAnalysis { body, head: None, mesh: None }) };

    let size = face.w.max(face.h);
    let head = classes(segmenter, image, Square::upright(face.cx, face.cy - 0.15 * size, 2.4 * size))?;
    let mesh = face::mesh(mesher, image, face::mesh_square(&face))?;
    Ok(PersonAnalysis { body, head: Some(head), mesh: Some(mesh) })
}

/// Value of class `class` at image pixel `(x, y)` of a square's maps; `None` outside it.
fn class_at(maps: &ClassMaps, class: usize, x: f32, y: f32, k: f32) -> Option<(f32, f32)> {
    let sq = maps.square.scaled(k);
    let (u, v) = sq.to_square(x, y);
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let n = SEG_SIDE as f32;
    let value = sample(&maps.probs[class], SEG_SIDE, SEG_SIDE, u * n, v * n);
    // Weight falling to 0 at the square's border, so two squares blend without a seam.
    let edge = u.min(1.0 - u).min(v).min(1.0 - v);
    Some((value, (edge / 0.08).min(1.0)))
}

/// Coverage (0–1) of a closed polygon at each pixel of a `w × h` plane, 4 × 4 samples per pixel.
fn fill_polygon(plane: &mut Plane, poly: &[(f32, f32)]) {
    if poly.len() < 3 {
        return;
    }
    let (minx, maxx) = poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (miny, maxy) = poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let clamp = |v: f32, n: usize| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the plane
        let i = v.max(0.0).min(n as f32) as usize;
        i
    };
    let inside = |x: f32, y: f32| {
        let mut odd = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[j]);
            if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
                odd = !odd;
            }
            j = i;
        }
        odd
    };
    for y in clamp(miny.floor(), plane.h)..clamp(maxy.ceil() + 1.0, plane.h) {
        for x in clamp(minx.floor(), plane.w)..clamp(maxx.ceil() + 1.0, plane.w) {
            let mut hits = 0_u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                        hits += 1;
                    }
                }
            }
            let v = &mut plane.data[y * plane.w + x];
            *v = v.max(hits as f32 / 16.0);
        }
    }
}

/// The iris of one eye as a polygon (a circle through its rim points).
fn iris_polygon(mesh: &[(f32, f32)], (centre, rim): (usize, [usize; 4])) -> Vec<(f32, f32)> {
    let c = mesh[centre];
    let r = rim.iter().map(|i| ((mesh[*i].0 - c.0).powi(2) + (mesh[*i].1 - c.1).powi(2)).sqrt()).sum::<f32>() / 4.0;
    (0..24)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 24.0;
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// Facial features drawn from the mesh at raster scale `k`.
fn feature(mesh: &[(f32, f32)], part: BodyPart, w: usize, h: usize, k: f32) -> Plane {
    let pts = |idx: &[usize]| -> Vec<(f32, f32)> { idx.iter().map(|i| (mesh[*i].0 * k, mesh[*i].1 * k)).collect() };
    let fill = |lists: &[Vec<(f32, f32)>]| {
        let mut p = Plane::zeros(w, h);
        for l in lists {
            fill_polygon(&mut p, l);
        }
        p
    };
    let scaled = |v: Vec<(f32, f32)>| v.into_iter().map(|(x, y)| (x * k, y * k)).collect::<Vec<_>>();
    let minus =
        |a: Plane, b: &Plane| Plane { w, h, data: a.data.iter().zip(&b.data).map(|(x, y)| x * (1.0 - y)).collect() };
    let times = |a: Plane, b: &Plane| Plane { w, h, data: a.data.iter().zip(&b.data).map(|(x, y)| x * y).collect() };
    let eyes = || fill(&[pts(outline::RIGHT_EYE), pts(outline::LEFT_EYE)]);
    let irises =
        || fill(&[scaled(iris_polygon(mesh, outline::RIGHT_IRIS)), scaled(iris_polygon(mesh, outline::LEFT_IRIS))]);
    match part {
        BodyPart::Eyebrows => fill(&[pts(outline::RIGHT_BROW), pts(outline::LEFT_BROW)]),
        BodyPart::Lips => minus(fill(&[pts(outline::LIPS_OUTER)]), &fill(&[pts(outline::LIPS_INNER)])),
        BodyPart::Teeth => fill(&[pts(outline::LIPS_INNER)]),
        BodyPart::Irises => times(irises(), &eyes()),
        BodyPart::EyeSclera => minus(eyes(), &irises()),
        _ => Plane::zeros(w, h),
    }
}

/// The values of `part` on a `w × h` raster of the upright photo (raster
/// pixel = working pixel × `k`), multiplied by the person's mask.
pub(crate) fn part_plane(a: &PersonAnalysis, part: BodyPart, person: &Plane, k: f32) -> Plane {
    let (w, h) = (person.w, person.h);
    let class = match part {
        BodyPart::Hair => Some(HAIR),
        BodyPart::FaceSkin => Some(FACE_SKIN),
        BodyPart::BodySkin => Some(BODY_SKIN),
        BodyPart::Clothes => Some(CLOTHES),
        BodyPart::Accessories => Some(OTHERS),
        _ => None,
    };
    let mut out = match (class, &a.mesh) {
        (Some(c), _) => {
            let mut p = Plane::zeros(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let body = class_at(&a.body, c, fx, fy, k).map_or(0.0, |(v, _)| v);
                    let v = match a.head.as_ref().and_then(|hd| class_at(hd, c, fx, fy, k)) {
                        Some((head, wgt)) => wgt * head + (1.0 - wgt) * body,
                        None => body,
                    };
                    p.data[y * w + x] = v;
                }
            }
            p
        }
        (None, Some(mesh)) if mesh.len() == MESH_POINTS => feature(mesh, part, w, h, k),
        (None, _) => Plane::zeros(w, h),
    };
    // Face skin is the face without its features.
    if part == BodyPart::FaceSkin
        && let Some(mesh) = a.mesh.as_ref().filter(|m| m.len() == MESH_POINTS)
    {
        for f in [BodyPart::Eyebrows, BodyPart::EyeSclera, BodyPart::Irises, BodyPart::Lips, BodyPart::Teeth] {
            let feat = feature(mesh, f, w, h, k);
            for (v, m) in out.data.iter_mut().zip(&feat.data) {
                *v *= 1.0 - m;
            }
        }
    }
    for (v, m) in out.data.iter_mut().zip(&person.data) {
        *v *= m;
    }
    out
}

/// Whether a part needs the face mesh (and so a visible face).
pub(crate) fn needs_face(part: BodyPart) -> bool {
    !matches!(
        part,
        BodyPart::Hair | BodyPart::FaceSkin | BodyPart::BodySkin | BodyPart::Clothes | BodyPart::Accessories
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_polygon_fills_its_pixels_and_half_covers_its_edges() {
        let mut p = Plane::zeros(10, 10);
        fill_polygon(&mut p, &[(2.0, 2.0), (6.5, 2.0), (6.5, 6.0), (2.0, 6.0)]);
        assert!((p.data[3 * 10 + 3] - 1.0).abs() < 1e-6);
        assert!((p.data[3 * 10 + 6] - 0.5).abs() < 1e-6, "{}", p.data[36]);
        assert!(p.data[8 * 10 + 8].abs() < f32::EPSILON);
    }

    #[test]
    fn class_maps_blend_head_over_body_inside_the_head_square() {
        let n = SEG_SIDE * SEG_SIDE;
        let maps = |sq: Square, v: f32| {
            let mut probs = vec![vec![0.0; n]; CLASSES];
            probs[HAIR] = vec![v; n];
            ClassMaps { square: sq, probs }
        };
        let a = PersonAnalysis {
            body: maps(Square::upright(50.0, 50.0, 100.0), 0.2),
            head: Some(maps(Square::upright(50.0, 25.0, 40.0), 1.0)),
            mesh: None,
        };
        let person = Plane { w: 100, h: 100, data: vec![1.0; 100 * 100] };
        let hair = part_plane(&a, BodyPart::Hair, &person, 1.0);
        assert!((hair.data[25 * 100 + 50] - 1.0).abs() < 1e-5, "head square wins in its middle");
        assert!((hair.data[80 * 100 + 50] - 0.2).abs() < 1e-5, "body below it");
        let masked = part_plane(&a, BodyPart::Hair, &Plane::zeros(100, 100), 1.0);
        assert!(masked.data.iter().all(|v| *v == 0.0), "nothing outside the person");
    }

    #[test]
    fn features_need_the_mesh_and_skin_parts_do_not() {
        assert!(needs_face(BodyPart::Lips) && needs_face(BodyPart::Irises));
        assert!(!needs_face(BodyPart::Hair) && !needs_face(BodyPart::FaceSkin));
    }
}
