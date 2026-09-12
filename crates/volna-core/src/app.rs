//! `App`: the whole viewer with no toolkit attached. Frontends feed it
//! [`Command`]s, drain [`Event`]s, perform the [`LoadRequest`]s it queues, and
//! ask it to lay out and paint the wave panel into a [`Scene`].

use std::sync::Arc;

use web_time::Instant;

use crate::data::{ScopeId, VarId};
use crate::document::{Delivered, Document, TraceState};
use crate::geometry::{Modifiers, Rect};
use crate::scene::{Scene, TextCache, TextMeasure};
use crate::session::{LoadRequest, LoadResult, OpenSpec, Session};
use crate::sidebar::{Key, ScopeTreeModel, VariableListModel};
use crate::theme::Theme;
use crate::wave::layout::WaveLayout;
use crate::wave::model::{PointerEvent, WaveModel};
use crate::wave::timeline::format_time;

/// Keyboard actions of the wave panel. Frontends bind keys to these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
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
    CloseTrace,
    ToggleSidebar,
    Action(Action),
    Pointer(PointerEvent),
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
    MenuSelect(String),
    MenuDismiss,
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
    pub waves: WaveModel,
    pub scopes: ScopeTreeModel,
    pub variables: VariableListModel,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    /// Height of the scope tree as a fraction of the sidebar.
    pub scopes_fraction: f32,
    pub drag: Option<ChromeDrag>,
    show_all_on_open: bool,
    events: Vec<Event>,
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
            waves: WaveModel::new(),
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
        self.doc.close();
        self.on_session_changed();
    }

    fn on_session_changed(&mut self) {
        let limits = self.doc.session().map(|s| s.info().time_range);
        self.waves.reset(limits);
        self.scopes.reset(self.doc.hierarchy());
        self.variables.reset(self.doc.hierarchy());
        let scope = self.scopes.selected;
        self.variables.set_scope(self.doc.hierarchy(), scope);
        self.changed();
    }

    // -- the pull-based load loop --------------------------------------------------

    /// Loads the frontend should perform, then hand to [`App::deliver`].
    pub fn take_requests(&mut self) -> Vec<LoadRequest> {
        self.doc.take_requests()
    }

    pub fn deliver(&mut self, result: LoadResult) {
        match self.doc.deliver(result) {
            Some(Delivered::Opened(Ok(session))) => {
                self.on_session_changed();
                if self.show_all_on_open {
                    let count = session.hierarchy().vars.len();
                    self.add_vars(&(0..count).collect::<Vec<_>>());
                }
            }
            Some(Delivered::Opened(Err(_))) => self.changed(),
            Some(Delivered::Signal { signal, result }) => {
                self.waves.finish_signal(signal, result);
                self.changed();
            }
            None => {}
        }
    }

    /// Events since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn changed(&mut self) {
        if self.events.last() != Some(&Event::Changed) {
            self.events.push(Event::Changed);
        }
    }

    // -- commands ----------------------------------------------------------------

    pub fn handle(&mut self, command: Command) {
        self.handle_at(command, Instant::now());
    }

    /// Like [`App::handle`] with an explicit clock for animations.
    pub fn handle_at(&mut self, command: Command, now: Instant) {
        match command {
            Command::Open(spec) => self.open(spec, false),
            Command::RequestOpenDialog => self.events.push(Event::OpenFileDialog),
            Command::CloseTrace => self.close_trace(),
            Command::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.changed();
            }
            Command::Action(a) => self.action(a, now),
            Command::Pointer(ev) => {
                if self.waves.pointer(&mut self.doc, ev) {
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
            Command::MenuSelect(id) => {
                self.waves.menu_select(&self.doc, &id);
                self.changed();
            }
            Command::MenuDismiss => {
                self.waves.menu_dismiss();
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
    }

    fn add_vars(&mut self, vars: &[VarId]) {
        if vars.is_empty() {
            return;
        }
        self.waves.add_vars(&mut self.doc, vars);
        self.changed();
    }

    fn action(&mut self, action: Action, now: Instant) {
        let doc = &mut self.doc;
        let w = &mut self.waves;
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
                doc.add_marker_at_cursor();
            }
            Action::ClearMarkers => doc.clear_markers(),
            Action::RemoveSelected => w.remove_selected(),
            Action::SelectAll => w.select_all(),
            Action::ClearSelection => w.clear_selection(doc),
            Action::CycleFormat => w.cycle_format(doc),
            Action::MoveSelectionUp => w.move_selection(-1),
            Action::MoveSelectionDown => w.move_selection(1),
        }
        self.changed();
    }

    // -- frames --------------------------------------------------------------------

    /// Advance animations. Returns true while another frame is needed.
    pub fn tick(&mut self, now: Instant) -> bool {
        self.waves.tick(now)
    }

    pub fn is_animating(&self) -> bool {
        self.waves.is_animating()
    }

    /// Lay the wave panel out in `bounds`; the result feeds hit regions.
    pub fn layout_waves(&mut self, bounds: Rect, theme: &Theme) -> &WaveLayout {
        self.waves.layout(bounds, &self.doc, theme)
    }

    /// Paint the wave panel with the layout from the last [`App::layout_waves`].
    pub fn render_waves(&mut self, theme: &Theme, measure: &mut dyn TextMeasure) -> &Scene {
        let mut scene = std::mem::take(&mut self.scene);
        self.render_waves_into(theme, measure, &mut scene);
        self.scene = scene;
        &self.scene
    }

    /// Like [`App::render_waves`] into a caller-owned buffer, for frontends
    /// that must hand the scene to their painter while the app is borrowed.
    pub fn render_waves_into(
        &mut self,
        theme: &Theme,
        measure: &mut dyn TextMeasure,
        scene: &mut Scene,
    ) {
        scene.clear();
        crate::wave::paint::paint(
            &self.waves,
            &self.doc,
            theme,
            &mut self.text,
            measure,
            scene,
        );
    }

    /// The last painted scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    // -- chrome text ------------------------------------------------------------------

    pub fn status(&self) -> Status {
        let mut s = Status {
            file: self.doc.name(),
            frame_ms: format!("{:.1} ms", self.waves.frame_ms_avg),
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
            let vp = self.waves.viewport;
            let px_per = vp.width() / f64::from(self.waves.wave_width).max(1.0);
            s.px_per = Some(format!("1 px = {}", format_time(px_per, info.timescale)));
            s.cursor = self
                .doc
                .cursor
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
        let w = &self.waves;
        format!(
            "items={} loaded={} selected={:?} anchor={:?} cursor={:?} markers={} viewport=({:.0},{:.0}) menu={} drag={:?} sidebar_w={}px scopes_frac={:.2}",
            w.items.len(),
            w.loaded_count(),
            w.selected,
            w.anchor,
            self.doc.cursor,
            self.doc.markers.len(),
            w.viewport.start,
            w.viewport.end,
            w.menu.is_some(),
            self.drag,
            self.sidebar_width,
            self.scopes_fraction
        )
    }
}
