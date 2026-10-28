//! The SCUNet network as an ONNX graph, a port of
//! `scripts/scunet/scunet_graph.py`.
//!
//! SCUNet(in_nc=3, config=[4]*7, dim=64) is a U-Net with three
//! down-sampling stages, a body and three up-sampling stages of Swin-Conv
//! blocks. Only standard opset-13 operators are used; layer normalisation
//! and GELU are written out in the form ONNX Runtime fuses.
//!
//! The output must be byte-identical to the Python one, so this follows it
//! step by step: the generated names share one counter, and every constant
//! and node is created in the order Python evaluates the expressions.

use std::collections::{BTreeMap, HashMap};

use crate::pickle::View;
use crate::proto::{self, FLOAT, INT64};
use crate::{ConvertError, Tensor};

const WINDOW: usize = 8;
const WINDOW_AREA: usize = WINDOW * WINDOW;
const HEAD_DIM: usize = 32;
const DIM: usize = 64;
const BLOCKS_PER_STAGE: usize = 4;
/// Added to the attention logits of masked pairs: large enough that the
/// softmax gives them no weight, small enough to stay finite in FP16.
const MASK_VALUE: f32 = -30000.0;
const OPSET: i64 = 13;
/// Constants with at most this many elements are shared between nodes.
const SHARED_MAX_ELEMENTS: usize = 16;

// The scalar constants are the f64 values of the Python code rounded to
// f32, as `np.float32(...)` does.
#[allow(clippy::cast_possible_truncation, reason = "np.float32() of a Python float")]
const LN_EPS: f32 = 1e-5_f64 as f32;
#[allow(clippy::cast_possible_truncation, reason = "np.float32() of a Python float")]
const SQRT_2: f32 = std::f64::consts::SQRT_2 as f32;
/// HEAD_DIM ** -0.5 = 1 / √32 = √2 / 8 (exact in binary).
#[allow(clippy::cast_possible_truncation, reason = "np.float32() of a Python float")]
const ATTENTION_SCALE: f32 = (std::f64::consts::SQRT_2 / 8.0) as f32;

/// Converts a size to the i64 of ONNX shapes and attributes (sizes here are
/// at most a few million, far from the limit).
fn int(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// The data of a constant.
#[derive(Clone, Copy)]
enum Data<'a> {
    F32(&'a [f32]),
    I64(&'a [i64]),
}

impl Data<'_> {
    fn len(self) -> usize {
        match self {
            Data::F32(d) => d.len(),
            Data::I64(d) => d.len(),
        }
    }

    fn data_type(self) -> u64 {
        match self {
            Data::F32(_) => FLOAT,
            Data::I64(_) => INT64,
        }
    }

    fn byte_len(self) -> usize {
        match self {
            Data::F32(d) => d.len() * 4,
            Data::I64(d) => d.len() * 8,
        }
    }

    fn write(self, out: &mut Vec<u8>) {
        match self {
            Data::F32(d) => out.extend(d.iter().flat_map(|v| v.to_le_bytes())),
            Data::I64(d) => out.extend(d.iter().flat_map(|v| v.to_le_bytes())),
        }
    }
}

/// The weights still to be used, as views into the checkpoint; each is
/// copied out only when it goes into the model, so the conversion holds
/// one weight at a time besides the checkpoint and the model.
struct Weights<'a>(BTreeMap<String, View<'a>>);

impl Weights<'_> {
    fn take(&mut self, name: &str) -> Result<Tensor, ConvertError> {
        self.0.remove(name).ok_or_else(|| ConvertError::MissingWeight(name.to_owned()))?.to_tensor()
    }
}

fn bad(msg: String) -> ConvertError {
    ConvertError::BadWeight(msg)
}

/// ONNX graph under construction: operators return the name of their
/// output tensor; weights are stored as initializers.
pub(crate) struct Graph {
    /// GraphProto field 1 entries (nodes), already serialised.
    nodes: Vec<u8>,
    /// GraphProto field 5 entries (initializers), already serialised.
    inits: Vec<u8>,
    /// Shared small constants: (data type, shape, bytes) → name.
    consts: HashMap<(u64, Vec<usize>, Vec<u8>), String>,
    /// Shifted-window masks by number of windows per side.
    masks: HashMap<usize, (String, String)>,
    counter: u64,
}

impl Graph {
    fn new(init_capacity: usize) -> Self {
        Self {
            nodes: Vec::new(),
            inits: Vec::with_capacity(init_capacity),
            consts: HashMap::new(),
            masks: HashMap::new(),
            counter: 0,
        }
    }

    fn name(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}_{}", self.counter)
    }

    /// An initializer; small identical constants (shapes, scalars) are shared.
    fn constant(&mut self, shape: &[usize], data: Data<'_>) -> String {
        let key = (data.len() <= SHARED_MAX_ELEMENTS).then(|| {
            let mut bytes = Vec::new();
            data.write(&mut bytes);
            (data.data_type(), shape.to_vec(), bytes)
        });
        if let Some(k) = &key
            && let Some(name) = self.consts.get(k)
        {
            return name.clone();
        }
        let name = self.name(if key.is_some() { "c" } else { "w" });
        let head = proto::tensor_head(&name, shape, data.data_type(), data.byte_len());
        proto::len_prefix(&mut self.inits, 5, head.len() + data.byte_len());
        self.inits.extend_from_slice(&head);
        data.write(&mut self.inits);
        if let Some(k) = key {
            self.consts.insert(k, name.clone());
        }
        name
    }

    fn f32s(&mut self, shape: &[usize], data: &[f32]) -> String {
        self.constant(shape, Data::F32(data))
    }

    /// A scalar constant. It has shape [1], not []: Python passes
    /// `np.float32(v)` through `np.ascontiguousarray`, which makes it 1-D.
    fn scalar(&mut self, v: f32) -> String {
        self.constant(&[1], Data::F32(&[v]))
    }

    fn weight(&mut self, t: &Tensor) -> String {
        self.f32s(&t.shape, &t.data)
    }

    fn ints(&mut self, values: &[i64]) -> String {
        self.constant(&[values.len()], Data::I64(values))
    }

    /// The shifted-window masks for a stage, created once per stage size.
    fn shift_masks(&mut self, n_win: usize) -> (String, String) {
        if let Some(m) = self.masks.get(&n_win) {
            return m.clone();
        }
        let (rows, cols) = shift_masks(n_win);
        let rows = self.f32s(&[n_win, 1, WINDOW_AREA, WINDOW_AREA], &rows);
        let cols = self.f32s(&[n_win, WINDOW_AREA, WINDOW_AREA], &cols);
        self.masks.insert(n_win, (rows.clone(), cols.clone()));
        (rows, cols)
    }

    fn push_node(&mut self, op_type: &str, inputs: &[&str], output: &str, name: &str, attrs: &[Vec<u8>]) {
        let node = proto::node(op_type, inputs, output, name, attrs);
        proto::bytes_field(&mut self.nodes, 1, &node);
    }

    fn op(&mut self, op_type: &str, inputs: &[&str], attrs: &[Vec<u8>]) -> String {
        let out = self.name(&op_type.to_lowercase());
        self.push_node(op_type, inputs, &out, &out, attrs);
        out
    }

    /// A node writing the named output of the graph; it gets a node name of
    /// its own.
    fn op_to(&mut self, op_type: &str, inputs: &[&str], attrs: &[Vec<u8>], out: &str) -> String {
        let name = self.name(&op_type.to_lowercase());
        self.push_node(op_type, inputs, out, &name, attrs);
        out.to_owned()
    }

    // --- layers -----------------------------------------------------------

    fn conv(
        &mut self,
        x: &str,
        w: &Tensor,
        b: Option<&Tensor>,
        stride: i64,
        pad: i64,
        out: Option<&str>,
    ) -> Result<String, ConvertError> {
        let &[_, _, k, _] = w.shape.as_slice() else {
            return Err(bad(format!("convolution weight of shape {:?}", w.shape)));
        };
        let k = int(k);
        let wn = self.weight(w);
        let bn = b.map(|b| self.weight(b));
        let mut inputs = vec![x, wn.as_str()];
        inputs.extend(bn.as_deref());
        let attrs = [
            proto::attr_ints("kernel_shape", &[k, k]),
            proto::attr_ints("strides", &[stride, stride]),
            proto::attr_ints("pads", &[pad; 4]),
        ];
        Ok(match out {
            Some(o) => self.op_to("Conv", &inputs, &attrs, o),
            None => self.op("Conv", &inputs, &attrs),
        })
    }

    fn conv_transpose(&mut self, x: &str, w: &Tensor) -> Result<String, ConvertError> {
        let &[_, _, k, _] = w.shape.as_slice() else {
            return Err(bad(format!("transposed convolution weight of shape {:?}", w.shape)));
        };
        let k = int(k);
        let wn = self.weight(w);
        let attrs = [proto::attr_ints("kernel_shape", &[k, k]), proto::attr_ints("strides", &[k, k])];
        Ok(self.op("ConvTranspose", &[x, &wn], &attrs))
    }

    /// y = x · wᵀ + b over the last axis (PyTorch nn.Linear).
    fn linear(&mut self, x: &str, w: &Tensor, b: &Tensor) -> Result<String, ConvertError> {
        let &[rows, cols] = w.shape.as_slice() else {
            return Err(bad(format!("linear weight of shape {:?}", w.shape)));
        };
        let mut wt = Vec::with_capacity(w.data.len());
        for c in 0..cols {
            wt.extend((0..rows).map(|r| w.data[r * cols + c]));
        }
        let wn = self.f32s(&[cols, rows], &wt);
        let mm = self.op("MatMul", &[x, &wn], &[]);
        let bn = self.weight(b);
        Ok(self.op("Add", &[&mm, &bn], &[]))
    }

    fn layer_norm(&mut self, x: &str, gamma: &Tensor, beta: &Tensor) -> String {
        let reduce = || [proto::attr_ints("axes", &[-1]), proto::attr_int("keepdims", 1)];
        let mean = self.op("ReduceMean", &[x], &reduce());
        let d = self.op("Sub", &[x, &mean], &[]);
        let two = self.scalar(2.0);
        let sq = self.op("Pow", &[&d, &two], &[]);
        let var = self.op("ReduceMean", &[&sq], &reduce());
        let eps = self.scalar(LN_EPS);
        let ve = self.op("Add", &[&var, &eps], &[]);
        let sd = self.op("Sqrt", &[&ve], &[]);
        let y = self.op("Div", &[&d, &sd], &[]);
        let g = self.weight(gamma);
        let yg = self.op("Mul", &[&y, &g], &[]);
        let b = self.weight(beta);
        self.op("Add", &[&yg, &b], &[])
    }

    /// Exact GELU: x · ½ · (1 + erf(x / √2)).
    fn gelu(&mut self, x: &str) -> String {
        let s = self.scalar(SQRT_2);
        let xs = self.op("Div", &[x, &s], &[]);
        let e = self.op("Erf", &[&xs], &[]);
        let one = self.scalar(1.0);
        let e1 = self.op("Add", &[&e, &one], &[]);
        let y = self.op("Mul", &[x, &e1], &[]);
        let half = self.scalar(0.5);
        self.op("Mul", &[&y, &half], &[])
    }

    fn slice(&mut self, x: &str, axis: i64, start: i64, end: i64) -> String {
        let s = self.ints(&[start]);
        let e = self.ints(&[end]);
        let a = self.ints(&[axis]);
        self.op("Slice", &[x, &s, &e, &a], &[])
    }

    fn concat(&mut self, xs: &[&str], axis: i64) -> String {
        self.op("Concat", xs, &[proto::attr_int("axis", axis)])
    }

    fn reshape(&mut self, x: &str, shape: &[usize]) -> String {
        let shape: Vec<i64> = shape.iter().map(|&d| int(d)).collect();
        let s = self.ints(&shape);
        self.op("Reshape", &[x, &s], &[])
    }

    fn transpose(&mut self, x: &str, perm: &[i64]) -> String {
        self.op("Transpose", &[x], &[proto::attr_ints("perm", perm)])
    }

    /// torch.roll(x, shift, axis) for an axis of length `size`.
    fn roll(&mut self, x: &str, shift: i64, axis: i64, size: usize) -> String {
        let size = int(size);
        let s = shift.rem_euclid(size);
        if s == 0 {
            return x.to_owned();
        }
        let a = self.slice(x, axis, size - s, size);
        let b = self.slice(x, axis, 0, size - s);
        self.concat(&[&a, &b], axis)
    }
}

// --- SCUNet blocks ---------------------------------------------------------

/// (heads, 2w−1, 2w−1) learned table → (heads, w², w²) bias between the
/// positions of a window.
pub(crate) fn relative_bias(params: &Tensor, heads: usize) -> Result<Vec<f32>, ConvertError> {
    let side = 2 * WINDOW - 1;
    if params.shape != [heads, side, side] {
        return Err(bad(format!("relative position table of shape {:?} for {heads} heads", params.shape)));
    }
    let mut out = Vec::with_capacity(heads * WINDOW_AREA * WINDOW_AREA);
    for h in 0..heads {
        for a in 0..WINDOW_AREA {
            for b in 0..WINDOW_AREA {
                // offsets between the two positions, shifted to 0..2w−1
                let dy = a / WINDOW + WINDOW - 1 - b / WINDOW;
                let dx = a % WINDOW + WINDOW - 1 - b % WINDOW;
                out.push(params.data[(h * side + dy) * side + dx]);
            }
        }
    }
    Ok(out)
}

/// Additive masks of the shifted windows: after the roll, the last row and
/// the last column of windows hold pixels from opposite sides of the image,
/// which must not attend to each other. Returned as a mask per window row
/// (n_win, 1, w², w²) and one per window column (n_win, w², w²), to be
/// added to the logits viewed as (…, rows, columns, w², w²): a pair is
/// masked if either says so, and the sum stays finite in FP16.
pub(crate) fn shift_masks(n_win: usize) -> (Vec<f32>, Vec<f32>) {
    let s = WINDOW - WINDOW / 2;
    let area = WINDOW_AREA * WINDOW_AREA;
    let mut rows = vec![0.0; n_win * area];
    let mut cols = vec![0.0; n_win * area];
    let last = (n_win - 1) * area;
    for a in 0..WINDOW_AREA {
        for b in 0..WINDOW_AREA {
            // pairs split by the seam: one end before the shift, the other after it
            if (a / WINDOW < s) != (b / WINDOW < s) {
                rows[last + a * WINDOW_AREA + b] = MASK_VALUE;
            }
            if (a % WINDOW < s) != (b % WINDOW < s) {
                cols[last + a * WINDOW_AREA + b] = MASK_VALUE;
            }
        }
    }
    (rows, cols)
}

/// Multi-head self-attention inside 8×8 windows of x (1, size, size, ch).
fn window_attention(
    g: &mut Graph,
    x: &str,
    p: &mut Weights<'_>,
    prefix: &str,
    size: usize,
    ch: usize,
    shifted: bool,
) -> Result<String, ConvertError> {
    let w = WINDOW;
    let half = int(w / 2);
    let heads = ch / HEAD_DIM;
    let n_win = size / w;
    let nw2 = n_win * n_win;
    let mut x = x.to_owned();
    if shifted {
        let r = g.roll(&x, -half, 1, size);
        x = g.roll(&r, -half, 2, size);
    }
    // (1, H, W, C) → (1, windows, w², C)
    let t = g.reshape(&x, &[1, n_win, w, n_win, w, ch]);
    let t = g.transpose(&t, &[0, 1, 3, 2, 4, 5]);
    let t = g.reshape(&t, &[1, nw2, WINDOW_AREA, ch]);
    let qkv = g.linear(
        &t,
        &p.take(&format!("{prefix}.embedding_layer.weight"))?,
        &p.take(&format!("{prefix}.embedding_layer.bias"))?,
    )?;
    // (1, nW, P, 3·heads·hd) → (3·heads, 1, nW, P, hd): q, k, v heads in order
    let qkv = g.reshape(&qkv, &[1, nw2, WINDOW_AREA, 3 * heads, HEAD_DIM]);
    let qkv = g.transpose(&qkv, &[3, 0, 1, 2, 4]);
    let hd = int(heads);
    let q = g.slice(&qkv, 0, 0, hd);
    let k = g.slice(&qkv, 0, hd, 2 * hd);
    let v = g.slice(&qkv, 0, 2 * hd, 3 * hd);
    let kt = g.transpose(&k, &[0, 1, 2, 4, 3]);
    let sim = g.op("MatMul", &[&q, &kt], &[]);
    let scale = g.scalar(ATTENTION_SCALE);
    let sim = g.op("Mul", &[&sim, &scale], &[]);
    let bias = relative_bias(&p.take(&format!("{prefix}.relative_position_params"))?, heads)?;
    let bias = g.f32s(&[heads, 1, 1, WINDOW_AREA, WINDOW_AREA], &bias);
    let mut sim = g.op("Add", &[&sim, &bias], &[]);
    if shifted {
        let (rows, cols) = g.shift_masks(n_win);
        let s = g.reshape(&sim, &[heads, 1, n_win, n_win, WINDOW_AREA, WINDOW_AREA]);
        let s = g.op("Add", &[&s, &rows], &[]);
        let s = g.op("Add", &[&s, &cols], &[]);
        sim = g.reshape(&s, &[heads, 1, nw2, WINDOW_AREA, WINDOW_AREA]);
    }
    let probs = g.op("Softmax", &[&sim], &[proto::attr_int("axis", -1)]);
    let out = g.op("MatMul", &[&probs, &v], &[]); // (heads, 1, nW, P, hd)
    let out = g.transpose(&out, &[1, 2, 3, 0, 4]);
    let out = g.reshape(&out, &[1, nw2, WINDOW_AREA, ch]);
    let out =
        g.linear(&out, &p.take(&format!("{prefix}.linear.weight"))?, &p.take(&format!("{prefix}.linear.bias"))?)?;
    // back to (1, H, W, C)
    let out = g.reshape(&out, &[1, n_win, n_win, w, w, ch]);
    let out = g.transpose(&out, &[0, 1, 3, 2, 4, 5]);
    let mut out = g.reshape(&out, &[1, size, size, ch]);
    if shifted {
        let r = g.roll(&out, half, 1, size);
        out = g.roll(&r, half, 2, size);
    }
    Ok(out)
}

/// Swin block on x (1, H, W, C): attention and MLP, each with a residual.
fn transformer(
    g: &mut Graph,
    x: &str,
    p: &mut Weights<'_>,
    prefix: &str,
    size: usize,
    ch: usize,
    shifted: bool,
) -> Result<String, ConvertError> {
    let a = g.layer_norm(x, &p.take(&format!("{prefix}.ln1.weight"))?, &p.take(&format!("{prefix}.ln1.bias"))?);
    let attn = window_attention(g, &a, p, &format!("{prefix}.msa"), size, ch, shifted)?;
    let x = g.op("Add", &[x, &attn], &[]);
    let m = g.layer_norm(&x, &p.take(&format!("{prefix}.ln2.weight"))?, &p.take(&format!("{prefix}.ln2.bias"))?);
    let m = g.linear(&m, &p.take(&format!("{prefix}.mlp.0.weight"))?, &p.take(&format!("{prefix}.mlp.0.bias"))?)?;
    let m = g.gelu(&m);
    let m = g.linear(&m, &p.take(&format!("{prefix}.mlp.2.weight"))?, &p.take(&format!("{prefix}.mlp.2.bias"))?)?;
    Ok(g.op("Add", &[&x, &m], &[]))
}

/// Swin-Conv block on x (1, 2·half, H, W): a 1×1 convolution splits the
/// channels into a residual convolution branch and a transformer branch, a
/// second 1×1 convolution mixes them back, plus a residual.
fn swin_conv_block(
    g: &mut Graph,
    x: &str,
    p: &mut Weights<'_>,
    prefix: &str,
    size: usize,
    half: usize,
    shifted: bool,
) -> Result<String, ConvertError> {
    let w1 = p.take(&format!("{prefix}.conv1_1.weight"))?;
    let b1 = p.take(&format!("{prefix}.conv1_1.bias"))?;
    let y = g.conv(x, &w1, Some(&b1), 1, 0, None)?;
    let h = int(half);
    let conv_x = g.slice(&y, 1, 0, h);
    let trans_x = g.slice(&y, 1, h, 2 * h);
    let c = g.conv(&conv_x, &p.take(&format!("{prefix}.conv_block.0.weight"))?, None, 1, 1, None)?;
    let c = g.op("Relu", &[&c], &[]);
    let c = g.conv(&c, &p.take(&format!("{prefix}.conv_block.2.weight"))?, None, 1, 1, None)?;
    let conv_x = g.op("Add", &[&c, &conv_x], &[]);
    let t = g.transpose(&trans_x, &[0, 2, 3, 1]);
    let t = transformer(g, &t, p, &format!("{prefix}.trans_block"), size, half, shifted)?;
    let trans_x = g.transpose(&t, &[0, 3, 1, 2]);
    let cat = g.concat(&[&conv_x, &trans_x], 1);
    let w2 = p.take(&format!("{prefix}.conv1_2.weight"))?;
    let b2 = p.take(&format!("{prefix}.conv1_2.bias"))?;
    let res = g.conv(&cat, &w2, Some(&b2), 1, 0, None)?;
    Ok(g.op("Add", &[x, &res], &[]))
}

/// BLOCKS_PER_STAGE Swin-Conv blocks, alternating plain and shifted windows.
/// In KAIR's SCUNet a window covering the whole stage is never shifted;
/// [`crate::MIN_TILE`] keeps every stage above one window.
fn stage(
    g: &mut Graph,
    x: &str,
    p: &mut Weights<'_>,
    prefix: &str,
    first: usize,
    size: usize,
    half: usize,
) -> Result<String, ConvertError> {
    let mut x = x.to_owned();
    for i in 0..BLOCKS_PER_STAGE {
        x = swin_conv_block(g, &x, p, &format!("{prefix}.{}", first + i), size, half, i % 2 == 1)?;
    }
    Ok(x)
}

/// The serialised ONNX model of SCUNet for 1×3×n×n inputs (n a multiple of
/// 64), with the KAIR weights `weights`.
pub(crate) fn build(weights: BTreeMap<String, View<'_>>, n: usize) -> Result<Vec<u8>, ConvertError> {
    // The initializers are written straight into the buffer that becomes
    // the file, so reserve about its final size: the weights, the expanded
    // relative-position biases and the masks, plus headers.
    let weight_bytes: usize = weights.values().map(|t| t.numel() * 4).sum();
    let mask_bytes = 2 * 4 * WINDOW_AREA * WINDOW_AREA * (n / 8 + n / 16 + n / 32 + n / 64);
    let mut g = Graph::new(weight_bytes + mask_bytes + (4 << 20));
    let p = &mut Weights(weights);
    let x1 = g.conv("input", &p.take("m_head.0.weight")?, None, 1, 1, None)?; // 64 × n
    let x = stage(&mut g, &x1, p, "m_down1", 0, n, DIM / 2)?;
    let x2 = g.conv(&x, &p.take("m_down1.4.weight")?, None, 2, 0, None)?; // 128 × n/2
    let x = stage(&mut g, &x2, p, "m_down2", 0, n / 2, DIM)?;
    let x3 = g.conv(&x, &p.take("m_down2.4.weight")?, None, 2, 0, None)?; // 256 × n/4
    let x = stage(&mut g, &x3, p, "m_down3", 0, n / 4, 2 * DIM)?;
    let x4 = g.conv(&x, &p.take("m_down3.4.weight")?, None, 2, 0, None)?; // 512 × n/8
    let x = stage(&mut g, &x4, p, "m_body", 0, n / 8, 4 * DIM)?;
    let x = g.op("Add", &[&x, &x4], &[]);
    let x = g.conv_transpose(&x, &p.take("m_up3.0.weight")?)?; // 256 × n/4
    let x = stage(&mut g, &x, p, "m_up3", 1, n / 4, 2 * DIM)?;
    let x = g.op("Add", &[&x, &x3], &[]);
    let x = g.conv_transpose(&x, &p.take("m_up2.0.weight")?)?; // 128 × n/2
    let x = stage(&mut g, &x, p, "m_up2", 1, n / 2, DIM)?;
    let x = g.op("Add", &[&x, &x2], &[]);
    let x = g.conv_transpose(&x, &p.take("m_up1.0.weight")?)?; // 64 × n
    let x = stage(&mut g, &x, p, "m_up1", 1, n, DIM / 2)?;
    let x = g.op("Add", &[&x, &x1], &[]);
    g.conv(&x, &p.take("m_tail.0.weight")?, None, 1, 1, Some("output"))?; // 3 × n
    Ok(model(g, n))
}

/// ModelProto { ir_version 7, producer, graph, opset_import }, with the
/// graph's fields in the order nodes, name, initializers, inputs, outputs.
fn model(g: Graph, n: usize) -> Vec<u8> {
    let mut graph_name = Vec::new();
    proto::bytes_field(&mut graph_name, 2, b"scunet_color_real");
    let mut io = Vec::new();
    proto::bytes_field(&mut io, 11, &proto::value_info("input", &[1, 3, n, n], FLOAT));
    proto::bytes_field(&mut io, 12, &proto::value_info("output", &[1, 3, n, n], FLOAT));
    let graph_len = g.nodes.len() + graph_name.len() + g.inits.len() + io.len();

    let mut head = Vec::with_capacity(g.nodes.len() + 64);
    proto::int_field(&mut head, 1, 7);
    proto::bytes_field(&mut head, 2, b"aktis");
    proto::len_prefix(&mut head, 7, graph_len);
    head.extend_from_slice(&g.nodes);
    head.extend_from_slice(&graph_name);

    let mut opset = Vec::new();
    proto::bytes_field(&mut opset, 1, b"");
    proto::int_field(&mut opset, 2, OPSET);
    let mut tail = io;
    proto::bytes_field(&mut tail, 8, &opset);

    // The initializers are most of the file: put the rest around them in
    // place rather than copying them.
    let mut out = g.inits;
    out.reserve_exact(head.len() + tail.len());
    out.splice(0..0, head);
    out.extend_from_slice(&tail);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    /// FNV-1a, to compare byte strings with the ones printed by Python.
    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
    }

    /// Little-endian f32 bytes of (0, 1, 2, …) · scale − 1.
    #[allow(clippy::cast_precision_loss, reason = "small test values")]
    fn ramp(shape: &[usize], scale: f32) -> Vec<u8> {
        let n: usize = shape.iter().product();
        (0..n).flat_map(|i| ((i as f32) * scale - 1.0).to_le_bytes()).collect()
    }

    #[test]
    fn scalars_match_numpy() {
        // np.float32(1e-5), np.float32(math.sqrt(2.0)), np.float32(32 ** -0.5)
        assert_eq!(LN_EPS.to_bits(), 0x3727_c5ac);
        assert_eq!(SQRT_2.to_bits(), 0x3fb5_04f3);
        assert_eq!(ATTENTION_SCALE.to_bits(), 0x3e35_04f3);
    }

    #[test]
    fn relative_bias_matches_python() {
        // scunet_graph._relative_bias(np.arange(2 * 225, dtype=np.float32).reshape(2, 15, 15))
        let params = Tensor { shape: vec![2, 15, 15], data: (0..450u16).map(f32::from).collect() };
        let b = relative_bias(&params, 2).unwrap();
        assert_eq!(b.len(), 2 * 4096);
        assert_eq!(b[0], 112.0);
        assert_eq!(b[63], 0.0);
        assert_eq!(b[64 * 63], 224.0);
        assert_eq!(b[4096 + 9 * 64 + 18], 321.0);
        assert_eq!(fnv(&b.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()), 0x5302_46ae_dd81_21f0);
        assert!(relative_bias(&params, 3).is_err());
    }

    #[test]
    fn shift_masks_match_python() {
        // scunet_graph._shift_masks(3)
        let (rows, cols) = shift_masks(3);
        assert_eq!(rows.len(), 3 * 4096);
        assert_eq!(rows.iter().filter(|&&v| v != 0.0).count(), 2048);
        assert_eq!(cols.iter().filter(|&&v| v != 0.0).count(), 2048);
        assert!(rows[..2 * 4096].iter().chain(&cols[..2 * 4096]).all(|&v| v == 0.0));
        let bytes = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>();
        assert_eq!(fnv(&bytes(&rows)), 0xed2a_16bd_38c3_e325);
        assert_eq!(fnv(&bytes(&cols)), 0x4b9e_d494_fbe3_e325);
    }

    /// A shifted Swin block on a 16×16 map with 32 channels, compared with
    /// the bytes Python's Graph produces for the same synthetic weights
    /// (names, sharing of constants, initializers and nodes).
    #[test]
    fn transformer_block_matches_python() {
        let specs: [(&str, &[usize], f32); 13] = [
            ("ln1.weight", &[32], 0.01),
            ("ln1.bias", &[32], 0.02),
            ("msa.embedding_layer.weight", &[96, 32], 0.001),
            ("msa.embedding_layer.bias", &[96], 0.003),
            ("msa.relative_position_params", &[1, 15, 15], 0.005),
            ("msa.linear.weight", &[32, 32], 0.002),
            ("msa.linear.bias", &[32], 0.004),
            ("ln2.weight", &[32], 0.03),
            ("ln2.bias", &[32], 0.05),
            ("mlp.0.weight", &[128, 32], 0.0007),
            ("mlp.0.bias", &[128], 0.006),
            ("mlp.2.weight", &[32, 128], 0.0009),
            ("mlp.2.bias", &[32], 0.008),
        ];
        let storages: Vec<Vec<u8>> = specs.iter().map(|(_, shape, scale)| ramp(shape, *scale)).collect();
        let w = specs
            .iter()
            .zip(&storages)
            .map(|((name, shape, _), raw)| (format!("b.{name}"), View::contiguous_f32(raw, shape)))
            .collect();
        let mut g = Graph::new(0);
        let p = &mut Weights(w);
        let y = transformer(&mut g, "x", p, "b", 16, 32, true).unwrap();
        assert!(p.0.is_empty());
        assert_eq!(y, "add_103");
        assert_eq!(g.nodes.len(), 3517);
        assert_eq!(g.inits.len(), 133_662);
        assert_eq!(fnv(&g.nodes), 0xbb3d_f818_ef5b_fa2d);
        assert_eq!(fnv(&g.inits), 0x3e4c_0ff8_3c33_d3b3);
    }
}
