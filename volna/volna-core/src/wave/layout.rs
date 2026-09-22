//! Pixel layout of the wave panel for one frame: the three columns, the
//! visible row range, the scrollbar, and the small interactive rectangles
//! (format badges, marker chips, column dividers). Pure: computed from the
//! panel bounds and model state, then read by both the painter and the input
//! handlers so a click lands on exactly what was drawn.

use std::ops::Range;
use std::sync::Arc;

use crate::document::Marker;
use crate::geometry::{Point, Rect, point, size};
use crate::wave::model::RowHeight;
use crate::wave::overlay::marker_chips;
use crate::wave::viewport::Viewport;

// Design-time sizes in logical pixels at zoom 1.0; the layout multiplies
// them by [`LayoutInput::zoom`] so every rectangle scales with the interface.
pub const MIN_COLUMN: f32 = 72.0;
const SPLITTER_TOLERANCE: f32 = 4.0;
pub const SCROLLBAR_W: f32 = 10.0;
const BADGE_W: f32 = 36.0;

#[derive(Clone, Debug, Default)]
pub struct WaveLayout {
    pub bounds: Rect,
    pub header: Rect,
    pub names: Rect,
    pub values: Rect,
    pub waves: Rect,
    pub rows: Range<usize>,
    /// Height of a default (1×) row; taller rows are whole multiples.
    pub row_h: f32,
    /// Row tops in multiples of `row_h`, with the content height last.
    tops: Arc<[u32]>,
    /// The interface zoom this layout was computed at; input handlers use
    /// it to scale thresholds and to store column widths unzoomed.
    pub zoom: f32,
    pub scroll_y: f32,
    pub max_scroll: f32,
    /// Track and thumb, when the rows overflow.
    pub scrollbar: Option<(Rect, Rect)>,
    /// Format badges of the visible rows, right-aligned in the values column.
    pub badges: Vec<(usize, Rect)>,
    /// Marker chips in the header, for markers inside the viewport.
    pub marker_chips: Vec<(usize, Rect)>,
    /// Grab zones of the two column dividers.
    pub names_split: Rect,
    pub values_split: Rect,
}

pub struct LayoutInput<'a> {
    pub bounds: Rect,
    /// Already zoomed (the theme's `row_height`).
    pub row_h: f32,
    /// Already zoomed (the theme's `timeline_height`).
    pub header_h: f32,
    /// Interface zoom; column widths below are design sizes it multiplies.
    pub zoom: f32,
    pub names_width: f32,
    pub values_width: f32,
    /// From [`row_tops`]: one entry per row plus the content height.
    pub row_tops: Arc<[u32]>,
    pub scroll_y: f32,
    pub markers: &'a [Marker],
    pub viewport: Viewport,
}

/// Prefix sums of row height multiples: row `i` spans `tops[i]..tops[i + 1]`.
pub fn row_tops(heights: impl IntoIterator<Item = RowHeight>) -> Arc<[u32]> {
    let mut top = 0u32;
    std::iter::once(0)
        .chain(heights.into_iter().map(|h| {
            top += u32::from(h.multiple());
            top
        }))
        .collect()
}

/// Row containing unit offset `unit` (past the end, continuing at 1× steps).
fn row_at_unit(tops: &[u32], unit: f32) -> usize {
    let count = tops.len().saturating_sub(1);
    let total = tops.last().copied().unwrap_or(0);
    if unit >= total as f32 {
        return count + (unit - total as f32) as usize;
    }
    tops.partition_point(|top| *top as f32 <= unit)
        .saturating_sub(1)
}

impl WaveLayout {
    pub fn compute(input: LayoutInput<'_>) -> WaveLayout {
        let bounds = input.bounds;
        let row_h = input.row_h;
        let header_h = input.header_h;
        let zoom = if input.zoom.is_finite() && input.zoom > 0.0 {
            input.zoom
        } else {
            1.0
        };
        let z = |v: f32| v * zoom;
        let min_column = z(MIN_COLUMN);
        let total_w = bounds.width();
        let names_w = z(input.names_width).clamp(
            min_column,
            (total_w - 2.0 * min_column - z(160.0)).max(min_column),
        );
        let values_w =
            z(input.values_width).clamp(min_column, (total_w - names_w - z(160.0)).max(min_column));

        let header = Rect::new(bounds.origin, size(bounds.width(), header_h));
        let rows_top = bounds.top() + header_h;
        let rows_h = (bounds.height() - header_h).max(0.0);
        let names = Rect::new(point(bounds.left(), rows_top), size(names_w, rows_h));
        let values = Rect::new(point(names.right(), rows_top), size(values_w, rows_h));
        let waves_w = (bounds.width() - names_w - values_w).max(0.0);
        let waves = Rect::new(point(values.right(), rows_top), size(waves_w, rows_h));

        let tops = input.row_tops;
        let item_count = tops.len().saturating_sub(1);
        let content_h = row_h * tops.last().copied().unwrap_or(0) as f32;
        let max_scroll = (content_h - rows_h).max(0.0);
        let scroll_y = input.scroll_y.clamp(0.0, max_scroll);

        let (first, last) = if row_h > 0.0 && item_count > 0 {
            let bottom = (scroll_y + rows_h) / row_h;
            (
                row_at_unit(&tops, scroll_y / row_h),
                // The last row that starts above the bottom edge, exclusive.
                tops[..item_count].partition_point(|top| (*top as f32) < bottom),
            )
        } else {
            (0, 0)
        };
        let rows = first.min(last)..last;

        // Badges sit on the first line of a row, like its name and value.
        let mut badges = Vec::new();
        for ix in rows.clone() {
            let y = rows_top + row_h * tops[ix] as f32 - scroll_y;
            let b = Rect::new(
                point(values.right() - z(BADGE_W + 6.0), y + z(4.0)),
                size(z(BADGE_W), row_h - z(8.0)),
            );
            badges.push((ix, b));
        }

        let wave_wf = f64::from(waves_w).max(1.0);
        let marker_chips = marker_chips(
            header,
            waves.left(),
            wave_wf,
            input.viewport,
            input.markers,
            zoom,
        );

        let scrollbar = if max_scroll > 0.0 && rows_h > 0.0 {
            let track = Rect::new(
                point(waves.right() - z(SCROLLBAR_W), rows_top),
                size(z(SCROLLBAR_W), rows_h),
            );
            let ratio = rows_h / content_h;
            let thumb_h = (rows_h * ratio).max(z(24.0));
            let travel = rows_h - thumb_h;
            let top = rows_top + travel * (scroll_y / max_scroll).clamp(0.0, 1.0);
            let thumb = Rect::new(
                point(track.left() + z(2.0), top),
                size(z(SCROLLBAR_W - 4.0), thumb_h),
            );
            Some((track, thumb))
        } else {
            None
        };

        let split_zone = |x: f32| {
            Rect::new(
                point(x - z(SPLITTER_TOLERANCE), bounds.top()),
                size(z(2.0 * SPLITTER_TOLERANCE), bounds.height()),
            )
        };

        WaveLayout {
            bounds,
            header,
            names,
            values,
            waves,
            rows,
            row_h,
            tops,
            zoom,
            scroll_y,
            max_scroll,
            scrollbar,
            badges,
            marker_chips,
            names_split: split_zone(names.right()),
            values_split: split_zone(values.right()),
        }
    }

    /// Row index under `y` (may be past the last item; callers bound it).
    pub fn row_at(&self, y: f32) -> Option<usize> {
        if y < self.names.top() {
            return None;
        }
        if self.row_h <= 0.0 {
            return None;
        }
        let unit = (y - self.names.top() + self.scroll_y) / self.row_h;
        (unit >= 0.0).then(|| row_at_unit(&self.tops, unit))
    }

    /// Top of row `ix` in the panel's coordinate space.
    pub fn row_y(&self, ix: usize) -> f32 {
        let count = self.tops.len().saturating_sub(1);
        let total = self.tops.last().copied().unwrap_or(0);
        let unit = match self.tops.get(ix) {
            Some(top) => *top as f32,
            None => (total as usize + ix - count) as f32,
        };
        self.names.top() + self.row_h * unit - self.scroll_y
    }

    /// Height of row `ix`; rows past the end are 1×.
    pub fn row_height(&self, ix: usize) -> f32 {
        let units = match (self.tops.get(ix), self.tops.get(ix + 1)) {
            (Some(top), Some(bottom)) => bottom - top,
            _ => 1,
        };
        self.row_h * units as f32
    }

    pub fn badge_at(&self, p: Point) -> Option<(usize, Rect)> {
        self.badges.iter().copied().find(|(_, b)| b.contains(p))
    }

    pub fn chip_at(&self, p: Point) -> Option<usize> {
        self.marker_chips
            .iter()
            .find(|(_, b)| b.contains(p))
            .map(|(ix, _)| *ix)
    }

    pub fn near_split(&self, p: Point) -> bool {
        self.names_split.contains(p) || self.values_split.contains(p)
    }

    pub fn wave_width_f64(&self) -> f64 {
        f64::from(self.waves.width()).max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wave::overlay::CHIP_W;

    fn layout(zoom: f32, items: usize) -> WaveLayout {
        WaveLayout::compute(LayoutInput {
            bounds: Rect::from_xywh(0.0, 0.0, 2000.0, 300.0),
            row_h: 24.0 * zoom,
            header_h: 32.0 * zoom,
            zoom,
            names_width: 220.0,
            values_width: 120.0,
            row_tops: row_tops(vec![RowHeight::DEFAULT; items]),
            scroll_y: 0.0,
            markers: &[Marker {
                id: 1,
                time: 50,
                label: None,
            }],
            viewport: Viewport {
                start: 0.0,
                end: 100.0,
            },
        })
    }

    #[test]
    fn every_rectangle_scales_with_the_interface_zoom() {
        let base = layout(1.0, 100);
        for zoom in [0.5f32, 2.0] {
            let l = layout(zoom, 100);
            assert_eq!(l.zoom, zoom);
            assert_eq!(l.row_h, 24.0 * zoom);
            assert_eq!(l.header.height(), 32.0 * zoom);
            assert_eq!(l.names.width(), 220.0 * zoom);
            assert_eq!(l.values.width(), 120.0 * zoom);
            assert_eq!(l.names_split.width(), 8.0 * zoom);
            let (_, b) = l.badges[0];
            assert_eq!(b.width(), BADGE_W * zoom);
            assert_eq!(b.height(), (24.0 - 8.0) * zoom);
            let (_, chip) = l.marker_chips[0];
            assert_eq!(chip.width(), CHIP_W * zoom);
            assert_eq!(chip.height(), 14.0 * zoom);
            let (track, _) = l.scrollbar.unwrap();
            assert_eq!(track.width(), SCROLLBAR_W * zoom);
            // Rows per screen follow the row height.
            let visible = (300.0 - 32.0 * zoom) / (24.0 * zoom);
            assert_eq!(l.rows.len(), visible.ceil() as usize);
            assert_eq!(l.row_at(l.names.top() + 24.0 * zoom * 3.5), Some(3));
        }
        assert_eq!(base.zoom, 1.0);
        assert_eq!(base.names_split.width(), 8.0);
    }

    #[test]
    fn tall_rows_shift_every_row_below_and_hit_test_to_their_full_height() {
        let heights = [1, 4, 1, 8, 2].map(|h| RowHeight::try_from(h).unwrap());
        let input = |scroll_y| LayoutInput {
            bounds: Rect::from_xywh(0.0, 0.0, 2000.0, 32.0 + 24.0 * 5.0),
            row_h: 24.0,
            header_h: 32.0,
            zoom: 1.0,
            names_width: 220.0,
            values_width: 120.0,
            row_tops: row_tops(heights),
            scroll_y,
            markers: &[],
            viewport: Viewport {
                start: 0.0,
                end: 100.0,
            },
        };
        let l = WaveLayout::compute(input(0.0));
        let top = l.names.top();
        assert_eq!(
            (0..5).map(|ix| l.row_y(ix) - top).collect::<Vec<_>>(),
            [0.0, 24.0, 120.0, 144.0, 336.0]
        );
        assert_eq!(l.row_height(1), 96.0);
        assert_eq!(l.row_height(3), 192.0);
        // The 5-unit-high viewport ends exactly where row 2 starts.
        assert_eq!(l.rows, 0..2);
        assert_eq!(l.badges.len(), 2);
        assert_eq!(l.badges[1].1.top(), top + 24.0 + 4.0);
        assert_eq!(l.row_at(top + 24.0 + 95.0), Some(1));
        assert_eq!(l.row_at(top + 120.0), Some(2));
        assert_eq!(l.max_scroll, 24.0 * 16.0 - 24.0 * 5.0);
        // Past the content, hit-testing continues in 1× steps.
        assert_eq!(l.row_at(top + 384.0 + 30.0 - l.scroll_y), Some(6));

        // Scrolled into the middle of the 8× row: it is the first visible row.
        let l = WaveLayout::compute(input(24.0 * 8.0));
        assert_eq!(l.rows, 3..4);
        assert_eq!(l.row_y(3), l.names.top() - 48.0);
    }
}
