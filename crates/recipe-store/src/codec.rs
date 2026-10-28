//! The recipe, its masks and the edits as JSON values, both ways.
//!
//! Enums are written as a plain string when the variant has no data
//! (`"as_shot"`) and as an object with one key otherwise
//! (`{"multipliers": [2.1, 1, 1.4]}`). Node ids are numbers, tool and
//! parameter ids strings, positions inside the image `[x, y]` fractions.

use commands::{Edit, SavedHistory, SavedStep};
use core_model::{
    AiMask, AiTarget, BodyPart, BrushMask, BrushStroke, DevelopSettings, Geometry, LensSettings, Link, MaskComponent,
    MaskDef, MaskOp, MaskSource, ModelId, Node, NodeGraph, NodeId, NodeKind, NormPoint, NormRect, ParamId, ParamValue,
    Params, ProfileChoice, PromptPoint, RasterKey, Recipe, ToolId, WhiteBalance,
};

use crate::Layout;
use crate::json::{Value, field, required};

type Fields = [(String, Value)];

// ----- helpers -----

/// `None` as `null`.
fn opt(v: Option<Value>) -> Value {
    v.unwrap_or(Value::Null)
}

/// A field that may be missing or `null`.
fn optional<'a>(fields: &'a Fields, key: &str) -> Option<&'a Value> {
    field(fields, key).filter(|v| **v != Value::Null)
}

fn f32s(values: &[f32]) -> Value {
    Value::Array(values.iter().map(|v| Value::f32(*v)).collect())
}

fn read_f32s<const N: usize>(v: &Value, what: &str) -> Result<[f32; N], String> {
    let items = v.as_array(what)?;
    if items.len() != N {
        return Err(format!("{what}: expected {N} numbers"));
    }
    let mut out = [0.0; N];
    for (o, item) in out.iter_mut().zip(items) {
        *o = item.as_f32(what)?;
    }
    Ok(out)
}

/// An enum variant: a plain string, or an object with one key and its data.
fn tag<'a>(v: &'a Value, what: &str) -> Result<(&'a str, Option<&'a Value>), String> {
    match v {
        Value::String(s) => Ok((s, None)),
        Value::Object(fields) if fields.len() == 1 => Ok((&fields[0].0, Some(&fields[0].1))),
        _ => Err(format!("{what}: expected a name or an object with one key")),
    }
}

/// The data of a variant that needs some.
fn data<'a>(d: Option<&'a Value>, what: &str) -> Result<&'a Value, String> {
    d.ok_or_else(|| format!("{what}: data missing"))
}

fn tagged(name: &str, value: Value) -> Value {
    Value::Object(vec![(name.to_owned(), value)])
}

fn id(v: NodeId) -> Value {
    Value::uint(v.get())
}

fn read_id(v: &Value, what: &str) -> Result<NodeId, String> {
    v.as_u32(what).map(NodeId::from_raw)
}

fn read_tool(v: &Value, what: &str) -> Result<ToolId, String> {
    ToolId::new(v.as_str(what)?).map_err(|e| format!("{what}: {e}"))
}

fn read_param(v: &Value, what: &str) -> Result<ParamId, String> {
    ParamId::new(v.as_str(what)?).map_err(|e| format!("{what}: {e}"))
}

fn ids(list: &[NodeId]) -> Value {
    Value::Array(list.iter().map(|n| id(*n)).collect())
}

fn read_ids(v: &Value, what: &str) -> Result<Vec<NodeId>, String> {
    v.as_array(what)?.iter().map(|n| read_id(n, what)).collect()
}

// ----- development -----

fn white_balance(wb: WhiteBalance) -> Value {
    match wb {
        WhiteBalance::AsShot => Value::text("as_shot"),
        WhiteBalance::Multipliers(m) => tagged("multipliers", f32s(&m)),
    }
}

fn read_white_balance(v: &Value) -> Result<WhiteBalance, String> {
    match tag(v, "white balance")? {
        ("as_shot", None) => Ok(WhiteBalance::AsShot),
        ("multipliers", Some(m)) => Ok(WhiteBalance::Multipliers(read_f32s(m, "white balance")?)),
        (other, _) => Err(format!("unknown white balance '{other}'")),
    }
}

fn profile(p: &ProfileChoice) -> Value {
    match p {
        ProfileChoice::Builtin => Value::text("builtin"),
        ProfileChoice::Named(name) => tagged("named", Value::text(name)),
    }
}

fn read_profile(v: &Value) -> Result<ProfileChoice, String> {
    match tag(v, "profile")? {
        ("builtin", None) => Ok(ProfileChoice::Builtin),
        ("named", Some(n)) => Ok(ProfileChoice::Named(n.as_str("profile")?.to_owned())),
        (other, _) => Err(format!("unknown profile '{other}'")),
    }
}

fn lens(l: &LensSettings) -> Value {
    Value::object([
        ("enabled", Value::Bool(l.enabled)),
        ("lens", opt(l.lens.as_deref().map(Value::text))),
        ("distortion", Value::Bool(l.distortion)),
        ("tca", Value::Bool(l.tca)),
        ("vignetting", Value::Bool(l.vignetting)),
        ("zoom", opt(l.zoom.map(Value::f32))),
    ])
}

fn read_lens(v: &Value) -> Result<LensSettings, String> {
    let f = v.as_object("lens corrections")?;
    let flag = |key: &str| required(f, key)?.as_bool(key);
    Ok(LensSettings {
        enabled: flag("enabled")?,
        lens: optional(f, "lens").map(|v| v.as_str("lens").map(str::to_owned)).transpose()?,
        distortion: flag("distortion")?,
        tca: flag("tca")?,
        vignetting: flag("vignetting")?,
        zoom: optional(f, "zoom").map(|v| v.as_f32("zoom")).transpose()?,
    })
}

fn develop(d: &DevelopSettings) -> Value {
    Value::object([
        ("white_balance", white_balance(d.white_balance)),
        ("profile", profile(&d.profile)),
        ("lens", lens(&d.lens)),
    ])
}

fn read_develop(v: &Value) -> Result<DevelopSettings, String> {
    let f = v.as_object("develop")?;
    Ok(DevelopSettings {
        white_balance: read_white_balance(required(f, "white_balance")?)?,
        profile: read_profile(required(f, "profile")?)?,
        lens: read_lens(required(f, "lens")?)?,
    })
}

fn geometry(g: &Geometry) -> Value {
    Value::object([
        ("angle", Value::f32(g.angle)),
        ("vertical", Value::f32(g.vertical)),
        ("horizontal", Value::f32(g.horizontal)),
        ("crop", opt(g.crop.map(rect))),
    ])
}

fn read_geometry(v: &Value) -> Result<Geometry, String> {
    let f = v.as_object("geometry")?;
    let num = |key: &str| required(f, key)?.as_f32(key);
    Ok(Geometry {
        angle: num("angle")?,
        vertical: num("vertical")?,
        horizontal: num("horizontal")?,
        crop: optional(f, "crop").map(|v| read_rect(v, "crop")).transpose()?,
    }
    .sanitized())
}

// ----- parameters -----

/// Floats as numbers, switches as `true`/`false`, choices as `{"choice": n}`.
fn param_value(v: ParamValue) -> Value {
    match v {
        ParamValue::Float(x) => Value::f32(x),
        ParamValue::Bool(b) => Value::Bool(b),
        ParamValue::Choice(c) => tagged("choice", Value::uint(c)),
    }
}

fn read_param_value(v: &Value, what: &str) -> Result<ParamValue, String> {
    match v {
        Value::Bool(b) => Ok(ParamValue::Bool(*b)),
        Value::Object(f) => match f.as_slice() {
            [(k, n)] if k == "choice" => Ok(ParamValue::Choice(n.as_u32(what)?)),
            _ => Err(format!("{what}: unknown value")),
        },
        other => Ok(ParamValue::Float(other.as_f32(what)?)),
    }
}

fn params(p: &Params) -> Value {
    Value::Object(p.iter().map(|(k, v)| (k.as_str().to_owned(), param_value(*v))).collect())
}

fn read_params(v: &Value) -> Result<Params, String> {
    let mut p = Params::new();
    for (k, value) in v.as_object("parameters")? {
        let id = ParamId::new(k).map_err(|e| format!("parameter '{k}': {e}"))?;
        p.set(id, read_param_value(value, k)?);
    }
    Ok(p)
}

// ----- masks -----

fn rect(r: NormRect) -> Value {
    f32s(&[r.min().x(), r.min().y(), r.max().x(), r.max().y()])
}

fn read_rect(v: &Value, what: &str) -> Result<NormRect, String> {
    let [x0, y0, x1, y1] = read_f32s(v, what)?;
    let corner = |x, y| NormPoint::new(x, y).ok_or_else(|| format!("{what}: not a finite point"));
    Ok(NormRect::new(corner(x0, y0)?, corner(x1, y1)?))
}

fn target(t: &AiTarget) -> Value {
    match t {
        AiTarget::Subject => Value::text("subject"),
        AiTarget::Object { points, rect: r } => {
            let clicks = points
                .iter()
                .map(|p| Value::Array(vec![Value::f32(p.at.x()), Value::f32(p.at.y()), Value::Bool(p.include)]))
                .collect();
            tagged("object", Value::object([("points", Value::Array(clicks)), ("rect", opt(r.map(rect)))]))
        }
        AiTarget::PersonPart { person, part } => {
            tagged("person_part", Value::object([("person", rect(*person)), ("part", Value::text(part.id()))]))
        }
    }
}

fn read_target(v: &Value) -> Result<AiTarget, String> {
    match tag(v, "AI target")? {
        ("subject", None) => Ok(AiTarget::Subject),
        ("object", d) => {
            let f = data(d, "object")?.as_object("object")?;
            let mut points = Vec::new();
            for click in required(f, "points")?.as_array("points")? {
                let items = click.as_array("click")?;
                let [x, y, include] = items else { return Err("click: expected [x, y, include]".to_owned()) };
                let at = NormPoint::new(x.as_f32("click")?, y.as_f32("click")?)
                    .ok_or_else(|| "click: not a finite point".to_owned())?;
                points.push(PromptPoint { at, include: include.as_bool("click")? });
            }
            let rect = optional(f, "rect").map(|r| read_rect(r, "rectangle")).transpose()?;
            Ok(AiTarget::Object { points, rect })
        }
        ("person_part", d) => {
            let f = data(d, "person part")?.as_object("person part")?;
            let name = required(f, "part")?.as_str("part")?;
            let part = BodyPart::from_id(name).ok_or_else(|| format!("unknown body part '{name}'"))?;
            Ok(AiTarget::PersonPart { person: read_rect(required(f, "person")?, "person")?, part })
        }
        (other, _) => Err(format!("unknown AI target '{other}'")),
    }
}

fn stroke(s: &BrushStroke) -> Value {
    let flat: Vec<f32> = s.points.iter().flat_map(|p| [p.x(), p.y()]).collect();
    Value::object([
        ("points", f32s(&flat)),
        ("radius", Value::f32(s.radius)),
        ("hardness", Value::f32(s.hardness)),
        ("opacity", Value::f32(s.opacity)),
    ])
}

fn read_stroke(v: &Value) -> Result<BrushStroke, String> {
    let f = v.as_object("stroke")?;
    let flat = required(f, "points")?.as_array("stroke points")?;
    if flat.len() % 2 != 0 {
        return Err("stroke points: an odd count of numbers".to_owned());
    }
    let mut points = Vec::with_capacity(flat.len() / 2);
    for pair in flat.chunks_exact(2) {
        let p = NormPoint::new(pair[0].as_f32("stroke point")?, pair[1].as_f32("stroke point")?)
            .ok_or_else(|| "stroke point: not finite".to_owned())?;
        points.push(p);
    }
    let num = |key: &str| required(f, key)?.as_f32(key);
    Ok(BrushStroke { points, radius: num("radius")?, hardness: num("hardness")?, opacity: num("opacity")? })
}

fn op(o: MaskOp) -> Value {
    Value::text(match o {
        MaskOp::Add => "add",
        MaskOp::Subtract => "subtract",
        MaskOp::Intersect => "intersect",
    })
}

fn read_op(v: &Value) -> Result<MaskOp, String> {
    match v.as_str("mask operation")? {
        "add" => Ok(MaskOp::Add),
        "subtract" => Ok(MaskOp::Subtract),
        "intersect" => Ok(MaskOp::Intersect),
        other => Err(format!("unknown mask operation '{other}'")),
    }
}

fn component(c: &MaskComponent) -> Value {
    let source = match &c.source {
        MaskSource::Ai(ai) => tagged(
            "ai",
            Value::object([
                ("target", target(&ai.target)),
                ("model", Value::text(ai.model.as_str())),
                ("raster", opt(ai.raster.map(|k| Value::String(k.to_string())))),
            ]),
        ),
        MaskSource::Brush(b) => tagged(
            "brush",
            Value::object([
                ("strokes", Value::Array(b.strokes.iter().map(stroke).collect())),
                ("edge_aware", Value::Bool(b.edge_aware)),
            ]),
        ),
    };
    Value::object([
        ("label", Value::text(&c.label)),
        ("op", op(c.op)),
        ("enabled", Value::Bool(c.enabled)),
        ("source", source),
    ])
}

fn read_component(v: &Value) -> Result<MaskComponent, String> {
    let f = v.as_object("mask component")?;
    let source = match tag(required(f, "source")?, "mask source")? {
        ("ai", d) => {
            let a = data(d, "AI mask")?.as_object("AI mask")?;
            let model = required(a, "model")?.as_str("model")?;
            let raster = optional(a, "raster")
                .map(|r| {
                    let hex = r.as_str("raster")?;
                    RasterKey::from_hex(hex).ok_or_else(|| format!("raster key '{hex}' is not valid"))
                })
                .transpose()?;
            MaskSource::Ai(AiMask {
                target: read_target(required(a, "target")?)?,
                model: ModelId::new(model).map_err(|e| format!("model: {e}"))?,
                raster,
            })
        }
        ("brush", d) => {
            let b = data(d, "brush")?.as_object("brush")?;
            let strokes =
                required(b, "strokes")?.as_array("strokes")?.iter().map(read_stroke).collect::<Result<_, _>>()?;
            MaskSource::Brush(BrushMask { strokes, edge_aware: required(b, "edge_aware")?.as_bool("edge_aware")? })
        }
        (other, _) => return Err(format!("unknown mask source '{other}'")),
    };
    Ok(MaskComponent {
        label: required(f, "label")?.as_str("label")?.to_owned(),
        op: read_op(required(f, "op")?)?,
        enabled: required(f, "enabled")?.as_bool("enabled")?,
        source,
    })
}

fn mask_def(d: &MaskDef) -> Value {
    Value::object([
        ("components", Value::Array(d.components.iter().map(component).collect())),
        ("refine_edges", Value::Bool(d.refine_edges)),
        ("feather", Value::f32(d.feather)),
        ("opacity", Value::f32(d.opacity)),
        ("invert", Value::Bool(d.invert)),
    ])
}

fn read_mask_def(v: &Value) -> Result<MaskDef, String> {
    let f = v.as_object("mask")?;
    let components =
        required(f, "components")?.as_array("components")?.iter().map(read_component).collect::<Result<_, _>>()?;
    Ok(MaskDef {
        components,
        refine_edges: required(f, "refine_edges")?.as_bool("refine_edges")?,
        feather: required(f, "feather")?.as_f32("feather")?,
        opacity: required(f, "opacity")?.as_f32("opacity")?,
        invert: required(f, "invert")?.as_bool("invert")?,
    })
}

// ----- graph -----

fn kind(k: &NodeKind) -> Value {
    match k {
        NodeKind::Source => Value::text("source"),
        NodeKind::Output => Value::text("output"),
        NodeKind::Mask => Value::text("mask"),
        NodeKind::Tool(t) => tagged("tool", Value::text(t.as_str())),
        NodeKind::Parallel { inputs } => tagged("parallel", Value::uint(*inputs)),
    }
}

fn read_kind(v: &Value) -> Result<NodeKind, String> {
    match tag(v, "node kind")? {
        ("source", None) => Ok(NodeKind::Source),
        ("output", None) => Ok(NodeKind::Output),
        ("mask", None) => Ok(NodeKind::Mask),
        ("tool", Some(t)) => Ok(NodeKind::Tool(read_tool(t, "tool")?)),
        ("parallel", Some(n)) => Ok(NodeKind::Parallel { inputs: n.as_u8("parallel inputs")? }),
        (other, _) => Err(format!("unknown node kind '{other}'")),
    }
}

fn node(n: &Node) -> Value {
    Value::object([
        ("id", id(n.id)),
        ("kind", kind(&n.kind)),
        ("enabled", Value::Bool(n.enabled)),
        ("label", Value::text(&n.label)),
        ("params", params(&n.params)),
        ("mask", opt(n.mask.as_ref().map(mask_def))),
    ])
}

fn read_node(v: &Value) -> Result<Node, String> {
    let f = v.as_object("node")?;
    Ok(Node {
        id: read_id(required(f, "id")?, "node id")?,
        kind: read_kind(required(f, "kind")?)?,
        enabled: required(f, "enabled")?.as_bool("enabled")?,
        label: required(f, "label")?.as_str("label")?.to_owned(),
        params: read_params(required(f, "params")?)?,
        mask: optional(f, "mask").map(read_mask_def).transpose()?,
    })
}

fn link(l: &Link) -> Value {
    Value::Array(vec![id(l.from), Value::uint(l.from_port), id(l.to), Value::uint(l.to_port)])
}

fn read_link(v: &Value) -> Result<Link, String> {
    let [from, from_port, to, to_port] = v.as_array("link")? else {
        return Err("link: expected [from, port, to, port]".to_owned());
    };
    Ok(Link {
        from: read_id(from, "link")?,
        from_port: from_port.as_u8("link port")?,
        to: read_id(to, "link")?,
        to_port: to_port.as_u8("link port")?,
    })
}

fn graph(g: &NodeGraph) -> Value {
    Value::object([
        ("next_id", Value::uint(g.next_id())),
        ("nodes", Value::Array(g.nodes().map(node).collect())),
        ("links", Value::Array(g.links().map(link).collect())),
    ])
}

fn read_graph(v: &Value) -> Result<NodeGraph, String> {
    let f = v.as_object("graph")?;
    let nodes = required(f, "nodes")?.as_array("nodes")?.iter().map(read_node).collect::<Result<_, _>>()?;
    let links = required(f, "links")?.as_array("links")?.iter().map(read_link).collect::<Result<_, _>>()?;
    let next_id = required(f, "next_id")?.as_u32("next_id")?;
    NodeGraph::from_parts(nodes, links, next_id).map_err(|e| e.to_string())
}

/// A recipe as a JSON value.
pub(crate) fn recipe(r: &Recipe) -> Value {
    Value::object([("develop", develop(&r.develop)), ("graph", graph(&r.graph)), ("geometry", geometry(&r.geometry))])
}

/// A recipe read back.
pub(crate) fn read_recipe(v: &Value) -> Result<Recipe, String> {
    let f = v.as_object("recipe")?;
    Ok(Recipe {
        develop: read_develop(required(f, "develop")?)?,
        graph: read_graph(required(f, "graph")?)?,
        // Recipes saved before geometry existed have none.
        geometry: optional(f, "geometry").map(read_geometry).transpose()?.unwrap_or_default(),
    })
}

// ----- edits -----

/// An edit as a JSON object: `"op"` names it, the other keys hold its data.
pub(crate) fn edit(e: &Edit) -> Value {
    let o = |name: &str, mut rest: Vec<(String, Value)>| {
        rest.insert(0, ("op".to_owned(), Value::text(name)));
        Value::Object(rest)
    };
    let kv = |k: &str, v: Value| (k.to_owned(), v);
    match e {
        Edit::SetParam { node, param, value } => o(
            "set_param",
            vec![kv("node", id(*node)), kv("param", Value::text(param.as_str())), kv("value", param_value(*value))],
        ),
        Edit::SetParams { node, values } => {
            let list =
                values.iter().map(|(p, v)| Value::Array(vec![Value::text(p.as_str()), param_value(*v)])).collect();
            o("set_params", vec![kv("node", id(*node)), kv("values", Value::Array(list))])
        }
        Edit::ResetParam { node, param } => {
            o("reset_param", vec![kv("node", id(*node)), kv("param", Value::text(param.as_str()))])
        }
        Edit::InsertSerial { after, tool, params: p } => o(
            "insert_serial",
            vec![kv("after", id(*after)), kv("tool", Value::text(tool.as_str())), kv("params", params(p))],
        ),
        Edit::RemoveNode { node } => o("remove_node", vec![kv("node", id(*node))]),
        Edit::Connect(l) => o("connect", vec![kv("link", link(l))]),
        Edit::Disconnect { to, port } => o("disconnect", vec![kv("to", id(*to)), kv("port", Value::uint(*port))]),
        Edit::SetEnabled { node, enabled } => {
            o("set_enabled", vec![kv("node", id(*node)), kv("enabled", Value::Bool(*enabled))])
        }
        Edit::SetWhiteBalance(wb) => o("set_white_balance", vec![kv("white_balance", white_balance(*wb))]),
        Edit::SetCameraProfile(p) => o("set_camera_profile", vec![kv("profile", profile(p))]),
        Edit::SetLensCorrection(l) => o("set_lens_correction", vec![kv("lens", lens(l))]),
        Edit::SetGeometry(g) => o("set_geometry", vec![kv("geometry", geometry(g))]),
        Edit::AddMask { to, def } => o("add_mask", vec![kv("to", id(*to)), kv("mask", mask_def(def))]),
        Edit::SetMask { node, def } => o("set_mask", vec![kv("node", id(*node)), kv("mask", mask_def(def))]),
    }
}

/// An edit read back.
pub(crate) fn read_edit(v: &Value) -> Result<Edit, String> {
    let f = v.as_object("edit")?;
    let node = |key: &str| read_id(required(f, key)?, key);
    let param = |key: &str| read_param(required(f, key)?, key);
    let name = required(f, "op")?.as_str("op")?;
    Ok(match name {
        "set_param" => Edit::SetParam {
            node: node("node")?,
            param: param("param")?,
            value: read_param_value(required(f, "value")?, "value")?,
        },
        "set_params" => {
            let mut values = Vec::new();
            for pair in required(f, "values")?.as_array("values")? {
                let [p, value] = pair.as_array("value")? else {
                    return Err("values: expected [parameter, value]".to_owned());
                };
                values.push((read_param(p, "parameter")?, read_param_value(value, "value")?));
            }
            Edit::SetParams { node: node("node")?, values }
        }
        "reset_param" => Edit::ResetParam { node: node("node")?, param: param("param")? },
        "insert_serial" => Edit::InsertSerial {
            after: node("after")?,
            tool: read_tool(required(f, "tool")?, "tool")?,
            params: read_params(required(f, "params")?)?,
        },
        "remove_node" => Edit::RemoveNode { node: node("node")? },
        "connect" => Edit::Connect(read_link(required(f, "link")?)?),
        "disconnect" => Edit::Disconnect { to: node("to")?, port: required(f, "port")?.as_u8("port")? },
        "set_enabled" => Edit::SetEnabled { node: node("node")?, enabled: required(f, "enabled")?.as_bool("enabled")? },
        "set_white_balance" => Edit::SetWhiteBalance(read_white_balance(required(f, "white_balance")?)?),
        "set_camera_profile" => Edit::SetCameraProfile(read_profile(required(f, "profile")?)?),
        "set_lens_correction" => Edit::SetLensCorrection(read_lens(required(f, "lens")?)?),
        "set_geometry" => Edit::SetGeometry(read_geometry(required(f, "geometry")?)?),
        "add_mask" => Edit::AddMask { to: node("to")?, def: read_mask_def(required(f, "mask")?)? },
        "set_mask" => Edit::SetMask { node: node("node")?, def: read_mask_def(required(f, "mask")?)? },
        other => return Err(format!("unknown edit '{other}'")),
    })
}

// ----- history and layout -----

/// A saved history as a JSON object.
pub(crate) fn history(h: &SavedHistory) -> Value {
    let steps = h
        .steps
        .iter()
        .map(|s| {
            Value::object([
                ("edit", edit(&s.edit)),
                ("created", ids(&s.created)),
                ("skipped", opt(s.skipped.as_deref().map(Value::text))),
            ])
        })
        .collect();
    Value::object([
        ("base", recipe(&h.base)),
        ("steps", Value::Array(steps)),
        ("redo", Value::Array(h.redo.iter().map(edit).collect())),
    ])
}

/// A saved history read back.
pub(crate) fn read_history(v: &Value) -> Result<SavedHistory, String> {
    let f = v.as_object("history")?;
    let mut steps = Vec::new();
    for s in required(f, "steps")?.as_array("steps")? {
        let sf = s.as_object("step")?;
        steps.push(SavedStep {
            edit: read_edit(required(sf, "edit")?)?,
            created: read_ids(required(sf, "created")?, "created")?,
            skipped: optional(sf, "skipped").map(|w| w.as_str("skipped").map(str::to_owned)).transpose()?,
        });
    }
    let redo = required(f, "redo")?.as_array("redo")?.iter().map(read_edit).collect::<Result<_, _>>()?;
    Ok(SavedHistory { base: read_recipe(required(f, "base")?)?, steps, redo })
}

/// Node positions, pan and zoom as a JSON object.
pub(crate) fn layout(l: &Layout) -> Value {
    let nodes = l.nodes.iter().map(|(n, [x, y])| Value::Array(vec![id(*n), Value::f32(*x), Value::f32(*y)])).collect();
    Value::object([("pan", f32s(&l.pan)), ("zoom", Value::f32(l.zoom)), ("nodes", Value::Array(nodes))])
}

/// Node positions read back.
pub(crate) fn read_layout(v: &Value) -> Result<Layout, String> {
    let f = v.as_object("layout")?;
    let mut nodes = Vec::new();
    for item in required(f, "nodes")?.as_array("positions")? {
        let [n, x, y] = item.as_array("position")? else {
            return Err("position: expected [node, x, y]".to_owned());
        };
        nodes.push((read_id(n, "position")?, [x.as_f32("position")?, y.as_f32("position")?]));
    }
    Ok(Layout { nodes, pan: read_f32s(required(f, "pan")?, "pan")?, zoom: required(f, "zoom")?.as_f32("zoom")? })
}
