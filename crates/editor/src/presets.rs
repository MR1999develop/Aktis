//! Presets and style brushes, as Capture One has styles and style brushes.
//!
//! - A **preset** is a chosen part of a photo's edits ([`Style`]) saved in
//!   a file of its own ([`recipe_store::PRESET_EXTENSION`]); applying it
//!   to a photo sets the tools it holds (one undo step).
//! - A **style brush** is the same with a brush: choosing one arms the
//!   Masks tool, and the first stroke adds the brush's tools behind a new
//!   mask, named after the brush ([`recipe_store::BRUSH_EXTENSION`]).
//!
//! Both live in one folder the user chooses (default `Presets` in the
//! editor's data folder), sub-folders being groups. A few style brushes are
//! built in, in Capture One's three groups. There is no universal preset
//! format (Lightroom uses XMP, Capture One `.costyle`): ours is versioned
//! JSON written by `recipe-store`.

use std::path::{Path, PathBuf};

use core_model::{NodeId, NodeKind, ParamId, ParamValue, Params, Recipe, Style, StyleNode, StyleSelection, ToolId};
use engine::export;
use recipe_store::{BRUSH_EXTENSION, PRESET_EXTENSION};
pub use recipe_store::{BrushSettings, Preset};
use tool_api::ToolRegistry;

/// Name of the default presets folder inside the editor's data folder.
pub const FOLDER_NAME: &str = "Presets";

/// Sub-folder where saved style brushes go.
pub const BRUSH_FOLDER: &str = "Style Brushes";

/// Deepest sub-folder level searched for presets.
const MAX_DEPTH: usize = 4;

/// The presets folder used when the user has not chosen one.
#[must_use]
pub fn default_folder() -> Option<PathBuf> {
    platform::editor_data_dir().map(|d| d.join(FOLDER_NAME))
}

/// Where a preset comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Built into the editor.
    Builtin,
    /// A file in the presets folder.
    File(PathBuf),
}

/// One preset or style brush of the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Where it comes from.
    pub origin: Origin,
    /// Group: the sub-folder (`"Portraits/Skin"`), the built-in group, or
    /// empty for the top of the folder.
    pub group: String,
    /// The preset.
    pub preset: Preset,
}

/// The presets and style brushes available.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Library {
    /// Every preset and brush, built-in brushes first, then by group and name.
    pub entries: Vec<Entry>,
    /// Files that could not be read, with the reason.
    pub problems: Vec<String>,
}

impl Library {
    /// The built-in brushes and every preset file under `folder` (a missing
    /// folder is an empty one).
    #[must_use]
    pub fn scan(folder: Option<&Path>) -> Self {
        let mut lib = Self { entries: builtin_brushes(), problems: Vec::new() };
        let mut files = Vec::new();
        if let Some(root) = folder {
            collect(root, root, 0, &mut files, &mut lib.problems);
        }
        files.sort_by(|a, b| {
            (a.group.to_lowercase(), a.preset.name.to_lowercase())
                .cmp(&(b.group.to_lowercase(), b.preset.name.to_lowercase()))
        });
        lib.entries.extend(files);
        lib
    }

    /// The presets (applied to the whole photo).
    pub fn presets(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.preset.brush.is_none())
    }

    /// The style brushes.
    pub fn brushes(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.preset.brush.is_some())
    }
}

/// Adds the presets in `dir` (and below) to `out`.
fn collect(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Entry>, problems: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect(root, &path, depth + 1, out, problems);
            }
            continue;
        }
        if !is_preset_file(&path) {
            continue;
        }
        match load(&path) {
            Ok(preset) => {
                let group = path
                    .parent()
                    .and_then(|p| p.strip_prefix(root).ok())
                    .map(|rel| rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
                    .unwrap_or_default();
                out.push(Entry { origin: Origin::File(path), group, preset });
            }
            Err(e) => problems.push(e),
        }
    }
}

/// `true` for a file with a preset or style brush extension (any case).
#[must_use]
pub fn is_preset_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(PRESET_EXTENSION) || e.eq_ignore_ascii_case(BRUSH_EXTENSION))
}

/// Reads a preset file.
///
/// # Errors
/// A message naming the file when it cannot be read or is not a preset.
pub fn load(path: &Path) -> Result<Preset, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
    let mut preset = recipe_store::decode_preset(&text).map_err(|e| format!("{name}: {e}"))?;
    if preset.name.is_empty() {
        preset.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
    Ok(preset)
}

/// Writes `preset` into `folder` (style brushes into its
/// [`BRUSH_FOLDER`]), named after it; an existing file is never replaced
/// (`Warm (2).rnepreset`). Returns the file written.
///
/// # Errors
/// A message when the folder cannot be made or the file written.
pub fn save(folder: &Path, preset: &Preset) -> Result<PathBuf, String> {
    let dir = if preset.brush.is_some() { folder.join(BRUSH_FOLDER) } else { folder.to_owned() };
    std::fs::create_dir_all(&dir).map_err(|e| format!("the folder {} cannot be made: {e}", dir.display()))?;
    let stem = Some(export::sanitize_stem(&preset.name)).filter(|s| !s.is_empty()).unwrap_or_else(|| "Preset".into());
    let target = export::non_destructive_path(&dir.join(format!("{stem}.{}", preset.extension())));
    let text = recipe_store::encode_preset(preset);
    export::write_atomic(&target, text.as_bytes()).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target)
}

/// One tool node offered in the Save Preset window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeChoice {
    /// The node.
    pub node: NodeId,
    /// Its name (label, or the tool's name).
    pub name: String,
    /// It has a mask: the mask is not saved, only the values.
    pub masked: bool,
    /// The kind of tool, to group the window's list; `None` for a tool
    /// this build lacks.
    pub category: Option<tool_api::Category>,
}

/// The tool nodes of `recipe` that can go into a preset, in acting order.
#[must_use]
pub fn node_choices(recipe: &Recipe, registry: &ToolRegistry) -> Vec<NodeChoice> {
    let g = &recipe.graph;
    core_model::tool_nodes(g)
        .into_iter()
        .filter_map(|id| {
            let node = g.node(id)?;
            let NodeKind::Tool(tool) = &node.kind else { return None };
            let found = registry.get(tool);
            let tool_name = found.map_or_else(|| tool.as_str().to_owned(), |t| t.descriptor().name.to_owned());
            let category = found.map(|t| t.descriptor().category);
            let name = if node.label.is_empty() { tool_name } else { format!("{} ({tool_name})", node.label) };
            let masked = node
                .kind
                .inputs()
                .iter()
                .position(|k| *k == core_model::PortKind::Mask)
                .and_then(|p| u8::try_from(p).ok())
                .is_some_and(|p| g.input_link(id, p).is_some());
            Some(NodeChoice { node: id, name, masked, category })
        })
        .collect()
}

/// What the Save Preset window starts with: every tool node without a mask
/// (masks belong to one photo), no development settings or geometry.
#[must_use]
pub fn default_selection(choices: &[NodeChoice]) -> StyleSelection {
    StyleSelection {
        nodes: choices.iter().filter(|c| !c.masked).map(|c| c.node).collect(),
        ..StyleSelection::default()
    }
}

/// `style` without the tools this build lacks (a preset made by a newer
/// version); returns them, to tell the user.
#[must_use]
pub fn usable(style: &Style, registry: &ToolRegistry) -> (Style, Vec<ToolId>) {
    let mut kept = style.clone();
    let mut missing = Vec::new();
    kept.nodes.retain(|n| {
        let known = registry.get(&n.tool).is_some();
        if !known && !missing.contains(&n.tool) {
            missing.push(n.tool.clone());
        }
        known
    });
    (kept, missing)
}

// ----- built-in style brushes -----

/// Groups of the built-in brushes, as Capture One names them.
pub const BUILTIN_GROUPS: [&str; 3] = ["Light & Contrast", "Color", "Enhancements"];

fn node(tool: &str, values: &[(&str, ParamValue)]) -> StyleNode {
    let mut params = Params::new();
    for (id, v) in values {
        params.set(ParamId::new(id).expect("constant id is valid"), *v);
    }
    StyleNode { tool: ToolId::new(tool).expect("constant id is valid"), params }
}

fn brush(group: usize, name: &str, soft: bool, nodes: Vec<StyleNode>) -> Entry {
    let settings = if soft {
        BrushSettings { radius: 0.06, hardness: 0.1, opacity: 0.5, edge_aware: false }
    } else {
        BrushSettings { radius: 0.04, hardness: 0.4, opacity: 1.0, edge_aware: false }
    };
    Entry {
        origin: Origin::Builtin,
        group: BUILTIN_GROUPS[group].to_owned(),
        preset: Preset { name: name.to_owned(), style: Style { nodes, ..Style::default() }, brush: Some(settings) },
    }
}

/// The style brushes built into the editor.
#[must_use]
pub fn builtin_brushes() -> Vec<Entry> {
    use ParamValue::{Choice, Float};
    let exposure = |v: f32| node("exposure", &[("exposure", Float(v))]);
    let exposure_with = |id: &str, v: f32| node("exposure", &[(id, Float(v))]);
    let hdr = |id: &str, v: f32| node("hdr", &[(id, Float(v))]);
    let balance = |hue: f32| node("color_balance", &[("master_hue", Float(hue)), ("master_amount", Float(20.0))]);
    vec![
        brush(0, "Brighten", false, vec![exposure(0.5)]),
        brush(0, "Darken", false, vec![exposure(-0.5)]),
        brush(0, "Dodge", true, vec![exposure(0.3)]),
        brush(0, "Burn", true, vec![exposure(-0.3)]),
        brush(0, "Add Contrast", false, vec![exposure_with("contrast", 25.0)]),
        brush(0, "Reduce Contrast", false, vec![exposure_with("contrast", -25.0)]),
        brush(0, "Recover Highlights", false, vec![hdr("highlight", 50.0)]),
        brush(0, "Open Shadows", false, vec![hdr("shadow", 50.0)]),
        brush(1, "Saturate", false, vec![exposure_with("saturation", 25.0)]),
        brush(1, "Desaturate", false, vec![exposure_with("saturation", -50.0)]),
        brush(1, "Warm", false, vec![balance(45.0)]),
        brush(1, "Cool", false, vec![balance(225.0)]),
        brush(2, "Clarity", false, vec![node("clarity", &[("clarity", Float(30.0)), ("method", Choice(0))])]),
        brush(2, "Structure", false, vec![node("clarity", &[("structure", Float(30.0))])]),
        brush(2, "Soften", true, vec![node("clarity", &[("clarity", Float(-30.0)), ("structure", Float(-30.0))])]),
        brush(2, "Dehaze", false, vec![node("dehaze", &[("amount", Float(30.0))])]),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn registry() -> ToolRegistry {
        engine::builtin_tools().unwrap()
    }

    #[test]
    fn built_in_brushes_use_real_tools_and_parameters() {
        let reg = registry();
        for e in builtin_brushes() {
            assert!(e.preset.brush.is_some());
            assert!(BUILTIN_GROUPS.contains(&e.group.as_str()));
            for n in &e.preset.style.nodes {
                let tool = reg.get(&n.tool).unwrap_or_else(|| panic!("{}: {}", e.preset.name, n.tool));
                for (id, v) in n.params.iter() {
                    let p = tool.descriptor().param(id).unwrap_or_else(|| panic!("{}: {id}", e.preset.name));
                    assert_eq!(p.sanitize(*v), *v, "{}: {id} out of range", e.preset.name);
                }
            }
        }
    }

    #[test]
    fn saved_presets_are_found_by_group_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("rne-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let warm =
            Preset { name: "Warm: soft?".into(), style: builtin_brushes()[10].preset.style.clone(), brush: None };
        let a = save(&dir, &warm).unwrap();
        let b = save(&dir, &warm).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.file_name().unwrap().to_string_lossy(), "Warm_ soft_.rnepreset");
        let mut dodge = builtin_brushes()[2].preset.clone();
        dodge.name = "My dodge".into();
        let c = save(&dir, &dodge).unwrap();
        assert_eq!(c.parent().unwrap(), dir.join(BRUSH_FOLDER));
        std::fs::write(dir.join("broken.rnepreset"), "{").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a preset").unwrap();

        let lib = Library::scan(Some(&dir));
        let files: Vec<&Entry> = lib.entries.iter().filter(|e| matches!(e.origin, Origin::File(_))).collect();
        assert_eq!(files.len(), 3);
        assert_eq!(lib.presets().count(), 2);
        assert_eq!(lib.brushes().filter(|e| e.group == BRUSH_FOLDER).count(), 1);
        assert_eq!(lib.problems.len(), 1);
        assert!(lib.problems[0].starts_with("broken.rnepreset"));
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(Library::scan(None).entries.len(), builtin_brushes().len());
    }

    #[test]
    fn choices_leave_masked_nodes_out_by_default() {
        let reg = registry();
        let mut r = Recipe::default();
        let src = r.graph.source();
        let a = r.graph.insert_serial(src, ToolId::new("exposure").unwrap(), Params::new()).unwrap();
        let b = r.graph.insert_serial(a, ToolId::new("clarity").unwrap(), Params::new()).unwrap();
        let m = r.graph.add_mask_node(core_model::MaskDef::default());
        r.graph.connect(core_model::Link { from: m, from_port: 0, to: b, to_port: 1 }).unwrap();
        r.graph.node_mut(a).unwrap().set_label("Base".into());
        let choices = node_choices(&r, &reg);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].name, "Base (Exposure)");
        assert!(!choices[0].masked && choices[1].masked);
        assert_eq!(default_selection(&choices).nodes, vec![a]);
    }

    #[test]
    fn unknown_tools_are_dropped_and_named() {
        let mut style = builtin_brushes()[0].preset.style.clone();
        style.nodes.push(node("hologram", &[]));
        let (kept, missing) = usable(&style, &registry());
        assert_eq!(kept.nodes.len(), 1);
        assert_eq!(missing, vec![ToolId::new("hologram").unwrap()]);
    }
}
