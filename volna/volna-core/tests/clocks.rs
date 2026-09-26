//! Declared clocks in Volna (docs/vtr_clocks.html): clock rulers and their
//! cursor chips, snapping and stepping, readouts, clock rows, pipelines counted in
//! their stream's clock (also on a Kanata import), workspaces and remote
//! loading. Headless: commands in, state and scenes out.

use std::sync::Arc;

use volna_core::app::{Action, App, ClockCommand, Command};
use volna_core::data::Member;
use volna_core::data::source::Lookup;
use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::panels::PanelId;
use volna_core::scene::MonoMeasure;
use volna_core::session::{OpenSpec, Session};
use volna_core::wave::PointerEvent;
use volna_core::wave::model::WaveRow;
use volna_core::wave::viewport::Viewport;
use volna_core::workspace::Workspace;
use volna_core::{Instant, Theme};

const BOUNDS: Rect = Rect::from_xywh(0.0, 0.0, 1200.0, 600.0);
/// The page's DVFS core clock (ps): 334, then 500, then 1000 ps.
const CORE: [(u64, u64, u64); 3] = [(400, 19772, 334), (20272, 49772, 500), (50772, 99772, 1000)];

fn core_edges() -> Vec<u64> {
    CORE.iter()
        .flat_map(|&(b, e, p)| (0..=(e - b) / p).map(move |k| b + k * p))
        .collect()
}

/// A picosecond trace: the core clock declared with its dumped waveform, a
/// 2 ns bus clock gated from 30 to 40 ns, a pipeline stream counted in the
/// core clock and a signal.
fn fixture() -> Arc<dyn Session> {
    use vtr::{Direction, ScopeType, SignalKind, TxStatus, Value, VarType};
    let file = tempfile::Builder::new().suffix(".vtr").tempfile().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    w.set_timescale(-12).unwrap();
    let top = w.add_scope(None, "top", ScopeType::Module, "top").unwrap();
    let (_, clk) = w
        .add_var(
            Some(top),
            "core_clk",
            VarType::Wire,
            Direction::Implicit,
            SignalKind::Bits {
                width: 1,
                states: 2,
            },
        )
        .unwrap();
    let core = w.add_clock(Some(top), "core_clk").unwrap();
    let bus = w.add_clock(Some(top), "bus_clk").unwrap();
    let pipe = w.add_stream(Some(top), "pipe", "PIPELINE").unwrap();
    let link = w.intern("top.core_clk");
    w.node_attr(pipe, "vtr.clock", Value::Str(link)).unwrap();
    let insn = w.add_generator(pipe, "insn").unwrap();
    for &(b, e, p) in &CORE {
        w.clock_run(core, b, p).unwrap();
        w.clock_stop(core, e + p / 2).unwrap();
    }
    w.clock_run(bus, 1000, 2000).unwrap();
    w.clock_stop(bus, 30000).unwrap();
    w.clock_run(bus, 41000, 2000).unwrap();
    // The dumped waveform: rising at every edge, falling half a period later.
    let mut changes: Vec<(u64, u64)> = Vec::new();
    for &(b, e, p) in &CORE {
        for k in 0..=(e - b) / p {
            changes.push((b + k * p, 1));
            changes.push((b + k * p + p / 2, 0));
        }
    }
    w.set_time(0).unwrap();
    w.emit_u64(clk, 0).unwrap();
    for (t, v) in changes {
        w.set_time(t).unwrap();
        w.emit_u64(clk, v).unwrap();
    }
    // Instructions whose stages start on core edges.
    let edges = core_edges();
    let f = w.intern("F");
    let x = w.intern("X");
    let lane = w.intern("0");
    for i in 0..20 {
        let tx = w.begin_tx(insn, edges[i]).unwrap();
        w.tx_stage(tx, f, lane, edges[i], edges[i + 1], &[])
            .unwrap();
        w.tx_stage(tx, x, lane, edges[i + 1], edges[i + 3], &[])
            .unwrap();
        w.end_tx(tx, edges[i + 3], TxStatus::Ok).unwrap();
    }
    w.set_time(101_000).unwrap();
    w.close().unwrap();
    OpenSpec::Path(file.path().into()).open().unwrap()
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for r in requests {
            app.deliver(r.perform());
        }
    }
}

fn frame(app: &mut App, id: PanelId, theme: &Theme) {
    app.layout_panel(id, BOUNDS, theme).unwrap();
    app.render_panel(id, theme, &mut MonoMeasure);
}

fn scope(session: &dyn Session, path: &[&str]) -> usize {
    match session.hierarchy().find_scope(path) {
        Lookup::Found(id) => id,
        other => panic!("{other:?}"),
    }
}

/// A wave panel showing the core clock's waveform, clocks loaded.
fn waves() -> (App, Arc<dyn Session>, PanelId) {
    let session = fixture();
    let mut app = App::new();
    app.set_session(session.clone());
    app.handle(Command::AddVars(vec![0]));
    pump(&mut app);
    let panel = app.panels.focused_id();
    assert!(
        app.doc.clocks.iter().all(|c| c.timeline().is_some()),
        "clocks load with the trace"
    );
    (app, session, panel)
}

fn waves_x(app: &App, panel: PanelId, t: f64) -> f32 {
    let w = app.panels.waves(panel).unwrap();
    let layout = w.last_layout();
    layout.waves.left() + w.viewport(&app.doc).x_of(t, layout.wave_width_f64()) as f32
}

fn click(app: &mut App, panel: PanelId, at: Point) {
    let down = PointerEvent::Down {
        position: at,
        button: MouseButton::Left,
        modifiers: Modifiers::default(),
    };
    app.handle(Command::Pointer(panel, down));
    app.handle(Command::Pointer(panel, PointerEvent::Up));
}

#[test]
fn clocks_load_with_the_trace_and_rulers_tick_edges_labels_and_speed_flags() {
    let (mut app, _, panel) = waves();
    let theme = Theme::one_dark();
    let core = app.doc.clocks.find("top.core_clk").unwrap();
    let edges = core_edges();
    assert_eq!(core.timeline().unwrap().edge_count(), edges.len() as u64);
    let bus = app
        .doc
        .clocks
        .find("top.bus_clk")
        .unwrap()
        .timeline()
        .unwrap()
        .clone();
    assert_eq!(bus.stretches().len(), 2);
    // Without pipelines no ruler is shown by default.
    frame(&mut app, panel, &theme);
    assert_eq!(
        app.panels
            .waves(panel)
            .unwrap()
            .last_layout()
            .rulers
            .height(),
        0.0
    );
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.core_clk".into(),
    )));
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.bus_clk".into(),
    )));
    app.doc.shared.viewport.set(Viewport {
        start: 0.0,
        end: 60_000.0,
    });
    frame(&mut app, panel, &theme);
    let layout = app.panels.waves(panel).unwrap().last_layout().clone();
    assert_eq!(layout.rulers.height(), 2.0 * 16.0 * theme.zoom);
    assert_eq!(layout.names.top(), layout.rulers.bottom());
    let texts: Vec<String> = app.scene().texts().map(str::to_owned).collect();
    for want in ["core_clk", "bus_clk", "→ 2.00 GHz"] {
        assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
    }
    // Cycle labels are multiples of the stretch's step: 20 at 334 ps and 10 at 500 ps here.
    let labels: Vec<i64> = texts.iter().filter_map(|t| t.parse().ok()).collect();
    assert!(
        labels.contains(&0) && labels.contains(&40) && labels.contains(&60),
        "{labels:?}"
    );
    // The gated bus clock is hatched while stopped (hatch lines in the ruler band).
    let hatch = app.scene().prims.iter().any(
        |p| matches!(p, volna_core::scene::Prim::Lines { color, .. } if *color == theme.wave_dense),
    );
    assert!(hatch);
}

#[test]
fn each_ruler_has_a_cursor_chip_with_its_cycle_and_go_to_counts_the_selected_clock() {
    let (mut app, _, panel) = waves();
    let theme = Theme::one_dark();
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.core_clk".into(),
    )));
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.bus_clk".into(),
    )));
    app.doc.shared.viewport.set(Viewport {
        start: 0.0,
        end: 10_000.0,
    });
    let edges = core_edges();
    app.doc.shared.cursor = Some(edges[12] + 167);
    frame(&mut app, panel, &theme);
    let texts: Vec<String> = app.scene().texts().map(str::to_owned).collect();
    // The time ruler keeps its time chip; every ruler adds its own cycle chip.
    assert!(texts.iter().any(|t| t == "4.575 ns"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t == "12 + 0.50"),
        "core chip: {texts:?}"
    );
    assert!(texts.iter().any(|t| t == "1 + 0.79"), "bus chip: {texts:?}");
    // Go to cycle counts the selected clock (the first ruler by default), centred.
    app.handle(Command::Clocks(ClockCommand::GoToCycle(100)));
    assert_eq!(app.doc.shared.cursor, Some(edges[100]));
    app.handle(Command::Clocks(ClockCommand::GoToCycle(1_000_000)));
    assert_eq!(
        app.doc.shared.cursor,
        Some(edges[100]),
        "a missing cycle leaves the cursor"
    );
    app.handle(Command::Clocks(ClockCommand::Select("top.bus_clk".into())));
    app.handle(Command::Clocks(ClockCommand::GoToCycle(10)));
    assert_eq!(app.doc.shared.cursor, Some(1000 + 10 * 2000));
    // A custom origin renumbers every clock of the panel.
    app.handle(Command::Clocks(ClockCommand::Select("top.core_clk".into())));
    app.doc.shared.cursor = Some(edges[100]);
    app.handle(Command::Action(Action::ToggleCycleOrigin));
    app.doc.shared.cursor = Some(edges[103]);
    app.doc.shared.viewport.set(Viewport {
        start: edges[99] as f64,
        end: edges[106] as f64,
    });
    frame(&mut app, panel, &theme);
    assert!(
        app.scene().texts().any(|t| t == "3"),
        "{:?}",
        app.scene().texts().collect::<Vec<_>>()
    );
    app.handle(Command::Clocks(ClockCommand::GoToCycle(-100)));
    assert_eq!(app.doc.shared.cursor, Some(edges[0]));
}

#[test]
fn clicks_snap_to_the_selected_clock_and_brackets_step_cycles() {
    let (mut app, _, panel) = waves();
    let theme = Theme::one_dark();
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.core_clk".into(),
    )));
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.bus_clk".into(),
    )));
    app.doc.shared.viewport.set(Viewport {
        start: 0.0,
        end: 20_000.0,
    });
    frame(&mut app, panel, &theme);
    let edges = core_edges();
    // A click on the bus ruler selects the bus clock and snaps to its edge.
    let layout = app.panels.waves(panel).unwrap().last_layout().clone();
    let bus_row = point(
        waves_x(&app, panel, 5050.0),
        layout.rulers.top() + 1.5 * 16.0 * theme.zoom,
    );
    click(&mut app, panel, bus_row);
    assert_eq!(app.doc.shared.cursor, Some(5000));
    assert_eq!(
        app.panels
            .waves(panel)
            .unwrap()
            .nav
            .clocks
            .selected
            .as_deref(),
        Some("top.bus_clk")
    );
    // Select the core clock: header clicks snap to its edges.
    app.handle(Command::Clocks(ClockCommand::Select("top.core_clk".into())));
    frame(&mut app, panel, &theme);
    let header = app.panels.waves(panel).unwrap().last_layout().header;
    let x = waves_x(&app, panel, edges[7] as f64 + 30.0);
    click(&mut app, panel, point(x, header.top() + 5.0));
    assert_eq!(app.doc.shared.cursor, Some(edges[7]));
    app.handle(Command::Action(Action::NextCycle));
    assert_eq!(app.doc.shared.cursor, Some(edges[8]));
    app.handle(Command::Action(Action::PrevCycle));
    app.handle(Command::Action(Action::PrevCycle));
    assert_eq!(app.doc.shared.cursor, Some(edges[6]));
    // Stepping crosses a change of speed without inventing edges.
    app.doc.shared.cursor = Some(19772);
    app.handle(Command::Action(Action::NextCycle));
    assert_eq!(app.doc.shared.cursor, Some(20272));
}

#[test]
fn the_status_bar_reads_cycles_and_the_cursor_to_marker_delta() {
    let (mut app, _, panel) = waves();
    let theme = Theme::one_dark();
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.core_clk".into(),
    )));
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.bus_clk".into(),
    )));
    frame(&mut app, panel, &theme);
    let edges = core_edges();
    app.doc.shared.cursor = Some(edges[10]);
    app.handle(Command::Action(Action::AddMarker));
    app.doc.shared.cursor = Some(edges[50] + 167);
    let s = app.status();
    assert_eq!(s.clocks, ["core_clk 50 + 0.50", "bus_clk 8 + 0.13"]);
    // 40 core cycles and 7 bus cycles lie between the marker and the cursor.
    assert_eq!(
        s.delta.as_deref(),
        Some("Δ 13.53 ns · 40 core_clk · 7 bus_clk")
    );
    // Before the marker the delta is negative.
    app.doc.shared.cursor = Some(edges[4]);
    let s = app.status();
    assert!(s.delta.unwrap().starts_with("Δ −"));
}

#[test]
fn a_clock_row_draws_from_stretches_and_survives_a_workspace() {
    let (mut app, session, panel) = waves();
    let theme = Theme::one_dark();
    let bus = scope(session.as_ref(), &["top", "bus_clk"]);
    app.handle(Command::AddToWaves(vec![Member::Stream(bus)]));
    let w = app.panels.waves(panel).unwrap();
    assert_eq!(w.items.len(), 2);
    assert_eq!(w.items[1].clock().unwrap().path, "top.bus_clk");
    app.doc.shared.viewport.set(Viewport {
        start: 0.0,
        end: 60_000.0,
    });
    app.doc.shared.cursor = Some(3500);
    frame(&mut app, panel, &theme);
    assert!(app.scene().texts().any(|t| t == "bus_clk"));
    assert!(
        app.scene().texts().any(|t| t == "1 + 0.25"),
        "value column shows the cycle"
    );
    // Next edge on the selected clock row steps through its edges.
    app.handle(Command::Action(Action::MoveSelectionDown));
    app.handle(Command::Action(Action::NextEdge));
    assert_eq!(app.doc.shared.cursor, Some(5000));
    // Save and restore: the row and the panel's clock choices come back.
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.bus_clk".into(),
    )));
    app.handle(Command::Clocks(ClockCommand::Select("top.bus_clk".into())));
    app.handle(Command::Action(Action::ToggleCycleOrigin));
    let saved = Workspace::capture(&app, "trace.vtr".into(), None).unwrap();
    let json = serde_json::to_value(&saved).unwrap();
    let panel_json = &json["panels"][0];
    assert_eq!(
        panel_json["rows"][1],
        serde_json::json!({"type": "clock", "clock": "top.bus_clk"})
    );
    assert_eq!(
        panel_json["clocks"],
        serde_json::json!({"rulers": ["top.bus_clk"], "selected": "top.bus_clk", "origin": 5000})
    );
    let mut restored = App::new();
    restored.set_session(session.clone());
    pump(&mut restored);
    let plan = Workspace::parse(&saved.to_bytes().unwrap())
        .unwrap()
        .prepare(
            &restored,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )
        .unwrap();
    assert!(
        plan.report().notices.is_empty(),
        "{:?}",
        plan.report().notices
    );
    plan.commit(&mut restored).unwrap();
    let w = restored.panels.waves(panel).unwrap();
    assert!(matches!(&w.items[1].row, WaveRow::Clock(c) if c.path == "top.bus_clk"));
    assert_eq!(w.nav.clocks, app.panels.waves(panel).unwrap().nav.clocks);
}

#[test]
fn a_pipeline_counts_in_its_stream_clock_and_its_clock_becomes_the_default_ruler() {
    let (mut app, session, waves) = waves();
    let theme = Theme::one_dark();
    let pipe = scope(session.as_ref(), &["top", "pipe"]);
    app.handle(Command::ActivateMembers(vec![Member::Stream(pipe)]));
    pump(&mut app);
    let pipeline = app.panels.focused_id();
    assert_ne!(pipeline, waves);
    // Rows stay put when the cursor moves (the pipeline tests' convention).
    app.handle(Command::PipelineActivity(
        pipeline,
        volna_core::pipeline::ActivityCommand::Toggle,
    ));
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.clock(&app.doc).unwrap().path, "top.core_clk");
    app.doc.shared.viewport.set(Viewport {
        start: 0.0,
        end: 8000.0,
    });
    frame(&mut app, pipeline, &theme);
    frame(&mut app, waves, &theme);
    // The pipeline's clock is every panel's default ruler.
    assert_eq!(
        app.panels
            .waves(waves)
            .unwrap()
            .last_layout()
            .rulers
            .height(),
        16.0 * theme.zoom
    );
    frame(&mut app, pipeline, &theme);
    let layout = app.panels.pipeline(pipeline).unwrap().last_layout().clone();
    let edges = core_edges();
    // A click inside cycle 5 puts the cursor on its edge, not on a whole time unit.
    let viewport = app
        .panels
        .pipeline(pipeline)
        .unwrap()
        .nav
        .viewport(&app.doc);
    let x = layout.cells.left()
        + viewport.x_of(edges[5] as f64 + 200.0, layout.cells_width_f64()) as f32;
    click(
        &mut app,
        pipeline,
        point(x, layout.row_y(3) + layout.rows.row_px / 2.0),
    );
    assert_eq!(app.doc.shared.cursor, Some(edges[5]));
    // Hovering a stage reads its cycles.
    let y = layout.row_y(3) + layout.rows.row_px / 2.0;
    let stage_x = layout.cells.left()
        + viewport.x_of(edges[4] as f64 + 100.0, layout.cells_width_f64()) as f32;
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Move {
            position: point(stage_x, y),
        },
    ));
    let hover = app.status().hover.unwrap();
    assert!(hover.contains("X cycles 4–6 (2 cycles)"), "{hover}");
}

#[test]
fn a_kanata_import_counts_its_pipeline_in_the_cycle_clock() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("t.kanata");
    std::fs::write(
        &log,
        "Kanata\t0004\nC=\t100\nI\t0\t0\t0\nS\t0\t0\tF\nC\t1\nE\t0\t0\tF\nS\t0\t0\tX\nI\t1\t1\t0\nS\t1\t0\tF\nC\t2\nE\t0\t0\tX\nR\t0\t0\t0\nE\t1\t0\tF\nS\t1\t0\tX\nC\t3\nE\t1\t0\tX\nR\t1\t1\t0\n",
    )
    .unwrap();
    let out = dir.path().join("t.vtr");
    let mut w = vtr::Writer::create(&out).unwrap();
    vtr_cli::kanata::convert_kanata(log.to_str().unwrap(), &mut w).unwrap();
    w.close().unwrap();
    let session = OpenSpec::Path(out).open().unwrap();
    let mut app = App::new();
    app.set_session(session.clone());
    pump(&mut app);
    let clock = app
        .doc
        .clocks
        .find("cpu.cycle")
        .unwrap()
        .timeline()
        .unwrap()
        .clone();
    assert_eq!(
        (clock.stretches()[0].begin, clock.stretches()[0].period),
        (100, 1)
    );
    let thread = scope(session.as_ref(), &["cpu", "thread0"]);
    app.handle(Command::ActivateMembers(vec![Member::Stream(thread)]));
    pump(&mut app);
    let pipeline = app.panels.focused_id();
    app.handle(Command::PipelineActivity(
        pipeline,
        volna_core::pipeline::ActivityCommand::Toggle,
    ));
    let theme = Theme::one_dark();
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    // The stream's clock is the pipeline's default ruler.
    assert_eq!(p.last_layout().rulers.height(), 16.0 * theme.zoom);
    assert!(app.scene().texts().any(|t| t == "cycle"));
    let layout = p.last_layout().clone();
    let y = layout.row_y(0) + layout.rows.row_px / 2.0;
    let viewport = p.nav.viewport(&app.doc);
    let x = layout.cells.left() + viewport.x_of(101.5, layout.cells_width_f64()) as f32;
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Move {
            position: point(x, y),
        },
    ));
    // Cycle 1 of the import (time 101), counted from the first recorded cycle.
    assert!(
        app.status()
            .hover
            .unwrap()
            .contains("X cycles 1–3 (2 cycles)")
    );
    click(&mut app, pipeline, point(x, y));
    assert_eq!(app.doc.shared.cursor, Some(101));
    let _ = Instant::now();
}

#[test]
fn clocks_load_through_the_remote_protocol_like_any_track() {
    use std::os::unix::net::UnixStream;
    use volna_core::remote::ClientStep;
    use volna_core::remote::client::RemoteClient;
    use volna_core::remote::memory::MemoryBudget;
    use volna_core::remote::transport::{read_packet, write_packet};
    let local = fixture();
    let (client_end, server_end) = UnixStream::pair().unwrap();
    let served = local.clone();
    let server = std::thread::spawn(move || {
        let input = server_end.try_clone().unwrap();
        volna_core::remote::server::serve(input, server_end, 7, move || Ok(served), || Ok(()))
    });
    let mut app = App::new();
    let budget = MemoryBudget::new(256 << 20);
    let mut client = RemoteClient::new(1, 64 << 20, budget).unwrap();
    let mut stream = client_end;
    // Drive the connection until no command is left and every result is delivered.
    let mut drive = |app: &mut App, client: &mut RemoteClient| loop {
        let Some(command) = client.take_command().unwrap() else {
            return;
        };
        write_packet(&mut stream, &command).unwrap();
        loop {
            let packet = read_packet(&mut stream).unwrap().unwrap();
            let mut step = client.accept(packet).unwrap();
            let done = loop {
                match step {
                    ClientStep::Yield => step = client.step().unwrap(),
                    ClientStep::Ack(ack) => {
                        write_packet(&mut stream, &ack).unwrap();
                        break false;
                    }
                    ClientStep::Complete { ack, result } => {
                        write_packet(&mut stream, &ack).unwrap();
                        let opened = matches!(&result, volna_core::LoadResult::Opened { .. });
                        app.deliver(result);
                        if opened {
                            // The open delivered: submit the clock loads it queued.
                            for request in app.take_requests() {
                                client.submit(request).map_err(|_| "rejected").unwrap();
                            }
                        }
                        break true;
                    }
                }
            };
            if done {
                break;
            }
        }
    };
    app.handle(Command::Open(OpenSpec::Remote {
        name: "remote.vtr".into(),
        limits: Default::default(),
    }));
    let _open_request = app.take_requests();
    drive(&mut app, &mut client);
    assert!(app.doc.session().is_some_and(|s| s.remote_id() == Some(7)));
    let remote = app
        .doc
        .clocks
        .find("top.core_clk")
        .unwrap()
        .timeline()
        .unwrap()
        .clone();
    let mut local_app = App::new();
    local_app.set_session(local);
    pump(&mut local_app);
    let expected = local_app
        .doc
        .clocks
        .find("top.core_clk")
        .unwrap()
        .timeline()
        .unwrap()
        .clone();
    assert_eq!(remote.edge_count(), core_edges().len() as u64);
    assert_eq!(remote.stretches(), expected.stretches());
    write_packet(
        &mut stream,
        &volna_core::remote::transport::Packet {
            session: 7,
            request: u64::MAX,
            sequence: 0,
            body: volna_core::remote::transport::Body::Command(
                volna_core::remote::transport::Command::Close,
            ),
        },
    )
    .unwrap();
    server.join().unwrap().unwrap();
}

#[test]
fn a_clock_generator_adds_as_a_ruler_or_as_a_waveform() {
    let (mut app, session, panel) = waves();
    let theme = Theme::one_dark();
    let h = session.hierarchy();
    let bus_stream = scope(session.as_ref(), &["top", "bus_clk"]);
    let edges = h
        .generators
        .iter()
        .position(|g| g.stream == bus_stream)
        .unwrap();
    let member = Member::Generator(edges);
    assert_eq!(app.member_clock(member).as_deref(), Some("top.bus_clk"));
    assert_eq!(
        app.member_clock(Member::Var(0)),
        None,
        "a variable declares no clock"
    );
    // Add as Ruler: the wave panel shows the clock's ruler, and adding it again changes nothing.
    app.handle(Command::AddClockRulers(vec![member, Member::Var(0)]));
    app.handle(Command::AddClockRulers(vec![member]));
    frame(&mut app, panel, &theme);
    let w = app.panels.waves(panel).unwrap();
    assert_eq!(
        w.nav.clocks.rulers.as_deref(),
        Some(&["top.bus_clk".to_owned()][..])
    );
    assert_eq!(w.last_layout().rulers.height(), 16.0 * theme.zoom);
    assert_eq!(w.items.len(), 1, "a ruler adds no row");
    // Add as Waveform: a clock row, not a transaction lane of its stretches.
    app.handle(Command::AddToWaves(vec![member]));
    let w = app.panels.waves(panel).unwrap();
    assert!(matches!(&w.items[1].row, WaveRow::Clock(c) if c.path == "top.bus_clk"));
}

#[test]
fn a_ruler_context_menu_hides_it() {
    use volna_core::wave::model::{MenuAction, MenuEntry, WaveMenuKind};
    let (mut app, _, panel) = waves();
    let theme = Theme::one_dark();
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.core_clk".into(),
    )));
    app.handle(Command::Clocks(ClockCommand::ToggleRuler(
        "top.bus_clk".into(),
    )));
    frame(&mut app, panel, &theme);
    let layout = app.panels.waves(panel).unwrap().last_layout().clone();
    // A right click on the second ruler opens its menu.
    let at = point(
        layout.waves.left() + 40.0,
        layout.rulers.top() + 1.5 * 16.0 * theme.zoom,
    );
    let right = PointerEvent::Down {
        position: at,
        button: MouseButton::Right,
        modifiers: Modifiers::default(),
    };
    app.handle(Command::Pointer(panel, right));
    let menu = app.panels.waves(panel).unwrap().menu.clone().unwrap();
    assert_eq!((menu.kind, menu.row), (WaveMenuKind::Ruler, 1));
    let hide = MenuAction::HideRuler("top.bus_clk".into());
    assert!(
        matches!(&menu.entries[..], [MenuEntry::Item(i)] if i.label == "Hide Ruler" && i.action == hide)
    );
    app.handle(Command::MenuSelect(panel, hide));
    frame(&mut app, panel, &theme);
    let w = app.panels.waves(panel).unwrap();
    assert_eq!(
        w.nav.clocks.rulers.as_deref(),
        Some(&["top.core_clk".to_owned()][..])
    );
    assert_eq!(w.last_layout().rulers.height(), 16.0 * theme.zoom);
    assert!(w.menu.is_none());
}
