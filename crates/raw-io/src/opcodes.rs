//! DNG opcode lists of the raw image.
//!
//! Phone cameras (Google Pixel and others) store their lens shading as
//! GainMap opcodes in OpcodeList2; without them the corners are dark and
//! the colours shift towards them. The decoder does not apply opcodes, so
//! GainMap is applied here, to the normalised mosaic. Every other opcode is
//! skipped; the ones the file marks as required are reported in a note.

use std::io::{Read, Seek};

use crate::tiff::{Ifd, PHOTOMETRIC_CFA, PHOTOMETRIC_LINEAR_RAW, TiffReader, tag};

/// Opcode flag: a reader may skip the opcode (for example for a preview).
const FLAG_OPTIONAL: u32 = 1;
/// Opcode id of GainMap.
const GAIN_MAP: u32 = 9;

/// What the opcode lists of a DNG file ask for, as far as this crate goes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DngOpcodes {
    /// GainMap opcodes of OpcodeList2, in file order.
    pub(crate) gain_maps: Vec<GainMap>,
    /// Opcodes not applied although the file requires them, and lists that could not be read.
    pub(crate) notes: Vec<String>,
}

/// One opcode as stored in a list.
#[derive(Debug, Clone, PartialEq)]
struct Opcode {
    id: u32,
    flags: u32,
    params: Vec<u8>,
}

impl Opcode {
    fn optional(&self) -> bool {
        self.flags & FLAG_OPTIONAL != 0
    }
}

/// Name of an opcode for notes, from the DNG specification.
fn opcode_name(id: u32) -> String {
    let name = match id {
        1 => "WarpRectilinear",
        2 => "WarpFisheye",
        3 => "FixVignetteRadial",
        4 => "FixBadPixelsConstant",
        5 => "FixBadPixelsList",
        6 => "TrimBounds",
        7 => "MapTable",
        8 => "MapPolynomial",
        9 => "GainMap",
        10 => "DeltaPerRow",
        11 => "DeltaPerColumn",
        12 => "ScalePerRow",
        13 => "ScalePerColumn",
        14 => "WarpRectilinear2",
        _ => return format!("number {id}"),
    };
    name.to_owned()
}

/// Reads big-endian numbers from an opcode list (always big-endian, whatever the file's byte order).
struct BigEndian<'a> {
    bytes: &'a [u8],
}

impl<'a> BigEndian<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.bytes.len() {
            return None;
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)?.try_into().ok().map(u32::from_be_bytes)
    }

    fn f32(&mut self) -> Option<f32> {
        self.take(4)?.try_into().ok().map(f32::from_be_bytes)
    }

    fn f64(&mut self) -> Option<f64> {
        self.take(8)?.try_into().ok().map(f64::from_be_bytes)
    }
}

/// The opcodes of a list: count, then for each id, version, flags, size and parameters.
fn parse_list(bytes: &[u8]) -> Option<Vec<Opcode>> {
    let mut r = BigEndian { bytes };
    let count = r.u32()?;
    let mut out = Vec::new();
    for _ in 0..count {
        let id = r.u32()?;
        let _version = r.u32()?;
        let flags = r.u32()?;
        let size = usize::try_from(r.u32()?).ok()?;
        out.push(Opcode { id, flags, params: r.take(size)?.to_vec() });
    }
    Some(out)
}

/// A GainMap opcode: a grid of gains, interpolated bilinearly, multiplying
/// some planes of the pixels of an area (every `row_pitch`-th row and
/// `col_pitch`-th column from its top-left corner). Coordinates are in the
/// image after the active-area crop.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GainMap {
    top: u32,
    left: u32,
    bottom: u32,
    right: u32,
    plane: u32,
    planes: u32,
    row_pitch: u32,
    col_pitch: u32,
    points_v: u32,
    points_h: u32,
    spacing_v: f64,
    spacing_h: f64,
    origin_v: f64,
    origin_h: f64,
    map_planes: u32,
    gains: Vec<f32>,
}

/// Where the decoded pixels sit in the image the opcode coordinates refer
/// to (the raw image cropped to its active area).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Column of the first decoded pixel in that image.
    pub(crate) x: usize,
    /// Row of the first decoded pixel in that image.
    pub(crate) y: usize,
    /// Width of that image.
    pub(crate) width: usize,
    /// Height of that image.
    pub(crate) height: usize,
}

impl GainMap {
    /// Parses and checks the parameters; `None` when malformed.
    fn parse(params: &[u8]) -> Option<Self> {
        let mut r = BigEndian { bytes: params };
        let mut map = Self {
            top: r.u32()?,
            left: r.u32()?,
            bottom: r.u32()?,
            right: r.u32()?,
            plane: r.u32()?,
            planes: r.u32()?,
            row_pitch: r.u32()?,
            col_pitch: r.u32()?,
            points_v: r.u32()?,
            points_h: r.u32()?,
            spacing_v: r.f64()?,
            spacing_h: r.f64()?,
            origin_v: r.f64()?,
            origin_h: r.f64()?,
            map_planes: r.u32()?,
            gains: Vec::new(),
        };
        let count = usize::try_from(map.points_v)
            .ok()?
            .checked_mul(usize::try_from(map.points_h).ok()?)?
            .checked_mul(usize::try_from(map.map_planes).ok()?)?;
        // Every gain is 4 bytes: refuse a count the parameters cannot hold before allocating.
        if count == 0 || r.bytes.len() != count.checked_mul(4)? {
            return None;
        }
        map.gains = (0..count).map(|_| r.f32()).collect::<Option<Vec<f32>>>()?;
        let axis_ok = |points: u32, spacing: f64, origin: f64| {
            points == 1 || (spacing.is_finite() && spacing > 0.0 && origin.is_finite())
        };
        let ok = map.planes >= 1
            && map.row_pitch >= 1
            && map.col_pitch >= 1
            && axis_ok(map.points_v, map.spacing_v, map.origin_v)
            && axis_ok(map.points_h, map.spacing_h, map.origin_h)
            && map.gains.iter().all(|g| g.is_finite());
        ok.then_some(map)
    }

    /// Gain of map point (`v`, `h`) in map plane `plane`.
    fn gain(&self, v: usize, h: usize, plane: usize) -> f32 {
        self.gains[(v * self.points_h as usize + h) * self.map_planes as usize + plane]
    }

    /// Multiplies the pixels of `data` (`width × height` pixels of `cpp`
    /// values, normalised so white is 1) placed at `at` by the map.
    /// Results are clipped at white, as the DNG SDK does, so photosites
    /// saturated in the file stay saturated; values below black are kept.
    pub(crate) fn apply(&self, data: &mut [f32], width: usize, height: usize, cpp: usize, at: Placement) {
        if width == 0 || cpp == 0 {
            return;
        }
        // An empty area means the whole image.
        let (top, left, bottom, right) = if self.top >= self.bottom || self.left >= self.right {
            (0, 0, at.height, at.width)
        } else {
            let edge = |v: u32| usize::try_from(v).unwrap_or(usize::MAX);
            (edge(self.top), edge(self.left), edge(self.bottom).min(at.height), edge(self.right).min(at.width))
        };
        let first_plane = (self.plane as usize).min(cpp);
        let end_plane = (self.plane as usize).saturating_add(self.planes as usize).min(cpp);
        let (row_pitch, col_pitch) = (self.row_pitch as usize, self.col_pitch as usize);
        let selected = |pos: usize, start: usize, end: usize, pitch: usize| {
            pos >= start && pos < end && (pos - start).is_multiple_of(pitch)
        };

        // Map position of each decoded column, or None when the column is not touched.
        let columns: Vec<Option<(usize, usize, f32)>> = (0..width)
            .map(|x| {
                let sx = at.x + x;
                selected(sx, left, right, col_pitch)
                    .then(|| map_position(sx, at.width, self.points_h, self.spacing_h, self.origin_h))
            })
            .collect();
        let mut row_gains = vec![0.0_f32; self.points_h as usize];
        for (y, row) in data.chunks_exact_mut(width * cpp).take(height).enumerate() {
            let sy = at.y + y;
            if !selected(sy, top, bottom, row_pitch) {
                continue;
            }
            let (v1, v2, fv) = map_position(sy, at.height, self.points_v, self.spacing_v, self.origin_v);
            for plane in first_plane..end_plane {
                // With more planes than map planes, the last map plane serves the rest.
                let map_plane = (plane - first_plane).min(self.map_planes as usize - 1);
                for (h, g) in row_gains.iter_mut().enumerate() {
                    *g = lerp(self.gain(v1, h, map_plane), self.gain(v2, h, map_plane), fv);
                }
                for (pixel, column) in row.chunks_exact_mut(cpp).zip(&columns) {
                    if let Some((h1, h2, fh)) = *column {
                        let v = &mut pixel[plane];
                        *v = (*v * lerp(row_gains[h1], row_gains[h2], fh)).min(1.0);
                    }
                }
            }
        }
    }
}

/// The two map points around pixel `pos` of an axis `size` pixels long, and
/// the weight of the second. Pixels are taken at their centres, relative to
/// the image size; positions outside the map use its edge.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)] // pixel positions; index is checked positive and below `last`
fn map_position(pos: usize, size: usize, points: u32, spacing: f64, origin: f64) -> (usize, usize, f32) {
    let last = points.saturating_sub(1) as usize;
    if last == 0 || size == 0 {
        return (0, 0, 0.0);
    }
    let index = ((pos as f64 + 0.5) / size as f64 - origin) / spacing;
    if index.is_nan() || index <= 0.0 {
        (0, 0, 0.0)
    } else if index >= last as f64 {
        (last, last, 0.0)
    } else {
        let i = index as usize;
        (i, i + 1, (index - i as f64) as f32)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The IFD of the main raw image: the first with NewSubFileType 0 holding a
/// mosaic or linear raw data, else the first with NewSubFileType 0.
fn raw_ifd<R: Read + Seek>(tiff: &mut TiffReader<R>, ifds: &[Ifd]) -> Option<usize> {
    let main: Vec<usize> =
        (0..ifds.len()).filter(|i| tiff.first_unsigned(&ifds[*i], tag::NEW_SUBFILE_TYPE).unwrap_or(0) == 0).collect();
    main.iter()
        .copied()
        .find(|i| {
            matches!(tiff.first_unsigned(&ifds[*i], tag::PHOTOMETRIC), Some(PHOTOMETRIC_CFA | PHOTOMETRIC_LINEAR_RAW))
        })
        .or_else(|| main.first().copied())
}

/// Reads the opcode lists of the raw image of a DNG file. Anything that is
/// not a DNG file gives nothing.
pub(crate) fn read_opcodes<R: Read + Seek>(reader: R) -> DngOpcodes {
    let mut out = DngOpcodes::default();
    let Some(mut tiff) = TiffReader::new(reader) else { return out };
    let ifds = tiff.ifds();
    if ifds.first().and_then(|root| root.get(tag::DNG_VERSION)).is_none() {
        return out;
    }
    let Some(raw) = raw_ifd(&mut tiff, &ifds) else { return out };
    for (list, number) in [(tag::OPCODE_LIST1, 1), (tag::OPCODE_LIST2, 2), (tag::OPCODE_LIST3, 3)] {
        let Some(entry) = ifds[raw].get(list).copied() else { continue };
        let Some(opcodes) = tiff.value_bytes(&entry).and_then(|b| parse_list(&b)) else {
            out.notes.push(format!("DNG OpcodeList{number} could not be read and was ignored"));
            continue;
        };
        for op in opcodes {
            if number == 2 && op.id == GAIN_MAP {
                match GainMap::parse(&op.params) {
                    Some(map) => out.gain_maps.push(map),
                    None if !op.optional() => {
                        out.notes.push("a DNG GainMap (lens shading) is damaged and was not applied".to_owned());
                    }
                    None => {}
                }
            } else if !op.optional() {
                out.notes.push(format!(
                    "DNG opcode {} (OpcodeList{number}) is not supported and was not applied",
                    opcode_name(op.id)
                ));
            }
        }
    }
    // One note per kind is enough (phones store one GainMap per Bayer position).
    let mut seen = std::collections::HashSet::new();
    out.notes.retain(|n| seen.insert(n.clone()));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)] // exact binary fractions in these small cases
pub(crate) mod tests {
    use super::*;
    use crate::tiff::tests::{TestIfd, Value, build};
    use std::io::Cursor;

    /// Area (top, left, bottom, right, plane, planes, row pitch, col pitch).
    type Area = [u32; 8];

    pub(crate) fn gain_map_params(
        area: Area,
        points: (u32, u32),
        spacing: (f64, f64),
        origin: (f64, f64),
        gains: &[f32],
        map_planes: u32,
    ) -> Vec<u8> {
        let mut p = Vec::new();
        for v in area {
            p.extend_from_slice(&v.to_be_bytes());
        }
        p.extend_from_slice(&points.0.to_be_bytes());
        p.extend_from_slice(&points.1.to_be_bytes());
        for v in [spacing.0, spacing.1, origin.0, origin.1] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        p.extend_from_slice(&map_planes.to_be_bytes());
        for g in gains {
            p.extend_from_slice(&g.to_be_bytes());
        }
        p
    }

    /// An opcode list of (id, flags, params).
    pub(crate) fn list(ops: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&u32::try_from(ops.len()).unwrap().to_be_bytes());
        for (id, flags, params) in ops {
            for v in [*id, 0x0103_0000, *flags, u32::try_from(params.len()).unwrap()] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.extend_from_slice(params);
        }
        b
    }

    fn whole(w: usize, h: usize) -> Placement {
        Placement { x: 0, y: 0, width: w, height: h }
    }

    /// 2×2 map with points at 0.25 and 0.75 of each axis.
    const MAP_2X2: [f32; 4] = [1.0, 2.0, 1.5, 1.0];

    /// The gains of [`MAP_2X2`] at the pixel centres of a 4×4 image
    /// (0.125, 0.375, 0.625, 0.875 → map positions 0, 0.25, 0.75, 1).
    const GAINS_4X4: [[f32; 4]; 4] = [
        [1.0, 1.25, 1.75, 2.0],
        [1.125, 1.28125, 1.59375, 1.75],
        [1.375, 1.34375, 1.28125, 1.25],
        [1.5, 1.375, 1.125, 1.0],
    ];

    fn map_2x2(area: Area) -> GainMap {
        GainMap::parse(&gain_map_params(area, (2, 2), (0.5, 0.5), (0.25, 0.25), &MAP_2X2, 1)).unwrap()
    }

    #[test]
    fn gains_are_interpolated_bilinearly_and_clamped_at_the_map_edges() {
        let mut data = vec![0.25_f32; 16];
        map_2x2([0, 0, 4, 4, 0, 1, 1, 1]).apply(&mut data, 4, 4, 1, whole(4, 4));
        for (i, v) in data.iter().enumerate() {
            assert_eq!(*v, 0.25 * GAINS_4X4[i / 4][i % 4], "pixel {i}");
        }
    }

    #[test]
    fn row_and_column_pitch_select_one_bayer_position() {
        // Top 1, left 0, pitch 2: odd rows, even columns (the B of an RGGB mosaic is the other way round).
        let map = GainMap::parse(&gain_map_params([1, 0, 4, 4, 0, 1, 2, 2], (1, 1), (1.0, 1.0), (0.0, 0.0), &[2.0], 1));
        let mut data = vec![0.25_f32; 16];
        map.unwrap().apply(&mut data, 4, 4, 1, whole(4, 4));
        for (i, v) in data.iter().enumerate() {
            let (row, col) = (i / 4, i % 4);
            let want = if row % 2 == 1 && col % 2 == 0 { 0.5 } else { 0.25 };
            assert_eq!(*v, want, "row {row}, col {col}");
        }
    }

    #[test]
    fn coordinates_account_for_the_crop_of_the_decoded_area() {
        // The image of the opcodes is 6×6; raw-io kept the 4×4 starting at (1, 1).
        let at = Placement { x: 1, y: 1, width: 6, height: 6 };
        let map = GainMap::parse(&gain_map_params([0, 0, 6, 6, 0, 1, 2, 2], (1, 1), (1.0, 1.0), (0.0, 0.0), &[2.0], 1));
        let mut data = vec![0.25_f32; 16];
        map.unwrap().apply(&mut data, 4, 4, 1, at);
        for (i, v) in data.iter().enumerate() {
            // Even rows and columns of the 6×6 image are odd ones of the crop.
            let (row, col) = (i / 4, i % 4);
            let want = if row % 2 == 1 && col % 2 == 1 { 0.5 } else { 0.25 };
            assert_eq!(*v, want, "row {row}, col {col}");
        }

        // The map spans the whole 6×6 image, not the crop: a 4×4 image placed
        // at (1, 1) of a 6×6 sees the gains of an 6×6 image there.
        let mut cropped = vec![0.5_f32; 16];
        map_2x2([0, 0, 0, 0, 0, 1, 1, 1]).apply(&mut cropped, 4, 4, 1, at);
        let mut full = vec![0.5_f32; 36];
        map_2x2([0, 0, 0, 0, 0, 1, 1, 1]).apply(&mut full, 6, 6, 1, whole(6, 6));
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(cropped[y * 4 + x], full[(y + 1) * 6 + x + 1], "{x},{y}");
            }
        }
    }

    #[test]
    fn planes_use_their_own_map_plane_and_the_last_one_for_the_rest() {
        let one = |plane: u32, planes: u32, gains: &[f32], map_planes| {
            let mut data = vec![0.25_f32; 3];
            let p =
                gain_map_params([0, 0, 1, 1, plane, planes, 1, 1], (1, 1), (1.0, 1.0), (0.0, 0.0), gains, map_planes);
            GainMap::parse(&p).unwrap().apply(&mut data, 1, 1, 3, whole(1, 1));
            data
        };
        assert_eq!(one(0, 3, &[2.0], 1), [0.5, 0.5, 0.5]);
        assert_eq!(one(0, 3, &[2.0, 3.0], 2), [0.5, 0.75, 0.75]);
        assert_eq!(one(1, 2, &[2.0, 3.0], 2), [0.25, 0.5, 0.75]);
    }

    #[test]
    fn results_clip_at_white_but_keep_noise_below_black() {
        let p = gain_map_params([0, 0, 1, 3, 0, 1, 1, 1], (1, 1), (1.0, 1.0), (0.0, 0.0), &[1.5], 1);
        let mut data = vec![0.9, -0.02, 0.5];
        GainMap::parse(&p).unwrap().apply(&mut data, 3, 1, 1, whole(3, 1));
        assert_eq!(data, [1.0, -0.03, 0.75]);
    }

    #[test]
    fn malformed_gain_maps_are_refused() {
        let area = [0, 0, 4, 4, 0, 1, 1, 1];
        assert!(GainMap::parse(&gain_map_params(area, (0, 1), (1.0, 1.0), (0.0, 0.0), &[], 1)).is_none());
        assert!(GainMap::parse(&gain_map_params(area, (2, 1), (0.0, 1.0), (0.0, 0.0), &[1.0, 1.0], 1)).is_none());
        assert!(GainMap::parse(&gain_map_params(area, (1, 1), (1.0, 1.0), (0.0, 0.0), &[1.0, 1.0], 1)).is_none());
        assert!(GainMap::parse(&gain_map_params(area, (1, 1), (1.0, 1.0), (0.0, 0.0), &[f32::NAN], 1)).is_none());
        assert!(
            GainMap::parse(&gain_map_params([0, 0, 4, 4, 0, 1, 0, 1], (1, 1), (1.0, 1.0), (0.0, 0.0), &[1.0], 1))
                .is_none()
        );
        // A huge declared map with no data must not allocate it.
        assert!(GainMap::parse(&gain_map_params(area, (u32::MAX, u32::MAX), (1.0, 1.0), (0.0, 0.0), &[], 1)).is_none());
        // Spacing and origin do not matter for a single point.
        assert!(GainMap::parse(&gain_map_params(area, (1, 1), (0.0, 0.0), (f64::NAN, 0.0), &[1.0], 1)).is_some());
    }

    #[test]
    fn opcode_lists_parse_and_truncated_lists_are_refused() {
        let bytes = list(&[(9, 0, vec![1, 2, 3]), (1, 1, vec![])]);
        let ops = parse_list(&bytes).unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!((ops[0].id, ops[0].params.as_slice(), ops[0].optional()), (9, [1_u8, 2, 3].as_slice(), false));
        assert!(ops[1].optional());
        for cut in 0..bytes.len() {
            assert!(parse_list(&bytes[..cut]).is_none(), "cut at {cut}");
        }
    }

    /// A DNG-like file: IFD0 a preview with DNGVersion, its SubIFD the raw image with `tags`.
    pub(crate) fn dng(big_endian: bool, tags: Vec<(u16, Value)>) -> Vec<u8> {
        let mut raw =
            vec![(tag::NEW_SUBFILE_TYPE, Value::Long(vec![0])), (tag::PHOTOMETRIC, Value::Short(vec![32803]))];
        raw.extend(tags);
        build(
            big_endian,
            &[
                TestIfd {
                    entries: vec![
                        (tag::NEW_SUBFILE_TYPE, Value::Long(vec![1])),
                        (tag::DNG_VERSION, Value::Undefined(vec![1, 4, 0, 0])),
                    ],
                    sub_ifds: vec![1],
                },
                TestIfd { entries: raw, sub_ifds: vec![] },
            ],
        )
    }

    #[test]
    fn gain_maps_of_the_raw_ifd_are_read_and_other_opcodes_reported() {
        let gain = gain_map_params([0, 0, 0, 0, 0, 1, 1, 1], (2, 2), (0.5, 0.5), (0.25, 0.25), &MAP_2X2, 1);
        let list2 = list(&[(9, 0, gain.clone()), (9, 0, gain.clone()), (99, 1, vec![0; 8]), (11, 0, vec![0; 4])]);
        let list3 = list(&[(1, 1, vec![0; 16]), (3, 0, vec![0; 8])]);
        for big_endian in [false, true] {
            let file = dng(
                big_endian,
                vec![
                    (tag::OPCODE_LIST2, Value::Undefined(list2.clone())),
                    (tag::OPCODE_LIST3, Value::Undefined(list3.clone())),
                ],
            );
            let found = read_opcodes(Cursor::new(file));
            assert_eq!(found.gain_maps.len(), 2);
            assert_eq!(found.gain_maps[0], GainMap::parse(&gain).unwrap());
            assert_eq!(
                found.notes,
                [
                    "DNG opcode DeltaPerColumn (OpcodeList2) is not supported and was not applied",
                    "DNG opcode FixVignetteRadial (OpcodeList3) is not supported and was not applied",
                ]
            );
        }
    }

    #[test]
    fn opcode_list_1_gain_maps_and_damaged_lists_are_reported_not_applied() {
        let gain = gain_map_params([0, 0, 0, 0, 0, 1, 1, 1], (1, 1), (1.0, 1.0), (0.0, 0.0), &[2.0], 1);
        let file = dng(
            false,
            vec![
                (tag::OPCODE_LIST1, Value::Undefined(list(&[(9, 0, gain)]))),
                (tag::OPCODE_LIST2, Value::Undefined(vec![0, 0, 0, 5, 1, 2])),
            ],
        );
        let found = read_opcodes(Cursor::new(file));
        assert!(found.gain_maps.is_empty());
        assert_eq!(
            found.notes,
            [
                "DNG opcode GainMap (OpcodeList1) is not supported and was not applied",
                "DNG OpcodeList2 could not be read and was ignored",
            ]
        );
    }

    #[test]
    fn files_that_are_not_dng_give_nothing() {
        let gain = gain_map_params([0, 0, 0, 0, 0, 1, 1, 1], (1, 1), (1.0, 1.0), (0.0, 0.0), &[2.0], 1);
        // A TIFF raw file (NEF, ARW …) without DNGVersion: its tags are not DNG tags.
        let plain = build(
            false,
            &[TestIfd {
                entries: vec![(tag::OPCODE_LIST2, Value::Undefined(list(&[(9, 0, gain)])))],
                sub_ifds: vec![],
            }],
        );
        assert_eq!(read_opcodes(Cursor::new(plain)), DngOpcodes::default());
        assert_eq!(read_opcodes(Cursor::new(b"FUJIFILMCCD-RAW".to_vec())), DngOpcodes::default());
    }
}
