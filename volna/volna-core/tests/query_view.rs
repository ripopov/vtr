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
