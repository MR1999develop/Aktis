"""The SCUNet network ("Practical Blind Image Denoising via Swin-Conv-UNet and
Data Synthesis", K. Zhang et al. 2022) as an ONNX graph with a fixed square
input, built from the original KAIR weights.

The graph reproduces SCUNet(in_nc=3, config=[4]*7, dim=64): a U-Net with
three down-sampling stages, a body and three up-sampling stages, each made of
Swin-Conv blocks (a convolution branch and a window-attention branch side by
side). Only standard ONNX operators (opset 13) are used, so ONNX Runtime and
its GPU back ends (DirectML, TensorRT for RTX) can run it; layer
normalisation and GELU are written out in the form ONNX Runtime fuses.

The input side must be a multiple of 64 (three halvings and 8×8 windows);
the input is 1×3×N×N in 0..1, the output has the same shape.
"""
from __future__ import annotations

import math
from typing import Final

import numpy as np

import onnxproto as pb

WINDOW: Final = 8
HEAD_DIM: Final = 32
DIM: Final = 64
BLOCKS_PER_STAGE: Final = 4
LN_EPS: Final = 1e-5
# added to the attention logits of masked pairs: large enough that the
# softmax gives them no weight, small enough to stay finite in FP16
MASK_VALUE: Final = -30000.0
OPSET: Final = 13


class Graph:
    """ONNX graph under construction: operators return the name of their
    output tensor; weights are stored as initializers."""

    def __init__(self) -> None:
        self.nodes: list[bytes] = []
        self.inits: list[bytes] = []
        self._consts: dict[tuple, str] = {}
        self._masks: dict[int, tuple[str, str]] = {}
        self._n = 0

    def _name(self, prefix: str) -> str:
        self._n += 1
        return f"{prefix}_{self._n}"

    def const(self, arr: np.ndarray, data_type: int = pb.FLOAT) -> str:
        """An initializer; small identical constants (shapes, scalars) are shared."""
        arr = np.ascontiguousarray(arr)
        key = None
        if arr.size <= 16:
            key = (data_type, arr.shape, arr.astype(pb.NP_TYPE[data_type]).tobytes())
            if key in self._consts:
                return self._consts[key]
        name = self._name("c" if key else "w")
        self.inits.append(pb.tensor(name, arr, data_type))
        if key:
            self._consts[key] = name
        return name

    def shift_masks(self, n_win: int) -> tuple[str, str]:
        """The shifted-window masks for a stage, created once per stage size."""
        if n_win not in self._masks:
            rows, cols = _shift_masks(n_win)
            self._masks[n_win] = (self.const(rows), self.const(cols))
        return self._masks[n_win]

    def ints(self, values: list[int]) -> str:
        return self.const(np.array(values, dtype=np.int64), pb.INT64)

    def op(self, op_type: str, inputs: list[str], attrs: list[bytes] = (), *, out: str | None = None) -> str:
        # a named output (the graph's) gets a node name of its own
        name = self._name(op_type.lower()) if out else None
        out = out or self._name(op_type.lower())
        self.nodes.append(pb.node(op_type, inputs, out, list(attrs), name=name))
        return out

    # --- layers -----------------------------------------------------------

    def conv(self, x: str, w: np.ndarray, b: np.ndarray | None, *, stride: int = 1, pad: int = 0,
             out: str | None = None) -> str:
        k = int(w.shape[2])
        inputs = [x, self.const(w)] + ([self.const(b)] if b is not None else [])
        return self.op("Conv", inputs, [pb.attr_ints("kernel_shape", [k, k]), pb.attr_ints("strides", [stride, stride]),
                                        pb.attr_ints("pads", [pad] * 4)], out=out)

    def conv_transpose(self, x: str, w: np.ndarray) -> str:
        k = int(w.shape[2])
        return self.op("ConvTranspose", [x, self.const(w)],
                       [pb.attr_ints("kernel_shape", [k, k]), pb.attr_ints("strides", [k, k])])

    def linear(self, x: str, w: np.ndarray, b: np.ndarray) -> str:
        """y = x · wᵀ + b over the last axis (PyTorch nn.Linear)."""
        return self.op("Add", [self.op("MatMul", [x, self.const(np.ascontiguousarray(w.T))]), self.const(b)])

    def layer_norm(self, x: str, gamma: np.ndarray, beta: np.ndarray) -> str:
        mean = self.op("ReduceMean", [x], [pb.attr_ints("axes", [-1]), pb.attr_int("keepdims", 1)])
        d = self.op("Sub", [x, mean])
        var = self.op("ReduceMean", [self.op("Pow", [d, self.const(np.float32(2.0))])],
                      [pb.attr_ints("axes", [-1]), pb.attr_int("keepdims", 1)])
        y = self.op("Div", [d, self.op("Sqrt", [self.op("Add", [var, self.const(np.float32(LN_EPS))])])])
        return self.op("Add", [self.op("Mul", [y, self.const(gamma)]), self.const(beta)])

    def gelu(self, x: str) -> str:
        """Exact GELU: x · ½ · (1 + erf(x / √2))."""
        e = self.op("Erf", [self.op("Div", [x, self.const(np.float32(math.sqrt(2.0)))])])
        y = self.op("Mul", [x, self.op("Add", [e, self.const(np.float32(1.0))])])
        return self.op("Mul", [y, self.const(np.float32(0.5))])

    def slice(self, x: str, axis: int, start: int, end: int) -> str:
        return self.op("Slice", [x, self.ints([start]), self.ints([end]), self.ints([axis])])

    def concat(self, xs: list[str], axis: int) -> str:
        return self.op("Concat", xs, [pb.attr_int("axis", axis)])

    def reshape(self, x: str, shape: list[int]) -> str:
        return self.op("Reshape", [x, self.ints(shape)])

    def transpose(self, x: str, perm: list[int]) -> str:
        return self.op("Transpose", [x], [pb.attr_ints("perm", perm)])

    def roll(self, x: str, shift: int, axis: int, size: int) -> str:
        """torch.roll(x, shift, axis) for an axis of length `size`."""
        s = shift % size
        if s == 0:
            return x
        return self.concat([self.slice(x, axis, size - s, size), self.slice(x, axis, 0, size - s)], axis)


# --- SCUNet blocks ---------------------------------------------------------

def _relative_bias(params: np.ndarray) -> np.ndarray:
    """(heads, 2w−1, 2w−1) learned table → (heads, w², w²) bias between the
    positions of a window."""
    w = WINDOW
    cord = np.array([[i, j] for i in range(w) for j in range(w)])
    rel = cord[:, None, :] - cord[None, :, :] + w - 1
    return params[:, rel[:, :, 0], rel[:, :, 1]]


def _shift_masks(n_win: int) -> tuple[np.ndarray, np.ndarray]:
    """Additive masks of the shifted windows: after the roll, the last row and
    the last column of windows hold pixels from opposite sides of the image,
    which must not attend to each other. Returned as a mask per window row
    (n_win, 1, w², w²) and one per window column (n_win, w², w²), to be added
    to the logits viewed as (…, rows, columns, w², w²): a pair is masked if
    either says so, and the sum stays finite in FP16."""
    w, s = WINDOW, WINDOW - WINDOW // 2
    y, x = np.divmod(np.arange(w * w), w)
    # pairs split by the seam: one end before the shift, the other after it
    split_y = (y[:, None] < s) != (y[None, :] < s)
    split_x = (x[:, None] < s) != (x[None, :] < s)
    rows = np.zeros((n_win, 1, w * w, w * w), dtype=np.float32)
    cols = np.zeros((n_win, w * w, w * w), dtype=np.float32)
    rows[-1, 0][split_y] = MASK_VALUE
    cols[-1][split_x] = MASK_VALUE
    return rows, cols


def _window_attention(g: Graph, x: str, p: dict, prefix: str, size: int, ch: int, shifted: bool) -> str:
    """Multi-head self-attention inside 8×8 windows of x (1, size, size, ch)."""
    w = WINDOW
    heads = ch // HEAD_DIM
    n_win = size // w
    if shifted:
        x = g.roll(g.roll(x, -(w // 2), 1, size), -(w // 2), 2, size)
    # (1, H, W, C) → (1, windows, w², C)
    t = g.reshape(x, [1, n_win, w, n_win, w, ch])
    t = g.reshape(g.transpose(t, [0, 1, 3, 2, 4, 5]), [1, n_win * n_win, w * w, ch])
    qkv = g.linear(t, p[f"{prefix}.embedding_layer.weight"], p[f"{prefix}.embedding_layer.bias"])
    # (1, nW, P, 3·heads·hd) → (3·heads, 1, nW, P, hd): q, k, v heads in order
    qkv = g.transpose(g.reshape(qkv, [1, n_win * n_win, w * w, 3 * heads, HEAD_DIM]), [3, 0, 1, 2, 4])
    q = g.slice(qkv, 0, 0, heads)
    k = g.slice(qkv, 0, heads, 2 * heads)
    v = g.slice(qkv, 0, 2 * heads, 3 * heads)
    sim = g.op("MatMul", [q, g.transpose(k, [0, 1, 2, 4, 3])])
    sim = g.op("Mul", [sim, g.const(np.float32(HEAD_DIM ** -0.5))])
    bias = _relative_bias(p[f"{prefix}.relative_position_params"]).reshape(heads, 1, 1, w * w, w * w)
    sim = g.op("Add", [sim, g.const(bias.astype(np.float32))])
    if shifted:
        rows, cols = g.shift_masks(n_win)
        sim = g.reshape(sim, [heads, 1, n_win, n_win, w * w, w * w])
        sim = g.op("Add", [g.op("Add", [sim, rows]), cols])
        sim = g.reshape(sim, [heads, 1, n_win * n_win, w * w, w * w])
    probs = g.op("Softmax", [sim], [pb.attr_int("axis", -1)])
    out = g.op("MatMul", [probs, v])  # (heads, 1, nW, P, hd)
    out = g.reshape(g.transpose(out, [1, 2, 3, 0, 4]), [1, n_win * n_win, w * w, ch])
    out = g.linear(out, p[f"{prefix}.linear.weight"], p[f"{prefix}.linear.bias"])
    # back to (1, H, W, C)
    out = g.reshape(out, [1, n_win, n_win, w, w, ch])
    out = g.reshape(g.transpose(out, [0, 1, 3, 2, 4, 5]), [1, size, size, ch])
    if shifted:
        out = g.roll(g.roll(out, w // 2, 1, size), w // 2, 2, size)
    return out


def _transformer(g: Graph, x: str, p: dict, prefix: str, size: int, ch: int, shifted: bool) -> str:
    """Swin block on x (1, H, W, C): attention and MLP, each with a residual."""
    a = g.layer_norm(x, p[f"{prefix}.ln1.weight"], p[f"{prefix}.ln1.bias"])
    x = g.op("Add", [x, _window_attention(g, a, p, f"{prefix}.msa", size, ch, shifted)])
    m = g.layer_norm(x, p[f"{prefix}.ln2.weight"], p[f"{prefix}.ln2.bias"])
    m = g.linear(m, p[f"{prefix}.mlp.0.weight"], p[f"{prefix}.mlp.0.bias"])
    m = g.linear(g.gelu(m), p[f"{prefix}.mlp.2.weight"], p[f"{prefix}.mlp.2.bias"])
    return g.op("Add", [x, m])


def _swin_conv_block(g: Graph, x: str, p: dict, prefix: str, size: int, half: int, shifted: bool) -> str:
    """Swin-Conv block on x (1, 2·half, H, W): a 1×1 convolution splits the
    channels into a residual convolution branch and a transformer branch, a
    second 1×1 convolution mixes them back, plus a residual."""
    y = g.conv(x, p[f"{prefix}.conv1_1.weight"], p[f"{prefix}.conv1_1.bias"])
    conv_x = g.slice(y, 1, 0, half)
    trans_x = g.slice(y, 1, half, 2 * half)
    c = g.conv(conv_x, p[f"{prefix}.conv_block.0.weight"], None, pad=1)
    c = g.conv(g.op("Relu", [c]), p[f"{prefix}.conv_block.2.weight"], None, pad=1)
    conv_x = g.op("Add", [c, conv_x])
    t = g.transpose(trans_x, [0, 2, 3, 1])
    t = _transformer(g, t, p, f"{prefix}.trans_block", size, half, shifted)
    trans_x = g.transpose(t, [0, 3, 1, 2])
    res = g.conv(g.concat([conv_x, trans_x], 1), p[f"{prefix}.conv1_2.weight"], p[f"{prefix}.conv1_2.bias"])
    return g.op("Add", [x, res])


def _stage(g: Graph, x: str, p: dict, prefix: str, first: int, size: int, half: int) -> str:
    """BLOCKS_PER_STAGE Swin-Conv blocks, alternating plain and shifted windows.
    In KAIR's SCUNet a window covering the whole stage is never shifted; with
    the training resolution of 256 px that never happens."""
    for i in range(BLOCKS_PER_STAGE):
        x = _swin_conv_block(g, x, p, f"{prefix}.{first + i}", size, half, shifted=i % 2 == 1)
    return x


def build(p: dict[str, np.ndarray], tile_px: int) -> bytes:
    """ONNX model of SCUNet for 1×3×tile_px×tile_px inputs, with the weights `p`
    (the KAIR state dict). Raises ValueError for an unusable tile size and
    KeyError if a weight is missing."""
    if tile_px % 64 or tile_px < 64:
        raise ValueError("the tile side must be a positive multiple of 64")
    g = Graph()
    n = tile_px
    x1 = g.conv("input", p["m_head.0.weight"], None, pad=1)                         # 64 × n
    x = _stage(g, x1, p, "m_down1", 0, n, DIM // 2)
    x2 = g.conv(x, p["m_down1.4.weight"], None, stride=2)                            # 128 × n/2
    x = _stage(g, x2, p, "m_down2", 0, n // 2, DIM)
    x3 = g.conv(x, p["m_down2.4.weight"], None, stride=2)                            # 256 × n/4
    x = _stage(g, x3, p, "m_down3", 0, n // 4, 2 * DIM)
    x4 = g.conv(x, p["m_down3.4.weight"], None, stride=2)                            # 512 × n/8
    x = _stage(g, x4, p, "m_body", 0, n // 8, 4 * DIM)
    x = g.conv_transpose(g.op("Add", [x, x4]), p["m_up3.0.weight"])                  # 256 × n/4
    x = _stage(g, x, p, "m_up3", 1, n // 4, 2 * DIM)
    x = g.conv_transpose(g.op("Add", [x, x3]), p["m_up2.0.weight"])                  # 128 × n/2
    x = _stage(g, x, p, "m_up2", 1, n // 2, DIM)
    x = g.conv_transpose(g.op("Add", [x, x2]), p["m_up1.0.weight"])                  # 64 × n
    x = _stage(g, x, p, "m_up1", 1, n, DIM // 2)
    g.conv(g.op("Add", [x, x1]), p["m_tail.0.weight"], None, pad=1, out="output")   # 3 × n
    return pb.model(g.nodes, g.inits, [pb.value_info("input", [1, 3, n, n])], [pb.value_info("output", [1, 3, n, n])],
                    graph_name="scunet_color_real", producer="aktis", opset=OPSET)
