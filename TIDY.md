# Tidy later

- `render-cpu::render` clones each node's image; fine for the reference
  renderer, but with the GPU unavailable it is the CPU fallback's main cost.
- `develop` bilinear (Bayer) and gradient-weighted (X-Trans) demosaics are
  preview quality; replace with quality algorithms on the GPU (phase 3,
  Markesteijn or similar for X-Trans) and keep these as the test reference.
- DNG opcodes other than GainMap (WarpRectilinear, FixBadPixels, MapTable …)
  are only reported; GainMap is applied on the CPU in `raw-io` at open time.
  When rawler gains opcode support (after 0.8.0), compare and drop ours.
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
- One ONNX Runtime for denoise, selection and tagging (`onnx-env`, loaded at
  run time). `get-ai.bat` installs the DirectML build (1.24): the TensorRT for
  RTX provider of the denoiser needs a build that has it; offer it as an
  option of `get-ai.bat`. The `ai-denoise` and `tagging-model` CI jobs still
  expect a runtime downloaded at build time: give them the ORT download step
  of the `ai-masks` job.
- `ai-models`: every file comes from its authors. SCUNet is downloaded as
  the KAIR checkpoint (`scunet_color_real_psnr.pth`, 72 MB) and made into
  the ONNX models on the user's computer by `scunet-onnx`; the checkpoint is
  kept in `models/sources` so the 960 px model needs no second download
  (delete it once both models are made, if disk space matters). SAM 2.1 and
  the MediaPipe person-parts model have no ONNX from their authors (only
  PyTorch and TFLite): they still come from third-party conversions, which
  the models window says by pointing at the authors' page. The NuGet ONNX Runtime packages and SigLIP 2 have no pinned size
  or SHA-256 yet: pin them once downloaded.
- Downloads use the system's `curl` and `tar` (Windows 10 and later have
  both); an in-process HTTP client would avoid depending on them.
- Filmstrip thumbnails are made from the embedded JPEG each time a folder
  is shown; keep them in a disk cache (keyed by path, size and date) once
  folders of thousands of photos are common.
- `ThumbCache` finds the least recently used entry by scanning (300
  entries); use a proper LRU list if the capacity grows.
- The Explorer lists drive letters A–Z with `Path::is_dir` at start-up; a
  disconnected network drive can make that slow. List drives in the worker.
- Edits live only in the catalog (no sidecar files), keyed by path like
  ratings: renaming in the editor moves them; a file renamed or moved
  outside the editor loses its edits until a "relink" or a content key
  (size + date + hash) is added.
- Dialogs (`app::dialog`: Rename, AI models) are drawn as a centred area,
  not a modal: clicks behind them still reach the window. Move them all to
  `egui::Modal` together, keeping `dialog_buttons` for the buttons.
- AI tagging loads SigLIP 2 at each "AI-tag this folder"; keep the encoders
  loaded while the window is open when tagging becomes frequent.
- Tool bar tools (crop, perspective, level, masks, AI select, heal, clone)
  are placeholders.
- Retouch: spots are healed on the CPU (host tool), one after another; a
  GPU kernel would avoid the read-back when the GPU renders. A very large
  spot copies two patches of its size (about 220 MB at the largest radius
  on a 24 MP image).
- Retouch: the undo label of a spot lists its parameters ("Change spot0
  kind, spot0 x, …"); give `Edit::SetParams` a label from the tool (for
  example "Add heal spot", "Move spot").
- Retouch: the automatic source is searched in the developed preview, not
  in the image entering the retouch node (earlier spots and tools are not
  seen), and only the selected retouch node's spots are drawn.
- The catalog keys photos by the path as written: on Windows the same file
  reached as `D:\Foto` and `d:\foto` gets two entries. Normalise the key
  (canonical path, case folded on Windows) before the first public release,
  with an upgrade of existing catalogs (`SCHEMA_VERSION`).
- `Catalog::metas` runs two queries per photo on the window's thread when a
  folder is shown; fine for hundreds of photos. Read the whole folder in one
  query (or in the worker) once folders of thousands of photos are common.
- The window's minimum size is 1100 × 640; the design guidelines ask for a
  layout that works at 1024 × 600 (about 960 × 520 usable). Check the
  Explorer, graph and inspector at that size and lower the minimum.
- Saving an edit makes its preview JPEG (downscale + encode of the image
  on screen, tens of milliseconds for a full-size render) on the window's
  thread, and the filmstrip reads preview blobs from the catalog there too
  when its thumbnail list changes; move both to the worker if it shows.
- A variant left before its first render keeps the preview it had (or the
  camera's) until its next change; store a "preview stale" flag in the
  catalog if that turns out to matter.
- A replaced preview briefly shows "…" in its filmstrip card (the old
  texture is forgotten before the new one is decoded).
- Variants are named "Variant N"; renaming them, and showing every variant
  as its own filmstrip card as Capture One can, are not offered yet.
- Undoing a history rewrite (reorder/delete in the History tab) is not
  saved: after reopening, the rewritten steps are the history.
- The GPU renderer still uploads the source and reads the result back at
  every render, and creates its buffers each time; keep the source on the
  GPU between renders of the same image (and draw the result straight from
  a texture, see above) if the timings in the status bar show it matters.
- `engine::geometry` warps on the CPU after the render, for every screen
  render while a geometry is set; move it to the GPU with the display
  transform. The fill enlargement is found by bisection on 64 border points.
- High dynamic range is a per-pixel tone curve; Capture One's is local
  (neighbourhood-aware): add a local version (guided filter on log luminance).
- The white-balance node re-balances with one Bradford matrix in the working
  space; re-developing with new multipliers (and the profile's illuminant
  interpolation) would match Capture One more closely for strong changes.
- Levels and Curve read the histogram of the image on screen (after the
  output curve), not of the node's input.
- The mask overlay is evaluated on the UI thread at up to 1024 px when the
  mask changes; move it to the worker if it is felt.
- AI Select loads ONNX Runtime and the models in its first job (seconds);
  they stay loaded. Person parts and "select people" are not offered in the
  window yet (the command line has them).
- Export: PNG uses the built-in deflate of `export` (hash chains, no lazy
  matching): files are somewhat larger than zlib's; TIFF is uncompressed.
  Add Deflate/LZW to TIFF and tune PNG if file sizes matter.
- Export shrinks with an area average (no ringing, a little soft) and adds
  no output sharpening; offer Lanczos and "sharpen for screen/print" later.
- EXIF text fields (copyright, artist) are written as UTF-8; strict EXIF
  readers show "©" as Latin-1. The XMP packet carries them correctly.
- The export window, like the other dialogs, is not modal (see above).
- Colour editor: Capture One's "View selected color range" is missing; the
  viewer has no way to show which pixels a range selects. Add it when the
  viewer can draw a node's weight as an overlay (as the mask overlay does).
- Presets: a preset applied to photos that are not open is written to the
  catalog without a new preview; the filmstrip shows the camera's until the
  photo is opened (a background render of the edit would fill it).
- Presets: no import/export of Lightroom (XMP `crs:`) or Capture One
  (`.costyle`) presets yet; their tools differ, only a subset would map.
