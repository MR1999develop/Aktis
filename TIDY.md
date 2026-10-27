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
