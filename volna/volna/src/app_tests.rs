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
            assert!(!ws.app.waves.items.is_empty());
            assert_eq!(ws.app.waves.loaded_count(), ws.app.waves.items.len());
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
            assert!(ws.app.waves.items.is_empty());
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
            ws.app.doc.cursor = Some(42);
            ws.app.waves.selected.insert(1);
            ws.app.waves.anchor = Some(1);
            ws.app.waves.viewport.start = 20.0;
            ws.app.waves.viewport.end = 80.0;
            ws.app.waves.names_width = 260.0;
            ws.app.waves.values_width = 140.0;
            ws.app.waves.scroll_y = 24.0;
            ws.app
                .doc
                .markers
                .push(volna_core::document::Marker { time: 30 });
        })
        .unwrap();
    cx.run_until_parked();
    let (before, history) = window
        .update(cx, |ws, _, _| {
            (
                ws.debug_state(),
                ws.app.waves.items[0].history.clone().unwrap(),
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
                let w = &ws.app.waves;
                assert!(Arc::ptr_eq(w.items[0].history.as_ref().unwrap(), &history));
                assert_eq!(w.names_width, 260.0);
                assert_eq!(w.values_width, 140.0);
                assert_eq!(w.scroll_y, 24.0);
                assert_eq!(ws.app.doc.markers[0].time, 30);
            })
            .unwrap();
    }
}
