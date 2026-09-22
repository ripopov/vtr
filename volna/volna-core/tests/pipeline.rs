//! Headless pipeline panel tests: open a track from the sidebar, load it,
//! paint it, navigate it, save and restore it. No GPU, no window.

use std::sync::Arc;
use std::time::Duration;

use volna_core::app::{Action, App, Command, Event, PanelLayout};
use volna_core::data::Member;
use volna_core::data::source::Lookup;
use volna_core::data::transactions::TrackRef;
use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::nav::LinkDim;
use volna_core::panels::{Layout, PanelId, PanelsCommand};
use volna_core::pipeline::{Hit, PipelineModel, RowView, Rows, TrackSource};
use volna_core::scene::{MonoMeasure, Prim};
use volna_core::session::{LoadRequest, OpenSpec, Session};
use volna_core::sidebar::Key;
use volna_core::wave::PointerEvent;
use volna_core::workspace::Workspace;
use volna_core::{Instant, Theme};

const STAGES: [&str; 3] = ["F", "D", "X"];

/// A trace with a PIPELINE stream of `n` instructions (three lane-0 stages
/// each, a stall overlay on every seventh, instruction 3 flushed, the last
/// one left open), an unrecognized stream with one stage-less record, and a
/// counter signal on the same cycle time base.
fn fixture(n: u64) -> Arc<dyn Session> {
    use vtr::{Direction, ScopeType, SignalKind, TxStatus, Value, VarType};
    let file = tempfile::Builder::new().suffix(".vtr").tempfile().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    w.set_timescale(0).unwrap();
    let unit = w.intern("cycle");
    w.set_file_attr("time.unit", Value::Str(unit)).unwrap();
    let cpu = w.begin_scope("cpu", ScopeType::Core, "");
    let (_, counter) = w.add_var(
        "counter",
        VarType::Logic,
        Direction::Implicit,
        SignalKind::Bits {
            width: 8,
            states: 2,
        },
    );
    let stream = w.add_stream(Some(cpu), "thread0", "PIPELINE");
    let insn = w.add_generator(stream, "instruction");
    let bus = w.add_stream(Some(cpu), "bus", "TX");
    let req = w.add_generator(bus, "request");
    w.end_scope().unwrap();
    let last = n + 4;
    for t in 0..=last {
        w.set_time(t).unwrap();
        w.emit_u64(counter, t % 256).unwrap();
    }
    let label = w.intern("vtr.label");
    let lane0 = w.intern("0");
    let lane1 = w.intern("1");
    let stl = w.intern("stl");
    let names: Vec<_> = STAGES.iter().map(|s| w.intern(s)).collect();
    for i in 0..n {
        let tx = w.begin_tx(insn, i).unwrap();
        let caption = format!("{:08x}: op{i}", 0x1000 + 4 * i);
        let value = if i == 0 {
            Value::Text(caption)
        } else {
            Value::Str(w.intern(&caption))
        };
        w.tx_attr(tx, label, &value).unwrap();
        let flushed = i == 3;
        let open = i + 1 == n;
        let count = if flushed { 2 } else { 3 };
        for (k, name) in names.iter().enumerate().take(count) {
            let (b, e) = (i + k as u64, i + k as u64 + 1);
            if open && k + 1 == count {
                w.tx_stage_begin(tx, *name, lane0, b).unwrap();
            } else {
                w.tx_stage(tx, *name, lane0, b, e, &[]).unwrap();
            }
        }
        if i % 7 == 0 {
            w.tx_stage(tx, stl, lane1, i + 1, i + 3, &[]).unwrap();
        }
        if flushed {
            w.end_tx(tx, i + 2, TxStatus::Aborted).unwrap();
        } else if !open {
            w.end_tx(tx, i + 3, TxStatus::Unset).unwrap();
        }
    }
    let tx = w.begin_tx(req, 2).unwrap();
    w.end_tx(tx, 9, TxStatus::Ok).unwrap();
    w.set_time(last).unwrap();
    w.close().unwrap();
    OpenSpec::Path(file.path().into()).open().unwrap()
}

fn scope(session: &dyn Session, path: &[&str]) -> usize {
    match session.hierarchy().find_scope(path) {
        Lookup::Found(id) => id,
        other => panic!("{other:?}"),
    }
}

fn stream_track(session: &dyn Session, path: &[&str]) -> TrackRef {
    let h = session.hierarchy();
    h.member_track(Member::Stream(scope(session, path)))
        .unwrap()
}

/// Perform every queued load synchronously.
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

const BOUNDS: Rect = Rect::from_xywh(0.0, 0.0, 1200.0, 600.0);

fn frame(app: &mut App, id: PanelId, theme: &Theme) {
    app.layout_panel(id, BOUNDS, theme).unwrap();
    app.render_panel(id, theme, &mut MonoMeasure);
}

/// Open the fixture's pipeline stream from the scope tree (Enter on its row).
fn opened(n: u64) -> (App, Arc<dyn Session>, PanelId, PanelId) {
    let session = fixture(n);
    let mut app = App::new();
    app.set_session(session.clone());
    // The first rows turn the start panel into a waveform panel.
    app.handle(Command::AddVars(vec![0]));
    let waves = app.panels.focused_id();
    assert!(app.panels.waves(waves).is_some());
    app.handle(Command::SelectScope(scope(
        session.as_ref(),
        &["cpu", "thread0"],
    )));
    app.take_events();
    app.handle(Command::ScopesKey(Key::Enter));
    let pipeline = app.panels.focused_id();
    assert_ne!(pipeline, waves);
    // These baseline painting/input tests inspect explicitly positioned rows.
    // Follow behavior is exercised separately below.
    app.handle(Command::PipelineActivity(
        pipeline,
        volna_core::pipeline::ActivityCommand::Toggle,
    ));
    (app, session, waves, pipeline)
}

#[test]
fn activity_follows_cursor_then_view_center_and_preserves_other_axes() {
    use volna_core::pipeline::{ActivityCommand, FollowActivity};
    use volna_core::wave::viewport::Viewport;
    let (mut app, _, waves, pipeline) = opened(1000);
    pump(&mut app);
    let theme = Theme::one_dark();
    app.doc.shared.viewport.set(Viewport {
        start: 100.0,
        end: 900.0,
    });
    app.doc.shared.cursor = Some(200);
    frame(&mut app, pipeline, &theme);
    let row_px = app.panels.pipeline(pipeline).unwrap().rows.value.row_px;
    let wave_rows = app.panels.waves(waves).unwrap().scroll_y;
    app.handle(Command::PipelineActivity(pipeline, ActivityCommand::Toggle));
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.follow, FollowActivity::Following);
    assert!(p.last_layout().row_range.contains(&200));
    let top = p.rows.value.top;
    frame(&mut app, pipeline, &theme);
    assert_eq!(app.panels.pipeline(pipeline).unwrap().rows.value.top, top);
    app.doc.shared.cursor = Some(1); // outside the view: anchor its center
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    assert!(p.last_layout().row_range.contains(&500));
    assert_eq!(p.rows.value.row_px, row_px);
    assert_eq!(app.doc.shared.cursor, Some(1));
    assert_eq!(
        app.doc.shared.viewport.value,
        Viewport {
            start: 100.0,
            end: 900.0
        }
    );
    assert_eq!(app.panels.waves(waves).unwrap().scroll_y, wave_rows);
    // Local links use the same policy without mutating shared navigation.
    app.handle(Command::Panels(PanelsCommand::ToggleLink {
        panel: pipeline,
        dim: LinkDim::Viewport,
    }));
    app.handle(Command::Panels(PanelsCommand::ToggleLink {
        panel: pipeline,
        dim: LinkDim::Cursor,
    }));
    let p = app.panels.pipeline_mut(pipeline).unwrap();
    p.nav.local_viewport.set(Viewport {
        start: 700.0,
        end: 900.0,
    });
    p.nav.local_cursor = Some(750);
    frame(&mut app, pipeline, &theme);
    assert!(
        app.panels
            .pipeline(pipeline)
            .unwrap()
            .last_layout()
            .row_range
            .contains(&750)
    );
    assert_eq!(app.doc.shared.cursor, Some(1));
}

#[test]
fn activity_vertical_override_edges_resume_and_idle_are_explicit() {
    use volna_core::pipeline::{ActivityCommand, FollowActivity};
    use volna_core::wave::viewport::Viewport;
    let (mut app, _, _, pipeline) = opened(1000);
    pump(&mut app);
    let theme = Theme::one_dark();
    app.doc.shared.viewport.set(Viewport {
        start: 400.0,
        end: 450.0,
    });
    app.doc.shared.cursor = None;
    frame(&mut app, pipeline, &theme);
    app.handle(Command::PipelineActivity(pipeline, ActivityCommand::Toggle));
    let area = cells(&mut app, pipeline, &theme);
    let position = point(area.left() + 100.0, area.top() + 100.0);
    let wheel = |dx, dy| {
        Command::Pointer(
            pipeline,
            PointerEvent::Wheel {
                position,
                dx,
                dy,
                precise: true,
                modifiers: Modifiers::default(),
            },
        )
    };
    app.handle(wheel(5.0, 0.0));
    assert_eq!(
        app.panels.pipeline(pipeline).unwrap().follow,
        FollowActivity::Following
    );
    app.handle(wheel(0.0, 2000.0));
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.follow, FollowActivity::Suspended);
    let top = p.rows.value.top;
    app.doc.shared.cursor = Some(440);
    frame(&mut app, pipeline, &theme);
    assert_eq!(app.panels.pipeline(pipeline).unwrap().rows.value.top, top);
    let p = app.panels.pipeline(pipeline).unwrap();
    assert!(p.activity(&app.doc).below > 0);
    let (_, rect, _) = p
        .last_layout()
        .activity_controls
        .iter()
        .find(|(command, _, _)| *command == ActivityCommand::RevealBelow)
        .unwrap();
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Down {
            position: point(rect.left() + 10.0, rect.top() + 10.0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.follow, FollowActivity::Suspended);
    assert!(p.activity(&app.doc).visible > 0);
    app.handle(Command::PipelineActivity(pipeline, ActivityCommand::Toggle));
    assert_eq!(
        app.panels.pipeline(pipeline).unwrap().follow,
        FollowActivity::Following
    );
    let top = app.panels.pipeline(pipeline).unwrap().rows.value.top;
    app.doc.shared.viewport.set(Viewport {
        start: 2000.0,
        end: 2100.0,
    });
    frame(&mut app, pipeline, &theme);
    assert_eq!(app.panels.pipeline(pipeline).unwrap().rows.value.top, top);
    let scene = app.render_panel(pipeline, &theme, &mut MonoMeasure);
    assert!(scene.prims.iter().any(|prim| matches!(prim, Prim::Text { text, .. } if text.contains("No pipeline activity") && text.contains('←'))));
}

#[test]
fn activity_state_survives_workspace_and_split() {
    use volna_core::pipeline::FollowActivity;
    for follow in [
        FollowActivity::Off,
        FollowActivity::Following,
        FollowActivity::Suspended,
    ] {
        let (mut app, session, _, pipeline) = opened(100);
        pump(&mut app);
        app.panels.pipeline_mut(pipeline).unwrap().follow = follow;
        assert_eq!(
            app.panels.pipeline(pipeline).unwrap().clone_view().follow,
            follow
        );
        let saved = Workspace::capture(&app, "trace.vtr".into(), None).unwrap();
        let mut restored = App::new();
        restored.set_session(session);
        Workspace::parse(&saved.to_bytes().unwrap())
            .unwrap()
            .prepare(
                &restored,
                "file:///tmp/trace.vtr",
                "file:///tmp/trace.vtr.volna.json",
            )
            .unwrap()
            .commit(&mut restored)
            .unwrap();
        assert_eq!(restored.panels.pipeline(pipeline).unwrap().follow, follow);
    }
}

#[test]
fn activity_keeps_long_overlaps_points_and_generator_row_offsets() {
    use volna_core::wave::viewport::Viewport;
    let file = tempfile::Builder::new().suffix(".vtr").tempfile().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let stream = writer.add_stream(None, "pipeline", "PIPELINE");
    let first = writer.add_generator(stream, "first");
    let second = writer.add_generator(stream, "second");
    let long = writer.begin_tx(first, 0).unwrap();
    writer.end_tx(long, 1000, vtr::TxStatus::Ok).unwrap();
    for i in 1..100 {
        let tx = writer.begin_tx(first, i).unwrap();
        writer.end_tx(tx, i, vtr::TxStatus::Ok).unwrap();
    }
    let point_tx = writer.begin_tx(second, 500).unwrap();
    writer.end_tx(point_tx, 500, vtr::TxStatus::Ok).unwrap();
    writer.set_time(1000).unwrap();
    writer.close().unwrap();
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    let track = stream_track(session.as_ref(), &["pipeline"]);
    let mut app = App::new();
    app.set_session(session);
    app.handle(Command::OpenPipeline { track });
    let panel = app.panels.focused_id();
    app.doc.shared.viewport.set(Viewport {
        start: 500.0,
        end: 500.5,
    });
    app.doc.shared.cursor = Some(500);
    let theme = Theme::one_dark();
    frame(&mut app, panel, &theme); // follow tolerates an undelivered track
    pump(&mut app);
    frame(&mut app, panel, &theme);
    let p = app.panels.pipeline(panel).unwrap();
    let activity = p.activity(&app.doc);
    assert_eq!(activity.visible + activity.above + activity.below, 2);
    assert_eq!(
        activity.target,
        Some(0),
        "nearest visible overlap stays put"
    );
    assert_eq!(
        activity.nearest_below,
        Some(100),
        "second generator follows first's rows"
    );
    app.doc.shared.viewport.set(Viewport {
        start: 500.1,
        end: 500.9,
    });
    frame(&mut app, panel, &theme);
    let activity = app.panels.pipeline(panel).unwrap().activity(&app.doc);
    assert_eq!(activity.visible + activity.above + activity.below, 1);
    assert_eq!(
        activity.target,
        Some(0),
        "fractional windows still contain long lifetimes"
    );
    app.doc.shared.viewport.set(Viewport {
        start: -2.0,
        end: -1.0,
    });
    frame(&mut app, panel, &theme);
    let activity = app.panels.pipeline(panel).unwrap().activity(&app.doc);
    assert_eq!(activity.target, None);
    assert!(activity.later);
}

fn cells(app: &mut App, id: PanelId, theme: &Theme) -> Rect {
    match app.layout_panel(id, BOUNDS, theme).unwrap() {
        PanelLayout::Pipeline(l) => l.cells,
        PanelLayout::Waves(_) | PanelLayout::Table(_) => panic!("not a pipeline panel"),
    }
}

#[test]
fn enter_on_a_stream_opens_a_panel_loads_the_track_once_and_paints_cells() {
    let (mut app, session, waves, pipeline) = opened(40);
    assert!(app.status().sidebar_notice.is_none());
    assert!(
        !app.take_events()
            .iter()
            .any(|e| matches!(e, Event::Notice(_)))
    );
    let requests = app.take_requests();
    let track = stream_track(session.as_ref(), &["cpu", "thread0"]);
    let tracks: Vec<_> = requests
        .iter()
        .filter_map(|r| match r {
            LoadRequest::Track { track, .. } => Some(*track),
            _ => None,
        })
        .collect();
    assert_eq!(tracks, [track], "the wave row's signals and one track load");
    let theme = Theme::one_dark();
    frame(&mut app, pipeline, &theme);
    assert!(matches!(
        app.panels.pipeline(pipeline).unwrap().rows(&app.doc),
        Rows::Loading
    ));
    assert!(
        app.scene()
            .texts()
            .any(|t| t.starts_with("Loading cpu.thread0"))
    );
    for r in requests {
        app.deliver(r.perform());
    }
    pump(&mut app);
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    let Rows::Ready(set) = p.rows(&app.doc) else {
        panic!("loaded")
    };
    assert_eq!(set.len(), 40);
    assert_eq!(PipelineModel::label(set.get(0).unwrap().1), "00001000: op0");
    assert_eq!(PipelineModel::label(set.get(1).unwrap().1), "00001004: op1");
    assert_eq!(p.palette().names(), STAGES);
    let fills: Vec<_> = STAGES.iter().map(|s| p.palette().style(s).fill).collect();
    let layout = p.last_layout().clone();
    let viewport = p.nav.viewport(&app.doc);
    // One quad per visible lane-0 stage of the visible rows (each stage is one
    // cycle wide; the fit viewport shows the whole trace).
    let mut expected = 0;
    for r in layout.row_range.clone() {
        let (_, tx) = set.get(r).unwrap();
        expected += tx
            .stages
            .iter()
            .filter(|s| s.lane == "0")
            .filter(|s| {
                let end = s.end.unwrap_or(tx.end).max(s.begin);
                (end as f64) > viewport.start && (s.begin as f64) < viewport.end
            })
            .count();
    }
    let painted = app
        .scene()
        .quads()
        .filter(|(rect, color)| fills.contains(color) && layout.cells.contains(rect.origin))
        .count();
    assert_eq!(painted, expected);
    let bands = app
        .scene()
        .quads()
        .filter(|(rect, color)| {
            *color == theme.editor.text.with_alpha(0.28) && layout.cells.contains(rect.origin)
        })
        .count();
    let overlays = layout
        .row_range
        .clone()
        .filter(|r| set.get(*r).unwrap().1.stages.iter().any(|s| s.lane == "1"))
        .count();
    assert_eq!(bands, overlays);
    assert!(app.scene().texts().any(|t| t == "00001004: op1"));
    assert!(
        app.scene().texts().any(|t| t == "cycle"),
        "the producer's unit"
    );

    // The generator of the loaded stream is resident: opening it queues nothing.
    let generator = Member::Generator(0);
    app.handle(Command::ActivateMembers(vec![generator]));
    assert_eq!(app.panels.len(), 3);
    assert!(app.take_requests().is_empty());
    let second = app.panels.focused_id();
    frame(&mut app, second, &theme);
    assert!(matches!(
        app.panels.pipeline(second).unwrap().rows(&app.doc),
        Rows::Ready(set) if set.len() == 40
    ));
    // Activating the stream again focuses its panel instead of opening one.
    app.handle(Command::ActivateMembers(vec![Member::Stream(scope(
        session.as_ref(),
        &["cpu", "thread0"],
    ))]));
    assert_eq!(app.panels.len(), 3);
    assert_eq!(app.panels.focused_id(), pipeline);
    // An unrecognized stream kind opens all the same, as one stage-less cell.
    let bus = scope(session.as_ref(), &["cpu", "bus"]);
    app.handle(Command::ActivateMembers(vec![Member::Stream(bus)]));
    pump(&mut app);
    let bus_panel = app.panels.focused_id();
    assert_eq!(app.panels.len(), 4);
    frame(&mut app, bus_panel, &theme);
    let grey = volna_core::pipeline::StagePalette::fallback().fill;
    assert_eq!(app.scene().quads().filter(|(_, c)| *c == grey).count(), 1);
    assert!(app.panels.waves(waves).is_some());
    let state = app.debug_state();
    assert!(state.contains("pipeline track=cpu.thread0"));
    assert!(state.contains("ready rows=40"));
}

#[test]
fn zoom_about_the_pointer_keeps_time_and_row_at_both_interface_zooms() {
    for zoom in [1.0f32, 2.0] {
        let (mut app, _, _, pipeline) = opened(200);
        pump(&mut app);
        let theme = Theme::one_dark().zoomed(zoom);
        let cells = cells(&mut app, pipeline, &theme);
        frame(&mut app, pipeline, &theme);
        let pointer = point(cells.left() + cells.width() * 0.3, cells.top() + 137.0);
        let before = {
            let p = app.panels.pipeline(pipeline).unwrap();
            let l = p.last_layout();
            (
                l.time_at(&p.nav.viewport(&app.doc), pointer.x),
                l.rows.row_at(pointer.y - l.cells.top()),
                l.rows.row_px,
            )
        };
        let now = Instant::now();
        app.handle_at(
            Command::Pointer(
                pipeline,
                PointerEvent::Wheel {
                    position: pointer,
                    dx: 0.0,
                    dy: 100.0,
                    modifiers: Modifiers::default(),
                    precise: false,
                },
            ),
            now,
        );
        assert!(app.is_animating());
        app.tick(now + Duration::from_secs(1));
        frame(&mut app, pipeline, &theme);
        let after = {
            let p = app.panels.pipeline(pipeline).unwrap();
            let l = p.last_layout();
            (
                l.time_at(&p.nav.viewport(&app.doc), pointer.x),
                l.rows.row_at(pointer.y - l.cells.top()),
                l.rows.row_px,
            )
        };
        assert!((before.0 - after.0).abs() < 1e-6, "time under the pointer");
        assert!((before.1 - after.1).abs() < 1e-6, "row under the pointer");
        assert!((after.2 - before.2 * 2.0).abs() < 1e-3, "rows doubled");
        assert_eq!(
            app.panels.pipeline(pipeline).unwrap().rows.value.row_px,
            after.2 / zoom,
            "stored at interface zoom 1.0"
        );
        // A pinch is immediate and also keeps the anchor.
        app.handle(Command::Pointer(
            pipeline,
            PointerEvent::Pinch {
                position: pointer,
                delta: -0.5,
            },
        ));
        assert!(!app.is_animating());
        frame(&mut app, pipeline, &theme);
        let p = app.panels.pipeline(pipeline).unwrap();
        let l = p.last_layout();
        assert!((l.time_at(&p.nav.viewport(&app.doc), pointer.x) - before.0).abs() < 1e-6);
        assert!((l.rows.row_at(pointer.y - l.cells.top()) - before.1).abs() < 1e-6);
    }
}

#[test]
fn linked_navigation_moves_the_wave_panel_and_unlinked_navigation_does_not() {
    let (mut app, _, waves, pipeline) = opened(60);
    pump(&mut app);
    let theme = Theme::one_dark();
    let cells = cells(&mut app, pipeline, &theme);
    frame(&mut app, pipeline, &theme);
    let shared_before = app.doc.shared.viewport.value;
    let now = Instant::now();
    let wheel = |app: &mut App, now| {
        app.handle_at(
            Command::Pointer(
                pipeline,
                PointerEvent::Wheel {
                    position: point(cells.left() + 200.0, cells.top() + 50.0),
                    dx: 0.0,
                    dy: 60.0,
                    modifiers: Modifiers::default(),
                    precise: false,
                },
            ),
            now,
        );
        app.tick(now + Duration::from_secs(1));
    };
    wheel(&mut app, now);
    let shared_after = app.doc.shared.viewport.value;
    assert_ne!(shared_before, shared_after);
    assert_eq!(
        app.panels.waves(waves).unwrap().viewport(&app.doc),
        shared_after
    );
    app.handle(Command::Panels(PanelsCommand::ToggleLink {
        panel: pipeline,
        dim: LinkDim::Viewport,
    }));
    wheel(&mut app, now);
    assert_eq!(
        app.doc.shared.viewport.value, shared_after,
        "shared unchanged"
    );
    let local = app
        .panels
        .pipeline(pipeline)
        .unwrap()
        .nav
        .viewport(&app.doc);
    assert_ne!(local, shared_after);
    // Trackpad deltas pan both axes without animation.
    let rows_before = app.panels.pipeline(pipeline).unwrap().rows.value;
    app.handle_at(
        Command::Pointer(
            pipeline,
            PointerEvent::Wheel {
                position: point(cells.left() + 200.0, cells.top() + 50.0),
                dx: 40.0,
                dy: -30.0,
                modifiers: Modifiers::default(),
                precise: true,
            },
        ),
        now,
    );
    assert!(!app.is_animating());
    let p = app.panels.pipeline(pipeline).unwrap();
    assert!(
        p.nav.viewport(&app.doc).start < local.start,
        "content follows the fingers"
    );
    assert!(p.rows.value.top > rows_before.top);
    assert_eq!(p.rows.value.row_px, rows_before.row_px);
    // Ctrl with precise deltas zooms only time, exactly like the wave panel.
    let viewport_before = p.nav.viewport(&app.doc);
    let anchor_x = cells.left() + 200.0;
    let time_before = p.last_layout().time_at(&viewport_before, anchor_x);
    let rows_before = p.rows.value;
    app.panels.pipeline_mut(pipeline).unwrap().follow =
        volna_core::pipeline::FollowActivity::Following;
    app.handle_at(
        Command::Pointer(
            pipeline,
            PointerEvent::Wheel {
                position: point(anchor_x, cells.top() + 50.0),
                dx: 0.0,
                dy: 120.0,
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
                precise: true,
            },
        ),
        now,
    );
    let p = app.panels.pipeline(pipeline).unwrap();
    let viewport_after = p.nav.viewport(&app.doc);
    assert!((viewport_after.width() - viewport_before.width() / 2.0).abs() < 1e-6);
    assert!(
        (p.last_layout().time_at(&viewport_after, anchor_x) - time_before).abs() < 1e-6,
        "time under the pointer"
    );
    assert_eq!(p.rows.value, rows_before);
    assert_eq!(p.follow, volna_core::pipeline::FollowActivity::Following);
    let rows_before = p.rows.value;
    // Keyboard: ↓ scrolls rows, = zooms both axes.
    app.handle_at(Command::Action(Action::MoveSelectionDown), now);
    app.tick(now + Duration::from_secs(1));
    let p = app.panels.pipeline(pipeline).unwrap();
    assert!((p.rows.value.top - (rows_before.top + 3.0)).abs() < 1.0);
    let width = p.nav.viewport(&app.doc).width();
    let row_px = p.rows.value.row_px;
    app.handle_at(Command::Action(Action::ZoomIn), now);
    app.tick(now + Duration::from_secs(1));
    let p = app.panels.pipeline(pipeline).unwrap();
    assert!((p.nav.viewport(&app.doc).width() - width / 2.0).abs() < 1e-6);
    assert!((p.rows.value.row_px - (row_px * 2.0).min(48.0)).abs() < 1e-3);
}

#[test]
fn click_sets_the_shared_cursor_to_the_cycle_and_the_wave_value_follows() {
    let (mut app, _, waves, pipeline) = opened(60);
    pump(&mut app);
    let theme = Theme::one_dark();
    let cells = cells(&mut app, pipeline, &theme);
    frame(&mut app, pipeline, &theme);
    let x = cells.left() + cells.width() * 0.5;
    let expected = {
        let p = app.panels.pipeline(pipeline).unwrap();
        p.last_layout()
            .time_at(&p.nav.viewport(&app.doc), x)
            .floor() as u64
    };
    let position = point(x, cells.top() + 20.0);
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Down {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    assert!(app.panels.get(pipeline).unwrap().dragging());
    assert_eq!(app.doc.shared.cursor, None, "a press is not yet a click");
    app.handle(Command::Pointer(pipeline, PointerEvent::Up));
    assert_eq!(app.doc.shared.cursor, Some(expected));
    assert_eq!(
        app.panels.waves(waves).unwrap().cursor(&app.doc),
        Some(expected)
    );
    frame(&mut app, waves, &theme);
    let value = {
        let row = &app.panels.waves(waves).unwrap().items[0];
        let history = row.history.as_ref().unwrap();
        row.translator
            .translate(&history.value(history.index_at(expected)))
            .text
    };
    assert!(
        app.scene().texts().any(|t| t == value),
        "value column shows {value} at cycle {expected}"
    );
    assert!(
        app.scene()
            .texts()
            .any(|t| t == format!("{expected} cycle"))
    );
    frame(&mut app, pipeline, &theme);
    assert!(
        app.scene()
            .texts()
            .any(|t| t == format!("{expected} cycle"))
    );
    assert!(app.status().cursor.unwrap().ends_with("cycle"));
    // A drag past the threshold pans instead of setting the cursor.
    let before = app
        .panels
        .pipeline(pipeline)
        .unwrap()
        .nav
        .viewport(&app.doc);
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Down {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Move {
            position: point(position.x + 40.0, position.y + 10.0),
        },
    ));
    app.handle(Command::Pointer(pipeline, PointerEvent::Up));
    assert_eq!(app.doc.shared.cursor, Some(expected));
    let after = app
        .panels
        .pipeline(pipeline)
        .unwrap()
        .nav
        .viewport(&app.doc);
    assert!(after.start < before.start);
    // Hover names the instruction and the stage for the status bar.
    frame(&mut app, pipeline, &theme);
    let (row, hover_at) = {
        let p = app.panels.pipeline(pipeline).unwrap();
        let l = p.last_layout();
        let row = l.row_range.start + 1;
        let (_, tx) = match p.rows(&app.doc) {
            Rows::Ready(set) => set.get(row).unwrap(),
            _ => panic!(),
        };
        let vp = p.nav.viewport(&app.doc);
        let x =
            l.cells.left() + vp.x_of(tx.stages[1].begin as f64 + 0.5, l.cells_width_f64()) as f32;
        (row, point(x, l.row_y(row) + l.rows.row_px / 2.0))
    };
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Move { position: hover_at },
    ));
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.hover, Some(Hit::Cell { row, stage: 1 }));
    let hover = app.status().hover.unwrap();
    assert!(hover.starts_with(&format!("#{row} · D [")), "{hover}");
    assert!(hover.contains("op"), "{hover}");
    // The click also selected the row it landed on, and Escape unwinds the
    // selection before the cursor.
    assert!(app.doc.selection().is_some());
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(app.doc.selection(), None);
    assert_eq!(app.doc.shared.cursor, Some(expected));
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(app.doc.shared.cursor, None);
    // The label divider resizes the label column, stored at zoom 1.0.
    let split = app
        .panels
        .pipeline(pipeline)
        .unwrap()
        .last_layout()
        .label_split;
    let grab = point(split.left() + split.width() / 2.0, split.top() + 100.0);
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Down {
            position: grab,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Move {
            position: point(250.0, grab.y),
        },
    ));
    app.handle(Command::Pointer(pipeline, PointerEvent::Up));
    assert_eq!(app.panels.pipeline(pipeline).unwrap().label_width, 250.0);
}

#[test]
fn density_mode_bounds_the_painted_rows_by_pixels() {
    let (mut app, _, _, pipeline) = opened(10_000);
    pump(&mut app);
    let theme = Theme::one_dark();
    app.panels
        .pipeline_mut(pipeline)
        .unwrap()
        .rows
        .set(RowView {
            top: 0.0,
            row_px: 0.5,
        });
    app.handle(Command::Action(Action::ZoomFit));
    app.tick(Instant::now() + Duration::from_secs(1));
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    let layout = p.last_layout().clone();
    assert_eq!(layout.row_step, 4);
    assert_eq!(layout.rows.top, 0.0);
    // Every quad of a painted row lies within its density step.
    let step_px = layout.rows.row_px * layout.row_step as f32;
    let mut steps: Vec<i64> = app
        .scene()
        .quads()
        .filter(|(rect, _)| layout.cells.contains(rect.origin) && rect.height() <= 2.5)
        .map(|(rect, _)| ((rect.top() - layout.cells.top()) / step_px).floor() as i64)
        .collect();
    steps.sort_unstable();
    steps.dedup();
    assert!(!steps.is_empty());
    assert!(
        steps.len() <= (layout.cells.height() / 2.0) as usize + 1,
        "{} painted rows",
        steps.len()
    );
    // Flushed rows are ticks in the label column when labels cannot be read.
    let ticks = app
        .scene()
        .quads()
        .filter(|(rect, color)| *color == theme.editor.error && layout.labels.contains(rect.origin))
        .count();
    assert_eq!(ticks, 1);
    assert!(!app.scene().texts().any(|t| t.starts_with("00001004")));
}

#[test]
fn flushed_and_open_rows_are_told_apart_by_colour() {
    let (mut app, _, _, pipeline) = opened(12);
    pump(&mut app);
    let theme = Theme::one_dark();
    app.panels
        .pipeline_mut(pipeline)
        .unwrap()
        .rows
        .set(RowView {
            top: 0.0,
            row_px: 24.0,
        });
    frame(&mut app, pipeline, &theme);
    let p = app.panels.pipeline(pipeline).unwrap();
    let layout = p.last_layout().clone();
    let flush_tint = theme.editor.error.with_alpha(0.16);
    let tinted: Vec<f32> = app
        .scene()
        .quads()
        .filter(|(_, c)| *c == flush_tint)
        .map(|(r, _)| r.top())
        .collect();
    assert_eq!(tinted, [layout.row_y(3)], "row 3 was flushed");
    let dashed = app.scene().prims.iter().any(|prim| {
        matches!(prim, Prim::Lines { segments, .. }
            if segments.iter().all(|[a, b]| a.x == b.x
                && a.y >= layout.row_y(11) && b.y <= layout.row_y(12)))
    });
    assert!(dashed, "the open row ends with a dashed edge");
    // The flushed label is muted and marked; the open one muted.
    let muted = app
        .scene()
        .prims
        .iter()
        .filter(|prim| {
            matches!(prim, Prim::Text { text, color, .. }
                if text.starts_with("0000") && *color == theme.panel.text_muted)
        })
        .count();
    assert_eq!(muted, 2);
    assert!(app.status().hover.is_none());
}

#[test]
fn workspace_round_trip_keeps_pipeline_panels_and_unresolved_tracks_survive() {
    let (mut app, session, waves, pipeline) = opened(30);
    pump(&mut app);
    let theme = Theme::one_dark();
    frame(&mut app, pipeline, &theme);
    app.handle(Command::Panels(PanelsCommand::ToggleLink {
        panel: pipeline,
        dim: LinkDim::Cursor,
    }));
    {
        let p = app.panels.pipeline_mut(pipeline).unwrap();
        p.nav.local_cursor = Some(7);
        p.rows.set(RowView {
            top: 4.5,
            row_px: 12.0,
        });
        p.label_width = 240.0;
    }
    let saved = Workspace::capture(&app, "trace.vtr".into(), None).unwrap();
    let json = serde_json::to_value(&saved).unwrap();
    let panel_json = json["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "pipeline")
        .unwrap()
        .clone();
    assert_eq!(panel_json["track"], serde_json::json!(["cpu", "thread0"]));
    assert_eq!(panel_json["rows"]["row_px"], 12.0);
    assert_eq!(panel_json["cursor"], 7);
    assert_eq!(panel_json["label_width"], 240.0);
    // Restore into a fresh app over the same session.
    let mut restored = App::new();
    restored.set_session(session.clone());
    let plan = Workspace::parse(&saved.to_bytes().unwrap())
        .unwrap()
        .prepare(
            &restored,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )
        .unwrap();
    assert!(plan.report().notices.is_empty());
    plan.commit(&mut restored).unwrap();
    assert_eq!(restored.panels.len(), 2);
    let p = restored.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.track.path(), ["cpu", "thread0"]);
    assert!(p.is_attached());
    assert!(!p.nav.link.cursor && p.nav.link.viewport);
    assert_eq!(p.nav.local_cursor, Some(7));
    assert_eq!(
        p.rows.value,
        RowView {
            top: 4.5,
            row_px: 12.0
        }
    );
    assert_eq!(p.label_width, 240.0);
    assert!(restored.panels.waves(waves).is_some());
    // The restored panel retains the track and loads it.
    let requests = restored.take_requests();
    assert!(
        requests
            .iter()
            .any(|r| matches!(r, LoadRequest::Track { .. }))
    );
    for r in requests {
        restored.deliver(r.perform());
    }
    frame(&mut restored, pipeline, &theme);
    assert!(matches!(
        restored.panels.pipeline(pipeline).unwrap().rows(&restored.doc),
        Rows::Ready(set) if set.len() == 30
    ));
    // A track path that is not in the trace restores unresolved and is
    // written back unchanged.
    let mut altered = json.clone();
    for panel in altered["panels"].as_array_mut().unwrap() {
        if panel["kind"] == "pipeline" {
            panel["track"] = serde_json::json!(["gone", "stream"]);
        }
    }
    let mut other = App::new();
    other.set_session(session.clone());
    let plan = Workspace::parse(serde_json::to_string(&altered).unwrap().as_bytes())
        .unwrap()
        .prepare(
            &other,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )
        .unwrap();
    assert!(
        plan.report()
            .notices
            .iter()
            .any(|n| n.contains("Missing pipeline track"))
    );
    plan.commit(&mut other).unwrap();
    let p = other.panels.pipeline(pipeline).unwrap();
    assert_eq!(
        p.track,
        TrackSource::Unresolved {
            path: vec!["gone".into(), "stream".into()]
        }
    );
    assert!(
        !other
            .take_requests()
            .iter()
            .any(|r| matches!(r, LoadRequest::Track { .. })),
        "an unresolved track loads nothing"
    );
    frame(&mut other, pipeline, &theme);
    assert!(other.scene().texts().any(|t| t == "Not in this trace"));
    let again = serde_json::to_value(Workspace::capture(&other, "trace.vtr".into(), None).unwrap())
        .unwrap();
    let kept = again["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "pipeline")
        .unwrap();
    assert_eq!(kept["track"], serde_json::json!(["gone", "stream"]));
    // Invalid saved rows are rejected as a whole.
    let mut bad = json.clone();
    for panel in bad["panels"].as_array_mut().unwrap() {
        if panel["kind"] == "pipeline" {
            panel["rows"]["row_px"] = serde_json::json!(1000.0);
        }
    }
    assert!(
        Workspace::parse(serde_json::to_string(&bad).unwrap().as_bytes())
            .unwrap()
            .prepare(
                &other,
                "file:///tmp/trace.vtr",
                "file:///tmp/trace.vtr.volna.json"
            )
            .is_err()
    );
}

#[test]
fn closing_the_last_panel_releases_the_track_and_late_delivery_is_ignored() {
    let (mut app, session, waves, pipeline) = opened(20);
    let track = stream_track(session.as_ref(), &["cpu", "thread0"]);
    let request = app
        .take_requests()
        .into_iter()
        .find(|r| matches!(r, LoadRequest::Track { .. }))
        .unwrap();
    // A split shares the load; closing one panel keeps the track.
    app.handle(Command::Action(Action::SplitRight));
    let split = app.panels.focused_id();
    assert!(app.panels.pipeline(split).unwrap().is_attached());
    assert!(app.take_requests().is_empty());
    app.handle(Command::Panels(PanelsCommand::Close(split)));
    assert!(app.doc.track(track).is_some());
    app.handle(Command::Panels(PanelsCommand::Close(pipeline)));
    assert!(
        app.doc.track(track).is_none(),
        "released with its last consumer"
    );
    assert_eq!(app.panels.focused_id(), waves);
    app.deliver(request.perform());
    assert!(app.doc.track(track).is_none(), "late delivery is a no-op");
    // Close-others from the wave panel releases every pipeline panel.
    app.handle(Command::ActivateMembers(vec![Member::Generator(0)]));
    let generator = app
        .panels
        .pipeline(app.panels.focused_id())
        .unwrap()
        .track
        .track()
        .unwrap();
    app.handle(Command::Panels(PanelsCommand::CloseOthers(waves)));
    assert_eq!(app.panels.len(), 1);
    assert!(app.doc.track(generator).is_none());
    // A failed load shows the error and the retry button reloads.
    let mut failing = App::new();
    failing.set_session(session.clone());
    failing.handle(Command::OpenPipeline { track });
    let request = failing.take_requests().pop().unwrap();
    let LoadRequest::Track {
        generation,
        request_id,
        track,
        ..
    } = request
    else {
        panic!()
    };
    failing.deliver(volna_core::session::LoadResult::Track {
        generation,
        request_id,
        track,
        result: Err(anyhow::anyhow!("disk on fire")),
    });
    let id = failing.panels.focused_id();
    let theme = Theme::one_dark();
    frame(&mut failing, id, &theme);
    assert!(failing.scene().texts().any(|t| t == "disk on fire"));
    let retry = failing
        .panels
        .pipeline(id)
        .unwrap()
        .last_layout()
        .retry
        .unwrap();
    failing.handle(Command::Pointer(
        id,
        PointerEvent::Down {
            position: Point {
                x: retry.left() + 2.0,
                y: retry.top() + 2.0,
            },
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    assert!(matches!(
        failing.take_requests().as_slice(),
        [LoadRequest::Track { .. }]
    ));
}

#[test]
fn a_stream_opened_first_replaces_the_start_panel_and_rows_find_a_waveform_tab() {
    let session = fixture(20);
    let mut app = App::new();
    app.set_session(session.clone());
    let start = app.panels.focused_id();
    assert!(app.panels.focused().kind.is_start());
    let track = stream_track(session.as_ref(), &["cpu", "thread0"]);
    app.handle(Command::OpenPipeline { track });
    assert_eq!(app.panels.len(), 1);
    let pipeline = app.panels.focused_id();
    assert_ne!(pipeline, start);
    assert!(app.panels.pipeline(pipeline).unwrap().is_attached());
    pump(&mut app);
    assert!(app.doc.track(track).is_some());
    // Rows added while the pipeline is focused open a waveform tab beside it.
    app.handle(Command::AddVars(vec![0]));
    assert_eq!(app.panels.len(), 2);
    let waves = app.panels.focused_id();
    assert_eq!(app.panels.waves(waves).unwrap().items.len(), 1);
    assert_eq!(
        app.panels.layout(),
        &Layout::Tabs {
            tabs: vec![pipeline, waves],
            active: waves
        }
    );
    // More rows with the pipeline focused reuse and focus that panel.
    app.handle(Command::Panels(PanelsCommand::Focus(pipeline)));
    app.handle(Command::AddVars(vec![0]));
    assert_eq!(app.panels.len(), 2);
    assert_eq!(app.panels.focused_id(), waves);
    assert_eq!(app.panels.waves(waves).unwrap().items.len(), 2);
    // Closing both leaves a start panel and releases the track.
    app.handle(Command::Panels(PanelsCommand::Close(waves)));
    app.handle(Command::Panels(PanelsCommand::Close(pipeline)));
    assert_eq!(app.panels.len(), 1);
    assert!(app.panels.focused().kind.is_start());
    assert!(app.doc.track(track).is_none());
    app.panels.validate().unwrap();
}

#[test]
fn the_checked_in_showcase_has_two_pipeline_streams_on_a_cycle_time_base() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../volna/examples/pipeline_showcase.vtr");
    let session = OpenSpec::Path(path).open().unwrap();
    assert_eq!(session.info().time_unit.as_deref(), Some("cycle"));
    let mut app = App::new();
    app.set_session(session.clone());
    let theme = Theme::one_dark();
    let mut counts = Vec::new();
    for core in ["cpu0", "cpu1"] {
        let stream = scope(session.as_ref(), &["soc", core, "pipeline"]);
        app.handle(Command::ActivateMembers(vec![Member::Stream(stream)]));
        pump(&mut app);
        let id = app.panels.focused_id();
        frame(&mut app, id, &theme);
        let p = app.panels.pipeline(id).unwrap();
        let Rows::Ready(set) = p.rows(&app.doc) else {
            panic!("{core} loaded")
        };
        counts.push(set.len());
        assert!(
            p.palette().names().len() >= 5,
            "{core}: {:?}",
            p.palette().names()
        );
        assert_eq!(p.palette().primary_lane(), "0");
        let flushed = (0..set.len())
            .filter(|r| set.get(*r).unwrap().1.status == vtr::TxStatus::Aborted)
            .count();
        let open = (0..set.len())
            .filter(|r| set.get(*r).unwrap().1.status == vtr::TxStatus::Open)
            .count();
        assert!(
            flushed > 0 && open > 0,
            "{core}: {flushed} flushed, {open} open"
        );
        assert!(app.scene().texts().any(|t| t.contains(": lw   a1, 0(a2)")));
    }
    // Without rows, the first stream takes the start panel's place: no
    // waveform panel is opened for a transaction-only session.
    assert_eq!(app.panels.len(), 2);
    assert!(app.panels.iter().all(|p| p.kind.pipeline().is_some()));
    assert!(counts.iter().all(|n| *n > 500), "{counts:?}");
    assert!(app.status().time_range.unwrap().ends_with("cycle"));
}
