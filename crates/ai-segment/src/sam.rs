//! Objects and people from clicks and rectangles: Segment Anything 2.1.
//!
//! The model is split in two. The encoder looks at the image once (the slow
//! part, about a second on a CPU) and gives an embedding; the decoder turns
//! the embedding and a prompt into masks in a few milliseconds. So every
//! click after the first is immediate, and the embedding is kept per image.
//!
//! The encoder takes the image squeezed to 1024 × 1024 (no padding),
//! ImageNet-normalised. Prompt coordinates are in that square. A rectangle
//! is two points with labels 2 (top-left) and 3 (bottom-right); clicks are
//! 1 (include) and 0 (exclude); without a rectangle a padding point with
//! label −1 is added, as SAM was trained. The decoder returns four masks
//! (logits) and a quality score each: the first is the single-mask answer,
//! the others are alternatives for an ambiguous single click.

use core_model::AiTarget;

use crate::inference::{AiError, Model, Tensor, output};
use crate::prep::{IMAGENET_MEAN, IMAGENET_STD, normalized_nchw, srgb_planes};
use crate::region::Plane;

/// Side of the square the encoder works on.
pub(crate) const SIDE: usize = 1024;
/// Side of the decoder's previous-mask input.
const MASK_INPUT_SIDE: usize = 256;

/// What the encoder saw: everything the decoder needs besides the prompt.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Embedding {
    image_embed: Tensor,
    high_res_0: Tensor,
    high_res_1: Tensor,
}

/// Runs the encoder on an upright image.
pub(crate) fn encode(encoder: &mut dyn Model, image: &core_model::ImageF32) -> Result<Embedding, AiError> {
    let planes = srgb_planes(image, SIDE, SIDE);
    let input = normalized_nchw(&planes, IMAGENET_MEAN, IMAGENET_STD);
    let outputs = encoder.run(vec![("image", input)])?;
    Ok(Embedding {
        high_res_0: output(&outputs, "high_res_0", 0)?.clone(),
        high_res_1: output(&outputs, "high_res_1", 1)?.clone(),
        image_embed: output(&outputs, "image_embed", 2)?.clone(),
    })
}

/// Point coordinates (in the 1024 square) and labels for `target`, whose
/// coordinates are already upright; `None` for a target without a prompt.
pub(crate) fn prompt(target: &AiTarget) -> Option<(Vec<[f32; 2]>, Vec<f32>)> {
    let AiTarget::Object { points, rect } = target else { return None };
    if !target.is_complete() {
        return None;
    }
    let side = SIDE as f32;
    let mut coords = Vec::with_capacity(points.len() + 2);
    let mut labels = Vec::with_capacity(points.len() + 2);
    for p in points {
        coords.push([p.at.x() * side, p.at.y() * side]);
        labels.push(if p.include { 1.0 } else { 0.0 });
    }
    match rect {
        Some(r) => {
            coords.push([r.min().x() * side, r.min().y() * side]);
            labels.push(2.0);
            coords.push([r.max().x() * side, r.max().y() * side]);
            labels.push(3.0);
        }
        None => {
            coords.push([0.0, 0.0]);
            labels.push(-1.0);
        }
    }
    Some((coords, labels))
}

/// Whether the prompt is a single click: then the best of the alternative
/// masks is taken instead of the single-mask answer.
fn ambiguous(target: &AiTarget) -> bool {
    matches!(target, AiTarget::Object { points, rect: None } if points.len() == 1)
}

/// The decoder's inputs for an embedding and a prompt.
pub(crate) fn decoder_inputs(e: &Embedding, coords: &[[f32; 2]], labels: &[f32]) -> Vec<(&'static str, Tensor)> {
    let n = coords.len();
    vec![
        ("image_embed", e.image_embed.clone()),
        ("high_res_0", e.high_res_0.clone()),
        ("high_res_1", e.high_res_1.clone()),
        ("point_coords", Tensor { shape: vec![1, n, 2], data: coords.iter().flatten().copied().collect() }),
        ("point_labels", Tensor { shape: vec![1, n], data: labels.to_vec() }),
        ("mask_input", Tensor::filled(&[1, 1, MASK_INPUT_SIDE, MASK_INPUT_SIDE], 0.0)),
        ("has_mask_input", Tensor::filled(&[1], 0.0)),
    ]
}

/// Picks one of the decoder's masks: its logits over the encoded image
/// (256 × 256 for the reference export).
pub(crate) fn choose(masks_out: &Tensor, scores: &Tensor, best_of_alternatives: bool) -> Result<Plane, AiError> {
    let (k, mh, mw) = match masks_out.shape.as_slice() {
        [1, k, mh, mw] if k * mh * mw == masks_out.data.len() && *k > 0 && *mh > 0 && *mw > 0 => (*k, *mh, *mw),
        other => return Err(AiError::UnexpectedOutput(format!("masks of shape {other:?}"))),
    };
    let choice = if best_of_alternatives && k > 1 && scores.data.len() >= k {
        (1..k).max_by(|a, b| scores.data[*a].total_cmp(&scores.data[*b])).unwrap_or(0)
    } else {
        0
    };
    Ok(Plane { w: mw, h: mh, data: masks_out.data[choice * mh * mw..(choice + 1) * mh * mw].to_vec() })
}

/// Runs the decoder for an upright `target` (coordinates relative to the
/// encoded image); returns the chosen mask's logits.
pub(crate) fn logits(decoder: &mut dyn Model, embedding: &Embedding, target: &AiTarget) -> Result<Plane, AiError> {
    let (coords, labels) = prompt(target).ok_or(AiError::IncompletePrompt)?;
    let outputs = decoder.run(decoder_inputs(embedding, &coords, &labels))?;
    let masks_out = output(&outputs, "masks", 0)?;
    let scores = output(&outputs, "iou", 1)?;
    choose(masks_out, scores, ambiguous(target))
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact values set by the tests
mod tests {
    use super::*;
    use core_model::{NormPoint, NormRect, PromptPoint};

    fn p(x: f32, y: f32) -> NormPoint {
        NormPoint::new(x, y).expect("finite")
    }

    #[test]
    fn clicks_get_a_padding_point_and_rectangles_two_corner_points() {
        let click = AiTarget::Object { points: vec![PromptPoint { at: p(0.5, 0.25), include: true }], rect: None };
        let (c, l) = prompt(&click).expect("complete");
        assert_eq!(c, vec![[512.0, 256.0], [0.0, 0.0]]);
        assert_eq!(l, vec![1.0, -1.0]);

        let rect = NormRect::new(p(0.75, 0.5), p(0.25, 0.0));
        let both = AiTarget::Object { points: vec![PromptPoint { at: p(0.5, 0.5), include: false }], rect: Some(rect) };
        let (c, l) = prompt(&both).expect("complete");
        assert_eq!(c, vec![[512.0, 512.0], [256.0, 0.0], [768.0, 512.0]]);
        assert_eq!(l, vec![0.0, 2.0, 3.0]);

        assert_eq!(prompt(&AiTarget::Subject), None);
        let only_exclude =
            AiTarget::Object { points: vec![PromptPoint { at: p(0.1, 0.1), include: false }], rect: None };
        assert_eq!(prompt(&only_exclude), None);
    }

    fn four_masks() -> (Tensor, Tensor) {
        // Mask i is uniformly i − 1.5 (logits), on a 2×2 grid.
        let data = (0..4).flat_map(|i| [i as f32 - 1.5; 4]).collect();
        (Tensor { shape: vec![1, 4, 2, 2], data }, Tensor { shape: vec![1, 4], data: vec![0.9, 0.2, 0.8, 0.5] })
    }

    #[test]
    fn a_single_click_takes_the_best_alternative_otherwise_the_first_mask() {
        let (m, s) = four_masks();
        let best = choose(&m, &s, true).expect("decodes");
        assert_eq!((best.w, best.h, best.data[0]), (2, 2, 0.5), "mask 2 has the best alternative score");
        assert_eq!(choose(&m, &s, false).expect("decodes").data[0], -1.5);
    }

    #[test]
    fn decoder_inputs_have_the_reference_names_and_shapes() {
        let e = Embedding {
            image_embed: Tensor::filled(&[1, 256, 64, 64], 0.0),
            high_res_0: Tensor::filled(&[1, 32, 256, 256], 0.0),
            high_res_1: Tensor::filled(&[1, 64, 128, 128], 0.0),
        };
        let inputs = decoder_inputs(&e, &[[1.0, 2.0], [0.0, 0.0]], &[1.0, -1.0]);
        let shape = |name: &str| inputs.iter().find(|(n, _)| *n == name).map(|(_, t)| t.shape.clone());
        assert_eq!(shape("point_coords"), Some(vec![1, 2, 2]));
        assert_eq!(shape("point_labels"), Some(vec![1, 2]));
        assert_eq!(shape("mask_input"), Some(vec![1, 1, 256, 256]));
        assert_eq!(shape("has_mask_input"), Some(vec![1]));
        assert_eq!(inputs.len(), 7);
    }
}
