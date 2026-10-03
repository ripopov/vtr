//! The start page's recent list (`volna/volna/ARCHITECTURE.md`, "Workspace persistence"): order, limit,
//! keys, removal, missing files and the events hosts act on.
use std::collections::BTreeSet;
use volna_core::testing::ProceduralTrace;
use volna_core::workspace::recent::{Recent, RecentCommand, RecentKey, RecentKind, SHOWN};
use volna_core::{App, Command, Event};

const NOW: u64 = 1_790_589_600; // 2026-09-28 10:00 UTC

fn trace(name: &str, ago: u64) -> Recent {
    Recent {
        uri: format!("file:///home/ada/sim/{name}"),
        kind: RecentKind::Trace,
        trace: None,
        opened: NOW - ago,
    }
}

/// A native app that remembered `n` traces, `t0.vtr` the newest.
fn app(n: u64) -> App {
    let mut app = App::new();
    app.enable_recent();
    for i in (0..n).rev() {
        app.remember_recent(trace(&format!("t{i}.vtr"), i * 3600));
    }
    app.take_events();
    app
}

fn names(app: &App) -> Vec<String> {
    let rows = app.recent_rows(NOW, Some("/home/ada"));
    rows[..app.recent_shown()]
        .iter()
        .map(|r| r.name.clone())
        .collect()
}

fn key(app: &mut App, key: RecentKey) {
    app.handle(Command::Recent(RecentCommand::Key(key)));
}

fn opened(app: &mut App) -> Vec<String> {
    app.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::OpenRecent(entry) => Some(entry.uri),
            _ => None,
        })
        .collect()
}

#[test]
fn enter_reopens_the_newest_and_digits_open_by_position() {
    let mut app = app(3);
    assert_eq!(names(&app), ["t0.vtr", "t1.vtr", "t2.vtr"]);
    key(&mut app, RecentKey::Enter);
    assert_eq!(opened(&mut app), ["file:///home/ada/sim/t0.vtr"]);
    key(&mut app, RecentKey::Down);
    key(&mut app, RecentKey::Down);
    key(&mut app, RecentKey::Down);
    assert_eq!(app.workspace.recent.selected, 2, "stops at the last row");
    key(&mut app, RecentKey::Up);
    key(&mut app, RecentKey::Enter);
    assert_eq!(opened(&mut app), ["file:///home/ada/sim/t1.vtr"]);
    key(&mut app, RecentKey::Digit(3));
    assert_eq!(opened(&mut app), ["file:///home/ada/sim/t2.vtr"]);
    key(&mut app, RecentKey::Digit(4));
    key(&mut app, RecentKey::Digit(0));
    assert!(opened(&mut app).is_empty(), "no such row");
    // The host records the successful open; it moves to the top once.
    app.remember_recent(trace("t2.vtr", 0));
    assert_eq!(names(&app), ["t2.vtr", "t0.vtr", "t1.vtr"]);
    assert_eq!(app.workspace.recent.selected, 0);
    let rows = app.recent_rows(NOW, Some("/home/ada"));
    assert_eq!(rows[0].when, "just now");
    assert_eq!(rows[1].when, "just now");
    assert_eq!(rows[2].when, "1 h ago");
    assert_eq!(rows[0].folder, "~/sim");
}

#[test]
fn the_page_shows_eight_until_show_all_and_the_limit_applies() {
    let mut app = app(12);
    assert_eq!(app.recent_shown(), SHOWN);
    key(&mut app, RecentKey::End);
    assert_eq!(app.workspace.recent.selected, SHOWN - 1);
    app.handle(Command::Recent(RecentCommand::ToggleAll));
    assert_eq!(app.recent_shown(), 12);
    key(&mut app, RecentKey::End);
    key(&mut app, RecentKey::Enter);
    assert_eq!(opened(&mut app), ["file:///home/ada/sim/t11.vtr"]);
    app.handle(Command::Recent(RecentCommand::ToggleAll));
    assert_eq!(
        app.workspace.recent.selected,
        SHOWN - 1,
        "kept on a shown row"
    );
    app.settings_loaded("{\"workspace.recentLimit\": 5}");
    app.remember_recent(trace("new.vtr", 0));
    assert_eq!(app.workspace.state.recent.len(), 5);
    assert_eq!(names(&app)[0], "new.vtr");
}

#[test]
fn removal_is_explicit_saved_and_not_an_undo_step() {
    let mut app = app(3);
    app.set_session(std::sync::Arc::new(ProceduralTrace::new(10)));
    app.close_trace();
    app.take_events();
    key(&mut app, RecentKey::Down);
    key(&mut app, RecentKey::Delete);
    assert_eq!(names(&app), ["t0.vtr", "t2.vtr"]);
    let events = app.take_events();
    assert!(events.iter().any(|e| matches!(e, Event::RecentChanged)));
    assert_eq!(app.undo_label(), None);
    app.handle(Command::Recent(RecentCommand::Forget(9)));
    assert!(
        !app.take_events()
            .iter()
            .any(|e| matches!(e, Event::RecentChanged))
    );
    app.handle(Command::Recent(RecentCommand::Clear));
    assert!(names(&app).is_empty());
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, Event::RecentChanged))
    );
    key(&mut app, RecentKey::Enter);
    assert!(opened(&mut app).is_empty());
}

#[test]
fn missing_files_stay_listed_and_can_still_be_tried() {
    let mut app = app(3);
    app.set_recent_missing(BTreeSet::from(["file:///home/ada/sim/t1.vtr".to_owned()]));
    let rows = app.recent_rows(NOW, None);
    assert_eq!(
        rows.iter().map(|r| r.missing).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(rows[1].when, "Not found");
    key(&mut app, RecentKey::Digit(2));
    assert_eq!(opened(&mut app), ["file:///home/ada/sim/t1.vtr"]);
    assert_eq!(app.workspace.state.recent.len(), 3, "kept");
    // Reopened after the file came back: no longer missing.
    app.remember_recent(trace("t1.vtr", 0));
    assert!(!app.recent_rows(NOW, None)[0].missing);
}

#[test]
fn a_disabled_list_shows_nothing_and_opens_nothing() {
    let mut app = App::new();
    app.remember_recent(trace("t.vtr", 0));
    assert_eq!(app.recent_shown(), 0);
    key(&mut app, RecentKey::Enter);
    app.handle(Command::Recent(RecentCommand::Open(0)));
    assert!(opened(&mut app).is_empty());
}

#[test]
fn a_trace_with_an_identity_reports_its_open() {
    let mut app = App::new();
    let spec = volna_trace::session::OpenSpec::Bytes {
        name: "t.vtr".into(),
        bytes: Vec::new(),
    };
    app.open_resource(spec, "file:///home/ada/sim/t.vtr".into());
    volna_core::testing::complete_open(&mut app, ProceduralTrace::session(10));
    let opened: Vec<_> = app
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::TraceOpened { trace_uri } => Some(trace_uri),
            _ => None,
        })
        .collect();
    assert_eq!(opened, ["file:///home/ada/sim/t.vtr"]);
}

#[test]
fn workspace_rows_name_the_trace_they_open() {
    let mut app = app(0);
    app.remember_recent(Recent {
        uri: "file:///home/ada/xs/chi_hang.volna.json".into(),
        kind: RecentKind::Workspace,
        trace: Some("file:///home/ada/xs/dual.vtr".into()),
        opened: NOW - 7200,
    });
    let row = &app.recent_rows(NOW, Some("/home/ada"))[0];
    assert_eq!(
        (row.name.as_str(), row.trace.as_deref(), row.when.as_str()),
        ("chi_hang", Some("dual.vtr"), "2 h ago")
    );
}
