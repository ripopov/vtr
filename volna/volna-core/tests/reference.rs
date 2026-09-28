//! The reference and the Measure lane: `R` measures from the cursor, `⇧R`
//! stops, Alt-click and a middle click measure from the pointer, `Z` zooms
//! to the measurement. The reference follows its marker, stays where a
//! removed marker was, is saved in workspaces, and never reaches the undo
//! journal.

use std::sync::Arc;

use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::marker::{MarkerId, Reference};
use volna_core::testing::ProceduralTrace;
use volna_core::wave::PointerEvent;
use volna_core::wave::overlay::MarkerLane;
use volna_core::wave::viewport::Viewport;
use volna_core::workspace::Workspace;
use volna_core::{Action, App, Command, Theme};

fn app() -> App {
    let mut app = App::new();
    app.set_session(Arc::new(ProceduralTrace::new(1000)));
    app.handle(Command::AddVars(vec![0, 1]));
    app.doc.shared.viewport.set(Viewport {
        start: 1000.0,
        end: 2000.0,
    });
    app
}

fn id(n: u32) -> MarkerId {
    MarkerId::new(n).unwrap()
}

fn act(app: &mut App, action: Action) {
    app.handle(Command::Action(action));
}

fn mark(app: &mut App, t: u64) {
    app.doc.shared.cursor = Some(t);
    act(app, Action::AddOrRenameMarker);
}

fn said(app: &App) -> Option<String> {
    app.status().announcement
}

fn frame<'a>(app: &'a mut App, theme: &Theme) -> &'a volna_core::Scene {
    let id = app.panels.focused_id();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 600.0), theme);
    app.render_panel(id, theme, &mut volna_core::scene::MonoMeasure)
}

fn lane(app: &mut App) -> MarkerLane {
    frame(app, &Theme::one_dark());
    app.panels
        .focused()
        .kind
        .marker_lane()
        .expect("a timed panel")
        .clone()
}

/// Panel x of time `t` in the focused panel's time column.
fn x_of(lane: &MarkerLane, app: &App, t: u64) -> f32 {
    let right = lane.band.right();
    let v = app.doc.shared.viewport.target();
    let w = f64::from(right - lane.time_left);
    lane.time_left + v.x_of(t as f64, w) as f32
}

fn press(app: &mut App, p: Point, button: MouseButton, alt: bool) {
    let panel = app.panels.focused_id();
    app.handle(Command::Pointer(
        panel,
        PointerEvent::Down {
            position: p,
            button,
            modifiers: Modifiers {
                alt,
                ..Default::default()
            },
        },
    ));
}

fn pointer(app: &mut App, event: PointerEvent) {
    let panel = app.panels.focused_id();
    app.handle(Command::Pointer(panel, event));
}

#[test]
fn r_measures_from_the_cursor_and_shift_r_stops() {
    let mut app = app();
    act(&mut app, Action::SetReference);
    assert_eq!(app.doc.reference(), None);
    assert_eq!(said(&app).as_deref(), Some("Place the cursor first"));

    mark(&mut app, 1200);
    act(&mut app, Action::SetReference);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(1))));
    assert_eq!(said(&app).as_deref(), Some("Measuring from marker 1"));
    app.doc.shared.cursor = Some(1500);
    act(&mut app, Action::SetReference);
    assert_eq!(app.doc.reference(), Some(Reference::Time(1500)));
    assert_eq!(app.doc.reference_time(), Some(1500));
    assert!(said(&app).unwrap().starts_with("Measuring from "));
    // Marking the cursor does not move the reference, nor attach it.
    act(&mut app, Action::AddOrRenameMarker);
    assert_eq!(app.doc.reference(), Some(Reference::Time(1500)));

    act(&mut app, Action::ClearReference);
    assert_eq!(app.doc.reference(), None);
    assert_eq!(said(&app).as_deref(), Some("Reference cleared"));
    // None of it is an edit: the last undo step is still adding marker 2.
    assert_eq!(app.undo_label(), Some("Add marker 2"));
}

#[test]
fn hosts_reach_the_reference_by_name() {
    for (name, action) in [
        ("setReference", Action::SetReference),
        ("clearReference", Action::ClearReference),
        ("zoomToMeasurement", Action::ZoomToMeasurement),
    ] {
        assert_eq!(Command::named(name), Some(Command::Action(action)));
    }
}

#[test]
fn the_reference_follows_its_marker_and_stays_when_the_marker_goes() {
    let mut app = app();
    mark(&mut app, 1200);
    mark(&mut app, 1600);
    app.doc.shared.cursor = Some(1600);
    act(&mut app, Action::SetReference);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(2))));

    act(&mut app, Action::RemoveMarkerAtCursor);
    assert_eq!(app.doc.reference(), Some(Reference::Time(1600)));
    // Undo brings the marker back but not the attachment: the reference
    // is navigation state, and it still reads the same time.
    act(&mut app, Action::ClearSelection);
    app.handle(Command::Undo);
    assert_eq!(app.doc.markers().len(), 2);
    assert_eq!(app.doc.reference(), Some(Reference::Time(1600)));

    // Undoing the add of the reference's marker detaches it too.
    app.doc.shared.cursor = Some(1800);
    act(&mut app, Action::AddOrRenameMarker);
    act(&mut app, Action::SetReference);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(3))));
    app.handle(Command::Undo);
    assert_eq!(app.doc.markers().len(), 2);
    assert_eq!(app.doc.reference(), Some(Reference::Time(1800)));

    // Removing all markers keeps it at its marker's time as well.
    app.doc.shared.cursor = Some(1200);
    act(&mut app, Action::SetReference);
    act(&mut app, Action::RemoveAllMarkers);
    assert_eq!(app.doc.reference(), Some(Reference::Time(1200)));
    // A marker reference must name a marker.
    assert!(!app.doc.set_reference(Some(Reference::Marker(id(9)))));
}

#[test]
fn the_measure_lane_appears_with_a_reference_and_joins_it_to_the_cursor() {
    let mut app = app();
    let before = lane(&mut app);
    assert!(before.measure.is_none());
    assert_eq!(before.strip(), before.band);
    let rows_top = app
        .panels
        .focused_waves()
        .unwrap()
        .last_layout()
        .waves
        .top();

    app.doc.shared.cursor = Some(1300);
    act(&mut app, Action::SetReference);
    app.doc.shared.cursor = Some(1700);
    let l = lane(&mut app);
    let m = l.measure.clone().expect("the Measure lane");
    assert_eq!(m.band.top(), l.band.bottom());
    assert_eq!(m.band.height(), l.band.height());
    let waves = app.panels.focused_waves().unwrap().last_layout().waves;
    assert_eq!(
        waves.top(),
        rows_top + m.band.height(),
        "the rows move down"
    );
    // The tag is centred on the reference; the live span runs from it to
    // the cursor, measured forward.
    let xr = x_of(&l, &app, 1300);
    assert!((m.tag.left() + m.tag.width() / 2.0 - xr).abs() <= 1.0);
    let live = m.live.clone().expect("a cursor away from the reference");
    assert_eq!((live.measurement.from, live.measurement.to), (1300, 1700));
    assert!(live.x0 >= m.tag.right() && (live.x1 - x_of(&l, &app, 1700)).abs() <= 1.0);
    assert!(!live.labels.is_empty());
    let mid = point(
        (live.x0 + live.x1) / 2.0,
        m.band.top() + m.band.height() / 2.0,
    );
    assert_eq!(l.hit(mid), Some(volna_core::marker::LaneHit::Live));

    // Before the reference the span is signed and ends at the tag.
    app.doc.shared.cursor = Some(1100);
    let l = lane(&mut app);
    let live = l.measure.unwrap().live.unwrap();
    assert!(live.measurement.dt() < 0);
    assert!(live.labels[0].starts_with('−'), "{:?}", live.labels);
    // On the reference, or without a cursor, only the tag remains.
    app.doc.shared.cursor = Some(1300);
    assert!(lane(&mut app).measure.unwrap().live.is_none());
    app.doc.shared.cursor = None;
    assert!(lane(&mut app).measure.unwrap().live.is_none());

    act(&mut app, Action::ClearReference);
    assert!(lane(&mut app).measure.is_none());
}

#[test]
fn a_press_on_the_live_span_keeps_the_cursor_and_elsewhere_moves_it() {
    let mut app = app();
    app.doc.shared.cursor = Some(1300);
    act(&mut app, Action::SetReference);
    app.doc.shared.cursor = Some(1700);
    let l = lane(&mut app);
    let m = l.measure.clone().unwrap();
    let live = m.live.unwrap();
    let y = m.band.top() + m.band.height() / 2.0;
    press(
        &mut app,
        point((live.x0 + live.x1) / 2.0, y),
        MouseButton::Left,
        false,
    );
    pointer(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.shared.cursor, Some(1700));
    // Past the cursor the lane works like the time header.
    let x = x_of(&l, &app, 1900);
    press(&mut app, point(x, y), MouseButton::Left, false);
    pointer(&mut app, PointerEvent::Up);
    assert!(app.doc.shared.cursor.unwrap() > 1850);
}

#[test]
fn alt_click_and_a_middle_click_measure_from_the_pointer() {
    let mut app = app();
    mark(&mut app, 1400);
    app.doc.shared.cursor = Some(1100);
    let l = lane(&mut app);
    let rows = app.panels.focused_waves().unwrap().last_layout().waves;
    let y = rows.top() + 4.0;

    // Alt-click on the header beside marker 1: the reference snaps onto
    // it, and the cursor stays.
    let header = app.panels.focused_waves().unwrap().last_layout().header;
    let x = x_of(&l, &app, 1400) + 2.0;
    press(
        &mut app,
        point(x, header.top() + 4.0),
        MouseButton::Left,
        true,
    );
    pointer(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(1))));
    assert_eq!(app.doc.shared.cursor, Some(1100));

    // A middle click without dragging measures from its point on release.
    let x = x_of(&l, &app, 1800);
    press(&mut app, point(x, y), MouseButton::Middle, false);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(1))));
    pointer(&mut app, PointerEvent::Up);
    let t = app.doc.reference_time().unwrap();
    assert!(matches!(app.doc.reference(), Some(Reference::Time(_))));
    assert!((1790..=1810).contains(&t), "{t}");

    // A middle drag pans and leaves the reference.
    let view = app.doc.shared.viewport.target();
    press(&mut app, point(x, y), MouseButton::Middle, false);
    pointer(
        &mut app,
        PointerEvent::Move {
            position: point(x - 80.0, y),
        },
    );
    pointer(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.reference_time(), Some(t));
    assert!(app.doc.shared.viewport.target().start > view.start);
    assert_eq!(app.undo_label(), Some("Add marker 1"));
}

#[test]
fn z_zooms_to_the_reference_and_the_cursor_with_a_margin() {
    let mut app = app();
    act(&mut app, Action::ZoomToMeasurement);
    assert_eq!(
        said(&app).as_deref(),
        Some("Nothing to zoom to · R measures from the cursor")
    );
    app.doc.shared.cursor = Some(1600);
    act(&mut app, Action::SetReference);
    app.doc.shared.cursor = Some(1400);
    act(&mut app, Action::ZoomToMeasurement);
    let v = app.doc.shared.viewport.target();
    // 200 units measured, 15% of them on each side.
    assert!(
        (v.start - 1370.0).abs() < 1e-6 && (v.end - 1630.0).abs() < 1e-6,
        "{v:?}"
    );
}

#[test]
fn the_measure_lane_paints_in_both_themes() {
    for id in ["one-dark", "github-light"] {
        let theme = Theme::builtin(id).unwrap();
        let mut app = app();
        app.doc.shared.cursor = Some(1300);
        act(&mut app, Action::SetReference);
        app.doc.shared.cursor = Some(1700);
        let scene = frame(&mut app, &theme);
        let texts: Vec<&str> = scene.texts().collect();
        for want in ["R → cursor", "R", "400 ns"] {
            assert!(texts.contains(&want), "{id}: {want} in {texts:?}");
        }
        // The measured interval is tinted in the cursor colour, and the
        // live span drawn in it.
        let tint = theme.wave_cursor.with_alpha(0.07);
        let quads: Vec<_> = scene.quads().collect();
        assert!(quads.iter().any(|(_, c)| *c == tint), "{id}: no tint");
        assert!(
            quads
                .iter()
                .filter(|(_, c)| *c == theme.wave_cursor)
                .count()
                > 2,
            "{id}: live span and reference line"
        );
    }
}

#[test]
fn workspaces_keep_the_reference_and_reject_one_without_its_marker() {
    let mut app = app();
    mark(&mut app, 1200);
    act(&mut app, Action::SetReference);
    let saved = Workspace::capture(&app, "trace.vtr".into(), None).unwrap();
    let mut json = serde_json::to_value(&saved).unwrap();
    assert_eq!(json["version"], 4);
    assert_eq!(
        json["shared"]["reference"],
        serde_json::json!({"marker": 1})
    );

    let restore = |bytes: &[u8]| {
        let mut restored = App::new();
        restored.set_session(Arc::new(ProceduralTrace::new(1000)));
        let plan = Workspace::parse(bytes)?.prepare(
            &restored,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )?;
        plan.commit(&mut restored)?;
        anyhow::Ok(restored)
    };
    let restored = restore(&saved.to_bytes().unwrap()).unwrap();
    assert_eq!(restored.doc.reference(), Some(Reference::Marker(id(1))));

    json["shared"]["reference"] = serde_json::json!({"time": 1500});
    let restored = restore(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(restored.doc.reference(), Some(Reference::Time(1500)));

    json["shared"]["reference"] = serde_json::json!({"marker": 2});
    let Err(error) = restore(&serde_json::to_vec(&json).unwrap()) else {
        panic!("a dangling reference restored");
    };
    assert!(format!("{error:#}").contains("reference to missing marker 2"));

    json["version"] = 3.into();
    assert!(restore(&serde_json::to_vec(&json).unwrap()).is_err());
}
