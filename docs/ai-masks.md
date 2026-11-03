# AI selection and masks

AI selection makes masks for the subject of a photo, for objects and people
picked with clicks or a rectangle, for every person (or car, dog …) in the
frame, and for parts of each person: hair, face skin, body skin, clothes,
accessories, eyebrows, eye sclera, irises, lips, teeth. AI parts combine with
brush strokes through add, subtract and intersect. A mask belongs to a
layer: the layer's tools act only where the mask is,
`out = in + mask · (tools(in) − in)`.

## In the layers

- A **layer** above the Background holds a `MaskDef`: an ordered list of
  components, then edge refinement, feather, opacity (the layer's) and
  invert for the whole mask.
- Each component is an AI selection or a set of brush strokes, and **adds**,
  **subtracts** or **intersects** with the components above it (fuzzy sets:
  union = max, intersection = min, difference = min(a, 1 − b)). The first
  enabled component always adds. A component can be disabled without losing it.
- **Brushes** (`MaskSource::Brush`): strokes with radius, hardness and
  opacity, sizes as fractions of the long side so they look the same at any
  resolution. A brush component set to subtract is an eraser. With
  `edge_aware` the painted area follows the photo's edges (auto-mask brush).
- `Edit::AddLayer { name, mask, tools }` adds a layer with its mask in one
  undoable step (the Masks and AI Select tools do it at the first stroke or
  click on the Background); `Edit::SetLayerMask` changes a mask — a click, a
  new component, a brush stroke, feather — and a slider drag or a stroke is
  one undo step. A layer switched off does nothing.

## What is stored, and why

An AI component stores its **prompt** (subject; clicks; a rectangle; a
person's box and the part) in the coordinates of the developed image, as
fractions of width and height, plus the id of the model and the **raster
key** of the model's answer. The raster (the model's output, up to 2048
pixels on the long side, 8 bits) lives in a cache folder, named by a 128-bit
FNV-1a hash of image, development settings, model and prompt. Brush strokes
are stored as strokes, never as pixels.

Models are not run again behind the user's back: their output can differ
slightly between GPUs, runtimes and versions, so recomputing on open would
make an edit change by itself. Everything after the model is deterministic
and done at render time, at the resolution being rendered, by crate `masks`
(the same code for the CPU and GPU renderers).

## Edge refinement

Models answer at about one megapixel; their edges are soft and they fill
holes they cannot resolve (the inside of a bicycle wheel). At render time
each AI component is refined against the photo's colours:

1. **Uncertain areas by colour.** Where the model is not sure (0.2–0.97),
   its value is weighed (Bayes' rule) against how much the pixel's colour
   looks like the object's sure part or like the background next to it
   (colour histograms, as in GrabCut). A colour typical of the background
   pulls a half-sure pixel out — this empties wheel rims and the gaps between
   fingers — while an ambiguous colour leaves the model's value alone.
2. **Edges by colour.** A colour guided filter (He, Sun & Tang) snaps the
   edge to the image. A colour guide separates a red frame from a yellow wall
   of the same brightness, which a brightness guide cannot; the first version
   used brightness and left glowing halos.
3. A mild contrast around 0.5 keeps the edge crisp.

The filter's local models are computed at up to 2048 pixels and applied with
the full-resolution image, so the edge is as sharp as the photo.

## Models

| Task | Model | Licence | Size |
| --- | --- | --- | --- |
| Subject | BiRefNet general (`birefnet_general`) | MIT | 973 MB |
| Objects and people from clicks/boxes | Segment Anything 2.1 Large (`sam2.1_large`) | Apache-2.0 | 869 MB |
| (lighter alternative) | Segment Anything 2.1 Small (`sam2.1_small`) | Apache-2.0 | 154 MB |
| Find people and 79 other COCO classes | YOLOX-S (`yolox_s`) | Apache-2.0 | 36 MB |
| Person parts and face features | MediaPipe selfie multiclass, BlazeFace, face landmarker (`mediapipe_parts`) | Apache-2.0 | 11 MB |

Only models whose licence allows commercial use are listed. The catalog
(`crates/ai-segment/src/catalog.rs`) has each file's URL, size and SHA-256;
`aktis-cli models` shows what is installed and where to put the rest, `aktis-cli models
--verify` checks the checksums. The SAM 2.1 and MediaPipe ONNX files come
from third-party releases (official weights, converted); before shipping,
the project should export and host them in its own release.

Segment Anything **Large** replaced Small as the default after comparing them
on the same photos: Small leaves grid-like artefacts and loses parts (a wheel
without its rim); Large gives clean masks. Large is about 20 s per photo on a
4-core CPU for the image encoder (once per photo), well under a second on a
GPU; each further click only runs the decoder (milliseconds).

**Objects** are segmented on the whole frame; when the object is small (under
10 % of the frame) it is segmented again on a crop around it, so it fills the
model's input and fine structure comes out.

**People**: the detector finds each person with a box; each box is a Segment
Anything rectangle prompt, so every person is a separate component ("Person
1", "Person 2" …) that can be refined with clicks or removed.

**Person parts**: for each person the engine runs the multiclass segmenter
on the whole person, finds the face with BlazeFace in the upper part of the
box, runs the segmenter again on the head at a much higher scale (hair and
face need the detail) and places the 478-point face mesh on the levelled
face. Hair, face skin, body skin, clothes and accessories come from the
segmenter (head result inside the head square, blended into the body
result); eyebrows, eye sclera, irises, lips and teeth are drawn from the
mesh outlines. Face skin excludes the features. Every part is multiplied by
the person's own Segment Anything mask, so it never spills onto a
neighbour. Facial features need a visible face; for a person seen from
behind they report "no face is visible".

Models see the photo **upright** (they were trained on upright photos) and as
an ordinary sRGB picture: linear light → sRGB primaries → sRGB curve. Only
an underexposed image (log-average luminance under 3 %, like a RAW file
developed without brightening) is brightened, up to 8×. Prompts go in and
rasters come out in the stored orientation, so they line up with the recipe.

## GPU of any vendor

Inference uses ONNX Runtime through the `ort` crate, **loaded at run time**:
nothing is linked or downloaded at build time, the editor starts without it,
and the library can be the build that fits the machine. Each model goes to the
first execution provider the library offers that accepts it:

| System | Library to install | Providers tried |
| --- | --- | --- |
| Windows, any GPU | `onnxruntime.dll` from the NuGet package `Microsoft.ML.OnnxRuntime.DirectML` (with its `DirectML.dll`) | DirectML |
| Windows/Linux, NVIDIA | `onnxruntime-*-gpu` release (CUDA 12) | CUDA |
| Linux, AMD | ROCm/MIGraphX build | MIGraphX, ROCm |
| Intel | OpenVINO build | OpenVINO |
| macOS | release build | Core ML |
| any | plain release build | CPU |

The library is found through `ORT_DYLIB_PATH`, next to the program, or in
the models folder. ONNX Runtime 1.22 or later is required. A `DirectML.dll`
next to the library is loaded first (Windows' own copy is often too old).
On Windows, `scripts\get-ai.bat` puts ONNX Runtime 1.24.4 with DirectML and
the models in place, and `scripts\ai-check.bat` tries them on a photo.

On the CPU, sessions use ONNX Runtime's basic graph optimisations only: with
the extended (layout) optimisations, version 1.24 corrupts the Segment
Anything encoder's output in about one run in four (rows of the mask go
missing, sometimes NaN). Basic optimisations gave the same result in 20 of
20 runs. GPU providers keep all optimisations.

Memory: BiRefNet needs more than 6 GB of RAM on the CPU; on a machine with
less, use Segment Anything (a click or a box) for the subject.

## Folders

- Models: `$AKTIS_MODELS_DIR`, else `models` in the data folder.
- Raster cache: `masks` in the data folder.
- Data folder: `$AKTIS_DATA_DIR`, else `%LOCALAPPDATA%\aktis`,
  `$XDG_DATA_HOME/aktis` (`~/.local/share/aktis`) or
  `~/Library/Application Support/aktis`.

## Command line

```sh
aktis-cli models                                   # what is installed, where the rest goes
aktis-cli mask photo.NEF --subject                 # writes photo-mask.jpg: selection in red
aktis-cli mask photo.NEF --people --mask-out people.jpg
aktis-cli mask photo.NEF --parts hair,lips         # those parts of every person
aktis-cli mask photo.NEF --people --subtract --parts face-skin   # people without their faces
aktis-cli mask photo.NEF --point 0.42,0.55 --not 0.47,0.30
aktis-cli mask photo.NEF --box 0.2,0.1,0.6,0.9 --feather 1
aktis-cli mask photo.NEF --subject --brush 0.03:0.1,0.2:0.3,0.25 --erase 0.02:0.5,0.5
aktis-cli mask photo.NEF --subject --invert        # the background
aktis-cli develop photo.NEF --exposure 1 --subject # brighten only the subject
```

Parts of the selection combine in order; `--add` (default), `--subtract` and
`--intersect` apply to what follows. Coordinates are fractions of the upright
photo. `aktis-cli mask` also reads 8-bit binary PPM files, so it can be tried on
any picture (`jpegtopnm x.jpg > x.ppm`).

## Not yet

- The window has no mask panel yet: clicks, boxes, brushes and the part
  list come with the viewer's tools (top bar: masks, AI object selection).
- Sky and background-by-category need a further model with a licence that
  allows commercial use.
- Text prompts ("the red car") need SAM 3, whose licence must be reviewed.
- Masks are evaluated on the CPU and uploaded; a GPU port of refinement,
  brushes and feather comes with the viewer's preview pyramid.
- Geometry (crop, lens correction) will map mask coordinates through the same
  transform as the image.
