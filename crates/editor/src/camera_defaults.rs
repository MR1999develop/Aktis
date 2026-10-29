//! Tool defaults per camera model, as Capture One's Save as Defaults: one
//! `.rnedefaults` file per camera in the editor's `Defaults` folder. A photo
//! of that camera never edited starts with the saved nodes; a node of a
//! saved tool starts with the saved values; resetting goes back to them.

use std::path::{Path, PathBuf};

use commands::{Edit, EditSession};
use core_model::{CameraDefaults, LensSettings, ParamId, ParamValue, Params, Recipe, Style, StyleNode, ToolId};
use engine::export;
use recipe_store::{CAMERA_DEFAULTS_EXTENSION, decode_camera_defaults, encode_camera_defaults};
use tool_api::ToolRegistry;

/// Sub-folder of the editor's data folder.
pub const FOLDER_NAME: &str = "Defaults";

/// The folder used on this computer.
#[must_use]
pub fn default_folder() -> Option<PathBuf> {
    platform::editor_data_dir().map(|d| d.join(FOLDER_NAME))
}

/// The recipe a photo never edited starts with: the default one, with the
/// settings of its custom lens (`lens`) and the nodes of its camera's
/// defaults (`nodes`, those this build has), in their order after the Source.
#[must_use]
pub fn start_recipe(registry: &ToolRegistry, lens: Option<LensSettings>, nodes: &[StyleNode]) -> Recipe {
    let mut recipe = Recipe::default();
    if let Some(lens) = lens {
        recipe.develop.lens = lens;
    }
    let style = Style { nodes: nodes.to_vec(), ..Style::default() };
    let (style, _) = crate::presets::usable(&style, registry);
    if style.nodes.is_empty() {
        return recipe;
    }
    let mut session = EditSession::new(recipe.clone());
    match session.apply(registry, Edit::ApplyStyle { name: "Camera defaults".to_owned(), style: Box::new(style) }) {
        Ok(()) => session.recipe().clone(),
        Err(_) => recipe,
    }
}

/// The camera defaults of a folder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AllCameraDefaults {
    folder: Option<PathBuf>,
    /// With the file each came from.
    cameras: Vec<(CameraDefaults, PathBuf)>,
    /// Files that could not be read.
    problems: Vec<String>,
}

impl AllCameraDefaults {
    /// Reads every file of `folder` (none when it is `None` or missing).
    #[must_use]
    pub fn load(folder: Option<PathBuf>) -> Self {
        let mut cameras = Vec::new();
        let mut problems = Vec::new();
        let entries = folder.as_deref().and_then(|d| std::fs::read_dir(d).ok());
        for entry in entries.into_iter().flatten().flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(CAMERA_DEFAULTS_EXTENSION)) {
                continue;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| decode_camera_defaults(&t).map_err(|e| e.to_string()))
            {
                Ok(d) => cameras.push((d, path)),
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
        Self { folder, cameras, problems }
    }

    /// The defaults of `camera` (case and spacing apart), if any.
    #[must_use]
    pub fn of(&self, camera: &str) -> Option<&CameraDefaults> {
        self.cameras.iter().map(|(d, _)| d).find(|d| core_model::same_lens(&d.camera, camera))
    }

    /// The nodes a photo of `camera` never edited starts with.
    #[must_use]
    pub fn start_nodes(&self, camera: &str) -> Vec<StyleNode> {
        self.of(camera).map(|d| d.nodes.clone()).unwrap_or_default()
    }

    /// The saved values of `tool` for `camera`.
    #[must_use]
    pub fn params(&self, camera: &str, tool: &ToolId) -> Option<&Params> {
        self.of(camera).and_then(|d| d.params(tool))
    }

    /// The saved value of one parameter, to reset to.
    #[must_use]
    pub fn value(&self, camera: &str, tool: &ToolId, param: &ParamId) -> Option<ParamValue> {
        self.params(camera, tool).and_then(|p| p.get(param))
    }

    /// Files that could not be read, to tell the user.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Saves `params` as the defaults of `tool` for `camera`.
    ///
    /// # Errors
    /// A message when there is no folder or the file cannot be written;
    /// nothing changes then.
    pub fn save(&mut self, camera: &str, tool: ToolId, params: Params) -> Result<(), String> {
        let mut defaults = self.of(camera).cloned().unwrap_or_else(|| CameraDefaults::new(camera.trim()));
        defaults.set(tool, params);
        self.write(defaults)
    }

    /// Forgets the defaults of `tool` for `camera` (the file goes when it is empty).
    ///
    /// # Errors
    /// A message when the file cannot be written or removed; nothing changes then.
    pub fn remove(&mut self, camera: &str, tool: &ToolId) -> Result<(), String> {
        let Some(mut defaults) = self.of(camera).cloned() else { return Ok(()) };
        if !defaults.remove(tool) {
            return Ok(());
        }
        if !defaults.is_empty() {
            return self.write(defaults);
        }
        let Some(i) = self.position(camera) else { return Ok(()) };
        let path = &self.cameras[i].1;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        self.cameras.remove(i);
        Ok(())
    }

    fn write(&mut self, defaults: CameraDefaults) -> Result<(), String> {
        let folder = self.folder.clone().ok_or("there is no folder for camera defaults on this computer")?;
        std::fs::create_dir_all(&folder).map_err(|e| format!("the folder {} cannot be made: {e}", folder.display()))?;
        let old = self.position(&defaults.camera);
        let path = match old {
            Some(i) => self.cameras[i].1.clone(),
            None => free_path(&folder, &defaults.camera),
        };
        export::write_atomic(&path, encode_camera_defaults(&defaults).as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        match old {
            Some(i) => self.cameras[i] = (defaults, path),
            None => self.cameras.push((defaults, path)),
        }
        Ok(())
    }

    fn position(&self, camera: &str) -> Option<usize> {
        self.cameras.iter().position(|(d, _)| core_model::same_lens(&d.camera, camera))
    }
}

/// A file for the defaults of a new camera that replaces nothing.
fn free_path(folder: &Path, camera: &str) -> PathBuf {
    let stem = Some(export::sanitize_stem(camera)).filter(|s| !s.is_empty()).unwrap_or_else(|| "Camera".into());
    export::non_destructive_path(&folder.join(format!("{stem}.{CAMERA_DEFAULTS_EXTENSION}")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolId {
        ToolId::new(name).unwrap()
    }

    #[test]
    fn a_photo_never_edited_starts_with_its_camera_s_nodes_and_custom_lens() {
        let reg = engine::builtin_tools().unwrap();
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.5));
        let nodes = vec![
            StyleNode { tool: tool("exposure"), params: p },
            StyleNode { tool: tool("no_such_tool"), params: Params::new() },
        ];
        let lens = LensSettings { vignetting: false, ..LensSettings::default() };
        let r = start_recipe(&reg, Some(lens.clone()), &nodes);
        assert_eq!(r.develop.lens, lens);
        let tools: Vec<_> = core_model::tool_nodes(&r.graph).into_iter().filter_map(|id| r.graph.node(id)).collect();
        assert_eq!(tools.len(), 1, "unknown tools are left out");
        assert_eq!(tools[0].params.get(&ParamId::new("exposure").unwrap()), Some(ParamValue::Float(0.5)));
        assert_eq!(start_recipe(&reg, None, &[]), Recipe::default());
    }

    #[test]
    fn defaults_are_saved_per_camera_found_again_and_removed() {
        let dir = std::env::temp_dir().join(format!("rne-camera-defaults-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut all = AllCameraDefaults::load(Some(dir.clone()));
        let exposure = ParamId::new("exposure").unwrap();
        let mut p = Params::new();
        p.set(exposure.clone(), ParamValue::Float(0.5));
        all.save("Sony ILCE-7M3", tool("exposure"), p.clone()).unwrap();
        all.save("Sony ILCE-7M3", tool("dehaze"), Params::new()).unwrap();
        all.save("Canon EOS R5", tool("exposure"), Params::new()).unwrap();
        assert_eq!(all.value("sony  ilce-7m3", &tool("exposure"), &exposure), Some(ParamValue::Float(0.5)));
        assert_eq!(all.start_nodes("Sony ILCE-7M3").len(), 2);
        assert!(all.start_nodes("Nikon Z 8").is_empty());

        let again = AllCameraDefaults::load(Some(dir.clone()));
        assert_eq!(again.of("Sony ILCE-7M3"), all.of("Sony ILCE-7M3"));
        assert_eq!(again.start_nodes("Canon EOS R5").len(), 1);

        all.remove("Canon EOS R5", &tool("exposure")).unwrap();
        assert!(all.of("Canon EOS R5").is_none(), "an empty camera goes");
        all.remove("Sony ILCE-7M3", &tool("dehaze")).unwrap();
        assert_eq!(AllCameraDefaults::load(Some(dir.clone())).start_nodes("Sony ILCE-7M3").len(), 1);
        std::fs::write(dir.join("broken.rnedefaults"), "{").unwrap();
        assert_eq!(AllCameraDefaults::load(Some(dir.clone())).problems().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
