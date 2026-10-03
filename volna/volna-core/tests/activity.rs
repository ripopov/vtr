//! Scope activity meters (docs/hierarchy-activity.html, stage 4): a trace
//! opened beside its activity index counts, for every scope row, the
//! signals that change in the viewport, the same as a brute force over its
//! histories, VTR and FST; a window narrower than the index's thresholds
//! shows a range until the read replaces it; a read for a window the view
//! has left is dropped; an index built for another trace is ignored.

// Exercise the executable handler directly over bounded test sockets; no server library is needed.
#[cfg(unix)]
#[path = "../../../tools/volna-server/src/server.rs"]
mod server;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use volna_core::app::App;
use volna_core::data::ScopeActivity;
use volna_core::session::{LoadRequest, LoadResult};
use volna_core::sidebar::TreeNode;
use volna_core::trace::{TraceId, Traced};
use volna_core::wave::Viewport;
use volna_trace::data::SignalRef;
use volna_trace::session::{OpenSpec, Session};
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
fn restoring_a_workspace_before_scope_sizes_arrive_keeps_activity_meters() {
    use volna_core::workspace::Workspace;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 3);
    index_beside(&path);
    let session = OpenSpec::Path(path).open().unwrap();
    // Counts may still be queued, in flight, or already retained.
    for initial in 0..3 {
        let mut app = App::new();
        app.set_session(session.clone());
        if initial == 2 {
            settle(&mut app);
        }
        let stale = (initial == 1).then(|| {
            app.take_requests()
                .into_iter()
                .find(|r| matches!(r, LoadRequest::Sizes { .. }))
                .expect("initial scope count")
                .perform()
        });
        let saved = Workspace::capture(&app, volna_core::testing::paths("run.vtr"), None).unwrap();
        saved
            .prepare(
                &app,
                "file:///tmp/run.vtr",
                "file:///tmp/run.vtr.volna.json",
            )
            .unwrap()
            .commit(&mut app)
            .unwrap();
        if let Some(stale) = stale {
            app.deliver(stale);
            assert!(app.doc.traces().get(TraceId::A).unwrap().sizes().is_none());
        }
        let requests = app.take_requests();
        assert_eq!(
            requests
                .iter()
                .filter(|r| matches!(r, LoadRequest::Sizes { .. }))
                .count(),
            usize::from(initial != 2),
            "only unfinished counts restart"
        );
        for request in requests {
            app.deliver(request.perform());
        }
        settle(&mut app);
        let window = app.activity_viewport();
        check_exact(
            &app,
            &session,
            window.start.max(0.0) as u64,
            window.end as u64,
        );
    }
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
    *budget = volna_trace::remote::memory::MemoryBudget::new(0);
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

fn build_views(app: &App) -> Vec<volna_core::sidebar::activity::ActivityBuildView> {
    app.activity.builds(
        app.doc.traces(),
        app.settings.resolved().hierarchy.activity_index,
    )
}

#[test]
fn build_offer_obeys_policy_and_not_now() {
    use volna_core::app::Command;
    use volna_core::sidebar::activity::ActivityBuildState;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 6);
    let (mut app, session) = open(&path);
    assert!(matches!(
        build_views(&app)[0].state,
        ActivityBuildState::Offer {
            estimated_seconds: 1
        }
    ));
    assert!(session.activity().is_none());
    app.handle(Command::DismissActivity(TraceId::A));
    settle(&mut app);
    assert!(build_views(&app).is_empty());
    app.settings_loaded(r#"{"hierarchy.activityIndex": "always"}"#);
    let requests = app.take_requests();
    assert!(matches!(
        requests.as_slice(),
        [LoadRequest::BuildActivity { .. }]
    ));
    for r in requests {
        app.deliver(r.perform());
    }
    settle(&mut app);
    assert!(session.activity().is_some());
    assert!(build_views(&app).is_empty());
    // Byte images have no destination and no build offer.
    let session = OpenSpec::Bytes {
        name: "bytes.vtr".into(),
        bytes: std::fs::read(&path).unwrap(),
    }
    .open()
    .unwrap();
    app.set_session(session);
    settle(&mut app);
    assert!(build_views(&app).is_empty());
}

#[test]
fn never_hides_the_offer_and_always_builds_with_the_sidebar_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 6);
    let (mut app, session) = open(&path);
    app.settings_loaded(r#"{"hierarchy.activityIndex": "never"}"#);
    settle(&mut app);
    assert!(build_views(&app).is_empty());
    assert!(session.activity().is_none());
    app.sidebar_visible = false;
    app.settings_loaded(r#"{"hierarchy.activityIndex": "always"}"#);
    settle(&mut app);
    assert!(session.activity().is_some());
    assert!(build_views(&app).is_empty());
}

#[test]
fn explicit_build_installs_meters_and_reports_block_progress() {
    use volna_core::app::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 6);
    let (mut app, session) = open(&path);
    app.handle(Command::AddVars(vec![Traced::new(TraceId::A, 0)]));
    settle(&mut app);
    let undo = app.undo_label().map(str::to_owned);
    let resident = session.resident_bytes();
    app.handle(Command::BuildActivity(TraceId::A));
    let r = app.take_requests().pop().unwrap();
    let LoadRequest::BuildActivity { options, .. } = &r else {
        panic!("expected build")
    };
    let control = Arc::clone(options.control.as_ref().unwrap());
    assert_eq!(control.progress().completed, 0);
    app.handle(Command::BuildActivity(TraceId::A));
    assert!(app.take_requests().is_empty(), "one build per trace");
    app.deliver(r.perform());
    assert!(
        session.resident_bytes() > resident,
        "session owns the new immutable index"
    );
    let p = control.progress();
    assert!(p.total > 1 && p.completed == p.total, "{p:?}");
    assert!(
        !control.cancel(),
        "completed publication wins over late cancellation"
    );
    settle(&mut app);
    assert_eq!(
        app.undo_label(),
        undo.as_deref(),
        "building is not journaled"
    );
    view(&mut app, 100, 1_000);
    settle(&mut app);
    check_exact(&app, &session, 100, 1_000);
    let reopened = OpenSpec::Path(path).open().unwrap();
    assert!(
        reopened.activity().is_some(),
        "the next open uses the sidecar"
    );
}

#[test]
fn cancelled_and_closed_builds_leave_no_sidecar_or_temporary_file() {
    use volna_core::app::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 6);
    let (mut app, session) = open(&path);
    // Cancellation before the request is taken also works.
    app.handle(Command::BuildActivity(TraceId::A));
    app.handle(Command::CancelActivity(TraceId::A));
    assert!(app.take_requests().is_empty());
    for close in [false, true] {
        app.handle(Command::BuildActivity(TraceId::A));
        let r = app.take_requests().pop().unwrap();
        if close {
            app.handle(Command::CloseTrace);
        } else {
            app.handle(Command::CancelActivity(TraceId::A));
        }
        app.deliver(r.perform());
        settle(&mut app);
        assert!(session.activity().is_none());
        assert!(build_views(&app).is_empty());
        let files: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|p| p.unwrap().file_name())
            .collect();
        assert_eq!(files, [std::ffi::OsString::from("run.vtr")]);
    }
}

#[test]
fn a_failed_build_can_be_retried_explicitly() {
    use volna_core::app::{Command, Event};
    use volna_core::sidebar::activity::ActivityBuildState;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.vtr");
    write_trace(&path, 6);
    let (mut app, session) = open(&path);
    app.handle(Command::BuildActivity(TraceId::A));
    let mut r = app.take_requests().pop().unwrap();
    let LoadRequest::BuildActivity { options, .. } = &mut r else {
        panic!("expected build")
    };
    options.memory = 1;
    app.deliver(r.perform());
    assert!(matches!(
        build_views(&app)[0].state,
        ActivityBuildState::Failed { .. }
    ));
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, Event::Notice(message) if message.contains("memory limit")))
    );
    assert!(
        app.take_requests().is_empty(),
        "failure does not automatically loop"
    );
    app.handle(Command::BuildActivity(TraceId::A));
    settle(&mut app);
    assert!(session.activity().is_some());
    assert!(build_views(&app).is_empty());
}

#[test]
fn fst_build_from_the_viewer_installs_the_same_meters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.fst");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/vtr-cli/tests/fixtures/activity/zlib.fst");
    std::fs::copy(fixture, &path).unwrap();
    let (mut app, session) = open(&path);
    app.handle(volna_core::app::Command::BuildActivity(TraceId::A));
    settle(&mut app);
    assert!(session.activity().is_some());
    for (t0, t1) in [(0, 20_000_000), (100_000, 100_004)] {
        view(&mut app, t0, t1);
        settle(&mut app);
        check_exact(&app, &session, t0, t1);
    }
}

#[cfg(unix)]
#[test]
fn viewer_builds_use_the_cache_for_a_read_only_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let traces = dir.path().join("traces");
    let cache = dir.path().join("cache");
    std::fs::create_dir(&traces).unwrap();
    let path = traces.join("run.vtr");
    write_trace(&path, 6);
    let (mut app, session) = open(&path);
    app.handle(volna_core::app::Command::BuildActivity(TraceId::A));
    let mut r = app.take_requests().pop().unwrap();
    let LoadRequest::BuildActivity { cache_dir, .. } = &mut r else {
        panic!("expected build")
    };
    *cache_dir = Some(cache.clone());
    std::fs::set_permissions(&traces, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(
        std::fs::File::create(traces.join("probe")).is_err(),
        "this test requires an unprivileged user"
    );
    let result = r.perform();
    std::fs::set_permissions(&traces, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.deliver(result);
    settle(&mut app);
    assert!(session.activity().is_some());
    let reader = vtr::Reader::open(&path).unwrap();
    let identity = vtr::activity::Identity::of(&reader).unwrap();
    let sidecar = vtr::activity::Sidecar::new(&path, &identity, Some(&cache));
    assert!(!sidecar.beside.exists());
    assert_eq!(sidecar.load(&identity).unwrap().0, sidecar.cached.unwrap());
}

#[test]
fn independent_sessions_share_a_single_sidecar_build() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.vtr");
    write_trace(&path, 191);
    let sessions: Vec<_> = (0..2)
        .map(|_| OpenSpec::Path(path.clone()).open().unwrap())
        .collect();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let controls: Vec<_> = (0..2)
        .map(|_| Arc::new(vtr::activity::BuildControl::default()))
        .collect();
    std::thread::scope(|scope| {
        for (session, control) in sessions.iter().zip(&controls) {
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                session
                    .build_activity(
                        &vtr::activity::BuildOptions {
                            threads: 2,
                            memory: 1 << 20,
                            control: Some(control.clone()),
                            ..Default::default()
                        },
                        &volna_trace::remote::memory::MemoryBudget::new(4 << 20),
                        None,
                    )
                    .unwrap();
            });
        }
    });
    assert_eq!(
        controls
            .iter()
            .filter(|c| c.progress().completed > 0)
            .count(),
        1
    );
    let bytes = sessions[0].activity_image(None).unwrap();
    assert_eq!(bytes, sessions[1].activity_image(None).unwrap());
    let index =
        vtr::activity::Index::decode(&bytes, &sessions[0].activity_identity().unwrap()).unwrap();
    assert_eq!(
        index.classify(300, 700),
        sessions[1].activity().unwrap().classify(300, 700)
    );
    assert!(
        !dir.path().read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp."))
    );
}

#[cfg(unix)]
#[test]
fn server_streams_a_cached_sidecar_and_keeps_it_after_disconnect() {
    use std::os::unix::net::UnixStream;
    use volna_trace::remote::ClientStep;
    use volna_trace::remote::{open::OpenTransfer, transport::*};
    use volna_trace::session::LoadResult;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote.vtr");
    write_trace(&path, 712);
    let session = OpenSpec::Path(path.clone()).open().unwrap();
    let (mut client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    server
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let server_session = session.clone();
    let service = std::thread::spawn(move || {
        server::serve(
            server.try_clone().unwrap(),
            server,
            71,
            || Ok(server_session),
            || Ok(()),
        )
    });
    let mut opening = OpenTransfer::new(
        1,
        TraceId::A,
        1,
        4 << 20,
        volna_trace::remote::memory::MemoryBudget::new(8 << 20),
    )
    .unwrap();
    write_packet(&mut client, &opening.command()).unwrap();
    loop {
        let packet = read_packet(&mut client).unwrap().unwrap();
        let mut step = opening.accept(packet).unwrap();
        loop {
            match step {
                ClientStep::Yield => step = opening.step().unwrap(),
                ClientStep::Ack(ack) => {
                    write_packet(&mut client, &ack).unwrap();
                    break;
                }
                ClientStep::Complete { ack, result } => {
                    write_packet(&mut client, &ack).unwrap();
                    assert!(matches!(result, LoadResult::Opened { result: Ok(_), .. }));
                    break;
                }
            }
        }
        if opening.is_complete() {
            break;
        }
    }
    opening.finish().unwrap();
    write_packet(
        &mut client,
        &Packet {
            session: 71,
            request: 2,
            sequence: 0,
            body: Body::Command(Command::Activity { build: true }),
        },
    )
    .unwrap();
    let mut receiver = Receiver::new(71, 2, vec![ObjectId::Activity], 4 << 20).unwrap();
    let mut bytes = Vec::new();
    while !receiver.is_complete() {
        let packet = read_packet(&mut client).unwrap().unwrap();
        let ack = acknowledgement(&packet);
        if let Receive::Data(chunk) = receiver.accept(packet).unwrap() {
            bytes.extend(chunk);
        }
        write_packet(&mut client, &ack).unwrap();
    }
    receiver.finish().unwrap();
    let image: Vec<u8> = bincode::deserialize(&bytes).unwrap();
    let index =
        vtr::activity::Index::decode(&image, &session.activity_identity().unwrap()).unwrap();
    assert_eq!(
        index.classify(500, 1100),
        session.activity().unwrap().classify(500, 1100)
    );
    // Simulate losing the client before the first response ACK. Publication
    // belongs to the reader, so its completed sidecar is retained.
    write_packet(
        &mut client,
        &Packet {
            session: 71,
            request: 3,
            sequence: 0,
            body: Body::Command(Command::Activity { build: false }),
        },
    )
    .unwrap();
    assert!(matches!(
        read_packet(&mut client).unwrap().unwrap().body,
        Body::Begin {
            object: ObjectId::Activity,
            ..
        }
    ));
    drop(client);
    assert!(service.join().unwrap().is_err());
    assert_eq!(session.activity_image(None).unwrap(), image);
    assert!(OpenSpec::Path(path).open().unwrap().activity().is_some());
}

#[cfg(unix)]
struct RemoteRig {
    wire: std::os::unix::net::UnixStream,
    client: volna_core::remote::client::RemoteClient,
    service: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    budget: volna_trace::remote::memory::MemoryBudget,
    commands: Vec<volna_trace::remote::transport::Command>,
}

#[cfg(unix)]
impl RemoteRig {
    fn open(source: Arc<dyn Session>) -> (Self, Arc<dyn Session>) {
        use volna_core::remote::client::RemoteClient;
        let (wire, server) = std::os::unix::net::UnixStream::pair().unwrap();
        for stream in [&wire, &server] {
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
        }
        let service = std::thread::spawn(move || {
            server::serve(
                server.try_clone().unwrap(),
                server,
                89,
                || Ok(source),
                || Ok(()),
            )
        });
        let budget = volna_trace::remote::memory::MemoryBudget::new(16 << 20);
        let client = RemoteClient::new(TraceId::A, 1, 8 << 20, budget.clone()).unwrap();
        let mut rig = Self {
            wire,
            client,
            service: Some(service),
            budget,
            commands: vec![],
        };
        let mut opened = None;
        rig.pump(|result| {
            if let LoadResult::Opened { result, .. } = result {
                opened = Some(result.unwrap());
            }
        });
        (rig, opened.expect("remote Open completed"))
    }

    fn pump(&mut self, mut deliver: impl FnMut(LoadResult)) {
        use volna_core::remote::ClientStep;
        use volna_trace::remote::transport::*;
        while let Some(command) = self.client.take_command().unwrap() {
            let expected = match &command.body {
                Body::Command(Command::Signals(ids)) => ids.len(),
                _ => 1,
            };
            if let Body::Command(c) = &command.body {
                self.commands.push(c.clone());
            }
            write_packet(&mut self.wire, &command).unwrap();
            let mut completed = 0;
            loop {
                let packet = read_packet(&mut self.wire).unwrap().unwrap();
                // Open uses many End frames; it completes at its last page.
                let object_end = matches!(packet.body, Body::End | Body::Error { .. });
                let opening = command.session == 0;
                let mut step = self.client.accept(packet).unwrap();
                let mut opened = false;
                loop {
                    match step {
                        ClientStep::Yield => step = self.client.step().unwrap(),
                        ClientStep::Ack(ack) => {
                            write_packet(&mut self.wire, &ack).unwrap();
                            break;
                        }
                        ClientStep::Complete { ack, result } => {
                            opened = matches!(result, LoadResult::Opened { .. });
                            deliver(result);
                            write_packet(&mut self.wire, &ack).unwrap();
                            break;
                        }
                    }
                }
                if opening {
                    if opened {
                        break;
                    }
                } else {
                    if object_end {
                        completed += 1;
                    }
                    if completed == expected {
                        break;
                    }
                }
            }
        }
    }

    fn settle(&mut self, app: &mut App) {
        for _ in 0..30 {
            let requests = app.take_requests();
            if requests.is_empty() {
                return;
            }
            for request in requests {
                if request.remote_id().is_some() {
                    if let Err(result) = self.client.submit(request) {
                        app.deliver(result);
                    }
                } else {
                    app.deliver(request.perform());
                }
            }
            self.pump(|result| app.deliver(result));
        }
        panic!("remote activity did not settle");
    }
}

#[cfg(unix)]
impl Drop for RemoteRig {
    fn drop(&mut self) {
        let _ = self.wire.shutdown(std::net::Shutdown::Both);
        if let Some(service) = self.service.take() {
            let _ = service.join().unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn remote_build_and_scope_counts_match_local_without_pan_index_requests() {
    use volna_trace::remote::transport::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote-built.vtr");
    write_trace(&path, 731);
    let local = OpenSpec::Path(path).open().unwrap();
    let (mut rig, remote) = RemoteRig::open(local.clone());
    assert!(remote.activity().is_none());
    assert!(!remote.activity_available());
    assert_eq!(remote.activity_identity(), local.activity_identity());
    let mut app = App::default();
    app.set_session(remote.clone());
    rig.settle(&mut app);
    let views = build_views(&app);
    assert_eq!(views.len(), 1);
    assert!(views[0].message().contains(remote.activity_host().unwrap()));
    let undo = app.undo_label().map(str::to_owned);
    app.handle(volna_core::app::Command::BuildActivity(TraceId::A));
    rig.settle(&mut app);
    assert!(remote.activity().is_some());
    assert!(build_views(&app).is_empty());
    assert_eq!(app.undo_label(), undo.as_deref());
    assert_eq!(
        rig.commands
            .iter()
            .filter(|c| matches!(c, Command::Activity { build: true }))
            .count(),
        1
    );
    for (a, b) in [
        (0, 0),
        (0, 80000),
        (5003, 5004),
        (5000, 5001),
        (70000, 70030),
        (70001, 70001),
    ] {
        view(&mut app, a, b);
        rig.settle(&mut app);
        check_exact(&app, &local, a, b);
    }
    assert_eq!(
        rig.commands
            .iter()
            .filter(|c| matches!(c, Command::Activity { .. }))
            .count(),
        1
    );
    assert!(
        rig.commands
            .iter()
            .any(|c| matches!(c, Command::Signals(_))),
        "undecided windows use complete signal reads"
    );
    assert!(rig.budget.used() > remote.activity().unwrap().memory_bytes());
}

#[cfg(unix)]
#[test]
fn remote_cached_index_loads_once_even_when_build_policy_is_never() {
    use volna_trace::remote::transport::Command;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote-cached.vtr");
    write_trace(&path, 738);
    index_beside(&path);
    let local = OpenSpec::Path(path).open().unwrap();
    let (mut rig, remote) = RemoteRig::open(local.clone());
    assert!(remote.activity_available());
    let mut app = App::default();
    app.settings_loaded(r#"{"hierarchy.activityIndex":"never"}"#);
    app.set_session(remote.clone());
    rig.settle(&mut app);
    assert!(remote.activity().is_some());
    assert!(rig.commands.contains(&Command::Activity { build: false }));
    assert!(!rig.commands.contains(&Command::Activity { build: true }));
    view(&mut app, 70000, 80000);
    rig.settle(&mut app);
    check_exact(&app, &local, 70000, 80000);
}

#[cfg(unix)]
#[test]
fn remote_exact_reads_span_multiple_complete_history_batches() {
    use volna_trace::remote::transport::{Command, MAX_BATCH};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("batch.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let top = Some(
        writer
            .add_scope(None, "top", ScopeType::Module, "top")
            .unwrap(),
    );
    let signals: Vec<_> = (0..MAX_BATCH * 2 + 7)
        .map(|i| {
            writer
                .add_var(
                    top,
                    &format!("s{i}"),
                    VarType::Reg,
                    Direction::Implicit,
                    SignalKind::Bits {
                        width: 32,
                        states: 2,
                    },
                )
                .unwrap()
                .1
        })
        .collect();
    for step in 0..1000 {
        writer.set_time(step * 10).unwrap();
        for (i, signal) in signals.iter().enumerate() {
            writer.emit_u64(*signal, step + i as u64).unwrap();
        }
    }
    writer.close().unwrap();
    index_beside(&path);
    let local = OpenSpec::Path(path).open().unwrap();
    assert!(local.activity().unwrap().classify(15, 15).undecided.len() > MAX_BATCH * 2);
    let (mut rig, remote) = RemoteRig::open(local.clone());
    let mut app = App::default();
    app.set_session(remote);
    rig.settle(&mut app);
    for (t0, t1) in [(15, 15), (20, 20)] {
        view(&mut app, t0, t1);
        let before = rig.commands.len();
        rig.settle(&mut app);
        check_exact(&app, &local, t0, t1);
        assert_eq!(
            rig.commands[before..]
                .iter()
                .filter(|c| matches!(c, Command::Signals(_)))
                .count(),
            3
        );
    }
}

#[cfg(unix)]
#[test]
fn remote_fst_build_and_exact_windows_match_local() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote.fst");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/vtr-cli/tests/fixtures/activity/zlib.fst"),
        &path,
    )
    .unwrap();
    let local = OpenSpec::Path(path).open().unwrap();
    let (mut rig, remote) = RemoteRig::open(local.clone());
    let mut app = App::default();
    app.set_session(remote.clone());
    rig.settle(&mut app);
    app.handle(volna_core::app::Command::BuildActivity(TraceId::A));
    rig.settle(&mut app);
    assert!(remote.activity().is_some());
    for (t0, t1) in [(0, 0), (0, 20_000_000), (100_000, 100_004)] {
        view(&mut app, t0, t1);
        rig.settle(&mut app);
        check_exact(&app, &local, t0, t1);
    }
}

#[cfg(unix)]
#[test]
fn a_client_disconnect_during_an_authorized_server_build_keeps_the_cache() {
    struct GatedBuild {
        inner: Arc<dyn Session>,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl Session for GatedBuild {
        fn info(&self) -> &volna_trace::data::TraceInfo {
            self.inner.info()
        }
        fn hierarchy(&self) -> &volna_trace::data::Hierarchy {
            self.inner.hierarchy()
        }
        fn load_signal(
            &self,
            s: SignalRef,
        ) -> anyhow::Result<Arc<dyn volna_trace::data::SignalHistory>> {
            self.inner.load_signal(s)
        }
        fn activity(&self) -> Option<Arc<volna_trace::data::ActivityIndex>> {
            self.inner.activity()
        }
        fn activity_identity(&self) -> Option<vtr::activity::Identity> {
            self.inner.activity_identity()
        }
        fn activity_build_info(&self) -> Option<volna_trace::data::ActivityBuildInfo> {
            self.inner.activity_build_info()
        }
        fn activity_image(&self, cache: Option<&Path>) -> anyhow::Result<Vec<u8>> {
            self.inner.activity_image(cache)
        }
        fn build_activity(
            &self,
            options: &vtr::activity::BuildOptions,
            budget: &volna_trace::remote::memory::MemoryBudget,
            cache: Option<&Path>,
        ) -> anyhow::Result<()> {
            self.entered.send(())?;
            self.release
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))?;
            self.inner.build_activity(options, budget, cache)
        }
    }
    use volna_trace::remote::transport::write_packet;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("disconnected.vtr");
    write_trace(&path, 993);
    let source = OpenSpec::Path(path.clone()).open().unwrap();
    let (entered, started) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let (mut rig, remote) = RemoteRig::open(Arc::new(GatedBuild {
        inner: source.clone(),
        entered,
        release: std::sync::Mutex::new(gate),
    }));
    rig.client
        .submit(LoadRequest::BuildActivity {
            trace: TraceId::A,
            generation: 1,
            session: remote.clone(),
            options: vtr::activity::BuildOptions {
                control: Some(Arc::new(vtr::activity::BuildControl::default())),
                ..Default::default()
            },
            budget: rig.budget.clone(),
            cache_dir: None,
        })
        .unwrap_or_else(|_| panic!("build rejected"));
    let command = rig.client.take_command().unwrap().unwrap();
    write_packet(&mut rig.wire, &command).unwrap();
    started
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(source.activity().is_none());
    rig.wire.shutdown(std::net::Shutdown::Both).unwrap();
    let results = rig.client.disconnect("lost connection during build");
    assert!(matches!(
        results.as_slice(),
        [LoadResult::ActivityBuilt { result: Err(_), .. }]
    ));
    release.send(()).unwrap();
    assert!(rig.service.take().unwrap().join().unwrap().is_err());
    assert!(
        remote.activity().is_none(),
        "a disconnected client installs no response"
    );
    assert!(source.activity().is_some());
    assert!(
        OpenSpec::Path(path).open().unwrap().activity().is_some(),
        "another client reuses the completed cache"
    );
}
