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
