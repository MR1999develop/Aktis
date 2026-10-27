#!/usr/bin/env python3
"""Prints the learned logit scale (exponentiated) and bias of a SigLIP
checkpoint on Hugging Face, reading only the safetensors header and the two
scalars with HTTP range requests (a few KB instead of the whole model).

Usage: siglip_head.py google/siglip2-base-patch16-224
"""
import json
import math
import struct
import sys
import urllib.request


def fetch(url: str, start: int, end: int) -> bytes:
    req = urllib.request.Request(url, headers={"Range": f"bytes={start}-{end}"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def main() -> None:
    repo = sys.argv[1]
    url = f"https://huggingface.co/{repo}/resolve/main/model.safetensors"
    header_len = struct.unpack("<Q", fetch(url, 0, 7))[0]
    header = json.loads(fetch(url, 8, 8 + header_len - 1))
    values = {}
    for name in ("logit_scale", "logit_bias"):
        info = header[name]
        if info["dtype"] != "F32":
            raise SystemExit(f"{name}: unexpected dtype {info['dtype']}")
        a, b = info["data_offsets"]
        raw = fetch(url, 8 + header_len + a, 8 + header_len + b - 1)
        values[name] = struct.unpack("<f", raw[:4])[0]
    print(f"{repo}: logit_scale={math.exp(values['logit_scale']):.4f} "
          f"(log {values['logit_scale']:.6f}) logit_bias={values['logit_bias']:.4f}")


if __name__ == "__main__":
    main()
