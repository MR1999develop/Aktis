//! The viewer while several photos are selected: a grid of tiles, one per
//! selected photo, as in a photo viewer (tiles 2.5 times a filmstrip
//! thumbnail when there is room, a "+N more" tile when they do not all fit).

/// Size of a tile when the viewer has room for it.
pub const TILE: [f32; 2] = [360.0, 240.0];
/// Space between tiles.
pub const GAP: f32 = 12.0;

/// Where the tiles go in a viewer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverviewLayout {
    /// Tiles per row.
    pub columns: usize,
    /// Size of a tile.
    pub tile: [f32; 2],
    /// Top-left corner of the first tile, from the viewer's corner.
    pub origin: [f32; 2],
    /// Tiles that fit in the viewer.
    pub slots: usize,
    /// Photos with a tile.
    pub shown: usize,
    /// Photos left out; when above zero the slot after the last tile
    /// reads "+N more" (if the viewer has one: see [`Self::has_more_tile`]).
    pub more: usize,
}

impl OverviewLayout {
    /// Lays out `count` tiles in a viewer of `size` points.
    #[must_use]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small counts
    pub fn new(size: [f32; 2], count: usize) -> Self {
        let (cw, ch) = (size[0].max(320.0), size[1].max(240.0));
        let w = TILE[0].min((cw - 24.0).max(180.0));
        let h = (w * TILE[1] / TILE[0]).min((ch - 24.0).max(122.0));
        let columns = (((cw - GAP) / (w + GAP)).floor() as usize).max(1);
        let rows = (((ch - GAP) / (h + GAP)).floor() as usize).max(1);
        let slots = columns * rows;
        let (shown, more) = match slots {
            _ if count <= slots => (count, 0),
            // No room for a "+N more" tile: one photo, the count is in the header.
            1 => (1, count - 1),
            _ => (slots - 1, count - (slots - 1)),
        };
        let used = if more > 0 { slots } else { shown.max(1) };
        let grid_columns = columns.min(used);
        let grid_rows = used.div_ceil(columns).max(1);
        let grid_w = grid_columns as f32 * w + grid_columns.saturating_sub(1) as f32 * GAP;
        let grid_h = grid_rows as f32 * h + grid_rows.saturating_sub(1) as f32 * GAP;
        let origin = [((cw - grid_w) * 0.5).max(6.0), ((ch - grid_h) * 0.5).max(6.0)];
        Self { columns, tile: [w, h], origin, slots, shown, more }
    }

    /// Whether the slot after the last photo shows "+N more".
    #[must_use]
    pub fn has_more_tile(&self) -> bool {
        self.more > 0 && self.slots > 1
    }

    /// Top-left corner of slot `i`, from the viewer's corner.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // small counts
    pub fn slot(&self, i: usize) -> [f32; 2] {
        let (row, column) = (i / self.columns, i % self.columns);
        [self.origin[0] + column as f32 * (self.tile[0] + GAP), self.origin[1] + row as f32 * (self.tile[1] + GAP)]
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn a_few_photos_get_full_size_tiles_centred() {
        let l = OverviewLayout::new([1200.0, 800.0], 2);
        assert_eq!(l.tile, TILE);
        assert_eq!(l.columns, 3);
        assert_eq!((l.shown, l.more), (2, 0));
        // Two tiles and a gap, centred.
        assert_eq!(l.origin[0], (1200.0 - (2.0 * TILE[0] + GAP)) / 2.0);
        assert_eq!(l.slot(1)[0], l.origin[0] + TILE[0] + GAP);
    }

    #[test]
    fn too_many_photos_end_with_a_more_tile() {
        let l = OverviewLayout::new([1200.0, 800.0], 40);
        // 3 columns × 3 rows: 8 photos and "+32 more".
        assert_eq!((l.columns, l.shown, l.more), (3, 8, 32));
        assert!(l.has_more_tile());
        assert_eq!(l.slot(3), [l.origin[0], l.origin[1] + TILE[1] + GAP]);
    }

    #[test]
    fn small_viewers_shrink_the_tiles() {
        let l = OverviewLayout::new([300.0, 200.0], 3);
        assert!(l.tile[0] < TILE[0] && l.tile[1] < TILE[1]);
        assert_eq!(l.columns, 1);
        assert_eq!((l.shown, l.more), (1, 2));
        assert!(!l.has_more_tile());
    }
}
