"""Converts the KAIR SCUNet weights (.pth) into the ONNX model used by an earlier denoiser.

Only Python and NumPy are needed (no PyTorch, no onnx package): the
checkpoint is read by `pth_weights.py`, the network is built by
`scunet_graph.py` and written by `onnxproto.py`, all in this folder.

    python tools/export_onnx_model.py --weights scunet_color_real_psnr.pth \
        --output models/scunet_color_real_448.onnx

The weights come from https://github.com/cszn/KAIR/releases/tag/v1.0
(SHA-256 fa78899b…809c2e). The graph has a fixed 1×3×N×N input (default
448): an earlier denoiser processes tiles of this size with a 64 px overlap. Larger
tiles (--tile, a multiple of 64) waste less work on the overlap and can be
faster on a GPU; name the file scunet_color_real_<tile>.onnx.

--verify runs the new model with onnxruntime, if it is installed, and checks
that it reduces the noise of a synthetic test tile.
"""
from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path
from typing import Final

import numpy as np

import pth_weights
import scunet_graph

DEFAULT_TILE_PX: Final = 448
# below this the deepest stage has a single window, where KAIR's model
# behaves differently (no shifted windows)
MIN_TILE_PX: Final = 192
EXPECTED_SHA256: Final = "fa78899ba2caec9d235a900e91d96c689da71c42029230c2028b00f09f809c2e"


def verify(model: Path, tile: int) -> bool:
    """True if the model lowers the RMSE of a noisy gradient; skipped (True)
    without onnxruntime."""
    try:
        import onnxruntime as ort  # noqa: PLC0415
    except ImportError:
        print("onnxruntime is not installed: verification skipped.")
        return True
    rng = np.random.default_rng(1)
    clean = np.tile(np.linspace(0.2, 0.8, tile, dtype=np.float32), (3, tile, 1))
    noisy = np.clip(clean + rng.normal(0, 0.08, clean.shape).astype(np.float32), 0, 1)
    session = ort.InferenceSession(str(model), providers=["CPUExecutionProvider"])
    out = session.run(None, {"input": noisy[None]})[0][0]
    err_in = float(np.sqrt(np.mean((noisy - clean) ** 2)))
    err_out = float(np.sqrt(np.mean((out - clean) ** 2)))
    print(f"Noise RMSE {err_in:.4f} -> {err_out:.4f}")
    return err_out < err_in


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--weights", required=True, type=Path, help="scunet_color_real_psnr.pth")
    ap.add_argument("--output", required=True, type=Path)
    ap.add_argument("--tile", type=int, default=DEFAULT_TILE_PX,
                    help=f"side of the input tile in pixels, a multiple of 64, at least {MIN_TILE_PX} "
                         f"(default {DEFAULT_TILE_PX})")
    ap.add_argument("--verify", action="store_true", help="run the model on a test tile (needs onnxruntime)")
    a = ap.parse_args()

    if a.tile % 64 or a.tile < MIN_TILE_PX:
        print(f"--tile must be a multiple of 64, at least {MIN_TILE_PX}.", file=sys.stderr)
        return 2
    try:
        data = a.weights.read_bytes()
    except OSError as e:
        print(f"Cannot read {a.weights}: {e.strerror}", file=sys.stderr)
        return 2
    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED_SHA256:
        print(f"Warning: SHA-256 differs from the expected one ({digest}).", file=sys.stderr)
    try:
        weights = pth_weights.load_state_dict(a.weights)
        model = scunet_graph.build(weights, a.tile)
    except pth_weights.WeightsError as e:
        print(f"{a.weights}: {e}", file=sys.stderr)
        return 1
    except KeyError as e:
        print(f"{a.weights} is not a SCUNet checkpoint: weight {e} is missing.", file=sys.stderr)
        return 1
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_bytes(model)
    print(f"Wrote {a.output} ({len(model) / 1e6:.1f} MB)")
    if a.verify and not verify(a.output, a.tile):
        print("Verification failed: the model does not reduce noise.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
