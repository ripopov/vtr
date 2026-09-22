//! Transaction lanes: a generator shown as one row of the waveform panel,
//! each record a bar over its lifetime. This module holds the lane row, its
//! geometry in row units, and the pure queries the painter and the input
//! handlers share (density bins, folded overlaps, hit tests, snapping).
//! Records come from the document's resident [`LoadedGenerator`]; stacking is
//! the generator's load-time [`LoadedGenerator::sub_row`].

use crate::data::loaded_tracks::LoadedGenerator;
use crate::data::transactions::{TrackRef, Transaction, TxStatus};
use crate::document::{Document, TrackLoadState};
use crate::pipeline::{PipelineModel, TrackSource};
use crate::wave::model::RowHeight;
use crate::wave::viewport::Viewport;

/// Below this many pixels for a median lifetime, bars are unreadable and the
/// lane draws a density strip instead.
pub const DENSITY_PX: f64 = 4.0;
/// A sub-row is this fraction of a 1× row, and the lane keeps this fraction
/// of a row free above and below its sub-rows.
const SUB_ROW: f32 = 2.0 / 3.0;
const LANE_PAD: f32 = 1.0 / 8.0;
/// Presets a new lane may take by default; deeper generators fold.
const DEFAULT_MAX: RowHeight = RowHeight::PRESETS[3];

/// A generator shown as a wave row.
#[derive(Clone, Debug, PartialEq)]
pub struct TxLane {
    pub source: TrackSource,
    pub name: String,
    pub scope: String,
    pub height: RowHeight,
    /// A new lane takes the default height for its depth once its records
    /// arrive; an explicit or restored height clears this.
    pub auto_height: bool,
}

/// What a lane can show now.
pub enum LaneData<'a> {
    Ready(&'a LoadedGenerator),
    Loading,
    Failed(&'a str),
    Unresolved,
    Unavailable,
}

impl TxLane {
    /// A lane for `track` of the open session's catalog.
    pub fn new(doc: &Document, track: TrackRef) -> Option<Self> {
        let path = doc
            .session()?
            .tracks()
            .iter()
            .find(|t| t.id == track)?
            .path
            .clone();
        let (name, scope) = path.split_last()?;
        let mut lane = Self {
            name: name.clone(),
            scope: scope.join("."),
            source: TrackSource::Resolved {
                track,
                path: path.clone(),
            },
            height: RowHeight::DEFAULT,
            auto_height: true,
        };
        lane.fit_height(doc);
        Some(lane)
    }

    /// A lane that keeps a saved path the session does not have.
    pub fn unresolved(path: Vec<String>, height: RowHeight) -> Self {
        let name = path.last().cloned().unwrap_or_default();
        let scope = path[..path.len().saturating_sub(1)].join(".");
        Self {
            source: TrackSource::Unresolved { path },
            name,
            scope,
            height,
            auto_height: false,
        }
    }

    pub fn track(&self) -> Option<TrackRef> {
        self.source.track()
    }

    /// The generator's records, when the document holds them.
    pub fn data<'a>(&self, doc: &'a Document) -> LaneData<'a> {
        let Some(track) = self.track() else {
            return LaneData::Unresolved;
        };
        match doc.track(track) {
            Some(TrackLoadState::Ready(loaded)) => loaded
                .generators
                .first()
                .map_or(LaneData::Unavailable, |g| LaneData::Ready(g)),
            Some(TrackLoadState::Loading) => LaneData::Loading,
            Some(TrackLoadState::Failed(error)) => LaneData::Failed(error),
            None => LaneData::Unavailable,
        }
    }

    pub fn generator<'a>(&self, doc: &'a Document) -> Option<&'a LoadedGenerator> {
        match self.data(doc) {
            LaneData::Ready(g) => Some(g),
            _ => None,
        }
    }

    /// Take the default height for the generator's depth once it is known.
    /// Returns whether the height changed.
    pub fn fit_height(&mut self, doc: &Document) -> bool {
        if !self.auto_height {
            return false;
        }
        let Some(depth) = self.generator(doc).map(LoadedGenerator::depth) else {
            return false;
        };
        self.auto_height = false;
        let height = default_height(depth);
        let changed = height != self.height;
        self.height = height;
        changed
    }
}

// -- geometry ------------------------------------------------------------------

/// Sub-rows a lane of `height` shows; deeper ones fold into the last.
pub fn capacity(height: RowHeight) -> u16 {
    let units = f32::from(height.multiple());
    (((units - 2.0 * LANE_PAD) / SUB_ROW).floor() as u16).max(1)
}

/// The smallest preset up to 4× that shows `depth` sub-rows.
pub fn default_height(depth: u16) -> RowHeight {
    RowHeight::PRESETS
        .into_iter()
        .take_while(|h| *h != DEFAULT_MAX)
        .chain([DEFAULT_MAX])
        .find(|h| capacity(*h) >= depth)
        .unwrap_or(DEFAULT_MAX)
}

/// Sub-rows folded away at `height`.
pub fn folded(depth: u16, height: RowHeight) -> u16 {
    depth.saturating_sub(capacity(height))
}

/// The sub-row a record with stacking `sub_row` is drawn on.
pub fn shown_sub_row(sub_row: u16, height: RowHeight) -> u16 {
    sub_row.min(capacity(height) - 1)
}

/// Pixel geometry of one lane row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaneGeometry {
    pub top: f32,
    pub height: f32,
    pub sub_h: f32,
    pub pad: f32,
    pub capacity: u16,
}

impl LaneGeometry {
    pub fn new(top: f32, height: f32, row_h: f32, rows: RowHeight) -> Self {
        Self {
            top,
            height,
            sub_h: row_h * SUB_ROW,
            pad: row_h * LANE_PAD,
            capacity: capacity(rows),
        }
    }

    /// Top of sub-row `sub`.
    pub fn sub_top(&self, sub: u16) -> f32 {
        self.top + self.pad + self.sub_h * f32::from(sub)
    }

    /// The shown sub-row under `y`, if any.
    pub fn sub_at(&self, y: f32) -> Option<u16> {
        let offset = y - self.top - self.pad;
        if offset < 0.0 || self.sub_h <= 0.0 {
            return None;
        }
        let sub = (offset / self.sub_h) as u16;
        (sub < self.capacity).then_some(sub)
    }
}

/// Whether bars are too narrow to read at `px_per_unit`.
pub fn is_density(generator: &LoadedGenerator, px_per_unit: f64) -> bool {
    generator.median_lifetime() as f64 * px_per_unit < DENSITY_PX
}

// -- queries -------------------------------------------------------------------

/// The window of trace time `viewport` shows, as an inclusive query range.
pub fn window(viewport: &Viewport) -> (u64, u64) {
    let start = viewport.start.max(0.0).floor() as u64;
    let end = viewport.end.max(0.0).ceil() as u64;
    (start, end.max(start))
}

/// One density column: how many records are open, and whether one failed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DensityColumn {
    pub open: u32,
    pub failed: bool,
}

/// Open records per pixel column over `viewport`, `columns` wide. One visit
/// of the window with difference arrays: O(records + columns).
pub fn density(
    generator: &LoadedGenerator,
    viewport: &Viewport,
    columns: usize,
) -> Vec<DensityColumn> {
    if columns == 0 {
        return Vec::new();
    }
    let mut open = vec![0i32; columns + 1];
    let mut failed = vec![0i32; columns + 1];
    let last = (columns - 1) as f64;
    let scale = viewport.px_per_unit(columns as f64);
    let column = |t: u64| ((t as f64 - viewport.start) * scale).clamp(0.0, last) as usize;
    let (start, end) = window(viewport);
    _ = generator.visit_window(start, end, |tx| {
        let (a, b) = (column(tx.begin), column(tx.end) + 1);
        open[a] += 1;
        open[b] -= 1;
        if tx.status == TxStatus::Error {
            failed[a] += 1;
            failed[b] -= 1;
        }
        true
    });
    let (mut n, mut f) = (0i32, 0i32);
    (0..columns)
        .map(|x| {
            n += open[x];
            f += failed[x];
            DensityColumn {
                open: n.max(0) as u32,
                failed: f > 0,
            }
        })
        .collect()
}

/// Time spans where two or more records folded into the last sub-row of a
/// lane `height` tall overlap. Empty when nothing folds.
pub fn fold_overlaps(
    generator: &LoadedGenerator,
    viewport: &Viewport,
    height: RowHeight,
) -> Vec<(u64, u64)> {
    let last = capacity(height) - 1;
    if generator.depth() <= capacity(height) {
        return Vec::new();
    }
    let (start, end) = window(viewport);
    let mut events = Vec::new();
    _ = generator.visit_window_ordinals(start, end, |ordinal, tx| {
        if generator.sub_row(ordinal) >= last && tx.end > tx.begin {
            events.push((tx.begin, 1i32));
            events.push((tx.end, -1));
        }
        true
    });
    events.sort_unstable();
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let (mut open, mut from) = (0i32, 0u64);
    for (time, delta) in events {
        let next = open + delta;
        if open < 2 && next >= 2 {
            from = time;
        } else if open >= 2 && next < 2 && time > from {
            match spans.last_mut() {
                Some(last) if last.1 >= from => last.1 = time,
                _ => spans.push((from, time)),
            }
        }
        open = next;
    }
    spans
}

/// The record under time `time` on shown sub-row `sub`: the last one drawn
/// that contains it, else the one with an edge nearest to it within
/// `tolerance`. Returns its canonical ordinal.
pub fn hit(
    generator: &LoadedGenerator,
    height: RowHeight,
    sub: u16,
    time: f64,
    tolerance: f64,
) -> Option<usize> {
    let start = (time - tolerance).max(0.0).floor() as u64;
    let end = (time + tolerance).max(0.0).ceil() as u64;
    let mut inside = None;
    let mut near: Option<(f64, usize)> = None;
    _ = generator.visit_window_ordinals(start, end, |ordinal, tx| {
        if shown_sub_row(generator.sub_row(ordinal), height) != sub {
            return true;
        }
        if tx.begin as f64 <= time && time <= tx.end as f64 {
            inside = Some(ordinal);
        }
        let d = (tx.begin as f64 - time)
            .abs()
            .min((tx.end as f64 - time).abs());
        if d <= tolerance && near.is_none_or(|(best, _)| d < best) {
            near = Some((d, ordinal));
        }
        true
    });
    inside.or(near.map(|(_, ordinal)| ordinal))
}

/// The record begin or end nearest to `time` within `tolerance`.
pub fn nearest_boundary(generator: &LoadedGenerator, time: f64, tolerance: f64) -> Option<u64> {
    let start = (time - tolerance).max(0.0).floor() as u64;
    let end = (time + tolerance).max(0.0).ceil() as u64;
    let mut best: Option<(f64, u64)> = None;
    _ = generator.visit_window(start, end, |tx| {
        for edge in [tx.begin, tx.end] {
            let d = (edge as f64 - time).abs();
            if d <= tolerance && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, edge));
            }
        }
        true
    });
    best.map(|(_, edge)| edge)
}

/// Records open at `time`: begun at or before it and not yet ended (a point
/// record is open at its own time), in canonical order.
pub fn open_at(generator: &LoadedGenerator, time: u64) -> Vec<&Transaction> {
    let mut open = Vec::new();
    _ = generator.visit_window(time, time, |tx| {
        if time < tx.end || tx.begin == tx.end {
            open.push(tx);
        }
        true
    });
    open
}

/// A record's caption: its `vtr.label`, else its identity.
pub fn label(tx: &Transaction) -> String {
    let label = PipelineModel::label(tx);
    if label.is_empty() {
        format!("#{}", tx.id.0)
    } else {
        label
    }
}

/// The value column of a lane at the cursor: the record open there, or how
/// many are, and whether any of them failed.
pub fn value_text(generator: &LoadedGenerator, cursor: u64) -> (String, bool) {
    let open = open_at(generator, cursor);
    let failed = open.iter().any(|tx| tx.status == TxStatus::Error);
    let text = match open.as_slice() {
        [] => "–".to_owned(),
        [tx] => label(tx),
        many => format!(
            "{} open: {}",
            many.len(),
            many.iter()
                .map(|tx| label(tx))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    };
    (text, failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::transactions::{TransactionRef, TxKind};
    use std::collections::HashMap;

    fn tx(id: u64, begin: u64, end: u64, status: TxStatus) -> Transaction {
        Transaction {
            id: TransactionRef(id),
            generator: TrackRef(1),
            begin,
            end,
            status,
            kind: TxKind::Producer,
            parent: None,
            attributes: vec![],
            events: vec![],
            stages: vec![],
        }
    }

    /// The demo's four overlapping reads: sub-rows 0, 1, 2, 0.
    fn episode() -> LoadedGenerator {
        LoadedGenerator::new(
            TrackRef(1),
            vec![
                tx(0, 2, 20, TxStatus::Ok),
                tx(1, 8, 26, TxStatus::Ok),
                tx(2, 14, 40, TxStatus::Error),
                tx(3, 22, 34, TxStatus::Ok),
            ],
            HashMap::new(),
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn heights_hold_whole_sub_rows_and_default_to_the_depth() {
        let caps: Vec<_> = RowHeight::PRESETS.into_iter().map(capacity).collect();
        assert_eq!(caps, [1, 2, 4, 5, 11]);
        assert_eq!(default_height(0).multiple(), 1);
        assert_eq!(default_height(1).multiple(), 1);
        assert_eq!(default_height(3).multiple(), 3);
        assert_eq!(default_height(5).multiple(), 4);
        assert_eq!(default_height(40).multiple(), 4, "capped at 4×");
        assert_eq!(folded(7, RowHeight::PRESETS[3]), 2);
        assert_eq!(shown_sub_row(6, RowHeight::DEFAULT), 0);
        let g = LaneGeometry::new(100.0, 24.0, 24.0, RowHeight::DEFAULT);
        assert_eq!(g.sub_at(104.0), Some(0));
        assert_eq!(g.sub_at(101.0), None);
        assert_eq!(g.sub_at(121.0), None);
    }

    #[test]
    fn density_fold_hit_and_value_queries() {
        let g = episode();
        assert_eq!(
            (0..4).map(|i| g.sub_row(i)).collect::<Vec<_>>(),
            [0, 1, 2, 0]
        );
        assert_eq!(g.depth(), 3);
        let vp = Viewport {
            start: 0.0,
            end: 60.0,
        };
        let d = density(&g, &vp, 60);
        assert_eq!(d[19].open, 3);
        assert!(d[39].failed && !d[41].failed);
        assert_eq!(d[50], DensityColumn::default());
        // At 1×, every sub-row folds into one; overlapping spans hatch.
        assert_eq!(fold_overlaps(&g, &vp, RowHeight::DEFAULT), [(8, 34)]);
        assert!(fold_overlaps(&g, &vp, RowHeight::PRESETS[2]).is_empty());
        // Sub-row 2 at 19 is the failing read; folded, the last drawn wins.
        let three = RowHeight::PRESETS[2];
        assert_eq!(hit(&g, three, 2, 19.0, 0.0), Some(2));
        assert_eq!(hit(&g, RowHeight::DEFAULT, 0, 19.0, 0.0), Some(2));
        assert_eq!(hit(&g, three, 0, 21.0, 0.0), None);
        assert_eq!(hit(&g, three, 0, 21.0, 1.5), Some(0));
        assert_eq!(nearest_boundary(&g, 21.2, 2.0), Some(22));
        assert_eq!(nearest_boundary(&g, 50.0, 2.0), None);
        assert_eq!(value_text(&g, 5), ("#0".into(), false));
        assert_eq!(value_text(&g, 36), ("#2".into(), true));
        assert_eq!(value_text(&g, 45).0, "–");
        assert!(value_text(&g, 19).0.starts_with("3 open"));
        assert!(is_density(&g, 0.1) && !is_density(&g, 1.0));
    }
}
