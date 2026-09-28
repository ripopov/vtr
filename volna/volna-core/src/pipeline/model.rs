//! `PipelineModel`: the state of a pipeline panel (the track it shows, its
//! navigation, the row axis, the label column, hover and drags) and every
//! input rule that mutates it. Data is the document's loaded track; the model
//! never copies records.

use std::sync::Arc;

use web_time::Instant;

use super::layout::{LABEL_W_DEFAULT, LABEL_W_MAX, LABEL_W_MIN, LayoutInput, PipelineLayout};
use super::palette::StagePalette;
use super::rows::{ROW_PX_DEFAULT, ROW_PX_MAX, ROW_PX_MIN, RowView};
use super::zoom::{self, CYCLE_PX_MAX, ROW_PX_CAP, ZoomBox, ZoomPoint};
use crate::data::loaded_tracks::LoadedGenerator;
use crate::data::transactions::{
    AttributeValue, TrackRef, Transaction, TransactionRef, TransactionStage, TxStatus,
};
use crate::document::{Document, TrackLoadState, TxSelection};
use crate::geometry::{Modifiers, MouseButton, Point};
use crate::nav::{Link, NavState, Tween};
use crate::panels::PanelId;
use crate::theme::Theme;
use crate::wave::model::PointerEvent;
use crate::wave::viewport::{FIT_MARGIN_PX, Viewport};

/// The per-transaction caption attribute (`docs/SPEC.md`).
pub const LABEL_ATTRIBUTE: &str = "vtr.label";
/// A drag shorter than this (at zoom 1.0) is a click.
const DRAG_THRESHOLD_PX: f32 = 3.0;
/// Rows scrolled by one keyboard step.
pub const SCROLL_ROWS: f64 = 3.0;
/// Row height factor of one Increase/Decrease Row Height step.
const ROW_HEIGHT_STEP: f64 = std::f64::consts::SQRT_2;

/// A panel is bound to a track of this session or retains a durable path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackSource {
    Resolved { track: TrackRef, path: Vec<String> },
    Unresolved { path: Vec<String> },
}

impl TrackSource {
    pub fn path(&self) -> &[String] {
        match self {
            Self::Resolved { path, .. } | Self::Unresolved { path } => path,
        }
    }

    pub fn track(&self) -> Option<TrackRef> {
        match self {
            Self::Resolved { track, .. } => Some(*track),
            Self::Unresolved { .. } => None,
        }
    }
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Activity(super::ActivityCommand),
    Row(usize),
    Cell {
        row: usize,
        stage: usize,
    },
    LabelSplit,
    Header,
    /// A chip on the Markers lane, by its index in the lane layout.
    Marker(usize),
    Retry,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    /// Pans both axes once the pointer moved past the threshold; a shorter
    /// left-button press is a click that sets the cursor.
    Pan {
        button: MouseButton,
        start: Point,
        last: Point,
        moved: bool,
    },
    Cursor,
    LabelSplit,
}

/// The rows a panel shows, or why it shows none.
pub enum Rows<'a> {
    Ready(RowSet<'a>),
    Loading,
    Failed(&'a str),
    /// The saved path is not a track of this trace.
    Unresolved,
    /// No trace is open or the panel is not attached to the document.
    Unavailable,
}

/// The concatenated transaction slices of a loaded track, generator by
/// generator in catalog order, each in the resident (begin, end, id) order.
pub struct RowSet<'a> {
    generators: &'a [Arc<LoadedGenerator>],
    starts: Vec<usize>,
    len: usize,
}

impl<'a> RowSet<'a> {
    fn new(generators: &'a [Arc<LoadedGenerator>]) -> Self {
        let mut starts = Vec::with_capacity(generators.len());
        let mut len = 0;
        for g in generators {
            starts.push(len);
            len += g.transactions().len();
        }
        Self {
            generators,
            starts,
            len,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn generators(&self) -> &'a [Arc<LoadedGenerator>] {
        self.generators
    }

    /// The row of one record, when this track holds its generator.
    pub fn row_of(&self, generator: TrackRef, id: TransactionRef) -> Option<usize> {
        let g = self
            .generators
            .iter()
            .position(|c| c.generator() == generator)?;
        Some(self.starts[g] + self.generators[g].transaction_ordinal(id)?)
    }

    /// Row `row` as its generator index and record.
    pub fn get(&self, row: usize) -> Option<(usize, &'a Transaction)> {
        if row >= self.len {
            return None;
        }
        let g = self.starts.partition_point(|&start| start <= row) - 1;
        Some((g, &self.generators[g].transactions()[row - self.starts[g]]))
    }
}

pub struct PipelineModel {
    pub follow: super::FollowActivity,
    pub track: TrackSource,
    pub nav: NavState,
    /// The row axis at interface zoom 1.0.
    pub rows: Tween<RowView>,
    /// Row height the two-axis zoom stops growing rows at, in design pixels:
    /// [`ROW_PX_CAP`], or a taller height set by a rows-only zoom.
    pub row_cap: f32,
    /// The aspect (`ln` row height − `ln` pixels per time unit) the two-axis
    /// zoom keeps while an axis is at a limit; see [`super::zoom`].
    aspect: Option<f64>,
    /// Design pixels, zoom applied at layout.
    pub label_width: f32,
    pub hover: Option<Hit>,
    pub drag: Option<Drag>,
    /// Last known pointer position over the panel.
    pub pointer: Option<Point>,
    /// Canvas paints so far, for tests that need real repaints.
    pub frames_painted: u64,
    attached: bool,
    palette: StagePalette,
    layout: PipelineLayout,
}

impl PipelineModel {
    pub fn new(track: TrackSource, link: Link) -> Self {
        let mut nav = NavState::new();
        nav.link = link;
        Self {
            track,
            follow: super::FollowActivity::default(),
            nav,
            rows: Tween::new(RowView::default()),
            row_cap: ROW_PX_CAP,
            aspect: None,
            label_width: LABEL_W_DEFAULT,
            hover: None,
            drag: None,
            pointer: None,
            frames_painted: 0,
            attached: false,
            palette: StagePalette::default(),
            layout: PipelineLayout::default(),
        }
    }

    /// Copy persistent view state for a split; the copy is not attached.
    pub fn clone_view(&self) -> Self {
        Self {
            track: self.track.clone(),
            follow: self.follow,
            nav: self.nav.clone_view(),
            rows: Tween::new(self.rows.target()),
            row_cap: self.row_cap,
            aspect: self.aspect,
            label_width: self.label_width,
            palette: self.palette.clone(),
            ..Self::new(self.track.clone(), self.nav.link)
        }
    }

    /// Retain the track through the document. Pair with [`PipelineModel::detach`].
    pub fn attach(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        if self.attached {
            return Ok(());
        }
        if let Some(track) = self.track.track() {
            doc.retain_track(track)?;
        }
        self.attached = true;
        self.refresh(doc);
        Ok(())
    }

    pub fn detach(&mut self, doc: &mut Document) {
        if !self.attached {
            return;
        }
        if let Some(track) = self.track.track() {
            doc.release_track(track);
        }
        self.attached = false;
    }

    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Rebuild the palette from the loaded track; call after a delivery.
    pub fn refresh(&mut self, doc: &Document) {
        if let Rows::Ready(set) = self.rows(doc) {
            self.palette = StagePalette::build(set.generators());
        }
    }

    pub fn retry(&mut self, doc: &mut Document) -> bool {
        self.track
            .track()
            .is_some_and(|track| doc.retry_track(track))
    }

    pub fn palette(&self) -> &StagePalette {
        &self.palette
    }

    pub fn rows<'a>(&self, doc: &'a Document) -> Rows<'a> {
        let Some(track) = self.track.track() else {
            return Rows::Unresolved;
        };
        if !self.attached {
            return Rows::Unavailable;
        }
        match doc.track(track) {
            Some(TrackLoadState::Ready(loaded)) => Rows::Ready(RowSet::new(&loaded.generators)),
            Some(TrackLoadState::Loading) => Rows::Loading,
            Some(TrackLoadState::Failed(error)) => Rows::Failed(error),
            None => Rows::Unavailable,
        }
    }

    pub fn row_count(&self, doc: &Document) -> usize {
        match self.rows(doc) {
            Rows::Ready(set) => set.len(),
            _ => 0,
        }
    }

    /// The caption of a transaction from its single `vtr.label` attribute.
    /// Both VTR `str` and `text` values are resolved to [`AttributeValue::Text`]
    /// at the session boundary.
    pub fn label(tx: &Transaction) -> String {
        tx.attributes
            .iter()
            .find_map(|attribute| match (&*attribute.key, &attribute.value) {
                (LABEL_ATTRIBUTE, AttributeValue::Text(text)) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The end of a stage: its own, or its transaction's while still open.
    pub fn stage_end(tx: &Transaction, stage: &TransactionStage) -> u64 {
        stage.end.unwrap_or(tx.end).max(stage.begin)
    }

    /// The layout of the last frame (see [`PipelineModel::layout`]).
    pub fn last_layout(&self) -> &PipelineLayout {
        &self.layout
    }

    pub fn is_animating(&self) -> bool {
        self.nav.is_animating() || self.rows.is_animating()
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let nav = self.nav.tick(now);
        let rows = self.rows.tick(now);
        nav || rows
    }

    // -- selection ---------------------------------------------------------------

    /// The row of the document's selected record, when this panel shows it.
    pub fn selected_row(&self, doc: &Document) -> Option<usize> {
        let selection = doc.selection()?;
        let Rows::Ready(set) = self.rows(doc) else {
            return None;
        };
        set.row_of(selection.track, selection.id)
    }

    /// Make row `row` the document selection. Returns whether it changed.
    pub fn select_row(&mut self, doc: &mut Document, panel: PanelId, row: usize) -> bool {
        let selection = {
            let Rows::Ready(set) = self.rows(doc) else {
                return false;
            };
            let Some((_, tx)) = set.get(row) else {
                return false;
            };
            TxSelection {
                track: tx.generator,
                id: tx.id,
                origin: panel,
            }
        };
        doc.select(Some(selection))
    }

    /// ↑ ↓ with a selection: move it by one row and keep it visible. Without
    /// one, the rows scroll, as they always have.
    pub fn move_selection(
        &mut self,
        doc: &mut Document,
        panel: PanelId,
        delta: isize,
        now: Instant,
    ) {
        let Some(row) = self.selected_row(doc) else {
            self.scroll_rows(doc, delta as f64 * SCROLL_ROWS, now);
            return;
        };
        let count = self.row_count(doc);
        let next = row
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
        if !self.select_row(doc, panel, next) {
            return;
        }
        self.reveal_row(doc, next);
    }

    /// Scroll to the record's row and fit the time axis to its lifetime.
    /// Returns whether the panel shows the record at all.
    pub fn reveal_record(
        &mut self,
        doc: &mut Document,
        track: TrackRef,
        id: TransactionRef,
        now: Instant,
    ) -> bool {
        let Some((row, begin, end)) = ({
            let Rows::Ready(set) = self.rows(doc) else {
                return false;
            };
            set.row_of(track, id).and_then(|row| {
                let (_, tx) = set.get(row)?;
                Some((row, tx.begin, tx.end))
            })
        }) else {
            return false;
        };
        self.suspend_follow();
        self.center_row(doc, row);
        let pad = ((end.saturating_sub(begin)) as f64 * 0.05).max(1.0);
        self.nav.animate_to(
            doc,
            crate::wave::viewport::Viewport {
                start: begin as f64 - pad,
                end: end as f64 + pad,
            },
            now,
        );
        true
    }

    /// Put `row` in the middle of the cells area.
    fn center_row(&mut self, doc: &Document, row: usize) {
        let zoom = self.layout.zoom.max(f32::EPSILON);
        let height = self.layout.cells.height();
        if height <= 0.0 {
            return;
        }
        let mut rows = self.rows.target().zoomed(zoom);
        rows.top = row as f64 + 0.5 - f64::from(height / rows.row_px) * 0.5;
        rows.clamp(height, self.row_count(doc), zoom);
        self.rows.set(rows.unzoomed(zoom));
    }

    /// Scroll only far enough to bring `row` back inside the cells area.
    fn reveal_row(&mut self, doc: &Document, row: usize) {
        let zoom = self.layout.zoom.max(f32::EPSILON);
        let height = self.layout.cells.height();
        if height <= 0.0 {
            return;
        }
        let mut rows = self.rows.target().zoomed(zoom);
        let visible = f64::from(height / rows.row_px);
        let position = row as f64;
        if position < rows.top {
            rows.top = position;
        } else if position + 1.0 > rows.top + visible {
            rows.top = position + 1.0 - visible;
        } else {
            return;
        }
        rows.clamp(height, self.row_count(doc), zoom);
        self.rows.set(rows.unzoomed(zoom));
    }

    /// Text for the status bar about what the pointer is over.
    pub fn hover_text(&self, doc: &Document) -> Option<String> {
        let (row, stage) = match self.hover? {
            Hit::Row(row) => (row, None),
            Hit::Cell { row, stage } => (row, Some(stage)),
            _ => return None,
        };
        let Rows::Ready(set) = self.rows(doc) else {
            return None;
        };
        let (_, tx) = set.get(row)?;
        let mut text = format!("#{row}");
        match tx.status {
            TxStatus::Aborted => text.push_str(" flushed"),
            TxStatus::Open => text.push_str(" open"),
            TxStatus::Error => text.push_str(" error"),
            _ => {}
        }
        match stage.and_then(|s| tx.stages.get(s)) {
            Some(s) => {
                text.push_str(&format!(
                    " · {} {}",
                    s.name,
                    self.span_text(doc, s.begin, Self::stage_end(tx, s))
                ));
                if s.lane != self.palette.primary_lane() {
                    text.push_str(&format!(" lane {}", s.lane));
                }
            }
            None => text.push_str(&format!(" · {}", self.span_text(doc, tx.begin, tx.end))),
        }
        let label = Self::label(tx);
        if !label.is_empty() {
            text.push_str(" · ");
            text.push_str(&label);
        }
        Some(text)
    }

    // -- layout ------------------------------------------------------------------

    /// Lay the panel out in `bounds` for this frame. Clamps the row view to
    /// the rows and the area, and derives hover state from the last pointer
    /// position. The result is kept for input handling.
    pub fn layout(
        &mut self,
        bounds: crate::geometry::Rect,
        doc: &Document,
        theme: &Theme,
    ) -> &PipelineLayout {
        let (row_count, failed) = match self.rows(doc) {
            Rows::Ready(set) => (set.len(), false),
            Rows::Failed(_) => (0, true),
            _ => (0, false),
        };
        let rulers = self.nav.clocks().rulers(&doc.clocks).len();
        let input = LayoutInput {
            bounds,
            header_h: theme.timeline_height,
            ruler_h: rulers as f32 * crate::wave::overlay::RULER_H * theme.zoom,
            zoom: theme.zoom,
            label_width: self.label_width,
            rows: self.rows.value,
            row_count,
            markers: doc.markers(),
            viewport: self.nav.viewport(doc),
            failed,
        };
        self.layout = PipelineLayout::compute(input);
        self.follow_activity(doc);
        let layout = if self.rows.value != input.rows {
            PipelineLayout::compute(LayoutInput {
                rows: self.rows.value,
                ..input
            })
        } else {
            self.layout.clone()
        };
        // The clamp is part of the view: keep it once nothing animates.
        if !self.rows.is_animating() {
            self.rows.set(layout.rows.unzoomed(layout.zoom));
        }
        self.layout = layout;
        let activity = self.activity(doc);
        self.layout.activity = activity;
        let area = self.layout.cells;
        let z = theme.zoom;
        let width = (170.0 * z).min(area.width());
        let x = area.left() + (area.width() - width) * 0.5;
        if area.height() >= 80.0 * z {
            for (command, count, y, direction) in [
                (
                    super::ActivityCommand::RevealAbove,
                    activity.above,
                    area.top() + 6.0 * z,
                    "↑",
                ),
                (
                    super::ActivityCommand::RevealBelow,
                    activity.below,
                    area.bottom() - 28.0 * z,
                    "↓",
                ),
            ] {
                if count > 0 {
                    self.layout.activity_controls.push((
                        command,
                        crate::geometry::Rect::from_xywh(x, y, width, 22.0 * z),
                        format!("{direction} {count} relevant rows"),
                    ));
                }
            }
        }
        self.hover = self.pointer.and_then(|p| self.hit_at(p, doc));
        &self.layout
    }

    fn hit_at(&self, p: Point, doc: &Document) -> Option<Hit> {
        let layout = &self.layout;
        if !layout.bounds.contains(p) {
            return None;
        }
        if let Some((command, _, _)) = layout
            .activity_controls
            .iter()
            .find(|(_, rect, _)| rect.contains(p))
        {
            return Some(Hit::Activity(*command));
        }
        if layout.label_split.contains(p) {
            return Some(Hit::LabelSplit);
        }
        if layout.retry.is_some_and(|r| r.contains(p)) {
            return Some(Hit::Retry);
        }
        if let Some(chip) = layout.marker_lane.chip_at(p) {
            return Some(Hit::Marker(chip));
        }
        if layout.header.contains(p) || layout.marker_lane.band.contains(p) {
            return Some(Hit::Header);
        }
        let row = layout.row_at(p.y)?;
        if layout.labels.contains(p) {
            return Some(Hit::Row(row));
        }
        let Rows::Ready(set) = self.rows(doc) else {
            return Some(Hit::Row(row));
        };
        let (_, tx) = set.get(row)?;
        let t = layout.time_at(&self.nav.viewport(doc), p.x);
        let primary = self.palette.primary_lane();
        // An overlay lane is drawn on top of the primary lane, so it wins.
        let mut best: Option<(usize, bool)> = None;
        for (ix, stage) in tx.stages.iter().enumerate() {
            let end = Self::stage_end(tx, stage);
            let inside = t >= stage.begin as f64 && (t < end as f64 || stage.begin == end);
            if !inside {
                continue;
            }
            let overlay = stage.lane != primary;
            if best.is_none_or(|(_, was_overlay)| overlay || !was_overlay) {
                best = Some((ix, overlay));
            }
        }
        Some(match best {
            Some((stage, _)) => Hit::Cell { row, stage },
            None => Hit::Row(row),
        })
    }

    // -- navigation --------------------------------------------------------------

    fn cells_w(&self) -> f64 {
        self.layout.cells_width_f64()
    }

    fn cells_h(&self) -> f32 {
        self.layout.cells.height().max(1.0)
    }

    /// The row view the next change builds on, at the current zoom.
    fn rows_target(&self) -> RowView {
        self.rows.target().zoomed(self.layout.zoom)
    }

    fn set_rows(&mut self, doc: &Document, mut rows: RowView, animate: Option<Instant>) {
        rows.clamp(self.cells_h(), self.row_count(doc), self.layout.zoom);
        let rows = rows.unzoomed(self.layout.zoom);
        match animate {
            Some(now) => self.rows.animate_to(rows, now, doc.navigation.animation),
            None => self.rows.set(rows),
        }
    }

    /// The row view a change builds on: the animation target when the
    /// change animates, else the displayed view; at the current zoom.
    fn rows_base(&self, animated: bool) -> RowView {
        if animated {
            self.rows_target()
        } else {
            self.rows.value.zoomed(self.layout.zoom)
        }
    }

    /// Whether a frame was laid out; zoom needs its areas and zoom factor.
    fn laid_out(&self) -> bool {
        self.layout.zoom > 0.0
    }

    fn fit_margin_px(&self) -> f64 {
        FIT_MARGIN_PX * f64::from(self.layout.zoom)
    }

    /// The whole trace with the fit margins, in this panel's cells width.
    fn fit_viewport(&self, doc: &Document) -> Viewport {
        Viewport::fit_px(doc.limits(), self.cells_w(), self.fit_margin_px())
    }

    /// Row height (at the current zoom) at which every row fits the cells
    /// area with the fit margins; infinite without rows.
    fn fit_row_px(&self, doc: &Document) -> f64 {
        let n = self.row_count(doc);
        if n == 0 {
            return f64::INFINITY;
        }
        let h = f64::from(self.cells_h());
        let inner = h - 2.0 * self.fit_margin_px();
        let inner = if inner >= h * 0.5 { inner } else { h };
        (inner / n as f64).max(f64::from(ROW_PX_MIN * self.layout.zoom))
    }

    /// Time units per cycle of the stream's clock around `t`; one without a
    /// clock, where the panel counts one time unit per cycle.
    fn cycle_units(&self, doc: &Document, t: f64) -> f64 {
        let (a, b) = doc.limits();
        let t = t.clamp(a as f64, b as f64) as u64;
        self.clock(doc)
            .and_then(|c| c.timeline())
            .and_then(|tl| {
                let at = tl.cycle_at(t)?;
                let period = match at.next_edge {
                    Some(next) => next - at.edge,
                    None => at.edge - tl.prev_edge(at.edge)?,
                };
                (period > 0).then_some(period as f64)
            })
            .unwrap_or(1.0)
    }

    /// The range of the two-axis zoom: time from the whole trace with its
    /// margins to [`CYCLE_PX_MAX`] per cycle; rows from where every row fits
    /// (no lower than the opening height when they all fit anyway) to the
    /// panel's cap.
    pub fn zoom_box(&self, doc: &Document, viewport: &Viewport) -> ZoomBox {
        let z = f64::from(self.layout.zoom);
        let w = self.cells_w();
        let t_lo = (w / self.fit_viewport(doc).width()).ln();
        let center = (viewport.start + viewport.end) * 0.5;
        let t_hi = (CYCLE_PX_MAX * z / self.cycle_units(doc, center))
            .ln()
            .max(t_lo);
        let cap = f64::from(self.row_cap) * z;
        let r_lo = self
            .fit_row_px(doc)
            .min(f64::from(ROW_PX_DEFAULT) * z)
            .min(cap);
        ZoomBox {
            time: (t_lo, t_hi),
            rows: (r_lo.ln(), cap.ln()),
        }
    }

    /// The zoom state of a viewport and a row view at the current zoom.
    fn zoom_point(&self, viewport: &Viewport, rows: RowView) -> ZoomPoint {
        ZoomPoint {
            time: (self.cells_w() / viewport.width()).ln(),
            rows: f64::from(rows.row_px).ln(),
        }
    }

    /// Adopt the aspect on screen unless an axis is at a limit, where the
    /// one from before it stopped is kept so the path can be walked back.
    fn update_aspect(&mut self, bounds: &ZoomBox, at: ZoomPoint) -> f64 {
        match self.aspect {
            Some(aspect) if !bounds.is_interior(at) => aspect,
            _ => *self.aspect.insert(at.aspect()),
        }
    }

    /// Immediate time-axis-only zoom matching the waveform panel's modified
    /// wheel gesture. The local row view and follow mode are unaffected.
    fn zoom_time_at(&mut self, doc: &mut Document, p: Point, factor: f64) {
        let x = f64::from((p.x - self.layout.cells.left()).max(0.0));
        self.nav.zoom_at(doc, x, self.cells_w(), factor);
        self.aspect = None;
    }

    /// Walk the two-axis zoom path by `factor` (> 1 zooms in) about a panel
    /// position; animated when `now` is given, otherwise immediate.
    pub fn zoom_about(&mut self, doc: &mut Document, p: Point, factor: f64, now: Option<Instant>) {
        let x = f64::from((p.x - self.layout.cells.left()).max(0.0));
        let y = (p.y - self.layout.cells.top()).max(0.0);
        self.zoom_path(doc, Some((x, y)), factor, now);
    }

    /// One step along the zoom path, anchored at cells-relative `(x, y)`, or
    /// for time at the visible cursor (else the centre) and for rows at the
    /// middle when `anchor` is `None`. An axis that reaches its fit snaps to
    /// the fit position so the whole trace, or every row, is on screen.
    fn zoom_path(
        &mut self,
        doc: &mut Document,
        anchor: Option<(f64, f32)>,
        factor: f64,
        now: Option<Instant>,
    ) {
        if !(self.laid_out() && factor.is_finite() && factor > 0.0) {
            return;
        }
        self.suspend_follow();
        let animated = now.is_some();
        let viewport = if animated {
            self.nav.viewport_state(doc).target()
        } else {
            self.nav.viewport(doc)
        };
        let mut rows = self.rows_base(animated);
        let bounds = self.zoom_box(doc, &viewport);
        let from = self.zoom_point(&viewport, rows);
        let aspect = self.update_aspect(&bounds, from);
        let to = zoom::walk(from, bounds, aspect, factor.ln());
        let w = self.cells_w();
        let h = self.cells_h();
        let time_factor = (to.time - from.time).exp();
        if to.time < from.time && ZoomBox::at_lo(to.time, bounds.time) {
            let fit = self.fit_viewport(doc);
            match now {
                Some(now) => self.nav.animate_to(doc, fit, now),
                None => self.nav.jump_to(doc, fit),
            }
        } else if to.time != from.time {
            match (anchor, now) {
                (Some((x, _)), Some(now)) => self.nav.zoom_target_at(doc, x, w, time_factor, now),
                (Some((x, _)), None) => self.nav.zoom_at(doc, x, w, time_factor),
                (None, Some(now)) => self.nav.zoom_center(doc, w, time_factor, now),
                (None, None) => self.nav.zoom_at(doc, w * 0.5, w, time_factor),
            }
        }
        if to.rows < from.rows && ZoomBox::at_lo(to.rows, bounds.rows) {
            rows.row_px = to.rows.exp() as f32;
            rows.top = -self.fit_margin_px() / f64::from(rows.row_px);
        } else if to.rows != from.rows {
            let y = anchor.map_or(h * 0.5, |(_, y)| y);
            rows.zoom_about(
                y,
                (to.rows - from.rows).exp(),
                h,
                self.row_count(doc),
                self.layout.zoom,
            );
        }
        self.set_rows(doc, rows, now);
    }

    /// Rows-only zoom by `factor` about `y` pixels below the top of the
    /// cells area: from where every row fits (no lower than the opening
    /// height when they all fit anyway) up to [`ROW_PX_MAX`]. The height
    /// becomes the panel's cap when it is above [`ROW_PX_CAP`].
    fn zoom_rows_at(&mut self, doc: &mut Document, y: f32, factor: f64, now: Option<Instant>) {
        if !(self.laid_out() && factor.is_finite() && factor > 0.0) {
            return;
        }
        self.suspend_follow();
        let z = self.layout.zoom;
        let mut rows = self.rows_base(now.is_some());
        let current = f64::from(rows.row_px);
        let lo = self
            .fit_row_px(doc)
            .min(f64::from(ROW_PX_DEFAULT * z))
            .min(current);
        let hi = f64::from(ROW_PX_MAX * z);
        let target = (current * factor).clamp(lo, hi);
        rows.zoom_about(y, target / current, self.cells_h(), self.row_count(doc), z);
        self.row_cap = (rows.row_px / z).max(ROW_PX_CAP);
        self.aspect = None;
        self.set_rows(doc, rows, now);
    }

    /// Increase / Decrease Row Height: a rows-only zoom step about the middle.
    pub fn step_row_height(&mut self, doc: &mut Document, delta: isize, now: Instant) {
        if delta == 0 {
            return self.reset_row_height(doc, now);
        }
        let factor = ROW_HEIGHT_STEP.powi(delta.signum() as i32);
        let y = self.cells_h() * 0.5;
        self.zoom_rows_at(doc, y, factor, Some(now));
    }

    /// Reset Row Height: the opening height and cap, about the middle.
    pub fn reset_row_height(&mut self, doc: &mut Document, now: Instant) {
        if !self.laid_out() {
            return;
        }
        self.suspend_follow();
        let z = self.layout.zoom;
        let mut rows = self.rows_target();
        let h = self.cells_h();
        let factor = f64::from(ROW_PX_DEFAULT * z) / f64::from(rows.row_px);
        rows.zoom_about(h * 0.5, factor, h, self.row_count(doc), z);
        self.row_cap = ROW_PX_CAP;
        self.aspect = None;
        self.set_rows(doc, rows, Some(now));
    }

    pub fn zoom_in(&mut self, doc: &mut Document, now: Instant) {
        self.zoom_path(doc, None, 2.0, Some(now));
    }

    pub fn zoom_out(&mut self, doc: &mut Document, now: Instant) {
        self.zoom_path(doc, None, 0.5, Some(now));
    }

    /// Zoom Fit: the whole trace with its margins and every row, shrinking
    /// rows only when they do not all fit already. The aspect from before is
    /// kept, so zooming back in returns to a readable view.
    pub fn zoom_fit(&mut self, doc: &mut Document, now: Instant) {
        if !self.laid_out() {
            return self.nav.zoom_fit(doc, self.cells_w(), 0.0, now);
        }
        let viewport = self.nav.viewport_state(doc).target();
        let mut rows = self.rows_target();
        let bounds = self.zoom_box(doc, &viewport);
        self.update_aspect(&bounds, self.zoom_point(&viewport, rows));
        let fit = self.fit_viewport(doc);
        self.nav.animate_to(doc, fit, now);
        if self.row_count(doc) > 0 {
            self.suspend_follow();
            rows.row_px = (f64::from(rows.row_px).min(self.fit_row_px(doc))) as f32;
            rows.top = -self.fit_margin_px() / f64::from(rows.row_px);
            self.set_rows(doc, rows, Some(now));
        }
    }

    pub fn zoom_to_cursor(&mut self, doc: &mut Document, now: Instant) {
        self.nav.zoom_to_cursor(doc, now);
    }

    pub fn go_to_start(&mut self, doc: &mut Document, now: Instant) {
        self.nav.go_to_start(doc, now);
    }

    pub fn go_to_end(&mut self, doc: &mut Document, now: Instant) {
        self.nav.go_to_end(doc, now);
    }

    pub fn go_to_cursor(&mut self, doc: &mut Document, now: Instant) {
        self.nav.go_to_cursor(doc, now);
    }

    pub fn pan_fraction(&mut self, doc: &mut Document, frac: f64, now: Instant) {
        self.nav.pan_fraction(doc, frac, now);
    }

    /// Scroll the rows by `rows` (animated).
    pub fn scroll_rows(&mut self, doc: &mut Document, rows: f64, now: Instant) {
        if rows != 0.0 {
            self.suspend_follow();
        }
        let mut target = self.rows_target();
        target.top += rows;
        self.set_rows(doc, target, Some(now));
    }

    /// Immediate pan of both axes by pixels (drag, trackpad scroll).
    fn pan_px(&mut self, doc: &mut Document, dx: f32, dy: f32) {
        if dx != 0.0 {
            let w = self.cells_w();
            self.nav.pan_px(doc, f64::from(dx), w);
        }
        if dy != 0.0 {
            self.suspend_follow();
            let mut rows = self.rows.value.zoomed(self.layout.zoom);
            rows.pan_px(dy, self.cells_h(), self.row_count(doc), self.layout.zoom);
            self.set_rows(doc, rows, None);
        }
    }

    /// Escape: cancel a drag, then clear the selection, then the cursor.
    /// Clearing the highlight never blanks a transaction panel.
    pub fn escape(&mut self, doc: &mut Document) {
        if self.drag.is_some() {
            self.drag = None;
        } else if self.selected_row(doc).is_some() {
            doc.select(None);
        } else {
            self.nav.set_cursor(doc, None);
        }
    }

    /// The clock this panel's stream counts its stages in (its `vtr.clock`).
    pub fn clock<'a>(&self, doc: &'a Document) -> Option<&'a crate::clock::Clock> {
        doc.clocks.linked(self.track.track()?)
    }

    /// The start of the cycle under panel x, clamped to the trace: the last
    /// edge of the stream's clock at or before it, or the whole time unit
    /// without a clock (a Kanata trace counts one unit per cycle).
    fn cycle_at(&self, doc: &Document, x: f32) -> u64 {
        let t = self.layout.time_at(&self.nav.viewport(doc), x).max(0.0);
        if let Some(at) = self
            .clock(doc)
            .and_then(|c| c.timeline())
            .and_then(|tl| tl.cycle_at(t as u64))
        {
            return at.edge;
        }
        let (a, b) = doc.limits();
        (t.floor() as u64).clamp(a, b)
    }

    /// `12–15 (3 cycles)`: an interval as cycles of the stream's clock, as
    /// the panel numbers them; `[12, 15)` in time units without a clock.
    fn span_text(&self, doc: &Document, begin: u64, end: u64) -> String {
        let Some(tl) = self.clock(doc).and_then(|c| c.timeline()) else {
            return format!("[{begin}, {end})");
        };
        let view = self.nav.clocks();
        let cycle = |t: u64| tl.cycle_at(t).map(|c| view.display_cycle(tl, c.cycle));
        match (cycle(begin), cycle(end)) {
            (Some(b), Some(e)) => {
                let n = e - b;
                format!(
                    "cycles {b}–{e} ({n} cycle{})",
                    if n == 1 { "" } else { "s" }
                )
            }
            _ => format!("[{begin}, {end})"),
        }
    }

    // -- pointer input -----------------------------------------------------------

    /// Handle pointer input. `panel` owns the selection a click writes.
    /// Returns true when something visible changed. Every pointer gesture is
    /// immediate, so `_now` (shared by all panel kinds) is unused.
    pub fn pointer(
        &mut self,
        doc: &mut Document,
        panel: PanelId,
        event: PointerEvent,
        now: Instant,
    ) -> bool {
        match event {
            PointerEvent::Down {
                position,
                button,
                modifiers,
            } => {
                self.pointer_down(doc, panel, position, button, modifiers, now);
                true
            }
            PointerEvent::Move { position } => self.pointer_move(doc, position),
            PointerEvent::Up => {
                let drag = self.drag.take();
                if let Some(Drag::Pan {
                    button: MouseButton::Left,
                    start,
                    moved: false,
                    ..
                }) = drag
                    && self.layout.cells.contains(start)
                {
                    // A click on a row selects it and puts the cursor on the
                    // cycle under the pointer; below the rows, cursor only.
                    let cycle = self.cycle_at(doc, start.x);
                    self.nav.set_cursor(doc, Some(cycle));
                    match self.layout.row_at(start.y) {
                        Some(row) => {
                            self.select_row(doc, panel, row);
                        }
                        None => {
                            doc.select(None);
                        }
                    }
                }
                drag.is_some()
            }
            PointerEvent::Leave => {
                let had = self.pointer.is_some() || self.hover.is_some();
                self.pointer = None;
                self.hover = None;
                had
            }
            PointerEvent::Wheel {
                position,
                dx,
                dy,
                modifiers,
                ..
            } => {
                self.wheel(doc, position, dx, dy, modifiers);
                true
            }
            PointerEvent::Pinch { position, delta } => {
                let factor = f64::from(1.0 + delta).clamp(0.2, 5.0);
                self.zoom_about(doc, position, factor, None);
                true
            }
        }
    }

    fn pointer_down(
        &mut self,
        doc: &mut Document,
        panel: PanelId,
        p: Point,
        button: MouseButton,
        modifiers: Modifiers,
        now: Instant,
    ) {
        self.pointer = Some(p);
        if button == MouseButton::Left
            && let Some(Hit::Activity(command)) = self.hit_at(p, doc)
        {
            self.activity_command(doc, command);
            return;
        }
        let lane_hit = self.layout.marker_lane.hit(p);
        let layout = &self.layout;
        if button == MouseButton::Left {
            if layout.label_split.contains(p) {
                self.drag = Some(Drag::LabelSplit);
                return;
            }
            if layout.retry.is_some_and(|r| r.contains(p)) {
                self.retry(doc);
                return;
            }
            if let Some(hit) = lane_hit {
                crate::marker::press(doc, &mut self.nav, hit, modifiers, now);
                return;
            }
            // A press on a clock ruler selects its clock, then works like the header.
            let rulers: Vec<String> = self
                .nav
                .clocks()
                .rulers(&doc.clocks)
                .iter()
                .map(|c| c.path.clone())
                .collect();
            let ruler = layout.ruler_at(p, rulers.len());
            if let Some(ix) = ruler {
                self.nav.select_clock(&rulers[ix]);
            }
            let strip = layout.header.contains(p) || layout.marker_lane.band.contains(p);
            if (strip || ruler.is_some()) && p.x >= layout.cells.left() {
                let cycle = self.cycle_at(doc, p.x);
                self.nav.set_cursor(doc, Some(cycle));
                self.drag = Some(Drag::Cursor);
                return;
            }
            if ruler.is_some() {
                return;
            }
            // The label column has no time under the pointer, so a click
            // there selects the row without moving the cursor.
            if layout.labels.contains(p)
                && let Some(row) = layout.row_at(p.y)
            {
                self.select_row(doc, panel, row);
                return;
            }
        }
        if layout.cells.contains(p) {
            self.drag = Some(Drag::Pan {
                button,
                start: p,
                last: p,
                moved: button != MouseButton::Left,
            });
        }
    }

    fn pointer_move(&mut self, doc: &mut Document, p: Point) -> bool {
        self.pointer = Some(p);
        match self.drag {
            Some(Drag::Pan {
                button,
                start,
                last,
                moved,
            }) => {
                let threshold = DRAG_THRESHOLD_PX * self.layout.zoom;
                let moved = moved || (p.x - start.x).hypot(p.y - start.y) > threshold;
                self.drag = Some(Drag::Pan {
                    button,
                    start,
                    last: p,
                    moved,
                });
                if moved {
                    self.pan_px(doc, last.x - p.x, last.y - p.y);
                }
                moved
            }
            Some(Drag::Cursor) => {
                let x =
                    p.x.clamp(self.layout.cells.left(), self.layout.cells.right());
                let cycle = self.cycle_at(doc, x);
                self.nav.set_cursor(doc, Some(cycle))
            }
            Some(Drag::LabelSplit) => {
                self.label_width = ((p.x - self.layout.bounds.left()) / self.layout.zoom)
                    .clamp(LABEL_W_MIN, LABEL_W_MAX);
                true
            }
            None => {
                let before = self.hover;
                self.hover = self.hit_at(p, doc);
                self.hover != before
            }
        }
    }

    /// Ctrl/⌘+wheel (and a browser pinch, which arrives as one) walks the
    /// two-axis zoom path, or zooms time only over the time header and
    /// rulers; Alt+wheel zooms rows only; a plain wheel scrolls rows and pans
    /// time with its horizontal delta; Shift+wheel pans time. Every host
    /// behaves the same, whether its deltas are a wheel's or a trackpad's.
    fn wheel(&mut self, doc: &mut Document, p: Point, dx: f32, dy: f32, modifiers: Modifiers) {
        let factor = 2f64.powf(f64::from(dy) / 120.0);
        if modifiers.control || modifiers.platform {
            if p.y < self.layout.cells.top() {
                self.zoom_time_at(doc, p, factor);
            } else {
                self.zoom_about(doc, p, factor, None);
            }
        } else if modifiers.alt {
            let y = (p.y - self.layout.cells.top()).max(0.0);
            self.zoom_rows_at(doc, y, factor, None);
        } else if modifiers.shift {
            // Some hosts translate Shift-wheel's vertical delta to horizontal.
            self.pan_px(doc, -(dx + dy), 0.0);
        } else {
            self.pan_px(doc, -dx, -dy);
        }
    }
}
