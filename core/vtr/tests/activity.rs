//! Activity index (docs/hierarchy-activity.html): the sidecar round-trips
//! through the loader and agrees with every change of every signal, rebuilds
//! byte for byte at any thread count, follows the threshold rule, and is
//! rejected for any other trace.

use rand::{rngs::StdRng, Rng, SeedableRng};
use std::path::{Path, PathBuf};
use vtr::activity::{self, Block, BlockScan, Budget, BuildOptions, Builder, Identity, Index, Sidecar, SourceFormat};
use vtr::*;

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A trace of mixed kinds and rhythms over many small blocks: clocks,
/// bursts with long sleeps, rare events, constants, copies that the writer
/// stores as dynamic aliases, repeated values in one time step and changes
/// at the first time step.
fn random_trace(path: &Path, seed: u64, steps: usize, block_records: usize) {
    let mut rng = StdRng::seed_from_u64(seed);
    let opts = WriterOptions { block_records, group_size: 64, chunk_records: 256, ..Default::default() };
    let mut w = Writer::create_with(path, opts).unwrap();
    let top = Some(w.add_scope(None, "top", ScopeType::Module, "").unwrap());
    let kinds = [
        SignalKind::Bits { width: 1, states: 2 },
        SignalKind::Bits { width: 1, states: 4 },
        SignalKind::Bits { width: 8, states: 2 },
        SignalKind::Bits { width: 64, states: 4 },
        SignalKind::Bits { width: 300, states: 2 },
        SignalKind::Real,
        SignalKind::VarLen,
    ];
    let n = rng.gen_range(20..90);
    // (signal, kind, per-step change probability while awake, copy of)
    let mut sigs = Vec::new();
    for i in 0..n {
        let kind = kinds[rng.gen_range(0..kinds.len())];
        let (_, s) = w.add_var(top, &format!("s{i}"), VarType::Wire, Direction::Implicit, kind).unwrap();
        let p = match rng.gen_range(0..5) {
            0 => 1.0,
            1 => 0.3,
            2 => 0.02,
            3 => 0.001,
            _ => 0.0,
        };
        let copy = if i > 0 && rng.gen_ratio(1, 6) { Some(rng.gen_range(0..i)) } else { None };
        sigs.push((s, kind, p, copy));
    }
    let mut vals = vec![0u64; n];
    let mut t = rng.gen_range(0..50u64);
    let mut awake = true;
    for step in 0..steps {
        w.set_time(t).unwrap();
        for i in 0..n {
            let (s, kind, p, copy) = sigs[i];
            let v = match copy {
                Some(c) => vals[c],
                None if step == 0 || (awake && rng.gen_bool(p)) => vals[i] + 1 + rng.gen_range(0..3),
                None => continue,
            };
            if v == vals[i] && step > 0 {
                continue;
            }
            vals[i] = v;
            emit(&mut w, s, kind, v);
            if rng.gen_ratio(1, 40) {
                // A glitch: two values in one time step are one change.
                emit(&mut w, s, kind, v + 7);
                emit(&mut w, s, kind, v);
            }
        }
        t += match rng.gen_range(0..100) {
            0..=79 => rng.gen_range(1..4),
            80..=97 => rng.gen_range(4..200),
            _ => rng.gen_range(1_000..1_000_000),
        };
        if rng.gen_ratio(1, 200) {
            awake = !awake;
        }
    }
    w.close().unwrap();
}

fn emit(w: &mut Writer, s: SignalId, kind: SignalKind, v: u64) {
    match kind {
        SignalKind::Bits { width: 1, .. } => w.emit_bit(s, (v & 1) as u8).unwrap(),
        SignalKind::Bits { width, .. } => w.emit_packed(s, 2, &vec![v as u8; (width as usize).div_ceil(8)]).unwrap(),
        SignalKind::Real => w.emit_real(s, v as f64).unwrap(),
        SignalKind::VarLen => w.emit_varlen(s, format!("v{v}").as_bytes()).unwrap(),
    }
}

/// Every change of every signal after the first time step, one per time step.
fn changes(r: &Reader) -> (u64, Vec<Vec<u64>>) {
    let t_min = if r.block_count() > 0 { r.block_range(0).0 } else { 0 };
    let out = (0..r.signal_count())
        .map(|s| {
            let mut c: Vec<u64> = r.load_signal(SignalId(s)).unwrap().times().iter().copied().filter(|&t| t > t_min).collect();
            c.dedup();
            c
        })
        .collect();
    (t_min, out)
}

fn index_of(r: &Reader, opts: &BuildOptions) -> (Vec<u8>, activity::Summary, Index) {
    let mut image = Vec::new();
    let summary = activity::build(r, &mut image, opts).unwrap();
    let index = Index::decode(&image, &Identity::of(r).unwrap()).unwrap();
    (image, summary, index)
}

/// The cells the interior `[x, y]` of a silence touches, by block ends.
fn cells(index: &Index, x: u64, y: u64) -> std::ops::RangeInclusive<usize> {
    let ends: Vec<u64> = index.blocks().iter().map(|b| b.end).collect();
    let cell = |t: u64| ends.partition_point(|&e| e < t).min(ends.len() - 1);
    cell(x)..=cell(y)
}

/// Checks the stretch lists against the changes: stretches start and end at
/// changes, the silences between them are real, gaps are exact, and every
/// silence inside a stretch is at most the smallest Δ of the cells its
/// interior touches, which is what makes wide windows exact.
fn check_against_changes(index: &Index, t_min: u64, ch: &[Vec<u64>]) {
    assert_eq!(index.t_min(), t_min);
    assert_eq!(index.signal_count() as usize, ch.len());
    for (s, c) in ch.iter().enumerate() {
        let st: Vec<_> = index.stretches(SignalId(s as u32)).collect();
        if c.is_empty() {
            assert!(st.is_empty(), "signal {s} never changes but has stretches");
            continue;
        }
        assert_eq!(st.first().unwrap().start, c[0], "signal {s}: first change");
        assert_eq!(st.last().unwrap().end, *c.last().unwrap(), "signal {s}: last change");
        let mut i = 0;
        for (k, x) in st.iter().enumerate() {
            assert_eq!(c[i], x.start, "signal {s} stretch {k} starts at a change");
            let mut gap = 0;
            while c[i] < x.end {
                let len = c[i + 1] - c[i];
                gap = gap.max(len);
                if len >= 2 {
                    let dmin = cells(index, c[i] + 1, c[i + 1] - 1).map(|b| index.blocks()[b].delta).min().unwrap();
                    assert!(len <= dmin, "signal {s}: dropped silence {}..{} longer than Δ {dmin}", c[i], c[i + 1]);
                }
                i += 1;
            }
            assert_eq!(c[i], x.end, "signal {s} stretch {k} ends at a change");
            assert_eq!(x.gap, gap, "signal {s} stretch {k} gap");
            i += 1;
        }
        assert_eq!(i, c.len(), "signal {s}: stretches cover every change");
    }
}

/// Dynamic aliases stored in the file's blocks.
fn alias_count(path: &Path, r: &Reader) -> usize {
    let bytes = std::fs::read(path).unwrap();
    let c = vtr::container::Container::parse(&bytes).unwrap();
    let mut n = 0;
    for e in c.entries.iter().filter(|e| e.kind == vtr::container::SectionKind::SignalBlock as u32) {
        let p = vtr::container::Container::payload(&bytes, e, false).unwrap();
        let h = vtr::block::BlockHeader::parse(p).unwrap();
        for (g, clen, off) in vtr::block::dirty_groups(p, &h) {
            let view = vtr::block::GroupView::parse(vtr::block::group_container(p, &h, clen, off).unwrap(), g * r.meta().group_size, &r.hierarchy().signals).unwrap();
            n += view.aliases.len();
        }
    }
    n
}

#[test]
fn round_trip_matches_every_change() {
    let dir = tmpdir();
    let (mut aliases, mut shared) = (0, 0);
    for seed in 0..12 {
        let path = dir.path().join(format!("t{seed}.vtr"));
        random_trace(&path, seed, 1500, [150, 700, 4000][seed as usize % 3]);
        let r = Reader::open(&path).unwrap();
        assert!(r.block_count() > 1 || seed % 3 == 2);
        aliases += alias_count(&path, &r);
        shared += (1..r.block_count()).filter(|&k| r.block_range(k - 1).1 == r.block_range(k).0).count();
        let (t_min, ch) = changes(&r);
        for budget in [Budget::default(), Budget { disk: 1.0, memory: 1.0 }, Budget { disk: 0.0, memory: 0.0 }] {
            let (_, summary, index) = index_of(&r, &BuildOptions { threads: 3, budget, ..Default::default() });
            check_against_changes(&index, t_min, &ch);
            assert_eq!(summary.stretches, index.stretch_count());
            assert_eq!(summary.changes, ch.iter().map(|c| c.len() as u64).sum::<u64>());
            assert_eq!(summary.blocks as usize, r.block_count());
            assert_eq!(index.budget(), budget);
            // Δ is a power of two within a quarter of its cell.
            for (k, b) in index.blocks().iter().enumerate() {
                let span = if k == 0 { b.end - b.start + 1 } else { b.end - index.blocks()[k - 1].end };
                assert!(b.delta.is_power_of_two() && b.delta <= (span / 4).max(1), "block {k}: Δ {} span {span}", b.delta);
                assert_eq!((b.start, b.end), r.block_range(k));
            }
        }
    }
    assert!(aliases > 0 && shared > 0, "the traces exercise dynamic aliases ({aliases}) and shared block boundaries ({shared})");
}

/// Windows of log-uniform width: placed uniformly, starting or ending
/// exactly on a change, or starting or ending on a block's boundary.
fn windows(rng: &mut StdRng, r: &Reader, ch: &[Vec<u64>], n: usize) -> Vec<(u64, u64)> {
    let (lo, hi) = r.time_range().unwrap();
    let span = hi - lo + 1;
    let all: Vec<u64> = ch.iter().flatten().copied().collect();
    let edges: Vec<u64> = (0..r.block_count()).flat_map(|k| [r.block_range(k).0, r.block_range(k).1]).collect();
    (0..n)
        .map(|k| {
            let w = ((rng.gen::<f64>() * (span as f64).ln()).exp() as u64).clamp(1, span) - 1;
            let at = |rng: &mut StdRng, v: &[u64]| if v.is_empty() { lo } else { v[rng.gen_range(0..v.len())] };
            let (t0, t1) = match k % 5 {
                0 => {
                    let t0 = lo + rng.gen_range(0..=span - 1 - w);
                    (t0, t0 + w)
                }
                1 => {
                    let t = at(rng, &all);
                    (t, t + w)
                }
                2 => {
                    let t = at(rng, &all);
                    (t.saturating_sub(w), t)
                }
                3 => {
                    let t = at(rng, &edges) + rng.gen_range(0..2);
                    (t, t + w)
                }
                _ => {
                    let t = at(rng, &edges) + rng.gen_range(0..2);
                    (t.saturating_sub(w), t)
                }
            };
            (t0, t1)
        })
        .collect()
}

fn truth(ch: &[Vec<u64>], t0: u64, t1: u64) -> Vec<SignalId> {
    (0..ch.len() as u32)
        .filter(|&s| {
            let c = &ch[s as usize];
            let i = c.partition_point(|&t| t < t0);
            i < c.len() && c[i] <= t1
        })
        .map(SignalId)
        .collect()
}

/// `classify` is never wrong and decides every window at least `exact_width`
/// wide; an undecided window touches at most two blocks; `resolve` of the
/// undecided signals completes the exact answer, and `resolve` of every
/// signal is the exact answer on its own.
#[test]
fn classify_and_resolve_match_every_change() {
    let dir = tmpdir();
    let mut rng = StdRng::seed_from_u64(3);
    let (mut undecided_windows, mut exact_windows) = (0, 0);
    for seed in 20..30 {
        let path = dir.path().join(format!("t{seed}.vtr"));
        random_trace(&path, seed, 2500, [120, 400, 3000][seed as usize % 3]);
        let r = Reader::open(&path).unwrap();
        let (_, ch) = changes(&r);
        let every: Vec<SignalId> = (0..r.signal_count()).map(SignalId).collect();
        for budget in [Budget::default(), Budget { disk: 0.0, memory: 0.0 }] {
            let (_, _, index) = index_of(&r, &BuildOptions { threads: 2, budget, ..Default::default() });
            let mut wins = windows(&mut rng, &r, &ch, 150);
            let (lo, hi) = r.time_range().unwrap();
            wins.extend([(0, lo), (lo, lo), (hi, hi + 5), (hi + 1, u64::MAX), (0, u64::MAX), (lo + 3, lo + 2)]);
            for (t0, t1) in wins {
                let want = truth(&ch, t0, t1);
                let c = index.classify(t0, t1);
                let win = format!("[{t0}, {t1}] seed {seed} {budget:?}");
                assert!(c.active.iter().all(|s| want.binary_search(s).is_ok()), "{win}: a quiet signal classified active");
                assert!(want.iter().all(|s| c.active.binary_search(s).is_ok() || c.undecided.binary_search(s).is_ok()), "{win}: an active signal classified quiet");
                if t0 <= t1 && (t1 - t0).saturating_add(1) >= index.exact_width(t0, t1) {
                    assert!(c.undecided.is_empty(), "{win}: at least Δ wide but undecided");
                }
                if !c.undecided.is_empty() {
                    undecided_windows += 1;
                    let blocks = cells(&index, t0, t1).count();
                    assert!(blocks <= 2, "{win}: an undecided window touches {blocks} blocks");
                } else {
                    exact_windows += 1;
                }
                let mut exact = c.active.clone();
                exact.extend(activity::resolve(&r, &c.undecided, t0, t1).unwrap());
                exact.sort();
                assert_eq!(exact, want, "{win}: classify + resolve");
                assert_eq!(activity::resolve(&r, &every, t0, t1).unwrap(), want, "{win}: resolve alone");
            }
        }
    }
    assert!(undecided_windows > 100 && exact_windows > 100, "both kinds of window are tested ({undecided_windows} undecided, {exact_windows} exact)");
}

#[test]
fn rebuild_is_byte_identical_at_any_thread_count() {
    let dir = tmpdir();
    let path = dir.path().join("t.vtr");
    random_trace(&path, 99, 4000, 300);
    let r = Reader::open(&path).unwrap();
    let images: Vec<Vec<u8>> = [1, 2, 7, 32].iter().map(|&threads| index_of(&r, &BuildOptions { threads, ..Default::default() }).0).collect();
    assert!(images.windows(2).all(|w| w[0] == w[1]));
}

/// Builds from synthetic scans: one signal whose silences in a block are
/// 2, 4, 8, … 2^12 time units long, and blocks of very different sizes.
#[test]
fn threshold_follows_each_blocks_budget() {
    let id = Identity { format: SourceFormat::Vtr, length: 1, toc_crc: 0 };
    let budget = Budget { disk: 1.0, memory: 0.04 };
    let mut image = Vec::new();
    let mut b = Builder::new(&mut image, id, 1, 0, budget).unwrap();
    let mut starts = Vec::new();
    // Byte sizes allow 12, 5, 0 and 100 stretches in memory (20 bytes each at 4%).
    for bytes in [6_000u64, 2_500, 1, 50_000] {
        let start = b.prev_end().map_or(1, |e| e + 1);
        let mut times = vec![start];
        for k in 1..=12 {
            times.push(times.last().unwrap() + (1 << k));
        }
        let end = *times.last().unwrap() + 100_000;
        let mut scan = BlockScan::new(Block { start, end, bytes }, b.prev_end(), 0, budget);
        for &t in &times {
            scan.push(0, t);
        }
        starts.push(start);
        b.add(scan).unwrap();
    }
    b.finish().unwrap();
    let index = Index::decode(&image, &id).unwrap();
    let deltas: Vec<u64> = index.blocks().iter().map(|b| b.delta).collect();
    // 12 silences fit (Δ 1 keeps all); 5 fit from Δ 2^7 (lengths 2^8..2^12,
    // counted with the silence into the block); none fit: Δ reaches its
    // quarter-span cap; 100 fit.
    let cap = |k: usize| {
        let span = index.blocks()[k].end - index.blocks()[k - 1].end;
        1u64 << (63 - (span / 4).leading_zeros())
    };
    assert_eq!(deltas[0], 1);
    assert_eq!(deltas[1], 1 << 8);
    assert_eq!(deltas[2], cap(2));
    assert_eq!(deltas[3], 1);
    // The zero-length disk budget pushes every block to its cap.
    let mut image = Vec::new();
    let tight = Budget { disk: 0.0, memory: 1.0 };
    let mut b = Builder::new(&mut image, id, 1, 0, tight).unwrap();
    let mut scan = BlockScan::new(Block { start: 1, end: 4096, bytes: 1 << 20 }, None, 0, tight);
    for t in [1, 3, 7, 15, 31, 63] {
        scan.push(0, t);
    }
    b.add(scan).unwrap();
    b.finish().unwrap();
    assert_eq!(Index::decode(&image, &id).unwrap().blocks()[0].delta, 1024);
}

#[test]
fn scans_are_checked_against_the_builder() {
    let id = Identity { format: SourceFormat::Vtr, length: 1, toc_crc: 0 };
    let budget = Budget::default();
    let mut b = Builder::new(Vec::new(), id, 2, 0, budget).unwrap();
    let block = Block { start: 10, end: 20, bytes: 100 };
    let mut twice = BlockScan::new(block, None, 0, budget);
    twice.push(0, 11);
    twice.push(1, 12);
    twice.push(0, 13);
    assert!(b.add(twice).is_err());
    let mut unknown = BlockScan::new(block, None, 0, budget);
    unknown.push(2, 11);
    assert!(b.add(unknown).is_err());
    assert!(b.add(BlockScan::new(block, Some(5), 0, budget)).is_err(), "wrong predecessor");
    b.add(BlockScan::new(block, None, 0, budget)).unwrap();
    assert!(b.add(BlockScan::new(Block { start: 15, end: 30, bytes: 1 }, Some(20), 0, budget)).is_err(), "overlapping block");
}

#[test]
fn sidecar_is_bound_to_its_trace() {
    let dir = tmpdir();
    let path = dir.path().join("run.vtr");
    random_trace(&path, 5, 800, 200);
    let r = Reader::open(&path).unwrap();
    let id = Identity::of(&r).unwrap();
    let sidecar = Sidecar::new(&path, &id, None);
    assert_eq!(sidecar.beside, dir.path().join("run.vtr.index"));
    let (written, summary) = sidecar.write(|w| activity::build(&r, w, &BuildOptions::default())).unwrap();
    assert_eq!(written, sidecar.beside);
    assert_eq!(std::fs::metadata(&written).unwrap().len(), summary.bytes);
    assert!(!dir.path().join("run.vtr.index.tmp").exists());
    assert_eq!(sidecar.load(&id).unwrap().1.stretch_count(), summary.stretches);
    let image = std::fs::read(&written).unwrap();
    drop(r);

    // Replaced by a re-simulated trace: another identity, the old index is ignored.
    random_trace(&path, 6, 800, 200);
    let r2 = Reader::open(&path).unwrap();
    let id2 = Identity::of(&r2).unwrap();
    assert_ne!(id, id2);
    assert!(matches!(Index::decode(&image, &id2), Err(Error::Invalid(_))));
    assert!(Sidecar::new(&path, &id2, None).load(&id2).is_none());
    drop(r2);

    // Truncated, or recovered without its trailer: no identity at all.
    let bytes = std::fs::read(&path).unwrap();
    let cut = dir.path().join("cut.vtr");
    std::fs::write(&cut, &bytes[..bytes.len() - 100]).unwrap();
    let r3 = Reader::open(&cut).unwrap();
    assert!(r3.recovered().is_some());
    assert!(matches!(Identity::of(&r3), Err(Error::Invalid(_))));
    assert!(activity::build(&r3, Vec::new(), &BuildOptions::default()).is_err());

    // A damaged or foreign-version index fails clearly.
    assert!(Index::decode(&image[..image.len() - 1], &id).is_err());
    let mut flipped = image.clone();
    flipped[200] ^= 0x40;
    assert!(Index::decode(&flipped, &id).is_err());
    let mut v2 = image.clone();
    v2[8] = 2;
    assert!(matches!(Index::decode(&v2, &id), Err(Error::UnsupportedVersion { major: 2, .. })));
}

#[test]
fn read_only_directory_falls_back_to_the_cache() {
    let dir = tmpdir();
    let traces = dir.path().join("traces");
    let cache = dir.path().join("cache").join("vtr").join("index");
    std::fs::create_dir(&traces).unwrap();
    let path = traces.join("run.vtr");
    random_trace(&path, 7, 500, 200);
    let r = Reader::open(&path).unwrap();
    let id = Identity::of(&r).unwrap();
    let sidecar = Sidecar::new(&path, &id, Some(&cache));
    set_read_only(&traces, true);
    if std::fs::File::create(traces.join("probe")).is_ok() {
        set_read_only(&traces, false);
        eprintln!("skipped: the directory stays writable (running as root)");
        return;
    }
    let result = sidecar.write(|w| activity::build(&r, w, &BuildOptions::default()));
    set_read_only(&traces, false);
    let (written, _) = result.unwrap();
    assert_eq!(Some(&written), sidecar.cached.as_ref());
    assert!(written.starts_with(&cache));
    assert!(!sidecar.beside.exists());
    // A copy of the trace elsewhere finds the same cached index.
    let copy = dir.path().join("copy.vtr");
    std::fs::copy(&path, &copy).unwrap();
    let found = Sidecar::new(&copy, &id, Some(&cache)).load(&id).unwrap();
    assert_eq!(found.0, written);
}

#[test]
fn a_failed_build_leaves_no_file() {
    let dir = tmpdir();
    let path = dir.path().join("run.vtr");
    random_trace(&path, 8, 300, 200);
    let r = Reader::open(&path).unwrap();
    let id = Identity::of(&r).unwrap();
    let sidecar = Sidecar::new(&path, &id, None);
    let err = sidecar.write(|w| {
        w.write_all(b"partial")?;
        Err(Error::State("cancelled"))
    });
    assert!(err.is_err());
    let left: Vec<PathBuf> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(left, vec![path]);
}

#[cfg(unix)]
fn set_read_only(dir: &Path, ro: bool) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(if ro { 0o555 } else { 0o755 })).unwrap();
}

#[cfg(not(unix))]
fn set_read_only(dir: &Path, ro: bool) {
    let mut p = std::fs::metadata(dir).unwrap().permissions();
    p.set_readonly(ro);
    std::fs::set_permissions(dir, p).unwrap();
}
