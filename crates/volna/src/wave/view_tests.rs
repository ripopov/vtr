use super::*;
use crate::data::{Hierarchy, TraceInfo, synth::SynthSource};
use gpui::{AppContext, TestAppContext};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

struct Source {
    inner: SynthSource,
    hierarchy: Hierarchy,
    loads: AtomicUsize,
    fail: AtomicBool,
}

impl Source {
    fn new() -> Arc<Self> {
        let inner = SynthSource::new(10);
        let mut hierarchy = inner.hierarchy().clone();
        let mut alias = hierarchy.vars[0].clone();
        alias.name = "alias".into();
        hierarchy.vars.push(alias);
        Arc::new(Self {
            inner,
            hierarchy,
            loads: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        })
    }
}

impl WaveSource for Source {
    fn info(&self) -> &TraceInfo {
        self.inner.info()
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        self.loads.fetch_add(1, SeqCst);
        anyhow::ensure!(!self.fail.load(SeqCst), "test failure");
        self.inner.load_signal(signal)
    }
}

#[gpui::test]
fn aliases_share_pending_and_loaded_histories(cx: &mut TestAppContext) {
    let source = Source::new();
    let view = cx.new(WaveView::new);
    view.update(cx, |view, cx| {
        view.set_source(Some(source.clone()), cx);
        view.add_vars(&[0, source.hierarchy.vars.len() - 1, 0], cx);
        assert_eq!(view.pending.len(), 1);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.add_vars(&[0], cx);
        assert_eq!(source.loads.load(SeqCst), 1);
        assert_eq!(view.items[1].name.as_ref(), "alias");
        let history = view.items[0].history.as_ref().unwrap();
        assert!(
            view.items
                .iter()
                .all(|i| Arc::ptr_eq(history, i.history.as_ref().unwrap()))
        );
        let history = Arc::downgrade(history);
        // The synthetic source owns its history too; only rows add strong references.
        let before = history.strong_count();
        view.set_source(None, cx);
        assert_eq!(history.strong_count(), before - 4);
    });
}

#[gpui::test]
fn stale_results_cannot_fill_rows_or_clear_new_pending_loads(cx: &mut TestAppContext) {
    let source = Source::new();
    let view = cx.new(WaveView::new);
    view.update(cx, |view, cx| {
        view.set_source(Some(source.clone()), cx);
        view.add_vars(&[0], cx);
        let old = view.load_generation;
        // Reopening even the same source creates a new generation.
        view.set_source(Some(source.clone()), cx);
        view.add_vars(&[0], cx);
        let signal = source.hierarchy.vars[0].signal;
        view.finish_signal(old, signal, source.inner.load_signal(signal), cx);
        view.finish_signal(old, signal, Err(anyhow::anyhow!("old failure")), cx);
        assert!(view.pending.contains(&signal));
        assert!(view.items[0].history.is_none());
        assert!(view.items[0].error.is_none());
    });
    cx.run_until_parked();
    view.update(cx, |view, _| assert!(view.items[0].history.is_some()));
}

#[gpui::test]
fn failed_loads_can_retry_for_all_alias_rows(cx: &mut TestAppContext) {
    let source = Source::new();
    source.fail.store(true, SeqCst);
    let view = cx.new(WaveView::new);
    view.update(cx, |view, cx| {
        view.set_source(Some(source.clone()), cx);
        view.add_vars(&[0, 0], cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert!(view.items.iter().all(|i| i.error.is_some()));
        source.fail.store(false, SeqCst);
        view.add_vars(&[0], cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(source.loads.load(SeqCst), 2);
        assert!(
            view.items
                .iter()
                .all(|i| i.error.is_none() && i.history.is_some())
        );
    });
}
