//! Faces: BlazeFace finds them, the face landmarker places 478 points on
//! each (eyelids, irises, eyebrows, lips).
//!
//! BlazeFace (short range) takes 128 × 128, values −1…1, and answers for 896
//! anchors: 16 numbers (box centre and size, six key points) and a score.
//! The landmarker takes the face upright in a 256 × 256 square, values 0…1,
//! and answers 478 points (x, y, z) in that square's pixels.

use crate::inference::{AiError, Model, Tensor, output};
use crate::prep::Planes;
use crate::warp::{Square, nhwc};

/// Side of the detector's input.
const DET_SIDE: usize = 128;
/// Side of the landmarker's input.
const MESH_SIDE: usize = 256;
/// Faces scoring less are ignored.
const MIN_SCORE: f32 = 0.5;
/// Overlapping detections (intersection over union) are merged.
const MERGE_IOU: f32 = 0.3;
/// The landmarker's square is this many times the detected face box.
const MESH_SCALE: f32 = 1.5;
/// Number of mesh points.
pub(crate) const MESH_POINTS: usize = 478;

/// A face found by the detector, in pixels of the image passed in.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Face {
    /// Box centre and size.
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// Right eye, left eye (as seen in the photo: the person's right is on the left).
    pub right_eye: (f32, f32),
    pub left_eye: (f32, f32),
    pub score: f32,
}

/// The anchors of the short-range model: a 16 × 16 grid with 2 anchors per
/// cell, then an 8 × 8 grid with 6, centres in 0–1.
fn anchors() -> Vec<(f32, f32)> {
    let mut v = Vec::with_capacity(896);
    for (grid, per_cell) in [(16_usize, 2_usize), (8, 6)] {
        for y in 0..grid {
            for x in 0..grid {
                for _ in 0..per_cell {
                    v.push(((x as f32 + 0.5) / grid as f32, (y as f32 + 0.5) / grid as f32));
                }
            }
        }
    }
    v
}

/// Faces in the square `sq` of `image`, in pixels of `image`, best first.
pub(crate) fn detect(model: &mut dyn Model, image: &Planes, sq: Square) -> Result<Vec<Face>, AiError> {
    let input = nhwc(image, sq, DET_SIDE, 2.0, -1.0);
    let outputs = model.run(vec![("input", input)])?;
    let boxes = output(&outputs, "regressors", 0)?;
    let scores = output(&outputs, "classificators", 1)?;
    let found = decode(boxes, scores)?;
    Ok(found
        .into_iter()
        .map(|f| {
            let to = |(u, v): (f32, f32)| sq.to_image(u, v);
            let (cx, cy) = to((f.cx, f.cy));
            Face {
                cx,
                cy,
                w: f.w * sq.side,
                h: f.h * sq.side,
                right_eye: to(f.right_eye),
                left_eye: to(f.left_eye),
                score: f.score,
            }
        })
        .collect())
}

/// Faces from the raw outputs, in 0–1 coordinates of the detector's input.
fn decode(boxes: &Tensor, scores: &Tensor) -> Result<Vec<Face>, AiError> {
    let a = anchors();
    if boxes.data.len() != a.len() * 16 || scores.data.len() != a.len() {
        return Err(AiError::UnexpectedOutput(format!("face boxes {:?}, scores {:?}", boxes.shape, scores.shape)));
    }
    let side = DET_SIDE as f32;
    let mut found: Vec<Face> = a
        .iter()
        .enumerate()
        .filter_map(|(i, (ax, ay))| {
            let score = crate::prep::sigmoid(scores.data[i].clamp(-100.0, 100.0));
            if score < MIN_SCORE {
                return None;
            }
            let r = &boxes.data[i * 16..(i + 1) * 16];
            let kp = |k: usize| (r[4 + 2 * k] / side + ax, r[5 + 2 * k] / side + ay);
            Some(Face {
                cx: r[0] / side + ax,
                cy: r[1] / side + ay,
                w: r[2] / side,
                h: r[3] / side,
                right_eye: kp(0),
                left_eye: kp(1),
                score,
            })
        })
        .collect();
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    // Weighted merge of overlapping detections, as MediaPipe does.
    let mut merged: Vec<Face> = Vec::new();
    while let Some(best) = found.first().cloned() {
        let (group, rest): (Vec<Face>, Vec<Face>) = found.into_iter().partition(|f| iou(f, &best) > MERGE_IOU);
        found = rest;
        let total: f32 = group.iter().map(|f| f.score).sum();
        let avg = |g: &dyn Fn(&Face) -> f32| group.iter().map(|f| g(f) * f.score).sum::<f32>() / total;
        merged.push(Face {
            cx: avg(&|f| f.cx),
            cy: avg(&|f| f.cy),
            w: avg(&|f| f.w),
            h: avg(&|f| f.h),
            right_eye: (avg(&|f| f.right_eye.0), avg(&|f| f.right_eye.1)),
            left_eye: (avg(&|f| f.left_eye.0), avg(&|f| f.left_eye.1)),
            score: best.score,
        });
    }
    Ok(merged)
}

fn iou(a: &Face, b: &Face) -> f32 {
    let ix = ((a.cx + a.w / 2.0).min(b.cx + b.w / 2.0) - (a.cx - a.w / 2.0).max(b.cx - b.w / 2.0)).max(0.0);
    let iy = ((a.cy + a.h / 2.0).min(b.cy + b.h / 2.0) - (a.cy - a.h / 2.0).max(b.cy - b.h / 2.0)).max(0.0);
    let inter = ix * iy;
    let union = a.w * a.h + b.w * b.h - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// The square the landmarker looks at for `face`: centred on the face box,
/// rotated so the eyes are level.
pub(crate) fn mesh_square(face: &Face) -> Square {
    let angle = (face.left_eye.1 - face.right_eye.1).atan2(face.left_eye.0 - face.right_eye.0);
    Square { cx: face.cx, cy: face.cy, side: face.w.max(face.h) * MESH_SCALE, angle }
}

/// The 478 mesh points of the face in square `sq` of `image`, in pixels of `image`.
pub(crate) fn mesh(model: &mut dyn Model, image: &Planes, sq: Square) -> Result<Vec<(f32, f32)>, AiError> {
    let input = nhwc(image, sq, MESH_SIDE, 1.0, 0.0);
    let outputs = model.run(vec![("input_12", input)])?;
    let pts = outputs
        .iter()
        .map(|(_, t)| t)
        .find(|t| t.data.len() == MESH_POINTS * 3)
        .ok_or_else(|| AiError::UnexpectedOutput("no 478-point face mesh in the output".to_owned()))?;
    let side = MESH_SIDE as f32;
    Ok(pts.data.chunks_exact(3).map(|p| sq.to_image(p[0] / side, p[1] / side)).collect())
}

/// Mesh point indices of each facial feature's outline, in order around it.
pub(crate) mod outline {
    /// Outer edge of the lips.
    pub const LIPS_OUTER: &[usize] =
        &[61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146];
    /// Inner edge of the lips: the mouth opening.
    pub const LIPS_INNER: &[usize] =
        &[78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95];
    /// Eyelid opening of the eye on the photo's left.
    pub const RIGHT_EYE: &[usize] = &[33, 246, 161, 160, 159, 158, 157, 173, 133, 155, 154, 153, 145, 144, 163, 7];
    /// Eyelid opening of the eye on the photo's right.
    pub const LEFT_EYE: &[usize] = &[263, 466, 388, 387, 386, 385, 384, 398, 362, 382, 381, 380, 374, 373, 390, 249];
    /// Eyebrow on the photo's left (upper edge, then lower edge back).
    pub const RIGHT_BROW: &[usize] = &[70, 63, 105, 66, 107, 55, 65, 52, 53, 46];
    /// Eyebrow on the photo's right.
    pub const LEFT_BROW: &[usize] = &[300, 293, 334, 296, 336, 285, 295, 282, 283, 276];
    /// Iris centre and four points on its rim, eye on the photo's left.
    pub const RIGHT_IRIS: (usize, [usize; 4]) = (468, [469, 470, 471, 472]);
    /// Iris centre and rim, eye on the photo's right.
    pub const LEFT_IRIS: (usize, [usize; 4]) = (473, [474, 475, 476, 477]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_short_range_model_has_896_anchors_in_two_grids() {
        let a = anchors();
        assert_eq!(a.len(), 896);
        assert_eq!(a[0], (1.0 / 32.0, 1.0 / 32.0));
        assert_eq!(a[512], (1.0 / 16.0, 1.0 / 16.0));
    }

    #[test]
    fn a_confident_anchor_becomes_a_face_and_duplicates_merge() {
        let mut boxes = vec![0.0; 896 * 16];
        let mut scores = vec![-10.0; 896];
        for i in [0_usize, 1] {
            boxes[i * 16 + 2] = 64.0; // width: half the input
            boxes[i * 16 + 3] = 64.0;
            scores[i] = 5.0;
        }
        let faces =
            decode(&Tensor { shape: vec![1, 896, 16], data: boxes }, &Tensor { shape: vec![1, 896, 1], data: scores })
                .expect("decodes");
        assert_eq!(faces.len(), 1, "two anchors on the same face merge");
        assert!((faces[0].w - 0.5).abs() < 1e-6 && (faces[0].cx - 1.0 / 32.0).abs() < 1e-6);
    }

    #[test]
    fn the_mesh_square_levels_the_eyes() {
        let face =
            Face { cx: 0.0, cy: 0.0, w: 10.0, h: 12.0, right_eye: (-2.0, 0.0), left_eye: (2.0, 2.0), score: 1.0 };
        let sq = mesh_square(&face);
        assert!((sq.side - 18.0).abs() < 1e-5);
        let (_, v0) = sq.to_square(-2.0, 0.0);
        let (_, v1) = sq.to_square(2.0, 2.0);
        assert!((v0 - v1).abs() < 1e-5, "eyes on one row of the square");
    }

    #[test]
    fn outlines_use_valid_mesh_points() {
        for list in [outline::LIPS_OUTER, outline::LIPS_INNER, outline::RIGHT_EYE, outline::LEFT_EYE] {
            assert!(list.iter().all(|i| *i < MESH_POINTS));
        }
        assert!(outline::LEFT_IRIS.1.iter().all(|i| *i < MESH_POINTS));
    }
}
