//! An open image: its developed pixels, its recipe and the edit history.
//!
//! Every change goes through [`commands::EditSession`], so the window gets
//! the same validation and undo as the command line and, later, MCP. Each
//! change bumps [`Document::generation`]; renders carry the generation they
//! were made for, so late results are recognised and dropped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use commands::{Edit, EditError, EditSession, GestureId, HistoryItem, RewriteReport, SavedHistory};
use core_model::{
    CaptureInfo, DevelopSettings, GraphError, ImageF32, LensSettings, NodeId, NodeKind, Orientation, ParamId,
    ParamValue, Params, Recipe, SensorImage, ToolId,
};
use tool_api::{ParamDescriptor, ToolRegistry};

/// The decoded RAW data, kept so the image can be developed again when the
/// development settings (lens corrections, profile) change.
#[derive(Debug)]
pub struct RawData {
    /// Normalised sensor values.
    pub sensor: SensorImage,
    /// Camera, lens, shooting data and colour calibration.
    pub capture: CaptureInfo,
    /// What the RAW reader noted (DNG gain maps applied, opcodes skipped),
    /// shown with the development notes.
    pub file_notes: Vec<String>,
}

/// The output of the development stage for one set of settings.
#[derive(Debug, Clone)]
pub struct DevelopedImage {
    /// The settings it was developed with.
    pub settings: DevelopSettings,
    /// Full-size developed image (linear, working space).
    pub developed: Arc<ImageF32>,
    /// Smaller copy for fast previews (may be the same image when small).
    pub preview: Arc<ImageF32>,
    /// Still smaller copy for renders while a control is dragged (may be
    /// the preview itself).
    pub draft: Arc<ImageF32>,
    /// Lens whose corrections were applied ("Sony FE 24-70mm f/2.8 GM"), if any.
    pub lens: Option<String>,
    /// What the development stage corrected, or why not, for the inspector.
    pub notes: Vec<String>,
    /// The camera profile used, in one line ([`develop::prepare::ProfileOutcome::describe`]).
    pub profile: String,
}

/// A decoded and developed image, as the open job delivers it.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The RAW file.
    pub path: PathBuf,
    /// "Maker Model".
    pub camera: String,
    /// How to turn the image upright.
    pub orientation: Orientation,
    /// `false` when the file had no white balance.
    pub has_camera_white_balance: bool,
    /// The RAW data, to develop again; `None` when it is not kept (tests).
    pub raw: Option<Arc<RawData>>,
    /// Lens recognised from the file in the lens database, if any.
    pub detected_lens: Option<String>,
    /// The lens as the file names it ([`core_model::CaptureInfo::lens_key`]),
    /// to find and save its custom lens.
    pub lens_key: Option<String>,
    /// Every lens of the database with corrections, those fitting the
    /// camera first, for choosing by hand.
    pub lens_choices: Vec<crate::lenses::LensChoice>,
    /// Exposure, date, GPS and the camera's rating, for the Metadata panel.
    pub info: raw_io::PhotoInfo,
    /// The developed image.
    pub image: DevelopedImage,
    /// The base curve that reproduces the camera's own picture of the photo
    /// (what Auto shows), when the file has one.
    pub camera_curve: Option<core_model::CurveFit>,
}

/// One parameter of a node, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamRow {
    /// Type, range, label.
    pub descriptor: ParamDescriptor,
    /// Current value (the default when unset).
    pub value: ParamValue,
}

/// One tool node, ready to draw in the inspector.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    /// The node.
    pub node: NodeId,
    /// Tool name ("Exposure").
    pub name: &'static str,
    /// `false` when bypassed.
    pub enabled: bool,
    /// Parameters in UI order.
    pub params: Vec<ParamRow>,
}

/// An open image with its recipe and history.
#[derive(Debug)]
pub struct Document {
    opened: Opened,
    session: EditSession,
    generation: u64,
    /// Saved by a newer editor: shown, never changed.
    read_only: bool,
}

impl Document {
    /// A document for a freshly opened image: the graph holds only the
    /// Source (and the Output it feeds); tools are added from the node graph.
    /// The history starts empty. `first_generation` should be above every
    /// generation of earlier documents, so their late renders are recognised.
    #[must_use]
    pub fn new(opened: Opened, first_generation: u64) -> Self {
        Self { opened, session: EditSession::new(Recipe::default()), generation: first_generation, read_only: false }
    }

    /// A document for an image whose edit was saved: it starts at `recipe`,
    /// with the saved history when it replays to `recipe` in this build.
    /// Returns the document and whether the history was kept (`false` also
    /// when none was given).
    #[must_use]
    pub fn restored(
        opened: Opened,
        first_generation: u64,
        registry: &ToolRegistry,
        recipe: Recipe,
        history: Option<SavedHistory>,
    ) -> (Self, bool) {
        let restored = history.and_then(|h| EditSession::restore(registry, &recipe, h));
        let kept = restored.is_some();
        let session = restored.unwrap_or_else(|| EditSession::new(recipe));
        (Self { opened, session, generation: first_generation, read_only: false }, kept)
    }

    /// The history in the form the catalog stores.
    #[must_use]
    pub fn saved_history(&self) -> SavedHistory {
        self.session.saved_history()
    }

    /// The decoded image, to start another variant of the same photo
    /// without reading the file again.
    #[must_use]
    pub fn opened(&self) -> &Opened {
        &self.opened
    }

    /// Makes the document read-only (its edit was saved by a newer editor):
    /// every change is refused with [`EditError::ReadOnly`], undo and redo do nothing.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// `true` when nothing can change.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn writable(&self) -> Result<(), EditError> {
        if self.read_only { Err(EditError::ReadOnly) } else { Ok(()) }
    }

    /// A document whose graph starts with `tools` (id, enabled) in a chain
    /// after the Source, those this build has; not part of the history.
    ///
    /// # Errors
    /// [`GraphError`] only if the graph cannot be built (a bug).
    pub fn with_tools(
        opened: Opened,
        registry: &ToolRegistry,
        tools: &[(&str, bool)],
        first_generation: u64,
    ) -> Result<Self, GraphError> {
        let mut recipe = Recipe::default();
        let mut last = recipe.graph.source();
        for &(name, enabled) in tools {
            let Ok(id) = ToolId::new(name) else { continue };
            if registry.get(&id).is_some() {
                last = recipe.graph.insert_serial(last, id, Params::new())?;
                if let Some(mut node) = recipe.graph.node_mut(last) {
                    node.set_enabled(enabled);
                }
            }
        }
        Ok(Self { opened, session: EditSession::new(recipe), generation: first_generation, read_only: false })
    }

    /// The RAW file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.opened.path
    }

    /// Follows the RAW file after it was renamed on disk.
    pub fn file_renamed(&mut self, to: PathBuf) {
        self.opened.path = to;
    }

    /// File name for titles ("DSC_0001.NEF").
    #[must_use]
    pub fn file_name(&self) -> String {
        self.opened.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    /// "Maker Model".
    #[must_use]
    pub fn camera(&self) -> &str {
        &self.opened.camera
    }

    /// How to turn the image upright.
    #[must_use]
    pub fn orientation(&self) -> Orientation {
        self.opened.orientation
    }

    /// Full-size developed image.
    #[must_use]
    pub fn developed(&self) -> &Arc<ImageF32> {
        &self.opened.image.developed
    }

    /// Smaller copy for previews.
    #[must_use]
    pub fn preview(&self) -> &Arc<ImageF32> {
        &self.opened.image.preview
    }

    /// Smallest copy, rendered while a control is dragged.
    #[must_use]
    pub fn draft(&self) -> &Arc<ImageF32> {
        &self.opened.image.draft
    }

    /// `true` when the draft is smaller than the preview, so dragging renders faster with it.
    #[must_use]
    pub fn draft_is_reduced(&self) -> bool {
        self.opened.image.draft.width() < self.opened.image.preview.width()
    }

    /// `true` when the preview is smaller than the full image, so a full render adds detail.
    #[must_use]
    pub fn preview_is_reduced(&self) -> bool {
        self.opened.image.preview.width() < self.opened.image.developed.width()
    }

    /// The preview's width over the full-size image's width (1 when the
    /// preview is the full image).
    #[must_use]
    pub fn preview_scale(&self) -> f32 {
        let (p, f) = (self.opened.image.preview.width(), self.opened.image.developed.width());
        #[allow(clippy::cast_precision_loss)] // image widths
        let scale = if f == 0 { 1.0 } else { p as f32 / f as f32 };
        scale
    }

    /// The draft's size over the full size (1 when there is no smaller draft).
    #[must_use]
    pub fn draft_scale(&self) -> f32 {
        let (d, f) = (self.opened.image.draft.width(), self.opened.image.developed.width());
        #[allow(clippy::cast_precision_loss)] // image widths
        let scale = if f == 0 { 1.0 } else { d as f32 / f as f32 };
        scale
    }

    /// Width and height of the upright full-size image.
    #[must_use]
    pub fn upright_size(&self) -> (u32, u32) {
        let (w, h) = (self.opened.image.developed.width(), self.opened.image.developed.height());
        if self.opened.orientation.swaps_axes() { (h, w) } else { (w, h) }
    }

    /// Width and height of the image as shown and exported: the upright
    /// full-size image, cut to the recipe's crop when `cropped`.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)] // pixel sizes
    pub fn shown_size(&self, cropped: bool) -> (u32, u32) {
        let (w, h) = self.upright_size();
        match self.recipe().geometry.crop.filter(|_| cropped) {
            Some(c) => {
                (((w as f32 * c.width()).round() as u32).max(1), ((h as f32 * c.height()).round() as u32).max(1))
            }
            None => (w, h),
        }
    }

    /// `false` when the file had no white balance.
    #[must_use]
    pub fn has_camera_white_balance(&self) -> bool {
        self.opened.has_camera_white_balance
    }

    /// Shooting data for the Metadata panel.
    #[must_use]
    pub fn info(&self) -> &raw_io::PhotoInfo {
        &self.opened.info
    }

    /// The edit history, oldest step first.
    #[must_use]
    pub fn history(&self) -> Vec<HistoryItem> {
        self.session.history()
    }

    /// Keeps the history steps at positions `order`, in that order; the
    /// others are deleted. One undo step.
    ///
    /// # Errors
    /// [`EditError::BadHistoryOrder`] for an invalid order; nothing changes.
    pub fn rewrite_history(&mut self, registry: &ToolRegistry, order: &[usize]) -> Result<RewriteReport, EditError> {
        self.writable()?;
        let before = self.session.undo_label();
        let report = self.session.rewrite_history(registry, order)?;
        if self.session.undo_label() != before || report.removed > 0 {
            self.generation += 1;
        }
        Ok(report)
    }

    /// Lens whose corrections are applied in the shown image, if any.
    #[must_use]
    pub fn lens(&self) -> Option<&str> {
        self.opened.image.lens.as_deref()
    }

    /// Lens recognised from the file, if any.
    #[must_use]
    pub fn detected_lens(&self) -> Option<&str> {
        self.opened.detected_lens.as_deref()
    }

    /// Lenses that fit the camera, for choosing one by hand.
    #[must_use]
    pub fn lens_choices(&self) -> &[crate::lenses::LensChoice] {
        &self.opened.lens_choices
    }

    /// Profile and lens-correction details of the development stage.
    #[must_use]
    pub fn develop_notes(&self) -> &[String] {
        &self.opened.image.notes
    }

    /// The camera profile the photo is developed with, in one line.
    #[must_use]
    pub fn profile_description(&self) -> &str {
        &self.opened.image.profile
    }

    /// The RAW data, to develop again.
    #[must_use]
    pub fn raw(&self) -> Option<&Arc<RawData>> {
        self.opened.raw.as_ref()
    }

    /// The light the camera measured (kelvin, tint): its white balance read
    /// through the sensor's colour matrix. `None` without RAW data.
    #[must_use]
    pub fn shot_light(&self) -> Option<(f32, f32)> {
        let raw = self.opened.raw.as_ref()?;
        let m = raw.sensor.as_shot_wb();
        let neutral = m.map(|v| if v > 0.0 { 1.0 / v } else { 1.0 });
        let xyz = raw.sensor.xyz_to_camera().inverse()?.apply(neutral);
        if !(xyz.iter().all(|v| v.is_finite()) && xyz[1] > 0.0) {
            return None;
        }
        Some(tool_adjust::white_balance::light_of(core_color::xyz_to_xy(xyz.map(f64::from))))
    }

    /// The developed-image point (where masks and retouching live: the
    /// file's own orientation, 0–1) under the upright point `upright`.
    #[must_use]
    pub fn developed_point(&self, upright: [f32; 2]) -> Option<core_model::NormPoint> {
        let p = core_model::NormPoint::new(upright[0], upright[1])?;
        Some(self.opened.orientation.inverse().to_upright(p))
    }

    /// The upright point (0–1) of a developed-image point.
    #[must_use]
    pub fn upright_point(&self, developed: core_model::NormPoint) -> [f32; 2] {
        let p = self.opened.orientation.to_upright(developed);
        [p.x(), p.y()]
    }

    /// The colour entering `node` at the upright point `at` (0–1 from the
    /// top-left corner): a 5 × 5 average of the developed preview, passed
    /// through the enabled tools between the Source and `node` (those that
    /// work pixel by pixel; the others, and masks, are left out). `None`
    /// outside the image.
    #[must_use]
    pub fn probe(&self, registry: &ToolRegistry, node: NodeId, at: [f32; 2]) -> Option<[f32; 3]> {
        let rgb = self.sample_preview(at)?;
        let graph = &self.recipe().graph;
        let mut chain = Vec::new();
        let mut cur = node;
        while let Some(link) = graph.input_link(cur, 0) {
            let from = graph.node(link.from)?;
            match &from.kind {
                NodeKind::Tool(id) => chain.push((from, id)),
                _ => break,
            }
            cur = link.from;
        }
        let mut px = ImageF32::from_pixels(1, 1, self.preview().space(), vec![[rgb[0], rgb[1], rgb[2], 1.0]]).ok()?;
        for (n, id) in chain.into_iter().rev() {
            let Some(tool) = registry.get(id) else { continue };
            if !n.enabled || tool.gpu_kernel() == tool_api::GpuKernel::Host {
                continue;
            }
            tool.apply_cpu(&mut px, &tool_api::ResolvedParams::resolve(tool.descriptor(), &n.params));
        }
        let p = px.pixels()[0];
        Some([p[0], p[1], p[2]])
    }

    /// A 5 × 5 average of the developed preview at the upright point `at`.
    #[must_use]
    pub fn sample_preview(&self, at: [f32; 2]) -> Option<[f32; 3]> {
        if !(0.0..=1.0).contains(&at[0]) || !(0.0..=1.0).contains(&at[1]) {
            return None;
        }
        let stored = self.opened.orientation.inverse().to_upright(core_model::NormPoint::new(at[0], at[1])?);
        let img = self.preview();
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        // pixel positions
        let (x, y) = (((stored.x() * w as f32) as u32).min(w - 1), ((stored.y() * h as f32) as u32).min(h - 1));
        let (mut sum, mut n) = ([0.0_f32; 3], 0.0_f32);
        for yy in y.saturating_sub(2)..=(y + 2).min(h - 1) {
            for xx in x.saturating_sub(2)..=(x + 2).min(w - 1) {
                if let Some(p) = img.get(xx, yy) {
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += v;
                    }
                    n += 1.0;
                }
            }
        }
        (n > 0.0).then(|| sum.map(|s| s / n))
    }

    /// The light (kelvin, tint) that makes the photo's average colour grey
    /// ("Auto" white balance, grey world): bright clipped and very dark
    /// pixels are left out. `None` without RAW data or for a black image.
    #[must_use]
    pub fn auto_light(&self) -> Option<(f32, f32)> {
        let shot = self.shot_light()?;
        let (mut sum, mut n) = ([0.0_f64; 3], 0_u32);
        for p in self.preview().pixels() {
            let m = p[0].max(p[1]).max(p[2]);
            if m < 0.95 && p[0].min(p[1]).min(p[2]) > 0.002 {
                for (s, v) in sum.iter_mut().zip(p) {
                    *s += f64::from(*v);
                }
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)] // a colour
        let mean = sum.map(|s| (s / f64::from(n)) as f32);
        tool_adjust::white_balance::neutral_light(mean, shot)
    }

    /// `true` when the recipe's development settings differ from those of
    /// the shown image, so it must be developed again.
    #[must_use]
    pub fn needs_develop(&self) -> bool {
        self.opened.raw.is_some() && self.opened.image.settings != self.session.recipe().develop
    }

    /// Replaces the developed image (after the development settings changed).
    /// An image for settings that are no longer current is ignored; returns
    /// whether it was taken.
    pub fn replace_developed(&mut self, image: DevelopedImage) -> bool {
        if image.settings != self.session.recipe().develop {
            return false;
        }
        self.opened.image = image;
        self.generation += 1;
        true
    }

    /// Changes the lens corrections (one undo step). Settings equal to the
    /// current ones change nothing.
    ///
    /// # Errors
    /// [`EditError`] when the settings are invalid; nothing changes.
    pub fn set_lens_correction(&mut self, registry: &ToolRegistry, lens: LensSettings) -> Result<(), EditError> {
        self.writable()?;
        if self.recipe().develop.lens == lens {
            return Ok(());
        }
        self.session.apply(registry, Edit::SetLensCorrection(lens))?;
        self.generation += 1;
        Ok(())
    }

    /// The current recipe.
    #[must_use]
    pub fn recipe(&self) -> &Recipe {
        self.session.recipe()
    }

    /// The base curve the picture is shown with: the recipe's, with Auto
    /// resolved for the file (the camera's look for a RAW file, Linear for a
    /// JPEG or TIFF).
    #[must_use]
    pub fn output_curve(&self) -> core_model::BaseCurve {
        let rendered = self.opened.raw.as_ref().is_some_and(|r| r.capture.rendered);
        self.recipe().curve.with_camera_fit(rendered, self.opened.camera_curve)
    }

    /// Increases with every change to the recipe.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Tool nodes in processing order with their parameters, for the inspector.
    /// Nodes of tools missing from this build are listed without parameters.
    #[must_use]
    pub fn node_rows(&self, registry: &ToolRegistry) -> Vec<NodeRow> {
        let graph = &self.recipe().graph;
        let order = graph.evaluation_order().unwrap_or_default();
        order
            .into_iter()
            .filter_map(|id| graph.node(id))
            .filter_map(|node| {
                let NodeKind::Tool(tool_id) = &node.kind else { return None };
                let Some(tool) = registry.get(tool_id) else {
                    return Some(NodeRow {
                        node: node.id,
                        name: "Missing tool",
                        enabled: node.enabled,
                        params: vec![],
                    });
                };
                let d = tool.descriptor();
                let params = d
                    .params
                    .iter()
                    .map(|p| {
                        let value = node.params.get(&p.id).map_or_else(|| p.default_value(), |v| p.sanitize(v));
                        ParamRow { descriptor: p.clone(), value }
                    })
                    .collect();
                Some(NodeRow { node: node.id, name: d.name, enabled: node.enabled, params })
            })
            .collect()
    }

    /// Starts a continuous gesture (a slider drag): its changes become one undo step.
    pub fn begin_gesture(&mut self) -> GestureId {
        self.session.begin_gesture()
    }

    /// Sets a parameter. A value equal to the current one changes nothing
    /// (no undo step, no new render).
    ///
    /// # Errors
    /// [`EditError`] when the node or parameter does not exist; nothing changes.
    pub fn set_param(
        &mut self,
        registry: &ToolRegistry,
        node: NodeId,
        param: ParamId,
        value: ParamValue,
        gesture: Option<GestureId>,
    ) -> Result<(), EditError> {
        self.writable()?;
        if self.current_value(node, &param) == Some(value) {
            return Ok(());
        }
        self.session.apply_in_gesture(registry, Edit::SetParam { node, param, value }, gesture)?;
        self.generation += 1;
        Ok(())
    }

    /// Sets several parameters of one node in one undo step (merged with
    /// the previous step when it set the same parameters in `gesture`).
    /// Values equal to the current ones change nothing.
    ///
    /// # Errors
    /// [`EditError`] when the node or a parameter does not exist; nothing changes.
    pub fn set_params(
        &mut self,
        registry: &ToolRegistry,
        node: NodeId,
        values: Vec<(ParamId, ParamValue)>,
        gesture: Option<GestureId>,
    ) -> Result<(), EditError> {
        self.writable()?;
        if values.iter().all(|(p, v)| self.current_value(node, p) == Some(*v)) {
            return Ok(());
        }
        self.session.apply_in_gesture(registry, Edit::SetParams { node, values }, gesture)?;
        self.generation += 1;
        Ok(())
    }

    /// Changes the perspective, straightening and crop (one undo step,
    /// merged with the previous one in the same `gesture`). Equal settings
    /// change nothing.
    ///
    /// # Errors
    /// Never in practice ([`EditError`] from the session).
    pub fn set_geometry(
        &mut self,
        registry: &ToolRegistry,
        geometry: core_model::Geometry,
        gesture: Option<GestureId>,
    ) -> Result<(), EditError> {
        if self.recipe().geometry == geometry.sanitized() {
            return Ok(());
        }
        self.session.apply_in_gesture(registry, Edit::SetGeometry(geometry), gesture)?;
        self.generation += 1;
        Ok(())
    }

    /// Chooses the base curve of the output render (an undoable edit).
    ///
    /// # Errors
    /// [`EditError`] when the document is read-only.
    pub fn set_base_curve(&mut self, registry: &ToolRegistry, curve: core_model::BaseCurve) -> Result<(), EditError> {
        self.writable()?;
        if self.recipe().curve == curve {
            return Ok(());
        }
        self.session.apply(registry, Edit::SetBaseCurve(curve))?;
        self.generation += 1;
        Ok(())
    }

    /// The kind of colour filter of the photo's sensor (`None` without RAW data).
    #[must_use]
    pub fn sensor_kind(&self) -> Option<core_model::SensorKind> {
        self.opened.raw.as_ref().map(|r| r.sensor.cfa().kind())
    }

    /// The mask node feeding the key input of tool node `node`, with its definition.
    #[must_use]
    pub fn mask_of(&self, node: NodeId) -> Option<(NodeId, &core_model::MaskDef)> {
        let graph = &self.recipe().graph;
        let port = graph
            .node(node)?
            .kind
            .inputs()
            .iter()
            .position(|k| *k == core_model::PortKind::Mask)
            .and_then(|p| u8::try_from(p).ok())?;
        let link = graph.input_link(node, port)?;
        let mask = graph.node(link.from)?;
        Some((mask.id, mask.mask.as_ref()?))
    }

    /// Gives tool node `to` a new mask (one undo step); returns the mask node.
    ///
    /// # Errors
    /// [`EditError`] when `to` has no mask input; nothing changes.
    pub fn add_mask(
        &mut self,
        registry: &ToolRegistry,
        to: NodeId,
        def: core_model::MaskDef,
    ) -> Result<NodeId, EditError> {
        self.session.apply(registry, Edit::AddMask { to, def })?;
        self.generation += 1;
        self.mask_of(to).map(|(id, _)| id).ok_or(EditError::NoMaskInput(to))
    }

    /// Applies a style (a preset) to the photo (one undo step): see
    /// [`Edit::ApplyStyle`].
    ///
    /// # Errors
    /// [`EditError`] when the style names a tool this build lacks, or the
    /// photo is read-only; nothing changes.
    pub fn apply_style(
        &mut self,
        registry: &ToolRegistry,
        name: &str,
        style: core_model::Style,
    ) -> Result<(), EditError> {
        self.writable()?;
        self.session.apply(registry, Edit::ApplyStyle { name: name.to_owned(), style: Box::new(style) })?;
        self.generation += 1;
        Ok(())
    }

    /// Paints style brush `name` (one undo step): its tools are added
    /// behind a new mask `mask` (see [`Edit::AddStyleBrush`]). Returns the
    /// first new tool node, whose mask further strokes extend.
    ///
    /// # Errors
    /// [`EditError`] as for [`Edit::AddStyleBrush`], or the photo is
    /// read-only; nothing changes.
    pub fn add_style_brush(
        &mut self,
        registry: &ToolRegistry,
        name: &str,
        style: core_model::Style,
        mask: core_model::MaskDef,
    ) -> Result<NodeId, EditError> {
        self.writable()?;
        let before: Vec<NodeId> = core_model::tool_nodes(&self.recipe().graph);
        let edit = Edit::AddStyleBrush { name: name.to_owned(), style: Box::new(style), mask };
        self.session.apply(registry, edit)?;
        self.generation += 1;
        core_model::tool_nodes(&self.recipe().graph)
            .into_iter()
            .find(|id| !before.contains(id))
            .ok_or(EditError::EmptyStyle)
    }

    /// Replaces the definition of mask node `mask` (one undo step, merged
    /// with the previous one in the same `gesture`).
    ///
    /// # Errors
    /// [`EditError`] when `mask` is not a mask node; nothing changes.
    pub fn set_mask(
        &mut self,
        registry: &ToolRegistry,
        mask: NodeId,
        def: core_model::MaskDef,
        gesture: Option<GestureId>,
    ) -> Result<(), EditError> {
        self.session.apply_in_gesture(registry, Edit::SetMask { node: mask, def }, gesture)?;
        self.generation += 1;
        Ok(())
    }

    /// Returns a parameter to its default (one undo step).
    ///
    /// # Errors
    /// [`EditError`] when the node or parameter does not exist; nothing changes.
    pub fn reset_param(&mut self, registry: &ToolRegistry, node: NodeId, param: ParamId) -> Result<(), EditError> {
        self.writable()?;
        if self.recipe().graph.node(node).is_some_and(|n| n.params.get(&param).is_none()) {
            return Ok(());
        }
        self.session.apply(registry, Edit::ResetParam { node, param })?;
        self.generation += 1;
        Ok(())
    }

    /// Enables or bypasses a node (one undo step).
    ///
    /// # Errors
    /// [`EditError`] when the node does not exist; nothing changes.
    pub fn set_enabled(&mut self, registry: &ToolRegistry, node: NodeId, enabled: bool) -> Result<(), EditError> {
        self.writable()?;
        if self.recipe().graph.node(node).is_some_and(|n| n.enabled == enabled) {
            return Ok(());
        }
        self.session.apply(registry, Edit::SetEnabled { node, enabled })?;
        self.generation += 1;
        Ok(())
    }

    /// Adds a node running `tool` right after `after` (what `after` fed now
    /// receives the new node's image); returns the new node.
    ///
    /// # Errors
    /// [`EditError`] for an unknown node or tool; nothing changes.
    pub fn add_node(&mut self, registry: &ToolRegistry, after: NodeId, tool: ToolId) -> Result<NodeId, EditError> {
        self.add_node_with(registry, after, tool, Params::new())
    }

    /// Where a node added from the menu goes: after `selected` when it has
    /// an image output (not the Output), else at the end of the chain, just
    /// before the Output.
    #[must_use]
    pub fn insertion_point(&self, selected: Option<NodeId>) -> NodeId {
        let graph = &self.recipe().graph;
        let gives_image = |id: NodeId| {
            id != graph.output()
                && graph.node(id).is_some_and(|n| n.kind.outputs().first() == Some(&core_model::PortKind::Image))
        };
        selected
            .filter(|id| gives_image(*id))
            .or_else(|| graph.input_link(graph.output(), 0).map(|l| l.from).filter(|id| gives_image(*id)))
            .unwrap_or_else(|| graph.source())
    }

    /// Like [`Self::add_node`], with the new node's first values (one undo step).
    ///
    /// # Errors
    /// [`EditError`] for an unknown node or tool; nothing changes.
    pub fn add_node_with(
        &mut self,
        registry: &ToolRegistry,
        after: NodeId,
        tool: ToolId,
        params: Params,
    ) -> Result<NodeId, EditError> {
        self.writable()?;
        let before: Vec<NodeId> = self.recipe().graph.nodes().map(|n| n.id).collect();
        self.session.apply(registry, Edit::InsertSerial { after, tool, params })?;
        self.generation += 1;
        let created = self.recipe().graph.nodes().map(|n| n.id).find(|id| !before.contains(id));
        // InsertSerial always creates one node; its id is the new one.
        Ok(created.unwrap_or(after))
    }

    /// Removes a node, joining the chain around it.
    ///
    /// # Errors
    /// [`EditError`] for an unknown node, the Source or the Output; nothing changes.
    pub fn remove_node(&mut self, registry: &ToolRegistry, node: NodeId) -> Result<(), EditError> {
        self.writable()?;
        self.session.apply(registry, Edit::RemoveNode { node })?;
        self.generation += 1;
        Ok(())
    }

    /// Undoes the last change; returns its label.
    pub fn undo(&mut self) -> Option<String> {
        self.writable().ok()?;
        let label = self.session.undo()?;
        self.generation += 1;
        Some(label)
    }

    /// Redoes the last undone change; returns its label.
    pub fn redo(&mut self) -> Option<String> {
        self.writable().ok()?;
        let label = self.session.redo()?;
        self.generation += 1;
        Some(label)
    }

    /// "Change exposure" when there is something to undo.
    #[must_use]
    pub fn undo_label(&self) -> Option<String> {
        self.writable().ok()?;
        self.session.undo_label()
    }

    /// Label of the change [`Self::redo`] would redo.
    #[must_use]
    pub fn redo_label(&self) -> Option<String> {
        self.writable().ok()?;
        self.session.redo_label()
    }

    /// Where an export goes by default: `<folder>/<file stem>.jpg`.
    #[must_use]
    pub fn export_path(&self, folder: &Path) -> PathBuf {
        let stem = self.opened.path.file_stem().map_or_else(|| "image".into(), |s| s.to_string_lossy().into_owned());
        folder.join(format!("{stem}.jpg"))
    }

    fn current_value(&self, node: NodeId, param: &ParamId) -> Option<ParamValue> {
        self.recipe().graph.node(node)?.params.get(param)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use core_model::WorkingSpace;

    fn registry() -> ToolRegistry {
        engine::builtin_tools().unwrap()
    }

    fn opened(path: &str, orientation: Orientation) -> Opened {
        let img = Arc::new(ImageF32::new(6, 4, WorkingSpace::LinearRec2020).unwrap());
        Opened {
            path: PathBuf::from(path),
            camera: "Nikon D3S".into(),
            orientation,
            has_camera_white_balance: true,
            raw: None,
            detected_lens: None,
            lens_choices: Vec::new(),
            lens_key: None,
            info: raw_io::PhotoInfo::default(),
            camera_curve: None,
            image: DevelopedImage {
                settings: DevelopSettings::default(),
                developed: Arc::clone(&img),
                preview: Arc::clone(&img),
                draft: img,
                lens: None,
                notes: Vec::new(),
                profile: String::new(),
            },
        }
    }

    /// The chain the tests start from (the window starts from the Source alone).
    const STARTING: &[(&str, bool)] = &[("exposure", true), ("denoise_ai", false)];

    fn exposure_param() -> ParamId {
        ParamId::new(tool_exposure_id()).unwrap()
    }

    fn tool_exposure_id() -> &'static str {
        "exposure"
    }

    #[test]
    fn a_new_document_has_only_the_source_and_nodes_are_added_after_a_port() {
        let reg = registry();
        let mut doc = Document::new(opened("a/DSC_1.NEF", Orientation::Normal), 1);
        assert!(doc.node_rows(&reg).is_empty());
        assert_eq!(doc.recipe().graph.nodes().count(), 2, "Source and Output");
        let source = doc.recipe().graph.source();
        let exposure = ToolId::new(tool_exposure_id()).unwrap();
        let a = doc.add_node(&reg, source, exposure.clone()).unwrap();
        let b = doc.add_node(&reg, source, exposure).unwrap();
        // b went between the Source and a.
        let rows: Vec<NodeId> = doc.node_rows(&reg).iter().map(|r| r.node).collect();
        assert_eq!(rows, vec![b, a]);
        doc.remove_node(&reg, b).unwrap();
        assert_eq!(doc.node_rows(&reg).len(), 1);
        assert!(doc.remove_node(&reg, source).is_err(), "the Source stays");
        doc.undo();
        assert_eq!(doc.node_rows(&reg).len(), 2);
        assert!(doc.add_node(&reg, source, ToolId::new("no_such_tool").unwrap()).is_err());
    }

    #[test]
    fn a_node_can_start_with_values_in_one_undo_step() {
        let reg = registry();
        let mut doc = Document::new(opened("a/DSC_1.NEF", Orientation::Normal), 1);
        let source = doc.recipe().graph.source();
        let mut p = Params::new();
        p.set(exposure_param(), ParamValue::Float(1.5));
        p.set(ParamId::new("not_a_param").unwrap(), ParamValue::Float(9.0));
        let id = doc.add_node_with(&reg, source, ToolId::new("exposure").unwrap(), p).unwrap();
        let node = doc.recipe().graph.node(id).unwrap();
        assert_eq!(node.params.get(&exposure_param()), Some(ParamValue::Float(1.5)));
        assert_eq!(node.params.get(&ParamId::new("not_a_param").unwrap()), None, "unknown keys are dropped");
        doc.undo();
        assert!(doc.recipe().graph.node(id).is_none());
    }

    #[test]
    fn probing_reads_the_preview_upright_through_the_tools_before_the_node() {
        let reg = registry();
        // 6 × 4 stored image, shown rotated 90° clockwise: upright it is 4 × 6.
        let mut px = vec![[0.1_f32, 0.1, 0.1, 1.0]; 24];
        px[0] = [1.0, 0.5, 0.25, 1.0]; // stored top-left corner
        let img = Arc::new(ImageF32::from_pixels(6, 4, WorkingSpace::LinearRec2020, px).unwrap());
        let mut o = opened("a/DSC_1.NEF", Orientation::Rotate90);
        o.image.preview = Arc::clone(&img);
        let mut doc = Document::new(o, 1);
        let source = doc.recipe().graph.source();
        let exposure = doc.add_node(&reg, source, ToolId::new("exposure").unwrap()).unwrap();
        doc.set_param(&reg, exposure, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        let after = doc.add_node(&reg, exposure, ToolId::new("hdr").unwrap()).unwrap();
        // Rotated 90° clockwise, the stored top-left corner is at the upright top-right.
        let at_exposure = doc.probe(&reg, exposure, [0.99, 0.01]).unwrap();
        let at_after = doc.probe(&reg, after, [0.99, 0.01]).unwrap();
        assert!(at_after[0] > at_exposure[0] * 1.9, "{at_exposure:?} {at_after:?}");
        assert!(at_exposure[0] > at_exposure[2], "the corner's colour is in the average");
        assert_eq!(doc.probe(&reg, after, [1.5, 0.0]), None);
        assert_eq!(doc.shot_light(), None, "no RAW data in this test");
    }

    #[test]
    fn a_node_from_the_menu_goes_after_the_selection_or_at_the_end_of_the_chain() {
        let reg = registry();
        let mut doc = Document::new(opened("a/DSC_1.NEF", Orientation::Normal), 1);
        let source = doc.recipe().graph.source();
        assert_eq!(doc.insertion_point(None), source, "an empty chain grows from the Source");
        let exposure = ToolId::new(tool_exposure_id()).unwrap();
        let a = doc.add_node(&reg, source, exposure.clone()).unwrap();
        let b = doc.add_node(&reg, a, exposure).unwrap();
        assert_eq!(doc.insertion_point(None), b, "after the last node");
        assert_eq!(doc.insertion_point(Some(a)), a, "after the selected node");
        let output = doc.recipe().graph.output();
        assert_eq!(doc.insertion_point(Some(output)), b, "never after the Output");
    }

    #[test]
    fn a_document_with_tools_has_them_and_an_empty_history() {
        let reg = registry();
        let doc = Document::with_tools(opened("a/DSC_1.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let rows = doc.node_rows(&reg);
        assert_eq!(rows[0].name, "Exposure");
        assert!(rows[0].enabled);
        assert_eq!(rows[0].params[0].value, ParamValue::Float(0.0));
        // The AI denoise node, when this build has it, comes after and starts bypassed.
        let denoise: Vec<_> = rows.iter().filter(|r| r.name == "AI denoise").collect();
        assert!(denoise.iter().all(|r| !r.enabled));
        assert_eq!(rows.len(), 1 + denoise.len());
        assert_eq!(doc.undo_label(), None);
        assert_eq!(doc.file_name(), "DSC_1.NEF");
    }

    #[test]
    fn a_renamed_file_keeps_its_edits() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("a/DSC_1.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(0.5), None).unwrap();
        doc.file_renamed(PathBuf::from("a/Como.NEF"));
        assert_eq!(doc.file_name(), "Como.NEF");
        assert_eq!(doc.path(), Path::new("a/Como.NEF"));
        assert_eq!(doc.history().len(), 1);
    }

    #[test]
    fn a_slider_drag_is_one_undo_step_and_each_change_is_a_new_generation() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        let g0 = doc.generation();
        let drag = doc.begin_gesture();
        for v in [0.1, 0.5, 1.2] {
            doc.set_param(&reg, node, exposure_param(), ParamValue::Float(v), Some(drag)).unwrap();
        }
        assert_eq!(doc.generation(), g0 + 3);
        assert_eq!(doc.undo_label().as_deref(), Some("Change exposure"));
        assert_eq!(doc.undo().as_deref(), Some("Change exposure"));
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(0.0));
        assert_eq!(doc.undo(), None);
        assert!(doc.redo().is_some());
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(1.2));
    }

    #[test]
    fn setting_the_same_value_changes_nothing() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        let g = doc.generation();
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        assert_eq!(doc.generation(), g);
        doc.undo();
        assert_eq!(doc.undo_label(), None);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_reset_returns_to_default() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(99.0), None).unwrap();
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(4.0));
        doc.reset_param(&reg, node, exposure_param()).unwrap();
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(0.0));
        assert_eq!(doc.undo_label().as_deref(), Some("Reset exposure"));
        let g = doc.generation();
        doc.reset_param(&reg, node, exposure_param()).unwrap();
        assert_eq!(doc.generation(), g, "resetting a default value is not a change");
    }

    #[test]
    fn bypass_is_undoable() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_enabled(&reg, node, false).unwrap();
        assert!(!doc.node_rows(&reg)[0].enabled);
        doc.undo();
        assert!(doc.node_rows(&reg)[0].enabled);
    }

    #[test]
    fn unknown_nodes_are_refused_without_changes() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let g = doc.generation();
        let source = doc.recipe().graph.source();
        assert!(doc.set_param(&reg, source, exposure_param(), ParamValue::Float(1.0), None).is_err());
        assert_eq!(doc.generation(), g);
    }

    #[test]
    fn upright_size_and_export_name_follow_the_file() {
        let reg = registry();
        let doc = Document::with_tools(opened("dir/IMG 7.CR3", Orientation::Rotate90), &reg, STARTING, 1).unwrap();
        assert_eq!(doc.upright_size(), (4, 6));
        assert_eq!(doc.export_path(Path::new("out")), Path::new("out").join("IMG 7.jpg"));
        assert!(!doc.preview_is_reduced());
    }

    #[test]
    fn deleting_a_history_step_is_a_new_generation_and_can_be_undone() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(2.0), None).unwrap();
        assert_eq!(doc.history().len(), 2);
        let g = doc.generation();
        let report = doc.rewrite_history(&reg, &[0]).unwrap();
        assert_eq!(report.removed, 1);
        assert!(doc.generation() > g);
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(1.0));
        let g = doc.generation();
        doc.rewrite_history(&reg, &[0]).unwrap();
        assert_eq!(doc.generation(), g, "the same order changes nothing");
        assert_eq!(doc.undo().as_deref(), Some("Delete a history step"));
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(2.0));
    }

    #[test]
    fn a_restored_document_has_the_same_recipe_and_history() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(2.0), None).unwrap();
        doc.undo();
        let o = doc.opened().clone();
        let (mut back, kept) = Document::restored(o, 9, &reg, doc.recipe().clone(), Some(doc.saved_history()));
        assert!(kept);
        assert_eq!((back.recipe(), back.history()), (doc.recipe(), doc.history()));
        assert_eq!(back.generation(), 9);
        assert_eq!(back.redo().as_deref(), Some("Change exposure"));
        assert_eq!(back.node_rows(&reg)[0].params[0].value, ParamValue::Float(2.0));

        // A history that does not lead to the recipe is dropped; the recipe stays.
        let o = doc.opened().clone();
        let (fresh, kept) = Document::restored(o, 1, &reg, Recipe::default(), Some(doc.saved_history()));
        assert!(!kept);
        assert_eq!(fresh.recipe(), &Recipe::default());
        assert!(fresh.history().is_empty());
    }

    #[test]
    fn a_read_only_document_refuses_every_change() {
        let reg = registry();
        let mut doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        doc.set_read_only(true);
        assert!(doc.is_read_only());
        let (recipe, g) = (doc.recipe().clone(), doc.generation());
        let source = doc.recipe().graph.source();
        let ro = Err(EditError::ReadOnly);
        assert_eq!(doc.set_param(&reg, node, exposure_param(), ParamValue::Float(2.0), None), ro);
        assert_eq!(doc.set_params(&reg, node, vec![(exposure_param(), ParamValue::Float(2.0))], None), ro);
        assert_eq!(doc.reset_param(&reg, node, exposure_param()), ro);
        assert_eq!(doc.set_enabled(&reg, node, false), ro);
        assert_eq!(doc.add_node(&reg, source, ToolId::new("exposure").unwrap()), Err(EditError::ReadOnly));
        assert_eq!(doc.remove_node(&reg, node), ro);
        assert_eq!(doc.set_lens_correction(&reg, LensSettings::off()), ro);
        assert_eq!(doc.rewrite_history(&reg, &[]), Err(EditError::ReadOnly));
        assert_eq!((doc.undo(), doc.undo_label(), doc.redo(), doc.redo_label()), (None, None, None, None));
        assert_eq!((doc.recipe(), doc.generation()), (&recipe, g));
    }

    #[test]
    fn missing_tools_are_listed_without_parameters() {
        let reg = registry();
        let doc = Document::with_tools(opened("x.NEF", Orientation::Normal), &reg, STARTING, 1).unwrap();
        let rows = doc.node_rows(&ToolRegistry::new());
        assert_eq!(rows.len(), doc.node_rows(&reg).len(), "one row per starting node");
        assert!(rows.iter().all(|r| r.name == "Missing tool" && r.params.is_empty()));
    }

    #[test]
    fn changing_lens_corrections_asks_for_a_new_development_and_undo_too() {
        let reg = registry();
        let mut o = opened("x.NEF", Orientation::Normal);
        let sensor = SensorImage::new(
            2,
            2,
            vec![0.0; 4],
            core_model::CfaPattern::Bayer(core_model::BayerPattern::Rggb),
            [1.0; 3],
            core_model::Matrix3::IDENTITY,
        )
        .unwrap();
        o.raw = Some(Arc::new(RawData { sensor, capture: CaptureInfo::default(), file_notes: Vec::new() }));
        let mut doc = Document::with_tools(o, &reg, STARTING, 1).unwrap();
        assert!(!doc.needs_develop());

        let off = LensSettings::off();
        doc.set_lens_correction(&reg, off.clone()).unwrap();
        assert!(doc.needs_develop());
        let g = doc.generation();
        let stale = DevelopedImage {
            settings: DevelopSettings::default(),
            developed: Arc::clone(doc.developed()),
            preview: Arc::clone(doc.preview()),
            draft: Arc::clone(doc.draft()),
            lens: None,
            notes: Vec::new(),
            profile: String::new(),
        };
        assert!(!doc.replace_developed(stale.clone()), "an image for old settings is refused");
        let fresh = DevelopedImage { settings: doc.recipe().develop.clone(), ..stale };
        assert!(doc.replace_developed(fresh));
        assert!(!doc.needs_develop() && doc.generation() > g);

        assert_eq!(doc.undo().as_deref(), Some("Change lens corrections"));
        assert!(doc.needs_develop());
        assert_eq!(doc.recipe().develop.lens, LensSettings::default());
    }
}
