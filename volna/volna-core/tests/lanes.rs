//! Headless tests of transaction lanes in the waveform panel, over the
//! checked-in example traces: adding a generator as a row, sharing its
//! records with other panels, stacking and default heights, the document
//! selection, cursor snapping and edge steps, row operations, workspaces,
//! and the painted bars, folds and density strip.

use std::path::PathBuf;
use std::sync::Arc;

use volna_core::Theme;
use volna_core::app::{Action, App, Command};
use volna_core::data::Member;
use volna_core::data::transactions::{TrackRef, TxStatus};
use volna_core::document::TrackLoadState;
use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::panels::PanelId;
use volna_core::scene::{MonoMeasure, Prim};
use volna_core::session::{OpenSpec, Session};
use volna_core::wave::PointerEvent;
use volna_core::wave::lane::{self, LaneGeometry};
use volna_core::wave::model::{MenuAction, RowHeight, WaveRow};
use volna_core::wave::viewport::Viewport;
use volna_core::workspace::Workspace;

const TRACE: &str = "file:///tmp/trace.vtr";
const LOCATION: &str = "file:///tmp/trace.vtr.volna.json";

fn example(name: &str) -> Arc<dyn Session> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../volna/examples")
        .join(name);
    OpenSpec::Path(path).open().unwrap()
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

fn frame(app: &mut App, id: PanelId) -> &volna_core::scene::Scene {
    frame_w(app, id, 1400.0)
}

fn frame_w(app: &mut App, id: PanelId, width: f32) -> &volna_core::scene::Scene {
    let theme = Theme::one_dark();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, width, 600.0), &theme)
        .unwrap();
    app.render_panel(id, &theme, &mut MonoMeasure)
}

fn track(session: &dyn Session, path: &str) -> TrackRef {
    session
        .tracks()
        .iter()
        .find(|t| t.path.join(".") == path)
        .unwrap_or_else(|| panic!("no track {path}"))
        .id
}

/// The hierarchy member of the generator `track`.
fn generator(session: &dyn Session, track: TrackRef) -> Member {
    Member::Generator(
        session
            .hierarchy()
            .generators
            .iter()
            .position(|g| g.track == track)
            .unwrap(),
    )
}

/// `pipeline_showcase.vtr` with the given generators added as lanes.
fn with_lanes(paths: &[&str]) -> (App, Arc<dyn Session>, PanelId, Vec<TrackRef>) {
    let session = example("pipeline_showcase.vtr");
    let mut app = App::new();
    app.set_session(session.clone());
    let tracks: Vec<_> = paths.iter().map(|p| track(session.as_ref(), p)).collect();
    app.handle(Command::AddToWaves(
        tracks
            .iter()
            .map(|t| generator(session.as_ref(), *t))
            .collect(),
    ));
    let waves = app.panels.focused_id();
    pump(&mut app);
    frame(&mut app, waves);
    (app, session, waves, tracks)
}

const CPU0: &str = "soc.cpu0.pipeline.instruction";
const CPU1: &str = "soc.cpu1.pipeline.instruction";

/// Show `start..end` in the panel and lay it out again.
fn show(app: &mut App, waves: PanelId, start: f64, end: f64) {
    let volna_core::App { panels, doc, .. } = app;
    panels
        .waves_mut(waves)
        .unwrap()
        .nav
        .jump_to(doc, Viewport { start, end });
    frame(app, waves);
}

/// Panel point in the middle of record `ordinal` of lane row `row`.
fn bar_point(app: &App, waves: PanelId, row: usize, ordinal: usize) -> Point {
    let w = app.panels.waves(waves).unwrap();
    let lane = w.items[row].lane().unwrap();
    let g = lane.generator(&app.doc).unwrap();
    let tx = &g.transactions()[ordinal];
    let layout = w.last_layout();
    let vp = w.viewport(&app.doc);
    let width = layout.wave_width_f64();
    let mid = (tx.begin + tx.end) as f64 / 2.0;
    let geometry = LaneGeometry::new(
        layout.row_y(row),
        layout.row_height(row),
        layout.row_h,
        lane.height,
    );
    let sub = lane::shown_sub_row(g.sub_row(ordinal), lane.height);
    point(
        layout.waves.left() + vp.x_of(mid, width) as f32,
        geometry.sub_top(sub) + geometry.sub_h / 2.0,
    )
}

fn click(app: &mut App, waves: PanelId, p: Point) {
    app.handle(Command::Pointer(
        waves,
        PointerEvent::Down {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(waves, PointerEvent::Up));
}

#[test]
fn add_to_waves_loads_shares_and_releases_generator_records() {
    let (mut app, session, waves, tracks) = with_lanes(&[CPU0, CPU1]);
    let w = app.panels.waves(waves).unwrap();
    assert_eq!(w.items.len(), 2);
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [0, 1]);
    // Depths 5 and 16: the smallest preset that fits, capped at 4×.
    let heights: Vec<_> = w.items.iter().map(|r| r.height().multiple()).collect();
    assert_eq!(heights, [4, 4]);
    let cpu1 = w.items[1].lane().unwrap();
    assert_eq!(cpu1.generator(&app.doc).unwrap().depth(), 16);
    assert_eq!(lane::folded(16, cpu1.height), 11);

    // A Pipeline panel over the same generator shares the one object.
    app.handle(Command::OpenPipeline { track: tracks[0] });
    pump(&mut app);
    let pipeline = app.panels.focused_id();
    let shared = |app: &App| match app.doc.track(tracks[0]) {
        Some(TrackLoadState::Ready(loaded)) => loaded.generators[0].clone(),
        _ => panic!("not resident"),
    };
    let object = shared(&app);
    let lane_object = app.panels.waves(waves).unwrap().items[0]
        .lane()
        .unwrap()
        .generator(&app.doc)
        .unwrap() as *const _;
    assert!(std::ptr::eq(Arc::as_ptr(&object), lane_object));

    // Removing the lanes keeps records a pipeline still shows, and frees
    // the rest.
    app.handle(Command::Panels(volna_core::panels::PanelsCommand::Focus(
        waves,
    )));
    app.panels.waves_mut(waves).unwrap().selected = [0, 1].into();
    app.handle(Command::Action(Action::RemoveSelected));
    assert!(app.panels.waves(waves).unwrap().items.is_empty());
    assert!(matches!(
        app.doc.track(tracks[0]),
        Some(TrackLoadState::Ready(_))
    ));
    assert!(app.doc.track(tracks[1]).is_none());
    app.handle(Command::Panels(volna_core::panels::PanelsCommand::Close(
        pipeline,
    )));
    assert!(app.doc.track(tracks[0]).is_none());

    // Streams and variables: variables become signal rows, streams have
    // no row form.
    let h = session.hierarchy();
    let stream = h
        .scopes
        .iter()
        .position(|s| matches!(s.role, volna_core::data::ScopeRole::Stream { .. }))
        .unwrap();
    app.handle(Command::Panels(volna_core::panels::PanelsCommand::Focus(
        waves,
    )));
    app.handle(Command::AddToWaves(vec![
        Member::Stream(stream),
        Member::Var(0),
    ]));
    let w = app.panels.waves(waves).unwrap();
    assert_eq!(w.items.len(), 1);
    assert!(w.items[0].signal().is_some());
}

#[test]
fn clicking_a_bar_selects_the_record_everywhere_and_empty_space_clears_it() {
    let (mut app, _session, waves, tracks) = with_lanes(&[CPU0]);
    let g = app.panels.waves(waves).unwrap().items[0]
        .lane()
        .unwrap()
        .generator(&app.doc)
        .unwrap()
        .transactions()
        .to_vec();
    // A record on sub-row 1, so the click must find the right sub-row.
    let ordinal = (0..g.len())
        .find(|&i| {
            app.panels.waves(waves).unwrap().items[0]
                .lane()
                .unwrap()
                .generator(&app.doc)
                .unwrap()
                .sub_row(i)
                == 1
        })
        .unwrap();
    let tx = &g[ordinal];
    show(
        &mut app,
        waves,
        tx.begin as f64 - 20.0,
        tx.end as f64 + 40.0,
    );
    let p = bar_point(&app, waves, 0, ordinal);
    click(&mut app, waves, p);
    let selection = app.doc.selection().expect("selected");
    assert_eq!(
        (selection.track, selection.id, selection.origin),
        (tracks[0], tx.id, waves)
    );
    assert!(app.panels.waves(waves).unwrap().pressed_record);
    // The cursor lands where the press was, snapped to a boundary nearby.
    let cursor = app.panels.waves(waves).unwrap().cursor(&app.doc).unwrap();
    assert!(cursor >= tx.begin && cursor <= tx.end);

    // Enter (Show Transaction) opens the Transaction panel on it.
    app.handle(Command::ShowTransaction { from: waves });
    let panel = app.panels.focused_id();
    assert!(app.panels.transaction(panel).is_some());

    // The highlight is painted with the focus colour.
    let theme = Theme::one_dark();
    let scene = frame(&mut app, waves);
    assert!(scene.prims.iter().any(|prim| matches!(
        prim,
        Prim::Quad { border_width, border_color, .. } if *border_width == 2.0 && *border_color == theme.border_focused
    )));

    // A press on empty lane space (below the stack) clears the selection.
    let layout = app.panels.waves(waves).unwrap().last_layout().clone();
    let empty = point(
        layout.waves.left() + 5.0,
        layout.row_y(0) + layout.row_height(0) - 2.0,
    );
    click(&mut app, waves, empty);
    assert!(app.doc.selection().is_none());
    assert!(!app.panels.waves(waves).unwrap().pressed_record);

    // Clicking near a bar end snaps the cursor to it.
    show(
        &mut app,
        waves,
        tx.begin as f64 - 20.0,
        tx.end as f64 + 40.0,
    );
    let w = app.panels.waves(waves).unwrap();
    let layout = w.last_layout().clone();
    let vp = w.viewport(&app.doc);
    let x = layout.waves.left() + vp.x_of(tx.end as f64, layout.wave_width_f64()) as f32 + 2.0;
    click(&mut app, waves, point(x, p.y));
    assert_eq!(
        app.panels.waves(waves).unwrap().cursor(&app.doc),
        Some(tx.end)
    );
}

#[test]
fn edge_steps_walk_record_begins_and_ends_on_a_lane() {
    let (mut app, _session, waves, _tracks) = with_lanes(&[CPU0]);
    let mut bounds: Vec<u64> = {
        let g = app.panels.waves(waves).unwrap().items[0]
            .lane()
            .unwrap()
            .generator(&app.doc)
            .unwrap();
        g.transactions()
            .iter()
            .flat_map(|t| [t.begin, t.end])
            .collect()
    };
    bounds.sort();
    bounds.dedup();
    let cursor = |app: &App| app.panels.waves(waves).unwrap().cursor(&app.doc);
    app.panels.waves_mut(waves).unwrap().selected = [0].into();
    app.panels.waves_mut(waves).unwrap().anchor = Some(0);
    let volna_core::App { panels, doc, .. } = &mut app;
    panels.waves_mut(waves).unwrap().set_cursor(doc, Some(0));
    for &expected in &bounds[..12] {
        app.handle(Command::Action(Action::NextEdge));
        assert_eq!(cursor(&app), Some(expected));
    }
    for &expected in bounds[..11].iter().rev() {
        app.handle(Command::Action(Action::PrevEdge));
        assert_eq!(cursor(&app), Some(expected));
    }
}

#[test]
fn lanes_cut_paste_resize_and_round_trip_through_the_workspace() {
    let (mut app, session, waves, tracks) = with_lanes(&[CPU0]);
    app.handle(Command::AddVars(vec![0]));
    assert_eq!(app.panels.waves(waves).unwrap().items.len(), 2);

    // Lanes cut and paste with signals, into another wave panel.
    app.panels.waves_mut(waves).unwrap().selected = [0, 1].into();
    app.handle(Command::Action(Action::CutSignals));
    assert!(app.panels.waves(waves).unwrap().items.is_empty());
    assert!(app.doc.track(tracks[0]).is_none(), "no lane shows it");
    app.handle(Command::Action(Action::SplitRight));
    let other = app.panels.focused_id();
    app.handle(Command::Action(Action::PasteSignals));
    pump(&mut app);
    let w = app.panels.waves(other).unwrap();
    assert!(matches!(w.items[0].row, WaveRow::Lane(_)));
    assert_eq!(w.items[0].height().multiple(), 4);
    assert!(w.items[0].lane().unwrap().generator(&app.doc).is_some());

    // The Height menu applies to lanes; folded sub-rows then show a chip.
    frame(&mut app, other);
    app.panels.waves_mut(other).unwrap().selected = [0].into();
    app.handle(Command::OpenSignalMenu(other));
    let menu = app.panels.waves(other).unwrap().menu.clone().unwrap();
    assert!(menu.items().any(|i| i.label == "Remove lane"));
    app.handle(Command::MenuSelect(
        other,
        MenuAction::RowHeight(RowHeight::DEFAULT),
    ));
    let lane = app.panels.waves(other).unwrap().items[0]
        .lane()
        .unwrap()
        .clone();
    assert_eq!(lane.height, RowHeight::DEFAULT);
    assert!(!lane.auto_height);
    let scene = frame(&mut app, other);
    assert!(scene.texts().any(|t| t == "+4"));

    // Workspaces save lanes as typed rows and restore them, heights intact.
    let saved =
        serde_json::to_value(Workspace::capture(&app, "trace.vtr".into(), None).unwrap()).unwrap();
    let rows = saved["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == other.0)
        .unwrap()["rows"]
        .clone();
    assert_eq!(rows[0]["type"], "lane");
    assert_eq!(
        rows[0]["generator"],
        serde_json::json!(["soc", "cpu0", "pipeline", "instruction"])
    );
    assert_eq!(rows[1]["type"], "signal");
    let restore = |app: &mut App, value: &serde_json::Value| {
        Workspace::parse(&serde_json::to_vec(value).unwrap())
            .unwrap()
            .prepare(app, TRACE, LOCATION)
            .unwrap()
            .commit(app)
            .unwrap()
    };
    restore(&mut app, &saved);
    pump(&mut app);
    let w = app.panels.waves(other).unwrap();
    let lane = w.items[0].lane().unwrap();
    assert_eq!(lane.height, RowHeight::DEFAULT);
    assert!(lane.generator(&app.doc).is_some(), "restored lanes load");
    assert_eq!(
        serde_json::to_value(Workspace::capture(&app, "trace.vtr".into(), None).unwrap()).unwrap(),
        saved
    );

    // A generator the trace lacks restores as an empty, reported row.
    let mut missing = saved.clone();
    for panel in missing["panels"].as_array_mut().unwrap() {
        if panel["id"] == other.0 {
            panel["rows"][0]["generator"] = serde_json::json!(["no", "such"]);
        }
    }
    let report = restore(&mut app, &missing);
    assert!(report.notices.iter().any(|n| n.contains("Missing lane")));
    let w = app.panels.waves(other).unwrap();
    assert!(w.items[0].lane().unwrap().track().is_none());
    let scene = frame(&mut app, other);
    assert!(scene.texts().any(|t| t == "not in trace"));
    drop(session);
}

#[test]
fn bars_fold_into_hatching_and_zoom_out_to_a_density_strip() {
    let (mut app, _session, waves, _tracks) = with_lanes(&[CPU1]);
    let theme = Theme::one_dark();
    let (median, depth, first) = {
        let g = app.panels.waves(waves).unwrap().items[0]
            .lane()
            .unwrap()
            .generator(&app.doc)
            .unwrap();
        (g.median_lifetime(), g.depth(), g.transactions()[0].clone())
    };
    assert!(depth > 5);

    // Zoomed in, bars carry captions; the name cell counts folded rows.
    show(&mut app, waves, 0.0, (median * 6) as f64);
    let scene = frame(&mut app, waves);
    let label = lane::label(&first);
    let caption = |t: &str| t.starts_with("800000") && t.contains(": ");
    assert!(
        scene.texts().filter(|t| caption(t)).count() > 5,
        "bar captions"
    );
    assert!(scene.texts().any(|t| t == format!("+{}", depth - 5)));
    assert!(scene.texts().any(|t| t.ends_with("records")));
    let bar_quads = scene
        .quads()
        .filter(|(_, c)| *c == theme.wave_signal)
        .count();
    assert!(bar_quads > 5);
    // Folded overlaps are hatched.
    assert!(
        scene
            .prims
            .iter()
            .any(|prim| matches!(prim, Prim::Lines { .. }))
    );

    // The value column lists what is open at the cursor.
    let volna_core::App { panels, doc, .. } = &mut app;
    panels
        .waves_mut(waves)
        .unwrap()
        .set_cursor(doc, Some(first.begin));
    let expected = {
        let g = app.panels.waves(waves).unwrap().items[0]
            .lane()
            .unwrap()
            .generator(&app.doc)
            .unwrap();
        lane::value_text(g, first.begin).0
    };
    assert!(expected.contains(&label));
    let scene = frame(&mut app, waves);
    assert!(
        scene
            .texts()
            .any(|t| t.len() > 8 && expected.starts_with(t.trim_end_matches('…'))),
        "value {expected}"
    );

    // Zoomed out over the whole trace in a narrow panel, one column per
    // pixel: no captions, and at most one quad per column.
    let end = app.doc.limits().1 as f64;
    show(&mut app, waves, 0.0, end);
    frame_w(&mut app, waves, 520.0);
    let (density, columns_w) = {
        let w = app.panels.waves(waves).unwrap();
        let g = w.items[0].lane().unwrap().generator(&app.doc).unwrap();
        let width = w.last_layout().wave_width_f64();
        (
            lane::is_density(g, w.viewport(&app.doc).px_per_unit(width)),
            width,
        )
    };
    assert!(density, "whole trace in {columns_w} px is a density strip");
    let scene = frame_w(&mut app, waves, 520.0);
    assert!(!scene.texts().any(caption));
    let columns = scene
        .quads()
        .filter(|(_, c)| c.h == theme.wave_signal.h && c.s == theme.wave_signal.s && c.a < 1.0)
        .count();
    assert!(
        columns > 10 && columns as f64 <= columns_w,
        "{columns} density runs"
    );
    // Bars are not hit in the strip.
    let p = {
        let layout = app.panels.waves(waves).unwrap().last_layout();
        point(layout.waves.left() + 50.0, layout.row_y(0) + 10.0)
    };
    click(&mut app, waves, p);
    assert!(app.doc.selection().is_none());
}

#[test]
fn a_failed_record_is_red_in_bars_and_values() {
    let session = example("feature_showcase.vtr");
    let mut app = App::new();
    app.set_session(session.clone());
    let read = track(session.as_ref(), "soc.dma.memory_bus.read");
    app.handle(Command::AddToWaves(vec![generator(session.as_ref(), read)]));
    let waves = app.panels.focused_id();
    pump(&mut app);
    let failed = {
        let g = app.panels.waves(waves).unwrap().items[0]
            .lane()
            .unwrap()
            .generator(&app.doc)
            .unwrap();
        g.transactions()
            .iter()
            .find(|t| t.status == TxStatus::Error)
            .unwrap()
            .clone()
    };
    let theme = Theme::one_dark();
    show(
        &mut app,
        waves,
        failed.begin as f64 - 10.0,
        failed.end as f64 + 10.0,
    );
    let volna_core::App { panels, doc, .. } = &mut app;
    panels
        .waves_mut(waves)
        .unwrap()
        .set_cursor(doc, Some(failed.begin + 1));
    let scene = frame(&mut app, waves);
    assert!(
        scene.quads().any(|(_, c)| c == theme.editor.error),
        "red bar"
    );
    assert!(scene.prims.iter().any(|prim| matches!(
        prim,
        Prim::Text { text, color, .. } if *color == theme.editor.error && text.starts_with("DMA transfer")
    )));
}
