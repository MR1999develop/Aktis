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
- `develop`: white balance on the mosaic, Bayer demosaic (RCD since
  29 October, bilinear before) with mirrored borders, camera matrix; linear RGB files (Linear DNG, Apple
  ProRAW) skip demosaicing; X-Trans demosaic (see "RAW formats" below).
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
- `cli`: `aktis-cli develop | info | gpu`, GPU with CPU fallback.
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
  (Ctrl+D, next to the RAW or to a chosen folder, never overwriting) with a
  "Show in folder" toast; status bar with camera, size, zoom, GPU; theme
  System/Light/Dark (dark by default); settings and recent files remembered.
- `tools/denoise-ai`: AI denoise node, SCUNet on ONNX Runtime (tiles with a 64 px mirrored margin, TensorRT for RTX /
  DirectML / CPU with fallback and tile-size search, grain and colour amounts
  applied to a cached network output). Linear light is sRGB-encoded in the
  working primaries for the network; noise below black is denoised, the
  correction fades out above encoded 1.0.
- `tool-api`: host tools (`GpuKernel::Host`, fallible `Tool::apply`): the GPU
  renderer splits the chain around them; both renderers pass a failing node
  through and report it (`tool_errors`).
- `cli`: `aktis-cli develop --denoise <grain %> [--denoise-color <colour %>]`;
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
  `SetLensCorrection` with undo. `aktis-cli info | lenses | profiles`, `develop
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
- `cli`: `aktis-cli mask`, `aktis-cli models`, `aktis-cli develop … --subject`; `--add`,
  `--subtract`, `--intersect`, `--parts`, `--brush`, `--erase`; PPM input.
- Checked by eye on test photos (dog and bicycle, truck, people at a bus,
  portraits): wheels empty between the spokes, no halos, parts of each
  person separate.

## Window layout, Explorer, filmstrip, editable history: written, to be compiled and checked on Windows

Layout agreed in the "Interface" chat (all text in English, dark warm greys,
copper accent `#E2894F`):

- Top: menu, then a tool bar (Crop C, Perspective K, Level L | Masks M,
  AI Select A | Heal H, Clone S: shown disabled, "coming soon"), Undo, Redo
  and an accent Export button.
- Left: Explorer and filmstrip side by side; below them the History and
  Metadata tabs (resizable).
- Centre: the photo above the node graph (grid background, columns
  following the links, masks under the node that uses them; click a node to
  edit it, click the background to deselect).
- Right: histogram (RGB and luminance, clipped black/white shares) above the
  settings of the selected node only; nothing selected → "No node
  selected / Select a node in the graph to edit its settings here."
- Bottom: status line (folder and photo count, camera, size, zoom, GPU).

Pieces:

- `catalog` (new crate): ratings 0–5, colour labels (Red, Yellow, Green,
  Blue, Purple; no heart), keywords from the AI (with confidence) or the
  user, in SQLite (`%LOCALAPPDATA%\aktis\catalog.sqlite`, WAL,
  layout version checked). Filters: rating is a
  multi-select of exact values (All, 0★ … 5★, not "at least"), label
  multi-select with "None", name text, keywords; the model is pure and
  tested, the SQLite store behind the default `sqlite` feature.
- `commands`: history editing. `EditSession::rewrite_history(order)` keeps
  the steps at `order` and replays them from the base recipe, remapping the
  ids of nodes created by replayed steps; a step that no longer applies
  stays in the list with its reason (`HistoryItem::skipped`), never dropped
  silently; the rewrite is one undo step. `history()` lists label, value
  ("+0.50") and kind of each step.
- `raw-io`: `read_thumbnail` (embedded preview, upright, ≤ 320 px for the
  filmstrip; no sensor decoding) and `PhotoInfo` (shutter, aperture, ISO,
  focal length, bias, date, GPS, lens, artist, copyright) for the Metadata
  tab.
- `editor`: folder tree (`FolderTree`, listed on demand), `list_folder`
  (RAW files and visible sub-folders in natural order), `Filmstrip` with
  filters and next/previous, thumbnail cache (`ThumbCache`, least recently
  used out, the worker asked only when the wanted list changes), graph
  layout, histogram, history gestures (`history_order::moved/without`),
  rename validation (`rename_target`). The worker lists folders and makes
  thumbnails on 3 threads (newest request replaces the queue).
- `platform`: Explorer roots (Pictures, then the drives on Windows; home,
  `/media`, `/mnt`, `/` on Ubuntu; `/Volumes` on macOS), "open with the
  default app".
- `app`: Explorer (folder tree, Refresh, rating chips, label swatches, name
  filter, "Showing N of M", AI tag chips with counts, "● ON-DEVICE", "AI-tag
  this folder (N)" with progress and Stop); filmstrip (172 px column, 3:2
  thumbnails, "★ N" badge, "AI · keyword" badge, colour square, name,
  clickable stars where the active star clears the rating, context menu:
  Rename F2, Batch Rename…, New/Clone Variant, Export ▶, Edit With ▶,
  Open With ▶, Show in Explorer, Rating ▶, Color Tag ▶, Remove Variant —
  the ones not built yet are disabled with "Coming soon"); keys when no
  text field has focus: arrows previous/next, 0–5 rating, 6–9 labels, F2
  rename; History tab (Original row, drag the grip to move a step, tick
  steps and "Delete N", skipped steps greyed and struck through with the
  reason); Metadata tab (camera data, rating, label, keywords with add and
  remove, Rename…); AI tagging in the background with SigLIP 2
  (`data/keywords/default.txt`, or `keywords.txt` in the data folder in
  Lightroom format), keywords stored in the catalog; File ▸ Show folder…;
  the last folder is opened again at start-up.

Not compiled here (no crates.io in the development container): the egui code
of `app` and the SQLite store of `catalog`. The pure parts (catalog model,
history rewrite, library, thumbnails cache, graph layout, rename) are
tested offline.

## Node graph editing, filmstrip selection, faster viewer: written, to be compiled and checked on Windows

- Node graph: a newly opened photo has only the Source (the Output is not
  drawn: the end of the chain feeds it, its port has an accent ring).
  Clicking a node's output port opens the list of tools by category; the
  chosen tool becomes a node right after it (`Document::add_node`, undoable,
  "Add serial node · exposure" in the History). Nodes are dragged anywhere,
  the background (or the middle button) pans; right-click a node to add
  after it, bypass or delete it. Positions are kept per photo while it is
  open (`editor::graph_view`: new nodes go right of their input and push the
  following ones aside unless the user moved them).
- Filmstrip selection as in a file manager: click selects and opens one
  photo; Ctrl+click adds (and opens) or removes; Shift+click selects the
  range from the last clicked photo and opens the clicked one; Ctrl+Shift
  adds the range. Stars and the Rating/Color Tag menus on a card of the
  selection act on all of it; 0–5 and 6–9 act on the selection; arrows go
  to the next photo and select it alone; Ctrl+A selects every visible
  photo, Escape goes back to the open one. With several photos selected the
  viewer shows their tiles ("Selected: N of M files | Active: …", tiles 2.5
  times a thumbnail, "+N more"); a click on a tile opens that photo alone.
- Viewer speed:
  - 8-bit sRGB conversion through a table checked to give exactly the
    previous codes, on every core, turned upright in the same pass (no
    copies): about 4× faster on 2 cores for a 2560 px preview, more on the
    user's machine;
  - the GPU renderer no longer copies the source image at every render;
  - while a slider is dragged a 1280 px draft is rendered (a quarter of the
    preview's pixels), the preview when the drag ends;
  - the full-size image is rendered only when the zoom shows more than the
    preview has (it used to be rendered after every change, seconds for a
    24 MP photo, delaying the next previews);
  - the status bar's GPU label shows the time of the last render and of its
    display conversion.

## RAW formats: X-Trans, DNG gain maps: written, to be compiled and checked on Windows

- `develop`: X-Trans demosaic on the CPU (`develop::xtrans`): white balance
  on the mosaic, green everywhere from the nearest greens along four
  directions weighted against the green difference along each (edges are
  followed, not crossed), then red and blue as green plus the
  distance-weighted mean of colour minus green in the 3×3 (else 5×5).
  Mirrored borders read the channel of the photosite actually read. About
  1.2 s for 26 MP on two cores. Tested: flat colour exact, ramps smooth,
  same pixels at any 6×6 crop phase, tiny images.
- `raw-io`: rawler 0.8.0 does not apply DNG opcodes, so a small TIFF reader
  finds the raw IFD's opcode lists and applies GainMap (OpcodeList2) to the
  normalised mosaic, in active-area coordinates offset by the default crop
  (Pixel and other phone DNGs no longer have dark corners). Other opcodes
  are skipped; required ones are reported in `RawFile::notes`, shown in
  the inspector's development notes and by `aktis-cli info`.
- Open dialog: ARI, DCS, FFF, ORI, QTK and RAW added; README lists the
  supported cameras and rawler 0.8.0's known limits per brand.

## Tasks of 28 October 2026

- AI models are downloaded from their authors: SCUNet as the KAIR
  checkpoint, converted on the user's computer by `scunet-onnx` (pure Rust,
  byte-identical to the Python reference); each pack names its home page.
- Output render: a film curve on stops, hue kept (max-RGB norm), highlights
  rolled to white and gamut mapped by desaturation, so exposure moves every
  tone by the same amount, as in Capture One, and no longer clips colours.
- Top bar: Undo/Redo on the left, tools centred (Crop C, Perspective K,
  Level L, Masks M, AI Select A, Heal H, Clone S), Export on the right.
- Node graph zooms with the wheel (and pinch) around the pointer.
- History/Metadata panel keeps its size; interface greys are neutral.
- Adjustment nodes (`tools/adjust`, CPU reference + WGSL, every kernel run on
  WebGPU/SwiftShader against the CPU): White balance (mode, picker, kelvin,
  tint; the Source develops as shot), Exposure v2 (contrast, brightness,
  saturation), High dynamic range, Levels, Curve; prepared: Colour balance,
  Colour editor, Black & white, Clarity and Dehaze (CPU), Vignetting.
  Panels like Capture One's for white balance, levels and curve, with
  pickers in the viewer and the pointer's colour marked on them.
- Several selected photos fill the viewer (grid shape for the largest
  tiles, 1600 px previews, up to 24).
- Scopes: histogram, waveform and parade.
- Viewer: right button eyedropper, middle button 4× loupe.
- Lens corrections: every lens, fitting ones first then others by maker,
  type to filter, tick for fitting lenses only.
- Geometry in the recipe (`core_model::Geometry`, applied by
  `engine::geometry` after the graph, for screen and export): Crop
  (aspect presets, handles, Enter/Esc), Level (drag a line
  or slider), Perspective (keystone sliders).
- Masks (brush) and AI Select (SAM clicks, subject) build the selected
  node's mask; the models run in the background; AI masks now render in
  the window from the shared raster cache.
- Edit ▸ Preferences…: theme, viewer background (interface, white as in
  Capture One, greys, black), presets folder (the export folder is chosen
  in the Export window).
- Filmstrip on its own resizable left panel: thumbnails grow with it up to
  240 px, then more columns.
- Panels like Capture One's for the colour and detail nodes
  (`app::color_ui`): Colour editor (Basic chips; Advanced and Skin Tone
  ranges as wedges on a hue wheel, dragged or picked on the photo; skin
  uniformity), Colour balance (Master, 3-Way and per-band wheels with a
  draggable puck and lightness/saturation arcs), Black & white (colour
  sensitivity, split tones), Clarity, Dehaze (shadow tone Auto or picked)
  and Vignetting. Wheel geometry in `editor::color_wheel`; new pickers
  `PickKind::ColorRange` and `PickKind::ShadowTone`.
- Presets (Capture One's styles): Image ▸ Presets ▸ Save preset… chooses
  what goes in (white balance, profile, lens corrections, geometry, each
  adjustment node; masks never); Apply (also on the filmstrip's card menu)
  sets the open photo or every selected photo, one undo step
  (`Edit::ApplyStyle`: each tool sets the photo's unmasked node of that
  tool, or is added before the output). Photos not open are changed in
  the catalog. No universal preset format exists (Lightroom: XMP `crs:`,
  Capture One: `.costyle`): ours are versioned JSON files (`.aktispreset`,
  `.aktisbrush`, `recipe_store::preset`) in a folder chosen in Preferences
  (default `Presets` in the data folder), sub-folders being groups.
- Style brushes (Capture One's): built-in Light & Contrast, Color and
  Enhancements brushes, plus saved ones (Image ▸ Style brushes ▸ Save
  style brush… from the selected node). Choosing one (Image menu or the
  Masks tool's bar) arms the brush with its size/hardness/flow; the first
  stroke adds its nodes, named after the brush, behind one new mask
  (`Edit::AddStyleBrush`) and selects them, so further strokes extend it.
## Heal and Clone (spot retouching): written, to be compiled and checked on Windows

- `tool-adjust::retouch`: a graph node holding up to 16 spots as plain
  parameters (eight each: heal/clone, centre, radius, source centre,
  feather, opacity; radius 0 = empty slot). Positions in the developed
  image (0–1), radius as a fraction of the long side, so previews and the
  full image match. Runs on the CPU (`GpuKernel::Host`). Clone copies the
  source with a feathered edge; Heal adds to the source the harmonic
  interpolation (Poisson integral of the disc, on a coarse grid) of the
  difference between the spot's border and the source's border, measured
  just outside the circle: tested to leave no seam on a smooth gradient
  and to remove a blemish while keeping the source's texture.
  `find_source` picks a circle 2.2–4.6 radii away whose surroundings match
  the spot's and whose inside is as even as they are.
- `editor::retouch`: a new spot goes into the selected retouch node when it
  has room, into a new one right after it when it is full, otherwise into
  a new retouch node right after the Source (then selected); one undo step
  per spot; hit-testing of the circles on screen.
- `app::retouch_ui`: Heal (H) and Clone (S) in the tool bar. Click to put a
  spot (source found automatically, drawn as a second circle with a line),
  click to select, drag either circle (one undo step per drag), Size /
  Feather / Opacity in the options bar (also `[` and `]` for the size),
  Heal/Clone switch, Delete or Backspace or the Delete button. Double-click
  does not zoom while these tools are on. The inspector lists a retouch
  node's spots with a Remove button each.
## Edits saved in the catalog, variants: written, to be compiled and checked on Windows

- Non-destructive and automatic, as in Capture One: the open photo's edit is
  saved in the catalog (no sidecar files; the RAW is never written) when
  another photo or variant is opened, when the window closes (close request,
  Quit, and eframe's `save`), and 30 s after a change.
- What is saved, per variant: the recipe, the history (base recipe, steps
  with their skipped reason, redo list), the node positions with the graph's
  pan and zoom, the variant's name, and a 1024 px JPEG of the edited image,
  shown in the filmstrip and in the tiles of a multiple selection instead
  of the camera's embedded preview.
- `recipe-store` (new crate): hand-written JSON with `"format": 1`; every
  edit, node, mask and develop setting round-trips exactly (floats
  included). The history is restored by replaying the steps through
  `EditSession::restore` (same validation as new edits); if it does not lead
  to the saved recipe (a tool missing in this build) the recipe is kept and
  the history dropped, with a notice.
- An edit saved by a newer editor (higher format) opens read-only: its
  recipe is shown when this version can read it, otherwise the photo is
  shown unedited; every change is refused (`EditError::ReadOnly`), nothing
  is saved over it, a banner above the settings and the status bar say why.
  A damaged edit is kept read-only the same way.
- Variants: Image ▸ Variants ▸ New variant (F7, from the original), Clone
  variant (F8, with history and node positions), Delete variant… (Ctrl+Del,
  with confirmation; a photo keeps at least one), and the list to switch;
  the same commands in the open photo's card menu; the status bar shows
  "Variant 2 · 2/3" with the list. The variant shown last is remembered and
  its preview is the photo's thumbnail.
- Catalog layout 2 (`SCHEMA_VERSION` 2): table `variants (path, number,
  name, format, data, preview, updated)` and `photos.variant`; layout-1
  catalogs are upgraded on open in one transaction. The SQL was checked with
  Python's sqlite3; the Rust store and the window were not compiled here.

## Export window: written, to be compiled and checked on Windows

- The Export… button, File ▸ Export… (Ctrl+D) and the filmstrip's Export ▶
  menu open the export window for the photos selected in the filmstrip
  ("Export N selected photos") or the open photo. File ▸ Export with
  previous settings (Ctrl+Shift+D, also in the card menu) skips the window.
  The last choices are remembered (`Settings::export`, `export_dir`).
- Format: JPEG (quality), PNG or TIFF at 8 or 16 bits, always sRGB with the
  profile embedded. Size: full, long edge / width / height in pixels, or a
  percentage, never enlarged unless asked; resampled in linear light (area
  average when shrinking). Name: a pattern with `{name}`, `{counter}`
  (start, digits), `{date}`, `{camera}` and free text, with a live example;
  unsafe characters and reserved names are replaced. Folder: next to the
  original, a sub-folder, or a chosen folder; existing files are never
  replaced (`photo (2).jpg`). Watermark: text with a built-in stroke font
  (ASCII, ©, accented letters), size and margin in % of the long edge,
  opacity, 9 positions, a soft shadow. Metadata: all, all but GPS,
  copyright only, none; a copyright field replaces the camera's notice.
- `export`: hand-built EXIF (TIFF structure: camera, lens, exposure, dates,
  GPS, Windows rating and keywords, artist, copyright, Orientation = 1) in
  JPEG APP1, PNG eXIf and TIFF IFD0; XMP packet (rating, keywords, rights,
  creator); PNG with a built-in zlib compressor; TIFF uncompressed. Files
  checked with Pillow and ImageMagick.
- `engine::finish_for_export`: the one step after the node graph (the
  recipe's straighten/keystone/crop on the upright picture, resize, 16-bit sRGB output turned upright,
  watermark). `render-cpu::to_srgb16` for 16-bit files.
- `editor::export_job`: photos exported one after another on a thread; the
  open photo reuses its developed image, the others are read and developed
  with their recipe (`EditorApp::recipe_for`: the open photo's recipe,
  otherwise the variant the catalog shows for it), masks included. Status bar: "Exporting
  3 of 10: name…", progress bar, Cancel; at the end a toast ("Exported 10
  photos", failures named) with "Show in folder".

## Tests and CI

GitHub Actions (`.github/workflows/ci.yml`), started by hand on a work branch
that passed the local checks:

- Windows and Ubuntu: fmt, clippy with warnings as errors, all tests.
  On Ubuntu the GPU tests run on Mesa's software Vulkan driver (llvmpipe)
  and must pass (`AKTIS_REQUIRE_GPU=1`); elsewhere they skip without a GPU.
- macOS: `cargo check` only.
- `real-raw`: downloads a public-domain Nikon D3S NEF (NASA, from the rawpy
  test set), runs the ignored real-file test, `aktis-cli info` and `aktis-cli develop`
  on the GPU and on the CPU, then opens the file in the window headless
  (egui_kittest on wgpu) and saves a screenshot (`window.png`).

Each job keeps its log and output files as an artifact of the run (three
days), stops after an hour at most and makes no commits: every build uses
`--locked`, so a `Cargo.lock` that misses a dependency fails the run.

- `ai-denoise`: generates the SCUNet model from the KAIR weights, runs the
  network test on the CPU, then `aktis-cli develop --denoise 80` on the Nikon D3S
  (JPEGs with and without the tool are kept).

First green run: Nikon D3S 4256×2832 develops in about 2 s on llvmpipe;
the GPU and CPU renders look the same.

## 28 October: shortcuts as in Capture One

- Ctrl+E turns the exposure warning on and off (View ▸ Exposure warning,
  the toolbar tick); Export… moved to Ctrl+D (Capture One's Process) and
  Export with previous settings to Ctrl+Shift+D.
- Straighten is R.

## 29 October: panels, masks, catalogs, import, Library

- Right panel no longer widens past its edge; double-click resets every
  slider; Dehaze gentler and on luminance only.
- Masks: linear (L) and radial (T) gradients as in Capture One, with
  handles for the last one, Alt takes away.
- Film Standard brighter and livelier; the crop box shows its size in
  pixels; the viewer background is the interface's colour or white.
- Catalogs (`*.aktiscatalog` folders): New, Open, recent catalogs; the old
  single catalog becomes "Default"; photos come in only by import.
- Import engine: background import with Stop; copy into dated sub-folders
  or add in place; backup folder; duplicates by name and capture time;
  preset, rating, keywords, photographer and copyright at import.
- JPEG and TIFF imported and edited (linear sRGB, Exif read), new Linear
  base curve (Auto for pictures); RAW+JPEG pairs whole or RAW only.
- Library: Catalog Collections (All Images, Recent Imports, Recent Edits,
  Trash), albums with drag and drop, the catalog's folders as a tree;
  Delete to the Trash, Restore, Empty Trash.
- Lens corrections: choosing a database lens after a custom lens no longer
  keeps showing the custom lens (its values are left with it).

Details in `docs/catalog.md`.

## 29 October, afternoon: import window, Library, Disk

- Import Images window as in Capture One: source on the left (sub-folders,
  Exclude duplicates, name filter), photos as a grid or a large preview with
  a strip, a tick per photo, options on the right; stars for the rating.
- Library: search by name, counts on the right of every row, sub-albums,
  Merge Into with Undo, the next photo opens after Move to Trash.
- Disk: right-click Import…, Cut/Paste, Rename, Delete (Recycle Bin) with
  warnings, the catalog following moved folders; "Import this folder" gone.
- Viewer: above 100 % the pixels show as they are (no smoothing).

## 29 October, evening: block D begins (demosaic, viewer, orientation)

- Bayer demosaic with RCD (ratio corrected demosaicing, the default of
  RawTherapee and darktable) in tiles on all cores, followed by a 3×3
  median of red − green and blue − green against false colours; X-Trans
  gets the same median after its demosaic. Tests compare RCD with the old
  bilinear demosaic on a slanted edge and on a zone plate.
- Viewer: photos larger than one GPU texture (GFX 50 and 100) are drawn
  in tiles at full size instead of being halved, so 100 % shows every
  pixel; thumbnails and previews use mipmaps (smooth when shown smaller).
- Orientation: when the decoder does not know how a RAW file is turned,
  the Exif Orientation tag decides, as for the filmstrip's previews
  (vertical photos showed horizontal in the viewer only).
- Viewer quality below 100 %: the photo is rendered exactly as large as it
  is shown, shrunk on the CPU by area averaging (`render_cpu::resize`, any
  size, not only whole fractions); a zoom or a window resize asks the same
  image again at the new size. The preview and draft copies are now exactly
  1920 and 1280 pixels (a GFX file gave 1651 before).
- X-Trans demosaic with Markesteijn's three-pass algorithm (dcraw's
  `xtrans_interpolate` in floating point, in tiles on all cores, borders
  mirrored keeping the 6×6 pattern), followed by the chroma median. On a
  zone plate its mean error is about a tenth of the simple demosaic's.
- Base curve Auto for RAW files is now "Camera Match": the film curve
  fitted to the JPEG the camera embedded in the file (as RawTherapee's
  Auto-Matched Curve): the percentiles of our development's luminance are
  paired with the picture's, and the film curve's grey, slope, shoulder and
  toe are fitted to them (`render_cpu::tone::fit_camera_curve`,
  `editor::camera_look`), then made 8 % gentler. Film Standard when the file
  has no picture or no film curve fits. Export uses the same curve.
- Base characteristics ▸ Demosaic: Auto (RCD for Bayer, Markesteijn for
  X-Trans), RCD, Bilinear, Markesteijn 3-pass, X-Trans fast; saved with the
  recipe (`DevelopSettings::demosaic`, optional field, older recipes read as
  Auto), undoable, the photo is developed again.
- Software renderers are no longer taken for a GPU (Microsoft Basic Render
  Driver, llvmpipe: what a virtual machine offers): they read back wrong
  pixels or fail, so the editor uses the CPU engine; `AKTIS_SOFTWARE_GPU=1`
  accepts them for debugging. `scripts\setup-windows.bat` installs Git, the
  C++ build tools, rustup and the pinned toolchain on a new PC.

## 29 October, night: block D, purple fringing

- Lens corrections ▸ Purple fringing (0–100, saved in the recipe and in
  custom lenses as `purple_fringing`, older files read as 0): near a strong
  luminance edge (in stops), pixels whose hue lies between magenta and
  blue-violet and that are clearly coloured take the chromaticity of the
  nearby pixels that are not fringe (the white of a highlight, the green of
  leaves), keeping their luminance; with nothing around, they turn
  neutral. Sky blues and reds are left alone. Edges, colours around and
  weights are computed on a grid of cells 2–12 pixels wide (5 for 26 MP),
  so it costs a few passes over the pixels and little memory
  (`develop::fringe`).
- Fujifilm photos never edited start with purple fringing at 50
  (`editor::camera_defaults::start_purple_fringing`).

## 29 October, night: block D, Sharpening and Noise Reduction

- Images know the size of their pixels compared with the full-size photo
  (`ImageF32::pixel_size`, 1 by default): `engine::downscale` multiplies
  it, the GPU engine and orientation keep it. Tools working at the scale of
  the pixel use it, so a reduced preview does not exaggerate them.
- Sharpening node (Details): Amount 0–100 (45 to start, as Capture One's
  180 on its 0–1000 scale), Radius 0.2–2.5 px, Threshold
  0–10, Halo suppression 0–100, as Capture One; unsharp mask on gamma-2.2
  luminance, threshold with a soft step, halo suppression limits overshoot
  above and below the neighbours' range; new unit "px" (`Unit::Pixels`).
- Noise Reduction node (Details): Luminance, Details, Color, Single pixel.
  Square-root encoding (photon noise about even across tones), luminance
  and two colour differences, noise measured per plane from the finest
  diagonal differences (MAD), "à trous" B3 wavelet (4 levels for
  luminance, 5 for colour) with the non-negative garrote; Details below 50
  smooths more, above 50 gives back fine detail; hot pixels (far above all
  eight neighbours) take the neighbours' median, on the full-size photo
  only.
- `tools/adjust/src/filters.rs`: exact small Gaussian, à trous wavelet
  streamed one level at a time with reused buffers (allocating fresh
  buffers per level made it 6 times slower), noise estimate.

## 30 October: phase 2 (Library) closed

- Recent Imports shows each import's local date and time ("29 Oct 2026,
  14:05"): the catalog asks SQLite for the computer's time-zone offset at
  the time of the import (`ImportBatch::offset`, `when_text`).
- Albums list their photos in the order they were added
  (`album_photos.added`, then insertion order).
- Library undo: the last move to the Trash or album merge
  (`library_collections::LibraryUndo`) is undone with Ctrl+Z while the
  pointer is over the Library, Edit ▸ Undo … or the message's Undo button;
  elsewhere Ctrl+Z stays the photo's undo.
- Photos are dragged onto albums from the overview tiles (several photos
  selected) too.
- Import Images window: sized to the app window (92 % × 86 %, never more
  than the window minus a margin; `import_window::fitted`); the source
  column scrolls, so on small screens (a 1280 × 960 virtual machine) the
  buttons stay visible. The three columns sit in a row exactly as high as
  the space left for them: the vertical separators between them took the
  whole screen height inside the modal and pushed title and buttons off
  the screen.
- Import… button on the bar under the menu, left of Undo (Ctrl+Shift+I as
  before).
- File ▸ Save Catalog As… (Ctrl+Shift+S): the open catalog (the Default
  one, say) copied whole to a new named catalog, which is then open
  (`Catalog::save_copy` with `VACUUM INTO`, `Library::save_catalog_copy`);
  the open photo is saved first, the old catalog is left as it was.

## 30 October: demosaic, base curve, colour noise, shooting data

- Demosaic is no longer a choice: RCD for Bayer sensors, LMMSE from ISO
  3200 (`develop::lmmse`, Zhang and Wu's directional estimate in a 2.2
  gamma, then the false-colour median; `LMMSE_FROM_ISO`), Markesteijn
  three-pass for X-Trans with VNG in the flat areas (`develop::vng`, as
  darktable's dual demosaic: a detail mask from the Sobel gradient of the
  square-root luminance decides how much of each). The ISO reaches the
  development through `CaptureInfo::iso`. Recipes and histories saved
  with a chosen method still open (the choice is ignored).
- Base curve: only Standard (the old Auto: the camera's look, or Film
  Standard without a camera picture) and Linear are offered; recipes with
  the other film curves open as Standard. The camera-matched curve keeps
  more contrast (`CAMERA_MATCH_SOFTEN` 0.92 → 0.97) and the film curves
  take saturation from reds (`RED_TRIM`, 12 % for pure red, about half for
  orange and skin, none for yellow, magenta and greys).
- A RAW file shot at ISO 400 or more starts with a Noise Reduction node
  for colour noise only (Luminance 0, Color 50; `start_noise_reduction`),
  unless its camera's defaults bring their own.
- Under the image, after the lens: aperture, shutter speed and ISO.
- Import window: a test draws the real window with the app's theme in
  three screen sizes and checks that every part stays on screen
  (`every_part_of_the_window_is_on_screen`).

## 30 October: variants in the filmstrip

- Each variant of a photo with several is a card of its own, right after
  the one before, as in Capture One: its stored preview (decoded once on
  the interface thread, the photo's thumbnail while there is none), the
  file name with the variant number, a click that opens it (switching
  variant when the photo is open). The open photo's list includes a new
  variant not stored yet. Rating, label, keywords and selection stay the
  photo's, shared by its variants.
- Deleting the first variant keeps the others: a variant's card menu has
  Delete Variant N… (only that one, asked first) and Move Photo to Trash
  (the photo with every variant); Delete over the filmstrip, with the open
  photo alone selected and several variants, deletes the open variant.

## 2 November: the name Aktis, Rust 1.99.1, GPLv3

- The editor is called Aktis: window `aktis`, command line `aktis-cli`,
  folders `aktis`, variables `AKTIS_*`, files `.aktiscatalog`,
  `.aktispreset`, `.aktisbrush`, `.aktisdefaults`, `.aktislens`,
  `.aktismask`. `platform::move_legacy_dirs` renames the old
  `raw-node-editor` folders (and the files' extensions) at the first start
  of either program; the window's remembered paths follow
  (`Settings::follow_moved_folder`).
- Rust 1.99.1. Licence GPL-3.0-only (`LICENSE`), `authors = ["MR"]` in
  every crate. The build scripts remap local paths out of the programs
  (`scripts/build-env.ps1`).
- The AI denoise models are looked for only in the editor's own folders.
- Zoom controls on the right of the status line (`zoom_bar`): Zoom out,
  the percentage (typed or dragged), Levels, Zoom in, Fit, 100 %, Keep
  zoom (`ViewState::keep_inside` fits a kept zoom to the new photo). The
  status line gives way in a narrow window (shooting data, lens, then the
  GPU's name); tests check the controls' order and glyphs.
- Rust 1.99's clippy: `chunks_exact` with a constant size became
  `as_chunks` throughout.

## 3 November: layers and tool panels instead of the node graph

- The node graph is gone. `core_model::Layers`: a Background and layers
  above it, each with a mask (`MaskDef`, its opacity the layer's) and its
  own tools; every tool at most once per layer, in the fixed order the
  tools declare (`ToolDescriptor::order`, `ToolRegistry::pipeline`); the
  retouch tool holds its spots in slots of parameters.
- Rendering (`render-cpu`, `gpu`, `engine`): the Background's tools on the
  developed image, then each layer's tools mixed in once through its mask.
  On the GPU a blend kernel mixes a layer in; a layer holding a host tool
  runs on the CPU. GPU and CPU agree on layers (`gpu_matches_cpu`).
- `commands`: edits address a tool in a layer (`ToolRef`); new edits add,
  delete, rename, move, switch and mask layers; a style brush is a new layer.
  History rewrites follow the layers made by deleted steps.
- `recipe-store` format 2. Format 1 (the node graph) is refused as
  `DecodeError::Obsolete`; such a photo starts again from its first settings,
  and the old edit is written over only once the photo is changed (opening
  it alone writes nothing but a new preview). Presets keep reading their old
  `nodes` key.
- The window: the filmstrip is a row under the photo; on the right, under
  the histogram, a panel per tool and the Layers, Base Characteristics,
  Lens Correction and Spots panels (`tool_panels`). Panels open and close,
  move by dragging their header, float in a window when dropped outside
  the column or from their menu, and dock again when the window closes;
  their arrangement is kept in the settings and in each workspace. Masks and
  AI Select draw the chosen layer's mask (on the Background the first stroke
  adds a layer), Heal and Clone put spots in the chosen layer, Delete deletes
  the chosen layer. The command line's selections make a layer.
- Sliders no longer report a change when egui only rounds a value to their
  step (it put three edits in the history of a photo just opened); a test
  draws every panel and checks that nothing is asked for.
- Review against the rules: the panel headers and the tool and layer
  switches have accessible names (a kittest test reads them), the Delete
  button of a layer sits away from Up and Down, the panels use the theme's
  spacing and radius; stored layers whose next free id could wrap round to
  the Background's are refused; the CPU blend stops on a mask of the wrong
  size instead of mixing only part of the image.

## 4 November: security review

- `docs/threat-model.md`: scope, assumptions, assets, trust boundaries,
  every entry point with its defences and tests, accepted risks. `SECURITY.md`
  says how to report a vulnerability.
- Downloads: `onnxruntime.dll` and `DirectML.dll` (Windows) are checked
  against a pinned SHA-256 before they are put in place. The checksum is of
  the DLL taken out of the NuGet package, which nuget.org may sign again.
  SigLIP 2 is downloaded at a fixed revision with pinned checksums. curl may
  use only HTTPS, redirects included. curl, tar, Explorer and PowerShell are
  started by full path (`platform::system_program`).
- Parsers: overflow checks in the release build of the crates that read
  files made elsewhere. Every parser's tests feed it thousands of damaged
  copies of a valid file (`crates/mutate`); what is accepted must write
  back the same. Fuzz targets for `cargo fuzz` are in `fuzz/`, starting
  from seed files the tests keep current. This found a keyword-list fix:
  a second byte-order mark at the start, or a space after `~`, no longer
  ends up inside a name. Names with bidirectional or invisible marks are
  refused.
- Catalog: SQLite opens in defensive mode with an untrusted schema, for
  catalogs that come from another computer.
- Supply chain: `deny.toml` (RustSec, licences, crates.io only), run by
  `check.bat` and every day by the `Audit` workflow. CI actions are pinned to
  release commits. The jobs that build run with a read-only token and no
  stored credentials, and one last job publishes their results. Downloads in
  CI (ONNX Runtime, the SCUNet weights, SigLIP 2) are checked against the
  editor's checksums. `async-executor` 1.14.0 (yanked) was replaced by
  1.13.3.

## 5 November: clean history, bounded GPU wait

- The history of every branch, published ones included, was rewritten:
  every commit has MR as author and committer, test data uses made-up
  names, and the names of other programs are gone from code, comments and
  commit messages. The repository is now `MR1999develop/Aktis`.
- The GPU renderer no longer waits for ever on a chunk: the read-back gives
  up after `GPU_WAIT` (60 s) with a read-back error.

## 7 November: CI without commits of its own

- The CI no longer commits: the `lockfile` job (which committed `Cargo.lock`
  as another author) and the job that pushed every result to the
  `ci-results/*` branches are gone. Results stay as artifacts of the run,
  every job has a read-only token and stops after an hour.
- `setup-windows.bat` sets this repository's commit author (MR and the
  account's no-reply address) instead of asking for one.

## 30 October: live filmstrip thumbnails

- The open photo's filmstrip card (and its variant's card) follows every
  change: each finished render (not the drafts made while dragging) is
  reduced to a thumbnail on the spot (`Library::show_live_thumbnail`,
  block means); the catalog's preview is still written with the edit.
