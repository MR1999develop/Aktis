# SCUNet models for the AI denoise tool

The `denoise_ai` tool (`tools/denoise-ai`) runs the SCUNet network from
ONNX files that are generated, not stored in the repository. These scripts
come from an earlier denoiser and need only Python 3 and NumPy (no PyTorch, no
`onnx` package): `pth_weights.py` reads the checkpoint safely (tensors only),
`scunet_graph.py` builds the network and `onnxproto.py` writes the file.

1. Download the original weights (72 MB, Apache-2.0, SHA-256 `fa78899b…809c2e`):
   <https://github.com/cszn/KAIR/releases/download/v1.0/scunet_color_real_psnr.pth>
2. Make the normal model (required, 448 px tiles, about 77 MB):

   ```sh
   python scripts/scunet/export_onnx_model.py --weights scunet_color_real_psnr.pth \
       --output models/scunet_color_real_448.onnx --verify
   ```

3. Optional, faster on a GPU: larger tiles (`--tile 704`, `960`, `1216`,
   `1600`, output `models/scunet_color_real_<tile>.onnx`) and, for TensorRT
   for RTX, their half-precision variants:

   ```sh
   python scripts/scunet/make_fp16_model.py models/scunet_color_real_*.onnx
   ```

Where the editor looks for them, in order: the folder (or file) in
`RNE_DENOISE_MODELS`; a `models` folder next to the program or up to three
folders above it; `RawNodeEditor/models` and then `Denoiser/models` in the
user's data folder (`%APPDATA%` on Windows, `~/.local/share` on Linux). The
models already installed for an earlier denoiser are therefore used as they are.

`--verify` needs `onnxruntime` for Python and checks that the new model
reduces the noise of a test tile.
