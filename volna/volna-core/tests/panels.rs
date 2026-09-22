use std::collections::BTreeSet;
use std::sync::Arc;

use volna_core::Document;
use volna_core::Session;
use volna_core::app::SettingsCommand;
use volna_core::data::source::Lookup;
use volna_core::data::synth::SynthSource;
use volna_core::panels::PanelsCommand;
use volna_core::panels::{Axis, Layout, PanelId, PanelKind, Panels};
use volna_core::wave::model::{LinkDim, PointerEvent};
use volna_core::{Action, App, Command, Instant};

fn linked_app() -> App {
    let mut app = App::new();
    app.set_session(Arc::new(SynthSource::new(100)));
    app.handle(Command::AddVars(vec![0, 1]));
    app
}

fn panel(app: &mut App, command: PanelsCommand) {
    app.handle(Command::Panels(command));
}

/// A collection whose start panel already gave way to a waveform panel.
fn wave_panels() -> Panels {
    let mut p = Panels::new();
    let start = p.focused_id();
    p.replace(start, PanelKind::Waves(Box::default())).unwrap();
    p
}

#[test]
fn focused_actions_update_one_shared_value_and_unlink_snapshots_the_displayed_frame() {
    let mut app = linked_app();
    let a = app.panels.focused_id();
    panel(
        &mut app,
        PanelsCommand::Split {
            panel: a,
            axis: Axis::Horizontal,
        },
    );
    let b = app.panels.focused_id();
    let now = Instant::now();
    app.handle_at(Command::Action(Action::ZoomIn), now);
    assert!(app.tick(now + std::time::Duration::from_millis(50)));
    let shared = app.doc.shared.viewport.value;
    assert_eq!(app.panels.waves(a).unwrap().viewport(&app.doc), shared);
    assert_eq!(app.panels.waves(b).unwrap().viewport(&app.doc), shared);
    panel(
        &mut app,
        PanelsCommand::ToggleLink {
            panel: b,
            dim: LinkDim::Viewport,
        },
    );
    app.tick(now + std::time::Duration::from_secs(1));
    assert_eq!(app.panels.waves(b).unwrap().viewport(&app.doc), shared);
    let retained = app.doc.shared.viewport.value;
    assert_ne!(retained, shared);
    app.handle_at(Command::Action(Action::PanRight), now);
    app.tick(now + std::time::Duration::from_secs(1));
    assert_eq!(app.panels.waves(a).unwrap().viewport(&app.doc), retained);
    assert_ne!(app.panels.waves(b).unwrap().viewport(&app.doc), shared);
    // Even with no followers, the last shared position remains authoritative.
    panel(
        &mut app,
        PanelsCommand::ToggleLink {
            panel: a,
            dim: LinkDim::Viewport,
        },
    );
    app.handle_at(Command::Action(Action::ZoomFit), now);
    app.tick(now + std::time::Duration::from_secs(1));
    panel(
        &mut app,
        PanelsCommand::ToggleLink {
            panel: b,
            dim: LinkDim::Viewport,
        },
    );
    assert_eq!(app.panels.waves(b).unwrap().viewport(&app.doc), retained);
    assert!(!app.panels.waves(b).unwrap().is_animating());
}

#[test]
fn cursor_links_are_independent_of_view_links_and_markers_use_the_focused_cursor() {
    let mut app = linked_app();
    let a = app.panels.focused_id();
    app.doc.shared.cursor = Some(1_u64 << 54);
    panel(
        &mut app,
        PanelsCommand::Split {
            panel: a,
            axis: Axis::Vertical,
        },
    );
    let b = app.panels.focused_id();
    panel(
        &mut app,
        PanelsCommand::ToggleLink {
            panel: b,
            dim: LinkDim::Cursor,
        },
    );
    assert_eq!(app.panels.waves(b).unwrap().cursor(&app.doc), Some(1 << 54));
    app.panels
        .waves_mut(b)
        .unwrap()
        .set_cursor(&mut app.doc, Some((1 << 54) + 1));
    app.handle(Command::Action(Action::AddMarker));
    assert_eq!(app.doc.markers[0].time, (1 << 54) + 1);
    assert_eq!(app.panels.waves(a).unwrap().cursor(&app.doc), Some(1 << 54));
    panel(
        &mut app,
        PanelsCommand::ToggleLink {
            panel: b,
            dim: LinkDim::Cursor,
        },
    );
    assert_eq!(app.panels.waves(b).unwrap().cursor(&app.doc), Some(1 << 54));
    app.handle(Command::Action(Action::AddMarker));
    let ids: BTreeSet<_> = app.doc.markers.iter().map(|m| m.id).collect();
    assert_eq!(ids.len(), 2);
    app.handle(Command::Action(Action::ClearMarkers));
    app.handle(Command::Action(Action::AddMarker));
    assert!(!ids.contains(&app.doc.markers[0].id));
}

#[test]
fn pending_and_loaded_histories_are_shared_across_all_panels() {
    let mut app = linked_app();
    let a = app.panels.focused_id();
    let requests = app.take_requests();
    assert_eq!(requests.len(), 1);
    panel(&mut app, PanelsCommand::NewTab { group_of: a });
    let b = app.panels.focused_id();
    app.handle(Command::AddVars(vec![0]));
    assert!(app.take_requests().is_empty());
    for r in requests {
        app.deliver(r.perform());
    }
    let first = app.panels.waves(a).unwrap().items[0]
        .signal()
        .unwrap()
        .history
        .clone()
        .unwrap();
    let second = app.panels.waves(b).unwrap().items[0]
        .signal()
        .unwrap()
        .history
        .clone()
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    panel(&mut app, PanelsCommand::NewTab { group_of: b });
    app.handle(Command::AddVars(vec![0]));
    assert!(
        app.take_requests().is_empty(),
        "adding an already loaded signal must not decode again"
    );
    assert!(Arc::ptr_eq(
        &first,
        app.panels.focused_waves().unwrap().items[0]
            .signal()
            .unwrap()
            .history
            .as_ref()
            .unwrap()
    ));
    let weak = Arc::downgrade(&first);
    drop(first);
    drop(second);
    for id in app.panels.layout().panels() {
        panel(&mut app, PanelsCommand::Close(id));
    }
    assert_eq!(
        weak.strong_count(),
        1,
        "only the synthetic source retains its history"
    );
    app.close_trace();
    assert!(weak.upgrade().is_none());
}

#[test]
fn pointer_targets_do_not_drift_when_focus_changes_or_a_trace_is_replaced() {
    let mut app = linked_app();
    let theme = volna_core::Theme::one_dark();
    let a = app.panels.focused_id();
    panel(
        &mut app,
        PanelsCommand::Split {
            panel: a,
            axis: Axis::Horizontal,
        },
    );
    let b = app.panels.focused_id();
    panel(
        &mut app,
        PanelsCommand::ToggleLink {
            panel: a,
            dim: LinkDim::Cursor,
        },
    );
    let bounds = volna_core::geometry::Rect::from_xywh(0.0, 0.0, 1200.0, 600.0);
    app.layout_panel(a, bounds, &theme).unwrap();
    let layout = app.panels.waves(a).unwrap().last_layout().clone();
    let click = PointerEvent::Down {
        position: volna_core::geometry::point(layout.waves.left() + 100.0, layout.row_y(0) + 10.0),
        button: volna_core::geometry::MouseButton::Left,
        modifiers: Default::default(),
    };
    app.handle(Command::Pointer(a, click));
    assert_eq!(app.panels.focused_id(), a);
    assert!(app.panels.waves(a).unwrap().cursor(&app.doc).is_some());
    assert_eq!(app.panels.waves(b).unwrap().cursor(&app.doc), None);
    app.close_trace();
    let before = app.debug_state();
    app.handle(Command::Pointer(a, click));
    assert_eq!(app.debug_state(), before);
}

#[test]
fn hierarchy_locators_round_trip_declarations_and_literal_dots() {
    let source = SynthSource::new(10);
    let mut h = source.hierarchy().clone();
    h.scopes[0].name = "test.bench".into();
    h.vars[0].name = "\\escaped.name".into();
    for id in 0..h.vars.len() {
        let (path, nth) = h.var_path(id);
        assert_eq!(h.find_var(&path, nth), Lookup::Found(id));
    }
    for id in 0..h.scopes.len() {
        assert_eq!(h.find_scope(&h.scope_path(id)), Lookup::Found(id));
    }
    assert_eq!(h.find_scope(&["test", "bench"]), Lookup::Missing);
    assert_eq!(h.find_var(&["absent"], None), Lookup::Missing);
    assert_eq!(h.find_scope(&[] as &[String]), Lookup::Missing);
}

#[test]
fn duplicate_variables_require_occurrence_but_duplicate_scopes_cannot_be_guessed() {
    let source = SynthSource::new(10);
    let mut h = source.hierarchy().clone();
    let duplicate = h.vars.len();
    h.vars.push(h.vars[0].clone());
    let scope = h.vars[0].scope;
    h.scopes[scope].vars.push(duplicate);
    let (path, nth) = h.var_path(duplicate);
    assert_eq!(nth, Some(1));
    assert_eq!(h.find_var(&path, None), Lookup::Ambiguous);
    assert_eq!(h.find_var(&path, Some(0)), Lookup::Found(0));
    assert_eq!(h.find_var(&path, Some(1)), Lookup::Found(duplicate));
    assert_eq!(h.find_var(&path, Some(2)), Lookup::Missing);
    let root = h.roots[0];
    h.roots.push(h.scopes.len());
    h.scopes.push(h.scopes[root].clone());
    assert_eq!(h.find_var(&path, Some(0)), Lookup::Ambiguous);
    assert_eq!(h.find_scope(&h.scope_path(scope)), Lookup::Ambiguous);
}

#[test]
fn split_shares_survive_normalization_and_close() {
    let mut p = wave_panels();
    let a = p.focused_id();
    let b = p.create(a, Some(Axis::Horizontal)).unwrap();
    p.set_layout(
        Layout::Split {
            split: Axis::Horizontal,
            sizes: vec![0.8, 0.2],
            children: vec![Layout::single(a), Layout::single(b)],
        },
        p.revision(),
    )
    .unwrap();
    let c = p.create(a, Some(Axis::Horizontal)).unwrap();
    let Layout::Split {
        sizes, children, ..
    } = p.layout()
    else {
        panic!()
    };
    assert_eq!(sizes, &[0.4, 0.4, 0.2]);
    assert_eq!(children.len(), 3);
    assert_eq!(p.layout().panels(), vec![a, c, b]);
    p.close(c).unwrap();
    let Layout::Split { sizes, .. } = p.layout() else {
        panic!()
    };
    assert!((sizes[0] - 2.0 / 3.0).abs() < 1e-6);
    assert!((sizes[1] - 1.0 / 3.0).abs() < 1e-6);
    p.close(a).unwrap();
    assert_eq!(p.layout(), &Layout::single(b));
    p.close(b).unwrap();
    assert_eq!(p.len(), 1);
    p.validate().unwrap();
}

#[test]
fn closing_inactive_tabs_preserves_active_identity_and_focus_cycles_in_layout_order() {
    let mut p = wave_panels();
    let a = p.focused_id();
    let b = p.create(a, None).unwrap();
    let c = p.create(a, None).unwrap();
    let d = p.create(c, Some(Axis::Vertical)).unwrap();
    p.focus(c).unwrap();
    p.close(a).unwrap();
    assert_eq!(p.layout().visible(), vec![c, d]);
    assert_eq!(p.focused_id(), c);
    p.focus_next(false).unwrap();
    assert_eq!(p.focused_id(), d);
    p.focus_next(false).unwrap();
    assert_eq!(p.focused_id(), b);
    p.focus_next(true).unwrap();
    assert_eq!(p.focused_id(), d);
    p.validate().unwrap();
}

#[test]
fn invalid_and_stale_dock_proposals_are_atomic() {
    let mut p = wave_panels();
    let a = p.focused_id();
    let old = p.revision();
    let b = p.create(a, Some(Axis::Horizontal)).unwrap();
    let before = p.layout().clone();
    for tree in [
        Layout::single(a),
        Layout::Tabs {
            tabs: vec![a, a, b],
            active: a,
        },
        Layout::Tabs {
            tabs: vec![a, b],
            active: PanelId(999),
        },
        Layout::Split {
            split: Axis::Vertical,
            sizes: vec![f32::NAN, 1.0],
            children: vec![Layout::single(a), Layout::single(b)],
        },
        Layout::Split {
            split: Axis::Vertical,
            sizes: vec![1.0],
            children: vec![Layout::single(a), Layout::single(b)],
        },
        Layout::Split {
            split: Axis::Vertical,
            sizes: vec![0.0, 1.0],
            children: vec![Layout::single(a), Layout::single(b)],
        },
    ] {
        assert!(p.set_layout(tree, p.revision()).is_err());
        assert_eq!(p.layout(), &before);
        assert_eq!(p.focused_id(), b);
    }
    assert!(p.set_layout(before.clone(), old).is_err());
    assert!(!p.set_layout(before, p.revision()).unwrap());
}

#[test]
fn split_copies_rows_but_shares_history_and_clears_transient_input() {
    let mut p = wave_panels();
    let mut doc = Document::new();
    let source = Arc::new(SynthSource::new(10));
    doc.set_session(source.clone());
    let a = p.focused_id();
    let w = p.focused_mut().kind.waves_mut().unwrap();
    w.add_vars(&mut doc, &[0, 1], Default::default());
    w.finish_signal(
        source.hierarchy().vars[0].signal,
        source.load_signal(source.hierarchy().vars[0].signal),
    );
    w.drag = Some(volna_core::wave::model::Drag::Cursor);
    let b = p.create(a, Some(Axis::Vertical)).unwrap();
    let wa = p.get(a).unwrap().kind.waves().unwrap();
    let wb = p.get(b).unwrap().kind.waves().unwrap();
    assert_eq!(wa.items.len(), wb.items.len());
    assert!(Arc::ptr_eq(
        wa.items[0].signal().unwrap().history.as_ref().unwrap(),
        wb.items[0].signal().unwrap().history.as_ref().unwrap()
    ));
    assert_eq!(wb.drag, None);
    let c = p.create(b, None).unwrap();
    assert!(p.get(c).unwrap().kind.waves().unwrap().items.is_empty());
    p.get_mut(b)
        .unwrap()
        .kind
        .waves_mut()
        .unwrap()
        .items
        .clear();
    assert_eq!(p.get(a).unwrap().kind.waves().unwrap().items.len(), 2);
}

#[test]
fn deterministic_random_commands_preserve_invariants_and_never_reuse_ids() {
    let mut p = Panels::new();
    let mut seen = BTreeSet::from([p.focused_id()]);
    let mut rng = 0x913c_729a_835e_19a3_u64;
    for _ in 0..5000 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let ids = p.layout().panels();
        let id = ids[(rng as usize / 8) % ids.len()];
        match rng % 8 {
            0..=2 if p.len() < 40 => {
                let axis = match rng % 3 {
                    0 => None,
                    1 => Some(Axis::Horizontal),
                    _ => Some(Axis::Vertical),
                };
                if let Ok(new) = p.create(id, axis) {
                    assert!(seen.insert(new));
                }
            }
            3 => {
                p.close(id).unwrap();
            }
            4 => {
                p.focus(id).unwrap();
            }
            5 => {
                p.focus_next(true).unwrap();
            }
            6 => {
                p.focus_next(false).unwrap();
            }
            7 => {
                p.reset().unwrap();
                assert!(seen.insert(p.focused_id()));
            }
            _ => {}
        }
        p.validate().unwrap();
        let json = serde_json::to_string(p.layout()).unwrap();
        let tree: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(&tree, p.layout());
    }
}

#[test]
fn close_others_keeps_any_kind_and_a_lone_settings_tab_gets_a_start_panel() {
    let mut app = linked_app();
    app.handle(Command::Action(Action::SplitRight));
    app.handle(Command::Action(Action::SplitDown));
    let id = app.panels.focused_id();
    app.handle(Command::Panels(PanelsCommand::CloseOthers(id)));
    assert_eq!(app.panels.len(), 1);
    assert_eq!(app.panels.focused_id(), id);
    app.panels.validate().unwrap();
    // From the settings tab, close-others leaves settings plus a start panel.
    app.handle(Command::Settings(SettingsCommand::Open));
    let settings = app.panels.settings_id().unwrap();
    app.handle(Command::Panels(PanelsCommand::CloseOthers(settings)));
    assert_eq!(app.panels.len(), 2);
    assert_eq!(app.panels.focused_id(), settings);
    let start = app.panels.iter().find(|p| p.kind.is_start()).unwrap().id;
    // The placeholder is the last content: its tab offers no close.
    assert!(app.panels.can_close(settings));
    assert!(!app.panels.can_close(start));
    app.panels.validate().unwrap();
    // Rows added now go to a waveform panel in the placeholder's spot.
    app.handle(Command::AddVars(vec![0]));
    assert_eq!(app.panels.len(), 2);
    let waves = app.panels.focused_id();
    assert_eq!(app.panels.waves(waves).unwrap().items.len(), 1);
    assert!(app.panels.iter().all(|p| !p.kind.is_start()));
}

#[test]
fn the_start_panel_gives_way_to_content_and_returns_when_the_last_panel_closes() {
    let mut app = App::new();
    app.set_session(Arc::new(SynthSource::new(100)));
    let start = app.panels.focused_id();
    assert!(app.panels.focused().kind.is_start());
    assert_eq!(app.panels.len(), 1);
    // Splitting the placeholder asks for a waveform panel, not a split.
    app.handle(Command::Action(Action::SplitRight));
    let waves = app.panels.focused_id();
    assert_ne!(waves, start);
    assert_eq!(app.panels.len(), 1);
    assert!(app.panels.get(start).is_none());
    assert!(app.panels.waves(waves).unwrap().nav.link.viewport);
    // Every panel closes; the last content panel becomes a fresh start panel.
    app.handle(Command::Action(Action::SplitRight));
    let second = app.panels.focused_id();
    assert!(app.panels.can_close(waves) && app.panels.can_close(second));
    app.handle(Command::Panels(PanelsCommand::Close(waves)));
    assert_eq!(app.panels.layout(), &Layout::single(second));
    app.handle(Command::Panels(PanelsCommand::Close(second)));
    assert_eq!(app.panels.len(), 1);
    let again = app.panels.focused_id();
    assert!(app.panels.focused().kind.is_start());
    assert!(again > second);
    app.panels.validate().unwrap();
    // Closing the placeholder changes nothing; the app action closes the trace.
    assert!(!app.panels.can_close(again));
    let revision = app.panels.revision();
    assert!(app.panels.close(again).unwrap().is_empty());
    assert_eq!(app.panels.revision(), revision);
    assert!(app.doc.is_loaded());
    app.handle(Command::Action(Action::ClosePanel));
    assert!(!app.doc.is_loaded());
    assert!(app.panels.focused().kind.is_start());
}
