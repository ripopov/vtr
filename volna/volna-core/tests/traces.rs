//! Several traces in one session (`docs/multiple-traces.html`, stage 1): a
//! VTR in nanoseconds and an FST in picoseconds on one exact timeline, rows
//! of either trace in one panel with their letters, closing one trace with
//! everything that shows it, names, renames, undo, and workspaces that
//! reopen every trace.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use volna_core::Theme;
use volna_core::app::{App, Command, Event};
use volna_core::data::source::Lookup;
use volna_core::data::{Member, VarId};
use volna_core::geometry::Rect;
use volna_core::panels::PanelId;
use volna_core::scene::MonoMeasure;
use volna_core::session::{LoadRequest, OpenSpec};
use volna_core::trace::{TraceId, Traced};
use volna_core::workspace::Workspace;
use volna_core::workspace::persistence::{Candidate, Content, Persistence, Target};

/// The second trace.
fn b() -> TraceId {
    TraceId::from_letter('B').unwrap()
}

fn landing() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples/landing.vtr")
}

/// Five changes of a few kinds, in picoseconds (`fixtures/fst_values.c`).
fn values_fst() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/values.fst")
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
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1100.0, 600.0), &theme)
        .unwrap();
    app.render_panel(id, &theme, &mut MonoMeasure);
}

fn var(app: &App, trace: TraceId, path: &str) -> Traced<VarId> {
    let h = app.doc.hierarchy(trace).expect("open trace");
    let path: Vec<&str> = path.split('.').collect();
    match h.find_var(&path, None) {
        Lookup::Found(id) => Traced::new(trace, id),
        other => panic!("{path:?}: {other:?}"),
    }
}

/// The histories of the first wave panel's signal rows, by name.
fn history(app: &App, trace: TraceId, name: &str) -> Arc<dyn volna_core::data::SignalHistory> {
    let waves = app.panels.first_waves().and_then(|id| app.panels.waves(id));
    waves
        .unwrap()
        .items()
        .iter()
        .filter_map(|e| e.signal())
        .find(|s| s.name == name && s.source.trace() == trace)
        .and_then(|s| s.history.clone())
        .unwrap_or_else(|| panic!("no loaded row {name} of {trace}"))
}

fn two_runs() -> App {
    let mut app = App::new();
    app.handle(Command::Open(OpenSpec::Path(landing())));
    pump(&mut app);
    app.handle(Command::AddTrace(OpenSpec::Path(landing())));
    pump(&mut app);
    app
}

#[test]
fn a_vtr_in_ns_and_an_fst_in_ps_share_one_exact_timeline() {
    let mut app = App::new();
    app.handle(Command::Open(OpenSpec::Path(landing())));
    pump(&mut app);
    let pc = var(&app, TraceId::A, "soc.cpu0.pc");
    app.handle(Command::AddVars(vec![pc]));
    pump(&mut app);
    let raw = history(&app, TraceId::A, "pc");
    let (ns_end, changes) = (app.doc.limits().1, raw.len());
    let third = raw.time(3);
    app.doc.shared.cursor = Some(757);
    let marker = app.doc.add_marker(783).unwrap();
    app.handle(Command::Action(volna_core::Action::NextMarker));
    assert_eq!(app.doc.time_base().timescale, -9);

    // The FST counts picoseconds: the session timeline becomes a thousand
    // times finer, and everything already on it follows exactly.
    app.handle(Command::AddTrace(OpenSpec::Path(values_fst())));
    assert_eq!(app.trace_chips()[1].detail(), "opening…");
    pump(&mut app);
    assert_eq!(app.doc.time_base().timescale, -12);
    assert_eq!(app.doc.shared.cursor, Some(783_000));
    assert_eq!(app.doc.markers()[0].time, 783_000);
    assert_eq!(app.doc.limits(), (0, ns_end * 1000));
    let placed = history(&app, TraceId::A, "pc");
    assert_eq!((placed.len(), placed.time(3)), (changes, third * 1000));
    assert_eq!(placed.index_at(third * 1000 - 1), Some(2));
    assert_eq!(placed.index_at(third * 1000), Some(3));
    let chips = app.trace_chips();
    assert_eq!(
        chips
            .iter()
            .map(|c| (c.trace, c.name.as_str(), c.detail()))
            .collect::<Vec<_>>(),
        [
            (TraceId::A, "landing.vtr", "VTR · ns".to_owned()),
            (b(), "values.fst", "FST · ps".to_owned()),
        ]
    );

    // B's rows keep their own picoseconds.
    let logic = var(&app, b(), "top.logic");
    app.handle(Command::AddVars(vec![logic]));
    pump(&mut app);
    let b = history(&app, b(), "logic");
    assert_eq!((b.time(0), b.time(1)), (5, 10));

    // Undo takes B out and keeps the finer unit: undoing the marker edit
    // made in nanoseconds lands exactly in picoseconds.
    app.handle(Command::Undo);
    assert_eq!(app.undo_label(), Some("Add trace B"));
    app.handle(Command::Undo);
    assert_eq!(app.doc.traces().len(), 1);
    assert_eq!(app.doc.time_base().timescale, -12);
    assert_eq!(app.undo_label(), Some("Add marker 1"));
    app.handle(Command::Undo);
    assert!(app.doc.markers().is_empty());
    app.handle(Command::Redo);
    assert_eq!(app.doc.markers()[0].time, 783_000);
    app.handle(Command::Redo);
    assert!(
        app.take_requests()
            .iter()
            .all(|r| !matches!(r, LoadRequest::Open { .. })),
        "a trace put back is not read again"
    );
    assert_eq!(app.doc.traces().len(), 2);
    assert_eq!(app.doc.markers()[0].id, marker);
}

#[test]
fn rows_of_both_traces_show_their_letters_in_one_panel() {
    let mut app = two_runs();
    app.handle(Command::AddVars(vec![
        var(&app, TraceId::A, "soc.cpu0.pc"),
        var(&app, b(), "soc.cpu0.pc"),
    ]));
    pump(&mut app);
    let id = app.panels.focused_id();
    frame(&mut app, id);
    // The member list marks what is on the waves, per trace.
    let shown = app.members_on_waves();
    let pc = |trace| var(&app, trace, "soc.cpu0.pc").map(Member::Var);
    assert!(shown.contains(&pc(TraceId::A)) && shown.contains(&pc(b())));
    assert_eq!(shown.len(), 2);
    let waves = app.panels.waves(id).unwrap();
    let rows: Vec<_> = waves.accessible_rows(&app.doc).map(|r| r.label).collect();
    assert_eq!(rows, ["pc, trace A", "pc, trace B"]);
    let layout = waves.last_layout();
    assert!(layout.name_left > layout.names.left(), "a trace gutter");
    let letters: Vec<String> = app
        .scene()
        .texts()
        .filter(|t| *t == "A" || *t == "B")
        .map(str::to_owned)
        .collect();
    assert_eq!(letters, ["A", "B"]);

    // The scope tree heads each trace's scopes with a row of its own.
    let roots: Vec<_> = app
        .scopes
        .visible
        .iter()
        .filter(|(node, _)| node.scope.is_none())
        .map(|(node, depth)| (node.trace, *depth))
        .collect();
    assert_eq!(roots, [(TraceId::A, 0), (b(), 0)]);
    app.handle(Command::RevealTrace(b()));
    assert_eq!(
        app.scopes.selected,
        Some(volna_core::sidebar::TreeNode::trace(b()))
    );
    // Searching looks through both traces.
    app.handle(Command::SetSearchEverywhere(true));
    app.handle(Command::SetFilter("resp_valid".into()));
    let found: Vec<_> = app.variables.rows.iter().map(|m| m.trace).collect();
    assert_eq!(found, [TraceId::A, b()]);
}

#[test]
fn closing_a_trace_takes_its_rows_and_panels_in_one_step() {
    let mut app = two_runs();
    let generator = match app
        .doc
        .hierarchy(b())
        .unwrap()
        .find_generator(&["soc", "l2", "bus", "read"])
    {
        Lookup::Found(id) => Member::Generator(id),
        other => panic!("{other:?}"),
    };
    app.handle(Command::AddVars(vec![
        var(&app, TraceId::A, "soc.cpu0.pc"),
        var(&app, b(), "soc.cpu0.pc"),
    ]));
    app.handle(Command::AddToWaves(vec![Traced::new(b(), generator)]));
    let pipeline = app.pipeline_streams()[1].1;
    assert_eq!(pipeline.trace, b());
    app.handle(Command::OpenPipeline { track: pipeline });
    pump(&mut app);
    let waves = app.panels.first_waves().unwrap();
    assert_eq!(app.panels.waves(waves).unwrap().items().len(), 3);
    assert_eq!(app.panels.len(), 2);
    let session = app.doc.session(b()).unwrap().clone();

    let rows = app.rows_revision();
    app.handle(Command::RemoveTrace(b()));
    assert_ne!(
        app.rows_revision(),
        rows,
        "the member list's marks go stale"
    );
    assert_eq!(app.undo_label(), Some("Close trace B"));
    assert_eq!(app.doc.traces().len(), 1);
    assert_eq!(app.panels.len(), 1, "B's pipeline closed with it");
    let items = app.panels.waves(waves).unwrap().items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].row.trace(), Some(TraceId::A));

    // One undo puts back the trace, already open, then its rows and panel.
    app.handle(Command::Undo);
    assert!(Arc::ptr_eq(app.doc.session(b()).unwrap(), &session));
    assert_eq!(app.panels.len(), 2);
    pump(&mut app);
    let items = app.panels.waves(waves).unwrap().items();
    assert_eq!(items.len(), 3);
    assert!(
        items
            .iter()
            .filter_map(|e| e.signal())
            .all(|s| s.history.is_some())
    );
    app.handle(Command::Redo);
    assert_eq!(app.doc.traces().len(), 1);
    assert_eq!(app.panels.len(), 1);

    // A holds the workspace: it closes only with everything.
    app.handle(Command::Undo);
    app.take_events();
    app.handle(Command::RemoveTrace(TraceId::A));
    assert_eq!(app.doc.traces().len(), 2);
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, Event::Notice(n) if n.contains("holds the workspace")))
    );
    assert!(!app.trace_chips()[0].closable);
}

fn copy_to(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::copy(landing(), &path).unwrap();
    path
}

#[test]
fn traces_are_named_by_what_tells_them_apart_and_renames_undo() {
    let dir = tempfile::tempdir().unwrap();
    let base = copy_to(&dir.path().join("runs/base"), "landing.vtr");
    let slow = copy_to(&dir.path().join("runs/dram20"), "landing.vtr");
    let mut app = App::new();
    app.handle(Command::Open(OpenSpec::Path(base)));
    pump(&mut app);
    assert_eq!(app.doc.name().as_deref(), Some("landing.vtr"));
    app.handle(Command::AddTrace(OpenSpec::Path(slow)));
    pump(&mut app);
    let names = |app: &App| {
        app.trace_chips()
            .into_iter()
            .map(|c| c.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&app), ["base", "dram20"]);
    app.handle(Command::RenameTrace(b(), Some("  slow DRAM ".into())));
    assert_eq!(app.undo_label(), Some("Rename trace B"));
    assert_eq!(names(&app), ["base", "slow DRAM"]);
    assert_eq!(app.doc.name().as_deref(), Some("base + slow DRAM"));
    app.handle(Command::Undo);
    assert_eq!(names(&app), ["base", "dram20"]);
    // An empty name goes back to the derived one; nothing to undo then.
    app.handle(Command::Redo);
    app.handle(Command::RenameTrace(b(), Some(" ".into())));
    assert_eq!(names(&app), ["base", "dram20"]);
    assert_eq!(app.undo_label(), Some("Rename trace B"));
}

fn uri(path: &Path) -> String {
    url::Url::from_file_path(path.canonicalize().unwrap())
        .unwrap()
        .to_string()
}

/// Open `trace` as a host with persistence does, restoring `sidecar`.
fn reopen(trace: &Path, sidecar: Content) -> App {
    let mut app = App::new();
    app.configure_persistence(Persistence::Auto);
    app.open_resource(OpenSpec::Path(trace.into()), uri(trace));
    pump(&mut app);
    let trace_uri = uri(trace);
    app.restore_candidates(
        &trace_uri,
        Candidate {
            target: Target::File {
                uri: format!("{trace_uri}.volna.json"),
            },
            content: sidecar,
            writable: true,
        },
        Candidate {
            target: Target::Storage {
                key: trace_uri.clone(),
            },
            content: Content::Missing,
            writable: true,
        },
    );
    pump(&mut app);
    app
}

#[test]
fn a_workspace_reopens_every_trace_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let cpu = copy_to(dir.path(), "cpu.vtr");
    let dram = dir.path().join("dram.fst");
    std::fs::copy(values_fst(), &dram).unwrap();
    let mut app = reopen(&cpu, Content::Missing);
    app.add_resource(OpenSpec::Path(dram.clone()), uri(&dram));
    pump(&mut app);
    app.handle(Command::RenameTrace(b(), Some("dram".into())));
    app.handle(Command::AddVars(vec![
        var(&app, TraceId::A, "soc.cpu0.pc"),
        var(&app, b(), "top.logic"),
    ]));
    pump(&mut app);
    app.save_workspace(None);
    let bytes = app
        .take_events()
        .into_iter()
        .find_map(|e| match e {
            Event::PersistWorkspace { bytes, .. } => Some(bytes),
            _ => None,
        })
        .expect("a workspace write");
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["traces"][1]["letter"], "B");
    assert_eq!(
        saved["traces"][1]["path"], "dram.fst",
        "beside the workspace"
    );
    assert_eq!(saved["traces"][1]["rename"], "dram");
    let rows = &saved["panels"][0]["rows"];
    assert!(rows[0].get("trace").is_none(), "A's rows name no trace");
    assert_eq!(rows[1]["trace"], "B");

    // Opening A again restores the workspace: it opens B first.
    let app = reopen(&cpu, Content::Bytes(bytes));
    assert_eq!(app.workspace.notices, Vec::<String>::new());
    assert_eq!(app.doc.traces().len(), 2);
    assert_eq!(app.doc.trace_name(b()).as_deref(), Some("dram"));
    assert_eq!(app.doc.time_base().timescale, -12);
    let restored = history(&app, b(), "logic");
    assert_eq!(restored.time(0), 5);
    assert!(!history(&app, TraceId::A, "pc").is_empty());
}

/// A trace counted in cycles, with no seconds to place it by.
struct Cycles {
    info: volna_core::data::TraceInfo,
    hierarchy: volna_core::data::Hierarchy,
}

impl volna_core::session::Session for Cycles {
    fn info(&self) -> &volna_core::data::TraceInfo {
        &self.info
    }
    fn hierarchy(&self) -> &volna_core::data::Hierarchy {
        &self.hierarchy
    }
    fn load_signal(
        &self,
        _: volna_core::data::SignalRef,
    ) -> anyhow::Result<Arc<dyn volna_core::data::SignalHistory>> {
        anyhow::bail!("no histories")
    }
}

#[test]
fn a_trace_in_cycles_does_not_join_one_in_seconds() {
    let mut app = two_runs();
    app.handle(Command::RemoveTrace(b()));
    app.handle(Command::AddTrace(OpenSpec::Bytes {
        name: "kanata.log".into(),
        bytes: Vec::new(),
    }));
    let Some(LoadRequest::Open {
        trace, generation, ..
    }) = app.take_requests().pop()
    else {
        panic!("an open request");
    };
    assert_eq!(trace, b());
    app.take_events();
    app.deliver(volna_core::LoadResult::Opened {
        trace,
        generation,
        result: Ok(Arc::new(Cycles {
            info: volna_core::data::TraceInfo {
                name: "kanata.log".into(),
                design_id: None,
                timescale: 0,
                time_range: (0, 100),
                signal_count: 0,
                change_count: None,
                time_unit: Some("cycle".into()),
            },
            hierarchy: Default::default(),
        })),
    });
    assert_eq!(app.doc.traces().len(), 1, "it left the set");
    let notice = app.take_events().into_iter().find_map(|e| match e {
        Event::Notice(text) => Some(text),
        _ => None,
    });
    assert_eq!(
        notice.as_deref(),
        Some(
            "Trace B was not added: its times are in cycle, the others' in ns: traces share \
             a timeline only in seconds-based units or in the same named unit"
        )
    );
}

fn landing_dram() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples/landing_dram.fst")
}

/// The value of a loaded row at `time`, as text.
fn text_at(history: &dyn volna_core::data::SignalHistory, time: u64) -> String {
    match history.value(history.index_at(time)) {
        volna_core::data::WaveValue::Text(text) => text,
        volna_core::data::WaveValue::Bytes(bytes) => String::from_utf8(bytes).unwrap(),
        volna_core::data::WaveValue::Bits(bits) => {
            u64::from_str_radix(&bits, 2).map_or(bits, |v| format!("{v:#x}"))
        }
        other => format!("{other:?}"),
    }
}

/// The checked-in pair is one run: the CPU's VTR in nanoseconds and the
/// DRAM controller's FST in picoseconds. On one timeline every DRAM burst
/// the VTR records starts where the controller issues ACT for its address.
#[test]
fn the_landing_cpu_and_dram_traces_line_up_on_one_timeline() {
    let mut app = App::new();
    app.handle(Command::Open(OpenSpec::Path(landing())));
    app.handle(Command::AddTrace(OpenSpec::Path(landing_dram())));
    pump(&mut app);
    assert_eq!(app.doc.time_base().timescale, -12);
    let burst = app
        .doc
        .session(TraceId::A)
        .unwrap()
        .tracks()
        .iter()
        .find(|t| t.path.join(".") == "soc.dram.bus.burst")
        .unwrap()
        .id;
    let burst = Traced::new(TraceId::A, burst);
    app.doc.retain_track(burst).unwrap();
    app.handle(Command::AddVars(vec![
        var(&app, b(), "lpddr_tb.ctrl.state"),
        var(&app, b(), "lpddr_tb.ctrl.addr"),
    ]));
    pump(&mut app);
    let (state, addr) = (history(&app, b(), "state"), history(&app, b(), "addr"));
    let bursts = app.doc.resident_generator(burst).unwrap();
    let records = bursts.transactions();
    let acts = (0..state.len())
        .filter(|&i| text_at(state.as_ref(), state.time(i)) == "ACT")
        .count();
    assert_eq!(records.len(), acts);
    assert!(records.len() >= 10);
    for tx in records {
        let address = tx
            .attributes
            .iter()
            .find(|a| a.key == "address")
            .map(|a| match a.value {
                volna_core::data::transactions::AttributeValue::U64(v) => v,
                ref other => panic!("{other:?}"),
            })
            .unwrap();
        assert_eq!(tx.begin % 1000, 0, "nanoseconds placed exactly");
        assert_eq!(text_at(state.as_ref(), tx.begin), "ACT");
        assert_eq!(text_at(state.as_ref(), tx.begin - 1), "IDLE");
        assert_eq!(text_at(addr.as_ref(), tx.begin), format!("{address:#x}"));
    }
}

/// `volna A B`: B is still opening when A's own workspace restores; it
/// joins all the same.
#[test]
fn a_trace_opening_while_a_workspace_restores_still_joins() {
    let dir = tempfile::tempdir().unwrap();
    let cpu = copy_to(dir.path(), "cpu.vtr");
    let mut app = App::new();
    app.configure_persistence(Persistence::Auto);
    app.open_resource(OpenSpec::Path(cpu.clone()), uri(&cpu));
    app.add_resource(OpenSpec::Path(values_fst()), uri(&values_fst()));
    let mut requests = app.take_requests();
    assert_eq!(requests.len(), 2);
    let second = requests.pop().unwrap();
    app.deliver(requests.pop().unwrap().perform());
    // A's sidecar holds a workspace of A alone.
    let saved = {
        let mut other = App::new();
        other.handle(Command::Open(OpenSpec::Path(cpu.clone())));
        pump(&mut other);
        Workspace::capture(&other, |_| Ok(Some("cpu.vtr".into())), None)
            .unwrap()
            .to_bytes()
            .unwrap()
    };
    let trace_uri = uri(&cpu);
    app.restore_candidates(
        &trace_uri,
        Candidate {
            target: Target::File {
                uri: format!("{trace_uri}.volna.json"),
            },
            content: Content::Bytes(saved),
            writable: true,
        },
        Candidate {
            target: Target::Storage {
                key: trace_uri.clone(),
            },
            content: Content::Missing,
            writable: true,
        },
    );
    app.deliver(second.perform());
    pump(&mut app);
    assert_eq!(app.doc.traces().len(), 2);
    assert!(app.trace_chips().iter().all(|c| !c.loading));
}

/// Closing a trace that is still opening cancels the open: nothing was
/// added, so there is nothing to undo.
#[test]
fn closing_a_trace_still_opening_cancels_it() {
    let mut app = App::new();
    app.handle(Command::Open(OpenSpec::Path(landing())));
    pump(&mut app);
    app.handle(Command::AddTrace(OpenSpec::Path(values_fst())));
    assert!(app.trace_chips()[1].loading);
    app.handle(Command::RemoveTrace(b()));
    assert_eq!(app.doc.traces().len(), 1);
    assert!(
        app.take_requests().is_empty(),
        "the open request is dropped"
    );
    assert!(!app.can_undo());
    assert_eq!(app.doc.traces().free_id(), Some(b()));
}

/// A workspace's times are counted in the unit it was saved in: restored
/// where the traces open in another unit, they convert, finer exactly and
/// coarser to the nearest.
#[test]
fn saved_times_follow_the_unit_of_the_traces_open_now() {
    let dir = tempfile::tempdir().unwrap();
    let cpu = copy_to(dir.path(), "cpu.vtr");
    let dram = dir.path().join("dram.fst");
    std::fs::copy(values_fst(), &dram).unwrap();
    // Saved with A alone, in nanoseconds; B was added to the file by hand.
    let mut saved = {
        let mut app = reopen(&cpu, Content::Missing);
        app.doc.shared.cursor = Some(783);
        app.doc.add_marker(757);
        let workspace = Workspace::capture(&app, |_| Ok(Some("cpu.vtr".into())), None);
        serde_json::to_value(workspace.unwrap()).unwrap()
    };
    assert_eq!(saved["timescale"], -9);
    let b_entry = serde_json::json!({"letter": "B", "path": "dram.fst", "name": "dram.fst",
        "timescale": -12, "time_range": [0, 20], "design_id": null});
    saved["traces"].as_array_mut().unwrap().push(b_entry);
    let app = reopen(&cpu, Content::Bytes(serde_json::to_vec(&saved).unwrap()));
    assert_eq!(app.doc.traces().len(), 2);
    assert_eq!(app.doc.timescale(), -12);
    assert_eq!(
        app.doc.shared.cursor,
        Some(783_000),
        "{:?}",
        app.workspace.notices
    );
    assert_eq!(app.doc.markers()[0].time, 757_000);

    // Saved in picoseconds, restored where B is missing: rounded to ns.
    saved["timescale"] = (-12).into();
    saved["shared"]["cursor"] = 783_499.into();
    std::fs::remove_file(&dram).unwrap();
    let app = reopen(&cpu, Content::Bytes(serde_json::to_vec(&saved).unwrap()));
    assert_eq!(app.doc.traces().len(), 1);
    assert_eq!(app.doc.shared.cursor, Some(783));
    assert!(
        app.workspace
            .notices
            .iter()
            .any(|n| n == "Times were saved in ps; they are rounded to ns"),
        "{:?}",
        app.workspace.notices
    );
}

/// Open Workspace flushes the open workspace with its own traces before
/// opening and closing the ones the new workspace names.
#[test]
fn open_workspace_flushes_the_old_one_before_changing_traces() {
    let dir = tempfile::tempdir().unwrap();
    let cpu = copy_to(dir.path(), "cpu.vtr");
    let (x, y) = (dir.path().join("x.fst"), dir.path().join("y.fst"));
    std::fs::copy(values_fst(), &x).unwrap();
    std::fs::copy(values_fst(), &y).unwrap();
    let mut app = reopen(&cpu, Content::Missing);
    app.add_resource(OpenSpec::Path(x.clone()), uri(&x));
    pump(&mut app);
    let writes = |app: &mut App| -> Vec<(Target, serde_json::Value)> {
        app.take_events()
            .into_iter()
            .filter_map(|e| match e {
                Event::PersistWorkspace { ticket, bytes } => {
                    Some((ticket.target, serde_json::from_slice(&bytes).unwrap()))
                }
                _ => None,
            })
            .collect()
    };
    // W2 names y.fst as B; the open workspace (W1) has unsaved rows.
    app.save_workspace(None);
    let (old, mut w2) = writes(&mut app).pop().unwrap();
    w2["traces"][1]["path"] = "y.fst".into();
    app.handle(Command::AddVars(vec![var(&app, TraceId::A, "soc.cpu0.pc")]));
    let w2_target = Target::File {
        uri: url::Url::from_file_path(dir.path().join("w2.volna.json"))
            .unwrap()
            .to_string(),
    };
    // Acknowledge the explicit save first.
    let ticket = app.workspace.scheduler.outstanding().cloned().unwrap();
    app.workspace_saved(ticket, None, volna_core::Instant::now());
    app.open_workspace(w2_target.clone(), &serde_json::to_vec(&w2).unwrap())
        .unwrap();
    let flushed = writes(&mut app);
    assert_eq!(flushed.len(), 1, "the old workspace is flushed first");
    assert_eq!(flushed[0].0, old);
    assert_eq!(flushed[0].1["traces"][1]["path"], "x.fst");
    assert_eq!(app.doc.traces().get(b()).unwrap().uri, Some(uri(&x)));
    let ticket = app.workspace.scheduler.outstanding().cloned().unwrap();
    app.workspace_saved(ticket, None, volna_core::Instant::now());
    pump(&mut app);
    assert_eq!(app.doc.traces().get(b()).unwrap().uri, Some(uri(&y)));
    assert!(
        writes(&mut app).iter().all(|(target, _)| *target != old),
        "nothing more is written to the old workspace"
    );
    assert_eq!(app.workspace.scheduler.target(), Some(&w2_target));
}
