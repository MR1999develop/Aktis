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

To do:

- First build of `raw-io`, `gpu` and `cli` on the development machine
  (crates.io is not reachable from the cloud workspace).
- `app`: minimal egui window — open, viewer, exposure slider, export, undo.

## Tests

Pure crates and export: 87 tests pass in the cloud workspace (with a type
stub for rawler and the gpu API). GPU tests (`gpu_matches_cpu`) and the
real-file test run only on a machine with the real dependencies; the GPU
tests skip without a GPU unless `RNE_REQUIRE_GPU=1`.
