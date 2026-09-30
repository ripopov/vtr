//! Activity index measurements for docs/hierarchy-activity.html.
//!
//! * `activity <trace>` builds the index with the library, as `vtr index`
//!   does, and reports the build time, the peak anonymous memory while
//!   building (file pages of the mapped trace excluded), the sidecar's size
//!   and thresholds, and the load time and size of the loaded index.
//! * `activity-export` writes the page's demo data from the same index.
//! * `gen-bursty` writes the synthetic picosecond trace with sleep phases and
//!   gated units that the page uses for irregular activity.

use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vtr::activity::{self, Budget, BuildOptions, Identity, Index, Sidecar};
use vtr::{Direction, NodeId, NodeKind, Reader, ScopeType, SignalId, SignalKind, VarType, Writer, WriterOptions};

fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Anonymous resident memory in bytes (`RssAnon`): the heap, not the mapped trace.
fn rss_anon() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("RssAnon:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .map_or(0, |kb| kb * 1024)
}

/// Samples `rss_anon` every millisecond until stopped; returns the largest sample.
struct PeakSampler {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<u64>,
}

impl PeakSampler {
    fn start() -> PeakSampler {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut peak = rss_anon();
            while !s.load(Ordering::Relaxed) {
                peak = peak.max(rss_anon());
                std::thread::sleep(Duration::from_millis(1));
            }
            peak.max(rss_anon())
        });
        PeakSampler { stop, thread }
    }

    fn stop(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap()
    }
}

fn quantiles(mut v: Vec<f64>) -> (f64, f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

/// Builds `path`'s index into a temporary file with the library and measures it.
pub fn run(path: &str, budget: Budget, threads: usize) -> serde_json::Value {
    let r = Reader::open(path).unwrap();
    let id = Identity::of(&r).unwrap();
    let file = id.length;
    let out = std::env::temp_dir().join(format!("vtr-bench-activity-{}.index", std::process::id()));
    let opts = BuildOptions { threads, budget, ..Default::default() };
    let base = rss_anon();
    let sampler = PeakSampler::start();
    let t = Instant::now();
    let summary = activity::build(&r, std::io::BufWriter::new(std::fs::File::create(&out).unwrap()), &opts).unwrap();
    let build_s = t.elapsed().as_secs_f64();
    let peak = sampler.stop().saturating_sub(base);
    let t = Instant::now();
    let index = Index::open(&out, &id).unwrap();
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    std::fs::remove_file(&out).unwrap();
    let blocks = index.blocks();
    let rel: Vec<f64> = blocks
        .iter()
        .enumerate()
        .map(|(k, b)| b.delta as f64 / if k == 0 { b.end - b.start + 1 } else { b.end - blocks[k - 1].end }.max(1) as f64)
        .collect();
    let (rmin, rmed, rmax) = quantiles(rel);
    let (dmin, dmax) = summary.delta.unwrap_or((0, 0));
    eprintln!(
        "{path}: {} signals, {} changes, {} blocks, Δ {dmin}..{dmax}, {} stretches, {} B ({:.3}%), build {build_s:.2} s, peak {:.1} MiB",
        summary.signals,
        summary.changes,
        summary.blocks,
        summary.stretches,
        summary.bytes,
        100.0 * summary.bytes as f64 / file as f64,
        peak as f64 / (1 << 20) as f64
    );
    json!({
        "file": path, "file_bytes": file, "signals": summary.signals, "changes": summary.changes, "blocks": summary.blocks,
        "disk": budget.disk, "memory": budget.memory, "threads": threads,
        "build_s": build_s, "build_ns_per_change": build_s * 1e9 / summary.changes.max(1) as f64,
        "peak_anon_bytes": peak,
        "sidecar_bytes": summary.bytes, "disk_share": summary.bytes as f64 / file as f64,
        "stretches": summary.stretches, "delta_min": dmin, "delta_max": dmax,
        "delta_over_block_min": rmin, "delta_over_block_median": rmed, "delta_over_block_max": rmax,
        "load_ms": load_ms, "loaded_bytes": index.memory_bytes(), "memory_share": index.memory_bytes() as f64 / file as f64,
    })
}

/// A picosecond SoC with sleep phases, gated units and an always-on domain: activity at very different rates.
pub fn gen_bursty(path: &str, scale: u64, seed: u64) {
    const UNITS: usize = 32;
    const PER_UNIT: usize = 500;
    const HALF: u64 = 2_000; // 250 MHz unit clocks, in ps
    let end: u64 = 5_000_000_000 * scale; // 5 ms per scale step
    let mut rng = StdRng::seed_from_u64(seed);
    let mut w = Writer::create_with(path, WriterOptions::default()).unwrap();
    w.set_timescale(-12).unwrap();
    let top = w.add_scope(None, "soc", ScopeType::Module, "").unwrap();
    let bit = |w: &mut Writer, p: NodeId, name: &str, width: u32| w.add_var(Some(p), name, VarType::Wire, Direction::Implicit, SignalKind::Bits { width, states: 2 }).unwrap().1;
    let aon = w.add_scope(Some(top), "aon", ScopeType::Module, "").unwrap();
    let rtc = bit(&mut w, aon, "rtc_clk", 1);
    let rtc_cnt = bit(&mut w, aon, "rtc_count", 32);
    let wake = bit(&mut w, aon, "awake", 1);
    let mut clk = Vec::new();
    let mut sigs: Vec<Vec<(SignalId, u32, f64)>> = Vec::new();
    for u in 0..UNITS {
        let p = w.add_scope(Some(top), &format!("unit{u}"), ScopeType::Module, "").unwrap();
        clk.push(bit(&mut w, p, "clk", 1));
        let mut v = Vec::new();
        for i in 0..PER_UNIT {
            let width = [1, 1, 1, 4, 8, 16, 32, 64][i % 8];
            // Per-cycle change probability, log-uniform over 1e-4 .. 0.3.
            let pr = (1e-4f64.ln() + rng.gen::<f64>() * (0.3f64 / 1e-4).ln()).exp();
            v.push((bit(&mut w, p, &format!("s{i}"), width), width, pr));
        }
        sigs.push(v);
    }
    // Schedule: awake phases (log-uniform 20 µs .. 1 ms) between sleeps (100 µs .. 5 ms);
    // while awake each unit runs bursts of 1 .. 100 µs separated by 1 µs .. 1 ms.
    let lu = |rng: &mut StdRng, a: f64, b: f64| (a.ln() + rng.gen::<f64>() * (b / a).ln()).exp() as u64;
    let mut on: Vec<Vec<(u64, u64)>> = vec![Vec::new(); UNITS];
    let mut awake = Vec::new();
    let mut t = lu(&mut rng, 1e8, 5e9);
    while t < end {
        let a1 = (t + lu(&mut rng, 2e7, 1e9)).min(end);
        awake.push((t, a1));
        for u in 0..UNITS {
            let mut x = t + lu(&mut rng, 1e6, 1e9);
            while x < a1 {
                let y = (x + lu(&mut rng, 1e6, 1e8)).min(a1);
                on[u].push((x / (2 * HALF) * 2 * HALF, y / (2 * HALF) * 2 * HALF));
                x = y + lu(&mut rng, 1e6, 1e9);
            }
        }
        t = a1 + lu(&mut rng, 1e8, 5e9);
    }
    // Events per 10 µs slab, sorted, emitted in time order.
    let slab = 10_000_000u64;
    let rtc_half = 15_258_789u64;
    let mut ev: Vec<(u64, u32, u64)> = Vec::new(); // (time, signal, value)
    let mut vals = vec![0u64; 3 + UNITS * (PER_UNIT + 1) + 16];
    let mut cur = vec![0usize; UNITS];
    let mut s0 = 0u64;
    let mut count = 0u64;
    while s0 < end {
        let s1 = (s0 + slab).min(end);
        ev.clear();
        let mut r = s0.div_ceil(rtc_half) * rtc_half;
        while r < s1 {
            ev.push((r, rtc.0, (r / rtc_half) & 1));
            if (r / rtc_half) & 1 == 1 {
                ev.push((r, rtc_cnt.0, r / rtc_half / 2));
            }
            r += rtc_half;
        }
        for &(a, b) in &awake {
            if a >= s0 && a < s1 { ev.push((a, wake.0, 1)); }
            if b >= s0 && b < s1 { ev.push((b, wake.0, 0)); }
        }
        for u in 0..UNITS {
            while cur[u] < on[u].len() && on[u][cur[u]].1 <= s0 {
                cur[u] += 1;
            }
            let mut k = cur[u];
            while k < on[u].len() && on[u][k].0 < s1 {
                let (a, b) = (on[u][k].0.max(s0), on[u][k].1.min(s1));
                let mut e = a.div_ceil(HALF) * HALF;
                while e < b {
                    ev.push((e, clk[u].0, (e / HALF) & 1));
                    e += HALF;
                }
                let cycles = (b - a) / (2 * HALF);
                for &(sid, width, pr) in &sigs[u] {
                    // Geometric gaps between changing cycles.
                    let mut c = 0u64;
                    loop {
                        let g = ((1.0 - rng.gen::<f64>()).ln() / (1.0 - pr).ln()).floor() as u64 + 1;
                        c += g;
                        if c >= cycles { break; }
                        let v = if width == 1 { vals[sid.0 as usize] ^ 1 } else { rng.gen::<u64>() & if width == 64 { u64::MAX } else { (1 << width) - 1 } };
                        vals[sid.0 as usize] = v;
                        ev.push((a.div_ceil(2 * HALF) * 2 * HALF + c * 2 * HALF, sid.0, v));
                    }
                }
                k += 1;
            }
        }
        ev.sort_unstable_by_key(|e| (e.0, e.1));
        let mut last = u64::MAX;
        for &(t, s, v) in &ev {
            if t >= s1 { continue; }
            if t != last {
                w.set_time(t).unwrap();
                last = t;
            }
            w.emit_u64(SignalId(s), v).unwrap();
            count += 1;
        }
        s0 = s1;
    }
    w.set_time(end).unwrap();
    w.close().unwrap();
    eprintln!("{path}: {count} records, {} awake phases, end {end} ps", awake.len());
}

/// Writes the demo data of docs/hierarchy-activity.html for `[a, b]`: the scope tree, the
/// library index's stretch lists clipped to the slice (identical lists shared), the thresholds of
/// the cells it touches, and exact answers for a few windows from every change in the slice. Uses
/// the trace's sidecar when valid, else builds the index in memory.
pub fn export(path: &str, a: u64, b: u64, out: &str) {
    let r = Reader::open(path).unwrap();
    let n = r.signal_count() as usize;
    let id = Identity::of(&r).unwrap();
    let index = Sidecar::new(path.as_ref(), &id, activity::default_cache_dir().as_deref()).load(&id).map(|x| x.1).unwrap_or_else(|| {
        let mut image = Vec::new();
        activity::build(&r, &mut image, &BuildOptions::default()).unwrap();
        Index::decode(&image, &id).unwrap()
    });
    let ends: Vec<u64> = index.blocks().iter().map(|k| k.end).collect();
    let cell = |t: u64| ends.partition_point(|&e| e < t).min(ends.len() - 1);
    let (c0, c1) = (cell(a), cell(b));
    // Every change in the slice, one per signal and time step.
    let mut sliced: Vec<Vec<u64>> = vec![Vec::new(); n];
    r.for_each_change(a, b, |t, s, _| {
        let c = &mut sliced[s.0 as usize];
        if t > index.t_min() && c.last() != Some(&t) {
            c.push(t)
        }
    })
    .unwrap();
    // Scopes in DFS order.
    let h = r.hierarchy();
    let mut order: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = h.roots().map(|x| x.0).collect();
    stack.reverse();
    while let Some(x) = stack.pop() {
        if h.kind(NodeId(x)) != NodeKind::Scope {
            continue;
        }
        order.push(x);
        let mut kids: Vec<u32> = h.children(NodeId(x)).map(|c| c.0).filter(|&c| h.kind(NodeId(c)) == NodeKind::Scope).collect();
        kids.reverse();
        stack.extend(kids);
    }
    let mut pos = vec![u32::MAX; h.len()];
    for (i, &x) in order.iter().enumerate() {
        pos[x as usize] = i as u32;
    }
    let ns = order.len();
    let par: Vec<u32> = order.iter().map(|&x| h.parent(NodeId(x)).map(|p| pos[p.0 as usize]).unwrap_or(u32::MAX)).collect();
    let mut sig_scopes: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut own = vec![0u32; ns];
    for id in h.nodes_of_kind(NodeKind::Var) {
        if let (Some(s), Some(p)) = (h.signal_of(id), h.parent(id)) {
            let sp = pos[p.0 as usize];
            if sp != u32::MAX {
                sig_scopes[s.0 as usize].push(sp);
                own[sp as usize] += 1;
            }
        }
    }
    for v in sig_scopes.iter_mut() {
        v.sort();
        v.dedup();
    }
    // The slice's changes split where the index keeps a silence; identical lists shared.
    let mut map = std::collections::HashMap::new();
    let mut lists: Vec<Vec<(u64, u64, u64)>> = Vec::new();
    let mut lid = vec![0u32; n];
    for s in 0..n {
        let kept: Vec<u64> = {
            let st: Vec<_> = index.stretches(SignalId(s as u32)).collect();
            st.iter().take(st.len().saturating_sub(1)).map(|x| x.end).filter(|&e| e >= a && e <= b).collect()
        };
        let mut l = Vec::new();
        if let Some(&first) = sliced[s].first() {
            let (mut st, mut gap) = (first, 0);
            for w in sliced[s].windows(2) {
                if kept.binary_search(&w[0]).is_ok() {
                    l.push((st, w[0], gap));
                    st = w[1];
                    gap = 0;
                } else {
                    gap = gap.max(w[1] - w[0]);
                }
            }
            l.push((st, *sliced[s].last().unwrap(), gap));
        }
        let next = lists.len() as u32;
        lid[s] = *map.entry(l.clone()).or_insert_with(|| {
            lists.push(l);
            next
        });
    }
    let mut st: Vec<Vec<u8>> = vec![Vec::new(); 12];
    for v in [a, b, ns as u64, n as u64, lists.len() as u64, (c1 - c0 + 1) as u64] {
        put(&mut st[0], v);
    }
    st[1] = order.iter().map(|&x| r.name(NodeId(x)).to_string()).collect::<Vec<_>>().join("\n").into_bytes();
    for i in 0..ns {
        put(&mut st[2], if par[i] == u32::MAX { 0 } else { (i as u32 - par[i]) as u64 });
        put(&mut st[3], own[i] as u64);
    }
    for s in 0..n {
        put(&mut st[4], lid[s] as u64);
        put(&mut st[5], sig_scopes[s].len() as u64);
        let mut cur = 0u32;
        for &x in &sig_scopes[s] {
            put(&mut st[6], (x - cur) as u64);
            cur = x;
        }
    }
    for l in &lists {
        put(&mut st[7], l.len() as u64);
        let mut end = a;
        for &(x, y, g) in l {
            put(&mut st[8], x - end);
            put(&mut st[9], y - x);
            put(&mut st[10], g);
            end = y;
        }
    }
    // Cell k covers (end of block k - 1, end of block k]: its first time and its Δ.
    let mut prev = a;
    for k in c0..=c1 {
        let s = if k == 0 { a } else { (ends[k - 1] + 1).max(a) };
        put(&mut st[11], s - prev);
        prev = s;
        put(&mut st[11], index.blocks()[k].delta);
    }
    let mut buf = Vec::new();
    for x in &st {
        put(&mut buf, x.len() as u64);
    }
    for x in &st {
        buf.extend_from_slice(x);
    }
    std::fs::write(out, &buf).unwrap();
    let total: usize = lists.iter().map(|l| l.len()).sum();
    let deltas: Vec<u64> = index.blocks()[c0..=c1].iter().map(|k| k.delta).collect();
    eprintln!("{out}: {ns} scopes, {n} signals, {} lists, {total} stretches, cells {c0}..={c1} deltas {deltas:?}, {} bytes", lists.len(), buf.len());
    // Exact answers from every change: active signals, a hash of per-scope counts, quiet scopes, two named units.
    let name_pos = |nm: &str| order.iter().position(|&x| r.name(NodeId(x)) == nm);
    let (fi, fv) = (name_pos("x_ct_ifu_top"), name_pos("x_ct_vfpu_top"));
    let wins = [(220000u64, 270000u64), (250000, 252000), (250000, 251000), (230000, 260000), (240000, 240100), (265000, 265016), (253333, 253335), (220000, 222000), (262000, 270000)];
    let mut js = Vec::new();
    for &(t0, t1) in &wins {
        let mut cnt = vec![0u32; ns];
        let mut stamp = vec![u32::MAX; ns];
        let mut total = 0;
        for s in 0..n {
            let c = &sliced[s];
            let i = c.partition_point(|&x| x < t0);
            if !(i < c.len() && c[i] <= t1) {
                continue;
            }
            total += 1;
            for &p0 in &sig_scopes[s] {
                let mut p = p0;
                while p != u32::MAX && stamp[p as usize] != s as u32 {
                    stamp[p as usize] = s as u32;
                    cnt[p as usize] += 1;
                    p = par[p as usize];
                }
            }
        }
        let hsh = cnt.iter().fold(0u32, |h, &c| h.wrapping_mul(31).wrapping_add(c));
        js.push(json!({"t0": t0, "t1": t1, "active": total, "hash": hsh, "quietScopes": cnt.iter().filter(|&&c| c == 0).count(),
            "ifu": fi.map(|i| cnt[i]), "vfpu": fv.map(|i| cnt[i]), "cells": cell(t1) - cell(t0) + 1}));
    }
    println!("{}", serde_json::to_string(&js).unwrap());
}
