#![allow(clippy::unwrap_used, clippy::float_cmp)]

use commands::{Edit, SavedHistory, SavedStep};
use core_model::{
    AiTarget, BodyPart, BrushMask, BrushStroke, DevelopSettings, LayerId, Layers, LensSettings, MaskComponent, MaskDef,
    MaskOp, MaskSource, ModelId, NormPoint, NormRect, ParamId, ParamValue, Params, ProfileChoice, PromptPoint,
    RasterKey, Recipe, StyleTool, ToolId, ToolRef, Tools, WhiteBalance,
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
    let linear =
        MaskComponent::linear("Sky", MaskOp::Add, core_model::LinearGradient { start: p(0.5, 0.1), end: p(0.5, 0.45) });
    let radial = MaskComponent::radial(
        "Face",
        MaskOp::Subtract,
        core_model::RadialGradient { centre: p(0.4, 0.6), radius: [0.15, 0.25], feather: 0.75 },
    );
    MaskDef {
        components: vec![subject, object, clicks_only, part, brush, empty, linear, radial],
        refine_edges: false,
        feather: 0.012_5,
        opacity: 0.7,
        invert: true,
    }
}

/// A recipe with every kind of layer, parameter value and development setting.
fn every_recipe() -> Recipe {
    let mut values = Params::new();
    values.set(param("exposure"), ParamValue::Float(-1.234_567_9));
    values.set(param("highlights"), ParamValue::Float(0.1));
    values.set(param("enabled_flag"), ParamValue::Bool(true));
    values.set(param("mode"), ParamValue::Choice(3));
    let mut layers = Layers::new();
    *layers.background.params_mut(&tool("exposure"), 0).unwrap() = values.clone();
    layers.background.params_mut(&tool("retouch"), 0).unwrap();
    *layers.background.params_mut(&tool("retouch"), 1).unwrap() = values;
    layers.background.set_enabled(&tool("curve"), false);
    let gone = layers.add("Gone".to_owned(), MaskDef::default(), Tools::new());
    let mut tools = Tools::new();
    tools.params_mut(&tool("clarity"), 0).unwrap().set(param("amount"), ParamValue::Float(0.5));
    let a = layers.add("Contrast \u{e8} \\ / \n".to_owned(), every_mask(), tools);
    layers.add("Empty".to_owned(), MaskDef::default(), Tools::new());
    layers.layer_mut(a).unwrap().enabled = false;
    layers.remove(gone).unwrap();
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
                amount: core_model::LensAmounts { distortion: 0.5, tca: 1.25, vignetting: 2.0 },
                manual: core_model::ManualLens { distortion: 0.1, vignetting: -0.35 },
                purple_fringing: 0.4,
                custom: Some("My zoom".to_owned()),
            },
        },
        layers,
        geometry: core_model::Geometry {
            angle: -3.25,
            vertical: 12.5,
            horizontal: -4.0,
            crop: Some(core_model::NormRect::new(
                core_model::NormPoint::new(0.1, 0.2).unwrap(),
                core_model::NormPoint::new(0.85, 0.9).unwrap(),
            )),
        },
        curve: core_model::BaseCurve::Linear,
    }
}

/// One edit of every kind.
fn every_edit() -> Vec<Edit> {
    let at = ToolRef::new(LayerId::from_raw(2), tool("exposure"));
    let mut values = Params::new();
    values.set(param("exposure"), ParamValue::Float(0.5));
    let tools = vec![StyleTool { tool: tool("exposure"), params: values.clone() }];
    vec![
        Edit::SetParam { at: at.clone(), param: param("exposure"), value: ParamValue::Float(0.75) },
        Edit::SetParam { at: at.clone().with_slot(3), param: param("auto"), value: ParamValue::Bool(false) },
        Edit::SetParams {
            at: ToolRef::background(tool("white_balance")),
            values: vec![
                (param("mode"), ParamValue::Choice(1)),
                (param("kelvin"), ParamValue::Float(5500.0)),
                (param("mode"), ParamValue::Choice(2)),
            ],
        },
        Edit::ResetParam { at, param: param("exposure") },
        Edit::ResetTool { layer: LayerId::BACKGROUND, tool: tool("hdr") },
        Edit::SetToolEnabled { layer: LayerId::from_raw(4), tool: tool("curve"), enabled: false },
        Edit::AddLayer { name: "Dodge".to_owned(), mask: every_mask(), tools: tools.clone() },
        Edit::AddLayer { name: String::new(), mask: MaskDef::default(), tools: Vec::new() },
        Edit::RemoveLayer { layer: LayerId::from_raw(2) },
        Edit::RenameLayer { layer: LayerId::from_raw(3), name: "Sky \u{2014} \"blue\"".to_owned() },
        Edit::MoveLayer { layer: LayerId::from_raw(3), index: 0 },
        Edit::SetLayerEnabled { layer: LayerId::from_raw(3), enabled: false },
        Edit::SetLayerMask { layer: LayerId::from_raw(5), mask: MaskDef::default() },
        Edit::SetWhiteBalance(WhiteBalance::AsShot),
        Edit::SetWhiteBalance(WhiteBalance::Multipliers([1.9, 1.0, 1.3])),
        Edit::SetCameraProfile(ProfileChoice::Builtin),
        Edit::SetCameraProfile(ProfileChoice::Named("Portrait".to_owned())),
        Edit::SetLensCorrection(LensSettings::off()),
        Edit::SetLensCorrection(LensSettings::default()),
        Edit::SetGeometry(core_model::Geometry { angle: 2.0, ..core_model::Geometry::default() }),
        Edit::SetBaseCurve(core_model::BaseCurve::Linear),
        Edit::ApplyStyle {
            name: "Warm".to_owned(),
            style: Box::new(core_model::Style {
                white_balance: Some(WhiteBalance::AsShot),
                geometry: Some(core_model::Geometry { angle: -1.0, ..core_model::Geometry::default() }),
                tools,
                ..core_model::Style::default()
            }),
        },
    ]
}

fn every_variant() -> Variant {
    let recipe = every_recipe();
    let steps = every_edit()
        .into_iter()
        .enumerate()
        .map(|(i, edit)| SavedStep {
            edit,
            created: if i % 4 == 0 { vec![LayerId::from_raw(7), LayerId::from_raw(8)] } else { vec![] },
            skipped: (i == 3).then(|| "it acts on a layer made by a removed step".to_owned()),
        })
        .collect();
    let history = SavedHistory { base: Recipe::default(), steps, redo: every_edit() };
    Variant { name: "Variant 2 \u{b7} \u{1f642}".to_owned(), recipe, history: Some(history) }
}

#[test]
fn every_edit_layer_and_mask_kind_round_trips() {
    let v = every_variant();
    let text = encode(&v);
    assert!(text.starts_with("{\"format\":2,"), "{}", &text[..40]);
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
fn a_variant_without_history_round_trips() {
    let v = Variant { name: String::new(), recipe: Recipe::default(), history: None };
    assert_eq!(decode(&encode(&v)), Ok(v));
}

#[test]
fn an_edit_of_the_node_graph_format_is_obsolete_and_not_shown() {
    let text = r#"{"format":1,"name":"Variant 1","recipe":{"develop":{},"graph":{"next_id":2,"nodes":[],"links":[]}},"history":null,"layout":null}"#;
    assert_eq!(decode(text), Err(DecodeError::Obsolete(1)));
    assert_eq!(format_of(text), Some(1));
    assert_eq!(decode_recipe_lenient(text), None);
    assert!(DecodeError::Obsolete(1).to_string().contains("no longer reads"));
}

#[test]
fn encoding_is_deterministic() {
    assert_eq!(encode(&every_variant()), encode(&every_variant()));
}

#[test]
fn a_newer_format_is_refused_but_its_recipe_can_still_be_shown() {
    let text = encode(&every_variant()).replacen("\"format\":2", "\"format\":7", 1);
    assert_eq!(decode(&text), Err(DecodeError::NewerVersion(7)));
    assert_eq!(format_of(&text), Some(7));
    let (name, recipe) = decode_recipe_lenient(&text).unwrap();
    assert_eq!((name, recipe), (every_variant().name, every_recipe()));
    // A newer kind of layer cannot be shown.
    let unknown = text.replacen("\"background\":", "\"ground\":", 1);
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
        good.replacen("\"format\":2", "\"format\":0", 1),
        good.replacen("\"format\":2", "\"format\":\"2\"", 1),
        good.replacen("\"slots\":[{", "\"slots\":[],\"x\":[{", 1),
        good.replacen("\"op\":\"set_param\"", "\"op\":\"paint\"", 1),
        good.replacen("\"next_id\":", "\"next\":", 1),
        good[..good.len() - 1].to_owned(),
    ];
    for bad in cases {
        assert!(matches!(decode(&bad), Err(DecodeError::Malformed(_))), "{}", &bad[..bad.len().min(60)]);
    }
}

#[test]
fn the_fuzzers_start_from_texts_of_the_current_format() {
    mutate::check_seed("recipe", "variant.json", encode(&every_variant()).as_bytes());
}

#[test]
fn damaged_texts_never_panic_and_what_is_accepted_round_trips() {
    let good = encode(&every_variant());
    for copy in mutate::text_copies(&good, 41, 3000) {
        // Compared as text: a stored "nan" is not equal to itself.
        if let Ok(v) = decode(&copy) {
            let written = encode(&v);
            assert_eq!(decode(&written).map(|again| encode(&again)), Ok(written), "accepted: {copy}");
        }
        let _ = decode_recipe_lenient(&copy);
    }
}

#[test]
fn layers_with_the_same_id_are_refused() {
    let mut r = Recipe::default();
    let a = r.layers.add("A".to_owned(), MaskDef::default(), Tools::new());
    let b = r.layers.add("B".to_owned(), MaskDef::default(), Tools::new());
    assert_eq!((a.get(), b.get()), (1, 2));
    let text = encode(&Variant { name: "x".to_owned(), recipe: r, history: None });
    let twice = text.replacen("\"id\":2,", "\"id\":1,", 1);
    assert_ne!(twice, text);
    assert!(matches!(decode(&twice), Err(DecodeError::Malformed(_))));
}
