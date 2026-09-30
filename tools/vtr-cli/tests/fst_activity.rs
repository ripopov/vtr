//! The FST front end of the activity index (docs/hierarchy-activity.html):
//! indexes of generated multi-block FSTs and of GTKWave fixtures packed with
//! lz4, fastlz and zlib, and gzip-wrapped, answer every window as the
//! recorded values read by fst-reader do; an FST and its VTR conversion give
//! the same answers; a block over the memory limit is refused.

use fst_writer::{FstFileType, FstInfo, FstScopeType, FstSignalType, FstVarDirection, FstVarType};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::path::{Path, PathBuf};
use vtr::activity::{self, BuildOptions, Index};
use vtr::SignalId;
use vtr_cli::fst::activity::FstTrace;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/activity").join(name)
}

/// Every recorded value after the header's start time, one per signal and
/// time step, as fst-reader reads them: the brute-force answer.
fn recorded(path: &Path) -> Vec<Vec<u64>> {
    let mut r = fst_reader::FstReader::open(std::io::BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let start = r.get_header().start_time;
    let mut ch: Vec<Vec<u64>> = vec![Vec::new(); r.get_header().max_handle as usize];
    r.read_signals(&fst_reader::FstFilter::all(), |t, h, _| {
        let c = &mut ch[h.get_index()];
        if t > start && c.last() != Some(&t) {
            c.push(t);
        }
        Ok::<(), ()>(())
    })
    .unwrap();
    ch
}

fn truth(ch: &[Vec<u64>], t0: u64, t1: u64) -> Vec<SignalId> {
    (0..ch.len())
        .filter(|&s| {
            let i = ch[s].partition_point(|&t| t < t0);
            i < ch[s].len() && ch[s][i] <= t1
        })
        .map(|s| SignalId(s as u32))
        .collect()
}

/// Windows of log-uniform width over `[lo, hi]`, half ending on a change.
fn windows(rng: &mut StdRng, ch: &[Vec<u64>], lo: u64, hi: u64, n: usize) -> Vec<(u64, u64)> {
    let all: Vec<u64> = ch.iter().flatten().copied().collect();
    let span = hi - lo + 1;
    (0..n)
        .map(|k| {
            let w = ((rng.gen::<f64>() * (span as f64).ln()).exp() as u64).clamp(1, span) - 1;
            if k % 2 == 0 || all.is_empty() {
                let t0 = lo + rng.gen_range(0..=span - 1 - w);
                (t0, t0 + w)
            } else {
                let t = all[rng.gen_range(0..all.len())];
                (t.saturating_sub(w), t)
            }
        })
        .collect()
}

/// Builds `path`'s index and checks classify + resolve, and resolve alone,
/// against the recorded values in every window; returns the index.
fn check(path: &Path, threads: usize, seed: u64) -> (FstTrace, Index) {
    let f = FstTrace::open(path).unwrap();
    let mut image = Vec::new();
    let summary = f.build(&mut image, &BuildOptions { threads, ..Default::default() }).unwrap();
    let index = Index::decode(&image, &f.identity()).unwrap();
    let ch = recorded(path);
    assert_eq!(summary.changes, ch.iter().map(|c| c.len() as u64).sum::<u64>(), "{}: changes", path.display());
    assert_eq!(index.signal_count() as usize, ch.len());
    let (lo, hi) = (index.blocks()[0].start, index.blocks().last().unwrap().end);
    let every: Vec<SignalId> = (0..ch.len() as u32).map(SignalId).collect();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut wins = windows(&mut rng, &ch, lo, hi, 120);
    wins.extend([(0, lo), (lo, hi), (hi, hi + 9), (hi + 1, u64::MAX)]);
    for (t0, t1) in wins {
        let want = truth(&ch, t0, t1);
        let c = index.classify(t0, t1);
        assert!(c.active.iter().all(|s| want.binary_search(s).is_ok()), "{}: [{t0}, {t1}] a quiet signal classified active", path.display());
        let mut answer = c.active.clone();
        answer.extend(f.resolve(&c.undecided, t0, t1).unwrap());
        answer.sort();
        assert_eq!(answer, want, "{}: [{t0}, {t1}] classify + resolve", path.display());
        assert_eq!(f.resolve(&every, t0, t1).unwrap(), want, "{}: [{t0}, {t1}] resolve alone", path.display());
    }
    (f, index)
}

/// A multi-block FST from fst-writer (alias2 blocks, lz4 chains): clocks,
/// buses, reals, a hierarchy alias, sleeps and bursts.
fn write_fst(path: &Path, seed: u64, steps: usize, block_steps: usize) {
    let mut rng = StdRng::seed_from_u64(seed);
    let info = FstInfo { start_time: 0, timescale_exponent: -12, version: "test".into(), date: "".into(), file_type: FstFileType::Verilog };
    let mut h = fst_writer::open_fst(path, &info).unwrap();
    h.scope("top", "top", FstScopeType::Module).unwrap();
    let mut sigs = Vec::new();
    for i in 0..rng.gen_range(10..40) {
        let (tpe, width) = match i % 4 {
            0 => (FstSignalType::bit_vec(1), 1),
            1 => (FstSignalType::bit_vec(8), 8),
            2 => (FstSignalType::bit_vec(33), 33),
            _ => (FstSignalType::real(), 0),
        };
        let vt = if width == 0 { FstVarType::Real } else { FstVarType::Wire };
        let id = h.var(format!("s{i}"), tpe, vt, FstVarDirection::Implicit, None).unwrap();
        let p = [1.0, 0.3, 0.02, 0.001][rng.gen_range(0..4)];
        sigs.push((id, width, p));
    }
    h.var("alias", FstSignalType::bit_vec(1), FstVarType::Wire, FstVarDirection::Implicit, Some(sigs[0].0)).unwrap();
    h.up_scope().unwrap();
    let mut b = h.finish().unwrap();
    let mut t = 0u64;
    let value = |rng: &mut StdRng, width: usize| -> Vec<u8> {
        if width == 0 {
            rng.gen::<f64>().to_le_bytes().to_vec()
        } else {
            (0..width).map(|_| b"01xz"[if rng.gen_ratio(1, 20) { rng.gen_range(2..4) } else { rng.gen_range(0..2) }]).collect()
        }
    };
    let mut awake = true;
    for step in 0..steps {
        if step > 0 && step % block_steps == 0 {
            b.flush().unwrap();
        }
        b.time_change(t).unwrap();
        for &(id, width, p) in &sigs {
            if step == 0 || (awake && rng.gen_bool(p)) {
                let v = value(&mut rng, width);
                b.signal_change(id, &v).unwrap();
            }
        }
        t += if rng.gen_ratio(1, 50) { rng.gen_range(1_000..1_000_000) } else { rng.gen_range(1..20) };
        if rng.gen_ratio(1, 150) {
            awake = !awake;
        }
    }
    b.finish().unwrap();
}

#[test]
fn generated_multi_block_fsts_match_fst_reader() {
    let dir = tempfile::tempdir().unwrap();
    for seed in 0..6 {
        let path = dir.path().join(format!("t{seed}.fst"));
        write_fst(&path, seed, 3000, [150, 700, 3000][seed as usize % 3]);
        let (f, index) = check(&path, 1 + seed as usize % 3, seed);
        assert!(index.blocks().len() > 1 || seed % 3 == 2);
        // Rebuilding with other thread counts gives the same bytes.
        let image = |threads| {
            let mut v = Vec::new();
            f.build(&mut v, &BuildOptions { threads, ..Default::default() }).unwrap();
            v
        };
        assert_eq!(image(1), image(8));
    }
}

#[test]
fn gtkwave_packings_and_the_gzip_wrapper() {
    let mut answers = Vec::new();
    for name in ["lz4.fst", "fastlz.fst", "zlib.fst", "wrapped.fst"] {
        let (f, index) = check(&fixture(name), 2, 1);
        answers.push((0..40u64).map(|k| index.classify(k * 17_000, k * 17_000 + 900)).map(|c| (c.active, c.undecided.len())).collect::<Vec<_>>());
        if name == "wrapped.fst" {
            // Bound to the wrapped file itself; the unwrapped copy goes with the trace.
            assert_eq!(f.identity().length, std::fs::metadata(fixture(name)).unwrap().len());
            let left = std::fs::read_dir(std::env::temp_dir()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(&format!("vtr-fst-{}-", std::process::id()))).count();
            assert_eq!(left, 1);
            drop(f);
            let left = std::fs::read_dir(std::env::temp_dir()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(&format!("vtr-fst-{}-", std::process::id()))).count();
            assert_eq!(left, 0);
        }
    }
    // One VCD, four packings: the same recorded values.
    assert!(answers.windows(2).all(|w| w[0] == w[1]));
}

/// The trace's answer, not the format's: an FST and its VTR conversion
/// (recording the same values) answer every window alike.
#[test]
fn an_fst_and_its_vtr_conversion_agree() {
    let dir = tempfile::tempdir().unwrap();
    let fst = dir.path().join("run.fst");
    let vtr_path = dir.path().join("run.vtr");
    write_fst(&fst, 42, 4000, 500);
    let opts = vtr::WriterOptions { dedup: false, block_records: 3000, ..Default::default() };
    let mut w = vtr::Writer::create_with(&vtr_path, opts).unwrap();
    vtr_cli::fst::convert_fst(fst.to_str().unwrap(), &mut w, &vtr_cli::fst::FstConvertOptions { states: None, progress: false }).unwrap();
    w.close().unwrap();
    let (f, fi) = check(&fst, 2, 7);
    let r = vtr::Reader::open(&vtr_path).unwrap();
    let mut image = Vec::new();
    activity::build(&r, &mut image, &BuildOptions::default()).unwrap();
    let vi = Index::decode(&image, &activity::Identity::of(&r).unwrap()).unwrap();
    assert_eq!(r.signal_count(), f.signal_count());
    assert_eq!(vi.t_min(), fi.t_min());
    let (lo, hi) = r.time_range().unwrap();
    let mut rng = StdRng::seed_from_u64(9);
    let ch = recorded(&fst);
    for (t0, t1) in windows(&mut rng, &ch, lo, hi, 200) {
        let answer = |c: activity::Classification, resolve: &dyn Fn(&[SignalId]) -> Vec<SignalId>| {
            let mut a = c.active;
            a.extend(resolve(&c.undecided));
            a.sort();
            a
        };
        let from_fst = answer(fi.classify(t0, t1), &|u| f.resolve(u, t0, t1).unwrap());
        let from_vtr = answer(vi.classify(t0, t1), &|u| activity::resolve(&r, u, t0, t1).unwrap());
        assert_eq!(from_fst, from_vtr, "[{t0}, {t1}]");
    }
}

#[test]
fn a_block_over_the_memory_limit_is_refused() {
    let f = FstTrace::open(fixture("zlib.fst")).unwrap();
    let err = f.build(Vec::new(), &BuildOptions { memory: 1 << 10, ..Default::default() }).unwrap_err().to_string();
    assert!(err.contains("block 0") && err.contains("memory limit"), "{err}");
}

#[test]
fn an_unfinished_fst_has_no_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cut.fst");
    let bytes = std::fs::read(fixture("lz4.fst")).unwrap();
    // Header and value changes only: the geometry and hierarchy come at close.
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    assert!(FstTrace::open(&path).is_err());
}
