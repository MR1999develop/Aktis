//! Saving and reopening edits, as Capture One does: each photo has one or
//! more variants, each with its own recipe, history, node positions and a
//! small preview of the edited image, all kept in the catalog (no sidecar
//! files, the RAW file is never written).
//!
//! The window saves the open variant when another photo or variant is
//! opened, when it closes, and [`AUTOSAVE_SECONDS`] after a change. An edit
//! saved by a newer version of the editor is opened read-only: shown when
//! this version can read its recipe, never saved over.
//!
//! The catalog calls (SQLite) stay in the window; this module decides what
//! to store and builds documents from what is stored, so it is tested
//! without a database.

use catalog::{StoredVariant, VariantInfo, variant_name};
use commands::{Edit, EditSession};
use core_model::{LensSettings, Recipe, Style};
use engine::Rgb8Image;
use recipe_store::{DecodeError, FORMAT_VERSION, Variant};
use tool_api::ToolRegistry;

use crate::document::{Document, Opened};
use crate::graph_view::GraphView;

/// Seconds after a change before the open variant is saved, even if the
/// user stays on it.
pub const AUTOSAVE_SECONDS: u64 = 30;

/// Longer side of the edited preview kept in the catalog: enough for the
/// filmstrip and the tiles of a multiple selection.
pub const EDITED_PREVIEW_SIDE: u32 = 1024;

/// JPEG quality of that preview.
pub const EDITED_PREVIEW_QUALITY: u8 = 85;

/// What to write to the catalog for one variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// Number, name and format of the variant.
    pub info: VariantInfo,
    /// The edit as text, when it changed.
    pub data: Option<String>,
    /// A JPEG of the edited image, when one is due.
    pub preview: Option<Vec<u8>>,
}

impl Save {
    /// `true` when there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_none() && self.preview.is_none()
    }
}

/// The variant being edited and what the catalog has for it.
#[derive(Debug, Clone)]
pub struct VariantState {
    /// Number, name and format (the format of this build once saved).
    pub info: VariantInfo,
    /// Why the variant cannot be saved: an editor newer than this one
    /// saved it. `None` when it can.
    pub read_only: Option<String>,
    /// The text the catalog has; `None` when nothing is stored yet.
    stored: Option<String>,
    /// Generation and graph view at the last load or save; `None` for a
    /// variant never written (written at the next save).
    clean: Option<(u64, GraphView)>,
    /// The stored preview is older than the stored edit.
    preview_stale: bool,
}

/// A photo's variant opened from what the catalog has.
#[derive(Debug)]
pub struct Reopened {
    /// The document, read-only when a newer editor saved it.
    pub doc: Document,
    /// The stored node positions, or an empty view.
    pub view: GraphView,
    /// What is stored.
    pub state: VariantState,
    /// Something to tell the user (read-only, history not restored, damaged edit).
    pub notice: Option<String>,
}

impl VariantState {
    /// A variant not in the catalog yet: written at the next save.
    #[must_use]
    pub fn unsaved(number: u32, name: String) -> Self {
        Self {
            info: VariantInfo { number, name, format: FORMAT_VERSION },
            read_only: None,
            stored: None,
            clean: None,
            preview_stale: false,
        }
    }

    /// The first variant of a photo never edited: nothing stored, nothing
    /// written until something changes.
    #[must_use]
    pub fn untouched(number: u32, doc: &Document, view: &GraphView) -> Self {
        Self { clean: Some((doc.generation(), view.clone())), ..Self::unsaved(number, variant_name(number)) }
    }

    /// Makes the next save write the variant when nothing is stored for it
    /// yet (before another variant is added, so this one stays in the list).
    pub fn ensure_stored(&mut self) {
        if self.stored.is_none() && self.read_only.is_none() {
            self.clean = None;
        }
    }

    /// `true` when the edit or the node positions changed since the last
    /// load or save (always `false` for a read-only variant).
    #[must_use]
    pub fn is_dirty(&self, doc: &Document, view: &GraphView) -> bool {
        self.read_only.is_none() && self.clean.as_ref().is_none_or(|(g, v)| *g != doc.generation() || v != view)
    }

    /// `true` when the stored preview should be made again (the edit was
    /// saved while the screen did not show it yet).
    #[must_use]
    pub fn preview_due(&self) -> bool {
        self.read_only.is_none() && self.preview_stale && self.stored.is_some()
    }

    /// The catalog has no preview of the stored edit (it was changed
    /// without being shown): one is made at the next save.
    pub fn preview_missing(&mut self) {
        if self.stored.is_some() {
            self.preview_stale = true;
        }
    }

    /// The variant as text, as the catalog stores it.
    #[must_use]
    pub fn encode(&self, doc: &Document, view: &GraphView) -> String {
        recipe_store::encode(&Variant {
            name: self.info.name.clone(),
            recipe: doc.recipe().clone(),
            history: Some(doc.saved_history()),
            layout: Some(view.layout()),
        })
    }

    /// What to write now: the edit when it differs from what is stored,
    /// and a preview made from `shown` (the image on screen for this
    /// variant, `None` when there is none to use) when the edit was written
    /// or the stored preview is stale. Nothing for a read-only variant.
    #[must_use]
    pub fn to_save(&self, doc: &Document, view: &GraphView, shown: Option<&Rgb8Image>) -> Save {
        let mut save = Save { info: self.info.clone(), data: None, preview: None };
        if self.read_only.is_some() {
            return save;
        }
        if self.is_dirty(doc, view) {
            let text = self.encode(doc, view);
            if self.stored.as_deref() != Some(text.as_str()) {
                save.data = Some(text);
            }
        }
        let due = save.data.is_some() || (self.preview_stale && self.stored.is_some());
        if due && let Some(image) = shown {
            save.preview = engine::preview_jpeg(image, EDITED_PREVIEW_SIDE, EDITED_PREVIEW_QUALITY).ok();
        }
        save
    }

    /// Records that `save` was written (or that there was nothing to write).
    pub fn saved(&mut self, doc: &Document, view: &GraphView, save: Save) {
        if self.read_only.is_some() {
            return;
        }
        let wrote_edit = save.data.is_some();
        if let Some(text) = save.data {
            self.stored = Some(text);
        }
        if save.preview.is_some() {
            self.preview_stale = false;
        } else if wrote_edit {
            self.preview_stale = true;
        }
        self.clean = Some((doc.generation(), view.clone()));
    }
}

/// The read-only notice for an edit in format `format`.
fn newer_notice(format: u32, shown: bool) -> String {
    let what = if shown {
        "its recipe is shown as this version reads it"
    } else {
        "this version cannot show its edits, so the photo is shown unedited"
    };
    format!(
        "Read-only: this edit was saved by a newer version of the editor (format {format}; this one reads up to \
         {FORMAT_VERSION}); {what}. Changes cannot be saved over it: make a new variant to edit the photo here."
    )
}

/// `view` with every node of the document placed, as the graph panel
/// places them when it first draws: so drawing it is not a change.
fn placed(mut view: GraphView, doc: &Document) -> GraphView {
    view.sync(&doc.recipe().graph);
    view
}

/// Applies `style` to variant `number` of a photo that is not open (a
/// preset applied to several photos of the filmstrip): the stored edit
/// (`None` for a photo never edited) with one more history step, as the
/// catalog stores it. The stored preview is left as it is; it is made
/// again when the photo is next shown.
///
/// # Errors
/// A message when the stored edit was saved by a newer editor, cannot be
/// read, or refuses the style.
pub fn apply_style_to_stored(
    registry: &ToolRegistry,
    number: u32,
    stored: Option<&StoredVariant>,
    name: &str,
    style: Style,
) -> Result<Save, String> {
    let variant = match stored {
        None => Variant { name: variant_name(number), recipe: Recipe::default(), history: None, layout: None },
        Some(s) if s.info.format > FORMAT_VERSION => {
            return Err(DecodeError::NewerVersion(s.info.format).to_string());
        }
        Some(s) => recipe_store::decode(&s.data).map_err(|e| e.to_string())?,
    };
    let mut session = variant
        .history
        .clone()
        .and_then(|h| EditSession::restore(registry, &variant.recipe, h))
        .unwrap_or_else(|| EditSession::new(variant.recipe.clone()));
    session
        .apply(registry, Edit::ApplyStyle { name: name.to_owned(), style: Box::new(style) })
        .map_err(|e| e.to_string())?;
    let info = VariantInfo { number, name: variant.name.clone(), format: FORMAT_VERSION };
    let data = recipe_store::encode(&Variant {
        name: variant.name,
        recipe: session.recipe().clone(),
        history: Some(session.saved_history()),
        layout: variant.layout,
    });
    Ok(Save { info, data: Some(data), preview: None })
}

/// The recipe of a stored variant, for using it without opening the photo
/// (an export): read leniently, so the edit of a newer editor is still
/// used as far as it is understood; `None` when nothing of it can be read.
#[must_use]
pub fn stored_recipe(stored: &StoredVariant) -> Option<Recipe> {
    if stored.info.format <= FORMAT_VERSION
        && let Ok(variant) = recipe_store::decode(&stored.data)
    {
        return Some(variant.recipe);
    }
    recipe_store::decode_recipe_lenient(&stored.data).map(|(_, recipe)| recipe)
}

/// The document of a photo as it came from the camera (a variant never
/// saved, or a new one): the default recipe, with `start_lens` (the
/// settings of its custom lens) when given; an empty history.
#[must_use]
pub fn fresh(
    opened: Opened,
    first_generation: u64,
    registry: &ToolRegistry,
    start_lens: Option<LensSettings>,
) -> Document {
    match start_lens {
        Some(lens) => {
            let mut recipe = Recipe::default();
            recipe.develop.lens = lens;
            Document::restored(opened, first_generation, registry, recipe, None).0
        }
        None => Document::new(opened, first_generation),
    }
}

/// Builds the document of variant `number` of a photo from what the
/// catalog has for it (`stored`; `None` for a variant never saved, which
/// starts with `start_lens`, the settings of its custom lens, when given).
#[must_use]
pub fn reopen(
    opened: Opened,
    first_generation: u64,
    registry: &ToolRegistry,
    number: u32,
    stored: Option<StoredVariant>,
    start_lens: Option<LensSettings>,
) -> Reopened {
    let Some(stored) = stored else {
        let doc = fresh(opened, first_generation, registry, start_lens);
        let view = placed(GraphView::default(), &doc);
        let state = VariantState::untouched(number, &doc, &view);
        return Reopened { doc, view, state, notice: None };
    };
    let decoded = if stored.info.format > FORMAT_VERSION {
        Err(DecodeError::NewerVersion(stored.info.format))
    } else {
        recipe_store::decode(&stored.data)
    };
    match decoded {
        Ok(variant) => {
            let had_history = variant.history.is_some();
            let (doc, kept) = Document::restored(opened, first_generation, registry, variant.recipe, variant.history);
            let view = placed(variant.layout.as_ref().map(GraphView::from_layout).unwrap_or_default(), &doc);
            let notice = (had_history && !kept).then(|| {
                "The undo history of this photo could not be restored in this version; its edits are kept.".to_owned()
            });
            let state = VariantState {
                info: VariantInfo { format: FORMAT_VERSION, ..stored.info },
                read_only: None,
                clean: Some((doc.generation(), view.clone())),
                stored: Some(stored.data),
                preview_stale: false,
            };
            Reopened { doc, view, state, notice }
        }
        Err(error) => {
            let readable = recipe_store::decode_recipe_lenient(&stored.data);
            let shown = readable.is_some();
            let mut doc = match readable {
                Some((_, recipe)) => Document::restored(opened, first_generation, registry, recipe, None).0,
                None => Document::new(opened, first_generation),
            };
            doc.set_read_only(true);
            let why = match error {
                DecodeError::NewerVersion(format) => newer_notice(format, shown),
                DecodeError::Malformed(why) => format!(
                    "Read-only: the saved edit of this photo is damaged ({why}); it is kept as it is. Make a new \
                     variant to edit the photo."
                ),
            };
            let view = placed(GraphView::default(), &doc);
            let state = VariantState {
                info: stored.info,
                read_only: Some(why.clone()),
                clean: Some((doc.generation(), view.clone())),
                stored: Some(stored.data),
                preview_stale: false,
            };
            Reopened { doc, view, state, notice: Some(why) }
        }
    }
}

/// The variants to list for a photo: those stored, and the open one when
/// it is not stored yet, by number.
#[must_use]
pub fn variant_list(stored: &[VariantInfo], open: Option<&VariantInfo>) -> Vec<VariantInfo> {
    let mut list = stored.to_vec();
    if let Some(open) = open {
        match list.iter_mut().find(|v| v.number == open.number) {
            Some(v) => v.name.clone_from(&open.name),
            None => list.push(open.clone()),
        }
    }
    list.sort_by_key(|v| v.number);
    list
}

/// The variant to show after deleting `deleted` from `list`: the one
/// before it, else the one after; `None` when it was the only one.
#[must_use]
pub fn after_delete(list: &[VariantInfo], deleted: u32) -> Option<u32> {
    let others = list.iter().map(|v| v.number).filter(|n| *n != deleted);
    others.clone().filter(|n| *n < deleted).max().or_else(|| others.min())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use core_model::{DevelopSettings, ImageF32, Orientation, ParamId, ParamValue, WorkingSpace};

    use super::*;
    use crate::document::DevelopedImage;

    fn registry() -> ToolRegistry {
        engine::builtin_tools().unwrap()
    }

    fn opened() -> Opened {
        let img = Arc::new(ImageF32::new(6, 4, WorkingSpace::LinearRec2020).unwrap());
        Opened {
            path: PathBuf::from("a/DSC_1.NEF"),
            camera: "Nikon D3S".into(),
            orientation: Orientation::Normal,
            has_camera_white_balance: true,
            raw: None,
            detected_lens: None,
            lens_choices: Vec::new(),
            lens_key: None,
            info: raw_io::PhotoInfo::default(),
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

    fn shown() -> Rgb8Image {
        Rgb8Image { width: 8, height: 6, pixels: vec![90; 8 * 6 * 3] }
    }

    /// Opens a never-edited photo and adds an exposure node set to +1.
    fn edited(reg: &ToolRegistry) -> Reopened {
        let mut r = reopen(opened(), 1, reg, 1, None, None);
        let source = r.doc.recipe().graph.source();
        let node = r.doc.add_node(reg, source, core_model::ToolId::new("exposure").unwrap()).unwrap();
        let p = ParamId::new("exposure").unwrap();
        r.doc.set_param(reg, node, p, ParamValue::Float(1.0), None).unwrap();
        r.view.sync(&r.doc.recipe().graph);
        r
    }

    /// What the catalog would hold after `save`.
    fn stored(save: &Save) -> StoredVariant {
        StoredVariant { info: save.info.clone(), data: save.data.clone().unwrap() }
    }

    #[test]
    fn a_style_applied_to_a_closed_photo_adds_one_history_step() {
        let reg = registry();
        let style = crate::presets::builtin_brushes()[0].preset.style.clone();
        // Never edited: a first variant with the style.
        let save = apply_style_to_stored(&reg, 1, None, "Bright", style.clone()).unwrap();
        assert_eq!(save.info.name, "Variant 1");
        let v = recipe_store::decode(save.data.as_deref().unwrap()).unwrap();
        assert_eq!(v.history.as_ref().map(|h| h.steps.len()), Some(1));
        assert_eq!(core_model::tool_nodes(&v.recipe.graph).len(), 1);
        // Edited before: its history and node positions are kept.
        let mut r = edited(&reg);
        let first = r.state.to_save(&r.doc, &r.view, None);
        r.state.saved(&r.doc, &r.view, first.clone());
        let row = stored(&first);
        let save = apply_style_to_stored(&reg, 1, Some(&row), "Bright", style.clone()).unwrap();
        let v = recipe_store::decode(save.data.as_deref().unwrap()).unwrap();
        assert_eq!(v.history.as_ref().map(|h| h.steps.len()), Some(3));
        assert!(v.layout.is_some());
        let reopened = reopen(opened(), 1, &reg, 1, Some(stored(&save)), None);
        assert!(reopened.notice.is_none());
        assert_eq!(reopened.doc.recipe(), &v.recipe);
        // A newer editor's edit is left alone.
        let newer = StoredVariant { info: VariantInfo { format: FORMAT_VERSION + 1, ..row.info.clone() }, ..row };
        assert!(apply_style_to_stored(&reg, 1, Some(&newer), "Bright", style).unwrap_err().contains("newer"));
    }

    #[test]
    fn a_photo_never_edited_writes_nothing_until_it_changes() {
        let reg = registry();
        let mut r = reopen(opened(), 1, &reg, 1, None, None);
        assert!(r.notice.is_none() && !r.doc.is_read_only());
        // The graph panel draws it.
        r.view.sync(&r.doc.recipe().graph);
        assert_eq!(r.state.info, VariantInfo { number: 1, name: "Variant 1".into(), format: FORMAT_VERSION });
        assert!(!r.state.is_dirty(&r.doc, &r.view));
        assert!(r.state.to_save(&r.doc, &r.view, Some(&shown())).is_empty());
    }

    #[test]
    fn an_edit_is_saved_with_a_preview_and_reopens_identical() {
        let reg = registry();
        let mut r = edited(&reg);
        assert!(r.state.is_dirty(&r.doc, &r.view));
        let save = r.state.to_save(&r.doc, &r.view, Some(&shown()));
        assert!(save.data.is_some());
        assert_eq!(&save.preview.as_deref().unwrap()[..2], &[0xff, 0xd8], "a JPEG");
        let row = stored(&save);
        r.state.saved(&r.doc, &r.view, save);
        assert!(!r.state.is_dirty(&r.doc, &r.view));
        assert!(r.state.to_save(&r.doc, &r.view, Some(&shown())).is_empty(), "nothing new");

        let back = reopen(opened(), 50, &reg, 1, Some(row), None);
        assert!(back.notice.is_none());
        assert_eq!(back.doc.recipe(), r.doc.recipe());
        assert_eq!(back.doc.history(), r.doc.history());
        assert_eq!(back.view, r.view);
        assert!(!back.state.is_dirty(&back.doc, &back.view));
    }

    #[test]
    fn moving_a_node_is_a_change_but_the_same_text_is_not_written_again() {
        let reg = registry();
        let mut r = edited(&reg);
        let save = r.state.to_save(&r.doc, &r.view, None);
        r.state.saved(&r.doc, &r.view, save);
        let source = r.doc.recipe().graph.source();
        r.view.move_node(source, [10.0, 0.0]);
        assert!(r.state.is_dirty(&r.doc, &r.view));
        let save = r.state.to_save(&r.doc, &r.view, None);
        assert!(save.data.is_some());
        r.state.saved(&r.doc, &r.view, save);
        r.view.move_node(source, [-10.0, 0.0]);
        r.view.move_node(source, [10.0, 0.0]);
        assert!(r.state.to_save(&r.doc, &r.view, None).is_empty(), "back where it was saved");
    }

    #[test]
    fn a_preview_missed_at_save_time_is_made_later() {
        let reg = registry();
        let mut r = edited(&reg);
        let save = r.state.to_save(&r.doc, &r.view, None);
        assert!(save.data.is_some() && save.preview.is_none());
        r.state.saved(&r.doc, &r.view, save);
        assert!(r.state.preview_due());
        let later = r.state.to_save(&r.doc, &r.view, Some(&shown()));
        assert!(later.data.is_none() && later.preview.is_some());
        r.state.saved(&r.doc, &r.view, later);
        assert!(!r.state.preview_due());
    }

    #[test]
    fn an_edit_from_a_newer_editor_opens_read_only_and_is_never_saved() {
        let reg = registry();
        let r = edited(&reg);
        let save = r.state.to_save(&r.doc, &r.view, None);
        let mut row = stored(&save);
        row.data = row.data.replacen("\"format\":1", "\"format\":9", 1);
        row.info.format = 9;
        let mut back = reopen(opened(), 5, &reg, 1, Some(row.clone()), None);
        assert!(back.doc.is_read_only());
        let why = back.state.read_only.clone().unwrap();
        assert!(why.contains("newer version") && why.contains("format 9"), "{why}");
        assert_eq!(back.notice.as_deref(), Some(why.as_str()));
        assert_eq!(back.doc.recipe(), r.doc.recipe(), "the recipe is still shown");
        assert_eq!(back.state.info.format, 9);
        back.view.pan = [5.0, 5.0];
        assert!(!back.state.is_dirty(&back.doc, &back.view));
        assert!(back.state.to_save(&back.doc, &back.view, Some(&shown())).is_empty());

        // A recipe this version cannot read: shown unedited.
        row.data = row.data.replacen("{\"tool\":\"exposure\"}", "{\"lut3d\":\"x\"}", 1);
        let unknown = reopen(opened(), 5, &reg, 1, Some(row), None);
        assert!(unknown.doc.is_read_only());
        assert_eq!(unknown.doc.recipe(), &core_model::Recipe::default());
        assert!(unknown.notice.unwrap().contains("cannot show"));
    }

    #[test]
    fn a_damaged_edit_is_kept_read_only() {
        let reg = registry();
        let row = StoredVariant {
            info: VariantInfo { number: 2, name: "Variant 2".into(), format: 1 },
            data: "{\"format\":1,".into(),
        };
        let r = reopen(opened(), 1, &reg, 2, Some(row), None);
        assert!(r.doc.is_read_only());
        assert!(r.notice.unwrap().contains("damaged"));
        assert!(r.state.to_save(&r.doc, &r.view, Some(&shown())).is_empty());
    }

    #[test]
    fn an_untouched_variant_is_written_before_another_is_added() {
        let reg = registry();
        let mut r = reopen(opened(), 1, &reg, 1, None, None);
        r.state.ensure_stored();
        let save = r.state.to_save(&r.doc, &r.view, None);
        assert!(save.data.is_some());
        r.state.saved(&r.doc, &r.view, save);
        r.state.ensure_stored();
        assert!(r.state.to_save(&r.doc, &r.view, None).is_empty(), "stored once is enough");
    }

    #[test]
    fn a_photo_never_edited_starts_with_its_custom_lens_and_is_not_saved_for_it() {
        let reg = registry();
        let lens = LensSettings { vignetting: false, custom: Some("Mine".into()), ..LensSettings::default() };
        let r = reopen(opened(), 1, &reg, 1, None, Some(lens.clone()));
        assert_eq!(r.doc.recipe().develop.lens, lens);
        assert!(r.doc.history().is_empty(), "not an edit of the user");
        assert!(r.state.to_save(&r.doc, &r.view, None).is_empty(), "nothing to save until edited");
    }

    #[test]
    fn a_new_variant_is_written_at_the_next_save() {
        let reg = registry();
        let r = reopen(opened(), 1, &reg, 1, None, None);
        let state = VariantState::unsaved(3, variant_name(3));
        assert!(state.is_dirty(&r.doc, &r.view));
        let save = state.to_save(&r.doc, &r.view, None);
        assert_eq!(save.info.number, 3);
        assert!(save.data.unwrap().contains("\"name\":\"Variant 3\""));
    }

    #[test]
    fn the_list_has_the_open_variant_and_deleting_picks_a_neighbour() {
        let v = |number| VariantInfo { number, name: variant_name(number), format: 1 };
        assert_eq!(variant_list(&[], Some(&v(1))), vec![v(1)]);
        assert_eq!(variant_list(&[v(3), v(1)], Some(&v(2))), vec![v(1), v(2), v(3)]);
        assert_eq!(variant_list(&[v(1), v(2)], Some(&v(2))), vec![v(1), v(2)]);
        assert_eq!(variant_list(&[v(1)], None), vec![v(1)]);
        let list = [v(1), v(3), v(4)];
        assert_eq!(after_delete(&list, 3), Some(1));
        assert_eq!(after_delete(&list, 1), Some(3));
        assert_eq!(after_delete(&[v(2)], 2), None);
    }
}
