//! The lensfun data model: mounts, cameras, lenses and their calibrations,
//! and how each is read from the XML tree.

use crate::xml::Element;

/// A name with its translations. The first variant is the untranslated one
/// (for cameras usually the EXIF text, "Nikon Corporation"); the others are
/// the localised forms in the order of the file ("Nikon").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    variants: Vec<String>,
}

impl Name {
    /// A name without translations.
    #[must_use]
    pub fn new(primary: impl Into<String>) -> Self {
        Self { variants: vec![primary.into()] }
    }

    /// The untranslated name.
    #[must_use]
    pub fn primary(&self) -> &str {
        &self.variants[0]
    }

    /// The shortest form, for display ("D3S" rather than "Nikon D3S").
    #[must_use]
    pub fn display(&self) -> &str {
        self.variants.iter().min_by_key(|v| v.chars().count()).map_or("", String::as_str)
    }

    /// Every form, untranslated first.
    pub fn variants(&self) -> impl Iterator<Item = &str> {
        self.variants.iter().map(String::as_str)
    }

    /// `true` when `text` equals one of the forms, ignoring case and runs of spaces.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        self.variants.iter().any(|v| loose_eq(v, text))
    }
}

/// Lensfun's name equality: case-insensitive, leading/trailing spaces ignored,
/// runs of white space count as one space.
#[must_use]
pub fn loose_eq(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
    norm(a) == norm(b)
}

/// A lens mount and the mounts whose lenses fit on it with an adapter-free
/// or standard adapter ("Sony E" accepts "Sony A" lenses through LA-EA).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Mount name as used by cameras and lenses ("Canon EF").
    pub name: String,
    /// Mounts whose lenses also fit.
    pub compat: Vec<String>,
}

/// A camera body.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// Manufacturer.
    pub maker: Name,
    /// Model.
    pub model: Name,
    /// Firmware or region variant, when the database distinguishes them.
    pub variant: Option<String>,
    /// Lens mount.
    pub mount: String,
    /// Diagonal of a 36×24 mm frame divided by the sensor's diagonal.
    pub crop_factor: f32,
}

/// Projection of a lens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensType {
    /// Straight lines stay straight (almost every lens).
    Rectilinear,
    /// Equidistant fisheye.
    Fisheye,
    /// Cylindrical panorama.
    Panoramic,
    /// Equirectangular panorama.
    Equirectangular,
    /// Orthographic fisheye.
    FisheyeOrthographic,
    /// Stereographic fisheye.
    FisheyeStereographic,
    /// Equisolid (equal-area) fisheye.
    FisheyeEquisolid,
    /// Fisheye following Thoby's formula (Nikkor 10.5 mm).
    FisheyeThoby,
}

impl LensType {
    fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "rectilinear" => Self::Rectilinear,
            "fisheye" => Self::Fisheye,
            "panoramic" => Self::Panoramic,
            "equirectangular" => Self::Equirectangular,
            "orthographic" => Self::FisheyeOrthographic,
            "stereographic" => Self::FisheyeStereographic,
            "equisolid" => Self::FisheyeEquisolid,
            "fisheye_thoby" => Self::FisheyeThoby,
            _ => return None,
        })
    }
}

/// Distortion models of the database (Hugin/PanoTools conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistortionModel {
    /// `Rd = Ru · (1 − k1 + k1·Ru²)`; terms `[k1, 0, 0]`.
    Poly3,
    /// `Rd = Ru · (1 + k1·Ru² + k2·Ru⁴)`; terms `[k1, k2, 0]`.
    Poly5,
    /// `Rd = Ru · (a·Ru³ + b·Ru² + c·Ru + 1 − a − b − c)`; terms `[a, b, c]`.
    PtLens,
}

/// Distortion measured at one focal length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distortion {
    /// The model the terms belong to.
    pub model: DistortionModel,
    /// Nominal focal length in mm.
    pub focal: f32,
    /// Real focal length in mm (measured, or derived from the terms).
    pub real_focal: f32,
    /// Model coefficients, see [`DistortionModel`].
    pub terms: [f32; 3],
}

/// Transverse chromatic aberration models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcaModel {
    /// Red and blue scaled by a constant; terms `[kr, kb, 0, 0, 0, 0]`.
    Linear,
    /// `Rd = Ru · (b·Ru² + c·Ru + v)` for red and blue; terms `[vr, vb, cr, cb, br, bb]`.
    Poly3,
}

/// Chromatic aberration measured at one focal length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tca {
    /// The model the terms belong to.
    pub model: TcaModel,
    /// Nominal focal length in mm.
    pub focal: f32,
    /// Model coefficients, see [`TcaModel`].
    pub terms: [f32; 6],
}

/// Vignetting measured at one focal length, aperture and distance, with the
/// PanoTools model `c = 1 + k1·r² + k2·r⁴ + k3·r⁶` (r = 1 at the corner).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vignetting {
    /// Nominal focal length in mm.
    pub focal: f32,
    /// F-number.
    pub aperture: f32,
    /// Focus distance in metres.
    pub distance: f32,
    /// `[k1, k2, k3]`.
    pub terms: [f32; 3],
}

/// Calibration data measured on one sensor format.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationSet {
    /// Crop factor of the camera used for the measurements.
    pub crop_factor: f32,
    /// Aspect ratio (long side / short side) of that camera's frames.
    pub aspect_ratio: f32,
    /// Distortion samples, any order.
    pub distortion: Vec<Distortion>,
    /// Chromatic aberration samples, any order.
    pub tca: Vec<Tca>,
    /// Vignetting samples, any order.
    pub vignetting: Vec<Vignetting>,
}

impl CalibrationSet {
    fn is_empty(&self) -> bool {
        self.distortion.is_empty() && self.tca.is_empty() && self.vignetting.is_empty()
    }
}

/// A lens and its calibration data.
#[derive(Debug, Clone, PartialEq)]
pub struct Lens {
    /// Manufacturer.
    pub maker: Name,
    /// Model.
    pub model: Name,
    /// Mounts the lens exists for.
    pub mounts: Vec<String>,
    /// Projection.
    pub lens_type: LensType,
    /// Shortest and longest focal length in mm (0 when unknown).
    pub focal_range: [f32; 2],
    /// Widest and narrowest f-number (0 when unknown).
    pub aperture_range: [f32; 2],
    /// Optical centre offset, in units of half the shorter image side.
    pub center: [f32; 2],
    /// Measurements, one set per sensor format.
    pub calibrations: Vec<CalibrationSet>,
}

impl Lens {
    /// "Maker Model", without repeating the maker when the model starts with it.
    #[must_use]
    pub fn full_name(&self) -> String {
        let maker = self.maker.primary();
        let model = self.model.primary();
        if model.to_lowercase().starts_with(&maker.to_lowercase()) || maker.is_empty() {
            model.to_owned()
        } else {
            format!("{maker} {model}")
        }
    }

    /// `true` when there is any distortion data.
    #[must_use]
    pub fn has_distortion(&self) -> bool {
        self.calibrations.iter().any(|c| !c.distortion.is_empty())
    }

    /// `true` when there is any chromatic aberration data.
    #[must_use]
    pub fn has_tca(&self) -> bool {
        self.calibrations.iter().any(|c| !c.tca.is_empty())
    }

    /// `true` when there is any vignetting data.
    #[must_use]
    pub fn has_vignetting(&self) -> bool {
        self.calibrations.iter().any(|c| !c.vignetting.is_empty())
    }
}

/// The contents of one database file.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Document {
    pub mounts: Vec<Mount>,
    pub cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    /// Entries that were skipped, with the reason.
    pub warnings: Vec<String>,
}

/// Reads a `<lensdatabase>` element.
pub(crate) fn read_document(root: &Element) -> Result<Document, String> {
    if root.name != "lensdatabase" {
        return Err(format!("expected <lensdatabase>, found <{}>", root.name));
    }
    let mut doc = Document::default();
    for el in &root.children {
        match el.name.as_str() {
            "mount" => match read_mount(el) {
                Ok(m) => doc.mounts.push(m),
                Err(e) => doc.warnings.push(format!("mount skipped: {e}")),
            },
            "camera" => match read_camera(el) {
                Ok(c) => doc.cameras.push(c),
                Err(e) => doc.warnings.push(format!("camera skipped: {e}")),
            },
            "lens" => match read_lens(el, &mut doc.warnings) {
                Ok(l) => doc.lenses.push(l),
                Err(e) => doc.warnings.push(format!("lens skipped: {e}")),
            },
            _ => {}
        }
    }
    Ok(doc)
}

fn read_name(el: &Element, tag: &str) -> Result<Name, String> {
    let mut untranslated = Vec::new();
    let mut translated = Vec::new();
    for e in el.children_named(tag) {
        let text = e.text.trim();
        if text.is_empty() {
            continue;
        }
        let list = if e.attr("lang").is_some() { &mut translated } else { &mut untranslated };
        list.push(text.to_owned());
    }
    untranslated.extend(translated);
    if untranslated.is_empty() {
        return Err(format!("no <{tag}>"));
    }
    untranslated.dedup();
    Ok(Name { variants: untranslated })
}

fn text_of<'a>(el: &'a Element, tag: &str) -> Option<&'a str> {
    el.child(tag).map(|c| c.text.trim()).filter(|t| !t.is_empty())
}

/// A positive, finite number.
fn positive(text: &str, what: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("{what} '{text}' is not a positive number"))
}

/// A finite number.
fn finite(text: &str, what: &str) -> Result<f32, String> {
    text.trim().parse::<f32>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("{what} '{text}' is not a number"))
}

/// "4:3" or "1.333".
fn aspect_ratio(text: &str) -> Result<f32, String> {
    let ratio = match text.split_once(':') {
        Some((a, b)) => positive(a, "aspect ratio")? / positive(b, "aspect ratio")?,
        None => positive(text, "aspect ratio")?,
    };
    // Stored as long side over short side.
    Ok(if ratio < 1.0 { 1.0 / ratio } else { ratio })
}

fn read_mount(el: &Element) -> Result<Mount, String> {
    let name = text_of(el, "name").ok_or("no <name>")?.to_owned();
    let compat = el.children_named("compat").map(|c| c.text.trim().to_owned()).filter(|c| !c.is_empty()).collect();
    Ok(Mount { name, compat })
}

fn read_camera(el: &Element) -> Result<Camera, String> {
    let maker = read_name(el, "maker")?;
    let model = read_name(el, "model")?;
    let who = || format!("{} {}", maker.primary(), model.primary());
    let mount = text_of(el, "mount").ok_or_else(|| format!("{}: no <mount>", who()))?.to_owned();
    let crop_factor =
        positive(text_of(el, "cropfactor").ok_or_else(|| format!("{}: no <cropfactor>", who()))?, "crop factor")
            .map_err(|e| format!("{}: {e}", who()))?;
    let variant = text_of(el, "variant").map(str::to_owned);
    Ok(Camera { maker, model, variant, mount, crop_factor })
}

fn read_lens(el: &Element, warnings: &mut Vec<String>) -> Result<Lens, String> {
    let maker = read_name(el, "maker").unwrap_or_else(|_| Name::new(""));
    let model = read_name(el, "model")?;
    let who = format!("{} {}", maker.primary(), model.primary()).trim().to_owned();
    let err = |e: String| format!("{who}: {e}");

    let mounts: Vec<String> =
        el.children_named("mount").map(|m| m.text.trim().to_owned()).filter(|m| !m.is_empty()).collect();
    let lens_type = match text_of(el, "type") {
        Some(t) => LensType::parse(t).ok_or_else(|| err(format!("unknown lens type '{t}'")))?,
        None => LensType::Rectilinear,
    };
    // Defaults as in lensfun: full frame, 3:2.
    let crop_factor = text_of(el, "cropfactor").map_or(Ok(1.0), |t| positive(t, "crop factor")).map_err(err)?;
    let aspect = text_of(el, "aspect-ratio").map_or(Ok(1.5), aspect_ratio).map_err(err)?;

    let mut focal_range = [0.0_f32; 2];
    if let Some(f) = el.child("focal") {
        if let Some(v) = f.attr("value") {
            let v = positive(v, "focal length").map_err(err)?;
            focal_range = [v, v];
        }
        if let Some(v) = f.attr("min") {
            focal_range[0] = positive(v, "focal length").map_err(err)?;
        }
        if let Some(v) = f.attr("max") {
            focal_range[1] = positive(v, "focal length").map_err(err)?;
        }
    }
    let mut aperture_range = [0.0_f32; 2];
    if let Some(a) = el.child("aperture") {
        if let Some(v) = a.attr("min") {
            aperture_range[0] = positive(v, "aperture").map_err(err)?;
        }
        if let Some(v) = a.attr("max") {
            aperture_range[1] = positive(v, "aperture").map_err(err)?;
        }
    }
    let center = match el.child("center") {
        Some(c) => [
            c.attr("x").map_or(Ok(0.0), |v| finite(v, "centre")).map_err(err)?,
            c.attr("y").map_or(Ok(0.0), |v| finite(v, "centre")).map_err(err)?,
        ],
        None => [0.0, 0.0],
    };

    let mut calibrations: Vec<CalibrationSet> = Vec::new();
    for cal in el.children_named("calibration") {
        let crop = cal.attr("cropfactor").map_or(Ok(crop_factor), |t| positive(t, "crop factor")).map_err(err)?;
        let aspect = cal.attr("aspect-ratio").map_or(Ok(aspect), aspect_ratio).map_err(err)?;
        // Calibration elements with the same sensor format form one set, as in lensfun.
        let index = match calibrations
            .iter()
            .position(|s| (s.crop_factor - crop).abs() < 1e-4 && (s.aspect_ratio - aspect).abs() < 1e-4)
        {
            Some(i) => i,
            None => {
                calibrations.push(CalibrationSet {
                    crop_factor: crop,
                    aspect_ratio: aspect,
                    distortion: Vec::new(),
                    tca: Vec::new(),
                    vignetting: Vec::new(),
                });
                calibrations.len() - 1
            }
        };
        let set = &mut calibrations[index];
        for item in &cal.children {
            let result = match item.name.as_str() {
                "distortion" => read_distortion(item).map(|d| d.map(|d| set.distortion.push(d))),
                "tca" => read_tca(item).map(|t| t.map(|t| set.tca.push(t))),
                "vignetting" => read_vignetting(item).map(|v| v.map(|v| set.vignetting.push(v))),
                _ => Ok(None),
            };
            if let Err(e) = result {
                warnings.push(format!("{who}: <{}> skipped: {e}", item.name));
            }
        }
    }
    calibrations.retain(|s| !s.is_empty());
    for set in &mut calibrations {
        keep_one_model(&mut set.distortion, |d| d.model, &who, "distortion", warnings);
        keep_one_model(&mut set.tca, |t| t.model, &who, "TCA", warnings);
    }

    Ok(Lens { maker, model, mounts, lens_type, focal_range, aperture_range, center, calibrations })
}

/// Lensfun interpolates only between samples of the first model it meets.
fn keep_one_model<T, M: PartialEq + Copy>(
    items: &mut Vec<T>,
    model: impl Fn(&T) -> M,
    who: &str,
    what: &str,
    warnings: &mut Vec<String>,
) {
    if let Some(first) = items.first().map(&model) {
        let before = items.len();
        items.retain(|i| model(i) == first);
        if items.len() != before {
            warnings.push(format!("{who}: {what} samples of a second model ignored"));
        }
    }
}

fn attr_or(el: &Element, names: &[&str], default: f32) -> Result<f32, String> {
    match names.iter().find_map(|n| el.attr(n)) {
        Some(v) => finite(v, names[0]),
        None => Ok(default),
    }
}

/// `Ok(None)` for models that mean "no data" ("none") or that this build
/// does not implement ("acm"), which the database does not use.
fn read_distortion(el: &Element) -> Result<Option<Distortion>, String> {
    let model = match el.attr("model").unwrap_or("none") {
        "poly3" => DistortionModel::Poly3,
        "poly5" => DistortionModel::Poly5,
        "ptlens" => DistortionModel::PtLens,
        "none" | "acm" => return Ok(None),
        other => return Err(format!("unknown model '{other}'")),
    };
    let focal = positive(el.attr("focal").ok_or("no focal length")?, "focal length")?;
    let terms = [attr_or(el, &["a", "k1"], 0.0)?, attr_or(el, &["b", "k2"], 0.0)?, attr_or(el, &["c", "k3"], 0.0)?];
    let real_focal = match el.attr("real-focal") {
        Some(v) => positive(v, "real focal length")?,
        // Lensfun's estimate when the real focal length was not measured.
        None => match model {
            DistortionModel::PtLens => focal * (1.0 - terms[0] - terms[1] - terms[2]),
            DistortionModel::Poly3 => focal * (1.0 - terms[0]),
            DistortionModel::Poly5 => focal,
        },
    };
    if !(real_focal.is_finite() && real_focal > 0.0) {
        return Err("terms give a non-positive real focal length".to_owned());
    }
    Ok(Some(Distortion { model, focal, real_focal, terms }))
}

fn read_tca(el: &Element) -> Result<Option<Tca>, String> {
    let model = match el.attr("model").unwrap_or("none") {
        "linear" => TcaModel::Linear,
        "poly3" => TcaModel::Poly3,
        "none" | "acm" => return Ok(None),
        other => return Err(format!("unknown model '{other}'")),
    };
    let focal = positive(el.attr("focal").ok_or("no focal length")?, "focal length")?;
    let terms = [
        attr_or(el, &["kr", "vr"], 1.0)?,
        attr_or(el, &["kb", "vb"], 1.0)?,
        attr_or(el, &["cr"], 0.0)?,
        attr_or(el, &["cb"], 0.0)?,
        attr_or(el, &["br"], 0.0)?,
        attr_or(el, &["bb"], 0.0)?,
    ];
    Ok(Some(Tca { model, focal, terms }))
}

fn read_vignetting(el: &Element) -> Result<Option<Vignetting>, String> {
    match el.attr("model").unwrap_or("none") {
        "pa" => {}
        "none" | "acm" => return Ok(None),
        other => return Err(format!("unknown model '{other}'")),
    }
    let focal = positive(el.attr("focal").ok_or("no focal length")?, "focal length")?;
    let aperture = positive(el.attr("aperture").ok_or("no aperture")?, "aperture")?;
    // Lensfun files always give the distance; "far away" if one does not.
    let distance = el.attr("distance").map_or(Ok(1000.0), |d| positive(d, "distance"))?;
    let terms = [attr_or(el, &["k1"], 0.0)?, attr_or(el, &["k2"], 0.0)?, attr_or(el, &["k3"], 0.0)?];
    Ok(Some(Vignetting { focal, aperture, distance, terms }))
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // values are read verbatim
mod tests {
    use super::*;
    use crate::xml;

    const SAMPLE: &str = r#"
<lensdatabase version="2">
    <mount><name>Sony E</name><compat>Sony A</compat></mount>
    <camera>
        <maker>NIKON CORPORATION</maker>
        <maker lang="en">Nikon</maker>
        <model>NIKON D3S</model>
        <model lang="en">D3S</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1</cropfactor>
    </camera>
    <camera><maker>Broken</maker><model>NoMount</model><cropfactor>1</cropfactor></camera>
    <lens>
        <maker>Sony</maker>
        <model>FE 28-70mm f/3.5-5.6 OSS</model>
        <mount>Sony E</mount>
        <cropfactor>1.534</cropfactor>
        <aspect-ratio>4:3</aspect-ratio>
        <center x="0.01" y="-0.02"/>
        <calibration>
            <distortion model="ptlens" focal="28" a="0.01" b="-0.03" c="0"/>
            <distortion model="poly3" focal="35" k1="-0.01"/>
            <tca model="poly3" focal="28" vr="1.0002" br="0.0001"/>
            <vignetting model="pa" focal="28" aperture="3.5" distance="10" k1="-0.5" k2="0.2" k3="-0.1"/>
            <vignetting model="acm" focal="28" aperture="3.5" distance="10" k1="-0.5"/>
            <distortion model="wavy" focal="50" a="1"/>
        </calibration>
    </lens>
</lensdatabase>"#;

    fn sample() -> Document {
        read_document(&xml::parse(SAMPLE).expect("xml")).expect("database")
    }

    #[test]
    fn cameras_keep_every_name_form_and_bad_entries_are_reported() {
        let doc = sample();
        assert_eq!(doc.cameras.len(), 1);
        let c = &doc.cameras[0];
        assert!(c.maker.matches("Nikon") && c.maker.matches("nikon  corporation"));
        assert!(c.model.matches("D3S") && c.model.matches("NIKON D3S"));
        assert_eq!(c.model.display(), "D3S");
        assert_eq!((c.mount.as_str(), c.crop_factor), ("Nikon F AF", 1.0));
        assert!(doc.warnings.iter().any(|w| w.contains("NoMount")), "{:?}", doc.warnings);
        assert_eq!(doc.mounts[0].compat, vec!["Sony A".to_owned()]);
    }

    #[test]
    fn lenses_read_calibrations_with_lensfun_defaults() {
        let doc = sample();
        let l = &doc.lenses[0];
        assert_eq!(l.full_name(), "Sony FE 28-70mm f/3.5-5.6 OSS");
        assert_eq!(l.center, [0.01, -0.02]);
        let set = &l.calibrations[0];
        assert_eq!((set.crop_factor, set.aspect_ratio), (1.534, 4.0 / 3.0));
        // Only the first distortion model is kept; the unknown one is reported.
        assert_eq!(set.distortion.len(), 1);
        let d = set.distortion[0];
        assert_eq!(d.model, DistortionModel::PtLens);
        assert!((d.real_focal - 28.0 * 1.02).abs() < 1e-4, "{}", d.real_focal);
        assert!(doc.warnings.iter().any(|w| w.contains("second model")), "{:?}", doc.warnings);
        assert!(doc.warnings.iter().any(|w| w.contains("wavy")), "{:?}", doc.warnings);
        // TCA terms not given default to v = 1, c = b = 0.
        assert_eq!(set.tca[0].terms, [1.0002, 1.0, 0.0, 0.0, 0.0001, 0.0]);
        // The ACM vignetting entry is not used.
        assert_eq!(set.vignetting.len(), 1);
        assert_eq!(set.vignetting[0].terms, [-0.5, 0.2, -0.1]);
    }

    #[test]
    fn names_compare_loosely() {
        assert!(loose_eq("  Canon   EOS 5D ", "canon eos 5d"));
        assert!(!loose_eq("Canon EOS 5D", "Canon EOS 5D Mark II"));
    }
}
