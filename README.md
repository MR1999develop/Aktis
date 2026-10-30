# RAW node editor (working name)

Non-destructive RAW photo editor with a catalog and a DaVinci Resolve–style
node graph, written in Rust, GPU-accelerated on any vendor.
Windows and Ubuntu; macOS prepared.

The full design is in the development plan document; this README covers how
to build and where things live.

## Build and test

On a new Windows PC, double-click `scripts\setup-windows.bat` first: it
installs Git, the Visual Studio 2022 C++ build tools, rustup and the pinned
toolchain with winget (log in `target\setup.log`).

On Windows, double-click `scripts\check.bat`: it runs format check, clippy,
tests and a release build, showing everything on screen, and writes
`target\check.log` (full output) and `target\check-summary.txt` (environment:
git commit, Rust, GPU, ONNX Runtime and models; then status, time and the
errors, warnings and failed tests of each step). `check.bat -Quick` skips the
release build, `check.bat -Step test` runs one step. `scripts\run.bat` starts the editor (drop a RAW
file on it to open that file).

AI (denoise, selection, tagging): the program ships without ONNX Runtime and
the models. The first time a tool needs them, the editor shows what is needed
(size, licence) and downloads it on request; *Tools → AI models* lists every
pack. On the command line the AI commands ask the same way, and
`rne models --download <tool|all>` installs ahead of time;
`scripts\get-ai.bat` does that for everything. All of it goes into
`%LOCALAPPDATA%\raw-node-editor\models` (crate `ai-models` keeps the list,
with sizes and SHA-256; `onnx-env` loads the one runtime the three AI modules
share). Then drop a RAW file on `scripts\ai-check.bat` to see subject, people,
person parts, subtract and brush selections in `target\ai-check\`.

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
| `crates/develop` | Fixed development stage on the CPU (white balance, Bayer demosaic with RCD, X-Trans demosaic with Markesteijn, false-colour suppression, lens corrections, camera profile). |
| `crates/render-cpu` | Reference renderer: evaluates the graph with each tool's CPU code. |
| `crates/platform` | The single place where Windows, Ubuntu and macOS differ. |
| `crates/raw-io` | RAW/DNG → sensor data with rawler; levels, valid area, CFA, DNG gain maps, colour matrices, lens and shooting data. |
| `crates/gpu` | GPU engine on wgpu (Vulkan, DirectX 12, Metal); must match `render-cpu`. Software renderers (a virtual machine's) are refused, so the CPU engine runs instead; `RNE_SOFTWARE_GPU=1` accepts them. |
| `crates/export` | JPEG with an embedded sRGB profile, atomic writes, non-destructive names. |
| `crates/engine` | Shared by window and command line: built-in tools, GPU render with CPU fallback, preview copy, display pixels, JPEG export. |
| `crates/editor` | The editor without the window: documents and undo, background jobs (open, preview, export, folder listings, thumbnails), viewer zoom and pan, folder tree and filmstrip model, graph layout, histogram. |
| `crates/catalog` | Ratings, colour labels and keywords of each photo (SQLite) and the Explorer's filters. |
| `crates/app` | The desktop window (egui): tool bar, Explorer, filmstrip, History and Metadata tabs, viewer, node graph, histogram, node settings, status bar. |
| `crates/lens-db` | Lensfun database reader, camera/lens lookup, distortion/TCA/vignetting maths. |
| `crates/masks` | Mask nodes → mask images: AI rasters refined by colour, brush strokes, add/subtract/intersect, feather, raster cache. |
| `crates/ai-segment` | AI selection (subject, objects, people, hair/skin/clothes/face features) with ONNX Runtime loaded at run time; model catalog. |
| `crates/cli` | `rne` command: develop a RAW file to JPEG without the window. |
| `crates/tagging` | AI tagging logic: keyword list, embeddings, zero-shot scores, learning from decisions, suggestions. |
| `crates/tagging-onnx` | SigLIP 2 image/text encoders on ONNX Runtime (local, offline). |
| `tools/exposure` | First tool: exposure in stops, CPU reference + WGSL kernel. |
| `tools/denoise-ai` | AI denoise: the SCUNet network on ONNX Runtime (TensorRT for RTX, DirectML, CPU), ported from an earlier denoiser. A host tool: runs outside the WGSL kernels. |
| `scripts/scunet` | Python + NumPy reference for `scunet-onnx`, which makes the SCUNet ONNX models from the original weights. |

Pixel data inside the editor are linear, scene-referred `f32` in Rec.2020;
quantisation happens only on output.

## Supported cameras

JPEG and TIFF pictures are imported and edited too (read as sRGB; see
`docs/catalog.md`). RAW files are decoded by [rawler](https://github.com/dnglab/dnglab) 0.8.0
(pinned); the editor develops Bayer and Fujifilm X-Trans mosaics and linear
(already demosaiced) DNGs. rawler recognises cameras through its own
database: a body newer than rawler 0.8.0, or a raw mode not listed for it,
is refused even when the format is known. DNG files carry their own
calibration and need no database entry. Monochrome sensors (Leica
Monochrom, Pentax K-3 III Monochrome) are not supported yet.

| Brand | Files | Known limits |
|---|---|---|
| Canon | CR3 (C-RAW and dual-pixel included), CR2, CRW | CR2 sRAW/mRAW are developed as linear RGB; CR3 burst rolls open their first frame; CRM movies are not offered. |
| Nikon | NEF, NRW | High Efficiency and High Efficiency★ NEF (Z 8, Z 9, Z 6III, Zf …) are refused by rawler 0.8.0; lossless, lossy and uncompressed NEF work. |
| Sony | ARW, SR2, SRF | Uncompressed, compressed and lossless compressed ARW work; the newest compression (TIFF code 32766, "ARW6") has no decoder in rawler 0.8.0. |
| Fujifilm | RAF | X-Trans and Bayer (GFX) sensors; uncompressed, lossless and lossy compressed RAF. X-Trans sensors use Markesteijn's three-pass demosaic (as RawTherapee and darktable); Bayer sensors, GFX included, use RCD. Base characteristics ▸ Demosaic offers the fast methods too. |
| Apple | DNG (ProRAW) | ProRAW is a linear DNG: no demosaic, white balance and camera matrix only. JPEG XL compressed DNGs decode only with 8- or 16-bit streams. |
| Google Pixel | DNG | Lens shading stored as DNG GainMap opcodes is applied when the file is opened; other opcodes (for example the WarpRectilinear distortion correction in OpcodeList3) are not, and a note says so when the file marks them as required. |
| Panasonic | RW2, RAW | |
| OM System / Olympus | ORF, ORI | |
| Leica | DNG (M, Q, SL, CL), RWL, RAW | RWL/RAW are the Panasonic-made compacts. |
| Pentax / Ricoh | PEF, DNG | |

The open dialog also lists Hasselblad 3FR/FFF, Phase One and Leaf IIQ/MOS,
Mamiya MEF, Samsung SRW, Epson ERF, Kodak DCR/KDC/DCS, Minolta MRW, ARRI ARI
and Apple QuickTake QTK. Sigma X3F (Foveon, no colour filter mosaic) is not
offered. DNG opcodes other than GainMap are not applied anywhere.

## Camera profiles and lens corrections

- **Base curve.** Auto shows a RAW file with the camera's own look: a film
  curve fitted to the JPEG the camera embedded in the file (Camera Match, a
  little gentler than the camera), Film Standard when there is none; JPEG
  and TIFF pictures get no curve. The other film curves can be chosen in
  Base characteristics.
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
- **Purple fringing.** Lens corrections ▸ Purple fringing (0–100) removes
  the violet halo along bright, high-contrast edges, also for lenses the
  database does not know: near a strong edge, magenta to blue-violet
  pixels take the colour of the pixels around them that are not fringe,
  keeping their brightness (`develop::fringe`, in the working space after
  the profile). Fujifilm photos start at 50, other cameras at 0; a custom
  lens keeps its own value.

## Sharpening and noise reduction

Two nodes of the Details group, as Capture One's tools, on the CPU:

- **Sharpening**: Amount (0–100, 45 to start; 25 adds the edge detail
  once), Radius (0.2–2.5 px of
  the full-size photo, 0.8), Threshold (0–10 levels, 1) and Halo
  suppression (0–100). An unsharp mask on the luminance encoded with a 2.2
  gamma; R, G and B are scaled together, so colours keep their hue. The
  radius follows the size of the copy being rendered (a reduced preview
  gets a smaller radius, `ImageF32::pixel_size`), so the effect is judged
  at 100 %, as in Capture One.
- **Noise Reduction**: Luminance, Details, Color (50 to start) and Single
  pixel (0). The noise is measured on the image itself, so the same
  settings fit any ISO and a preview as well as the full photo; each plane
  (luminance and two colour differences of square-root encoded values) is
  cleaned with an "à trous" wavelet and the non-negative garrote, which
  keeps edges above the noise. Single pixel removes hot pixels on the
  full-size photo. Place it first in the graph, before tone and colour.

About 2.4 s (noise reduction) and 1 s (sharpening) for 24 MP on two
cores; a preview copy takes a tenth of that. `tools/adjust/src/filters.rs`
has the shared Gaussian, wavelet and noise estimate.

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

- **Models** are not in the repository: the editor downloads
  the original SCUNet weights from their authors (KAIR, 72 MB) and makes
  `scunet_color_real_448.onnx` (and on request the faster 960 px one) from
  them on this computer, with the `scunet-onnx` crate (`scripts/scunet` is
  the Python reference it is tested against); an earlier denoiser's are used too. They are looked for in
  `RNE_DENOISE_MODELS`, a `models` folder next to the program (or up to
  three folders above it), then the editor's models folder
  (`%LOCALAPPDATA%\raw-node-editor\models`) and an earlier denoiser's
  (`%APPDATA%\Denoiser\models`). Without a model the node passes the
  image through and says why.
- **GPU**: on Windows TensorRT for RTX (RTX 30xx and later, with
  `tensorrt_rtx_1_4.dll` and `tensorrt_onnxparser_rtx_1_4.dll` next to `rne`;
  uses the `_fp16` models) or DirectML (any DirectX 12 card); elsewhere the
  CPU, which is slow (about 17 s per megapixel on 12 threads).
  `RNE_DENOISE_DEVICE=cpu|directml|rtx` forces a device. With several tile
  sizes installed, the fastest for the GPU is measured once per run.
- ONNX Runtime is loaded at run time, shared with AI selection and tagging
  (see above; `ORT_DYLIB_PATH` points to another build, for example one with
  TensorRT for RTX). Without the `ai-denoise` feature (`--no-default-features`
  on `cli`) the tool is left out.

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

## The window

Left: the Library (Catalog Collections, Albums, the catalog's Folders, the
Disk, then rating/label/name/keyword filters and AI keywords) and the
filmstrip side by side, History and Metadata tabs below. Centre: the
photo above the node graph (a new photo has only the Source: click a
node's output port to add a tool after it; drag nodes, drag the background
to pan). Right: histogram and the settings of the node selected in the
graph. Filmstrip: click, Ctrl+click and Shift+click select as in
a photo viewer (several selected photos show as tiles in the viewer); keys:
arrows previous/next, 0–5 rating, 6–9 colour label (on the selection), F2
rename, Delete to the Trash, Ctrl+A select all, Escape back to one photo;
drag photos onto an album; Ctrl+Z over the Library undoes its last move to
the Trash or album merge. Photos come in through File ▸ Import Images…
and live in a catalog (`Name.rnecatalog`): see `docs/catalog.md`.

## Status

Phase 0 (foundations) is done; phase 1 (tracer bullet) is complete with the
first window. Development now follows the user's plan in five phases
(catalog and import, Library, metadata and tagging, filmstrip options,
variants). See `docs/progress.md`.
