//! Building AI mask components, and filling in their rasters.
//!
//! The UI (and the command line) create components with the functions
//! here, put them in a mask definition, then call [`compute_pending`]; the
//! definition with its raster keys goes into the recipe through an
//! undoable edit.

use core_model::{
    AiTarget, BodyPart, ImageF32, MaskComponent, MaskDef, MaskOp, MaskSource, NormRect, Orientation, PromptPoint,
};
use masks::RasterStore;

use crate::detect::Detection;
use crate::engine::{Segmenter, default_model};
use crate::inference::AiError;

/// "Select subject".
#[must_use]
pub fn subject(op: MaskOp) -> MaskComponent {
    MaskComponent::ai("Subject", op, AiTarget::Subject, default_model(&AiTarget::Subject))
}

/// An object chosen with clicks and/or a rectangle.
#[must_use]
pub fn object(label: impl Into<String>, op: MaskOp, points: Vec<PromptPoint>, rect: Option<NormRect>) -> MaskComponent {
    let target = AiTarget::Object { points, rect };
    let model = default_model(&target);
    MaskComponent::ai(label, op, target, model)
}

/// A part of the person inside `person` (a box from "Select people").
#[must_use]
pub fn person_part(label: impl Into<String>, op: MaskOp, person: NormRect, part: BodyPart) -> MaskComponent {
    let target = AiTarget::PersonPart { person, part };
    let model = default_model(&target);
    MaskComponent::ai(label, op, target, model)
}

/// One component per detection, labelled "Person 1", "Person 2", "Dog 1" …
/// in order of confidence; together they select all of them.
#[must_use]
pub fn from_detections(detections: &[Detection], op: MaskOp) -> Vec<MaskComponent> {
    let mut counts = std::collections::HashMap::new();
    detections
        .iter()
        .map(|d| {
            let n = counts.entry(d.class).and_modify(|n| *n += 1).or_insert(1_u32);
            let name = d.class_name();
            let mut label: String = name.chars().take(1).flat_map(char::to_uppercase).collect();
            label.push_str(name.get(1..).unwrap_or(""));
            object(format!("{label} {n}"), op, vec![], Some(d.rect))
        })
        .collect()
}

/// Computes the raster of every AI component of `def` that has none, and
/// records its key. A raster already in `store` for the same image, model
/// and prompt is reused without running the model. Returns the components
/// that failed, with why; the others are filled in even when some fail.
pub fn compute_pending(
    segmenter: &Segmenter,
    image_id: &str,
    image: &ImageF32,
    orientation: Orientation,
    def: &mut MaskDef,
    store: &dyn RasterStore,
) -> Vec<(usize, AiError)> {
    let mut failures = Vec::new();
    for (index, component) in def.components.iter_mut().enumerate() {
        let MaskSource::Ai(ai) = &mut component.source else { continue };
        if ai.raster.is_some() {
            continue;
        }
        let key = masks::raster_key(image_id, &ai.model, &ai.target);
        if store.raster(&key).is_none() {
            let raster = match segmenter.compute(image_id, image, orientation, ai) {
                Ok(r) => r,
                Err(e) => {
                    failures.push((index, e));
                    continue;
                }
            };
            if let Err(e) = store.store(key, raster) {
                failures.push((index, AiError::Image(format!("the mask could not be saved: {e}"))));
                continue;
            }
        }
        ai.raster = Some(key);
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::NormPoint;

    fn det(class: usize, x: f32) -> Detection {
        let a = NormPoint::new(x, 0.1).expect("finite");
        let b = NormPoint::new(x + 0.1, 0.5).expect("finite");
        Detection { class, score: 0.9, rect: NormRect::new(a, b) }
    }

    #[test]
    fn detections_become_numbered_rectangle_prompts() {
        let comps = from_detections(&[det(0, 0.1), det(16, 0.3), det(0, 0.5)], MaskOp::Add);
        let labels: Vec<&str> = comps.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Person 1", "Dog 1", "Person 2"]);
        let MaskSource::Ai(ai) = &comps[2].source else { panic!("AI component") };
        assert_eq!(ai.target, AiTarget::Object { points: vec![], rect: Some(det(0, 0.5).rect) });
        assert_eq!(ai.model.as_str(), "sam2.1_large");
        assert!(comps.iter().all(|c| c.op == MaskOp::Add && c.enabled));
    }

    #[test]
    fn the_subject_uses_the_subject_model() {
        let MaskSource::Ai(ai) = &subject(MaskOp::Add).source else { panic!("AI component") };
        assert_eq!(ai.model.as_str(), "birefnet_general");
        assert_eq!(ai.raster, None);
    }
}
