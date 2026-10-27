# Lens database (lensfun)

These files are a copy of part of the [lensfun](https://github.com/lensfun/lensfun)
database (`data/db/`), read directly by the `lens-db` crate and built into the
editor. They describe cameras (mount, crop factor) and lenses (distortion,
transverse chromatic aberration and vignetting measured at several focal
lengths, apertures and distances).

- Source: https://github.com/lensfun/lensfun, commit
  `413bf8f261825f3e3062c17868e7766f2ebe8a21` (2026-10-25), database format version 2.
- Licence: **Creative Commons Attribution-ShareAlike 3.0** (CC BY-SA 3.0), see
  `COPYING.CC_BY-SA_3.0`. Copyright the lensfun contributors. The files are
  unchanged; changes to them (and only to them) must be shared under the same
  licence. This does not affect the licence of the editor's code.

## What is included

The files covering the major camera and lens makers: Canon, Nikon, Sony
(and Konica Minolta A-mount), Fujifilm, Panasonic/Lumix, Leica, Olympus and
OM System, Pentax and Ricoh, Hasselblad, Sigma, Tamron, Tokina, Samyang,
Zeiss, and `misc.xml` (Viltrox, Voigtländer, TTArtisan, Laowa, Meike, 7Artisans
and others). Left out: action cameras, medium-format film backs, and small or
discontinued brands (Casio, Kodak, Samsung, Schneider, Soligor, Vivitar,
USSR lenses, Contax). They can be added by copying the file here and listing
it in `crates/lens-db/src/data.rs`.

## Updating

Run `bash data/lensfun/update.sh [commit]` from the repository root: it
fetches lensfun at the given commit (default: the latest), copies the same
files, and prints the commit to record above. Then run the tests.

Users can also put newer lensfun XML files (for example from
`lensfun-update-data`) in the editor's `lensfun` settings folder; entries
there replace the built-in ones.
