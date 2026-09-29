//! Range-activity index measurements for docs/hierarchy-activity.html.
//!
//! `activity <in.vtr>` answers "which signals change in [t0, t1]" for random
//! windows three ways and checks two indexes against the exact answer from
//! every change of every signal:
//!
//! * the block-anchored stretch index of the design: each signal's changes in
//!   one block form stretches split at silences longer than the block's own
//!   threshold Δb, chosen so the block's table stays within `--eps` of the
//!   block's compressed bytes;
//! * the earlier global stretch index: one Δ for the whole run within the
//!   same budget.
//!
//! It reports sizes, the share of windows answered exactly, undecided shares,
//! query and resolve times, the scope walk and the build cost per change.
//! `gen-bursty <out.vtr>` writes the synthetic picosecond trace with sleep
//! phases and gated units that the page uses for irregular activity.

use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::json;
use std::time::Instant;
use vtr::{Direction, NodeId, NodeKind, Reader, ScopeType, SignalId, SignalKind, VarType, Writer, WriterOptions};

const SIGNAL_BLOCK: u32 = 4;
const FLOOR: u64 = u64::MAX;

fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zsize(b: &[u8]) -> usize {
    zstd::bulk::compress(b, 3).unwrap().len()
}

/// Every change time per signal, strictly after the first time step (initial values are not changes).
fn collect(r: &Reader) -> (Vec<Vec<u64>>, u64, u64) {
    let n = r.signal_count() as usize;
    let (tmin, tmax) = r.time_range().unwrap();
    let mut ch: Vec<Vec<u64>> = vec![Vec::new(); n];
    r.for_each_change(tmin, tmax, |t, s, _| {
        let c = &mut ch[s.0 as usize];
        if t > tmin && c.last() != Some(&t) {
            c.push(t)
        }
    })
    .unwrap();
    (ch, tmin, tmax)
}

/// Splits `c` (one signal's changes inside one cell) into stretches at gaps longer than `d`.
fn split(c: &[u64], d: u64, out: &mut Vec<(u64, u64, u64)>) {
    let (mut st, mut mx) = (c[0], 0u64);
    for w in c.windows(2) {
        let l = w[1] - w[0];
        if l > d {
            out.push((st, w[0], mx));
            st = w[1];
            mx = 0;
        } else {
            mx = mx.max(l);
        }
    }
    out.push((st, *c.last().unwrap(), mx));
}

/// Column streams of one table: signal-id deltas, stretch counts, starts (from the cell start or the
/// previous stretch's end), ends (the last stretch from the cell end, so busy signals cost ~0), largest gaps.
fn encode(rows: &[(u32, Vec<(u64, u64, u64)>)], origin: u64, span: u64) -> Vec<u8> {
    let mut st: Vec<Vec<u8>> = vec![Vec::new(); 5];
    let end_of_cell = origin + span - 1;
    let mut last = 0u32;
    for (s, list) in rows {
        put(&mut st[0], (*s - last) as u64);
        last = *s;
        put(&mut st[1], list.len() as u64);
        let mut end = origin;
        for (j, &(a, b, g)) in list.iter().enumerate() {
            put(&mut st[2], a - end);
            put(&mut st[3], if j + 1 == list.len() { end_of_cell - b } else { b - a });
            put(&mut st[4], g);
            end = b;
        }
    }
    let mut out = Vec::new();
    for x in &st {
        put(&mut out, x.len() as u64);
    }
    for x in &st {
        out.extend_from_slice(x);
    }
    out
}

/// Flattened per-signal stretch lists: `off[s]..off[s+1]` index `s0/s1/g`.
struct Index {
    off: Vec<u64>,
    s0: Vec<u64>,
    s1: Vec<u64>,
    g: Vec<u64>,
}

impl Index {
    fn new(n: usize) -> Index {
        Index { off: Vec::with_capacity(n + 1), s0: Vec::new(), s1: Vec::new(), g: Vec::new() }
    }
    fn push_signal(&mut self, list: &[(u64, u64, u64)]) {
        if self.off.is_empty() {
            self.off.push(0);
        }
        for &(a, b, g) in list {
            self.s0.push(a);
            self.s1.push(b);
            self.g.push(g);
        }
        self.off.push(self.s0.len() as u64);
    }
    /// 0 quiet, 1 active, 2 undecided.
    #[inline]
    fn classify(&self, s: usize, t0: u64, t1: u64) -> u8 {
        self.classify_within(s, t0, t1, u64::MAX)
    }
    /// As `classify`, where every dropped silence overlapping the window is at most `dwin` long.
    #[inline]
    fn classify_within(&self, s: usize, t0: u64, t1: u64, dwin: u64) -> u8 {
        let (a, b) = (self.off[s] as usize, self.off[s + 1] as usize);
        let j = a + self.s1[a..b].partition_point(|&e| e < t0);
        if j == b || self.s0[j] > t1 {
            return 0;
        }
        if self.s0[j] >= t0 || self.s1[j] <= t1 || self.g[j] <= t1 - t0 + 1 || dwin <= t1 - t0 + 1 {
            return 1;
        }
        2
    }
    fn stretches(&self) -> usize {
        self.s0.len()
    }
}

struct Trace {
    ch: Vec<Vec<u64>>,
    tmax: u64,
    /// Cell b covers [start[b], start[b+1]); the last cell ends at tmax.
    start: Vec<u64>,
    bytes: Vec<u64>,
}

impl Trace {
    fn cells(&self) -> usize {
        self.start.len()
    }
    fn cell_end(&self, b: usize) -> u64 {
        if b + 1 < self.start.len() { self.start[b + 1] - 1 } else { self.tmax }
    }
    fn cell_of(&self, t: u64) -> usize {
        self.start.partition_point(|&s| s <= t).max(1) - 1
    }
    /// For each signal, its changes inside cell b.
    fn rows(&self, b: usize) -> Vec<(u32, &[u64])> {
        let (lo, hi) = (self.start[b], self.cell_end(b));
        let mut out = Vec::new();
        for (s, c) in self.ch.iter().enumerate() {
            let i = c.partition_point(|&t| t < lo);
            let j = c.partition_point(|&t| t <= hi);
            if i < j {
                out.push((s as u32, &c[i..j]));
            }
        }
        out
    }
}

fn table(rows: &[(u32, &[u64])], d: u64) -> Vec<(u32, Vec<(u64, u64, u64)>)> {
    rows.iter()
        .map(|&(s, c)| {
            let mut l = Vec::new();
            split(c, d, &mut l);
            (s, l)
        })
        .collect()
}

/// Candidate thresholds: 0, 1, 2, 4, … up to the span, then the floor (no split inside a cell).
fn candidates(span: u64) -> Vec<u64> {
    let mut v = vec![0u64];
    let mut d = 1u64;
    while d < span {
        v.push(d);
        d = d.saturating_mul(2);
    }
    v.push(FLOOR);
    v
}

/// Bytes of one stretch in the packed in-memory layout (start, end, largest gap as 32-bit block offsets).
const STRETCH_BYTES: f64 = 12.0;

/// Smallest threshold whose table fits both caps: `budget` compressed bytes and the stretch cap.
/// When even one stretch per dirty signal does not fit, the cell keeps that floor.
fn choose(rows: &[(u32, &[u64])], origin: u64, span: u64, budget: f64, mem: f64) -> (u64, usize, usize) {
    let c = candidates(span);
    let cap = ((mem / STRETCH_BYTES) as usize).max(rows.len());
    let size = |d: u64| {
        let t = table(rows, d);
        let n = t.iter().map(|r| r.1.len()).sum::<usize>();
        (zsize(&encode(&t, origin, span)), n)
    };
    let fits = |z: usize, n: usize| z as f64 <= budget && n <= cap;
    let (fz, fnn) = size(FLOOR);
    if !fits(fz, fnn) {
        return (FLOOR, fz, fnn);
    }
    let (mut lo, mut hi) = (0usize, c.len() - 1);
    let mut best = (FLOOR, fz, fnn);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (z, n) = size(c[mid]);
        if fits(z, n) {
            best = (c[mid], z, n);
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    best
}

/// The single-threshold index of the whole run: smallest Δ within `eps` of the file and the stretch cap.
fn global_delta(tr: &Trace, eps: f64, mem: f64, file: f64) -> (u64, usize, usize) {
    let n = tr.ch.len();
    let span = tr.tmax - tr.start[0] + 1;
    let global = |d: u64| -> (usize, usize) {
        let rows: Vec<(u32, Vec<(u64, u64, u64)>)> = tr
            .ch
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_empty())
            .map(|(s, c)| {
                let mut l = Vec::new();
                split(c, d, &mut l);
                (s as u32, l)
            })
            .collect();
        let cnt = rows.iter().map(|r| r.1.len()).sum();
        (zsize(&encode(&rows, tr.start[0], span)), cnt)
    };
    let gc = candidates(span);
    let (mut lo, mut hi) = (0usize, gc.len() - 1);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (z, cnt) = global(gc[mid]);
        if z as f64 <= eps * file && cnt as f64 <= (mem * file / STRETCH_BYTES).max(n as f64) {
            hi = mid
        } else {
            lo = mid + 1
        }
    }
    let (z, cnt) = global(gc[lo]);
    (gc[lo], z, cnt)
}

/// Per-block thresholds of the local index. Each block takes the smallest power of two whose
/// silences ending in the block fit its own budget (`eps` of its bytes at `bps` compressed bytes
/// per stretch, and `mem` of its bytes in the packed in-memory layout), capped at a quarter of its span.
fn local_deltas(tr: &Trace, eps: f64, mem: f64, bps: f64) -> Vec<u64> {
    let nb = tr.cells();
    let mut hist = vec![[0u64; 66]; nb];
    for c in &tr.ch {
        for (i, &y) in c.iter().enumerate() {
            let cy = tr.cell_of(y);
            if i > 0 {
                let l = y - c[i - 1];
                hist[cy][64 - l.leading_zeros() as usize] += 1; // bucket k: 2^(k-1) <= l < 2^k
            }
        }
    }
    (0..nb)
        .map(|b| {
            let span_b = tr.cell_end(b) - tr.start[b] + 1;
            let cap_d = 1u64 << (63 - (span_b / 4).max(1).leading_zeros());
            let budget = eps * tr.bytes[b] as f64;
            let cap_n = (mem * tr.bytes[b] as f64 / STRETCH_BYTES) as u64;
            // Silences longer than 2^j, counted conservatively from bucket j + 1 up.
            let over = |j: usize| -> u64 { hist[b][j + 1..].iter().sum() };
            let fits = |k: u64| k as f64 * bps <= budget && k <= cap_n;
            if fits(hist[b].iter().sum()) {
                return 0;
            }
            let mut j = 0usize;
            while (1u64 << j) < cap_d && !fits(over(j)) {
                j += 1;
            }
            (1u64 << j).min(cap_d)
        })
        .collect()
}

/// One signal's stretches under the local thresholds: a silence is kept when it is longer than
/// the smallest threshold of the blocks it touches.
fn local_list(c: &[u64], tr: &Trace, ld: &[u64]) -> Vec<(u64, u64, u64)> {
    let mut l = Vec::new();
    if c.is_empty() {
        return l;
    }
    let (mut st, mut mx) = (c[0], 0u64);
    for w in c.windows(2) {
        let len = w[1] - w[0];
        let (a, b) = (tr.cell_of(w[0]), tr.cell_of(w[1]));
        if len > ld[a..=b].iter().copied().min().unwrap() {
            l.push((st, w[0], mx));
            st = w[1];
            mx = 0;
        } else {
            mx = mx.max(len);
        }
    }
    l.push((st, *c.last().unwrap(), mx));
    l
}

fn quantiles(mut v: Vec<f64>) -> (f64, f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

pub fn run(path: &str, eps: f64, mem: f64, windows: usize, threads: usize, seed: u64) -> serde_json::Value {
    let r = Reader::open(path).unwrap();
    let file = std::fs::metadata(path).unwrap().len();
    let n = r.signal_count() as usize;
    let t = Instant::now();
    let (ch, tmin, tmax) = collect(&r);
    let collect_s = t.elapsed().as_secs_f64();
    let changes: u64 = ch.iter().map(|c| c.len() as u64).sum();
    let nb = r.block_count();
    let bytes: Vec<u64> = r.sections().iter().filter(|e| e.kind == SIGNAL_BLOCK).map(|e| e.len).collect();
    assert_eq!(bytes.len(), nb, "one SIGNAL_BLOCK section per block");
    let start: Vec<u64> = (0..nb).map(|b| if b == 0 { tmin } else { r.block_range(b).0 }).collect();
    let tr = Trace { ch, tmax, start, bytes };
    let span = tmax - tmin + 1;
    let durs: Vec<f64> = (0..nb).map(|b| (tr.cell_end(b) - tr.start[b] + 1) as f64).collect();
    let (dmin, dmed, dmax) = quantiles(durs.clone());
    eprintln!("{path}: {n} signals, {changes} changes, {nb} blocks, span {span}, collect {collect_s:.1}s");

    // ---- block-anchored index: Δb per cell within eps of the cell's bytes ----
    let t = Instant::now();
    let per: Vec<(u64, usize, usize, usize)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|k| {
                let tr = &tr;
                sc.spawn(move || {
                    let mut out = Vec::new();
                    let mut b = k;
                    while b < tr.cells() {
                        let rows = tr.rows(b);
                        let span_b = tr.cell_end(b) - tr.start[b] + 1;
                        let (d, z, cnt) = choose(&rows, tr.start[b], span_b, eps * tr.bytes[b] as f64, mem * tr.bytes[b] as f64);
                        out.push((b, (d, z, cnt, rows.len())));
                        b += threads;
                    }
                    out
                })
            })
            .collect();
        let mut all: Vec<_> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
        all.sort_by_key(|x| x.0);
        all.into_iter().map(|x| x.1).collect()
    });
    let choose_s = t.elapsed().as_secs_f64();
    let deltas: Vec<u64> = per.iter().map(|p| p.0).collect();
    let block_z: usize = per.iter().map(|p| p.1).sum();
    let dirty: usize = per.iter().map(|p| p.3).sum();
    let floor_cells = deltas.iter().filter(|&&d| d == FLOOR).count();
    let rel: Vec<f64> = (0..nb).map(|b| if deltas[b] == FLOOR { 1.0 } else { deltas[b] as f64 / durs[b] }).collect();
    let (rmin, rmed, rmax) = quantiles(rel);
    // Floor-only size (one stretch per dirty signal per cell) for reference.
    let floor_z: usize = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|k| {
                let tr = &tr;
                sc.spawn(move || {
                    let mut z = 0;
                    let mut b = k;
                    while b < tr.cells() {
                        let rows = tr.rows(b);
                        let span_b = tr.cell_end(b) - tr.start[b] + 1;
                        z += zsize(&encode(&table(&rows, FLOOR), tr.start[b], span_b));
                        b += threads;
                    }
                    z
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).sum()
    });

    // Build the flattened index (single thread, timed: the per-change cost a writer would pay).
    let t = Instant::now();
    let mut ix = Index::new(n);
    let mut runs = 0usize; // runs of consecutive dirty cells per signal (the whole-run layer)
    let mut list = Vec::new();
    for c in &tr.ch {
        list.clear();
        let mut i = 0;
        let mut prev_cell = usize::MAX - 1;
        while i < c.len() {
            let b = tr.cell_of(c[i]);
            let hi = tr.cell_end(b);
            let j = i + c[i..].partition_point(|&x| x <= hi);
            split(&c[i..j], deltas[b], &mut list);
            if b != prev_cell.wrapping_add(1) {
                runs += 1;
            }
            prev_cell = b;
            i = j;
        }
        ix.push_signal(&list);
    }
    if ix.off.is_empty() {
        ix.off.push(0);
    }
    let build_s = t.elapsed().as_secs_f64();
    let largest_cell = per.iter().map(|p| p.2).max().unwrap_or(0);

    // ---- global index: one Δ for the run within the same budget ----
    let (gdelta, gz, gcnt) = global_delta(&tr, eps, mem, file as f64);
    let mut gix = Index::new(n);
    for c in &tr.ch {
        let mut l = Vec::new();
        if !c.is_empty() {
            split(c, gdelta, &mut l);
        }
        gix.push_signal(&l);
    }
    if gix.off.is_empty() {
        gix.off.push(0);
    }

    // ---- local index: uncut lists, a threshold per block capped at a quarter of its span ----
    // A silence is kept when it is longer than the smallest threshold of the blocks it touches,
    // so a window at least that wide anywhere is exact and a narrower one touches at most two blocks.
    let bps = if gcnt > 0 { gz as f64 / gcnt as f64 } else { 2.0 };
    let t = Instant::now();
    let ldelta = local_deltas(&tr, eps, mem, bps);
    // Range minimum over consecutive cells: cells touched by a silence are few, so a short scan suffices.
    let mut lix = Index::new(n);
    let mut lrows: Vec<(u32, Vec<(u64, u64, u64)>)> = Vec::new();
    for (s, c) in tr.ch.iter().enumerate() {
        let l = local_list(c, &tr, &ldelta);
        lix.push_signal(&l);
        if !l.is_empty() {
            lrows.push((s as u32, l));
        }
    }
    if lix.off.is_empty() {
        lix.off.push(0);
    }
    let local_build_s = t.elapsed().as_secs_f64();
    let lz = zsize(&encode(&lrows, tmin, span));
    drop(lrows);
    let ldrel: Vec<f64> = (0..nb).map(|b| ldelta[b] as f64 / durs[b]).collect();
    let (ldmin, ldmed, ldmax) = quantiles(ldrel);

    // ---- random windows: half uniform in time, half centred on a random change ----
    let mut rng = StdRng::seed_from_u64(seed);
    let prefix: Vec<u64> = tr.ch.iter().scan(0u64, |a, c| { *a += c.len() as u64; Some(*a) }).collect();
    let mut rows = Vec::new();
    let mut qns = Vec::new();
    let mut lrows_w: Vec<(usize, u64)> = Vec::new();
    for k in 0..windows {
        let w = ((rng.gen::<f64>() * (span as f64).ln()).exp() as u64).clamp(1, span) - 1;
        let t0 = if k % 2 == 0 || changes == 0 {
            tmin + rng.gen_range(0..=span - 1 - w)
        } else {
            let x = rng.gen_range(0..changes);
            let s = prefix.partition_point(|&p| p <= x);
            let base = if s == 0 { 0 } else { prefix[s - 1] };
            let c = tr.ch[s][(x - base) as usize];
            c.saturating_sub(w / 2).clamp(tmin, tmax - w)
        };
        let t1 = t0 + w;
        let mut truth = 0usize;
        let mut und = Vec::new();
        let mut gund = 0usize;
        let mut lund = 0usize;
        let q = Instant::now();
        let mut cls = vec![0u8; n];
        for s in 0..n {
            cls[s] = ix.classify(s, t0, t1);
        }
        qns.push(q.elapsed().as_secs_f64() * 1e9 / n.max(1) as f64);
        let (c0, c1) = (tr.cell_of(t0), tr.cell_of(t1));
        let dwin = ldelta[c0..=c1].iter().copied().min().unwrap();
        for s in 0..n {
            let c = &tr.ch[s];
            let i = c.partition_point(|&x| x < t0);
            let act = i < c.len() && c[i] <= t1;
            truth += act as usize;
            match cls[s] {
                2 => und.push(s as u32),
                v => assert_eq!(v == 1, act, "block index wrong: signal {s} [{t0},{t1}]"),
            }
            match lix.classify_within(s, t0, t1, dwin) {
                2 => lund += 1,
                v => assert_eq!(v == 1, act, "local index wrong: signal {s} [{t0},{t1}]"),
            }
            match gix.classify(s, t0, t1) {
                2 => gund += 1,
                v => assert_eq!(v == 1, act, "global index wrong: signal {s} [{t0},{t1}]"),
            }
        }
        rows.push((t0, t1, truth, und, gund, c1 - c0 + 1));
        lrows_w.push((lund, ldelta[c0..=c1].iter().copied().min().unwrap()));
    }
    let nw = rows.len() as f64;
    let exact = rows.iter().filter(|r| r.3.is_empty()).count();
    let gexact = rows.iter().filter(|r| r.4 == 0).count();
    let crossing = rows.iter().filter(|r| r.5 > 1).count();
    assert!(rows.iter().filter(|r| r.5 > 1).all(|r| r.3.is_empty()), "a window crossing a block boundary must be exact");
    let narrow: Vec<&_> = rows.iter().filter(|r| r.5 == 1 && (deltas[tr.cell_of(r.0)] == FLOOR || r.1 - r.0 + 1 < deltas[tr.cell_of(r.0)])).collect();
    assert!(rows.iter().filter(|r| r.5 == 1 && deltas[tr.cell_of(r.0)] != FLOOR && r.1 - r.0 + 1 >= deltas[tr.cell_of(r.0)]).all(|r| r.3.is_empty()), "a window at least Δb wide must be exact");
    let narrow_exact = narrow.iter().filter(|r| r.3.is_empty()).count();
    let und_share: Vec<f64> = rows.iter().filter(|r| !r.3.is_empty()).map(|r| r.3.len() as f64 / n as f64).collect();
    let (_, und_med, und_max) = quantiles(und_share);
    let gblocks = rows.iter().filter(|r| r.4 > 0).map(|r| r.5).max().unwrap_or(0);
    let (_, q_med, _) = quantiles(qns);
    let lexact = lrows_w.iter().filter(|x| x.0 == 0).count();
    assert!(rows.iter().zip(&lrows_w).all(|(r, l)| r.1 - r.0 + 1 < l.1 || l.0 == 0), "local: a window at least the smallest touched Δ wide must be exact");
    let lblocks = rows.iter().zip(&lrows_w).filter(|(_, l)| l.0 > 0).map(|(r, _)| r.5).max().unwrap_or(0);
    let lund_share: Vec<f64> = lrows_w.iter().filter(|x| x.0 > 0).map(|x| x.0 as f64 / n as f64).collect();
    let (_, lund_med, lund_max) = quantiles(lund_share);

    // ---- resolve undecided signals from the trace: the worst windows plus a few others ----
    let readers: Vec<Reader> = (0..threads).map(|_| Reader::open(path).unwrap()).collect();
    let mut todo: Vec<usize> = (0..rows.len()).filter(|&i| !rows[i].3.is_empty()).collect();
    todo.sort_by_key(|&i| std::cmp::Reverse(rows[i].3.len()));
    todo.truncate(12);
    let mut res_ms = Vec::new();
    for &i in &todo {
        let (t0, t1, _, ref und, _, _) = rows[i];
        let t = Instant::now();
        let active: usize = std::thread::scope(|sc| {
            let per = und.len().div_ceil(threads);
            let hs: Vec<_> = readers
                .iter()
                .enumerate()
                .map(|(k, rr)| {
                    sc.spawn(move || und.iter().skip(k * per).take(per).filter(|&&s| !rr.changes(SignalId(s), t0, t1).unwrap().is_empty()).count())
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).sum()
        });
        res_ms.push(t.elapsed().as_secs_f64() * 1e3);
        let _ = active;
    }
    let (_, res_med, res_max) = quantiles(res_ms);
    // Upper bound: decode the largest cell completely on one thread.
    let big = (0..nb).max_by_key(|&b| tr.bytes[b]).unwrap();
    let t = Instant::now();
    let mut cnt = 0u64;
    r.for_each_change(tr.start[big], tr.cell_end(big), |_, _, _| cnt += 1).unwrap();
    let block_decode_ms = t.elapsed().as_secs_f64() * 1e3;

    // ---- scope walk: distinct signals below every scope, all signals active ----
    let h = r.hierarchy();
    let nn = h.len();
    let mut vars_of: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut nvars = 0usize;
    for id in h.nodes_of_kind(NodeKind::Var) {
        if let (Some(s), Some(p)) = (h.signal_of(id), h.parent(id)) {
            vars_of[s.0 as usize].push(p.0);
            nvars += 1;
        }
    }
    let parent: Vec<u32> = (0..nn as u32).map(|i| h.parent(NodeId(i)).map(|p| p.0).unwrap_or(u32::MAX)).collect();
    let nscopes = h.nodes_of_kind(NodeKind::Scope).count();
    let mut cntv = vec![0u32; nn];
    let mut stamp = vec![u32::MAX; nn];
    let t = Instant::now();
    let mut steps = 0u64;
    for s in 0..n {
        for &p0 in &vars_of[s] {
            let mut p = p0;
            while p != u32::MAX && stamp[p as usize] != s as u32 {
                stamp[p as usize] = s as u32;
                cntv[p as usize] += 1;
                steps += 1;
                p = parent[p as usize];
            }
        }
    }
    let walk_ms = t.elapsed().as_secs_f64() * 1e3;

    json!({
        "file": path, "file_bytes": file, "signals": n, "vars": nvars, "scopes": nscopes, "changes": changes,
        "span": span, "blocks": nb, "block_span_min": dmin, "block_span_median": dmed, "block_span_max": dmax,
        "eps": eps, "mem": mem,
        "block_index": {
            "stretches": ix.stretches(), "dirty_pairs": dirty, "dirty_runs": runs,
            "zstd_bytes": block_z, "share": block_z as f64 / file as f64,
            "floor_zstd_bytes": floor_z, "floor_share": floor_z as f64 / file as f64,
            "floor_cells": floor_cells, "delta_over_block_min": rmin, "delta_over_block_median": rmed, "delta_over_block_max": rmax,
            "largest_cell_stretches": largest_cell, "memory_bytes_12": ix.stretches() * 12 + dirty * 4,
            "delta_max": deltas.iter().copied().filter(|&d| d != FLOOR).max(),
            "choose_s": choose_s, "build_ns_per_change": build_s * 1e9 / changes.max(1) as f64,
            "windows": rows.len(), "crossing": crossing, "narrow": narrow.len(), "narrow_exact": narrow_exact, "exact_share": exact as f64 / nw,
            "undecided_median": und_med, "undecided_max": und_max,
            "query_ns_per_signal": q_med,
            "resolve_ms_median": res_med, "resolve_ms_max": res_max, "resolve_threads": threads,
            "largest_block_decode_ms_1thread": block_decode_ms,
        },
        "global_index": {
            "delta": gdelta, "stretches": gcnt, "zstd_bytes": gz, "share": gz as f64 / file as f64,
            "exact_share": gexact as f64 / nw, "max_blocks_under_undecided_window": gblocks,
            "quarter_shortest_block": dmin / 4.0,
        },
        "local_index": {
            "stretches": lix.stretches(), "zstd_bytes": lz, "share": lz as f64 / file as f64,
            "delta_min": ldelta.iter().min(), "delta_max": ldelta.iter().max(),
            "delta_over_block_min": ldmin, "delta_over_block_median": ldmed, "delta_over_block_max": ldmax,
            "exact_share": lexact as f64 / nw, "undecided_median": lund_med, "undecided_max": lund_max,
            "max_blocks_under_undecided_window": lblocks, "build_ns_per_change": local_build_s * 1e9 / changes.max(1) as f64,
            "memory_bytes_12": lix.stretches() * 12,
        },
        "scope_walk": {"steps": steps, "ms": walk_ms},
        "collect_s": collect_s,
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

/// Writes the demo data of docs/hierarchy-activity.html: the block-anchored index of `[a, b]`
/// (Δb chosen on the whole blocks within `eps`), the scope tree, and exact answers for `wins`.
pub fn export(path: &str, a: u64, b: u64, eps: f64, mem: f64, out: &str) {
    let r = Reader::open(path).unwrap();
    let n = r.signal_count() as usize;
    let (ch, tmin, tmax) = collect(&r);
    let nb = r.block_count();
    let bytes: Vec<u64> = r.sections().iter().filter(|e| e.kind == SIGNAL_BLOCK).map(|e| e.len).collect();
    let start: Vec<u64> = (0..nb).map(|k| if k == 0 { tmin } else { r.block_range(k).0 }).collect();
    let tr = Trace { ch, tmax, start, bytes };
    let (c0, c1) = (tr.cell_of(a), tr.cell_of(b));
    let file = std::fs::metadata(path).unwrap().len() as f64;
    let (_, gz, gcnt) = global_delta(&tr, eps, mem, file);
    let ld = local_deltas(&tr, eps, mem, gz as f64 / gcnt.max(1) as f64);
    let deltas: Vec<u64> = ld[c0..=c1].to_vec();
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
    // Stretch lists of the slice under the local thresholds; identical lists shared.
    let mut map = std::collections::HashMap::new();
    let mut lists: Vec<Vec<(u64, u64, u64)>> = Vec::new();
    let mut lid = vec![0u32; n];
    let sliced: Vec<Vec<u64>> = tr.ch.iter().map(|c| c.iter().copied().filter(|&t| t >= a && t <= b).collect()).collect();
    for s in 0..n {
        let l = local_list(&sliced[s], &tr, &ld);
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
    let mut prev = a;
    for k in c0..=c1 {
        let s = tr.start[k].max(a);
        put(&mut st[11], s - prev);
        prev = s;
        put(&mut st[11], deltas[k - c0]);
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
    eprintln!("{out}: {ns} scopes, {n} signals, {} lists, {total} stretches, cells {c0}..={c1} deltas {deltas:?}, {} bytes", lists.len(), buf.len());
    for k in c0..=c1 {
        eprintln!("cell {k}: start {} end {} bytes {}", tr.start[k], tr.cell_end(k), tr.bytes[k]);
    }
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
            "ifu": fi.map(|i| cnt[i]), "vfpu": fv.map(|i| cnt[i]), "cells": tr.cell_of(t1) - tr.cell_of(t0) + 1}));
    }
    println!("{}", serde_json::to_string(&js).unwrap());
}
