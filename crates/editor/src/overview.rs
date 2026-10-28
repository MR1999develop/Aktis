//! The viewer while several photos are selected, as in Capture One's multi
//! view: the photos fill the viewer in a grid whose shape gives each the
//! largest possible size; past [`MAX_TILES`] the last tile reads "+N more".

/// Most photos shown at once.
pub const MAX_TILES: usize = 24;
/// Space between tiles and around the grid.
pub const GAP: f32 = 6.0;
/// Shape assumed for the photos when choosing the grid (3:2).
pub const PHOTO_ASPECT: f32 = 1.5;

/// Where the tiles go in a viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewLayout {
    /// Tiles per row.
    pub columns: usize,
    /// Size of a tile.
    pub tile: [f32; 2],
    /// Top-left corner of the first tile, from the viewer's corner.
    pub origin: [f32; 2],
    /// Tiles in the grid.
    pub slots: usize,
    /// Photos with a tile.
    pub shown: usize,
    /// Photos left out; when above zero the last slot reads "+N more".
    pub more: usize,
}

impl OverviewLayout {
    /// Lays out `count` photos in a viewer of `size` points, filling it.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // small counts
    pub fn new(size: [f32; 2], count: usize) -> Self {
        let (w, h) = (size[0].max(1.0), size[1].max(1.0));
        let slots = count.clamp(1, MAX_TILES);
        let (shown, more) = if count > MAX_TILES { (MAX_TILES - 1, count - (MAX_TILES - 1)) } else { (count, 0) };
        // The column count that gives each photo the largest area.
        let mut best = (1, 0.0_f32);
        for columns in 1..=slots {
            let rows = slots.div_ceil(columns);
            let cw = (w - GAP * (columns as f32 + 1.0)) / columns as f32;
            let ch = (h - GAP * (rows as f32 + 1.0)) / rows as f32;
            if cw <= 0.0 || ch <= 0.0 {
                continue;
            }
            let photo_w = cw.min(ch * PHOTO_ASPECT);
            let area = photo_w * photo_w / PHOTO_ASPECT;
            if area > best.1 {
                best = (columns, area);
            }
        }
        let columns = best.0;
        let rows = slots.div_ceil(columns);
        let tile = [
            ((w - GAP * (columns as f32 + 1.0)) / columns as f32).max(1.0),
            ((h - GAP * (rows as f32 + 1.0)) / rows as f32).max(1.0),
        ];
        Self { columns, tile, origin: [GAP, GAP], slots, shown, more }
    }

    /// Whether the last slot shows "+N more".
    #[must_use]
    pub fn has_more_tile(&self) -> bool {
        self.more > 0
    }

    /// Top-left corner of slot `i`, from the viewer's corner. The last row,
    /// when not full, is centred.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // small counts
    pub fn slot(&self, i: usize) -> [f32; 2] {
        let (row, column) = (i / self.columns, i % self.columns);
        let rows = self.slots.div_ceil(self.columns);
        let in_row = if row + 1 == rows { self.slots - row * self.columns } else { self.columns };
        let shift = (self.columns - in_row) as f32 * (self.tile[0] + GAP) / 2.0;
        [
            self.origin[0] + shift + column as f32 * (self.tile[0] + GAP),
            self.origin[1] + row as f32 * (self.tile[1] + GAP),
        ]
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn two_photos_side_by_side_fill_a_wide_viewer() {
        let l = OverviewLayout::new([1600.0, 900.0], 2);
        assert_eq!((l.columns, l.shown, l.more), (2, 2, 0));
        assert_eq!(l.tile, [(1600.0 - 3.0 * GAP) / 2.0, 900.0 - 2.0 * GAP]);
        assert_eq!(l.slot(1)[0], GAP + l.tile[0] + GAP);
    }

    #[test]
    fn four_photos_make_a_two_by_two_grid_and_a_tall_viewer_stacks() {
        assert_eq!(OverviewLayout::new([1600.0, 1000.0], 4).columns, 2);
        assert_eq!(OverviewLayout::new([500.0, 1400.0], 3).columns, 1);
    }

    #[test]
    fn an_incomplete_last_row_is_centred() {
        let l = OverviewLayout::new([1600.0, 1000.0], 5);
        assert_eq!(l.columns, 3);
        let last = l.slot(4);
        let full_row_middle = l.slot(1)[0] + l.tile[0] / 2.0;
        // Two tiles in the last row, centred on the full row's middle.
        assert!((last[0] - (full_row_middle + GAP / 2.0)).abs() < 1e-3, "{last:?}");
    }

    #[test]
    fn too_many_photos_end_with_a_more_tile() {
        let l = OverviewLayout::new([1600.0, 1000.0], 40);
        assert_eq!((l.slots, l.shown, l.more), (MAX_TILES, MAX_TILES - 1, 40 - (MAX_TILES - 1)));
        assert!(l.has_more_tile());
    }
}
