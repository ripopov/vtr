//! `App`: the whole viewer with no toolkit attached. Frontends feed it
//! [`Command`]s, drain [`Event`]s, perform the [`LoadRequest`]s it queues, and
//! ask it to lay out and paint the wave panel into a [`Scene`].

use std::sync::Arc;

use web_time::Instant;

use crate::data::Member;
use crate::data::transactions::{TrackRef, TransactionRef};
use crate::data::{ScopeId, VarId};
use crate::document::{Delivered, Document, TraceState};
use crate::geometry::{Modifiers, Rect};
use crate::panels::{Panel, PanelId, PanelKind, Panels, PanelsCommand};
use crate::pipeline::{PipelineLayout, PipelineModel, TrackSource};
use crate::scene::{Scene, TextCache, TextMeasure};
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};
use crate::settings::{self, Value};
use crate::sidebar::{Key, MemberListModel, ScopeTreeModel};
use crate::theme::Theme;
use crate::transaction::{TransactionCommand, TransactionModel};
use crate::wave::layout::WaveLayout;
use crate::wave::model::{MenuAction, PointerEvent, WaveMenuKind, WaveRow};
use crate::wave::timeline::{TimeBase, format_time};

/// Keyboard actions of the wave panel. Frontends bind keys to these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SplitRight,
    SplitDown,
    NewPanel,
    ClosePanel,
    FocusNextPanel,
    FocusPrevPanel,
    ToggleViewportLink,
    ToggleCursorLink,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomToCursor,
    PanPageLeft,
    PanPageRight,
    GoToStart,
    GoToEnd,
    GoToCursor,
    PanLeft,
    PanRight,
    NextEdge,
    PrevEdge,
    AddMarker,
    ClearMarkers,
    RemoveSelected,
    /// Copy the selected wave rows to the document clipboard; cut also
    /// removes them. Paste inserts copies below the selection, sharing data.
    CopySignals,
    CutSignals,
    PasteSignals,
    SelectAll,
    ClearSelection,
    CycleFormat,
    /// Draw the selected rows as plots, or back to digital.
    ToggleAnalog,
    /// Step the selected rows to the next larger / smaller height preset,
    /// or back to the default height.
    IncreaseRowHeight,
    DecreaseRowHeight,
    ResetRowHeight,
    MoveSelectionUp,
    MoveSelectionDown,
    /// `G`: put the selected wave rows under a new group and rename it.
    GroupSelection,
    /// `Shift+G`: dissolve the selected groups, keeping their rows.
    Ungroup,
    /// `F2`: rename the selected group.
    RenameGroup,
    /// `Alt+←` / `Alt+→`: fold or unfold the selected group and every group
    /// inside it. Plain `←` / `→` ([`Action::PanLeft`] / [`Action::PanRight`])
    /// fold and unfold a selected group, and pan otherwise.
    FoldGroupDeep,
    UnfoldGroupDeep,
    /// Step the cursor to the next / previous rising edge of the panel's
    /// selected clock (`]` / `[`).
    NextCycle,
    PrevCycle,
    /// Number every clock's cycles from the cursor's cycle, or from the
    /// first edge again when the origin is already there.
    ToggleCycleOrigin,
}

impl Command {
    /// Stable command names for hosts whose shortcuts run outside the viewer.
    /// Routing and the meaning of each command remain in the core.
    pub fn named(name: &str) -> Option<Self> {
        let action = match name {
            "openWorkspace" => return Some(Self::RequestOpenWorkspace),
            "saveWorkspace" => return Some(Self::SaveWorkspace),
            "saveWorkspaceAs" => return Some(Self::RequestSaveWorkspaceAs),
            "splitRight" => Action::SplitRight,
            "splitDown" => Action::SplitDown,
            "newPanel" => Action::NewPanel,
            "closePanel" => Action::ClosePanel,
            "focusNextPanel" => Action::FocusNextPanel,
            "focusPrevPanel" => Action::FocusPrevPanel,
            "toggleViewportLink" => Action::ToggleViewportLink,
            "toggleCursorLink" => Action::ToggleCursorLink,
            "nextCycle" => Action::NextCycle,
            "prevCycle" => Action::PrevCycle,
            "toggleCycleOrigin" => Action::ToggleCycleOrigin,
            _ => {
                let index = name.strip_prefix("focusPanel")?.parse::<usize>().ok()?;
                return (1..=9)
                    .contains(&index)
                    .then_some(Self::Panels(PanelsCommand::FocusIndex(index - 1)));
            }
        };
        Some(Self::Action(action))
    }
}

/// Which sash of the workspace chrome is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeDrag {
    Sidebar,
    ScopesSplit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Frontend-owned operations (clipboard, dialogs) report a concise error
    /// through the same accessible notice path as core failures.
    Notice(String),
    /// Start opening a trace.
    Open(OpenSpec),
    /// Ask the frontend for a file (it answers with [`Command::Open`]).
    RequestOpenDialog,
    RequestOpenWorkspace,
    RequestSaveWorkspaceAs,
    SaveWorkspace,
    RequestQuit,
    CloseTrace,
    ToggleSidebar,
    Action(Action),
    Pointer(PanelId, PointerEvent),
    Panels(PanelsCommand),
    /// Add rows for these variables to the wave view.
    AddVars(Vec<VarId>),
    /// Add members to the wave view: variables as signal rows, generators as
    /// transaction lanes, clock streams and generators as clock rows. Other
    /// streams and log sites have no row form.
    AddToWaves(Vec<Member>),
    /// Show the clocks of these members (clock streams or their generators;
    /// other members are ignored) as rulers of the panel `AddToWaves` targets.
    AddClockRulers(Vec<Member>),
    ActivateMembers(Vec<Member>),
    /// Show a stream or generator as a pipeline panel: focus the panel that
    /// already shows it, or open one below the focused panel.
    OpenPipeline {
        track: TrackRef,
    },
    /// Capture one generator or an all-signal selection and open the reduced
    /// immutable table view below the invoking content panel.
    OpenTable {
        selected: Vec<Member>,
        clicked: Option<Member>,
    },
    /// Open from a Waves selection or a single-generator Pipeline panel.
    OpenTableFromPanel {
        panel: PanelId,
        row: Option<usize>,
    },
    Table(PanelId, crate::table::TableCommand),
    /// Make one record the document selection, optionally moving the
    /// originating panel's cursor to the time under the pointer.
    SelectTransaction {
        panel: PanelId,
        track: TrackRef,
        id: TransactionRef,
        cursor: Option<u64>,
    },
    /// Show the selection in a Transaction panel: focus the first one that is
    /// not pinned, or open one beside the panel that asked.
    ShowTransaction {
        from: PanelId,
    },
    Transaction(PanelId, TransactionCommand),
    /// The focused panel's clocks: rulers, the selected clock and go-to-cycle.
    Clocks(ClockCommand),
    /// Scroll the pipeline showing this panel's record to its row and fit the
    /// time axis to its lifetime, or open that track as a pipeline.
    RevealTransaction {
        panel: PanelId,
    },
    PipelineActivity(PanelId, crate::pipeline::ActivityCommand),
    SetSearchEverywhere(bool),
    SelectScope(ScopeId),
    ToggleScope(ScopeId),
    ExpandAllScopes(bool),
    ScopesKey(Key),
    SetFilter(String),
    SelectVar {
        ix: usize,
        modifiers: Modifiers,
    },
    /// The `+` button: add every listed variable.
    AddAllVars,
    /// Enter in the filter box: add the selected variables, or all.
    AddSelectedOrAllVars,
    VariablesKey(Key, Modifiers),
    /// Add a scope's variables to the wave view as a group named after it;
    /// with `recursive`, child scopes with variables become folded subgroups.
    AddScopeAsGroup {
        scope: ScopeId,
        recursive: bool,
    },
    /// The frontend's name editor finished: `Some` renames the group being
    /// renamed, `None` cancels.
    RenameGroup(PanelId, Option<String>),
    /// Open the selected signal's row menu, or report a wave-row menu choice.
    OpenSignalMenu(PanelId),
    MenuSelect(PanelId, MenuAction),
    MenuDismiss(PanelId),
    SetSidebarWidth(f32),
    /// Scope tree height as a fraction of the sidebar.
    SetScopesFraction(f32),
    ChromeDragStart(ChromeDrag),
    ChromeDragEnd,
    Settings(SettingsCommand),
}

/// The focused timed panel's clock choices, for menus and the palette.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClockChoices {
    /// Every clock of the trace: its path and whether it is shown as a ruler.
    pub clocks: Vec<(String, bool)>,
    /// Cycles are numbered from a chosen origin.
    pub origin: bool,
}

/// What a timed panel shows of the trace's clocks (`docs/vtr_clocks.html`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClockCommand {
    /// Show or hide the ruler of the clock with this path.
    ToggleRuler(String),
    /// The clock clicks snap to and `[` / `]` step through.
    Select(String),
    /// Put the cursor on this cycle of the panel's selected clock, as the
    /// panel numbers its cycles.
    GoToCycle(i64),
}

/// The settings editor. GUI edits become surgical text edits in the core;
/// the query and JSON toggle live here so the command palette can drive them.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsCommand {
    /// Open the Settings tab, or focus its search box when it is open.
    Open,
    Close,
    Set {
        id: String,
        value: Value,
    },
    Reset {
        id: String,
    },
    /// The JSON view's save: replace the whole document text.
    ReplaceText(String),
    Query(String),
    ToggleJson,
    /// Open the tab filtered to one setting (`@id:`), from the palette.
    Reveal {
        id: String,
    },
    /// Step `appearance.zoom` (⌘= / ⌘- / ⌘0); the new value is written to
    /// the settings file like any other change.
    Zoom(settings::ZoomStep),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Something visible changed; repaint.
    Changed,
    LayoutChanged {
        revision: u64,
    },
    Notice(String),
    LoadWorkspace {
        trace_uri: String,
    },
    PersistWorkspace {
        ticket: crate::workspace::persistence::SaveTicket,
        bytes: Vec<u8>,
    },
    OpenWorkspaceDialog,
    SaveWorkspaceDialog,
    TraceClosed {
        trace_uri: String,
    },
    Quit,
    /// Show the platform file dialog, then send [`Command::Open`].
    OpenFileDialog,
    /// Scroll the scope tree so this row is visible.
    RevealScopeRow(usize),
    /// Scroll the variable list so this row is visible.
    RevealVarRow(usize),
    /// Typing in the variable list: move keyboard focus to the filter box.
    FocusFilter,
    /// Write `settings.json`; acknowledge through [`App::settings_saved`].
    WriteSettings {
        ticket: u64,
        bytes: Vec<u8>,
    },
    /// Resolved settings changed for these ids (registry order is not implied).
    SettingsChanged {
        keys: Vec<&'static str>,
    },
    /// Move keyboard focus to the settings search box.
    FocusSettingsSearch,
}

/// Transient state of the settings editor, owned by the core.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub query: String,
    pub json: bool,
}

/// Text the status bar shows, already formatted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub sidebar_notice: Option<String>,
    pub workspace_notice: Option<String>,
    pub panel: Option<String>,
    /// What the pointer is over in the focused pipeline panel.
    pub hover: Option<String>,
    pub links: Option<crate::nav::Link>,
    pub file: Option<String>,
    pub time_range: Option<String>,
    pub signals: Option<String>,
    pub changes: Option<String>,
    pub px_per: Option<String>,
    pub cursor: Option<String>,
    /// The cursor's cycle in each clock ruler of the focused panel, e.g.
    /// `core_clk 150231 + 0.42`.
    pub clocks: Vec<String>,
    /// Cursor minus the nearest marker, in time and in whole cycles of each
    /// ruler clock: `Δ 20 ns · 40 core_clk · 10 bus_clk`.
    pub delta: Option<String>,
    pub markers: Option<String>,
    pub frame_ms: String,
    /// Decoded trace data against the open trace's memory budget.
    pub memory: Option<MemoryStatus>,
}

/// How much of the client memory budget loaded trace data holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryStatus {
    pub used: u64,
    pub limit: u64,
}

impl MemoryStatus {
    /// From this fraction on, the indicator warns that loads may fail.
    pub const NEARLY_FULL: f32 = 0.9;

    pub fn fraction(self) -> f32 {
        if self.limit == 0 {
            return 1.0;
        }
        (self.used as f64 / self.limit as f64).clamp(0.0, 1.0) as f32
    }

    pub fn nearly_full(self) -> bool {
        self.fraction() >= Self::NEARLY_FULL
    }

    /// Compact "used / limit" in the limit's unit, e.g. `412 / 512 MiB`.
    pub fn label(self) -> String {
        let (unit, scale) = byte_unit(self.limit);
        format!(
            "{} / {} {unit}",
            short_amount(self.used as f64 / scale),
            short_amount(self.limit as f64 / scale)
        )
    }

    /// The indicator's tooltip.
    pub fn detail(self) -> String {
        let (unit, scale) = byte_unit(self.limit);
        format!(
            "Memory budget: {:.1} of {} {unit} used ({:.0}%) by loaded trace data. \
             Loads that would exceed the budget fail. Click to change the limits.",
            self.used as f64 / scale,
            short_amount(self.limit as f64 / scale),
            f64::from(self.fraction()) * 100.0
        )
    }
}

/// One preset in the status bar's memory menu: choosing it sets `id` to `mib`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryChoice {
    pub id: &'static str,
    pub mib: u64,
    pub label: String,
    pub checked: bool,
}

impl MemoryChoice {
    pub fn command(&self) -> Command {
        Command::Settings(SettingsCommand::Set {
            id: self.id.into(),
            value: settings::Value::Integer(self.mib as i64),
        })
    }
}

/// The memory menu the status bar meter opens: presets for both limits. A
/// value the host owns (VS Code's `volna.memory.*`) is shown but not editable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryMenu {
    pub budget_title: String,
    pub budget: Vec<MemoryChoice>,
    pub object_title: String,
    pub object: Vec<MemoryChoice>,
    /// Shown under the object presets.
    pub object_note: Option<&'static str>,
    /// Why the presets are disabled, when the host owns the values.
    pub locked: Option<&'static str>,
}

pub const BUDGET_PRESETS_MIB: [u64; 8] = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768];
pub const OBJECT_PRESETS_MIB: [u64; 7] = [64, 128, 256, 512, 1024, 2048, 4096];

/// `512 MiB`, `2 GiB`, `1.5 GiB`.
pub fn format_mib(mib: u64) -> String {
    if mib >= 1024 {
        format!("{} GiB", short_amount(mib as f64 / 1024.0))
    } else {
        format!("{mib} MiB")
    }
}

fn memory_choices(id: &'static str, presets: &[u64], current: u64) -> Vec<MemoryChoice> {
    let mut values = presets.to_vec();
    // A hand-written value keeps its place and its check mark.
    if !values.contains(&current) {
        values.push(current);
        values.sort_unstable();
    }
    values
        .into_iter()
        .map(|mib| MemoryChoice {
            id,
            mib,
            label: format_mib(mib),
            checked: mib == current,
        })
        .collect()
}

fn byte_unit(bytes: u64) -> (&'static str, f64) {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= 1024.0 * MIB {
        ("GiB", 1024.0 * MIB)
    } else {
        ("MiB", MIB)
    }
}

/// One decimal below 10 (`0.3`, `1.5`), whole numbers above; no trailing `.0`.
fn short_amount(value: f64) -> String {
    let text = if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

pub const SIDEBAR_FRACTION_MIN: f32 = 0.15;

/// What a start panel shows about the open trace: enough to choose the
/// first view without a waveform panel being assumed.
#[derive(Clone, Debug, PartialEq)]
pub struct StartSummary {
    pub name: String,
    /// The trace extent as the status bar prints it.
    pub time_range: String,
    pub variables: usize,
    /// Streams and generators of the transaction catalog.
    pub tracks: usize,
    /// The recognized PIPELINE streams: (dotted path, track).
    pub pipelines: Vec<(String, TrackRef)>,
}

/// The rectangles a frontend needs for hit regions, by panel kind.
pub enum PanelLayout<'a> {
    Waves(&'a WaveLayout),
    Pipeline(&'a PipelineLayout),
    Table(&'a crate::table::TableLayout),
}

pub struct App {
    pub doc: Document,
    pub workspace: crate::workspace::State,
    pub panels: Panels,
    pub scopes: ScopeTreeModel,
    pub variables: MemberListModel,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    /// Height of the scope tree as a fraction of the sidebar.
    pub scopes_fraction: f32,
    pub drag: Option<ChromeDrag>,
    pub settings: settings::Store,
    pub settings_view: SettingsView,
    show_all_on_open: bool,
    pub(crate) events: Vec<Event>,
    text: TextCache,
    scene: Scene,
    table_budget: crate::remote::memory::MemoryBudget,
    /// Generators retained for wave lanes, one document retain each, for
    /// the document generation that granted them.
    lane_tracks: (u64, std::collections::HashSet<TrackRef>),
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        App {
            doc: Document::new(),
            workspace: crate::workspace::State::default(),
            panels: Panels::new(),
            scopes: ScopeTreeModel::default(),
            variables: MemberListModel::default(),
            sidebar_width: 280.0,
            sidebar_visible: true,
            scopes_fraction: 0.42,
            drag: None,
            settings: settings::Store::new(settings::Host::Native),
            settings_view: SettingsView::default(),
            show_all_on_open: false,
            events: Vec::new(),
            text: TextCache::default(),
            scene: Scene::default(),
            table_budget: {
                let (limit, object_limit) = settings::Settings::default()
                    .limits()
                    .bytes()
                    .expect("valid defaults");
                let budget = crate::remote::memory::MemoryBudget::new(limit);
                budget.set_object_limit(object_limit);
                budget
            },
            lane_tracks: Default::default(),
        }
    }

    // -- opening -------------------------------------------------------------------

    /// Open a VTR file from disk (native).
    #[cfg(not(target_family = "wasm"))]
    pub fn open_path(&mut self, path: std::path::PathBuf) {
        self.open(OpenSpec::Path(path), false);
    }

    /// Open a VTR image held in memory (web hosts and drag-drop).
    pub fn open_bytes(&mut self, name: String, bytes: Vec<u8>) {
        self.open(OpenSpec::Bytes { name, bytes }, false);
    }

    /// Open the synthetic stress trace and show all of its signals.
    pub fn open_synthetic(&mut self, transitions: usize) {
        self.open(OpenSpec::Synthetic(transitions), true);
    }

    pub(crate) fn open_now(&mut self, spec: OpenSpec, show_all: bool) {
        self.show_all_on_open = show_all;
        self.doc.open(spec);
        self.changed();
    }

    /// Replace the session immediately (tests and hosts that hold one).
    pub fn set_session(&mut self, session: Arc<dyn Session>) {
        let session = if session.memory_budget().is_none() {
            match self.account_local_session(session) {
                Ok(session) => session,
                Err(error) => {
                    self.events.push(Event::Notice(format!(
                        "Trace exceeds the memory budget: {error:#}"
                    )));
                    return;
                }
            }
        } else {
            session
        };
        self.doc.set_session(session);
        self.on_session_changed();
    }

    pub(crate) fn close_now(&mut self) {
        self.doc.close();
        self.on_session_changed();
    }

    fn on_session_changed(&mut self) {
        if let Err(e) = self.panels.reset() {
            self.events.push(Event::Notice(e.to_string()));
        }
        self.layout_changed();
        self.scopes.reset(self.doc.hierarchy());
        self.variables.reset(self.doc.hierarchy());
        let scope = self.scopes.selected;
        self.variables.set_scope(self.doc.hierarchy(), scope);
        self.changed();
    }

    pub(crate) fn workspace_restored(&mut self) {
        self.sync_lane_tracks();
        self.sync_analog_summaries();
        self.drag = None;
        self.layout_changed();
        self.changed();
    }

    // -- the pull-based load loop --------------------------------------------------

    /// Loads the frontend should perform, then hand to [`App::deliver`].
    pub fn take_requests(&mut self) -> Vec<LoadRequest> {
        let mut requests = self.doc.take_requests();
        if requests
            .iter()
            .any(|r| matches!(r, LoadRequest::Signals { .. }))
        {
            let wanted = self.signal_demand();
            requests.retain_mut(|request| {
                if let LoadRequest::Signals {
                    generation,
                    signals,
                    ..
                } = request
                {
                    self.doc
                        .retain_queued_signals(*generation, signals, &wanted)
                } else {
                    true
                }
            });
        }
        requests
    }

    pub(crate) fn signal_demand(&self) -> std::collections::HashSet<crate::data::SignalRef> {
        self.panels
            .iter()
            .filter_map(|panel| panel.kind.waves())
            .flat_map(|waves| waves.items.iter().map(|e| &e.row))
            .filter_map(WaveRow::signal_ref)
            .chain(
                self.panels
                    .iter()
                    .filter_map(|panel| panel.kind.table())
                    .flat_map(|table| table.signal_demand()),
            )
            .collect()
    }

    pub fn deliver(&mut self, mut result: LoadResult) {
        if let LoadResult::Opened { result: opened, .. } = &mut result
            && opened
                .as_ref()
                .is_ok_and(|session| session.memory_budget().is_none())
        {
            let session = opened.as_ref().expect("checked successful open").clone();
            *opened = self.account_local_session(session);
        }
        match self.doc.deliver(result) {
            Some(Delivered::Summary) => self.changed(),
            Some(Delivered::Track) => {
                for pipeline in self.panels.pipelines_mut() {
                    pipeline.refresh(&self.doc);
                }
                let Self { panels, doc, .. } = self;
                for waves in panels.iter_mut().filter_map(|p| p.kind.waves_mut()) {
                    waves.fit_lanes(doc);
                }
                for table in self.panels.iter_mut().filter_map(|p| p.kind.table_mut()) {
                    table.refresh(&self.doc);
                }
                self.changed();
            }
            Some(Delivered::Signals(results)) => {
                for (signal, result) in results {
                    let result = result.map_err(|e| e.to_string());
                    for panel in self.panels.iter_mut() {
                        if let Some(w) = panel.kind.waves_mut() {
                            w.finish_signal(signal, result.clone().map_err(anyhow::Error::msg));
                        }
                        if let Some(table) = panel.kind.table_mut() {
                            table.finish_signal(signal, result.clone());
                            table.refresh(&self.doc);
                        }
                    }
                }
                self.sync_analog_summaries();
                self.changed();
            }
            Some(Delivered::Opened(Ok(session))) => {
                self.on_session_changed();
                if self.show_all_on_open {
                    let count = session.hierarchy().vars.len();
                    self.add_vars(&(0..count).collect::<Vec<_>>());
                }
                self.session_ready_for_workspace();
            }
            Some(Delivered::Opened(Err(_))) => {
                self.workspace.loading = false;
                self.changed();
            }
            None => {}
        }
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn changed(&mut self) {
        if !self.events.contains(&Event::Changed) {
            self.events.push(Event::Changed);
        }
    }

    /// Frontends mirror the current layout, so one pending event with the
    /// latest revision stands for every edit since the last drain.
    pub(crate) fn layout_changed(&mut self) {
        self.events
            .retain(|e| !matches!(e, Event::LayoutChanged { .. }));
        self.events.push(Event::LayoutChanged {
            revision: self.panels.revision(),
        });
        self.changed();
    }

    /// A created panel's content may retain document data; a removed
    /// panel's content releases it.
    fn created(&mut self, id: PanelId) {
        let resident = self.resident_histories();
        if let Some(table) = self.panels.get_mut(id).and_then(|p| p.kind.table_mut())
            && let Err(error) = table.attach(&mut self.doc, &resident)
        {
            table.state = crate::table::TableState::Failed(error.to_string());
        }
        if let Some(pipeline) = self.panels.pipeline_mut(id)
            && let Err(error) = pipeline.attach(&mut self.doc)
        {
            self.events.push(Event::Notice(error.to_string()));
        }
        if let Some(model) = self.panels.transaction_mut(id)
            && let Err(error) = model.attach(&mut self.doc)
        {
            self.events.push(Event::Notice(error.to_string()));
        }
    }

    fn removed(&mut self, panels: Vec<Panel>) {
        for mut panel in panels {
            if let Some(table) = panel.kind.table_mut() {
                table.detach(&mut self.doc);
            }
            if let Some(pipeline) = panel.kind.pipeline_mut() {
                pipeline.detach(&mut self.doc);
            }
            if let Some(model) = panel.kind.transaction_mut() {
                model.detach(&mut self.doc);
            }
        }
    }

    /// An empty waveform panel fitted to the trace, linked per the settings.
    fn fresh_waves(&self) -> PanelKind {
        let mut waves = crate::wave::model::WaveModel::new();
        waves.reset(self.doc.session().map(|s| s.info().time_range));
        waves.nav.link = self.settings.resolved().link_by_default();
        PanelKind::Waves(Box::new(waves))
    }

    /// Put `kind` where the panel `id` is (the start panel giving way to
    /// content). Returns the new panel's ID.
    fn replace_panel(&mut self, id: PanelId, kind: PanelKind) -> Result<PanelId, String> {
        match self.panels.replace(id, kind) {
            Ok((new, removed)) => {
                self.removed(removed);
                self.created(new);
                self.layout_changed();
                Ok(new)
            }
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
                Err(error.to_string())
            }
        }
    }

    /// The waveform panel new rows go to: the focused one, the start panel
    /// turned into one, the first one in layout order (focused), a start
    /// panel elsewhere turned into one, or a new tab beside the focused panel.
    fn waves_target(&mut self) -> Option<PanelId> {
        let focused = self.panels.focused_id();
        let kind = &self.panels.focused().kind;
        if kind.waves().is_some() {
            return Some(focused);
        }
        if kind.is_start() {
            let waves = self.fresh_waves();
            return self.replace_panel(focused, waves).ok();
        }
        if let Some(id) = self.panels.first_waves() {
            self.panel_command(PanelsCommand::Focus(id));
            return Some(id);
        }
        if let Some(start) = self.panels.first_start() {
            let waves = self.fresh_waves();
            let id = self.replace_panel(start, waves).ok()?;
            self.panel_command(PanelsCommand::Focus(id));
            return Some(id);
        }
        let waves = self.fresh_waves();
        match self.panels.open(waves, focused, None) {
            Ok(id) => {
                self.layout_changed();
                Some(id)
            }
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
                None
            }
        }
    }

    fn panel_command(&mut self, command: PanelsCommand) {
        let revision = self.panels.revision();
        let old_focus = self.panels.focused_id();
        let start =
            |panels: &Panels, id: PanelId| panels.get(id).is_some_and(|p| p.kind.is_start());
        let result = match command {
            // Splitting or tabbing a start panel asks for a waveform panel
            // where the placeholder is; nothing is left to put beside it.
            PanelsCommand::Split { panel, .. } | PanelsCommand::NewTab { group_of: panel }
                if start(&self.panels, panel) =>
            {
                let waves = self.fresh_waves();
                _ = self.replace_panel(panel, waves);
                return;
            }
            PanelsCommand::Split { panel, axis } => self
                .panels
                .create(panel, Some(axis))
                .map(|id| self.created(id)),
            PanelsCommand::NewTab { group_of } => self
                .panels
                .create(group_of, None)
                .map(|id| self.created(id)),
            PanelsCommand::Close(id) => self.panels.close(id).map(|removed| self.removed(removed)),
            PanelsCommand::CloseOthers(id) => self
                .panels
                .close_others(id)
                .map(|removed| self.removed(removed)),
            PanelsCommand::Focus(id) => self.panels.focus(id).map(|_| ()),
            PanelsCommand::FocusNext => self.panels.focus_next(false).map(|_| ()),
            PanelsCommand::FocusPrev => self.panels.focus_next(true).map(|_| ()),
            PanelsCommand::FocusIndex(ix) => self.panels.focus_index(ix).map(|_| ()),
            PanelsCommand::Rename(id, title) => self.panels.rename(id, title).map(|_| ()),
            PanelsCommand::SetLayout {
                layout,
                from_revision,
            } => self.panels.set_layout(layout, from_revision).map(|_| ()),
            PanelsCommand::ToggleLink { panel, dim } => {
                self.panels.toggle_link(panel, &self.doc, dim)
            }
        };
        if old_focus != self.panels.focused_id()
            && let Some(w) = self.panels.waves_mut(old_focus)
        {
            w.menu_dismiss();
        }
        if let Err(e) = result {
            self.events.push(Event::Notice(e.to_string()));
            // A rejected dock proposal must be resynchronized too.
            self.layout_changed();
        } else if revision != self.panels.revision() {
            self.layout_changed();
        } else {
            self.changed();
        }
    }

    // -- commands ----------------------------------------------------------------

    /// Delayed input from a panel or dialog belongs to the document generation
    /// that produced it, even when a restored file reuses saved panel IDs.
    pub fn handle_if_current(&mut self, generation: u64, command: Command) -> bool {
        if generation != self.doc.generation() {
            return false;
        }
        self.handle(command);
        true
    }

    pub fn handle(&mut self, command: Command) {
        self.handle_at(command, Instant::now());
    }

    /// Like [`App::handle`] with an explicit clock for animations.
    pub fn handle_at(&mut self, command: Command, now: Instant) {
        let before = crate::workspace::Stamp::capture(self, &command);
        let tracked = before.as_ref().map(|_| command.clone());
        let selection = self.doc.selection();
        let pointer = matches!(command, Command::Pointer(..));
        let press = matches!(
            command,
            Command::Pointer(_, PointerEvent::Down { .. } | PointerEvent::Up)
        );
        match command {
            Command::Notice(message) => {
                self.events.push(Event::Notice(message));
                self.changed();
            }
            Command::Open(spec) => self.open(spec, false),
            Command::RequestOpenDialog => self.events.push(Event::OpenFileDialog),
            Command::RequestOpenWorkspace | Command::RequestSaveWorkspaceAs => {
                if self.workspace.scheduler.enabled() {
                    self.events
                        .push(if matches!(command, Command::RequestOpenWorkspace) {
                            Event::OpenWorkspaceDialog
                        } else {
                            Event::SaveWorkspaceDialog
                        });
                } else {
                    self.report_workspace_error(
                        "Workspace persistence is disabled for this session".into(),
                    );
                }
            }
            Command::SaveWorkspace => self.save_workspace(None),
            Command::RequestQuit => self.request_quit(),
            Command::CloseTrace => self.close_trace(),
            Command::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.changed();
            }
            Command::Action(a) => self.action(a, now),
            Command::Panels(command) => self.panel_command(command),
            Command::Pointer(id, ev) => {
                if matches!(ev, PointerEvent::Down { .. }) && self.panels.get(id).is_some() {
                    self.panel_command(PanelsCommand::Focus(id));
                }
                if let Some(panel) = self.panels.get_mut(id)
                    && panel.pointer(&mut self.doc, ev, now)
                {
                    self.changed();
                }
            }
            Command::AddVars(vars) => self.add_vars(&vars),
            Command::AddToWaves(members) => self.add_to_waves(&members),
            Command::AddClockRulers(members) => self.add_clock_rulers(&members),
            Command::ActivateMembers(members) => self.activate_members(&members),
            Command::OpenPipeline { track } => self.open_pipeline(track),
            Command::OpenTable { selected, clicked } => self.open_table(&selected, clicked),
            Command::OpenTableFromPanel { panel, row } => self.open_table_from_panel(panel, row),
            Command::PipelineActivity(panel, command) => {
                if let Some(pipeline) = self.panels.pipeline_mut(panel) {
                    pipeline.activity_command(&self.doc, command);
                    self.changed();
                }
            }
            Command::Table(panel, command) => {
                let Self { panels, doc, .. } = self;
                if let Some(table) = panels.get_mut(panel).and_then(|p| p.kind.table_mut())
                    && table.command(doc, panel, command, now)
                {
                    self.changed();
                }
            }
            Command::SelectTransaction {
                panel,
                track,
                id,
                cursor,
            } => self.select_transaction(panel, track, id, cursor),
            Command::ShowTransaction { from } => self.show_transaction(from),
            Command::Clocks(command) => self.clock_command(command),
            Command::Transaction(panel, command) => {
                let Self { panels, doc, .. } = self;
                if let Some(model) = panels.transaction_mut(panel)
                    && model.command(doc, panel, command, now)
                {
                    self.changed();
                }
            }
            Command::RevealTransaction { panel } => self.reveal_transaction(panel, now),
            Command::SetSearchEverywhere(search) => {
                self.variables.search_everywhere = search;
                self.variables.rebuild(self.doc.hierarchy());
                self.events.push(Event::FocusFilter);
                self.changed();
            }
            Command::SelectScope(id) => {
                if self.scopes.select(id) || self.variables.search_everywhere {
                    self.variables.set_scope(self.doc.hierarchy(), Some(id));
                }
                self.changed();
            }
            Command::ToggleScope(id) => {
                if let Some(h) = self.doc.hierarchy() {
                    self.scopes.toggle(h, id);
                }
                self.changed();
            }
            Command::ExpandAllScopes(expand) => {
                if let Some(h) = self.doc.hierarchy() {
                    self.scopes.set_all(h, expand);
                }
                self.changed();
            }
            Command::ScopesKey(key) => {
                let Some(h) = self.doc.hierarchy() else {
                    return;
                };
                let out = self.scopes.key(h, &key);
                if out.changed {
                    let scope = self.scopes.selected;
                    self.variables.set_scope(self.doc.hierarchy(), scope);
                }
                if let Some(row) = out.reveal {
                    self.events.push(Event::RevealScopeRow(row));
                }
                if let Some(member) = out.activate {
                    self.activate_members(&[member]);
                }
                self.changed();
            }
            Command::SetFilter(text) => {
                self.variables.set_filter(self.doc.hierarchy(), &text);
                self.changed();
            }
            Command::SelectVar { ix, modifiers } => {
                self.variables.select(ix, modifiers);
                self.changed();
            }
            Command::AddAllVars => {
                let vars = self.variables.listed_vars();
                self.add_vars(&vars);
            }
            Command::AddSelectedOrAllVars => {
                let vars = self.variables.selected_or_all();
                self.activate_members(&vars);
            }
            Command::VariablesKey(key, modifiers) => {
                if key == Key::Escape && self.variables.selected.is_empty() {
                    self.variables.set_filter(self.doc.hierarchy(), "");
                    self.changed();
                }
                let out = self.variables.key(&key, modifiers);
                if let Some(members) = out.activate {
                    self.activate_members(&members);
                }
                if let Some(row) = out.reveal {
                    self.events.push(Event::RevealVarRow(row));
                }
                if out.focus_filter {
                    self.events.push(Event::FocusFilter);
                }
                if out.changed {
                    self.changed();
                }
            }
            Command::AddScopeAsGroup { scope, recursive } => self.add_scope_group(scope, recursive),
            Command::RenameGroup(panel, name) => {
                if let Some(waves) = self.panels.waves_mut(panel)
                    && waves.rename.is_some()
                {
                    waves.finish_rename(name.as_deref());
                    self.changed();
                }
            }
            Command::OpenSignalMenu(panel) => {
                if let Some(waves) = self.panels.waves_mut(panel) {
                    waves.open_selected_signal_menu(&self.doc);
                    if waves.menu.is_some() {
                        self.changed();
                    }
                }
            }
            Command::MenuSelect(panel, action) => {
                if matches!(action, MenuAction::OpenTable) {
                    let row = self
                        .panels
                        .waves(panel)
                        .and_then(|waves| waves.menu.as_ref())
                        .filter(|menu| menu.kind == WaveMenuKind::Signal)
                        .map(|menu| menu.row);
                    let Some(row) = row else { return };
                    self.open_table_from_panel(panel, Some(row));
                    if let Some(waves) = self.panels.waves_mut(panel) {
                        waves.menu_dismiss();
                    }
                    self.changed();
                    return;
                }
                if let Some(clipboard) = match action {
                    MenuAction::CopySignals => Some(Action::CopySignals),
                    MenuAction::CutSignals => Some(Action::CutSignals),
                    MenuAction::PasteSignals => Some(Action::PasteSignals),
                    _ => None,
                } {
                    let Some(waves) = self.panels.waves_mut(panel).filter(|w| {
                        w.menu
                            .as_ref()
                            .is_some_and(|m| m.kind == WaveMenuKind::Signal)
                    }) else {
                        return;
                    };
                    waves.menu_dismiss();
                    match clipboard {
                        Action::CopySignals => waves.copy_selected(&mut self.doc),
                        Action::CutSignals => waves.cut_selected(&mut self.doc),
                        _ => self.paste_signals(panel),
                    }
                    self.changed();
                    return;
                }
                if matches!(action, MenuAction::RemoveSignals) {
                    if let Some(waves) = self.panels.waves_mut(panel)
                        && waves
                            .menu
                            .as_ref()
                            .is_some_and(|menu| menu.kind == WaveMenuKind::Signal)
                    {
                        waves.menu_dismiss();
                        waves.remove_selected();
                        self.changed();
                    }
                    return;
                }
                let retry = self
                    .panels
                    .waves_mut(panel)
                    .and_then(|w| w.menu_select(&self.doc, &action));
                if let Some(signal) = retry
                    && self.doc.request_signal(signal)
                {
                    for panel in self.panels.iter_mut() {
                        if let Some(waves) = panel.kind.waves_mut() {
                            for row in waves.items.iter_mut().filter_map(|e| e.row.signal_mut()) {
                                if row.source.signal() == Some(signal) {
                                    row.error = None;
                                }
                            }
                        }
                    }
                }
                self.changed();
            }
            Command::MenuDismiss(panel) => {
                if let Some(w) = self.panels.waves_mut(panel) {
                    w.menu_dismiss();
                }
                self.changed();
            }
            Command::SetSidebarWidth(w) => {
                self.sidebar_width = w.clamp(160.0, 640.0);
                self.changed();
            }
            Command::SetScopesFraction(f) => {
                self.scopes_fraction = f.clamp(SIDEBAR_FRACTION_MIN, 1.0 - SIDEBAR_FRACTION_MIN);
                self.changed();
            }
            Command::ChromeDragStart(d) => {
                self.drag = Some(d);
                self.changed();
            }
            Command::ChromeDragEnd => {
                self.drag = None;
                self.changed();
            }
            Command::Settings(command) => self.settings_command(command, now),
        }
        // Pointer input moves rows but never adds or removes them; a press
        // or release may fold a group.
        if !pointer {
            self.sync_lane_tracks();
            self.sync_analog_summaries();
        } else if press {
            self.sync_group_summaries();
        }
        if self.doc.selection() != selection {
            self.sync_selection();
        }
        if let Some(command) = tracked
            && before != crate::workspace::Stamp::capture(self, &command)
        {
            self.workspace.scheduler.changed(now);
        }
    }

    // -- the selected record ---------------------------------------------------------

    /// Hand the document selection to every panel that reads it: the
    /// transaction panels that are not pinned, and the tables whose
    /// generator holds the record.
    pub(crate) fn sync_selection(&mut self) {
        let Self { panels, doc, .. } = self;
        for (_, model) in panels.transactions_mut() {
            model.follow_selection(doc);
        }
        let Self { panels, doc, .. } = self;
        for table in panels.iter_mut().filter_map(|p| p.kind.table_mut()) {
            table.follow_selection(doc);
        }
        self.changed();
    }

    fn select_transaction(
        &mut self,
        panel: PanelId,
        track: TrackRef,
        id: TransactionRef,
        cursor: Option<u64>,
    ) {
        let Self { panels, doc, .. } = self;
        doc.select(Some(crate::document::TxSelection {
            track,
            id,
            origin: panel,
        }));
        if let Some(time) = cursor
            && let Some(nav) = panels.get_mut(panel).and_then(|p| p.kind.nav_mut())
        {
            nav.set_cursor(doc, Some(time));
        }
        self.changed();
    }

    /// Enter, a double-click, the Details button and the palette all land
    /// here: focus a transaction panel that is free, or open one.
    fn show_transaction(&mut self, from: PanelId) {
        if self.doc.selection().is_none() {
            self.events
                .push(Event::Notice("Select a record first.".into()));
            self.changed();
            return;
        }
        if let Some(id) = self.panels.first_free_transaction() {
            self.panel_command(PanelsCommand::Focus(id));
            self.sync_selection();
            return;
        }
        let model = TransactionModel::new(
            self.table_memory_budget(),
            self.settings.resolved().transaction.detail_items,
        );
        let kind = PanelKind::Transaction(Box::new(model));
        if self
            .panels
            .get(from)
            .is_some_and(|panel| panel.kind.is_start())
        {
            _ = self.replace_panel(from, kind);
            self.sync_selection();
            return;
        }
        match self
            .panels
            .open(kind, from, Some(crate::panels::Axis::Horizontal))
        {
            Ok(id) => {
                self.created(id);
                self.layout_changed();
                self.sync_selection();
            }
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
            }
        }
    }

    /// Reveal the panel's record where it is drawn: the pipeline that already
    /// has a row for it, or a new pipeline over its track.
    fn reveal_transaction(&mut self, panel: PanelId, now: Instant) {
        let Some(shown) = self
            .panels
            .transaction(panel)
            .and_then(|model| model.shown().cloned())
        else {
            return;
        };
        let Some(track) = shown.track.track() else {
            return;
        };
        let target = self.panels.pipeline_showing(&self.doc, track, shown.id);
        let Some(target) = target else {
            self.open_pipeline(track);
            return;
        };
        self.panel_command(PanelsCommand::Focus(target));
        let Self { panels, doc, .. } = self;
        if let Some(pipeline) = panels.pipeline_mut(target) {
            pipeline.reveal_record(doc, track, shown.id, now);
        }
        self.changed();
    }

    /// Variables become wave rows, streams and generators become pipeline
    /// panels (one per distinct track), log sites only report a notice.
    fn activate_members(&mut self, members: &[Member]) {
        let Some(h) = self.doc.hierarchy() else {
            return;
        };
        let mut vars = Vec::new();
        let mut tracks = Vec::new();
        let mut log = false;
        for &member in members {
            if let Member::Var(id) = member {
                if id < h.vars.len() {
                    vars.push(id);
                }
            } else if let Some(track) = h.member_track(member) {
                if h.is_log(member) {
                    log = true;
                } else if !tracks.contains(&track) {
                    tracks.push(track);
                }
            }
        }
        if !vars.is_empty() {
            self.add_vars(&vars);
        }
        let notice =
            log.then_some("Log sites are listed for inspection; log panels are not available yet.");
        self.variables.notice = notice.map(str::to_owned);
        if let Some(notice) = notice {
            self.events.push(Event::Notice(notice.into()));
        }
        for track in tracks {
            self.open_pipeline(track);
        }
        self.changed();
    }

    /// Focus the pipeline panel showing `track`, or open one split below
    /// the focused panel. Any stream or generator of the catalog qualifies;
    /// the stream kind is never inspected.
    fn resident_histories(
        &self,
    ) -> std::collections::HashMap<crate::data::SignalRef, Arc<dyn crate::data::SignalHistory>>
    {
        self.panels
            .iter()
            .filter_map(|panel| panel.kind.waves())
            .flat_map(|waves| waves.items.iter().map(|e| &e.row))
            .filter_map(WaveRow::signal)
            .filter_map(|row| Some((row.source.signal()?, row.history.clone()?)))
            .chain(
                self.panels
                    .iter()
                    .filter_map(|panel| panel.kind.table())
                    .flat_map(|table| table.histories()),
            )
            .collect()
    }

    /// Charge a newly opened local trace to the process budget.
    fn account_local_session(&self, session: Arc<dyn Session>) -> anyhow::Result<Arc<dyn Session>> {
        crate::session::account_local_session(session, self.table_budget.clone())
    }

    pub fn memory_menu(&self) -> MemoryMenu {
        let memory = &self.settings.resolved().memory;
        let locked = ["memory.budgetMiB", "memory.objectMiB"]
            .iter()
            .any(|id| self.settings.is_overridden(id));
        MemoryMenu {
            budget_title: format!("Memory budget: {}", format_mib(memory.budget_mib)),
            budget: memory_choices("memory.budgetMiB", &BUDGET_PRESETS_MIB, memory.budget_mib),
            object_title: format!("Object size limit: {}", format_mib(memory.object_mib)),
            object: memory_choices("memory.objectMiB", &OBJECT_PRESETS_MIB, memory.object_mib),
            // Only a remote trace fixes the object limit when it connects.
            object_note: self
                .doc
                .session()
                .is_some_and(|s| s.remote_id().is_some())
                .then_some("This remote trace applies it when reopened"),
            locked: locked.then_some("Set in VS Code settings (volna.memory.*)"),
        }
    }

    /// Apply the `memory.*` settings live: to the process budget local traces
    /// and procedural sessions use, and to an open remote trace's own budget.
    /// A remote trace keeps the object limit it negotiated when it opened.
    pub(crate) fn apply_memory_limits(&self) {
        let Ok((limit, object_limit)) = self.settings.resolved().limits().bytes() else {
            return;
        };
        for budget in [self.table_budget.clone(), self.table_memory_budget()] {
            budget.set_limit(limit);
            budget.set_object_limit(object_limit);
        }
    }

    pub(crate) fn table_memory_budget(&self) -> crate::remote::memory::MemoryBudget {
        self.doc
            .session()
            .and_then(|session| session.memory_budget())
            .unwrap_or_else(|| self.table_budget.clone())
    }

    fn open_table(&mut self, selected: &[Member], clicked: Option<Member>) {
        let Some(session) = self.doc.session() else {
            return;
        };
        let clicked_only;
        let members = match clicked {
            Some(member) if !selected.contains(&member) => {
                clicked_only = [member];
                &clicked_only[..]
            }
            _ => selected,
        };
        let source = if !members.is_empty() && members.iter().all(|m| matches!(m, Member::Var(_))) {
            let vars = members.iter().filter_map(|m| m.var()).collect::<Vec<_>>();
            crate::table::TableSource::signals(session.hierarchy(), &vars)
        } else if members.len() == 1 {
            let track = session
                .hierarchy()
                .member_track(members[0])
                .ok_or_else(|| anyhow::anyhow!("Choose one generator, or only signals."));
            track.and_then(|track| {
                crate::table::TableSource::generator(session.hierarchy(), session.tracks(), track)
            })
        } else {
            Err(anyhow::anyhow!("Choose one generator, or only signals."))
        };
        match source {
            Ok(source) => self.open_table_source(source, self.panels.focused_id()),
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
            }
        }
    }

    fn open_table_from_panel(&mut self, panel: PanelId, row: Option<usize>) {
        let Some(session) = self.doc.session() else {
            return;
        };
        let source = if let Some(waves) = self.panels.get(panel).and_then(|p| p.kind.waves()) {
            let rows = match row {
                Some(row) if !waves.selected.contains(&row) => {
                    std::collections::BTreeSet::from([row])
                }
                _ => waves.selected.clone(),
            };
            // A group opens the rows it holds.
            let rows = crate::wave::tree::selected_leaves(&waves.items, &rows);
            let vars = rows
                .iter()
                .filter_map(|&index| match waves.signal(index)?.source {
                    crate::wave::model::RowSource::Resolved { var, .. } => Some(var),
                    _ => None,
                })
                .collect::<Vec<_>>();
            // Lanes alone open their generator's table.
            let lane = rows
                .iter()
                .find_map(|&index| waves.items.get(index)?.lane_track());
            match lane {
                Some(track) if vars.is_empty() => crate::table::TableSource::generator(
                    session.hierarchy(),
                    session.tracks(),
                    track,
                ),
                _ => crate::table::TableSource::signals(session.hierarchy(), &vars),
            }
        } else if let Some(track) = self
            .panels
            .get(panel)
            .and_then(|p| p.kind.pipeline())
            .and_then(|pipeline| pipeline.track.track())
        {
            crate::table::TableSource::generator(session.hierarchy(), session.tracks(), track)
        } else {
            return;
        };
        match source {
            Ok(source) => self.open_table_source(source, panel),
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
            }
        }
    }

    fn open_table_source(&mut self, source: crate::table::TableSource, beside: PanelId) {
        let mut table = crate::table::TableModel::new(
            source,
            self.settings.resolved().link_by_default(),
            self.table_memory_budget(),
        );
        table.nav.reset(Some(self.doc.limits()));
        let kind = PanelKind::Table(Box::new(table));
        if self
            .panels
            .get(beside)
            .is_some_and(|panel| panel.kind.is_start())
        {
            _ = self.replace_panel(beside, kind);
            return;
        }
        match self
            .panels
            .open(kind, beside, Some(crate::panels::Axis::Vertical))
        {
            Ok(id) => {
                self.created(id);
                self.layout_changed();
            }
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
            }
        }
    }

    fn open_pipeline(&mut self, track: TrackRef) {
        let Some(session) = self.doc.session().cloned() else {
            return;
        };
        if !session.capabilities().transactions {
            self.events
                .push(Event::Notice("This trace records no transactions.".into()));
            return;
        }
        let Some(declaration) = session.tracks().iter().find(|t| t.id == track) else {
            self.events.push(Event::Notice(format!(
                "Unknown transaction track {}",
                track.0
            )));
            return;
        };
        if let Some(id) = self.panels.pipeline_for_track(track) {
            self.panel_command(PanelsCommand::Focus(id));
            return;
        }
        let model = PipelineModel::new(
            TrackSource::Resolved {
                track,
                path: declaration.path.clone(),
            },
            self.settings.resolved().link_by_default(),
        );
        let focused = self.panels.focused_id();
        if self.panels.focused().kind.is_start() {
            _ = self.replace_panel(focused, PanelKind::Pipeline(Box::new(model)));
            return;
        }
        match self.panels.open(
            PanelKind::Pipeline(Box::new(model)),
            focused,
            Some(crate::panels::Axis::Vertical),
        ) {
            Ok(id) => {
                self.created(id);
                if let Some(w) = self.panels.waves_mut(focused) {
                    w.menu_dismiss();
                }
                self.layout_changed();
            }
            Err(error) => {
                self.events.push(Event::Notice(error.to_string()));
                self.changed();
            }
        }
    }

    fn add_scope_group(&mut self, scope: ScopeId, recursive: bool) {
        let Some(target) = self.waves_target() else {
            return;
        };
        let loaded = self.resident_histories();
        if let Some(w) = self.panels.waves_mut(target)
            && w.add_scope_group(&mut self.doc, scope, recursive, loaded)
        {
            self.changed();
        }
    }

    fn add_vars(&mut self, vars: &[VarId]) {
        if vars.is_empty() {
            return;
        }
        let Some(target) = self.waves_target() else {
            return;
        };
        // Reuse histories already held in another panel before queuing work.
        let loaded = self.resident_histories();
        if let Some(w) = self.panels.waves_mut(target) {
            w.add_vars(&mut self.doc, vars, loaded);
        }
        self.changed();
    }

    /// Signal rows for variables and lanes for generators, in the wave panel
    /// that receives new signals.
    fn add_to_waves(&mut self, members: &[Member]) {
        let Some(h) = self.doc.hierarchy() else {
            return;
        };
        let vars: Vec<VarId> = members.iter().filter_map(|m| m.var()).collect();
        let mut tracks = Vec::new();
        let mut clocks: Vec<String> = Vec::new();
        for &member in members {
            let Some(track) = h.member_track(member) else {
                continue;
            };
            // A clock stream or its generator is drawn from its stretches.
            if let Some(path) = self.member_clock(member) {
                if !clocks.contains(&path) {
                    clocks.push(path);
                }
            } else if matches!(member, Member::Generator(_))
                && !h.is_log(member)
                && !tracks.contains(&track)
            {
                tracks.push(track);
            }
        }
        self.add_vars(&vars);
        if tracks.is_empty() && clocks.is_empty() {
            return;
        }
        let Some(target) = self.waves_target() else {
            return;
        };
        let Self { panels, doc, .. } = self;
        if let Some(w) = panels.waves_mut(target) {
            w.add_lanes(doc, &tracks);
            w.add_clocks(&clocks);
        }
        self.sync_lane_tracks();
        self.changed();
    }

    /// The path of the clock a sidebar member declares: a clock stream or its
    /// generator. Frontends offer it as a ruler or a clock row.
    pub fn member_clock(&self, member: Member) -> Option<String> {
        let session = self.doc.session()?;
        let track = session.hierarchy().member_track(member)?;
        let clock = self.doc.clocks.of_track(track, session.tracks())?;
        Some(clock.path.clone())
    }

    fn add_clock_rulers(&mut self, members: &[Member]) {
        let mut paths: Vec<String> = Vec::new();
        for &member in members {
            if let Some(path) = self.member_clock(member)
                && !paths.contains(&path)
            {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return;
        }
        let Some(target) = self.waves_target() else {
            return;
        };
        let Self { panels, doc, .. } = self;
        if let Some(w) = panels.waves_mut(target) {
            for path in &paths {
                w.nav.clocks.show_ruler(&doc.clocks, path);
            }
        }
        self.changed();
    }

    /// Hold one document retain for every generator a wave lane shows, and
    /// release those no lane shows any more. A new session drops earlier
    /// retains with its tracks.
    /// Hold an analog summary for every long history a plot shows, and
    /// release the others with their memory.
    pub(crate) fn sync_analog_summaries(&mut self) {
        let wanted = self
            .panels
            .iter()
            .filter_map(|panel| panel.kind.waves())
            .flat_map(|waves| waves.items.iter().map(|e| &e.row))
            .filter_map(WaveRow::signal)
            .filter(|s| s.analog.is_some())
            .filter_map(|s| {
                let kind = s.translator.numeric_kind()?;
                let history = s.history.as_ref()?;
                let signal = s.source.signal()?;
                (history.len() >= crate::wave::analog::SUMMARY_MIN_CHANGES)
                    .then(|| ((signal, kind), history.clone()))
            })
            .collect();
        let budget = self.table_memory_budget();
        self.doc.sync_summaries(wanted, &budget);
        self.sync_group_summaries();
    }

    /// Hold an activity summary for every visible folded group whose
    /// signals change more often than a frame can walk, and release the
    /// others with their memory.
    pub(crate) fn sync_group_summaries(&mut self) {
        let mut wanted = std::collections::HashMap::new();
        for waves in self.panels.iter().filter_map(|panel| panel.kind.waves()) {
            for &i in waves.visible().iter() {
                let i = i as usize;
                if !waves.items[i].group().is_some_and(|g| g.collapsed) {
                    continue;
                }
                let members = waves.group_histories(i);
                if members.iter().map(|h| h.len()).sum::<usize>() > crate::wave::group::WALK_MAX {
                    wanted.insert(crate::wave::group::key(&members), members);
                }
            }
        }
        let budget = self.table_memory_budget();
        self.doc.sync_group_summaries(wanted, &budget);
    }

    pub(crate) fn sync_lane_tracks(&mut self) {
        let generation = self.doc.generation();
        if self.lane_tracks.0 != generation {
            self.lane_tracks = (generation, Default::default());
        }
        let wanted: std::collections::HashSet<TrackRef> = self
            .panels
            .iter()
            .filter_map(|panel| panel.kind.waves())
            .flat_map(|waves| waves.items.iter().map(|e| &e.row))
            .filter_map(WaveRow::lane_track)
            .collect();
        let held = &mut self.lane_tracks.1;
        if *held == wanted {
            return;
        }
        for &track in held.difference(&wanted) {
            self.doc.release_track(track);
        }
        held.retain(|track| wanted.contains(track));
        let mut added = false;
        for &track in &wanted {
            if !held.contains(&track) && self.doc.retain_track(track).is_ok() {
                held.insert(track);
                added = true;
            }
        }
        // Records another panel already holds are ready at once.
        if added {
            let Self { panels, doc, .. } = self;
            for waves in panels.iter_mut().filter_map(|p| p.kind.waves_mut()) {
                waves.fit_lanes(doc);
            }
        }
    }

    /// Paste the document clipboard into a wave panel, sharing histories
    /// resident in any panel before queuing loads.
    fn paste_signals(&mut self, panel: PanelId) {
        if self.doc.copied_rows.is_empty() || self.panels.waves(panel).is_none() {
            return;
        }
        let loaded = self.resident_histories();
        if let Some(w) = self.panels.waves_mut(panel) {
            w.paste(&mut self.doc, &loaded);
        }
        self.changed();
    }

    /// Apply a clock choice or go-to to the focused timed panel.
    fn clock_command(&mut self, command: ClockCommand) {
        let now = Instant::now();
        let doc = &mut self.doc;
        let Some(nav) = self.panels.focused_mut().kind.nav_mut() else {
            return;
        };
        match command {
            ClockCommand::ToggleRuler(path) => nav.clocks.toggle_ruler(&doc.clocks, &path),
            ClockCommand::Select(path) => nav.select_clock(&path),
            ClockCommand::GoToCycle(cycle) => {
                if let Err(message) = nav.go_to_cycle(doc, cycle, now) {
                    self.events.push(Event::Notice(message));
                }
            }
        }
        self.changed();
    }

    /// The focused panel's clock choices; `None` without clocks or a timed panel.
    pub fn clock_choices(&self) -> Option<ClockChoices> {
        let clocks = &self.doc.clocks;
        if clocks.is_empty() {
            return None;
        }
        let nav = self.panels.focused().kind.nav()?;
        let rulers = nav.clocks.ruler_paths(clocks);
        Some(ClockChoices {
            clocks: clocks
                .iter()
                .map(|c| (c.path.clone(), rulers.contains(&c.path)))
                .collect(),
            origin: nav.clocks.origin.is_some(),
        })
    }

    /// The clocks the workspace's pipelines count in, in panel order: the
    /// rulers a panel shows until it chooses its own.
    fn pipeline_clocks(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for p in self.panels.iter().filter_map(|p| p.kind.pipeline()) {
            if let Some(c) = p.clock(&self.doc)
                && !paths.contains(&c.path)
            {
                paths.push(c.path.clone());
            }
        }
        paths
    }

    fn action(&mut self, action: Action, now: Instant) {
        use crate::panels::Axis;
        use crate::wave::model::LinkDim;
        let panel = self.panels.focused_id();
        let panel_command = match action {
            Action::SplitRight => Some(PanelsCommand::Split {
                panel,
                axis: Axis::Horizontal,
            }),
            Action::SplitDown => Some(PanelsCommand::Split {
                panel,
                axis: Axis::Vertical,
            }),
            Action::NewPanel => Some(PanelsCommand::NewTab { group_of: panel }),
            // A lone start panel has nothing left to close but the trace.
            Action::ClosePanel
                if self.panels.focused().kind.is_start()
                    && self
                        .panels
                        .iter()
                        .all(|p| !p.kind.is_content() || p.id == panel) =>
            {
                self.close_trace();
                return;
            }
            Action::ClosePanel => Some(PanelsCommand::Close(panel)),
            Action::FocusNextPanel => Some(PanelsCommand::FocusNext),
            Action::FocusPrevPanel => Some(PanelsCommand::FocusPrev),
            Action::ToggleViewportLink => Some(PanelsCommand::ToggleLink {
                panel,
                dim: LinkDim::Viewport,
            }),
            Action::ToggleCursorLink => Some(PanelsCommand::ToggleLink {
                panel,
                dim: LinkDim::Cursor,
            }),
            _ => None,
        };
        if let Some(command) = panel_command {
            self.panel_command(command);
            return;
        }
        if action == Action::PasteSignals {
            self.paste_signals(panel);
            return;
        }
        if matches!(
            action,
            Action::NextCycle | Action::PrevCycle | Action::ToggleCycleOrigin
        ) {
            let doc = &mut self.doc;
            if let Some(nav) = self.panels.focused_mut().kind.nav_mut() {
                match action {
                    Action::NextCycle => _ = nav.step_cycle(doc, true, now),
                    Action::PrevCycle => _ = nav.step_cycle(doc, false, now),
                    _ => nav.toggle_cycle_origin(doc),
                }
                self.changed();
            }
            return;
        }
        let doc = &mut self.doc;
        match &mut self.panels.focused_mut().kind {
            PanelKind::Table(table) => match action {
                Action::GoToStart => {
                    table.command(doc, panel, crate::table::TableCommand::First, now);
                }
                Action::GoToEnd => {
                    table.command(doc, panel, crate::table::TableCommand::Last, now);
                }
                Action::MoveSelectionUp => {
                    table.command(doc, panel, crate::table::TableCommand::Previous, now);
                }
                Action::MoveSelectionDown => {
                    table.command(doc, panel, crate::table::TableCommand::Next, now);
                }
                Action::ClearSelection => {
                    table.command(doc, panel, crate::table::TableCommand::ClearSelection, now);
                }
                Action::GoToCursor
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::ZoomFit
                | Action::ZoomToCursor
                | Action::PanPageLeft
                | Action::PanPageRight
                | Action::PanLeft
                | Action::PanRight
                | Action::NextEdge
                | Action::PrevEdge
                | Action::AddMarker
                | Action::ClearMarkers
                | Action::RemoveSelected
                | Action::CopySignals
                | Action::CutSignals
                | Action::PasteSignals
                | Action::SelectAll
                | Action::CycleFormat
                | Action::ToggleAnalog
                | Action::IncreaseRowHeight
                | Action::DecreaseRowHeight
                | Action::ResetRowHeight
                | Action::GroupSelection
                | Action::Ungroup
                | Action::RenameGroup
                | Action::FoldGroupDeep
                | Action::UnfoldGroupDeep => return,
                Action::SplitRight
                | Action::SplitDown
                | Action::NewPanel
                | Action::ClosePanel
                | Action::FocusNextPanel
                | Action::FocusPrevPanel
                | Action::ToggleViewportLink
                | Action::ToggleCursorLink
                | Action::NextCycle
                | Action::PrevCycle
                | Action::ToggleCycleOrigin => unreachable!(),
            },
            PanelKind::Waves(w) => match action {
                Action::ZoomIn => w.zoom_in(doc, now),
                Action::ZoomOut => w.zoom_out(doc, now),
                Action::ZoomFit => w.zoom_fit(doc, now),
                Action::ZoomToCursor => w.zoom_to_cursor(doc, now),
                Action::PanPageLeft => w.pan_fraction(doc, -1.0, now),
                Action::PanPageRight => w.pan_fraction(doc, 1.0, now),
                Action::GoToStart => w.go_to_start(doc, now),
                Action::GoToEnd => w.go_to_end(doc, now),
                Action::GoToCursor => w.go_to_cursor(doc, now),
                Action::PanLeft => {
                    if !w.fold_key(false, false) {
                        w.pan_fraction(doc, -0.25, now);
                    }
                }
                Action::PanRight => {
                    if !w.fold_key(true, false) {
                        w.pan_fraction(doc, 0.25, now);
                    }
                }
                Action::FoldGroupDeep => _ = w.fold_key(false, true),
                Action::UnfoldGroupDeep => _ = w.fold_key(true, true),
                Action::GroupSelection => _ = w.group_selected(),
                Action::Ungroup => _ = w.ungroup_selected(),
                Action::RenameGroup => _ = w.start_rename(),
                Action::NextEdge => w.next_edge(doc, now),
                Action::PrevEdge => w.prev_edge(doc, now),
                Action::AddMarker => {
                    if let Some(c) = w.cursor(doc) {
                        doc.add_marker(c);
                    }
                }
                Action::ClearMarkers => doc.clear_markers(),
                Action::RemoveSelected => w.remove_selected(),
                Action::CopySignals => w.copy_selected(doc),
                Action::CutSignals => w.cut_selected(doc),
                Action::SelectAll => w.select_all(),
                Action::ClearSelection => w.clear_selection(doc),
                Action::CycleFormat => w.cycle_format(doc),
                Action::ToggleAnalog => w.toggle_analog(),
                Action::IncreaseRowHeight => w.step_row_height(1),
                Action::DecreaseRowHeight => w.step_row_height(-1),
                Action::ResetRowHeight => w.step_row_height(0),
                Action::MoveSelectionUp => w.move_selection(-1),
                Action::MoveSelectionDown => w.move_selection(1),
                // Panel actions and paste were resolved before borrowing a wave model.
                Action::PasteSignals
                | Action::SplitRight
                | Action::SplitDown
                | Action::NewPanel
                | Action::ClosePanel
                | Action::FocusNextPanel
                | Action::FocusPrevPanel
                | Action::ToggleViewportLink
                | Action::ToggleCursorLink
                | Action::NextCycle
                | Action::PrevCycle
                | Action::ToggleCycleOrigin => unreachable!(),
            },
            // The same keys, with rows in place of selection: ↑ ↓ scroll rows,
            // zoom scales both axes, Escape cancels a drag then the cursor.
            PanelKind::Pipeline(p) => match action {
                Action::ZoomIn => p.zoom_in(doc, now),
                Action::ZoomOut => p.zoom_out(doc, now),
                Action::ZoomFit => p.zoom_fit(doc, now),
                Action::ZoomToCursor => p.zoom_to_cursor(doc, now),
                Action::PanPageLeft => p.pan_fraction(doc, -1.0, now),
                Action::PanPageRight => p.pan_fraction(doc, 1.0, now),
                Action::GoToStart => p.go_to_start(doc, now),
                Action::GoToEnd => p.go_to_end(doc, now),
                Action::GoToCursor => p.go_to_cursor(doc, now),
                Action::PanLeft => p.pan_fraction(doc, -0.25, now),
                Action::PanRight => p.pan_fraction(doc, 0.25, now),
                Action::AddMarker => {
                    if let Some(c) = p.nav.cursor(doc) {
                        doc.add_marker(c);
                    }
                }
                Action::ClearMarkers => doc.clear_markers(),
                Action::ClearSelection => p.escape(doc),
                Action::MoveSelectionUp => p.move_selection(doc, panel, -1, now),
                Action::MoveSelectionDown => p.move_selection(doc, panel, 1, now),
                Action::NextEdge
                | Action::PrevEdge
                | Action::RemoveSelected
                | Action::CopySignals
                | Action::CutSignals
                | Action::PasteSignals
                | Action::SelectAll
                | Action::CycleFormat
                | Action::ToggleAnalog
                | Action::IncreaseRowHeight
                | Action::DecreaseRowHeight
                | Action::ResetRowHeight
                | Action::GroupSelection
                | Action::Ungroup
                | Action::RenameGroup
                | Action::FoldGroupDeep
                | Action::UnfoldGroupDeep => return,
                Action::SplitRight
                | Action::SplitDown
                | Action::NewPanel
                | Action::ClosePanel
                | Action::FocusNextPanel
                | Action::FocusPrevPanel
                | Action::ToggleViewportLink
                | Action::ToggleCursorLink
                | Action::NextCycle
                | Action::PrevCycle
                | Action::ToggleCycleOrigin => unreachable!(),
            },
            _ => return,
        }
        self.changed();
    }

    // -- frames --------------------------------------------------------------------

    /// Advance animations. Returns true while another frame is needed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut animating = self.doc.shared.viewport.tick(now);
        for panel in self.panels.iter_mut() {
            animating |= panel.tick(&self.doc, now);
        }
        let waiting_to_save = self.workspace_tick(now);
        let waiting_for_settings = self.settings_tick(now, false);
        animating || waiting_to_save || waiting_for_settings
    }

    pub fn is_animating(&self) -> bool {
        self.doc.shared.viewport.is_animating() || self.panels.iter().any(Panel::is_animating)
    }

    /// Lay a canvas panel out in `bounds`; the result feeds hit regions.
    /// `None` for panels the core does not paint.
    pub fn layout_panel(
        &mut self,
        id: PanelId,
        bounds: Rect,
        theme: &Theme,
    ) -> Option<PanelLayout<'_>> {
        self.doc.clocks.defaults = self.pipeline_clocks();
        let doc = &self.doc;
        Some(match &mut self.panels.get_mut(id)?.kind {
            PanelKind::Waves(w) => PanelLayout::Waves(w.layout(bounds, doc, theme)),
            PanelKind::Pipeline(p) => PanelLayout::Pipeline(p.layout(bounds, doc, theme)),
            PanelKind::Table(table) => PanelLayout::Table(table.layout(bounds, theme)),
            _ => return None,
        })
    }

    /// Paint a canvas panel with the layout from the last [`App::layout_panel`].
    pub fn render_panel(
        &mut self,
        id: PanelId,
        theme: &Theme,
        measure: &mut dyn TextMeasure,
    ) -> &Scene {
        let mut scene = std::mem::take(&mut self.scene);
        self.render_panel_into(id, theme, measure, &mut scene);
        self.scene = scene;
        &self.scene
    }

    /// Like [`App::render_panel`] into a caller-owned buffer, for frontends
    /// that must hand the scene to their painter while the app is borrowed.
    pub fn render_panel_into(
        &mut self,
        id: PanelId,
        theme: &Theme,
        measure: &mut dyn TextMeasure,
        scene: &mut Scene,
    ) {
        scene.clear();
        let Some(panel) = self.panels.get(id) else {
            return;
        };
        let focused = id == self.panels.focused_id();
        let bounds = match &panel.kind {
            PanelKind::Waves(w) => {
                crate::wave::paint::paint(
                    w,
                    &self.doc,
                    theme,
                    &mut self.text,
                    measure,
                    scene,
                    focused,
                );
                w.last_layout().bounds
            }
            PanelKind::Pipeline(p) => {
                crate::pipeline::paint::paint(
                    p,
                    &self.doc,
                    theme,
                    &mut self.text,
                    measure,
                    scene,
                    focused,
                );
                p.last_layout().bounds
            }
            PanelKind::Table(table) => {
                crate::table::paint::paint(table, theme, scene);
                table.layout.bounds
            }
            _ => return,
        };
        if focused && self.panels.len() > 1 {
            scene.quad(
                bounds,
                crate::Color::TRANSPARENT,
                0.0,
                1.0,
                theme.border_focused,
            );
        }
    }

    /// The last painted scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    // -- chrome text ------------------------------------------------------------------

    /// The recognized PIPELINE streams of the open trace: (dotted path, track).
    pub fn pipeline_streams(&self) -> Vec<(String, TrackRef)> {
        use crate::data::transactions::TrackKind;
        self.doc
            .session()
            .map(|session| {
                session
                    .tracks()
                    .iter()
                    .filter(|t| matches!(&t.kind, TrackKind::Stream { kind } if kind == "PIPELINE"))
                    .map(|t| (t.path.join("."), t.id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What a start panel shows, once a trace is open.
    pub fn start_summary(&self) -> Option<StartSummary> {
        let session = self.doc.session()?;
        let info = session.info();
        let base = TimeBase::of(info);
        let (a, b) = info.time_range;
        Some(StartSummary {
            name: info.name.clone(),
            time_range: format!(
                "{} – {}",
                format_time(a as f64, base),
                format_time(b as f64, base)
            ),
            variables: session.hierarchy().vars.len(),
            tracks: session.tracks().len(),
            pipelines: self.pipeline_streams(),
        })
    }

    pub fn status(&self) -> Status {
        let mut s = Status {
            sidebar_notice: self.variables.notice.clone(),
            workspace_notice: self
                .workspace
                .scheduler
                .error()
                .map(str::to_owned)
                .or_else(|| self.workspace.notices.last().cloned()),
            file: self.doc.name(),
            frame_ms: format!("{:.1} ms", self.panels.focused().frame_ms_avg()),
            ..Default::default()
        };
        if let Some(src) = self.doc.session() {
            let budget = self.table_memory_budget();
            s.memory = Some(MemoryStatus {
                used: budget.used(),
                limit: budget.limit(),
            });
            let info = src.info();
            let base = TimeBase::of(info);
            let (a, b) = info.time_range;
            s.time_range = Some(format!(
                "{} – {}",
                format_time(a as f64, base),
                format_time(b as f64, base)
            ));
            s.signals = Some(format!("{} signals", info.signal_count));
            s.changes = info.change_count.map(|n| format!("{n} changes"));
            s.panel = (self.panels.len() > 1).then(|| self.panels.focused().title());
            let focused = self.panels.focused();
            let (nav, width) = match &focused.kind {
                PanelKind::Waves(w) => (&w.nav, f64::from(w.wave_width)),
                PanelKind::Pipeline(p) => {
                    s.hover = p.hover_text(&self.doc);
                    (&p.nav, p.last_layout().cells_width_f64())
                }
                PanelKind::Table(table) => (&table.nav, f64::from(table.layout.body.width())),
                _ => return s,
            };
            s.links = Some(nav.link);
            let vp = nav.viewport(&self.doc);
            let px_per = vp.width() / width.max(1.0);
            s.px_per = Some(format!("1 px = {}", format_time(px_per, base)));
            let cursor = nav.cursor(&self.doc);
            s.cursor = cursor.map(|c| format_time(c as f64, base));
            let rulers = nav.clocks.rulers(&self.doc.clocks);
            if let Some(c) = cursor {
                s.clocks = rulers
                    .iter()
                    .map(|clock| crate::clock::position_at(&nav.clocks, clock, c))
                    .collect();
                if let Some(m) = self.doc.markers.iter().min_by_key(|m| m.time.abs_diff(c)) {
                    let dt = c as i128 - m.time as i128;
                    let sign = if dt < 0 { "−" } else { "" };
                    let mut parts = vec![format!(
                        "Δ {sign}{}",
                        format_time(dt.unsigned_abs() as f64, base)
                    )];
                    for clock in &rulers {
                        if let Some(n) = clock
                            .timeline()
                            .and_then(|t| crate::clock::cycles_between(t, m.time, c))
                        {
                            parts.push(format!("{n} {}", clock.name));
                        }
                    }
                    s.delta = Some(parts.join(" · "));
                }
            }
            if !self.doc.markers.is_empty() {
                s.markers = Some(format!("{} markers", self.doc.markers.len()));
            }
        }
        s
    }

    pub fn trace_state(&self) -> &TraceState {
        self.doc.state()
    }

    /// One-line summary of the viewer state, for diagnostics and tests.
    pub fn debug_state(&self) -> String {
        self.panels.layout().panels().into_iter().map(|id| {
        let panel = self.panels.get(id).unwrap();
        if let Some(p) = panel.kind.pipeline() {
            let rows = p.rows.target();
            let (state, count) = match p.rows(&self.doc) {
                crate::pipeline::Rows::Ready(set) => ("ready", set.len()),
                crate::pipeline::Rows::Loading => ("loading", 0),
                crate::pipeline::Rows::Failed(_) => ("failed", 0),
                crate::pipeline::Rows::Unresolved => ("unresolved", 0),
                crate::pipeline::Rows::Unavailable => ("unavailable", 0),
            };
            return format!(
                "panel={} pipeline track={} focused={} linked=({},{}) {state} rows={count} top={:.2} row_px={:.2} label_w={} cursor={:?} markers={} viewport=({:.0},{:.0}) hover={:?} drag={:?}",
                id.0, p.track.path().join("."), id == self.panels.focused_id(), p.nav.link.viewport, p.nav.link.cursor,
                rows.top, rows.row_px, p.label_width,
                p.nav.cursor(&self.doc), self.doc.markers.len(),
                p.nav.viewport(&self.doc).start, p.nav.viewport(&self.doc).end,
                p.hover, p.drag,
            );
        }
        if let Some(model) = panel.kind.transaction() {
            let state = match model.state(&self.doc) {
                crate::transaction::TxPanelState::Empty => "empty",
                crate::transaction::TxPanelState::Refused(_) => "refused",
                crate::transaction::TxPanelState::Loading => "loading",
                crate::transaction::TxPanelState::Failed(_) => "failed",
                crate::transaction::TxPanelState::Missing => "missing",
                crate::transaction::TxPanelState::Ready(_) => "ready",
            };
            return format!(
                "panel={} transaction focused={} {state} record={:?} pinned={} back={} forward={} selection={:?}",
                id.0,
                id == self.panels.focused_id(),
                model.shown().map(|shown| (shown.track.path().join("."), shown.id.0)),
                model.pinned,
                model.can_go_back(),
                model.can_go_forward(),
                self.doc.selection().map(|s| s.id.0),
            );
        }
        if let Some(table) = panel.kind.table() {
            return format!(
                "panel={} table focused={} rows={} top={} selected={:?} status={}",
                id.0,
                id == self.panels.focused_id(),
                table.len(),
                table.viewport.top,
                table.selected,
                table.status()
            );
        }
        let Some(w) = panel.kind.waves() else {
            let kind = match &panel.kind {
                PanelKind::Start => "start",
                PanelKind::Settings => "settings",
                _ => "unsupported",
            };
            return format!(
                "panel={} {kind} focused={} drag={:?} sidebar_w={}px scopes_frac={:.2}",
                id.0,
                id == self.panels.focused_id(),
                self.drag,
                self.sidebar_width,
                self.scopes_fraction
            );
        };
        format!(
            "panel={} focused={} linked=({},{}) items={} loaded={} selected={:?} anchor={:?} cursor={:?} markers={} viewport=({:.0},{:.0}) menu={} drag={:?} sidebar_w={}px scopes_frac={:.2}",
            id.0, id == self.panels.focused_id(), w.nav.link.viewport, w.nav.link.cursor,
            w.items.len(),
            w.loaded_count(),
            w.selected,
            w.anchor,
            w.cursor(&self.doc),
            self.doc.markers.len(),
            w.viewport(&self.doc).start,
            w.viewport(&self.doc).end,
            w.menu.is_some(),
            self.drag,
            self.sidebar_width,
            self.scopes_fraction
        )
        }).collect::<Vec<_>>().join("\n")
    }
}
