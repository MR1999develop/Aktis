# Fuzzing

Coverage-guided fuzzing (libFuzzer, through `cargo fuzz`) of the parsers
that read files made elsewhere. Each target feeds generated files to one
entry point and fails on a panic that the editor would not catch, or on a
value that is accepted but does not write back the same.

| Target | What it reads |
|---|---|
| `raw_file` | A photo as the editor opens it: RAW and DNG (rawler, DNG opcodes, Exif), JPEG and TIFF. The first byte picks the file name extension. |
| `recipe` | Stored edits, presets and brushes, custom lenses, camera defaults. |
| `dcp` | DNG camera profiles (`.dcp`). |
| `lens_xml` | Lens database files (lensfun XML), and a correction with every lens read. |
| `checkpoint` | The PyTorch checkpoint (`.pth`) the AI denoise model is made from. |
| `keywords` | Lightroom keyword lists, import keywords, the import folder pattern (it must stay below the chosen folder). |

The RAW decoder is a third-party crate with known panics on rare files.
`raw-io` catches them, as the editor does, so `raw_file` reports only what
escapes into this project's code.

## Running

Needs the nightly compiler and `cargo-fuzz` (Linux or macOS; Windows only
with the MSVC address sanitizer):

```
rustup toolchain install nightly
cargo install cargo-fuzz --locked
cargo +nightly fuzz run recipe seeds/recipe -- -max_total_time=300
```

`fuzz/seeds/<target>` holds valid starting files. The tests of each crate
check that they are of the current format (run the tests with
`AKTIS_WRITE_SEEDS=1` to write them again after a change of format). New
files that libFuzzer finds go to `fuzz/corpus`, crashes to `fuzz/artifacts`;
neither is committed.

On GitHub the `Fuzz` workflow (started by hand) runs every target for a few
minutes and keeps the crashes it finds.

## A crash found

1. `cargo +nightly fuzz run <target> artifacts/<target>/<crash-file>` shows it again.
2. Fix the cause, and add the file (or a smaller one made from it) as a
   test of the crate, so it never comes back.
3. Look for the same mistake elsewhere in the parser.

The quick checks in the ordinary tests (`mutate::copies`) run on every
`scripts\check.bat` and need no nightly.
