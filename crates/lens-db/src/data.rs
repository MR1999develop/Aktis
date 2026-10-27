//! The lensfun database files built into the editor (see `data/lensfun/README.md`).

/// Embeds `data/lensfun/<name>.xml`.
macro_rules! lensfun {
    ($name:literal) => {
        ($name, include_str!(concat!("../../../data/lensfun/", $name, ".xml")))
    };
}

/// File name (without `.xml`) and contents of each built-in file.
pub(crate) const FILES: &[(&str, &str)] = &[
    lensfun!("compact-canon"),
    lensfun!("compact-fujifilm"),
    lensfun!("compact-leica"),
    lensfun!("compact-nikon"),
    lensfun!("compact-olympus"),
    lensfun!("compact-panasonic"),
    lensfun!("compact-pentax"),
    lensfun!("compact-ricoh"),
    lensfun!("compact-sigma"),
    lensfun!("compact-sony"),
    lensfun!("mil-canon"),
    lensfun!("mil-fujifilm"),
    lensfun!("mil-hasselblad"),
    lensfun!("mil-leica"),
    lensfun!("mil-nikon"),
    lensfun!("mil-olympus"),
    lensfun!("mil-panasonic"),
    lensfun!("mil-pentax"),
    lensfun!("mil-samyang"),
    lensfun!("mil-sigma"),
    lensfun!("mil-sony"),
    lensfun!("mil-tamron"),
    lensfun!("mil-tokina"),
    lensfun!("mil-zeiss"),
    lensfun!("misc"),
    lensfun!("om-system"),
    lensfun!("rf-leica"),
    lensfun!("slr-canon"),
    lensfun!("slr-hasselblad"),
    lensfun!("slr-konica-minolta"),
    lensfun!("slr-leica"),
    lensfun!("slr-nikon"),
    lensfun!("slr-olympus"),
    lensfun!("slr-panasonic"),
    lensfun!("slr-pentax"),
    lensfun!("slr-ricoh"),
    lensfun!("slr-samyang"),
    lensfun!("slr-sigma"),
    lensfun!("slr-sony"),
    lensfun!("slr-tamron"),
    lensfun!("slr-tokina"),
    lensfun!("slr-zeiss"),
];
