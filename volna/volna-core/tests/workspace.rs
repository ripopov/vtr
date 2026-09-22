use serde_json::{Value, json};
use std::sync::Arc;
use volna_core::data::synth::SynthSource;
use volna_core::panels::{PanelId, PanelsCommand};
use volna_core::workspace::{MAX_BYTES, Workspace, resolve_trace};
use volna_core::{Action, App, Command};

const TRACE: &str = "vscode-remote://ssh-remote+board/home/user/trace.vtr";
const LOCATION: &str = "vscode-remote://ssh-remote+board/home/user/trace.vtr.volna.json";
fn app() -> App {
    let mut app = App::new();
    app.set_session(Arc::new(SynthSource::new(100)));
    app.handle(Command::AddVars(vec![0, 1, 2]));
    app
}
fn capture(app: &App) -> Workspace {
    Workspace::capture(app, "trace.vtr".into(), None).unwrap()
}
fn value(app: &App) -> Value {
    serde_json::to_value(capture(app)).unwrap()
}
fn prepare(value: &Value, app: &App) -> anyhow::Result<volna_core::workspace::RestorePlan> {
    Workspace::parse(&serde_json::to_vec(value)?).and_then(|ws| ws.prepare(app, TRACE, LOCATION))
}

#[test]
fn complete_round_trip_preserves_layout_rows_links_chrome_and_exact_times() {
    let mut app = app();
    app.handle(Command::Action(Action::SplitRight));
    app.handle(Command::Action(Action::ToggleCursorLink));
    app.handle(Command::Action(Action::ToggleViewportLink));
    let w = app.panels.focused_waves_mut().unwrap();
    w.nav.local_cursor = Some((1 << 54) + 19);
    w.nav
        .local_viewport
        .set(volna_core::wave::viewport::Viewport {
            start: -13.5,
            end: 420.25,
        });
    w.scroll_y = 2.5;
    app.doc.shared.cursor = Some(u64::MAX - 1);
    app.doc.add_marker(u64::MAX - 2);
    app.doc.markers[0].label = Some("interrupt".into());
    app.handle(Command::SetFilter("clock".into()));
    app.handle(Command::Panels(PanelsCommand::Rename(
        app.panels.focused_id(),
        Some("decode".into()),
    )));
    let before = value(&app);
    let plan = prepare(&before, &app).unwrap();
    assert!(plan.report().notices.is_empty());
    plan.commit(&mut app).unwrap();
    assert_eq!(value(&app), before);
    assert_eq!(app.doc.shared.cursor, Some(u64::MAX - 1));
    app.doc.add_marker(10);
    assert_ne!(app.doc.markers[0].id, app.doc.markers[1].id);
}

#[test]
fn missing_rows_scopes_and_unknown_translators_survive_without_guessing() {
    let mut app = app();
    let mut saved = value(&app);
    saved["panels"][0]["rows"][0]["signal"] = json!(["missing.scope", "escaped.signal"]);
    saved["panels"][0]["rows"][0]["nth"] = json!(3);
    saved["panels"][0]["rows"][0]["format"] = json!("future-format");
    saved["panels"][0]["rows"][1]["format"] = json!("future-bus");
    saved["sidebar"]["selected_scope"] = json!(["missing.scope"]);
    saved["sidebar"]["expanded"] = json!([["missing.scope"]]);
    let plan = prepare(&saved, &app).unwrap();
    assert_eq!(plan.report().notices.len(), 5);
    plan.commit(&mut app).unwrap();
    let row = &app.panels.focused_waves().unwrap().items[0];
    assert!(row.source.signal().is_none());
    assert!(row.history.is_none());
    assert_eq!(value(&app), saved);
    app.handle(Command::Action(Action::CycleFormat));
    assert_ne!(value(&app)["panels"][0]["rows"][1]["format"], "future-bus");
}

#[test]
fn unknown_panel_payload_is_preserved_verbatim_including_large_integers() {
    let mut app = app();
    let mut saved = capture(&app);
    let raw = r#"{ "id": 999, "kind": "pipeline", "version": 47, "title": "future", "ticks":18446744073709551614, "payload" : [1, {"foo": true}] }"#;
    saved.panels.push(serde_json::from_str(raw).unwrap());
    saved.layout = volna_core::panels::Layout::Tabs {
        tabs: vec![app.panels.focused_id(), PanelId(999)],
        active: PanelId(999),
    };
    saved.focused = PanelId(999);
    let plan = saved.prepare(&app, TRACE, LOCATION).unwrap();
    assert_eq!(plan.report().notices.len(), 1);
    plan.commit(&mut app).unwrap();
    assert_eq!(capture(&app).panels[1].get(), raw);
    // ID allocation after restore cannot collide with opaque panel identities.
    app.handle(Command::Action(Action::NewPanel));
    assert_eq!(app.panels.focused_id(), PanelId(1000));
}

#[test]
fn invalid_restores_are_atomic_and_unlinked_null_cursor_is_required() {
    let mut app = app();
    let original = value(&app);
    let before = app.debug_state();
    let mut invalid = Vec::new();
    for (key, val) in [
        ("format", json!("other")),
        ("version", json!(1)),
        ("focused", json!(999)),
    ] {
        let mut v = original.clone();
        v[key] = val;
        invalid.push(v);
    }
    let mut v = original.clone();
    v["layout"]["tabs"] = json!([2, 2]);
    invalid.push(v);
    let mut v = original.clone();
    v["shared"]["viewport"]["end"] = json!(-100);
    invalid.push(v);
    let mut v = original.clone();
    v["panels"][0]["selected"] = json!([999]);
    invalid.push(v);
    let mut v = original.clone();
    v["panels"][0]["link"]["cursor"] = json!(false);
    invalid.push(v.clone());
    v["panels"][0]["cursor"] = Value::Null;
    prepare(&v, &app).unwrap().commit(&mut app).unwrap();
    assert_eq!(value(&app), v);
    prepare(&original, &app).unwrap().commit(&mut app).unwrap();
    let mut v = original.clone();
    v["shared"]["markers"] = json!([{"id":1,"time":1,"label":null},{"id":1,"time":2,"label":null}]);
    invalid.push(v);
    let mut v = original.clone();
    v["trace"]["path"] = json!("different.vtr");
    invalid.push(v);
    for v in invalid {
        assert!(prepare(&v, &app).is_err(), "accepted {v}");
        assert_eq!(app.debug_state(), before);
        assert_eq!(value(&app), original);
    }
}

#[test]
fn stale_plan_cannot_replace_a_new_trace_and_timescale_mismatch_only_warns() {
    let mut app = app();
    let mut v = value(&app);
    v["trace"]["timescale"] = json!(-3);
    let plan = prepare(&v, &app).unwrap();
    assert_eq!(plan.report().notices.len(), 1);
    app.set_session(Arc::new(SynthSource::new(20)));
    let before = app.debug_state();
    assert!(plan.commit(&mut app).is_err());
    assert_eq!(app.debug_state(), before);
}

#[test]
fn parser_rejects_truncation_oversized_files_and_excessive_nesting() {
    let bytes = capture(&app()).to_bytes().unwrap();
    for len in 0..bytes.len() {
        assert!(Workspace::parse(&bytes[..len]).is_err());
    }
    assert!(Workspace::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    let mut v = value(&app());
    for _ in 0..150 {
        v["layout"] = json!({"split":"horizontal","sizes":[1.0],"children":[v["layout"].take()]});
    }
    assert!(Workspace::parse(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn uri_resolution_preserves_scheme_authority_and_escaped_paths() {
    assert_eq!(resolve_trace("trace.vtr", LOCATION).unwrap(), TRACE);
    assert_eq!(
        resolve_trace("../other%20dir/a.vtr", LOCATION).unwrap(),
        "vscode-remote://ssh-remote+board/home/other%20dir/a.vtr"
    );
    assert_eq!(resolve_trace(TRACE, "storage:opaque").unwrap(), TRACE);
    assert!(resolve_trace("trace.vtr", "storage:opaque").is_err());
}

use volna_core::Instant;
use volna_core::workspace::persistence::{
    self, Candidate, Content, IDLE, Persistence, SaveTicket, Scheduler, Target,
};
fn file(uri: &str) -> Target {
    Target::File { uri: uri.into() }
}
fn scheduler() -> Scheduler {
    let mut s = Scheduler::default();
    s.policy = Persistence::Auto;
    s.begin(Some(file(LOCATION)), None).unwrap();
    s
}

#[test]
fn idle_scheduler_serializes_writes_and_only_acknowledges_the_exact_ticket() {
    let now = Instant::now();
    let mut s = scheduler();
    s.changed(now);
    assert!(s.next(now + IDLE / 2, false, false).is_none());
    assert!(s.next(now + IDLE, true, false).is_none());
    let first = s.next(now + IDLE * 2, false, false).unwrap();
    s.changed(now + IDLE * 2);
    assert!(s.next(now + IDLE * 4, false, true).is_none());
    let mut stale = first.clone();
    stale.epoch -= 1;
    assert!(!s.acknowledge(&stale, None, now));
    stale = first.clone();
    stale.target = file("file:///elsewhere.json");
    assert!(!s.acknowledge(&stale, None, now));
    assert!(s.acknowledge(&first, None, now));
    assert!(s.dirty());
    let latest = s.next(now + IDLE * 4, false, false).unwrap();
    assert!(!s.acknowledge(&first, None, now));
    assert!(s.acknowledge(&latest, None, now));
    assert!(!s.dirty());
    s.begin(Some(file(LOCATION)), None).unwrap();
    assert!(!s.acknowledge(&latest, None, now));
}

#[test]
fn save_as_switches_only_after_success_and_preserves_later_edits() {
    let now = Instant::now();
    let mut s = scheduler();
    let destination = file("file:///tmp/chosen.volna.json");
    s.changed(now);
    let old = s.next(now, false, true).unwrap();
    s.save_as(destination.clone());
    assert!(s.next(now, false, true).is_none());
    s.acknowledge(&old, None, now);
    let failed = s.next(now, false, false).unwrap();
    assert_eq!(failed.target, destination);
    s.acknowledge(&failed, Some("permission denied".into()), now);
    assert_eq!(s.target(), Some(&file(LOCATION)));
    s.save_as(destination.clone());
    let new = s.next(now, false, false).unwrap();
    s.changed(now);
    s.acknowledge(&new, None, now);
    assert_eq!(s.target(), Some(&destination));
    assert!(s.dirty());
    assert_eq!(
        s.next(now + IDLE, false, false).unwrap().target,
        destination
    );
}

#[test]
fn errors_retry_the_same_target_and_suspension_requires_explicit_save() {
    let now = Instant::now();
    let mut s = scheduler();
    s.changed(now);
    let first = s.next(now, false, true).unwrap();
    s.acknowledge(&first, Some("disk full".into()), now);
    assert!(s.dirty());
    assert_eq!(s.error(), Some("disk full"));
    assert!(s.next(now + IDLE / 2, false, false).is_none());
    let retry = s.next(now + IDLE, false, false).unwrap();
    assert_eq!(retry.target, first.target);
    s.acknowledge(&retry, None, now);
    s.suspend("newer workspace".into());
    s.changed(now);
    assert!(s.next(now + IDLE, false, true).is_none());
    s.save();
    let explicit = s.next(now, false, false).unwrap();
    s.acknowledge(&explicit, None, now);
    assert!(!s.suspended());
    assert!(!s.dirty());
    let mut disabled = Scheduler::default();
    disabled.changed(now);
    disabled.save_as(file(LOCATION));
    assert!(disabled.next(now, false, true).is_none());
}

#[test]
fn tickets_keep_64_bit_identity_outside_javascript_numbers() {
    let ticket = SaveTicket {
        target: file(LOCATION),
        epoch: u64::MAX - 2,
        revision: u64::MAX - 1,
    };
    let json = serde_json::to_value(&ticket).unwrap();
    assert_eq!(json["epoch"], "18446744073709551613");
    assert_eq!(json["revision"], "18446744073709551614");
    assert_eq!(serde_json::from_value::<SaveTicket>(json).unwrap(), ticket);
}

#[test]
fn fallback_base_survives_permission_changes_and_disappearance_of_the_sidecar() {
    let side_bytes = capture(&app()).to_bytes().unwrap();
    let base = persistence::hash(&side_bytes);
    let mut fallback = capture(&app());
    fallback.trace.path = TRACE.into();
    fallback.supersedes = Some(base.clone());
    let fallback = Candidate {
        target: Target::Storage { key: TRACE.into() },
        content: Content::Bytes(fallback.to_bytes().unwrap()),
        writable: true,
    };
    let mut side = Candidate {
        target: file(LOCATION),
        content: Content::Bytes(side_bytes),
        writable: false,
    };
    for writable in [false, true] {
        side.writable = writable;
        let chosen = persistence::select(side.clone(), fallback.clone(), false).unwrap();
        assert_eq!(chosen.target, fallback.target);
        assert_eq!(chosen.supersedes, Some(base.clone()));
    }
    side.content = Content::Missing;
    let chosen = persistence::select(side.clone(), fallback.clone(), false).unwrap();
    assert_eq!(chosen.target, fallback.target);
    assert_eq!(chosen.supersedes, Some(base));
    let mut changed = capture(&app());
    changed.sidebar.filter = "changed".into();
    side.content = Content::Bytes(changed.to_bytes().unwrap());
    let chosen = persistence::select(side.clone(), fallback.clone(), false).unwrap();
    assert_eq!(chosen.target, side.target);
    assert_eq!(chosen.notices.len(), 1);
    side.content = Content::Error("permission denied".into());
    assert!(persistence::select(side, fallback, false).is_err());
}

fn persistent_app() -> App {
    let mut app = App::new();
    app.configure_persistence(Persistence::Auto);
    app.open_resource(volna_core::session::OpenSpec::Synthetic(100), TRACE.into());
    for request in app.take_requests() {
        app.deliver(request.perform());
    }
    app.restore_candidates(
        TRACE,
        Candidate {
            target: file(LOCATION),
            content: Content::Missing,
            writable: true,
        },
        Candidate {
            target: Target::Storage { key: TRACE.into() },
            content: Content::Missing,
            writable: true,
        },
    );
    app.take_events();
    app
}
fn emitted_save(app: &mut App) -> (SaveTicket, Vec<u8>) {
    app.take_events()
        .into_iter()
        .find_map(|event| match event {
            volna_core::Event::PersistWorkspace { ticket, bytes } => Some((ticket, bytes)),
            _ => None,
        })
        .expect("workspace write")
}

#[test]
fn app_autosave_tracks_edits_but_not_hover_noops_or_animation_frames() {
    use volna_core::wave::model::PointerEvent;
    let mut app = persistent_app();
    let now = Instant::now();
    app.handle_at(Command::AddVars(vec![0, 1]), now);
    let revision = app.workspace.scheduler.revision();
    app.handle_at(Command::MenuDismiss(app.panels.focused_id()), now);
    app.handle_at(
        Command::Pointer(
            app.panels.focused_id(),
            PointerEvent::Move {
                position: volna_core::geometry::point(30.0, 100.0),
            },
        ),
        now,
    );
    app.handle_at(Command::SetFilter(String::new()), now);
    assert_eq!(app.workspace.scheduler.revision(), revision);
    app.handle_at(Command::Action(Action::ZoomIn), now);
    let revision = app.workspace.scheduler.revision();
    app.tick(now + IDLE / 10);
    assert_eq!(app.workspace.scheduler.revision(), revision);
    app.tick(now + IDLE * 2);
    let (ticket, bytes) = emitted_save(&mut app);
    assert_eq!(ticket.revision, revision);
    Workspace::parse(&bytes).unwrap();
    app.workspace_saved(ticket, None, now + IDLE * 2);
    assert!(!app.workspace.scheduler.dirty());
}

#[test]
fn automatic_corruption_suspends_saves_but_explicit_failures_leave_policy_and_state_intact() {
    let mut app = App::new();
    app.configure_persistence(Persistence::Auto);
    app.open_resource(volna_core::session::OpenSpec::Synthetic(100), TRACE.into());
    for request in app.take_requests() {
        app.deliver(request.perform());
    }
    let mut future = value(&app);
    future["version"] = json!(99);
    app.restore_candidates(
        TRACE,
        Candidate {
            target: file(LOCATION),
            content: Content::Bytes(serde_json::to_vec(&future).unwrap()),
            writable: true,
        },
        Candidate {
            target: Target::Storage { key: TRACE.into() },
            content: Content::Missing,
            writable: true,
        },
    );
    assert!(app.workspace.scheduler.suspended());
    app.handle(Command::AddVars(vec![0]));
    app.tick(Instant::now() + IDLE * 2);
    assert!(
        !app.take_events()
            .iter()
            .any(|e| matches!(e, volna_core::Event::PersistWorkspace { .. }))
    );
    let before = app.debug_state();
    let target = app.workspace.scheduler.target().cloned();
    assert!(app.open_workspace(file(LOCATION), b"invalid").is_err());
    assert_eq!(app.debug_state(), before);
    assert_eq!(app.workspace.scheduler.target(), target.as_ref());
    app.save_workspace(None);
    let (ticket, _) = emitted_save(&mut app);
    app.workspace_saved(ticket, None, Instant::now());
    assert!(!app.workspace.scheduler.suspended());
}

#[test]
fn opening_closing_and_explicit_restore_wait_for_successful_flush() {
    let mut app = persistent_app();
    app.handle(Command::AddVars(vec![0, 1]));
    let saved = capture(&app).to_bytes().unwrap();
    app.handle(Command::Action(Action::SplitRight));
    app.open_workspace(file(LOCATION), &saved).unwrap();
    assert_eq!(app.panels.len(), 2);
    let (flush, _) = emitted_save(&mut app);
    app.workspace_saved(flush, Some("disk full".into()), Instant::now());
    assert_eq!(app.panels.len(), 2);
    app.open_workspace(file(LOCATION), &saved).unwrap();
    let (flush, _) = emitted_save(&mut app);
    app.workspace_saved(flush, None, Instant::now());
    assert_eq!(app.panels.len(), 1);
    app.handle(Command::Action(Action::SplitRight));
    app.close_trace();
    assert!(app.doc.is_loaded());
    let (flush, _) = emitted_save(&mut app);
    app.workspace_saved(flush, None, Instant::now());
    assert!(!app.doc.is_loaded());
}

#[test]
fn opaque_panel_rename_preserves_unknown_members_and_large_numbers() {
    let mut app = app();
    let mut saved = capture(&app);
    saved.panels.push(serde_json::from_str(r#"{"id":90,"kind":"future","version":9,"title":null,"number":18446744073709551614,"uninterpreted":1e999}"#).unwrap());
    saved.layout = volna_core::panels::Layout::Tabs {
        tabs: vec![app.panels.focused_id(), PanelId(90)],
        active: PanelId(90),
    };
    saved.focused = PanelId(90);
    saved
        .prepare(&app, TRACE, LOCATION)
        .unwrap()
        .commit(&mut app)
        .unwrap();
    app.handle(Command::Panels(PanelsCommand::Rename(
        PanelId(90),
        Some("renamed".into()),
    )));
    let captured = capture(&app);
    let raw = captured.panels[1].get();
    assert!(raw.contains("18446744073709551614"));
    assert!(raw.contains("1e999"));
    assert!(raw.contains("renamed"));
}

#[test]
fn a_lone_start_panel_round_trips_and_gives_way_to_a_saved_waveform_panel() {
    let mut app = App::new();
    app.set_session(Arc::new(SynthSource::new(100)));
    assert!(app.panels.focused().kind.is_start());
    let before = value(&app);
    assert_eq!(before["panels"][0]["kind"], "start");
    let plan = prepare(&before, &app).unwrap();
    assert!(plan.report().notices.is_empty());
    plan.commit(&mut app).unwrap();
    assert!(app.panels.focused().kind.is_start());
    assert_eq!(value(&app), before);
    app.handle(Command::AddVars(vec![0]));
    let after = value(&app);
    assert_eq!(after["panels"].as_array().unwrap().len(), 1);
    assert_eq!(after["panels"][0]["kind"], "waves");
}

#[test]
fn links_on_an_unfocused_panel_are_persistent_edits() {
    let mut app = persistent_app();
    app.handle(Command::Action(Action::NewPanel));
    let first = app.panels.focused_id();
    app.handle(Command::Action(Action::SplitRight));
    let focused = app.panels.focused_id();
    let revision = app.workspace.scheduler.revision();
    app.handle(Command::Panels(PanelsCommand::ToggleLink {
        panel: first,
        dim: volna_core::wave::model::LinkDim::Viewport,
    }));
    assert_eq!(app.panels.focused_id(), focused);
    assert_eq!(app.workspace.scheduler.revision(), revision + 1);
    assert!(!app.panels.waves(first).unwrap().nav.link.viewport);
}

#[test]
fn stale_panel_input_cannot_target_reused_saved_ids_and_new_ids_keep_increasing() {
    let mut app = app();
    let saved = capture(&app).to_bytes().unwrap();
    let generation = app.doc.generation();
    app.handle(Command::Action(Action::NewPanel));
    let highest = app.panels.focused_id();
    Workspace::parse(&saved)
        .unwrap()
        .prepare(&app, TRACE, LOCATION)
        .unwrap()
        .commit(&mut app)
        .unwrap();
    let before = value(&app);
    assert!(!app.handle_if_current(generation, Command::Action(Action::NewPanel)));
    assert_eq!(value(&app), before);
    assert!(app.handle_if_current(app.doc.generation(), Command::Action(Action::NewPanel)));
    assert!(app.panels.focused_id() > highest);
}

fn row_heights(app: &App) -> Vec<u8> {
    app.panels
        .focused_waves()
        .unwrap()
        .items
        .iter()
        .map(|item| item.height.multiple())
        .collect()
}

#[test]
fn row_heights_round_trip_and_restore_tall_row_geometry() {
    use volna_core::Theme;
    use volna_core::geometry::Rect;
    let mut app = app();
    let theme = Theme::one_dark();
    let w = app.panels.focused_waves_mut().unwrap();
    w.selected = [1].into();
    w.anchor = Some(1);
    app.handle(Command::Action(Action::IncreaseRowHeight));
    app.handle(Command::Action(Action::IncreaseRowHeight));
    let w = app.panels.focused_waves_mut().unwrap();
    w.selected = [2].into();
    w.anchor = Some(2);
    for _ in 0..4 {
        app.handle(Command::Action(Action::IncreaseRowHeight));
    }
    assert_eq!(row_heights(&app), [1, 3, 8]);
    let saved = value(&app);
    // The default height is implied; taller rows store their multiple.
    let rows = &saved["panels"][0]["rows"];
    assert!(rows[0].get("height").is_none());
    assert_eq!(
        (&rows[1]["height"], &rows[2]["height"]),
        (&json!(3), &json!(8))
    );

    // Restore into a fresh session: heights, geometry and the file agree.
    let mut restored = App::new();
    restored.set_session(Arc::new(SynthSource::new(100)));
    prepare(&saved, &restored)
        .unwrap()
        .commit(&mut restored)
        .unwrap();
    assert_eq!(row_heights(&restored), [1, 3, 8]);
    assert_eq!(value(&restored), saved);
    let id = restored.panels.focused_id();
    restored.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 800.0), &theme);
    let layout = restored.panels.focused_waves().unwrap().last_layout();
    let top = layout.names.top();
    assert_eq!(
        (0..3)
            .map(|row| (layout.row_y(row) - top) / layout.row_h)
            .collect::<Vec<_>>(),
        [0.0, 1.0, 4.0]
    );
    assert_eq!(layout.row_height(2), 8.0 * layout.row_h);
}

#[test]
fn unsupported_row_heights_reject_the_whole_restore() {
    let app = app();
    for height in [json!(0), json!(5), json!(9), json!("2"), json!(2.5)] {
        let mut saved = value(&app);
        saved["panels"][0]["rows"][0]["height"] = height.clone();
        assert!(prepare(&saved, &app).is_err(), "{height}");
    }
}

#[test]
fn row_height_changes_schedule_an_autosave() {
    use volna_core::wave::{RowHeight, model::MenuAction};
    let mut app = persistent_app();
    let now = Instant::now();
    app.handle_at(Command::AddVars(vec![0, 1]), now);
    for command in [
        Command::Action(Action::IncreaseRowHeight),
        Command::Action(Action::DecreaseRowHeight),
        Command::Action(Action::IncreaseRowHeight),
        Command::Action(Action::ResetRowHeight),
    ] {
        let revision = app.workspace.scheduler.revision();
        app.handle_at(command.clone(), now);
        assert!(app.workspace.scheduler.revision() > revision, "{command:?}");
    }
    let panel = app.panels.focused_id();
    app.handle_at(Command::OpenSignalMenu(panel), now);
    let revision = app.workspace.scheduler.revision();
    app.handle_at(
        Command::MenuSelect(panel, MenuAction::RowHeight(RowHeight::PRESETS[3])),
        now,
    );
    assert!(app.workspace.scheduler.revision() > revision);
    app.tick(now + IDLE * 2);
    let (_, bytes) = emitted_save(&mut app);
    let saved: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved["panels"][0]["rows"][1]["height"], json!(4));
}

fn restoring_app(policy: Persistence) -> App {
    let mut app = App::new();
    app.configure_persistence(policy);
    app.open_resource(volna_core::session::OpenSpec::Synthetic(100), TRACE.into());
    for request in app.take_requests() {
        app.deliver(request.perform());
    }
    app
}

/// During the research phase an older workspace is discarded, not migrated:
/// restore starts fresh, keeps autosave running and overwrites the file.
#[test]
fn older_workspace_versions_are_discarded_and_overwritten() {
    let mut old = value(&app());
    old["version"] = json!(volna_core::workspace::VERSION - 1);
    let old = serde_json::to_vec(&old).unwrap();
    assert!(Workspace::is_outdated(&old));
    assert!(Workspace::parse(&old).is_err());
    let missing = || Candidate {
        target: Target::Storage { key: TRACE.into() },
        content: Content::Missing,
        writable: true,
    };
    for (policy, sidecar, fallback) in [
        (
            Persistence::Auto,
            Content::Bytes(old.clone()),
            Content::Missing,
        ),
        (
            Persistence::Auto,
            Content::Missing,
            Content::Bytes(old.clone()),
        ),
        (
            Persistence::Explicit(file(LOCATION)),
            Content::Bytes(old.clone()),
            Content::Missing,
        ),
    ] {
        let mut app = restoring_app(policy.clone());
        app.restore_candidates(
            TRACE,
            Candidate {
                target: file(LOCATION),
                content: sidecar,
                writable: true,
            },
            Candidate {
                content: fallback,
                ..missing()
            },
        );
        assert!(!app.workspace.scheduler.suspended(), "{policy:?}");
        assert!(
            app.workspace
                .notices
                .iter()
                .any(|n| n.contains("older Volna")),
            "{policy:?}: {:?}",
            app.workspace.notices
        );
        assert_eq!(app.workspace.scheduler.target(), Some(&file(LOCATION)));
        app.handle(Command::AddVars(vec![0]));
        app.tick(Instant::now() + IDLE * 2);
        let (ticket, bytes) = emitted_save(&mut app);
        assert_eq!(ticket.target, file(LOCATION));
        assert_eq!(
            Workspace::parse(&bytes).unwrap().version,
            volna_core::workspace::VERSION
        );
    }
    // A newer or foreign file is not "older".
    let mut newer = value(&app());
    newer["version"] = json!(volna_core::workspace::VERSION + 1);
    assert!(!Workspace::is_outdated(
        &serde_json::to_vec(&newer).unwrap()
    ));
    newer["version"] = json!(1);
    newer["format"] = json!("other");
    assert!(!Workspace::is_outdated(
        &serde_json::to_vec(&newer).unwrap()
    ));
}
