//! Declared clocks (docs/vtr_clocks.html): writer calls, the dedicated clock
//! block, `ClockTimeline` queries and recovery.

use rand::{Rng, SeedableRng};
use vtr::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-clock-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A producer call.
#[derive(Clone, Copy, Debug)]
enum Call {
    Run(u64, u64),
    Stop(u64),
}

/// Brute force: every edge the calls describe, with `close` ending a running clock.
fn brute_edges(calls: &[Call], close: u64) -> Vec<u64> {
    let mut edges = Vec::new();
    let mut running: Option<(u64, u64)> = None;
    let finish = |run: Option<(u64, u64)>, t: u64, edges: &mut Vec<u64>| {
        if let Some((first, period)) = run {
            let mut e = first;
            edges.push(e);
            while e + period <= t {
                e += period;
                edges.push(e);
            }
        }
    };
    for c in calls {
        match *c {
            Call::Run(first, period) => running = Some((first, period)),
            Call::Stop(t) => finish(running.take(), t, &mut edges),
        }
    }
    finish(running, close, &mut edges);
    edges
}

fn timeline_edges(tl: &ClockTimeline) -> Vec<u64> {
    (0..tl.edge_count()).map(|c| tl.edge(c).unwrap()).collect()
}

fn write(path: &std::path::Path, calls: &[Call], close: u64) {
    let mut w = Writer::create(path).unwrap();
    let top = w.add_scope(None, "top", ScopeType::Module, "top").unwrap();
    let clk = w.add_clock(Some(top), "clk").unwrap();
    for c in calls {
        match *c {
            Call::Run(first, period) => w.clock_run(clk, first, period).unwrap(),
            Call::Stop(t) => w.clock_stop(clk, t).unwrap(),
        }
    }
    w.set_time(close).unwrap();
    w.close().unwrap();
}

#[test]
fn fixed_changing_gated_and_single_edge_clocks() {
    type Case = (&'static str, Vec<Call>, u64, Vec<(u64, u64, u64)>, bool);
    let cases: Vec<Case> = vec![
        ("fixed", vec![Call::Run(1, 2)], 100, vec![(1, 99, 2)], true),
        // The demo's DVFS clock: stop where the delay changes, run at the next rising edge.
        (
            "dvfs",
            vec![Call::Run(400, 334), Call::Stop(20000), Call::Run(20272, 500), Call::Stop(50000), Call::Run(50772, 1000), Call::Stop(100000)],
            120000,
            vec![(400, 19772, 334), (20272, 49772, 500), (50772, 99772, 1000)],
            false,
        ),
        ("gated", vec![Call::Run(0, 10), Call::Stop(55), Call::Run(300, 10)], 330, vec![(0, 50, 10), (300, 330, 10)], true),
        ("single", vec![Call::Run(7, 10), Call::Stop(12), Call::Stop(40)], 50, vec![(7, 7, 0)], false),
        ("late close", vec![Call::Run(90, 10)], 20, vec![(90, 90, 0)], true),
    ];
    for (name, calls, close, want, open) in cases {
        let path = tmp(&format!("{}.vtr", name.replace(' ', "_")));
        write(&path, &calls, close);
        let rd = Reader::open(&path).unwrap();
        assert_eq!(rd.clocks().len(), 1, "{name}");
        let info = &rd.clocks()[0];
        assert_eq!(info.path, "top.clk");
        assert_eq!(rd.find_clock("top.clk"), Some(info.id));
        let tl = rd.clock(info.id).unwrap();
        let got: Vec<_> = tl.stretches().iter().map(|s| (s.begin, s.end, s.period)).collect();
        assert_eq!(got, want, "{name}");
        assert_eq!(tl.is_open(), open, "{name}");
        assert_eq!(timeline_edges(&tl), brute_edges(&calls, close.max(want.last().unwrap().0)), "{name}");
        assert!(std::sync::Arc::ptr_eq(&tl, &rd.clock(info.id).unwrap()), "timelines are loaded once");
    }
}

#[test]
fn misuse_is_an_error_and_adds_nothing() {
    let path = tmp("misuse.vtr");
    let mut w = Writer::create(&path).unwrap();
    let clk = w.add_clock(None, "clk").unwrap();
    assert!(w.clock_run(clk, 0, 0).is_err(), "zero period");
    assert!(w.clock_run(ClockId(7), 0, 1).is_err(), "unknown clock");
    w.clock_stop(clk, 5).unwrap(); // not running: nothing to do
    w.clock_run(clk, 10, 4).unwrap();
    assert!(matches!(w.clock_run(clk, 20, 4), Err(Error::State(_))), "run on a running clock");
    assert!(w.clock_stop(clk, 9).is_err(), "stop before the first edge");
    w.clock_stop(clk, 29).unwrap(); // last edge 26
    assert!(w.clock_run(clk, 26, 4).is_err(), "first edge not after the previous last edge");
    w.clock_run(clk, 27, 4).unwrap();
    w.clock_stop(clk, 27).unwrap();
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    let tl = rd.clock(ClockId(0)).unwrap();
    assert_eq!(timeline_edges(&tl), vec![10, 14, 18, 22, 26, 27]);
}

#[test]
fn random_calls_reproduce_brute_force_edges() {
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xc10c);
    for round in 0..200 {
        let mut calls = Vec::new();
        let mut last_edge: Option<u64> = None;
        let mut t = 0u64;
        for _ in 0..rng.gen_range(1..8) {
            // A new stretch may begin right after the previous last edge, before the stop time.
            let first = last_edge.map_or(rng.gen_range(0..20), |e| e + rng.gen_range(1..40));
            let period = rng.gen_range(1..25);
            calls.push(Call::Run(first, period));
            let stop = first.max(t) + rng.gen_range(0..300);
            calls.push(Call::Stop(stop));
            last_edge = Some(first + (stop - first) / period * period);
            t = stop;
        }
        let close = if rng.gen_bool(0.5) {
            calls.pop();
            t + rng.gen_range(0..100)
        } else {
            t
        };
        let path = tmp(&format!("random{round}.vtr"));
        write(&path, &calls, close);
        let rd = Reader::open(&path).unwrap();
        let tl = rd.clock(ClockId(0)).unwrap();
        let want = brute_edges(&calls, close);
        assert_eq!(timeline_edges(&tl), want, "round {round}: {calls:?} close {close}");
        let end = want.last().copied().unwrap_or(0) + 5;
        for t in 0..=end {
            let last = want.iter().rposition(|&e| e <= t);
            let c = tl.cycle_at(t);
            assert_eq!(c.map(|c| (c.cycle, c.edge, c.next_edge)), last.map(|l| (l as u64, want[l], want.get(l + 1).copied())), "round {round} t {t}");
            assert_eq!(tl.next_edge(t), want.iter().copied().find(|&e| e > t));
            assert_eq!(tl.prev_edge(t), want.iter().copied().rfind(|&e| e < t));
        }
    }
}

#[test]
fn stretches_live_in_their_own_blocks_and_survive_recovery() {
    let path = tmp("recovery.vtr");
    let opts = WriterOptions { background: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let top = w.add_scope(None, "top", ScopeType::Module, "top").unwrap();
    let clk = w.add_clock(Some(top), "clk").unwrap();
    let pipe = w.add_stream(Some(top), "pipeline", "PIPELINE").unwrap();
    let gen = w.add_generator(pipe, "insn").unwrap();
    let (_, sig) = w.add_var(Some(top), "x", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap();
    w.clock_run(clk, 0, 2).unwrap();
    for t in 0..100u64 {
        w.set_time(t).unwrap();
        w.emit_u64(sig, t).unwrap();
        let tx = w.begin_tx(gen, t).unwrap();
        w.end_tx(tx, t + 1, TxStatus::Ok).unwrap();
        if t == 40 {
            w.clock_stop(clk, t).unwrap();
            w.clock_run(clk, 41, 3).unwrap();
        }
        if t == 50 {
            w.flush().unwrap();
        }
    }
    w.close().unwrap();
    let full = std::fs::read(&path).unwrap();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    let info = rd.clocks()[0].clone();
    // Every TX_BLOCK holding stretches holds nothing else.
    let mut clock_blocks = 0;
    for e in rd.sections().iter().filter(|e| e.kind == 5) {
        let bytes = &full[e.payload_offset() as usize..(e.payload_offset() + e.len) as usize];
        let h = vtr::txblock::TxBlockHeader::parse(bytes).unwrap();
        let gens: Vec<u32> = vtr::txblock::block_generators(bytes, &h).collect();
        if gens.contains(&info.generator.0) {
            assert_eq!(gens, vec![info.generator.0]);
            clock_blocks += 1;
        }
    }
    assert_eq!(clock_blocks, 2, "one block at the explicit flush, one at close");
    let tl = rd.clock(info.id).unwrap();
    assert_eq!(tl.stretches().iter().map(|s| (s.begin, s.end, s.period)).collect::<Vec<_>>(), vec![(0, 40, 2), (41, 98, 3)]);
    // Cut the file after the first clock block: the stretch ended before the flush survives.
    let first_clock_block = rd.sections().iter().filter(|e| e.kind == 5).map(|e| e.payload_offset() + e.len).find(|&end| {
        let bytes = &full[..end as usize];
        Reader::from_bytes(bytes.to_vec()).ok().is_some_and(|r| r.clocks().len() == 1 && r.clock(ClockId(0)).is_ok_and(|t| !t.stretches().is_empty()))
    });
    let cut = first_clock_block.expect("a clock block before close") as usize + 10;
    let rd = Reader::from_bytes(full[..cut].to_vec()).unwrap();
    assert!(rd.recovered());
    let tl = rd.clock(ClockId(0)).unwrap();
    assert_eq!(tl.stretches().iter().map(|s| (s.begin, s.end, s.period)).collect::<Vec<_>>(), vec![(0, 40, 2)]);
    assert!(!tl.is_open());
}

#[test]
fn streams_name_their_clock() {
    let path = tmp("link.vtr");
    let mut w = Writer::create(&path).unwrap();
    let cpu = w.add_scope(None, "cpu", ScopeType::Core, "").unwrap();
    let clk = w.add_clock(Some(cpu), "cycle").unwrap();
    let path_id = w.intern("cpu.cycle");
    let pipe = w.add_stream(Some(cpu), "thread0", "PIPELINE").unwrap();
    w.node_attr(pipe, "vtr.clock", Value::Str(path_id)).unwrap();
    let other = w.add_stream(Some(cpu), "thread1", "PIPELINE").unwrap();
    let w_stream = w.clock_stream(clk).unwrap();
    w.clock_run(clk, 0, 1).unwrap();
    w.set_time(10).unwrap();
    w.close().unwrap();
    let rd = Reader::open(&path).unwrap();
    assert_eq!(rd.stream_clock(pipe), Some(ClockId(0)));
    assert_eq!(rd.stream_clock(other), None);
    assert_eq!(rd.clock(ClockId(0)).unwrap().edge_count(), 11);
    assert_eq!(w_stream, rd.clocks()[0].stream);
}
