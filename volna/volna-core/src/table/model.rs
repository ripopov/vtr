use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::sync::Arc;

use web_time::Instant;

use super::columns::{ColumnSet, TransactionColumn};
use super::layout::{MAX_PREPARED_ROWS, ROW_HEIGHT, RowViewport, TableLayout};
use super::source::TableSource;
use crate::data::loaded_tracks::LoadedGenerator;
use crate::data::text::{append_exact, format_attribute, push_limited, truncate, truncate_ref};
use crate::data::transactions::{Transaction, TransactionRef};
use crate::data::value_view::ValueView;
use crate::data::{SignalHistory, SignalRef};
use crate::document::{Document, TrackLoadState};
use crate::geometry::{MouseButton, Rect};
use crate::nav::{Link, NavState};
use crate::remote::memory::{MemoryBudget, Reservation};
use crate::theme::Theme;
use crate::wave::model::PointerEvent;

pub use crate::data::text::{COPY_BYTES, PREVIEW_BYTES};

pub const PANEL_BYTES: u64 = 4 * 1024 * 1024;
pub const PREVIEW_ATTRIBUTES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowIdentity {
    Transaction(TransactionRef),
    SignalTime(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedCell {
    pub text: String,
    pub changed: bool,
    pub missing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRow {
    pub ordinal: u64,
    pub identity: RowIdentity,
    pub time: u64,
    pub cells: Vec<PreparedCell>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedWindow {
    pub start: u64,
    pub end: u64,
    pub rows: Vec<PreparedRow>,
    pub bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccessibleRow {
    pub ordinal: u64,
    pub label: String,
    pub selected: bool,
    pub bounds: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableState {
    Loading,
    Ready,
    Empty,
    Failed(String),
    Refused(String),
    Unavailable,
}

enum Rows {
    None,
    Generator(Arc<LoadedGenerator>),
    Signals {
        histories: Vec<Arc<dyn SignalHistory>>,
        axis: Option<Arc<[u64]>>,
        _axis_reservation: Option<Reservation>,
    },
}

struct SignalAxisBuild {
    histories: Vec<Arc<dyn SignalHistory>>,
    cursor: Vec<usize>,
    heap: BinaryHeap<Reverse<(u64, usize)>>,
    axis: Vec<u64>,
    upper_bytes: u64,
    reservation: Reservation,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TableDrag {
    Vertical { grab: f32 },
    Horizontal { grab: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TableCommand {
    Scroll(f32),
    Select(u64),
    GoTo(u64),
    First,
    Last,
    Previous,
    Next,
    Page(i8),
    ToggleTransactionColumn(TransactionColumn),
    ToggleSignalColumn(usize),
    ResetColumns,
    ClearSelection,
    Cancel,
    Retry,
}

pub struct TableModel {
    pub source: TableSource,
    pub state: TableState,
    pub columns: ColumnSet,
    pub viewport: RowViewport,
    pub selected: Option<RowIdentity>,
    pub nav: NavState,
    pub window: PreparedWindow,
    pub horizontal: f32,
    pub layout: TableLayout,
    pub request: u64,
    drag: Option<TableDrag>,
    rows: Rows,
    axis_build: Option<SignalAxisBuild>,
    signal_results: HashMap<SignalRef, Result<Arc<dyn SignalHistory>, String>>,
    attached: bool,
    budget: MemoryBudget,
    _panel_reservation: Option<Reservation>,
}

impl TableModel {
    pub fn new(source: TableSource, link: Link, budget: MemoryBudget) -> Self {
        let columns = match &source {
            TableSource::Generator(_) => ColumnSet::transactions_default(),
            TableSource::Signals(signals) => ColumnSet::signals(signals.len()),
        };
        let reservation = budget.reserve(PANEL_BYTES);
        let state = match &reservation {
            Ok(_) => TableState::Loading,
            Err(error) => TableState::Refused(format!(
                "Table needs {PANEL_BYTES} bytes; admission failed: {error}"
            )),
        };
        let mut nav = NavState::new();
        nav.link = link;
        Self {
            source,
            state,
            columns,
            viewport: RowViewport::default(),
            selected: None,
            nav,
            window: PreparedWindow::default(),
            horizontal: 0.0,
            layout: TableLayout::default(),
            request: 0,
            drag: None,
            rows: Rows::None,
            axis_build: None,
            signal_results: HashMap::new(),
            attached: false,
            budget,
            _panel_reservation: reservation.ok(),
        }
    }

    /// Copy only persistent panel choices. Source data is retained through the
    /// document again when the split is installed; viewport and selection
    /// intentionally reopen at row one.
    pub fn clone_view(&self) -> Self {
        let mut clone = Self::new(self.source.clone(), self.nav.link, self.budget.clone());
        clone.columns = self.columns.clone();
        clone
    }

    pub fn attach(
        &mut self,
        doc: &mut Document,
        resident: &HashMap<SignalRef, Arc<dyn SignalHistory>>,
    ) -> anyhow::Result<()> {
        if self.attached || matches!(self.state, TableState::Refused(_)) {
            return Ok(());
        }
        self.attached = true;
        self.nav.reset(Some(doc.limits()));
        match &mut self.source {
            TableSource::Generator(source) => {
                let Some(track) = source.track() else {
                    self.state = TableState::Unavailable;
                    return Ok(());
                };
                doc.retain_track(track)?;
                self.refresh(doc);
            }
            TableSource::Signals(sources) => {
                let Some(hierarchy) = doc.hierarchy() else {
                    self.state = TableState::Unavailable;
                    return Ok(());
                };
                if sources.iter_mut().any(|source| !source.resolve(hierarchy)) {
                    self.state = TableState::Unavailable;
                    return Ok(());
                }
                for signal in sources.iter().filter_map(|source| source.signal) {
                    if let Some(history) = resident.get(&signal) {
                        self.signal_results.insert(signal, Ok(history.clone()));
                    } else {
                        doc.request_signal(signal);
                    }
                }
                self.refresh(doc);
            }
        }
        Ok(())
    }

    pub fn detach(&mut self, doc: &mut Document) {
        if !self.attached {
            return;
        }
        if let TableSource::Generator(source) = &self.source
            && let Some(track) = source.track()
        {
            doc.release_track(track);
        }
        self.attached = false;
        self.rows = Rows::None;
        self.axis_build = None;
        self.signal_results.clear();
        self.window = PreparedWindow::default();
        self.request = self.request.wrapping_add(1);
    }

    pub fn signal_demand(&self) -> Box<dyn Iterator<Item = SignalRef> + '_> {
        if !self.attached {
            return Box::new(std::iter::empty());
        }
        match &self.source {
            TableSource::Signals(sources) => Box::new(sources.iter().filter_map(|s| s.signal)),
            _ => Box::new(std::iter::empty()),
        }
    }

    pub fn histories(&self) -> Vec<(SignalRef, Arc<dyn SignalHistory>)> {
        self.signal_results
            .iter()
            .filter_map(|(&signal, result)| result.as_ref().ok().map(|h| (signal, h.clone())))
            .collect()
    }

    pub fn finish_signal(
        &mut self,
        signal: SignalRef,
        result: Result<Arc<dyn SignalHistory>, String>,
    ) {
        if self.attached && self.signal_demand().any(|wanted| wanted == signal) {
            self.signal_results.insert(signal, result);
        }
    }

    pub fn refresh(&mut self, doc: &Document) {
        if matches!(self.state, TableState::Refused(_)) || !self.attached {
            return;
        }
        match &self.source {
            TableSource::Generator(source) => {
                let Some(track) = source.track() else {
                    self.state = TableState::Unavailable;
                    return;
                };
                match doc.track(track) {
                    Some(TrackLoadState::Loading) => self.state = TableState::Loading,
                    Some(TrackLoadState::Failed(error)) => {
                        self.state = TableState::Failed(error.clone())
                    }
                    Some(TrackLoadState::Ready(loaded)) => {
                        let Some(generator) = loaded
                            .generators
                            .iter()
                            .find(|g| g.generator() == track)
                            .cloned()
                        else {
                            self.state = TableState::Unavailable;
                            return;
                        };
                        let empty = generator.transactions().is_empty();
                        self.rows = Rows::Generator(generator);
                        self.state = if empty {
                            TableState::Empty
                        } else {
                            TableState::Ready
                        };
                        self.invalidate_window();
                    }
                    None => self.state = TableState::Unavailable,
                }
            }
            TableSource::Signals(sources) => {
                if self.axis_build.is_some() {
                    self.state = TableState::Loading;
                    return;
                }
                if matches!(self.rows, Rows::Signals { .. })
                    && matches!(self.state, TableState::Ready | TableState::Empty)
                {
                    return;
                }
                let mut histories = Vec::with_capacity(sources.len());
                for source in sources {
                    let Some(signal) = source.signal else {
                        self.state = TableState::Unavailable;
                        return;
                    };
                    match self.signal_results.get(&signal) {
                        Some(Ok(history)) => histories.push(history.clone()),
                        Some(Err(error)) => {
                            self.state = TableState::Failed(error.clone());
                            return;
                        }
                        None => {
                            self.state = TableState::Loading;
                            return;
                        }
                    }
                }
                let started = self.begin_signal_rows(histories).and_then(|ready| {
                    if ready.is_none() {
                        self.advance_signal_axis(4096)
                    } else {
                        Ok(ready)
                    }
                });
                match started {
                    Ok(Some(empty)) => {
                        self.state = if empty {
                            TableState::Empty
                        } else {
                            TableState::Ready
                        }
                    }
                    Ok(None) => self.state = TableState::Loading,
                    Err(error) => self.state = TableState::Refused(format!("{error:#}")),
                }
                self.invalidate_window();
            }
        }
    }

    #[cfg(test)]
    fn build_signal_rows(
        &mut self,
        histories: Vec<Arc<dyn SignalHistory>>,
    ) -> anyhow::Result<bool> {
        if let Some(empty) = self.begin_signal_rows(histories)? {
            return Ok(empty);
        }
        loop {
            if let Some(empty) = self.advance_signal_axis(usize::MAX)? {
                return Ok(empty);
            }
        }
    }

    fn begin_signal_rows(
        &mut self,
        histories: Vec<Arc<dyn SignalHistory>>,
    ) -> anyhow::Result<Option<bool>> {
        if histories.len() <= 1 {
            let empty = histories.first().is_none_or(|h| h.is_empty());
            self.rows = Rows::Signals {
                histories,
                axis: None,
                _axis_reservation: None,
            };
            return Ok(Some(empty));
        }
        let upper = histories.iter().try_fold(0u64, |sum, history| {
            sum.checked_add(history.len() as u64)
                .ok_or_else(|| anyhow::anyhow!("signal axis size overflow"))
        })?;
        let upper_bytes = upper
            .checked_mul(8)
            .ok_or_else(|| anyhow::anyhow!("signal axis size overflow"))?;
        let reservation = self.budget.reserve(upper_bytes).map_err(|error| {
            anyhow::anyhow!(
                "Signal axis needs at most {upper_bytes} bytes before merge; admission failed: {error}"
            )
        })?;
        let cursor = vec![0usize; histories.len()];
        let mut heap = BinaryHeap::new();
        for (index, history) in histories.iter().enumerate() {
            if !history.is_empty() {
                heap.push(Reverse((history.time(0), index)));
            }
        }
        let mut axis = Vec::new();
        axis.try_reserve_exact(usize::try_from(upper)?)?;
        self.axis_build = Some(SignalAxisBuild {
            histories,
            cursor,
            heap,
            axis,
            upper_bytes,
            reservation,
        });
        Ok(None)
    }

    fn advance_signal_axis(&mut self, limit: usize) -> anyhow::Result<Option<bool>> {
        let Some(build) = self.axis_build.as_mut() else {
            return Ok(None);
        };
        for _ in 0..limit {
            let Some(Reverse((next, signal))) = build.heap.pop() else {
                let SignalAxisBuild {
                    histories,
                    mut axis,
                    upper_bytes,
                    mut reservation,
                    ..
                } = self.axis_build.take().expect("active axis build");
                let actual = (axis.len() as u64)
                    .checked_mul(8)
                    .ok_or_else(|| anyhow::anyhow!("signal axis size overflow"))?;
                reservation.shrink(upper_bytes - actual)?;
                axis.shrink_to_fit();
                let empty = axis.is_empty();
                self.rows = Rows::Signals {
                    histories,
                    axis: Some(axis.into()),
                    _axis_reservation: Some(reservation),
                };
                return Ok(Some(empty));
            };
            if build.axis.last().copied() != Some(next) {
                build.axis.push(next);
            }
            let history = &build.histories[signal];
            let index = &mut build.cursor[signal];
            while *index < history.len() && history.time(*index) == next {
                *index += 1;
            }
            if *index < history.len() {
                build.heap.push(Reverse((history.time(*index), signal)));
            }
        }
        Ok(None)
    }

    /// Advance bounded cooperative work. Hosts call this through the ordinary
    /// animation tick, so wasm and native never merge an unbounded axis in one
    /// UI task; dropping the panel drops the builder and its reservation.
    pub fn tick(&mut self, now: Instant) -> bool {
        let changed = self.nav.tick(now);
        if self.axis_build.is_none() {
            return changed;
        }
        match self.advance_signal_axis(4096) {
            Ok(Some(empty)) => {
                self.state = if empty {
                    TableState::Empty
                } else {
                    TableState::Ready
                };
                self.invalidate_window();
                true
            }
            Ok(None) => true,
            Err(error) => {
                self.axis_build = None;
                self.state = TableState::Refused(format!("{error:#}"));
                true
            }
        }
    }

    pub fn is_animating(&self) -> bool {
        self.axis_build.is_some() || self.nav.is_animating()
    }

    fn invalidate_window(&mut self) {
        self.request = self.request.wrapping_add(1);
        self.window = PreparedWindow::default();
    }

    pub fn len(&self) -> u64 {
        match &self.rows {
            Rows::None => 0,
            Rows::Generator(generator) => generator.transactions().len() as u64,
            Rows::Signals {
                histories, axis, ..
            } => axis.as_ref().map_or_else(
                || histories.first().map_or(0, |h| h.len() as u64),
                |a| a.len() as u64,
            ),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn status(&self) -> String {
        match &self.state {
            TableState::Loading => "Loading source…".into(),
            TableState::Ready => format!("{} rows", self.len()),
            TableState::Empty => "Source is empty".into(),
            TableState::Failed(error) => format!("Load failed: {error}"),
            TableState::Refused(error) => format!("Load refused: {error}"),
            TableState::Unavailable => "Source unavailable".into(),
        }
    }

    pub fn row_time(&self, ordinal: u64) -> Option<u64> {
        let index = usize::try_from(ordinal).ok()?;
        match &self.rows {
            Rows::Generator(generator) => generator.transactions().get(index).map(|tx| tx.begin),
            Rows::Signals {
                histories, axis, ..
            } => axis
                .as_ref()
                .and_then(|axis| axis.get(index).copied())
                .or_else(|| {
                    histories
                        .first()
                        .filter(|_| axis.is_none())
                        .map(|h| h.time(index))
                }),
            Rows::None => None,
        }
    }

    fn ordinal_of(&self, identity: &RowIdentity) -> Option<u64> {
        match (&self.rows, identity) {
            (Rows::Generator(generator), RowIdentity::Transaction(id)) => {
                generator.transaction_ordinal(*id).map(|x| x as u64)
            }
            (
                Rows::Signals {
                    histories, axis, ..
                },
                RowIdentity::SignalTime(time),
            ) => axis
                .as_ref()
                .map(|a| a.binary_search(time).ok().map(|x| x as u64))
                .unwrap_or_else(|| {
                    histories.first().and_then(|h| {
                        let i = h.index_at(*time)?;
                        (h.time(i) == *time).then_some(i as u64)
                    })
                }),
            _ => None,
        }
    }

    pub fn selected_ordinal(&self) -> Option<u64> {
        self.selected
            .as_ref()
            .and_then(|identity| self.ordinal_of(identity))
    }

    pub fn select(
        &mut self,
        doc: &mut Document,
        panel: crate::panels::PanelId,
        ordinal: u64,
        now: Instant,
    ) -> bool {
        let Some((identity, time)) = self.identity_and_time(ordinal) else {
            return false;
        };
        // A selected record is the document's, so every panel over the same
        // generator highlights it and the transaction panel shows it.
        if let (RowIdentity::Transaction(id), Rows::Generator(generator)) = (&identity, &self.rows)
        {
            doc.select(Some(crate::document::TxSelection {
                track: generator.generator(),
                id: *id,
                origin: panel,
            }));
        }
        self.selected = Some(identity);
        self.viewport.reveal(
            ordinal,
            self.len(),
            self.layout.body.height(),
            self.layout.row_height.max(ROW_HEIGHT),
        );
        self.nav.set_cursor(doc, Some(time));
        self.nav.reveal_cursor(doc, now);
        true
    }

    /// Adopt the document selection when it names a record of this table.
    /// Returns whether the highlighted row changed.
    pub fn follow_selection(&mut self, doc: &Document) -> bool {
        let Rows::Generator(generator) = &self.rows else {
            return false;
        };
        let wanted = match doc.selection() {
            Some(selection) if selection.track == generator.generator() => {
                Some(RowIdentity::Transaction(selection.id))
            }
            Some(_) => return false,
            None => None,
        };
        let changed = self.selected != wanted;
        self.selected = wanted;
        changed
    }

    fn identity_and_time(&self, ordinal: u64) -> Option<(RowIdentity, u64)> {
        let index = usize::try_from(ordinal).ok()?;
        match &self.rows {
            Rows::Generator(generator) => {
                let tx = generator.transactions().get(index)?;
                Some((RowIdentity::Transaction(tx.id), tx.begin))
            }
            Rows::Signals { .. } => {
                let time = self.row_time(ordinal)?;
                Some((RowIdentity::SignalTime(time), time))
            }
            Rows::None => None,
        }
    }

    pub fn command(
        &mut self,
        doc: &mut Document,
        panel: crate::panels::PanelId,
        command: TableCommand,
        now: Instant,
    ) -> bool {
        match command {
            TableCommand::Scroll(pixels) => {
                self.viewport.scroll(
                    pixels,
                    self.len(),
                    self.layout.body.height(),
                    self.layout.row_height.max(ROW_HEIGHT),
                );
                self.prepare_visible();
                true
            }
            TableCommand::Select(row) | TableCommand::GoTo(row) => {
                self.select(doc, panel, row, now)
            }
            TableCommand::First => self.select(doc, panel, 0, now),
            TableCommand::Last => self
                .len()
                .checked_sub(1)
                .is_some_and(|row| self.select(doc, panel, row, now)),
            TableCommand::Previous => self
                .selected_ordinal()
                .and_then(|r| r.checked_sub(1))
                .is_some_and(|r| self.select(doc, panel, r, now)),
            TableCommand::Next => self
                .selected_ordinal()
                .unwrap_or(0)
                .checked_add(1)
                .filter(|&r| r < self.len())
                .is_some_and(|r| self.select(doc, panel, r, now)),
            TableCommand::Page(direction) => {
                let page = RowViewport::visible_rows(
                    self.layout.body.height(),
                    self.layout.row_height.max(ROW_HEIGHT),
                )
                .max(1);
                let current = self.selected_ordinal().unwrap_or(self.viewport.top);
                let target = if direction < 0 {
                    current.saturating_sub(page)
                } else {
                    current
                        .saturating_add(page)
                        .min(self.len().saturating_sub(1))
                };
                self.select(doc, panel, target, now)
            }
            TableCommand::ToggleTransactionColumn(column) => {
                let changed = self.columns.toggle_transaction(column);
                if changed {
                    self.invalidate_window();
                }
                changed
            }
            TableCommand::ToggleSignalColumn(index) => {
                let ColumnSet::Signals { time, visible } = &mut self.columns else {
                    return false;
                };
                if index == 0 {
                    if visible.iter().any(|v| *v) {
                        *time = !*time;
                    } else {
                        return false;
                    }
                } else if index - 1 < visible.len() {
                    let only_data_column = !*time && visible.iter().filter(|v| **v).count() == 1;
                    let value = &mut visible[index - 1];
                    if *value && only_data_column {
                        return false;
                    }
                    *value = !*value;
                } else {
                    return false;
                }
                self.invalidate_window();
                true
            }
            TableCommand::ResetColumns => {
                self.columns.reset();
                self.invalidate_window();
                true
            }
            TableCommand::ClearSelection => {
                let changed = self.selected.take().is_some();
                if changed && doc.selection().is_some_and(|s| s.origin == panel) {
                    doc.select(None);
                }
                changed
            }
            TableCommand::Cancel => {
                self.detach(doc);
                self.state = TableState::Failed("Cancelled".into());
                true
            }
            TableCommand::Retry => self.retry(doc),
        }
    }

    fn retry(&mut self, doc: &mut Document) -> bool {
        if self._panel_reservation.is_none() {
            match self.budget.reserve(PANEL_BYTES) {
                Ok(reservation) => {
                    self._panel_reservation = Some(reservation);
                    self.state = TableState::Loading;
                }
                Err(error) => {
                    self.state = TableState::Refused(format!(
                        "Table needs {PANEL_BYTES} bytes; admission failed: {error}"
                    ));
                    return true;
                }
            }
        }
        if !self.attached {
            let resident = HashMap::new();
            return self.attach(doc, &resident).is_ok();
        }
        if matches!(self.state, TableState::Refused(_)) {
            self.state = TableState::Loading;
            self.rows = Rows::None;
            self.axis_build = None;
            self.refresh(doc);
            return true;
        }
        match &self.source {
            TableSource::Generator(source) => {
                source.track().is_some_and(|track| doc.retry_track(track))
            }
            TableSource::Signals(sources) => {
                let mut requested = false;
                for signal in sources.iter().filter_map(|s| s.signal) {
                    if self.signal_results.get(&signal).is_some_and(Result::is_err) {
                        self.signal_results.remove(&signal);
                        requested |= doc.request_signal(signal);
                    }
                }
                if requested {
                    self.state = TableState::Loading;
                }
                requested
            }
        }
    }

    pub fn pointer(
        &mut self,
        doc: &mut Document,
        panel: crate::panels::PanelId,
        event: PointerEvent,
        now: Instant,
    ) -> bool {
        match event {
            PointerEvent::Down {
                position,
                button: MouseButton::Left,
                ..
            } if self.layout.vertical_bar.contains(position) => {
                let grab = if self.layout.vertical_thumb.contains(position) {
                    position.y - self.layout.vertical_thumb.top()
                } else {
                    self.layout.vertical_thumb.height() * 0.5
                };
                self.drag = Some(TableDrag::Vertical { grab });
                self.drag_vertical(position.y, grab);
                self.prepare_visible();
                true
            }
            PointerEvent::Down {
                position,
                button: MouseButton::Left,
                ..
            } if self.layout.horizontal_bar.contains(position) => {
                let grab = if self.layout.horizontal_thumb.contains(position) {
                    position.x - self.layout.horizontal_thumb.left()
                } else {
                    self.layout.horizontal_thumb.width() * 0.5
                };
                self.drag = Some(TableDrag::Horizontal { grab });
                self.drag_horizontal(position.x, grab);
                true
            }
            PointerEvent::Down {
                position,
                button: MouseButton::Left,
                ..
            } if self.layout.body.contains(position) || self.layout.gutter.contains(position) => {
                let y = position.y - self.layout.body.top();
                self.viewport
                    .row_at(y, self.len(), self.layout.row_height)
                    .is_some_and(|row| self.select(doc, panel, row, now))
            }
            PointerEvent::Wheel { dx, dy, .. } => {
                self.horizontal = (self.horizontal + dx).max(0.0);
                self.viewport.scroll(
                    dy,
                    self.len(),
                    self.layout.body.height(),
                    self.layout.row_height,
                );
                self.prepare_visible();
                true
            }
            PointerEvent::Move { position } => match self.drag {
                Some(TableDrag::Vertical { grab }) => {
                    self.drag_vertical(position.y, grab);
                    self.prepare_visible();
                    true
                }
                Some(TableDrag::Horizontal { grab }) => {
                    self.drag_horizontal(position.x, grab);
                    true
                }
                None => false,
            },
            PointerEvent::Up | PointerEvent::Leave if self.drag.take().is_some() => true,
            _ => false,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn drag_vertical(&mut self, pointer_y: f32, grab: f32) {
        let track =
            (self.layout.vertical_bar.height() - self.layout.vertical_thumb.height()).max(0.0);
        let visible = RowViewport::visible_rows(
            self.layout.body.height(),
            self.layout.row_height.max(ROW_HEIGHT),
        )
        .max(1);
        let max_top = self.len().saturating_sub(visible);
        let offset = (pointer_y - grab - self.layout.vertical_bar.top()).clamp(0.0, track);
        self.viewport.top = normalized_u64(offset, track, max_top);
        self.viewport.subrow_px = 0.0;
    }

    fn drag_horizontal(&mut self, pointer_x: f32, grab: f32) {
        let track =
            (self.layout.horizontal_bar.width() - self.layout.horizontal_thumb.width()).max(0.0);
        let max_scroll = (self.layout.content_width - self.layout.body.width()).max(0.0);
        let offset = (pointer_x - grab - self.layout.horizontal_bar.left()).clamp(0.0, track);
        self.horizontal = if track == 0.0 {
            0.0
        } else {
            offset / track * max_scroll
        };
    }

    pub fn layout(&mut self, bounds: Rect, theme: &Theme) -> &TableLayout {
        let zoom = theme.zoom;
        let row_height = ROW_HEIGHT * zoom;
        let gutter_width = 64.0 * zoom;
        let bar = 12.0 * zoom;
        let status_height = row_height;
        let header = Rect::from_xywh(
            bounds.left() + gutter_width,
            bounds.top(),
            (bounds.width() - gutter_width - bar).max(0.0),
            row_height,
        );
        let body = Rect::from_xywh(
            header.left(),
            header.bottom(),
            header.width(),
            (bounds.height() - row_height - status_height - bar).max(0.0),
        );
        let gutter = Rect::from_xywh(bounds.left(), body.top(), gutter_width, body.height());
        let vertical_bar = Rect::from_xywh(body.right(), body.top(), bar, body.height());
        let horizontal_bar = Rect::from_xywh(body.left(), body.bottom(), body.width(), bar);
        let status = Rect::from_xywh(
            bounds.left(),
            horizontal_bar.bottom(),
            bounds.width(),
            status_height,
        );
        let widths = self.visible_column_widths();
        let content_width = widths.iter().map(|(_, width)| *width * zoom).sum::<f32>();
        self.horizontal = self
            .horizontal
            .clamp(0.0, (content_width - body.width()).max(0.0));
        let previous_columns: Vec<_> = self
            .layout
            .columns
            .iter()
            .map(|column| column.source_index)
            .collect();
        let mut x = body.left() - self.horizontal;
        let mut columns = Vec::new();
        for (source_index, width) in widths {
            let width = width * zoom;
            let rect = Rect::from_xywh(x, header.top(), width, row_height);
            if rect.right() >= body.left() && rect.left() <= body.right() {
                columns.push(super::layout::ColumnRect {
                    source_index,
                    prepared_index: columns.len(),
                    rect,
                });
            }
            x += width;
        }
        self.viewport.clamp(self.len(), body.height(), row_height);
        let visible = RowViewport::visible_rows(body.height(), row_height) as f32;
        let thumb_h = (vertical_bar.height() * (visible / self.len().max(1) as f32).min(1.0))
            .max(24.0 * zoom)
            .min(vertical_bar.height());
        let max_top = self.len().saturating_sub(visible.ceil() as u64);
        let fraction = if max_top == 0 {
            0.0
        } else {
            self.viewport.top as f32 / max_top as f32
        };
        let vertical_thumb = Rect::from_xywh(
            vertical_bar.left() + 2.0,
            vertical_bar.top() + (vertical_bar.height() - thumb_h) * fraction,
            (bar - 4.0).max(0.0),
            thumb_h,
        );
        let thumb_w = (horizontal_bar.width() * horizontal_bar.width() / content_width.max(1.0))
            .max(24.0 * zoom)
            .min(horizontal_bar.width());
        let hfrac = self.horizontal / (content_width - body.width()).max(1.0);
        let horizontal_thumb = Rect::from_xywh(
            horizontal_bar.left() + (horizontal_bar.width() - thumb_w) * hfrac,
            horizontal_bar.top() + 2.0,
            thumb_w,
            (bar - 4.0).max(0.0),
        );
        self.layout = TableLayout {
            bounds,
            header,
            body,
            gutter,
            vertical_bar,
            vertical_thumb,
            horizontal_bar,
            horizontal_thumb,
            status,
            columns,
            row_height,
            horizontal: self.horizontal,
            content_width,
        };
        if previous_columns
            != self
                .layout
                .columns
                .iter()
                .map(|column| column.source_index)
                .collect::<Vec<_>>()
        {
            self.invalidate_window();
        }
        self.prepare_visible();
        &self.layout
    }

    fn visible_column_widths(&self) -> Vec<(usize, f32)> {
        match &self.columns {
            ColumnSet::Transactions(visible) => TransactionColumn::ALL
                .iter()
                .enumerate()
                .filter(|(_, c)| visible.contains(c))
                .map(|(i, c)| (i, c.width()))
                .collect(),
            ColumnSet::Signals { time, visible } => std::iter::once((0, 130.0))
                .filter(|_| *time)
                .chain(
                    visible
                        .iter()
                        .enumerate()
                        .filter(|(_, show)| **show)
                        .map(|(i, _)| (i + 1, 180.0)),
                )
                .collect(),
        }
    }

    pub fn column_title(&self, source_index: usize) -> &str {
        match &self.source {
            TableSource::Generator(_) => TransactionColumn::ALL
                .get(source_index)
                .map_or("", |c| c.title()),
            TableSource::Signals(signals) => {
                if source_index == 0 {
                    "Time"
                } else {
                    signals
                        .get(source_index - 1)
                        .map_or("", |s| s.name.as_str())
                }
            }
        }
    }

    fn prepare_visible(&mut self) {
        if !matches!(self.state, TableState::Ready | TableState::Empty) {
            self.window = PreparedWindow::default();
            return;
        }
        let visible = RowViewport::visible_rows(self.layout.body.height(), self.layout.row_height)
            .saturating_add(4)
            .min(MAX_PREPARED_ROWS);
        let start = self.viewport.top.saturating_sub(2);
        let end = start.saturating_add(visible).min(self.len());
        if self.window.start == start
            && self.window.end == end
            && self.window.rows.len() as u64 == end.saturating_sub(start)
        {
            return;
        }
        let mut window = PreparedWindow {
            start,
            end,
            rows: Vec::with_capacity((end - start) as usize),
            bytes: 0,
        };
        let mut signal_hints = match &self.rows {
            Rows::Signals { histories, .. } => vec![None; histories.len()],
            _ => Vec::new(),
        };
        for ordinal in start..end {
            let Some(row) = self.prepare_row(ordinal, &mut signal_hints) else {
                continue;
            };
            window.bytes += row.cells.iter().map(|c| c.text.len()).sum::<usize>();
            if window.bytes > PANEL_BYTES as usize {
                break;
            }
            window.rows.push(row);
        }
        self.window = window;
    }

    fn prepare_row(&self, ordinal: u64, signal_hints: &mut [Option<usize>]) -> Option<PreparedRow> {
        let index = usize::try_from(ordinal).ok()?;
        match &self.rows {
            Rows::Generator(generator) => {
                let tx = generator.transactions().get(index)?;
                let cells = self
                    .layout
                    .columns
                    .iter()
                    .filter_map(|column| TransactionColumn::ALL.get(column.source_index))
                    .map(|&column| PreparedCell {
                        text: transaction_cell(tx, column),
                        changed: false,
                        missing: false,
                    })
                    .collect();
                Some(PreparedRow {
                    ordinal,
                    identity: RowIdentity::Transaction(tx.id),
                    time: tx.begin,
                    cells,
                })
            }
            Rows::Signals {
                histories, axis, ..
            } => {
                let row_time = axis
                    .as_ref()
                    .and_then(|a| a.get(index).copied())
                    .or_else(|| {
                        histories
                            .first()
                            .filter(|_| axis.is_none())
                            .map(|h| h.time(index))
                    })?;
                let mut cells = Vec::new();
                for column in &self.layout.columns {
                    if column.source_index == 0 {
                        cells.push(PreparedCell {
                            text: row_time.to_string(),
                            changed: false,
                            missing: false,
                        });
                        continue;
                    }
                    let history = histories.get(column.source_index - 1)?;
                    let hint = signal_hints.get_mut(column.source_index - 1)?;
                    let sample = history.index_at_hint(row_time, hint.unwrap_or(0));
                    *hint = sample;
                    let missing = sample.is_none();
                    let changed = sample.is_some_and(|i| history.time(i) == row_time);
                    let text = if missing {
                        "—".into()
                    } else {
                        format_view(history.value_view(sample), PREVIEW_BYTES)
                    };
                    cells.push(PreparedCell {
                        text,
                        changed,
                        missing,
                    });
                }
                Some(PreparedRow {
                    ordinal,
                    identity: RowIdentity::SignalTime(row_time),
                    time: row_time,
                    cells,
                })
            }
            Rows::None => None,
        }
    }

    pub fn prepared_row(&self, ordinal: u64) -> Option<&PreparedRow> {
        ordinal
            .checked_sub(self.window.start)
            .and_then(|offset| self.window.rows.get(offset as usize))
            .filter(|row| row.ordinal == ordinal)
    }

    /// Semantic projection of only the recycled visible rows. No offscreen
    /// source row is materialized for accessibility.
    pub fn accessible_rows(&self) -> impl Iterator<Item = AccessibleRow> + '_ {
        self.window.rows.iter().filter_map(|row| {
            let y =
                self.layout.body.top() + self.viewport.y_of(row.ordinal, self.layout.row_height);
            let bounds = Rect::from_xywh(
                self.layout.gutter.left(),
                y,
                self.layout.body.right() - self.layout.gutter.left(),
                self.layout.row_height,
            );
            (bounds.bottom() > self.layout.body.top() && bounds.top() < self.layout.body.bottom())
                .then(|| AccessibleRow {
                    ordinal: row.ordinal,
                    label: format!(
                        "Row {}. {}",
                        row.ordinal.saturating_add(1),
                        row.cells
                            .iter()
                            .map(|cell| cell.text.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    selected: self.selected.as_ref() == Some(&row.identity),
                    bounds,
                })
        })
    }

    pub fn copy_tsv(&self) -> anyhow::Result<String> {
        let ordinal = self
            .selected_ordinal()
            .ok_or_else(|| anyhow::anyhow!("Select a row to copy."))?;
        let index = usize::try_from(ordinal)?;
        let mut text = String::new();
        match (&self.rows, &self.source) {
            (Rows::Generator(generator), TableSource::Generator(source)) => {
                let tx = &generator.transactions()[index];
                append_exact(
                    &mut text,
                    "Generator\tID\tBegin\tEnd\tDuration\tStatus\n",
                    COPY_BYTES,
                )?;
                for (part_index, part) in source.path().iter().enumerate() {
                    if part_index > 0 {
                        append_exact(&mut text, ".", COPY_BYTES)?;
                    }
                    append_exact(&mut text, part, COPY_BYTES)?;
                }
                append_exact(
                    &mut text,
                    &format!(
                        "\t{}\t{}\t{}\t{}\t{}",
                        tx.id.0,
                        tx.begin,
                        tx.end,
                        tx.end.saturating_sub(tx.begin),
                        tx.status.name()
                    ),
                    COPY_BYTES,
                )?;
            }
            (Rows::Signals { histories, .. }, TableSource::Signals(signals)) => {
                let time = self.row_time(ordinal).unwrap();
                append_exact(&mut text, "Time", COPY_BYTES)?;
                for source in signals {
                    append_exact(&mut text, "\t", COPY_BYTES)?;
                    append_exact(&mut text, &source.name, COPY_BYTES)?;
                }
                append_exact(&mut text, "\n", COPY_BYTES)?;
                append_exact(&mut text, &time.to_string(), COPY_BYTES)?;
                for (_, history) in signals.iter().zip(histories) {
                    append_exact(&mut text, "\t", COPY_BYTES)?;
                    let sample = history.index_at(time);
                    let value = if sample.is_none() {
                        "—".into()
                    } else {
                        let (value, cut) = bounded_view(
                            history.value_view(sample),
                            COPY_BYTES.saturating_sub(text.len()),
                        );
                        anyhow::ensure!(!cut, "Row exceeds the 64 KiB clipboard limit.");
                        value
                    };
                    append_exact(&mut text, &value, COPY_BYTES)?;
                }
            }
            _ => return Err(anyhow::anyhow!("Source is not ready.")),
        }
        Ok(text)
    }
}

fn transaction_cell(tx: &Transaction, column: TransactionColumn) -> String {
    match column {
        TransactionColumn::Label => tx
            .attributes
            .iter()
            .find(|a| a.key == "vtr.label")
            .map_or_else(|| "—".into(), |a| format_attribute(&a.value, PREVIEW_BYTES)),
        TransactionColumn::Begin => tx.begin.to_string(),
        TransactionColumn::Duration => tx.end.saturating_sub(tx.begin).to_string(),
        TransactionColumn::End => tx.end.to_string(),
        TransactionColumn::Id => tx.id.0.to_string(),
        TransactionColumn::Status => tx.status.name().into(),
        TransactionColumn::Attributes => {
            let mut text = String::new();
            let mut attributes = tx.attributes.iter().filter(|a| a.key != "vtr.label");
            for (shown, attribute) in attributes.by_ref().take(PREVIEW_ATTRIBUTES).enumerate() {
                if shown > 0 {
                    push_limited(&mut text, " · ", PREVIEW_BYTES);
                }
                push_limited(&mut text, &attribute.key, PREVIEW_BYTES);
                push_limited(&mut text, "=", PREVIEW_BYTES);
                let remaining = PREVIEW_BYTES.saturating_sub(text.len());
                push_limited(
                    &mut text,
                    &format_attribute(&attribute.value, remaining),
                    PREVIEW_BYTES,
                );
                if text.len() >= PREVIEW_BYTES {
                    break;
                }
            }
            if attributes.next().is_some() {
                push_limited(&mut text, " …", PREVIEW_BYTES);
            }
            if text.is_empty() { "—".into() } else { text }
        }
    }
}

fn format_view(value: ValueView<'_>, limit: usize) -> String {
    bounded_view(value, limit).0
}

fn bounded_view(value: ValueView<'_>, limit: usize) -> (String, bool) {
    match value {
        ValueView::Unavailable => ("—".into(), false),
        ValueView::Real(value) => {
            let text = value.to_string();
            let cut = text.len() > limit;
            (truncate(text, limit), cut)
        }
        ValueView::Text(value) => {
            let cut = value.len() > limit;
            (truncate_ref(&value, limit), cut)
        }
        ValueView::Logic(value) => {
            let mut text = String::with_capacity(value.width.min(limit));
            let room = limit.saturating_sub(if value.width > limit { "…".len() } else { 0 });
            let shown = value.width.min(room);
            for index in 0..shown {
                text.push(char::from(value.bit(index)));
            }
            let cut = shown < value.width;
            if cut {
                push_limited(&mut text, "…", limit);
            }
            (text, cut)
        }
        ValueView::Bytes(value) => {
            let mut text = String::new();
            let mut cut = false;
            push_limited(&mut text, "b\"", limit);
            for &byte in value.iter() {
                for escaped in std::ascii::escape_default(byte) {
                    if text.len() >= limit.saturating_sub(2) {
                        cut = true;
                        break;
                    }
                    text.push(char::from(escaped));
                }
                if cut {
                    break;
                }
            }
            if cut {
                push_limited(&mut text, "…", limit);
            }
            push_limited(&mut text, "\"", limit);
            (text, cut)
        }
    }
}

/// Convert a normalized scrollbar coordinate to an exact row using integer
/// arithmetic. Floating point only quantizes the finite pixel input; it never
/// represents the possibly full-width `u64` logical row count.
fn normalized_u64(offset: f32, track: f32, maximum: u64) -> u64 {
    if maximum == 0 || !offset.is_finite() || !track.is_finite() || track <= 0.0 {
        return 0;
    }
    const UNITS_PER_PIXEL: f64 = 1024.0;
    let denominator = ((track as f64 * UNITS_PER_PIXEL).round() as u64).max(1);
    let numerator =
        ((offset.clamp(0.0, track) as f64 * UNITS_PER_PIXEL).round() as u64).min(denominator);
    ((maximum as u128 * numerator as u128) / denominator as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::history::VecHistory;
    use crate::data::transactions::{AttributeValue, TransactionAttribute, TxKind, TxStatus};
    use crate::data::{SignalShape, WaveValue};
    use crate::pipeline::TrackSource;
    use crate::table::SignalSource;

    fn history(times: &[u64], values: &[&str]) -> Arc<dyn SignalHistory> {
        Arc::new(VecHistory {
            shape: SignalShape::Bit,
            times: times.to_vec(),
            values: values
                .iter()
                .map(|value| WaveValue::Bits((*value).into()))
                .collect(),
            initial: WaveValue::Unavailable,
        })
    }

    fn signal_source(name: &str, id: u32) -> SignalSource {
        SignalSource {
            path: vec![name.into()],
            nth: None,
            var: Some(id as usize),
            signal: Some(SignalRef(id)),
            name: name.into(),
        }
    }

    fn signal_model(budget: MemoryBudget) -> TableModel {
        TableModel::new(
            TableSource::Signals(vec![signal_source("a", 1), signal_source("b", 2)]),
            Link::default(),
            budget,
        )
    }

    #[test]
    fn merged_axis_collapses_coincident_and_repeated_changes() {
        let budget = MemoryBudget::new(16 * 1024 * 1024);
        let mut model = signal_model(budget.clone());
        let a = history(&[5, 5, 10, 30], &["0", "1", "0", "1"]);
        let b = history(&[1, 10, 20], &["1", "0", "1"]);
        assert!(!model.build_signal_rows(vec![a, b]).unwrap());
        let Rows::Signals {
            axis: Some(axis), ..
        } = &model.rows
        else {
            panic!()
        };
        assert_eq!(&**axis, &[1, 5, 10, 20, 30]);
        // Four MiB panel reservation plus exactly 8 bytes per distinct time.
        assert_eq!(budget.used(), PANEL_BYTES + 5 * 8);
        drop(model);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn single_signal_uses_history_as_its_axis_without_allocation() {
        let budget = MemoryBudget::new(8 * 1024 * 1024);
        let mut model = TableModel::new(
            TableSource::Signals(vec![signal_source("a", 1)]),
            Link::default(),
            budget.clone(),
        );
        model
            .build_signal_rows(vec![history(&[2, 8], &["0", "1"])])
            .unwrap();
        assert_eq!(model.len(), 2);
        assert_eq!(model.row_time(1), Some(8));
        assert_eq!(budget.used(), PANEL_BYTES);
    }

    #[test]
    fn axis_refusal_is_atomic_and_reports_required_admission() {
        let budget = MemoryBudget::new(PANEL_BYTES + 16);
        let mut model = signal_model(budget.clone());
        let error = model
            .build_signal_rows(vec![
                history(&[1, 2], &["0", "1"]),
                history(&[3, 4], &["0", "1"]),
            ])
            .unwrap_err()
            .to_string();
        assert!(error.contains("32 bytes"));
        assert_eq!(budget.used(), PANEL_BYTES);
        assert_eq!(model.len(), 0);
    }

    #[test]
    fn signal_axis_work_is_sliced_and_cancel_releases_the_build() {
        let budget = MemoryBudget::new(16 * 1024 * 1024);
        let mut model = signal_model(budget.clone());
        let times: Vec<u64> = (0..20_000).collect();
        let values = vec!["0"; times.len()];
        assert_eq!(
            model
                .begin_signal_rows(vec![history(&times, &values), history(&times, &values)])
                .unwrap(),
            None
        );
        assert!(model.tick(Instant::now()));
        assert!(model.axis_build.is_some(), "one tick must remain bounded");
        assert!(budget.used() > PANEL_BYTES);

        model.attached = true;
        assert!(model.command(
            &mut Document::new(),
            crate::panels::PanelId(1),
            TableCommand::Cancel,
            Instant::now(),
        ));
        assert!(model.axis_build.is_none());
        assert_eq!(budget.used(), PANEL_BYTES);
    }

    #[test]
    fn signal_window_formats_only_horizontally_intersecting_columns() {
        let sources = (0..20)
            .map(|index| signal_source(&format!("s{index}"), index + 1))
            .collect();
        let histories = (0..20).map(|_| history(&[1, 2], &["0", "1"])).collect();
        let mut model = TableModel::new(
            TableSource::Signals(sources),
            Link::default(),
            MemoryBudget::new(16 * 1024 * 1024),
        );
        model.build_signal_rows(histories).unwrap();
        model.state = TableState::Ready;
        model.layout(Rect::from_xywh(0.0, 0.0, 320.0, 240.0), &Theme::one_dark());
        let first: Vec<_> = model
            .layout
            .columns
            .iter()
            .map(|column| column.source_index)
            .collect();
        assert!(
            model
                .window
                .rows
                .iter()
                .all(|row| row.cells.len() == first.len())
        );
        assert!(first.len() < 21);

        model.horizontal = 2_000.0;
        model.layout(Rect::from_xywh(0.0, 0.0, 320.0, 240.0), &Theme::one_dark());
        let second: Vec<_> = model
            .layout
            .columns
            .iter()
            .map(|column| column.source_index)
            .collect();
        assert_ne!(first, second);
        assert!(
            model
                .window
                .rows
                .iter()
                .all(|row| row.cells.len() == second.len())
        );
    }

    #[test]
    fn signal_projection_borrows_before_enforcing_preview_and_copy_limits() {
        struct BorrowOnly {
            value: String,
        }
        impl SignalHistory for BorrowOnly {
            fn shape(&self) -> SignalShape {
                SignalShape::Text
            }
            fn len(&self) -> usize {
                1
            }
            fn time(&self, _: usize) -> u64 {
                7
            }
            fn value(&self, _: Option<usize>) -> WaveValue {
                panic!("table must not clone the complete signal value")
            }
            fn value_view(&self, _: Option<usize>) -> crate::data::value_view::ValueView<'_> {
                crate::data::value_view::ValueView::Text(self.value.as_str().into())
            }
            fn bit(&self, _: Option<usize>) -> crate::data::Bit {
                crate::data::Bit::Other
            }
        }

        let mut model = TableModel::new(
            TableSource::Signals(vec![signal_source("wide", 1)]),
            Link::default(),
            MemoryBudget::new(16 * 1024 * 1024),
        );
        model
            .build_signal_rows(vec![Arc::new(BorrowOnly {
                value: "x".repeat(COPY_BYTES + 1),
            })])
            .unwrap();
        model.state = TableState::Ready;
        model.layout(Rect::from_xywh(0.0, 0.0, 500.0, 240.0), &Theme::one_dark());
        assert!(model.window.rows[0].cells[1].text.len() <= PREVIEW_BYTES);
        model.selected = Some(RowIdentity::SignalTime(7));
        assert!(model.copy_tsv().unwrap_err().to_string().contains("64 KiB"));
    }

    fn transaction(id: u64, begin: u64, label: &str) -> Transaction {
        Transaction {
            id: TransactionRef(id),
            generator: crate::data::transactions::TrackRef(7),
            begin,
            end: begin + 8,
            status: TxStatus::Ok,
            kind: TxKind::Producer,
            parent: None,
            attributes: vec![
                TransactionAttribute {
                    key: "vtr.label".into(),
                    value: AttributeValue::Text(label.into()),
                },
                TransactionAttribute {
                    key: "addr".into(),
                    value: AttributeValue::U64(0x80),
                },
            ],
            events: vec![],
            stages: vec![],
        }
    }

    #[test]
    fn generator_window_is_bounded_and_selection_uses_identity() {
        let records = (1..=1000)
            .map(|id| transaction(id, id * 4, &format!("row {id}")))
            .collect();
        let generator = Arc::new(
            LoadedGenerator::new(
                crate::data::transactions::TrackRef(7),
                records,
                HashMap::new(),
                vec![],
            )
            .unwrap(),
        );
        let mut model = TableModel::new(
            TableSource::Generator(TrackSource::Resolved {
                track: crate::data::transactions::TrackRef(7),
                path: vec!["soc".into(), "request".into()],
            }),
            Link::default(),
            MemoryBudget::new(16 * 1024 * 1024),
        );
        model.rows = Rows::Generator(generator);
        model.state = TableState::Ready;
        model.layout.body = Rect::from_xywh(0.0, 0.0, 800.0, 10_000.0);
        model.layout.row_height = ROW_HEIGHT;
        model.prepare_visible();
        assert_eq!(model.window.rows.len(), MAX_PREPARED_ROWS as usize);
        model.selected = Some(RowIdentity::Transaction(TransactionRef(900)));
        assert_eq!(model.selected_ordinal(), Some(899));
        assert_eq!(
            model.copy_tsv().unwrap(),
            "Generator\tID\tBegin\tEnd\tDuration\tStatus\nsoc.request\t900\t3600\t3608\t8\tok"
        );
    }

    #[test]
    fn row_viewport_keeps_u64_positions_exact() {
        let mut viewport = RowViewport {
            top: u64::MAX - 1000,
            subrow_px: 0.0,
        };
        viewport.reveal(u64::MAX - 10, u64::MAX, 240.0, 24.0);
        assert_eq!(viewport.top, u64::MAX - 19);
        assert_eq!(viewport.row_at(216.0, u64::MAX, 24.0), Some(u64::MAX - 10));
    }

    #[test]
    fn normalized_scrollbar_math_preserves_u64_endpoints() {
        assert_eq!(normalized_u64(0.0, 997.0, u64::MAX), 0);
        assert_eq!(normalized_u64(997.0, 997.0, u64::MAX), u64::MAX);
        assert_eq!(
            normalized_u64(498.5, 997.0, u64::MAX),
            (u64::MAX as u128 / 2) as u64
        );
    }

    #[test]
    fn retry_reenters_panel_admission_after_capacity_is_released() {
        let budget = MemoryBudget::new(PANEL_BYTES);
        let blocker = budget.reserve(PANEL_BYTES).unwrap();
        let mut model = TableModel::new(
            TableSource::Signals(vec![signal_source("a", 1)]),
            Link::default(),
            budget.clone(),
        );
        assert!(matches!(model.state, TableState::Refused(_)));
        drop(blocker);

        assert!(model.retry(&mut Document::new()));
        assert_eq!(budget.used(), PANEL_BYTES);
        assert!(matches!(model.state, TableState::Unavailable));
    }
}
