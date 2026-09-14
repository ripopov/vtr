#![cfg(not(target_family = "wasm"))]
use std::{
    sync::Arc,
    task::{Context, Poll, Waker},
};
use volna_core::{
    App, Command, Theme,
    data::synth::SynthSource,
    geometry::Rect,
    panels::PanelsCommand,
    query_view::QueryView,
    scene::{MonoMeasure, Prim},
    wave::{Viewport, demand::Progress},
};
use vtr_query::{Budget, local_session::LocalSession, wave::Limits};
fn fixture() -> (tempfile::TempDir, App, LocalSession) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("view.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, a) = writer.add_bits("a", 1, 2);
    let (_, b) = writer.add_bits("b", 1, 2);
    writer
        .add_alias("alias", vtr::VarType::Wire, vtr::Direction::Implicit, a)
        .unwrap();
    for time in 0..128 {
        writer.set_time(time).unwrap();
        writer.emit_u64(a, time % 2).unwrap();
        writer.emit_u64(b, 1 - time % 2).unwrap();
    }
    writer.close().unwrap();
    let mut app = App::new();
    app.set_session(Arc::new(
        volna_core::session::LocalSession::open(&path).unwrap(),
    ));
    let session =
        futures_lite::future::block_on(LocalSession::open(path, Budget::new(16 << 20), 4).unwrap())
            .unwrap();
    (dir, app, session)
}
fn limits() -> Limits {
    Limits {
        bytes: 16384,
        records: 2,
        work: 32,
    }
}
fn layout(app: &mut App, theme: &Theme) {
    app.layout_waves(
        app.panels.focused_id(),
        Rect::from_xywh(0.0, 0.0, 1000.0, 250.0),
        theme,
    );
}

#[test]
fn exact_cursor_values_replace_dense_ambiguity_and_reject_stale_times() {
    let (_dir, mut app, session) = fixture();
    let mut view =
        QueryView::attach(&mut app, session, limits(), 4, 8, &Budget::new(65536)).unwrap();
    app.handle(Command::AddVars(vec![0, 1, 2]));
    layout(&mut app, &Theme::one_dark());
    drain(&mut view, &mut app);
    app.doc.shared.cursor = Some(61);
    let _ = view.poll(&mut app, &mut Context::from_waker(Waker::noop()));
    app.doc.shared.cursor = Some(62);
    drain(&mut view, &mut app);
    let rows = &app.panels.focused_waves().unwrap().items;
    for (index, expected) in ["0", "1", "0"].into_iter().enumerate() {
        let signal = rows[index].source.signal().unwrap().0;
        assert!(rows[index].query.as_ref().unwrap().sample_at(62).is_none());
        let sample = rows[index].cursor_sample.as_ref().unwrap();
        assert_eq!(
            sample.value_at(&app.doc, signal, 62, 4096).unwrap(),
            Some(volna_core::data::WaveValue::Bits(expected.into()))
        );
        assert!(
            sample
                .value_at(&app.doc, signal, 61, 4096)
                .unwrap()
                .is_none()
        );
        assert!(rows[index].history.is_none());
    }
    assert!(matches!(
        view.poll(&mut app, &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(Progress::Idle))
    ));
    // The value column must not wait for overview geometry to finish scanning.
    for item in &mut app.panels.focused_waves_mut().unwrap().items {
        item.query = None;
    }
    let values = app.panels.focused_waves().unwrap().last_layout().values;
    let scene = app.render_waves(
        app.panels.focused_id(),
        &Theme::one_dark(),
        &mut MonoMeasure,
    );
    for expected in ["0", "1"] {
        assert!(scene.prims.iter().any(|prim| matches!(prim, Prim::Text { text, origin, .. } if text == expected && origin.x >= values.left() && origin.x < values.right())));
    }
    app.doc.shared.cursor = None;
    drain(&mut view, &mut app);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| row.cursor_sample.is_none())
    );
}

#[test]
fn cursor_input_admission_waits_for_capacity_without_closing_the_view() {
    let (_dir, mut app, session) = fixture();
    let budget = Budget::new(65536);
    let mut view = QueryView::attach(&mut app, session, limits(), 4, 8, &budget).unwrap();
    app.handle(Command::AddVars(vec![0]));
    layout(&mut app, &Theme::one_dark());
    drain(&mut view, &mut app);
    let pin = budget.reserve(budget.limit() - budget.used()).unwrap();
    app.doc.shared.cursor = Some(61);
    assert!(matches!(
        view.poll(&mut app, &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(Progress::Backpressure))
    ));
    assert!(!view.is_closed());
    drop(pin);
    drain(&mut view, &mut app);
    let row = &app.panels.focused_waves().unwrap().items[0];
    assert_eq!(
        row.cursor_sample
            .as_ref()
            .unwrap()
            .value_at(&app.doc, 0, 61, 4096)
            .unwrap(),
        Some(volna_core::data::WaveValue::Bits("1".into()))
    );
}
fn drain(view: &mut QueryView<LocalSession>, app: &mut App) {
    futures_lite::future::block_on(async {
        for _ in 0..10000 {
            match futures_lite::future::poll_fn(|cx| view.poll(app, cx))
                .await
                .unwrap()
            {
                Progress::Idle => return,
                _ => futures_lite::future::yield_now().await,
            }
        }
        panic!("app queries did not become idle");
    });
}

#[test]
fn edge_commands_query_raw_changes_without_loading_histories() {
    let (_dir, mut app, session) = fixture();
    let mut view =
        QueryView::attach(&mut app, session, limits(), 16, 8, &Budget::new(65536)).unwrap();
    app.handle(Command::AddVars(vec![0, 1]));
    layout(&mut app, &Theme::one_dark());
    drain(&mut view, &mut app);
    app.configure_persistence(volna_core::workspace::persistence::Persistence::Auto);
    app.panels.focused_waves_mut().unwrap().selected = [0].into();
    app.panels.focused_waves_mut().unwrap().anchor = Some(0);
    for (origin, action, expected) in [
        (60, volna_core::Action::NextEdge, 61),
        (60, volna_core::Action::PrevEdge, 59),
        (0, volna_core::Action::PrevEdge, 0),
        (127, volna_core::Action::NextEdge, 127),
    ] {
        app.doc.shared.cursor = Some(origin);
        let revision = app.workspace.scheduler.revision();
        app.handle(Command::Action(action));
        assert_eq!(
            app.doc.shared.cursor,
            Some(origin),
            "input must not execute reader work"
        );
        drain(&mut view, &mut app);
        assert_eq!(app.doc.shared.cursor, Some(expected));
        assert_eq!(
            app.workspace.scheduler.revision(),
            revision + u64::from(origin != expected)
        );
        assert!(
            app.panels
                .focused_waves()
                .unwrap()
                .items
                .iter()
                .all(|item| item.history.is_none())
        );
        assert!(app.take_requests().is_empty());
    }
}

#[test]
fn edge_requests_are_replaced_and_stale_cursor_or_selection_results_are_discarded() {
    let (_dir, mut app, session) = fixture();
    let mut view = QueryView::attach(
        &mut app,
        session,
        Limits {
            work: 1,
            ..limits()
        },
        16,
        8,
        &Budget::new(65536),
    )
    .unwrap();
    app.handle(Command::AddVars(vec![0, 1]));
    layout(&mut app, &Theme::one_dark());
    drain(&mut view, &mut app);
    app.panels.focused_waves_mut().unwrap().selected = [0].into();
    app.panels.focused_waves_mut().unwrap().anchor = Some(0);
    app.doc.shared.cursor = Some(60);
    app.handle(Command::Action(volna_core::Action::PrevEdge));
    let _ = view.poll(&mut app, &mut Context::from_waker(Waker::noop()));
    app.handle(Command::Action(volna_core::Action::NextEdge));
    drain(&mut view, &mut app);
    assert_eq!(app.doc.shared.cursor, Some(61));

    app.handle(Command::Action(volna_core::Action::PrevEdge));
    let _ = view.poll(&mut app, &mut Context::from_waker(Waker::noop()));
    app.doc.shared.cursor = Some(90);
    drain(&mut view, &mut app);
    assert_eq!(app.doc.shared.cursor, Some(90));

    app.handle(Command::Action(volna_core::Action::PrevEdge));
    let _ = view.poll(&mut app, &mut Context::from_waker(Waker::noop()));
    let waves = app.panels.focused_waves_mut().unwrap();
    waves.selected = [1].into();
    waves.anchor = Some(1);
    drain(&mut view, &mut app);
    assert_eq!(app.doc.shared.cursor, Some(90));
    // Returning to the old selection must not revive a discarded command.
    let waves = app.panels.focused_waves_mut().unwrap();
    waves.selected = [0].into();
    waves.anchor = Some(0);
    drain(&mut view, &mut app);
    assert_eq!(app.doc.shared.cursor, Some(90));
}

#[test]
fn edge_navigation_respects_unlinked_cursor_and_document_replacement() {
    let (_dir, mut app, session) = fixture();
    let mut view =
        QueryView::attach(&mut app, session, limits(), 16, 8, &Budget::new(65536)).unwrap();
    app.handle(Command::AddVars(vec![0]));
    layout(&mut app, &Theme::one_dark());
    drain(&mut view, &mut app);
    app.doc.shared.cursor = Some(10);
    let waves = app.panels.focused_waves_mut().unwrap();
    waves.selected = [0].into();
    waves.anchor = Some(0);
    waves.link.cursor = false;
    waves.local_cursor = Some(70);
    app.handle(Command::Action(volna_core::Action::NextEdge));
    drain(&mut view, &mut app);
    assert_eq!(app.panels.focused_waves().unwrap().local_cursor, Some(71));
    assert_eq!(app.doc.shared.cursor, Some(10));
    app.handle(Command::Action(volna_core::Action::PrevEdge));
    let _ = view.poll(&mut app, &mut Context::from_waker(Waker::noop()));
    app.set_session(Arc::new(SynthSource::new(16)));
    let cursor = app.doc.shared.cursor;
    drain(&mut view, &mut app);
    assert!(view.is_closed());
    assert_eq!(app.doc.shared.cursor, cursor);
}

#[test]
fn hiding_a_panel_discards_queued_and_running_edge_commands() {
    for (submitted, poll_hidden) in [(false, false), (false, true), (true, false), (true, true)] {
        let (_dir, mut app, session) = fixture();
        let mut view = QueryView::attach(
            &mut app,
            session,
            Limits {
                work: 1,
                ..limits()
            },
            16,
            8,
            &Budget::new(65536),
        )
        .unwrap();
        app.handle(Command::AddVars(vec![0]));
        layout(&mut app, &Theme::one_dark());
        drain(&mut view, &mut app);
        app.doc.shared.cursor = Some(60);
        let waves = app.panels.focused_waves_mut().unwrap();
        waves.selected = [0].into();
        waves.anchor = Some(0);
        let original = app.panels.focused_id();
        app.handle(Command::Action(volna_core::Action::PrevEdge));
        if submitted {
            let _ = view.poll(&mut app, &mut Context::from_waker(Waker::noop()));
        }
        app.handle(Command::Action(volna_core::Action::NewPanel));
        assert!(!app.panels.layout().visible().contains(&original));
        if poll_hidden {
            drain(&mut view, &mut app);
        }
        assert_eq!(app.doc.shared.cursor, Some(60));
        app.handle(Command::Action(volna_core::Action::ClosePanel));
        assert_eq!(app.panels.focused_id(), original);
        drain(&mut view, &mut app);
        assert_eq!(
            app.doc.shared.cursor,
            Some(60),
            "showing a panel must not revive its discarded command"
        );
    }
}
#[test]
fn app_rows_and_viewports_drive_queries_without_frontend_policy() {
    let (_dir, mut app, session) = fixture();
    let mut view =
        QueryView::attach(&mut app, session, limits(), 16, 8, &Budget::new(65536)).unwrap();
    app.handle(Command::AddVars(vec![0, 1, 2]));
    assert!(app.take_requests().is_empty());
    let theme = Theme::one_dark();
    layout(&mut app, &theme);
    drain(&mut view, &mut app);
    let old = app.panels.focused_waves().unwrap().items[0]
        .query
        .clone()
        .unwrap();
    for row in &app.panels.focused_waves().unwrap().items {
        assert!(row.query.is_some());
        assert!(row.history.is_none());
    }
    assert!(matches!(
        view.poll(&mut app, &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(Progress::Idle))
    ));
    assert!(
        Arc::ptr_eq(
            &old,
            app.panels.focused_waves().unwrap().items[0]
                .query
                .as_ref()
                .unwrap()
        ),
        "idle polling must not allocate new snapshots"
    );
    app.doc.shared.viewport.set(Viewport {
        start: 64.0,
        end: 96.0,
    });
    layout(&mut app, &theme);
    drain(&mut view, &mut app);
    let new = app.panels.focused_waves().unwrap().items[0]
        .query
        .as_ref()
        .unwrap();
    assert_ne!(new.demand(), old.demand());
    app.doc.shared.cursor = Some(65);
    let scene = app.render_waves(app.panels.focused_id(), &theme, &mut MonoMeasure);
    assert!(
        scene
            .prims
            .iter()
            .any(|p| matches!(p, Prim::Quad { fill, .. } if *fill == theme.wave_dense))
    );
    assert!(app.take_requests().is_empty());
}
#[test]
fn hiding_rows_releases_snapshots_and_replacing_the_document_closes_queries() {
    let (_dir, mut app, session) = fixture();
    let mut view =
        QueryView::attach(&mut app, session, limits(), 16, 8, &Budget::new(65536)).unwrap();
    app.handle(Command::AddVars(vec![0, 1]));
    let old_panel = app.panels.focused_id();
    let theme = Theme::one_dark();
    layout(&mut app, &theme);
    drain(&mut view, &mut app);
    app.handle(Command::Panels(PanelsCommand::NewTab {
        group_of: old_panel,
    }));
    layout(&mut app, &theme);
    drain(&mut view, &mut app);
    assert!(
        app.panels
            .waves(old_panel)
            .unwrap()
            .items
            .iter()
            .all(|row| row.query.is_none())
    );
    app.set_session(Arc::new(SynthSource::new(10)));
    assert!(matches!(
        view.poll(&mut app, &mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(Progress::Idle))
    ));
    assert!(view.is_closed());
    assert!(app.doc.query_snapshot().is_none());
}
#[test]
fn failed_admission_leaves_the_existing_document_and_load_requests_unchanged() {
    let (_dir, mut app, session) = fixture();
    app.handle(Command::AddVars(vec![0]));
    assert!(QueryView::attach(&mut app, session, limits(), 16, 8, &Budget::new(0)).is_err());
    assert!(app.doc.query_snapshot().is_none());
    assert!(!app.take_requests().is_empty());
}

#[test]
fn restoring_rows_keeps_the_query_session_and_reinstalls_render_data() {
    use volna_core::workspace::Workspace;
    let (_dir, mut app, session) = fixture();
    let mut view =
        QueryView::attach(&mut app, session, limits(), 16, 8, &Budget::new(65536)).unwrap();
    app.handle(Command::AddVars(vec![0, 1]));
    let theme = Theme::one_dark();
    layout(&mut app, &theme);
    drain(&mut view, &mut app);
    let generation = app.doc.generation();
    let snapshot = app.doc.query_snapshot();
    let saved = Workspace::capture(&app, "view.vtr".into(), None).unwrap();
    saved
        .prepare(
            &app,
            "file:///tmp/view.vtr",
            "file:///tmp/view.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    assert_ne!(app.doc.generation(), generation);
    assert_eq!(app.doc.query_snapshot(), snapshot);
    assert!(app.take_requests().is_empty());
    layout(&mut app, &theme);
    drain(&mut view, &mut app);
    assert!(!view.is_closed());
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| row.query.is_some() && row.history.is_none())
    );
}

#[test]
fn query_document_loads_sidebar_and_waves_through_one_scheduler() {
    let (_dir, _resident_app, session) = fixture();
    let budget = Budget::new(1 << 20);
    let mut app = App::new();
    app.set_query_document("view.vtr".into(), session.info(), 16, &budget)
        .unwrap();
    let mut view = QueryView::attach(&mut app, session, limits(), 16, 8, &budget).unwrap();
    assert!(app.variables.rows.is_empty());
    drain(&mut view, &mut app);
    assert!(
        app.doc
            .query_hierarchy()
            .unwrap()
            .state(None)
            .unwrap()
            .complete
    );
    assert_eq!(app.variables.rows.len(), 3);
    assert!(app.variables.complete);
    app.handle(Command::AddAllVars);
    assert!(app.take_requests().is_empty());
    layout(&mut app, &Theme::one_dark());
    drain(&mut view, &mut app);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| row.query.is_some() && row.history.is_none())
    );
    app.doc.close();
    drain(&mut view, &mut app);
    assert!(view.is_closed());
}

#[test]
fn metadata_capacity_failure_keeps_accepted_pages_and_stops_refetching() {
    let (_dir, _resident_app, session) = fixture();
    let budget = Budget::new(1 << 20);
    let mut app = App::new();
    app.set_query_document("view.vtr".into(), session.info(), 2, &budget)
        .unwrap();
    let mut view = QueryView::attach(&mut app, session, limits(), 16, 8, &budget).unwrap();
    futures_lite::future::block_on(async {
        for _ in 0..1000 {
            match futures_lite::future::poll_fn(|cx| view.poll(&mut app, cx)).await {
                Err(vtr_query::Error::ResourceLimit) => return,
                Err(error) => panic!("unexpected error: {error}"),
                Ok(_) => futures_lite::future::yield_now().await,
            }
        }
        panic!("metadata capacity failure was not reported");
    });
    assert_eq!(app.doc.query_hierarchy().unwrap().len(), 2);
    assert!(!app.variables.complete);
    drain(&mut view, &mut app);
    assert_eq!(app.doc.query_hierarchy().unwrap().len(), 2);
}

#[test]
fn scheduler_fetches_referenced_scope_and_alias_names_in_bounded_parts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("names.vtr");
    let scope_name = "scope界".repeat(1200);
    let signal_name = "signalλ".repeat(1200);
    let mut writer = vtr::Writer::create(&path).unwrap();
    let root = writer.begin_scope(&scope_name, vtr::ScopeType::Module, "");
    let (_, signal) = writer.add_bits(&signal_name, 32, 4);
    writer
        .add_alias(
            &signal_name,
            vtr::VarType::Wire,
            vtr::Direction::Input,
            signal,
        )
        .unwrap();
    writer.end_scope().unwrap();
    writer.close().unwrap();
    let session =
        futures_lite::future::block_on(LocalSession::open(path, Budget::new(16 << 20), 2).unwrap())
            .unwrap();
    let budget = Budget::new(1 << 20);
    let mut app = App::new();
    app.set_query_document("names.vtr".into(), session.info(), 8, &budget)
        .unwrap();
    let mut view = QueryView::attach(
        &mut app,
        session,
        Limits {
            bytes: 1024,
            records: 1,
            work: 4,
        },
        16,
        8,
        &budget,
    )
    .unwrap();
    drain(&mut view, &mut app);
    assert_eq!(app.scopes.selected, Some(root.0 as usize));
    let hierarchy = app.doc.browser_hierarchy().unwrap();
    assert_eq!(
        hierarchy.scope(root.0 as usize).unwrap().name,
        Some(scope_name.as_str())
    );
    assert_eq!(app.variables.rows.len(), 2);
    assert!(app.variables.complete);
    for &var in &app.variables.rows {
        assert_eq!(
            hierarchy.variable(var).unwrap().name,
            Some(signal_name.as_str())
        );
    }
    app.handle(Command::AddAllVars);
    let waves = app.panels.focused_waves().unwrap();
    assert_eq!(waves.items.len(), 2);
    assert_eq!(waves.items[0].name, signal_name);
    assert_eq!(waves.items[0].scope, scope_name);
    assert_eq!(
        waves.items[0].source.signal(),
        waves.items[1].source.signal()
    );
    volna_core::workspace::Workspace::capture(&app, "names.vtr".into(), None).unwrap();
    drop(view);
    drop(app);
    assert_eq!(
        budget.used(),
        0,
        "name assembly and cached names release client admission"
    );
}

#[test]
fn restored_paths_demand_collapsed_ancestors_without_loading_unrelated_subtrees() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    writer.begin_scope("root", vtr::ScopeType::Module, "");
    let mut selected = None;
    for level in 1..6 {
        let name = if level == 3 {
            "literal.long.scope".repeat(100)
        } else {
            format!("level{level}")
        };
        selected = Some(writer.begin_scope(&name, vtr::ScopeType::Module, ""));
    }
    let (_, signal) = writer.add_bits("wanted", 32, 4);
    for _ in 1..6 {
        writer.end_scope().unwrap();
    }
    let unrelated = writer.begin_scope("unrelated", vtr::ScopeType::Module, "");
    writer.begin_scope("hidden", vtr::ScopeType::Module, "");
    writer.add_bits("not_requested", 1, 2);
    for _ in 0..3 {
        writer.end_scope().unwrap();
    }
    writer.close().unwrap();
    let mut original = App::new();
    original.set_session(Arc::new(
        volna_core::session::LocalSession::open(&path).unwrap(),
    ));
    original.handle(Command::AddVars(vec![0]));
    original.handle(Command::ExpandAllScopes(false));
    original.handle(Command::ToggleScope(0));
    original.handle(Command::SelectScope(5));
    let saved =
        volna_core::workspace::Workspace::capture(&original, "deep.vtr".into(), None).unwrap();
    let session =
        futures_lite::future::block_on(LocalSession::open(path, Budget::new(16 << 20), 2).unwrap())
            .unwrap();
    let budget = Budget::new(1 << 20);
    let mut app = App::new();
    app.set_query_document("deep.vtr".into(), session.info(), 32, &budget)
        .unwrap();
    saved
        .prepare(
            &app,
            "file:///tmp/deep.vtr",
            "file:///tmp/deep.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    let mut view = QueryView::attach(
        &mut app,
        session,
        Limits {
            bytes: 1024,
            records: 1,
            work: 4,
        },
        16,
        8,
        &budget,
    )
    .unwrap();
    drain(&mut view, &mut app);
    let row = &app.panels.focused_waves().unwrap().items[0];
    assert_eq!(row.source.signal().unwrap().0, signal.0);
    assert_eq!(row.name, "wanted");
    assert_eq!(app.scopes.selected, Some(selected.unwrap().0 as usize));
    assert!(
        app.doc
            .query_hierarchy()
            .unwrap()
            .state(Some(unrelated.0))
            .is_none()
    );
    assert!(app.variables.complete);
    assert!(app.take_requests().is_empty());
}

#[test]
fn root_variable_paths_round_trip_between_native_and_query_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("roots.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    writer.begin_scope("(top)", vtr::ScopeType::Module, "");
    let (_, inside) = writer.add_bits("same", 1, 2);
    writer.end_scope().unwrap();
    let (_, outside) = writer.add_bits("same", 1, 2);
    writer
        .add_alias("same", vtr::VarType::Wire, vtr::Direction::Input, outside)
        .unwrap();
    writer.close().unwrap();
    let mut native = App::new();
    native.set_session(Arc::new(
        volna_core::session::LocalSession::open(&path).unwrap(),
    ));
    let h = native.doc.hierarchy().unwrap();
    assert_eq!(h.var_path(0).0, ["(top)", "same"]);
    assert_eq!(h.var_path(1), (vec!["same".into()], Some(0)));
    assert_eq!(h.var_path(2), (vec!["same".into()], Some(1)));
    native.handle(Command::AddVars(vec![1, 0, 2]));
    native.handle(Command::SelectScope(1));
    let saved =
        volna_core::workspace::Workspace::capture(&native, "roots.vtr".into(), None).unwrap();
    let session =
        futures_lite::future::block_on(LocalSession::open(path, Budget::new(16 << 20), 2).unwrap())
            .unwrap();
    let budget = Budget::new(1 << 20);
    let mut app = App::new();
    app.set_query_document("roots.vtr".into(), session.info(), 16, &budget)
        .unwrap();
    saved
        .prepare(
            &app,
            "file:///tmp/roots.vtr",
            "file:///tmp/roots.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    let mut view = QueryView::attach(&mut app, session, limits(), 16, 8, &budget).unwrap();
    drain(&mut view, &mut app);
    let signals: Vec<_> = app
        .panels
        .focused_waves()
        .unwrap()
        .items
        .iter()
        .map(|row| row.source.signal().unwrap().0)
        .collect();
    assert_eq!(signals, [outside.0, inside.0, outside.0]);
    let saved = volna_core::workspace::Workspace::capture(&app, "roots.vtr".into(), None).unwrap();
    saved
        .prepare(
            &native,
            "file:///tmp/roots.vtr",
            "file:///tmp/roots.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut native)
        .unwrap();
    assert_eq!(
        native
            .panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .map(|row| row.source.signal().unwrap().0)
            .collect::<Vec<_>>(),
        signals
    );
    assert_eq!(native.scopes.selected, Some(1));
}

#[test]
fn permitted_aliases_keep_native_and_query_shapes_identical() {
    use volna_core::data::SignalShape;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aliases.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let (_, event) = writer.add_var(
        "event",
        vtr::VarType::Event,
        vtr::Direction::Implicit,
        vtr::SignalKind::Bits {
            width: 1,
            states: 2,
        },
    );
    assert!(
        writer
            .add_alias("invalid", vtr::VarType::Wire, vtr::Direction::Input, event)
            .is_err()
    );
    writer
        .add_alias(
            "event_alias",
            vtr::VarType::Event,
            vtr::Direction::Input,
            event,
        )
        .unwrap();
    let (_, held) = writer.add_bits("held", 1, 2);
    assert!(
        writer
            .add_alias("invalid", vtr::VarType::Event, vtr::Direction::Input, held)
            .is_err()
    );
    writer
        .add_alias("reg_alias", vtr::VarType::Reg, vtr::Direction::Input, held)
        .unwrap();
    writer.close().unwrap();
    let mut native = App::new();
    native.set_session(Arc::new(
        volna_core::session::LocalSession::open(&path).unwrap(),
    ));
    native.handle(Command::AddVars(vec![0, 1, 2, 3]));
    let expected: Vec<_> = native
        .panels
        .focused_waves()
        .unwrap()
        .items
        .iter()
        .map(|row| row.shape)
        .collect();
    assert_eq!(
        expected,
        [
            SignalShape::Event,
            SignalShape::Event,
            SignalShape::Bit,
            SignalShape::Bit
        ]
    );
    let session =
        futures_lite::future::block_on(LocalSession::open(path, Budget::new(16 << 20), 2).unwrap())
            .unwrap();
    let budget = Budget::new(1 << 20);
    let mut app = App::new();
    app.set_query_document("aliases.vtr".into(), session.info(), 16, &budget)
        .unwrap();
    let mut view = QueryView::attach(&mut app, session, limits(), 16, 8, &budget).unwrap();
    drain(&mut view, &mut app);
    let node = app.doc.query_hierarchy().unwrap().declaration(3).unwrap();
    assert!(
        matches!(node.data, vtr_query::metadata::DeclarationData::Variable { type_code, alias: true, .. } if type_code == vtr::VarType::Reg.code())
    );
    app.handle(Command::AddAllVars);
    assert_eq!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .map(|row| row.shape)
            .collect::<Vec<_>>(),
        expected
    );
}

/// Replays a real saved viewport through the native asynchronous scheduler.
/// Run with VOLNA_PAN_WORKSPACE and --ignored --nocapture under --profile viewer.
#[test]
#[ignore]
fn saved_workspace_pan_latency() {
    use std::time::Instant;
    use volna_core::workspace::Workspace;
    let workspace_path = std::path::PathBuf::from(
        std::env::var_os("VOLNA_PAN_WORKSPACE").expect("set VOLNA_PAN_WORKSPACE"),
    );
    let bytes = std::fs::read(&workspace_path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let trace_path = workspace_path
        .parent()
        .unwrap()
        .join(json["trace"]["path"].as_str().unwrap());
    let mut app = App::new();
    app.set_session(Arc::new(
        volna_core::session::LocalSession::open(&trace_path).unwrap(),
    ));
    let session = futures_lite::future::block_on(
        LocalSession::open(trace_path.clone(), Budget::new(512 << 20), 4).unwrap(),
    )
    .unwrap();
    let mut view = QueryView::attach(
        &mut app,
        session,
        Limits {
            bytes: 256 << 10,
            records: 128,
            work: 4096,
        },
        4096,
        256,
        &Budget::new(128 << 20),
    )
    .unwrap();
    Workspace::parse(&bytes)
        .unwrap()
        .prepare(
            &app,
            url::Url::from_file_path(&trace_path).unwrap().as_str(),
            url::Url::from_file_path(&workspace_path).unwrap().as_str(),
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    let theme = Theme::one_dark();
    let start = json["shared"]["viewport"]["start"].as_f64().unwrap();
    let end = json["shared"]["viewport"]["end"].as_f64().unwrap();
    for pan in 0..11 {
        if let Ok(first) = std::env::var("VOLNA_PAN_FIRST_ROW") {
            app.panels.focused_waves_mut().unwrap().scroll_y = first.parse::<f32>().unwrap() * theme.row_height;
        }
        let shift = f64::from(pan) * (end - start) * 0.05;
        app.doc.shared.viewport.set(Viewport {
            start: start + shift,
            end: end + shift,
        });
        app.layout_waves(
            app.panels.focused_id(),
            Rect::from_xywh(0.0, 0.0, 1920.0, 1000.0),
            &theme,
        );
        let visible = app.panels.focused_waves().unwrap().last_layout().rows.len();
        let timer = Instant::now();
        drain(&mut view, &mut app);
        let query_ms = timer.elapsed().as_secs_f64() * 1000.0;
        let timer = Instant::now();
        let scene = app.render_waves(app.panels.focused_id(), &theme, &mut MonoMeasure);
        std::hint::black_box(&scene);
        println!(
            "PAN {pan}: visible={visible} query_ms={query_ms:.3} render_ms={:.3}",
            timer.elapsed().as_secs_f64() * 1000.0
        );
    }
}
