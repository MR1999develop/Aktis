//! Choosing the camera profile and the lens correction for one image, from
//! its shooting data, the recipe and what is installed.

use core_color::{CameraNames, ColorProfile, ProfileLibrary};
use core_model::{CaptureInfo, DevelopSettings, ImageF32, LensSettings, ProfileChoice, SensorImage, WorkingSpace};
use lens_db::{Adjustments, Applied, Camera, CorrectionRequest, Lens, LensCorrection, LensDatabase};

use crate::{DevelopError, DevelopInputs};

/// A developed image and what was used for it.
#[derive(Debug)]
pub struct Developed {
    /// Linear image in the working space.
    pub image: ImageF32,
    /// The camera profile used, and a note when it is not the one asked for.
    pub profile: ProfileOutcome,
    /// The lens correction applied, and a note on what was not corrected.
    pub lens: LensOutcome,
}

/// Develops a RAW file's sensor data with the recipe's development settings:
/// chooses the profile (named ones from `library`) and the lens correction
/// (from `db`), then runs [`crate::develop_with`].
///
/// # Errors
/// [`DevelopError`] as [`crate::develop_with`].
pub fn develop_capture(
    sensor: &SensorImage,
    info: &CaptureInfo,
    settings: &DevelopSettings,
    space: WorkingSpace,
    db: &LensDatabase,
    library: Option<&ProfileLibrary>,
) -> Result<Developed, DevelopError> {
    let profile = choose_profile(info, &settings.profile, library);
    let lens = prepare_lens(db, info, &settings.lens, sensor.width(), sensor.height());
    let inputs = DevelopInputs { profile: profile.profile.as_ref(), lens: lens.correction.as_ref() };
    let image = crate::develop_with(sensor, settings, space, &inputs)?;
    Ok(Developed { image, profile, lens })
}

/// The colour profile to use, and why when it is not the one asked for.
#[derive(Debug, Clone)]
pub struct ProfileOutcome {
    /// The profile; `None` when the file has no usable matrix list, in which
    /// case the sensor's own matrix is used.
    pub profile: Option<ColorProfile>,
    /// A note for the user when the choice could not be honoured.
    pub note: Option<String>,
}

/// The profile for `choice`. A named profile missing from `library` (or with
/// no library) falls back to the camera's own matrices, with a note.
#[must_use]
pub fn choose_profile(info: &CaptureInfo, choice: &ProfileChoice, library: Option<&ProfileLibrary>) -> ProfileOutcome {
    let camera = format!("{} {}", info.clean_make, info.clean_model).trim().to_owned();
    let builtin = || ColorProfile::from_matrices(camera.clone(), &info.color_matrices).ok();
    match choice {
        ProfileChoice::Builtin => ProfileOutcome { profile: builtin(), note: None },
        ProfileChoice::Named(name) => {
            let names = camera_names(info);
            match library.and_then(|lib| lib.find(&names, name)) {
                Some(Ok(p)) => ProfileOutcome { profile: Some(p), note: None },
                Some(Err(e)) => ProfileOutcome {
                    profile: builtin(),
                    note: Some(format!("profile '{name}' cannot be used ({e}); built-in matrices used")),
                },
                None => ProfileOutcome {
                    profile: builtin(),
                    note: Some(format!("profile '{name}' is not installed for {camera}; built-in matrices used")),
                },
            }
        }
    }
}

/// The ways the camera of `info` may be named in a profile.
#[must_use]
pub fn camera_names(info: &CaptureInfo) -> CameraNames {
    CameraNames::new(&info.camera_make, &info.camera_model, &info.clean_make, &info.clean_model)
}

/// What the lens correction will do for an image.
#[derive(Debug, Clone)]
pub struct LensOutcome {
    /// The prepared correction; `None` when nothing will be corrected.
    pub correction: Option<LensCorrection>,
    /// Camera as found in the lens database.
    pub camera: Option<String>,
    /// Lens as found in the lens database.
    pub lens: Option<String>,
    /// What is corrected.
    pub applied: Applied,
    /// Why nothing (or less than asked) is corrected.
    pub note: Option<String>,
}

impl LensOutcome {
    fn none(camera: Option<&Camera>, lens: Option<&Lens>, note: impl Into<String>) -> Self {
        Self {
            correction: None,
            camera: camera.map(camera_name),
            lens: lens.map(Lens::full_name),
            applied: Applied::default(),
            note: Some(note.into()),
        }
    }
}

fn camera_name(c: &Camera) -> String {
    format!("{} {}", c.maker.display(), c.model.display())
}

/// The camera of `info` in `db`.
#[must_use]
pub fn find_camera<'a>(db: &'a LensDatabase, info: &CaptureInfo) -> Option<&'a Camera> {
    db.find_camera(&info.camera_make, &info.camera_model)
        .or_else(|| db.find_camera(&info.clean_make, &info.clean_model))
}

/// The lens of `info` in `db`: the one chosen by hand, else the best match
/// for the name in the file, else, for cameras with a fixed lens, the only
/// lens on the camera's mount.
#[must_use]
pub fn find_lens<'a>(
    db: &'a LensDatabase,
    camera: Option<&Camera>,
    info: &CaptureInfo,
    chosen: Option<&str>,
) -> Option<&'a Lens> {
    if let Some(name) = chosen {
        return db.lens_named(name, camera);
    }
    match info.lens_model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(model) => db.find_lenses(camera, info.lens_make.as_deref(), model).first().map(|m| m.lens),
        None => {
            let mount = &camera?.mount;
            let mut on_mount = db.lenses().iter().filter(|l| l.mounts.iter().any(|m| m == mount));
            match (on_mount.next(), on_mount.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
    }
}

/// Prepares the lens correction of a `width × height` sensor image.
/// When the database cannot help (unknown lens, camera or focal length) the
/// corrections set by hand are still applied, with a note on why the
/// database's are not.
#[must_use]
pub fn prepare_lens(
    db: &LensDatabase,
    info: &CaptureInfo,
    settings: &LensSettings,
    width: u32,
    height: u32,
) -> LensOutcome {
    if !settings.enabled {
        return LensOutcome::none(None, None, "lens corrections are switched off");
    }
    let adjust = adjustments(settings);
    let request = |crop_factor: f32, focal: f32| CorrectionRequest {
        crop_factor,
        focal,
        aperture: info.aperture.unwrap_or(0.0),
        distance: info.focus_distance.unwrap_or(1000.0),
        width,
        height,
        distortion: settings.distortion,
        tca: settings.tca,
        vignetting: settings.vignetting,
        adjust,
    };
    // Without database data: only what is set by hand.
    let by_hand = |camera: Option<&Camera>, lens: Option<&Lens>, why: String| {
        let mut out = LensOutcome::none(camera, lens, why);
        if adjust.has_manual()
            && let Ok(mut c) = LensCorrection::manual_only(&request(1.0, 1.0))
        {
            if let Some(z) = settings.zoom {
                c.set_zoom(z);
            }
            out.applied = c.applied();
            out.correction = (!out.applied.is_empty()).then_some(c);
            out.note = out.note.map(|n| format!("{n}; only the manual corrections are applied"));
        }
        out
    };
    let camera = find_camera(db, info);
    let lens = find_lens(db, camera, info, settings.lens.as_deref());
    let Some(lens) = lens else {
        let why = match (&settings.lens, &info.lens_model) {
            (Some(name), _) => format!("lens '{name}' is not in the lens database"),
            (None, Some(model)) => format!("lens '{model}' is not in the lens database"),
            (None, None) => "the file does not name the lens".to_owned(),
        };
        return by_hand(camera, None, why);
    };
    let Some(camera) = camera else {
        let why = format!(
            "camera {} {} is not in the lens database (its sensor size is needed)",
            info.clean_make, info.clean_model
        );
        return by_hand(None, Some(lens), why);
    };
    let Some(focal) = info.focal_length.filter(|f| *f > 0.0) else {
        return by_hand(Some(camera), Some(lens), "the file does not record the focal length".to_owned());
    };
    match LensCorrection::new(lens, &request(camera.crop_factor, focal)) {
        Err(e) => by_hand(Some(camera), Some(lens), e.to_string()),
        Ok(mut c) => {
            if let Some(z) = settings.zoom {
                c.set_zoom(z);
            }
            let applied = c.applied();
            let asked = [settings.distortion, settings.tca, settings.vignetting];
            let got = [applied.distortion, applied.tca, applied.vignetting];
            let missing: Vec<&str> = ["distortion", "chromatic aberration", "vignetting"]
                .iter()
                .zip(asked.iter().zip(got))
                .filter(|(_, (a, g))| **a && !g)
                .map(|(n, _)| *n)
                .collect();
            let note = (!missing.is_empty())
                .then(|| format!("no {} data for this lens at {focal} mm on this sensor", missing.join(", ")));
            LensOutcome {
                correction: (!applied.is_empty()).then_some(c),
                camera: Some(camera_name(camera)),
                lens: Some(lens.full_name()),
                applied,
                note,
            }
        }
    }
}

/// The amounts and manual corrections of `settings` for the lens database.
fn adjustments(settings: &LensSettings) -> Adjustments {
    let s = settings.clone().sanitized();
    Adjustments {
        distortion: s.amount.distortion,
        tca: s.amount.tca,
        vignetting: s.amount.vignetting,
        manual_distortion: s.manual.distortion,
        manual_vignetting: s.manual.vignetting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(lens: Option<&str>) -> CaptureInfo {
        CaptureInfo {
            camera_make: "SONY".to_owned(),
            camera_model: "ILCE-7RM5".to_owned(),
            clean_make: "Sony".to_owned(),
            clean_model: "ILCE-7RM5".to_owned(),
            lens_make: Some("Sony".to_owned()),
            lens_model: lens.map(str::to_owned),
            focal_length: Some(35.0),
            aperture: Some(4.0),
            focus_distance: None,
            color_matrices: Vec::new(),
        }
    }

    #[test]
    fn a_known_lens_is_found_and_prepared() {
        let db = LensDatabase::builtin();
        let out = prepare_lens(db, &info(Some("FE 24-70mm F2.8 GM")), &LensSettings::default(), 600, 400);
        assert!(out.correction.is_some(), "{out:?}");
        assert!(out.applied.distortion, "{out:?}");
        assert!(out.lens.as_deref().is_some_and(|l| l.contains("24-70mm f/2.8 GM")), "{out:?}");
    }

    #[test]
    fn missing_data_is_explained() {
        let db = LensDatabase::builtin();
        let off = prepare_lens(db, &info(Some("FE 24-70mm F2.8 GM")), &LensSettings::off(), 600, 400);
        assert!(off.correction.is_none() && off.note.is_some());
        let unknown = prepare_lens(db, &info(Some("Imaginary 13mm F0.7")), &LensSettings::default(), 600, 400);
        assert!(unknown.note.as_deref().is_some_and(|n| n.contains("not in the lens database")), "{unknown:?}");
        let unnamed = prepare_lens(db, &info(None), &LensSettings::default(), 600, 400);
        assert!(unnamed.correction.is_none(), "{unnamed:?}");
        let mut no_focal = info(Some("FE 24-70mm F2.8 GM"));
        no_focal.focal_length = None;
        let out = prepare_lens(db, &no_focal, &LensSettings::default(), 600, 400);
        assert!(out.note.as_deref().is_some_and(|n| n.contains("focal length")), "{out:?}");
    }

    #[test]
    fn manual_corrections_apply_to_a_lens_the_database_does_not_know() {
        let db = LensDatabase::builtin();
        let mut settings = LensSettings::default();
        settings.manual.vignetting = 0.5;
        let out = prepare_lens(db, &info(Some("Imaginary 13mm F0.7")), &settings, 600, 400);
        assert!(out.correction.is_some() && out.applied.vignetting && !out.applied.distortion, "{out:?}");
        assert!(out.note.as_deref().is_some_and(|n| n.contains("only the manual")), "{out:?}");
        // At 0 % nothing of the database's distortion correction is left.
        let known = info(Some("FE 24-70mm F2.8 GM"));
        let mut none = LensSettings::default();
        none.amount.distortion = 0.0;
        none.amount.tca = 0.0;
        none.amount.vignetting = 0.0;
        let out = prepare_lens(db, &known, &none, 600, 400);
        let mut c = out.correction.expect("the data is still there");
        c.set_zoom(1.0);
        for p in c.source(590.0, 390.0) {
            assert!((p[0] - 590.0).abs() < 1e-2 && (p[1] - 390.0).abs() < 1e-2, "{p:?}");
        }
        assert!((c.vignetting_gain(0.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_lens_chosen_by_hand_wins() {
        let db = LensDatabase::builtin();
        let settings = LensSettings { lens: Some("Sony FE 24-70mm f/2.8 GM".to_owned()), ..LensSettings::default() };
        let out = prepare_lens(db, &info(Some("Something else")), &settings, 600, 400);
        assert!(out.correction.is_some(), "{out:?}");
    }

    #[test]
    fn a_missing_named_profile_falls_back_with_a_note() {
        let mut i = info(None);
        i.color_matrices = vec![core_model::CalibratedMatrix {
            illuminant: core_model::Illuminant::D65,
            xyz_to_camera: core_model::Matrix3::from_rows([
                [0.82, -0.3, -0.07],
                [-0.43, 1.2, 0.25],
                [-0.04, 0.13, 0.58],
            ]),
        }];
        let out = choose_profile(&i, &ProfileChoice::Named("Adobe Standard".to_owned()), None);
        assert!(out.profile.is_some() && out.note.is_some(), "{out:?}");
        let out = choose_profile(&i, &ProfileChoice::Builtin, None);
        assert!(out.profile.is_some_and(|p| p.name == "Built-in") && out.note.is_none());
    }
}
