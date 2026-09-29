//! Direct manipulation of markers: a click on a chip goes to its marker, a
//! drag moves it as one undo step (Esc cancels, the start shows dotted),
//! Alt-click or a middle click measures from it, right-click menus list the
//! lane's verbs, *Copy as Text* hands the host a readable reference, and a
//! double-click on the header or an empty part of the lane adds a marker.

use std::sync::Arc;
use volna_core::testing::a_all;

use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::marker::{LaneHit, LaneVerb, MarkerId, Reference};
use volna_core::testing::ProceduralTrace;
use volna_core::wave::PointerEvent;
use volna_core::wave::model::{MenuAction, MenuEntry};
use volna_core::wave::overlay::MarkerLane;
use volna_core::wave::viewport::Viewport;
use volna_core::{Action, App, Command, Event, Theme};

fn app() -> App {
    let mut app = App::new();
    app.set_session(Arc::new(ProceduralTrace::new(1000)));
    app.handle(Command::AddVars(a_all(vec![0, 1])));
    app.doc.shared.viewport.set(Viewport {
        start: 1000.0,
        end: 2000.0,
    });
    for t in [1200, 1600] {
        app.doc.shared.cursor = Some(t);
        app.handle(Command::Action(Action::AddOrRenameMarker));
    }
    app.doc.shared.cursor = Some(1100);
    app
}

fn id(n: u32) -> MarkerId {
    MarkerId::new(n).unwrap()
}

fn frame<'a>(app: &'a mut App, theme: &Theme) -> &'a volna_core::Scene {
    let id = app.panels.focused_id();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 600.0), theme);
    app.render_panel(id, theme, &mut volna_core::scene::MonoMeasure)
}

fn lane(app: &mut App) -> MarkerLane {
    frame(app, &Theme::one_dark());
    app.panels.focused().kind.marker_lane().unwrap().clone()
}

fn centre(r: Rect) -> Point {
    point(r.left() + r.width() / 2.0, r.top() + r.height() / 2.0)
}

/// The middle of marker `ix`'s chip.
fn chip(app: &mut App, ix: usize) -> Point {
    let l = lane(app);
    let c = l.chips.iter().find(|c| c.markers == (ix..ix + 1)).unwrap();
    centre(c.rect)
}

fn send(app: &mut App, event: PointerEvent) {
    let panel = app.panels.focused_id();
    app.handle(Command::Pointer(panel, event));
}

fn down(app: &mut App, p: Point, button: MouseButton, modifiers: Modifiers) {
    send(
        app,
        PointerEvent::Down {
            position: p,
            button,
            modifiers,
        },
    );
}

fn times(app: &App) -> Vec<(u32, u64)> {
    app.doc
        .markers()
        .iter()
        .map(|m| (m.id.get(), m.time))
        .collect()
}

#[test]
fn a_click_on_a_chip_goes_to_its_marker_and_a_drag_moves_it_as_one_step() {
    let mut app = app();
    let p = chip(&mut app, 0);
    // A press that moves less than the slop is a click.
    down(&mut app, p, MouseButton::Left, Modifiers::default());
    assert_eq!(
        app.doc.shared.cursor,
        Some(1100),
        "nothing happens on press"
    );
    send(
        &mut app,
        PointerEvent::Move {
            position: point(p.x + 2.0, p.y),
        },
    );
    send(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.shared.cursor, Some(1200));
    assert_eq!(times(&app), [(1, 1200), (2, 1600)]);
    // It was a jump: ` returns.
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(app.doc.shared.cursor, Some(1100));

    // Measuring from marker 1: the reference follows the drag.
    app.doc.set_reference(Some(Reference::Marker(id(1))));
    let before = app.undo_label().map(str::to_owned);
    let p = chip(&mut app, 0);
    down(&mut app, p, MouseButton::Left, Modifiers::default());
    for dx in [20.0, 60.0, 120.0] {
        send(
            &mut app,
            PointerEvent::Move {
                position: point(p.x + dx, p.y),
            },
        );
    }
    let moved = app
        .doc
        .markers()
        .iter()
        .find(|m| m.id == id(1))
        .unwrap()
        .time;
    assert!((1300..1400).contains(&moved), "{moved}");
    assert_eq!(app.doc.reference_time(), Some(moved));
    // Still dragging: no step yet, and the start shows dotted.
    assert_eq!(app.undo_label().map(str::to_owned), before);
    let theme = Theme::one_dark();
    let ghost = theme.panel.text_muted;
    let dots = frame(&mut app, &theme)
        .quads()
        .filter(|(r, c)| *c == ghost && r.width() == 1.0 && r.height() <= 3.0)
        .count();
    assert!(dots > 20, "a dotted line at the start: {dots}");
    send(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.shared.cursor, Some(1100), "a drag keeps the cursor");
    assert_eq!(app.undo_label(), Some("Move marker 1"));
    let dots = frame(&mut app, &theme)
        .quads()
        .filter(|(r, c)| *c == ghost && r.width() == 1.0 && r.height() <= 3.0)
        .count();
    assert_eq!(dots, 0, "the dotted line goes with the drag");

    // Past marker 2 the list stays in time order, numbers unchanged.
    let p = chip(&mut app, 0);
    down(&mut app, p, MouseButton::Left, Modifiers::default());
    send(
        &mut app,
        PointerEvent::Move {
            position: point(p.x + 400.0, p.y),
        },
    );
    send(&mut app, PointerEvent::Up);
    let ids: Vec<u32> = app.doc.markers().iter().map(|m| m.id.get()).collect();
    assert_eq!(ids, [2, 1]);
    app.handle(Command::Undo);
    app.handle(Command::Undo);
    assert_eq!(times(&app), [(1, 1200), (2, 1600)]);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(1))));
}

#[test]
fn esc_cancels_a_drag_and_records_nothing() {
    let mut app = app();
    let before = app.undo_label().map(str::to_owned);
    let p = chip(&mut app, 1);
    down(&mut app, p, MouseButton::Left, Modifiers::default());
    send(
        &mut app,
        PointerEvent::Move {
            position: point(p.x - 150.0, p.y),
        },
    );
    assert_ne!(times(&app)[1], (2, 1600));
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(times(&app), [(1, 1200), (2, 1600)]);
    send(&mut app, PointerEvent::Up);
    assert_eq!(app.undo_label().map(str::to_owned), before);
    assert_eq!(
        app.doc.shared.cursor,
        Some(1100),
        "Esc did not also go there"
    );
    // Dragged away and back: no step either.
    let p = chip(&mut app, 1);
    down(&mut app, p, MouseButton::Left, Modifiers::default());
    send(
        &mut app,
        PointerEvent::Move {
            position: point(p.x + 100.0, p.y),
        },
    );
    send(&mut app, PointerEvent::Move { position: p });
    send(&mut app, PointerEvent::Up);
    assert_eq!(times(&app), [(1, 1200), (2, 1600)]);
    assert_eq!(app.undo_label().map(str::to_owned), before);
}

#[test]
fn alt_click_and_a_middle_click_on_a_chip_measure_from_its_marker() {
    let mut app = app();
    let p = chip(&mut app, 1);
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    down(&mut app, p, MouseButton::Left, alt);
    send(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(2))));
    assert_eq!(app.doc.shared.cursor, Some(1100));
    let p = chip(&mut app, 0);
    down(&mut app, p, MouseButton::Middle, Modifiers::default());
    send(&mut app, PointerEvent::Up);
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(1))));
    assert_eq!(times(&app), [(1, 1200), (2, 1600)]);
}

fn menu_items(app: &App) -> Vec<(String, Option<String>)> {
    app.menu()
        .expect("a menu")
        .items()
        .map(|i| (i.label.clone(), i.badge.clone()))
        .collect()
}

fn right_click(app: &mut App, p: Point) {
    down(app, p, MouseButton::Right, Modifiers::default());
    send(app, PointerEvent::Up);
}

/// Right-click at `at`, then choose `verb` from the menu that opens.
fn choose(app: &mut App, at: Point, verb: LaneVerb) {
    right_click(app, at);
    assert!(
        app.menu()
            .unwrap()
            .items()
            .any(|i| i.action == MenuAction::Lane(verb)),
        "{verb:?} is offered"
    );
    let panel = app.panels.focused_id();
    app.handle(Command::MenuSelect(panel, MenuAction::Lane(verb)));
    assert!(app.menu().is_none(), "a choice closes the menu");
}

#[test]
fn a_chips_menu_lists_its_verbs_with_their_keys_and_each_works() {
    let mut app = app();
    assert!(app.doc.rename_marker(id(2), "req B"));
    let p = chip(&mut app, 1);
    right_click(&mut app, p);
    let menu = app.menu().unwrap();
    assert!(matches!(&menu.entries[0], MenuEntry::Label(l) if l == "Marker 2 “req B”"));
    let key = |s: &str| Some(s.to_owned());
    assert_eq!(
        menu_items(&app),
        [
            ("Go to Marker".into(), key("2")),
            ("Rename…".into(), key("double-click")),
            ("Measure from Here".into(), key("Alt-click")),
            ("Copy as Text".into(), None),
            ("Move to Cursor".into(), None),
            ("Remove Marker".into(), None),
        ]
    );
    assert_eq!(
        app.doc.shared.cursor,
        Some(1100),
        "a right-click moves nothing"
    );

    let p = chip(&mut app, 1);
    let panel = app.panels.focused_id();
    app.handle(Command::MenuDismiss(panel));
    choose(&mut app, p, LaneVerb::GoTo(id(2)));
    assert_eq!(app.doc.shared.cursor, Some(1600));
    let p = chip(&mut app, 1);
    choose(&mut app, p, LaneVerb::MeasureFrom(id(2)));
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(2))));
    app.take_events();
    let p = chip(&mut app, 1);
    choose(&mut app, p, LaneVerb::Copy(id(2)));
    let copied: Vec<String> = app
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::CopyText(text) => Some(text),
            _ => None,
        })
        .collect();
    let trace = app.doc.name().unwrap();
    assert_eq!(copied, [format!("{trace} marker 2 “req B” at 1.6 µs")]);
    let p = chip(&mut app, 1);
    choose(&mut app, p, LaneVerb::Rename(id(2)));
    assert!(app.text_edit().is_some(), "the name field opens");
    app.handle(Command::CommitText(app.text_edit().unwrap().target, None));
    app.doc.shared.cursor = Some(1700);
    let p = chip(&mut app, 1);
    choose(&mut app, p, LaneVerb::MoveToCursor(id(2)));
    assert_eq!(times(&app), [(1, 1200), (2, 1700)]);
    assert_eq!(app.undo_label(), Some("Move marker 2"));
    assert_eq!(
        app.doc.reference_time(),
        Some(1700),
        "the reference follows"
    );
    app.doc.shared.cursor = Some(1200);
    let p = chip(&mut app, 1);
    choose(&mut app, p, LaneVerb::MoveToCursor(id(2)));
    assert_eq!(times(&app), [(1, 1200), (2, 1700)], "marker 1 is there");
    let p = chip(&mut app, 1);
    choose(&mut app, p, LaneVerb::Remove(id(2)));
    assert_eq!(times(&app), [(1, 1200)]);
    assert_eq!(app.undo_label(), Some("Remove marker 2"));
}

#[test]
fn span_and_measure_menus_offer_their_verbs() {
    let mut app = app();
    let l = lane(&mut app);
    let span = l.spans.iter().find(|s| s.first == 0).unwrap();
    let (x0, x1) = l.shown(span);
    let on_span = point((x0 + x1) / 2.0, centre(l.band).y);
    assert_eq!(l.hit(on_span), Some(LaneHit::Span(0)));
    right_click(&mut app, on_span);
    assert_eq!(
        menu_items(&app),
        [
            ("Measure 1 → 2".into(), None),
            ("Zoom to Span".into(), Some("double-click".into())),
            ("Copy as Text".into(), None),
        ]
    );
    app.take_events();
    choose(&mut app, on_span, LaneVerb::CopySpan(id(1), id(2)));
    let trace = app.doc.name().unwrap();
    assert!(app.take_events().contains(&Event::CopyText(format!(
        "{trace} marker 1 → marker 2: 400 ns"
    ))));
    choose(&mut app, on_span, LaneVerb::MeasureSpan(id(1), id(2)));
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(1))));
    assert_eq!(app.doc.shared.cursor, Some(1600));
    choose(&mut app, on_span, LaneVerb::ZoomToSpan(id(1), id(2)));
    let v = app.doc.shared.viewport.target();
    assert!(
        v.start < 1200.0 && v.end > 1600.0 && v.width() < 600.0,
        "{v:?}"
    );

    // The live span, the R tag and the × share the measurement's menu.
    app.doc.shared.viewport.set(Viewport {
        start: 1000.0,
        end: 2000.0,
    });
    app.doc.shared.cursor = Some(1800);
    let m = lane(&mut app).measure.unwrap();
    for p in [centre(m.tag), centre(m.clear)] {
        right_click(&mut app, p);
        assert_eq!(
            menu_items(&app),
            [
                ("Zoom to Measurement".into(), Some("Z".into())),
                ("Clear Reference".into(), Some("⇧R".into())),
            ]
        );
        assert!(app.doc.reference().is_some(), "a right-click on × keeps it");
    }
    choose(&mut app, centre(m.tag), LaneVerb::ZoomToMeasurement);
    let v = app.doc.shared.viewport.target();
    assert!(
        v.start < 1200.0 && v.end > 1800.0 && v.width() < 1000.0,
        "{v:?}"
    );
    let m = lane(&mut app).measure.unwrap();
    choose(&mut app, centre(m.tag), LaneVerb::ClearReference);
    assert_eq!(app.doc.reference(), None);
}

#[test]
fn a_menu_closes_on_dismiss_and_on_the_next_press() {
    let mut app = app();
    let p = chip(&mut app, 0);
    right_click(&mut app, p);
    let first = app.menu().unwrap().row;
    let panel = app.panels.focused_id();
    app.handle(Command::MenuDismiss(panel));
    assert!(app.menu().is_none());
    right_click(&mut app, p);
    assert_ne!(app.menu().unwrap().row, first, "each menu is new");
    down(
        &mut app,
        point(p.x, p.y + 200.0),
        MouseButton::Left,
        Modifiers::default(),
    );
    send(&mut app, PointerEvent::Up);
    assert!(app.menu().is_none());
    // A cluster has no menu.
    app.doc.add_marker(1601);
    let l = lane(&mut app);
    let cluster = l.chips.iter().find(|c| c.is_cluster()).unwrap().rect;
    right_click(&mut app, centre(cluster));
    assert!(app.menu().is_none());
}

#[test]
fn a_double_click_adds_on_the_header_and_the_empty_lane_only() {
    let mut app = app();
    let l = lane(&mut app);
    let panel = app.panels.focused_id();
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    let x = (l.time_left + l.band.right()) / 2.0 + 150.0;
    assert!(app.adds_marker_at(panel, point(x, layout.header.top() + 4.0)));
    assert!(
        app.adds_marker_at(panel, point(x, centre(l.band).y)),
        "empty lane"
    );
    let span = l.spans.iter().find(|s| s.first == 0).unwrap();
    let (x0, x1) = l.shown(span);
    assert!(!app.adds_marker_at(panel, point((x0 + x1) / 2.0, centre(l.band).y)));
    assert!(!app.adds_marker_at(panel, centre(l.chips[0].rect)));
    assert!(
        !app.adds_marker_at(panel, point(x, layout.waves.top() + 4.0)),
        "rows"
    );
    assert!(
        !app.adds_marker_at(panel, point(20.0, centre(l.band).y)),
        "title"
    );

    // The first click puts the cursor there; the host then sends M.
    down(
        &mut app,
        point(x, layout.header.top() + 4.0),
        MouseButton::Left,
        Modifiers::default(),
    );
    send(&mut app, PointerEvent::Up);
    let c = app.doc.shared.cursor.unwrap();
    app.handle(Command::Action(Action::AddOrRenameMarker));
    assert_eq!(app.doc.markers().last().unwrap().time, c);
    assert_eq!(app.doc.markers().len(), 3);
}
