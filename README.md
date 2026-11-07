# Aktis

Non-destructive RAW photo editor with a catalog, tool panels and layers as
Capture One's, written in Rust, GPU-accelerated on any vendor.
Windows and Ubuntu; macOS prepared.

The full design is in the development plan document; this README covers how
to build and where things live.

## Build and test

On a new Windows PC, double-click `scripts\setup-windows.bat` first: it
installs Git, the Visual Studio 2022 C++ build tools, rustup and the pinned
toolchain with winget (log in `target\setup.log`).

On Windows, double-click `scripts\check.bat`: it runs format check,
`cargo deny` (known vulnerabilities, licences and sources of the
dependencies, see `deny.toml`; install it once with
`cargo install cargo-deny --locked`), clippy, tests and a release build,
showing everything on screen, and writes
`target\check.log` (full output) and `target\check-summary.txt` (environment:
git commit, Rust, GPU, ONNX Runtime and models; then status, time and the
errors, warnings and failed tests of each step). `check.bat -Quick` skips the
release build, `check.bat -Step test` runs one step. `scripts\run.bat` starts the editor (drop a RAW
file on it to open that file).

AI (denoise, selection, tagging): the program ships without ONNX Runtime and
the models. The first time a tool needs them, the editor shows what is needed
(size, licence) and downloads it on request; *Tools → AI models* lists every
pack. On the command line the AI commands ask the same way, and
`aktis-cli models --download <tool|all>` installs ahead of time;
`scripts\get-ai.bat` does that for everything. All of it goes into
`%LOCALAPPDATA%\aktis\models` (crate `ai-models` keeps the list,
with sizes and SHA-256; `onnx-env` loads the one runtime the three AI modules
share). Then drop a RAW file on `scripts\ai-check.bat` to see subject, people,
person parts, subtract and brush selections in `target\ai-check\`.

By hand:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`rust-toolchain.toml` pins the compiler (Rust 1.99.1); rustup installs it on first use.

The scripts build through `scripts\build-env.ps1`, which takes this
computer's paths (user folder, Cargo's folder, project folder) out of the
programs with `--remap-path-prefix`; the `.bat` files call cargo through
`scripts\cargo.ps1` for that. Its flags differ from a plain `cargo` run, so
switching between the two rebuilds the workspace once.

The window is `aktis`, the command line `aktis-cli`; environment variables
start with `AKTIS_`. Settings, data and caches live in an `aktis` folder of
the system's usual places. Folders of the editor's old name
(`raw-node-editor`) are renamed at the first start, with the files inside
given the new extensions (`.aktispreset`, `.aktisbrush`, `.aktisdefaults`,
`.aktislens`, `.aktismask`); catalogs made as `*.rnecatalog` keep their name
and still open.

## Layout

| Crate | Responsibility |
| --- | --- |
| `crates/core-model` | Domain types: images, sensor data, parameters, layers, recipe. No I/O. |
| `crates/core-color` | Transfer curves, primaries, camera profiles (dual-illuminant matrices, DCP files). |
| `crates/tool-api` | The `Tool` contract, parameter descriptors, the tool registry. |
| `crates/commands` | Edits with validation and undo/redo: the only way UI and MCP change a recipe. |
| `crates/develop` | Fixed development stage on the CPU (white balance, Bayer demosaic with RCD, X-Trans demosaic with Markesteijn, false-colour suppression, lens corrections, camera profile). |
| `crates/render-cpu` | Reference renderer: renders the layers with each tool's CPU code. |
| `crates/platform` | The single place where Windows, Ubuntu and macOS differ. |
| `crates/raw-io` | RAW/DNG → sensor data with rawler; levels, valid area, CFA, DNG gain maps, colour matrices, lens and shooting data. |
| `crates/gpu` | GPU engine on wgpu (Vulkan, DirectX 12, Metal); must match `render-cpu`. Software renderers (a virtual machine's) are refused, so the CPU engine runs instead; `AKTIS_SOFTWARE_GPU=1` accepts them. |
| `crates/export` | JPEG with an embedded sRGB profile, atomic writes, non-destructive names. |
| `crates/engine` | Shared by window and command line: built-in tools, GPU render with CPU fallback, preview copy, display pixels, JPEG export. |
| `crates/editor` | The editor without the window: documents and undo, background jobs (open, preview, export, folder listings, thumbnails), viewer zoom and pan, folder tree and filmstrip model, layers, histogram. |
| `crates/catalog` | Ratings, colour labels and keywords of each photo (SQLite) and the Explorer's filters. |
| `crates/app` | The desktop window (egui): tool bar, Explorer, History and Metadata tabs, viewer, filmstrip, histogram, tool panels (docked or floating) with Layers, status bar. |
| `crates/lens-db` | Lensfun database reader, camera/lens lookup, distortion/TCA/vignetting maths. |
| `crates/masks` | Layer masks → mask images: AI rasters refined by colour, brush strokes, add/subtract/intersect, feather, raster cache. |
| `crates/ai-segment` | AI selection (subject, objects, people, hair/skin/clothes/face features) with ONNX Runtime loaded at run time; model catalog. |
| `crates/cli` | `aktis-cli` command: develop a RAW file to JPEG without the window. |
| `crates/tagging` | AI tagging logic: keyword list, embeddings, zero-shot scores, learning from decisions, suggestions. |
| `crates/tagging-onnx` | SigLIP 2 image/text encoders on ONNX Runtime (local, offline). |
| `tools/exposure` | First tool: exposure in stops, CPU reference + WGSL kernel. |
| `tools/denoise-ai` | AI denoise: the SCUNet network on ONNX Runtime (TensorRT for RTX, DirectML, CPU). A host tool: runs outside the WGSL kernels. |
| `scripts/scunet` | Python + NumPy reference for `scunet-onnx`, which makes the SCUNet ONNX models from the original weights. |
| `crates/mutate` | Test helper only: damaged copies of a valid file, which every parser's tests feed it. |
| `fuzz` | Coverage-guided fuzzing of the parsers (`cargo fuzz`, nightly; its own workspace). See `fuzz/README.md`. |

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
| Fujifilm | RAF | X-Trans and Bayer (GFX) sensors; uncompressed, lossless and lossy compressed RAF. X-Trans sensors use Markesteijn's three-pass demosaic with VNG in flat areas (darktable's dual demosaic); Bayer sensors, GFX included, use RCD, or LMMSE from ISO 3200. |
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

- **Base curve.** Standard shows a RAW file with the camera's own look: a
  film curve fitted to the JPEG the camera embedded in the file (a touch
  gentler than the camera), a film curve when there is none; JPEG and TIFF
  pictures get no curve. Linear, the other choice, shows the pixels as they
  are. The film curves take a little saturation from reds.
- **Demosaic.** Chosen for each photo, not offered as a setting: RCD for
  Bayer sensors, LMMSE for Bayer sensors from ISO 3200 (it copes better
  with noise), Markesteijn three-pass with VNG in the flat areas for X-Trans.
- **Colour noise.** A RAW file shot at ISO 400 or more starts with Noise
  Reduction on for colour noise only (unless its camera's defaults have
  their own), which can be changed or reset like any tool.
- **Profiles.** By default each camera uses its own calibration matrices
  (illuminant A and D65 for about 740 cameras, from rawler's camera data),
  mixed according to the white balance as the DNG specification describes.
  DCP profiles copied into the editor's own `profiles` folder can be chosen
  by name (`aktis-cli profiles photo.ARW` lists them); other programs' folders are
  not searched. Matrices, forward
  matrices, hue/saturation maps and look tables are applied; profile tone
  curves wait for tone mapping (phase 4).
- **Lens corrections.** `data/lensfun/` holds the lensfun database for the
  major camera and lens makers (CC BY-SA 3.0, see its README). Camera and
  lens are recognised from the EXIF data; distortion, lateral chromatic
  aberration and vignetting are corrected in camera RGB between demosaicing
  and the profile, with lensfun's own interpolation and maths. Newer lensfun
  XML files dropped in the editor's `lensfun` folder override the built-in
  data. `aktis-cli info` shows what would be corrected; `aktis-cli lenses <words>` searches
  the database.
- **Purple fringing.** Lens corrections ▸ Purple fringing (0–100) removes
  the violet halo along bright, high-contrast edges, also for lenses the
  database does not know: near a strong edge, magenta to blue-violet
  pixels take the colour of the pixels around them that are not fringe,
  keeping their brightness (`develop::fringe`, in the working space after
  the profile). Fujifilm photos start at 50, other cameras at 0; a custom
  lens keeps its own value.

## Sharpening and noise reduction

Two tools of the Details group, as Capture One's, on the CPU:

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
  full-size photo. It acts before tone and colour (the fixed order below).

About 2.4 s (noise reduction) and 1 s (sharpening) for 24 MP on two
cores; a preview copy takes a tenth of that. `tools/adjust/src/filters.rs`
has the shared Gaussian, wavelet and noise estimate.

## Adding a tool

1. Create `tools/<name>` with a type implementing `tool_api::Tool`
   (descriptor, CPU reference, GPU kernel).
2. Register it in the composition root (the app and the CLI).
3. Its parameters appear in the inspector and over MCP automatically.

## AI denoise

The `denoise_ai` tool removes noise with the SCUNet neural network (trained
on real camera noise, Apache-2.0). The network runs
once per input image; its two parameters, *Grain (luminance)* and *Colour
blotches*, only decide how much of its correction is applied, so moving them
is instant. It acts right after retouching, before every other tool.

- **Models** are not in the repository: the editor downloads
  the original SCUNet weights from their authors (KAIR, 72 MB) and makes
  `scunet_color_real_448.onnx` (and on request the faster 960 px one) from
  them on this computer, with the `scunet-onnx` crate (`scripts/scunet` is
  the Python reference it is tested against). They are looked for in
  `AKTIS_DENOISE_MODELS`, a `models` folder next to the program (or up to
  three folders above it), then the editor's models folder
  (`%LOCALAPPDATA%\aktis\models`). Without a model the tool passes the
  image through and says why.
- **GPU**: on Windows TensorRT for RTX (RTX 30xx and later, with
  `tensorrt_rtx_1_4.dll` and `tensorrt_onnxparser_rtx_1_4.dll` next to `aktis-cli`;
  uses the `_fp16` models) or DirectML (any DirectX 12 card); elsewhere the
  CPU, which is slow (about 17 s per megapixel on 12 threads).
  `AKTIS_DENOISE_DEVICE=cpu|directml|rtx` forces a device. With several tile
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
Disk, then rating/label/name/keyword filters and AI keywords), History and
Metadata tabs below. Centre: the photo above the filmstrip (a row of cards;
drag its top edge to make them larger). Right: the histogram above the tool
panels, as Capture One's: Layers, Base Characteristics, Lens Correction, one
panel per tool and the Spots of Heal and Clone. A click on a panel's header
opens or closes it; dragging the header moves the panel, and dropped outside
the column it floats in a window of its own (its … menu floats and docks it
too; closing the window docks it; View ▸ Reset tool panels puts them all
back). The order, which are closed and which float are kept, also in each
workspace.

Edits are in layers, as Capture One's: the Background holds the tools that
act on the whole photo; each layer above it has a mask (brush, gradients, AI
selection) and tools of its own, which act on the image under it and are
mixed in through the mask, at the layer's opacity. The tool panels show and
change the layer chosen in the Layers panel. Masks (B) and AI Select (A) draw
the mask of the chosen layer; on the Background the first stroke or click
adds a new layer. Heal and Clone put their spots in the chosen layer. In a
layer every tool acts once, in a fixed order: retouch, AI denoise, noise
reduction, white balance, exposure, high dynamic range, levels, curve, colour
balance, colour editor, black & white, dehaze, clarity, sharpening,
vignetting. Edits saved before the layers (the node graph of format 1) are
not read: such a photo starts again from its first settings, and the old
edit stays in the catalog until the photo is changed.

Filmstrip: click, Ctrl+click and Shift+click select as in a
file manager (several selected photos show as tiles in the viewer); keys:
arrows previous/next, 0–5 rating, 6–9 colour label (on the selection), F2
rename, Delete to the Trash, Ctrl+A select all, Escape back to one photo;
drag photos onto an album; Ctrl+Z over the Library undoes its last move to
the Trash or album merge. Photos come in through File ▸ Import Images… (or the Import… button,
left of Undo on the bar under the menu) and live in a catalog (`Name.aktiscatalog`): see `docs/catalog.md`.
Zoom: the wheel and pinch around the pointer, a double click between fit
and 100 %, and on the right of the status line Zoom out, the zoom in percent
(type or drag), Levels (Capture One's zoom levels), Zoom in, Fit, 100 % and
Keep zoom (another photo opens at the same zoom and place, to compare one
detail). In a narrow window the status line drops the shooting data, then
the lens, and shortens the GPU's name.
A photo with several variants shows each as a card of its own in the
filmstrip, right after the one before ("IMG_0001.CR3 (2)"), with its own
preview; a click opens that variant. Deleting a variant, the first one
included, leaves the photo and its other variants.

## Security

The editor opens files made by others and downloads AI models, so every
parser works under size limits and checked arithmetic, and is tested with
damaged files. Every download is checked against its SHA-256 before it is
used. `docs/threat-model.md` says what is defended and which risks are
accepted. `SECURITY.md` says how to report a vulnerability. On GitHub the
`Audit` workflow checks the dependencies every day, and the `Fuzz`
workflow (started by hand) fuzzes the parsers.

## Contributions

Aktis is maintained by MR alone; outside contributions (issues, pull
requests) are not accepted.

## Licence

Copyright (C) 2026 MR. Aktis is free software under the GNU General Public
License, version 3 only (`GPL-3.0-only`); the full text is in `LICENSE`.

## Status

Phase 0 (foundations) is done; phase 1 (tracer bullet) is complete with the
first window. Development now follows the user's plan in five phases
(catalog and import, Library, metadata and tagging, filmstrip options,
variants). See `docs/progress.md`.
