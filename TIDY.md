# Tidy later

- `render-cpu::render` clones the image under each layer to mix it in; fine
  for the reference renderer, but with the GPU unavailable it is the CPU
  fallback's main cost.
- `develop`: Bayer with RCD, X-Trans with Markesteijn (3 passes), both on
  the CPU; move them to the GPU (phase 3) keeping these as the test
  reference. The simple X-Trans demosaic (`develop::xtrans`) stays for
  images smaller than 12 pixels.
- Markesteijn on a 26 MP X-Trans file is about 4× the simple demosaic
  (CIELab of eight candidates per photosite); SIMD or the GPU if opening
  Fujifilm files feels slow.
- LMMSE (`develop::lmmse`, Bayer from ISO 3200) is plain scalar code with
  clamped reads in every window: likely 2–3 s on a 24 MP file; precompute
  the windows' running sums, SIMD or the GPU.
- X-Trans dual demosaic (`develop::vng`): the detail thresholds (`FLAT`,
  `DETAIL`, on the square-root luminance gradient) are first guesses; tune
  them on real noisy Fujifilm files, perhaps from the measured noise.
- The colour noise reduction a high-ISO RAW starts with
  (`camera_defaults::start_noise_reduction`) uses fixed values, not the
  Noise Reduction values saved as the camera's defaults (Save as Defaults
  of the tool alone); only saved Noise Reduction defaults replace it.
- Base curve "Standard" on a JPEG or TIFF shows the picture without a curve
  (as Linear): the menu says Standard either way.
- Variants in the filmstrip: rating, colour label, keywords, selection and
  album membership belong to the photo (Capture One keeps them per
  variant); the arrow keys move from photo to photo, not from variant to
  variant; variant previews are decoded on the interface thread (small
  JPEGs, once each).
- RCD on a 50 MP file takes about 1.3× the old bilinear demosaic; SIMD or
  the GPU if opening large files feels slow.
- Camera Match: the fit compares luminance only; the camera's saturation
  (Fujifilm film simulations) is not matched yet. The softening (3 %) is
  fixed; a control would let the user choose.
- Camera Match is fitted when the photo opens (about 0.1 s); the export of
  a photo not open fits it again from the full development.
- Purple fringing (`develop::fringe`): global only; Capture One also offers
  it in local adjustment layers (a mask). Green fringing (behind the focus
  plane) is not touched. The thresholds (edge 0.25–0.75 stops, chroma,
  hue window) were set on synthetic images: tune them on real Fujifilm and
  fast-lens photos.
- Sharpening and Noise Reduction run on the CPU (Host kernels): about
  1 s and 2.4 s for 24 MP on two cores. Move them to the GPU (separable
  passes) if editing full-size photos feels slow. Capture One also derives
  the noise reduction from camera and ISO and applies some by default; ours
  only runs when the user turns it on (or from a camera default).
- Noise Reduction works on the rendered image, not on the RAW data as
  Capture One does; it acts first among the tone tools.
- Neural X-Trans demosaic: candidates found (vkdt `jddcnn`, DemosaicNet's
  X-Trans model); weights licences and input contracts to confirm first.
- Below 100 % each zoom step re-renders the layers to get the photo at
  the new size; keep the last full-size render in the worker and only
  shrink it again when only the size changed.
- Area averaging is a little soft next to Capture One's screen rendering;
  try a sharper filter (Lanczos, or a light sharpening after shrinking).
- Viewer tiles: with linear filtering each tile is smoothed on its own, so
  a faint line could show at tile borders when a very large photo is zoomed
  out; overlap tiles by a pixel if it is ever seen.
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
  persist it in the settings folder once the app has
  settings, so the GPU search is not repeated at each start.
- `denoise-ai` fingerprints the whole input image (about 0.3 s at 24 MP) to
  reuse the network output; the app can key the cache on the upstream
  recipe instead.
- On 5 November `gpu_matches_cpu` hung once for 49 minutes, spinning a
  core, while other builds ran; alone it passes in 14 s and the hang did not
  come back. The GPU read-back now gives up after `GPU_WAIT` (60 s), but the
  cause is unknown. The CI jobs stop after an hour; `check.ps1` still puts
  no limit on how long the tests may run: give it one.
- The GPU renderer reads back before a host tool and uploads after it; fine
  for the denoiser (seconds of network time), revisit if host tools multiply.
- With the AI denoise tool on, the editor's preview worker is busy for the
  whole network run (seconds on a GPU, minutes on a CPU) with no progress
  and no cancel. Add a job control (progress, cancel) and progressive tiles
  (starting from the visible area) into `editor::worker` when the tool
  becomes a common one.
- Masks are evaluated on the CPU at full resolution for every render and
  uploaded to the GPU; move refinement and feather to WGSL and cache per layer
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
  the models window says by pointing at the authors' page. Every file has a
  pinned SHA-256: for the NuGet packages, the DLL taken out of them (nuget.org
  may sign a package again); SigLIP 2 at a fixed revision of its repository.
  Moving to a newer ONNX Runtime or SigLIP 2 means new checksums
  (`every_file_is_checked_against_a_checksum` fails without them).
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
- AI tagging loads SigLIP 2 at each "AI-tag this folder"; keep the encoders
  loaded while the window is open when tagging becomes frequent.
- Tool bar tools (crop, perspective, level, masks, AI select, heal, clone)
  are placeholders.
- Retouch: spots are healed on the CPU (host tool), one after another; a
  GPU kernel would avoid the read-back when the GPU renders. A very large
  spot copies two patches of its size (about 220 MB at the largest radius
  on a 24 MP image).
- Retouch: the undo label of a spot is "Change spot 4"; "Add heal spot"
  or "Move spot" would need a label from the tool in `Edit::SetParams`.
- Retouch: the automatic source is searched in the developed preview, not
  in the image under the layer (earlier spots are not seen), and only the
  selected layer's spots are drawn.
- The catalog keys photos by the path as written: on Windows the same file
  reached as `D:\Foto` and `d:\foto` gets two entries. Normalise the key
  (canonical path, case folded on Windows) before the first public release,
  with an upgrade of existing catalogs (`SCHEMA_VERSION`).
- `Catalog::metas` runs two queries per photo on the window's thread when a
  folder is shown; fine for hundreds of photos. Read the whole folder in one
  query (or in the worker) once folders of thousands of photos are common.
- The window's minimum size is 1100 × 640; the design guidelines ask for a
  layout that works at 1024 × 600 (about 960 × 520 usable). Check the
  Explorer, viewer, filmstrip and tool panels at that size and lower the minimum.
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
- The white-balance tool re-balances with one Bradford matrix in the working
  space; re-developing with new multipliers (and the profile's illuminant
  interpolation) would match Capture One more closely for strong changes.
- Levels and Curve read the histogram of the image on screen (after the
  output curve), not of the tool's input.
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
- Colour editor: Capture One's "View selected color range" is missing; the
  viewer has no way to show which pixels a range selects. Add it when the
  viewer can draw a tool's weight as an overlay (as the mask overlay does).
- Presets: a preset applied to photos that are not open is written to the
  catalog without a new preview; the filmstrip shows the camera's until the
  photo is opened (a background render of the edit would fill it).
- Presets: no import/export of Lightroom (XMP `crs:`) or Capture One
  (`.costyle`) presets yet; their tools differ, only a subset would map.
- Base curve: Auto is Film Standard for every camera. Capture One keeps a
  default curve (and profile) per camera model (Base Characteristics ▸
  Save as Defaults / Apply Defaults); add that once there are per-camera
  settings. The CLI still renders with Film Standard only.
- The curves' shapes are ours (the published names, not Capture One's
  numbers, which are not documented); compare side by side with Capture
  One on the same RAW files and adjust `render-cpu::tone::Shape`.
- Straighten: Capture One also has Rotate Snap, Auto Rotate and Flip;
  not done yet.
- Mask brush (Capture One): Opacity is the stroke's strength; Capture
  One's Flow (amount per pass, building up), Airbrush and pen pressure need
  per-dab accumulation in `masks::brush`; not done. Fill Mask is in the
  options bar; Rasterize Mask and the grey-scale mask view (Alt+M) are missing.
- Zoom (Capture One): the Zoom cursor (Z, click to zoom in, drag a box),
  the Pan cursor (H) with right-click Navigator, the Loupe (P) and Shift +
  wheel zooming every photo shown are not done; Space held pans and the
  wheel steps through the zoom levels.
- Custom lenses: a photo never edited opens developed with the default
  lens settings, then again with its custom lens (two developments); give
  `Worker::open` the starting settings. Applying a custom lens to several
  selected photos from the filmstrip, Capture One's Sharpness Falloff and
  Purple Fringing, and a manual chromatic aberration are not offered.
- Camera defaults (Save as Defaults): kept per camera model for the tools of
  the Background only; Capture One also keeps base curve and ICC profile
  per camera, and an "Apply Defaults" to existing photos is not offered.
- Gradient masks: their points are kept inside the photo (`NormPoint`
  clamps), so a linear gradient cannot start beyond an edge; a radial
  gradient cannot be rotated; only the last gradient of a mask shows
  handles, and a gradient cannot be inverted on its own (use Alt to
  take it away, or the mask's Invert).
- Catalogs and import: the Import Images window counts the RAW files of
  the source on the interface thread; a large tree should be read by the
  worker (the big window with thumbnails comes next and will). Photos imported
  before catalog version 4 have no capture time, so they are never taken
  as duplicates. A file without a capture date is filed under its
  modification day in UTC. Photographer and copyright given at import are
  kept (table `texts`) but not shown until the Metadata panel. Photos
  dropped on the window and "Import this folder" import in place with the
  default options. Missing: removing photos from the catalog with a menu
  (`Catalog::remove` exists), locating missing files, catalog backup,
  Import Catalog, export as a catalog, user collections (albums, smart
  albums, projects).
- JPEG and TIFF: read as sRGB; an embedded colour profile (Adobe RGB,
  ProPhoto, Display P3) is not applied yet (a note says so): use
  `moxcms`, already in the lock file. CMYK and float TIFFs, GPS and the
  rating of XMP are not read. A picture gets no lens correction from the
  lens database or custom lenses, but the camera defaults of its camera
  model (tuned for RAW files) still apply to it. The thumbnail decodes
  the whole picture (no embedded preview used).
- Library collections: a catalog folder shows its own photos, not those of
  its sub-folders; the Library undoes only its last change (one step, move
  to Trash or album merge; adding to or removing from an album, renaming
  and deleting albums have no undo); smart albums, projects and groups were
  left out by choice.
- Disk folders: Cut/Paste moves on the same drive only (another drive needs
  a copy and a delete); Delete uses PowerShell on Windows (`platform::
  move_to_trash`), slow to start. The import window keeps no thumbnails
  between openings; folders can be deleted only from the Disk tree, not
  from the catalog's Folders.
- Layers: every tool acts at most once per layer, in a fixed order; two
  exposures with different masks need two layers. A layer mixes in the
  result of all its tools once (Capture One's way): a style brush of several
  tools painted at half opacity gives a slightly different result from the
  node graph, which masked each tool on its own.
- Layers: the colour pickers and the probe of a layer's tool see the
  Background's tools but not the layers below it.
- Layer masks are evaluated again at every render (CPU, full size); cache
  them per layer and mask definition when photos with many layers are
  common.
- Tool panels float in windows inside the main window (egui windows), not
  in windows of their own on the desktop (egui viewports): a second monitor
  cannot hold them yet. Reordering panels by dragging their header has no
  test that drags (the layout logic has); add a kittest drag, now that the
  headers are accessible widgets.
- A floating panel opens at the pointer; its size and place are remembered
  by egui (the window's memory), not in the workspaces.
- Edits saved before the layers (format 1, the node graph) are not shown
  when the photo opens and are written over at its first change; there is
  no conversion.
- The tools' sliders, number fields and combo boxes (`inspector`) are named
  only by the label beside them, so the test that draws every panel runs
  without the accessibility check; give them `labelled_by` (or a name in
  their `WidgetInfo`) and turn the check on. The panels' own controls
  already pass it.
- Security (see `docs/threat-model.md`, accepted risks R1–R6):
  - Before the first release: `release.yml` still packs only the sources.
    It must build the Windows programs (`--locked`, local paths removed),
    with `SHA256SUMS` (signed), an SBOM (`cargo cyclonedx`) and Authenticode
    with a timestamp; the provenance attestation is already there.
  - `SECURITY.md` sends reports to GitHub's private vulnerability reporting;
    it must be switched on in the repository's settings
    (Security → Private vulnerability reporting).
  - The `Fuzz` workflow has not run yet: run it once, then turn what it
    finds into tests of the crate.
  - `async-executor` (Linux only, egui's accessibility) is held at 1.13.3,
    because 1.14.0 was yanked and 1.14.1 is from 2 November. Take 1.14.1
    from 7 November on (`cargo update -p async-executor`), after looking at
    its diff.
  - Stored edits accept `"nan"` and `"inf"` as numbers (the encoder writes
    them so a value survives a round trip). A hostile file can put NaN into
    a parameter, which may give wrong pixels (not checked yet). Refuse
    non-finite values where a parameter cannot have them.
