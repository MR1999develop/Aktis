//! "Select subject": BiRefNet, a salient-object model that needs no prompt.
//!
//! Input: the image squeezed to 1024 × 1024, ImageNet-normalised. Output: one
//! 1024 × 1024 map; some exports end in a sigmoid and some give logits, so
//! values outside [0, 1] are read as logits.

use core_model::MaskImage;

use crate::inference::{AiError, Model, Tensor, output};
use crate::prep::{IMAGENET_MEAN, IMAGENET_STD, Planes, normalized_nchw, sigmoid, srgb_planes};

/// Side of the square the model works on.
pub(crate) const SIDE: usize = 1024;
/// Input and output names of the reference export.
const INPUT: &str = "input_image";
const OUTPUT: &str = "output_image";

/// The model's input for `planes` (already `SIDE × SIDE`).
pub(crate) fn input(planes: &Planes) -> Tensor {
    normalized_nchw(planes, IMAGENET_MEAN, IMAGENET_STD)
}

/// The subject mask at `w × h` from the model's output map.
pub(crate) fn decode(map: &Tensor, w: usize, h: usize) -> Result<MaskImage, AiError> {
    let (mh, mw) = match map.shape.as_slice() {
        [.., mh, mw] if mh * mw == map.data.len() && *mh > 0 && *mw > 0 => (*mh, *mw),
        other => return Err(AiError::UnexpectedOutput(format!("subject map of shape {other:?}"))),
    };
    let logits = map.data.iter().any(|v| !(0.0..=1.0).contains(v));
    let probabilities: Vec<f32> =
        if logits { map.data.iter().map(|v| sigmoid(*v)).collect() } else { map.data.clone() };
    let values = masks::resize_plane(&probabilities, mw, mh, w, h);
    let (w, h) = (u32::try_from(w), u32::try_from(h));
    match (w, h) {
        (Ok(w), Ok(h)) => MaskImage::from_values(w, h, values).map_err(|e| AiError::Image(e.to_string())),
        _ => Err(AiError::Image("mask too large".to_owned())),
    }
}

/// Runs the whole pipeline on an upright image; the mask is `w × h`.
pub(crate) fn segment(
    model: &mut dyn Model,
    image: &core_model::ImageF32,
    w: usize,
    h: usize,
) -> Result<MaskImage, AiError> {
    let planes = srgb_planes(image, SIDE, SIDE);
    let outputs = model.run(vec![(INPUT, input(&planes))])?;
    decode(output(&outputs, OUTPUT, 0)?, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probabilities_are_kept_and_logits_are_squashed() {
        let probs = Tensor::new(&[1, 1, 1, 2], vec![0.0, 1.0]).expect("valid");
        let m = decode(&probs, 2, 1).expect("decodes");
        assert_eq!(m.values(), &[0.0, 1.0]);
        let logits = Tensor::new(&[1, 1, 1, 2], vec![-8.0, 8.0]).expect("valid");
        let m = decode(&logits, 2, 1).expect("decodes");
        assert!(m.values()[0] < 0.001 && m.values()[1] > 0.999);
    }

    #[test]
    fn the_square_map_is_stretched_back_to_the_image_aspect() {
        let map = Tensor::new(&[1, 1, 2, 2], vec![1.0, 0.0, 1.0, 0.0]).expect("valid");
        let m = decode(&map, 6, 2).expect("decodes");
        assert_eq!((m.width(), m.height()), (6, 2));
        assert!(m.values()[0] > 0.99 && m.values()[5] < 0.01);
    }

    #[test]
    fn a_malformed_output_is_an_error() {
        let bad = Tensor { shape: vec![3], data: vec![0.0; 2] };
        assert!(matches!(decode(&bad, 2, 2), Err(AiError::UnexpectedOutput(_))));
    }
}
