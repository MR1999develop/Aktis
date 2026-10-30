# Catalogs, import and the Library

How the editor keeps photos, as in Capture One's catalogs. Code:
`crates/catalog` (model, import planning, SQLite store),
`crates/editor/src/catalogs.rs` and `import_job.rs`, and in the window
`catalog_ui.rs`, `library_ui.rs` and `library_collections.rs`.

## Catalogs

- A catalog is a folder `Name.rnecatalog` holding its database
  `catalog.sqlite`, so it can be moved or copied whole.
- File ▸ New Catalog… (Ctrl+N) makes one in the catalogs folder
  (Preferences ▸ Catalogs; by default `Catalogs` in the editor's data
  folder). File ▸ Open Catalog… (Ctrl+O) and Open Recent Catalog switch
  catalogs: the open photo is saved and closed first.
- File ▸ Save Catalog As… (Ctrl+Shift+S) saves a copy of the open catalog
  (photos, edits, ratings, keywords, albums, Trash) under a new name and
  folder, then opens the copy; the old catalog stays as it was. Use it to
  give the photos imported into `Default` a catalog of their own. The copy
  is made by SQLite (`VACUUM INTO`), consistent while the catalog is open.
- At start the catalog open when the editor was closed opens again. The first
  time, `Default.rnecatalog` is made and takes over the single catalog of
  earlier versions (ratings, keywords, edits; every photo it knew counts as
  imported).
- Saving: ratings, labels, keywords, albums and the Trash are written at once.
  Edits are saved 30 s after the last change, and also when another photo,
  variant or catalog is opened and when the window closes. There is no Save
  command (Save Catalog As makes a copy under another name).
- Variants in the filmstrip, as in Capture One: a photo with several
  variants has a card for each, in number order, right after each other;
  each shows the preview stored for that variant (the photo's thumbnail
  while there is none) and its number after the file name. A click opens
  that variant (the open photo switches to it). Rating, colour label,
  keywords and selection belong to the photo, so they are shared by its
  variants (`Catalog::multi_variants`, `Catalog::variant_preview`,
  `editor::library::strip_cards`).
- Deleting a variant (its card's menu ▸ Delete Variant N…, or Delete over
  the filmstrip while the open photo has several) removes only that
  variant, after a confirmation: the photo and its other variants stay,
  the first variant included when another is deleted, and the other way
  round. Move Photo to Trash in the same menu takes the photo with all its
  variants.

## Import

- The Import Images window, as Capture One's: on the left the source
  (folder, Include sub-folders, Exclude duplicates, a name filter, Check All /
  Uncheck All); in the middle its photos as a grid, or one photo large over
  a strip of thumbnails (← → move, Space ticks); on the right the options.
  Each photo has a tick: the ticked ones are imported; duplicates are shown
  dimmed with "in catalog". The folder, capture times and thumbnails are
  read on threads (`editor::import_scan`).
- Photos come into the editor only through an import; a file that is not in
  the catalog does not open. File ▸ Import Images… (Ctrl+Shift+I), photos
  dropped on the window, "Import this folder" under the Disk tree, or a photo
  given on the command line.
- Formats: camera RAW and DNG, JPEG, TIFF. A RAW+JPEG pair (same name, same
  folder) can come in whole or as the RAW alone.
- Import To: add the photos where they are, or copy them to a folder in
  sub-folders made from a pattern (`{yyyy}/{yyyy}-{mm}-{dd}` by default;
  `{yyyy}`, `{yy}`, `{mm}`, `{dd}` from the capture day, `Undated` without
  one). A copy never replaces another file ("a (2).NEF"); an import run again
  finds its copies (same content) instead of copying twice. Optionally a
  backup of the originals goes to a second folder.
- Duplicates: a photo with the same file name and capture time as one in the
  catalog (or earlier in the same import) is left out, wherever it is.
- A preset, a rating (click a star; the same star again takes it off),
  keywords, the photographer and the copyright can be given to every photo
  imported.
- The import runs in the background ("Importing 12 of 340…" with Stop in the
  status bar); the photos show up in the import as they come in.
- JPEG and TIFF pictures are read as sRGB into linear light and edited like
  RAW files; Auto shows them with the Linear curve (no second tone curve) and
  without lens corrections. Embedded colour profiles are not applied yet.

## The Library

The left panel, from the top:

- **Search**: type part of a file name to see the matching photos of the
  whole catalog (the Trash apart); Escape or another collection ends it.
- **Catalog Collections**: All Images; Recent Imports (the last 10 imports,
  by local date and time, with their number of photos); Recent Edits (edited in the last 30 days,
  latest first); Trash. Right-click the Trash for Empty Trash…. Every row
  shows its number of photos on the right.
- **Albums**: photos put together by hand, in a tree of sub-albums. "+"
  makes an album (drop photos on it to start the album with them).
  Right-click an album: New Sub-album…, Rename Album…, Merge Into ▸ (its
  photos and sub-albums go into the other album; the message offers Undo),
  Delete Album… (its photos stay in the catalog, its sub-albums move up).
  Drag photos from the filmstrip (or the tiles shown when several are
  selected) onto an album, or use the card menu: Add to Album ▸, Remove
  from Album. An album lists its photos in the order they were put there.
- **Folders**: the folders photos were imported from, as a tree with counts
  (folders without photos of their own that lead to one sub-folder are
  shown as one line). A folder shows its own photos.
- **Disk**: the computer's folders, to look at photos. Right-click a folder:
  Import… (the Import Images window on it), Cut and Paste (moves the folder;
  on the same drive only), Rename…, Delete… (to the Recycle Bin, optionally
  removing its photos from the catalog), Show in Explorer. Each change asks
  first; the catalog follows a folder moved or renamed, so its photos keep
  their edits.

Trash: Delete with the pointer over the filmstrip, or Move to Trash in the
card menu, moves the photos (or the selection) to the Trash: they leave
every other collection but keep their edits. When the open photo goes, the
next one in the filmstrip opens (the one before when it was the last).
Restore takes them back; Empty Trash… removes them from the catalog (the
files stay on the disk).

Undo in the Library: the last move to the Trash or album merge can be
taken back with Ctrl+Z while the pointer is over the Library (Explorer or
filmstrip), with Edit ▸ Undo move to Trash / Undo album merge, or with the
message's Undo button. Elsewhere Ctrl+Z undoes the photo's edits. Emptying
the Trash ends the undo of a move to the Trash.
