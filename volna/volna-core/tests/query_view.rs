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
