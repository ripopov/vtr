//! Headless Transaction panel tests over the checked-in example traces: the
//! document selection and its writers, the prepared view of a record, jumps
//! that load another track, history, pinning, limits and the workspace.

use std::path::PathBuf;
use std::sync::Arc;

use volna_core::app::{Action, App, Command};
use volna_core::data::text::Radix;
use volna_core::data::transactions::{TrackRef, TransactionRef, TxStatus};
use volna_core::geometry::{Modifiers, MouseButton, Rect, point};
use volna_core::panels::PanelId;
use volna_core::pipeline::Rows;
use volna_core::scene::MonoMeasure;
use volna_core::session::{OpenSpec, Session};
use volna_core::transaction::{RefRole, SectionKey, TransactionCommand, TxPanelState, TxView};
use volna_core::wave::PointerEvent;
use volna_core::workspace::Workspace;
use volna_core::{Instant, Theme};

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

fn frame(app: &mut App, id: PanelId) {
    let theme = Theme::one_dark();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 600.0), &theme)
        .unwrap();
    app.render_panel(id, &theme, &mut MonoMeasure);
}

/// The catalog track whose dotted path is `path`.
fn track(session: &dyn Session, path: &str) -> TrackRef {
    session
        .tracks()
        .iter()
        .find(|t| t.path.join(".") == path)
        .unwrap_or_else(|| panic!("no track {path}"))
        .id
}

/// The record of `track` whose `vtr.label` is `label`: tests name records the
/// way a reader sees them, so they do not depend on the writer's id order.
fn labelled(session: &dyn Session, track: TrackRef, label: &str) -> TransactionRef {
    use volna_core::data::transactions::AttributeValue;
    let loaded = session.load_track(track).unwrap();
    loaded
        .generators
        .iter()
        .flat_map(|g| g.transactions())
        .find(|t| {
            t.attributes.iter().any(|a| {
                a.key == "vtr.label"
                    && matches!(&a.value, AttributeValue::Text(text) if text == label)
            })
        })
        .unwrap_or_else(|| panic!("no record labelled {label:?}"))
        .id
}

/// A trace with one pipeline panel open, loaded and laid out.
fn with_pipeline(name: &str, stream: &str) -> (App, Arc<dyn Session>, PanelId) {
    let session = example(name);
    let mut app = App::new();
    app.set_session(session.clone());
    app.handle(Command::OpenPipeline {
        track: track(session.as_ref(), stream),
    });
    let pipeline = app.panels.focused_id();
    pump(&mut app);
    // Rows at the top, not following activity, so positions are predictable.
    app.handle(Command::PipelineActivity(
        pipeline,
        volna_core::pipeline::ActivityCommand::Toggle,
    ));
    app.panels
        .pipeline_mut(pipeline)
        .unwrap()
        .rows
        .set(volna_core::pipeline::RowView::default());
    frame(&mut app, pipeline);
    (app, session, pipeline)
}

fn view(app: &App, panel: PanelId) -> TxView {
    match app.panels.transaction(panel).unwrap().state(&app.doc) {
        TxPanelState::Ready(view) => *view,
        TxPanelState::Empty => panic!("empty"),
        TxPanelState::Loading => panic!("loading"),
        TxPanelState::Missing => panic!("missing"),
        TxPanelState::Failed(e) | TxPanelState::Refused(e) => panic!("{e}"),
    }
}

fn select(app: &mut App, panel: PanelId, track: TrackRef, id: u64) {
    app.handle(Command::SelectTransaction {
        panel,
        track,
        id: TransactionRef(id),
        cursor: None,
    });
}

/// Select a record in the pipeline and show it; returns the new panel.
fn show(app: &mut App, pipeline: PanelId, track: TrackRef, id: u64) -> PanelId {
    select(app, pipeline, track, id);
    app.handle(Command::ShowTransaction { from: pipeline });
    let panel = app.panels.focused_id();
    assert!(app.panels.transaction(panel).is_some());
    panel
}

#[test]
fn a_click_selects_the_row_enter_shows_it_and_every_reader_follows() {
    let (mut app, session, pipeline) = with_pipeline("pipeline_showcase.vtr", "soc.cpu0.pipeline");
    let insn = track(session.as_ref(), "soc.cpu0.pipeline.instruction");
    // The operand-stalled add is on screen when the panel opens.
    let (row, position, cycle) = {
        let p = app.panels.pipeline(pipeline).unwrap();
        let Rows::Ready(set) = p.rows(&app.doc) else {
            panic!("not loaded")
        };
        let row = set.row_of(insn, TransactionRef(4)).unwrap();
        let layout = p.last_layout();
        assert!(layout.row_range.contains(&row));
        let vp = p.nav.viewport(&app.doc);
        let x = layout.cells.left() + vp.x_of(6.5, layout.cells_width_f64()) as f32;
        (
            row,
            point(x, layout.row_y(row) + layout.rows.row_px / 2.0),
            6,
        )
    };
    app.handle(Command::Pointer(
        pipeline,
        PointerEvent::Down {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(pipeline, PointerEvent::Up));
    let selection = app.doc.selection().unwrap();
    assert_eq!((selection.track, selection.id), (insn, TransactionRef(4)));
    assert_eq!(selection.origin, pipeline);
    assert_eq!(app.doc.shared.cursor, Some(cycle));
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.selected_row(&app.doc), Some(row));

    // The painter outlines the row and draws its four wakeups as arrows.
    frame(&mut app, pipeline);
    let theme = Theme::one_dark();
    let arrows = app
        .scene()
        .prims
        .iter()
        .filter(|p| {
            matches!(p, volna_core::scene::Prim::Lines { color, .. }
                if *color == theme.tx_relation_in || *color == theme.tx_relation_out)
        })
        .count();
    assert_eq!(arrows, 2, "one batch into the row and one out of it");

    // Enter opens a Transaction panel beside the pipeline, showing the record.
    app.handle(Command::ShowTransaction { from: pipeline });
    let panel = app.panels.focused_id();
    let v = view(&app, panel);
    assert_eq!(v.identity.id, TransactionRef(4));
    assert_eq!(
        v.identity.label.as_deref(),
        Some("80000008: add  a3, a1, a0")
    );
    // A second Enter focuses the same panel instead of opening another one.
    app.handle(Command::ShowTransaction { from: pipeline });
    assert_eq!(app.panels.focused_id(), panel);
    let count = app
        .panels
        .iter()
        .filter(|p| p.kind.transaction().is_some())
        .count();
    assert_eq!(count, 1);

    // ↓ moves the selection; the unpinned panel follows it.
    app.handle(Command::Panels(volna_core::panels::PanelsCommand::Focus(
        pipeline,
    )));
    app.handle(Command::Action(Action::MoveSelectionDown));
    let p = app.panels.pipeline(pipeline).unwrap();
    assert_eq!(p.selected_row(&app.doc), Some(row + 1));
    assert_ne!(view(&app, panel).identity.id, TransactionRef(4));

    // Escape clears the highlight but not the page being read, then the cursor.
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(app.doc.selection(), None);
    assert!(app.doc.shared.cursor.is_some());
    assert!(matches!(
        app.panels.transaction(panel).unwrap().state(&app.doc),
        TxPanelState::Ready(_)
    ));
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(app.doc.shared.cursor, None);
}

#[test]
fn the_view_reads_identity_timing_lifeline_stages_and_relations() {
    let (mut app, session, pipeline) = with_pipeline("pipeline_showcase.vtr", "soc.cpu0.pipeline");
    let insn = track(session.as_ref(), "soc.cpu0.pipeline.instruction");
    let panel = show(&mut app, pipeline, insn, 33);
    let v = view(&app, panel);
    assert_eq!(v.identity.stream, ["soc", "cpu0", "pipeline"]);
    assert_eq!(v.identity.generator, "instruction");
    assert_eq!(
        v.identity.label.as_deref(),
        Some("80000060: lw   a1, 0(a2)")
    );
    assert_eq!(v.identity.status, TxStatus::Unset);
    assert_eq!(v.identity.status_text, "ended");
    assert_eq!((v.timing.begin, v.timing.end), (33, Some(47)));
    assert_eq!(v.timing.duration, 14);
    assert!(
        v.timing.begin_text.contains("cycle"),
        "{}",
        v.timing.begin_text
    );

    // One primary lane with five cells, in the pipeline's ladder colours.
    assert_eq!(v.lifeline.len(), 1);
    let lane = &v.lifeline[0];
    assert!(lane.primary);
    let names: Vec<_> = lane.cells.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["F", "D", "X", "M", "W"]);
    let palette = app.panels.pipeline(pipeline).unwrap().palette();
    assert_eq!(lane.cells[3].style, palette.style("M"));
    assert!((lane.cells[3].width - 10.0 / 14.0).abs() < 1e-4);

    // The label is the title, never a row; the pc cycles its radix per key.
    let keys: Vec<_> = v.attributes.rows.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["insn_id", "pc", "iteration"]);
    assert_eq!(v.attributes.total, 3);
    let pc = &v.attributes.rows[1];
    assert_eq!(
        (pc.value.as_str(), pc.radix),
        ("2147483744", Some(Radix::Dec))
    );
    app.handle(Command::Transaction(
        panel,
        TransactionCommand::Radix("pc".into()),
    ));
    assert_eq!(view(&app, panel).attributes.rows[1].value, "0x80000060");

    // The cache miss is a stage attribute, shown as a chip on its stage.
    let m = v.stages.rows.iter().find(|s| s.name == "M").unwrap();
    assert!(
        m.attributes
            .iter()
            .any(|c| c.key == "miss" && c.value == "true")
    );
    assert!((m.share - 10.0 / 14.0).abs() < 1e-4);

    // Relations by kind and direction; the caused bus read is on a track no
    // panel has loaded, so it names that track and loads on a jump.
    let related: Vec<_> = v
        .related
        .rows
        .iter()
        .map(|r| (r.group.as_str(), r.target.transaction.0, r.loaded))
        .collect();
    assert_eq!(
        related,
        [
            ("causes · to", 34, false),
            ("wakeup · to", 36, true),
            ("wakeup · from", 28, true)
        ]
    );
    let caused = &v.related.rows[0];
    assert_eq!(caused.role, RefRole::Relation { outgoing: true });
    assert_eq!(
        caused.track.as_deref(),
        Some(&["soc".to_owned(), "l2".into(), "bus".into(), "read".into()][..])
    );
    assert_eq!(
        caused.relation_label.as_deref(),
        Some("instruction 24 causes request")
    );

    // Times link to the shared cursor, and the lifetime to the viewport.
    app.handle(Command::Transaction(panel, TransactionCommand::Cursor(36)));
    assert_eq!(app.doc.shared.cursor, Some(36));
    assert!(
        view(&app, panel)
            .cursor
            .is_some_and(|c| (c - 3.0 / 14.0).abs() < 1e-4)
    );
    app.handle_at(
        Command::Transaction(panel, TransactionCommand::FitLifetime),
        Instant::now(),
    );
    let target = app.doc.shared.viewport.target();
    assert!(target.start <= 33.0 && target.end >= 47.0 && target.width() < 20.0);

    // The complete record copies as TSV.
    let tsv = app
        .panels
        .transaction(panel)
        .unwrap()
        .copy_tsv(&app.doc)
        .unwrap();
    assert!(tsv.starts_with("Generator\tID\tBegin\tEnd\tDuration\tStatus\tKind\n"));
    assert!(tsv.contains("M\t0\t36\t46\tmiss=true"), "{tsv}");
}

#[test]
fn a_jump_loads_the_target_track_back_returns_and_reveal_finds_the_row() {
    let (mut app, session, pipeline) = with_pipeline("pipeline_showcase.vtr", "soc.cpu0.pipeline");
    let insn = track(session.as_ref(), "soc.cpu0.pipeline.instruction");
    let read = track(session.as_ref(), "soc.l2.bus.read");
    let panel = show(&mut app, pipeline, insn, 33);
    app.handle(Command::Transaction(
        panel,
        TransactionCommand::Jump {
            track: read,
            id: TransactionRef(34),
        },
    ));
    assert!(matches!(
        app.panels.transaction(panel).unwrap().state(&app.doc),
        TxPanelState::Loading
    ));
    pump(&mut app);
    let v = view(&app, panel);
    assert_eq!(v.identity.label.as_deref(), Some("read 0x10000020"));
    assert_eq!(v.identity.parent, Some(TransactionRef(33)));
    assert_eq!(v.related.rows[0].role, RefRole::Parent);
    assert_eq!(v.related.rows[0].target.transaction, TransactionRef(33));
    assert!(v.related.rows[0].loaded);
    // The jump is the selection now; the cpu0 pipeline has no row for it.
    assert_eq!(app.doc.selection().unwrap().id, TransactionRef(34));
    assert_eq!(
        app.panels
            .pipeline(pipeline)
            .unwrap()
            .selected_row(&app.doc),
        None
    );
    // Seen from the instruction, the loaded bus read is also its child.
    let model = app.panels.transaction(panel).unwrap();
    assert!(model.can_go_back() && !model.can_go_forward());

    app.handle(Command::Transaction(panel, TransactionCommand::Back));
    let v = view(&app, panel);
    assert_eq!(v.identity.id, TransactionRef(33));
    // Back released the bus track, so its child edge is no longer known.
    assert!(!v.related.rows.iter().any(|r| r.role == RefRole::Child));
    // Any panel holding that generator makes the child reachable again.
    let focused = app.panels.focused_id();
    app.handle(Command::OpenPipeline {
        track: track(session.as_ref(), "soc.l2.bus"),
    });
    pump(&mut app);
    app.handle(Command::Panels(volna_core::panels::PanelsCommand::Focus(
        focused,
    )));
    let v = view(&app, panel);
    assert!(v.related.rows.iter().any(|r| r.role == RefRole::Child
        && r.target.transaction == TransactionRef(34)
        && r.loaded));
    assert_eq!(app.doc.selection().unwrap().id, TransactionRef(33));
    assert!(app.panels.transaction(panel).unwrap().can_go_forward());

    // Reveal scrolls the pipeline to the row and fits time to the lifetime.
    app.handle_at(Command::RevealTransaction { panel }, Instant::now());
    assert_eq!(app.panels.focused_id(), pipeline);
    frame(&mut app, pipeline);
    let p = app.panels.pipeline(pipeline).unwrap();
    let row = p.selected_row(&app.doc).unwrap();
    assert!(p.last_layout().row_range.contains(&row));

    // Closing the pipeline keeps the panel's record: it retains its own track.
    app.handle(Command::Panels(volna_core::panels::PanelsCommand::Close(
        pipeline,
    )));
    assert_eq!(view(&app, panel).identity.id, TransactionRef(33));
    // Reveal now opens the record's track as a pipeline.
    app.handle(Command::RevealTransaction { panel });
    assert!(app.panels.pipeline(app.panels.focused_id()).is_some());
}

#[test]
fn a_pinned_panel_keeps_its_record_and_the_next_one_opens_beside_it() {
    let (mut app, session, pipeline) = with_pipeline("pipeline_showcase.vtr", "soc.cpu0.pipeline");
    let insn = track(session.as_ref(), "soc.cpu0.pipeline.instruction");
    let first = show(&mut app, pipeline, insn, 4);
    app.handle(Command::Transaction(first, TransactionCommand::Pin(true)));
    let second = show(&mut app, pipeline, insn, 33);
    assert_ne!(first, second);
    assert_eq!(view(&app, first).identity.id, TransactionRef(4));
    assert_eq!(view(&app, second).identity.id, TransactionRef(33));
    // Unpinning adopts the current selection.
    app.handle(Command::Transaction(first, TransactionCommand::Pin(false)));
    assert_eq!(view(&app, first).identity.id, TransactionRef(33));
    assert!(app.debug_state().contains("transaction"));
}

#[test]
fn every_value_tag_an_open_record_and_a_cross_stream_parent_read_as_recorded() {
    let (mut app, session, pipeline) = with_pipeline("feature_showcase.vtr", "soc.dma.memory_bus");
    let read = track(session.as_ref(), "soc.dma.memory_bus.read");
    let write = track(session.as_ref(), "soc.dma.memory_bus.write");
    let transfer = labelled(session.as_ref(), read, "DMA transfer 0");
    let submission = labelled(
        session.as_ref(),
        track(session.as_ref(), "firmware.driver.submit_transfer"),
        "submission 0",
    );
    let instruction = labelled(
        session.as_ref(),
        track(session.as_ref(), "soc.cpu.thread0.instructions"),
        "pc 0x80000000",
    );
    let unfinished = labelled(session.as_ref(), write, "unfinished write 0");
    let panel = show(&mut app, pipeline, read, transfer.0);
    let v = view(&app, panel);
    assert_eq!(v.identity.kind, Some("consumer"));
    assert_eq!(v.identity.status_text, "ok");
    assert_eq!(v.attributes.total, 18);
    assert!(v.filterable());
    let kinds: Vec<_> = v.attributes.rows.iter().map(|a| a.kind).collect();
    for kind in [
        "null", "bool", "int", "real", "text", "logic", "time", "enum", "pointer", "fixed", "list",
        "map",
    ] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    let message = v
        .attributes
        .rows
        .iter()
        .find(|a| a.key == "message")
        .unwrap();
    assert_eq!(message.value, "DMA → memory: café ✓");
    // One event, one stage on a non-primary lane of its own record.
    assert_eq!(v.events.rows.len(), 1);
    assert_eq!(
        (v.events.rows[0].name.as_str(), v.events.rows[0].time),
        ("request", 104)
    );
    assert_eq!(v.event_ticks.len(), 1);
    assert_eq!(v.stages.rows[0].lane, "memory");
    // The firmware span that parents it lives on another, unloaded stream.
    let parent = &v.related.rows[0];
    assert_eq!(parent.role, RefRole::Parent);
    assert_eq!(parent.target.transaction, submission);
    assert!(!parent.loaded);
    assert!(
        v.related
            .rows
            .iter()
            .any(|r| r.group == "request · from" && r.target.transaction == instruction)
    );

    // The limit cuts rows, never the count; the filter narrows the rows.
    app.panels
        .transaction_mut(panel)
        .unwrap()
        .set_detail_items(5);
    let v = view(&app, panel);
    assert_eq!((v.attributes.rows.len(), v.attributes.total), (5, 18));
    assert!(v.attributes.cut());
    app.handle(Command::Transaction(
        panel,
        TransactionCommand::Filter("sign".into()),
    ));
    let v = view(&app, panel);
    let keys: Vec<_> = v.attributes.rows.iter().map(|a| a.key.as_str()).collect();
    assert_eq!(keys, ["signed", "unsigned"]);
    assert_eq!((v.attributes.matched, v.attributes.total), (2, 18));
    app.handle(Command::Transaction(
        panel,
        TransactionCommand::Collapse(SectionKey::Attributes, true),
    ));
    assert!(view(&app, panel).attributes.collapsed);

    // A record still open at the end has no fabricated end.
    select(&mut app, pipeline, write, unfinished.0);
    let v = view(&app, panel);
    assert_eq!(v.identity.status, TxStatus::Open);
    assert_eq!((v.timing.end, v.timing.end_text.as_str()), (None, "open"));
    assert!(v.timing.duration_text.starts_with('≥'));
}

#[test]
fn the_workspace_restores_a_pinned_record_and_the_reader_choices() {
    let (mut app, session, pipeline) = with_pipeline("pipeline_showcase.vtr", "soc.cpu0.pipeline");
    let insn = track(session.as_ref(), "soc.cpu0.pipeline.instruction");
    let pinned = show(&mut app, pipeline, insn, 33);
    for command in [
        TransactionCommand::Pin(true),
        TransactionCommand::Radix("pc".into()),
        TransactionCommand::Collapse(SectionKey::Events, true),
    ] {
        app.handle(Command::Transaction(pinned, command));
    }
    let free = show(&mut app, pipeline, insn, 4);
    let saved = Workspace::capture(&app, "trace.vtr".into(), None).unwrap();
    let json = serde_json::to_value(&saved).unwrap();
    let entry = json["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "transaction" && p["pinned"].is_object())
        .unwrap()
        .clone();
    assert_eq!(entry["pinned"]["id"], 33);
    assert_eq!(entry["radix"]["pc"], "hex");

    let mut restored = App::new();
    restored.set_session(session.clone());
    let plan = Workspace::parse(&saved.to_bytes().unwrap())
        .unwrap()
        .prepare(
            &restored,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )
        .unwrap();
    assert!(plan.report().notices.is_empty(), "{:?}", plan.report());
    plan.commit(&mut restored).unwrap();
    pump(&mut restored);
    let v = view(&restored, pinned);
    assert_eq!(v.identity.id, TransactionRef(33));
    assert_eq!(v.attributes.rows[1].value, "0x80000060");
    assert!(v.events.collapsed);
    // An unpinned panel restores empty and waits for the next selection.
    assert!(matches!(
        restored
            .panels
            .transaction(free)
            .unwrap()
            .state(&restored.doc),
        TxPanelState::Empty
    ));
    select(&mut restored, pipeline, insn, 4);
    assert_eq!(view(&restored, free).identity.id, TransactionRef(4));
    assert_eq!(view(&restored, pinned).identity.id, TransactionRef(33));
}

#[test]
fn a_table_row_is_the_document_selection() {
    let session = example("pipeline_showcase.vtr");
    let mut app = App::new();
    app.set_session(session.clone());
    let read = track(session.as_ref(), "soc.l2.bus.read");
    let generator = session
        .hierarchy()
        .generators
        .iter()
        .position(|g| g.track == read)
        .unwrap();
    app.handle(Command::OpenTable {
        selected: vec![volna_core::data::Member::Generator(generator)],
        clicked: None,
    });
    let table = app.panels.focused_id();
    pump(&mut app);
    frame(&mut app, table);
    app.handle(Command::Table(
        table,
        volna_core::table::TableCommand::GoTo(3),
    ));
    let selection = app.doc.selection().unwrap();
    assert_eq!((selection.track, selection.origin), (read, table));
    app.handle(Command::ShowTransaction { from: table });
    let panel = app.panels.focused_id();
    assert_eq!(view(&app, panel).identity.id, selection.id);
    // A jump elsewhere in the same generator moves the table's highlight.
    let other = session_first_other(&app, read, selection.id);
    app.handle(Command::Transaction(
        panel,
        TransactionCommand::Jump {
            track: read,
            id: other,
        },
    ));
    assert_eq!(
        app.panels
            .get(table)
            .unwrap()
            .kind
            .table()
            .unwrap()
            .selected,
        Some(volna_core::table::RowIdentity::Transaction(other))
    );
}

fn session_first_other(app: &App, track: TrackRef, not: TransactionRef) -> TransactionRef {
    app.doc
        .resident_generator(track)
        .unwrap()
        .transactions()
        .iter()
        .map(|tx| tx.id)
        .find(|id| *id != not)
        .unwrap()
}
