//! Scope activity cost (docs/hierarchy-activity.html, stage 4): opens a
//! trace that has an activity index beside it (`vtr index TRACE` first) and
//! times what the scope meters add: the census weights built once, then one
//! frame's classification and per-scope count for windows from the whole
//! trace down to a few time units, and the read of the undecided signals.
//! Best of five runs per window, on the loader thread's path.
//!
//! cargo run --release -p volna-core --example activity_cost -- TRACE
use std::time::Instant;

use volna_core::data::{ActivityCounter, ActivityCounts};
use volna_trace::session::OpenSpec;

fn best<T>(mut f: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<(f64, T)> {
    let mut out = None;
    let mut ms = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let r = f()?;
        ms = ms.min(t.elapsed().as_secs_f64() * 1e3);
        out = Some(r);
    }
    Ok((ms, out.unwrap()))
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: activity_cost TRACE"))?;
    let t = Instant::now();
    let session = OpenSpec::Path(path.clone().into()).open()?;
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let index = session
        .activity()
        .ok_or_else(|| anyhow::anyhow!("{path}: no activity index; run `vtr index {path}`"))?;
    let h = session.hierarchy();
    println!(
        "{path}: {} scopes, {} variables, {} signals, {} stretches ({:.1} MiB loaded); open {open_ms:.0} ms",
        h.scope_count(),
        h.var_count(),
        index.signal_count(),
        index.stretch_count(),
        index.memory_bytes() as f64 / (1 << 20) as f64
    );
    let (counter_ms, counter) = best(|| Ok(ActivityCounter::build(h)))?;
    println!(
        "census weights: {counter_ms:.1} ms, {:.1} MiB",
        counter.resident_bytes() as f64 / (1 << 20) as f64
    );
    let lo = index.blocks().first().map_or(0, |b| b.start);
    let hi = index.blocks().last().map_or(0, |b| b.end);
    let span = hi - lo;
    println!(
        "{:>14} {:>10} {:>10} {:>9} {:>9}",
        "window", "changing", "undecided", "frame ms", "read ms"
    );
    for divisor in [1u64, 10, 100, 1_000, 10_000, 100_000, 1_000_000] {
        let width = (span / divisor).max(1);
        let t0 = lo + span / 2 - width / 2;
        let window = (t0, t0 + width);
        let (frame_ms, counts) = best(|| Ok(ActivityCounts::classify(&index, &counter, window)))?;
        let changing = (0..h.scope_count())
            .filter_map(|s| counts.get(s))
            .map(|c| c.0)
            .max()
            .unwrap_or(0);
        let read = if counts.exact() {
            "-".to_string()
        } else {
            let (ms, _) = best(|| session.resolve_activity(&counts.undecided, window.0, window.1))?;
            format!("{ms:.1}")
        };
        println!(
            "{:>14} {changing:>10} {:>10} {frame_ms:>9.2} {read:>9}",
            width,
            counts.undecided.len()
        );
    }
    Ok(())
}
