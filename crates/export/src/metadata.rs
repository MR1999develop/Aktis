//! What an exported file says about the photo: EXIF (camera, lens,
//! exposure, date, GPS, rating, keywords, copyright) and an XMP packet
//! (rating, keywords, copyright, author), filtered by the user's choice.

use crate::ExportError;
use crate::tiff::{Field, Ifd, Value, tiff_bytes};

/// Which metadata an export keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MetadataChoice {
    /// Everything known about the photo.
    #[default]
    All,
    /// Everything but the GPS position.
    AllButGps,
    /// The copyright notice alone.
    CopyrightOnly,
    /// Nothing.
    None,
}

impl MetadataChoice {
    /// Every choice, in menu order.
    pub const ALL: [Self; 4] = [Self::All, Self::AllButGps, Self::CopyrightOnly, Self::None];

    /// Label for menus.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::AllButGps => "All except GPS position",
            Self::CopyrightOnly => "Copyright only",
            Self::None => "None",
        }
    }
}

/// The photo's data that can be written into an exported file. Every field
/// is optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoMetadata {
    /// Camera maker as the camera writes it.
    pub make: Option<String>,
    /// Camera model as the camera writes it.
    pub model: Option<String>,
    /// Lens name.
    pub lens: Option<String>,
    /// Exposure time as a fraction of a second (numerator, denominator).
    pub exposure_time: Option<(u32, u32)>,
    /// F-number.
    pub aperture: Option<f32>,
    /// ISO sensitivity.
    pub iso: Option<u32>,
    /// Focal length in mm.
    pub focal_length: Option<f32>,
    /// Exposure compensation in EV.
    pub exposure_bias: Option<f32>,
    /// Capture date and time as cameras write it ("2026:08:14 18:42:07").
    pub captured: Option<String>,
    /// Latitude and longitude in degrees (south and west negative).
    pub gps: Option<(f64, f64)>,
    /// Rating, 1–5 (0 or `None`: not rated).
    pub rating: Option<u8>,
    /// Keywords.
    pub keywords: Vec<String>,
    /// Photographer.
    pub artist: Option<String>,
    /// Copyright notice.
    pub copyright: Option<String>,
    /// The program that made the file.
    pub software: Option<String>,
}

impl PhotoMetadata {
    /// What `choice` keeps of this.
    #[must_use]
    pub fn filtered(self, choice: MetadataChoice) -> Self {
        match choice {
            MetadataChoice::All => self,
            MetadataChoice::AllButGps => Self { gps: None, ..self },
            MetadataChoice::CopyrightOnly => Self { copyright: self.copyright, ..Self::default() },
            MetadataChoice::None => Self::default(),
        }
    }

    /// `true` when nothing would be written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// EXIF and TIFF tags written.
mod tag {
    pub const MAKE: u16 = 0x010F;
    pub const MODEL: u16 = 0x0110;
    pub const ORIENTATION: u16 = 0x0112;
    pub const SOFTWARE: u16 = 0x0131;
    pub const ARTIST: u16 = 0x013B;
    pub const RATING: u16 = 0x4746;
    pub const RATING_PERCENT: u16 = 0x4749;
    pub const COPYRIGHT: u16 = 0x8298;
    pub const EXIF_IFD: u16 = 0x8769;
    pub const GPS_IFD: u16 = 0x8825;
    pub const XP_KEYWORDS: u16 = 0x9C9E;
    pub const EXPOSURE_TIME: u16 = 0x829A;
    pub const F_NUMBER: u16 = 0x829D;
    pub const ISO: u16 = 0x8827;
    pub const EXIF_VERSION: u16 = 0x9000;
    pub const DATE_ORIGINAL: u16 = 0x9003;
    pub const DATE_DIGITIZED: u16 = 0x9004;
    pub const EXPOSURE_BIAS: u16 = 0x9204;
    pub const FOCAL_LENGTH: u16 = 0x920A;
    pub const COLOR_SPACE: u16 = 0xA001;
    pub const PIXEL_X: u16 = 0xA002;
    pub const PIXEL_Y: u16 = 0xA003;
    pub const LENS_MODEL: u16 = 0xA434;
    pub const GPS_VERSION: u16 = 0x0000;
    pub const GPS_LAT_REF: u16 = 0x0001;
    pub const GPS_LAT: u16 = 0x0002;
    pub const GPS_LON_REF: u16 = 0x0003;
    pub const GPS_LON: u16 = 0x0004;
}

/// A positive value as a rational with `scale` as denominator.
fn rational(v: f64, scale: u32) -> Option<(u32, u32)> {
    let n = (v * f64::from(scale)).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // range checked
    (v.is_finite() && n >= 0.0 && n <= f64::from(u32::MAX)).then_some((n as u32, scale))
}

/// Degrees as degrees, minutes and seconds (hundredths).
fn dms(degrees: f64) -> Vec<(u32, u32)> {
    let a = degrees.abs();
    let d = a.floor();
    let m = ((a - d) * 60.0).floor();
    let s = ((a - d) * 60.0 - m) * 60.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–180, 0–59
    let (d, m) = (d as u32, m as u32);
    vec![(d, 1), (m, 1), rational(s, 100).unwrap_or((0, 1))]
}

/// A camera date ("YYYY:MM:DD HH:MM:SS"), when the text is one.
fn exif_date(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let ok = b.len() == 19
        && digits.iter().all(|&i| b[i].is_ascii_digit())
        && [4, 7].iter().all(|&i| b[i] == b':' || b[i] == b'-')
        && b[10] == b' '
        && [13, 16].iter().all(|&i| b[i] == b':');
    ok.then(|| {
        let mut d = s.to_owned();
        d.replace_range(4..5, ":");
        d.replace_range(7..8, ":");
        d
    })
}

fn text(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// The EXIF fields of IFD0, with the EXIF and GPS sub-directories; empty
/// when `meta` is. `width` and `height` are the exported image's size.
pub(crate) fn exif_ifd(meta: &PhotoMetadata, width: u32, height: u32) -> Ifd {
    let mut ifd0 = Ifd::default();
    if meta.is_empty() {
        return ifd0;
    }
    let ascii = |ifd: &mut Ifd, tag: u16, v: Option<&String>| {
        if let Some(t) = text(v) {
            ifd.push(tag, Value::Ascii(t));
        }
    };
    ascii(&mut ifd0, tag::MAKE, meta.make.as_ref());
    ascii(&mut ifd0, tag::MODEL, meta.model.as_ref());
    // The exported pixels are upright.
    ifd0.push(tag::ORIENTATION, Value::Short(vec![1]));
    ascii(&mut ifd0, tag::SOFTWARE, meta.software.as_ref());
    ascii(&mut ifd0, tag::ARTIST, meta.artist.as_ref());
    ascii(&mut ifd0, tag::COPYRIGHT, meta.copyright.as_ref());
    if let Some(r) = meta.rating.filter(|r| (1..=5).contains(r)) {
        ifd0.push(tag::RATING, Value::Short(vec![u16::from(r)]));
        let percent = [1, 25, 50, 75, 99][usize::from(r - 1)];
        ifd0.push(tag::RATING_PERCENT, Value::Short(vec![percent]));
    }
    let keywords: Vec<&str> = meta.keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty()).collect();
    if !keywords.is_empty() {
        // Windows' "Tags": UTF-16LE, separated by semicolons, NUL-terminated.
        let joined = keywords.join(";");
        let bytes = joined.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        ifd0.push(tag::XP_KEYWORDS, Value::Byte(bytes));
    }

    let mut exif = Ifd::default();
    if let Some((n, d)) = meta.exposure_time.filter(|(n, d)| *n > 0 && *d > 0) {
        exif.push(tag::EXPOSURE_TIME, Value::Rational(vec![(n, d)]));
    }
    if let Some(r) = meta.aperture.and_then(|a| rational(f64::from(a), 10)) {
        exif.push(tag::F_NUMBER, Value::Rational(vec![r]));
    }
    if let Some(iso) = meta.iso.filter(|i| *i > 0) {
        exif.push(tag::ISO, Value::Short(vec![u16::try_from(iso).unwrap_or(u16::MAX)]));
    }
    let has_camera_data = !exif.fields.is_empty() || meta.make.is_some() || meta.model.is_some();
    if has_camera_data || meta.captured.is_some() {
        exif.push(tag::EXIF_VERSION, Value::Undefined(b"0232".to_vec()));
    }
    if let Some(date) = meta.captured.as_deref().and_then(exif_date) {
        exif.push(tag::DATE_ORIGINAL, Value::Ascii(date.clone()));
        exif.push(tag::DATE_DIGITIZED, Value::Ascii(date));
    }
    if let Some(bias) = meta.exposure_bias.filter(|b| b.is_finite()) {
        #[allow(clippy::cast_possible_truncation)] // a few EV, in sixths
        let sixths = (f64::from(bias) * 6.0).round() as i32;
        exif.push(tag::EXPOSURE_BIAS, Value::SRational(vec![(sixths, 6)]));
    }
    if let Some(r) = meta.focal_length.and_then(|f| rational(f64::from(f), 10)) {
        exif.push(tag::FOCAL_LENGTH, Value::Rational(vec![r]));
    }
    ascii(&mut exif, tag::LENS_MODEL, meta.lens.as_ref());
    if !exif.fields.is_empty() {
        exif.push(tag::COLOR_SPACE, Value::Short(vec![1])); // sRGB
        exif.push(tag::PIXEL_X, Value::Long(vec![width]));
        exif.push(tag::PIXEL_Y, Value::Long(vec![height]));
        ifd0.fields.push(Field::SubIfd(tag::EXIF_IFD, exif));
    }

    if let Some((lat, lon)) = meta.gps.filter(|(a, b)| a.is_finite() && b.is_finite()) {
        let mut gps = Ifd::default();
        gps.push(tag::GPS_VERSION, Value::Byte(vec![2, 3, 0, 0]));
        gps.push(tag::GPS_LAT_REF, Value::Ascii(if lat < 0.0 { "S" } else { "N" }.into()));
        gps.push(tag::GPS_LAT, Value::Rational(dms(lat)));
        gps.push(tag::GPS_LON_REF, Value::Ascii(if lon < 0.0 { "W" } else { "E" }.into()));
        gps.push(tag::GPS_LON, Value::Rational(dms(lon)));
        ifd0.fields.push(Field::SubIfd(tag::GPS_IFD, gps));
    }
    ifd0
}

/// The EXIF block (a TIFF structure, without the JPEG `Exif\0\0` prefix),
/// or `None` when there is nothing to write.
///
/// # Errors
/// [`ExportError::FileTooLarge`] for absurdly long texts.
pub(crate) fn exif_block(meta: &PhotoMetadata, width: u32, height: u32) -> Result<Option<Vec<u8>>, ExportError> {
    let ifd = exif_ifd(meta, width, height);
    if ifd.fields.is_empty() { Ok(None) } else { tiff_bytes(&ifd).map(Some) }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters XML 1.0 does not allow are dropped.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// An XMP packet with the rating, keywords, copyright and author (what
/// photo libraries read), or `None` when there are none.
#[must_use]
pub(crate) fn xmp_packet(meta: &PhotoMetadata) -> Option<String> {
    let rating = meta.rating.filter(|r| (1..=5).contains(r));
    let keywords: Vec<&str> = meta.keywords.iter().map(|k| k.trim()).filter(|k| !k.is_empty()).collect();
    let copyright = text(meta.copyright.as_ref());
    let artist = text(meta.artist.as_ref());
    if rating.is_none() && keywords.is_empty() && copyright.is_none() && artist.is_none() {
        return None;
    }
    let mut x = String::from("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    x.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    x.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    x.push_str("  <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"");
    x.push_str(" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"");
    if let Some(r) = rating {
        x.push_str(&format!(" xmp:Rating=\"{r}\""));
    }
    x.push_str(">\n");
    if !keywords.is_empty() {
        x.push_str("   <dc:subject><rdf:Bag>");
        for k in keywords {
            x.push_str(&format!("<rdf:li>{}</rdf:li>", xml_escape(k)));
        }
        x.push_str("</rdf:Bag></dc:subject>\n");
    }
    if let Some(c) = copyright {
        x.push_str(&format!(
            "   <dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:rights>\n",
            xml_escape(&c)
        ));
    }
    if let Some(a) = artist {
        x.push_str(&format!("   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", xml_escape(&a)));
    }
    x.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiff::read::{ascii, ifd0, number, rationals, sub};

    fn sample() -> PhotoMetadata {
        PhotoMetadata {
            make: Some("NIKON CORPORATION".into()),
            model: Some("NIKON D3S".into()),
            lens: Some("AF-S Nikkor 24-70mm f/2.8G".into()),
            exposure_time: Some((1, 250)),
            aperture: Some(5.6),
            iso: Some(800),
            focal_length: Some(35.0),
            exposure_bias: Some(-0.7),
            captured: Some("2026:08:14 18:42:07".into()),
            gps: Some((45.986, -9.258)),
            rating: Some(4),
            keywords: vec!["lake".into(), "Como & co".into()],
            artist: Some("Ada".into()),
            copyright: Some("\u{a9} 2026 Ada".into()),
            software: Some("Aktis 0.1".into()),
        }
    }

    #[test]
    fn every_field_reads_back_from_the_exif_block() {
        let b = exif_block(&sample(), 6000, 4000).expect("writes").expect("not empty");
        let d = ifd0(&b);
        assert_eq!(ascii(&d, tag::MAKE), "NIKON CORPORATION");
        assert_eq!(ascii(&d, tag::MODEL), "NIKON D3S");
        assert_eq!(number(&d, tag::ORIENTATION), 1);
        assert_eq!(ascii(&d, tag::COPYRIGHT), "\u{a9} 2026 Ada");
        assert_eq!(ascii(&d, tag::ARTIST), "Ada");
        assert_eq!(number(&d, tag::RATING), 4);
        let xp = &d[&tag::XP_KEYWORDS].2;
        let units: Vec<u16> = xp.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).expect("UTF-16"), "lake;Como & co\0");
        let e = sub(&b, &d, tag::EXIF_IFD);
        assert_eq!(rationals(&e, tag::EXPOSURE_TIME), vec![1.0 / 250.0]);
        assert!((rationals(&e, tag::F_NUMBER)[0] - 5.6).abs() < 1e-6);
        assert_eq!(number(&e, tag::ISO), 800);
        assert_eq!(ascii(&e, tag::DATE_ORIGINAL), "2026:08:14 18:42:07");
        assert!((rationals(&e, tag::EXPOSURE_BIAS)[0] + 4.0 / 6.0).abs() < 1e-9);
        assert_eq!(rationals(&e, tag::FOCAL_LENGTH), vec![35.0]);
        assert_eq!(ascii(&e, tag::LENS_MODEL), "AF-S Nikkor 24-70mm f/2.8G");
        assert_eq!((number(&e, tag::PIXEL_X), number(&e, tag::PIXEL_Y)), (6000, 4000));
        assert_eq!(number(&e, tag::COLOR_SPACE), 1);
        let g = sub(&b, &d, tag::GPS_IFD);
        assert_eq!(ascii(&g, tag::GPS_LAT_REF), "N");
        assert_eq!(ascii(&g, tag::GPS_LON_REF), "W");
        let lat = rationals(&g, tag::GPS_LAT);
        assert!((lat[0] + lat[1] / 60.0 + lat[2] / 3600.0 - 45.986).abs() < 1e-5, "{lat:?}");
        let lon = rationals(&g, tag::GPS_LON);
        assert!((lon[0] + lon[1] / 60.0 + lon[2] / 3600.0 - 9.258).abs() < 1e-5, "{lon:?}");
    }

    #[test]
    fn the_choices_keep_what_they_say() {
        let no_gps = exif_block(&sample().filtered(MetadataChoice::AllButGps), 10, 10).expect("ok").expect("some");
        let d = ifd0(&no_gps);
        assert!(!d.contains_key(&tag::GPS_IFD));
        assert!(d.contains_key(&tag::EXIF_IFD));

        let copyright =
            exif_block(&sample().filtered(MetadataChoice::CopyrightOnly), 10, 10).expect("ok").expect("some");
        let d = ifd0(&copyright);
        let tags: Vec<u16> = d.keys().copied().collect();
        assert_eq!(tags, vec![tag::ORIENTATION, tag::COPYRIGHT]);

        assert_eq!(exif_block(&sample().filtered(MetadataChoice::None), 10, 10).expect("ok"), None);
        assert_eq!(xmp_packet(&sample().filtered(MetadataChoice::None)), None);
    }

    #[test]
    fn the_xmp_packet_carries_rating_keywords_and_rights_escaped() {
        let x = xmp_packet(&sample()).expect("packet");
        assert!(x.contains("xmp:Rating=\"4\""));
        assert!(x.contains("<rdf:li>lake</rdf:li><rdf:li>Como &amp; co</rdf:li>"));
        assert!(x.contains("\u{a9} 2026 Ada"));
        assert!(x.starts_with("<?xpacket begin=\"\u{feff}\""));
        assert!(x.ends_with("<?xpacket end=\"w\"?>"));
    }

    #[test]
    fn only_real_camera_dates_are_written() {
        assert_eq!(exif_date("2026:08:14 18:42:07").as_deref(), Some("2026:08:14 18:42:07"));
        assert_eq!(exif_date("2026-08-14 18:42:07").as_deref(), Some("2026:08:14 18:42:07"));
        assert_eq!(exif_date("yesterday"), None);
        assert_eq!(exif_date("2026:08:14"), None);
    }
}
