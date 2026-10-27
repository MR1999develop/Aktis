# RAW node editor (working name)

Non-destructive RAW photo editor with a catalog and a DaVinci Resolve–style
node graph, written in Rust, GPU-accelerated on any vendor.
Windows and Ubuntu; macOS prepared.

The full design is in the development plan document; this README covers how
to build and where things live.

## Build and test

On Windows, double-click `scripts\check.bat`: it runs format check, clippy,
tests and a release build, and writes `target\check.log` and a summary in
`target\check-summary.txt`. `scripts\run.bat` starts the editor (drop a RAW
file on it to open that file).

AI selection: `scripts\get-ai.bat` downloads ONNX Runtime with DirectML and the
models once (about 2 GB, into `%LOCALAPPDATA%\rne\models`); then drop a RAW
file on `scripts\ai-check.bat` to see subject, people, person parts, subtract
and brush selections in `target\ai-check\`.

By hand:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`rust-toolchain.toml` pins the compiler (Rust 1.97); rustup installs it on first use.

## Layout

| Crate | Responsibility |
| --- | --- |
| `crates/core-model` | Domain types: images, sensor data, parameters, node graph, recipe. No I/O. |
| `crates/core-color` | Transfer curves, primaries, camera profiles (dual-illuminant matrices, DCP files). |
| `crates/tool-api` | The `Tool` contract, parameter descriptors, the tool registry. |
| `crates/commands` | Edits with validation and undo/redo: the only way UI and MCP change a recipe. |
| `crates/develop` | Fixed development stage on the CPU (white balance, demosaic, lens corrections, camera profile). |
| `crates/render-cpu` | Reference renderer: evaluates the graph with each tool's CPU code. |
| `crates/platform` | The single place where Windows, Ubuntu and macOS differ. |
| `crates/raw-io` | RAW/DNG → sensor data with rawler; levels, valid area, CFA, colour matrices, lens and shooting data. |
| `crates/gpu` | GPU engine on wgpu (Vulkan, DirectX 12, Metal); must match `render-cpu`. |
| `crates/export` | JPEG with an embedded sRGB profile, atomic writes, non-destructive names. |
| `crates/engine` | Shared by window and command line: built-in tools, GPU render with CPU fallback, preview copy, display pixels, JPEG export. |
| `crates/editor` | The editor without the window: documents and undo, background jobs (open, preview, export), viewer zoom and pan. |
| `crates/app` | The desktop window (egui): menu, viewer, inspector, status bar. |
| `crates/lens-db` | Lensfun database reader, camera/lens lookup, distortion/TCA/vignetting maths. |
| `crates/masks` | Mask nodes → mask images: AI rasters refined by colour, brush strokes, add/subtract/intersect, feather, raster cache. |
| `crates/ai-segment` | AI selection (subject, objects, people, hair/skin/clothes/face features) with ONNX Runtime loaded at run time; model catalog. |
| `crates/cli` | `rne` command: develop a RAW file to JPEG without the window. |
| `crates/tagging` | AI tagging logic: keyword list, embeddings, zero-shot scores, learning from decisions, suggestions. |
| `crates/tagging-onnx` | SigLIP 2 image/text encoders on ONNX Runtime (local, offline). |
| `tools/exposure` | First tool: exposure in stops, CPU reference + WGSL kernel. |
| `tools/denoise-ai` | AI denoise: the SCUNet network on ONNX Runtime (TensorRT for RTX, DirectML, CPU), ported from an earlier denoiser. A host tool: runs outside the WGSL kernels. |
| `scripts/scunet` | Python + NumPy scripts that make the SCUNet ONNX models from the original weights. |

Pixel data inside the editor are linear, scene-referred `f32` in Rec.2020;
quantisation happens only on output.

## Camera profiles and lens corrections

- **Profiles.** By default each camera uses its own calibration matrices
  (illuminant A and D65 for about 740 cameras, from rawler's camera data),
  mixed according to the white balance as the DNG specification describes.
  DCP profiles copied into the editor's own `profiles` folder can be chosen
  by name (`rne profiles photo.ARW` lists them); other programs' folders are
  not searched. Matrices, forward
  matrices, hue/saturation maps and look tables are applied; profile tone
  curves wait for tone mapping (phase 4).
- **Lens corrections.** `data/lensfun/` holds the lensfun database for the
  major camera and lens makers (CC BY-SA 3.0, see its README). Camera and
  lens are recognised from the EXIF data; distortion, lateral chromatic
  aberration and vignetting are corrected in camera RGB between demosaicing
  and the profile, with lensfun's own interpolation and maths. Newer lensfun
  XML files dropped in the editor's `lensfun` folder override the built-in
  data. `rne info` shows what would be corrected; `rne lenses <words>` searches
  the database.

## Adding a tool

1. Create `tools/<name>` with a type implementing `tool_api::Tool`
   (descriptor, CPU reference, GPU kernel).
2. Register it in the composition root (the app and the CLI).
3. Its parameters appear in the inspector and over MCP automatically.

## AI denoise

The `denoise_ai` node removes noise with the SCUNet neural network (trained
on real camera noise, Apache-2.0), as an earlier denoiser does. The network runs
once per input image; its two parameters, *Grain (luminance)* and *Colour
blotches*, only decide how much of its correction is applied, so moving them
is instant. Place it after the exposure: the network judges the noise at the
final brightness.

- **Models** are not in the repository: make them with `scripts/scunet`
  (see its README) or reuse an earlier denoiser's. They are looked for in
  `RNE_DENOISE_MODELS`, a `models` folder next to the program (or up to
  three folders above it), then the `RawNodeEditor/models` and
  `Denoiser/models` data folders. Without a model the node passes the
  image through and says why.
- **GPU**: on Windows TensorRT for RTX (RTX 30xx and later, with
  `tensorrt_rtx_1_4.dll` and `tensorrt_onnxparser_rtx_1_4.dll` next to `rne`;
  uses the `_fp16` models) or DirectML (any DirectX 12 card); elsewhere the
  CPU, which is slow (about 17 s per megapixel on 12 threads).
  `RNE_DENOISE_DEVICE=cpu|directml|rtx` forces a device. With several tile
  sizes installed, the fastest for the GPU is measured once per run.
- The first build downloads ONNX Runtime (Internet access needed). Without
  the `ai-denoise` feature (`--no-default-features` on `cli`) the tool and
  ONNX Runtime are left out.

## Try it

```sh
cargo run --release -p app -- path/to/photo.ARW     # the window
cargo run --release -p cli -- develop path/to/photo.ARW --exposure 0.5
cargo run --release -p cli -- develop path/to/photo.ARW --profile "Adobe Standard" --no-lens
cargo run --release -p cli -- develop path/to/photo.ARW --exposure 0.5 --denoise 80
cargo run --release -p cli -- info path/to/photo.ARW
cargo run --release -p cli -- lenses sony 24-70
cargo run --release -p cli -- profiles path/to/photo.ARW
cargo run --release -p cli -- gpu
cargo run --release -p cli -- tag photo.ARW --model path/to/siglip2 --keywords keywords.txt
cargo run --release -p cli -- models
cargo run --release -p cli -- mask path/to/photo.ARW --people
cargo run --release -p cli -- mask path/to/photo.ARW --people --subtract --parts face-skin
cargo run --release -p cli -- develop path/to/photo.ARW --exposure 1 --subject
```

AI tagging (semi-automatic keywords): see `docs/ai-tagging.md`.
AI selection needs ONNX Runtime and the models: see `docs/ai-masks.md`.

Use `--release`: the CPU development stage is slow in debug builds.

RAW samples for the ignored real-file test go in `testdata/` (not committed):
`cargo test -p raw-io --release -- --ignored --nocapture`.

## Status

Phase 0 (foundations) is done; phase 1 (tracer bullet) is complete with the
first window. See `docs/progress.md`.
