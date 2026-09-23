//! Signal-history cost: opens an FST or VTR trace through the shared session
//! backend, loads the named signals in one batch and prints, per run, the
//! load time, the RSS growth (Linux) and the bytes the histories count, then
//! per signal the cost of random point queries (`index_at`), 1400-column
//! frame sweeps (`index_at_hint`) over the whole signal and over 1/100 of
//! it, a sequential scan of change times, and random and sequential numeric
//! value decodes. Run one configuration per process so RSS deltas are clean.
//!
//! The stress trace is `volna/volna/examples/large_fst.fst`
//! (see `volna/volna/examples/README.md`); `vtr convert` gives its VTR twin.
//!
//! taskset -c 0-7 cargo run --release -p volna-core --example history_cost -- \
//!   volna/volna/examples/large_fst.fst clk sine_100m
use std::hint::black_box;
use std::time::Instant;

use volna_core::data::{SignalHistory, SignalShape, Translators};
use volna_core::session::OpenSpec;

const QUERIES: usize = 2_000_000;
const COLUMNS: u64 = 1400;
const DECODE_SCAN: usize = 20_000_000;

/// Resident set size of this process; `None` where /proc is unavailable.
fn rss() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    Some(statm.split_whitespace().nth(1)?.parse::<u64>().ok()? * 4096)
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn best<R>(runs: usize, mut f: impl FnMut() -> R) -> f64 {
    (0..runs)
        .map(|_| {
            let t = Instant::now();
            black_box(f());
            t.elapsed().as_secs_f64()
        })
        .fold(f64::INFINITY, f64::min)
}

fn xorshift(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn report(name: &str, h: &dyn SignalHistory, translators: &Translators) -> serde_json::Value {
    let len = h.len();
    let (t0, t1) = (h.time(0), h.time(len - 1) + 1);
    let mut seed = 0x9e37_79b9_7f4a_7c15;
    let times: Vec<u64> = (0..QUERIES)
        .map(|_| t0 + xorshift(&mut seed) % (t1 - t0))
        .collect();
    let point_ns = best(3, || {
        times
            .iter()
            .map(|&t| h.index_at(t).unwrap_or(0))
            .sum::<usize>()
    }) / QUERIES as f64
        * 1e9;
    let sweep_us = |a: u64, b: u64| {
        best(5, || {
            let mut hint = h.index_at(a).unwrap_or(0);
            for c in 1..=COLUMNS {
                hint = h
                    .index_at_hint(a + (b - a) * c / COLUMNS, hint)
                    .unwrap_or(0);
            }
            hint
        }) * 1e6
    };
    let mid = (t0 + t1) / 2;
    let frame_full_us = sweep_us(t0, t1);
    let frame_hundredth_us = sweep_us(mid, mid + (t1 - t0) / 100);
    let scan_ns = best(3, || {
        (0..len).fold(0u64, |acc, i| acc.wrapping_add(h.time(i)))
    }) / len as f64
        * 1e9;
    let format = match h.shape() {
        SignalShape::Vector { .. } => "sdec",
        SignalShape::Real => "real",
        _ => "udec",
    };
    let kind = translators.get(format).and_then(|t| t.numeric_kind());
    let (decode_random_ns, decode_scan_ns) = match kind {
        Some(kind) => {
            let idx: Vec<usize> = (0..QUERIES)
                .map(|_| (xorshift(&mut seed) % len as u64) as usize)
                .collect();
            let random = best(3, || {
                idx.iter()
                    .map(|&i| kind.read(&h.value_view(Some(i))).unwrap_or(0.0))
                    .sum::<f64>()
            }) / QUERIES as f64
                * 1e9;
            let n = len.min(DECODE_SCAN);
            let scan = best(1, || {
                (0..n)
                    .map(|i| kind.read(&h.value_view(Some(i))).unwrap_or(0.0))
                    .sum::<f64>()
            }) / n as f64
                * 1e9;
            (Some(random), Some(scan))
        }
        None => (None, None),
    };
    println!(
        "  {name}: {len} changes, point {point_ns:.0} ns, frame {frame_full_us:.0} µs (1/100: \
         {frame_hundredth_us:.0} µs), scan {scan_ns:.2} ns/change, decode {} / {} ns",
        decode_random_ns.map_or("-".into(), |v| format!("{v:.0}")),
        decode_scan_ns.map_or("-".into(), |v| format!("{v:.1}")),
    );
    serde_json::json!({
        "signal": name, "changes": len, "counted_bytes": h.resident_bytes(),
        "point_ns": point_ns, "frame_full_us": frame_full_us,
        "frame_hundredth_us": frame_hundredth_us, "scan_ns": scan_ns,
        "decode_random_ns": decode_random_ns, "decode_scan_ns": decode_scan_ns,
    })
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or_else(|| {
        anyhow::anyhow!("usage: history_cost <trace.fst|trace.vtr> <signal name>...")
    })?;
    let names: Vec<String> = args.collect();
    anyhow::ensure!(!names.is_empty(), "name at least one signal");
    let session = OpenSpec::Path(path.clone().into()).open()?;
    let h = session.hierarchy();
    let signals = names
        .iter()
        .map(|n| {
            h.vars
                .iter()
                .find(|v| &v.name == n)
                .map(|v| v.signal)
                .ok_or_else(|| anyhow::anyhow!("no signal named {n}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let before = rss();
    let t = Instant::now();
    let loads = session.load_signals(&signals);
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    let after = rss();
    let histories = loads
        .into_iter()
        .map(|(_, h)| h)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let counted: u64 = histories.iter().map(|h| h.resident_bytes()).sum();
    let changes: usize = histories.iter().map(|h| h.len()).sum();
    let rss_growth = before.zip(after).map(|(a, b)| b.saturating_sub(a));
    println!(
        "{path}: load {load_ms:.0} ms ({:.1} ns/change), RSS +{}, counted {:.0} MiB ({:.2} B/change)",
        load_ms * 1e6 / changes as f64,
        rss_growth.map_or("n/a".into(), |b| format!(
            "{:.0} MiB ({:.2} B/change)",
            mib(b),
            b as f64 / changes as f64
        )),
        mib(counted),
        counted as f64 / changes as f64,
    );
    let translators = Translators::builtin();
    let signals: Vec<_> = names
        .iter()
        .zip(&histories)
        .map(|(n, h)| report(n, h.as_ref(), &translators))
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "trace": path, "load_ms": load_ms, "changes": changes,
            "rss_growth_bytes": rss_growth, "counted_bytes": counted, "signals": signals,
        })
    );
    Ok(())
}
