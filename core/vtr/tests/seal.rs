//! Sealing a writer from another thread (docs/crash-safe-vtr.html, stage 4):
//! the file keeps exactly what reached the encoder and ends complete.

use rand::{rngs::StdRng, Rng, SeedableRng};
use std::path::{Path, PathBuf};
use vtr::*;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-seal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

const CRASH: Ending = Ending::Crashed { signal: 11, code: 1, address: 0, thread: 7, sealed: true };

/// A random run: every kind of signal, transactions with events and stages,
/// log records, a gated clock, and signals declared while it runs.
struct Run {
    w: Writer,
    rng: StdRng,
    sigs: Vec<SignalId>,
    top: NodeId,
    gen: NodeId,
    site: LogSiteId,
    clk: ClockId,
    /// First edge of the running stretch.
    clk_running: Option<u64>,
    open: Vec<TxId>,
    key: StrId,
    t: u64,
    late: u32,
}

impl Run {
    fn new(path: &Path, seed: u64) -> Run {
        let opts = WriterOptions { block_records: 256, chunk_records: 32, tx_block_bytes: 300, group_size: 4, dedup: false, ..Default::default() };
        let mut w = Writer::create_with(path, opts).unwrap();
        let top = w.add_scope(None, "top", ScopeType::Module, "").unwrap();
        let kinds = [
            (VarType::Wire, SignalKind::Bits { width: 8, states: 2 }),
            (VarType::Wire, SignalKind::Bits { width: 1, states: 2 }),
            (VarType::Logic, SignalKind::Bits { width: 6, states: 4 }),
            (VarType::Wire, SignalKind::Bits { width: 100, states: 2 }),
            (VarType::Logic, SignalKind::Bits { width: 70, states: 4 }),
            (VarType::Real, SignalKind::Real),
            (VarType::String, SignalKind::VarLen),
        ];
        let sigs = kinds.iter().enumerate().map(|(i, &(vt, k))| w.add_var(Some(top), &format!("s{i}"), vt, Direction::Implicit, k).unwrap().1).collect();
        let stream = w.add_stream(Some(top), "bus", "TRANSACTOR").unwrap();
        let gen = w.add_generator(stream, "rd").unwrap();
        let log = w.add_stream(None, "log", LOG_STREAM_KIND).unwrap();
        let site = w.add_log_site(&LogSiteSpec::new(log, Severity::Info, "at {} {}", &[LogArgType::U64, LogArgType::Text])).unwrap();
        let clk = w.add_clock(Some(top), "clk").unwrap();
        let key = w.intern("addr");
        Run { w, rng: StdRng::seed_from_u64(seed), sigs, top, gen, site, clk, clk_running: None, open: Vec::new(), key, t: 0, late: 0 }
    }

    fn step(&mut self) {
        let w = &mut self.w;
        match self.rng.gen_range(0..100) {
            0..=9 => {
                self.t += self.rng.gen_range(1..4);
                w.set_time(self.t).unwrap();
            }
            10..=69 => {
                let i = self.rng.gen_range(0..self.sigs.len());
                let s = self.sigs[i];
                let v: u64 = self.rng.gen();
                match w.signal_kind(s).unwrap() {
                    SignalKind::Bits { states: 4, width } if v % 5 == 0 => {
                        let text: String = (0..width).map(|k| b"01xz"[(v >> (k % 60)) as usize & 3] as char).collect();
                        w.emit_logic_str(s, text.as_bytes()).unwrap()
                    }
                    SignalKind::Bits { width, .. } if width > 64 => w.emit_words(s, &[v as u32, (v >> 32) as u32, !v as u32, 3]).unwrap(),
                    SignalKind::Bits { .. } => w.emit_u64(s, v).unwrap(),
                    SignalKind::Real => w.emit_real(s, v as f64 / 7.0).unwrap(),
                    SignalKind::VarLen => w.emit_varlen(s, format!("v{}", v % 1000).as_bytes()).unwrap(),
                }
            }
            70..=77 => {
                let tx = w.begin_tx(self.gen, self.t).unwrap();
                w.tx_attr(tx, self.key, &Value::U64(self.t)).unwrap();
                w.tx_event(tx, self.t, self.key, &[]).unwrap();
                w.tx_stage(tx, self.key, StrId(0), self.t, self.t + 1, &[]).unwrap();
                self.open.push(tx);
            }
            78..=85 if !self.open.is_empty() => {
                let tx = self.open.remove(self.rng.gen_range(0..self.open.len()));
                w.end_tx(tx, self.t + 2, TxStatus::Ok).unwrap();
            }
            86..=93 => {
                w.log(self.site, self.t, &[LogArg::U64(self.t), LogArg::Text(if self.t % 2 == 0 { "even" } else { "odd" })]).unwrap();
            }
            94..=96 => {
                match self.clk_running {
                    Some(first) if self.t >= first => {
                        w.clock_stop(self.clk, self.t).unwrap();
                        self.clk_running = None;
                    }
                    Some(_) => {}
                    None => {
                        w.clock_run(self.clk, self.t + 1, 2).unwrap();
                        self.clk_running = Some(self.t + 1);
                    }
                }
            }
            97 => {
                // A signal declared mid-run, into a group that may already have changes.
                self.late += 1;
                let (_, s) = w.add_var(Some(self.top), &format!("late{}", self.late), VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 12, states: 2 }).unwrap();
                self.sigs.push(s);
            }
            _ => {}
        }
    }
}

fn signal_paths(r: &Reader) -> Vec<(String, SignalId)> {
    (0..r.signal_count()).filter_map(|i| {
        let s = SignalId(i);
        let path = (0..9).map(|k| format!("top.s{k}")).chain((1..100).map(|k| format!("top.late{k}"))).find(|p| r.find_signal(p, '.') == Some(s))?;
        Some((path, s))
    })
    .collect()
}

/// Everything in `sealed` is an unchanged prefix or subset of `full`.
fn assert_sealed_prefix(sealed: &Reader, full: &Reader, what: &str) {
    for (path, s) in signal_paths(sealed) {
        let a = sealed.load_signal(s).unwrap();
        let b = full.load_signal(full.find_signal(&path, '.').unwrap()).unwrap();
        assert!(a.len() <= b.len(), "{what} {path}");
        assert_eq!(a.initial(), b.initial(), "{what} {path}: initial value");
        for i in 0..a.len() {
            assert_eq!((a.times()[i], a.get(i)), (b.times()[i], b.get(i)), "{what} {path} change {i}");
        }
    }
    let full_tx = full.transactions(&TxQuery::default()).unwrap();
    for t in sealed.transactions(&TxQuery::default()).unwrap() {
        if sealed.log_site(t.generator).is_some() || sealed.full_path(sealed.generator_stream(t.generator).unwrap(), ".") == ending::STREAM {
            continue;
        }
        let f = full_tx.iter().find(|f| f.id == t.id).unwrap_or_else(|| panic!("{what}: transaction {} is not in the full file", t.id));
        assert_eq!((t.begin, t.end, t.status, t.attrs.len(), t.events.len(), t.stages.len()), (f.begin, f.end, f.status, f.attrs.len(), f.events.len(), f.stages.len()), "{what}: transaction {}", t.id);
    }
    let mut full_logs = Vec::new();
    full.visit_log(&LogQuery::default(), |r| {
        full_logs.push((r.id, r.time, r.format(full.strings())));
        true
    })
    .unwrap();
    let mut n = 0;
    sealed
        .visit_log(&LogQuery::default(), |r| {
            if sealed.full_path(r.site.stream, ".") != ending::STREAM {
                assert_eq!((r.id, r.time, r.format(sealed.strings())), full_logs[n], "{what}: log record {n}");
                n += 1;
            }
            true
        })
        .unwrap();
    // Every record is readable: the ending record's strings must not hide the program's log sites.
    assert_eq!(n as u64 + 1, sealed.log_count(), "{what}: log records readable");
    assert_eq!(sealed.strings().find(vtr::LOG_STREAM_KIND).map(|_| (0..sealed.strings().len()).filter(|&i| sealed.str(StrId(i as u32)) == vtr::LOG_STREAM_KIND).count()), Some(1), "{what}: strings interned once");
}

#[test]
fn seal_after_random_numbers_of_calls() {
    let mut rng = StdRng::seed_from_u64(1);
    for case in 0..40 {
        let seed = rng.gen();
        let calls = rng.gen_range(0..3000);
        let what = format!("case {case} (seed {seed}, {calls} calls)");
        let full_path = tmp(&format!("full{case}.vtr"));
        let mut full = Run::new(&full_path, seed);
        (0..calls).for_each(|_| full.step());
        full.w.close().unwrap();

        let sealed_path = tmp(&format!("sealed{case}.vtr"));
        let mut run = Run::new(&sealed_path, seed);
        (0..calls).for_each(|_| run.step());
        let sealer = run.w.sealer();
        assert!(!sealer.state().busy());
        // The owner stops; another thread finishes the file.
        std::thread::spawn(move || sealer.seal(CRASH)).join().unwrap().unwrap();
        drop(run);

        let sealed = Reader::open(&sealed_path).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(sealed.recovered(), None, "{what}");
        assert_eq!(sealed.ending().unwrap().0, CRASH, "{what}");
        assert_sealed_prefix(&sealed, &Reader::open(&full_path).unwrap(), &what);
    }
}

#[test]
fn seal_keeps_every_chunk_handed_over() {
    // Narrow signals only: a chunk is handed over every 64 changes, so the
    // sealed file holds exactly the first 64 * floor(n / 64) of them.
    for n in [0u64, 1, 63, 64, 65, 255, 256, 257, 1000, 5000] {
        let path = tmp(&format!("exact{n}.vtr"));
        let opts = WriterOptions { block_records: 256, chunk_records: 64, group_size: 2, dedup: false, ..Default::default() };
        let mut w = Writer::create_with(&path, opts).unwrap();
        let sigs: Vec<_> = (0..4).map(|i| w.add_var(None, &format!("s{i}"), VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 16, states: 2 }).unwrap().1).collect();
        for i in 0..n {
            w.set_time(i / 3).unwrap();
            w.emit_u64(sigs[(i * 7 % 4) as usize], i).unwrap();
        }
        w.sealer().seal(CRASH).unwrap();
        drop(w);
        let r = Reader::open(&path).unwrap();
        let mut kept = Vec::new();
        r.for_each_change(0, u64::MAX, |t, s, v| kept.push((t, s, v.as_u64().unwrap()))).unwrap();
        kept.sort_by_key(|&(_, _, v)| v);
        let expected: Vec<_> = (0..n / 64 * 64).map(|i| (i / 3, sigs[(i * 7 % 4) as usize], i)).collect();
        assert_eq!(kept, expected, "{n} changes");
        assert_eq!(r.ending().unwrap(), (CRASH, Some(if n >= 64 { (n / 64 * 64 - 1) / 3 } else { 0 })));
    }
}

#[test]
fn seal_before_anything_reached_the_encoder() {
    let path = tmp("empty.vtr");
    let mut w = Writer::create(&path).unwrap();
    w.add_var(None, "a", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap();
    w.sealer().seal(Ending::Stopped { signal: 15 }).unwrap();
    drop(w);
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.ending().unwrap(), (Ending::Stopped { signal: 15 }, Some(0)));
    assert_eq!(r.str(StrId(0)), "");
    assert_eq!(r.signal_count(), 0, "the declaration never reached the encoder");
}

#[test]
fn seal_fails_when_it_cannot_help() {
    let mut w = Writer::create(tmp("twice.vtr")).unwrap();
    let sealer = w.sealer();
    sealer.seal(CRASH).unwrap();
    assert!(matches!(sealer.seal(CRASH), Err(Error::State(_))), "sealed once");
    w.close().ok();
    let mut w = Writer::create(tmp("closed.vtr")).unwrap();
    let sealer = w.sealer();
    w.close().unwrap();
    assert!(matches!(sealer.seal(CRASH), Err(Error::State(_))), "closed");
    let w = Writer::create_with(tmp("inline.vtr"), WriterOptions { background: false, ..Default::default() }).unwrap();
    assert!(matches!(w.sealer().seal(CRASH), Err(Error::State(_))), "inline");
}

#[test]
fn a_panic_poisons_and_close_seals() {
    let path = tmp("poisoned.vtr");
    let opts = WriterOptions { block_records: 100, chunk_records: 10, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, a) = w.add_var(None, "a", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap();
    for t in 0..250 {
        w.set_time(t).unwrap();
        w.emit_u64(a, t).unwrap();
    }
    assert!(!w.is_poisoned() && !w.state().busy());
    // A batch the owner marked is busy while it runs.
    w.guarded(|w| assert!(w.state().busy()));
    assert!(!w.state().busy());
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.guarded(|_| panic!("injected panic"))));
    assert!(caught.is_err());
    assert!(w.is_poisoned() && w.sealer().state().busy());
    w.close().unwrap();
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.ending().unwrap().0, Ending::Poisoned);
    // Blocks of 100 changes are handed over in chunks of 16 (16 per declared signal):
    // two blocks and three chunks of the third reached the encoder.
    assert_eq!(r.load_signal(a).unwrap().len(), 248, "the chunks handed over before the panic");
}
