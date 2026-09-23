//! `WaveModel`: the state of the waveform panel (displayed signals, selection,
//! viewport, scroll, hover, drags, and row menus) and every input rule that
//! mutates it. Navigation reads through the document while linked; markers
//! always belong to the document.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use web_time::Instant;

use super::analog::{self, Analog, AnalogDraw, AnalogRange};
use super::lane::{self, LaneGeometry, TxLane};
use super::layout::{LayoutInput, MIN_COLUMN, WaveLayout};
use super::viewport::Viewport;
use crate::data::loaded_tracks::LoadedGenerator;
use crate::data::transactions::TrackRef;
use crate::data::{SignalHistory, SignalRef, SignalShape, Translator, VarId};
use crate::document::{Document, TxSelection};
use crate::geometry::{Modifiers, MouseButton, Point, point};
use crate::nav::NavState;
pub use crate::nav::{Link, LinkDim};
use crate::panels::PanelId;
use crate::selection;
use crate::theme::Theme;

/// A ⌘-drag narrower than this (at zoom 1.0) is a click, not a zoom range.
pub const ZOOM_RANGE_MIN_PX: f32 = 4.0;
/// A press on a signal name must travel this far (at zoom 1.0) to start
/// moving rows; shorter presses are clicks.
pub const ROW_DRAG_MIN_PX: f32 = 4.0;
/// A press this close (at zoom 1.0) to the bottom edge of a row's name cell
/// resizes the row instead of selecting it.
pub const ROW_EDGE_GRAB_PX: f32 = 3.0;
/// While moving rows, the pointer this close to the top or bottom edge of the
/// rows (in rows) scrolls them, faster the deeper it goes.
const ROW_DRAG_EDGE_ROWS: f32 = 1.0;
/// Auto-scroll speed in rows per second per row of depth into the edge zone.
const ROW_DRAG_SCROLL_RATE: f32 = 12.0;

/// A row is either bound to this session or retains an unresolved durable locator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowSource {
    Resolved {
        var: VarId,
        signal: SignalRef,
    },
    Unresolved {
        path: Vec<String>,
        nth: Option<usize>,
        ambiguous: bool,
    },
}

impl RowSource {
    pub fn signal(&self) -> Option<SignalRef> {
        match self {
            Self::Resolved { signal, .. } => Some(*signal),
            _ => None,
        }
    }

    pub fn locator(&self, hierarchy: &crate::data::Hierarchy) -> (Vec<String>, Option<usize>) {
        match self {
            Self::Resolved { var, .. } => hierarchy.var_path(*var),
            Self::Unresolved { path, nth, .. } => (path.clone(), *nth),
        }
    }
}

/// A row's height as a whole multiple of the theme's row height, so rows keep
/// the grid rhythm and follow the interface zoom. Only [`RowHeight::PRESETS`]
/// exist; workspaces store the multiple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct RowHeight(u8);

impl RowHeight {
    pub const PRESETS: [RowHeight; 5] = [
        RowHeight(1),
        RowHeight(2),
        RowHeight(3),
        RowHeight(4),
        RowHeight(8),
    ];
    pub const DEFAULT: RowHeight = RowHeight(1);
    /// What a 1× row grows to when it is first drawn as a plot.
    pub const ANALOG: RowHeight = RowHeight(3);

    pub fn multiple(self) -> u8 {
        self.0
    }

    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }

    fn preset_index(self) -> usize {
        Self::PRESETS.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// The next preset up (`1`) or down (`-1`), saturating at the ends.
    pub fn step(self, delta: isize) -> Self {
        let ix = self.preset_index() as isize + delta;
        Self::PRESETS[ix.clamp(0, Self::PRESETS.len() as isize - 1) as usize]
    }
}

impl Default for RowHeight {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for RowHeight {
    type Error = String;
    fn try_from(multiple: u8) -> Result<Self, Self::Error> {
        Self::PRESETS
            .into_iter()
            .find(|p| p.0 == multiple)
            .ok_or_else(|| format!("unsupported row height {multiple}"))
    }
}

impl From<RowHeight> for u8 {
    fn from(height: RowHeight) -> u8 {
        height.0
    }
}

#[derive(Clone)]
pub struct DisplayedSignal {
    pub source: RowSource,
    /// Unavailable translator requested by a workspace; cleared by an explicit format change.
    pub requested_format: Option<String>,
    pub name: String,
    pub scope: String,
    pub shape: SignalShape,
    pub translator: Arc<dyn Translator>,
    pub history: Option<Arc<dyn SignalHistory>>,
    pub error: Option<String>,
    pub height: RowHeight,
    /// Drawn as a plot instead of digital values.
    pub analog: Option<Analog>,
}

impl DisplayedSignal {
    /// The translator a workspace file records for this row.
    pub fn format_id(&self) -> String {
        self.requested_format
            .clone()
            .unwrap_or_else(|| self.translator.id().into())
    }
}

/// One row of the waveform panel: a signal, or a transaction generator
/// shown as a lane of bars. Both share the row operations (selection,
/// reordering, clipboard, heights, removal and workspace entries).
#[derive(Clone)]
pub enum WaveRow {
    Signal(DisplayedSignal),
    Lane(TxLane),
}

impl WaveRow {
    pub fn name(&self) -> &str {
        match self {
            Self::Signal(s) => &s.name,
            Self::Lane(l) => &l.name,
        }
    }

    pub fn height(&self) -> RowHeight {
        match self {
            Self::Signal(s) => s.height,
            Self::Lane(l) => l.height,
        }
    }

    /// Set the height; a lane keeps an explicit choice from then on.
    pub fn set_height(&mut self, height: RowHeight) {
        match self {
            Self::Signal(s) => {
                s.height = height;
                if let Some(a) = &mut s.analog {
                    a.restore_height = None;
                }
            }
            Self::Lane(l) => {
                l.height = height;
                l.auto_height = false;
            }
        }
    }

    pub fn signal(&self) -> Option<&DisplayedSignal> {
        match self {
            Self::Signal(s) => Some(s),
            Self::Lane(_) => None,
        }
    }

    pub fn signal_mut(&mut self) -> Option<&mut DisplayedSignal> {
        match self {
            Self::Signal(s) => Some(s),
            Self::Lane(_) => None,
        }
    }

    pub fn lane(&self) -> Option<&TxLane> {
        match self {
            Self::Lane(l) => Some(l),
            Self::Signal(_) => None,
        }
    }

    /// The loaded signal of a signal row.
    pub fn signal_ref(&self) -> Option<SignalRef> {
        self.signal()?.source.signal()
    }

    /// The generator of a resolved lane.
    pub fn lane_track(&self) -> Option<TrackRef> {
        self.lane()?.track()
    }
}

/// What the cursor snaps to and steps through on a row.
enum EdgeSource<'a> {
    History(Arc<dyn SignalHistory>),
    Lane(&'a LoadedGenerator),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Cursor,
    ZoomRange {
        start: Point,
        current: Point,
    },
    Pan {
        last_x: f32,
    },
    NamesSplit,
    ValuesSplit,
    Scroll {
        grab: f32,
    },
    /// Resizing `row` (and the selection containing it) by the bottom edge
    /// of its name cell; `top` is the row's top at the press.
    RowHeight {
        row: usize,
        top: f32,
    },
    /// Moving the selected rows by their names. `gap` is the insertion point
    /// (a row index, or the row count for the end) once the press has moved
    /// far enough and the drop would change the order. A plain press on an
    /// already selected row keeps the group for dragging and narrows the
    /// selection to `collapse` only if it ends as a click.
    Rows {
        press_y: f32,
        started: bool,
        gap: Option<usize>,
        collapse: Option<usize>,
        /// Last auto-scroll step, while the pointer is in an edge zone.
        scrolled_at: Option<Instant>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Format(String),
    RetryLoad,
    OpenTable,
    CopySignals,
    CutSignals,
    PasteSignals,
    RemoveSignals,
    RowHeight(RowHeight),
    /// Draw the rows digitally (`None`) or as a plot.
    Draw(Option<AnalogDraw>),
    Range(AnalogRange),
    ToggleAnalog,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub action: MenuAction,
    pub label: String,
    pub badge: Option<String>,
    pub checked: bool,
}

/// A menu entry: a choice, a labelled submenu of choices, or a group divider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Submenu {
        label: String,
        items: Vec<MenuItem>,
    },
    /// A group title.
    Label(String),
    Separator,
}

impl MenuItem {
    fn plain(action: MenuAction, label: &str) -> Self {
        Self {
            action,
            label: label.into(),
            badge: None,
            checked: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveMenuKind {
    Format,
    Signal,
}

/// A format-badge or signal-name menu. The frontend shows its own popup widget
/// at the panel position and reports the typed choice.
#[derive(Clone, Debug)]
pub struct WaveMenu {
    pub kind: WaveMenuKind,
    pub row: usize,
    pub position: Point,
    pub entries: Vec<MenuEntry>,
}

impl WaveMenu {
    /// Every choice, with submenus flattened in order.
    pub fn items(&self) -> impl Iterator<Item = &MenuItem> {
        self.entries.iter().flat_map(|entry| match entry {
            MenuEntry::Item(item) => std::slice::from_ref(item),
            MenuEntry::Submenu { items, .. } => items.as_slice(),
            MenuEntry::Separator | MenuEntry::Label(_) => &[],
        })
    }
}

/// Pointer input over the wave panel, in the same coordinate space as the
/// bounds passed to [`WaveModel::layout`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Down {
        position: Point,
        button: MouseButton,
        modifiers: Modifiers,
    },
    Move {
        position: Point,
    },
    Up,
    /// The pointer left the panel (or the window).
    Leave,
    /// Wheel or trackpad scroll in logical pixels. `precise` marks trackpad
    /// deltas (pixel-exact scrolling) as opposed to mouse wheel notches.
    Wheel {
        position: Point,
        dx: f32,
        dy: f32,
        modifiers: Modifiers,
        precise: bool,
    },
    /// Trackpad pinch; `delta` is the scale change of this event.
    Pinch {
        position: Point,
        delta: f32,
    },
}

pub struct WaveModel {
    pub items: Vec<WaveRow>,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    /// Link flags and the local viewport/cursor kept while unlinked.
    pub nav: NavState,
    pub scroll_y: f32,
    pub names_width: f32,
    pub values_width: f32,
    pub hover_row: Option<usize>,
    pub badge_hover: Option<usize>,
    /// The row whose bottom edge the pointer can drag to resize it.
    pub edge_hover: Option<usize>,
    /// Pointer is over a column divider / a marker chip (drives repaints).
    pub split_hover: bool,
    pub chip_hover: bool,
    pub drag: Option<Drag>,
    /// Width of the waves column at the last layout, for keyboard zoom.
    pub wave_width: f32,
    /// Duration of the last panel paint, and a smoothed average.
    pub frames_painted: u64,
    pub frame_ms: f32,
    pub frame_ms_avg: f32,
    pub menu: Option<WaveMenu>,
    /// Last known pointer position over the panel.
    pub pointer: Option<Point>,
    /// The last press landed on a lane bar and selected its record, so a
    /// second click opens it.
    pub pressed_record: bool,
    /// When analog ranges last eased, while one is moving.
    analog_eased_at: Option<Instant>,
    layout: WaveLayout,
}

impl Default for WaveModel {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveModel {
    pub fn viewport(&self, doc: &Document) -> Viewport {
        self.nav.viewport(doc)
    }

    pub fn cursor(&self, doc: &Document) -> Option<u64> {
        self.nav.cursor(doc)
    }

    pub fn set_cursor(&mut self, doc: &mut Document, cursor: Option<u64>) -> bool {
        self.nav.set_cursor(doc, cursor)
    }

    /// Copy persistent view content, sharing immutable histories. Pixel
    /// layout, pointer capture, menus, and animation belong to the old view.
    pub fn clone_view(&self, with_rows: bool) -> Self {
        Self {
            items: if with_rows {
                self.items.clone()
            } else {
                Vec::new()
            },
            selected: if with_rows {
                self.selected.clone()
            } else {
                BTreeSet::new()
            },
            anchor: if with_rows { self.anchor } else { None },
            nav: self.nav.clone_view(),
            scroll_y: if with_rows { self.scroll_y } else { 0.0 },
            names_width: self.names_width,
            values_width: self.values_width,
            ..Self::new()
        }
    }

    pub fn new() -> Self {
        WaveModel {
            items: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            nav: NavState::new(),
            scroll_y: 0.0,
            names_width: 220.0,
            values_width: 120.0,
            hover_row: None,
            badge_hover: None,
            edge_hover: None,
            split_hover: false,
            chip_hover: false,
            drag: None,
            wave_width: 800.0,
            frames_painted: 0,
            frame_ms: 0.0,
            frame_ms_avg: 0.0,
            menu: None,
            pointer: None,
            pressed_record: false,
            analog_eased_at: None,
            layout: WaveLayout::default(),
        }
    }

    /// Forget every row and interaction; called when the document's session changes.
    pub fn reset(&mut self, limits: Option<(u64, u64)>) {
        self.nav.reset(limits);
        self.items.clear();
        self.selected.clear();
        self.anchor = None;
        self.scroll_y = 0.0;
        self.menu = None;
        self.drag = None;
        self.hover_row = None;
        self.badge_hover = None;
    }

    /// The layout of the last frame (see [`WaveModel::layout`]).
    pub fn last_layout(&self) -> &WaveLayout {
        &self.layout
    }

    pub fn is_animating(&self) -> bool {
        self.nav.is_animating()
            || self.row_drag_scroll_speed() != 0.0
            || self.analog_rows().any(|a| !a.is_settled())
    }

    pub fn tick(&mut self, doc: &Document, now: Instant) -> bool {
        let animated = self.nav.tick(now);
        animated | self.row_drag_scroll(now) | self.ease_analog(doc, now)
    }

    fn analog_rows(&self) -> impl Iterator<Item = &Analog> {
        self.items
            .iter()
            .filter_map(|row| row.signal()?.analog.as_ref())
    }

    /// Fit the visible plots' target ranges to the current viewport.
    fn update_analog_targets(&mut self, doc: &Document) {
        let vp = self.viewport(doc);
        let rows = self.layout.rows.clone();
        let len = self.items.len();
        for item in self.items[rows.start.min(len)..rows.end.min(len)]
            .iter_mut()
            .filter_map(WaveRow::signal_mut)
        {
            if let (Some(a), Some(h)) = (&mut item.analog, &item.history)
                && let Some(kind) = item.translator.numeric_kind()
            {
                let series = analog::Series::of(doc, item.source.signal(), h, kind);
                a.update_target(&series, item.translator.as_ref(), item.shape, &vp);
            }
        }
    }

    /// Ease every plot's displayed range towards its target.
    fn ease_analog(&mut self, doc: &Document, now: Instant) -> bool {
        self.update_analog_targets(doc);
        let dt = self.analog_eased_at.map_or(0.0, |at| {
            now.saturating_duration_since(at).as_secs_f64().min(0.1)
        });
        let mut moving = false;
        for item in self.items.iter_mut().filter_map(WaveRow::signal_mut) {
            if let Some(a) = &mut item.analog {
                moving |= a.ease(dt);
            }
        }
        self.analog_eased_at = moving.then_some(now);
        moving
    }

    pub fn loaded_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.signal().is_some_and(|s| s.history.is_some()))
            .count()
    }

    /// The signal of row `row`, if it is a signal row.
    pub fn signal(&self, row: usize) -> Option<&DisplayedSignal> {
        self.items.get(row)?.signal()
    }

    // -- items ---------------------------------------------------------------

    /// Append rows for `vars`, sharing `loaded` histories for the same signal
    /// and queuing a load for the rest.
    pub fn add_vars(
        &mut self,
        doc: &mut Document,
        vars: &[VarId],
        loaded: HashMap<SignalRef, Arc<dyn SignalHistory>>,
    ) {
        let Some(session) = doc.session().cloned() else {
            return;
        };
        let h = session.hierarchy();
        let first_new = self.items.len();
        for &var in vars {
            let Some(v) = h.vars.get(var) else { continue };
            let translator = doc.translators.default_for(v.shape);
            // Variable identity/format stay per row; aliases share immutable data.
            let history = loaded.get(&v.signal).cloned();
            let needs_load = history.is_none();
            // Reals open as plots: their text is rarely readable at a glance.
            let analog = (v.shape == SignalShape::Real
                && analog::supports(v.shape, translator.as_ref()))
            .then(|| {
                let mut a = Analog::new(AnalogDraw::Linear, AnalogRange::Trace);
                a.restore_height = Some(RowHeight::DEFAULT);
                a
            });
            self.items.push(WaveRow::Signal(DisplayedSignal {
                source: RowSource::Resolved {
                    var,
                    signal: v.signal,
                },
                requested_format: None,
                name: v.name.clone(),
                scope: h.scope_path(v.scope).join("."),
                shape: v.shape,
                translator,
                history,
                error: None,
                height: if analog.is_some() {
                    RowHeight::ANALOG
                } else {
                    RowHeight::DEFAULT
                },
                analog,
            }));
            if needs_load {
                doc.request_signal(v.signal);
            }
        }
        if self.items.len() > first_new {
            self.selected.clear();
            self.selected.extend(first_new..self.items.len());
            self.anchor = Some(first_new);
        }
    }

    /// Append a lane for each generator in `tracks` and select them. A lane
    /// takes the default height for its depth as soon as its records are
    /// resident; the app retains them for as long as a lane shows them.
    pub fn add_lanes(&mut self, doc: &Document, tracks: &[TrackRef]) {
        let first_new = self.items.len();
        self.items.extend(
            tracks
                .iter()
                .filter_map(|&track| TxLane::new(doc, track))
                .map(WaveRow::Lane),
        );
        if self.items.len() > first_new {
            self.selected = (first_new..self.items.len()).collect();
            self.anchor = Some(first_new);
        }
    }

    /// Records arrived: new lanes take their default height.
    pub fn fit_lanes(&mut self, doc: &Document) {
        for row in &mut self.items {
            if let WaveRow::Lane(lane) = row {
                lane.fit_height(doc);
            }
        }
    }

    /// A history load finished (the document already checked its generation).
    pub fn finish_signal(
        &mut self,
        signal: SignalRef,
        result: anyhow::Result<Arc<dyn SignalHistory>>,
    ) {
        let result = result.map_err(|e| e.to_string());
        for item in self
            .items
            .iter_mut()
            .filter_map(WaveRow::signal_mut)
            .filter(|i| i.source.signal() == Some(signal))
        {
            item.history = result.as_ref().ok().cloned();
            item.error = result.as_ref().err().cloned();
        }
    }

    pub fn remove_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let mut ix = 0;
        let selected = std::mem::take(&mut self.selected);
        self.items.retain(|_| {
            let keep = !selected.contains(&ix);
            ix += 1;
            keep
        });
        self.anchor = None;
    }

    /// Copy the selected rows, in display order, to the document clipboard.
    /// Rows keep their format and height; histories stay with the panels.
    pub fn copy_selected(&self, doc: &mut Document) {
        if self.selected.is_empty() {
            return;
        }
        doc.copied_rows = self
            .selected
            .iter()
            .filter_map(|&row| self.items.get(row))
            .map(|row| match row {
                WaveRow::Signal(item) => WaveRow::Signal(DisplayedSignal {
                    history: None,
                    // A resolved row reloads on paste; an unresolved one keeps its reason.
                    error: item
                        .source
                        .signal()
                        .is_none()
                        .then(|| item.error.clone())
                        .flatten(),
                    ..item.clone()
                }),
                // Lanes hold no data; the document keeps what lanes show.
                WaveRow::Lane(lane) => WaveRow::Lane(lane.clone()),
            })
            .collect();
    }

    pub fn cut_selected(&mut self, doc: &mut Document) {
        self.copy_selected(doc);
        self.remove_selected();
    }

    /// Where the selected rows land, in display order, if moved to `gap`
    /// (`0..=len`), or `None` when that would not change the order.
    fn move_target(&self, gap: usize) -> Option<usize> {
        let first = *self.selected.first()?;
        let last = *self.selected.last()?;
        if last >= self.items.len() {
            return None;
        }
        let gap = gap.min(self.items.len());
        let at = gap - self.selected.range(..gap).count();
        let contiguous = last - first + 1 == self.selected.len();
        (!contiguous || at != first).then_some(at)
    }

    /// Move the selected rows, keeping their order, to the insertion point
    /// `gap` (`0..=len`, counted before the move); they stay selected.
    /// Returns whether the order changed.
    pub fn move_selected_to(&mut self, gap: usize) -> bool {
        let Some(at) = self.move_target(gap) else {
            return false;
        };
        let anchor = self
            .anchor
            .and_then(|a| self.selected.iter().position(|row| *row == a));
        let (mut moved, mut rest) = (Vec::new(), Vec::new());
        for (ix, item) in std::mem::take(&mut self.items).into_iter().enumerate() {
            if self.selected.contains(&ix) {
                moved.push(item);
            } else {
                rest.push(item);
            }
        }
        let count = moved.len();
        rest.splice(at..at, moved);
        self.items = rest;
        self.selected = (at..at + count).collect();
        self.anchor = Some(at + anchor.unwrap_or(0));
        self.menu = None;
        true
    }

    /// Insert the document clipboard below the selection (or at the end) and
    /// select the new rows. Duplicates share `loaded` histories for the same
    /// signal; the rest load once, like newly added variables.
    pub fn paste(
        &mut self,
        doc: &mut Document,
        loaded: &HashMap<SignalRef, Arc<dyn SignalHistory>>,
    ) {
        if doc.copied_rows.is_empty() {
            return;
        }
        let at = self
            .selected
            .last()
            .map_or(self.items.len(), |&row| (row + 1).min(self.items.len()));
        let mut rows = doc.copied_rows.clone();
        for row in rows.iter_mut().filter_map(WaveRow::signal_mut) {
            if let Some(signal) = row.source.signal() {
                row.history = loaded.get(&signal).cloned();
                if row.history.is_none() {
                    doc.request_signal(signal);
                }
            }
        }
        let count = rows.len();
        self.items.splice(at..at, rows);
        self.selected = (at..at + count).collect();
        self.anchor = Some(at);
        self.menu = None;
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.items.len()).collect();
    }

    /// Escape: cancel a drag, close the menu, clear selection, then clear cursor.
    pub fn clear_selection(&mut self, doc: &mut Document) {
        if self.drag.is_some() {
            self.drag = None;
        } else if self.menu.is_some() {
            self.menu = None;
        } else if !self.selected.is_empty() {
            self.selected.clear();
        } else {
            self.set_cursor(doc, None);
        }
    }

    /// Click selection with platform conventions: plain = single, cmd/ctrl =
    /// toggle, shift = range from the anchor.
    fn select_row(&mut self, row: usize, modifiers: Modifiers) {
        if row >= self.items.len() {
            return;
        }
        selection::select(&mut self.selected, &mut self.anchor, row, modifiers);
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let current = self
            .anchor
            .or_else(|| self.selected.iter().next().copied())
            .unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.items.len() as isize - 1) as usize;
        self.selected.clear();
        self.selected.insert(next);
        self.anchor = Some(next);
    }

    // -- translators -----------------------------------------------------------

    fn set_translator(&mut self, doc: &Document, rows: &[usize], id: &str) {
        let Some(t) = doc.translators.get(id) else {
            return;
        };
        for &row in rows {
            if let Some(item) = self.items.get_mut(row).and_then(WaveRow::signal_mut)
                && t.applies(item.shape)
            {
                item.translator = t.clone();
                item.requested_format = None;
            }
        }
    }

    pub fn cycle_format(&mut self, doc: &Document) {
        let rows: Vec<usize> = self.selected.iter().copied().collect();
        for row in rows {
            let Some(item) = self.items.get_mut(row).and_then(WaveRow::signal_mut) else {
                continue;
            };
            let options = doc.translators.applicable(item.shape);
            if options.is_empty() {
                continue;
            }
            let pos = options
                .iter()
                .position(|t| t.id() == item.translator.id())
                .unwrap_or(0);
            item.translator = options[(pos + 1) % options.len()].clone();
            item.requested_format = None;
        }
    }

    // -- analog ------------------------------------------------------------------

    /// Whether row `row` can be drawn as a plot in its current format.
    pub fn can_plot(&self, row: usize) -> bool {
        self.signal(row)
            .is_some_and(|s| analog::supports(s.shape, s.translator.as_ref()))
    }

    /// Draw `rows` as plots with `draw`, or digitally with `None`; rows that
    /// cannot be plotted are skipped. A 1× row grows to
    /// [`RowHeight::ANALOG`] and gets 1× back when analog is turned off,
    /// unless it was resized in between.
    pub fn set_analog(&mut self, rows: &[usize], draw: Option<AnalogDraw>) {
        for &row in rows {
            if draw.is_some() && !self.can_plot(row) {
                continue;
            }
            let Some(item) = self.items.get_mut(row).and_then(WaveRow::signal_mut) else {
                continue;
            };
            match (draw, &mut item.analog) {
                (Some(draw), Some(a)) => a.draw = draw,
                (Some(draw), None) => {
                    let mut a = Analog::new(draw, AnalogRange::Trace);
                    if item.height == RowHeight::DEFAULT {
                        a.restore_height = Some(item.height);
                        item.height = RowHeight::ANALOG;
                    }
                    item.analog = Some(a);
                }
                (None, Some(a)) => {
                    if let Some(h) = a
                        .restore_height
                        .filter(|_| item.height == RowHeight::ANALOG)
                    {
                        item.height = h;
                    }
                    item.analog = None;
                }
                (None, None) => {}
            }
        }
        self.scroll_y = self.scroll_y.min(self.layout.max_scroll.max(0.0));
    }

    /// `A`: plot the selected rows, or turn them all back to digital when
    /// every plottable one already is a plot.
    pub fn toggle_analog(&mut self) {
        let rows: Vec<usize> = self.selected.iter().copied().collect();
        self.toggle_analog_rows(&rows);
    }

    fn toggle_analog_rows(&mut self, rows: &[usize]) {
        let rows: Vec<usize> = rows.iter().copied().filter(|&r| self.can_plot(r)).collect();
        let on = rows
            .iter()
            .any(|&r| self.signal(r).is_some_and(|s| s.analog.is_none()));
        for r in rows {
            let draw = self.signal(r).and_then(|s| {
                on.then(|| {
                    s.analog
                        .as_ref()
                        .map_or(AnalogDraw::default_for(s.shape), |a| a.draw)
                })
            });
            self.set_analog(&[r], draw);
        }
    }

    /// Choose the vertical range of the plotted `rows`; type limits apply
    /// only to formats that have them.
    pub fn set_analog_range(&mut self, rows: &[usize], range: AnalogRange) {
        for &row in rows {
            if let Some(item) = self.items.get_mut(row).and_then(WaveRow::signal_mut)
                && (range != AnalogRange::Type || item.translator.limits(item.shape).is_some())
                && let Some(a) = &mut item.analog
            {
                a.range = range;
            }
        }
    }

    /// Open the format menu for `row` at a panel position.
    pub fn open_format_menu(&mut self, doc: &Document, row: usize, position: Point) {
        let Some(item) = self.signal(row) else {
            return;
        };
        let current = item.translator.id();
        let mut entries: Vec<_> = doc
            .translators
            .applicable(item.shape)
            .into_iter()
            .map(|t| {
                MenuEntry::Item(MenuItem {
                    action: MenuAction::Format(t.id().into()),
                    label: t.name().into(),
                    badge: Some(t.badge().into()),
                    checked: t.id() == current,
                })
            })
            .collect();
        // Draw and Range sections for rows that can be plots.
        if analog::supports(item.shape, item.translator.as_ref()) {
            let a = item.analog.as_ref();
            let choice = |action, label: &str, checked| {
                MenuEntry::Item(MenuItem {
                    action,
                    label: label.into(),
                    badge: None,
                    checked,
                })
            };
            let draw = a.map(|a| a.draw);
            entries.extend([
                MenuEntry::Separator,
                MenuEntry::Label("Draw".into()),
                choice(MenuAction::Draw(None), "Digital", draw.is_none()),
                choice(
                    MenuAction::Draw(Some(AnalogDraw::Step)),
                    "Analog · step",
                    draw == Some(AnalogDraw::Step),
                ),
                choice(
                    MenuAction::Draw(Some(AnalogDraw::Linear)),
                    "Analog · linear",
                    draw == Some(AnalogDraw::Linear),
                ),
            ]);
            if let Some(a) = a {
                let mut ranges = vec![AnalogRange::Trace, AnalogRange::Window];
                if item.translator.limits(item.shape).is_some() {
                    ranges.push(AnalogRange::Type);
                }
                entries.extend([MenuEntry::Separator, MenuEntry::Label("Range".into())]);
                entries.extend(ranges.into_iter().map(|range| {
                    let label = match range {
                        AnalogRange::Trace => "Whole trace",
                        AnalogRange::Window => "Visible window",
                        AnalogRange::Type => "Type limits",
                    };
                    choice(MenuAction::Range(range), label, a.range == range)
                }));
            }
        }
        if item.error.is_some() && item.source.signal().is_some() {
            entries.push(MenuEntry::Item(MenuItem::plain(
                MenuAction::RetryLoad,
                "Retry loading",
            )));
        }
        self.menu = Some(WaveMenu {
            kind: WaveMenuKind::Format,
            row,
            position,
            entries,
        });
    }

    /// Open the signal-name context menu. Right-clicking within an existing
    /// selection preserves the group; an unselected row becomes the selection.
    pub fn open_signal_menu(&mut self, doc: &Document, row: usize, position: Point) {
        if row >= self.items.len() {
            return;
        }
        if !self.selected.contains(&row) {
            self.select_row(row, Modifiers::default());
        }
        // A preset is checked only when every target row already has it.
        let targets = self.menu_rows(row);
        let mut heights = targets.iter().map(|&r| self.items[r].height());
        let first = heights.next();
        let shared = first.filter(|h| heights.all(|other| other == *h));
        let heights = RowHeight::PRESETS
            .into_iter()
            .map(|height| MenuItem {
                action: MenuAction::RowHeight(height),
                label: if height.is_default() {
                    "1× (Default)".into()
                } else {
                    format!("{}×", height.multiple())
                },
                badge: None,
                checked: shared == Some(height),
            })
            .collect();
        self.menu = Some(WaveMenu {
            kind: WaveMenuKind::Signal,
            row,
            position,
            entries: vec![
                MenuEntry::Item(MenuItem::plain(MenuAction::OpenTable, "Open in table")),
                MenuEntry::Separator,
                MenuEntry::Item(MenuItem::plain(MenuAction::CutSignals, "Cut")),
                MenuEntry::Item(MenuItem::plain(MenuAction::CopySignals, "Copy")),
            ],
        });
        let menu = self.menu.as_mut().expect("just opened");
        if !doc.copied_rows.is_empty() {
            menu.entries.push(MenuEntry::Item(MenuItem::plain(
                MenuAction::PasteSignals,
                "Paste",
            )));
        }
        let plottable: Vec<usize> = targets
            .iter()
            .copied()
            .filter(|&r| self.can_plot(r))
            .collect();
        if !plottable.is_empty() {
            let checked = plottable
                .iter()
                .all(|&r| self.signal(r).is_some_and(|s| s.analog.is_some()));
            let menu = self.menu.as_mut().expect("just opened");
            menu.entries.extend([
                MenuEntry::Separator,
                MenuEntry::Item(MenuItem {
                    action: MenuAction::ToggleAnalog,
                    label: "Show as analog".into(),
                    badge: None,
                    checked,
                }),
            ]);
        }
        let menu = self.menu.as_mut().expect("just opened");
        menu.entries.extend([
            MenuEntry::Separator,
            MenuEntry::Submenu {
                label: "Height".into(),
                items: heights,
            },
            MenuEntry::Separator,
            MenuEntry::Item(MenuItem::plain(
                MenuAction::RemoveSignals,
                if self.items[row].lane().is_some() {
                    "Remove lane"
                } else {
                    "Remove signal"
                },
            )),
        ]);
    }

    /// Open the signal menu for the keyboard selection, positioned beside its
    /// name cell. Off-screen selections use the nearest panel edge.
    pub fn open_selected_signal_menu(&mut self, doc: &Document) {
        let Some(row) = self
            .anchor
            .filter(|row| self.selected.contains(row))
            .or_else(|| self.selected.iter().next().copied())
        else {
            return;
        };
        let y = (self.layout.row_y(row) + self.layout.row_height(row))
            .clamp(self.layout.names.top(), self.layout.names.bottom());
        self.open_signal_menu(
            doc,
            row,
            point(self.layout.names.left() + 8.0 * self.layout.zoom, y),
        );
    }

    /// The frontend's popup reported a choice.
    /// Return a failed canonical signal to retry through the document owner.
    pub fn menu_select(&mut self, doc: &Document, action: &MenuAction) -> Option<SignalRef> {
        let menu = self.menu.take()?;
        if matches!(
            action,
            MenuAction::OpenTable
                | MenuAction::CopySignals
                | MenuAction::CutSignals
                | MenuAction::PasteSignals
                | MenuAction::RemoveSignals
        ) {
            return None;
        }
        let rows = self.menu_rows(menu.row);
        match action {
            MenuAction::Format(id) => self.set_translator(doc, &rows, id),
            MenuAction::RowHeight(height) => self.resize_rows(&rows, menu.row, |_| *height),
            MenuAction::Draw(draw) => self.set_analog(&rows, *draw),
            MenuAction::Range(range) => self.set_analog_range(&rows, *range),
            MenuAction::ToggleAnalog => self.toggle_analog_rows(&rows),
            _ => {
                let row = self.signal(menu.row)?;
                return row.error.as_ref().and_then(|_| row.source.signal());
            }
        }
        None
    }

    /// The rows a menu opened on `row` acts on: the selection containing it, or itself.
    fn menu_rows(&self, row: usize) -> Vec<usize> {
        if self.selected.contains(&row) {
            self.selected.iter().copied().collect()
        } else {
            vec![row]
        }
    }

    // -- row heights -------------------------------------------------------------

    /// Top of `row` in multiples of the base row height.
    fn row_units_before(&self, row: usize) -> u32 {
        self.items[..row.min(self.items.len())]
            .iter()
            .map(|item| u32::from(item.height().multiple()))
            .sum()
    }

    /// Resize `rows`, keeping `anchor`'s top where it is on screen.
    fn resize_rows(
        &mut self,
        rows: &[usize],
        anchor: usize,
        height: impl Fn(RowHeight) -> RowHeight,
    ) {
        let before = self.row_units_before(anchor);
        for &row in rows {
            if let Some(item) = self.items.get_mut(row) {
                item.set_height(height(item.height()));
            }
        }
        let shift = self.row_units_before(anchor) as f32 - before as f32;
        self.scroll_y = (self.scroll_y + shift * self.layout.row_h).max(0.0);
    }

    /// Step every selected row to the next larger (`1`) or smaller (`-1`)
    /// preset; `0` resets them to the default height.
    pub fn step_row_height(&mut self, delta: isize) {
        let Some(anchor) = self
            .anchor
            .filter(|row| self.selected.contains(row))
            .or_else(|| self.selected.first().copied())
        else {
            return;
        };
        let rows: Vec<usize> = self.selected.iter().copied().collect();
        self.resize_rows(&rows, anchor, |h| {
            if delta == 0 {
                RowHeight::DEFAULT
            } else {
                h.step(delta)
            }
        });
    }

    pub fn menu_dismiss(&mut self) {
        self.menu = None;
    }

    // -- navigation ------------------------------------------------------------

    fn wave_w(&self) -> f64 {
        f64::from(self.wave_width).max(1.0)
    }

    fn animate_to(&mut self, doc: &mut Document, target: Viewport, now: Instant) {
        self.nav.animate_to(doc, target, now);
    }

    /// Immediate zoom (mouse wheel) around a pixel position in the waves column.
    fn zoom_at(&mut self, doc: &mut Document, x_px: f32, factor: f64) {
        let w = self.wave_w();
        self.nav.zoom_at(doc, f64::from(x_px), w, factor);
    }

    pub fn zoom_in(&mut self, doc: &mut Document, now: Instant) {
        let w = self.wave_w();
        self.nav.zoom_center(doc, w, 2.0, now);
    }

    pub fn zoom_out(&mut self, doc: &mut Document, now: Instant) {
        let w = self.wave_w();
        self.nav.zoom_center(doc, w, 0.5, now);
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

    /// Immediate pan by pixels (mouse drag / wheel).
    fn pan_px(&mut self, doc: &mut Document, dx: f32) {
        let w = self.wave_w();
        self.nav.pan_px(doc, f64::from(dx), w);
    }

    /// What row `row` snaps to: a signal's changes or a lane's record
    /// begins and ends.
    fn edge_source<'a>(&self, doc: &'a Document, row: usize) -> Option<EdgeSource<'a>> {
        match self.items.get(row)? {
            WaveRow::Signal(s) => s.history.clone().map(EdgeSource::History),
            WaveRow::Lane(l) => l.generator(doc).map(EdgeSource::Lane),
        }
    }

    fn edge_row(&self) -> Option<usize> {
        self.anchor
            .filter(|r| self.selected.contains(r))
            .or_else(|| self.selected.iter().next().copied())
    }

    /// Step the cursor to the selected row's next edge: a signal's next
    /// value change, or a lane's next record begin or end.
    pub fn next_edge(&mut self, doc: &mut Document, now: Instant) {
        let from = self
            .cursor(doc)
            .unwrap_or(self.viewport(doc).start.max(0.0) as u64);
        let next = match self.edge_row().and_then(|row| self.edge_source(doc, row)) {
            Some(EdgeSource::History(h)) => h.next_change_after(from),
            Some(EdgeSource::Lane(g)) => g.next_boundary(from),
            None => return,
        };
        if let Some(t) = next {
            self.set_cursor(doc, Some(t));
            self.nav.reveal_cursor(doc, now);
        }
    }

    pub fn prev_edge(&mut self, doc: &mut Document, now: Instant) {
        let from = self
            .cursor(doc)
            .unwrap_or(self.viewport(doc).end.max(0.0) as u64);
        let prev = match self.edge_row().and_then(|row| self.edge_source(doc, row)) {
            Some(EdgeSource::History(h)) => h.prev_change_before(from),
            Some(EdgeSource::Lane(g)) => g.prev_boundary(from),
            None => return,
        };
        if let Some(t) = prev {
            self.set_cursor(doc, Some(t));
            self.nav.reveal_cursor(doc, now);
        }
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

    // -- layout ------------------------------------------------------------------

    /// Lay the panel out in `bounds` for this frame. Clamps the scroll, records
    /// the waves width used by keyboard zoom, and derives hover state from the
    /// last pointer position. The result is kept for input handling.
    pub fn layout(
        &mut self,
        bounds: crate::geometry::Rect,
        doc: &Document,
        theme: &Theme,
    ) -> &WaveLayout {
        // Keep the same rows on screen when the interface zoom changes.
        if self.layout.row_h > 0.0 && self.layout.row_h != theme.row_height {
            self.scroll_y *= theme.row_height / self.layout.row_h;
        }
        let layout = WaveLayout::compute(LayoutInput {
            bounds,
            row_h: theme.row_height,
            header_h: theme.timeline_height,
            zoom: theme.zoom,
            names_width: self.names_width,
            values_width: self.values_width,
            row_tops: super::layout::row_tops(self.items.iter().map(WaveRow::height)),
            scroll_y: self.scroll_y,
            markers: &doc.markers,
            viewport: self.viewport(doc),
        });
        self.scroll_y = layout.scroll_y;
        self.wave_width = layout.waves.width();
        self.layout = layout;
        // Only signal rows have a format.
        let items = &self.items;
        self.layout
            .badges
            .retain(|(ix, _)| items.get(*ix).is_some_and(|row| row.signal().is_some()));
        self.update_hover();
        self.update_row_drag();
        self.update_analog_targets(doc);
        &self.layout
    }

    // -- moving rows by drag ---------------------------------------------------

    /// Follow the pointer during a row drag: start once it has moved far
    /// enough, then track the insertion gap under it.
    fn update_row_drag(&mut self) {
        let (
            Some(Drag::Rows {
                press_y,
                started,
                collapse,
                scrolled_at,
                ..
            }),
            Some(p),
        ) = (self.drag, self.pointer)
        else {
            return;
        };
        let layout = &self.layout;
        let started = started || (p.y - press_y).abs() >= ROW_DRAG_MIN_PX * layout.zoom;
        let gap = started
            .then(|| self.gap_at(p.y))
            .and_then(|gap| self.move_target(gap).map(|_| gap));
        self.drag = Some(Drag::Rows {
            press_y,
            started,
            gap,
            collapse,
            scrolled_at,
        });
    }

    /// The insertion gap nearest to `y`, which is clamped to the rows area.
    fn gap_at(&self, y: f32) -> usize {
        let layout = &self.layout;
        let len = self.items.len();
        let y = y.clamp(layout.names.top(), layout.names.bottom() - 1.0);
        match layout.row_at(y).filter(|row| *row < len) {
            Some(row) if y >= layout.row_y(row) + layout.row_height(row) / 2.0 => row + 1,
            Some(row) => row,
            None => len,
        }
    }

    /// Auto-scroll speed in pixels per second while a started row drag holds
    /// the pointer near the top (negative) or bottom (positive) edge.
    fn row_drag_scroll_speed(&self) -> f32 {
        let (Some(Drag::Rows { started: true, .. }), Some(p)) = (self.drag, self.pointer) else {
            return 0.0;
        };
        let layout = &self.layout;
        let zone = ROW_DRAG_EDGE_ROWS * layout.row_h;
        if zone <= 0.0 || layout.max_scroll <= 0.0 {
            return 0.0;
        }
        let rows_per_s = |depth: f32| (depth / zone).min(4.0) * ROW_DRAG_SCROLL_RATE;
        let top = layout.names.top() + zone - p.y;
        let bottom = p.y - (layout.names.bottom() - zone);
        if top > 0.0 && self.scroll_y > 0.0 {
            -rows_per_s(top) * layout.row_h
        } else if bottom > 0.0 && self.scroll_y < layout.max_scroll {
            rows_per_s(bottom) * layout.row_h
        } else {
            0.0
        }
    }

    /// Advance the row-drag auto-scroll; the next layout moves the gap.
    fn row_drag_scroll(&mut self, now: Instant) -> bool {
        let speed = self.row_drag_scroll_speed();
        let Some(Drag::Rows { scrolled_at, .. }) = &mut self.drag else {
            return false;
        };
        if speed == 0.0 {
            *scrolled_at = None;
            return false;
        }
        let dt = scrolled_at.map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f32());
        *scrolled_at = Some(now);
        let before = self.scroll_y;
        self.scroll_y = (self.scroll_y + speed * dt.min(0.1)).clamp(0.0, self.layout.max_scroll);
        self.scroll_y != before || dt == 0.0
    }

    /// The record of lane row `row` under panel point `p`, when bars are
    /// drawn (not the density strip).
    pub fn lane_hit(
        &self,
        doc: &Document,
        row: usize,
        p: Point,
    ) -> Option<(TrackRef, crate::data::transactions::TransactionRef)> {
        let lane = self.items.get(row)?.lane()?;
        let generator = lane.generator(doc)?;
        let layout = &self.layout;
        let viewport = self.viewport(doc);
        let width = layout.wave_width_f64();
        let ppu = viewport.px_per_unit(width);
        if lane::is_density(generator, ppu) {
            return None;
        }
        let geometry = LaneGeometry::new(
            layout.row_y(row),
            layout.row_height(row),
            layout.row_h,
            lane.height,
        );
        let sub = geometry.sub_at(p.y)?;
        let time = viewport.time_at(f64::from(p.x - layout.waves.left()), width);
        let tolerance = 3.0 * f64::from(layout.zoom) / ppu;
        let ordinal = lane::hit(generator, lane.height, sub, time, tolerance)?;
        let tx = &generator.transactions()[ordinal];
        Some((tx.generator, tx.id))
    }

    /// The visible row whose name-cell bottom edge is under `p`.
    pub fn row_edge_at(&self, p: Point) -> Option<usize> {
        let layout = &self.layout;
        if !layout.names.contains(p) {
            return None;
        }
        let grab = ROW_EDGE_GRAB_PX * layout.zoom;
        layout
            .rows
            .clone()
            .filter(|&r| r < self.items.len())
            .map(|r| (r, (layout.row_y(r) + layout.row_height(r) - p.y).abs()))
            .filter(|&(r, d)| {
                d <= grab && layout.row_y(r) + layout.row_height(r) > layout.names.top() + grab
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(r, _)| r)
    }

    fn update_hover(&mut self) {
        self.edge_hover = self.pointer.and_then(|p| self.row_edge_at(p));
        let (hover_row, badge_hover) = match self.pointer {
            Some(p) if self.layout.bounds.contains(p) && p.y >= self.layout.names.top() => {
                let row = self.layout.row_at(p.y).filter(|r| *r < self.items.len());
                let badge = self.layout.badge_at(p).map(|(ix, _)| ix);
                (row, badge)
            }
            _ => (None, None),
        };
        self.hover_row = hover_row;
        self.badge_hover = badge_hover;
    }

    /// Forget every hover state; returns whether any was set.
    fn clear_hover(&mut self) -> bool {
        let had = self.hover_row.is_some()
            || self.badge_hover.is_some()
            || self.edge_hover.is_some()
            || self.split_hover
            || self.chip_hover;
        self.hover_row = None;
        self.badge_hover = None;
        self.edge_hover = None;
        self.split_hover = false;
        self.chip_hover = false;
        had
    }

    // -- pointer input -------------------------------------------------------------

    /// Handle pointer input over panel `panel`. Returns true when something
    /// visible changed.
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
                let had = self.drag.is_some();
                if let Some(Drag::Rows { gap, collapse, .. }) = self.drag {
                    self.drag = None;
                    match (gap, collapse) {
                        (Some(gap), _) => {
                            self.move_selected_to(gap);
                        }
                        (None, Some(row)) => self.select_row(row, Modifiers::default()),
                        (None, None) => {}
                    }
                } else if let Some(Drag::ZoomRange { start, current }) = self.drag.take()
                    && (current.x - start.x).abs() >= ZOOM_RANGE_MIN_PX * self.layout.zoom
                {
                    let layout = &self.layout;
                    let viewport = self.viewport(doc);
                    let a =
                        viewport.time_at(f64::from(start.x - layout.waves.left()), self.wave_w());
                    let b =
                        viewport.time_at(f64::from(current.x - layout.waves.left()), self.wave_w());
                    let target = Viewport {
                        start: a.min(b),
                        end: a.max(b),
                    };
                    self.animate_to(doc, target, now);
                }
                had
            }
            PointerEvent::Leave => {
                self.pointer = None;
                self.clear_hover()
            }
            PointerEvent::Wheel {
                position,
                dx,
                dy,
                modifiers,
                ..
            } => {
                self.wheel(doc, position, dx, dy, modifiers, now);
                true
            }
            PointerEvent::Pinch { position, delta } => {
                let factor = f64::from(1.0 + delta).clamp(0.2, 5.0);
                let x = (position.x - self.layout.waves.left()).max(0.0);
                self.zoom_at(doc, x, factor);
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
        let layout = self.layout.clone();
        self.pointer = Some(p);
        self.menu = None;
        self.pressed_record = false;
        if button == MouseButton::Left {
            if layout.names_split.contains(p) {
                self.drag = Some(Drag::NamesSplit);
                return;
            }
            if layout.values_split.contains(p) {
                self.drag = Some(Drag::ValuesSplit);
                return;
            }
            if let Some((track, thumb)) = layout.scrollbar {
                if thumb.contains(p) {
                    self.drag = Some(Drag::Scroll {
                        grab: p.y - thumb.top(),
                    });
                    return;
                }
                if track.contains(p) {
                    let travel = track.height() - thumb.height();
                    let frac =
                        ((p.y - track.top() - thumb.height() / 2.0) / travel).clamp(0.0, 1.0);
                    self.scroll_y = layout.max_scroll * frac;
                    self.drag = Some(Drag::Scroll {
                        grab: thumb.height() / 2.0,
                    });
                    return;
                }
            }
            if let Some(ix) = layout.chip_at(p) {
                if modifiers.shift {
                    doc.remove_marker(ix);
                } else {
                    let t = doc.markers[ix].time;
                    self.set_cursor(doc, Some(t));
                }
                return;
            }
        }
        let in_waves_x = p.x >= layout.waves.left() && p.x < layout.waves.right();
        let wave_wf = layout.wave_width_f64();
        let snap_px = doc.navigation.snap_px * f64::from(layout.zoom);
        if in_waves_x && button == MouseButton::Left && (modifiers.control || modifiers.platform) {
            let displayed = self.viewport(doc);
            self.nav.viewport_state_mut(doc).set(displayed);
            self.drag = Some(Drag::ZoomRange {
                start: p,
                current: p,
            });
            return;
        }
        if layout.header.contains(p) {
            if in_waves_x && button == MouseButton::Left {
                let x = f64::from(p.x - layout.waves.left());
                let t = snapped_time(&self.viewport(doc), None, x, wave_wf, snap_px);
                self.set_cursor(doc, Some(t));
                self.drag = Some(Drag::Cursor);
            }
            return;
        }
        let row = layout.row_at(p.y).filter(|r| *r < self.items.len());
        if in_waves_x {
            match button {
                MouseButton::Left => {
                    let x = f64::from(p.x - layout.waves.left());
                    let t = snapped_time(
                        &self.viewport(doc),
                        row.and_then(|r| self.edge_source(doc, r)).as_ref(),
                        x,
                        wave_wf,
                        snap_px,
                    );
                    // A press on a lane selects the record under it for
                    // every panel, or clears the selection over empty space.
                    if let Some(r) = row.filter(|r| self.items[*r].lane().is_some()) {
                        let hit = self.lane_hit(doc, r, p);
                        doc.select(hit.map(|(track, id)| TxSelection {
                            track,
                            id,
                            origin: panel,
                        }));
                        self.pressed_record = hit.is_some();
                    }
                    self.set_cursor(doc, Some(t));
                    self.drag = Some(Drag::Cursor);
                    if let Some(r) = row
                        && (!self.selected.contains(&r) || modifiers.shift || modifiers.secondary())
                    {
                        self.select_row(r, modifiers);
                    }
                }
                MouseButton::Middle | MouseButton::Right => {
                    self.drag = Some(Drag::Pan { last_x: p.x });
                }
            }
            return;
        }
        // Signal-name context menu. Preserve a group when the clicked row is
        // already selected; otherwise make this the single selected row.
        if button == MouseButton::Right && layout.names.contains(p) {
            if let Some(row) = row {
                self.open_signal_menu(doc, row, p);
            }
            return;
        }
        // Names / values columns.
        if button != MouseButton::Left {
            return;
        }
        if let Some(row) = self.row_edge_at(p) {
            self.drag = Some(Drag::RowHeight {
                row,
                top: layout.row_y(row),
            });
            return;
        }
        if let Some((ix, b)) = layout.badge_at(p) {
            let pos = point(b.left(), b.bottom() + 4.0 * layout.zoom);
            if !self.selected.contains(&ix) {
                self.select_row(ix, modifiers);
            }
            self.open_format_menu(doc, ix, pos);
            return;
        }
        let Some(r) = row else {
            self.selected.clear();
            return;
        };
        // A plain press inside the selection keeps the group so it can be
        // dragged; releasing without a drag then selects just this row.
        let keep_group = self.selected.contains(&r) && !modifiers.shift && !modifiers.secondary();
        if !keep_group {
            self.select_row(r, modifiers);
        }
        if self.selected.contains(&r) {
            self.drag = Some(Drag::Rows {
                press_y: p.y,
                started: false,
                gap: None,
                collapse: keep_group.then_some(r),
                scrolled_at: None,
            });
        }
    }

    fn pointer_move(&mut self, doc: &mut Document, p: Point) -> bool {
        self.pointer = Some(p);
        let layout = &self.layout;
        let wave_wf = layout.wave_width_f64();
        match self.drag {
            Some(Drag::ZoomRange { start, .. }) => {
                self.drag = Some(Drag::ZoomRange { start, current: p });
                true
            }
            Some(Drag::Cursor) => {
                let x = f64::from(p.x - layout.waves.left()).clamp(0.0, wave_wf);
                let row = layout.row_at(p.y).filter(|r| *r < self.items.len());
                let t = snapped_time(
                    &self.viewport(doc),
                    row.and_then(|r| self.edge_source(doc, r)).as_ref(),
                    x,
                    wave_wf,
                    doc.navigation.snap_px * f64::from(layout.zoom),
                );
                self.set_cursor(doc, Some(t));
                true
            }
            Some(Drag::Pan { last_x }) => {
                let dx = last_x - p.x;
                self.drag = Some(Drag::Pan { last_x: p.x });
                self.pan_px(doc, dx);
                true
            }
            // Column widths are kept at zoom 1.0 (they are saved in workspaces).
            Some(Drag::NamesSplit) => {
                self.names_width = ((p.x - layout.bounds.left()) / layout.zoom).max(MIN_COLUMN);
                true
            }
            Some(Drag::ValuesSplit) => {
                self.values_width = ((p.x - layout.names.right()) / layout.zoom).max(MIN_COLUMN);
                true
            }
            Some(Drag::Rows { gap, started, .. }) => {
                self.update_row_drag();
                !matches!(self.drag, Some(Drag::Rows { gap: g, started: s, .. }) if g == gap && s == started)
                    || self.row_drag_scroll_speed() != 0.0
            }
            Some(Drag::RowHeight { row, top }) => {
                let want = (p.y - top) / layout.row_h.max(1.0);
                let height = RowHeight::PRESETS
                    .into_iter()
                    .min_by(|a, b| {
                        (f32::from(a.multiple()) - want)
                            .abs()
                            .total_cmp(&(f32::from(b.multiple()) - want).abs())
                    })
                    .unwrap_or_default();
                let rows = self.menu_rows(row);
                if rows.iter().all(|&r| self.items[r].height() == height) {
                    return false;
                }
                self.resize_rows(&rows, row, |_| height);
                true
            }
            Some(Drag::Scroll { grab }) => {
                if let Some((track, thumb)) = layout.scrollbar {
                    let travel = track.height() - thumb.height();
                    if travel > 0.0 {
                        let frac = ((p.y - grab - track.top()) / travel).clamp(0.0, 1.0);
                        self.scroll_y = layout.max_scroll * frac;
                        return true;
                    }
                }
                false
            }
            None => {
                // Hover feedback only needs a repaint when the hovered row or
                // badge changes, when we enter/leave splitter zones, or when
                // the pointer leaves the table while something was hovered.
                let (prev_row, prev_badge, prev_split, prev_chip, prev_edge) = (
                    self.hover_row,
                    self.badge_hover,
                    self.split_hover,
                    self.chip_hover,
                    self.edge_hover,
                );
                if layout.bounds.contains(p) {
                    self.split_hover = layout.near_split(p);
                    self.chip_hover = layout.chip_at(p).is_some();
                    self.update_hover();
                } else {
                    self.clear_hover();
                }
                // A plot's hover readout follows the pointer.
                let reading = p.x >= self.layout.waves.left()
                    && self
                        .hover_row
                        .is_some_and(|r| self.signal(r).is_some_and(|s| s.analog.is_some()));
                self.hover_row != prev_row
                    || self.badge_hover != prev_badge
                    || self.split_hover != prev_split
                    || self.chip_hover != prev_chip
                    || self.edge_hover != prev_edge
                    || reading
            }
        }
    }

    /// Wheel over waves pans time; over names/values it scrolls rows.
    fn wheel(
        &mut self,
        doc: &mut Document,
        p: Point,
        dx: f32,
        dy: f32,
        modifiers: Modifiers,
        now: Instant,
    ) {
        if modifiers.control || modifiers.platform {
            let factor = 2f64.powf(f64::from(dy) / 120.0);
            let x = (p.x - self.layout.waves.left()).max(0.0);
            self.zoom_at(doc, x, factor);
        } else if modifiers.shift {
            // Some hosts translate Shift-wheel's vertical delta to horizontal.
            self.scroll_y = (self.scroll_y - dy - dx).clamp(0.0, self.layout.max_scroll);
        } else if p.x >= self.layout.waves.left() {
            self.pan_fraction(doc, -f64::from(dx + dy) / 1000.0, now);
        } else {
            if dx.abs() > 0.0 {
                self.pan_px(doc, -dx);
            }
            if dy.abs() > 0.0 {
                self.scroll_y = (self.scroll_y - dy).clamp(0.0, self.layout.max_scroll);
            }
        }
    }
}

/// Where a click on the waves at `x_px` lands after snapping to the nearest
/// edge of the row within `snap_px` pixels (0 disables snapping): a signal
/// transition, or a lane's record begin or end.
fn snapped_time(
    vp: &Viewport,
    edges: Option<&EdgeSource<'_>>,
    x_px: f64,
    width_px: f64,
    snap_px: f64,
) -> u64 {
    let raw = vp.time_at(x_px, width_px).round().max(0.0);
    let Some(edges) = edges else {
        return raw as u64;
    };
    if snap_px <= 0.0 {
        return raw as u64;
    }
    let tol = snap_px / vp.px_per_unit(width_px);
    let h = match edges {
        EdgeSource::History(h) => h.as_ref(),
        EdgeSource::Lane(g) => {
            let exact = vp.time_at(x_px, width_px).max(0.0);
            return lane::nearest_boundary(g, exact, tol).unwrap_or(raw as u64);
        }
    };
    let t = raw as u64;
    let mut best: Option<(f64, u64)> = None;
    let mut consider = |cand: u64| {
        let d = (cand as f64 - raw).abs();
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, cand));
        }
    };
    match h.index_at(t) {
        Some(i) => {
            consider(h.time(i));
            if i + 1 < h.len() {
                consider(h.time(i + 1));
            }
        }
        None => {
            if !h.is_empty() {
                consider(h.time(0));
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(t)
}
