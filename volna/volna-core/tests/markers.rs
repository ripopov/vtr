//! Walking and jumping between markers: `.` and `,` step in time order, the
//! digit keys go to a marker by number, and `` ` `` swaps with the place
//! before the last jump. All of it is navigation: the view keeps its zoom,
//! pans only when the marker is near or past an edge, and nothing reaches
//! the undo journal.

use std::sync::Arc;
use volna_core::testing::a_all;

use volna_core::marker::MarkerId;
use volna_core::nav::NavState;
use volna_core::panels::{Axis, PanelsCommand};
use volna_core::testing::ProceduralTrace;
use volna_core::wave::model::LinkDim;
use volna_core::wave::viewport::Viewport;
use volna_core::{Action, App, Command, Instant};

fn app() -> App {
    let mut app = App::new();
    app.set_session(Arc::new(ProceduralTrace::new(100)));
    app.handle(Command::AddVars(a_all(vec![0, 1])));
    app
}

fn id(n: u32) -> MarkerId {
    MarkerId::new(n).unwrap()
}

/// Where the shared view is headed.
fn view(app: &App) -> Viewport {
    app.doc.shared.viewport.target()
}

fn said(app: &App) -> Option<String> {
    app.status().announcement
}

fn same_width(a: Viewport, b: Viewport) -> bool {
    (a.width() - b.width()).abs() < 1e-6 * a.width()
}

fn centred_on(v: Viewport, t: u64) -> bool {
    ((v.start + v.end) / 2.0 - t as f64).abs() < 1e-6 * v.width()
}

/// A view a tenth of the trace wide, and markers 1 to 4 at: a tenth into
/// the view, its middle, far off screen to the right, and in the view's
/// last 5%.
struct Scene {
    view: Viewport,
    at: [u64; 4],
}

fn scene(app: &mut App) -> Scene {
    let (lo, hi) = app.doc.limits();
    let span = hi - lo;
    assert!(span >= 1000, "the trace is long enough to place markers");
    let (start, end) = (lo + span / 10, lo + span / 5);
    let w = end - start;
    let at = [
        start + w / 10,
        start + w / 2,
        lo + span * 4 / 5,
        start + w * 97 / 100,
    ];
    for t in at {
        app.doc.shared.cursor = Some(t);
        app.handle(Command::Action(Action::AddOrRenameMarker));
    }
    assert_eq!(app.doc.markers().len(), 4);
    let view = Viewport {
        start: start as f64,
        end: end as f64,
    };
    app.doc.shared.viewport.set(view);
    Scene { view, at }
}

#[test]
fn walking_keeps_the_zoom_and_pans_only_near_or_past_an_edge() {
    let mut app = app();
    let s = scene(&mut app);
    let undo = app.undo_label().map(str::to_owned);
    app.doc.shared.cursor = Some(s.at[0] + 1);

    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[1]));
    assert_eq!(
        view(&app),
        s.view,
        "a marker in the middle 90% does not pan"
    );
    assert_eq!(said(&app).as_deref(), Some("At marker 2 · ` returns"));

    // Marker 4 is on screen but in the last 5%: centre it, same width.
    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[3]));
    assert!(same_width(view(&app), s.view));
    assert!(centred_on(view(&app), s.at[3]));
    let near_four = view(&app);

    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[2]));
    assert!(same_width(view(&app), s.view));
    assert!(centred_on(view(&app), s.at[2]));

    // Nothing further: the cursor and view stay, and the status bar says so.
    let far = view(&app);
    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[2]));
    assert_eq!(view(&app), far);
    assert_eq!(said(&app).as_deref(), Some("No marker after the cursor"));

    app.handle(Command::Action(Action::PrevMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[3]));
    assert_eq!(said(&app).as_deref(), Some("At marker 4 · ` returns"));
    assert!(same_width(view(&app), near_four) && centred_on(view(&app), s.at[3]));

    // By number, including one that does not exist.
    app.handle(Command::Action(Action::GoToMarker(id(1))));
    assert_eq!(app.doc.shared.cursor, Some(s.at[0]));
    app.handle(Command::Action(Action::GoToMarker(id(7))));
    assert_eq!(app.doc.shared.cursor, Some(s.at[0]));
    assert_eq!(said(&app).as_deref(), Some("No marker 7"));
    app.handle(Command::Action(Action::GoToMarker(id(1))));
    assert_eq!(said(&app).as_deref(), Some("At marker 1"), "already there");
    app.handle(Command::Action(Action::PrevMarker));
    assert_eq!(said(&app).as_deref(), Some("No marker before the cursor"));

    // The next key clears the status line.
    app.handle(Command::Action(Action::ZoomIn));
    assert_eq!(said(&app), None);

    assert_eq!(
        app.undo_label().map(str::to_owned),
        undo,
        "walking is navigation, not an edit"
    );
}

#[test]
fn without_a_cursor_walks_start_from_the_view() {
    let mut app = app();
    let s = scene(&mut app);
    app.doc.shared.cursor = None;
    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[0]), "first in the view");
    app.doc.shared.cursor = None;
    app.handle(Command::Action(Action::PrevMarker));
    assert_eq!(app.doc.shared.cursor, Some(s.at[3]), "last in the view");
    app.handle(Command::Action(Action::RemoveAllMarkers));
    app.doc.shared.cursor = None;
    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(
        said(&app).as_deref(),
        Some("No marker in or after the view")
    );
    app.handle(Command::Action(Action::PrevMarker));
    assert_eq!(
        said(&app).as_deref(),
        Some("No marker in or before the view")
    );
    assert_eq!(app.doc.shared.cursor, None);
}

#[test]
fn back_swaps_with_the_place_before_the_last_jump() {
    let mut app = app();
    let s = scene(&mut app);
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(said(&app).as_deref(), Some("No jump to return from"));

    let start_cursor = Some(s.at[0] + 1);
    app.doc.shared.cursor = start_cursor;
    app.handle(Command::Action(Action::GoToMarker(id(3))));
    let there = (app.doc.shared.cursor, view(&app));
    assert_eq!(there.0, Some(s.at[2]));
    assert_ne!(there.1, s.view);

    app.handle(Command::Action(Action::JumpBack));
    assert_eq!((app.doc.shared.cursor, view(&app)), (start_cursor, s.view));
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!((app.doc.shared.cursor, view(&app)), there, "forward again");
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(
        (app.doc.shared.cursor, view(&app)),
        (start_cursor, s.view),
        "twice more is back at the start"
    );

    // A jump to where the cursor already is keeps the return place.
    app.handle(Command::Action(Action::GoToMarker(id(3))));
    app.handle(Command::Action(Action::GoToMarker(id(3))));
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(app.doc.shared.cursor, start_cursor);
}

#[test]
fn reveal_pans_only_outside_the_middle_ninety_percent() {
    let mut app = app();
    let (lo, hi) = app.doc.limits();
    let span = (hi - lo) as f64;
    let view = Viewport {
        start: lo as f64 + span * 0.4,
        end: lo as f64 + span * 0.6,
    };
    let w = view.width();
    let now = Instant::now();
    let mut nav = NavState::new();
    for (t, pans) in [
        (view.start + w * 0.05, false),
        (view.end - w * 0.05, false),
        (view.start + w * 0.5, false),
        (view.start + w * 0.04, true),
        (view.end - w * 0.04, true),
        (view.start - w, true),
        (view.end + w, true),
    ] {
        app.doc.shared.viewport.set(view);
        let t = t.round() as u64;
        assert_eq!(nav.reveal(&mut app.doc, t, now), pans, "t = {t}");
        let after = app.doc.shared.viewport.target();
        assert!(same_width(after, view), "the zoom is kept");
        assert_eq!(pans, after != view);
        if pans {
            assert!(centred_on(after, t));
        }
    }
    // At the trace's end the view may stop short of centring, but the time
    // still lands in its middle 90%.
    app.doc.shared.viewport.set(view);
    assert!(nav.reveal(&mut app.doc, hi, now));
    let after = app.doc.shared.viewport.target();
    assert!(same_width(after, view));
    let edge = after.width() * 0.05;
    assert!((after.start + edge..=after.end - edge).contains(&(hi as f64)));
    assert!(
        !nav.reveal(&mut app.doc, hi, now),
        "a second reveal is a no-op"
    );
}

#[test]
fn unlinked_cursors_walk_and_return_on_their_own() {
    let mut app = app();
    let s = scene(&mut app);
    let a = app.panels.focused_id();
    app.doc.shared.cursor = Some(s.at[0]);
    app.handle(Command::Panels(PanelsCommand::Split {
        panel: a,
        axis: Axis::Vertical,
    }));
    let b = app.panels.focused_id();
    app.handle(Command::Panels(PanelsCommand::ToggleLink {
        panel: b,
        dim: LinkDim::Cursor,
    }));
    let cursor = |app: &App, p| app.panels.waves(p).unwrap().cursor(&app.doc);

    // B walks its own cursor; A keeps the shared one.
    app.handle(Command::Action(Action::NextMarker));
    assert_eq!(cursor(&app, b), Some(s.at[1]));
    assert_eq!(cursor(&app, a), Some(s.at[0]));
    app.handle(Command::Action(Action::GoToMarker(id(4))));
    assert_eq!(cursor(&app, b), Some(s.at[3]));

    // A walks the shared cursor; B's stays put.
    app.handle(Command::Panels(PanelsCommand::Focus(a)));
    app.handle(Command::Action(Action::GoToMarker(id(3))));
    assert_eq!(cursor(&app, a), Some(s.at[2]));
    assert_eq!(cursor(&app, b), Some(s.at[3]));

    // Each panel returns from its own last jump.
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(cursor(&app, a), Some(s.at[0]));
    assert_eq!(cursor(&app, b), Some(s.at[3]));
    app.handle(Command::Panels(PanelsCommand::Focus(b)));
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(cursor(&app, b), Some(s.at[1]));
    assert_eq!(cursor(&app, a), Some(s.at[0]));
}

#[test]
fn hosts_reach_the_walk_by_name() {
    let named = |name| Command::named(name);
    assert_eq!(
        named("nextMarker"),
        Some(Command::Action(Action::NextMarker))
    );
    assert_eq!(
        named("prevMarker"),
        Some(Command::Action(Action::PrevMarker))
    );
    assert_eq!(named("jumpBack"), Some(Command::Action(Action::JumpBack)));
    assert_eq!(
        named("goToMarker3"),
        Some(Command::Action(Action::GoToMarker(id(3))))
    );
    assert_eq!(named("goToMarker0"), None);
    assert_eq!(named("goToMarker"), None);
}

fn frame(app: &mut App) -> &volna_core::Scene {
    let theme = volna_core::Theme::one_dark();
    let id = app.panels.focused_id();
    app.layout_panel(
        id,
        volna_core::geometry::Rect::from_xywh(0.0, 0.0, 1200.0, 600.0),
        &theme,
    );
    app.render_panel(id, &theme, &mut volna_core::scene::MonoMeasure)
}

fn lane(app: &App) -> volna_core::wave::overlay::MarkerLane {
    app.panels
        .focused()
        .kind
        .marker_lane()
        .expect("a timed panel")
        .clone()
}

#[test]
fn m_on_a_marker_opens_its_name_field_and_the_chip_grows_to_the_name() {
    use volna_core::app::EditTarget;
    let mut app = app();
    let s = scene(&mut app);
    let panel = app.panels.focused_id();
    assert_eq!(app.text_edit(), None);

    app.doc.shared.cursor = Some(s.at[1]);
    app.handle(Command::Action(Action::AddOrRenameMarker));
    assert_eq!(
        app.doc.markers().len(),
        4,
        "no second marker at the same time"
    );
    frame(&mut app);
    let edit = app.text_edit().expect("the name field is open");
    assert_eq!(edit.target, EditTarget::Marker { panel, id: id(2) });
    assert_eq!((edit.text.as_str(), edit.select_all), ("", true));
    // Over the chip, after its number, inside the lane.
    let l = lane(&app);
    let chip = l.chips.iter().find(|c| c.markers.contains(&1)).unwrap();
    let rect = edit.rect.expect("the chip is in view");
    assert!(rect.left() > chip.rect.left() && rect.left() <= chip.rect.right());
    assert!(rect.top() >= l.band.top() && rect.bottom() <= l.band.bottom());
    assert!(rect.right() <= l.band.right());

    app.handle(Command::CommitText(edit.target, Some("  req B  ".into())));
    assert_eq!(app.text_edit(), None);
    assert_eq!(app.doc.markers()[1].label.as_deref(), Some("req B"));
    assert_eq!(app.undo_label(), Some("Rename marker 2"));
    frame(&mut app);
    let chip = lane(&app)
        .chips
        .into_iter()
        .find(|c| c.markers.contains(&1))
        .unwrap();
    assert_eq!(chip.text, "2 req B");

    // Opening again starts with the name; cancelling keeps it, and an
    // empty name returns the chip to its number.
    app.handle(Command::Action(Action::AddOrRenameMarker));
    let edit = app.text_edit().unwrap();
    assert_eq!(edit.text, "req B");
    app.handle(Command::CommitText(edit.target, None));
    assert_eq!(app.doc.markers()[1].label.as_deref(), Some("req B"));
    app.handle(Command::Action(Action::AddOrRenameMarker));
    let target = app.text_edit().unwrap().target;
    app.handle(Command::CommitText(target, Some("   ".into())));
    assert_eq!(app.doc.markers()[1].label, None);
    frame(&mut app);
    let chip = lane(&app)
        .chips
        .into_iter()
        .find(|c| c.markers.contains(&1))
        .unwrap();
    assert_eq!(chip.text, "2");

    // A stale answer is ignored.
    app.handle(Command::CommitText(target, Some("late".into())));
    assert_eq!(app.doc.markers()[1].label, None);
}

#[test]
fn a_key_that_reaches_the_panel_closes_the_name_field_without_renaming() {
    let mut app = app();
    let s = scene(&mut app);
    app.doc.shared.cursor = Some(s.at[0]);
    app.handle(Command::Action(Action::AddOrRenameMarker));
    let target = app.text_edit().unwrap().target;
    app.handle(Command::Action(Action::ZoomIn));
    assert_eq!(app.text_edit(), None);
    app.handle(Command::CommitText(target, Some("x".into())));
    assert_eq!(app.doc.markers()[0].label, None);
}

#[test]
fn renaming_by_chip_reveals_the_marker_and_follows_panel_focus() {
    use volna_core::app::EditTarget;
    let mut app = app();
    let s = scene(&mut app);
    let a = app.panels.focused_id();
    // Marker 3 is off the view: naming it pans there, keeping the zoom.
    app.handle(Command::RenameMarker(a, id(3)));
    assert!(centred_on(view(&app), s.at[2]) && same_width(view(&app), s.view));
    app.tick(Instant::now() + std::time::Duration::from_secs(5));
    frame(&mut app);
    assert!(app.text_edit().unwrap().rect.is_some());
    // The field belongs to its panel: another focused panel shows none.
    app.handle(Command::Panels(PanelsCommand::Split {
        panel: a,
        axis: Axis::Vertical,
    }));
    assert_eq!(app.text_edit(), None);
    app.handle(Command::Panels(PanelsCommand::Focus(a)));
    let edit = app.text_edit().unwrap();
    assert_eq!(
        edit.target,
        EditTarget::Marker {
            panel: a,
            id: id(3)
        }
    );
    app.handle(Command::CommitText(edit.target, Some("irq".into())));
    assert_eq!(app.doc.markers()[3].label.as_deref(), Some("irq"));
    // Removing the marker closes a field open on it.
    app.handle(Command::RenameMarker(a, id(1)));
    assert!(app.text_edit().is_some());
    assert!(app.doc.remove_marker(id(1)));
    assert_eq!(app.text_edit(), None);
}

#[test]
fn the_tooltip_of_a_chip_has_the_full_name() {
    use volna_core::geometry::point;
    use volna_core::wave::PointerEvent;
    let mut app = app();
    let s = scene(&mut app);
    let long = "the first beat of the read burst on AXI";
    assert!(app.doc.rename_marker(id(1), long));
    // Zoomed out, marker 2 is close enough that the name is cut before it.
    app.doc.shared.viewport.set(Viewport {
        start: s.view.start,
        end: s.view.start + s.view.width() * 4.0,
    });
    frame(&mut app);
    let chip = lane(&app).chips[0].clone();
    assert!(
        chip.text.starts_with("1 the") && chip.text.ends_with('…'),
        "{}",
        chip.text
    );
    assert!(!frame(&mut app).texts().any(|t| t == long));
    let p = point(chip.rect.left() + 4.0, chip.rect.top() + 4.0);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Move { position: p },
    ));
    let texts: Vec<String> = frame(&mut app).texts().map(str::to_owned).collect();
    assert!(texts.iter().any(|t| t == "Marker 1"), "{texts:?}");
    assert!(texts.iter().any(|t| t == long), "{texts:?}");
}
