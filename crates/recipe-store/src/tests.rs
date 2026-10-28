#![allow(clippy::unwrap_used, clippy::float_cmp)]

use commands::{Edit, SavedHistory, SavedStep};
use core_model::{
    AiTarget, BodyPart, BrushMask, BrushStroke, DevelopSettings, LensSettings, Link, MaskComponent, MaskDef, MaskOp,
    MaskSource, ModelId, NodeGraph, NodeId, NodeKind, NormPoint, NormRect, ParamId, ParamValue, Params, ProfileChoice,
    PromptPoint, RasterKey, Recipe, ToolId, WhiteBalance,
};

use super::*;

fn p(x: f32, y: f32) -> NormPoint {
    NormPoint::new(x, y).unwrap()
}

fn tool(name: &str) -> ToolId {
    ToolId::new(name).unwrap()
}

fn param(name: &str) -> ParamId {
    ParamId::new(name).unwrap()
}

fn model() -> ModelId {
    ModelId::new("sam2.1_small").unwrap()
}

/// A mask with every kind of component and target.
fn every_mask() -> MaskDef {
    let mut subject = MaskComponent::ai("Subject \u{2014} \"main\"", MaskOp::Add, AiTarget::Subject, model());
    if let MaskSource::Ai(ai) = &mut subject.source {
        ai.raster = Some(RasterKey::from_bytes([0xab; 16]));
    }
    let object = MaskComponent::ai(
        "Bike",
        MaskOp::Intersect,
        AiTarget::Object {
            points: vec![
                PromptPoint { at: p(0.25, 0.75), include: true },
                PromptPoint { at: p(0.1, 0.3), include: false },
            ],
            rect: Some(NormRect::new(p(0.9, 0.1), p(0.2, 0.8))),
        },
        model(),
    );
    let clicks_only = MaskComponent::ai(
        "Cup",
        MaskOp::Add,
        AiTarget::Object { points: vec![PromptPoint { at: p(0.5, 0.5), include: true }], rect: None },
        model(),
    );
    let part = MaskComponent::ai(
        "Hair",
        MaskOp::Subtract,
        AiTarget::PersonPart { person: NormRect::new(p(0.1, 0.2), p(0.4, 0.9)), part: BodyPart::EyeSclera },
        model(),
    );
    let stroke = BrushStroke {
        points: vec![p(0.0, 0.0), p(0.333_333_34, 1.0), p(1.0, 0.5)],
        radius: 0.02,
        hardness: 0.5,
        opacity: 0.8,
    };
    let mut brush =
        MaskComponent::brush("Brush", MaskOp::Subtract, BrushMask { strokes: vec![stroke], edge_aware: true });
    brush.enabled = false;
    let empty = MaskComponent::brush("Empty", MaskOp::Add, BrushMask::default());
    MaskDef {
        components: vec![subject, object, clicks_only, part, brush, empty],
        refine_edges: false,
        feather: 0.012_5,
        opacity: 0.7,
        invert: true,
    }
}

/// A recipe with every kind of node, parameter value and development setting.
fn every_recipe() -> Recipe {
    let mut g = NodeGraph::new();
    let mut values = Params::new();
    values.set(param("exposure"), ParamValue::Float(-1.234_567_9));
    values.set(param("highlights"), ParamValue::Float(0.1));
    values.set(param("enabled_flag"), ParamValue::Bool(true));
    values.set(param("mode"), ParamValue::Choice(3));
    let a = g.insert_serial(g.source(), tool("exposure"), values).unwrap();
    let b = g.insert_serial(a, tool("curve"), Params::new()).unwrap();
    g.node_mut(b).unwrap().set_enabled(false);
    g.node_mut(b).unwrap().set_label("Contrast \u{e8} \\ / \n".to_owned());
    let par = g.add_node(NodeKind::Parallel { inputs: 3 }, Params::new()).unwrap();
    g.connect(Link { from: a, from_port: 0, to: par, to_port: 2 }).unwrap();
    let m = g.add_mask_node(every_mask());
    g.connect(Link { from: m, from_port: 0, to: b, to_port: 1 }).unwrap();
    let gone = g.add_node(NodeKind::Mask, Params::new()).unwrap();
    g.remove_node(gone).unwrap();
    Recipe {
        develop: DevelopSettings {
            white_balance: WhiteBalance::Multipliers([2.1, 1.0, 1.456_789]),
            profile: ProfileChoice::Named("Camera Standard".to_owned()),
            lens: LensSettings {
                enabled: true,
                lens: Some("Sony FE 24-70mm f/2.8 GM".to_owned()),
                distortion: false,
                tca: true,
                vignetting: false,
                zoom: Some(1.05),
            },
        },
        graph: g,
    }
}

/// One edit of every kind.
fn every_edit() -> Vec<Edit> {
    let n = NodeId::from_raw(2);
    let mut values = Params::new();
    values.set(param("exposure"), ParamValue::Float(0.5));
    vec![
        Edit::SetParam { node: n, param: param("exposure"), value: ParamValue::Float(0.75) },
        Edit::SetParam { node: n, param: param("auto"), value: ParamValue::Bool(false) },
        Edit::SetParams {
            node: n,
            values: vec![
                (param("mode"), ParamValue::Choice(1)),
                (param("kelvin"), ParamValue::Float(5500.0)),
                (param("mode"), ParamValue::Choice(2)),
            ],
        },
        Edit::ResetParam { node: n, param: param("exposure") },
        Edit::InsertSerial { after: NodeId::from_raw(0), tool: tool("exposure"), params: values },
        Edit::InsertSerial { after: n, tool: tool("hdr"), params: Params::new() },
        Edit::RemoveNode { node: n },
        Edit::Connect(Link { from: NodeId::from_raw(3), from_port: 0, to: n, to_port: 1 }),
        Edit::Disconnect { to: n, port: 1 },
        Edit::SetEnabled { node: n, enabled: false },
        Edit::SetWhiteBalance(WhiteBalance::AsShot),
        Edit::SetWhiteBalance(WhiteBalance::Multipliers([1.9, 1.0, 1.3])),
        Edit::SetCameraProfile(ProfileChoice::Builtin),
        Edit::SetCameraProfile(ProfileChoice::Named("Portrait".to_owned())),
        Edit::SetLensCorrection(LensSettings::off()),
        Edit::SetLensCorrection(LensSettings::default()),
        Edit::AddMask { to: n, def: every_mask() },
        Edit::SetMask { node: NodeId::from_raw(5), def: MaskDef::default() },
    ]
}

fn every_variant() -> Variant {
    let recipe = every_recipe();
    let steps = every_edit()
        .into_iter()
        .enumerate()
        .map(|(i, edit)| SavedStep {
            edit,
            created: if i % 4 == 0 { vec![NodeId::from_raw(7), NodeId::from_raw(8)] } else { vec![] },
            skipped: (i == 3).then(|| "it acts on a node made by a removed step".to_owned()),
        })
        .collect();
    let history = SavedHistory { base: Recipe::default(), steps, redo: every_edit() };
    let layout = Layout {
        nodes: vec![(NodeId::from_raw(0), [24.0, 24.0]), (NodeId::from_raw(2), [202.5, -13.25])],
        pan: [-40.0, 12.5],
        zoom: 1.25,
    };
    Variant { name: "Variant 2 \u{b7} \u{1f642}".to_owned(), recipe, history: Some(history), layout: Some(layout) }
}

#[test]
fn every_edit_node_and_mask_kind_round_trips() {
    let v = every_variant();
    let text = encode(&v);
    assert!(text.starts_with("{\"format\":1,"), "{}", &text[..40]);
    assert_eq!(decode(&text), Ok(v));
}

#[test]
fn each_edit_round_trips_on_its_own() {
    for edit in every_edit() {
        let v = codec::edit(&edit);
        let back = codec::read_edit(&json::parse(&v.to_text()).unwrap()).unwrap();
        assert_eq!(back, edit);
    }
}

#[test]
fn a_variant_without_history_or_layout_round_trips() {
    let v = Variant { name: String::new(), recipe: Recipe::default(), history: None, layout: None };
    assert_eq!(decode(&encode(&v)), Ok(v));
}

#[test]
fn encoding_is_deterministic() {
    assert_eq!(encode(&every_variant()), encode(&every_variant()));
}

#[test]
fn a_newer_format_is_refused_but_its_recipe_can_still_be_shown() {
    let text = encode(&every_variant()).replacen("\"format\":1", "\"format\":7", 1);
    assert_eq!(decode(&text), Err(DecodeError::NewerVersion(7)));
    assert_eq!(format_of(&text), Some(7));
    let (name, recipe) = decode_recipe_lenient(&text).unwrap();
    assert_eq!((name, recipe), (every_variant().name, every_recipe()));
    // A newer kind of node cannot be shown.
    let unknown = text.replacen("{\"tool\":\"curve\"}", "{\"spline\":\"curve\"}", 1);
    assert_ne!(unknown, text);
    assert_eq!(decode_recipe_lenient(&unknown), None);
    assert!(DecodeError::NewerVersion(7).to_string().contains("newer version"));
}

#[test]
fn broken_texts_are_refused_with_a_reason() {
    let good = encode(&every_variant());
    let cases = [
        String::new(),
        "[]".to_owned(),
        "{}".to_owned(),
        good.replacen("\"format\":1", "\"format\":0", 1),
        good.replacen("\"format\":1", "\"format\":\"1\"", 1),
        good.replacen("\"kind\":\"source\"", "\"kind\":\"output\"", 1),
        good.replacen("\"op\":\"set_param\"", "\"op\":\"paint\"", 1),
        good.replacen("\"next_id\":", "\"next\":", 1),
        good[..good.len() - 1].to_owned(),
    ];
    for bad in cases {
        assert!(matches!(decode(&bad), Err(DecodeError::Malformed(_))), "{}", &bad[..bad.len().min(60)]);
    }
}

#[test]
fn a_graph_with_a_loop_is_refused() {
    let mut r = Recipe::default();
    let a = r.graph.insert_serial(r.graph.source(), tool("exposure"), Params::new()).unwrap();
    let b = r.graph.insert_serial(a, tool("exposure"), Params::new()).unwrap();
    assert_eq!((a.get(), b.get()), (2, 3));
    let v = Variant { name: "x".to_owned(), recipe: r, history: None, layout: None };
    // Source → a becomes b → a: a loop a → b → a.
    let text = encode(&v);
    let looped = text.replacen("[0,0,2,0]", "[3,0,2,0]", 1);
    assert_ne!(looped, text);
    assert!(matches!(decode(&looped), Err(DecodeError::Malformed(_))));
}
