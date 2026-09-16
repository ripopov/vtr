//! `App`: the whole viewer with no toolkit attached. Frontends feed it
//! [`Command`]s, drain [`Event`]s, perform the [`LoadRequest`]s it queues, and
//! ask it to lay out and paint the wave panel into a [`Scene`].

use std::sync::Arc;

use web_time::Instant;

use crate::data::{ScopeId, VarId};
use crate::document::{Delivered, Document, TraceState};
use crate::geometry::{Modifiers, Rect};
use crate::panels::{PanelId, Panels, PanelsCommand};
use crate::scene::{Scene, TextCache, TextMeasure};
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};
use crate::sidebar::{Key, ScopeTreeModel, VariableListModel};
use crate::theme::Theme;
use crate::wave::layout::WaveLayout;
use crate::wave::model::{MenuAction, PointerEvent, WaveModel};
use crate::wave::timeline::format_time;

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
    /// The format menu's popup reported a choice / was dismissed.
    MenuSelect(PanelId, MenuAction),
    MenuDismiss(PanelId),
    SetSidebarWidth(f32),
    /// Scope tree height as a fraction of the sidebar.
    SetScopesFraction(f32),
    ChromeDragStart(ChromeDrag),
    ChromeDragEnd,
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
}

/// Text the status bar shows, already formatted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub workspace_notice: Option<String>,
    pub panel: Option<String>,
    pub links: Option<crate::wave::model::Link>,
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

pub struct App {
    pub doc: Document,
    pub workspace: crate::workspace::State,
    pub panels: Panels,
    pub scopes: ScopeTreeModel,
    pub variables: VariableListModel,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    /// Height of the scope tree as a fraction of the sidebar.
    pub scopes_fraction: f32,
    pub drag: Option<ChromeDrag>,
    show_all_on_open: bool,
    pub(crate) events: Vec<Event>,
    text: TextCache,
    scene: Scene,
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
            scopes: ScopeTreeModel::new(),
            variables: VariableListModel::new(),
            sidebar_width: 280.0,
            sidebar_visible: true,
            scopes_fraction: 0.42,
            drag: None,
            show_all_on_open: false,
            events: Vec::new(),
            text: TextCache::default(),
            scene: Scene::default(),
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

    fn open(&mut self, spec: OpenSpec, show_all: bool) {
        self.open_with_workspace(spec, show_all);
    }

    pub(crate) fn open_now(&mut self, spec: OpenSpec, show_all: bool) {
        self.show_all_on_open = show_all;
        self.doc.open(spec);
        self.changed();
    }

    /// Replace the session immediately (tests and hosts that hold one).
    pub fn set_session(&mut self, session: Arc<dyn Session>) {
        self.doc.set_session(session);
        self.on_session_changed();
    }

    pub fn close_trace(&mut self) {
        self.close_with_workspace();
    }

    pub(crate) fn close_now(&mut self) {
        self.doc.close();
        self.on_session_changed();
    }

    fn on_session_changed(&mut self) {
        let limits = self.doc.session().map(|s| s.info().time_range);
        if let Err(e) = self.panels.reset(limits) {
            self.events.push(Event::Notice(e.to_string()));
        }
        if let Some(w) = self.panels.focused_waves_mut() {
            w.link = self.workspace.preferences.link_by_default;
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
            .collect()
    }

    pub fn deliver(&mut self, result: LoadResult) {
        match self.doc.deliver(result) {
            Some(Delivered::Track) => self.changed(),
            Some(Delivered::Signals(results)) => {
                for (signal, result) in results {
                    let result = result.map_err(|e| e.to_string());
                    for panel in self.panels.iter_mut() {
                        if let Some(w) = panel.kind.waves_mut() {
                            w.finish_signal(signal, result.clone().map_err(anyhow::Error::msg));
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

    fn layout_changed(&mut self) {
        self.events.push(Event::LayoutChanged {
            revision: self.panels.revision(),
        });
        self.changed();
    }

    fn panel_command(&mut self, command: PanelsCommand) {
        let revision = self.panels.revision();
        let old_focus = self.panels.focused_id();
        let result = match command {
            PanelsCommand::Split { panel, axis } => {
                self.panels.create(panel, Some(axis)).map(|_| ())
            }
            PanelsCommand::NewTab { group_of } => self.panels.create(group_of, None).map(|_| ()),
            PanelsCommand::Close(id) => self.panels.close(id),
            PanelsCommand::CloseOthers(id) => self.panels.close_others(id),
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
        match command {
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
                if let Some(w) = self.panels.waves_mut(id)
                    && w.pointer(&mut self.doc, ev)
                {
                    self.changed();
                }
            }
            Command::AddVars(vars) => self.add_vars(&vars),
            Command::SelectScope(id) => {
                if self.scopes.select(id) {
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
                let vars = self.variables.rows.clone();
                self.add_vars(&vars);
            }
            Command::AddSelectedOrAllVars => {
                let vars = self.variables.selected_or_all();
                self.add_vars(&vars);
            }
            Command::VariablesKey(key, modifiers) => {
                let out = self.variables.key(&key, modifiers);
                if let Some(vars) = out.add {
                    self.add_vars(&vars);
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
            Command::MenuSelect(panel, action) => {
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
        }
        if let Some(command) = tracked
            && before != crate::workspace::Stamp::capture(self, &command)
        {
            self.workspace.scheduler.changed(now);
        }
    }

    fn add_vars(&mut self, vars: &[VarId]) {
        if vars.is_empty() {
            return;
        }
        // Reuse histories already held in another panel before queuing work.
        let loaded: std::collections::HashMap<_, _> = self
            .panels
            .iter()
            .filter_map(|p| p.kind.waves())
            .flat_map(|w| &w.items)
            .filter_map(|row| Some((row.source.signal()?, row.history.clone()?)))
            .collect();
        if let Some(w) = self.panels.focused_mut().kind.waves_mut() {
            w.add_vars_with_histories(&mut self.doc, vars, loaded);
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
            Action::ClosePanel if self.panels.len() == 1 => {
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
        let Some(w) = self.panels.focused_mut().kind.waves_mut() else {
            return;
        };
        match action {
            Action::ZoomIn => w.zoom_in(doc, now),
            Action::ZoomOut => w.zoom_out(doc, now),
            Action::ZoomFit => w.zoom_fit(doc, now),
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
        }
        self.changed();
    }

    // -- frames --------------------------------------------------------------------

    /// Advance animations. Returns true while another frame is needed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut animating = self.doc.shared.viewport.tick(now);
        for panel in self.panels.iter_mut() {
            if let Some(w) = panel.kind.waves_mut() {
                animating |= w.tick(now);
            }
        }
        let waiting_to_save = self.workspace_tick(now);
        animating || waiting_to_save
    }

    pub fn is_animating(&self) -> bool {
        self.doc.shared.viewport.is_animating()
            || self
                .panels
                .iter()
                .filter_map(|p| p.kind.waves())
                .any(WaveModel::is_animating)
    }

    /// Lay the wave panel out in `bounds`; the result feeds hit regions.
    pub fn layout_waves(
        &mut self,
        id: PanelId,
        bounds: Rect,
        theme: &Theme,
    ) -> Option<&WaveLayout> {
        Some(self.panels.waves_mut(id)?.layout(bounds, &self.doc, theme))
    }

    /// Paint the wave panel with the layout from the last [`App::layout_waves`].
    pub fn render_waves(
        &mut self,
        id: PanelId,
        theme: &Theme,
        measure: &mut dyn TextMeasure,
    ) -> &Scene {
        let mut scene = std::mem::take(&mut self.scene);
        self.render_waves_into(id, theme, measure, &mut scene);
        self.scene = scene;
        &self.scene
    }

    /// Like [`App::render_waves`] into a caller-owned buffer, for frontends
    /// that must hand the scene to their painter while the app is borrowed.
    pub fn render_waves_into(
        &mut self,
        id: PanelId,
        theme: &Theme,
        measure: &mut dyn TextMeasure,
        scene: &mut Scene,
    ) {
        scene.clear();
        let Some(w) = self.panels.waves(id) else {
            return;
        };
        let focused = id == self.panels.focused_id();
        crate::wave::paint::paint(w, &self.doc, theme, &mut self.text, measure, scene, focused);
        if focused && self.panels.len() > 1 {
            scene.quad(
                w.last_layout().bounds,
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

    pub fn status(&self) -> Status {
        let mut s = Status {
            workspace_notice: self
                .workspace
                .scheduler
                .error()
                .map(str::to_owned)
                .or_else(|| self.workspace.notices.last().cloned()),
            file: self.doc.name(),
            frame_ms: format!(
                "{:.1} ms",
                self.panels
                    .focused()
                    .kind
                    .waves()
                    .map_or(0.0, |w| w.frame_ms_avg)
            ),
            ..Default::default()
        };
        if let Some(src) = self.doc.session() {
            let info = src.info();
            let (a, b) = info.time_range;
            s.time_range = Some(format!(
                "{} – {}",
                format_time(a as f64, info.timescale),
                format_time(b as f64, info.timescale)
            ));
            s.signals = Some(format!("{} signals", info.signal_count));
            s.changes = info.change_count.map(|n| format!("{n} changes"));
            s.panel = (self.panels.len() > 1).then(|| self.panels.focused().title());
            let Some(w) = self.panels.focused().kind.waves() else {
                return s;
            };
            s.links = Some(w.link);
            let vp = w.viewport(&self.doc);
            let px_per = vp.width() / f64::from(w.wave_width).max(1.0);
            s.px_per = Some(format!("1 px = {}", format_time(px_per, info.timescale)));
            s.cursor = w
                .cursor(&self.doc)
                .map(|c| format_time(c as f64, info.timescale));
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
        let Some(w) = panel.kind.waves() else { return format!("panel={} unsupported", id.0) };
        format!(
            "panel={} focused={} linked=({},{}) items={} loaded={} selected={:?} anchor={:?} cursor={:?} markers={} viewport=({:.0},{:.0}) menu={} drag={:?} sidebar_w={}px scopes_frac={:.2}",
            id.0, id == self.panels.focused_id(), w.link.viewport, w.link.cursor,
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
