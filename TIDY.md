# Tidy later

- `render-cpu::render` clones each node's image; fine for the reference
  renderer, revisit if it becomes a hot path.
- `develop` bilinear demosaic is preview quality; replace with a quality
  algorithm on the GPU (phase 3) and keep this one as the test reference.
- `commands::EditSession` drops the oldest step with `Vec::remove(0)`;
  switch to `VecDeque` if profiling ever shows it.
