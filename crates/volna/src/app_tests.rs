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
