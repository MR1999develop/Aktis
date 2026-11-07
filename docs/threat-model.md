# Threat model

What can go wrong when Aktis reads files and downloads made by others, what
stops it, and which risks are accepted. Update this page in the same change
that adds an input, a format, a download, a process the program starts, or
anything it sends out.

## Scope

The editor (`app`) and the command line (`aktis-cli`), one user on their own
computer: Windows first, Ubuntu and macOS prepared. There is no server, no
account and no secret: the program listens on no port and sends nothing
anywhere. Its only network use is downloading the AI models and ONNX Runtime
when the user asks for them.

## Assumptions

Each one says what would make it false; then this page must be looked at again.

| | Assumption | False when |
|---|---|---|
| A1 | The user's account, the program's folder and the editor's data folder (`%LOCALAPPDATA%\aktis`) are the user's: whoever can write there already has the user's authority. | The program is installed where other users can write, or runs elevated. |
| A2 | Any file the user opens, imports or drops on the window may have been made by anyone to attack the program: RAW, DNG, JPEG, TIFF, DCP profiles, lens files, presets, keyword lists, catalogs. | Never: this always holds. |
| A3 | The download hosts (nuget.org, GitHub, Hugging Face) or the TLS connection to them may be compromised. The list of downloads compiled into the program is trusted. | The list is read from the network (a manifest), or a download is added without a checksum. |
| A4 | Environment variables (`AKTIS_DATA_DIR`, `AKTIS_MODELS_DIR`, `ORT_DYLIB_PATH` …) and command-line flags are set by the user. | Another program starts the editor for someone else (a URI handler, a service). |
| A5 | The program never runs with more rights than the user's. | An installer or a feature asks for elevation. |

## Assets

| Size | Asset | What matters |
|---|---|---|
| L | The user's photos | Integrity and availability: originals are never written; exports only create new files. |
| L | Edits, catalog, keywords, ratings | Integrity and availability: atomic writes, undo, versioned formats. |
| L | The computer itself | Integrity: no code from a file or a download runs with the user's authority. |
| M | Personal data inside photos (place, names, camera serial numbers) | Confidentiality: nothing leaves the computer except in the files the user exports. |

There is no XL asset yet. When releases are signed, the signing key becomes one.

## Data flows and trust boundaries

```
 files from anyone ──TB1──▶ parsers (raw-io, recipe-store, core-color,     ──▶ typed values ──▶ editor, engines, GPU
                            lens-db, tagging, catalog)                                              │
                                                                                                    ▼
                                                                         exports: new files only, names cleaned
 internet ──TB2──▶ curl (HTTPS only) ──▶ <file>.part ──▶ size + SHA-256 ──▶ models folder
                                                                                 │
                                                       ──TB4──▶ ONNX Runtime, DirectML (native code, in the process)
 the user ──▶ window / command line ──▶ commands (undo) ──▶ catalog (SQLite)
 the editor ──TB3──▶ explorer, PowerShell (Recycle Bin), curl, tar: full paths, argument lists, no shell
```

- **TB1, file contents.** Everything inside a file is untrusted until a parser
  turns it into a checked type.
- **TB2, the network.** What arrives counts only once its size and SHA-256
  match the compiled-in catalog.
- **TB3, other programs.** The editor starts system programs only, each by its
  full path (`platform::system_program`), with every argument passed as it
  is. The path of a file goes to PowerShell Base64-encoded inside a fixed
  script.
- **TB4, native libraries.** ONNX Runtime and DirectML run with the
  editor's authority. Only the files whose checksums the program pins are
  loaded, by absolute path, never through the DLL search order.

## Entry points and their defences

| Input | Read by | Defences | Tests |
|---|---|---|---|
| RAW, DNG | `rawler` (third party), `raw-io` (TIFF tags, DNG opcodes, Exif) | File size limit (1 GiB); IFD count, depth and value-size limits; the decoder's panics are caught (`catch_unwind`); checked size arithmetic; overflow checks in the release build. | Damaged copies (`mutate`), fuzz target `raw_file` |
| JPEG, TIFF pictures | `image` (jpeg, tiff), `raw-io` | Same file size limit; the decoder's own memory limits; images above `core_model::MAX_PIXELS` refused. | `raw_file` |
| Edits, presets, brushes, custom lenses, camera defaults | `recipe-store` (own JSON parser) | 64 MiB and depth limits; one versioned format; unknown and duplicate fields refused; what is accepted writes back the same. | Damaged copies, fuzz target `recipe` |
| DCP profiles | `core-color` | 32 MiB limit, table-size limit, damaged files refused. | Damaged copies, fuzz target `dcp` |
| Lens files (lensfun XML) | `lens-db` (own XML reader) | 64 MiB and depth limits; no DTD processing and no external entities. | Damaged copies, fuzz target `lens_xml` |
| PyTorch checkpoint (`.pth`) | `scunet-onnx` (own zip and pickle readers) | SHA-256 pinned before reading; the pickle reader runs no code (an allowlist of names, no imports); the zip is read in memory, so no entry name becomes a path. | Damaged copies, fuzz target `checkpoint` |
| Keyword lists, import keywords, import folder pattern | `tagging`, `catalog` | 100,000 keywords at most; control and bidirectional characters refused; the folder pattern gives only plain folder names (no `.`, `..`, drive or root), so imports stay below the chosen folder. | Damaged copies, fuzz target `keywords` |
| A catalog from another computer | SQLite (`rusqlite`, bundled) | Defensive mode and an untrusted schema; every value is a bound parameter; a newer schema is refused. | `a_catalog_from_elsewhere_cannot_rewrite_its_own_structure` |
| AI models, ONNX Runtime | `ai-models` download | Fixed URLs and revisions; HTTPS only, redirects included; size and SHA-256 of every file (of the file taken from the archive, for the NuGet packages); `.part` files renamed only when checked. | `every_file_is_checked_against_a_checksum`, `curl_refuses_plain_http` |
| A file named on the command line | Same parsers as above | Treated as any file from anyone (A2). | — |

## Threats per element (STRIDE, from the outside in)

- **Files (TB1).** *Tampering* and *elevation*: a crafted file that runs code
  or writes outside the folder the user chose. Answered by memory-safe parsers
  (`unsafe_code = "forbid"` in every crate of the project), the limits above,
  name cleaning and non-destructive export paths. *Denial of service*: a file
  that hangs or exhausts memory. Answered by size limits; a hang affects only
  the worker thread.
- **Downloads (TB2).** *Spoofing* and *tampering*: a server or proxy that
  serves other code. Answered by pinned checksums and HTTPS-only curl.
  *Denial of service*: a failed download changes nothing that is already
  installed.
- **Processes (TB3).** *Elevation*: a program of the same name planted on the
  search path; an argument read as a command. Answered by full paths and
  argument lists.
- **Native libraries (TB4).** *Elevation*: a different DLL loaded. Answered by
  absolute paths and pinned checksums.
- **Catalog.** *Tampering*: a catalog file that rewrites its own structure or
  calls functions from its schema. Answered by SQLite's defensive mode.
- **Information disclosure.** The program has no telemetry and makes no request
  except the downloads the user starts. URLs carry no data of the user.
- **Repudiation.** Not applicable: one user, no shared state.

## Supply chain

- `Cargo.lock` with checksums; every build in CI is `--locked`.
- `cargo deny` (`deny.toml`): RustSec vulnerabilities, yanked crates, licences
  compatible with GPL-3.0, crates.io as the only source. Run by
  `scripts\check.bat` and every day by the `Audit` workflow, so a vulnerability
  published today fails tomorrow even without a new commit.
- New dependency versions wait a few days before they are taken (a yanked
  `async-executor` 1.14.0 was replaced by the older 1.13.3 rather than by a
  release two days old).
- GitHub Actions are pinned to release commits. Every CI job gets a
  read-only token, keeps no credentials and makes no commits; results stay
  as artifacts of the run.
- Fuzzing: `fuzz/` (cargo fuzz), started by hand with the `Fuzz` workflow.

## Accepted risks

| | Risk | Why it is accepted | Look again when |
|---|---|---|---|
| R1 | `rawler`, a third-party RAW decoder, parses untrusted files in the editor's process. A logic error could give wrong pixels, and a memory error in any `unsafe` code it has would run with the user's authority. | It is Rust and its panics are caught. A separate decoding process would copy 100+ MB images between processes and add an IPC format to secure. | A memory-safety advisory for `rawler`; before version 1.0; or when the editor reads files without the user choosing them (watch folders, a network share). |
| R2 | ONNX Runtime (C++) parses the model files in the process. | Only files whose checksums are pinned are installed; a model the user puts in the models folder is the user's own choice (A1). | Users can load models of their own choosing from the window. |
| R3 | On Linux, `curl`, `tar`, `xdg-open` and `gio` are found on the search path. | Their place differs between distributions; the search path is the user's (A1). | Linux packages are released. |
| R4 | The NuGet packages are not pinned, only the DLLs inside them, and the system's `tar` reads the unchecked archive. | nuget.org may sign a package again, which changes its bytes but not the files inside. The `tar` of Windows extracts one named member into a fresh folder. | Extraction moves into the program, or a package without a pinned inner file is added. |
| R5 | Releases have no signature, `SHA256SUMS`, SBOM or provenance yet. | Nothing is released yet. | Before the first public release (see `SECURITY.md`). |
| R6 | Crates no longer maintained deep in the tree (`paste` through `tokenizers`; `zbus`, `zvariant` through egui's accessibility on Linux; `ttf-parser`) are reported, not failing. | They are not this project's to replace. | Every egui or tokenizers upgrade. |

## Alternatives not taken

- **A TLS stack inside the program.** The system's curl is used instead: no
  certificate store to maintain, and security fixes come with Windows updates.
- **Checking the Authenticode signature of the DLLs when they load.** It needs
  unsafe FFI, which the project forbids. A pinned checksum is stricter: it
  accepts one exact file, not anything Microsoft has signed.
- **Pinning the NuGet packages themselves.** See R4.

## Review log

- **2026-11-04, security design review.** A full pass over the code base, with
  the threat model written down for the first time. Fixed in the same change:
  - checksums for `onnxruntime.dll` and `DirectML.dll` (Windows) and for
    SigLIP 2, now at a fixed revision;
  - curl held to HTTPS, redirects included;
  - system programs started by full path;
  - overflow checks in the release build of the parser crates;
  - damaged-copy tests for every parser, plus fuzz targets;
  - `cargo deny` locally and every day;
  - CI with pinned actions, read-only build jobs and checked downloads;
  - SQLite's defensive mode;
  - keyword names: invisible marks refused, and a second byte-order mark at
    the start of a file no longer ends up inside the first name.
