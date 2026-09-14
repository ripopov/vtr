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
    window.update(cx, |ws, _, cx| ws.queue(slow, cx)).unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, _| {
            assert!(
                matches!(ws.app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(s, &current))
            );
            assert!(ws.app.panels.focused_waves().unwrap().items.is_empty());
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
            ws.app.doc.shared.viewport.viewport.start = 20.0;
            ws.app.doc.shared.viewport.viewport.end = 80.0;
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
                assert!(Arc::ptr_eq(w.items[0].history.as_ref().unwrap(), &history));
                assert_eq!(w.names_width, 260.0);
                assert_eq!(w.values_width, 140.0);
                assert_eq!(w.scroll_y, 24.0);
                assert_eq!(ws.app.doc.markers[0].time, 30);
            })
            .unwrap();
    }
}

// Native reader workers run outside GPUI's deterministic executor.
fn wait_native(cx: &mut TestAppContext, mut ready: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if ready(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native worker did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui_kit::test]
fn native_idle_save_reopens_a_copied_trace_with_its_workspace(cx: &mut TestAppContext) {
    cx.executor().allow_parking(); // this integration uses real reader threads
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
            ws.enable_native_persistence(store, Persistence::Auto, Default::default());
            ws.open_path(trace.clone(), cx);
        })
        .unwrap();
    wait_native(cx, |cx| {
        window
            .update(cx, |ws, _, _| ws.app.doc.is_loaded())
            .unwrap()
    });
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
    wait_native(cx, |cx| {
        window
            .update(cx, |ws, _, _| ws.app.doc.is_loaded())
            .unwrap()
    });
    window
        .update(cx, |ws, _, _| {
            let ids: Vec<_> = ws.app.panels.iter().map(|panel| panel.id).collect();
            for id in ids {
                ws.app.layout_waves(
                    id,
                    volna_core::geometry::Rect::from_xywh(0.0, 0.0, 900.0, 300.0),
                    &volna_core::Theme::one_dark(),
                );
            }
            ws.wake_queries();
        })
        .unwrap();
    wait_native(cx, |cx| {
        window
            .update(cx, |ws, _, _| {
                assert!(
                    ws.queries.is_some(),
                    "query host missing: {:?}",
                    ws.app.workspace.notices
                );
                assert!(
                    ws.app.workspace.notices.is_empty(),
                    "{:?}",
                    ws.app.workspace.notices
                );
                ws.app.panels.iter().all(|panel| {
                    panel.kind.waves().unwrap().items.iter().all(|row| {
                        assert!(row.error.is_none(), "{:?}", row.error);
                        row.query.is_some()
                    })
                })
            })
            .unwrap()
    });
    window
        .update(cx, |ws, _, _| {
            assert_eq!(ws.app.panels.len(), 2);
            for panel in ws.app.panels.iter() {
                let waves = panel.kind.waves().unwrap();
                assert_eq!(waves.items.len(), 2);
                assert!(
                    waves
                        .items
                        .iter()
                        .all(|row| row.query.is_some() && row.history.is_none())
                );
            }
            assert!(!ws.app.workspace.scheduler.dirty());
        })
        .unwrap();
}

#[gpui_kit::test]
fn native_queries_wake_from_idle_for_new_rows_and_viewports(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    init(cx);
    let window = cx.add_window(Workspace::new);
    window
        .update(cx, |ws, _, cx| {
            ws.open_path(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/picorv32.vtr"),
                cx,
            )
        })
        .unwrap();
    wait_native(cx, |cx| {
        window
            .update(cx, |ws, _, _| ws.app.doc.is_loaded())
            .unwrap()
    });
    cx.run_until_parked(); // controller is idle before the user adds a row
    window
        .update(cx, |ws, _, cx| {
            assert!(ws.app.doc.query_snapshot().is_some());
            ws.dispatch(Command::AddVars(vec![0]), None, cx);
            ws.app.layout_waves(
                ws.app.panels.focused_id(),
                volna_core::geometry::Rect::from_xywh(0.0, 0.0, 900.0, 300.0),
                &volna_core::Theme::one_dark(),
            );
            ws.wake_queries();
        })
        .unwrap();
    wait_native(cx, |cx| {
        window
            .update(cx, |ws, _, _| {
                ws.app.panels.focused_waves().unwrap().items[0]
                    .query
                    .is_some()
            })
            .unwrap()
    });
    let old = window
        .update(cx, |ws, _, _| {
            let old = ws.app.panels.focused_waves().unwrap().items[0]
                .query
                .as_ref()
                .unwrap()
                .demand();
            ws.app.doc.shared.viewport.set(volna_core::wave::Viewport {
                start: 0.0,
                end: 100.0,
            });
            ws.wake_queries();
            old
        })
        .unwrap();
    wait_native(cx, |cx| {
        window
            .update(cx, |ws, _, _| {
                ws.app.panels.focused_waves().unwrap().items[0]
                    .query
                    .as_ref()
                    .is_some_and(|data| data.demand() != old)
            })
            .unwrap()
    });
    window
        .update(cx, |ws, _, cx| {
            assert!(
                ws.app.panels.focused_waves().unwrap().items[0]
                    .history
                    .is_none()
            );
            ws.dispatch(Command::CloseTrace, None, cx);
            assert!(ws.queries.is_none());
            assert!(ws.app.doc.query_snapshot().is_none());
        })
        .unwrap();
}
