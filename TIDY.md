# Tidy later

- `render-cpu::render` clones each node's image; fine for the reference
  renderer, revisit if it becomes a hot path.
- `develop` bilinear demosaic is preview quality; replace with a quality
  algorithm on the GPU (phase 3) and keep this one as the test reference.
- `commands::EditSession` drops the oldest step with `Vec::remove(0)`;
  switch to `VecDeque` if profiling ever shows it.
- `render_cpu::to_srgb8` and `engine::downscale` run on one thread; a
  full-size display conversion of a 24 MP image takes a noticeable fraction
  of a second. Parallelise with rayon or move it to the GPU when the viewer
  shows the image straight from a GPU texture.
- The window and the renderer use two wgpu devices. Sharing eframe's device
  (`CreationContext::wgpu_render_state`) would let the viewer draw the render
  without the read-back; do it with the GPU display transform (phase 4).
- Fonts: egui's default font for now. Windows should load Segoe UI from the
  system, Ubuntu should ship Inter or Noto Sans (see the design guidelines).
- File dialogs have no parent window yet (`rfd::FileDialog::set_parent`), so
  on Windows they are not modal to the editor.
- Lens corrections and DCP tables run on the CPU in `develop`; move them to
  the GPU with the demosaic (phase 3), keeping these as the reference.
- DCP tone curves and `BaselineExposureOffset` are read but not applied:
  wire them into tone mapping (phase 4).
- Profiles embedded in DNG files (forward matrices, hue/sat maps) are not
  read yet: rawler only exposes the colour matrices.
- Lensfun fisheye lenses are corrected within their own projection; a
  fisheye → rectilinear conversion is not offered.
- In-camera crop modes (APS-C crop on a full-frame body) use the body's crop
  factor; deriving it from the image size would be more robust.
- `denoise-ai` keeps the tile-speed profile only for the running process;
  persist it in the settings folder (as an earlier denoiser does) once the app has
  settings, so the GPU search is not repeated at each start.
- `denoise-ai` fingerprints the whole input image (about 0.3 s at 24 MP) to
  reuse the network output; the app can key the cache on the upstream
  recipe instead.
- The GPU renderer reads back before a host tool and uploads after it; fine
  for the denoiser (seconds of network time), revisit if host tools multiply.
- With the AI denoise node on, the editor's preview worker is busy for the
  whole network run (seconds on a GPU, minutes on a CPU) with no progress
  and no cancel. Port an earlier denoiser's `JobControl` and progressive tiles
  (starting from the visible area) into `editor::worker` when the node
  becomes a common one.
- Masks are evaluated on the CPU at full resolution for every render and
  uploaded to the GPU; move refinement and feather to WGSL and cache per node
  once the viewer renders previews interactively.
- `ai-segment` decoder inputs clone the Segment Anything embedding (16 MB)
  per click; pass views once profiling shows it matters.
- Person parts sample the class maps pixel by pixel over the whole raster;
  restrict to the person's box when large photos show it is slow.
- CPU sessions use basic graph optimisations to avoid an ONNX Runtime 1.24
  bug (see docs/ai-masks.md); re-test the extended level with newer releases.
- The SAM 2.1 and MediaPipe ONNX files come from third-party GitHub
  releases; export them from the official weights and host them in the
  project's own release.
