//! The activity index builder runs in bounded memory (docs/hierarchy-activity.html):
//! building a run eight times longer peaks at the same heap, within 24 bytes
//! per signal plus the scans in flight. A counting allocator measures the
//! heap, so this file holds a single test.

use rand::{rngs::StdRng, Rng, SeedableRng};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use vtr::activity::{self, BuildOptions};
use vtr::*;

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(n: usize) {
    let now = CURRENT.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        grew(l.size());
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        CURRENT.fetch_sub(l.size(), Ordering::Relaxed);
        System.dealloc(p, l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        grew(l.size());
        System.alloc_zeroed(l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if new > l.size() {
            grew(new - l.size());
        } else {
            CURRENT.fetch_sub(l.size() - new, Ordering::Relaxed);
        }
        System.realloc(p, l, new)
    }
}

#[global_allocator]
static A: Counting = Counting;

const SIGNALS: usize = 20_000;

/// A design of `SIGNALS` signals, a tenth of them busy at any time, for `steps` time steps.
fn write_trace(path: &std::path::Path, steps: usize) {
    let mut rng = StdRng::seed_from_u64(1);
    let opts = WriterOptions { block_records: 40_000, ..Default::default() };
    let mut w = Writer::create_with(path, opts).unwrap();
    let top = Some(w.add_scope(None, "top", ScopeType::Module, "").unwrap());
    let sigs: Vec<SignalId> = (0..SIGNALS)
        .map(|i| w.add_var(top, &format!("s{i}"), VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 16, states: 2 }).unwrap().1)
        .collect();
    let mut t = 0;
    for step in 0..steps {
        w.set_time(t).unwrap();
        let phase = (step / 50) % 10;
        for &s in sigs[phase..].iter().step_by(10) {
            if rng.gen_ratio(1, 4) {
                w.emit_u64(s, rng.gen()).unwrap();
            }
        }
        t += rng.gen_range(1..20);
    }
    w.close().unwrap();
}

/// Peak heap bytes above the level before the build.
fn build_peak(path: &std::path::Path, opts: &BuildOptions) -> (usize, activity::Summary) {
    let r = Reader::open(path).unwrap();
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let summary = activity::build(&r, std::io::sink(), opts).unwrap();
    (PEAK.load(Ordering::Relaxed) - base, summary)
}

#[test]
fn memory_does_not_grow_with_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let (short, long) = (dir.path().join("short.vtr"), dir.path().join("long.vtr"));
    write_trace(&short, 2_000);
    write_trace(&long, 16_000);
    let opts = BuildOptions { threads: 3, memory: 8 << 20, ..Default::default() };
    let (p1, s1) = build_peak(&short, &opts);
    let (p8, s8) = build_peak(&long, &opts);
    eprintln!("short: {} blocks, {} changes, peak {p1} B; long: {} blocks, {} changes, peak {p8} B", s1.blocks, s1.changes, s8.blocks, s8.changes);
    assert!(s8.blocks >= 8 * s1.blocks - 8 && s8.changes > 7 * s1.changes, "the long run is eight times the short one");
    // A few dozen bytes per block are all that grows with the run.
    assert!(p8 <= p1 + p1 / 10 + 64 * s8.blocks as usize, "peak grew with the run: {p1} -> {p8}");
    assert!(p8 as u64 <= 24 * SIGNALS as u64 + opts.memory + (1 << 20), "peak {p8} over 24 B/signal + {} in flight", opts.memory);
    // A block that does not fit the limit is refused, naming it.
    let tight = BuildOptions { memory: 1 << 10, ..opts };
    let r = Reader::open(&short).unwrap();
    let err = activity::build(&r, std::io::sink(), &tight).unwrap_err().to_string();
    assert!(err.contains("block 0") && err.contains("memory limit"), "{err}");
}
