//! Pixel layout of the wave panel for one frame: the three columns, the
//! visible row range, the scrollbar, and the small interactive rectangles
//! (format badges, marker chips, column dividers). Pure: computed from the
//! panel bounds and model state, then read by both the painter and the input
//! handlers so a click lands on exactly what was drawn.

use std::ops::Range;

use crate::document::Marker;
use crate::geometry::{Point, Rect, point, size, snap};
use crate::wave::viewport::Viewport;

// Design-time sizes in logical pixels at zoom 1.0; the layout multiplies
// them by [`LayoutInput::zoom`] so every rectangle scales with the interface.
pub const MIN_COLUMN: f32 = 72.0;
const SPLITTER_TOLERANCE: f32 = 4.0;
pub const SCROLLBAR_W: f32 = 10.0;
const BADGE_W: f32 = 36.0;
const CHIP_W: f32 = 24.0;

#[derive(Clone, Debug, Default)]
pub struct WaveLayout {
    pub bounds: Rect,
    pub header: Rect,
    pub names: Rect,
    pub values: Rect,
    pub waves: Rect,
    pub rows: Range<usize>,
    pub row_h: f32,
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
    pub item_count: usize,
    pub scroll_y: f32,
    pub markers: &'a [Marker],
    pub viewport: Viewport,
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

        let content_h = row_h * input.item_count as f32;
        let max_scroll = (content_h - rows_h).max(0.0);
        let scroll_y = input.scroll_y.clamp(0.0, max_scroll);

        let first = (scroll_y / row_h).floor().max(0.0) as usize;
        let last = (((scroll_y + rows_h) / row_h).ceil() as usize).min(input.item_count);
        let rows = first.min(last)..last;

        let mut badges = Vec::new();
        for ix in rows.clone() {
            let y = rows_top + row_h * ix as f32 - scroll_y;
            let b = Rect::new(
                point(values.right() - z(BADGE_W + 6.0), y + z(4.0)),
                size(z(BADGE_W), row_h - z(8.0)),
            );
            badges.push((ix, b));
        }

        let wave_wf = f64::from(waves_w).max(1.0);
        let mut marker_chips = Vec::new();
        for (ix, m) in input.markers.iter().enumerate() {
            let x = input.viewport.x_of(m.time as f64, wave_wf);
            if x < 0.0 || x > wave_wf {
                continue;
            }
            let xp = snap(waves.left() + x as f32);
            marker_chips.push((ix, marker_chip_bounds(header, xp, z(CHIP_W), zoom)));
        }

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
        let ix = ((y - self.names.top() + self.scroll_y) / self.row_h).floor();
        (ix >= 0.0).then_some(ix as usize)
    }

    /// Top of row `ix` in the panel's coordinate space.
    pub fn row_y(&self, ix: usize) -> f32 {
        self.names.top() + self.row_h * ix as f32 - self.scroll_y
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

fn marker_chip_bounds(header: Rect, x: f32, chip_w: f32, zoom: f32) -> Rect {
    Rect::new(
        point(x + 1.0, header.top() + 2.0 * zoom),
        size(chip_w, 14.0 * zoom),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(zoom: f32, items: usize) -> WaveLayout {
        WaveLayout::compute(LayoutInput {
            bounds: Rect::from_xywh(0.0, 0.0, 2000.0, 300.0),
            row_h: 24.0 * zoom,
            header_h: 32.0 * zoom,
            zoom,
            names_width: 220.0,
            values_width: 120.0,
            item_count: items,
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
}
