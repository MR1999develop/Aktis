//! A built-in stroke font for watermarks: printable ASCII, the copyright
//! sign and accented Latin letters, drawn as lines and elliptical arcs of
//! one width (a clean, technical look), so no font file or font crate is
//! needed.
//!
//! Units: the capital height is 10, the x-height 7, descenders reach -3; y
//! points up. Each glyph is a small path: `M x y` moves, `L x y` draws a
//! line, `A cx cy rx ry from to` draws an elliptical arc (angles in degrees,
//! counter-clockwise; `from > to` runs clockwise), joined to the pen.

/// Capital height in font units.
pub const CAP_HEIGHT: f32 = 10.0;
/// Stroke width in font units.
pub const STROKE: f32 = 1.1;
/// Space between glyphs (ink to ink) in font units.
const GAP: f32 = 1.5;

/// A polyline in font units.
pub type Polyline = Vec<(f32, f32)>;

/// Width of the ink (stroke centres) and path of a character.
fn glyph(c: char) -> (f32, &'static str) {
    match c {
        ' ' => (2.6, ""),
        'A' => (6.0, "M0 0 L3 10 L6 0 M1.2 4 L4.8 4"),
        'B' => (6.2, "M0 0 L0 10 L3.5 10 A3.5 7.5 2.5 2.5 90 -90 L0 5 M3.5 5 L3.7 5 A3.7 2.5 2.5 2.5 90 -90 L0 0"),
        'C' => (6.6, "A3.5 5 3.5 5 50 310"),
        'D' => (6.0, "M0 0 L0 10 L2 10 A2 5 4 5 90 -90 L0 0"),
        'E' => (5.5, "M5.5 10 L0 10 L0 0 L5.5 0 M0 5 L4.5 5"),
        'F' => (5.5, "M5.5 10 L0 10 L0 0 M0 5 L4.5 5"),
        'G' => (7.0, "A3.5 5 3.5 5 50 340 L6.8 4.6 L4 4.6"),
        'H' => (6.0, "M0 0 L0 10 M6 0 L6 10 M0 5 L6 5"),
        'I' => (0.0, "M0 0 L0 10"),
        'J' => (4.5, "M4.5 10 L4.5 2.5 A2.25 2.5 2.25 2.5 0 -180"),
        'K' => (5.8, "M0 0 L0 10 M5.5 10 L0 3.5 M1.8 5.5 L5.8 0"),
        'L' => (5.0, "M0 10 L0 0 L5 0"),
        'M' => (7.0, "M0 0 L0 10 L3.5 2 L7 10 L7 0"),
        'N' => (6.0, "M0 0 L0 10 L6 0 L6 10"),
        'O' => (7.0, "A3.5 5 3.5 5 0 360"),
        'P' => (6.2, "M0 0 L0 10 L3.5 10 A3.5 7.25 2.75 2.75 90 -90 L0 4.5"),
        'Q' => (7.0, "A3.5 5 3.5 5 0 360 M4.2 2.2 L7 -0.8"),
        'R' => (6.2, "M0 0 L0 10 L3.5 10 A3.5 7.25 2.75 2.75 90 -90 L0 4.5 M3 4.5 L6 0"),
        'S' => (6.0, "A3 7.5 3 2.5 20 270 A3 2.5 3 2.5 90 -160"),
        'T' => (6.0, "M0 10 L6 10 M3 10 L3 0"),
        'U' => (6.0, "M0 10 L0 3 A3 3 3 3 180 360 L6 10"),
        'V' => (6.0, "M0 10 L3 0 L6 10"),
        'W' => (8.0, "M0 10 L2 0 L4 7 L6 0 L8 10"),
        'X' => (6.0, "M0 10 L6 0 M0 0 L6 10"),
        'Y' => (6.0, "M0 10 L3 5 L6 10 M3 5 L3 0"),
        'Z' => (6.0, "M0 10 L6 10 L0 0 L6 0"),
        'a' => (5.5, "A2.75 3.5 2.75 3.5 0 360 M5.5 7 L5.5 0"),
        'b' => (5.5, "M0 10 L0 0 A2.75 3.5 2.75 3.5 180 540"),
        'c' => (5.5, "A2.75 3.5 2.75 3.5 45 315"),
        'd' => (5.5, "A2.75 3.5 2.75 3.5 0 360 M5.5 10 L5.5 0"),
        'e' => (5.5, "M0 3.5 L5.5 3.5 A2.75 3.5 2.75 3.5 0 315"),
        'f' => (4.0, "A3.2 8.5 1.6 1.5 60 180 L1.6 0 M0 7 L3.6 7"),
        'g' => (5.5, "A2.75 3.5 2.75 3.5 0 360 M5.5 7 L5.5 -1 A2.75 -1 2.75 2 0 -150"),
        'h' => (5.0, "M0 10 L0 0 M0 4.5 A2.5 4.5 2.5 2.5 180 0 L5 0"),
        'i' => (0.0, "M0 0 L0 7 M0 9.3 L0 9.6"),
        'j' => (1.5, "M1.5 7 L1.5 -1.5 A0 -1.5 1.5 1.5 0 -120 M1.5 9.3 L1.5 9.6"),
        'k' => (4.8, "M0 10 L0 0 M4.5 7 L0 2.5 M1.6 4 L4.8 0"),
        'l' => (0.0, "M0 10 L0 0"),
        'm' => (8.0, "M0 0 L0 7 M0 4.8 A2 4.8 2 2.2 180 0 L4 0 M4 4.8 A6 4.8 2 2.2 180 0 L8 0"),
        'n' => (5.0, "M0 0 L0 7 M0 4.5 A2.5 4.5 2.5 2.5 180 0 L5 0"),
        'o' => (5.5, "A2.75 3.5 2.75 3.5 0 360"),
        'p' => (5.5, "M0 7 L0 -3 A2.75 3.5 2.75 3.5 180 540"),
        'q' => (5.5, "A2.75 3.5 2.75 3.5 0 360 M5.5 7 L5.5 -3"),
        'r' => (4.0, "M0 0 L0 7 M0 4 A3 4 3 3 180 70"),
        's' => (5.0, "A2.5 5.25 2.5 1.75 20 270 A2.5 1.75 2.5 1.75 90 -160"),
        't' => (4.0, "M1.5 9.5 L1.5 1.5 A3 1.5 1.5 1.5 180 300 M0 7 L4 7"),
        'u' => (5.0, "M0 7 L0 2.5 A2.5 2.5 2.5 2.5 180 360 M5 7 L5 0"),
        'v' => (5.0, "M0 7 L2.5 0 L5 7"),
        'w' => (7.0, "M0 7 L1.75 0 L3.5 5.5 L5.25 0 L7 7"),
        'x' => (5.0, "M0 7 L5 0 M0 0 L5 7"),
        'y' => (5.0, "M0 7 L2.53 0.5 M5 7 L1.2 -3"),
        'z' => (5.0, "M0 7 L5 7 L0 0 L5 0"),
        '0' => (6.0, "A3 5 3 5 0 360"),
        '1' => (2.5, "M0.5 8 L2.5 10 L2.5 0"),
        '2' => (6.0, "A3 7 3 3 160 -40 L0 0 L6 0"),
        '3' => (6.0, "A3 7.5 2.8 2.5 150 -90 A3 2.6 3 2.6 90 -150"),
        '4' => (6.0, "M4.5 0 L4.5 10 L0 3 L6 3"),
        '5' => (6.0, "M5.5 10 L0.5 10 L0 5.5 A3 3.2 3 3.2 130 -150"),
        '6' => (6.0, "A3 3.2 3 3.2 0 360 M4.93 9.92 A4.2 4.6 4.2 5.4 80 180 L0 3.2"),
        '7' => (6.0, "M0 10 L6 10 L2 0"),
        '8' => (6.0, "A3 7.6 2.6 2.4 -90 270 A3 2.6 3 2.6 90 450"),
        '9' => (6.0, "A3 6.8 3 3.2 0 360 M6 6.8 L6 5.4 A1.8 5.4 4.2 5.4 0 -100"),
        '.' => (0.0, "M0 0 L0 0.3"),
        ',' => (0.6, "M0.6 0.4 L0 -1.8"),
        ':' => (0.0, "M0 0 L0 0.3 M0 6 L0 6.3"),
        ';' => (0.6, "M0.6 0.4 L0 -1.8 M0.6 6 L0.6 6.3"),
        '!' => (0.0, "M0 10 L0 3 M0 0 L0 0.3"),
        '?' => (5.2, "A2.6 7.4 2.6 2.6 160 -90 L2.6 3 M2.6 0 L2.6 0.3"),
        '-' => (3.5, "M0 4 L3.5 4"),
        '_' => (6.0, "M0 -1 L6 -1"),
        '(' => (1.5, "A3 5 3 6.2 120 240"),
        ')' => (1.5, "A-1.5 5 3 6.2 60 -60"),
        '[' => (2.0, "M2 11 L0 11 L0 -1 L2 -1"),
        ']' => (2.0, "M0 11 L2 11 L2 -1 L0 -1"),
        '{' => (2.5, "M2.5 11 L1.5 10.5 L1.2 6 L0 5 L1.2 4 L1.5 -0.5 L2.5 -1"),
        '}' => (2.5, "M0 11 L1 10.5 L1.3 6 L2.5 5 L1.3 4 L1 -0.5 L0 -1"),
        '/' => (4.0, "M0 -1 L4 11"),
        '\\' => (4.0, "M0 11 L4 -1"),
        '|' => (0.0, "M0 11 L0 -1"),
        '&' => (6.0, "M6 0 L1.4 6.6 A2.6 8.1 1.5 1.9 220 -40 L0.8 3.4 A2.6 2.2 2.4 2.2 160 330 L5.6 4.4"),
        '@' => (7.2, "A3.9 4.8 1.5 1.9 0 360 M5.4 6.7 L5.4 3.6 A6.3 3.6 0.9 0.9 180 360 A4 4.8 3.2 4.3 0 300"),
        '#' => (6.5, "M1.5 0 L2.5 10 M4 0 L5 10 M0 3.3 L6 3.3 M0.5 6.7 L6.5 6.7"),
        '%' => (7.5, "A1.3 8.3 1.3 1.7 0 360 M6.5 10 L1 0 M7.5 1.7 A6.2 1.7 1.3 1.7 0 360"),
        '*' => (5.0, "M2.5 10 L2.5 5 M0.3 8.9 L4.7 6.1 M0.3 6.1 L4.7 8.9"),
        '+' => (6.0, "M0 4.5 L6 4.5 M3 1.5 L3 7.5"),
        '=' => (5.5, "M0 3 L5.5 3 M0 6 L5.5 6"),
        '<' => (5.0, "M5 8 L0 4.5 L5 1"),
        '>' => (5.0, "M0 8 L5 4.5 L0 1"),
        '\'' => (0.0, "M0 10 L0 7.5"),
        '"' => (2.0, "M0 10 L0 7.5 M2 10 L2 7.5"),
        '`' => (1.2, "M0 10 L1.2 8.5"),
        '~' => (5.0, "M0 4 L1 5 L2.5 4.3 L4 3.6 L5 4.6"),
        '^' => (5.0, "M0 7 L2.5 10 L5 7"),
        '$' => (6.0, "A3 7.5 3 2.5 20 270 A3 2.5 3 2.5 90 -160 M3 11 L3 -1"),
        '\u{a9}' => (10.4, "A5.2 5 5.2 5.2 0 360 M6.93 6.35 A5.4 5 2 2.1 40 320"),
        _ => (5.2, "A2.6 7.4 2.6 2.6 160 -90 L2.6 3 M2.6 0 L2.6 0.3"), // '?'
    }
}

/// Accents drawn over a base letter, centred on x = 0 at the x-height.
fn accent(kind: char) -> &'static str {
    match kind {
        '`' => "M-0.8 10 L0.8 8.4",
        '\'' => "M-0.8 8.4 L0.8 10",
        '^' => "M-1.3 8.5 L0 9.8 L1.3 8.5",
        '~' => "M-1.5 8.7 L-0.5 9.5 L0.5 8.9 L1.5 9.6",
        _ => "M-1.2 9.2 L-1.2 9.5 M1.2 9.2 L1.2 9.5", // diaeresis
    }
}

/// An accented Latin letter as its base letter and accent.
fn decompose(c: char) -> Option<(char, char)> {
    let table: [(&str, char, char); 18] = [
        ("\u{e0}\u{e8}\u{ec}\u{f2}\u{f9}", '`', 'a'),
        ("\u{e1}\u{e9}\u{ed}\u{f3}\u{fa}", '\'', 'a'),
        ("\u{e2}\u{ea}\u{ee}\u{f4}\u{fb}", '^', 'a'),
        ("\u{e4}\u{eb}\u{ef}\u{f6}\u{fc}", ':', 'a'),
        ("\u{c0}\u{c8}\u{cc}\u{d2}\u{d9}", '`', 'A'),
        ("\u{c1}\u{c9}\u{cd}\u{d3}\u{da}", '\'', 'A'),
        ("\u{c2}\u{ca}\u{ce}\u{d4}\u{db}", '^', 'A'),
        ("\u{c4}\u{cb}\u{cf}\u{d6}\u{dc}", ':', 'A'),
        ("\u{f1}", '~', 'n'),
        ("\u{d1}", '~', 'N'),
        ("\u{e3}", '~', 'a'),
        ("\u{f5}", '~', 'o'),
        ("\u{c3}", '~', 'A'),
        ("\u{d5}", '~', 'O'),
        ("\u{ff}", ':', 'y'),
        ("\u{fd}", '\'', 'y'),
        ("\u{dd}", '\'', 'Y'),
        ("\u{e7}", ',', 'c'),
    ];
    for (letters, kind, first) in table {
        if let Some(i) = letters.chars().position(|l| l == c) {
            let base = if letters.chars().count() == 5 {
                let vowels = if first.is_ascii_uppercase() { "AEIOU" } else { "aeiou" };
                vowels.chars().nth(i)?
            } else {
                first
            };
            return Some((base, kind));
        }
    }
    None
}

/// Parses a glyph path into polylines, shifted by `(dx, dy)`.
fn parse(path: &str, dx: f32, dy: f32, out: &mut Vec<Polyline>) {
    let mut command = 'M';
    let mut numbers: Vec<f32> = Vec::new();
    let mut current: Polyline = Vec::new();
    let mut tokens = Vec::new();
    for word in path.split_whitespace() {
        let (head, rest) =
            word.split_at(word.chars().next().filter(char::is_ascii_alphabetic).map_or(0, char::len_utf8));
        if !head.is_empty() {
            tokens.push(Err(head.chars().next().unwrap_or('M')));
        }
        if !rest.is_empty() {
            tokens.push(Ok(rest.parse::<f32>().unwrap_or(0.0)));
        }
    }
    let flush = |current: &mut Polyline, out: &mut Vec<Polyline>| {
        if !current.is_empty() {
            out.push(std::mem::take(current));
        }
    };
    for token in tokens {
        match token {
            Err(c) => {
                command = c;
                numbers.clear();
            }
            Ok(n) => numbers.push(n),
        }
        match command {
            'M' if numbers.len() == 2 => {
                flush(&mut current, out);
                current.push((numbers[0] + dx, numbers[1] + dy));
                numbers.clear();
                command = 'L';
            }
            'L' if numbers.len() == 2 => {
                current.push((numbers[0] + dx, numbers[1] + dy));
                numbers.clear();
            }
            'A' if numbers.len() == 6 => {
                let [cx, cy, rx, ry, from, to] =
                    [numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5]];
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few dozen steps
                let steps = ((to - from).abs() / 8.0).ceil().max(4.0) as u32;
                for k in 0..=steps {
                    #[allow(clippy::cast_precision_loss)] // small step counts
                    let a = (from + (to - from) * k as f32 / steps as f32).to_radians();
                    current.push((cx + rx * a.cos() + dx, cy + ry * a.sin() + dy));
                }
                numbers.clear();
            }
            _ => {}
        }
    }
    flush(&mut current, out);
}

/// The strokes of `text` laid out on one line from x = 0 (baseline y = 0),
/// and the advance width, in font units. Characters the font lacks are
/// drawn as `?`.
#[must_use]
pub fn layout(text: &str) -> (Vec<Polyline>, f32) {
    let mut strokes = Vec::new();
    let mut x = 0.0_f32;
    for (i, c) in text.chars().enumerate() {
        if i > 0 {
            x += GAP + STROKE;
        }
        let (base, mark) = decompose(c).map_or((c, None), |(b, m)| (b, Some(m)));
        let (width, path) = glyph(base);
        // Accented i loses its dot.
        let path = if base == 'i' && mark.is_some() { "M0 0 L0 7" } else { path };
        parse(path, x, 0.0, &mut strokes);
        if let Some(m) = mark {
            let lift = if base.is_ascii_uppercase() { 3.4 } else { 0.0 };
            if m == ',' {
                parse("M0 0 L0.4 -1 L-0.6 -2", x + width / 2.0, 0.0, &mut strokes);
            } else {
                parse(accent(m), x + width / 2.0, lift, &mut strokes);
            }
        }
        x += width;
    }
    (strokes, x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_ascii_character_has_ink_except_space() {
        for c in (0x21_u8..0x7F).map(char::from) {
            let (width, path) = glyph(c);
            assert!(width >= 0.0);
            assert!(!path.is_empty(), "{c}");
            let mut strokes = Vec::new();
            parse(path, 0.0, 0.0, &mut strokes);
            assert!(!strokes.is_empty(), "{c}");
            for (x, y) in strokes.iter().flatten() {
                assert!((-3.5..=12.5).contains(y) && (-2.5..=11.0).contains(x), "{c}: {x}, {y}");
            }
        }
        assert!(layout(" ").0.is_empty());
    }

    #[test]
    fn paths_parse_into_polylines() {
        let mut s = Vec::new();
        parse("M0 0 L3 10 L6 0 M1.2 4 L4.8 4", 1.0, 0.0, &mut s);
        assert_eq!(s, vec![vec![(1.0, 0.0), (4.0, 10.0), (7.0, 0.0)], vec![(2.2, 4.0), (5.8, 4.0)]]);
        let mut arc = Vec::new();
        parse("A0 0 2 1 0 90", 0.0, 0.0, &mut arc);
        let (first, last) = (arc[0][0], *arc[0].last().expect("points"));
        assert!((first.0 - 2.0).abs() < 1e-5 && first.1.abs() < 1e-5);
        assert!(last.0.abs() < 1e-5 && (last.1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn text_advances_and_accents_sit_above_their_letter() {
        let (_, one) = layout("A");
        let (_, two) = layout("AA");
        assert!((two - (2.0 * one + GAP + STROKE)).abs() < 1e-5);
        let (strokes, _) = layout("\u{e8}");
        let top = strokes.iter().flatten().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(top > 8.0, "the accent of è is drawn ({top})");
        let (upper, _) = layout("\u{c8}");
        let top = upper.iter().flatten().map(|p| p.1).fold(f32::MIN, f32::max);
        assert!(top > 11.0, "the accent of È sits above the capital ({top})");
        assert_eq!(decompose('\u{f9}'), Some(('u', '`')));
        assert_eq!(decompose('\u{c9}'), Some(('E', '\'')));
        assert_eq!(decompose('x'), None);
    }
}
