//! Pixel layout of the pipeline panel for one frame: the header, the label
//! column, the cells area, the visible row range and density step, and the
//! small interactive rectangles (label divider, marker chips, retry). Pure:
//! computed from the panel bounds and model state, then read by both the
//! painter and the input handlers so a click lands on exactly what was drawn.

use std::ops::Range;

use super::rows::RowView;
use crate::document::Marker;
use crate::geometry::{Point, Rect, point, size};
use crate::wave::overlay::marker_chips;
use crate::wave::viewport::Viewport;

// Design-time sizes in logical pixels at zoom 1.0.
pub const LABEL_W_MIN: f32 = 60.0;
pub const LABEL_W_MAX: f32 = 420.0;
pub const LABEL_W_DEFAULT: f32 = 190.0;
const SPLITTER_TOLERANCE: f32 = 4.0;
const RETRY_W: f32 = 72.0;
const RETRY_H: f32 = 22.0;
/// Rows thinner than this are painted in density steps of at least this height.
pub const DENSITY_PX: f32 = 2.0;

#[derive(Clone, Debug, Default)]
pub struct PipelineLayout {
    pub activity_controls: Vec<(super::ActivityCommand, Rect, String)>,
    pub bounds: Rect,
    pub header: Rect,
    /// The label column below the header.
    pub labels: Rect,
    /// The cells area below the header, right of the labels.
    pub cells: Rect,
    /// Grab zone of the label divider.
    pub label_split: Rect,
    /// The row view this frame paints with, row height at the interface zoom.
    pub rows: RowView,
    /// Rows that touch the cells area, aligned to `row_step`.
    pub row_range: Range<usize>,
    /// Paint every `row_step`-th row at `row_step` rows' combined height.
    pub row_step: usize,
    pub row_count: usize,
    pub zoom: f32,
    pub marker_chips: Vec<(usize, Rect)>,
    /// The retry button of a failed load.
    pub retry: Option<Rect>,
}

#[derive(Clone, Copy)]
pub struct LayoutInput<'a> {
    pub bounds: Rect,
    /// Already zoomed (the theme's `timeline_height`).
    pub header_h: f32,
    pub zoom: f32,
    /// Design pixels; multiplied by `zoom`.
    pub label_width: f32,
    /// Design pixels; its row height is multiplied by `zoom`.
    pub rows: RowView,
    pub row_count: usize,
    pub markers: &'a [Marker],
    pub viewport: Viewport,
    pub failed: bool,
}

impl PipelineLayout {
    pub fn compute(input: LayoutInput<'_>) -> Self {
        let bounds = input.bounds;
        let zoom = if input.zoom.is_finite() && input.zoom > 0.0 {
            input.zoom
        } else {
            1.0
        };
        let z = |v: f32| v * zoom;
        let header_h = input.header_h;
        let labels_w = z(input.label_width.clamp(LABEL_W_MIN, LABEL_W_MAX))
            .min((bounds.width() - z(LABEL_W_MIN)).max(0.0));
        let header = Rect::new(bounds.origin, size(bounds.width(), header_h));
        let rows_top = bounds.top() + header_h;
        let rows_h = (bounds.height() - header_h).max(0.0);
        let labels = Rect::new(point(bounds.left(), rows_top), size(labels_w, rows_h));
        let cells = Rect::new(
            point(labels.right(), rows_top),
            size((bounds.width() - labels_w).max(0.0), rows_h),
        );
        let mut rows = input.rows.zoomed(zoom);
        rows.clamp(rows_h, input.row_count, zoom);
        let row_step = if rows.row_px < DENSITY_PX {
            (DENSITY_PX / rows.row_px).ceil().max(1.0) as usize
        } else {
            1
        };
        let first = rows.top.floor().max(0.0) as usize;
        let first = first / row_step * row_step;
        let last = ((rows.top + f64::from(rows_h / rows.row_px)).ceil().max(0.0) as usize)
            .min(input.row_count);
        let row_range = first.min(last)..last;
        let marker_chips = marker_chips(
            header,
            cells.left(),
            f64::from(cells.width()).max(1.0),
            input.viewport,
            input.markers,
            zoom,
        );
        let retry = input.failed.then(|| {
            Rect::new(
                point(
                    cells.left() + (cells.width() - z(RETRY_W)) / 2.0,
                    rows_top + rows_h / 2.0 + z(20.0),
                ),
                size(z(RETRY_W), z(RETRY_H)),
            )
        });
        let label_split = Rect::new(
            point(labels.right() - z(SPLITTER_TOLERANCE), bounds.top()),
            size(z(2.0 * SPLITTER_TOLERANCE), bounds.height()),
        );
        Self {
            activity_controls: Vec::new(),
            bounds,
            header,
            labels,
            cells,
            label_split,
            rows,
            row_range,
            row_step,
            row_count: input.row_count,
            zoom,
            marker_chips,
            retry,
        }
    }

    /// Row under `y`, when it exists.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        if y < self.cells.top() || y >= self.cells.bottom() {
            return None;
        }
        let row = self.rows.row_at(y - self.cells.top()).floor();
        (row >= 0.0 && (row as usize) < self.row_count).then_some(row as usize)
    }

    /// Top of row `ix` in the panel's coordinate space.
    pub fn row_y(&self, ix: usize) -> f32 {
        self.cells.top() + self.rows.y_of(ix as f64)
    }

    pub fn chip_at(&self, p: Point) -> Option<usize> {
        self.marker_chips
            .iter()
            .find(|(_, b)| b.contains(p))
            .map(|(ix, _)| *ix)
    }

    pub fn cells_width_f64(&self) -> f64 {
        f64::from(self.cells.width()).max(1.0)
    }

    /// Time under panel x, in the cells area's viewport.
    pub fn time_at(&self, viewport: &Viewport, x: f32) -> f64 {
        viewport.time_at(f64::from(x - self.cells.left()), self.cells_width_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(zoom: f32, row_px: f32, rows: usize) -> PipelineLayout {
        PipelineLayout::compute(LayoutInput {
            bounds: Rect::from_xywh(0.0, 0.0, 1000.0, 332.0),
            header_h: 32.0 * zoom,
            zoom,
            label_width: 190.0,
            rows: RowView { top: 3.0, row_px },
            row_count: rows,
            markers: &[Marker {
                id: 1,
                time: 50,
                label: None,
            }],
            viewport: Viewport {
                start: 0.0,
                end: 100.0,
            },
            failed: false,
        })
    }

    #[test]
    fn rectangles_scale_with_zoom_and_density_steps_bound_the_row_count() {
        for zoom in [1.0f32, 2.0] {
            let l = layout(zoom, 20.0, 1000);
            assert_eq!(l.labels.width(), 190.0 * zoom);
            assert_eq!(l.cells.left(), 190.0 * zoom);
            assert_eq!(l.rows.row_px, 20.0 * zoom);
            assert_eq!(l.row_step, 1);
            assert_eq!(l.row_range.start, 3);
            let visible = ((332.0 - 32.0 * zoom) / (20.0 * zoom)).ceil() as usize;
            assert_eq!(l.row_range.end, 3 + visible);
            assert_eq!(l.row_at(l.cells.top() + 20.0 * zoom * 1.5), Some(4));
            assert_eq!(l.row_at(l.cells.top() - 1.0), None);
            assert_eq!(l.marker_chips.len(), 1);
            assert_eq!(l.label_split.width(), 8.0 * zoom);
        }
        let dense = layout(1.0, 0.5, 100_000);
        assert_eq!(dense.row_step, 4);
        assert_eq!(dense.row_range.start, 0);
        assert!(dense.row_range.len() / dense.row_step <= 300 / 2 + 1);
        let few = layout(1.0, 20.0, 5);
        assert_eq!(few.row_range, 0..5);
        assert!(few.rows.top < 0.0, "over-scroll space before the first row");
    }
}
