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
use crate::wave::model::{MenuAction, PointerEvent, WaveMenuKind};
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
    SelectAll,
    ClearSelection,
    CycleFormat,
    /// Step the selected rows to the next larger / smaller height preset,
    /// or back to the default height.
    IncreaseRowHeight,
    DecreaseRowHeight,
    ResetRowHeight,
    MoveSelectionUp,
    MoveSelectionDown,
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
    pub markers: Option<String>,
    pub frame_ms: String,
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
            table_budget: crate::remote::memory::MemoryBudget::new(512 * 1024 * 1024),
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
            match crate::session::account_local_session(session, self.table_budget.clone()) {
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
            .flat_map(|waves| &waves.items)
            .filter_map(|row| row.source.signal())
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
            *opened = crate::session::account_local_session(session, self.table_budget.clone());
        }
        match self.doc.deliver(result) {
            Some(Delivered::Track) => {
                for pipeline in self.panels.pipelines_mut() {
                    pipeline.refresh(&self.doc);
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
            Command::OpenSignalMenu(panel) => {
                if let Some(waves) = self.panels.waves_mut(panel) {
                    waves.open_selected_signal_menu();
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
                            for row in &mut waves.items {
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
            .flat_map(|waves| &waves.items)
            .filter_map(|row| Some((row.source.signal()?, row.history.clone()?)))
            .chain(
                self.panels
                    .iter()
                    .filter_map(|panel| panel.kind.table())
                    .flat_map(|table| table.histories()),
            )
            .collect()
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
                Some(row) if !waves.selected.contains(&row) => vec![row],
                _ => waves.selected.iter().copied().collect(),
            };
            let vars = rows
                .into_iter()
                .filter_map(|index| match waves.items.get(index)?.source {
                    crate::wave::model::RowSource::Resolved { var, .. } => Some(var),
                    _ => None,
                })
                .collect::<Vec<_>>();
            crate::table::TableSource::signals(session.hierarchy(), &vars)
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
                | Action::SelectAll
                | Action::CycleFormat
                | Action::IncreaseRowHeight
                | Action::DecreaseRowHeight
                | Action::ResetRowHeight => return,
                Action::SplitRight
                | Action::SplitDown
                | Action::NewPanel
                | Action::ClosePanel
                | Action::FocusNextPanel
                | Action::FocusPrevPanel
                | Action::ToggleViewportLink
                | Action::ToggleCursorLink => unreachable!(),
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
                Action::PanLeft => w.pan_fraction(doc, -0.25, now),
                Action::PanRight => w.pan_fraction(doc, 0.25, now),
                Action::NextEdge => w.next_edge(doc, now),
                Action::PrevEdge => w.prev_edge(doc, now),
                Action::AddMarker => {
                    if let Some(c) = w.cursor(doc) {
                        doc.add_marker(c);
                    }
                }
                Action::ClearMarkers => doc.clear_markers(),
                Action::RemoveSelected => w.remove_selected(),
                Action::SelectAll => w.select_all(),
                Action::ClearSelection => w.clear_selection(doc),
                Action::CycleFormat => w.cycle_format(doc),
                Action::IncreaseRowHeight => w.step_row_height(1),
                Action::DecreaseRowHeight => w.step_row_height(-1),
                Action::ResetRowHeight => w.step_row_height(0),
                Action::MoveSelectionUp => w.move_selection(-1),
                Action::MoveSelectionDown => w.move_selection(1),
                // Panel actions were resolved before borrowing a wave model.
                Action::SplitRight
                | Action::SplitDown
                | Action::NewPanel
                | Action::ClosePanel
                | Action::FocusNextPanel
                | Action::FocusPrevPanel
                | Action::ToggleViewportLink
                | Action::ToggleCursorLink => unreachable!(),
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
                | Action::SelectAll
                | Action::CycleFormat
                | Action::IncreaseRowHeight
                | Action::DecreaseRowHeight
                | Action::ResetRowHeight => return,
                Action::SplitRight
                | Action::SplitDown
                | Action::NewPanel
                | Action::ClosePanel
                | Action::FocusNextPanel
                | Action::FocusPrevPanel
                | Action::ToggleViewportLink
                | Action::ToggleCursorLink => unreachable!(),
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
            animating |= panel.tick(now);
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
            s.cursor = nav.cursor(&self.doc).map(|c| format_time(c as f64, base));
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
