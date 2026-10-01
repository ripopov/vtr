//! GPUI adapter tests: the load loop runs on the GPUI executor and theme
//! changes leave core state alone. Viewer semantics are tested headlessly in
//! `volna-core`.

use super::*;
use gpui_kit::TestAppContext;
use std::sync::Arc;
use volna_core::session::{LoadRequest, LoadResult, OpenSpec};
use volna_core::testing::ProceduralTrace;
use volna_core::testing::a_all;
use volna_core::trace::TraceId;

fn init(cx: &mut TestAppContext) {
    cx.update(crate::init_app);
}

#[gpui_kit::test]
fn desktop_startup_hosts_workspace_and_dialogs(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use gpui_kit::component::WindowExt as _;

    init(cx);
    for embedded in [false, true] {
        let (workspace, handle) = cx.update(|cx| {
            let workspace = crate::open_main_window(cx, embedded).unwrap();
            let handle = *cx.windows().last().unwrap();
            assert_eq!(workspace.read(cx).embedded, embedded);
            (workspace, handle)
        });
        let mut vcx = VisualTestContext::from_window(handle, cx);
        vcx.update(|window, cx| {
            let root = gpui_kit::base::Root::read(window, cx);
            assert_eq!(root.view().entity_id(), workspace.entity_id());
            window.open_dialog(cx, |dialog, _, _| {
                dialog.child(
                    div()
                        .debug_selector(|| "startup-dialog".into())
                        .child("Startup dialog"),
                )
            });
        });
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("startup-dialog").is_some());
        vcx.update(|window, cx| window.close_dialog(cx));
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("startup-dialog").is_none());
    }
}

#[gpui_kit::test]
fn desktop_window_matches_shell_identity(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let options = crate::window_options(cx);
        assert_eq!(options.app_id.as_deref(), Some(crate::desktop::APP_ID));
        let launcher = include_str!("../packaging/io.github.ripopov.volna.desktop");
        let wm_class = launcher
            .lines()
            .find_map(|line| line.strip_prefix("StartupWMClass="));
        assert_eq!(options.app_id.as_deref(), wm_class);
        #[cfg(target_os = "linux")]
        {
            let icon = options.icon.expect("X11 window icon");
            assert_eq!(icon.dimensions(), (256, 256));
            assert!(icon.pixels().any(|pixel| pixel.0[0] > 240));
        }
    });
}

#[gpui_kit::test]
fn loads_run_on_the_executor_and_fill_rows(cx: &mut TestAppContext) {
    init(cx);
    let window = cx.add_window(Workspace::new);
    let trace = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    window
        .update(cx, |ws, _, cx| ws.open_path(trace.into(), cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, window, cx| {
            assert!(ws.app.doc.is_loaded());
            let count = ws.app.doc.hierarchy(TraceId::A).unwrap().var_count();
            ws.dispatch(Command::AddVars(a_all(0..count)), Some(window), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, _| {
            assert!(!ws.app.panels.focused_waves().unwrap().items().is_empty());
            assert_eq!(
                ws.app.panels.focused_waves().unwrap().loaded_count(),
                ws.app.panels.focused_waves().unwrap().items().len()
            );
        })
        .unwrap();
}

/// Scope rows gain a right-aligned signal count once the count after open
/// is delivered on the executor, in light and dark themes alike.
#[gpui_kit::test]
fn scope_rows_show_their_signal_count_in_both_themes(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::sidebar::{ScopeTreeModel, TreeNode};
    use volna_core::trace::Traced;
    init(cx);
    let window = cx.add_window(Workspace::new);
    let trace = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    window
        .update(cx, |ws, _, cx| ws.open_path(trace.into(), cx))
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.run_until_parked();
    let root = window
        .update(&mut vcx, |ws, _, _| {
            let root = ws
                .app
                .doc
                .hierarchy(TraceId::A)
                .unwrap()
                .roots()
                .first()
                .unwrap();
            let size = ScopeTreeModel::size(
                ws.app.doc.traces(),
                TreeNode::scope(Traced::new(TraceId::A, root)),
            )
            .expect("counted after open");
            assert!(size.signals > 0 && size.variables >= size.signals);
            root
        })
        .unwrap();
    for appearance in [
        crate::theme::Appearance::Light,
        crate::theme::Appearance::Dark,
    ] {
        vcx.update(|_, cx| {
            crate::theme::install(
                crate::theme::CoreTheme::from_host(&crate::theme::HostPalette {
                    appearance,
                    ..Default::default()
                }),
                cx,
            )
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let row = vcx
            .debug_bounds(Box::leak(format!("scope-row-A-{root}").into_boxed_str()))
            .expect("root scope row");
        let count = vcx
            .debug_bounds(Box::leak(format!("scope-size-A-{root}").into_boxed_str()))
            .expect("signal count on the row");
        assert!(count.size.width > gpui_kit::px(0.0), "{appearance:?}");
        assert!(
            count.left() > row.center().x && count.right() <= row.right(),
            "{appearance:?}: the count sits at the row's right, {count:?} in {row:?}"
        );
        assert!(row.top() <= count.top() && count.bottom() <= row.bottom());
    }
}

/// With an activity index beside the trace, scope rows show how many of
/// their signals change in the view and a meter instead of the size alone,
/// in light and dark themes alike.
#[gpui_kit::test]
fn scope_rows_show_activity_meters_in_both_themes(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::sidebar::TreeNode;
    use volna_core::trace::Traced;
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("picorv32.vtr");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr"),
        &trace,
    )
    .unwrap();
    let reader = vtr::Reader::open(&trace).unwrap();
    let id = vtr::activity::Identity::of(&reader).unwrap();
    vtr::activity::Sidecar::new(&trace, &id, None)
        .write(|w| vtr::activity::build(&reader, w, &Default::default()))
        .unwrap();
    let window = cx.add_window(Workspace::new);
    window
        .update(cx, |ws, _, cx| ws.open_path(trace.clone(), cx))
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.run_until_parked();
    // The first frame asks for the classification; its result paints the next.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    let root = window
        .update(&mut vcx, |ws, _, _| {
            let root = ws
                .app
                .doc
                .hierarchy(TraceId::A)
                .unwrap()
                .roots()
                .first()
                .unwrap();
            let a = ws
                .app
                .scope_activity(TreeNode::scope(Traced::new(TraceId::A, root)))
                .expect("meters once classified");
            assert!(
                a.exact() && a.changing > 0 && a.changing <= a.total,
                "{a:?}"
            );
            root
        })
        .unwrap();
    for appearance in [
        crate::theme::Appearance::Light,
        crate::theme::Appearance::Dark,
    ] {
        vcx.update(|_, cx| {
            crate::theme::install(
                crate::theme::CoreTheme::from_host(&crate::theme::HostPalette {
                    appearance,
                    ..Default::default()
                }),
                cx,
            )
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        let mut bounds = |name: String| vcx.debug_bounds(Box::leak(name.into_boxed_str()));
        let row = bounds(format!("scope-row-A-{root}")).expect("root scope row");
        let count = bounds(format!("scope-activity-A-{root}")).expect("changing of total");
        let meter = bounds(format!("scope-meter-A-{root}")).expect("meter");
        assert!(
            bounds(format!("scope-size-A-{root}")).is_none(),
            "the count replaces the size"
        );
        assert!(
            count.left() > row.center().x && count.right() <= meter.left(),
            "{appearance:?}: {count:?} {meter:?}"
        );
        assert!(
            meter.right() <= row.right() && meter.size.width > gpui_kit::px(20.0),
            "{appearance:?}: {meter:?} in {row:?}"
        );
        assert!(row.top() <= meter.top() && meter.bottom() <= row.bottom());
    }
}

#[gpui_kit::test]
fn latest_open_wins_and_a_stale_open_cannot_replace_the_trace(cx: &mut TestAppContext) {
    init(cx);
    let window = cx.add_window(Workspace::new);
    // Take the slow open's request out of the queue so it can complete late.
    let trace = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    let slow = window
        .update(cx, |ws, _, cx| {
            ws.app.open_path(trace.into());
            let mut reqs = ws.app.take_requests();
            ws.after(None, cx);
            reqs.pop().unwrap()
        })
        .unwrap();
    let current = ProceduralTrace::session(7);
    window
        .update(cx, |ws, _, cx| {
            ws.app.open_bytes("current.vtr".into(), Vec::new());
            let LoadRequest::Open { generation, .. } = ws.app.take_requests().pop().unwrap() else {
                panic!("expected an open request");
            };
            ws.app.deliver(LoadResult::Opened {
                trace: TraceId::A,
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
                matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(&s, &current))
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
    let source: Arc<dyn Session> = Arc::new(ProceduralTrace::new(100));
    window
        .update(cx, |ws, _, cx| {
            ws.set_session(source.clone(), cx);
            ws.app.handle(Command::SetSidebarWidth(355.0));
            ws.app.handle(Command::SetScopesFraction(0.61));
            ws.app.handle(Command::AddVars(a_all(vec![0; 100])));
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
            ws.app.doc.add_marker(30);
        })
        .unwrap();
    cx.run_until_parked();
    let (before, history) = window
        .update(cx, |ws, _, _| {
            (
                ws.debug_state(),
                ws.app.panels.focused_waves().unwrap().items()[0]
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
                    matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(&s, &source))
                );
                assert_eq!(ws.debug_state(), before);
                let w = &ws.app.panels.focused_waves().unwrap();
                assert!(Arc::ptr_eq(
                    w.items()[0].signal().unwrap().history.as_ref().unwrap(),
                    &history
                ));
                assert_eq!(w.names_width, 260.0);
                assert_eq!(w.values_width, 140.0);
                assert_eq!(w.scroll_y, 24.0);
                assert_eq!(ws.app.doc.markers()[0].time, 30);
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
            ws.dispatch(Command::AddVars(a_all(vec![0, 1])), None, cx);
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
                assert_eq!(waves.items().len(), 2);
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
    let source: Arc<dyn Session> = Arc::new(ProceduralTrace::new(20));
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
    let source: Arc<dyn Session> = Arc::new(ProceduralTrace::new(100));
    window
        .update(cx, |ws, window, cx| {
            ws.set_session(source.clone(), cx);
            ws.app.handle(Command::AddVars(a_all(vec![0; 400])));
            ws.after(None, cx);
            ws.app.doc.shared.cursor = Some(42);
            ws.app.doc.shared.viewport.value.start = 20.0;
            ws.app.doc.shared.viewport.value.end = 80.0;
            ws.app.doc.add_marker(30);
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
                    matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(&s, &source))
                );
                assert_eq!(ws.debug_state(), before);
                assert_eq!(w.names_width, 260.0);
                assert!(w.selected.contains(&2));
                assert_eq!(ws.app.doc.markers()[0].time, 30);
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.app.handle(Command::AddVars(a_all(vec![0; 4])));
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2])), Some(window), cx);
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
                    .items()
                    .iter()
                    .map(|item| item.height().multiple())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    // All three new rows are selected: Shift+F10, then (past Group
    // selection and Color) Height ▸ 2×.
    vcx.simulate_keystrokes("shift-f10");
    vcx.run_until_parked();
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.wave_menu.is_some())
            .unwrap(),
        "the popup is hosted"
    );
    vcx.simulate_keystrokes("down down down down down down right down enter");
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2])), Some(window), cx);
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
                    .items()
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
            let rows = &ws.app.panels.focused_waves().unwrap().items();
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
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2, 3])), Some(window), cx);
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

/// Long transient text is cropped in the message slot: it moves neither the
/// memory meter nor anything else on the right, and a narrow window clips
/// the left context before any right-hand tool.
#[gpui_kit::test]
fn status_bar_groups_stay_put_under_long_messages(cx: &mut TestAppContext) {
    use gpui_kit::{VisualTestContext, px, size};
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
            .unwrap_or_else(|| panic!("{selector}"))
    };
    let trace = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    window
        .update(&mut vcx, |ws, window, cx| {
            let session = OpenSpec::Path(trace.into()).open().unwrap();
            ws.set_session(session, cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2, 3])), Some(window), cx);
        })
        .unwrap();
    let set_message = |vcx: &mut VisualTestContext, text: Option<String>| {
        window
            .update(vcx, |ws, _, cx| {
                ws.app.variables.notice = text;
                cx.notify();
            })
            .unwrap();
    };

    vcx.simulate_resize(size(px(1200.0), px(700.0)));
    let meter = bounds(&mut vcx, "status-memory");
    let right = bounds(&mut vcx, "status-right");
    let long = "#123 · X cycles 4–5 (1 cycle) ".repeat(40);
    set_message(&mut vcx, Some(long.clone()));
    assert_eq!(bounds(&mut vcx, "status-memory"), meter, "meter moved");
    assert_eq!(bounds(&mut vcx, "status-right"), right, "right group moved");
    let slot = bounds(&mut vcx, "status-message");
    let left = bounds(&mut vcx, "status-left");
    assert!(slot.left() >= left.right(), "{slot:?} after {left:?}");
    assert!(slot.right() <= right.left(), "{slot:?} before {right:?}");
    let bar = slot.size.height;
    assert!(f32::from(bar) <= 24.0, "one line: {slot:?}");

    // Too narrow for everything: the message empties, the left group is
    // clipped, and the right group keeps its full width at the edge.
    vcx.simulate_resize(size(px(560.0), px(700.0)));
    let right = bounds(&mut vcx, "status-right");
    let meter = bounds(&mut vcx, "status-memory");
    assert!(
        f32::from(right.right()) <= 560.0,
        "{right:?} inside the window"
    );
    assert!(meter.left() >= right.left() && meter.right() <= right.right());
    let left = bounds(&mut vcx, "status-left");
    assert!(
        left.right() <= right.left(),
        "{left:?} clipped before {right:?}"
    );
    set_message(&mut vcx, None);
    assert_eq!(
        bounds(&mut vcx, "status-right"),
        right,
        "message-free width"
    );
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
            let session = Arc::new(ProceduralTrace::new(100));
            ws.set_session(session.clone(), cx);
            let vector = session
                .hierarchy()
                .vars()
                .position(|v| matches!(v.shape, SignalShape::Vector { .. }))
                .unwrap();
            ws.dispatch(Command::AddVars(a_all(vec![vector])), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let row = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                let item = ws.app.panels.focused_waves().unwrap().items()[0].clone();
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

/// Row colours (`docs/RATIONALE.md`, "Volna row colours"): the signal menu hosts a Color
/// submenu, and the palette's `Color: …` commands colour the selection as
/// one undo step.
#[gpui_kit::test]
fn color_submenu_and_palette_commands_colour_the_selection(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::wave::{MenuEntry, Tint};
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2])), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    window
        .update(&mut vcx, |ws, window, cx| {
            ws.app.panels.focused_waves_mut().unwrap().selected = [0, 1].into();
            let panel = ws.app.panels.focused_id();
            ws.dispatch(Command::OpenSignalMenu(panel), Some(window), cx);
        })
        .unwrap();
    vcx.run_until_parked();
    // The hosted popup renders with its Color submenu.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    window
        .update(&mut vcx, |ws, _, _| {
            assert!(ws.wave_menu.is_some(), "the popup is hosted");
            let menu = ws.app.panels.focused_waves().unwrap().menu.as_ref().unwrap();
            assert!(menu.entries.iter().any(
                |e| matches!(e, MenuEntry::Submenu { label, items } if label == "Color" && items.len() == 6)
            ));
        })
        .unwrap();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    let labels = window
        .update(&mut vcx, |ws, _, _| {
            crate::palette::commands(&ws.app, "color")
                .into_iter()
                .map(|(label, _)| label)
                .filter(|l| l.starts_with("Color: "))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(
        labels,
        [
            "Color: Default",
            "Color: Blue",
            "Color: Cyan",
            "Color: Violet",
            "Color: Pink",
            "Color: Grey"
        ]
    );
    vcx.update(|window, cx| {
        window.dispatch_action(
            Box::new(SetTint {
                tint: Some(Tint::Pink),
            }),
            cx,
        )
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    window
        .update(&mut vcx, |ws, _, _| {
            let w = ws.app.panels.focused_waves().unwrap();
            assert_eq!(
                (0..3).map(|i| w.ink(i)).collect::<Vec<_>>(),
                [Some(Tint::Pink), Some(Tint::Pink), None]
            );
            assert_eq!(ws.app.undo_label(), Some("Color 2 signals Pink"));
        })
        .unwrap();
}

/// Stacked areas from the keyboard and the group menu
/// (`docs/stacked-areas.html`): `Shift+A` stacks a selected group as one
/// step, the stacked row paints, the menu's Draw section shows it checked
/// and its Activity entry unstacks it, and the palette lists the command.
#[gpui_kit::test]
fn shift_a_and_the_group_menu_stack_and_unstack_a_group(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::wave::model::MenuAction;
    use volna_core::wave::{GroupStyle, MenuEntry};
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2])), Some(window), cx);
            let w = ws.app.panels.focused_waves_mut().unwrap();
            w.selected = [0, 1, 2].into();
            w.anchor = Some(0);
            ws.dispatch(Command::Action(Action::GroupSelection), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    // Escape keeps the name "Group 1"; the group stays selected.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    let style = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                let g = ws.app.panels.focused_waves().unwrap().items()[0]
                    .group()
                    .unwrap()
                    .clone();
                (g.style, g.height.multiple())
            })
            .unwrap()
    };
    assert_eq!(style(&mut vcx), (GroupStyle::Activity, 1));
    vcx.simulate_keystrokes("shift-a");
    vcx.run_until_parked();
    assert_eq!(style(&mut vcx), (GroupStyle::Stack { peak: true }, 3));
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    window
        .update(&mut vcx, |ws, window, cx| {
            assert_eq!(ws.app.undo_label(), Some("Stack Group 1"));
            let panel = ws.app.panels.focused_id();
            ws.dispatch(Command::OpenSignalMenu(panel), Some(window), cx);
        })
        .unwrap();
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    window
        .update(&mut vcx, |ws, window, cx| {
            assert!(ws.wave_menu.is_some(), "the popup is hosted");
            let menu = ws.app.panels.focused_waves().unwrap().menu.clone().unwrap();
            let stacked = menu
                .items()
                .find(|i| i.action == MenuAction::Stack(true))
                .unwrap();
            assert_eq!(
                (stacked.label.as_str(), stacked.checked),
                ("Stacked area", true)
            );
            assert!(
                menu.entries
                    .iter()
                    .any(|e| matches!(e, MenuEntry::Label(l) if l == "Draw"))
            );
            let peak = menu.items().find(|i| i.label == "Peak of total").unwrap();
            assert_eq!(
                (&peak.action, peak.checked),
                (&MenuAction::Peak(false), true)
            );
            // The popup answers a choice with the command it carries.
            let panel = ws.app.panels.focused_id();
            ws.dispatch(
                Command::MenuSelect(panel, MenuAction::Stack(false)),
                Some(window),
                cx,
            );
        })
        .unwrap();
    vcx.run_until_parked();
    assert_eq!(style(&mut vcx), (GroupStyle::Activity, 1));
    let labels = window
        .update(&mut vcx, |ws, _, _| {
            crate::palette::commands(&ws.app, "stacked")
                .into_iter()
                .map(|(label, _)| label)
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert!(
        labels.iter().any(|l| l == "Toggle Stacked Area"),
        "{labels:?}"
    );
}

/// Groups from the keyboard: `G` groups the selection and opens the name
/// editor over the group, typing and Enter rename it, `F2` and Escape leave
/// the name, `Alt+←`/`Alt+→` fold and unfold, and `Shift+G` dissolves it.
#[gpui_kit::test]
fn group_keys_and_the_hosted_name_editor(cx: &mut TestAppContext) {
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2, 3])), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let outline = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                ws.app
                    .panels
                    .focused_waves()
                    .unwrap()
                    .items()
                    .iter()
                    .map(|e| {
                        let mark = match e.group() {
                            Some(g) if g.collapsed => "+",
                            Some(_) => "-",
                            None => "",
                        };
                        format!("{}{mark}{}", "  ".repeat(usize::from(e.depth)), e.name())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    let names: Vec<String> = outline(&mut vcx);
    window
        .update(&mut vcx, |ws, _, _| {
            let w = ws.app.panels.focused_waves_mut().unwrap();
            w.selected = [1, 2].into();
            w.anchor = Some(1);
        })
        .unwrap();
    vcx.simulate_keystrokes("g");
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let editing = window
        .update(&mut vcx, |ws, window, cx| {
            let hosted = ws.rename.as_ref().expect("the name editor is hosted");
            let focused = hosted.input.read(cx).focus_handle(cx).is_focused(window);
            (hosted.input.read(cx).text().to_owned(), focused)
        })
        .unwrap();
    assert_eq!(editing, ("Group 1".to_owned(), true));
    assert!(
        window
            .update(&mut vcx, |ws, _, cx| ws
                .rename
                .as_ref()
                .unwrap()
                .input
                .read(cx)
                .accent_color())
            .unwrap()
            .is_none(),
        "a group's name field keeps the input's outline"
    );
    // The name starts selected, so typing replaces it.
    vcx.simulate_keystrokes("a x i enter");
    vcx.run_until_parked();
    assert_eq!(
        outline(&mut vcx),
        [
            names[0].clone(),
            "-axi".into(),
            format!("  {}", names[1]),
            format!("  {}", names[2]),
            names[3].clone()
        ]
    );
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.rename.is_none())
            .unwrap()
    );
    // The keys are the panel's again: F2 reopens the editor, Escape keeps the name.
    vcx.simulate_keystrokes("f2");
    vcx.run_until_parked();
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.rename.is_some())
            .unwrap()
    );
    vcx.simulate_keystrokes("z escape");
    vcx.run_until_parked();
    assert!(
        window
            .update(&mut vcx, |ws, _, _| ws.rename.is_none())
            .unwrap()
    );
    assert_eq!(outline(&mut vcx)[1], "-axi");
    // After an arrow key the name is no longer selected and typing appends.
    vcx.simulate_keystrokes("f2 right 2 enter");
    vcx.run_until_parked();
    assert_eq!(outline(&mut vcx)[1], "-axi2");
    // Enter alone keeps the selected name.
    vcx.simulate_keystrokes("f2 enter");
    vcx.run_until_parked();
    assert_eq!(outline(&mut vcx)[1], "-axi2");
    // A double-click on the group's name opens the editor with the keys.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let at = window
        .update(&mut vcx, |ws, _, _| {
            let w = ws.app.panels.focused_waves().unwrap();
            let l = w.last_layout();
            let (y, _) = l.entry_span(1).unwrap();
            // The middle of the name, clear of the chevron and the sidebar's sash.
            let on_name: Vec<_> = (0..400)
                .map(|dx| volna_core::geometry::point(l.names.left() + dx as f32, y + 4.0))
                .filter(|&p| w.group_name_at(p) == Some(1))
                .collect();
            on_name[on_name.len() / 2]
        })
        .unwrap();
    let at = gpui_kit::point(gpui_kit::px(at.x), gpui_kit::px(at.y));
    vcx.simulate_mouse_move(at, None, gpui_kit::Modifiers::default());
    for click_count in [1, 2] {
        vcx.simulate_event(gpui_kit::MouseDownEvent {
            button: gpui_kit::MouseButton::Left,
            position: at,
            modifiers: gpui_kit::Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        vcx.simulate_event(gpui_kit::MouseUpEvent {
            button: gpui_kit::MouseButton::Left,
            position: at,
            modifiers: gpui_kit::Modifiers::default(),
            click_count,
        });
    }
    vcx.run_until_parked();
    vcx.simulate_keystrokes("b u s enter");
    vcx.run_until_parked();
    assert_eq!(outline(&mut vcx)[1], "-bus");
    vcx.simulate_keystrokes("alt-left");
    vcx.run_until_parked();
    assert_eq!(outline(&mut vcx)[1], "+bus");
    vcx.simulate_keystrokes("alt-right");
    vcx.run_until_parked();
    assert_eq!(outline(&mut vcx)[1], "-bus");
    vcx.simulate_keystrokes("shift-g");
    vcx.run_until_parked();
    assert_eq!(outline(&mut vcx), names);
}

/// A wave row becomes a tree item with its level, selection and, for a
/// group, its expanded state.
#[test]
fn wave_rows_are_tree_items() {
    use gpui_kit::accesskit::Role;
    use volna_core::geometry::Rect;
    use volna_core::wave::model::AccessibleRow;
    let row = AccessibleRow {
        entry: 3,
        label: "Write, 7 rows".into(),
        level: 2,
        expanded: Some(false),
        selected: true,
        bounds: Rect::from_xywh(0.0, 48.0, 220.0, 24.0),
    };
    let node = crate::canvas::wave_row_node(&row, 2.0);
    assert_eq!(node.role(), Role::TreeItem);
    assert_eq!(node.level(), Some(2));
    assert_eq!(node.is_expanded(), Some(false));
    assert_eq!(node.is_selected(), Some(true));
    assert_eq!(node.label(), Some("Write, 7 rows"));
    assert_eq!(node.bounds().map(|b| b.y1), Some(144.0));
    let leaf = crate::canvas::wave_row_node(
        &AccessibleRow {
            expanded: None,
            selected: false,
            ..row
        },
        1.0,
    );
    assert_eq!(leaf.is_expanded(), None);
}

/// Undo keys reach the history from the panels; in the group name editor
/// and the filter box they edit the text instead. The Edit menu and the
/// palette name the steps (`volna/volna/ARCHITECTURE.md`, "Undo and redo").
#[gpui_kit::test]
fn undo_keys_reach_the_history_and_text_fields_keep_their_own(cx: &mut TestAppContext) {
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0, 1, 2, 3])), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let rows = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                ws.app
                    .panels
                    .focused_waves()
                    .unwrap()
                    .items()
                    .iter()
                    .map(|e| e.name().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    let (undo, redo) = if cfg!(target_os = "macos") {
        ("cmd-z", "cmd-shift-z")
    } else {
        ("ctrl-z", "ctrl-shift-z")
    };
    let all = rows(&mut vcx);
    window
        .update(&mut vcx, |ws, _, _| {
            let w = ws.app.panels.focused_waves_mut().unwrap();
            w.selected = [1].into();
            w.anchor = Some(1);
        })
        .unwrap();
    vcx.simulate_keystrokes("delete");
    vcx.run_until_parked();
    assert_eq!(rows(&mut vcx).len(), 3);

    // The Edit menu and the palette name the step.
    let edit_menu = |vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            let menus = cx.get_menus().expect("the application menu");
            let edit = menus
                .iter()
                .find(|m| m.name == "Edit")
                .expect("an Edit menu");
            edit.items
                .iter()
                .filter_map(|item| match item {
                    gpui_kit::OwnedMenuItem::Action { name, disabled, .. } => {
                        Some((name.clone(), *disabled))
                    }
                    _ => None,
                })
                .take(2)
                .collect::<Vec<_>>()
        })
    };
    let removed = window
        .update(&mut vcx, |ws, _, _| ws.app.undo_label().unwrap().to_owned())
        .unwrap();
    assert_eq!(
        edit_menu(&mut vcx),
        [(format!("Undo {removed}"), false), ("Redo".into(), true)]
    );
    let labels = window
        .update(&mut vcx, |ws, _, _| {
            crate::palette::commands(&ws.app, "")
                .into_iter()
                .map(|(label, _)| label)
                .take(2)
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(labels, [format!("Undo: {removed}"), "Redo".to_owned()]);

    vcx.simulate_keystrokes(undo);
    vcx.run_until_parked();
    assert_eq!(rows(&mut vcx), all);
    assert_eq!(
        edit_menu(&mut vcx),
        [
            ("Undo Add 4 signals".into(), false),
            (format!("Redo {removed}"), false)
        ]
    );
    vcx.simulate_keystrokes(redo);
    vcx.run_until_parked();
    assert_eq!(rows(&mut vcx).len(), 3);
    vcx.simulate_keystrokes(undo);
    vcx.run_until_parked();
    if !cfg!(target_os = "macos") {
        vcx.simulate_keystrokes("ctrl-y");
        vcx.run_until_parked();
        assert_eq!(rows(&mut vcx).len(), 3, "Ctrl+Y redoes");
        vcx.simulate_keystrokes(undo);
        vcx.run_until_parked();
    }
    assert_eq!(rows(&mut vcx), all);

    // In the group name editor, undo edits the name.
    window
        .update(&mut vcx, |ws, _, _| {
            let w = ws.app.panels.focused_waves_mut().unwrap();
            w.selected = [1, 2].into();
            w.anchor = Some(1);
        })
        .unwrap();
    vcx.simulate_keystrokes("g");
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let steps = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| ws.app.history.undo_steps().count())
            .unwrap()
    };
    let grouped = steps(&mut vcx);
    let name = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, cx| {
                ws.rename
                    .as_ref()
                    .map(|r| r.input.read(cx).text().to_owned())
            })
            .unwrap()
    };
    vcx.simulate_keystrokes("a x");
    vcx.run_until_parked();
    assert_eq!(name(&mut vcx).as_deref(), Some("ax"));
    vcx.simulate_keystrokes(undo);
    vcx.run_until_parked();
    assert_eq!(
        name(&mut vcx).as_deref(),
        Some("a"),
        "typing undoes as a run"
    );
    vcx.simulate_keystrokes(undo);
    vcx.run_until_parked();
    assert_eq!(name(&mut vcx).as_deref(), Some("Group 1"));
    vcx.simulate_keystrokes(redo);
    vcx.run_until_parked();
    assert_eq!(name(&mut vcx).as_deref(), Some("a"));
    assert_eq!(steps(&mut vcx), grouped, "the cockpit history is untouched");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(steps(&mut vcx), grouped, "naming joins the group's step");
    assert_eq!(rows(&mut vcx)[1], "a");

    // In the filter box too; the panel's undo then takes the group back.
    window
        .update(&mut vcx, |ws, window, cx| {
            let handle = ws.filter.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        })
        .unwrap();
    vcx.simulate_keystrokes("c l k");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("backspace");
    vcx.run_until_parked();
    let filter = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| ws.app.variables.filter.clone())
            .unwrap()
    };
    assert_eq!(filter(&mut vcx), "cl");
    vcx.simulate_keystrokes(undo);
    vcx.run_until_parked();
    assert_eq!(filter(&mut vcx), "clk");
    assert_eq!(rows(&mut vcx)[1], "a");
    window
        .update(&mut vcx, |ws, window, cx| {
            window.focus(&ws.waves_focus, cx);
        })
        .unwrap();
    vcx.simulate_keystrokes(undo);
    vcx.run_until_parked();
    assert_eq!(rows(&mut vcx), all);
}

/// `.` `,` `1`–`9` and `` ` `` walk the focused panel's cursor between
/// markers, and the status bar says where it landed.
#[gpui_kit::test]
fn marker_walk_keys_move_the_cursor(cx: &mut TestAppContext) {
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
    let (near, far) = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            let (near, far) = (lo + (hi - lo) / 4, lo + (hi - lo) * 3 / 4);
            for t in [near, far] {
                ws.app.doc.shared.cursor = Some(t);
                ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
            }
            ws.app.doc.shared.cursor = Some(lo);
            (near, far)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let state = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                (ws.app.doc.shared.cursor, ws.app.status().announcement)
            })
            .unwrap()
    };
    vcx.simulate_keystrokes(".");
    assert_eq!(
        state(&mut vcx),
        (Some(near), Some("At marker 1 · ` returns".into()))
    );
    vcx.simulate_keystrokes("2");
    assert_eq!(state(&mut vcx).0, Some(far));
    vcx.simulate_keystrokes(",");
    assert_eq!(state(&mut vcx).0, Some(near));
    vcx.simulate_keystrokes("`");
    assert_eq!(state(&mut vcx).0, Some(far));
    vcx.simulate_keystrokes("9");
    assert_eq!(state(&mut vcx), (Some(far), Some("No marker 9".into())));
}

/// `M` on a marker, or a double-click on its chip, opens a name field over
/// the chip; the field keeps the keys the panel binds, and `↵` names it.
#[gpui_kit::test]
fn marker_name_field_opens_over_the_chip_and_keeps_its_keys(cx: &mut TestAppContext) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, VisualTestContext, point,
    };
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
    let (near, far) = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            let (near, far) = (lo + (hi - lo) / 4, lo + (hi - lo) * 3 / 4);
            for t in [near, far] {
                ws.app.doc.shared.cursor = Some(t);
                ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
            }
            ws.app.doc.shared.cursor = Some(near);
            (near, far)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    let draw = |vcx: &mut VisualTestContext| {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    };
    draw(&mut vcx);
    let names = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                let m = ws.app.doc.markers();
                (
                    m[0].label.clone(),
                    m[1].label.clone(),
                    ws.app.doc.shared.cursor,
                )
            })
            .unwrap()
    };

    vcx.simulate_keystrokes("m");
    draw(&mut vcx);
    let field = vcx
        .debug_bounds("marker-name")
        .expect("the name field is laid over the chip");
    let chip = window
        .update(&mut vcx, |ws, _, _| {
            ws.app.panels.focused().kind.marker_lane().unwrap().chips[0].rect
        })
        .unwrap();
    assert!(
        f32::from(field.origin.x) > chip.left() && f32::from(field.origin.y) >= chip.top() - 4.0
    );
    // Outlined in marker 1's chip colour.
    let (accent, chip) = window
        .update(&mut vcx, |ws, _, cx| {
            let chip = crate::theme::core_theme(cx)
                .marker(0)
                .background
                .with_alpha(1.0);
            (
                ws.rename.as_ref().unwrap().input.read(cx).accent_color(),
                crate::theme::hsla(chip),
            )
        })
        .unwrap();
    assert_eq!(accent, Some(chip));
    // `2` and `.` walk markers in the panel, but type into the field.
    vcx.simulate_keystrokes("r e q space 2 . enter");
    draw(&mut vcx);
    assert_eq!(names(&mut vcx), (Some("req 2.".into()), None, Some(near)));
    assert!(
        vcx.debug_bounds("marker-name").is_none(),
        "Enter closes the field"
    );

    // A double-click on marker 2's chip opens its field; Escape cancels.
    let chip = window
        .update(&mut vcx, |ws, _, _| {
            ws.app.panels.focused().kind.marker_lane().unwrap().chips[1].rect
        })
        .unwrap();
    let at = point(
        gpui_kit::px(chip.left() + 3.0),
        gpui_kit::px(chip.top() + 3.0),
    );
    for click_count in [1, 2] {
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
        });
    }
    draw(&mut vcx);
    assert!(vcx.debug_bounds("marker-name").is_some());
    let focused = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, window, cx| {
                ws.rename
                    .as_ref()
                    .is_some_and(|h| h.input.read(cx).focus_handle(cx).is_focused(window))
            })
            .unwrap()
    };
    assert!(
        focused(&mut vcx),
        "the field has the keys after a double-click"
    );
    vcx.simulate_keystrokes("x escape");
    draw(&mut vcx);
    assert_eq!(names(&mut vcx), (Some("req 2.".into()), None, Some(far)));
    assert!(vcx.debug_bounds("marker-name").is_none());
    // Double-click again and type a name.
    for click_count in [1, 2] {
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
        });
    }
    draw(&mut vcx);
    vcx.simulate_keystrokes("e n d enter");
    draw(&mut vcx);
    assert_eq!(
        names(&mut vcx),
        (Some("req 2.".into()), Some("end".into()), Some(far))
    );
}

/// A double-click on the span between two markers zooms the panel to them.
#[gpui_kit::test]
fn double_click_on_a_span_zooms_to_it(cx: &mut TestAppContext) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, VisualTestContext, point,
    };
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
    let (a, b) = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            let (a, b) = (lo + (hi - lo) * 2 / 5, lo + (hi - lo) / 2);
            for t in [a, b] {
                ws.app.doc.shared.cursor = Some(t);
                ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
            }
            (a, b)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let (x, y) = window
        .update(&mut vcx, |ws, _, _| {
            let lane = ws.app.panels.focused().kind.marker_lane().unwrap().clone();
            let span = lane.spans.iter().find(|s| s.first == 0).expect("a span");
            let (x0, x1) = lane.shown(span);
            ((x0 + x1) / 2.0, lane.band.top() + lane.band.height() / 2.0)
        })
        .unwrap();
    let at = point(gpui_kit::px(x), gpui_kit::px(y));
    for click_count in [1, 2] {
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
        });
    }
    vcx.run_until_parked();
    let v = window
        .update(&mut vcx, |ws, _, _| ws.app.doc.shared.viewport.target())
        .unwrap();
    let (a, b) = (a as f64, b as f64);
    assert!(v.start < a && v.end > b, "{v:?}");
    assert!(v.width() < (b - a) * 1.5, "zoomed in to the span: {v:?}");
}

/// `R` measures from the cursor, `⇧R` stops, `Z` zooms to the measurement,
/// a double-click on the live span zooms to it too, and the Measure lane's
/// `×` clears the reference.
#[gpui_kit::test]
fn reference_keys_and_a_double_click_on_the_live_span(cx: &mut TestAppContext) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, VisualTestContext, point,
    };
    use volna_core::marker::Reference;
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
    let (a, b) = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            let (a, b) = (lo + (hi - lo) * 2 / 5, lo + (hi - lo) / 2);
            ws.app.doc.shared.cursor = Some(a);
            (a, b)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let reference = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| ws.app.doc.reference())
            .unwrap()
    };
    vcx.simulate_keystrokes("r");
    assert_eq!(reference(&mut vcx), Some(Reference::Time(a)));
    vcx.simulate_keystrokes("shift-r");
    assert_eq!(reference(&mut vcx), None);
    vcx.simulate_keystrokes("r");
    window
        .update(&mut vcx, |ws, _, _| ws.app.doc.shared.cursor = Some(b))
        .unwrap();
    vcx.simulate_keystrokes("z");
    let v = window
        .update(&mut vcx, |ws, _, _| ws.app.doc.shared.viewport.target())
        .unwrap();
    let (fa, fb) = (a as f64, b as f64);
    assert!(
        v.start < fa && v.end > fb && v.width() < (fb - fa) * 1.5,
        "{v:?}"
    );

    // Zoom out again, then double-click the live span.
    vcx.simulate_keystrokes("f");
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let (x, y) = window
        .update(&mut vcx, |ws, _, _| {
            let lane = ws.app.panels.focused().kind.marker_lane().unwrap().clone();
            let m = lane.measure.clone().expect("the Measure lane");
            let (x0, x1) = lane.live_shown(m.live.as_ref().expect("the live span"));
            ((x0 + x1) / 2.0, m.band.top() + m.band.height() / 2.0)
        })
        .unwrap();
    let at = point(gpui_kit::px(x), gpui_kit::px(y));
    for click_count in [1, 2] {
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count,
        });
    }
    vcx.run_until_parked();
    let (v, cursor) = window
        .update(&mut vcx, |ws, _, _| {
            (
                ws.app.doc.shared.viewport.target(),
                ws.app.doc.shared.cursor,
            )
        })
        .unwrap();
    assert_eq!(cursor, Some(b), "a press on the live span keeps the cursor");
    assert!(
        v.start < fa && v.end > fb && v.width() < (fb - fa) * 1.5,
        "{v:?}"
    );
    // A click on the Measure lane's × clears the reference.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let clear = window
        .update(&mut vcx, |ws, _, _| {
            let lane = ws.app.panels.focused().kind.marker_lane().unwrap().clone();
            lane.measure.expect("the Measure lane").clear
        })
        .unwrap();
    let at = point(
        gpui_kit::px(clear.left() + clear.width() / 2.0),
        gpui_kit::px(clear.top() + clear.height() / 2.0),
    );
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    vcx.run_until_parked();
    assert_eq!(reference(&mut vcx), None);
}

/// A marker chip drags with the mouse as one step, a right-click hosts its
/// menu, *Copy as Text* reaches the clipboard, and a double-click on the
/// time header adds a marker.
#[gpui_kit::test]
fn marker_chips_drag_open_menus_copy_and_double_click_adds(cx: &mut TestAppContext) {
    use gpui_kit::{
        Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, VisualTestContext,
        point,
    };
    use volna_core::marker::LaneVerb;
    use volna_core::wave::model::MenuAction;
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
    let a = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            let a = lo + (hi - lo) / 4;
            ws.app.doc.shared.cursor = Some(a);
            ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
            a
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let chip = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                let lane = ws.app.panels.focused().kind.marker_lane().unwrap().clone();
                let r = lane.chips[0].rect;
                (r.left() + 3.0, r.top() + r.height() / 2.0)
            })
            .unwrap()
    };
    let (x, y) = chip(&mut vcx);
    let at = |x: f32, y: f32| point(gpui_kit::px(x), gpui_kit::px(y));
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: at(x, y),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    for dx in [30.0, 90.0] {
        vcx.simulate_event(MouseMoveEvent {
            position: at(x + dx, y),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
    }
    vcx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: at(x + 90.0, y),
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    vcx.run_until_parked();
    let (moved, label) = window
        .update(&mut vcx, |ws, _, _| {
            (
                ws.app.doc.markers()[0].time,
                ws.app.undo_label().map(str::to_owned),
            )
        })
        .unwrap();
    assert!(moved > a, "{moved} > {a}");
    assert_eq!(label.as_deref(), Some("Move marker 1"));

    // A right-click on the chip hosts its menu; Copy as Text fills the clipboard.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let (x, y) = chip(&mut vcx);
    vcx.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: at(x, y),
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    vcx.simulate_event(MouseUpEvent {
        button: MouseButton::Right,
        position: at(x, y),
        modifiers: Modifiers::default(),
        click_count: 1,
    });
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, window, cx| {
            assert!(ws.wave_menu.is_some(), "the lane menu is hosted");
            let panel = ws.app.panels.focused_id();
            let id = ws.app.doc.markers()[0].id;
            ws.dispatch(
                Command::MenuSelect(panel, MenuAction::Lane(LaneVerb::Copy(id))),
                Some(window),
                cx,
            );
        })
        .unwrap();
    vcx.run_until_parked();
    let copied = vcx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(copied.contains(" marker 1 at "), "{copied}");

    // A double-click on the time header marks where it lands.
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let (hx, hy) = window
        .update(&mut vcx, |ws, _, _| {
            let l = ws.app.panels.focused_waves().unwrap().last_layout();
            (l.waves.left() + l.waves.width() * 0.8, l.header.top() + 6.0)
        })
        .unwrap();
    for click_count in [1, 2] {
        vcx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: at(hx, hy),
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: at(hx, hy),
            modifiers: Modifiers::default(),
            click_count,
        });
    }
    vcx.run_until_parked();
    let (count, cursor, last) = window
        .update(&mut vcx, |ws, _, _| {
            (
                ws.app.doc.markers().len(),
                ws.app.doc.shared.cursor,
                ws.app.doc.markers().last().map(|m| m.time),
            )
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(last, cursor);
}

/// `'` opens the palette's marker mode: rows in time order, filtered by
/// typing, where `⇧↵` measures from the highlighted marker, `Del` removes it
/// and keeps the list, `F2` renames it and `↵` goes to it.
#[gpui_kit::test]
fn the_marker_navigator_lists_filters_and_acts_on_markers(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::marker::Reference;
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
    let (start, times) = window
        .update(cx, |ws, window, cx| {
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            let times = [
                lo + (hi - lo) / 5,
                lo + (hi - lo) * 2 / 5,
                lo + (hi - lo) * 3 / 5,
            ];
            for (t, name) in times.iter().zip(["req A", "resp A", "req B"]) {
                ws.app.doc.shared.cursor = Some(*t);
                ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
                let id = ws
                    .app
                    .doc
                    .markers()
                    .iter()
                    .find(|m| m.time == *t)
                    .unwrap()
                    .id;
                ws.app.doc.rename_marker(id, name);
            }
            ws.app.doc.shared.cursor = Some(lo);
            (lo, times)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let shown = |vcx: &mut VisualTestContext| {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        [
            vcx.debug_bounds("marker-row-1").is_some(),
            vcx.debug_bounds("marker-row-2").is_some(),
            vcx.debug_bounds("marker-row-3").is_some(),
        ]
    };
    assert_eq!(shown(&mut vcx), [false; 3]);
    vcx.simulate_keystrokes("'");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [true; 3], "every marker is listed");
    let rows = |vcx: &mut VisualTestContext| {
        vcx.debug_bounds("marker-row-1")
            .zip(vcx.debug_bounds("marker-row-3"))
    };
    let (one, three) = rows(&mut vcx).unwrap();
    assert!(one.top() < three.top(), "in time order");

    // Typing filters by name words. With nothing highlighted, Del edits the
    // query as usual: removing the stray "x" brings “req B” back.
    vcx.simulate_input(" req bx");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [false; 3]);
    vcx.simulate_keystrokes("left delete");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [false, false, true]);
    // ⇧↵ measures from the match without moving the cursor.
    vcx.simulate_keystrokes("shift-enter");
    vcx.run_until_parked();
    let (reference, cursor) = window
        .update(&mut vcx, |ws, _, _| {
            (ws.app.doc.reference(), ws.app.doc.shared.cursor)
        })
        .unwrap();
    assert_eq!(
        reference,
        Some(Reference::Marker(
            volna_core::marker::MarkerId::new(3).unwrap()
        ))
    );
    assert_eq!(cursor, Some(start), "no jump");
    assert_eq!(shown(&mut vcx), [false; 3], "the palette closed");

    // Del removes the highlighted marker and keeps the list open.
    vcx.simulate_keystrokes("'");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("delete");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [false, true, true]);
    let label = window
        .update(&mut vcx, |ws, _, _| ws.app.undo_label().map(str::to_owned))
        .unwrap();
    assert_eq!(label.as_deref(), Some("Remove marker 1"));

    // F2 opens the highlighted marker's name field.
    vcx.simulate_keystrokes("f2");
    vcx.run_until_parked();
    let edit = window
        .update(&mut vcx, |ws, _, _| ws.app.text_edit().map(|e| e.text))
        .unwrap();
    assert_eq!(edit.as_deref(), Some("resp A"));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();

    // ↵ goes to a marker found by number.
    vcx.simulate_keystrokes("'");
    vcx.run_until_parked();
    vcx.simulate_input("3");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let cursor = window
        .update(&mut vcx, |ws, _, _| ws.app.doc.shared.cursor)
        .unwrap();
    assert_eq!(cursor, Some(times[2]));
}

/// Typing "mar…" in the palette shows markers under the commands, filtered
/// by the other words, and *Find Marker… (@)* switches the open palette to
/// the marker mode.
#[gpui_kit::test]
fn the_palette_offers_markers_when_the_query_asks_for_them(cx: &mut TestAppContext) {
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            for (k, name) in [(1, "req A"), (2, "resp A"), (3, "req B")] {
                let t = lo + (hi - lo) * k / 5;
                ws.app.doc.shared.cursor = Some(t);
                ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
                let id = ws
                    .app
                    .doc
                    .markers()
                    .iter()
                    .find(|m| m.time == t)
                    .unwrap()
                    .id;
                ws.app.doc.rename_marker(id, name);
            }
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, window, cx| ws.open_palette(window, cx))
        .unwrap();
    vcx.run_until_parked();
    let shown = |vcx: &mut VisualTestContext| {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        [
            vcx.debug_bounds("marker-row-1").is_some(),
            vcx.debug_bounds("marker-row-2").is_some(),
            vcx.debug_bounds("marker-row-3").is_some(),
        ]
    };
    assert_eq!(
        shown(&mut vcx),
        [false; 3],
        "an empty query lists no markers"
    );
    vcx.simulate_input("mar");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [true; 3]);
    vcx.simulate_input(" req");
    vcx.run_until_parked();
    assert_eq!(
        shown(&mut vcx),
        [true, false, true],
        "the other words filter"
    );

    // Find Marker… switches to @ in place: the palette stays open with all.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    vcx.simulate_input("find marker");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [true; 3]);
    // Still the palette: ↵ goes to the first marker and closes it.
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(shown(&mut vcx), [false; 3]);
    let at = window
        .update(&mut vcx, |ws, _, _| {
            (ws.app.doc.shared.cursor, ws.app.doc.markers()[0].time)
        })
        .unwrap();
    assert_eq!(at.0, Some(at.1));
}

/// A command chosen in the palette with the keyboard runs in the panel
/// that had focus before the palette opened.
#[gpui_kit::test]
fn palette_commands_chosen_by_keyboard_reach_the_panel(cx: &mut TestAppContext) {
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.dispatch(Command::AddVars(a_all(vec![0])), Some(window), cx);
            let (lo, hi) = ws.app.doc.limits();
            for t in [lo + (hi - lo) / 4, lo + (hi - lo) / 2] {
                ws.app.doc.shared.cursor = Some(t);
                ws.dispatch(Command::Action(Action::AddOrRenameMarker), Some(window), cx);
            }
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    let markers = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| ws.app.doc.markers().len())
            .unwrap()
    };
    assert_eq!(markers(&mut vcx), 2);
    window
        .update(&mut vcx, |ws, window, cx| ws.open_palette(window, cx))
        .unwrap();
    vcx.run_until_parked();
    vcx.simulate_input("remove all markers");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(markers(&mut vcx), 0, "Remove All Markers ran");

    // A command that opens a name field leaves the keys in it once the
    // palette has closed.
    vcx.simulate_keystrokes("m");
    vcx.run_until_parked();
    assert_eq!(markers(&mut vcx), 1);
    window
        .update(&mut vcx, |ws, window, cx| ws.open_palette(window, cx))
        .unwrap();
    vcx.run_until_parked();
    vcx.simulate_input("add or name marker");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    vcx.simulate_input("irq");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let name = window
        .update(&mut vcx, |ws, _, _| ws.app.doc.markers()[0].label.clone())
        .unwrap();
    assert_eq!(name.as_deref(), Some("irq"));
}

/// The start page lists what was opened, newest first; the keys reopen it,
/// a missing file stays listed, and a workspace is listed as itself
/// (`volna/volna/ARCHITECTURE.md`, "Workspace persistence").
#[gpui_kit::test]
fn start_page_reopens_recent_traces_and_workspaces(cx: &mut TestAppContext) {
    use crate::native_workspace::{Store, file_uri};
    use gpui_kit::VisualTestContext;
    use volna_core::workspace::persistence::{Persistence, Target};
    use volna_core::workspace::recent::RecentKind;
    init(cx);
    let temporary = tempfile::tempdir().unwrap();
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    let (a, b) = (
        temporary.path().join("a.vtr"),
        temporary.path().join("b.vtr"),
    );
    std::fs::copy(example, &a).unwrap();
    std::fs::copy(example, &b).unwrap();
    let config = temporary.path().join("config");
    let mut store = Store::new(Some(config.clone())).unwrap();
    store.data_dir = temporary.path().join("fallback");
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
    let draw = |vcx: &mut VisualTestContext| {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    };
    let names = |vcx: &mut VisualTestContext| {
        window
            .update(vcx, |ws, _, _| {
                ws.app
                    .recent_rows(0, None)
                    .into_iter()
                    .map(|r| (r.name, r.missing))
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    window
        .update(&mut vcx, |ws, _, cx| {
            ws.enable_native_persistence(store, Persistence::Auto, false, cx);
            ws.open_path(a.clone(), cx);
        })
        .unwrap();
    draw(&mut vcx);
    window
        .update(&mut vcx, |ws, _, cx| ws.open_path(b.clone(), cx))
        .unwrap();
    draw(&mut vcx);
    std::fs::remove_file(&a).unwrap();
    window
        .update(&mut vcx, |ws, window, cx| {
            assert!(ws.app.doc.is_loaded());
            ws.dispatch(Command::CloseTrace, Some(window), cx)
        })
        .unwrap();
    draw(&mut vcx);
    assert_eq!(
        names(&mut vcx),
        [("b.vtr".to_owned(), false), ("a.vtr".to_owned(), true)]
    );
    let saved = std::fs::read_to_string(config.join("state.json")).unwrap();
    assert!(saved.find("b.vtr").unwrap() < saved.find("a.vtr").unwrap());
    assert!(vcx.debug_bounds("recent-0").is_some());
    assert!(vcx.debug_bounds("recent-1").is_some());
    assert!(vcx.debug_bounds("recent-2").is_none());
    let labels = window
        .update(&mut vcx, |ws, _, _| {
            crate::palette::commands(&ws.app, "")
                .into_iter()
                .map(|(label, _)| label)
                .filter(|label| label.contains("Recent"))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(labels.len(), 3, "{labels:?}");
    assert!(labels[0].starts_with("Open Recent: b.vtr — "), "{labels:?}");

    // The missing file: the usual error, and the entry stays.
    vcx.simulate_keystrokes("down enter");
    draw(&mut vcx);
    window
        .update(&mut vcx, |ws, _, _| {
            assert!(matches!(ws.app.trace_state(), TraceState::Error(_)));
        })
        .unwrap();
    assert_eq!(names(&mut vcx).len(), 2);

    // Enter on the newest entry reopens it.
    vcx.simulate_keystrokes("home enter");
    draw(&mut vcx);
    let workspace_file = temporary.path().join("debug.volna.json");
    window
        .update(&mut vcx, |ws, _, cx| {
            assert!(ws.app.doc.is_loaded());
            assert!(
                ws.app
                    .workspace
                    .trace_uri
                    .as_deref()
                    .unwrap()
                    .ends_with("/b.vtr")
            );
            ws.app.save_workspace(Some(Target::File {
                uri: file_uri(&workspace_file).unwrap(),
            }));
            ws.after(None, cx);
        })
        .unwrap();
    draw(&mut vcx);
    assert!(workspace_file.exists());

    // A workspace opened by path is listed as itself, naming its trace.
    window
        .update(&mut vcx, |ws, window, cx| {
            ws.dispatch(Command::CloseTrace, Some(window), cx);
            ws.open_workspace_path(workspace_file.clone(), cx);
        })
        .unwrap();
    draw(&mut vcx);
    window
        .update(&mut vcx, |ws, window, cx| {
            assert!(ws.app.doc.is_loaded());
            let first = &ws.app.workspace.state.recent[0];
            assert_eq!(first.kind, RecentKind::Workspace);
            assert!(first.trace.as_deref().unwrap().ends_with("/b.vtr"));
            assert_eq!(ws.app.workspace.state.recent.len(), 3);
            ws.dispatch(Command::CloseTrace, Some(window), cx);
        })
        .unwrap();
    draw(&mut vcx);
    assert_eq!(names(&mut vcx)[0].0, "debug");

    // Digits open by position; Delete removes the selected entry.
    vcx.simulate_keystrokes("3");
    draw(&mut vcx);
    assert!(
        window
            .update(&mut vcx, |ws, _, _| matches!(
                ws.app.trace_state(),
                TraceState::Error(_)
            ))
            .unwrap()
    );
    vcx.simulate_keystrokes("end delete");
    draw(&mut vcx);
    assert_eq!(
        names(&mut vcx)
            .into_iter()
            .map(|(n, _)| n)
            .collect::<Vec<_>>(),
        ["debug", "b.vtr"]
    );
    let saved = std::fs::read_to_string(config.join("state.json")).unwrap();
    assert!(!saved.contains("a.vtr"));
}

/// Two traces side by side (`docs/multiple-traces.html`, stage 1): the
/// title bar shows a chip per trace, the scope tree heads each trace's
/// scopes with its own row, a chip click selects that row, the command
/// line's second file joins the first, and a pipeline tab names its trace.
#[gpui_kit::test]
fn trace_chips_and_rows_show_every_open_trace(cx: &mut TestAppContext) {
    use gpui_kit::Modifiers;
    use gpui_kit::VisualTestContext;
    use volna_core::sidebar::TreeNode;
    init(cx);
    let temporary = tempfile::tempdir().unwrap();
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/landing.vtr");
    let (a, b) = (
        temporary.path().join("cpu.vtr"),
        temporary.path().join("dram.vtr"),
    );
    std::fs::copy(example, &a).unwrap();
    std::fs::copy(example, &b).unwrap();
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
        .update(cx, |ws, _, cx| {
            // This test needs both trace rows in view; build banners have their own test.
            ws.app.handle(Command::Settings(SettingsCommand::Set {
                id: "hierarchy.activityIndex".into(),
                value: volna_core::settings::Value::Text("never".into()),
            }));
            ws.open_paths(vec![a.clone(), b.clone()], cx)
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    let draw = |vcx: &mut VisualTestContext| {
        vcx.run_until_parked();
        vcx.update(|window, cx| window.draw(cx).clear(cx));
    };
    draw(&mut vcx);
    let b_id = TraceId::from_letter('B').unwrap();
    window
        .update(&mut vcx, |ws, _, _| {
            let chips = ws.app.trace_chips();
            assert_eq!(
                chips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
                ["cpu.vtr", "dram.vtr"]
            );
            assert!(chips.iter().all(|c| !c.loading));
        })
        .unwrap();
    assert!(vcx.debug_bounds("trace-chip-A").is_some());
    let chip = vcx.debug_bounds("trace-chip-B").expect("B's chip");
    assert!(vcx.debug_bounds("trace-row-A").is_some());
    assert!(vcx.debug_bounds("trace-row-B").is_some());
    vcx.simulate_click(chip.center(), Modifiers::default());
    draw(&mut vcx);
    window
        .update(&mut vcx, |ws, window, cx| {
            assert_eq!(ws.app.scopes.selected, Some(TreeNode::trace(b_id)));
            let track = ws
                .app
                .pipeline_streams()
                .into_iter()
                .find(|(_, t)| t.trace == b_id)
                .expect("B records a pipeline")
                .1;
            ws.dispatch(Command::OpenPipeline { track }, Some(window), cx);
            assert_eq!(ws.app.panels.focused().kind.trace(), Some(b_id));
        })
        .unwrap();
    draw(&mut vcx);
    // The palette names each trace's verbs; A closes only with everything.
    window
        .update(&mut vcx, |ws, _, _| {
            let labels: Vec<String> = crate::palette::commands(&ws.app, "")
                .into_iter()
                .map(|(label, _)| label)
                .filter(|label| label.contains("Trace"))
                .collect();
            for label in [
                "Add Trace…",
                "Reveal Trace A cpu.vtr",
                "Rename Trace B dram.vtr…",
                "Close Trace B dram.vtr",
            ] {
                assert!(labels.iter().any(|l| l == label), "{label}: {labels:?}");
            }
            assert!(!labels.iter().any(|l| l == "Close Trace A cpu.vtr"));
        })
        .unwrap();
    // Closing B leaves A alone, named plainly again.
    window
        .update(&mut vcx, |ws, window, cx| {
            ws.dispatch(Command::RemoveTrace(b_id), Some(window), cx);
            assert_eq!(ws.app.trace_chips().len(), 1);
        })
        .unwrap();
    draw(&mut vcx);
    assert!(vcx.debug_bounds("trace-chip-A").is_none());
    assert!(vcx.debug_bounds("trace-row-A").is_none());
}

/// Volna Mixed is a light window around a Volna Dark wave panel whatever the
/// system says; `volna` follows the system everywhere, waves included.
#[gpui_kit::test]
fn volna_mixed_is_a_light_window_with_dark_waves_whatever_the_system(cx: &mut TestAppContext) {
    init(cx);
    let window = cx.add_window(Workspace::new);
    let trace = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr");
    window
        .update(cx, |ws, _, cx| ws.open_path(trace.into(), cx))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, window, cx| {
            ws.dispatch(Command::AddVars(a_all(0..1)), Some(window), cx)
        })
        .unwrap();
    cx.run_until_parked();
    let dark = volna_core::Theme::volna(true);
    let light = volna_core::Theme::volna(false);
    let workspace = window.root(cx).unwrap();
    let (waves, generation) = workspace.read_with(cx, |ws, _| {
        let waves = ws.app.panels.focused_id();
        assert!(ws.app.panels.waves(waves).is_some());
        (waves, ws.app.doc.generation())
    });
    // The theme the wave panel's canvas paints with.
    let panel_theme = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            crate::canvas::PanelCanvas::new(workspace.clone(), waves, generation).core_theme(cx)
        })
    };
    let set = |cx: &mut TestAppContext, theme: Option<&str>, dark: bool| {
        workspace.update(cx, |ws, cx| {
            if let Some(theme) = theme {
                ws.app
                    .settings_loaded(&format!("{{\"appearance.theme\": \"{theme}\"}}"));
                ws.apply_theme_setting(cx);
            }
            ws.set_system_dark(dark, cx);
        })
    };
    let chrome = |cx: &mut TestAppContext| cx.update(|cx| *crate::theme::core_theme(cx));
    // A light system: light chrome, dark waves.
    set(cx, Some("volna-mixed"), false);
    assert_eq!(chrome(cx).editor.bg, light.editor.bg);
    cx.update(|cx| {
        assert_eq!(
            crate::theme::theme(cx).panel.bg,
            crate::theme::hsla(light.panel.bg)
        );
        assert_eq!(crate::theme::canvas_theme(cx).editor.bg, dark.editor.bg);
    });
    assert_eq!(panel_theme(cx).wave_signal, dark.wave_signal);
    // The system turns dark: nothing changes.
    set(cx, None, true);
    assert_eq!(chrome(cx).editor.bg, light.editor.bg);
    assert_eq!(panel_theme(cx).editor.bg, dark.editor.bg);
    // `volna` follows the system, in the waves too.
    set(cx, Some("volna"), true);
    assert_eq!(chrome(cx).editor.bg, dark.editor.bg);
    set(cx, None, false);
    assert_eq!(panel_theme(cx).editor.bg, light.editor.bg);
    assert_eq!(chrome(cx).editor.bg, light.editor.bg);
}

/// With a trace open the Settings tab sits in the dock; its search box
/// still receives letters the wave keymap binds (`m`, `f`, `s`).
#[gpui_kit::test]
fn settings_search_takes_wave_keys_while_a_trace_is_open(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    use volna_core::app::SettingsCommand;
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
            ws.set_session(Arc::new(ProceduralTrace::new(100)), cx);
            ws.app.settings_loaded("{}\n");
            ws.dispatch(Command::Settings(SettingsCommand::Open), Some(window), cx);
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(root.into(), cx);
    vcx.run_until_parked();
    window
        .update(&mut vcx, |ws, window, cx| {
            ws.dispatch(Command::Settings(SettingsCommand::Open), Some(window), cx);
        })
        .unwrap();
    vcx.run_until_parked();
    let focused = window
        .update(&mut vcx, |ws, window, cx| {
            let id = ws.app.panels.settings_id().unwrap();
            let view = ws.settings_view(id, window, cx);
            view.read(cx).search_focus(cx).is_focused(window)
        })
        .unwrap();
    assert!(focused, "opening Settings focuses its search");
    vcx.simulate_keystrokes("m f s");
    vcx.run_until_parked();
    let (query, markers) = window
        .update(&mut vcx, |ws, _, _| {
            (
                ws.app.settings_view.query.clone(),
                ws.app.doc.markers().len(),
            )
        })
        .unwrap();
    assert_eq!(query, "mfs");
    assert_eq!(markers, 0, "`m` did not drop a marker");
}

#[gpui_kit::test]
fn activity_build_banner_drives_build_dismiss_and_cancel(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, VisualTestContext};
    init(cx);
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("run.vtr");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/examples/picorv32.vtr"),
        &trace,
    )
    .unwrap();
    let window = cx.add_window(Workspace::new);
    window
        .update(cx, |ws, _, cx| ws.open_path(trace.clone(), cx))
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let offer = vcx.debug_bounds("activity-banner-A").expect("build offer");
    let build = vcx.debug_bounds("activity-build-A").expect("Build button");
    let dismiss = vcx
        .debug_bounds("activity-dismiss-A")
        .expect("Not now button");
    assert!(offer.contains(&build.center()) && offer.contains(&dismiss.center()));
    vcx.simulate_click(dismiss.center(), Modifiers::default());
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(vcx.debug_bounds("activity-banner-A").is_none());
    assert!(!trace.with_extension("vtr.index").exists());
    // Reopening offers again; Build installs the index through the executor.
    window
        .update(&mut vcx, |ws, _, cx| ws.open_path(trace.clone(), cx))
        .unwrap();
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let build = vcx
        .debug_bounds("activity-build-A")
        .expect("Build after reopen");
    vcx.simulate_click(build.center(), Modifiers::default());
    vcx.run_until_parked();
    // The build and then the census/classification finish through the load loop.
    for _ in 0..3 {
        vcx.update(|window, cx| window.draw(cx).clear(cx));
        vcx.run_until_parked();
    }
    assert!(trace.with_extension("vtr.index").exists());
    assert!(vcx.debug_bounds("activity-banner-A").is_none());
    window
        .update(&mut vcx, |ws, _, _| {
            assert!(ws.app.doc.session(TraceId::A).unwrap().activity().is_some());
        })
        .unwrap();
    // Hold the queued load to draw and click Cancel deterministically.
    let other = dir.path().join("other.vtr");
    std::fs::copy(&trace, &other).unwrap();
    window
        .update(&mut vcx, |ws, _, cx| ws.open_path(other.clone(), cx))
        .unwrap();
    vcx.run_until_parked();
    let request = window
        .update(&mut vcx, |ws, _, _| {
            ws.app.handle(Command::BuildActivity(TraceId::A));
            ws.app.take_requests().pop().expect("build request")
        })
        .unwrap();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(vcx.debug_bounds("activity-progress-A").is_some());
    let cancel = vcx
        .debug_bounds("activity-cancel-A")
        .expect("Cancel button");
    vcx.simulate_click(cancel.center(), Modifiers::default());
    let result = request.perform();
    window
        .update(&mut vcx, |ws, _, cx| {
            ws.app.deliver(result);
            ws.after(None, cx);
        })
        .unwrap();
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(vcx.debug_bounds("activity-banner-A").is_none());
    assert!(!other.with_extension("vtr.index").exists());
    // Several missing indexes keep the scope tree usable rather than filling its panel.
    let mut paths = Vec::new();
    for i in 0..4 {
        let path = dir.path().join(format!("trace-{i}.vtr"));
        std::fs::copy(&trace, &path).unwrap();
        paths.push(path);
    }
    window
        .update(&mut vcx, |ws, _, cx| ws.open_paths(paths, cx))
        .unwrap();
    vcx.run_until_parked();
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    let panel = vcx.debug_bounds("scopes-panel").expect("scope panel");
    let banners = vcx.debug_bounds("activity-banners").expect("build offers");
    let tree = vcx.debug_bounds("scope-tree").expect("scope tree");
    assert!(banners.size.height <= panel.size.height * 0.4 + gpui_kit::px(1.0));
    assert!(tree.size.height >= panel.size.height * 0.4);
}
