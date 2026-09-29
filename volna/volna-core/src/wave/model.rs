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
use super::overlay::SpanClocks;
use super::tint::{self, Tint};
use super::tree::{self, Entry, Place, Splice};
use super::viewport::Viewport;
use crate::clock::ClockKey;
use crate::data::loaded_tracks::LoadedGenerator;
use crate::data::transactions::TrackRef;
use crate::data::{SignalHistory, SignalRef, SignalShape, Translator, VarId};
use crate::document::{Document, TxSelection};
use crate::geometry::{Modifiers, MouseButton, Point, point};
use crate::history::{Before, Edit, History, MergeKey, RowSelection, count};
use crate::nav::NavState;
pub use crate::nav::{Link, LinkDim};
use crate::panels::PanelId;
use crate::selection;
use crate::theme::Theme;
use crate::trace::{TraceId, Traced};

/// A ⌘-drag narrower than this (at zoom 1.0) is a click, not a zoom range.
pub const ZOOM_RANGE_MIN_PX: f32 = 4.0;
/// A press on a signal name must travel this far (at zoom 1.0) to start
/// moving rows; shorter presses are clicks.
pub const ROW_DRAG_MIN_PX: f32 = 4.0;
/// A middle press that moves less than this is a click, not a pan.
const CLICK_SLOP_PX: f32 = 3.0;
/// A press this close (at zoom 1.0) to the bottom edge of a row's name cell
/// resizes the row instead of selecting it.
pub const ROW_EDGE_GRAB_PX: f32 = 3.0;
/// While moving rows, the pointer this close to the top or bottom edge of the
/// rows (in rows) scrolls them, faster the deeper it goes.
const ROW_DRAG_EDGE_ROWS: f32 = 1.0;
/// Auto-scroll speed in rows per second per row of depth into the edge zone.
const ROW_DRAG_SCROLL_RATE: f32 = 12.0;

/// A row is either bound to a variable of an open trace or retains an
/// unresolved durable locator in its trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowSource {
    Resolved {
        trace: TraceId,
        var: VarId,
        signal: SignalRef,
    },
    Unresolved {
        trace: TraceId,
        path: Vec<String>,
        nth: Option<usize>,
        ambiguous: bool,
    },
}

impl RowSource {
    pub fn trace(&self) -> TraceId {
        match self {
            Self::Resolved { trace, .. } | Self::Unresolved { trace, .. } => *trace,
        }
    }

    pub fn signal(&self) -> Option<Traced<SignalRef>> {
        match self {
            Self::Resolved { trace, signal, .. } => Some(Traced::new(*trace, *signal)),
            _ => None,
        }
    }

    pub fn var(&self) -> Option<Traced<VarId>> {
        match self {
            Self::Resolved { trace, var, .. } => Some(Traced::new(*trace, *var)),
            _ => None,
        }
    }

    /// The row's path in its trace and which of equal paths it is.
    pub fn locator(&self, doc: &Document) -> (Vec<String>, Option<usize>) {
        match self {
            Self::Resolved { trace, var, .. } => doc
                .hierarchy(*trace)
                .map_or_else(|| (Vec::new(), None), |h| h.var_path(*var)),
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
    /// Its own colour; `None` inherits (see [`tint::ink`]).
    pub tint: Option<Tint>,
}

impl DisplayedSignal {
    /// The translator a workspace file records for this row.
    pub fn format_id(&self) -> String {
        self.requested_format
            .clone()
            .unwrap_or_else(|| self.translator.id().into())
    }
}

/// A declared clock drawn from its stretches: no dumped waveform is needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockRow {
    /// The clock's trace and path (its stream's path joined with '.').
    pub key: ClockKey,
    pub name: String,
    pub height: RowHeight,
    pub tint: Option<Tint>,
}

impl ClockRow {
    pub fn new(key: ClockKey) -> Self {
        let path = &key.item;
        Self {
            name: path.rsplit('.').next().unwrap_or(path).to_owned(),
            key,
            height: RowHeight::DEFAULT,
            tint: None,
        }
    }

    /// The clock's timeline, once its trace's clock is loaded.
    pub fn timeline<'a>(&self, doc: &'a Document) -> Option<&'a Arc<crate::clock::ClockTimeline>> {
        doc.clocks.find(&self.key)?.timeline()
    }
}

/// A named group of rows. Folded, it hides its rows and draws their
/// activity in its own row (`docs/wave_groups.html`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRow {
    pub name: String,
    pub collapsed: bool,
    /// The group's own row; its members keep their heights.
    pub height: RowHeight,
    /// The colour its rows without their own are drawn in.
    pub tint: Option<Tint>,
}

impl GroupRow {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collapsed: false,
            height: RowHeight::DEFAULT,
            tint: None,
        }
    }
}

/// One row of the waveform panel: a signal, a transaction generator shown
/// as a lane of bars, a declared clock, or a group of rows. All share the
/// row operations (selection, reordering, clipboard, heights, removal and
/// workspace entries).
#[derive(Clone)]
pub enum WaveRow {
    Signal(DisplayedSignal),
    Lane(TxLane),
    Clock(ClockRow),
    Group(GroupRow),
}

impl WaveRow {
    pub fn name(&self) -> &str {
        match self {
            Self::Signal(s) => &s.name,
            Self::Lane(l) => &l.name,
            Self::Clock(c) => &c.name,
            Self::Group(g) => &g.name,
        }
    }

    pub fn height(&self) -> RowHeight {
        match self {
            Self::Signal(s) => s.height,
            Self::Lane(l) => l.height,
            Self::Clock(c) => c.height,
            Self::Group(g) => g.height,
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
            Self::Clock(c) => c.height = height,
            Self::Group(g) => g.height = height,
        }
    }

    /// The row's own colour, without what it inherits.
    pub fn tint(&self) -> Option<Tint> {
        match self {
            Self::Signal(s) => s.tint,
            Self::Lane(l) => l.tint,
            Self::Clock(c) => c.tint,
            Self::Group(g) => g.tint,
        }
    }

    pub fn set_tint(&mut self, tint: Option<Tint>) {
        match self {
            Self::Signal(s) => s.tint = tint,
            Self::Lane(l) => l.tint = tint,
            Self::Clock(c) => c.tint = tint,
            Self::Group(g) => g.tint = tint,
        }
    }

    pub fn signal(&self) -> Option<&DisplayedSignal> {
        match self {
            Self::Signal(s) => Some(s),
            _ => None,
        }
    }

    pub fn signal_mut(&mut self) -> Option<&mut DisplayedSignal> {
        match self {
            Self::Signal(s) => Some(s),
            _ => None,
        }
    }

    pub fn lane(&self) -> Option<&TxLane> {
        match self {
            Self::Lane(l) => Some(l),
            _ => None,
        }
    }

    pub fn clock(&self) -> Option<&ClockRow> {
        match self {
            Self::Clock(c) => Some(c),
            _ => None,
        }
    }

    pub fn group(&self) -> Option<&GroupRow> {
        match self {
            Self::Group(g) => Some(g),
            _ => None,
        }
    }

    /// The loaded signal of a signal row.
    pub fn signal_ref(&self) -> Option<Traced<SignalRef>> {
        self.signal()?.source.signal()
    }

    /// The generator of a resolved lane.
    pub fn lane_track(&self) -> Option<Traced<TrackRef>> {
        self.lane()?.track()
    }

    /// The trace a signal, lane or clock row shows; groups belong to none.
    pub fn trace(&self) -> Option<TraceId> {
        match self {
            Self::Signal(s) => Some(s.source.trace()),
            Self::Lane(l) => Some(l.source.trace()),
            Self::Clock(c) => Some(c.key.trace),
            Self::Group(_) => None,
        }
    }

    /// A copy that holds no trace data: a signal row drops its history and,
    /// when resolved, its load error, so it loads again when attached. The
    /// clipboard, the undo journal and closed panels keep rows like this.
    pub fn detached(&self) -> Self {
        let mut row = self.clone();
        if let Self::Signal(item) = &mut row {
            item.history = None;
            if item.source.signal().is_some() {
                item.error = None;
            }
        }
        row
    }

    /// Whether two rows describe the same cockpit content: what a workspace
    /// stores of them, without the fold state (undo's projection).
    pub fn same(&self, other: &Self) -> bool {
        let analog = |a: &Option<Analog>| a.as_ref().map(|a| (a.draw, a.range));
        if self.tint() != other.tint() {
            return false;
        }
        match (self, other) {
            (Self::Signal(a), Self::Signal(b)) => {
                fn format(s: &DisplayedSignal) -> &str {
                    s.requested_format
                        .as_deref()
                        .unwrap_or_else(|| s.translator.id())
                }
                a.source == b.source
                    && format(a) == format(b)
                    && a.height == b.height
                    && analog(&a.analog) == analog(&b.analog)
            }
            (Self::Lane(a), Self::Lane(b)) => a.source == b.source && a.height == b.height,
            (Self::Clock(a), Self::Clock(b)) => a == b,
            (Self::Group(a), Self::Group(b)) => a.name == b.name && a.height == b.height,
            _ => false,
        }
    }
}

/// Give detached signal rows their data: a history another row already
/// holds for the same signal, or one queued load. Lanes and plots follow
/// through the app's reconciliation after each command.
pub(crate) fn attach_rows<'a>(
    rows: impl IntoIterator<Item = &'a mut Entry>,
    doc: &mut Document,
    resident: &Resident,
) {
    for row in rows.into_iter().filter_map(|e| e.row.signal_mut()) {
        let Some(signal) = row.source.signal() else {
            continue;
        };
        if row.history.is_some() || row.error.is_some() {
            continue;
        }
        row.history = resident.get(&signal).cloned();
        if row.history.is_none() {
            doc.request_signal(signal);
        }
    }
}

/// Histories already held by some row, by signal: new rows share them.
pub type Resident = HashMap<Traced<SignalRef>, Arc<dyn SignalHistory>>;

/// What the cursor snaps to and steps through on a row.
enum EdgeSource<'a> {
    History(Arc<dyn SignalHistory>),
    Lane(&'a LoadedGenerator),
    Clock(Arc<crate::clock::ClockTimeline>),
    /// A folded or open group: every change of its loaded signals.
    Group(Vec<Arc<dyn SignalHistory>>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Cursor,
    ZoomRange {
        start: Point,
        current: Point,
    },
    /// A marker chip held by the pointer: a click goes to the marker, a
    /// drag moves it.
    Marker(crate::marker::MarkerDrag),
    /// Panning with the middle or right button. A middle press that has
    /// not moved yet keeps its `click` point: released there, it measures
    /// from that point instead.
    Pan {
        last_x: f32,
        click: Option<Point>,
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
    /// (a visible position, or the visible row count for the end) once the
    /// press has moved far enough and the drop would change the tree; the
    /// rows land at `depth`, or inside the folded group `into` (an entry).
    /// A plain press on an already selected row keeps the selection for
    /// dragging and narrows it to `collapse` only if it ends as a click.
    Rows {
        press_y: f32,
        started: bool,
        gap: Option<usize>,
        depth: u8,
        into: Option<usize>,
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
    /// Hide the ruler of the clock with this path.
    HideRuler(ClockKey),
    /// Put the selected rows under a new group and rename it.
    Group,
    /// Dissolve the selected groups, keeping their rows.
    Ungroup,
    Rename,
    /// Fold (`true`) or unfold the menu's group.
    Fold(bool),
    /// Fold or unfold every group of the panel.
    FoldAll(bool),
    /// A choice of a Markers or Measure lane menu (`WaveMenuKind::Lane`).
    Lane(crate::marker::LaneVerb),
    /// Colour the menu's rows; `None` is Default.
    Tint(Option<Tint>),
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
    /// A clock ruler's menu; `row` is the ruler's index.
    Ruler,
    /// A Markers or Measure lane menu, which the app holds for any timed
    /// panel (`App::menu`); `row` counts the menus opened, so each is new.
    Lane,
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

/// A wave row as assistive technology sees it (a tree item).
#[derive(Clone, Debug, PartialEq)]
pub struct AccessibleRow {
    pub entry: usize,
    pub label: String,
    /// 1 at the top level.
    pub level: usize,
    /// Groups only: whether their rows are shown.
    pub expanded: Option<bool>,
    pub selected: bool,
    /// The row's name cell.
    pub bounds: crate::geometry::Rect,
}

/// Inverses of a wave panel's row edits since the app last collected them
/// into the undo journal (`volna/volna/ARCHITECTURE.md`, "Undo and redo").
#[derive(Default)]
pub(crate) struct RowJournal {
    /// Each edit's label and inverse splices, in the order they were made.
    pub edits: Vec<(String, Vec<Splice>)>,
    /// The selection before the first of them; holding it marks the
    /// journal as waiting to be collected.
    pub selection: Before<RowSelection>,
    /// The adjustment the edits repeat, and one they may be continued by.
    pub merge: Option<MergeKey>,
    pub continues: Option<MergeKey>,
}

impl RowJournal {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}

pub struct WaveModel {
    /// The rows as a tree in pre-order (see [`tree`]). Only
    /// [`WaveModel::splice`] changes the vector; row edits go through
    /// [`WaveModel::edit_rows`], which journals their inverse. Folds, loaded
    /// histories and a new lane's height change rows in place: they are
    /// navigation or trace data, not cockpit edits.
    items: Vec<Entry>,
    journal: RowJournal,
    /// Selected entries; always visible ones.
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
    /// Pointer is over a column divider (drives repaints).
    pub split_hover: bool,
    /// The Markers lane chip or span under the pointer (drives repaints).
    pub lane_hover: Option<crate::marker::LaneHit>,
    pub drag: Option<Drag>,
    /// Width of the waves column at the last layout, for keyboard zoom.
    pub wave_width: f32,
    /// Canvas paints so far, for tests that need real repaints.
    pub frames_painted: u64,
    pub menu: Option<WaveMenu>,
    /// Last known pointer position over the panel.
    pub pointer: Option<Point>,
    /// The last press landed on a lane bar and selected its record, so a
    /// second click opens it.
    pub pressed_record: bool,
    /// The group whose name is being edited; the frontend hosts a text
    /// input over [`WaveModel::rename_rect`] and answers with
    /// `Command::RenameGroup`.
    pub rename: Option<usize>,
    /// When analog ranges last eased, while one is moving.
    analog_eased_at: Option<Instant>,
    layout: WaveLayout,
    /// Counts changes of the rows' shape (see [`WaveModel::revision`]).
    revision: u64,
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
            journal: RowJournal::default(),
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
            lane_hover: None,
            drag: None,
            wave_width: 800.0,
            frames_painted: 0,
            menu: None,
            pointer: None,
            pressed_record: false,
            rename: None,
            analog_eased_at: None,
            layout: WaveLayout::default(),
            revision: 0,
        }
    }

    /// Forget every row and interaction; called when the document's session changes.
    pub fn reset(&mut self, limits: Option<(u64, u64)>) {
        self.nav.reset(limits);
        self.items.clear();
        self.journal = RowJournal::default();
        self.selected.clear();
        self.anchor = None;
        self.scroll_y = 0.0;
        self.menu = None;
        self.drag = None;
        self.rename = None;
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

    fn signals_mut(&mut self) -> impl Iterator<Item = &mut DisplayedSignal> {
        self.items.iter_mut().filter_map(|e| e.row.signal_mut())
    }

    /// Fit the visible plots' target ranges to the current viewport.
    fn update_analog_targets(&mut self, doc: &Document) {
        let vp = self.viewport(doc);
        let entries: Vec<usize> = self
            .layout
            .rows
            .clone()
            .filter_map(|pos| self.layout.entry(pos))
            .collect();
        let items = &mut self.items;
        for item in items
            .iter_mut()
            .enumerate()
            .filter(|(i, _)| entries.binary_search(i).is_ok())
            .filter_map(|(_, e)| e.row.signal_mut())
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
        for item in self.signals_mut() {
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

    /// The trace entry `ix` shows: a leaf's own, or the one trace every
    /// row of a group comes from.
    pub fn row_trace(&self, ix: usize) -> Option<TraceId> {
        let entry = self.items.get(ix)?;
        if !entry.is_group() {
            return entry.row.trace();
        }
        let mut traces = tree::leaves(&self.items, ix).filter_map(|j| self.items[j].row.trace());
        let first = traces.next()?;
        traces.all(|t| t == first).then_some(first)
    }

    /// The signal of row `row`, if it is a signal row.
    pub fn signal(&self, row: usize) -> Option<&DisplayedSignal> {
        self.items.get(row)?.signal()
    }

    // -- items ---------------------------------------------------------------

    /// The painted rows in order, as entry indices: every entry outside a
    /// folded group.
    pub fn visible(&self) -> Arc<[u32]> {
        tree::visible(&self.items)
    }

    /// The rows as a tree in pre-order (see [`tree`]).
    pub fn items(&self) -> &[Entry] {
        &self.items
    }

    // -- row edits -------------------------------------------------------------

    /// Perform `splices` on the rows and return their inverse (see
    /// [`tree::apply`]); the undo journal applies its edits through here.
    /// Nothing is journaled, and nothing changes when the splices do not fit.
    pub(crate) fn splice(&mut self, splices: Vec<Splice>) -> Result<Vec<Splice>, String> {
        let inverse = tree::apply(&mut self.items, splices)?;
        self.edited();
        Ok(inverse)
    }

    /// Edit the rows: perform `splices` planned against the current rows,
    /// journal their inverse under `label`, and select `selection`.
    fn edit_rows(&mut self, label: String, splices: Vec<Splice>, selection: RowSelection) {
        if splices.is_empty() {
            return;
        }
        let selection_before = self.row_selection();
        self.journal.selection.note(|| selection_before);
        let inverse = self
            .splice(splices)
            .expect("row edits are planned against the current rows");
        self.journal.edits.push((label, inverse));
        self.set_row_selection(selection);
    }

    /// Rewrite rows in place as one edit: `f` changes a copy of each of
    /// `rows` and says whether it did. Returns whether any row changed.
    fn rewrite_rows(
        &mut self,
        label: String,
        merge: Option<MergeKey>,
        rows: impl IntoIterator<Item = usize>,
        mut f: impl FnMut(&mut Entry) -> bool,
    ) -> bool {
        let mut splices: Vec<Splice> = Vec::new();
        let mut rows: Vec<usize> = rows.into_iter().filter(|&r| r < self.items.len()).collect();
        rows.sort_unstable();
        rows.dedup();
        for row in rows {
            let mut entry = self.items[row].clone();
            if !f(&mut entry) {
                continue;
            }
            match splices.last_mut() {
                Some(run) if run.at + run.remove == row => {
                    run.remove += 1;
                    run.insert.push(entry);
                }
                _ => splices.push(Splice {
                    at: row,
                    remove: 1,
                    insert: vec![entry],
                }),
            }
        }
        if splices.is_empty() {
            return false;
        }
        if merge.is_some() {
            self.journal.merge = merge;
        }
        let selection = self.row_selection();
        let (rename, menu) = (self.rename, self.menu.take());
        self.edit_rows(label, splices, selection);
        // In-place rewrites keep every index, so what names rows stays valid.
        self.rename = rename;
        self.menu = menu;
        true
    }

    /// The selection, as steps store it.
    pub(crate) fn row_selection(&self) -> RowSelection {
        RowSelection {
            selected: self.selected.clone(),
            anchor: self.anchor,
        }
    }

    /// Select rows (an edit's result, or a restored step's context); rows
    /// that no longer exist are left out and hidden ones give way to the
    /// group that shows them.
    pub(crate) fn set_row_selection(&mut self, selection: RowSelection) {
        let len = self.items.len();
        self.selected = selection
            .selected
            .into_iter()
            .filter(|&i| i < len)
            .collect();
        self.anchor = selection.anchor.filter(|&a| a < len);
        self.fix_hidden_selection();
    }

    /// Hand the row edits since the last call to the undo journal, with the
    /// selection before them and their merge keys.
    pub(crate) fn take_edits(&mut self, panel: crate::panels::PanelId, history: &mut History) {
        if self.journal.is_empty() {
            return;
        }
        let mut journal = std::mem::take(&mut self.journal);
        if let Some(selection) = journal.selection.take() {
            history.note_selection(panel, selection);
        }
        for (label, splices) in journal.edits {
            history.record(Edit::Rows { panel, splices }, Some(label));
        }
        if let Some(key) = journal.merge {
            history.set_merge(key.in_panel(panel));
        }
        if let Some(key) = journal.continues {
            history.set_continues(key.in_panel(panel));
        }
    }

    /// Install rows that are not an edit (a restored workspace).
    pub(crate) fn restore_rows(&mut self, items: Vec<Entry>) {
        debug_assert!(tree::validate(&items).is_ok());
        self.items = items;
        self.edited();
    }

    /// Drop every row's trace data: the panel is closed and kept by the
    /// undo journal.
    pub(crate) fn detach_rows(&mut self) {
        for entry in &mut self.items {
            entry.row = entry.row.detached();
        }
        self.drag = None;
        self.menu = None;
        self.rename = None;
        self.clear_hover();
    }

    /// Give detached rows their data again (see [`attach_rows`]).
    pub(crate) fn attach_rows(&mut self, doc: &mut Document, resident: &Resident) {
        attach_rows(&mut self.items, doc, resident);
    }

    /// Whether some resolved signal row waits for a history.
    pub(crate) fn needs_attach(&self) -> bool {
        self.items.iter().any(|e| {
            e.signal().is_some_and(|s| {
                s.source.signal().is_some() && s.history.is_none() && s.error.is_none()
            })
        })
    }

    /// Retry loading `signal`: its rows show loading again.
    pub(crate) fn clear_error(&mut self, signal: Traced<SignalRef>) {
        for row in self
            .signals_mut()
            .filter(|s| s.source.signal() == Some(signal))
        {
            row.error = None;
        }
    }

    /// Scroll so entry `entry` (or the visible group holding it) is on
    /// screen, as of the last layout.
    pub(crate) fn reveal_entry(&mut self, entry: usize) {
        let row_h = self.layout.row_h;
        let height = self.layout.names.height();
        if row_h <= 0.0 || height <= 0.0 || entry >= self.items.len() {
            return;
        }
        let visible = self.visible();
        let mut shown = entry;
        while visible.binary_search(&(shown as u32)).is_err() {
            match tree::parent(&self.items, shown) {
                Some(p) => shown = p,
                None => return,
            }
        }
        let top = self.row_units_before(shown) as f32 * row_h;
        let bottom = top + f32::from(self.items[shown].height().multiple()) * row_h;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + height {
            self.scroll_y = (bottom - height).max(0.0);
        }
    }

    /// Append `rows` at the top level and select them.
    fn push_rows(&mut self, label: String, rows: impl IntoIterator<Item = WaveRow>) {
        let first = self.items.len();
        let insert: Vec<Entry> = rows.into_iter().map(|row| Entry::new(0, row)).collect();
        let n = insert.len();
        if n == 0 {
            return;
        }
        let selection = RowSelection {
            selected: (first..first + n).collect(),
            anchor: Some(first),
        };
        let splice = Splice {
            at: first,
            remove: 0,
            insert,
        };
        self.edit_rows(label, vec![splice], selection);
    }

    /// Append rows for `vars`, sharing `loaded` histories for the same signal
    /// and queuing a load for the rest.
    pub fn add_vars(&mut self, doc: &mut Document, vars: &[Traced<VarId>], loaded: Resident) {
        let rows = var_rows(doc, vars, &loaded);
        let label = format!("Add {}", count(rows.len(), "signal", "signals"));
        self.push_rows(label, rows);
    }

    /// Append a group named after `scope` holding its variables and, when
    /// `recursive`, a folded subgroup for each child scope that has
    /// variables; select it. Scopes deeper than groups can nest are left out.
    pub fn add_scope_group(
        &mut self,
        doc: &mut Document,
        scope: Traced<crate::data::ScopeId>,
        recursive: bool,
        loaded: Resident,
    ) -> bool {
        let Some(session) = doc.session(scope.trace).cloned() else {
            return false;
        };
        let h = session.hierarchy();
        let mut entries = Vec::new();
        let mut stack = vec![(scope.item, 0u8)];
        while let Some((id, depth)) = stack.pop() {
            let Some(s) = h.get_scope(id) else { continue };
            let mut group = GroupRow::new(s.name);
            group.collapsed = depth > 0;
            entries.push(Entry::new(depth, WaveRow::Group(group)));
            let vars: Vec<_> = s.vars.iter().map(|v| scope.with(v)).collect();
            let rows = var_rows(doc, &vars, &loaded);
            entries.extend(rows.into_iter().map(|row| Entry::new(depth + 1, row)));
            // Child scopes with variables below them, while groups can nest.
            if recursive && depth + 2 < tree::MAX_DEPTH {
                let children = s.children.iter().rev().filter(|&c| h.has_vars(c));
                stack.extend(children.map(|c| (c, depth + 1)));
            }
        }
        if entries.is_empty() {
            return false;
        }
        let first = self.items.len();
        let label = format!("Add group {}", entries[0].name());
        let selection = RowSelection {
            selected: BTreeSet::from([first]),
            anchor: Some(first),
        };
        let splice = Splice {
            at: first,
            remove: 0,
            insert: entries,
        };
        self.edit_rows(label, vec![splice], selection);
        true
    }

    /// Append a lane for each generator in `tracks` and select them. A lane
    /// takes the default height for its depth as soon as its records are
    /// resident; the app retains them for as long as a lane shows them.
    pub fn add_lanes(&mut self, doc: &Document, tracks: &[Traced<TrackRef>]) {
        let lanes: Vec<WaveRow> = tracks
            .iter()
            .filter_map(|&track| TxLane::new(doc, track))
            .map(WaveRow::Lane)
            .collect();
        let label = format!("Add {}", count(lanes.len(), "lane", "lanes"));
        self.push_rows(label, lanes);
    }

    /// Append a clock row for each clock and select them.
    pub fn add_clocks(&mut self, keys: &[ClockKey]) {
        let label = format!("Add {}", count(keys.len(), "clock row", "clock rows"));
        self.push_rows(
            label,
            keys.iter()
                .map(|k| WaveRow::Clock(ClockRow::new(k.clone()))),
        );
    }

    /// Records arrived: new lanes take their default height.
    pub fn fit_lanes(&mut self, doc: &Document) {
        for row in &mut self.items {
            if let WaveRow::Lane(lane) = &mut row.row {
                lane.fit_height(doc);
            }
        }
    }

    /// A history load finished (the document already checked its generation).
    pub fn finish_signal(
        &mut self,
        signal: Traced<SignalRef>,
        result: anyhow::Result<Arc<dyn SignalHistory>>,
    ) {
        let result = result.map_err(|e| e.to_string());
        for item in self
            .signals_mut()
            .filter(|i| i.source.signal() == Some(signal))
        {
            item.history = result.as_ref().ok().cloned();
            item.error = result.as_ref().err().cloned();
        }
    }

    /// The rows changed shape: forget state that names entries.
    fn edited(&mut self) {
        self.rename = None;
        self.menu = None;
        self.revision += 1;
    }

    /// Changes whenever rows are added, removed, moved or rewritten, so
    /// what is derived from them can be kept until then.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Remove the selected rows; a selected group goes with everything in it.
    pub fn remove_selected(&mut self) {
        self.remove_selected_as("Remove");
    }

    fn remove_selected_as(&mut self, verb: &str) {
        let roots = tree::roots(&self.items, &self.selected);
        let label = match roots.as_slice() {
            [g] if self.items[*g].is_group() => {
                format!("{verb} group {}", self.items[*g].name())
            }
            [r] => format!("{verb} {}", self.items[*r].name()),
            _ => format!("{verb} {}", count(roots.len(), "row", "rows")),
        };
        let splices = tree::removal(&self.items, &self.selected);
        self.edit_rows(label, splices, RowSelection::default());
    }

    /// Remove the rows of a trace that is closing, and the groups only it
    /// filled, as one edit named `label`. Returns whether any went.
    pub(crate) fn remove_trace_rows(&mut self, trace: TraceId, label: String) -> bool {
        let rows = tree::of_trace(&self.items, trace);
        if rows.is_empty() {
            return false;
        }
        let splices = tree::removal(&self.items, &rows);
        self.edit_rows(label, splices, RowSelection::default());
        true
    }

    /// Load every resolved signal row again: its trace was placed anew.
    /// Rows keep their place, format and selection.
    pub(crate) fn reload_rows(&mut self, doc: &mut Document) {
        for row in self.signals_mut() {
            if row.source.signal().is_some() {
                row.history = None;
                row.error = None;
            }
        }
        attach_rows(&mut self.items, doc, &Resident::new());
    }

    /// Copy the selected rows, in display order and with the groups they
    /// head, to the document clipboard. Rows keep their format, height and
    /// the colour they are drawn in; histories stay with the panels. The
    /// clipboard is not journaled.
    pub fn copy_selected(&self, doc: &mut Document) {
        if self.selected.is_empty() {
            return;
        }
        let roots = tree::roots(&self.items, &self.selected);
        let mut rows: Vec<Entry> = tree::extract(&self.items, &self.selected)
            .iter()
            .map(Entry::detached)
            .collect();
        for (row, root) in rows.iter_mut().filter(|e| e.depth == 0).zip(roots) {
            row.row.set_tint(self.ink(root));
        }
        doc.copied_rows = rows;
    }

    pub fn cut_selected(&mut self, doc: &mut Document) {
        self.copy_selected(doc);
        self.remove_selected_as("Cut");
    }

    /// Move the selected rows, with the rows of selected groups and keeping
    /// their order, to `to`; they stay selected. Returns whether the tree
    /// changed.
    pub fn move_selected_to(&mut self, to: Place) -> bool {
        let Some(moved) = tree::plan_move(&self.items, &self.selected, to) else {
            return false;
        };
        let rank = self.anchor.and_then(|a| {
            tree::roots(&self.items, &self.selected)
                .iter()
                .position(|r| *r == a)
        });
        let roots = moved.roots;
        let anchor = rank
            .and_then(|k| roots.iter().nth(k).copied())
            .or_else(|| roots.first().copied());
        let label = match roots.len() {
            1 => format!(
                "Move {}",
                self.items[tree::roots(&self.items, &self.selected)[0]].name()
            ),
            n => format!("Move {n} rows"),
        };
        self.edit_rows(
            label,
            moved.splices,
            RowSelection {
                selected: roots,
                anchor,
            },
        );
        true
    }

    /// Insert the document clipboard below the last selected row, at its
    /// level (or at the end), and select the new rows. Duplicates share
    /// `loaded` histories for the same signal; the rest load once, like newly
    /// added variables.
    pub fn paste(&mut self, doc: &mut Document, loaded: &Resident) {
        if doc.copied_rows.is_empty() {
            return;
        }
        let to = match tree::roots(&self.items, &self.selected).last() {
            Some(&r) => Place {
                at: tree::subtree_end(&self.items, r),
                depth: self.items[r].depth,
            },
            None => Place {
                at: self.items.len(),
                depth: 0,
            },
        };
        let mut rows = doc.copied_rows.clone();
        for row in rows.iter_mut().filter_map(|e| e.row.signal_mut()) {
            if let Some(signal) = row.source.signal() {
                row.history = loaded.get(&signal).cloned();
                if row.history.is_none() {
                    doc.request_signal(signal);
                }
            }
        }
        let to = if rows.iter().any(|e| e.depth + to.depth >= tree::MAX_DEPTH) {
            Place {
                at: self.items.len(),
                depth: 0,
            }
        } else {
            to
        };
        let Some((splice, roots)) = tree::insertion(to, rows) else {
            return;
        };
        let label = format!("Paste {}", count(roots.len(), "row", "rows"));
        let anchor = roots.first().copied();
        self.edit_rows(
            label,
            vec![splice],
            RowSelection {
                selected: roots,
                anchor,
            },
        );
    }

    pub fn select_all(&mut self) {
        self.selected = self.visible().iter().map(|&i| i as usize).collect();
    }

    /// Escape: cancel a drag or a rename, close the menu, clear selection,
    /// then clear cursor.
    pub fn clear_selection(&mut self, doc: &mut Document) {
        if self.drag.is_some() {
            self.drag = None;
        } else if self.rename.is_some() {
            self.rename = None;
        } else if self.menu.is_some() {
            self.menu = None;
        } else if !self.selected.is_empty() {
            self.selected.clear();
        } else {
            self.set_cursor(doc, None);
        }
    }

    /// Click selection with platform conventions: plain = single, cmd/ctrl =
    /// toggle, shift = the visible rows from the anchor.
    fn select_row(&mut self, row: usize, modifiers: Modifiers) {
        if row >= self.items.len() {
            return;
        }
        if modifiers.shift {
            let visible = self.visible();
            let pos = |i: usize| visible.binary_search(&(i as u32)).ok();
            if let Some(b) = pos(row) {
                let a = self.anchor.and_then(pos).unwrap_or(b);
                self.selected
                    .extend(visible[a.min(b)..=a.max(b)].iter().map(|&i| i as usize));
            }
            return;
        }
        selection::select(&mut self.selected, &mut self.anchor, row, modifiers);
    }

    /// Select the visible row `delta` rows from the anchor.
    pub fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            return;
        }
        let pos = self
            .anchor
            .or_else(|| self.selected.iter().next().copied())
            .and_then(|i| visible.binary_search(&(i as u32)).ok())
            .unwrap_or(0) as isize;
        let next = visible[(pos + delta).clamp(0, visible.len() as isize - 1) as usize] as usize;
        self.selected.clear();
        self.selected.insert(next);
        self.anchor = Some(next);
    }

    // -- groups ------------------------------------------------------------------

    /// A name no group of the panel has yet: "Group 1", "Group 2", …
    fn fresh_group_name(&self) -> String {
        (1..)
            .map(|n| format!("Group {n}"))
            .find(|name| {
                !self
                    .items
                    .iter()
                    .any(|e| e.group().is_some_and(|g| g.name == *name))
            })
            .expect("unbounded")
    }

    /// `G`: put the selected rows, in order, under a new group at the place
    /// of the first one, select it and start renaming it.
    pub fn group_selected(&mut self) -> bool {
        let group = WaveRow::Group(GroupRow::new(self.fresh_group_name()));
        let Some((splices, g)) = tree::group(&self.items, &self.selected, group) else {
            return false;
        };
        let roots = tree::roots(&self.items, &self.selected).len();
        let label = format!("Group {}", count(roots, "row", "rows"));
        self.edit_rows(
            label,
            splices,
            RowSelection {
                selected: BTreeSet::from([g]),
                anchor: Some(g),
            },
        );
        // Naming the new group joins this step, however long it takes.
        self.journal.continues = Some(MergeKey::of("rename", &g));
        self.rename = Some(g);
        true
    }

    /// `Shift+G`: dissolve the selected groups; their rows move up one level
    /// in place and become the selection.
    pub fn ungroup_selected(&mut self) -> bool {
        let groups: BTreeSet<usize> = self
            .selected
            .iter()
            .copied()
            .filter(|&i| self.items.get(i).is_some_and(Entry::is_group))
            .collect();
        if groups.is_empty() {
            return false;
        }
        let label = match groups.len() {
            1 => "Ungroup".to_owned(),
            n => format!("Ungroup {n} groups"),
        };
        let (splices, children) = tree::ungroup(&self.items, &groups);
        let anchor = children.first().copied();
        self.edit_rows(
            label,
            splices,
            RowSelection {
                selected: children,
                anchor,
            },
        );
        true
    }

    /// Start renaming the anchored group (`F2`, double-click).
    pub fn start_rename(&mut self) -> bool {
        let Some(g) = self
            .anchor
            .filter(|a| self.selected.contains(a))
            .or_else(|| self.selected.first().copied())
            .filter(|&i| self.items.get(i).is_some_and(Entry::is_group))
        else {
            return false;
        };
        self.menu = None;
        self.rename = Some(g);
        true
    }

    /// Finish renaming: `Some` commits a non-empty name, `None` cancels.
    pub fn finish_rename(&mut self, name: Option<&str>) -> bool {
        let Some(g) = self.rename.take() else {
            return false;
        };
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
            return false;
        };
        self.rewrite_rows(
            "Rename group".into(),
            Some(MergeKey::of("rename", &g)),
            [g],
            |e| match &mut e.row {
                WaveRow::Group(group) if group.name != name => {
                    group.name = name.to_owned();
                    true
                }
                _ => false,
            },
        )
    }

    /// Fold or unfold group `i`, and with `deep` every group inside it.
    /// Selected rows the fold hides leave the selection, and the group takes
    /// their place, so no command acts on rows that cannot be seen.
    pub fn set_folded(&mut self, i: usize, collapsed: bool, deep: bool) -> bool {
        if !self.items.get(i).is_some_and(Entry::is_group) {
            return false;
        }
        let changed = tree::set_collapsed(&mut self.items, i, collapsed, deep);
        self.fix_hidden_selection();
        if changed {
            self.edited();
        }
        changed
    }

    /// Fold or unfold every group.
    pub fn fold_all(&mut self, collapsed: bool) -> bool {
        let mut changed = false;
        for i in 0..self.items.len() {
            changed |= tree::set_collapsed(&mut self.items, i, collapsed, false);
        }
        self.fix_hidden_selection();
        if changed {
            self.edited();
        }
        changed
    }

    /// `←` / `→` on a selected group: fold it (`open == false`) or unfold
    /// it. Returns `false` when the anchor is not a group, so the keys pan.
    pub fn fold_key(&mut self, open: bool, deep: bool) -> bool {
        let Some(g) = self
            .anchor
            .filter(|a| self.selected.contains(a))
            .filter(|&a| self.items.get(a).is_some_and(Entry::is_group))
        else {
            return false;
        };
        self.set_folded(g, !open, deep);
        true
    }

    /// Replace selected rows hidden inside folded groups by the visible
    /// group that holds them.
    pub(crate) fn fix_hidden_selection(&mut self) {
        let visible = self.visible();
        let shown = |items: &[Entry], mut i: usize| {
            while visible.binary_search(&(i as u32)).is_err() {
                match tree::parent(items, i) {
                    Some(p) => i = p,
                    None => break,
                }
            }
            i
        };
        let selected = std::mem::take(&mut self.selected);
        self.selected = selected
            .into_iter()
            .filter(|&i| i < self.items.len())
            .map(|i| shown(&self.items, i))
            .collect();
        self.anchor = self
            .anchor
            .filter(|&a| a < self.items.len())
            .map(|a| shown(&self.items, a));
    }

    /// The signal, lane and clock entries of `rows` and the groups among
    /// them: value commands such as formats apply to these.
    fn leaves_of(&self, rows: &[usize]) -> Vec<usize> {
        let set: BTreeSet<usize> = rows.iter().copied().collect();
        tree::selected_leaves(&self.items, &set)
    }

    // -- colours ----------------------------------------------------------------

    /// The colour entry `ix` is drawn in (see [`tint::ink`]).
    pub fn ink(&self, ix: usize) -> Option<Tint> {
        tint::ink(&self.items, ix)
    }

    /// Colour the selected rows and groups, or clear their own colour with
    /// `None`: one step, and nothing when every row already has it.
    pub fn tint_selected(&mut self, tint: Option<Tint>) -> bool {
        let rows: Vec<usize> = self.selected.iter().copied().collect();
        self.set_tint(&rows, tint)
    }

    /// Give `rows` their own colour `tint` (see [`WaveModel::tint_selected`]).
    pub fn set_tint(&mut self, rows: &[usize], tint: Option<Tint>) -> bool {
        let what = match rows {
            [row] => match self.items.get(*row) {
                Some(e) => e.name().to_owned(),
                None => return false,
            },
            _ if rows.iter().all(|&r| self.signal(r).is_some()) => {
                count(rows.len(), "signal", "signals")
            }
            _ => count(rows.len(), "row", "rows"),
        };
        let label = format!("Color {what} {}", Tint::label(tint));
        self.rewrite_rows(label, None, rows.iter().copied(), |e| {
            if e.row.tint() == tint {
                return false;
            }
            e.row.set_tint(tint);
            true
        })
    }

    // -- translators -----------------------------------------------------------

    fn set_translator(&mut self, doc: &Document, rows: &[usize], id: &str) {
        let Some(t) = doc.translators.get(id) else {
            return;
        };
        let label = format!("Format {}", t.name());
        let merge = MergeKey::of("format", &rows);
        self.rewrite_rows(label, Some(merge), rows.iter().copied(), |e| {
            match e.row.signal_mut() {
                Some(item) if t.applies(item.shape) && item.format_id() != t.id() => {
                    item.translator = t.clone();
                    item.requested_format = None;
                    true
                }
                _ => false,
            }
        });
    }

    pub fn cycle_format(&mut self, doc: &Document) {
        let rows = tree::selected_leaves(&self.items, &self.selected);
        let merge = MergeKey::of("format", &rows);
        let mut names: Vec<String> = Vec::new();
        let changed = self.rewrite_rows(String::new(), Some(merge), rows, |e| {
            let Some(item) = e.row.signal_mut() else {
                return false;
            };
            let options = doc.translators.applicable(item.shape);
            if options.is_empty() {
                return false;
            }
            let pos = options
                .iter()
                .position(|t| t.id() == item.translator.id())
                .unwrap_or(0);
            let next = options[(pos + 1) % options.len()].clone();
            if next.id() == item.format_id() {
                return false;
            }
            if !names.iter().any(|n| n == next.name()) {
                names.push(next.name().to_owned());
            }
            item.translator = next;
            item.requested_format = None;
            true
        });
        if changed && let Some((label, _)) = self.journal.edits.last_mut() {
            *label = match names.as_slice() {
                [name] => format!("Format {name}"),
                _ => "Next format".into(),
            };
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
        self.draw_rows(rows, |_| draw);
    }

    /// Draw each of `rows` as `draw` says: as a plot, or digitally with
    /// `None`; rows that cannot be plotted are skipped. One edit.
    fn draw_rows(&mut self, rows: &[usize], draw: impl Fn(&DisplayedSignal) -> Option<AnalogDraw>) {
        let label = match rows
            .iter()
            .filter_map(|&r| self.signal(r))
            .map(&draw)
            .next()
        {
            Some(Some(AnalogDraw::Step)) => "Show as step plot",
            Some(Some(AnalogDraw::Linear)) => "Show as plot",
            _ => "Show as digital",
        };
        let merge = MergeKey::of("analog", &rows);
        self.rewrite_rows(label.into(), Some(merge), rows.iter().copied(), |e| {
            let Some(item) = e.row.signal_mut() else {
                return false;
            };
            let draw = draw(item);
            if draw.is_some() && !analog::supports(item.shape, item.translator.as_ref()) {
                return false;
            }
            match (draw, &mut item.analog) {
                (Some(draw), Some(a)) if a.draw == draw => false,
                (Some(draw), Some(a)) => {
                    a.draw = draw;
                    true
                }
                (Some(draw), None) => {
                    let mut a = Analog::new(draw, AnalogRange::Trace);
                    if item.height == RowHeight::DEFAULT {
                        a.restore_height = Some(item.height);
                        item.height = RowHeight::ANALOG;
                    }
                    item.analog = Some(a);
                    true
                }
                (None, Some(a)) => {
                    if let Some(h) = a
                        .restore_height
                        .filter(|_| item.height == RowHeight::ANALOG)
                    {
                        item.height = h;
                    }
                    item.analog = None;
                    true
                }
                (None, None) => false,
            }
        });
        self.scroll_y = self.scroll_y.min(self.layout.max_scroll.max(0.0));
    }

    /// `A`: plot the selected rows, or turn them all back to digital when
    /// every plottable one already is a plot.
    pub fn toggle_analog(&mut self) {
        let rows = tree::selected_leaves(&self.items, &self.selected);
        self.toggle_analog_rows(&rows);
    }

    fn toggle_analog_rows(&mut self, rows: &[usize]) {
        let rows: Vec<usize> = rows.iter().copied().filter(|&r| self.can_plot(r)).collect();
        let on = rows
            .iter()
            .any(|&r| self.signal(r).is_some_and(|s| s.analog.is_none()));
        self.draw_rows(&rows, |s| {
            on.then(|| {
                s.analog
                    .as_ref()
                    .map_or(AnalogDraw::default_for(s.shape), |a| a.draw)
            })
        });
    }

    /// Choose the vertical range of the plotted `rows`; type limits apply
    /// only to formats that have them.
    pub fn set_analog_range(&mut self, rows: &[usize], range: AnalogRange) {
        let merge = MergeKey::of("analog", &rows);
        self.rewrite_rows(
            "Plot range".into(),
            Some(merge),
            rows.iter().copied(),
            |e| match e.row.signal_mut() {
                Some(item)
                    if range != AnalogRange::Type
                        || item.translator.limits(item.shape).is_some() =>
                {
                    match &mut item.analog {
                        Some(a) if a.range != range => {
                            a.range = range;
                            true
                        }
                        _ => false,
                    }
                }
                _ => false,
            },
        );
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
        let plottable: Vec<usize> = self
            .leaves_of(&targets)
            .into_iter()
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
        // Groups: make one, and dissolve, rename or fold the ones selected.
        let group = self.items[row].group();
        let any_group = targets.iter().any(|&r| self.items[r].is_group());
        let menu = self.menu.as_mut().expect("just opened");
        menu.entries.extend([
            MenuEntry::Separator,
            MenuEntry::Item(MenuItem {
                badge: Some("G".into()),
                ..MenuItem::plain(MenuAction::Group, "Group selection")
            }),
        ]);
        if any_group {
            menu.entries.push(MenuEntry::Item(MenuItem {
                badge: Some("⇧G".into()),
                ..MenuItem::plain(MenuAction::Ungroup, "Ungroup")
            }));
        }
        if let Some(g) = group {
            menu.entries.extend([
                MenuEntry::Item(MenuItem {
                    badge: Some("F2".into()),
                    ..MenuItem::plain(MenuAction::Rename, "Rename…")
                }),
                MenuEntry::Item(if g.collapsed {
                    MenuItem {
                        badge: Some("→".into()),
                        ..MenuItem::plain(MenuAction::Fold(false), "Unfold")
                    }
                } else {
                    MenuItem {
                        badge: Some("←".into()),
                        ..MenuItem::plain(MenuAction::Fold(true), "Fold")
                    }
                }),
            ]);
        }
        if self.items.iter().any(Entry::is_group) {
            menu.entries.extend([
                MenuEntry::Item(MenuItem::plain(MenuAction::FoldAll(true), "Fold all")),
                MenuEntry::Item(MenuItem::plain(MenuAction::FoldAll(false), "Unfold all")),
            ]);
        }
        // A colour is checked only when every target row has it as its own.
        let mut own = targets.iter().map(|&r| self.items[r].tint());
        let first = own.next();
        let shared = first.filter(|t| own.all(|other| other == *t));
        let colors = std::iter::once(None)
            .chain(Tint::ALL.map(Some))
            .map(|tint| MenuItem {
                action: MenuAction::Tint(tint),
                label: Tint::label(tint).into(),
                badge: None,
                checked: shared == Some(tint),
            })
            .collect();
        menu.entries.extend([
            MenuEntry::Separator,
            MenuEntry::Submenu {
                label: "Color".into(),
                items: colors,
            },
            MenuEntry::Submenu {
                label: "Height".into(),
                items: heights,
            },
            MenuEntry::Separator,
            MenuEntry::Item(MenuItem::plain(
                MenuAction::RemoveSignals,
                if any_group {
                    "Remove with contents"
                } else if self.items[row].lane().is_some() {
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
        let bottom = self
            .layout
            .entry_span(row)
            .map_or(self.layout.names.top(), |(y, h)| y + h);
        let y = bottom.clamp(self.layout.names.top(), self.layout.names.bottom());
        self.open_signal_menu(
            doc,
            row,
            point(self.layout.names.left() + 8.0 * self.layout.zoom, y),
        );
    }

    /// The frontend's popup reported a choice.
    /// Return a failed canonical signal to retry through the document owner.
    /// Open the menu of clock ruler `ruler` (its index among the panel's rulers).
    pub fn open_ruler_menu(&mut self, doc: &Document, ruler: usize, position: Point) {
        let Some(path) = self
            .nav
            .clocks()
            .rulers(&doc.clocks)
            .get(ruler)
            .map(|c| c.key())
        else {
            return;
        };
        self.menu = Some(WaveMenu {
            kind: WaveMenuKind::Ruler,
            row: ruler,
            position,
            entries: vec![MenuEntry::Item(MenuItem::plain(
                MenuAction::HideRuler(path),
                "Hide Ruler",
            ))],
        });
    }

    pub fn menu_select(
        &mut self,
        doc: &Document,
        action: &MenuAction,
    ) -> Option<Traced<SignalRef>> {
        let menu = self.menu.take()?;
        match action {
            MenuAction::HideRuler(path) => {
                self.nav.hide_ruler(&doc.clocks, path);
                return None;
            }
            _ if menu.kind == WaveMenuKind::Ruler => return None,
            _ => {}
        }
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
        let leaves = self.leaves_of(&rows);
        match action {
            MenuAction::Format(id) => self.set_translator(doc, &leaves, id),
            MenuAction::RowHeight(height) => self.resize_rows(&rows, menu.row, |_| *height),
            MenuAction::Tint(tint) => _ = self.set_tint(&rows, *tint),
            MenuAction::Draw(draw) => self.set_analog(&leaves, *draw),
            MenuAction::Range(range) => self.set_analog_range(&leaves, *range),
            MenuAction::ToggleAnalog => self.toggle_analog_rows(&leaves),
            MenuAction::Group => _ = self.group_selected(),
            MenuAction::Ungroup => _ = self.ungroup_selected(),
            MenuAction::Rename => {
                self.anchor = Some(menu.row);
                self.start_rename();
            }
            MenuAction::Fold(collapsed) => _ = self.set_folded(menu.row, *collapsed, false),
            MenuAction::FoldAll(collapsed) => _ = self.fold_all(*collapsed),
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
        self.visible()
            .iter()
            .take_while(|&&i| (i as usize) < row)
            .map(|&i| u32::from(self.items[i as usize].height().multiple()))
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
        let mut heights: Vec<RowHeight> = Vec::new();
        let merge = MergeKey::of("height", &rows);
        let changed = self.rewrite_rows(String::new(), Some(merge), rows.iter().copied(), |e| {
            let h = height(e.height());
            if h == e.height() {
                return false;
            }
            e.set_height(h);
            if !heights.contains(&h) {
                heights.push(h);
            }
            true
        });
        if !changed {
            return;
        }
        if let Some((label, _)) = self.journal.edits.last_mut() {
            *label = match heights.as_slice() {
                [h] => format!("Row height {}×", h.multiple()),
                _ => "Row height".into(),
            };
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
        let margin = super::viewport::FIT_MARGIN_PX * f64::from(self.layout.zoom.max(f32::EPSILON));
        self.nav.zoom_fit(doc, self.wave_w(), margin, now);
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
        match &self.items.get(row)?.row {
            WaveRow::Signal(s) => s.history.clone().map(EdgeSource::History),
            WaveRow::Lane(l) => l.generator(doc).map(EdgeSource::Lane),
            WaveRow::Clock(c) => c.timeline(doc).cloned().map(EdgeSource::Clock),
            WaveRow::Group(_) => Some(EdgeSource::Group(self.group_histories(row))),
        }
    }

    /// The loaded histories of the signals in group `group`, in order.
    pub fn group_histories(&self, group: usize) -> Vec<Arc<dyn SignalHistory>> {
        tree::leaves(&self.items, group)
            .filter_map(|j| self.items[j].signal()?.history.clone())
            .collect()
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
            Some(EdgeSource::Clock(c)) => c.next_edge(from),
            Some(EdgeSource::Group(hs)) => {
                hs.iter().filter_map(|h| h.next_change_after(from)).min()
            }
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
            Some(EdgeSource::Clock(c)) => c.prev_edge(from),
            Some(EdgeSource::Group(hs)) => {
                hs.iter().filter_map(|h| h.prev_change_before(from)).max()
            }
            None => return,
        };
        if let Some(t) = prev {
            self.set_cursor(doc, Some(t));
            self.nav.reveal_cursor(doc, now);
        }
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
        let rulers = self.nav.clocks().rulers(&doc.clocks);
        let spans = SpanClocks::of(&rulers, self.nav.clocks(), &doc.clocks, doc.time_base());
        let visible = self.visible();
        let items = &self.items;
        let layout = WaveLayout::compute(LayoutInput {
            bounds,
            row_h: theme.row_height,
            header_h: theme.timeline_height,
            ruler_h: rulers.len() as f32 * super::overlay::RULER_H * theme.zoom,
            zoom: theme.zoom,
            names_width: self.names_width,
            values_width: self.values_width,
            trace_gutter: doc.traces().is_combined(),
            row_tops: super::layout::row_tops(visible.iter().map(|&i| items[i as usize].height())),
            visible,
            scroll_y: self.scroll_y,
            markers: doc.markers(),
            spans,
            measuring: doc
                .reference_time()
                .map(|reference| super::overlay::Measuring {
                    reference,
                    cursor: self.nav.cursor(doc),
                }),
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
    /// enough, then track the drop under it.
    fn update_row_drag(&mut self) {
        let (
            Some(Drag::Rows {
                press_y,
                started,
                collapse,
                scrolled_at,
                depth,
                ..
            }),
            Some(p),
        ) = (self.drag, self.pointer)
        else {
            return;
        };
        let started = started || (p.y - press_y).abs() >= ROW_DRAG_MIN_PX * self.layout.zoom;
        let (gap, depth, into) = match started.then(|| self.drop_at(p)).flatten() {
            Some((gap, depth, into)) => (Some(gap), depth, into),
            None => (None, depth, None),
        };
        self.drag = Some(Drag::Rows {
            press_y,
            started,
            gap,
            depth,
            into,
            collapse,
            scrolled_at,
        });
    }

    /// Where dragged rows would land under `p`: a visible gap, the depth
    /// there (chosen by the pointer's x among the valid depths), and the
    /// folded group the middle of whose row takes them. `None` when that
    /// drop would not change the tree.
    fn drop_at(&self, p: Point) -> Option<(usize, u8, Option<usize>)> {
        let layout = &self.layout;
        let visible = &layout.visible;
        let y = p.y.clamp(layout.names.top(), layout.names.bottom() - 1.0);
        if let Some(pos) = layout.row_at(y).filter(|&pos| pos < visible.len()) {
            let group = visible[pos] as usize;
            let (top, h) = (layout.row_y(pos), layout.row_height(pos));
            let f = (y - top) / h.max(1.0);
            if (0.25..0.75).contains(&f) && self.items[group].group().is_some_and(|g| g.collapsed) {
                let to = Place::into(&self.items, group);
                if tree::plan_move(&self.items, &self.selected, to).is_some() {
                    return Some((pos, to.depth, Some(group)));
                }
            }
        }
        let gap = self.gap_at(y);
        let depths = tree::gap_depths(&self.items, visible, gap);
        let indent = super::layout::INDENT * layout.zoom;
        let want = ((p.x - super::layout::indent_x(layout.name_left, 0, layout.zoom)) / indent)
            .floor()
            .max(0.0)
            .min(f32::from(tree::MAX_DEPTH)) as u8;
        let depth = want.clamp(*depths.start(), *depths.end());
        let to = Place::gap(&self.items, visible, gap, depth);
        tree::plan_move(&self.items, &self.selected, to).map(|_| (gap, depth, None))
    }

    /// Where a finished drag drops, from its gap, depth and target group.
    fn drag_place(&self, gap: usize, depth: u8, into: Option<usize>) -> Place {
        match into {
            Some(group) => Place::into(&self.items, group),
            None => Place::gap(&self.items, &self.layout.visible, gap, depth),
        }
    }

    /// The visible insertion gap nearest to `y`, which is clamped to the rows area.
    fn gap_at(&self, y: f32) -> usize {
        let layout = &self.layout;
        let len = layout.visible.len();
        let y = y.clamp(layout.names.top(), layout.names.bottom() - 1.0);
        match layout.row_at(y).filter(|row| *row < len) {
            Some(row) if y >= layout.row_y(row) + layout.row_height(row) / 2.0 => row + 1,
            Some(row) => row,
            None => len,
        }
    }

    /// Where the name editor of the group being renamed sits: over its name,
    /// after the chevron, to the end of the names column.
    pub fn rename_rect(&self) -> Option<crate::geometry::Rect> {
        let g = self.rename?;
        let (y, _) = self.layout.entry_span(g)?;
        let layout = &self.layout;
        let x = super::layout::indent_x(layout.name_left, self.items.get(g)?.depth, layout.zoom)
            + super::layout::CHEVRON_W * layout.zoom;
        let right = layout.names.right() - 4.0 * layout.zoom;
        (right > x && y + layout.row_h > layout.names.top() && y < layout.names.bottom())
            .then(|| crate::geometry::Rect::from_xywh(x, y, right - x, layout.row_h))
    }

    /// The group whose name (not its chevron) is under `p`: a double-click
    /// there renames it.
    pub fn group_name_at(&self, p: Point) -> Option<usize> {
        if !self.layout.names.contains(p) || self.chevron_at(p).is_some() {
            return None;
        }
        let i = self.layout.entry_at(p.y)?;
        self.items.get(i)?.is_group().then_some(i)
    }

    /// The rows on screen for assistive technology, as a tree: each with its
    /// level, its expanded state (groups only) and its selection.
    pub fn accessible_rows(&self) -> impl Iterator<Item = AccessibleRow> + '_ {
        let layout = &self.layout;
        layout.rows.clone().filter_map(move |pos| {
            let entry = layout.entry(pos)?;
            let e = &self.items[entry];
            let (label, expanded) = match &e.row {
                WaveRow::Group(g) => {
                    let n = tree::leaves(&self.items, entry).count();
                    (
                        format!("{}, {n} row{}", g.name, if n == 1 { "" } else { "s" }),
                        Some(!g.collapsed),
                    )
                }
                row => (row.name().to_owned(), None),
            };
            // With a trace gutter, a row says which trace it shows.
            let gutter = layout.name_left > layout.names.left();
            let label = match gutter.then(|| self.row_trace(entry)).flatten() {
                Some(trace) => format!("{label}, trace {trace}"),
                None => label,
            };
            Some(AccessibleRow {
                entry,
                label,
                level: usize::from(e.depth) + 1,
                expanded,
                selected: self.selected.contains(&entry),
                bounds: crate::geometry::Rect::from_xywh(
                    layout.names.left(),
                    layout.row_y(pos),
                    layout.names.width(),
                    layout.row_height(pos),
                ),
            })
        })
    }

    /// The group whose chevron is under `p`.
    fn chevron_at(&self, p: Point) -> Option<usize> {
        let layout = &self.layout;
        if !layout.names.contains(p) {
            return None;
        }
        let i = layout.entry_at(p.y)?;
        let e = self.items.get(i).filter(|e| e.is_group())?;
        let (y, _) = layout.entry_span(i)?;
        let x = super::layout::indent_x(layout.name_left, e.depth, layout.zoom);
        let w = super::layout::CHEVRON_W * layout.zoom;
        (p.x >= x - 4.0 * layout.zoom && p.x < x + w && p.y < y + layout.row_h).then_some(i)
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
    ) -> Option<(Traced<TrackRef>, crate::data::transactions::TransactionRef)> {
        let lane = self.items.get(row)?.lane()?;
        let generator = lane.generator(doc)?;
        let layout = &self.layout;
        let viewport = self.viewport(doc);
        let width = layout.wave_width_f64();
        let ppu = viewport.px_per_unit(width);
        if lane::is_density(generator, ppu) {
            return None;
        }
        let (y, h) = layout.entry_span(row)?;
        let geometry = LaneGeometry::new(y, h, layout.row_h, lane.height);
        let sub = geometry.sub_at(p.y)?;
        let time = viewport.time_at(f64::from(p.x - layout.waves.left()), width);
        let tolerance = 3.0 * f64::from(layout.zoom) / ppu;
        let ordinal = lane::hit(generator, lane.height, sub, time, tolerance)?;
        let tx = &generator.transactions()[ordinal];
        Some((Traced::new(lane.source.trace(), tx.generator), tx.id))
    }

    /// The entry whose name-cell bottom edge is under `p`.
    pub fn row_edge_at(&self, p: Point) -> Option<usize> {
        let layout = &self.layout;
        if !layout.names.contains(p) {
            return None;
        }
        let grab = ROW_EDGE_GRAB_PX * layout.zoom;
        layout
            .rows
            .clone()
            .filter(|&r| r < layout.visible.len())
            .map(|r| (r, (layout.row_y(r) + layout.row_height(r) - p.y).abs()))
            .filter(|&(r, d)| {
                d <= grab && layout.row_y(r) + layout.row_height(r) > layout.names.top() + grab
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .and_then(|(r, _)| layout.entry(r))
    }

    fn update_hover(&mut self) {
        self.edge_hover = self.pointer.and_then(|p| self.row_edge_at(p));
        let (hover_row, badge_hover) = match self.pointer {
            Some(p) if self.layout.bounds.contains(p) && p.y >= self.layout.names.top() => {
                let row = self.layout.entry_at(p.y);
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
            || self.lane_hover.is_some();
        self.hover_row = None;
        self.badge_hover = None;
        self.edge_hover = None;
        self.split_hover = false;
        self.lane_hover = None;
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
                self.pointer_down(doc, panel, position, button, modifiers, now);
                true
            }
            PointerEvent::Move { position } => self.pointer_move(doc, position),
            PointerEvent::Up => {
                let had = self.drag.is_some();
                if let Some(Drag::Marker(held)) = self.drag {
                    self.drag = None;
                    held.release(doc, &mut self.nav, now);
                } else if let Some(Drag::Pan {
                    click: Some(click), ..
                }) = self.drag
                {
                    self.drag = None;
                    self.measure_from(doc, click);
                } else if let Some(Drag::Rows {
                    gap,
                    depth,
                    into,
                    collapse,
                    ..
                }) = self.drag
                {
                    self.drag = None;
                    match (gap, collapse) {
                        (Some(gap), _) => {
                            let to = self.drag_place(gap, depth, into);
                            self.move_selected_to(to);
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

    /// Put the document's reference at `p` (Alt-click, a middle click),
    /// snapped like the cursor to the selected clock and the row under the
    /// pointer, and onto a marker there.
    fn measure_from(&mut self, doc: &mut Document, p: Point) {
        let layout = &self.layout;
        let wave_wf = layout.wave_width_f64();
        let x = f64::from(p.x - layout.waves.left()).clamp(0.0, wave_wf);
        let snap_px = doc.navigation.snap_px * f64::from(layout.zoom);
        let viewport = self.viewport(doc);
        let clock = self.nav.selected_clock(doc);
        let edges = layout
            .entry_at(p.y)
            .filter(|_| p.y >= layout.waves.top())
            .and_then(|r| self.edge_source(doc, r));
        let t = snapped_time(
            &viewport,
            edges.as_ref(),
            clock.as_deref(),
            x,
            wave_wf,
            snap_px,
        );
        let raw = viewport.time_at(x, wave_wf);
        let tolerance = snap_px / viewport.px_per_unit(wave_wf);
        let reference = crate::marker::reference_near(doc.markers(), raw, tolerance, t);
        doc.set_reference(Some(reference));
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
            match layout.marker_lane.hit(p) {
                Some(crate::marker::LaneHit::Chip(ix)) => {
                    let viewport = self.viewport(doc);
                    let marker_x = doc.markers().get(ix).map_or(p.x, |m| {
                        layout.waves.left()
                            + viewport.x_of(m.time as f64, layout.wave_width_f64()) as f32
                    });
                    self.drag =
                        crate::marker::MarkerDrag::begin(doc, ix, p, marker_x).map(Drag::Marker);
                    return;
                }
                Some(crate::marker::LaneHit::Span(_)) | None => {}
                Some(hit) => {
                    crate::marker::press(doc, &mut self.nav, hit, now);
                    return;
                }
            }
        }
        let in_waves_x = p.x >= layout.waves.left() && p.x < layout.waves.right();
        let wave_wf = layout.wave_width_f64();
        let snap_px = doc.navigation.snap_px * f64::from(layout.zoom);
        // Alt-click measures from the pointer; a middle click does on release.
        if in_waves_x && button == MouseButton::Left && modifiers.alt {
            self.measure_from(doc, p);
            return;
        }
        if in_waves_x && button == MouseButton::Middle {
            self.drag = Some(Drag::Pan {
                last_x: p.x,
                click: Some(p),
            });
            return;
        }
        if in_waves_x && button == MouseButton::Left && (modifiers.control || modifiers.platform) {
            let displayed = self.viewport(doc);
            self.nav.viewport_state_mut(doc).set(displayed);
            self.drag = Some(Drag::ZoomRange {
                start: p,
                current: p,
            });
            return;
        }
        // A press on a clock ruler selects its clock, then works like the header.
        let rulers: Vec<ClockKey> = self
            .nav
            .clocks()
            .rulers(&doc.clocks)
            .iter()
            .map(|c| c.key())
            .collect();
        let ruler = layout.ruler_at(p, rulers.len());
        if let Some(ix) = ruler {
            if button == MouseButton::Right {
                self.open_ruler_menu(doc, ix, p);
                return;
            }
            self.nav.select_clock(rulers[ix].clone());
        }
        // The time strip above the rows: header, rulers and the lane between chips.
        if layout.header.contains(p) || ruler.is_some() || layout.marker_lane.strip().contains(p) {
            if in_waves_x && button == MouseButton::Left {
                let x = f64::from(p.x - layout.waves.left());
                let clock = self.nav.selected_clock(doc);
                let t = snapped_time(
                    &self.viewport(doc),
                    None,
                    clock.as_deref(),
                    x,
                    wave_wf,
                    snap_px,
                );
                self.set_cursor(doc, Some(t));
                self.drag = Some(Drag::Cursor);
            }
            return;
        }
        let row = layout.entry_at(p.y);
        if in_waves_x {
            match button {
                MouseButton::Left => {
                    let x = f64::from(p.x - layout.waves.left());
                    let clock = self.nav.selected_clock(doc);
                    let t = snapped_time(
                        &self.viewport(doc),
                        row.and_then(|r| self.edge_source(doc, r)).as_ref(),
                        clock.as_deref(),
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
                    self.drag = Some(Drag::Pan {
                        last_x: p.x,
                        click: None,
                    });
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
        if let Some(row) = self.row_edge_at(p)
            && let Some((top, _)) = layout.entry_span(row)
        {
            self.drag = Some(Drag::RowHeight { row, top });
            return;
        }
        // The chevron folds or unfolds its group; Alt does the groups inside too.
        if let Some(g) = self.chevron_at(p) {
            let collapsed = self.items[g].group().is_some_and(|g| g.collapsed);
            self.set_folded(g, !collapsed, modifiers.alt);
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
                depth: 0,
                into: None,
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
                let row = layout.entry_at(p.y);
                let clock = self.nav.selected_clock(doc);
                let t = snapped_time(
                    &self.viewport(doc),
                    row.and_then(|r| self.edge_source(doc, r)).as_ref(),
                    clock.as_deref(),
                    x,
                    wave_wf,
                    doc.navigation.snap_px * f64::from(layout.zoom),
                );
                self.set_cursor(doc, Some(t));
                true
            }
            Some(Drag::Marker(mut held)) => {
                // Snapped like the cursor: to the selected clock and the row
                // under the pointer.
                let Some(x) = held.target_x(p, layout.zoom) else {
                    return false;
                };
                let row = layout.entry_at(p.y).filter(|_| p.y >= layout.waves.top());
                let clock = self.nav.selected_clock(doc);
                let t = snapped_time(
                    &self.viewport(doc),
                    row.and_then(|r| self.edge_source(doc, r)).as_ref(),
                    clock.as_deref(),
                    f64::from(x - layout.waves.left()).clamp(0.0, wave_wf),
                    wave_wf,
                    doc.navigation.snap_px * f64::from(layout.zoom),
                );
                self.drag = Some(Drag::Marker(held));
                doc.move_marker(held.id, t);
                true
            }
            Some(Drag::Pan { last_x, click }) => {
                // A middle press pans once it moves past the threshold.
                if let Some(c) = click
                    && (p.x - c.x).hypot(p.y - c.y) <= CLICK_SLOP_PX * layout.zoom
                {
                    return false;
                }
                let dx = last_x - p.x;
                self.drag = Some(Drag::Pan {
                    last_x: p.x,
                    click: None,
                });
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
                let (prev_row, prev_badge, prev_split, prev_lane, prev_edge) = (
                    self.hover_row,
                    self.badge_hover,
                    self.split_hover,
                    self.lane_hover.take(),
                    self.edge_hover,
                );
                if layout.bounds.contains(p) {
                    self.split_hover = layout.near_split(p);
                    self.lane_hover = layout.marker_lane.hit(p);
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
                    || self.lane_hover != prev_lane
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

/// Rows for `vars`, as [`WaveModel::add_vars`] adds them.
fn var_rows(doc: &mut Document, vars: &[Traced<VarId>], loaded: &Resident) -> Vec<WaveRow> {
    let mut rows = Vec::with_capacity(vars.len());
    for &var in vars {
        let Some(session) = doc.session(var.trace).cloned() else {
            continue;
        };
        let h = session.hierarchy();
        let Some(v) = h.get_var(var.item) else {
            continue;
        };
        let signal = var.with(v.signal);
        let translator = doc.translators.default_for(v.shape);
        // Variable identity/format stay per row; aliases share immutable data.
        let history = loaded.get(&signal).cloned();
        let needs_load = history.is_none();
        // Reals open as plots: their text is rarely readable at a glance.
        let analog = (v.shape == SignalShape::Real
            && analog::supports(v.shape, translator.as_ref()))
        .then(|| {
            let mut a = Analog::new(AnalogDraw::Linear, AnalogRange::Trace);
            a.restore_height = Some(RowHeight::DEFAULT);
            a
        });
        rows.push(WaveRow::Signal(DisplayedSignal {
            source: RowSource::Resolved {
                trace: var.trace,
                var: var.item,
                signal: v.signal,
            },
            requested_format: None,
            name: v.name.to_owned(),
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
            tint: None,
        }));
        if needs_load {
            doc.request_signal(signal);
        }
    }
    rows
}

/// Where a click on the waves at `x_px` lands after snapping to the nearest
/// edge within `snap_px` pixels (0 disables snapping): of the row (a signal
/// transition, a lane's record begin or end, a clock row's edge) or of the
/// panel's selected clock, whichever is closer.
fn snapped_time(
    vp: &Viewport,
    edges: Option<&EdgeSource<'_>>,
    clock: Option<&crate::clock::ClockTimeline>,
    x_px: f64,
    width_px: f64,
    snap_px: f64,
) -> u64 {
    let raw = vp.time_at(x_px, width_px).round().max(0.0);
    if snap_px <= 0.0 {
        return raw as u64;
    }
    let tol = snap_px / vp.px_per_unit(width_px);
    let exact = vp.time_at(x_px, width_px).max(0.0);
    let clock_edge = clock.and_then(|c| crate::clock::nearest_edge(c, exact, tol));
    let row_edge = edges.and_then(|e| row_edge(e, raw, exact, tol));
    [clock_edge, row_edge]
        .into_iter()
        .flatten()
        .min_by(|a, b| {
            (*a as f64 - exact)
                .abs()
                .total_cmp(&(*b as f64 - exact).abs())
        })
        .unwrap_or(raw as u64)
}

/// The edge of a row nearest to `raw` within `tol`.
fn row_edge(edges: &EdgeSource<'_>, raw: f64, exact: f64, tol: f64) -> Option<u64> {
    match edges {
        EdgeSource::History(h) => history_edge(h.as_ref(), raw, tol),
        EdgeSource::Lane(g) => lane::nearest_boundary(g, exact, tol),
        EdgeSource::Clock(c) => crate::clock::nearest_edge(c, exact, tol),
        EdgeSource::Group(hs) => hs
            .iter()
            .filter_map(|h| history_edge(h.as_ref(), raw, tol))
            .min_by(|a, b| (*a as f64 - raw).abs().total_cmp(&(*b as f64 - raw).abs())),
    }
}

/// The change of `h` nearest to `raw` within `tol`.
fn history_edge(h: &dyn SignalHistory, raw: f64, tol: f64) -> Option<u64> {
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
    best.map(|(_, c)| c)
}
