use super::*;
use futures::channel::oneshot;
use gpui::TestAppContext;

#[gpui::test]
fn latest_open_wins_and_stale_demo_cannot_add_rows(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(crate::theme::Theme::one_dark()));
    let window = cx.add_window(Workspace::new);
    let (tx, rx) = oneshot::channel();
    window
        .update(cx, |ws, _, cx| {
            ws.load_source("slow demo", async { rx.await.unwrap() }, true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let current: Arc<dyn WaveSource> = Arc::new(SynthSource::new(7));
    window
        .update(cx, |ws, _, cx| {
            let source = current.clone();
            ws.load_source("new file", async { Ok(source) }, false, cx);
        })
        .unwrap();
    cx.run_until_parked();
    tx.send(Ok(Arc::new(SynthSource::new(100)) as Arc<dyn WaveSource>))
        .ok()
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |ws, _, cx| {
            assert!(matches!(&ws.state, TraceState::Loaded(s) if Arc::ptr_eq(s, &current)));
            assert!(ws.waves.read(cx).items.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn closing_invalidates_pending_success_and_error(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(crate::theme::Theme::one_dark()));
    let window = cx.add_window(Workspace::new);
    for fail in [false, true] {
        let (tx, rx) = oneshot::channel();
        window
            .update(cx, |ws, _, cx| {
                ws.load_source("slow file", async { rx.await.unwrap() }, false, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |ws, window, cx| ws.close_trace(&CloseTrace, window, cx))
            .unwrap();
        let result = if fail {
            Err(anyhow::anyhow!("late error"))
        } else {
            Ok(Arc::new(SynthSource::new(10)) as Arc<dyn WaveSource>)
        };
        tx.send(result).ok().unwrap();
        cx.run_until_parked();
        window
            .update(cx, |ws, _, _| {
                assert!(matches!(ws.state, TraceState::Empty))
            })
            .unwrap();
    }
}

#[gpui::test]
fn theme_changes_preserve_trace_and_interaction_state(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(crate::theme::Theme::one_dark()));
    let window = cx.add_window(Workspace::new);
    let source: Arc<dyn WaveSource> = Arc::new(SynthSource::new(100));
    window
        .update(cx, |ws, _, cx| {
            ws.set_source(source.clone(), cx);
            ws.sidebar_width = px(355.0);
            ws.scopes_fraction = 0.61;
            ws.waves.update(cx, |w, cx| {
                w.add_vars(&[0; 100], cx);
                w.cursor = Some(42);
                w.selected.insert(1);
                w.anchor = Some(1);
                w.viewport.start = 20.0;
                w.viewport.end = 80.0;
                w.names_width = px(260.0);
                w.values_width = px(140.0);
                w.scroll_y = px(24.0);
                w.markers.push(crate::wave::view::Marker { time: 30 });
            });
        })
        .unwrap();
    cx.run_until_parked();
    let (before, history) = window
        .update(cx, |ws, _, cx| {
            (
                ws.debug_state(cx),
                ws.waves.read(cx).items[0].history.clone().unwrap(),
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
            crate::theme::Theme::from_host(&crate::theme::HostPalette {
                appearance,
                ..Default::default()
            })
            .install(cx)
        });
        cx.run_until_parked();
        window
            .update(cx, |ws, _, cx| {
                assert!(matches!(&ws.state, TraceState::Loaded(s) if Arc::ptr_eq(s, &source)));
                assert_eq!(ws.debug_state(cx), before);
                let w = ws.waves.read(cx);
                assert!(Arc::ptr_eq(w.items[0].history.as_ref().unwrap(), &history));
                assert_eq!(w.names_width, px(260.0));
                assert_eq!(w.values_width, px(140.0));
                assert_eq!(w.scroll_y, px(24.0));
                assert_eq!(w.markers[0].time, 30);
            })
            .unwrap();
    }
}
