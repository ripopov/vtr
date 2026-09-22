//! `PipelineModel`: the state of a pipeline panel (the track it shows, its
//! navigation, the row axis, the label column, hover and drags) and every
//! input rule that mutates it. Data is the document's loaded track; the model
//! never copies records.

use std::sync::Arc;

use web_time::Instant;

use super::layout::{LABEL_W_DEFAULT, LABEL_W_MAX, LABEL_W_MIN, LayoutInput, PipelineLayout};
use super::palette::StagePalette;
use super::rows::RowView;
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

/// The per-transaction caption attribute (`docs/SPEC.md`).
pub const LABEL_ATTRIBUTE: &str = "vtr.label";
/// A drag shorter than this (at zoom 1.0) is a click.
const DRAG_THRESHOLD_PX: f32 = 3.0;
/// Rows scrolled by one keyboard step.
pub const SCROLL_ROWS: f64 = 3.0;

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
    Cell { row: usize, stage: usize },
    LabelSplit,
    Header,
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
    /// Design pixels, zoom applied at layout.
    pub label_width: f32,
    pub hover: Option<Hit>,
    pub drag: Option<Drag>,
    /// Last known pointer position over the panel.
    pub pointer: Option<Point>,
    /// Duration of the last panel paint, and a smoothed average.
    pub frames_painted: u64,
    pub frame_ms: f32,
    pub frame_ms_avg: f32,
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
            label_width: LABEL_W_DEFAULT,
            hover: None,
            drag: None,
            pointer: None,
            frames_painted: 0,
            frame_ms: 0.0,
            frame_ms_avg: 0.0,
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

    /// Record the paint time of the last frame.
    pub fn record_frame(&mut self, ms: f32) {
        self.frames_painted += 1;
        self.frame_ms = ms;
        self.frame_ms_avg = if self.frame_ms_avg == 0.0 {
            ms
        } else {
            self.frame_ms_avg * 0.9 + ms * 0.1
        };
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
                    " · {} [{}, {})",
                    s.name,
                    s.begin,
                    Self::stage_end(tx, s)
                ));
                if s.lane != self.palette.primary_lane() {
                    text.push_str(&format!(" lane {}", s.lane));
                }
            }
            None => text.push_str(&format!(" · [{}, {}]", tx.begin, tx.end)),
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
        let input = LayoutInput {
            bounds,
            header_h: theme.timeline_height,
            zoom: theme.zoom,
            label_width: self.label_width,
            rows: self.rows.value,
            row_count,
            markers: &doc.markers,
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
        if layout.header.contains(p) {
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

    /// Immediate time-axis-only zoom matching the waveform panel's modified
    /// wheel gesture. The local row view and follow mode are unaffected.
    fn zoom_time_at(&mut self, doc: &mut Document, p: Point, factor: f64) {
        let x = f64::from((p.x - self.layout.cells.left()).max(0.0));
        self.nav.zoom_at(doc, x, self.cells_w(), factor);
    }

    /// Zoom both axes by `factor` about a panel position; animated when
    /// `now` is given, otherwise immediate.
    pub fn zoom_about(&mut self, doc: &mut Document, p: Point, factor: f64, now: Option<Instant>) {
        self.suspend_follow();
        let x = f64::from((p.x - self.layout.cells.left()).max(0.0));
        let y = (p.y - self.layout.cells.top()).max(0.0);
        let w = self.cells_w();
        match now {
            Some(now) => self.nav.zoom_target_at(doc, x, w, factor, now),
            None => self.nav.zoom_at(doc, x, w, factor),
        }
        let mut rows = if now.is_some() {
            self.rows_target()
        } else {
            self.rows.value.zoomed(self.layout.zoom)
        };
        rows.zoom_about(
            y,
            factor,
            self.cells_h(),
            self.row_count(doc),
            self.layout.zoom,
        );
        self.set_rows(doc, rows, now);
    }

    /// Keyboard zoom: time about the cursor when visible (else the centre),
    /// rows about the middle of the cells area.
    fn zoom_center(&mut self, doc: &mut Document, factor: f64, now: Instant) {
        self.suspend_follow();
        let w = self.cells_w();
        self.nav.zoom_center(doc, w, factor, now);
        let mut rows = self.rows_target();
        rows.zoom_about(
            self.cells_h() / 2.0,
            factor,
            self.cells_h(),
            self.row_count(doc),
            self.layout.zoom,
        );
        self.set_rows(doc, rows, Some(now));
    }

    pub fn zoom_in(&mut self, doc: &mut Document, now: Instant) {
        self.zoom_center(doc, 2.0, now);
    }

    pub fn zoom_out(&mut self, doc: &mut Document, now: Instant) {
        self.zoom_center(doc, 0.5, now);
    }

    pub fn zoom_fit(&mut self, doc: &mut Document, now: Instant) {
        self.nav.zoom_fit(doc, now);
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

    /// The integer cycle under panel x, clamped to the trace.
    fn cycle_at(&self, doc: &Document, x: f32) -> u64 {
        let t = self.layout.time_at(&self.nav.viewport(doc), x).floor();
        let (a, b) = doc.limits();
        (t.max(0.0) as u64).clamp(a, b)
    }

    // -- pointer input -----------------------------------------------------------

    /// Handle pointer input. `panel` owns the selection a click writes.
    /// Returns true when something visible changed.
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
                self.pointer_down(doc, panel, position, button, modifiers);
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
                precise,
            } => {
                self.wheel(doc, position, dx, dy, modifiers, precise, now);
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
    ) {
        self.pointer = Some(p);
        if button == MouseButton::Left
            && let Some(Hit::Activity(command)) = self.hit_at(p, doc)
        {
            self.activity_command(doc, command);
            return;
        }
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
            if let Some(ix) = layout.chip_at(p) {
                if modifiers.shift {
                    doc.remove_marker(ix);
                } else {
                    let t = doc.markers[ix].time;
                    self.nav.set_cursor(doc, Some(t));
                }
                return;
            }
            if layout.header.contains(p) && p.x >= layout.cells.left() {
                let cycle = self.cycle_at(doc, p.x);
                self.nav.set_cursor(doc, Some(cycle));
                self.drag = Some(Drag::Cursor);
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

    #[allow(clippy::too_many_arguments)]
    fn wheel(
        &mut self,
        doc: &mut Document,
        p: Point,
        dx: f32,
        dy: f32,
        modifiers: Modifiers,
        precise: bool,
        now: Instant,
    ) {
        if modifiers.control || modifiers.platform {
            let factor = 2f64.powf(f64::from(dy) / 120.0);
            self.zoom_time_at(doc, p, factor);
        } else if modifiers.shift {
            // Some hosts translate Shift-wheel's vertical delta to horizontal.
            self.pan_px(doc, -(dx + dy), 0.0);
        } else if precise {
            self.pan_px(doc, -dx, -dy);
        } else {
            // An unmodified mouse wheel zooms both axes.
            let factor = 2f64.powf(f64::from(dy.clamp(-100.0, 100.0)) / 100.0);
            self.zoom_about(doc, p, factor, Some(now));
        }
    }
}
