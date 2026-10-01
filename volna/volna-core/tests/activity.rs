//! Scope activity meters (docs/hierarchy-activity.html, stage 4): a trace
//! opened beside its activity index counts, for every scope row, the
//! signals that change in the viewport, the same as a brute force over its
//! histories, VTR and FST; a window narrower than the index's thresholds
//! shows a range until the read replaces it; a read for a window the view
//! has left is dropped; an index built for another trace is ignored.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use volna_core::app::App;
use volna_core::data::{ScopeActivity, SignalRef};
use volna_core::session::{LoadRequest, LoadResult, OpenSpec, Session};
use volna_core::sidebar::TreeNode;
use volna_core::trace::{TraceId, Traced};
use volna_core::wave::Viewport;
use vtr::{Direction, ScopeType, SignalKind, VarType, Writer, WriterOptions};

/// A small design over several blocks: a clock, bursty units under nested
/// scopes, a port aliasing its parent's net. Everything pauses from 14,990
/// to 65,000, and unit2 sleeps after that.
fn write_trace(path: &Path, seed: u64) {
    let opts = WriterOptions {
        block_records: 400,
        ..Default::default()
    };
    let mut w = Writer::create_with(path, opts).unwrap();
    let bit = SignalKind::Bits {
        width: 1,
        states: 2,
    };
    let byte = SignalKind::Bits {
        width: 8,
        states: 2,
    };
    let top = Some(w.add_scope(None, "top", ScopeType::Module, "top").unwrap());
    let (_, clk) = w
        .add_var(top, "clk", VarType::Wire, Direction::Input, bit)
        .unwrap();
    let mut units = Vec::new();
    for u in 0..3 {
        let unit = Some(
            w.add_scope(top, &format!("unit{u}"), ScopeType::Module, "unit")
                .unwrap(),
        );
        w.add_alias(unit, "clk", VarType::Wire, Direction::Input, clk)
            .unwrap();
        let (_, a) = w
            .add_var(unit, "a", VarType::Reg, Direction::Implicit, byte)
            .unwrap();
        let core = Some(
            w.add_scope(unit, "core", ScopeType::Module, "core")
                .unwrap(),
        );
        let (_, b) = w
            .add_var(core, "b", VarType::Reg, Direction::Implicit, bit)
            .unwrap();
        let (_, c) = w
            .add_var(core, "c", VarType::Reg, Direction::Implicit, byte)
            .unwrap();
        units.push([a, b, c]);
    }
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut rand = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    for step in 0..3000u64 {
        let t = step * 10 + if step >= 1500 { 50_000 } else { 0 };
        w.set_time(t).unwrap();
        w.emit_bit(clk, (step & 1) as u8).unwrap();
        for (u, sigs) in units.iter().enumerate() {
            // unit2 sleeps through the second half.
            if u == 2 && step >= 1500 {
                continue;
            }
            for &s in sigs {
                if rand(10 + 20 * u as u64) == 0 {
                    w.emit_u64(s, rand(200)).unwrap();
                }
            }
        }
    }
    w.close().unwrap();
}

fn index_beside(trace: &Path) {
    let r = vtr::Reader::open(trace).unwrap();
    let id = vtr::activity::Identity::of(&r).unwrap();
    vtr::activity::Sidecar::new(trace, &id, None)
        .write(|w| vtr::activity::build(&r, w, &vtr::activity::BuildOptions::default()))
        .unwrap();
}

/// Performs every request, as a frontend's executor does, until none are left.
fn settle(app: &mut App) {
    for _ in 0..20 {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for r in requests {
            app.deliver(r.perform());
        }
    }
    panic!("requests keep coming");
}

fn open(path: &Path) -> (App, Arc<dyn Session>) {
    let session = OpenSpec::Path(path.to_path_buf()).open().unwrap();
    let mut app = App::new();
    app.set_session(session.clone());
    settle(&mut app);
    (app, session)
}

/// For every scope, the distinct signals below it with a change after the
/// first time step in `[t0, t1]`, from the session's histories.
fn brute(session: &Arc<dyn Session>, t_min: u64, t0: u64, t1: u64) -> Vec<u32> {
    let h = session.hierarchy();
    let signals: HashSet<SignalRef> = (0..h.var_count()).map(|v| h.var(v).signal).collect();
    let signals: Vec<SignalRef> = signals.into_iter().collect();
    let changing: HashSet<SignalRef> = session
        .load_signals(&signals)
        .into_iter()
        .filter(|(_, history)| {
            let history = history.as_ref().unwrap();
            (0..history.len())
                .map(|i| history.time(i))
                .any(|t| t > t_min && t >= t0 && t <= t1)
        })
        .map(|(s, _)| s)
        .collect();
    (0..h.scope_count())
        .map(|scope| {
            let (mut below, mut stack) = (HashSet::new(), vec![scope]);
            while let Some(s) = stack.pop() {
                below.extend(
                    h.scope(s)
                        .vars
                        .iter()
                        .map(|v| h.var(v).signal)
                        .filter(|s| changing.contains(s)),
                );
                stack.extend(h.scope(s).children);
            }
            below.len() as u32
        })
        .collect()
}

fn node(scope: usize) -> TreeNode {
    TreeNode::scope(Traced::new(TraceId::A, scope))
}

fn view(app: &mut App, t0: u64, t1: u64) {
    app.doc.shared.viewport.set(Viewport {
        start: t0 as f64,
        end: t1 as f64,
    });
}

/// Every scope's meter over the brute force, once exact.
fn check_exact(app: &App, session: &Arc<dyn Session>, t0: u64, t1: u64) {
    let t_min = session.activity().unwrap().t_min();
    let want = brute(session, t_min, t0, t1);
    for (scope, &n) in want.iter().enumerate() {
        let a = app.scope_activity(node(scope)).unwrap();
        assert!(a.exact(), "[{t0}, {t1}] scope {scope} still a range");
        assert_eq!(a.changing, n, "[{t0}, {t1}] scope {scope}");
    }
}

#[test]
fn meters_count_the_viewport_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 1);
    index_beside(&path);
    let (mut app, session) = open(&path);
    assert!(session.activity().is_some());
    let top = node(session.hierarchy().roots().first().unwrap());
    for (t0, t1) in [
        (0, 80_000),
        (100, 1_000),
        (14_990, 15_010),
        (20_000, 21_000),
        (64_000, 65_005),
        (70_003, 70_004),
    ] {
        view(&mut app, t0, t1);
        settle(&mut app);
        check_exact(&app, &session, t0, t1);
    }
    // While unit2 sleeps its rows are quiet and faint; the rest are not.
    view(&mut app, 70_000, 70_200);
    settle(&mut app);
    let h = session.hierarchy();
    let quiet: Vec<String> = (0..h.scope_count())
        .filter(|&s| app.scope_activity(node(s)).unwrap().quiet())
        .map(|s| h.scope_path(s).join("."))
        .collect();
    assert!(quiet.contains(&"top.unit2.core".to_string()), "{quiet:?}");
    assert!(!app.scope_activity(top).unwrap().quiet());
    // Trace rows have no meter.
    assert_eq!(app.scope_activity(TreeNode::trace(TraceId::A)), None);
}

/// A narrow window first shows what the index decides, a range, then the
/// exact count once the undecided signals are read.
#[test]
fn a_narrow_view_shows_a_range_until_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 2);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let top = node(session.hierarchy().roots().first().unwrap());
    let index = session.activity().unwrap();
    // A window between two clock edges, narrower than every threshold.
    let (t0, t1) = (5_003, 5_004);
    assert!(index.exact_width(t0, t1) > t1 - t0 + 1);
    view(&mut app, t0, t1);
    let classify = app.take_requests();
    assert!(
        classify
            .iter()
            .all(|r| matches!(r, LoadRequest::Activity { .. }))
            && classify.len() == 1
    );
    for r in classify {
        app.deliver(r.perform());
    }
    let pending = app.scope_activity(top).unwrap();
    assert!(
        !pending.exact() && pending.changing < pending.upper,
        "{pending:?}"
    );
    assert!(pending.label().contains('–'));
    let read = app.take_requests();
    assert!(matches!(
        read.as_slice(),
        [LoadRequest::ResolveActivity { .. }]
    ));
    for r in read {
        app.deliver(r.perform());
    }
    let exact = app.scope_activity(top).unwrap();
    assert!(exact.exact() && pending.changing <= exact.changing && exact.changing <= pending.upper);
    check_exact(&app, &session, t0, t1);
    assert!(app.take_requests().is_empty(), "nothing left to do");
}

#[test]
fn a_read_for_a_window_the_view_left_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let top = node(session.hierarchy().roots().first().unwrap());
    view(&mut app, 5_003, 5_004);
    for r in app.take_requests() {
        app.deliver(r.perform());
    }
    let read: Vec<LoadResult> = app
        .take_requests()
        .into_iter()
        .map(LoadRequest::perform)
        .collect();
    assert_eq!(read.len(), 1);
    // The view moves on before the read returns.
    view(&mut app, 100, 1_000);
    for r in app.take_requests() {
        app.deliver(r.perform());
    }
    let wide = app.scope_activity(top).unwrap();
    for r in read {
        app.deliver(r);
    }
    assert!(wide.changing > 0);
    assert_eq!(
        app.scope_activity(top),
        Some(wide),
        "the late read changed nothing"
    );
    settle(&mut app);
    check_exact(&app, &session, 100, 1_000);
}

#[test]
fn a_late_read_is_dropped_before_the_next_classification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let top = node(session.hierarchy().roots().first().unwrap());
    view(&mut app, 5_003, 5_004);
    for r in app.take_requests() {
        app.deliver(r.perform());
    }
    let pending = Arc::clone(app.activity.counts(TraceId::A).unwrap());
    assert!(!pending.exact());
    let read = app.take_requests().pop().unwrap().perform();
    view(&mut app, 100, 1_000);
    assert_eq!(
        app.scope_activity(top),
        None,
        "old meters must not describe the new view"
    );
    app.deliver(read);
    assert!(
        Arc::ptr_eq(app.activity.counts(TraceId::A).unwrap(), &pending),
        "a late read must not replace the pending counts"
    );
    settle(&mut app);
    check_exact(&app, &session, 100, 1_000);
}

#[test]
fn fast_pans_coalesce_and_drop_old_classifications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let top = node(session.hierarchy().roots().first().unwrap());
    view(&mut app, 100, 1_000);
    let classification = app.take_requests().pop().unwrap().perform();
    for t0 in [2_000, 3_000, 4_000] {
        view(&mut app, t0, t0 + 100);
        assert!(
            app.take_requests().is_empty(),
            "only one classification in flight"
        );
    }
    app.deliver(classification);
    assert_eq!(
        app.scope_activity(top),
        None,
        "stale classifications stay hidden"
    );
    let requests = app.take_requests();
    assert!(matches!(
        requests.as_slice(),
        [LoadRequest::Activity {
            window: (4_000, 4_100),
            ..
        }]
    ));
    for r in requests {
        app.deliver(r.perform());
    }
    settle(&mut app);
    check_exact(&app, &session, 4_000, 4_100);
}

#[test]
fn replacing_a_trace_rejects_results_before_the_next_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let top = node(session.hierarchy().roots().first().unwrap());
    view(&mut app, 100, 1_000);
    let classification = app.take_requests().pop().unwrap().perform();
    app.set_session(Arc::clone(&session));
    assert_eq!(app.scope_activity(top), None);
    app.deliver(classification);
    assert_eq!(app.scope_activity(top), None);
    settle(&mut app);
    let viewport = app.activity_viewport();
    check_exact(&app, &session, viewport.start as u64, viewport.end as u64);
}

#[test]
fn meters_follow_the_focused_panels_unlinked_viewport() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    app.handle(volna_core::app::Command::AddVars(vec![Traced::new(
        TraceId::A,
        0,
    )]));
    view(&mut app, 0, 80_000);
    let nav = app.panels.focused_mut().kind.nav_mut().unwrap();
    nav.link.viewport = false;
    nav.local_viewport.set(Viewport {
        start: 70_000.0,
        end: 70_200.0,
    });
    settle(&mut app);
    check_exact(&app, &session, 70_000, 70_200);
    app.panels
        .focused_mut()
        .kind
        .nav_mut()
        .unwrap()
        .link
        .viewport = true;
    settle(&mut app);
    check_exact(&app, &session, 0, 80_000);
}

#[test]
fn meters_use_each_traces_timescale_and_round_inwards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let fine = dir.path().join("fine.vtr");
    let mut writer = Writer::create(&fine).unwrap();
    writer.set_timescale(-12).unwrap();
    writer.close().unwrap();
    let fine = OpenSpec::Path(fine).open().unwrap();
    app.doc.add_session(fine).unwrap();
    assert_eq!(
        app.doc
            .traces()
            .get(TraceId::A)
            .unwrap()
            .placement()
            .scale(),
        1000
    );
    // At 70,000 ns the clock changes; at 70,001 ns no signal changes.
    // Neither change is inside [70,000.1, 70,001.9] ns.
    view(&mut app, 70_000_100, 70_001_900);
    settle(&mut app);
    check_exact(&app, &session, 70_001, 70_001);
    assert_eq!(
        app.activity.counts(TraceId::A).unwrap().window,
        (70_001, 70_001)
    );
    // A view between two adjacent trace ticks contains no trace time.
    view(&mut app, 70_000_100, 70_000_900);
    settle(&mut app);
    check_exact(&app, &session, 70_001, 70_000);
}

#[test]
fn failed_reads_report_errors_and_stop_showing_pending_meters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let (mut app, session) = open(&path);
    let top = node(session.hierarchy().roots().first().unwrap());
    app.take_events();
    view(&mut app, 5_003, 5_004);
    for r in app.take_requests() {
        app.deliver(r.perform());
    }
    assert!(!app.scope_activity(top).unwrap().exact());
    let mut read = app.take_requests().pop().unwrap();
    let LoadRequest::ResolveActivity { budget, .. } = &mut read else {
        panic!("expected read")
    };
    *budget = volna_core::remote::memory::MemoryBudget::new(0);
    app.deliver(read.perform());
    assert!(app.take_events().iter().any(|e| matches!(e, volna_core::app::Event::Notice(message) if message.contains("memory budget exceeded"))));
    assert_eq!(
        app.scope_activity(top),
        None,
        "a failed read must not claim to be pending"
    );
    assert!(app.take_requests().is_empty(), "a failure must not loop");
    view(&mut app, 100, 1_000);
    settle(&mut app);
    check_exact(&app, &session, 100, 1_000);
}

#[test]
fn an_index_of_another_trace_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 4);
    index_beside(&path);
    // Simulated again: the sidecar beside it describes the old run.
    write_trace(&path, 5);
    let (app, session) = open(&path);
    assert!(session.activity().is_none());
    let top = node(session.hierarchy().roots().first().unwrap());
    assert_eq!(
        app.scope_activity(top),
        None,
        "no meters, only the size column"
    );
    assert!(volna_core::sidebar::ScopeTreeModel::size(app.doc.traces(), top).is_some());
}

/// The same meters from an FST, through the FST front end.
#[test]
fn fst_traces_count_alike() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zlib.fst");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/vtr-cli/tests/fixtures/activity/zlib.fst");
    std::fs::copy(fixture, &path).unwrap();
    let f = vtr_cli::fst::activity::FstTrace::open(&path).unwrap();
    vtr::activity::Sidecar::new(&path, &f.identity(), None)
        .write(|w| f.build(w, &vtr::activity::BuildOptions::default()))
        .unwrap();
    let (mut app, session) = open(&path);
    assert!(session.activity().is_some());
    for (t0, t1) in [(0, 20_000_000), (1_000_000, 1_001_000), (100_000, 100_004)] {
        view(&mut app, t0, t1);
        settle(&mut app);
        check_exact(&app, &session, t0, t1);
    }
}

#[test]
fn a_row_labels_its_count_and_range() {
    let exact = ScopeActivity {
        changing: 2483,
        upper: 2483,
        total: 6779,
    };
    assert_eq!(exact.label(), "2,483 / 6,779");
    assert_eq!(exact.detail(), "2,483 of 6,779 signals change in the view");
    let range = ScopeActivity {
        changing: 1462,
        upper: 2210,
        total: 6779,
    };
    assert_eq!(range.label(), "1,462–2,210 / 6,779");
    let (lo, hi) = range.shares();
    assert!(lo < hi && hi < 1.0);
    assert!(
        ScopeActivity {
            changing: 0,
            upper: 0,
            total: 5
        }
        .quiet()
    );
    assert!(
        !ScopeActivity {
            changing: 0,
            upper: 1,
            total: 5
        }
        .quiet(),
        "faint only when nothing can change"
    );
    assert!(
        !ScopeActivity {
            changing: 0,
            upper: 0,
            total: 0
        }
        .quiet()
    );
}
