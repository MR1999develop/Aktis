"""Makes the half-precision (FP16) variant of a an earlier denoiser ONNX model.

TensorRT for RTX (and TensorRT 11) only run a network in FP16 if the model
itself is FP16 ("strongly typed"): unlike TensorRT 10 they no longer convert
it on their own. This script rewrites `scunet_color_real_<N>.onnx` into
`scunet_color_real_<N>_fp16.onnx`:

* the weights are stored in FP16 (half the size) and the convolutions and
  matrix products run in FP16, on the Tensor Cores of RTX cards;
* the steps where FP16 can overflow or lose too much precision stay in FP32:
  the means and variances of the layer normalisations and the attention
  softmax (`--keep-fp32`), plus the element-wise steps that directly follow
  them;
* input and output stay FP32, so the program uses both files the same way.

Only Python and NumPy are needed (and `onnxproto.py` from this folder):

    python tools/make_fp16_model.py models/scunet_color_real_448.onnx
    python tools/make_fp16_model.py models/scunet_color_real_*.onnx
"""
from __future__ import annotations

import argparse
import glob
import sys
from pathlib import Path
from typing import Final

import numpy as np

import onnxproto as pb
from onnxproto import get, parse, serialize

FLOAT: Final = pb.FLOAT
FLOAT16: Final = pb.FLOAT16
# operators that always run in FP16: the heavy ones, where the speed is
HEAVY: Final = frozenset({"Conv", "ConvTranspose", "MatMul"})
# operators that only move data: they keep the type of their input
LAYOUT: Final = frozenset({"Reshape", "Transpose", "Slice", "Concat", "Relu"})
DEFAULT_KEEP_FP32: Final = "ReduceMean,Softmax,Pow,Sqrt"
# largest finite FP16 value
FP16_MAX: Final = 65504.0

# ------------------------------------------------------------ ONNX pieces

def cast_node(src: str, dst: str, to: int) -> bytes:
    return pb.node("Cast", [src], dst, [pb.attr_int("to", to)])


def tensor_info(t: bytes) -> tuple[str, int, list[int]]:
    f = parse(t)
    name = get(f, 8)[0].decode()
    dtype = get(f, 2)[0] if get(f, 2) else FLOAT
    return name, dtype, get(f, 1)


def tensor_to_fp16(t: bytes, new_name: str | None = None) -> bytes:
    f = parse(t)
    raw = get(f, 9)
    if raw:
        arr = np.frombuffer(raw[0], dtype="<f4")
    else:  # float_data (packed or not)
        vals = []
        for no, wire, v in f:
            if no == 4:
                vals.extend(np.frombuffer(v, "<f4") if wire == 2 else [np.frombuffer(v, "<f4")[0]])
        arr = np.asarray(vals, dtype=np.float32)
    big = np.abs(arr) > FP16_MAX
    # the attention masks use -1e9 for "never": -65504 works the same after
    # the softmax; anything else out of range is worth a warning
    if big.any() and not (arr[big] <= -1e8).all():
        print(f"  warning: {int(big.sum())} weights beyond the FP16 range were clipped", file=sys.stderr)
    half = np.clip(arr, -FP16_MAX, FP16_MAX).astype("<f2")
    out = []
    for no, wire, v in f:
        if no in (2, 4, 9):
            continue
        if no == 8 and new_name:
            v = new_name.encode()
        out.append((no, wire, v))
    out += [(2, 0, FLOAT16), (9, 2, half.tobytes())]
    return serialize(out)


# ------------------------------------------------------------ conversion

def convert(model: bytes, keep_fp32: set[str]) -> tuple[bytes, dict[str, int]]:
    """FP16 version of the ONNX `model`; also returns how many steps run in
    each precision and how many conversions were added."""
    m = parse(model)
    graph_i = next(i for i, (no, _, _) in enumerate(m) if no == 7)
    g = parse(m[graph_i][2])

    inits = {}
    for t in get(g, 5):
        name, dtype, _ = tensor_info(t)
        inits[name] = (dtype, t)
    nodes = [parse(n) for n in get(g, 1)]
    graph_inputs = [get(parse(v), 1)[0].decode() for v in get(g, 11)]
    graph_outputs = [get(parse(v), 1)[0].decode() for v in get(g, 12)]

    # 1. precision of each node, in graph order
    dtype = {n: FLOAT for n in graph_inputs}
    plan = []
    for nd in nodes:
        op = get(nd, 4)[0].decode()
        ins = [i.decode() for i in get(nd, 1)]
        acts = [i for i in ins if i not in inits]          # activations
        act_types = {dtype[i] for i in acts if dtype.get(i) in (FLOAT, FLOAT16)}
        if op in keep_fp32:
            p = FLOAT
        elif op in HEAVY:
            p = FLOAT16
        elif op in LAYOUT:
            p = FLOAT16 if FLOAT16 in act_types else (FLOAT if act_types else FLOAT16)
        else:  # element-wise: FP32 if it follows an FP32 step (normalisation chain)
            p = FLOAT if FLOAT in act_types else FLOAT16
        for o in get(nd, 2):
            dtype[o.decode()] = p
        plan.append((nd, op, ins, p))

    # 2. float initializers: FP16 for FP16 consumers, FP32 copy if also needed
    need = {}
    for _, op, ins, p in plan:
        for k, i in enumerate(ins):
            if i in inits and inits[i][0] == FLOAT:
                if op == "Reshape" or (op == "Slice" and k > 0):
                    continue
                need.setdefault(i, set()).add(p)
    new_inits = []
    rename32 = {}
    for name, (dt, t) in inits.items():
        kinds = need.get(name, {FLOAT}) if dt == FLOAT else {dt}
        if dt == FLOAT and FLOAT16 in kinds:
            new_inits.append(tensor_to_fp16(t))
            if FLOAT in kinds:
                rename32[name] = name + "_f32"
                f = [(no, w, (rename32[name].encode() if no == 8 else v)) for no, w, v in parse(t)]
                new_inits.append(serialize(f))
        else:
            new_inits.append(t)
    init_type = {}
    for name, (dt, _) in inits.items():
        kinds = need.get(name, set())
        init_type[name] = FLOAT16 if (dt == FLOAT and FLOAT16 in kinds) else dt

    # 3. nodes, with a Cast wherever an activation has the wrong precision
    out_nodes = []
    casts = {}
    stats = {"fp16": 0, "fp32": 0, "casts": 0}
    for nd, op, ins, p in plan:
        new_ins = []
        for k, i in enumerate(ins):
            if i in inits:
                if init_type[i] in (FLOAT, FLOAT16) and init_type[i] != p and not (
                        op == "Reshape" or (op == "Slice" and k > 0)):
                    i = rename32.get(i, i) if p == FLOAT else i
                new_ins.append(i)
                continue
            t = dtype.get(i)
            if t in (FLOAT, FLOAT16) and t != p:
                key = (i, p)
                if key not in casts:
                    casts[key] = f"{i}_to{'16' if p == FLOAT16 else '32'}"
                    out_nodes.append(cast_node(i, casts[key], p))
                    stats["casts"] += 1
                i = casts[key]
            new_ins.append(i)
        stats["fp16" if p == FLOAT16 else "fp32"] += 1
        f = [(no, w, v) for no, w, v in nd if no != 1]
        f = [(1, 2, i.encode()) for i in new_ins] + f
        out_nodes.append(serialize(f))

    # 4. FP32 output under the original name
    for o in graph_outputs:
        if dtype.get(o) == FLOAT16:
            tmp = o + "_f16"
            out_nodes = [_rename_output(n, o, tmp) for n in out_nodes]
            out_nodes.append(cast_node(tmp, o, FLOAT))
            stats["casts"] += 1

    # 5. reassemble the graph in its original field order
    new_g = []
    emitted_nodes = emitted_inits = False
    for no, w, v in g:
        if no == 1:
            if not emitted_nodes:
                new_g += [(1, 2, n) for n in out_nodes]
                emitted_nodes = True
        elif no == 5:
            if not emitted_inits:
                new_g += [(5, 2, t) for t in new_inits]
                emitted_inits = True
        else:
            new_g.append((no, w, v))
    m[graph_i] = (7, 2, serialize(new_g))
    m = [(no, w, (b"aktis fp16" if no == 2 else v)) for no, w, v in m]
    return serialize(m), stats


def _rename_output(node: bytes, old: str, new: str) -> bytes:
    f = parse(node)
    if old.encode() not in get(f, 2):
        return node
    return serialize([(no, w, (new.encode() if no == 2 and v == old.encode() else v)) for no, w, v in f])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("models", nargs="+", help="FP32 models (wildcards allowed)")
    ap.add_argument("--keep-fp32", default=DEFAULT_KEEP_FP32,
                    help=f"operators kept in FP32 (default {DEFAULT_KEEP_FP32}; empty = all FP16)")
    a = ap.parse_args()
    keep = {s for s in a.keep_fp32.split(",") if s}
    paths = [Path(p) for pat in a.models for p in (glob.glob(pat) or [pat])]
    paths = [p for p in paths if not p.stem.endswith("_fp16")]
    if not paths:
        print("No model to convert.", file=sys.stderr)
        return 2
    for src in paths:
        dst = src.with_name(src.stem + "_fp16.onnx")
        data, stats = convert(src.read_bytes(), keep)
        dst.write_bytes(data)
        print(f"Wrote {dst} ({len(data) / 1e6:.1f} MB): {stats['fp16']} steps in FP16, "
              f"{stats['fp32']} in FP32, {stats['casts']} conversions")
    return 0


if __name__ == "__main__":
    sys.exit(main())
