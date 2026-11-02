//! The segmentation engine: loads models on demand, keeps the Segment
//! Anything embedding of the current image, and turns AI mask components
//! into rasters.
//!
//! Images are passed as the editor stores them (the developed image, before
//! the camera orientation is applied) with their [`Orientation`]. Models see
//! the image upright, because they were trained on upright photos; prompts
//! go in and rasters come out in the stored orientation, so they line up
//! with the recipe.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use core_model::{AiMask, AiTarget, BodyPart, ImageF32, MaskImage, ModelId, NormRect, Orientation, PromptPoint};

use crate::catalog::{
    BIREFNET_GENERAL, MEDIAPIPE_PARTS, ModelSpec, ModelStatus, ModelStore, SAM21_LARGE, Task, YOLOX_S, spec,
};
use crate::detect::{self, Detection};
use crate::inference::{AiError, Inference, Model};
use crate::parts::{self, PersonAnalysis};
use crate::prep::{Planes, sigmoid, srgb_planes};
use crate::region::{self, Plane};
use crate::{sam, subject};

/// Long side of the rasters the engine produces. Objects are segmented a
/// second time on a crop around them, so their rasters carry more detail
/// than one model pass over the whole frame; edge refinement at render time
/// brings the mask to full resolution.
pub const RASTER_LONG_SIDE: u32 = 2048;

/// Objects covering more of the frame than this are not segmented a second
/// time on a crop: the large model already sees them in enough detail.
const ZOOM_MAX_AREA: f32 = 0.1;
/// Long side of the working copy of the photo used for person parts.
const WORK_LONG_SIDE: u32 = 3072;
/// People analysed and kept for further parts.
const PERSON_CACHE: usize = 16;
/// Margin around an object in its zoomed crop, as a fraction of its size.
const ZOOM_MARGIN: f32 = 0.15;

/// The model used for each kind of target.
#[must_use]
pub fn default_model(target: &AiTarget) -> ModelId {
    match target {
        AiTarget::Subject => BIREFNET_GENERAL.model_id(),
        AiTarget::Object { .. } => SAM21_LARGE.model_id(),
        AiTarget::PersonPart { .. } => MEDIAPIPE_PARTS.model_id(),
    }
}

type Shared = Arc<Mutex<Box<dyn Model>>>;

/// Loads models from a [`ModelStore`] and runs them.
pub struct Segmenter {
    inference: Arc<dyn Inference>,
    store: ModelStore,
    sessions: Mutex<HashMap<PathBuf, Shared>>,
    /// The last image encoded by Segment Anything: (image id, orientation, embedding).
    embedding: Mutex<Option<(String, Orientation, Arc<sam::Embedding>)>>,
    /// Working copy of the last photo used for person parts.
    work: Mutex<Option<(String, Orientation, Arc<Planes>)>>,
    /// People already analysed: their mask and their parts, by image and box.
    people: Mutex<HashMap<String, Arc<(Plane, PersonAnalysis)>>>,
}

impl std::fmt::Debug for Segmenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segmenter").field("store", &self.store).finish_non_exhaustive()
    }
}

impl Segmenter {
    /// An engine running models from `store` with `inference`.
    #[must_use]
    pub fn new(inference: Arc<dyn Inference>, store: ModelStore) -> Self {
        Self {
            inference,
            store,
            sessions: Mutex::new(HashMap::new()),
            embedding: Mutex::new(None),
            work: Mutex::new(None),
            people: Mutex::new(HashMap::new()),
        }
    }

    /// The model folder.
    #[must_use]
    pub fn store(&self) -> &ModelStore {
        &self.store
    }

    /// The loaded model playing `role` in `spec`, loading it on first use.
    fn session(&self, spec: &'static ModelSpec, role: &str) -> Result<Shared, AiError> {
        let file = spec.file(role).ok_or_else(|| AiError::UnknownModel(spec.model_id()))?;
        let path = self.store.path(file);
        if let Some(s) = self.sessions.lock().unwrap_or_else(PoisonError::into_inner).get(&path) {
            return Ok(Arc::clone(s));
        }
        if self.store.status(spec) != ModelStatus::Ready {
            return Err(AiError::ModelFile { model: spec.id, path, url: file.url });
        }
        // Loading takes seconds; do it without holding the map's lock.
        let model: Shared = Arc::new(Mutex::new(self.inference.load(&path)?));
        let mut map = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Arc::clone(map.entry(path).or_insert(model)))
    }

    /// Devices the loaded models run on, as "file: device".
    #[must_use]
    pub fn devices(&self) -> Vec<String> {
        let map = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        let mut v: Vec<String> = map
            .iter()
            .map(|(path, m)| {
                let device = m.lock().unwrap_or_else(PoisonError::into_inner).device();
                format!("{}: {device}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())
            })
            .collect();
        v.sort();
        v
    }

    /// Computes the raster of one AI component: the subject, or the object
    /// under its prompt. `image_id` names the image (and its development
    /// settings) so the Segment Anything embedding is reused across clicks.
    ///
    /// # Errors
    /// [`AiError`] when a model is missing or fails, or the prompt is incomplete.
    pub fn compute(
        &self,
        image_id: &str,
        image: &ImageF32,
        orientation: Orientation,
        ai: &AiMask,
    ) -> Result<MaskImage, AiError> {
        let spec = spec(&ai.model).ok_or_else(|| AiError::UnknownModel(ai.model.clone()))?;
        if !ai.target.is_complete() {
            return Err(AiError::IncompletePrompt);
        }
        let upright_target = upright(&ai.target, orientation);
        let raster = match (spec.task, &upright_target) {
            (Task::Subject, AiTarget::Subject) => {
                let up = upright_image(image, orientation)?;
                let (w, h) = raster_size(&up);
                let model = self.session(spec, "model")?;
                let mut model = model.lock().unwrap_or_else(PoisonError::into_inner);
                subject::segment(model.as_mut(), &up, w, h)?
            }
            (Task::Promptable, AiTarget::Object { .. }) => {
                self.object_raster(spec, image_id, image, orientation, &upright_target)?
            }
            (Task::PersonParts, AiTarget::PersonPart { person, part }) => {
                self.part_raster(spec, image_id, image, orientation, *person, *part)?
            }
            _ => {
                return Err(AiError::UnexpectedOutput(format!(
                    "model '{}' cannot select this kind of target",
                    spec.id
                )));
            }
        };
        raster.oriented(orientation.inverse()).map_err(|e| AiError::Image(e.to_string()))
    }

    /// Segments an object (upright target) in two passes: on the whole frame,
    /// then again on a crop around what the first pass found, so the object
    /// fills the model's input and its fine structure comes out.
    fn object_raster(
        &self,
        spec: &'static ModelSpec,
        image_id: &str,
        image: &ImageF32,
        orientation: Orientation,
        target: &AiTarget,
    ) -> Result<MaskImage, AiError> {
        let (w, h) = raster_size_of(image, orientation);
        let embedding = self.embedding_for(spec, image_id, image, orientation)?;
        let decoder = self.session(spec, "decoder")?;
        let first = {
            let mut decoder = decoder.lock().unwrap_or_else(PoisonError::into_inner);
            sam::logits(decoder.as_mut(), &embedding, target)?
        };
        let AiTarget::Object { points, rect } = target else { return Err(AiError::IncompletePrompt) };
        let found = first.bbox_above(0.0);
        let Some(region) = union(found, *rect) else { return to_mask(first.resized(w, h).map(sigmoid)) };
        if region::area(region) > ZOOM_MAX_AREA {
            return to_mask(first.resized(w, h).map(sigmoid));
        }

        let crop_rect = region::expand(region, ZOOM_MARGIN, 0.01);
        let crop = region::crop(&upright_image(image, orientation)?, crop_rect)?;
        let zoomed = {
            let encoder = self.session(spec, "encoder")?;
            let mut encoder = encoder.lock().unwrap_or_else(PoisonError::into_inner);
            sam::encode(encoder.as_mut(), &crop)?
        };
        // The same clicks in the crop's coordinates, and a box: the one given,
        // else the first pass's result (it removes the ambiguity of one click).
        let box_in_crop = rect.or_else(|| found.map(|f| region::expand(f, 0.02, 0.0))).and_then(|r| {
            Some(NormRect::new(region::into_rect(r.min(), crop_rect)?, region::into_rect(r.max(), crop_rect)?))
        });
        let clicks = points
            .iter()
            .filter_map(|p| Some(PromptPoint { at: region::into_rect(p.at, crop_rect)?, include: p.include }))
            .collect();
        let second = {
            let mut decoder = decoder.lock().unwrap_or_else(PoisonError::into_inner);
            sam::logits(decoder.as_mut(), &zoomed, &AiTarget::Object { points: clicks, rect: box_in_crop })?
        };
        let mut raster = Plane { w, h, data: vec![-30.0; w * h] };
        raster.paste(&second, crop_rect);
        to_mask(raster.map(sigmoid))
    }

    /// A part of the person in `person` (upright box), as an upright raster.
    fn part_raster(
        &self,
        spec: &'static ModelSpec,
        image_id: &str,
        image: &ImageF32,
        orientation: Orientation,
        person: NormRect,
        part: BodyPart,
    ) -> Result<MaskImage, AiError> {
        let key = format!("{image_id}/{orientation:?}/{}", masks::fingerprint(&[format!("{person:?}").as_bytes()]));
        let cached = self.people.lock().unwrap_or_else(PoisonError::into_inner).get(&key).cloned();
        let entry = match cached {
            Some(e) => e,
            None => {
                let object_spec =
                    crate::catalog::spec(&default_model(&AiTarget::Object { points: vec![], rect: None }))
                        .ok_or_else(|| AiError::UnknownModel(SAM21_LARGE.model_id()))?;
                let target = AiTarget::Object { points: vec![], rect: Some(person) };
                let mask = self.object_raster(object_spec, image_id, image, orientation, &target)?;
                let mask = Plane { w: mask.width() as usize, h: mask.height() as usize, data: mask.into_values() };
                let work = self.work_planes(image_id, image, orientation)?;
                let (ww, wh) = (work.w as f32, work.h as f32);
                let px = (person.min().x() * ww, person.min().y() * wh, person.max().x() * ww, person.max().y() * wh);
                let segmenter = self.session(spec, "segmenter")?;
                let faces = self.session(spec, "faces")?;
                let mesher = self.session(spec, "landmarks")?;
                let analysis = {
                    let mut s = segmenter.lock().unwrap_or_else(PoisonError::into_inner);
                    let mut f = faces.lock().unwrap_or_else(PoisonError::into_inner);
                    let mut m = mesher.lock().unwrap_or_else(PoisonError::into_inner);
                    parts::analyse(s.as_mut(), f.as_mut(), m.as_mut(), &work, px)?
                };
                let entry = Arc::new((mask, analysis));
                let mut people = self.people.lock().unwrap_or_else(PoisonError::into_inner);
                if people.len() >= PERSON_CACHE {
                    people.clear();
                }
                people.insert(key, Arc::clone(&entry));
                entry
            }
        };
        let (mask, analysis) = (&entry.0, &entry.1);
        if parts::needs_face(part) && analysis.mesh.is_none() {
            return Err(AiError::NoFace);
        }
        let work_w = self.work.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map_or(1, |(_, _, p)| p.w);
        let k = mask.w as f32 / work_w.max(1) as f32;
        to_mask(parts::part_plane(analysis, part, mask, k))
    }

    /// The upright photo at working size, sRGB-encoded, kept for the next part.
    fn work_planes(&self, image_id: &str, image: &ImageF32, orientation: Orientation) -> Result<Arc<Planes>, AiError> {
        if let Some((id, o, p)) = self.work.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && id == image_id
            && *o == orientation
        {
            return Ok(Arc::clone(p));
        }
        let up = upright_image(image, orientation)?;
        let (w, h) = masks::fit_within(up.width(), up.height(), WORK_LONG_SIDE);
        let planes = Arc::new(srgb_planes(&up, w as usize, h as usize));
        *self.work.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((image_id.to_owned(), orientation, Arc::clone(&planes)));
        Ok(planes)
    }

    /// The Segment Anything embedding of the image, computed once per image.
    fn embedding_for(
        &self,
        spec: &'static ModelSpec,
        image_id: &str,
        image: &ImageF32,
        orientation: Orientation,
    ) -> Result<Arc<sam::Embedding>, AiError> {
        if let Some((id, o, e)) = self.embedding.lock().unwrap_or_else(PoisonError::into_inner).as_ref()
            && id == image_id
            && *o == orientation
        {
            return Ok(Arc::clone(e));
        }
        let up = upright_image(image, orientation)?;
        let encoder = self.session(spec, "encoder")?;
        let embedding = {
            let mut encoder = encoder.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::new(sam::encode(encoder.as_mut(), &up)?)
        };
        *self.embedding.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((image_id.to_owned(), orientation, Arc::clone(&embedding)));
        Ok(embedding)
    }

    /// Finds people (or other COCO classes) in the image. Boxes are in the
    /// stored orientation, best first.
    ///
    /// # Errors
    /// [`AiError`] when the detector is missing or fails.
    pub fn detect(
        &self,
        image: &ImageF32,
        orientation: Orientation,
        classes: &[usize],
        min_score: f32,
    ) -> Result<Vec<Detection>, AiError> {
        let up = upright_image(image, orientation)?;
        let model = self.session(&YOLOX_S, "model")?;
        let mut model = model.lock().unwrap_or_else(PoisonError::into_inner);
        let wanted = |c: usize| classes.is_empty() || classes.contains(&c);
        let mut found = detect::detect(model.as_mut(), &up, min_score, &wanted)?;
        let back = orientation.inverse();
        for d in &mut found {
            d.rect = NormRect::new(back.to_upright(d.rect.min()), back.to_upright(d.rect.max()));
        }
        Ok(found)
    }
}

/// The smallest rectangle holding both, if any.
fn union(a: Option<NormRect>, b: Option<NormRect>) -> Option<NormRect> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let min = core_model::NormPoint::new(a.min().x().min(b.min().x()), a.min().y().min(b.min().y()))?;
            let max = core_model::NormPoint::new(a.max().x().max(b.max().x()), a.max().y().max(b.max().y()))?;
            Some(NormRect::new(min, max))
        }
        (a, b) => a.or(b),
    }
}

/// A plane of mask values as a [`MaskImage`].
fn to_mask(p: Plane) -> Result<MaskImage, AiError> {
    match (u32::try_from(p.w), u32::try_from(p.h)) {
        (Ok(w), Ok(h)) => MaskImage::from_values(w, h, p.data).map_err(|e| AiError::Image(e.to_string())),
        _ => Err(AiError::Image("mask too large".to_owned())),
    }
}

/// The image turned upright.
fn upright_image(image: &ImageF32, orientation: Orientation) -> Result<ImageF32, AiError> {
    image.oriented(orientation).map_err(|e| AiError::Image(e.to_string()))
}

/// Raster size for an upright image.
fn raster_size(up: &ImageF32) -> (usize, usize) {
    let (w, h) = masks::fit_within(up.width(), up.height(), RASTER_LONG_SIDE);
    (w as usize, h as usize)
}

/// Raster size for the upright version of a stored image, without turning it.
fn raster_size_of(image: &ImageF32, orientation: Orientation) -> (usize, usize) {
    let (w, h) =
        if orientation.swaps_axes() { (image.height(), image.width()) } else { (image.width(), image.height()) };
    let (w, h) = masks::fit_within(w, h, RASTER_LONG_SIDE);
    (w as usize, h as usize)
}

/// A target with its coordinates turned upright.
fn upright(target: &AiTarget, orientation: Orientation) -> AiTarget {
    match target {
        AiTarget::Subject => AiTarget::Subject,
        AiTarget::Object { points, rect } => AiTarget::Object {
            points: points
                .iter()
                .map(|p| PromptPoint { at: orientation.to_upright(p.at), include: p.include })
                .collect(),
            rect: rect.map(|r| NormRect::new(orientation.to_upright(r.min()), orientation.to_upright(r.max()))),
        },
        AiTarget::PersonPart { person, part } => AiTarget::PersonPart {
            person: NormRect::new(orientation.to_upright(person.min()), orientation.to_upright(person.max())),
            part: *part,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Tensor;
    use core_model::{NormPoint, WorkingSpace};
    use std::path::Path;

    /// A fake runtime: the "subject" model selects the left half of its
    /// input square; the SAM encoder counts its runs; the decoder selects the
    /// right half; the detector finds nothing.
    #[derive(Default)]
    struct Fake {
        encoder_runs: Arc<Mutex<usize>>,
    }

    struct FakeModel {
        name: String,
        encoder_runs: Arc<Mutex<usize>>,
    }

    impl Model for FakeModel {
        fn run(&mut self, inputs: Vec<(&'static str, Tensor)>) -> Result<Vec<(String, Tensor)>, AiError> {
            let half = |left: bool, k: usize, side: usize| {
                let data =
                    (0..k * side * side).map(|i| if ((i % side) < side / 2) == left { 9.0 } else { -9.0 }).collect();
                Tensor { shape: vec![1, k, side, side], data }
            };
            Ok(match self.name.as_str() {
                n if n.contains("BiRefNet") => {
                    assert_eq!(inputs[0].1.shape, vec![1, 3, 1024, 1024]);
                    vec![("output_image".to_owned(), half(true, 1, 8))]
                }
                n if n.contains("encoder") => {
                    *self.encoder_runs.lock().expect("lock") += 1;
                    vec![
                        ("high_res_0".to_owned(), Tensor::filled(&[1, 32, 256, 256], 0.0)),
                        ("high_res_1".to_owned(), Tensor::filled(&[1, 64, 128, 128], 0.0)),
                        ("image_embed".to_owned(), Tensor::filled(&[1, 256, 64, 64], 0.0)),
                    ]
                }
                n if n.contains("decoder") => {
                    vec![("masks".to_owned(), half(false, 4, 16)), ("iou".to_owned(), Tensor::filled(&[1, 4], 0.5))]
                }
                n if n.contains("selfie") => {
                    // Hair (class 1) in the top half of the square, background below.
                    let mut data = vec![0.0; 256 * 256 * 6];
                    for (i, px) in data.as_chunks_mut::<6>().0.iter_mut().enumerate() {
                        px[if i / 256 < 128 { 1 } else { 0 }] = 8.0;
                    }
                    vec![("Identity".to_owned(), Tensor { shape: vec![1, 256, 256, 6], data })]
                }
                n if n.contains("face_detector") => vec![
                    ("regressors".to_owned(), Tensor::filled(&[1, 896, 16], 0.0)),
                    ("classificators".to_owned(), Tensor::filled(&[1, 896, 1], -10.0)),
                ],
                _ => vec![("output".to_owned(), Tensor::filled(&[1, 8400, 85], 0.0))],
            })
        }

        fn device(&self) -> String {
            "fake".to_owned()
        }
    }

    impl Inference for Fake {
        fn load(&self, path: &Path) -> Result<Box<dyn Model>, AiError> {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Ok(Box::new(FakeModel { name, encoder_runs: Arc::clone(&self.encoder_runs) }))
        }
    }

    /// A model folder with empty files of the right sizes (extended, never
    /// written), removed when dropped.
    struct TempStore(ModelStore);

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.dir());
        }
    }

    fn store(name: &str) -> TempStore {
        let dir = std::env::temp_dir().join(format!("aktis-ai-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let store = ModelStore::new(&dir);
        for spec in crate::catalog::CATALOG {
            for f in spec.files {
                let file = std::fs::File::create(store.path(f)).expect("create");
                file.set_len(f.bytes).expect("set size");
            }
        }
        TempStore(store)
    }

    fn image(w: u32, h: u32) -> ImageF32 {
        ImageF32::from_pixels(w, h, WorkingSpace::LinearRec2020, vec![[0.18, 0.18, 0.18, 1.0]; (w * h) as usize])
            .expect("valid image")
    }

    #[test]
    fn the_subject_raster_comes_back_in_the_stored_orientation() {
        let fake = Arc::new(Fake::default());
        let dir = store("subject");
        let seg = Segmenter::new(fake, dir.0.clone());
        let ai = AiMask { target: AiTarget::Subject, model: default_model(&AiTarget::Subject), raster: None };
        // Stored landscape 40×20, shown rotated 90° clockwise: upright it is 20×40.
        let m = seg.compute("img", &image(40, 20), Orientation::Rotate90, &ai).expect("computes");
        assert_eq!((m.width(), m.height()), (40, 20), "raster in the stored orientation");
        // The model selected the upright left half, which is the stored bottom half.
        assert!(m.get(20, 19).expect("inside") > 0.9 && m.get(20, 0).expect("inside") < 0.1);
        assert_eq!(seg.devices().len(), 1);
    }

    #[test]
    fn segment_anything_encodes_each_image_once_and_needs_a_prompt() {
        let fake = Arc::new(Fake::default());
        let runs = Arc::clone(&fake.encoder_runs);
        let dir = store("sam");
        let seg = Segmenter::new(fake, dir.0.clone());
        let click = |x| AiTarget::Object {
            points: vec![PromptPoint { at: NormPoint::new(x, 0.5).expect("finite"), include: true }],
            rect: None,
        };
        let ai = |t: AiTarget| AiMask { model: default_model(&t), target: t, raster: None };
        let m = seg.compute("img", &image(30, 20), Orientation::Normal, &ai(click(0.7))).expect("first click");
        assert!(m.get(29, 10).expect("inside") > 0.9 && m.get(0, 10).expect("inside") < 0.1);
        seg.compute("img", &image(30, 20), Orientation::Normal, &ai(click(0.2))).expect("second click");
        assert_eq!(*runs.lock().expect("lock"), 1, "same image: the embedding is reused");
        seg.compute("other", &image(30, 20), Orientation::Normal, &ai(click(0.2))).expect("new image");
        assert_eq!(*runs.lock().expect("lock"), 2);

        let empty = AiTarget::Object { points: vec![], rect: None };
        assert_eq!(seg.compute("img", &image(30, 20), Orientation::Normal, &ai(empty)), Err(AiError::IncompletePrompt));
    }

    #[test]
    fn missing_model_files_are_reported_with_their_download() {
        let dir = std::env::temp_dir().join(format!("aktis-ai-empty-{}", std::process::id()));
        let seg = Segmenter::new(Arc::new(Fake::default()), ModelStore::new(&dir));
        let ai = AiMask { target: AiTarget::Subject, model: default_model(&AiTarget::Subject), raster: None };
        match seg.compute("img", &image(4, 4), Orientation::Normal, &ai) {
            Err(AiError::ModelFile { model, url, .. }) => {
                assert_eq!(model, "birefnet_general");
                assert!(url.starts_with("https://"));
            }
            other => panic!("expected a missing model, got {other:?}"),
        }
        let unknown = AiMask { model: ModelId::new("nope").expect("id"), ..ai };
        assert!(matches!(
            seg.compute("img", &image(4, 4), Orientation::Normal, &unknown),
            Err(AiError::UnknownModel(_))
        ));
    }

    #[test]
    fn person_parts_are_cut_by_the_person_mask_and_features_need_a_face() {
        let dir = store("parts");
        let seg = Segmenter::new(Arc::new(Fake::default()), dir.0.clone());
        let person = NormRect::new(NormPoint::new(0.0, 0.0).expect("p"), NormPoint::new(1.0, 1.0).expect("p"));
        let part = |part| {
            let target = AiTarget::PersonPart { person, part };
            AiMask { model: default_model(&target), target, raster: None }
        };
        let hair = seg.compute("img", &image(40, 40), Orientation::Normal, &part(BodyPart::Hair)).expect("hair");
        // Hair is the top half (segmenter) of the right half (person mask from the fake decoder).
        assert!(hair.get(30, 5).expect("inside") > 0.9, "top right");
        assert!(hair.get(5, 5).expect("inside") < 0.1, "outside the person");
        assert!(hair.get(30, 35).expect("inside") < 0.1, "below the hair");
        let lips = seg.compute("img", &image(40, 40), Orientation::Normal, &part(BodyPart::Lips));
        assert_eq!(lips, Err(AiError::NoFace));
    }

    #[test]
    fn detection_runs_and_finds_nothing_in_an_empty_output() {
        let dir = store("detect");
        let seg = Segmenter::new(Arc::new(Fake::default()), dir.0.clone());
        assert!(seg.detect(&image(64, 48), Orientation::Normal, &[detect::PERSON], 0.3).expect("runs").is_empty());
    }
}
