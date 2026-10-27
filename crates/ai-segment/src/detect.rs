//! "Select people" (and other kinds of object): the YOLOX-S detector.
//!
//! The detector finds each person, car, dog … with a box; each box then
//! becomes the rectangle prompt of a Segment Anything component, so every
//! person is a separate, editable part of the mask.
//!
//! Input: the image scaled to fit 640 × 640, placed top-left on grey (114),
//! BGR order, values 0–255. Output: 8400 rows (three grids of strides 8, 16
//! and 32) of box centre and size relative to the grid cell, objectness and
//! 80 class scores, already through a sigmoid.

use core_model::{ImageF32, NormPoint, NormRect};

use crate::inference::{AiError, Model, Tensor, output};
use crate::prep::{Planes, srgb_planes};

/// Side of the square the detector works on.
pub(crate) const SIDE: usize = 640;
/// Grey of the padding, as in training.
const PAD: f32 = 114.0;
/// Grid strides of the three output levels.
const STRIDES: [usize; 3] = [8, 16, 32];
/// Boxes overlapping more than this (intersection over union) are duplicates.
const NMS_IOU: f32 = 0.45;

/// The 80 COCO classes, in the model's order.
pub const COCO_CLASSES: [&str; 80] = [
    "person",
    "bicycle",
    "car",
    "motorcycle",
    "airplane",
    "bus",
    "train",
    "truck",
    "boat",
    "traffic light",
    "fire hydrant",
    "stop sign",
    "parking meter",
    "bench",
    "bird",
    "cat",
    "dog",
    "horse",
    "sheep",
    "cow",
    "elephant",
    "bear",
    "zebra",
    "giraffe",
    "backpack",
    "umbrella",
    "handbag",
    "tie",
    "suitcase",
    "frisbee",
    "skis",
    "snowboard",
    "sports ball",
    "kite",
    "baseball bat",
    "baseball glove",
    "skateboard",
    "surfboard",
    "tennis racket",
    "bottle",
    "wine glass",
    "cup",
    "fork",
    "knife",
    "spoon",
    "bowl",
    "banana",
    "apple",
    "sandwich",
    "orange",
    "broccoli",
    "carrot",
    "hot dog",
    "pizza",
    "donut",
    "cake",
    "chair",
    "couch",
    "potted plant",
    "bed",
    "dining table",
    "toilet",
    "tv",
    "laptop",
    "mouse",
    "remote",
    "keyboard",
    "cell phone",
    "microwave",
    "oven",
    "toaster",
    "sink",
    "refrigerator",
    "book",
    "clock",
    "vase",
    "scissors",
    "teddy bear",
    "hair drier",
    "toothbrush",
];

/// Index of the "person" class.
pub const PERSON: usize = 0;

/// The index of a class name ("person", "dog"), ignoring case.
#[must_use]
pub fn class_index(name: &str) -> Option<usize> {
    COCO_CLASSES.iter().position(|c| c.eq_ignore_ascii_case(name.trim()))
}

/// Something the detector found.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    /// Index in [`COCO_CLASSES`].
    pub class: usize,
    /// Confidence, 0–1.
    pub score: f32,
    /// Its box, in normalised coordinates of the image passed in.
    pub rect: NormRect,
}

impl Detection {
    /// The class name ("person").
    #[must_use]
    pub fn class_name(&self) -> &'static str {
        COCO_CLASSES.get(self.class).copied().unwrap_or("object")
    }
}

/// The detector's input, and the scale from image pixels to input pixels.
pub(crate) fn letterbox(image: &ImageF32) -> (Tensor, f32) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let ratio = (SIDE as f32 / w as f32).min(SIDE as f32 / h as f32);
    let scaled = |v: usize| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≤ SIDE
        let s = ((v as f32 * ratio).round() as usize).clamp(1, SIDE);
        s
    };
    let (sw, sh) = (scaled(w), scaled(h));
    let planes: Planes = srgb_planes(image, sw, sh);
    let mut data = vec![PAD; 3 * SIDE * SIDE];
    // BGR order: output channel c takes RGB plane 2 − c.
    for c in 0..3 {
        let src = &planes.rgb[2 - c];
        for y in 0..sh {
            let row = &src[y * sw..(y + 1) * sw];
            let dst = &mut data[c * SIDE * SIDE + y * SIDE..c * SIDE * SIDE + y * SIDE + sw];
            for (d, s) in dst.iter_mut().zip(row) {
                *d = s * 255.0;
            }
        }
    }
    (Tensor { shape: vec![1, 3, SIDE, SIDE], data }, ratio)
}

/// Boxes from the raw output, in pixels of the input square (x0, y0, x1, y1),
/// with class and score, for classes accepted by `wanted`.
fn raw_boxes(
    out: &Tensor,
    min_score: f32,
    wanted: &dyn Fn(usize) -> bool,
) -> Result<Vec<([f32; 4], usize, f32)>, AiError> {
    let (rows, cols) = match out.shape.as_slice() {
        [1, rows, cols] if rows * cols == out.data.len() && *cols > 5 => (*rows, *cols),
        other => return Err(AiError::UnexpectedOutput(format!("detections of shape {other:?}"))),
    };
    let expected: usize = STRIDES.iter().map(|s| (SIDE / s) * (SIDE / s)).sum();
    if rows != expected {
        return Err(AiError::UnexpectedOutput(format!("{rows} detection rows, expected {expected}")));
    }
    let mut found = Vec::new();
    let mut row = 0;
    for stride in STRIDES {
        let n = SIDE / stride;
        for gy in 0..n {
            for gx in 0..n {
                let r = &out.data[row * cols..(row + 1) * cols];
                row += 1;
                let objectness = r[4];
                if objectness < min_score {
                    continue;
                }
                let Some((class, class_score)) =
                    r[5..].iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, s)| (i, *s))
                else {
                    continue;
                };
                let score = objectness * class_score;
                if score < min_score || !wanted(class) {
                    continue;
                }
                let s = stride as f32;
                let cx = (r[0] + gx as f32) * s;
                let cy = (r[1] + gy as f32) * s;
                let bw = r[2].exp() * s;
                let bh = r[3].exp() * s;
                found.push(([cx - bw / 2.0, cy - bh / 2.0, cx + bw / 2.0, cy + bh / 2.0], class, score));
            }
        }
    }
    Ok(found)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let iw = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let ih = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// Keeps the best box of each group of overlapping boxes of the same class.
fn non_max_suppression(mut boxes: Vec<([f32; 4], usize, f32)>) -> Vec<([f32; 4], usize, f32)> {
    boxes.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut kept: Vec<([f32; 4], usize, f32)> = Vec::new();
    for b in boxes {
        if kept.iter().all(|k| k.1 != b.1 || iou(&k.0, &b.0) <= NMS_IOU) {
            kept.push(b);
        }
    }
    kept
}

/// Detections from the raw output, for an image of `w × h` scaled by `ratio`.
pub(crate) fn decode(
    out: &Tensor,
    ratio: f32,
    w: usize,
    h: usize,
    min_score: f32,
    wanted: &dyn Fn(usize) -> bool,
) -> Result<Vec<Detection>, AiError> {
    let boxes = non_max_suppression(raw_boxes(out, min_score, wanted)?);
    let (fw, fh) = (w as f32 * ratio, h as f32 * ratio);
    Ok(boxes
        .into_iter()
        .filter_map(|(b, class, score)| {
            let a = NormPoint::new(b[0] / fw, b[1] / fh)?;
            let z = NormPoint::new(b[2] / fw, b[3] / fh)?;
            let rect = NormRect::new(a, z);
            // Boxes squeezed to nothing by the clamp lie outside the image.
            let size = (rect.max().x() - rect.min().x()) * (rect.max().y() - rect.min().y());
            (size > 0.0).then_some(Detection { class, score, rect })
        })
        .collect())
}

/// Runs the detector on an upright image.
pub(crate) fn detect(
    model: &mut dyn Model,
    image: &ImageF32,
    min_score: f32,
    wanted: &dyn Fn(usize) -> bool,
) -> Result<Vec<Detection>, AiError> {
    let (input, ratio) = letterbox(image);
    let outputs = model.run(vec![("images", input)])?;
    decode(output(&outputs, "output", 0)?, ratio, image.width() as usize, image.height() as usize, min_score, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::WorkingSpace;

    const ROWS: usize = 8400;
    const COLS: usize = 85;

    /// An all-quiet output with one confident person at grid cell (gx, gy) of stride 8.
    fn output_with_person(gx: usize, gy: usize, w_cells: f32, h_cells: f32) -> Tensor {
        let mut data = vec![0.0; ROWS * COLS];
        let row = gy * (SIDE / 8) + gx;
        let r = &mut data[row * COLS..(row + 1) * COLS];
        r[0] = 0.5;
        r[1] = 0.5;
        r[2] = w_cells.ln();
        r[3] = h_cells.ln();
        r[4] = 0.9;
        r[5 + PERSON] = 0.95;
        Tensor { shape: vec![1, ROWS, COLS], data }
    }

    #[test]
    fn class_names_are_found_case_insensitively() {
        assert_eq!(class_index("Person"), Some(PERSON));
        assert_eq!(class_index(" dog "), Some(16));
        assert_eq!(class_index("unicorn"), None);
        assert_eq!(COCO_CLASSES.len(), 80);
    }

    #[test]
    fn a_box_is_decoded_from_its_grid_cell_and_mapped_back_through_the_letterbox() {
        // Centre of cell (40, 20) at stride 8 is (324, 164); 16×32 cells is 128×256 px.
        let out = output_with_person(40, 20, 16.0, 32.0);
        // An image of 1280×640 was halved to 640×320.
        let d = decode(&out, 0.5, 1280, 640, 0.3, &|_| true).expect("decodes");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].class_name(), "person");
        assert!((d[0].score - 0.855).abs() < 1e-4);
        let (min, max) = (d[0].rect.min(), d[0].rect.max());
        assert!((min.x() - 260.0 / 640.0).abs() < 1e-4 && (max.x() - 388.0 / 640.0).abs() < 1e-4);
        assert!((min.y() - 36.0 / 320.0).abs() < 1e-4 && (max.y() - 292.0 / 320.0).abs() < 1e-4);
    }

    #[test]
    fn unwanted_classes_and_weak_boxes_are_dropped() {
        let out = output_with_person(10, 10, 4.0, 4.0);
        assert!(decode(&out, 1.0, 640, 640, 0.3, &|c| c != PERSON).expect("decodes").is_empty());
        assert!(decode(&out, 1.0, 640, 640, 0.9, &|_| true).expect("decodes").is_empty());
    }

    #[test]
    fn overlapping_boxes_of_one_class_keep_the_best() {
        let a = ([0.0, 0.0, 10.0, 10.0], PERSON, 0.9);
        let b = ([1.0, 1.0, 11.0, 11.0], PERSON, 0.8);
        let c = ([1.0, 1.0, 11.0, 11.0], 16, 0.7);
        let d = ([50.0, 50.0, 60.0, 60.0], PERSON, 0.6);
        let kept = non_max_suppression(vec![b, a, c, d]);
        assert_eq!(kept, vec![a, c, d]);
    }

    #[test]
    fn the_letterbox_keeps_the_aspect_and_pads_with_grey() {
        let img = ImageF32::from_pixels(64, 32, WorkingSpace::LinearSrgb, vec![[0.18, 0.18, 0.18, 1.0]; 64 * 32])
            .expect("valid image");
        let (t, ratio) = letterbox(&img);
        assert_eq!(t.shape, vec![1, 3, SIDE, SIDE]);
        assert!((ratio - 10.0).abs() < 1e-6);
        assert!((t.data[0] - 117.6).abs() < 1.0, "image area holds the picture: {}", t.data[0]);
        assert!((t.data[(SIDE - 1) * SIDE] - PAD).abs() < 1e-6, "below it is padding");
    }

    #[test]
    fn a_wrongly_shaped_output_is_an_error() {
        let bad = Tensor { shape: vec![1, 10, 85], data: vec![0.0; 850] };
        assert!(matches!(decode(&bad, 1.0, 10, 10, 0.3, &|_| true), Err(AiError::UnexpectedOutput(_))));
    }
}
