# Progress

## Phase 0 — Foundations: done

- Cargo workspace, toolchain pinned to Rust 1.97, lints (`unsafe_code` forbidden,
  clippy with warnings as errors in CI), rustfmt config.
- CI on Windows and Ubuntu (fmt, clippy, tests); macOS `cargo check` only.
- `core-model`: validated ids, `ImageF32` (size checked before allocating),
  `SensorImage`, `Params`, node graph (Source/Output protected, acyclic,
  one link per input, serial insert, removal that reconnects the chain,
  evaluation order), `Recipe`.
- `tool-api`: `Tool` trait, descriptors validated at registration,
  `ResolvedParams` (missing → default, out of range → clamped, wrong type →
  default), GPU uniform packing, registry.
- `commands`: `Edit` + `EditSession` with undo/redo, slider gestures merged
  into one undo step, strong guarantee (a refused edit changes nothing),
  500-step limit.
- `platform`: dialog button order, settings folder per OS.

## Phase 1 — Tracer bullet: in progress

Done:

- `core-color`: sRGB curve, Rec.2020/sRGB matrices, camera → working matrix.
- `develop`: white balance on the mosaic, bilinear Bayer demosaic with
  mirrored borders, camera matrix; linear RGB files (Linear DNG, Apple
  ProRAW) skip demosaicing (X-Trans reported as not supported yet).
- `core-model`: EXIF orientation applied exactly (pixels moved, not resampled).
- `tools/exposure`: CPU reference and WGSL per-pixel kernel.
- `render-cpu`: graph evaluation, missing tools passed through and reported,
  provisional sRGB 8-bit output (clip; tone mapping comes in phase 4).
- `raw-io` (rawler 0.8.0, pinned): file size checked first, decoder panics
  turned into errors, black/white levels per CFA position keeping noise below
  black, default crop / active area, Bayer phase at the crop origin, X-Trans
  tile, D65 colour matrix preferred.
- `gpu` (wgpu 29): adapter + device, per-pixel kernel template, one pipeline
  per tool (cached, shader errors caught), whole chain on the GPU with one
  readback, chunking within buffer/dispatch/memory limits.
- `export`: JPEG (4:4:4 from quality 90) with an sRGB ICC profile built in
  code (checked against LittleCMS's own sRGB: identical output), atomic write,
  `photo (2).jpg` instead of overwriting.
- `cli`: `rne develop | info | gpu`, GPU with CPU fallback.
- `engine`: what the window and the CLI share (tools, GPU-or-CPU render,
  preview copy, display pixels, JPEG export).
- `editor`: documents with undo/redo (slider drag = one step), background
  worker (open, previews where only the newest request is kept, export),
  render plan (fast preview while editing, one full-size render when idle,
  late results discarded), viewer geometry (fit, 100 % on HiDPI, zoom at
  the cursor, pan clamped to the image).
- `app` (egui 0.37, wgpu 30): open by dialog, drag and drop, command line
  or recent files; viewer with wheel/pinch zoom, double-click 100 %/fit,
  drag to pan; inspector generated from tool descriptors (slider, reset
  from the context menu, bypass); Edit ▸ Undo/Redo with labels; Export
  (Ctrl+E, next to the RAW or to a chosen folder, never overwriting) with a
  "Show in folder" toast; status bar with camera, size, zoom, GPU; theme
  System/Light/Dark (dark by default); settings and recent files remembered.
- `tools/denoise-ai`: AI denoise node, SCUNet on ONNX Runtime ported from
  an earlier denoiser (tiles with a 64 px mirrored margin, TensorRT for RTX /
  DirectML / CPU with fallback and tile-size search, grain and colour amounts
  applied to a cached network output). Linear light is sRGB-encoded in the
  working primaries for the network; noise below black is denoised, the
  correction fades out above encoded 1.0.
- `tool-api`: host tools (`GpuKernel::Host`, fallible `Tool::apply`): the GPU
  renderer splits the chain around them; both renderers pass a failing node
  through and report it (`tool_errors`).
- `cli`: `rne develop --denoise <grain %> [--denoise-color <colour %>]`;
  `engine` registers the node, the window shows it bypassed in every new
  document (switch it on in the inspector) and tells when it cannot run.

## Camera profiles and lens corrections: done

- Camera profiles (`core-color`): dual-illuminant matrices weighted by the
  white balance's colour temperature (DNG SDK method: neutral → xy by fixed
  point, Robertson CCT, inverse-temperature mix, Bradford to D50/D65); DCP
  reader (matrices, forward matrices, HueSatMap, LookTable; checked against
  all 161 RawTherapee profiles) and a search of the editor's own `profiles`
  folder by `UniqueCameraModel`. Recipe: `ProfileChoice`.
- Lens corrections (`lens-db`, `develop`): lensfun XML read directly (own
  small XML reader), built-in data for the major brands, newer files from the
  user's `lensfun` folder; lensfun's camera/lens lookup (strict word match,
  mount and sensor-size checks, maker aliases), spline/IDW interpolation and
  coordinate rescaling; vignetting gain, then distortion + TCA resampling per
  channel (Catmull-Rom, multithreaded) with automatic zoom against empty
  borders. Recipe: `LensSettings`; edits `SetCameraProfile`,
  `SetLensCorrection` with undo. `rne info | lenses | profiles`, `develop
  --profile --lens --lens-zoom --no-lens`.
- Window: "Lens corrections" section at the top of the inspector (on/off,
  lens chosen automatically or from the lenses that fit the camera,
  distortion / chromatic aberration / vignetting, fill the frame); changes
  and their undo develop the image again in the background.
## AI selection and masks: done (command line); mask panel in the window to do

Design and models in `docs/ai-masks.md`.

- `core-model`: `MaskImage`, `MaskDef` with components (add, subtract,
  intersect) that are AI selections or brush strokes; AI prompts in
  normalised coordinates (subject, clicks, box, person part); raster keys,
  `ModelId`, `BodyPart`; `NodeKind::Mask` with a key output; orientation
  inverse and point mapping.
- `commands`: `AddMask` (mask node on a tool's key input) and `SetMask`,
  undoable, merged within a gesture.
- `masks`: rasters scaled to the render; refinement by colour (uncertain
  areas judged against object and background colours, colour guided filter);
  brush strokes; feather, combination; stable FNV-1a keys; raster cache on disk.
- `render-cpu`, `gpu`: tools blend through their key input; GPU = CPU with
  masks is tested.
- `ai-segment`: ONNX Runtime loaded at run time (DirectML, CUDA,
  ROCm/MIGraphX, OpenVINO, Core ML, CPU); BiRefNet (subject); Segment
  Anything 2.1 Large (clicks, boxes; a second pass on a crop for small
  objects); YOLOX-S (people and COCO classes → one component per person);
  MediaPipe multiclass + BlazeFace + face mesh (hair, face skin, body skin,
  clothes, accessories, eyebrows, sclera, irises, lips, teeth); catalog with
  SHA-256.
- `cli`: `rne mask`, `rne models`, `rne develop … --subject`; `--add`,
  `--subtract`, `--intersect`, `--parts`, `--brush`, `--erase`; PPM input.
- Checked by eye on test photos (dog and bicycle, truck, people at a bus,
  portraits): wheels empty between the spokes, no halos, parts of each
  person separate.

## Tests and CI

GitHub Actions (`.github/workflows/ci.yml`) on every push:

- Windows and Ubuntu: fmt, clippy with warnings as errors, all tests.
  On Ubuntu the GPU tests run on Mesa's software Vulkan driver (llvmpipe)
  and must pass (`RNE_REQUIRE_GPU=1`); elsewhere they skip without a GPU.
- macOS: `cargo check` only.
- `real-raw`: downloads a public-domain Nikon D3S NEF (NASA, from the rawpy
  test set), runs the ignored real-file test, `rne info` and `rne develop`
  on the GPU and on the CPU, then opens the file in the window headless
  (egui_kittest on wgpu) and saves a screenshot (`window.png`).
- `lockfile`: when a `Cargo.toml` adds dependencies, it resolves them,
  commits `Cargo.lock` and the other jobs build that commit.

Each job pushes its log and output files to the branch `ci-results/<branch>--<job>`
(Linux, Windows, macOS, real-raw), so results can be read with git:
`git fetch origin "refs/heads/ci-results/*:refs/remotes/origin/ci-results/*"`.

- `ai-denoise`: generates the SCUNet model from the KAIR weights, runs the
  network test on the CPU, then `rne develop --denoise 80` on the Nikon D3S
  (JPEGs with and without the node are published).

First green run: Nikon D3S 4256×2832 develops in about 2 s on llvmpipe;
the GPU and CPU renders look the same.
