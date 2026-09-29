//! Headless tests of undo and redo (`docs/undo-redo.html`).
//!
//! The model-based test drives seeded random commands, gestures, navigation,
//! undo and redo over the landing trace (signals, groups, lanes, clocks,
//! pipelines, tables and transaction panels) and checks the journal against
//! the undoable projection of the workspace: the capture with its
//! navigation fields cleared. Focused tests cover grouping, gestures,
//! memory, loads, labels, context and clearing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use volna_core::testing::{a, a_all};
use volna_core::trace::TraceId;

use serde_json::Value;
use volna_core::app::{Action, App, ClockCommand, Command, EditTarget, Event};
use volna_core::data::Member;
use volna_core::data::transactions::TrackRef;
use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::panels::{Layout, PanelId, PanelsCommand};
use volna_core::scene::MonoMeasure;
use volna_core::session::{OpenSpec, Session};
use volna_core::table::TableCommand;
use volna_core::table::columns::TransactionColumn;
use volna_core::transaction::TransactionCommand;
use volna_core::wave::PointerEvent;
use volna_core::workspace::Workspace;
use volna_core::{Instant, Theme};

const TRACE: &str = "file:///tmp/landing.vtr";
const LOCATION: &str = "file:///tmp/landing.vtr.volna.json";

fn landing_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples/landing.vtr")
}

fn landing() -> Arc<dyn Session> {
    OpenSpec::Path(landing_path()).open().unwrap()
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
}

fn frame(app: &mut App, id: PanelId) {
    let theme = Theme::one_dark();
    if app
        .layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 640.0), &theme)
        .is_some()
    {
        app.render_panel(id, &theme, &mut MonoMeasure);
    }
}

fn opened(session: Arc<dyn Session>) -> App {
    let mut app = App::new();
    app.set_session(session);
    pump(&mut app);
    app
}

/// What undo must restore exactly: the workspace capture without
/// navigation (focus, active tabs, sash sizes, viewports, cursors,
/// scrolling, column widths, selection, folds, the selected clock,
/// pipeline row zoom, transaction sections) or chrome (the sidebar).
fn projection(app: &App) -> Value {
    let bytes = Workspace::capture(app, volna_core::testing::paths("trace.vtr"), None)
        .unwrap()
        .to_bytes()
        .unwrap();
    let mut v: Value = serde_json::from_slice(&bytes).unwrap();
    let o = v.as_object_mut().unwrap();
    for key in ["focused", "sidebar"] {
        o.remove(key);
    }
    let shared = o["shared"].as_object_mut().unwrap();
    shared.remove("viewport");
    shared.remove("cursor");
    shared.remove("reference");
    strip_layout(&mut o["layout"]);
    for panel in o["panels"].as_array_mut().unwrap() {
        let p = panel.as_object_mut().unwrap();
        for key in [
            "link",
            "viewport",
            "cursor",
            "scroll_y",
            "selected",
            "follow",
            "row_cap",
            "label_width",
            "collapsed",
        ] {
            p.remove(key);
        }
        match p["kind"].as_str().unwrap() {
            "waves" => {
                p.remove("columns");
                strip_folds(&mut p["rows"]);
            }
            "pipeline" => _ = p.remove("rows"),
            _ => {}
        }
        if let Some(clocks) = p.get_mut("clocks").and_then(Value::as_object_mut) {
            clocks.remove("selected");
            if clocks.is_empty() {
                p.remove("clocks");
            }
        }
    }
    v
}

fn strip_layout(layout: &mut Value) {
    let o = layout.as_object_mut().unwrap();
    o.remove("active");
    o.remove("sizes");
    if let Some(children) = o.get_mut("children") {
        for child in children.as_array_mut().unwrap() {
            strip_layout(child);
        }
    }
}

fn strip_folds(rows: &mut Value) {
    for row in rows.as_array_mut().unwrap() {
        let o = row.as_object_mut().unwrap();
        o.remove("collapsed");
        if let Some(rows) = o.get_mut("rows") {
            strip_folds(rows);
        }
    }
}

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

/// Where the model-based test stands: the projection at every undo depth
/// (counted from the start of the history, evicted steps included).
struct Oracle {
    states: Vec<Value>,
    revision: u64,
    last: Value,
}

impl Oracle {
    fn new(app: &App) -> Self {
        let p = projection(app);
        Self {
            states: vec![p.clone()],
            revision: app.history.revision(),
            last: p,
        }
    }

    fn depth(app: &App) -> usize {
        app.history.evicted() as usize + app.history.undo_steps().count()
    }

    /// Check the state after an input of `kind`.
    fn check(&mut self, app: &App, kind: Kind, what: &str) {
        let p = projection(app);
        let depth = Self::depth(app);
        let moved = app.history.revision() != self.revision;
        match kind {
            Kind::Undo | Kind::Redo => {
                assert_eq!(
                    p, self.states[depth],
                    "{what}: undo/redo must restore the projection of depth {depth}"
                );
            }
            Kind::Look => {
                assert!(!moved, "{what}: navigation made a step");
                assert_eq!(p, self.last, "{what}: navigation changed the cockpit");
            }
            Kind::Edit => {
                if moved {
                    assert!(depth <= self.states.len(), "{what}: skipped a depth");
                    if depth > 0 {
                        assert_ne!(
                            Some(&p),
                            self.states.get(depth - 1),
                            "{what}: a step that changes nothing ({:?})",
                            app.undo_label()
                        );
                    }
                    self.states.truncate(depth);
                    self.states.push(p.clone());
                    assert!(!app.history.can_redo(), "{what}: a new step keeps redo");
                } else {
                    assert_eq!(p, self.last, "{what}: changed the cockpit without a step");
                }
            }
        }
        let history = &app.history;
        assert!(
            history.bytes() <= history.max_bytes() || history.undo_steps().count() <= 1,
            "{what}: the journal holds {} bytes over its cap",
            history.bytes()
        );
        self.revision = history.revision();
        self.last = p;
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Edit,
    Look,
    Undo,
    Redo,
}

/// The fixture's catalogue, for choosing arguments.
struct Catalog {
    vars: usize,
    scopes: usize,
    generators: Vec<Member>,
    tracks: Vec<TrackRef>,
    clocks: Vec<String>,
}

impl Catalog {
    fn of(app: &App) -> Self {
        let session = app.doc.session(TraceId::A).unwrap();
        let h = session.hierarchy();
        Self {
            vars: h.vars.len(),
            scopes: h.scopes.len(),
            generators: (0..h.generators.len()).map(Member::Generator).collect(),
            tracks: h.generators.iter().map(|g| g.track).collect(),
            clocks: app.doc.clocks.iter().map(|c| c.path.clone()).collect(),
        }
    }
}

struct Driver {
    app: App,
    rng: Rng,
    now: Instant,
    catalog: Catalog,
}

impl Driver {
    fn new(seed: u64) -> Self {
        let app = opened(landing());
        let catalog = Catalog::of(&app);
        Self {
            app,
            rng: Rng(seed),
            now: Instant::now(),
            catalog,
        }
    }

    fn send(&mut self, command: Command) {
        // Mostly slow input; now and then quick repeats that may merge.
        let ms = if self.rng.chance(3) { 200 } else { 1500 };
        self.now += Duration::from_millis(ms);
        self.app.handle_at(command, self.now);
        pump(&mut self.app);
        let focused = self.app.panels.focused_id();
        frame(&mut self.app, focused);
    }

    fn focused(&self) -> PanelId {
        self.app.panels.focused_id()
    }

    fn pointer(&mut self, event: PointerEvent) {
        let id = self.focused();
        self.send(Command::Pointer(id, event));
    }

    /// Select some visible rows of the focused wave panel (navigation).
    fn select_rows(&mut self) -> bool {
        let id = self.focused();
        let n = self.rng.below(3) + 1;
        let Some(w) = self.app.panels.waves_mut(id) else {
            return false;
        };
        let visible = w.visible();
        if visible.is_empty() {
            return false;
        }
        w.selected.clear();
        for _ in 0..n {
            let k = (self.rng.next() % visible.len() as u64) as usize;
            w.selected.insert(visible[k] as usize);
        }
        w.anchor = w.selected.first().copied();
        true
    }

    /// Where the name of visible row `pos` is, as of the last layout.
    fn name_point(&self, pos: usize, frac: f32) -> Option<Point> {
        let w = self.app.panels.waves(self.focused())?;
        let l = w.last_layout();
        if pos >= l.visible.len() {
            return None;
        }
        let y = l.row_y(pos);
        (y >= l.names.top() && y + l.row_h < l.names.bottom())
            .then(|| point(l.names.left() + 60.0, y + l.row_h * frac))
    }

    /// Press a row's name, drag it to another row, then release or press
    /// Esc. One gesture, so one step at most.
    fn drag_rows(&mut self, cancel: bool) -> bool {
        let visible = self
            .app
            .panels
            .waves(self.focused())
            .map_or(0, |w| w.last_layout().visible.len());
        if visible < 2 {
            return false;
        }
        let from = self.rng.below(visible);
        let to = self.rng.below(visible);
        let (Some(a), Some(b)) = (self.name_point(from, 0.5), self.name_point(to, 0.8)) else {
            return false;
        };
        self.pointer(PointerEvent::Down {
            position: a,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        });
        self.pointer(PointerEvent::Move {
            position: point(a.x, a.y + 6.0),
        });
        self.pointer(PointerEvent::Move { position: b });
        if cancel {
            self.send(Command::Action(Action::ClearSelection));
        }
        self.pointer(PointerEvent::Up);
        true
    }

    /// Drag a marker chip some way along its lane, or only click it, then
    /// release or cancel with Esc: one step, or none.
    fn drag_marker(&mut self, cancel: bool) -> bool {
        let Some(lane) = self
            .app
            .panels
            .get(self.focused())
            .and_then(|p| p.kind.marker_lane())
        else {
            return false;
        };
        let chips: Vec<_> = lane.chips.iter().filter(|c| !c.is_cluster()).collect();
        if chips.is_empty() {
            return false;
        }
        let chip = chips[self.rng.below(chips.len())].rect;
        let a = point(chip.left() + 2.0, chip.top() + chip.height() / 2.0);
        let dx = self.rng.below(240) as f32 - 120.0;
        self.pointer(PointerEvent::Down {
            position: a,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        });
        self.pointer(PointerEvent::Move {
            position: point(a.x + dx / 2.0, a.y),
        });
        self.pointer(PointerEvent::Move {
            position: point(a.x + dx, a.y),
        });
        if cancel {
            self.send(Command::Action(Action::ClearSelection));
        }
        self.pointer(PointerEvent::Up);
        true
    }

    /// Drag a row's bottom edge down a few rows and back or not, then
    /// release or cancel with Esc.
    fn drag_height(&mut self, cancel: bool) -> bool {
        let Some(w) = self.app.panels.waves(self.focused()) else {
            return false;
        };
        let l = w.last_layout();
        if l.visible.is_empty() {
            return false;
        }
        let pos = self.rng.below(l.visible.len().min(4));
        let bottom = l.row_y(pos) + l.row_height(pos);
        let row_h = l.row_h;
        let x = l.names.left() + 60.0;
        if bottom + 4.0 * row_h > l.names.bottom() {
            return false;
        }
        self.pointer(PointerEvent::Down {
            position: point(x, bottom - 1.0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        });
        let grow = (self.rng.below(3) + 1) as f32;
        self.pointer(PointerEvent::Move {
            position: point(x, bottom + grow * row_h),
        });
        if self.rng.chance(3) {
            // Back where it started: no step.
            self.pointer(PointerEvent::Move {
                position: point(x, bottom - 1.0),
            });
        }
        if cancel {
            self.send(Command::Action(Action::ClearSelection));
        }
        self.pointer(PointerEvent::Up);
        true
    }

    /// A second run of the design as trace B: it joins, closes (with its
    /// rows and panels), is renamed, and has rows and pipelines of its own.
    fn trace_input(&mut self) -> (Kind, String) {
        let b = TraceId::new(1).unwrap();
        if self.app.doc.traces().get(b).is_none() {
            return (
                Kind::Edit,
                self.run(Command::AddTrace(OpenSpec::Path(landing_path()))),
            );
        }
        fn in_b<T>(item: T) -> volna_core::trace::Traced<T> {
            volna_core::trace::Traced::new(TraceId::new(1).unwrap(), item)
        }
        let command = match self.rng.below(8) {
            0 => Command::RemoveTrace(b),
            // Refused while B is open: A holds the workspace.
            1 => Command::RemoveTrace(TraceId::A),
            2 => Command::RenameTrace(
                b,
                self.rng
                    .chance(3)
                    .then(|| format!("run {}", self.rng.below(3))),
            ),
            3..=4 => {
                let vars = (0..2).map(|_| in_b(self.rng.below(self.catalog.vars)));
                Command::AddVars(vars.collect())
            }
            5 => {
                let g = self.catalog.generators[self.rng.below(self.catalog.generators.len())];
                Command::AddToWaves(vec![in_b(g)])
            }
            6 => {
                let track = self.catalog.tracks[self.rng.below(self.catalog.tracks.len())];
                Command::OpenPipeline { track: in_b(track) }
            }
            _ => Command::OpenTable {
                selected: vec![
                    a(Member::Var(self.rng.below(self.catalog.vars))),
                    in_b(Member::Var(self.rng.below(self.catalog.vars))),
                ],
                clicked: None,
            },
        };
        (Kind::Edit, self.run(command))
    }

    /// One random input; returns its kind and a description.
    fn input(&mut self) -> (Kind, String) {
        if self.rng.chance(12) {
            return self.trace_input();
        }
        let r = self.rng.below(100);
        let id = self.focused();
        let action = |a: Action| (Kind::Edit, Command::Action(a));
        let (kind, command) = match r {
            0..=17 => return (Kind::Undo, self.run(Command::Undo)),
            18..=24 => return (Kind::Redo, self.run(Command::Redo)),
            25..=28 => {
                let n = self.rng.below(3) + 1;
                let vars: Vec<usize> = (0..n).map(|_| self.rng.below(self.catalog.vars)).collect();
                (Kind::Edit, Command::AddVars(a_all(vars)))
            }
            29..=30 => {
                let g = self.catalog.generators[self.rng.below(self.catalog.generators.len())];
                (Kind::Edit, Command::AddToWaves(a_all(vec![g])))
            }
            31 => (
                Kind::Edit,
                Command::AddScopeAsGroup {
                    scope: a(self.rng.below(self.catalog.scopes)),
                    recursive: self.rng.chance(2),
                },
            ),
            32..=37 => {
                self.select_rows();
                return (Kind::Look, "select rows".into());
            }
            38..=39 => action(Action::RemoveSelected),
            40 => action(Action::CutSignals),
            41 => (Kind::Look, Command::Action(Action::CopySignals)),
            42 => action(Action::PasteSignals),
            43..=45 => action(Action::CycleFormat),
            46 => action(Action::ToggleAnalog),
            47 => action(Action::IncreaseRowHeight),
            48 => action(Action::DecreaseRowHeight),
            49 => action(Action::ResetRowHeight),
            50..=51 => {
                let label = self.run(Command::Action(Action::GroupSelection));
                if self
                    .app
                    .panels
                    .waves(id)
                    .is_some_and(|w| w.rename.is_some())
                {
                    let name = self
                        .rng
                        .chance(2)
                        .then(|| format!("g{}", self.rng.below(9)));
                    // Naming later still joins the group's step.
                    self.now += Duration::from_secs(5);
                    return (
                        Kind::Edit,
                        self.run(Command::CommitText(
                            self.app.text_edit().expect("the name field is open").target,
                            name,
                        )) + &label,
                    );
                }
                return (Kind::Edit, label);
            }
            52 => action(Action::Ungroup),
            53 => (Kind::Look, Command::Action(Action::FoldGroupDeep)),
            54 => (Kind::Look, Command::Action(Action::UnfoldGroupDeep)),
            55..=56 => {
                // Half the time on a marker, where M opens its name field.
                let on_marker = self.rng.below(self.app.doc.markers().len() * 2);
                let t = match self.app.doc.markers().get(on_marker) {
                    Some(m) => m.time,
                    None => self.rng.below(2000) as u64 * 10,
                };
                if let Some(nav) = self.app.panels.focused_mut().kind.nav_mut() {
                    nav.set_cursor(&mut self.app.doc, Some(t));
                }
                let label = self.run(Command::Action(Action::AddOrRenameMarker));
                let Some(edit) = self
                    .app
                    .text_edit()
                    .filter(|e| matches!(e.target, EditTarget::Marker { .. }))
                else {
                    return (Kind::Edit, label);
                };
                let name = match self.rng.below(4) {
                    0 => None,
                    1 => Some(" ".into()),
                    n => Some(format!("m{n}")),
                };
                return (
                    Kind::Edit,
                    self.run(Command::CommitText(edit.target, name)) + &label,
                );
            }
            57 => action(if self.rng.chance(2) {
                Action::RemoveMarkerAtCursor
            } else {
                Action::RemoveAllMarkers
            }),
            58..=59 if !self.catalog.clocks.is_empty() => {
                let path = self.catalog.clocks[self.rng.below(self.catalog.clocks.len())].clone();
                (
                    Kind::Edit,
                    Command::Clocks(ClockCommand::ToggleRuler(a(path))),
                )
            }
            60 => action(Action::ToggleCycleOrigin),
            61 => action(Action::SplitRight),
            62 => action(Action::SplitDown),
            63 => action(Action::NewPanel),
            // Closing a lone start panel closes the trace; not here.
            64..=65 if !self.app.panels.focused().kind.is_start() => action(Action::ClosePanel),
            66 => (Kind::Edit, Command::Panels(PanelsCommand::CloseOthers(id))),
            67..=68 => (Kind::Look, Command::Action(Action::FocusNextPanel)),
            69..=70 => {
                let track = self.catalog.tracks[self.rng.below(self.catalog.tracks.len())];
                (Kind::Edit, Command::OpenPipeline { track: a(track) })
            }
            71 => {
                let vars: Vec<Member> = (0..2)
                    .map(|_| Member::Var(self.rng.below(self.catalog.vars)))
                    .collect();
                (
                    Kind::Edit,
                    Command::OpenTable {
                        selected: a_all(vars),
                        clicked: None,
                    },
                )
            }
            72 => {
                let g = self.catalog.generators[self.rng.below(self.catalog.generators.len())];
                (
                    Kind::Edit,
                    Command::OpenTable {
                        selected: a_all(vec![g]),
                        clicked: None,
                    },
                )
            }
            73..=74 => {
                let command = match self.rng.below(3) {
                    0 => TableCommand::ToggleSignalColumn(self.rng.below(3)),
                    1 => TableCommand::ToggleTransactionColumn(
                        TransactionColumn::ALL[self.rng.below(TransactionColumn::ALL.len())],
                    ),
                    _ => TableCommand::ResetColumns,
                };
                (Kind::Edit, Command::Table(id, command))
            }
            75..=76 => return (Kind::Edit, self.show_record()),
            77 => (
                Kind::Edit,
                Command::Transaction(id, TransactionCommand::Pin(self.rng.chance(2))),
            ),
            78 => {
                let key = ["addr", "data", "pc"][self.rng.below(3)].to_owned();
                (
                    Kind::Edit,
                    Command::Transaction(id, TransactionCommand::Radix(key)),
                )
            }
            79..=80 => {
                let title = self
                    .rng
                    .chance(3)
                    .then(|| format!("p{}", self.rng.below(5)));
                (
                    Kind::Edit,
                    Command::Panels(PanelsCommand::Rename(id, title)),
                )
            }
            81 => return (Kind::Edit, self.rearrange()),
            82..=84 => {
                let cancel = self.rng.chance(4);
                self.drag_rows(cancel);
                return (Kind::Edit, format!("drag rows (cancel {cancel})"));
            }
            85..=86 => {
                let cancel = self.rng.chance(4);
                self.drag_marker(cancel);
                return (Kind::Edit, format!("drag marker (cancel {cancel})"));
            }
            87..=89 => {
                let cancel = self.rng.chance(4);
                self.drag_height(cancel);
                return (Kind::Edit, format!("drag height (cancel {cancel})"));
            }
            90..=92 => (Kind::Look, Command::Action(Action::ZoomIn)),
            93 => (Kind::Look, Command::Action(Action::PanLeft)),
            94 => (Kind::Look, Command::Action(Action::ToggleViewportLink)),
            95 => (Kind::Look, Command::ToggleSidebar),
            96 => (Kind::Look, Command::SetFilter("c".into())),
            97 => {
                let walk = match self.rng.below(4) {
                    0 => Action::NextMarker,
                    1 => Action::PrevMarker,
                    2 => Action::JumpBack,
                    n => Action::GoToMarker(volna_core::marker::MarkerId::new(n as u32).unwrap()),
                };
                (Kind::Look, Command::Action(walk))
            }
            // The reference is navigation state: never a step.
            98 => {
                let measure = match self.rng.below(3) {
                    0 => Action::SetReference,
                    1 => Action::ClearReference,
                    _ => Action::ZoomToMeasurement,
                };
                (Kind::Look, Command::Action(measure))
            }
            _ => (Kind::Look, Command::Action(Action::ZoomFit)),
        };
        (kind, self.run(command))
    }

    fn run(&mut self, command: Command) -> String {
        let text = format!("{command:?}");
        self.send(command);
        text
    }

    /// Select a record of a resident generator (the first by identity, so a
    /// seed replays the same inputs) and show it in a transaction panel.
    fn show_record(&mut self) -> String {
        let Some(generator) = self
            .app
            .doc
            .resident_generators(TraceId::A)
            .min_by_key(|g| g.generator().0)
            .cloned()
        else {
            return "no record".into();
        };
        let txs = generator.transactions();
        if txs.is_empty() {
            return "no record".into();
        }
        let tx = &txs[self.rng.below(txs.len())];
        let id = self.focused();
        self.send(Command::SelectTransaction {
            panel: id,
            track: a(tx.generator),
            id: tx.id,
            cursor: None,
        });
        self.run(Command::ShowTransaction { from: id })
    }

    /// Reverse the tabs of the focused panel's group, as the dock would.
    fn rearrange(&mut self) -> String {
        let mut layout = self.app.panels.layout().clone();
        fn reverse(layout: &mut Layout) -> bool {
            match layout {
                Layout::Tabs { tabs, .. } if tabs.len() > 1 => {
                    tabs.reverse();
                    true
                }
                Layout::Tabs { .. } => false,
                Layout::Split { children, .. } => children.iter_mut().any(reverse),
            }
        }
        if !reverse(&mut layout) {
            return "nothing to rearrange".into();
        }
        let from_revision = self.app.panels.revision();
        self.run(Command::Panels(PanelsCommand::SetLayout {
            layout,
            from_revision,
        }))
    }
}

/// Runs `steps` random inputs; returns the labels of the steps they made.
fn model_based(
    seed: u64,
    steps: usize,
    max_bytes: Option<usize>,
) -> std::collections::BTreeSet<String> {
    let mut d = Driver::new(seed);
    if let Some(max) = max_bytes {
        d.app.history.set_max_bytes(max);
    }
    let mut oracle = Oracle::new(&d.app);
    let mut labels = std::collections::BTreeSet::new();
    for step in 0..steps {
        let (kind, what) = d.input();
        let what = format!("seed {seed} step {step}: {what}");
        oracle.check(&d.app, kind, &what);
        labels.extend(d.app.undo_label().map(str::to_owned));
    }
    // Everything undoes back to the oldest kept step, and redoes again.
    while d.app.can_undo() {
        d.send(Command::Undo);
        oracle.check(&d.app, Kind::Undo, &format!("seed {seed}: unwinding"));
    }
    while d.app.can_redo() {
        d.send(Command::Redo);
        oracle.check(&d.app, Kind::Redo, &format!("seed {seed}: replaying"));
    }
    labels
}

/// `VOLNA_UNDO_SEEDS=n` runs `n` more seeds for exploration.
#[test]
fn random_commands_undo_and_redo_to_every_earlier_cockpit() {
    let more: u64 = std::env::var("VOLNA_UNDO_SEEDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let mut labels = std::collections::BTreeSet::new();
    for seed in [1, 7, 42, 1234, 99991].into_iter().chain(100..100 + more) {
        labels.extend(model_based(seed, 400, None));
    }
    // The second trace joined, was renamed and closed with what showed it.
    for step in ["Add trace B", "Rename trace B", "Close trace B"] {
        assert!(labels.contains(step), "no {step:?} step in {labels:?}");
    }
}

#[test]
fn a_small_cap_evicts_the_oldest_steps_and_keeps_the_newest() {
    for seed in [3, 5] {
        model_based(seed, 300, Some(12 * 1024));
    }
    let mut d = Driver::new(11);
    d.app.history.set_max_bytes(1);
    d.send(Command::AddVars(a_all(vec![0, 1, 2])));
    d.send(Command::AddVars(a_all(vec![3])));
    assert_eq!(
        d.app.history.undo_steps().count(),
        1,
        "the newest step stays"
    );
    assert_eq!(d.app.undo_label(), Some("Add 1 signal"));
    assert!(d.app.history.evicted() >= 1);
}

// -- focused behaviour --------------------------------------------------------

/// The landing trace with four signals in one wave panel.
fn four_rows() -> (App, PanelId, Instant) {
    let mut app = opened(landing());
    let now = Instant::now();
    app.handle_at(Command::AddVars(a_all(vec![0, 1, 2, 3])), now);
    pump(&mut app);
    let id = app.panels.focused_id();
    frame(&mut app, id);
    (app, id, now)
}

fn select(app: &mut App, id: PanelId, rows: &[usize]) {
    let w = app.panels.waves_mut(id).unwrap();
    w.selected = rows.iter().copied().collect();
    w.anchor = rows.first().copied();
}

fn names(app: &App, id: PanelId) -> Vec<String> {
    app.panels
        .waves(id)
        .unwrap()
        .items()
        .iter()
        .map(|e| format!("{}{}", " ".repeat(e.depth.into()), e.name()))
        .collect()
}

/// What undo and redo announced since the last call.
fn notices(app: &mut App) -> Vec<String> {
    app.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Announce(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn repeated_format_presses_merge_within_the_window_only() {
    let (mut app, id, now) = four_rows();
    select(&mut app, id, &[1]);
    let steps = |app: &App| app.history.undo_steps().count();
    let before = steps(&app);
    let format = |app: &App| {
        app.panels.waves(id).unwrap().items()[1]
            .signal()
            .unwrap()
            .format_id()
    };
    let original = format(&app);
    let mut t = now + Duration::from_secs(10);
    app.handle_at(Command::Action(Action::CycleFormat), t);
    t += Duration::from_millis(400);
    app.handle_at(Command::Action(Action::CycleFormat), t);
    assert_eq!(steps(&app), before + 1, "TT within a second is one step");
    t += Duration::from_millis(1500);
    app.handle_at(Command::Action(Action::CycleFormat), t);
    assert_eq!(steps(&app), before + 2, "a pause of 1.5 s makes two");
    assert!(app.undo_label().unwrap().starts_with("Format "));
    app.handle(Command::Undo);
    app.handle(Command::Undo);
    assert_eq!(format(&app), original);
    // Another selection is another target: no merge.
    app.handle_at(Command::Action(Action::CycleFormat), t);
    select(&mut app, id, &[2]);
    app.handle_at(
        Command::Action(Action::CycleFormat),
        t + Duration::from_millis(100),
    );
    assert_eq!(steps(&app), before + 2);
}

#[test]
fn a_new_group_and_its_name_are_one_step() {
    let (mut app, id, now) = four_rows();
    select(&mut app, id, &[1, 2]);
    let steps = app.history.undo_steps().count();
    app.handle_at(Command::Action(Action::GroupSelection), now);
    assert_eq!(app.undo_label(), Some("Group 2 rows"));
    // Typing the name takes a while; it still joins the group's step.
    app.handle_at(
        Command::CommitText(app.text_edit().unwrap().target, Some("bus".into())),
        now + Duration::from_secs(30),
    );
    assert_eq!(app.history.undo_steps().count(), steps + 1);
    assert_eq!(app.undo_label(), Some("Group 2 rows"));
    assert!(names(&app, id).contains(&"bus".to_owned()));
    app.handle(Command::Undo);
    assert_eq!(names(&app, id).len(), 4);
    assert!(names(&app, id).iter().all(|n| !n.starts_with(' ')));
    app.handle(Command::Redo);
    assert!(names(&app, id).contains(&"bus".to_owned()));
}

#[test]
fn a_drag_is_one_step_esc_leaves_none_and_back_to_start_leaves_none() {
    let (mut app, id, now) = four_rows();
    let steps = |app: &App| app.history.undo_steps().count();
    let at = |app: &App, pos: usize, frac: f32| {
        let l = app.panels.waves(id).unwrap().last_layout();
        point(l.names.left() + 60.0, l.row_y(pos) + l.row_h * frac)
    };
    let edge = |app: &App, pos: usize| {
        let l = app.panels.waves(id).unwrap().last_layout();
        point(
            l.names.left() + 60.0,
            l.row_y(pos) + l.row_height(pos) - 1.0,
        )
    };
    let press = |app: &mut App, p: Point| {
        app.handle_at(
            Command::Pointer(
                id,
                PointerEvent::Down {
                    position: p,
                    button: MouseButton::Left,
                    modifiers: Modifiers::default(),
                },
            ),
            now,
        );
    };
    let to = |app: &mut App, p: Point| {
        app.handle_at(
            Command::Pointer(id, PointerEvent::Move { position: p }),
            now,
        );
        frame(app, id);
    };
    select(&mut app, id, &[0]);
    let before = steps(&app);
    let original = names(&app, id);

    // Move row 0 below row 2: one step on release.
    let (a, b) = (at(&app, 0, 0.5), at(&app, 2, 0.8));
    press(&mut app, a);
    to(&mut app, point(a.x, a.y + 6.0));
    to(&mut app, b);
    assert_eq!(
        steps(&app),
        before,
        "nothing is recorded before the release"
    );
    app.handle_at(Command::Pointer(id, PointerEvent::Up), now);
    assert_eq!(steps(&app), before + 1);
    assert_eq!(app.undo_label().map(|l| l.starts_with("Move ")), Some(true));
    app.handle(Command::Undo);
    assert_eq!(names(&app, id), original);
    frame(&mut app, id);

    // Resize a row and cancel with Esc: the height comes back, no step.
    let heights = |app: &App| -> Vec<u8> {
        app.panels
            .waves(id)
            .unwrap()
            .items()
            .iter()
            .map(|e| e.height().multiple())
            .collect()
    };
    let flat = heights(&app);
    let redo = app.history.can_redo();
    let e = edge(&app, 1);
    press(&mut app, e);
    to(&mut app, point(e.x, e.y + 60.0));
    assert_ne!(heights(&app), flat, "the drag resizes live");
    app.handle_at(Command::Action(Action::ClearSelection), now);
    assert_eq!(heights(&app), flat, "Esc rolls the gesture back");
    app.handle_at(Command::Pointer(id, PointerEvent::Up), now);
    frame(&mut app, id);
    assert_eq!(steps(&app), before);
    assert_eq!(
        app.history.can_redo(),
        redo,
        "a cancelled gesture keeps redo"
    );

    // ⌘Z during a drag rolls it back too, and does not undo more.
    let e = edge(&app, 1);
    press(&mut app, e);
    to(&mut app, point(e.x, e.y + 60.0));
    app.handle_at(Command::Undo, now);
    assert_eq!(heights(&app), flat);
    assert_eq!(steps(&app), before);
    app.handle_at(Command::Pointer(id, PointerEvent::Up), now);
    frame(&mut app, id);

    // Out and back: no step.
    let e = edge(&app, 1);
    press(&mut app, e);
    to(&mut app, point(e.x, e.y + 60.0));
    to(&mut app, e);
    app.handle_at(Command::Pointer(id, PointerEvent::Up), now);
    assert_eq!(heights(&app), flat);
    assert_eq!(steps(&app), before);
}

#[test]
fn undo_restores_the_selection_focus_and_announces_the_label() {
    let (mut app, id, _) = four_rows();
    let title = app.panels.get(id).unwrap().title();
    select(&mut app, id, &[1, 2]);
    app.handle(Command::Action(Action::RemoveSelected));
    assert_eq!(app.undo_label(), Some("Remove 2 rows"));
    // Focus moves away; undo brings it back with the rows selected.
    app.handle(Command::Action(Action::SplitRight));
    let split = app.panels.focused_id();
    assert_ne!(split, id);
    app.handle(Command::Undo);
    assert_eq!(app.panels.focused_id(), id, "undoing the split refocuses");
    notices(&mut app);
    app.handle(Command::Undo);
    assert_eq!(notices(&mut app), ["Undid Remove 2 rows"]);
    assert_eq!(app.panels.focused_id(), id);
    let w = app.panels.waves(id).unwrap();
    assert_eq!(w.items().len(), 4);
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(app.redo_label(), Some("Remove 2 rows"));
    app.handle(Command::Redo);
    assert_eq!(notices(&mut app), ["Redid Remove 2 rows"]);
    assert_eq!(
        app.status().announcement.as_deref(),
        Some("Redid Remove 2 rows")
    );
    assert!(app.panels.waves(id).unwrap().selected.is_empty());
    app.handle(Command::Redo);
    assert_eq!(notices(&mut app), [format!("Redid Split {title}")]);
    assert_eq!(
        app.panels.focused_id(),
        split,
        "redoing the split focuses it"
    );
    // Nothing left.
    app.handle(Command::Redo);
    assert_eq!(notices(&mut app), ["Nothing to redo"]);
    // The next edit clears the status line.
    app.handle(Command::AddVars(a_all(vec![5])));
    assert_eq!(app.status().announcement, None);
}

#[test]
fn navigation_never_makes_a_step_and_undo_keeps_the_time_axis() {
    let (mut app, id, _) = four_rows();
    // A marker to walk to, away from the cursor.
    app.doc.shared.cursor = Some(5);
    app.handle(Command::Action(Action::AddOrRenameMarker));
    app.doc.shared.cursor = Some(0);
    select(&mut app, id, &[0]);
    app.handle(Command::Action(Action::RemoveSelected));
    let revision = app.history.revision();
    for action in [
        Action::ZoomIn,
        Action::PanLeft,
        Action::GoToStart,
        Action::FocusNextPanel,
        Action::SelectAll,
        Action::ClearSelection,
        Action::ToggleCursorLink,
        Action::NextMarker,
        Action::PrevMarker,
        Action::GoToMarker(volna_core::marker::MarkerId::new(1).unwrap()),
        Action::JumpBack,
    ] {
        app.handle(Command::Action(action));
    }
    app.handle(Command::ToggleSidebar);
    app.handle(Command::SetFilter("clk".into()));
    assert_eq!(app.history.revision(), revision);
    // Undo after zooming: rows change, the viewport does not.
    let vp = app.panels.waves(id).unwrap().viewport(&app.doc);
    app.handle(Command::Undo);
    assert_eq!(app.panels.waves(id).unwrap().items().len(), 4);
    assert_eq!(app.panels.waves(id).map(|w| w.viewport(&app.doc)), Some(vp));
}

#[test]
fn removed_rows_come_back_sharing_resident_histories() {
    let (mut app, id, _) = four_rows();
    let history_of = |app: &App, row: usize| {
        app.panels.waves(id).unwrap().items()[row]
            .signal()
            .unwrap()
            .history
            .clone()
            .unwrap()
    };
    let kept = history_of(&app, 0);
    // A second panel keeps row 0's history resident.
    app.handle(Command::Action(Action::SplitDown));
    app.handle(Command::Panels(PanelsCommand::Focus(id)));
    select(&mut app, id, &[0]);
    app.handle(Command::Action(Action::RemoveSelected));
    app.handle(Command::Undo);
    assert!(
        Arc::ptr_eq(&history_of(&app, 0), &kept),
        "undo shares the resident history"
    );
    assert!(
        app.take_requests().is_empty(),
        "no load for a resident signal"
    );

    // With no copy left, undo queues one load and shows loading.
    app.handle(Command::Undo);
    assert_eq!(app.panels.len(), 1);
    select(&mut app, id, &[3]);
    app.handle(Command::Action(Action::RemoveSelected));
    pump(&mut app);
    app.handle(Command::Undo);
    let row = app.panels.waves(id).unwrap().items()[3].signal().unwrap();
    assert!(
        row.history.is_none() && row.error.is_none(),
        "loading again"
    );
    assert!(!app.take_requests().is_empty());
}

#[test]
fn an_add_undone_before_its_load_ignores_the_late_delivery() {
    let mut app = opened(landing());
    app.handle(Command::AddVars(a_all(vec![5, 6])));
    let requests = app.take_requests();
    assert!(!requests.is_empty());
    let id = app.panels.focused_id();
    app.handle(Command::Undo);
    for request in requests {
        app.deliver(request.perform());
    }
    assert!(app.panels.get(id).is_none(), "the start panel came back");
    // Redo during pending loads re-adds the rows, which load.
    app.handle(Command::Redo);
    pump(&mut app);
    let w = app.panels.waves(id).unwrap();
    assert_eq!(w.items().len(), 2);
    assert_eq!(w.loaded_count(), 2);
}

#[test]
fn closing_a_table_returns_its_memory_and_undo_reserves_it_again() {
    let mut app = opened(landing());
    let budget = |app: &App| app.status().memory.unwrap().used;
    app.handle(Command::AddVars(a_all(vec![0])));
    pump(&mut app);
    let base = budget(&app);
    app.handle(Command::OpenTable {
        selected: a_all(vec![Member::Var(0), Member::Var(1)]),
        clicked: None,
    });
    pump(&mut app);
    let table = app.panels.focused_id();
    assert!(app.panels.get(table).unwrap().kind.table().is_some());
    let open = budget(&app);
    assert!(open >= base + volna_core::table::model::PANEL_BYTES);
    app.handle(Command::Action(Action::ClosePanel));
    pump(&mut app);
    assert!(
        budget(&app) <= open - volna_core::table::model::PANEL_BYTES,
        "the journal holds no ledger bytes"
    );
    app.handle(Command::Undo);
    pump(&mut app);
    assert_eq!(
        app.panels.focused_id(),
        table,
        "the table is back under its ID"
    );
    let t = app.panels.get(table).unwrap().kind.table().unwrap();
    assert!(!t.is_empty(), "{}", t.status());
    assert_eq!(budget(&app), open);
}

#[test]
fn a_closed_panel_comes_back_with_its_rows_and_title() {
    let (mut app, id, _) = four_rows();
    app.handle(Command::Action(Action::SplitRight));
    let right = app.panels.focused_id();
    app.handle(Command::Panels(PanelsCommand::Rename(
        right,
        Some("bus".into()),
    )));
    let rows = names(&app, right);
    app.handle(Command::Action(Action::ClosePanel));
    assert!(app.panels.get(right).is_none());
    assert_eq!(app.undo_label(), Some("Close bus"));
    app.handle(Command::Undo);
    pump(&mut app);
    assert_eq!(names(&app, right), rows);
    assert_eq!(app.panels.get(right).unwrap().title(), "bus");
    assert_eq!(app.panels.waves(right).unwrap().loaded_count(), 4);
    // Closing the last content panel leaves a start panel; undo reverses it.
    app.handle(Command::Panels(PanelsCommand::CloseOthers(right)));
    app.handle(Command::Action(Action::ClosePanel));
    assert!(app.panels.focused().kind.is_start());
    app.handle(Command::Undo);
    assert_eq!(app.panels.focused_id(), right);
    app.handle(Command::Undo);
    assert!(app.panels.get(id).is_some());
}

#[test]
fn markers_rulers_titles_columns_and_pins_round_trip() {
    let mut app = opened(landing());
    let catalog = Catalog::of(&app);
    let generator = *catalog
        .generators
        .iter()
        .find(|&&g| app.member_clock(a(g)).is_none())
        .unwrap();
    app.handle(Command::AddVars(a_all(vec![0])));
    let id = app.panels.focused_id();
    let set_cursor = |app: &mut App, t: u64| {
        let App { panels, doc, .. } = app;
        panels.waves_mut(id).unwrap().set_cursor(doc, Some(t));
    };
    for t in [100, 200, 300] {
        set_cursor(&mut app, t);
        app.handle(Command::Action(Action::AddOrRenameMarker));
    }
    assert_eq!(app.undo_label(), Some("Add marker 3"));
    // ⇧M removes the marker at the cursor; its number is free again.
    set_cursor(&mut app, 200);
    app.handle(Command::Action(Action::RemoveMarkerAtCursor));
    assert_eq!(app.undo_label(), Some("Remove marker 2"));
    set_cursor(&mut app, 400);
    app.handle(Command::Action(Action::AddOrRenameMarker));
    assert_eq!(app.undo_label(), Some("Add marker 2"));
    app.handle(Command::Undo);
    app.handle(Command::Undo);
    assert_eq!(
        app.doc
            .markers()
            .iter()
            .map(|m| (m.id.get(), m.time))
            .collect::<Vec<_>>(),
        [(1, 100), (2, 200), (3, 300)],
        "undo restores the removed marker with its number"
    );
    app.handle(Command::Action(Action::RemoveAllMarkers));
    assert_eq!(app.undo_label(), Some("Remove all markers"));
    app.handle(Command::Undo);
    assert_eq!(app.doc.markers().len(), 3);
    // M on a marker opens its name field; the name is one step.
    set_cursor(&mut app, 300);
    app.handle(Command::Action(Action::AddOrRenameMarker));
    assert_eq!(app.doc.markers().len(), 3, "M on a marker adds none");
    let target = app.text_edit().unwrap().target;
    app.handle(Command::CommitText(target, Some("retry".into())));
    assert_eq!(app.undo_label(), Some("Rename marker 3"));
    app.handle(Command::Undo);
    assert_eq!(app.doc.markers()[2].label, None);
    app.handle(Command::Redo);
    assert_eq!(app.doc.markers()[2].label.as_deref(), Some("retry"));

    let clock = catalog.clocks[0].clone();
    let name = clock.rsplit('.').next().unwrap().to_owned();
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(a(clock.clone()))));
    let shown = app.panels.waves(id).unwrap().nav.clocks().rulers.clone();
    assert!(app.undo_label().unwrap().ends_with(&name));
    set_cursor(&mut app, 500);
    app.handle(Command::Action(Action::ToggleCycleOrigin));
    assert_eq!(app.undo_label(), Some("Set cycle origin"));
    app.handle(Command::Undo);
    assert_eq!(app.panels.waves(id).unwrap().nav.clocks().origin, None);
    app.handle(Command::Undo);
    assert_ne!(app.panels.waves(id).unwrap().nav.clocks().rulers, shown);
    app.handle(Command::Redo);
    assert_eq!(app.panels.waves(id).unwrap().nav.clocks().rulers, shown);

    // A table's columns.
    app.handle(Command::OpenTable {
        selected: a_all(vec![generator]),
        clicked: None,
    });
    pump(&mut app);
    let table = app.panels.focused_id();
    let columns = |app: &App| {
        app.panels
            .get(table)
            .unwrap()
            .kind
            .table()
            .unwrap()
            .columns
            .get()
            .clone()
    };
    let before = columns(&app);
    app.handle(Command::Table(
        table,
        TableCommand::ToggleTransactionColumn(TransactionColumn::Status),
    ));
    assert_ne!(columns(&app), before);
    assert_eq!(app.undo_label(), Some("Change columns"));
    app.handle(Command::Undo);
    assert_eq!(columns(&app), before);

    // A transaction panel pinned on a record comes back pinned on it.
    pump(&mut app);
    let track = app
        .doc
        .hierarchy(TraceId::A)
        .unwrap()
        .member_track(generator)
        .unwrap();
    let generator = app.doc.resident_generator(a(track)).unwrap();
    let [first, second] = [&generator.transactions()[0], &generator.transactions()[1]];
    let select = |app: &mut App, tx: &volna_core::data::transactions::Transaction| {
        app.handle(Command::SelectTransaction {
            panel: table,
            track: a(tx.generator),
            id: tx.id,
            cursor: None,
        })
    };
    select(&mut app, first);
    app.handle(Command::ShowTransaction { from: table });
    let tx = app.panels.focused_id();
    app.handle(Command::Transaction(tx, TransactionCommand::Pin(true)));
    assert_eq!(app.undo_label(), Some("Pin transaction"));
    app.handle(Command::Transaction(tx, TransactionCommand::Pin(false)));
    select(&mut app, second);
    let shown = |app: &App| app.panels.transaction(tx).unwrap().shown().unwrap().id;
    assert_eq!(shown(&app), second.id, "unpinned, it follows");
    app.handle(Command::Undo);
    let model = app.panels.transaction(tx).unwrap();
    assert!(model.pinned());
    assert_eq!(shown(&app), first.id, "pinned on the record it froze");
    app.handle(Command::Transaction(
        tx,
        TransactionCommand::Radix("addr".into()),
    ));
    assert_eq!(app.undo_label(), Some("Radix of addr"));
    app.handle(Command::Undo);
    assert!(app.panels.transaction(tx).unwrap().prefs().radix.is_empty());
}

#[test]
fn the_history_is_cleared_with_the_trace_and_never_saved() {
    let (mut app, _, _) = four_rows();
    assert!(app.can_undo());
    let with = Workspace::capture(&app, volna_core::testing::paths(TRACE), None)
        .unwrap()
        .to_bytes()
        .unwrap();
    let revision = app.history.revision();
    let mut plain = App::new();
    plain.set_session(landing());
    plain.handle(Command::AddVars(a_all(vec![0, 1, 2, 3])));
    pump(&mut plain);
    plain.history.clear();
    let without = Workspace::capture(&plain, volna_core::testing::paths(TRACE), None)
        .unwrap()
        .to_bytes()
        .unwrap();
    assert_eq!(with, without, "the workspace file holds no history");

    // Restoring a workspace replaces what the steps name.
    let plan = Workspace::parse(&with)
        .unwrap()
        .prepare(&app, TRACE, LOCATION)
        .unwrap();
    plan.commit(&mut app).unwrap();
    assert!(!app.can_undo() && !app.can_redo());
    assert_ne!(app.history.revision(), revision);

    app.handle(Command::AddVars(a_all(vec![1])));
    assert!(app.can_undo());
    app.handle(Command::CloseTrace);
    assert!(!app.can_undo());
    app.set_session(landing());
    assert!(!app.can_undo());
}

#[test]
fn undo_and_redo_are_named_commands_and_mark_the_workspace_changed() {
    assert_eq!(Command::named("undo"), Some(Command::Undo));
    assert_eq!(Command::named("redo"), Some(Command::Redo));
    let mut app = opened(landing());
    app.handle(Command::Undo);
    assert_eq!(notices(&mut app), ["Nothing to undo"]);
    app.handle(Command::AddVars(a_all(vec![0])));
    assert_eq!(app.undo_label(), Some("Add 1 signal"));
    assert!(app.can_undo() && !app.can_redo());
}
