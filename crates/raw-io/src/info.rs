//! Shooting data of a photo for the Metadata panel: exposure, date, GPS,
//! lens, and the rating set in the camera.

use rawler::decoders::RawMetadata;
use rawler::formats::tiff::Rational;

/// What the file says about how the photo was taken. Every field is
/// optional: files differ in what they record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoInfo {
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
    /// Date and time of capture as written by the camera ("2026:08:14 18:42:07").
    pub captured: Option<String>,
    /// Latitude and longitude in degrees (south and west negative).
    pub gps: Option<(f64, f64)>,
    /// Lens name, maker included when known.
    pub lens: Option<String>,
    /// Rating stored in the file by the camera (0–5).
    pub camera_rating: Option<u8>,
    /// Photographer.
    pub artist: Option<String>,
    /// Copyright notice.
    pub copyright: Option<String>,
}

impl PhotoInfo {
    /// "1/500 s", "2 s", "0.8 s".
    #[must_use]
    pub fn shutter_text(&self) -> Option<String> {
        let (n, d) = self.exposure_time?;
        if n == 0 || d == 0 {
            return None;
        }
        let seconds = f64::from(n) / f64::from(d);
        Some(if seconds >= 1.0 {
            format!("{} s", trim_decimal(seconds))
        } else if n == 1 {
            format!("1/{d} s")
        } else if seconds >= 0.3 {
            format!("{} s", trim_decimal(seconds))
        } else {
            format!("1/{} s", (1.0 / seconds).round())
        })
    }

    /// "1/500 s · f/8 · ISO 200" with whatever is known.
    #[must_use]
    pub fn exposure_text(&self) -> Option<String> {
        let mut parts = Vec::new();
        parts.extend(self.shutter_text());
        parts.extend(self.aperture.map(|a| format!("f/{}", trim_decimal(f64::from(a)))));
        parts.extend(self.iso.map(|i| format!("ISO {i}")));
        (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
    }

    /// "2026-08-14 18:42" from the camera's "2026:08:14 18:42:07".
    #[must_use]
    pub fn captured_text(&self) -> Option<String> {
        let raw = self.captured.as_deref()?.trim();
        let (date, time) = raw.split_once(' ').unwrap_or((raw, ""));
        let date = date.replace(':', "-");
        let time: String = time.split(':').take(2).collect::<Vec<_>>().join(":");
        let text = if time.is_empty() { date } else { format!("{date} {time}") };
        (!text.is_empty()).then_some(text)
    }

    /// "45.9860 N · 9.2580 E".
    #[must_use]
    pub fn gps_text(&self) -> Option<String> {
        let (lat, lon) = self.gps?;
        let ns = if lat < 0.0 { 'S' } else { 'N' };
        let ew = if lon < 0.0 { 'W' } else { 'E' };
        Some(format!("{:.4} {ns} \u{b7} {:.4} {ew}", lat.abs(), lon.abs()))
    }
}

/// "8", "2.8", "0.5": at most two decimals, no trailing zeros.
fn trim_decimal(v: f64) -> String {
    let text = format!("{v:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn rational(r: Rational) -> Option<f64> {
    (r.d != 0).then(|| f64::from(r.n) / f64::from(r.d)).filter(|v| v.is_finite())
}

/// Degrees from EXIF degrees, minutes, seconds; negative for `S`/`W`.
fn degrees(dms: [Rational; 3], reference: Option<&str>) -> Option<f64> {
    let d = rational(dms[0])?;
    let m = rational(dms[1]).unwrap_or(0.0);
    let s = rational(dms[2]).unwrap_or(0.0);
    let value = d + m / 60.0 + s / 3600.0;
    let negative = matches!(reference.map(str::trim), Some("S" | "W" | "s" | "w"));
    Some(if negative { -value } else { value })
}

fn text(s: Option<&String>) -> Option<String> {
    s.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty())
}

/// Reads the shooting data out of the decoder's metadata.
pub(crate) fn from_metadata(m: &RawMetadata) -> PhotoInfo {
    let e = &m.exif;
    let lens = match (text(e.lens_make.as_ref()), text(e.lens_model.as_ref())) {
        (Some(make), Some(model)) if !model.to_lowercase().starts_with(&make.to_lowercase()) => {
            Some(format!("{make} {model}"))
        }
        (_, Some(model)) => Some(model),
        (Some(make), None) => Some(make),
        (None, None) => None,
    };
    #[allow(clippy::cast_possible_truncation)] // focal lengths and f-numbers are small
    let small = |r: Option<Rational>| r.and_then(rational).filter(|v| *v > 0.0).map(|v| v as f32);
    let aperture = small(e.fnumber).or_else(|| {
        // APEX aperture value: f-number = 2^(Av / 2).
        e.aperture_value.and_then(rational).map(|av| 2_f64.powf(av / 2.0)).and_then(small_f64)
    });
    let gps = e.gps.as_ref().and_then(|g| {
        let lat = degrees(g.gps_latitude?, g.gps_latitude_ref.as_deref())?;
        let lon = degrees(g.gps_longitude?, g.gps_longitude_ref.as_deref())?;
        // 0/0 is what cameras write without a fix.
        (lat.abs() > 1e-9 || lon.abs() > 1e-9).then_some((lat, lon))
    });
    #[allow(clippy::cast_possible_truncation)] // ratings are 0–5
    let camera_rating = m.rating.filter(|r| *r <= 5).map(|r| r as u8);
    #[allow(clippy::cast_possible_truncation)] // exposure bias is a few EV
    let exposure_bias = e
        .exposure_bias
        .and_then(|r| (r.d != 0).then(|| f64::from(r.n) / f64::from(r.d)))
        .filter(|v| v.is_finite())
        .map(|v| v as f32);
    PhotoInfo {
        exposure_time: e.exposure_time.filter(|r| r.n > 0 && r.d > 0).map(|r| (r.n, r.d)),
        aperture,
        iso: e
            .iso_speed
            .filter(|v| *v > 0)
            .or(e.recommended_exposure_index.filter(|v| *v > 0))
            .or(e.iso_speed_ratings.map(u32::from).filter(|v| *v > 0)),
        focal_length: small(e.focal_length),
        exposure_bias,
        captured: text(e.date_time_original.as_ref()).or_else(|| text(e.create_date.as_ref())),
        gps,
        lens,
        camera_rating,
        artist: text(e.artist.as_ref()),
        copyright: text(e.copyright.as_ref()),
    }
}

#[allow(clippy::cast_possible_truncation)] // f-numbers are small
fn small_f64(v: f64) -> Option<f32> {
    (v.is_finite() && v > 0.0).then_some(v as f32)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn r(n: u32, d: u32) -> Rational {
        Rational { n, d }
    }

    #[test]
    fn shutter_speeds_read_like_on_a_camera() {
        let at = |n, d| PhotoInfo { exposure_time: Some((n, d)), ..PhotoInfo::default() }.shutter_text();
        assert_eq!(at(1, 500).as_deref(), Some("1/500 s"));
        assert_eq!(at(10, 5000).as_deref(), Some("1/500 s"));
        assert_eq!(at(2, 1).as_deref(), Some("2 s"));
        assert_eq!(at(8, 10).as_deref(), Some("0.8 s"));
        assert_eq!(at(0, 1), None);
    }

    #[test]
    fn exposure_date_and_gps_read_naturally() {
        let info = PhotoInfo {
            exposure_time: Some((1, 500)),
            aperture: Some(8.0),
            iso: Some(200),
            captured: Some("2026:08:14 18:42:07".into()),
            gps: Some((45.986, -9.258)),
            ..PhotoInfo::default()
        };
        assert_eq!(info.exposure_text().as_deref(), Some("1/500 s \u{b7} f/8 \u{b7} ISO 200"));
        assert_eq!(info.captured_text().as_deref(), Some("2026-08-14 18:42"));
        assert_eq!(info.gps_text().as_deref(), Some("45.9860 N \u{b7} 9.2580 W"));
        assert_eq!(PhotoInfo::default().exposure_text(), None);
    }

    #[test]
    fn gps_degrees_minutes_seconds_become_signed_degrees() {
        let v = degrees([r(45, 1), r(30, 1), r(36, 1)], Some("S")).unwrap_or_default();
        assert!((v + 45.51).abs() < 1e-9);
        assert_eq!(degrees([r(1, 0), r(0, 1), r(0, 1)], Some("N")), None);
    }

    #[test]
    fn decimals_are_trimmed() {
        assert_eq!(trim_decimal(8.0), "8");
        assert_eq!(trim_decimal(2.8), "2.8");
        assert_eq!(trim_decimal(1.25), "1.25");
    }
}
