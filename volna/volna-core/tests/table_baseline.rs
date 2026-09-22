//! Headless vertical-slice checks for the reduced table panel.

use std::path::PathBuf;

use volna_core::app::{App, Command, PanelLayout};
use volna_core::data::Member;
use volna_core::geometry::Rect;
use volna_core::scene::MonoMeasure;
use volna_core::session::{LoadRequest, OpenSpec, Session};
use volna_core::table::columns::TransactionColumn;
use volna_core::table::{RowIdentity, TableCommand, TableSource, TableState};
use volna_core::workspace::Workspace;
use volna_core::{Instant, Theme};

fn fixture() -> std::sync::Arc<dyn Session> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples/pipeline_showcase.vtr");
    OpenSpec::Path(path).open().unwrap()
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            break;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
}

fn frame(app: &mut App, panel: volna_core::panels::PanelId) {
    let bounds = Rect::from_xywh(0.0, 0.0, 1000.0, 500.0);
    assert!(matches!(
        app.layout_panel(panel, bounds, &Theme::one_dark()),
        Some(PanelLayout::Table(_))
    ));
    app.render_panel(panel, &Theme::one_dark(), &mut MonoMeasure);
}

#[test]
fn generator_table_loads_selects_follows_copies_and_keeps_bounded_rows() {
    let session = fixture();
    let mut app = App::new();
    app.set_session(session.clone());
    let budget = app.doc.session().unwrap().memory_budget().unwrap();
    let session_bytes = budget.used();
    assert!(session_bytes > 0, "the native VTR owner must be admitted");
    app.handle(Command::OpenTable {
        selected: vec![Member::Generator(0)],
        clicked: None,
    });
    let panel = app.panels.focused_id();
    assert!(matches!(
        app.take_requests().as_slice(),
        [LoadRequest::Track { .. }]
    ));
    // Requeue through the ordinary pump after observing request shape.
    app.handle(Command::Table(panel, TableCommand::Cancel));
    app.handle(Command::Table(panel, TableCommand::Retry));
    pump(&mut app);
    assert!(
        budget.used() > session_bytes + volna_core::table::model::PANEL_BYTES,
        "the native track and table must share the session ledger"
    );
    frame(&mut app, panel);
    let table = app.panels.get(panel).unwrap().kind.table().unwrap();
    assert!(matches!(table.state, TableState::Ready));
    assert!(table.len() > 20);
    assert!(table.window.rows.len() <= 256);
    assert!(matches!(table.source, TableSource::Generator(_)));

    let now = Instant::now();
    app.handle_at(Command::Table(panel, TableCommand::GoTo(10)), now);
    let table = app.panels.get(panel).unwrap().kind.table().unwrap();
    assert!(matches!(table.selected, Some(RowIdentity::Transaction(_))));
    assert_eq!(app.doc.shared.cursor, table.row_time(10));
    assert!(
        table
            .copy_tsv()
            .unwrap()
            .starts_with("Generator\tID\tBegin\tEnd\tDuration\tStatus\n")
    );

    // Selecting a row is a document-wide selection, not a private strip.
    let selection = app.doc.selection().expect("the row is the selection");
    assert_eq!(selection.origin, panel);
    assert_eq!(
        Some(volna_core::table::RowIdentity::Transaction(selection.id)),
        app.panels
            .get(panel)
            .unwrap()
            .kind
            .table()
            .unwrap()
            .selected
    );
    app.handle(Command::Table(
        panel,
        TableCommand::ToggleTransactionColumn(TransactionColumn::Id),
    ));
    frame(&mut app, panel);
    let table = app.panels.get(panel).unwrap().kind.table().unwrap();
    assert!(
        table.accessible_rows().count() <= volna_core::table::layout::MAX_PREPARED_ROWS as usize
    );
    assert!(app.scene().texts().any(|text| text == "ID"));
}

#[test]
fn fixed_signal_set_builds_shared_change_axis_and_workspace_reopens_at_first_row() {
    let session = fixture();
    let mut app = App::new();
    app.set_session(session.clone());
    assert!(session.hierarchy().vars.len() >= 2);
    app.handle(Command::OpenTable {
        selected: vec![Member::Var(0), Member::Var(1)],
        clicked: None,
    });
    let panel = app.panels.focused_id();
    pump(&mut app);
    frame(&mut app, panel);
    let table = app.panels.get(panel).unwrap().kind.table().unwrap();
    assert!(matches!(table.source, TableSource::Signals(_)));
    assert!(!table.is_empty());
    let time = table.row_time(0).unwrap();
    app.handle(Command::Table(panel, TableCommand::Select(0)));
    assert_eq!(app.doc.shared.cursor, Some(time));
    let tsv = app
        .panels
        .get(panel)
        .unwrap()
        .kind
        .table()
        .unwrap()
        .copy_tsv()
        .unwrap();
    assert!(tsv.starts_with("Time\t"));

    let saved = Workspace::capture(&app, "file:///tmp/trace.vtr".into(), None).unwrap();
    let bytes = saved.to_bytes().unwrap();
    fn contains_table_v2(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                (object.get("kind").and_then(|value| value.as_str()) == Some("table")
                    && object.get("version").and_then(|value| value.as_u64()) == Some(2))
                    || object.values().any(contains_table_v2)
            }
            serde_json::Value::Array(values) => values.iter().any(contains_table_v2),
            _ => false,
        }
    }
    assert!(contains_table_v2(&serde_json::from_slice(&bytes).unwrap()));
    let parsed = Workspace::parse(&bytes).unwrap();
    let plan = parsed
        .prepare(
            &app,
            "file:///tmp/trace.vtr",
            "file:///tmp/session.volna.json",
        )
        .unwrap();
    plan.commit(&mut app).unwrap();
    pump(&mut app);
    let restored = app.panels.get(panel).unwrap().kind.table().unwrap();
    assert!(matches!(restored.source, TableSource::Signals(_)));
    assert_eq!(restored.viewport.top, 0);
    assert_eq!(restored.selected, None);
}
