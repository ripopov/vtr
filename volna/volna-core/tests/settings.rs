//! Settings over the core loop: the editor tab, GUI edits and the write
//! queue, host deliveries, and the projection into the viewer. Headless.

use std::time::Duration;

use volna_core::Instant;
use volna_core::app::{App, Command, Event, SettingsCommand};
use volna_core::panels::PanelsCommand;
use volna_core::session::OpenSpec;
use volna_core::settings::{Animation, Host, Value, WRITE_IDLE, ZoomStep};
use volna_core::workspace::persistence::{Candidate, Content, Persistence, Target};

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for r in requests {
            app.deliver(r.perform());
        }
    }
}

fn app() -> App {
    let mut app = App::new();
    app.configure_settings(Host::Native, Some("./settings.schema.json".into()));
    app
}

fn events(app: &mut App) -> Vec<Event> {
    app.take_events()
}

fn changed_keys(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .flat_map(|e| match e {
            Event::SettingsChanged { keys } => keys.clone(),
            _ => vec![],
        })
        .collect()
}

fn write(events: &[Event]) -> Option<(u64, String)> {
    events.iter().find_map(|e| match e {
        Event::WriteSettings { ticket, bytes } => {
            Some((*ticket, String::from_utf8(bytes.clone()).unwrap()))
        }
        _ => None,
    })
}

#[test]
fn gui_edits_change_the_document_surgically_and_write_after_idle() {
    let mut app = app();
    app.settings_loaded("{\n  // mine\n  \"waves.animation\": \"reduced\",\n}\n");
    assert_eq!(changed_keys(&events(&mut app)), vec!["waves.animation"]);
    assert_eq!(app.doc.navigation.animation, Animation::Reduced);
    let t0 = Instant::now();
    app.handle_at(
        Command::Settings(SettingsCommand::Set {
            id: "waves.snapPixels".into(),
            value: Value::Integer(0),
        }),
        t0,
    );
    let ev = events(&mut app);
    assert_eq!(changed_keys(&ev), vec!["waves.snapPixels"]);
    assert!(write(&ev).is_none(), "writes wait for the idle time");
    assert_eq!(app.doc.navigation.snap_px, 0.0);
    assert_eq!(
        app.settings.text(),
        "{\n  // mine\n  \"waves.animation\": \"reduced\",\n  \"waves.snapPixels\": 0\n}\n"
    );
    assert!(app.tick(t0 + Duration::from_millis(10)));
    assert!(write(&events(&mut app)).is_none());
    app.tick(t0 + WRITE_IDLE);
    let (ticket, text) = write(&events(&mut app)).expect("one write");
    assert_eq!(text, app.settings.text());
    assert!(app.tick(t0 + WRITE_IDLE), "outstanding until acknowledged");
    app.settings_saved(ticket, None, t0 + WRITE_IDLE);
    assert!(!app.settings.pending_write());
    // The watcher echo of that write changes nothing; an external edit does.
    app.settings_external(text.as_bytes());
    assert!(changed_keys(&events(&mut app)).is_empty());
    app.settings_external(b"{\"waves.animation\": \"off\", \"waves.snapPixels\": 0}");
    assert_eq!(changed_keys(&events(&mut app)), vec!["waves.animation"]);
    assert_eq!(app.doc.navigation.animation, Animation::Off);
    // Reset removes the property; unknown ids and bad values are notices.
    app.handle_at(
        Command::Settings(SettingsCommand::Reset {
            id: "waves.animation".into(),
        }),
        t0,
    );
    assert_eq!(changed_keys(&events(&mut app)), vec!["waves.animation"]);
    assert_eq!(app.settings.text(), "{\"waves.snapPixels\": 0}");
    app.handle_at(
        Command::Settings(SettingsCommand::Set {
            id: "waves.snapPixels".into(),
            value: Value::Integer(99),
        }),
        t0,
    );
    let ev = events(&mut app);
    assert!(changed_keys(&ev).is_empty());
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Notice(text) if text.contains("0–24")))
    );
}

#[test]
fn syntax_errors_keep_values_and_block_gui_edits_until_fixed() {
    let mut app = app();
    app.settings_loaded("{\"panels.linkByDefault\": false}");
    events(&mut app);
    let t0 = Instant::now();
    app.handle_at(
        Command::Settings(SettingsCommand::ReplaceText(
            "{\"panels.linkByDefault\": fal".into(),
        )),
        t0,
    );
    let ev = events(&mut app);
    assert!(changed_keys(&ev).is_empty());
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Notice(t) if t.contains("syntax error on line 1")))
    );
    assert!(!app.settings.resolved().panels.link_by_default);
    assert!(!app.settings.editable());
    app.handle_at(
        Command::Settings(SettingsCommand::Set {
            id: "panels.linkByDefault".into(),
            value: Value::Bool(true),
        }),
        t0,
    );
    assert!(
        events(&mut app)
            .iter()
            .any(|e| matches!(e, Event::Notice(t) if t.contains("syntax error")))
    );
    app.handle_at(
        Command::Settings(SettingsCommand::ReplaceText(
            "{\"panels.linkByDefault\": true}".into(),
        )),
        t0,
    );
    assert_eq!(
        changed_keys(&events(&mut app)),
        vec!["panels.linkByDefault"]
    );
    assert!(app.settings.editable());
    // New panels take the live default.
    app.open_synthetic(10);
    pump(&mut app);
    assert!(app.panels.focused_waves().unwrap().link.viewport);
}

#[test]
fn host_overrides_win_and_are_never_written() {
    let mut app = App::new();
    app.configure_settings(Host::Vscode, None);
    app.settings_loaded("{\"waves.snapPixels\": 3}");
    events(&mut app);
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert("waves.snapPixels".to_owned(), Value::Integer(9));
    overrides.insert("remote.memoryMiB".to_owned(), Value::Integer(64));
    overrides.insert(
        "workspace.autosave".to_owned(),
        Value::Text("vscode".into()),
    );
    app.set_host_settings(overrides);
    let keys = changed_keys(&events(&mut app));
    assert_eq!(
        keys,
        vec!["remote.memoryMiB", "waves.snapPixels", "workspace.autosave"]
    );
    assert_eq!(app.settings.resolved().remote_limits().memory_mib, 64);
    assert_eq!(app.doc.navigation.snap_px, 9.0);
    assert_eq!(app.settings.text(), "{\"waves.snapPixels\": 3}");
    assert!(!app.settings.pending_write());
}

#[test]
fn the_settings_tab_is_chrome_that_survives_traces_and_never_reaches_a_workspace() {
    let mut app = app();
    app.configure_persistence(Persistence::Auto);
    let t0 = Instant::now();
    app.handle_at(Command::Settings(SettingsCommand::Open), t0);
    let ev = events(&mut app);
    assert!(ev.contains(&Event::FocusSettingsSearch));
    assert!(ev.iter().any(|e| matches!(e, Event::LayoutChanged { .. })));
    let settings = app.panels.settings_id().expect("tab open");
    assert_eq!(app.panels.focused_id(), settings);
    assert_eq!(app.panels.len(), 2);
    // Opening again focuses; it is not a second tab.
    app.handle_at(Command::Settings(SettingsCommand::Open), t0);
    assert_eq!(app.panels.len(), 2);
    // Reveal from the palette filters the editor to one id.
    app.handle_at(
        Command::Settings(SettingsCommand::Reveal {
            id: "waves.snapPixels".into(),
        }),
        t0,
    );
    assert_eq!(app.settings_view.query, "@id:waves.snapPixels");
    assert!(!app.settings_view.json);
    app.handle_at(Command::Settings(SettingsCommand::ToggleJson), t0);
    assert!(app.settings_view.json);
    // A trace opens and closes around the tab; it keeps its id and focus.
    app.open_resource(OpenSpec::Synthetic(50), "file:///t.vtr".into());
    pump(&mut app);
    app.restore_candidates(
        "file:///t.vtr",
        Candidate {
            target: Target::File {
                uri: "file:///t.vtr.volna.json".into(),
            },
            content: Content::Missing,
            writable: true,
        },
        Candidate {
            target: Target::Storage { key: "k".into() },
            content: Content::Missing,
            writable: true,
        },
    );
    events(&mut app);
    assert_eq!(app.panels.settings_id(), Some(settings));
    assert_eq!(app.panels.focused_id(), settings);
    assert_eq!(app.panels.len(), 2);
    let waves = app
        .panels
        .layout()
        .panels()
        .into_iter()
        .find(|id| *id != settings)
        .unwrap();
    // The saved workspace never mentions the tab.
    app.handle_at(Command::Panels(PanelsCommand::Focus(waves)), t0);
    app.handle_at(Command::Action(volna_core::app::Action::AddMarker), t0);
    app.handle_at(Command::SaveWorkspace, t0);
    let saved = events(&mut app)
        .into_iter()
        .find_map(|e| match e {
            Event::PersistWorkspace { bytes, .. } => Some(String::from_utf8(bytes).unwrap()),
            _ => None,
        })
        .expect("workspace write");
    let json: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(json["panels"].as_array().unwrap().len(), 1);
    assert_eq!(json["layout"]["tabs"].as_array().unwrap().len(), 1);
    assert!(!saved.contains("Settings"));
    // Closing the trace keeps the tab; closing the tab removes it.
    app.handle_at(Command::CloseTrace, t0);
    events(&mut app);
    assert_eq!(app.panels.settings_id(), Some(settings));
    app.handle_at(Command::Settings(SettingsCommand::Close), t0);
    assert_eq!(app.panels.settings_id(), None);
    assert_eq!(app.panels.len(), 1);
    assert!(app.debug_state().contains("panel="));
}

#[test]
fn theme_names_validate_the_theme_key_and_unavailable_keys_are_diagnosed() {
    let mut app = app();
    app.settings_loaded("{\"appearance.theme\": \"paper\", \"remote.memoryMiB\": 4}");
    events(&mut app);
    assert_eq!(app.settings.resolved().appearance.theme, "one-dark");
    assert_eq!(app.settings.diagnostics().len(), 2);
    app.set_theme_names(vec!["paper".into()]);
    assert_eq!(changed_keys(&events(&mut app)), vec!["appearance.theme"]);
    assert_eq!(app.settings.resolved().appearance.theme, "paper");
    assert_eq!(app.settings.diagnostics().len(), 1);
    assert!(
        app.settings.diagnostics()[0]
            .message
            .contains("not used on this host")
    );
}

#[test]
fn zoom_steps_write_the_setting_and_reset_removes_it() {
    let mut app = app();
    app.settings_loaded("{\n  \"waves.snapPixels\": 4,\n}\n");
    events(&mut app);
    assert_eq!(app.settings.resolved().appearance.zoom, 1.0);
    let t0 = Instant::now();
    app.handle_at(Command::Settings(SettingsCommand::Zoom(ZoomStep::In)), t0);
    let ev = events(&mut app);
    assert_eq!(changed_keys(&ev), vec!["appearance.zoom"]);
    assert_eq!(app.settings.resolved().appearance.zoom, 1.1);
    assert!(app.settings.text().contains("\"appearance.zoom\": 1.1"));
    assert!(app.settings.is_modified("appearance.zoom"));
    for _ in 0..30 {
        app.handle_at(Command::Settings(SettingsCommand::Zoom(ZoomStep::In)), t0);
    }
    assert_eq!(app.settings.resolved().appearance.zoom, 3.0);
    // A step at the limit is a no-op, not an error notice.
    app.handle_at(Command::Settings(SettingsCommand::Zoom(ZoomStep::In)), t0);
    assert!(
        !events(&mut app)
            .iter()
            .any(|e| matches!(e, Event::Notice(_)))
    );
    app.handle_at(Command::Settings(SettingsCommand::Zoom(ZoomStep::Out)), t0);
    assert_eq!(app.settings.resolved().appearance.zoom, 2.9);
    events(&mut app);
    app.handle_at(
        Command::Settings(SettingsCommand::Zoom(ZoomStep::Reset)),
        t0,
    );
    let ev = events(&mut app);
    assert_eq!(changed_keys(&ev), vec!["appearance.zoom"]);
    assert_eq!(app.settings.resolved().appearance.zoom, 1.0);
    assert!(!app.settings.text().contains("appearance.zoom"));
    assert!(app.settings.text().contains("\"waves.snapPixels\": 4"));
    // The change reaches the file after the idle interval.
    app.handle_at(Command::Settings(SettingsCommand::Zoom(ZoomStep::Out)), t0);
    events(&mut app);
    app.tick(t0 + WRITE_IDLE + Duration::from_millis(1));
    let (_, text) = write(&events(&mut app)).expect("a write");
    assert!(text.contains("\"appearance.zoom\": 0.9"));
    // An out-of-range file value falls back to the default with a diagnostic.
    app.settings_external(b"{ \"appearance.zoom\": 9 }");
    events(&mut app);
    assert_eq!(app.settings.resolved().appearance.zoom, 1.0);
    assert!(!app.settings.diagnostics().is_empty());
}
