# RAW node editor (working name)

Non-destructive RAW photo editor with a catalog and a DaVinci Resolve–style
node graph, written in Rust, GPU-accelerated on any vendor.
Windows and Ubuntu; macOS prepared.

The full design is in the development plan document; this README covers how
to build and where things live.

## Build and test

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
| `crates/core-color` | Transfer curves, primaries, camera → working-space matrices. |
| `crates/tool-api` | The `Tool` contract, parameter descriptors, the tool registry. |
| `crates/commands` | Edits with validation and undo/redo: the only way UI and MCP change a recipe. |
| `crates/develop` | Fixed development stage on the CPU (white balance, demosaic, camera matrix). |
| `crates/render-cpu` | Reference renderer: evaluates the graph with each tool's CPU code. |
| `crates/platform` | The single place where Windows, Ubuntu and macOS differ. |
| `crates/raw-io` | RAW/DNG → sensor data with rawler; levels, valid area, CFA, colour matrix. |
| `crates/gpu` | GPU engine on wgpu (Vulkan, DirectX 12, Metal); must match `render-cpu`. |
| `crates/export` | JPEG with an embedded sRGB profile, atomic writes, non-destructive names. |
| `crates/cli` | `rne` command: develop a RAW file to JPEG without the window. |
| `crates/tagging` | AI tagging logic: keyword list, embeddings, zero-shot scores, learning from decisions, suggestions. |
| `crates/tagging-onnx` | SigLIP 2 image/text encoders on ONNX Runtime (local, offline). |
| `tools/exposure` | First tool: exposure in stops, CPU reference + WGSL kernel. |

Pixel data inside the editor are linear, scene-referred `f32` in Rec.2020;
quantisation happens only on output.

## Adding a tool

1. Create `tools/<name>` with a type implementing `tool_api::Tool`
   (descriptor, CPU reference, GPU kernel).
2. Register it in the composition root (the app and the CLI).
3. Its parameters appear in the inspector and over MCP automatically.

## Try it

```sh
cargo run --release -p cli -- develop path/to/photo.ARW --exposure 0.5
cargo run --release -p cli -- info path/to/photo.ARW
cargo run --release -p cli -- gpu
cargo run --release -p cli -- tag photo.ARW --model path/to/siglip2 --keywords keywords.txt
```

AI tagging (semi-automatic keywords): see `docs/ai-tagging.md`.

Use `--release`: the CPU development stage is slow in debug builds.

RAW samples for the ignored real-file test go in `testdata/` (not committed):
`cargo test -p raw-io --release -- --ignored --nocapture`.

## Status

Phase 0 (foundations) is done; phase 1 (tracer bullet) has everything but
the window. Next: a minimal egui window. See `docs/progress.md`.
