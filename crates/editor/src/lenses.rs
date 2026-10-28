//! The lenses offered for lens corrections: every lens of the database with
//! correction data, those that fit the camera's mount first, the others by
//! maker; filtered as the user types.

use lens_db::{Camera, Lens, LensDatabase, loose_eq};

/// One lens in the chooser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensChoice {
    /// Full name, as the recipe stores it ("Sony FE 24-70mm f/2.8 GM").
    pub name: String,
    /// Maker, for grouping the lenses of other mounts.
    pub maker: String,
    /// Mounts, shown next to lenses of other mounts.
    pub mounts: String,
    /// It fits the camera's mount (or a mount the database calls compatible).
    pub compatible: bool,
}

/// Every lens with correction data, compatible lenses first; each part
/// sorted by maker, then name. Without a known camera, none is compatible.
#[must_use]
pub fn lens_choices(db: &LensDatabase, camera: Option<&Camera>) -> Vec<LensChoice> {
    let compat: Vec<&str> = camera.map(|c| db.compatible_mounts(&c.mount)).unwrap_or_default();
    let fits = |l: &Lens| {
        camera.is_some_and(|c| l.mounts.iter().any(|m| loose_eq(m, &c.mount) || compat.iter().any(|x| loose_eq(m, x))))
    };
    let mut out: Vec<LensChoice> = db
        .lenses()
        .iter()
        .filter(|l| l.has_distortion() || l.has_tca() || l.has_vignetting())
        .map(|l| LensChoice {
            name: l.full_name(),
            maker: l.maker.primary().to_owned(),
            mounts: l.mounts.join(", "),
            compatible: fits(l),
        })
        .collect();
    out.sort_by(|a, b| {
        b.compatible
            .cmp(&a.compatible)
            .then_with(|| a.maker.to_lowercase().cmp(&b.maker.to_lowercase()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    // The same lens on several mounts appears once, compatible if any is.
    out.dedup_by(|b, a| a.name.eq_ignore_ascii_case(&b.name) && a.compatible == b.compatible);
    let compatible: Vec<String> = out.iter().filter(|c| c.compatible).map(|c| c.name.to_lowercase()).collect();
    out.retain(|c| c.compatible || !compatible.contains(&c.name.to_lowercase()));
    out
}

/// The choices whose name (or mounts) contain every word of `text`, as
/// people type lens names: case, `/` and the decimal comma apart ("FE
/// 24-70mm F2.8" finds "Sony FE 24-70mm f/2.8 GM"). Only compatible ones
/// when `compatible_only` and some choice is compatible (without a known
/// camera none is, and the tick would hide every lens).
#[must_use]
pub fn filter<'a>(choices: &'a [LensChoice], text: &str, compatible_only: bool) -> Vec<&'a LensChoice> {
    let words: Vec<String> = text.split_whitespace().map(searchable).filter(|w| !w.is_empty()).collect();
    let only = compatible_only && choices.iter().any(|c| c.compatible);
    choices
        .iter()
        .filter(|c| !only || c.compatible)
        .filter(|c| {
            let hay = searchable(&format!("{} {} {}", c.name, c.maker, c.mounts));
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// `text` as the search compares it: lower case, without `/`, with `.`
/// for a decimal comma.
#[must_use]
pub fn searchable(text: &str) -> String {
    text.to_lowercase().replace('/', "").replace(',', ".")
}

/// `choices` (as [`filter`] returns them) in groups with a title: the
/// compatible lenses, then the other mounts' lenses by maker.
#[must_use]
pub fn groups<'a>(choices: &[&'a LensChoice]) -> Vec<(String, Vec<&'a LensChoice>)> {
    let mut out: Vec<(String, Vec<&'a LensChoice>)> = Vec::new();
    for c in choices {
        let title =
            if c.compatible { "Fits this camera".to_owned() } else { format!("Other mounts \u{b7} {}", c.maker) };
        match out.last_mut() {
            Some((t, list)) if *t == title => list.push(c),
            _ => out.push((title, vec![c])),
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn choice(name: &str, maker: &str, compatible: bool) -> LensChoice {
        LensChoice { name: name.into(), maker: maker.into(), mounts: "Sony E".into(), compatible }
    }

    #[test]
    fn typing_filters_by_every_word_and_the_tick_keeps_compatible_lenses() {
        let all = vec![
            choice("Sony FE 24-70mm f/2.8 GM", "Sony", true),
            choice("Sigma 35mm f/1.4 DG HSM Art", "Sigma", true),
            choice("Canon EF 50mm f/1.8 STM", "Canon", false),
        ];
        assert_eq!(filter(&all, "", false).len(), 3);
        assert_eq!(filter(&all, "", true).len(), 2);
        let found = filter(&all, "50 CANON", false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].maker, "Canon");
        assert!(filter(&all, "50 canon", true).is_empty());
        // As the files and people write it.
        assert_eq!(filter(&all, "FE 24-70mm F2.8", true).len(), 1);
        assert_eq!(filter(&all, "f/1,4 art", false).len(), 1);
        // Without a camera nothing is compatible: the tick hides nothing.
        let unknown: Vec<LensChoice> = all.iter().map(|c| LensChoice { compatible: false, ..c.clone() }).collect();
        assert_eq!(filter(&unknown, "50", true).len(), 1);
        let g = groups(&filter(&all, "", false));
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "Fits this camera");
        assert_eq!(g[1].0, "Other mounts \u{b7} Canon");
    }

    #[test]
    fn the_database_offers_every_lens_with_the_fitting_ones_first() {
        let db = LensDatabase::builtin();
        let camera = db.find_camera("Sony", "ILCE-7M3");
        let list = lens_choices(db, camera);
        assert!(list.len() > 500, "{}", list.len());
        let first_other = list.iter().position(|c| !c.compatible).unwrap();
        assert!(first_other > 10, "the E-mount lenses come first");
        assert!(list[first_other..].iter().all(|c| !c.compatible));
        assert!(list.iter().any(|c| !c.compatible && c.maker.eq_ignore_ascii_case("Canon")));
        // Without a camera every lens is offered, none as fitting.
        assert!(lens_choices(db, None).iter().all(|c| !c.compatible));
    }
}
