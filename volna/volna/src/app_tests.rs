//! GPUI adapter tests: the load loop runs on the GPUI executor and theme
//! changes leave core state alone. Viewer semantics are tested headlessly in
//! `volna-core`.

use super::*;
use gpui_kit::TestAppContext;
use std::sync::Arc;
use volna_core::data::synth::SynthSource;
use volna_core::session::{LoadRequest, LoadResult, OpenSpec};

fn init(cx: &mut TestAppContext) {
    cx.update(crate::init_app);
}

#[gpui_kit::test]
fn loads_run_on_the_executor_and_fill_rows(cx: &mut TestAppContext) {
    init(cx);
    let window = cx.add_window(Workspace::new);
    window
        .update(cx, |ws, _, cx| ws.open_synthetic(1000, cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, _| {
            assert!(ws.app.doc.is_loaded());
            assert!(!ws.app.panels.focused_waves().unwrap().items.is_empty());
            assert_eq!(
                ws.app.panels.focused_waves().unwrap().loaded_count(),
                ws.app.panels.focused_waves().unwrap().items.len()
            );
        })
        .unwrap();
}

#[gpui_kit::test]
fn latest_open_wins_and_stale_demo_cannot_add_rows(cx: &mut TestAppContext) {
    init(cx);
    let window = cx.add_window(Workspace::new);
    // Take the slow open's request out of the queue so it can complete late.
    let slow = window
        .update(cx, |ws, _, cx| {
            ws.app.open_synthetic(50);
            let mut reqs = ws.app.take_requests();
            ws.after(None, cx);
            reqs.pop().unwrap()
        })
        .unwrap();
    let current: Arc<dyn Session> = Arc::new(SynthSource::new(7));
    window
        .update(cx, |ws, _, cx| {
            ws.app.handle(Command::Open(OpenSpec::Synthetic(7)));
            let LoadRequest::Open { generation, .. } = ws.app.take_requests().pop().unwrap() else {
                panic!("expected an open request");
            };
            ws.app.deliver(LoadResult::Opened {
                generation,
                result: Ok(current.clone()),
            });
            ws.after(None, cx);
        })
        .unwrap();
    window
        .update(cx, |ws, _, cx| ws.spawn_request(slow, cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, _| {
            assert!(
                matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(s, &current))
            );
            assert!(matches!(
                ws.app.panels.focused().kind,
                volna_core::panels::PanelKind::Start
            ));
        })
        .unwrap();
}

#[gpui_kit::test]
fn theme_changes_preserve_trace_and_interaction_state(cx: &mut TestAppContext) {
    init(cx);
    let window = cx.add_window(Workspace::new);
    let source: Arc<dyn Session> = Arc::new(SynthSource::new(100));
    window
        .update(cx, |ws, _, cx| {
            ws.set_session(source.clone(), cx);
            ws.app.handle(Command::SetSidebarWidth(355.0));
            ws.app.handle(Command::SetScopesFraction(0.61));
            ws.app.handle(Command::AddVars(vec![0; 100]));
            ws.after(None, cx);
            ws.app.doc.shared.cursor = Some(42);
            ws.app
                .panels
                .focused_waves_mut()
                .unwrap()
                .selected
                .insert(1);
            ws.app.panels.focused_waves_mut().unwrap().anchor = Some(1);
            ws.app.doc.shared.viewport.value.start = 20.0;
            ws.app.doc.shared.viewport.value.end = 80.0;
            ws.app.panels.focused_waves_mut().unwrap().names_width = 260.0;
            ws.app.panels.focused_waves_mut().unwrap().values_width = 140.0;
            ws.app.panels.focused_waves_mut().unwrap().scroll_y = 24.0;
            ws.app.doc.markers.push(volna_core::document::Marker {
                id: 1,
                time: 30,
                label: None,
            });
        })
        .unwrap();
    cx.run_until_parked();
    let (before, history) = window
        .update(cx, |ws, _, _| {
            (
                ws.debug_state(),
                ws.app.panels.focused_waves().unwrap().items[0]
                    .signal()
                    .unwrap()
                    .history
                    .clone()
                    .unwrap(),
            )
        })
        .unwrap();
    for appearance in [
        crate::theme::Appearance::Light,
        crate::theme::Appearance::Dark,
        crate::theme::Appearance::HighContrastDark,
        crate::theme::Appearance::HighContrastLight,
    ] {
        cx.update(|cx| {
            crate::theme::install(
                crate::theme::CoreTheme::from_host(&crate::theme::HostPalette {
                    appearance,
                    ..Default::default()
                }),
                cx,
            )
        });
        cx.run_until_parked();
        window
            .update(cx, |ws, _, _| {
                assert!(
                    matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(s, &source))
                );
                assert_eq!(ws.debug_state(), before);
                let w = &ws.app.panels.focused_waves().unwrap();
                assert!(Arc::ptr_eq(
                    w.items[0].signal().unwrap().history.as_ref().unwrap(),
                    &history
                ));
                assert_eq!(w.names_width, 260.0);
                assert_eq!(w.values_width, 140.0);
                assert_eq!(w.scroll_y, 24.0);
                assert_eq!(ws.app.doc.markers[0].time, 30);
            })
            .unwrap();
    }
}

#[gpui_kit::test]
fn native_idle_save_reopens_a_copied_trace_with_its_workspace(cx: &mut TestAppContext) {
    use crate::native_workspace::Store;
    use volna_core::workspace::persistence::Persistence;
    init(cx);
    let temporary = tempfile::tempdir().unwrap();
    let trace = temporary.path().join("trace.vtr");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr"),
        &trace,
    )
    .unwrap();
    let mut store = Store::new(Some(temporary.path().join("config"))).unwrap();
    store.data_dir = temporary.path().join("fallback");
    let window = cx.add_window(Workspace::new);
    window
        .update(cx, |ws, _, cx| {
            ws.enable_native_persistence(store, Persistence::Auto, false, cx);
            ws.open_path(trace.clone(), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, cx| {
            assert!(ws.app.doc.is_loaded());
            assert!(
                !ws.app.workspace.scheduler.suspended(),
                "{:?}",
                ws.app.workspace.notices
            );
            ws.dispatch(Command::AddVars(vec![0, 1]), None, cx);
            ws.dispatch(Command::Action(Action::SplitRight), None, cx);
            ws.app.tick(Instant::now() + Duration::from_secs(2));
            ws.after(None, cx);
            assert!(!ws.app.workspace.scheduler.dirty());
            assert!(temporary.path().join("trace.vtr.volna.json").exists());
            ws.open_path(trace.clone(), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, _| {
            assert_eq!(ws.app.panels.len(), 2);
            for panel in ws.app.panels.iter() {
                let waves = panel.kind.waves().unwrap();
                assert_eq!(waves.items.len(), 2);
                assert_eq!(waves.loaded_count(), 2);
            }
            assert!(!ws.app.workspace.scheduler.dirty());
        })
        .unwrap();
}

#[gpui_kit::test]
fn settings_tab_results_json_view_and_palette_render(cx: &mut TestAppContext) {
    use volna_core::app::SettingsCommand;
    init(cx);
    // Dialogs (the palette) need the component root, as in production.
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    window
        .update(cx, |ws, window, cx| {
            ws.app
                .settings_loaded("{\n  \"waves.snapPixels\": 4, // four\n}\n");
            ws.dispatch(Command::Settings(SettingsCommand::Open), Some(window), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let settings = window
        .update(cx, |ws, _, _| {
            ws.app.panels.settings_id().expect("tab open")
        })
        .unwrap();
    // The tab renders without a trace, then with one, in every mode.
    for query in [
        "",
        "snap px",
        "@modified",
        "@id:waves",
        "nothing matches this",
    ] {
        window
            .update(cx, |ws, window, cx| {
                ws.dispatch(
                    Command::Settings(SettingsCommand::Query(query.into())),
                    Some(window),
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
    }
    window
        .update(cx, |ws, window, cx| {
            ws.dispatch(
                Command::Settings(SettingsCommand::ToggleJson),
                Some(window),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, window, cx| {
            let view = ws.settings_view(settings, window, cx);
            assert!(view.read(cx).json.is_some(), "the JSON view was rendered");
            let text = view.read(cx).json.as_ref().unwrap().text(cx);
            assert_eq!(text, ws.app.settings.text());
            ws.dispatch(
                Command::Settings(SettingsCommand::ReplaceText(
                    "{\"waves.snapPixels\": 4,,}".into(),
                )),
                Some(window),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let source: Arc<dyn Session> = Arc::new(SynthSource::new(20));
    window
        .update(cx, |ws, window, cx| {
            ws.set_session(source, cx);
            ws.dispatch(
                Command::Settings(SettingsCommand::ToggleJson),
                Some(window),
                cx,
            );
            ws.dispatch(
                Command::Settings(SettingsCommand::Set {
                    id: "panels.linkByDefault".into(),
                    value: volna_core::settings::Value::Bool(false),
                }),
                Some(window),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, window, cx| {
            assert_eq!(ws.app.panels.settings_id(), Some(settings));
            assert_eq!(ws.app.panels.len(), 2);
            // The edit was refused while the document is broken.
            assert!(ws.app.settings.text().contains(",,"));
            ws.open_palette(window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, window, cx| {
            ws.dispatch(Command::Settings(SettingsCommand::Close), Some(window), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, _| {
            assert_eq!(ws.app.panels.settings_id(), None);
            assert_eq!(ws.app.panels.len(), 1);
        })
        .unwrap();
}

/// `appearance.zoom` scales gpui-kit's rem size, Volna's chrome and the wave
/// table together, and changes it like a theme change: viewer state stays.
#[gpui_kit::test]
fn interface_zoom_scales_settings_tab_and_wave_rows_and_keeps_viewer_state(
    cx: &mut TestAppContext,
) {
    use volna_core::app::SettingsCommand;
    use volna_core::panels::PanelsCommand;
    use volna_core::settings::{Value, ZoomStep};
    init(cx);
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    let source: Arc<dyn Session> = Arc::new(SynthSource::new(100));
    window
        .update(cx, |ws, window, cx| {
            ws.set_session(source.clone(), cx);
            ws.app.handle(Command::AddVars(vec![0; 400]));
            ws.after(None, cx);
            ws.app.doc.shared.cursor = Some(42);
            ws.app.doc.shared.viewport.value.start = 20.0;
            ws.app.doc.shared.viewport.value.end = 80.0;
            ws.app.doc.markers.push(volna_core::document::Marker {
                id: 1,
                time: 30,
                label: None,
            });
            let w = ws.app.panels.focused_waves_mut().unwrap();
            w.selected.insert(2);
            w.anchor = Some(2);
            w.names_width = 260.0;
            w.scroll_y = 48.0;
            // A second group so the settings tab and a wave panel are both visible.
            ws.dispatch(Command::Action(Action::SplitRight), Some(window), cx);
            ws.dispatch(Command::Settings(SettingsCommand::Open), Some(window), cx);
            ws.dispatch(
                Command::Panels(PanelsCommand::FocusIndex(0)),
                Some(window),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let (before, settings, waves) = window
        .update(cx, |ws, _, _| {
            let waves = ws.app.panels.focused_id();
            assert!(ws.app.panels.waves(waves).is_some());
            (
                ws.debug_state(),
                ws.app.panels.settings_id().expect("settings tab"),
                waves,
            )
        })
        .unwrap();
    let design_row = crate::theme::CoreTheme::one_dark().row_height;
    let design_font = crate::theme::CoreTheme::one_dark().ui_size;
    for zoom in [0.5f32, 1.0, 2.0] {
        window
            .update(cx, |ws, window, cx| {
                ws.dispatch(
                    Command::Settings(SettingsCommand::Set {
                        id: "appearance.zoom".into(),
                        value: Value::Number(f64::from(zoom)),
                    }),
                    Some(window),
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |ws, window, cx| {
                assert_eq!(ws.app.settings.resolved().appearance.zoom, f64::from(zoom));
                let t = *crate::theme::theme(cx);
                assert_eq!(t.zoom, zoom);
                assert_eq!(t.row_height, design_row * zoom);
                assert_eq!(t.ui_size, design_font * zoom);
                // gpui-kit's Root derives the rem size from this font size.
                assert_eq!(
                    gpui_kit::component::Theme::global(cx).font_size,
                    px(design_font * zoom)
                );
                assert_eq!(f32::from(window.rem_size()), design_font * zoom);
                // The settings tab is open and has a live view.
                assert_eq!(ws.app.panels.settings_id(), Some(settings));
                let view = ws.settings_view(settings, window, cx);
                assert!(view.read(cx).json.is_none());
                // The wave panel was laid out and painted at this zoom.
                let w = ws.app.panels.waves(waves).unwrap();
                let layout = w.last_layout();
                assert_eq!(layout.zoom, zoom, "wave panel rendered at zoom {zoom}");
                assert_eq!(layout.row_h, design_row * zoom);
                // Hit regions scale too; the names column is clamped to the
                // (narrow, split) test panel at 2x, so check it where it fits.
                assert_eq!(layout.names_split.width(), 8.0 * zoom);
                if zoom <= 1.0 {
                    assert_eq!(layout.names.width(), 260.0 * zoom);
                } else {
                    assert!(layout.names.width() >= 72.0 * zoom);
                }
                assert!(layout.bounds.width() > 0.0 && w.frames_painted > 0);
                // Viewer state is untouched, as for a theme change.
                assert!(
                    matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(s, &source))
                );
                assert_eq!(ws.debug_state(), before);
                assert_eq!(w.names_width, 260.0);
                assert!(w.selected.contains(&2));
                assert_eq!(ws.app.doc.markers[0].time, 30);
                // The first visible row is the same at every zoom.
                assert_eq!(w.scroll_y, 48.0 * zoom);
            })
            .unwrap();
    }
    // Keyboard steps go through the settings file; reset removes the key.
    window
        .update(cx, |ws, window, cx| {
            ws.dispatch(
                Command::Settings(SettingsCommand::Zoom(ZoomStep::In)),
                Some(window),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, cx| {
            assert_eq!(crate::theme::theme(cx).zoom, 2.1);
            assert!(ws.app.settings.text().contains("\"appearance.zoom\": 2.1"));
            assert_eq!(ws.debug_state(), before);
        })
        .unwrap();
    window
        .update(cx, |ws, window, cx| {
            ws.dispatch(
                Command::Settings(SettingsCommand::Zoom(ZoomStep::Reset)),
                Some(window),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, cx| {
            assert_eq!(crate::theme::theme(cx).zoom, 1.0);
            assert_eq!(crate::theme::theme(cx).row_height, design_row);
            assert!(!ws.app.settings.text().contains("appearance.zoom"));
            assert_eq!(
                ws.app.panels.waves(waves).unwrap().last_layout().row_h,
                design_row
            );
        })
        .unwrap();
}

/// A workspace hosted inside the component `Root`, updated like a plain window.
/// Tabs close from their own close button after the title, as in Zed, and
/// from a middle click; the toolbar no longer carries a close button.
#[gpui_kit::test]
fn tab_close_buttons_and_middle_click_close_their_own_tab(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
    use volna_core::app::SettingsCommand;
    use volna_core::panels::PanelsCommand;
    init(cx);
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    let (first, second, settings) = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(SynthSource::new(100)), cx);
            ws.app.handle(Command::AddVars(vec![0; 4]));
            let first = ws.app.panels.focused_id();
            ws.dispatch(
                Command::Panels(PanelsCommand::NewTab { group_of: first }),
                Some(window),
                cx,
            );
            let second = ws.app.panels.focused_id();
            ws.dispatch(Command::Settings(SettingsCommand::Open), Some(window), cx);
            (first, second, ws.app.panels.settings_id().unwrap())
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    let bounds = |vcx: &mut VisualTestContext, selector: String| {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.debug_bounds(Box::leak(selector.into_boxed_str()))
    };
    for id in [first, second, settings] {
        assert!(
            bounds(&mut vcx, format!("tab-{}", id.0)).is_some(),
            "tab {id:?}"
        );
    }
    // Press the second tab's close button.
    let tab = bounds(&mut vcx, format!("tab-{}", second.0)).unwrap();
    let close = bounds(&mut vcx, format!("tab-close-{}", second.0)).expect("close button");
    assert!(
        close.left() >= tab.center().x,
        "the close button follows the title"
    );
    vcx.simulate_mouse_move(close.center(), None, Modifiers::default());
    vcx.simulate_click(close.center(), Modifiers::default());
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, _, _| {
            assert!(ws.app.panels.get(second).is_none());
            assert!(ws.app.panels.get(first).is_some());
            assert_eq!(ws.app.panels.settings_id(), Some(settings));
        })
        .unwrap();
    // A middle click on the title closes a tab too.
    let tab = bounds(&mut vcx, format!("tab-{}", settings.0)).unwrap();
    vcx.simulate_mouse_move(tab.center(), None, Modifiers::default());
    vcx.simulate_mouse_down(tab.center(), MouseButton::Middle, Modifiers::default());
    vcx.simulate_mouse_up(tab.center(), MouseButton::Middle, Modifiers::default());
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, _, _| {
            assert_eq!(ws.app.panels.settings_id(), None);
            assert_eq!(ws.app.panels.len(), 1);
        })
        .unwrap();
}

/// The signal menu's Height submenu is reachable from the keyboard, and the
/// row-height actions reach the core through GPUI's action dispatch.
#[gpui_kit::test]
fn signal_menu_height_submenu_and_row_height_actions(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    init(cx);
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(SynthSource::new(100)), cx);
            ws.dispatch(Command::AddVars(vec![0, 1, 2]), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let heights = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                ws.app
                    .panels
                    .focused_waves()
                    .unwrap()
                    .items
                    .iter()
                    .map(|item| item.height().multiple())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    // All three new rows are selected: Shift+F10, then Height ▸ 2×.
    vcx.simulate_keystrokes("shift-f10");
    vcx.run_until_parked();
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.wave_menu.is_some())
            .unwrap(),
        "the popup is hosted"
    );
    vcx.simulate_keystrokes("down down down down right down enter");
    vcx.run_until_parked();
    assert_eq!(heights(&mut vcx), [2, 2, 2]);
    window
        .update(&mut vcx, |ws, _, _| {
            assert!(ws.app.panels.focused_waves().unwrap().menu.is_none());
            assert!(ws.wave_menu.is_none());
        })
        .unwrap();
    // Palette actions act on the selection.
    vcx.update(|window, cx| {
        window.dispatch_action(Box::new(IncreaseRowHeight), cx);
    });
    vcx.run_until_parked();
    assert_eq!(heights(&mut vcx), [3, 3, 3]);
    vcx.update(|window, cx| {
        window.dispatch_action(Box::new(DecreaseRowHeight), cx);
        window.dispatch_action(Box::new(DecreaseRowHeight), cx);
    });
    vcx.run_until_parked();
    assert_eq!(heights(&mut vcx), [1, 1, 1]);
    vcx.update(|window, cx| {
        window.dispatch_action(Box::new(IncreaseRowHeight), cx);
        window.dispatch_action(Box::new(ResetRowHeight), cx);
    });
    vcx.run_until_parked();
    assert_eq!(heights(&mut vcx), [1, 1, 1]);
}

/// ⌘C / ⌘V (ctrl on Linux) on the wave panel duplicate rows that share data,
/// and ⌘X removes them.
#[gpui_kit::test]
fn wave_copy_paste_keys_duplicate_rows(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    init(cx);
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(SynthSource::new(100)), cx);
            ws.dispatch(Command::AddVars(vec![0, 1, 2]), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let names = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                ws.app
                    .panels
                    .focused_waves()
                    .unwrap()
                    .items
                    .iter()
                    .map(|item| item.name().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    let original = names(&mut vcx);
    window
        .update(&mut vcx, |ws, _, _| {
            ws.app.panels.focused_waves_mut().unwrap().selected = [0].into();
        })
        .unwrap();
    let (copy, cut, paste) = if cfg!(target_os = "macos") {
        ("cmd-c", "cmd-x", "cmd-v")
    } else {
        ("ctrl-c", "ctrl-x", "ctrl-v")
    };
    vcx.simulate_keystrokes(copy);
    vcx.simulate_keystrokes(paste);
    vcx.simulate_keystrokes(paste);
    vcx.run_until_parked();
    let clk = original[0].as_str();
    assert_eq!(
        names(&mut vcx),
        [clk, clk, clk, original[1].as_str(), original[2].as_str()]
    );
    window
        .update(&mut vcx, |ws, _, _| {
            let rows = &ws.app.panels.focused_waves().unwrap().items;
            assert!(rows[..3].iter().all(|row| Arc::ptr_eq(
                row.signal().unwrap().history.as_ref().unwrap(),
                rows[0].signal().unwrap().history.as_ref().unwrap()
            )));
        })
        .unwrap();
    vcx.simulate_keystrokes(cut);
    vcx.run_until_parked();
    assert_eq!(
        names(&mut vcx),
        [clk, clk, original[1].as_str(), original[2].as_str()]
    );
}

/// The status bar meter appears with a trace and fills with the used share.
#[gpui_kit::test]
fn status_bar_memory_meter_tracks_the_budget(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    init(cx);
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    let bounds = |vcx: &mut VisualTestContext, selector: &'static str| {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.debug_bounds(selector)
    };
    assert!(bounds(&mut vcx, "status-memory").is_none(), "no trace");
    let trace = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    window
        .update(&mut vcx, |ws, window, cx| {
            let session = OpenSpec::Path(trace.into()).open().unwrap();
            ws.set_session(session, cx);
            ws.dispatch(Command::AddVars(vec![0, 1, 2, 3]), Some(window), cx);
        })
        .unwrap();
    vcx.run_until_parked();
    let memory = window
        .update(&mut vcx, |ws, _, _| ws.app.status().memory.unwrap())
        .unwrap();
    assert!(memory.used > 0);
    // The label sits inside the pill, on top of the fill.
    let pill = bounds(&mut vcx, "status-memory").unwrap();
    assert_eq!(f32::from(pill.size.width), MEMORY_PILL_W);
    let label = bounds(&mut vcx, "status-memory-label").unwrap();
    assert!(
        label.left() > pill.left() && label.right() < pill.right(),
        "{label:?} inside {pill:?}"
    );
    assert!(
        f32::from((label.center().x - pill.center().x).abs()) < 1.0,
        "centred"
    );
    let fill = bounds(&mut vcx, "status-memory-fill").unwrap();
    let expected = (memory.fraction() * MEMORY_PILL_W).max(3.0);
    assert!(
        (f32::from(fill.size.width) - expected).abs() < 0.5,
        "{fill:?} for {memory:?}"
    );

    // Clicking the meter opens its menu; Memory budget ▸ 1 GiB applies live.
    use gpui_kit::Modifiers;
    vcx.simulate_click(pill.center(), Modifiers::default());
    vcx.run_until_parked();
    // Keep the pointer off the popup so hover cannot steer the keyboard.
    vcx.simulate_mouse_move(
        gpui_kit::point(gpui_kit::px(1.0), gpui_kit::px(1.0)),
        None,
        Modifiers::default(),
    );
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.status_menu.is_some())
            .unwrap()
    );
    // At the right edge of the window the submenus open to the left.
    vcx.simulate_keystrokes("down left down down enter");
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, _, _| {
            assert_eq!(ws.app.settings.resolved().memory.budget_mib, 1024);
            assert_eq!(ws.app.status().memory.unwrap().limit, 1024 * 1024 * 1024);
            assert!(ws.status_menu.is_none());
        })
        .unwrap();
    let pill = bounds(&mut vcx, "status-memory").unwrap();
    let label = window
        .update(&mut vcx, |ws, _, _| ws.app.status().memory.unwrap().label())
        .unwrap();
    assert!(label.ends_with("/ 1 GiB"), "{label}");

    // Memory Settings… opens the Settings tab filtered to the memory page.
    vcx.simulate_click(pill.center(), Modifiers::default());
    vcx.run_until_parked();
    vcx.simulate_keystrokes("up enter");
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, _, _| {
            assert!(ws.app.panels.settings_id().is_some());
            assert_eq!(ws.app.settings_view.query, "@id:memory");
        })
        .unwrap();
}

struct RootWindow {
    root: gpui_kit::WindowHandle<gpui_kit::component::Root>,
    workspace: gpui_kit::Entity<Workspace>,
}

impl RootWindow {
    fn update<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Workspace, &mut gpui_kit::Window, &mut gpui_kit::Context<Workspace>) -> R,
    ) -> anyhow::Result<R> {
        use gpui_kit::AppContext as _;
        let workspace = self.workspace.clone();
        // Lease only the workspace: dialogs update the root themselves.
        cx.update_window(self.root.into(), move |_, window, cx| {
            workspace.update(cx, |ws, cx| f(ws, window, cx))
        })
    }
}

/// `A` on the wave panel draws the selected vector as a plot and back, and
/// the format popup hosts its Draw and Range sections.
#[gpui_kit::test]
fn analog_key_and_format_popup_sections(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::data::SignalShape;
    use volna_core::geometry::{Modifiers, MouseButton, point};
    use volna_core::wave::PointerEvent;
    init(cx);
    let mut workspace = None;
    let root = cx.add_window(|window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        gpui_kit::component::Root::new(ws, window, cx)
    });
    let window = RootWindow {
        root,
        workspace: workspace.unwrap(),
    };
    window
        .update(cx, |ws, window, cx| {
            let session = Arc::new(SynthSource::new(100));
            ws.set_session(session.clone(), cx);
            let vector = session
                .hierarchy()
                .vars
                .iter()
                .position(|v| matches!(v.shape, SignalShape::Vector { .. }))
                .unwrap();
            ws.dispatch(Command::AddVars(vec![vector]), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let row = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                let item = ws.app.panels.focused_waves().unwrap().items[0].clone();
                (
                    item.signal().unwrap().analog.as_ref().map(|a| a.draw),
                    item.height().multiple(),
                )
            })
            .unwrap()
    };
    window
        .update(&mut vcx, |ws, _, _| {
            ws.app.panels.focused_waves_mut().unwrap().selected = [0].into();
        })
        .unwrap();
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(
        row(&mut vcx),
        (Some(volna_core::wave::analog::AnalogDraw::Step), 3)
    );
    // A frame paints the plot; then the badge opens the format popup.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    window
        .update(&mut vcx, |ws, window, cx| {
            let panel = ws.app.panels.focused_id();
            let badge = ws.app.panels.focused_waves().unwrap().last_layout().badges[0].1;
            ws.dispatch(
                Command::Pointer(
                    panel,
                    PointerEvent::Down {
                        position: point(badge.left() + 2.0, badge.top() + 2.0),
                        button: MouseButton::Left,
                        modifiers: Modifiers::default(),
                    },
                ),
                Some(window),
                cx,
            );
        })
        .unwrap();
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, _, _| {
            let menu = ws
                .app
                .panels
                .focused_waves()
                .unwrap()
                .menu
                .as_ref()
                .unwrap();
            assert!(
                menu.entries
                    .contains(&volna_core::wave::MenuEntry::Label("Range".into()))
            );
            assert!(ws.wave_menu.is_some(), "the popup is hosted");
        })
        .unwrap();
    // Escape closes the popup and returns the keys to the panel.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.wave_menu.is_none())
            .unwrap()
    );
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(row(&mut vcx), (None, 1));
}
