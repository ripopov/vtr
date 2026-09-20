//! Exact integer row navigation with viewport-local pixel conversion.

use crate::geometry::Rect;

pub const ROW_HEIGHT: f32 = 24.0;
pub const MAX_PREPARED_ROWS: u64 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RowViewport {
    pub top: u64,
    pub subrow_px: f32,
}

impl RowViewport {
    pub fn visible_rows(height: f32, row_height: f32) -> u64 {
        (height.max(0.0) / row_height.max(1.0)).ceil() as u64
    }
    pub fn clamp(&mut self, rows: u64, height: f32, row_height: f32) {
        if !self.subrow_px.is_finite() {
            self.subrow_px = 0.0;
        }
        let visible = Self::visible_rows(height, row_height).max(1);
        let last_top = rows.saturating_sub(visible);
        self.top = self.top.min(last_top);
        self.subrow_px = self.subrow_px.clamp(0.0, row_height.max(1.0));
        if self.top == last_top {
            self.subrow_px = 0.0;
        }
    }
    pub fn scroll(&mut self, pixels: f32, rows: u64, height: f32, row_height: f32) {
        if !pixels.is_finite() {
            return;
        }
        let total = self.subrow_px + pixels;
        let whole = (total / row_height.max(1.0)).floor();
        let remainder = total - whole * row_height.max(1.0);
        if whole < 0.0 {
            let delta = (-whole) as u64;
            if delta > self.top {
                self.top = 0;
                self.subrow_px = 0.0;
            } else {
                self.top -= delta;
                self.subrow_px = remainder;
            }
        } else {
            self.top = self.top.saturating_add(whole as u64);
            self.subrow_px = remainder;
        }
        self.clamp(rows, height, row_height);
    }
    pub fn reveal(&mut self, row: u64, rows: u64, height: f32, row_height: f32) {
        let visible = Self::visible_rows(height, row_height).max(1);
        if row < self.top {
            self.top = row;
        } else if row >= self.top.saturating_add(visible) {
            self.top = row.saturating_add(1).saturating_sub(visible);
        }
        self.subrow_px = 0.0;
        self.clamp(rows, height, row_height);
    }
    pub fn row_at(&self, y: f32, rows: u64, row_height: f32) -> Option<u64> {
        if !y.is_finite() || y < 0.0 {
            return None;
        }
        let local = ((y + self.subrow_px) / row_height.max(1.0)).floor() as u64;
        self.top.checked_add(local).filter(|&row| row < rows)
    }
    pub fn y_of(&self, row: u64, row_height: f32) -> f32 {
        let delta = if row >= self.top {
            (row - self.top) as f32
        } else {
            -((self.top - row) as f32)
        };
        delta * row_height - self.subrow_px
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ColumnRect {
    pub source_index: usize,
    /// Position in each prepared row's visible-cell vector.
    pub prepared_index: usize,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct TableLayout {
    pub bounds: Rect,
    pub header: Rect,
    pub body: Rect,
    pub gutter: Rect,
    pub vertical_bar: Rect,
    pub vertical_thumb: Rect,
    pub horizontal_bar: Rect,
    pub horizontal_thumb: Rect,
    pub status: Rect,
    pub columns: Vec<ColumnRect>,
    pub row_height: f32,
    pub horizontal: f32,
    pub content_width: f32,
}
