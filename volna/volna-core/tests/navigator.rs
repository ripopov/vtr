//! The marker navigator (`'`, or `@` in the palette): rows in time order
//! with number, name, time, the step from the previous marker and the
//! distance from the reference; typing filters by name words or number;
//! `↵` goes, `⇧↵` measures from, `F2` renames and `Del` removes.

use std::sync::Arc;

use volna_core::marker::{LaneVerb, MarkerId, Reference, navigator_query};
use volna_core::testing::ProceduralTrace;
use volna_core::{Action, App, Command, Event};

fn id(n: u32) -> MarkerId {
    MarkerId::new(n).unwrap()
}

/// Markers 1–4 at 1200, 1600, 2000 and 900: “req A”, “resp A”, “req B”, none.
fn app() -> App {
    let mut app = App::new();
    app.set_session(Arc::new(ProceduralTrace::new(1000)));
    app.handle(Command::AddVars(vec![0]));
    for (t, name) in [
        (1200, "req A"),
        (1600, "resp A"),
        (2000, "req B"),
        (900, ""),
    ] {
        app.doc.shared.cursor = Some(t);
        app.handle(Command::Action(Action::AddOrRenameMarker));
        let m = app.doc.markers().iter().find(|m| m.time == t).unwrap().id;
        app.doc.rename_marker(m, name);
    }
    app.doc.shared.cursor = Some(100);
    app
}

fn ids(app: &App, query: &str) -> Vec<u32> {
    app.navigator_rows(query)
        .iter()
        .map(|r| r.id.get())
        .collect()
}

#[test]
fn rows_list_markers_in_time_order_and_filter_by_name_words_or_number() {
    let app = app();
    assert_eq!(ids(&app, ""), [4, 1, 2, 3]);
    assert_eq!(ids(&app, "req"), [1, 3]);
    assert_eq!(ids(&app, "RESP"), [2], "ignoring case");
    assert_eq!(ids(&app, "a req"), [1], "every word must match");
    assert_eq!(ids(&app, "3"), [3], "a number finds its marker");
    assert_eq!(ids(&app, "zzz"), Vec::<u32>::new());
    assert_eq!(navigator_query("@ req"), Some(" req"));
    assert_eq!(navigator_query("  @"), Some(""));
    assert_eq!(navigator_query("req"), None);
}

#[test]
fn rows_carry_time_step_and_distance_from_the_reference() {
    let mut app = app();
    let rows = app.navigator_rows("");
    let r1 = &rows[1];
    assert_eq!((r1.id, r1.name.as_deref()), (id(1), Some("req A")));
    assert_eq!(r1.time, "1.2 µs");
    assert_eq!(rows[0].step, None, "the first has no step");
    assert_eq!(r1.step.as_deref(), Some("+300 ns"));
    assert_eq!(r1.from_reference, None, "no reference yet");
    // Steps come from the whole list, so a filter keeps them.
    let filtered = app.navigator_rows("resp");
    assert_eq!(filtered[0].step.as_deref(), Some("+400 ns"));

    app.doc.set_reference(Some(Reference::Marker(id(2))));
    let rows = app.navigator_rows("");
    let from: Vec<Option<&str>> = rows.iter().map(|r| r.from_reference.as_deref()).collect();
    assert_eq!(
        from,
        [
            Some("−700 ns"),
            Some("−400 ns"),
            Some("0 ns"),
            Some("400 ns")
        ]
    );
    let marked: Vec<bool> = rows.iter().map(|r| r.is_reference).collect();
    assert_eq!(marked, [false, false, true, false]);
}

#[test]
fn the_navigator_opens_by_key_or_name_and_its_keys_act_on_a_marker() {
    let mut app = app();
    app.take_events();
    app.handle(Command::Action(Action::MarkerNavigator));
    assert!(app.take_events().contains(&Event::OpenMarkerNavigator));
    assert_eq!(
        Command::named("markerNavigator"),
        Some(Command::Action(Action::MarkerNavigator))
    );
    let before = app.undo_label().map(str::to_owned);

    // ↵ goes to the marker as a jump.
    app.handle(Command::Lane(LaneVerb::GoTo(id(2))));
    assert_eq!(app.doc.shared.cursor, Some(1600));
    app.handle(Command::Action(Action::JumpBack));
    assert_eq!(app.doc.shared.cursor, Some(100));
    // ⇧↵ measures from it.
    app.handle(Command::Lane(LaneVerb::MeasureFrom(id(3))));
    assert_eq!(app.doc.reference(), Some(Reference::Marker(id(3))));
    // F2 opens its name field.
    app.handle(Command::Lane(LaneVerb::Rename(id(1))));
    let edit = app.text_edit().expect("the name field");
    assert_eq!(edit.text, "req A");
    app.handle(Command::CommitText(edit.target, None));
    assert_eq!(
        app.undo_label().map(str::to_owned),
        before,
        "all navigation"
    );
    // Del removes it, one step.
    app.handle(Command::Lane(LaneVerb::Remove(id(4))));
    assert_eq!(ids(&app, ""), [1, 2, 3]);
    assert_eq!(app.undo_label(), Some("Remove marker 4"));
}
