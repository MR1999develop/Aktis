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
| `crates/core-color` | Transfer curves, primaries, camera → working-space matrices. |
| `crates/tool-api` | The `Tool` contract, parameter descriptors, the tool registry. |
| `crates/commands` | Edits with validation and undo/redo: the only way UI and MCP change a recipe. |
| `crates/develop` | Fixed development stage on the CPU (white balance, demosaic, camera matrix). |
| `crates/render-cpu` | Reference renderer: evaluates the graph with each tool's CPU code. |
| `crates/platform` | The single place where Windows, Ubuntu and macOS differ. |
| `crates/raw-io` | RAW/DNG → sensor data with rawler; levels, valid area, CFA, colour matrix. |
| `crates/gpu` | GPU engine on wgpu (Vulkan, DirectX 12, Metal); must match `render-cpu`. |
| `crates/export` | JPEG with an embedded sRGB profile, atomic writes, non-destructive names. |
| `crates/engine` | Shared by window and command line: built-in tools, GPU render with CPU fallback, preview copy, display pixels, JPEG export. |
| `crates/editor` | The editor without the window: documents and undo, background jobs (open, preview, export), viewer zoom and pan. |
| `crates/app` | The desktop window (egui): menu, viewer, inspector, status bar. |
| `crates/cli` | `rne` command: develop a RAW file to JPEG without the window. |
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
cargo run --release -p app -- path/to/photo.ARW     # the window
cargo run --release -p cli -- develop path/to/photo.ARW --exposure 0.5
cargo run --release -p cli -- info path/to/photo.ARW
cargo run --release -p cli -- gpu
```

Use `--release`: the CPU development stage is slow in debug builds.

RAW samples for the ignored real-file test go in `testdata/` (not committed):
`cargo test -p raw-io --release -- --ignored --nocapture`.

## Status

Phase 0 (foundations) is done; phase 1 (tracer bullet) is complete with the
first window. See `docs/progress.md`.
