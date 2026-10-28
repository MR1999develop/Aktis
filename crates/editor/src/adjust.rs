//! The editing logic behind the white balance, levels, curve, colour
//! editor and dehaze panels, kept apart from the drawing so it is tested
//! without a window: which parameters a mode, a picked colour, a moved
//! handle or a dragged curve point set.

use core_model::{NodeId, ParamId, ParamValue};
use tool_adjust::curve::{self, MAX_POINTS, Spline};
use tool_adjust::{color_editor, dehaze, levels, white_balance};
use tool_api::{Params, ResolvedParams, ToolRegistry};

use crate::Document;

/// What a picker in the viewer is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    /// White balance: the clicked colour should be grey.
    Neutral,
    /// Levels: the clicked colour becomes the input black point.
    LevelsBlack,
    /// Levels: the clicked colour becomes the input white point.
    LevelsWhite,
    /// Curve: a point is added where the clicked colour sits on the curve.
    CurvePoint,
    /// Colour editor: a range (the Advanced range [`Pick::channel`], or
    /// the skin tone range when it is [`color_editor::ADVANCED`]) is
    /// centred on the clicked colour's hue and turned on.
    ColorRange,
    /// Dehaze: the shadow tone becomes Custom, with the clicked colour.
    ShadowTone,
}

/// A picker waiting for a click in the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    /// The node it edits.
    pub node: NodeId,
    /// What it sets.
    pub kind: PickKind,
    /// The levels set or curve it edits (an index into
    /// [`levels::CHANNELS`] or [`curve::CURVES`]), or the colour editor's
    /// range (see [`PickKind::ColorRange`]); 0 for the other pickers.
    pub channel: usize,
}

fn pid(id: &str) -> ParamId {
    ParamId::new(id).expect("tool parameter ids are valid")
}

/// A colour (linear) encoded with the 2.2 gamma the levels and curve use, 0–1.
#[must_use]
pub fn encode(v: f32) -> f32 {
    v.max(0.0).powf(1.0 / 2.2)
}

/// The value a levels set or a curve reads from `rgb`, encoded: the
/// luminance for the RGB set, the Luma curve (and the RGB curve), else the
/// channel's own value. `channel` follows [`curve::CURVES`] when `curve` is
/// true, else [`levels::CHANNELS`].
#[must_use]
pub fn channel_value(rgb: [f32; 3], channel: usize, curve: bool) -> f32 {
    let luma = rgb[0] * 0.2627 + rgb[1] * 0.6780 + rgb[2] * 0.0593;
    let v = match (curve, channel) {
        (true, 0 | 1) | (false, 0) => luma,
        (true, c) => rgb[(c - 2).min(2)],
        (false, c) => rgb[(c - 1).min(2)],
    };
    encode(v)
}

/// A node's parameters resolved against its tool.
fn resolved(doc: &Document, registry: &ToolRegistry, node: NodeId) -> Option<ResolvedParams> {
    let n = doc.recipe().graph.node(node)?;
    let core_model::NodeKind::Tool(id) = &n.kind else { return None };
    Some(ResolvedParams::resolve(registry.get(id)?.descriptor(), &n.params))
}

/// The first values of a new node of `tool` in `doc`: for white balance,
/// the camera's light as both the as-shot and the chosen one.
#[must_use]
pub fn initial_params(doc: &Document, tool: &str) -> Params {
    let mut p = Params::new();
    if tool == "white_balance"
        && let Some((k, t)) = doc.shot_light()
    {
        for (id, v) in [
            (white_balance::SHOT_KELVIN, k),
            (white_balance::SHOT_TINT, t),
            (white_balance::KELVIN, k),
            (white_balance::TINT, t),
        ] {
            p.set(pid(id), ParamValue::Float(v));
        }
    }
    p
}

/// The values a white-balance mode sets: the mode with its kelvin and tint
/// (Shot: the as-shot light; Auto: grey world; a light source: its preset;
/// Custom: the current values).
#[must_use]
pub fn wb_mode_values(
    doc: &Document,
    registry: &ToolRegistry,
    node: NodeId,
    mode: white_balance::Mode,
) -> Option<Vec<(ParamId, ParamValue)>> {
    use white_balance::{KELVIN, MODE, Mode, SHOT_KELVIN, SHOT_TINT, TINT};
    let r = resolved(doc, registry, node)?;
    let shot = (r.float(SHOT_KELVIN), r.float(SHOT_TINT));
    let (k, t) = match mode {
        Mode::Shot => shot,
        Mode::Auto => doc.auto_light().unwrap_or(shot),
        Mode::Custom => (r.float(KELVIN), r.float(TINT)),
        preset => preset.preset().unwrap_or(shot),
    };
    Some(vec![
        (pid(MODE), ParamValue::Choice(mode.index())),
        (pid(KELVIN), ParamValue::Float(k)),
        (pid(TINT), ParamValue::Float(t)),
    ])
}

/// What a click with `pick` on a pixel whose colour entering the node is
/// `rgb` sets.
#[must_use]
pub fn pick_values(
    doc: &Document,
    registry: &ToolRegistry,
    pick: Pick,
    rgb: [f32; 3],
) -> Option<Vec<(ParamId, ParamValue)>> {
    let r = resolved(doc, registry, pick.node)?;
    match pick.kind {
        PickKind::Neutral => {
            use white_balance::{KELVIN, MODE, Mode, SHOT_KELVIN, SHOT_TINT, TINT};
            let (k, t) = white_balance::neutral_light(rgb, (r.float(SHOT_KELVIN), r.float(SHOT_TINT)))?;
            Some(vec![
                (pid(MODE), ParamValue::Choice(Mode::Custom.index())),
                (pid(KELVIN), ParamValue::Float(k)),
                (pid(TINT), ParamValue::Float(t)),
            ])
        }
        PickKind::LevelsBlack | PickKind::LevelsWhite => {
            let c = pick.channel.min(levels::CHANNELS.len() - 1);
            let set = levels::Set::of(&r, c);
            let v = (channel_value(rgb, c, false) * 255.0).round().clamp(0.0, 255.0);
            let (which, v) = if pick.kind == PickKind::LevelsBlack {
                (0, v.min(set.in_white - 2.0).max(0.0))
            } else {
                (1, v.max(set.in_black + 2.0).min(255.0))
            };
            Some(vec![(pid(&levels::param_id(c, which)), ParamValue::Float(v))])
        }
        PickKind::CurvePoint => {
            let c = pick.channel.min(curve::CURVES.len() - 1);
            let pts = curve::points(&r, c);
            let x = channel_value(rgb, c, true).min(1.0);
            let y = Spline::new(&pts).eval(x).clamp(0.0, 1.0);
            let (pts, _) = insert_point(&pts, [x, y])?;
            Some(curve_values(c, &pts))
        }
        PickKind::ColorRange => {
            let (hue, chroma) = tool_adjust::hue_chroma(rgb);
            if chroma < MIN_RANGE_CHROMA {
                return None;
            }
            let id = |v: &str| {
                if pick.channel >= color_editor::ADVANCED {
                    color_editor::skin_id(v)
                } else {
                    color_editor::advanced_id(pick.channel, v)
                }
            };
            Some(vec![
                (pid(&id("on")), ParamValue::Bool(true)),
                (pid(&id("center")), ParamValue::Float(hue.round().rem_euclid(360.0))),
            ])
        }
        PickKind::ShadowTone => {
            let max = rgb[0].max(rgb[1]).max(rgb[2]);
            if max <= 1e-6 {
                return None;
            }
            let mut values = vec![(pid(dehaze::SHADOW_TONE), ParamValue::Choice(dehaze::CUSTOM_TONE))];
            for (id, v) in dehaze::TONE_RGB.iter().zip(rgb) {
                // Its brightness is ignored: the strongest channel at 1, to the slider's step.
                values.push((pid(id), ParamValue::Float(((v / max).clamp(0.0, 1.0) * 100.0).round() / 100.0)));
            }
            Some(values)
        }
    }
}

/// Least chroma (0–1, as [`tool_adjust::hue_chroma`] measures it) a picked
/// colour needs to centre a colour range on its hue: the hue of a near grey
/// means nothing.
pub const MIN_RANGE_CHROMA: f32 = 0.05;

/// The parameters storing `points` as curve `c`.
#[must_use]
pub fn curve_values(c: usize, points: &[[f32; 2]]) -> Vec<(ParamId, ParamValue)> {
    #[allow(clippy::cast_precision_loss)] // a small count
    let n = points.len().min(MAX_POINTS) as f32;
    let mut v = vec![(pid(&curve::count_id(c)), ParamValue::Float(n))];
    for (i, p) in points.iter().take(MAX_POINTS).enumerate() {
        v.push((pid(&curve::point_id(c, i, 'x')), ParamValue::Float(p[0].clamp(0.0, 1.0))));
        v.push((pid(&curve::point_id(c, i, 'y')), ParamValue::Float(p[1].clamp(0.0, 1.0))));
    }
    v
}

/// Closest gap allowed between two points' inputs.
pub const MIN_GAP: f32 = 0.01;

/// `points` (sorted by input) with `p` added in order; the new list and the
/// new point's index. `None` when the curve is full or `p` is too close to
/// an existing point's input.
#[must_use]
pub fn insert_point(points: &[[f32; 2]], p: [f32; 2]) -> Option<(Vec<[f32; 2]>, usize)> {
    if points.len() >= MAX_POINTS || points.iter().any(|q| (q[0] - p[0]).abs() < MIN_GAP) {
        return None;
    }
    let i = points.iter().position(|q| q[0] > p[0]).unwrap_or(points.len());
    let mut out = points.to_vec();
    out.insert(i, [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]);
    Some((out, i))
}

/// `points` with point `i` moved towards `p`, kept between its neighbours.
#[must_use]
pub fn move_point(points: &[[f32; 2]], i: usize, p: [f32; 2]) -> Vec<[f32; 2]> {
    let mut out = points.to_vec();
    if i >= out.len() {
        return out;
    }
    let lo = if i == 0 { 0.0 } else { out[i - 1][0] + MIN_GAP };
    let hi = if i + 1 == out.len() { 1.0 } else { out[i + 1][0] - MIN_GAP };
    out[i] = [p[0].clamp(lo, hi.max(lo)), p[1].clamp(0.0, 1.0)];
    out
}

/// `points` without point `i`; `None` when only two remain.
#[must_use]
pub fn remove_point(points: &[[f32; 2]], i: usize) -> Option<Vec<[f32; 2]>> {
    (points.len() > 2 && i < points.len()).then(|| {
        let mut out = points.to_vec();
        out.remove(i);
        out
    })
}

/// The point of `points` nearest to `p` within `radius` (same units).
#[must_use]
pub fn nearest_point(points: &[[f32; 2]], p: [f32; 2], radius: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, q)| (i, (q[0] - p[0]).hypot(q[1] - p[1])))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Where the midtone handle of a levels set sits, 0–255: the input whose
/// output is half-way between the output points.
#[must_use]
pub fn midtone_position(set: &levels::Set) -> f32 {
    let g = (-set.midtone / 100.0).exp2();
    set.in_black + 0.5_f32.powf(1.0 / g) * (set.in_white - set.in_black)
}

/// The midtone value that puts the handle at `x` (0–255), within ±100.
#[must_use]
pub fn midtone_for_position(set: &levels::Set, x: f32) -> f32 {
    let span = (set.in_white - set.in_black).max(1.0);
    let t = ((x - set.in_black) / span).clamp(0.02, 0.98);
    // 0.5 = t^g  →  g = ln 0.5 / ln t; midtone = −100·log2 g.
    let g = 0.5_f32.ln() / t.ln();
    (-100.0 * g.log2()).clamp(-100.0, 100.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn points_are_added_in_order_moved_between_neighbours_and_removed() {
        let pts = vec![[0.0, 0.0], [1.0, 1.0]];
        let (pts, i) = insert_point(&pts, [0.5, 0.7]).unwrap();
        assert_eq!((pts.clone(), i), (vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]], 1));
        assert!(insert_point(&pts, [0.505, 0.2]).is_none(), "too close");
        let moved = move_point(&pts, 1, [1.2, -0.3]);
        assert_eq!(moved[1], [0.99, 0.0]);
        let moved = move_point(&pts, 0, [0.7, 0.2]);
        assert_eq!(moved[0], [0.49, 0.2]);
        assert_eq!(remove_point(&pts, 1).unwrap(), vec![[0.0, 0.0], [1.0, 1.0]]);
        assert!(remove_point(&[[0.0, 0.0], [1.0, 1.0]], 0).is_none());
        assert_eq!(nearest_point(&pts, [0.52, 0.68], 0.05), Some(1));
        assert_eq!(nearest_point(&pts, [0.3, 0.9], 0.05), None);
        let full: Vec<[f32; 2]> = (0..8).map(|i| [i as f32 / 7.0, 0.5]).collect();
        assert!(insert_point(&full, [0.07, 0.1]).is_none());
    }

    #[test]
    fn curve_values_store_the_count_and_every_point() {
        let v = curve_values(1, &[[0.0, 0.1], [0.4, 0.5], [1.0, 0.9]]);
        assert_eq!(v.len(), 7);
        assert_eq!(v[0], (pid("luma_points"), ParamValue::Float(3.0)));
        assert_eq!(v[3], (pid("luma_x1"), ParamValue::Float(0.4)));
        assert_eq!(v[6], (pid("luma_y2"), ParamValue::Float(0.9)));
    }

    #[test]
    fn the_midtone_handle_and_its_value_agree() {
        let mut set = levels::Set { in_black: 20.0, in_white: 230.0, midtone: 0.0, out_black: 0.0, out_white: 255.0 };
        assert!((midtone_position(&set) - 125.0).abs() < 1e-3);
        for m in [-80.0, -20.0, 0.0, 35.0, 90.0] {
            set.midtone = m;
            let x = midtone_position(&set);
            assert!((midtone_for_position(&set, x) - m).abs() < 1e-2, "{m}");
            // The handle's input gives the middle output.
            assert!((set.apply(x / 255.0) - 0.5).abs() < 1e-4);
        }
    }

    fn doc() -> (Document, ToolRegistry) {
        use std::sync::Arc;
        let img = Arc::new(core_model::ImageF32::new(4, 4, core_model::WorkingSpace::LinearRec2020).unwrap());
        let opened = crate::Opened {
            path: "a/b.NEF".into(),
            camera: "Nikon D3S".into(),
            orientation: core_model::Orientation::Normal,
            has_camera_white_balance: true,
            raw: None,
            detected_lens: None,
            lens_choices: Vec::new(),
            lens_key: None,
            info: raw_io::PhotoInfo::default(),
            image: crate::DevelopedImage {
                settings: core_model::DevelopSettings::default(),
                developed: Arc::clone(&img),
                preview: Arc::clone(&img),
                draft: img,
                lens: None,
                notes: Vec::new(),
                profile: String::new(),
            },
        };
        (Document::new(opened, 1), engine::builtin_tools().unwrap())
    }

    fn add(doc: &mut Document, reg: &ToolRegistry, tool: &str) -> NodeId {
        let source = doc.recipe().graph.source();
        doc.add_node(reg, source, core_model::ToolId::new(tool).unwrap()).unwrap()
    }

    #[test]
    fn pickers_set_the_values_of_their_tool() {
        let (mut doc, reg) = doc();
        let lv = add(&mut doc, &reg, "levels");
        let v = pick_values(&doc, &reg, Pick { node: lv, kind: PickKind::LevelsBlack, channel: 0 }, [0.05; 3]).unwrap();
        assert_eq!(v, vec![(pid("rgb_in_black"), ParamValue::Float((encode(0.05) * 255.0).round()))]);
        let v = pick_values(&doc, &reg, Pick { node: lv, kind: PickKind::LevelsWhite, channel: 3 }, [0.0, 0.0, 9.0])
            .unwrap();
        assert_eq!(v, vec![(pid("blue_in_white"), ParamValue::Float(255.0))]);

        let cv = add(&mut doc, &reg, "curve");
        let v = pick_values(&doc, &reg, Pick { node: cv, kind: PickKind::CurvePoint, channel: 1 }, [0.18; 3]).unwrap();
        assert_eq!(v[0], (pid("luma_points"), ParamValue::Float(3.0)));
        let ParamValue::Float(x) = v[3].1 else { panic!() };
        let ParamValue::Float(y) = v[4].1 else { panic!() };
        assert!((x - encode(0.18)).abs() < 1e-3 && (x - y).abs() < 1e-6, "a point on the straight curve: {x} {y}");

        let wb = add(&mut doc, &reg, "white_balance");
        let v =
            pick_values(&doc, &reg, Pick { node: wb, kind: PickKind::Neutral, channel: 0 }, [0.5, 0.4, 0.25]).unwrap();
        assert_eq!(v[0], (pid("mode"), ParamValue::Choice(white_balance::Mode::Custom.index())));
        let ParamValue::Float(k) = v[1].1 else { panic!() };
        assert!(k < 5500.0, "{k}");
    }

    #[test]
    fn colour_pickers_centre_a_range_and_set_the_shadow_tone() {
        let (mut doc, reg) = doc();
        let ce = add(&mut doc, &reg, "color_editor");
        let pick = |channel| Pick { node: ce, kind: PickKind::ColorRange, channel };
        // A green: hue 120°.
        let v = pick_values(&doc, &reg, pick(1), [0.1, 0.6, 0.1]).unwrap();
        assert_eq!(v, vec![(pid("adv2_on"), ParamValue::Bool(true)), (pid("adv2_center"), ParamValue::Float(120.0))]);
        // A skin colour sets the skin tone range.
        let v = pick_values(&doc, &reg, pick(color_editor::ADVANCED), [0.6, 0.3, 0.2]).unwrap();
        assert_eq!(v[0], (pid("skin_on"), ParamValue::Bool(true)));
        let ParamValue::Float(h) = v[1].1 else { panic!() };
        assert!((h - 15.0).abs() < 1e-3, "{h}");
        // Near red on the other side of 0° wraps into 0–360.
        let v = pick_values(&doc, &reg, pick(0), [0.6, 0.1, 0.104]).unwrap();
        assert_eq!(v[1], (pid("adv1_center"), ParamValue::Float(0.0)));
        let v = pick_values(&doc, &reg, pick(0), [0.6, 0.1, 0.15]).unwrap();
        assert_eq!(v[1], (pid("adv1_center"), ParamValue::Float(354.0)));
        assert!(pick_values(&doc, &reg, pick(0), [0.3, 0.3, 0.3]).is_none(), "a grey has no hue");
        // The values are valid for the tool: the document takes them.
        doc.set_params(&reg, ce, pick_values(&doc, &reg, pick(2), [0.1, 0.2, 0.6]).unwrap(), None).unwrap();

        let dh = add(&mut doc, &reg, "dehaze");
        let tone = Pick { node: dh, kind: PickKind::ShadowTone, channel: 0 };
        let v = pick_values(&doc, &reg, tone, [0.1, 0.2, 0.4]).unwrap();
        assert_eq!(
            v,
            vec![
                (pid("shadow_tone"), ParamValue::Choice(1)),
                (pid("tone_red"), ParamValue::Float(0.25)),
                (pid("tone_green"), ParamValue::Float(0.5)),
                (pid("tone_blue"), ParamValue::Float(1.0)),
            ]
        );
        assert!(pick_values(&doc, &reg, tone, [0.0; 3]).is_none(), "black has no colour");
        doc.set_params(&reg, dh, v, None).unwrap();
    }

    #[test]
    fn white_balance_modes_set_their_light() {
        let (mut doc, reg) = doc();
        let wb = add(&mut doc, &reg, "white_balance");
        let v = wb_mode_values(&doc, &reg, wb, white_balance::Mode::Tungsten).unwrap();
        assert_eq!(v[1], (pid("kelvin"), ParamValue::Float(2850.0)));
        let v = wb_mode_values(&doc, &reg, wb, white_balance::Mode::Shot).unwrap();
        assert_eq!(v[1], (pid("kelvin"), ParamValue::Float(5500.0)), "the node's as-shot light");
        assert_eq!(initial_params(&doc, "white_balance"), Params::new(), "no RAW data here");
    }

    #[test]
    fn channel_values_read_luminance_or_the_channel() {
        let rgb = [0.5, 0.25, 0.125];
        assert!((channel_value(rgb, 2, false) - encode(0.25)).abs() < 1e-7);
        assert!((channel_value(rgb, 4, true) - encode(0.125)).abs() < 1e-7);
        let luma = encode(0.5 * 0.2627 + 0.25 * 0.6780 + 0.125 * 0.0593);
        assert!((channel_value(rgb, 0, false) - luma).abs() < 1e-7);
        assert!((channel_value(rgb, 1, true) - luma).abs() < 1e-7);
    }
}
