# AI tagging (semi-automatic keywords)

Status: the tagging engine, the model integration and `rne tag` are in place;
the catalogue (DAM) that stores embeddings and decisions and the UI that shows
suggestions come with the catalogue phase.

## Goal

Recognise what is in a photo and propose keywords from the user's own keyword
list. The user stays in control: suggestions are proposed, the user confirms
or rejects them, and every decision teaches the system. Nothing is applied
silently unless the user turns that on, and even then only for keywords the
user has already confirmed many times.

## How it works

```text
photo ──► preview ──► image model ──► embedding (768 numbers, once per photo)
                                          │
keyword list ──► text model ──► one embedding per keyword name and synonym
                                          │
                           zero-shot probability per keyword
                                          + learned correction from decisions
                                          ▼
                              suggestions ──► accept / reject ──► decisions
                                                                     │
                                         re-learn the keywords that changed ◄┘
```

1. **Embedding.** Each photo is encoded once by the image half of the model
   into a unit vector. It is stored in the catalogue with the model id and is
   the only thing the rest of the pipeline needs: scoring, learning,
   "similar photos" and text search all run on stored embeddings in
   milliseconds, without touching the RAW file again.
2. **Zero-shot.** Each keyword's name and synonyms go through the text half
   of the model. SigLIP compares an image and a text with a learned scale and
   bias and a sigmoid, which gives an independent probability per keyword
   (a photo can be both "Tramonto" and "Mare"), with no training at all.
3. **Learning from decisions.** For each keyword with examples, a small
   logistic regression learns a correction on top of the zero-shot logit:
   positives are the photos the user applied the keyword to, negatives the
   photos where the user rejected it, plus a weak background sample of other
   photos. A penalty keeps the correction near zero while examples are few,
   so the model's own opinion dominates at first and the user's usage
   dominates later. Keywords without visual meaning (a client, a trip, a
   person's name) start "unlikely" and are learned from examples alone.
4. **Suggestions.** Keywords above a probability threshold, at most N per
   photo, most likely first. Only the most specific keyword of a branch is
   proposed (accepting "Airone" applies "Uccelli" and "Animali" too);
   categories (`[Luoghi]`) are never proposed; a rejected keyword is never
   proposed again for that photo; rejecting a parent silences its children.
5. **Optional auto-apply.** Off by default. When on, a keyword is applied
   without asking only above a high probability *and* after the user has
   confirmed it on at least 10 photos. Auto-applied keywords are marked as
   such, remain reviewable and are never used as training examples, so the
   system cannot reinforce its own mistakes.

Decisions (accepted / rejected / auto) are the only ground truth and are
model-independent. Everything else (embeddings, prompt embeddings, learned
corrections) is derived and can be recomputed, so the model can be replaced
by a better one later without losing any of the user's work.

## Choices

| Topic | Choice | Why |
| --- | --- | --- |
| Model | Google SigLIP 2, ViT-B/16, 224 px (Apache-2.0) | Multilingual (Italian keywords work as written), open vocabulary (any keyword, no fixed label set), real per-label probabilities (sigmoid), small enough for a laptop CPU. |
| Where it runs | Locally, on the user's computer | Photos never leave the machine; works offline; no per-image cost. |
| Runtime | ONNX Runtime through the `ort` crate | Mature, fast on CPU; optional GPU through DirectML on Windows (any vendor) or CUDA. The model is behind two traits (`ImageEncoder`, `TextEncoder`), so the runtime can change without touching the rest. |
| Model files | `onnx-community/siglip2-base-patch16-224-ONNX` | Separate vision and text graphs plus `tokenizer.json`; 8-bit quantised variants for a smaller download. |
| Keyword list | Lightroom's "Export Keywords" text format | Users can bring their existing keyword list; hierarchy, synonyms and categories map directly. |
| Image input | Linear Rec.2020 editor image → box-filter downscale in linear light → sRGB → model | Exact, alias-free and cheap: colour conversion happens at 224 × 224. |

## Crates

| Crate | Responsibility |
| --- | --- |
| `crates/tagging` | Pure logic, no I/O: vocabulary, embeddings, preprocessing, zero-shot head, learning, suggestion rules, similarity search. |
| `crates/tagging-onnx` | `ImageEncoder` / `TextEncoder` on ONNX Runtime for SigLIP-family exports; model specs (input size, token length, logit scale and bias). |
| `crates/cli` | `rne tag`: suggestions for RAW and JPEG files, and text search. |

## Try it

Download the model folder (`onnx/vision_model.onnx`, `onnx/text_model.onnx`
or their `_quantized` versions, and `tokenizer.json`) from
`onnx-community/siglip2-base-patch16-224-ONNX`, then:

```sh
cargo run --release -p cli -- tag photo.NEF other.jpg --model path/to/siglip2 \
    --keywords crates/tagging/testdata/keywords-it.txt --template "una foto di {}"
cargo run --release -p cli -- tag *.jpg --model path/to/siglip2 --search "barca al tramonto"
```

The CI job `tagging-model` does exactly this on sample photos and publishes
the output on the branch `ci-results/tagging-model`.

## Next steps (with the catalogue)

- Store per photo: embedding + model id, and the decisions; store prompt
  embeddings per (model, text) so a keyword list of thousands is encoded once.
- Background job: encode new photos from the catalogue preview (the RAW's
  embedded JPEG is enough; no full development needed), a few cores only,
  pausable; re-learn only the keywords whose decisions changed.
- UI: suggestion chips under the photo (accept with a click, reject with
  another), batch review grid "photos suggested for keyword X", "find
  similar", free-text search box.
- Write applied keywords to XMP (`dc:subject`, `lr:hierarchicalSubject`).
- First-run model download with the user's consent (vision 372 MB +
  text 1.1 GB + tokenizer 34 MB; quantised 95 MB + 283 MB), checksum-verified.
- Later, separate from this: faces and people (more sensitive, opt-in).
