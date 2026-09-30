//! Headless tests of stacked areas (`docs/stacked-areas.html`): the
//! arithmetic of a stack over random histories (column means add up to the
//! total's mean, both drawing paths agree, the whole-trace walk, stepping
//! through the sum), stacking and unstacking with their undo steps and
//! heights, the group menu, workspaces, the painted row (layer order, gaps,
//! the undefined fill, the net line, cells, swatches and the readout) and
//! the feature showcase's four stackable scopes.

use std::path::PathBuf;
use std::sync::Arc;

use volna_core::Theme;
use volna_core::app::{Action, App, Command};
use volna_core::data::history::VecHistory;
use volna_core::data::{NumericKind, SignalHistory, SignalShape, WaveValue};
use volna_core::geometry::{Point, Rect, point};
use volna_core::panels::PanelId;
use volna_core::scene::{MonoMeasure, Prim, Scene};
use volna_core::session::{OpenSpec, Session};
use volna_core::testing::a;
use volna_core::trace::TraceId;
use volna_core::wave::analog::Sample;
use volna_core::wave::model::{MenuAction, MenuEntry};
use volna_core::wave::stack::{self, IntegralSummary, Layer, Reading, TotalSummary};
use volna_core::wave::viewport::Viewport;
use volna_core::wave::{GroupStyle, PointerEvent};
use volna_core::workspace::Workspace;

const TRACE: &str = "file:///tmp/stack.vtr";
const LOCATION: &str = "file:///tmp/stack.vtr.volna.json";

// -- the arithmetic, over random histories -----------------------------------

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// A random member: a 4-state bit, an 8-bit vector read unsigned or
/// signed, or a real of either sign; changes at unaligned times, stretches
/// of X, and sometimes no value before a late first change.
fn member(rng: &mut Rng, k: usize) -> Layer {
    member_until(rng, k, 10_000)
}

fn member_until(rng: &mut Rng, k: usize, end: u64) -> Layer {
    let kind = rng.below(4);
    let (shape, reading) = match kind {
        0 => (SignalShape::Bit, Reading::Bit),
        1 => (
            SignalShape::Vector { width: 8 },
            Reading::Number(NumericKind::Unsigned),
        ),
        2 => (
            SignalShape::Vector { width: 8 },
            Reading::Number(NumericKind::Signed),
        ),
        _ => (SignalShape::Real, Reading::Number(NumericKind::Real)),
    };
    let late = rng.below(3) == 0;
    let mut t = if late { 200 + rng.below(800) } else { 0 };
    let (mut times, mut values) = (Vec::new(), Vec::new());
    while t < end {
        let x = rng.below(12) == 0;
        values.push(match (kind, x) {
            (0, true) => WaveValue::Bits("x".into()),
            (0, false) => WaveValue::Bits(format!("{}", rng.below(2))),
            (3, true) => WaveValue::Real(f64::NAN),
            (3, false) => WaveValue::Real((rng.below(2001) as f64 - 1000.0) / 64.0),
            (_, true) => WaveValue::Bits("xxxxxxxx".into()),
            (_, false) => WaveValue::Bits(format!("{:08b}", rng.below(256))),
        });
        times.push(t);
        // Sometimes two changes at one time: the last one holds.
        t += if rng.below(10) == 0 {
            0
        } else {
            1 + rng.below(60)
        };
    }
    let history: Arc<dyn SignalHistory> = Arc::new(VecHistory {
        shape,
        times,
        values,
        initial: WaveValue::Unavailable,
    });
    Layer::new(k, history, reading)
}

fn members(seed: u64) -> Vec<Layer> {
    let mut rng = Rng(seed);
    let n = 1 + rng.below(5) as usize;
    (0..n).map(|k| member(&mut rng, k)).collect()
}

/// The sum over `[ta, tb)` split at every change of any layer: its
/// integral, or `None` when some layer is undefined for a while there.
fn brute_integral(layers: &[Layer], ta: f64, tb: f64) -> Option<f64> {
    let mut cuts: Vec<f64> = vec![ta, tb];
    for l in layers {
        let h = l.history.as_ref();
        cuts.extend(
            (0..h.len())
                .map(|i| h.time(i) as f64)
                .filter(|&t| t > ta && t < tb),
        );
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let mut sum = 0.0;
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let at = |l: &Layer| {
            let i = if a < 0.0 {
                None
            } else {
                l.history.index_at(a.floor() as u64)
            };
            l.sample(i)
        };
        let values: Vec<Sample> = layers.iter().map(at).collect();
        sum += stack::total(&values)? * (b - a);
    }
    Some(sum)
}

#[test]
fn column_means_add_up_to_the_mean_of_the_total() {
    for seed in 1..40u64 {
        let layers = members(0x9e37_79b9 ^ (seed * 7919));
        for (start, end, width) in [
            (0.0, 10_000.0, 300),
            (-150.0, 2_500.5, 211),
            (4_000.25, 4_400.75, 97),
        ] {
            let vp = Viewport { start, end };
            let frame = stack::columns(&layers, &vp, width);
            assert!(!frame.exact);
            for c in 0..width {
                let (ta, tb) = (
                    vp.time_at(c as f64, width as f64),
                    vp.time_at(c as f64 + 1.0, width as f64),
                );
                let values = frame.values(c);
                match (stack::total(values), brute_integral(&layers, ta, tb)) {
                    (Some(means), Some(integral)) => {
                        let mean = integral / (tb - ta);
                        assert!(
                            (means - mean).abs() <= 1e-9 * mean.abs().max(1.0),
                            "seed {seed} column {c}: {means} vs {mean}"
                        );
                    }
                    (None, None) => {}
                    (means, brute) => panic!("seed {seed} column {c}: {means:?} vs {brute:?}"),
                }
                // Bits count 0 or 1, so their means lie between.
                for (l, v) in layers.iter().zip(values) {
                    if let (Reading::Bit, Sample::Value(v)) = (l.reading, v) {
                        assert!((0.0..=1.0).contains(v), "seed {seed}: bit mean {v}");
                    }
                }
            }
        }
    }
}

#[test]
fn exact_steps_and_columns_agree_where_a_column_holds_no_change() {
    for seed in 1..30u64 {
        let layers = members(0x2545_f491 ^ (seed * 104_729));
        let vp = Viewport {
            start: 3_000.0,
            end: 3_600.0,
        };
        let width = 1_200;
        let steps = stack::steps(&layers, &vp, width);
        let columns = stack::columns(&layers, &vp, width);
        assert!(steps.exact);
        for c in 0..width {
            let (ta, tb) = (
                vp.time_at(c as f64, width as f64),
                vp.time_at(c as f64 + 1.0, width as f64),
            );
            let inside = layers.iter().any(|l| {
                let h = l.history.as_ref();
                (0..h.len()).any(|i| (h.time(i) as f64) > ta && (h.time(i) as f64) < tb)
            });
            if inside {
                continue;
            }
            let span = steps.span_at(c as f64 + 0.5).expect("steps cover the view");
            for (k, (s, m)) in steps.values(span).iter().zip(columns.values(c)).enumerate() {
                match (s, m) {
                    (Sample::Value(s), Sample::Value(m)) => {
                        assert!(
                            (s - m).abs() < 1e-9,
                            "seed {seed} column {c} layer {k}: {s} vs {m}"
                        )
                    }
                    (Sample::Value(_), _) | (_, Sample::Value(_)) => {
                        panic!("seed {seed} column {c} layer {k}: {s:?} vs {m:?}")
                    }
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn a_stack_draws_negative_parts_below_zero_and_stops_at_an_undefined_layer() {
    let mut bands = Vec::new();
    let values = [
        Sample::Value(2.0),
        Sample::Value(-1.5),
        Sample::Value(3.0),
        Sample::Value(-0.5),
    ];
    let c = stack::compose(&values, |k, lo, hi| bands.push((k, lo, hi)));
    // The last layer is on the baseline; negatives grow down from zero.
    assert_eq!(
        bands,
        [
            (3, -0.5, 0.0),
            (2, 0.0, 3.0),
            (1, -2.0, -0.5),
            (0, 3.0, 5.0)
        ]
    );
    assert_eq!((c.top, c.bottom, c.undefined), (5.0, -2.0, false));
    assert_eq!(stack::total(&values), Some(3.0));
    let mut drawn = Vec::new();
    let values = [Sample::Value(1.0), Sample::Undefined, Sample::Value(2.0)];
    let c = stack::compose(&values, |k, _, _| drawn.push(k));
    assert_eq!((drawn, c.undefined, c.top), (vec![2], true, 2.0));
    assert_eq!(stack::total(&values), None, "the sum reads X");
    assert_eq!(
        stack::total(&[Sample::Missing]),
        None,
        "missing is not zero"
    );
}

#[test]
fn the_whole_trace_walk_finds_the_highest_top_and_lowest_bottom() {
    for seed in 1..40u64 {
        let layers = members(0x5bd1_e995 ^ (seed * 31));
        let summary = TotalSummary::build(&layers);
        let mut times: Vec<u64> = layers
            .iter()
            .flat_map(|l| {
                (0..l.history.len())
                    .map(|i| l.history.time(i))
                    .collect::<Vec<_>>()
            })
            .collect();
        times.sort_unstable();
        times.dedup();
        let mut range = (0.0f64, 0.0f64);
        let initial: Vec<Sample> = layers.iter().map(|l| l.sample(None)).collect();
        for values in
            std::iter::once(initial).chain(times.iter().map(|&t| stack::values_at(&layers, t)))
        {
            let c = stack::compose(&values, |_, _, _| {});
            range = (range.0.min(c.bottom), range.1.max(c.top));
        }
        let (lo, hi) = summary.range();
        assert!(
            (lo - range.0).abs() < 1e-6 && (hi - range.1).abs() < 1e-6,
            "seed {seed}: {:?} vs {range:?}",
            summary.range()
        );
        assert_eq!(summary.key(), &stack::key(&layers));
    }
    assert_eq!(stack::clean_range((0.0, 4.2)), (0.0, 5.0));
    assert_eq!(stack::clean_range((-13.0, 151.0)), (-15.0, 200.0));
    assert_eq!(stack::clean_range((0.0, 0.0)), (0.0, 1.0));
    assert_eq!(stack::clean_range((0.0, 6.0)), (0.0, 6.0));
}

fn bits(times: &[(u64, &str)]) -> Layer {
    let history = Arc::new(VecHistory {
        shape: SignalShape::Bit,
        times: times.iter().map(|(t, _)| *t).collect(),
        values: times
            .iter()
            .map(|(_, v)| WaveValue::Bits((*v).into()))
            .collect(),
        initial: WaveValue::Unavailable,
    });
    Layer::new(0, history, Reading::Bit)
}

#[test]
fn stepping_through_the_sum_skips_changes_that_cancel_out() {
    // At 10 one bit rises as the other falls: the sum stays 1.
    let layers = [
        bits(&[(0, "0"), (10, "1"), (30, "0")]),
        bits(&[(0, "1"), (10, "0"), (20, "1")]),
    ];
    assert_eq!(stack::total_edge(&layers, 0, true, 100), Some(20));
    assert_eq!(stack::total_edge(&layers, 20, true, 100), Some(30));
    assert_eq!(stack::total_edge(&layers, 30, false, 100), Some(20));
    assert_eq!(stack::total_edge(&layers, 20, false, 100), Some(0));
    assert_eq!(stack::total_edge(&layers, 30, true, 100), None);
    // Weak drives count as their level; X and don't-care do not.
    let weak = bits(&[(0, "h"), (5, "l"), (6, "-")]);
    assert_eq!(weak.at(0), Sample::Value(1.0));
    assert_eq!(weak.at(5), Sample::Value(0.0));
    assert_eq!(weak.at(6), Sample::Undefined);
}

// -- integral summaries (stage 2) ----------------------------------------------

#[test]
fn integral_prefixes_equal_a_direct_walk_at_every_block_boundary() {
    for seed in 1..20u64 {
        let mut rng = Rng(0x7f4a_7c15 ^ (seed * 131));
        let layer = member_until(&mut rng, 0, 20_000);
        let h = layer.history.as_ref();
        let summary = IntegralSummary::build(&layer.history, layer.reading);
        let direct = |k: usize| {
            (0..k).fold((0.0, 0u32), |(sum, count), j| {
                let dur = (h.time(j + 1) - h.time(j)) as f64;
                match layer.sample(Some(j)) {
                    Sample::Value(v) => (sum + v * dur, count),
                    _ if dur > 0.0 => (sum, count + 1),
                    _ => (sum, count),
                }
            })
        };
        let len = h.len();
        let b = stack::INTEGRAL_BLOCK;
        for k in [
            0,
            1,
            b - 1,
            b,
            b + 1,
            b + b / 2,
            b + b / 2 + 1,
            2 * b - 1,
            2 * b,
            len / 2,
            len - 2,
            len - 1,
        ] {
            let ((s, u), (ds, du)) = (summary.prefix(h, k), direct(k));
            assert!(
                (s - ds).abs() <= 1e-9 * ds.abs().max(1.0),
                "seed {seed} k {k}: {s} vs {ds}"
            );
            assert_eq!(u, du, "seed {seed} k {k}");
        }
        // One f64 and one u32 per block of 64 changes.
        let blocks = (len - 2) / b + 1;
        assert_eq!(summary.resident_bytes(), blocks as u64 * 12);
    }
    // The last change's prefix, also where it starts a block.
    for len in [2usize, 64, 65, 66, 129] {
        let h: Arc<dyn SignalHistory> = Arc::new(VecHistory {
            shape: SignalShape::Bit,
            times: (0..len as u64).map(|t| 3 * t).collect(),
            values: (0..len)
                .map(|i| WaveValue::Bits(format!("{}", i % 2)))
                .collect(),
            initial: WaveValue::Unavailable,
        });
        let summary = IntegralSummary::build(&h, Reading::Bit);
        let ones = (len - 1) / 2;
        assert_eq!(
            summary.prefix(h.as_ref(), len - 1),
            (3.0 * ones as f64, 0),
            "{len}"
        );
    }
}

#[test]
fn column_means_from_summaries_equal_the_walked_ones() {
    for seed in 1..20u64 {
        let mut rng = Rng(0x94d0_49bb ^ (seed * 7));
        let walked: Vec<Layer> = (0..3).map(|k| member_until(&mut rng, k, 100_000)).collect();
        let summarized: Vec<Layer> = walked
            .iter()
            .map(|l| {
                let s = Arc::new(IntegralSummary::build(&l.history, l.reading));
                l.clone().with_summary(Some(s))
            })
            .collect();
        assert!(summarized.iter().all(|l| l.summary.is_some()));
        for (start, end, width) in [
            (0.0, 100_000.0, 97),
            (-500.5, 60_000.25, 64),
            (20_000.0, 90_000.0, 40),
        ] {
            let vp = Viewport { start, end };
            let a = stack::columns(&walked, &vp, width);
            let b = stack::columns(&summarized, &vp, width);
            for c in 0..width {
                assert_eq!(a.changes(c), b.changes(c), "seed {seed} column {c}");
                for (k, (x, y)) in a.values(c).iter().zip(b.values(c)).enumerate() {
                    match (x, y) {
                        (Sample::Value(x), Sample::Value(y)) => assert!(
                            (x - y).abs() <= 1e-9 * x.abs().max(1.0),
                            "seed {seed} {start}..{end} column {c} layer {k}: {x} vs {y}"
                        ),
                        (Sample::Undefined, Sample::Undefined) => {}
                        _ => panic!("seed {seed} column {c} layer {k}: {x:?} vs {y:?}"),
                    }
                }
            }
        }
    }
}

#[test]
fn a_long_layer_waits_for_its_summary_rather_than_walk() {
    let mut rng = Rng(99);
    let mut layers: Vec<Layer> = (0..2)
        .map(|k| member_until(&mut rng, k, 1_000_000))
        .collect();
    let vp = Viewport {
        start: 0.0,
        end: 1_000_000.0,
    };
    layers[0].building = true;
    assert!(stack::frame(&layers, &vp, 800).waiting());
    let s = Arc::new(IntegralSummary::build(
        &layers[0].history,
        layers[0].reading,
    ));
    layers[0] = layers[0].clone().with_summary(Some(s));
    assert!(!stack::frame(&layers, &vp, 800).waiting());
    // A summary of another reading is not used.
    let other = Arc::new(IntegralSummary::build(
        &layers[1].history,
        Reading::Number(NumericKind::Float),
    ));
    assert!(
        layers[1]
            .clone()
            .with_summary(Some(other))
            .summary
            .is_none()
    );
}

/// Two one-bit valids changing 30,000 and 20,000 times: long enough for
/// summaries.
fn long_trace() -> (tempfile::NamedTempFile, Arc<dyn Session>) {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    w.set_timescale(-9).unwrap();
    let top = w
        .add_scope(None, "top", vtr::ScopeType::Module, "top")
        .unwrap();
    let ids: Vec<_> = ["a", "b"]
        .iter()
        .map(|n| {
            w.add_var(
                Some(top),
                n,
                vtr::VarType::Wire,
                vtr::Direction::Output,
                vtr::SignalKind::Bits {
                    width: 1,
                    states: 2,
                },
            )
            .unwrap()
            .1
        })
        .collect();
    for t in 0..30_000u64 {
        w.set_time(t).unwrap();
        w.emit_bit(ids[0], (t & 1) as u8).unwrap();
        w.emit_bit(ids[1], u8::from(t % 3 == 0)).unwrap();
    }
    w.close().unwrap();
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    (file, session)
}

#[test]
fn long_stacks_hold_integral_summaries_until_their_histories_go() {
    let (_f, session) = long_trace();
    let mut app = App::new();
    app.set_session(session);
    pump(&mut app);
    let top = scope(&app, &["top"]);
    app.handle(Command::AddScopeAsGroup {
        scope: a(top),
        recursive: false,
    });
    pump(&mut app);
    let id = app.panels.focused_id();
    let layers = app.panels.waves(id).unwrap().stack_layers(0);
    assert!(
        layers
            .iter()
            .all(|l| app.doc.integral(&l.history, l.reading).is_none()),
        "not stacked yet"
    );
    app.handle(Command::Action(Action::ToggleStack));
    // Queued for the worker: until they arrive the frame waits.
    assert!(layers.iter().all(|l| matches!(
        app.doc.integral(&l.history, l.reading),
        Some(stack::IntegralLoad::Building { .. })
    )));
    let App { panels, doc, .. } = &mut app;
    panels.waves_mut(id).unwrap().nav.jump_to(
        doc,
        Viewport {
            start: 0.0,
            end: 30_000.0,
        },
    );
    let shown = texts(frame(&mut app, id));
    assert!(shown.iter().any(|s| s == "Summarizing…"), "{shown:?}");
    pump(&mut app);
    for l in &layers {
        let Some(stack::IntegralLoad::Ready(s)) = app.doc.integral(&l.history, l.reading) else {
            panic!("no summary");
        };
        assert!(s.matches(&l.history, l.reading));
    }
    let shown = texts(frame(&mut app, id));
    assert!(!shown.iter().any(|s| s == "Summarizing…"), "{shown:?}");
    // Unstacked, the summaries go; removed, so do the histories.
    app.handle(Command::Action(Action::ToggleStack));
    assert!(
        layers
            .iter()
            .all(|l| app.doc.integral(&l.history, l.reading).is_none())
    );
    let weak: Vec<_> = layers.iter().map(|l| Arc::downgrade(&l.history)).collect();
    drop(layers);
    app.handle(Command::Action(Action::RemoveSelected));
    app.history.clear();
    assert!(
        weak.iter().all(|w| w.strong_count() == 0),
        "the histories are released with their rows"
    );
}

// -- the panel ---------------------------------------------------------------

/// `top` holds, in order: `p` (4-bit count 2), `q` (4-bit count 3), `r`
/// (a bit, 1), `mode` (text) and `tick` (an event), over 0–100 ns; `q` is X
/// at 40–50 ns and `r` falls at 60 ns. `other.flow` is a real, 0.5 and
/// −1.5 from 70 ns.
fn trace() -> (tempfile::NamedTempFile, Arc<dyn Session>) {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    w.set_timescale(-9).unwrap();
    let top = w
        .add_scope(None, "top", vtr::ScopeType::Module, "top")
        .unwrap();
    let var = |w: &mut vtr::Writer, name: &str, typ, kind| {
        w.add_var(Some(top), name, typ, vtr::Direction::Output, kind)
            .unwrap()
            .1
    };
    let p = var(
        &mut w,
        "p",
        vtr::VarType::Wire,
        vtr::SignalKind::Bits {
            width: 4,
            states: 4,
        },
    );
    let q = var(
        &mut w,
        "q",
        vtr::VarType::Wire,
        vtr::SignalKind::Bits {
            width: 4,
            states: 4,
        },
    );
    let r = var(
        &mut w,
        "r",
        vtr::VarType::Wire,
        vtr::SignalKind::Bits {
            width: 1,
            states: 2,
        },
    );
    let mode = var(
        &mut w,
        "mode",
        vtr::VarType::String,
        vtr::SignalKind::VarLen,
    );
    let tick = var(
        &mut w,
        "tick",
        vtr::VarType::Event,
        vtr::SignalKind::Bits {
            width: 1,
            states: 2,
        },
    );
    let other = w
        .add_scope(None, "other", vtr::ScopeType::Module, "other")
        .unwrap();
    let (_, flow) = w
        .add_var(
            Some(other),
            "flow",
            vtr::VarType::Real,
            vtr::Direction::Output,
            vtr::SignalKind::Real,
        )
        .unwrap();
    for t in [0u64, 40, 50, 60, 70, 100] {
        w.set_time(t).unwrap();
        if t == 0 {
            w.emit_u64(p, 2).unwrap();
            w.emit_bit(r, 1).unwrap();
            w.emit_varlen(mode, b"run").unwrap();
            w.emit_real(flow, 0.5).unwrap();
        }
        match t {
            40 => w.emit_logic_str(q, b"xxxx").unwrap(),
            0 | 50 => w.emit_u64(q, 3).unwrap(),
            60 => w.emit_bit(r, 0).unwrap(),
            70 => w.emit_real(flow, -1.5).unwrap(),
            _ => {}
        }
        w.emit_bit(tick, 1).unwrap();
    }
    w.close().unwrap();
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    (file, session)
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
}

fn frame(app: &mut App, id: PanelId) -> &Scene {
    let theme = Theme::one_dark();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 600.0), &theme)
        .unwrap();
    app.render_panel(id, &theme, &mut MonoMeasure)
}

fn scope(app: &App, path: &[&str]) -> volna_core::data::ScopeId {
    match app.doc.hierarchy(TraceId::A).unwrap().find_scope(path) {
        volna_core::data::source::Lookup::Found(s) => s,
        other => panic!("{path:?}: {other:?}"),
    }
}

/// The `top` group, selected, in a fresh app over [`trace`].
fn app() -> (tempfile::NamedTempFile, App, PanelId) {
    let (file, session) = trace();
    let mut app = App::new();
    app.set_session(session);
    pump(&mut app);
    let top = scope(&app, &["top"]);
    app.handle(Command::AddScopeAsGroup {
        scope: a(top),
        recursive: false,
    });
    pump(&mut app);
    let id = app.panels.focused_id();
    frame(&mut app, id);
    (file, app, id)
}

fn cursor(app: &mut App, id: PanelId, t: u64) {
    let App { panels, doc, .. } = app;
    panels.waves_mut(id).unwrap().set_cursor(doc, Some(t));
}

fn group(app: &App, id: PanelId) -> volna_core::wave::GroupRow {
    app.panels.waves(id).unwrap().items()[0]
        .group()
        .unwrap()
        .clone()
}

fn texts(scene: &Scene) -> Vec<String> {
    scene.texts().map(str::to_owned).collect()
}

#[test]
fn shift_a_stacks_the_selected_group_as_one_undoable_step() {
    let (_f, mut app, id) = app();
    assert_eq!(group(&app, id).style, GroupStyle::Activity);
    app.handle(Command::Action(Action::ToggleStack));
    let g = group(&app, id);
    assert_eq!((g.style, g.height.multiple()), (GroupStyle::Stack, 3));
    assert_eq!(app.undo_label(), Some("Stack top"));
    // The layers: every signal with a number, in list order.
    let w = app.panels.waves(id).unwrap();
    let names: Vec<&str> = w
        .stack_layers(0)
        .iter()
        .map(|l| w.items()[l.entry].name())
        .collect();
    assert_eq!(names, ["p", "q", "r"]);
    app.handle(Command::Undo);
    let g = group(&app, id);
    assert_eq!((g.style, g.height.multiple()), (GroupStyle::Activity, 1));
    app.handle(Command::Redo);
    assert_eq!(group(&app, id).style, GroupStyle::Stack);
    // Unstacking gives 1× back, unless the row was resized meanwhile.
    app.handle(Command::Action(Action::ToggleStack));
    assert_eq!(app.undo_label(), Some("Unstack top"));
    assert_eq!(group(&app, id).height.multiple(), 1);
    app.handle(Command::Action(Action::ToggleStack));
    app.handle(Command::Action(Action::IncreaseRowHeight));
    app.handle(Command::Action(Action::ToggleStack));
    assert_eq!(group(&app, id).height.multiple(), 4);
    // Folding is navigation, not a step.
    app.handle(Command::Action(Action::ToggleStack));
    let label = app.undo_label().map(str::to_owned);
    app.handle(Command::Action(Action::PanLeft));
    assert!(group(&app, id).collapsed);
    assert_eq!(app.undo_label(), label.as_deref());
}

#[test]
fn the_group_menu_draws_it_as_activity_or_a_stacked_area() {
    let (_f, mut app, id) = app();
    let w = app.panels.waves_mut(id).unwrap();
    w.open_signal_menu(&app.doc, 0, Point::default());
    let menu = w.menu.clone().unwrap();
    let draw = menu
        .entries
        .iter()
        .position(|e| matches!(e, MenuEntry::Label(l) if l == "Draw"))
        .expect("a Draw section");
    let item = |k: usize| match &menu.entries[draw + k] {
        MenuEntry::Item(item) => item.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        (item(1).label.as_str(), item(1).checked),
        ("Activity", true)
    );
    assert_eq!(
        (
            item(2).label.as_str(),
            item(2).badge.as_deref(),
            item(2).checked
        ),
        ("Stacked area", Some("⇧A"), false)
    );
    app.handle(Command::MenuSelect(id, MenuAction::Stack(true)));
    assert_eq!(group(&app, id).style, GroupStyle::Stack);
    assert_eq!(app.undo_label(), Some("Stack top"));
    // A signal row's menu has no such section.
    let w = app.panels.waves_mut(id).unwrap();
    w.selected.clear();
    w.open_signal_menu(&app.doc, 1, Point::default());
    assert!(
        !w.menu
            .as_ref()
            .unwrap()
            .items()
            .any(|i| matches!(i.action, MenuAction::Stack(_)))
    );
}

#[test]
fn stacked_groups_round_trip_through_workspaces_at_version_6() {
    let (_f, mut app, _) = app();
    app.handle(Command::Action(Action::ToggleStack));
    let saved = serde_json::to_value(
        Workspace::capture(&app, volna_core::testing::paths(TRACE), None).unwrap(),
    )
    .unwrap();
    let panel = &saved["panels"][0];
    assert_eq!(panel["version"], 6);
    assert_eq!(panel["rows"][0]["style"], "stack");
    assert_eq!(panel["rows"][0]["height"], 3);
    let (_f2, session) = trace();
    let mut restored = App::new();
    restored.set_session(session);
    Workspace::parse(&serde_json::to_vec(&saved).unwrap())
        .and_then(|ws| ws.prepare(&restored, TRACE, LOCATION))
        .unwrap()
        .commit(&mut restored)
        .unwrap();
    let rid = restored.panels.focused_id();
    assert_eq!(group(&restored, rid).style, GroupStyle::Stack);
    let again = serde_json::to_value(
        Workspace::capture(&restored, volna_core::testing::paths(TRACE), None).unwrap(),
    )
    .unwrap();
    assert_eq!(again["panels"], saved["panels"]);
    // Activity is the default and is not written.
    app.handle(Command::Action(Action::ToggleStack));
    let plain = serde_json::to_value(
        Workspace::capture(&app, volna_core::testing::paths(TRACE), None).unwrap(),
    )
    .unwrap();
    assert!(plain["panels"][0]["rows"][0].get("style").is_none());
    // A version 5 panel is refused like any older one.
    let mut old = saved.clone();
    old["panels"][0]["version"] = 5.into();
    let mut other = App::new();
    other.set_session(trace().1);
    let plan = Workspace::parse(&serde_json::to_vec(&old).unwrap())
        .and_then(|ws| ws.prepare(&other, TRACE, LOCATION))
        .unwrap();
    assert!(
        plan.report()
            .notices
            .iter()
            .any(|r| r.contains("waves version 5")),
        "{:?}",
        plan.report().notices
    );
}

/// Filled rectangles of `color` inside `area`.
fn quads_of(scene: &Scene, area: Rect, color: volna_core::color::Color) -> Vec<Rect> {
    scene
        .quads()
        .filter(|(r, c)| *c == color && area.contains(r.origin) && r.width() > 0.0)
        .map(|(r, _)| r)
        .collect()
}

fn row_rect(app: &App, id: PanelId, pos: usize) -> Rect {
    let l = app.panels.waves(id).unwrap().last_layout();
    Rect::from_xywh(
        l.waves.left(),
        l.row_y(pos),
        l.waves.width(),
        l.row_height(pos),
    )
}

#[test]
fn the_stack_paints_layers_in_list_order_with_gaps_and_the_undefined_rest() {
    let (_f, mut app, id) = app();
    app.handle(Command::Action(Action::ToggleStack));
    let App { panels, doc, .. } = &mut app;
    panels.waves_mut(id).unwrap().nav.jump_to(
        doc,
        Viewport {
            start: 0.0,
            end: 100.0,
        },
    );
    cursor(&mut app, id, 20);
    frame(&mut app, id);
    let scene = app.scene();
    let t = Theme::one_dark();
    let row = row_rect(&app, id, 0);
    let color = |k| t.layer_fill(k, 3);
    let (p, q, r) = (
        quads_of(scene, row, color(0)),
        quads_of(scene, row, color(1)),
        quads_of(scene, row, color(2)),
    );
    assert!(
        !p.is_empty() && !q.is_empty() && !r.is_empty(),
        "every layer is painted"
    );
    // Until 40 ns the last member lies on the baseline and the first on top.
    let first = |rs: &[Rect]| {
        *rs.iter()
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .unwrap()
    };
    let (p0, q0, r0) = (first(&p), first(&q), first(&r));
    assert!(
        r0.bottom() > q0.bottom() && q0.bottom() > p0.bottom(),
        "{p0:?} {q0:?} {r0:?}"
    );
    // One pixel of canvas between neighbours.
    assert_eq!(q0.bottom(), r0.top() - 1.0, "a gap between q and r");
    assert_eq!(p0.bottom(), q0.top() - 1.0, "a gap between p and q");
    // From 60 ns r is 0, so q sits on the baseline.
    let late = |rs: &[Rect]| {
        *rs.iter()
            .max_by(|a, b| a.left().total_cmp(&b.left()))
            .unwrap()
    };
    assert_eq!(late(&q).bottom(), r0.bottom());
    // While q is X, only r is drawn and the rest of the column is hatched.
    let undefined = quads_of(scene, row, t.wave_undef_fill);
    let l = app.panels.waves(id).unwrap().last_layout();
    let x_of =
        |time: f64| l.waves.left() + (time / 100.0 * f64::from(l.waves.width().floor())) as f32;
    assert!(
        undefined
            .iter()
            .any(|u| (u.left() - x_of(40.0)).abs() <= 1.0 && (u.right() - x_of(50.0)).abs() <= 1.0),
        "{undefined:?}"
    );
    // The cells: the sum, the header line, swatches and the rows left out.
    let text = texts(scene);
    assert!(text.iter().any(|s| s == "Σ 6"), "{text:?}");
    assert!(text.iter().any(|s| s == "stacked · 3 layers"), "{text:?}");
    assert_eq!(
        text.iter().filter(|s| s.starts_with("not sta")).count(),
        2,
        "{text:?}"
    );
    let swatches = scene.prims.iter().filter(|p| matches!(p, Prim::Quad { fill, radius, .. } if *radius > 0.0 && (0..3).any(|k| *fill == color(k)))).count();
    assert_eq!(swatches, 3, "one swatch per layer row");
    // The sum reads X while a layer is undefined.
    cursor(&mut app, id, 45);
    let text = texts(frame(&mut app, id));
    assert!(text.iter().any(|s| s == "Σ X"), "{text:?}");
    // Folded, the group keeps its area.
    app.handle(Command::Action(Action::PanLeft));
    assert!(group(&app, id).collapsed);
    frame(&mut app, id);
    let scene = app.scene();
    assert!(!quads_of(scene, row_rect(&app, id, 0), color(2)).is_empty());
}

#[test]
fn signed_members_stack_below_zero_with_a_net_line() {
    let (_f, mut app, id) = app();
    // Move `flow` into the group, last: it becomes the baseline layer.
    let flow = app
        .doc
        .hierarchy(TraceId::A)
        .unwrap()
        .find_var(&["other", "flow"], None);
    let volna_core::data::source::Lookup::Found(flow) = flow else {
        panic!()
    };
    app.handle(Command::AddVars(vec![a(flow)]));
    pump(&mut app);
    let w = app.panels.waves_mut(id).unwrap();
    let end = w.items().len() - 1;
    w.selected = [end].into();
    w.anchor = Some(end);
    w.move_selected_to(volna_core::wave::tree::Place { at: end, depth: 1 });
    w.selected = [0].into();
    w.anchor = Some(0);
    app.handle(Command::Action(Action::ToggleStack));
    pump(&mut app);
    let App { panels, doc, .. } = &mut app;
    panels.waves_mut(id).unwrap().nav.jump_to(
        doc,
        Viewport {
            start: 0.0,
            end: 100.0,
        },
    );
    cursor(&mut app, id, 80);
    frame(&mut app, id);
    let scene = app.scene();
    let text = texts(scene);
    // 2 + 3 + 0 − 1.5 at 80 ns.
    assert!(text.iter().any(|s| s == "Σ 3.5"), "{text:?}");
    let t = Theme::one_dark();
    let row = row_rect(&app, id, 0);
    let inside = |p: &Point| {
        p.y >= row.top() && p.y <= row.bottom() && p.x >= row.left() && p.x <= row.right()
    };
    let net = scene.prims.iter().any(|p| matches!(p, Prim::Lines { color, segments, .. } if *color == t.editor.text && !segments.is_empty() && segments.iter().all(|[a, b]| inside(a) && inside(b))));
    assert!(net, "a net line where both signs occur: {text:?}");
    // The scale holds the negative part: its bottom label is below zero.
    assert!(text.iter().any(|s| s.starts_with('-')), "{text:?}");
    let _ = point(0.0, 0.0);
}

#[test]
fn hovering_the_stack_lists_every_layer_and_the_sum() {
    let (_f, mut app, id) = app();
    app.handle(Command::Action(Action::ToggleStack));
    let App { panels, doc, .. } = &mut app;
    panels.waves_mut(id).unwrap().nav.jump_to(
        doc,
        Viewport {
            start: 0.0,
            end: 100.0,
        },
    );
    frame(&mut app, id);
    let row = row_rect(&app, id, 0);
    // Near the bottom at 20 ns: over r, the baseline layer.
    app.handle(Command::Pointer(
        id,
        PointerEvent::Move {
            position: point(row.left() + row.width() * 0.2, row.bottom() - 7.0),
        },
    ));
    let text = texts(frame(&mut app, id));
    for want in [
        "p",
        "q",
        "r",
        "Σ 3 layers",
        "2",
        "3",
        "1",
        "33%",
        "50%",
        "17%",
    ] {
        assert!(text.iter().any(|s| s == want), "{want:?} in {text:?}");
    }
    let w = app.panels.waves(id).unwrap();
    let rows: Vec<String> = w.accessible_rows(&app.doc).map(|r| r.label).collect();
    assert_eq!(rows[0], "top, stacked area, 3 layers");
    // Shift → steps to the next change of the sum.
    let w = app.panels.waves_mut(id).unwrap();
    w.selected = [0].into();
    w.anchor = Some(0);
    cursor(&mut app, id, 0);
    app.handle(Command::Action(Action::NextEdge));
    assert_eq!(app.panels.waves(id).unwrap().cursor(&app.doc), Some(40));
}

// -- the feature showcase ----------------------------------------------------

fn showcase() -> Arc<dyn Session> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples/feature_showcase.vtr");
    OpenSpec::Path(path).open().unwrap()
}

/// Add `path` as a group, stack it and return its layers' names and whole-trace range.
fn stacked(app: &mut App, path: &[&str]) -> (Vec<String>, (f64, f64)) {
    let s = scope(app, path);
    app.handle(Command::AddScopeAsGroup {
        scope: a(s),
        recursive: false,
    });
    pump(app);
    app.handle(Command::Action(Action::ToggleStack));
    pump(app);
    let id = app.panels.focused_id();
    let w = app.panels.waves(id).unwrap();
    let g = *w.selected.first().unwrap();
    let layers = w.stack_layers(g);
    let names = layers
        .iter()
        .map(|l| w.items()[l.entry].name().to_owned())
        .collect();
    let range = match app.doc.stack_total(&stack::key(&layers)) {
        Some(stack::TotalLoad::Ready(s)) => s.range(),
        _ => panic!("{path:?}: no whole-trace walk"),
    };
    (names, range)
}

#[test]
fn the_showcase_scopes_stack_bits_counts_and_signed_currents() {
    let mut app = App::new();
    app.set_session(showcase());
    pump(&mut app);
    let (names, range) = stacked(&mut app, &["soc", "cpu", "occupancy"]);
    assert_eq!(names, ["fetch", "decode", "execute", "memory", "writeback"]);
    assert_eq!(range, (0.0, 5.0), "up to five instructions in flight");
    let (names, range) = stacked(&mut app, &["soc", "power"]);
    assert_eq!(
        names,
        ["io_mw", "sram_mw", "dma_mw", "core_mw", "leakage_mw"],
        "the text mode is left out"
    );
    assert!(range.0 == 0.0 && range.1 > 60.0, "{range:?}");
    let (names, range) = stacked(&mut app, &["soc", "dma", "queue"]);
    assert_eq!(names, ["ch0", "ch1", "ch2", "ch3"]);
    assert!(range.1 >= 8.0, "{range:?}");
    let (names, range) = stacked(&mut app, &["soc", "pmic"]);
    assert_eq!(names, ["solar_ma", "usb_ma", "radio_ma", "core_ma"]);
    assert!(range.0 < -50.0 && range.1 > 150.0, "{range:?}");
    // The text mode says it is left out.
    let id = app.panels.focused_id();
    cursor(&mut app, id, 800);
    let shown = texts(frame(&mut app, id));
    assert!(shown.iter().any(|s| s.starts_with("not sta")), "{shown:?}");
    // In the fault window channel 2's count is unknown: the queue sum reads
    // X. Folded, the four stacks fit the panel.
    app.panels.waves_mut(id).unwrap().fold_all(true);
    let shown = texts(frame(&mut app, id));
    let sums: Vec<&String> = shown.iter().filter(|s| s.starts_with('Σ')).collect();
    assert_eq!(sums.len(), 4, "{shown:?}");
    assert_eq!(sums[0], "Σ 3", "instructions in flight at 800 ns");
    assert_eq!(sums[2], "Σ X");
}
