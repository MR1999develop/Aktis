//! Finding a lens from the name a camera writes into the EXIF data.
//!
//! The rules follow lensfun's `lfDatabase::FindLenses`, so a lens is found
//! here exactly when other lensfun-based programs find it: the name is split
//! into words, every word of the EXIF name must appear in the database name,
//! focal lengths and apertures read from the name must agree, the mount must
//! fit the camera, and the lens must have been measured on a sensor no
//! larger than the camera's.

use crate::model::{Camera, Lens};

/// Splits a lens name into sorted, case-folded words as lensfun does:
/// digits (with decimal points, "50.0" → "50"), runs of punctuation and runs
/// of letters. Single punctuation marks and a lone "f" are dropped, except
/// "+" and "*", which matter in some names.
pub(crate) fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let first = chars[start];
        let mut strip = 0;
        if first.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i - start >= 2 && chars[i - 2] == '.' && chars[i - 1] == '0' {
                strip = 2;
            }
        } else if first.is_ascii_punctuation() {
            while i < chars.len() && chars[i].is_ascii_punctuation() {
                i += 1;
            }
        } else {
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !chars[i].is_ascii_digit()
                && !chars[i].is_ascii_punctuation()
            {
                i += 1;
            }
        }
        if i - start == 1 && (first.is_ascii_punctuation() || first.eq_ignore_ascii_case(&'f')) && !"*+".contains(first)
        {
            continue;
        }
        let word: String = chars[start..i - strip].iter().collect::<String>().to_lowercase();
        out.push(word);
    }
    out.sort();
    out
}

/// Lensfun's fuzzy comparison: a score of 0–100 for how well `candidate`
/// matches `pattern`. With `all_words`, every pattern word must be present.
pub(crate) fn fuzzy_score(pattern: &[String], candidate: &[String], all_words: bool) -> u32 {
    if pattern.is_empty() || candidate.is_empty() {
        return 0;
    }
    // Both lists are sorted: walk them together, counting shared words.
    let mut matched = 0_usize;
    let mut ci = 0;
    for p in pattern {
        match candidate[ci..].iter().position(|c| c == p) {
            Some(off) => {
                matched += 1;
                ci += off + 1;
            }
            None if all_words => return 0,
            None => {}
        }
    }
    u32::try_from(matched * 200 / (pattern.len() + candidate.len())).unwrap_or(0)
}

/// Focal range and widest aperture read from a lens name, as lensfun guesses
/// them: "24-70mm f/2.8", "1:4 12-24mm", "4.5-5.6/18-55". Zero where unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Guess {
    pub min_focal: f32,
    pub max_focal: f32,
    pub min_aperture: f32,
}

pub(crate) fn guess_from_name(name: &str) -> Guess {
    let lower = name.to_lowercase();
    if ["adapter", "reducer", "booster", "extender", "converter", "magnifier"].iter().any(|w| lower.contains(w)) {
        return Guess::default();
    }
    // "[min]-[max]mm [f/|f|1:|1/][aperture]"
    for (mm, _) in lower.match_indices("mm") {
        let (min_focal, max_focal) = focal_before(&lower[..mm]);
        if min_focal > 0.0 {
            let after = lower[mm + 2..].trim_start();
            let after = ["f/", "1:", "1/", "f"].iter().find_map(|p| after.strip_prefix(p)).unwrap_or(after);
            let min_aperture = leading_number(after).or_else(|| aperture_after_ratio(&lower)).unwrap_or(0.0);
            return Guess { min_focal, max_focal, min_aperture };
        }
    }
    // "1:[aperture] [min]-[max]"
    if let Some(i) = lower.find("1:") {
        let rest = &lower[i + 2..];
        if let Some(aperture) = leading_number(rest) {
            let after = rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-');
            let (min_focal, max_focal) = range_at_start(after.trim_start().trim_start_matches('/').trim_start());
            if min_focal > 0.0 {
                return Guess { min_focal, max_focal, min_aperture: aperture };
            }
        }
    }
    Guess::default()
}

/// "24-70" or "50" immediately before "mm" (spaces allowed).
fn focal_before(text: &str) -> (f32, f32) {
    let t = text.trim_end();
    let start = t
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    range_at_start(t[start..].trim_start_matches('-'))
}

/// The aperture in "1:3.5-5.6".
fn aperture_after_ratio(lower: &str) -> Option<f32> {
    lower.find("1:").and_then(|i| leading_number(&lower[i + 2..]))
}

/// "24-70…" → (24, 70); "50…" → (50, 50); otherwise (0, 0).
fn range_at_start(text: &str) -> (f32, f32) {
    let Some(a) = leading_number(text) else { return (0.0, 0.0) };
    let rest = text.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let b = rest.strip_prefix('-').and_then(leading_number).unwrap_or(a);
    (a, b)
}

fn leading_number(text: &str) -> Option<f32> {
    let end = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    text[..end].trim_end_matches('.').parse::<f32>().ok().filter(|v| *v > 0.0 && v.is_finite())
}

/// Lensfun's numeric comparison: 0 when either is unknown, +1 within 1 %, −1 otherwise.
fn compare(a: f32, b: f32) -> i32 {
    if a == 0.0 || b == 0.0 {
        return 0;
    }
    let r = a / b;
    if r <= 0.99 || r >= 1.01 { -1 } else { 1 }
}

/// Everything known about the lens being looked for.
#[derive(Debug, Clone)]
pub(crate) struct Query<'a> {
    pub camera: Option<&'a Camera>,
    /// Mounts that fit the camera besides its own.
    pub compat_mounts: Vec<&'a str>,
    pub maker: Option<&'a str>,
    pub words: Vec<String>,
    pub guess: Guess,
    pub all_words: bool,
}

/// Lensfun's `MatchScore`; 0 means "not this lens".
pub(crate) fn score(q: &Query<'_>, lens: &Lens) -> u32 {
    let mut score = 0_u32;

    if let Some(camera) = q.camera
        && !lens.calibrations.is_empty()
    {
        let mut crop_score = 0;
        for set in &lens.calibrations {
            let (cam, cal) = (camera.crop_factor, set.crop_factor);
            // A lens measured on a smaller sensor does not cover this camera's frame.
            if cam < cal * 0.96 {
                continue;
            }
            let s = if cam >= cal * 1.41 {
                2
            } else if cam >= cal * 1.31 {
                4
            } else if cam >= cal * 1.21 {
                6
            } else if cam >= cal * 1.11 {
                8
            } else if cam >= cal * 1.01 {
                10
            } else if cam >= cal {
                5
            } else {
                3
            };
            crop_score = crop_score.max(s);
        }
        if crop_score == 0 {
            return 0;
        }
        score += crop_score;
    }

    let lens_guess = lens_parameters(lens);
    for (a, b) in [
        (q.guess.min_focal, lens_guess.min_focal),
        (q.guess.max_focal, lens_guess.max_focal),
        (q.guess.min_aperture, lens_guess.min_aperture),
    ] {
        match compare(a, b) {
            -1 => return 0,
            1 => score += 10,
            _ => {}
        }
    }

    if let Some(camera) = q.camera
        && !lens.mounts.is_empty()
    {
        if lens.mounts.iter().any(|m| crate::model::loose_eq(m, &camera.mount)) {
            score += 10;
        } else if lens.mounts.iter().any(|m| q.compat_mounts.iter().any(|c| crate::model::loose_eq(m, c))) {
            score += 9;
        } else {
            return 0;
        }
    }

    // EXIF makers are spelt in many ways ("NIKON CORPORATION", "Nikon",
    // "Lumix" for Panasonic): a lens from another maker is not this lens.
    if let Some(maker) = q.maker {
        if !maker_matches(maker, lens) {
            return 0;
        }
        score += 10;
    }

    let best = lens.model.variants().map(|v| fuzzy_score(&q.words, &words(v), q.all_words)).max().unwrap_or(0);
    if best == 0 {
        return 0;
    }
    score + (best * 4 / 10).max(1)
}

/// Words of a maker's name that identify it, case and accents folded.
fn maker_words(text: &str) -> Vec<String> {
    const GENERIC: [&str; 14] = [
        "corporation",
        "corp",
        "co",
        "ltd",
        "inc",
        "imaging",
        "company",
        "optical",
        "camera",
        "cameras",
        "ag",
        "gmbh",
        "digital",
        "solutions",
    ];
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !GENERIC.contains(w))
        .map(str::to_owned)
        .collect()
}

/// Lower case without the accents common in maker names ("Voigtländer").
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' | 'å' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            other => other,
        })
        .collect()
}

/// Brand names that EXIF data and the database use for the same maker.
const MAKER_ALIASES: [(&str, &str); 6] = [
    ("rokinon", "samyang"),
    ("kenko", "tokina"),
    ("lumix", "panasonic"),
    ("om", "olympus"),
    ("asahi", "pentax"),
    ("ricoh", "pentax"),
];

/// `true` when `maker` names the lens's maker, or a brand the lens is sold
/// under ("Leica" for a Panasonic Leica DG lens, "Carl Zeiss" for Zeiss).
fn maker_matches(maker: &str, lens: &Lens) -> bool {
    let wanted = maker_words(maker);
    if wanted.is_empty() {
        return true;
    }
    let known: Vec<String> = lens
        .maker
        .variants()
        .chain(lens.model.variants())
        .flat_map(|v| fold(v).split(|c: char| !c.is_alphanumeric()).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let has = |w: &str| known.iter().any(|k| k == w);
    wanted.iter().any(|w| has(w) || MAKER_ALIASES.iter().any(|(a, b)| (a == w && has(b)) || (b == w && has(a))))
}

/// Focal range and widest aperture of a database lens: declared, else read
/// from its name, else taken from its calibration data.
pub(crate) fn lens_parameters(lens: &Lens) -> Guess {
    let mut g =
        Guess { min_focal: lens.focal_range[0], max_focal: lens.focal_range[1], min_aperture: lens.aperture_range[0] };
    if g.min_focal == 0.0 || g.min_aperture == 0.0 {
        let n = guess_from_name(lens.model.primary());
        if g.min_focal == 0.0 {
            g.min_focal = n.min_focal;
            if g.max_focal == 0.0 {
                g.max_focal = n.max_focal;
            }
        }
        if g.min_aperture == 0.0 {
            g.min_aperture = n.min_aperture;
        }
    }
    if g.min_focal == 0.0 {
        let focals = lens.calibrations.iter().flat_map(|s| {
            s.distortion
                .iter()
                .map(|d| d.focal)
                .chain(s.tca.iter().map(|t| t.focal))
                .chain(s.vignetting.iter().map(|v| v.focal))
        });
        let (lo, hi) = focals.fold((f32::MAX, 0.0_f32), |(lo, hi), f| (lo.min(f), hi.max(f)));
        if hi > 0.0 {
            g.min_focal = lo;
            g.max_focal = hi;
        }
    }
    if g.min_aperture == 0.0 {
        let lo =
            lens.calibrations.iter().flat_map(|s| s.vignetting.iter().map(|v| v.aperture)).fold(f32::MAX, f32::min);
        if lo < f32::MAX {
            g.min_aperture = lo;
        }
    }
    if g.max_focal == 0.0 {
        g.max_focal = g.min_focal;
    }
    g
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // parsed decimal literals
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        words(s)
    }

    #[test]
    fn names_split_into_words_like_lensfun() {
        assert_eq!(w("FE 24-70mm F2.8 GM"), ["2.8", "24", "70", "fe", "gm", "mm"]);
        assert_eq!(w("Sony FE 24-70mm f/2.8 GM"), ["2.8", "24", "70", "fe", "gm", "mm", "sony"]);
        assert_eq!(w("EF50mm f/1.4 USM"), ["1.4", "50", "ef", "mm", "usm"]);
        assert_eq!(w("Zeiss Batis 2/25"), ["2", "25", "batis", "zeiss"]);
        assert_eq!(w("Sonnar T* 55mm F1.8 ZA"), ["*", "1.8", "55", "mm", "sonnar", "t", "za"]);
        assert_eq!(w("50.0 mm"), ["50", "mm"]);
    }

    #[test]
    fn every_word_of_the_exif_name_must_be_in_the_database_name() {
        let exif = w("FE 24-70mm F2.8 GM");
        assert!(fuzzy_score(&exif, &w("Sony FE 24-70mm f/2.8 GM"), true) > 80);
        assert!(fuzzy_score(&exif, &w("Sony FE 24-70mm f/2.8 GM II"), true) > 0);
        assert_eq!(fuzzy_score(&exif, &w("Sony FE 24-105mm f/4 G OSS"), true), 0);
        assert_eq!(fuzzy_score(&w("FE 24-70mm F2.8 GM II"), &w("Sony FE 24-70mm f/2.8 GM"), true), 0);
        // Loose mode counts shared words.
        assert!(fuzzy_score(&w("FE 24-70mm F2.8 GM II"), &w("Sony FE 24-70mm f/2.8 GM"), false) > 0);
    }

    #[test]
    fn makers_match_across_spellings_and_brands() {
        use crate::model::{LensType, Name};
        let lens = |maker: &str, model: &str| Lens {
            maker: Name::new(maker),
            model: Name::new(model),
            mounts: vec![],
            lens_type: LensType::Rectilinear,
            focal_range: [0.0; 2],
            aperture_range: [0.0; 2],
            center: [0.0; 2],
            calibrations: vec![],
        };
        assert!(maker_matches("NIKON CORPORATION", &lens("Nikon", "AF-S Nikkor 50mm f/1.8G")));
        assert!(maker_matches("Voigtlander", &lens("Voigtländer", "Nokton 40mm f/1.2")));
        assert!(maker_matches("Lumix", &lens("Panasonic", "Lumix G Vario 12-32mm")));
        assert!(maker_matches("Leica", &lens("Panasonic", "Leica DG Summilux 25mm f/1.4")));
        assert!(maker_matches("Rokinon", &lens("Samyang", "12mm f/2.0 NCS CS")));
        assert!(!maker_matches("Sony", &lens("Tamron", "E 20mm F2.8 F050")));
    }

    #[test]
    fn focal_lengths_and_aperture_are_read_from_names() {
        let g = guess_from_name("Canon EF 24-70mm f/2.8L II USM");
        assert_eq!((g.min_focal, g.max_focal, g.min_aperture), (24.0, 70.0, 2.8));
        let g = guess_from_name("FE 50mm F1.8");
        assert_eq!((g.min_focal, g.max_focal, g.min_aperture), (50.0, 50.0, 1.8));
        let g = guess_from_name("smc PENTAX-DA 1:3.5-5.6 18-55mm AL");
        assert_eq!((g.min_focal, g.max_focal, g.min_aperture), (18.0, 55.0, 3.5));
        let g = guess_from_name("Summilux-M 1:1.4/50 ASPH.");
        assert_eq!(g.min_aperture, 1.4);
        let g = guess_from_name("LEICA SUMMILUX 50mm f/1.4");
        assert_eq!((g.min_focal, g.min_aperture), (50.0, 1.4));
        let g = guess_from_name("Sigma 24-70mm F2.8 DG DN | Art 019");
        assert_eq!((g.min_focal, g.max_focal, g.min_aperture), (24.0, 70.0, 2.8));
        assert_eq!(guess_from_name("Extender EF 1.4x III"), Guess::default());
    }
}
