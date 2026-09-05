//! Read/navigation benchmarks: wellen (FST) versus VTR on the same trace,
//! with result parity checks. Mirrors the wavepeek query patterns.

use crate::replay::Rng;
use crate::util::best_of;
use serde_json::json;
use std::time::Instant;
use vtr::{Reader, SignalId};
use wellen::SignalRef;

struct Plan {
    sigs: Vec<u32>,        // random signal indices
    times: Vec<u64>,       // random query times
    window: (u64, u64),    // 1% window in the middle
    scan_sig: u32,         // signal for change scan
    clk: u32,              // most active 1-bit signal (posedge trigger)
    bus: u32,              // a vector signal for the condition
}

fn make_plan(vtr: &Reader, seed: u64) -> Plan {
    let n = vtr.signal_count();
    let mut rng = Rng(seed);
    let mut sigs = Vec::new();
    for _ in 0..1000 {
        sigs.push(rng.below(n as u64) as u32);
    }
    let (t0, t1) = vtr.time_range().unwrap_or((0, 1));
    let mut times = Vec::new();
    for _ in 0..100 {
        times.push(t0 + rng.below(t1 - t0 + 1));
    }
    let span = (t1 - t0).max(100);
    let mid = t0 + span / 2;
    let window = (mid, mid + span / 100);
    // clk: 1-bit signal with the most changes among a sample; bus: widest signal among the sample.
    let h = vtr.hierarchy();
    let mut best_clk = (0u32, 0usize);
    let mut best_bus = (0u32, 0u32);
    let sample: Vec<u32> = (0..n.min(400)).map(|i| i * (n / n.min(400)).max(1)).collect();
    for &s in &sample {
        match h.signal_kind(SignalId(s)) {
            Some(vtr::SignalKind::Bits { width: 1, .. }) => {
                let c = vtr.load_signal(SignalId(s)).map(|d| d.len()).unwrap_or(0);
                if c > best_clk.1 {
                    best_clk = (s, c);
                }
            }
            Some(vtr::SignalKind::Bits { width, .. }) if width > best_bus.1 && width <= 64 => best_bus = (s, width),
            _ => {}
        }
    }
    Plan { scan_sig: sigs[0], sigs, times, window, clk: best_clk.0, bus: best_bus.0 }
}

/// Signals and times used by the point queries (shared with the C harness).
pub fn plan(vtr: &Reader, seed: u64) -> (Vec<u32>, Vec<u64>) {
    let p = make_plan(vtr, seed);
    (p.sigs, p.times)
}

pub fn run(fst_path: &str, vtr_path: &str, seed: u64) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let mut parity_errors = 0u64;

    // ---- open ----
    let (vtr_open, vtr) = best_of(3, || Reader::open(vtr_path).expect("open vtr"));
    let (wl_open, wl) = best_of(3, || wellen::simple::read(fst_path).expect("open fst"));
    out.insert("open".into(), json!({"fst_wellen": wl_open, "vtr": vtr_open}));
    let plan = make_plan(&vtr, seed);

    // ---- hierarchy browse: every var with its full path ----
    let (t_wl, n_wl) = best_of(3, || {
        let h = wl.hierarchy();
        let mut n = 0usize;
        let mut bytes = 0usize;
        for v in h.all_vars() {
            let var = &h[v];
            bytes += var.full_name(h).len();
            n += 1;
        }
        (n, bytes)
    });
    let (t_vtr, n_vtr) = best_of(3, || {
        // Depth-first walk building each full path in one reusable buffer.
        fn walk(vtr: &Reader, node: vtr::NodeId, path: &mut String, n: &mut usize, bytes: &mut usize) {
            let h = vtr.hierarchy();
            let len = path.len();
            if len > 0 {
                path.push('.');
            }
            path.push_str(vtr.name(node));
            if h.kind(node) == vtr::NodeKind::Var {
                *bytes += path.len();
                *n += 1;
            }
            for c in h.children(node) {
                walk(vtr, c, path, n, bytes);
            }
            path.truncate(len);
        }
        let mut n = 0usize;
        let mut bytes = 0usize;
        let mut path = String::new();
        for r in vtr.hierarchy().roots() {
            walk(&vtr, r, &mut path, &mut n, &mut bytes);
        }
        (n, bytes)
    });
    {
        let f = std::fs::File::open(fst_path).unwrap();
        let hdr = fst_reader::FstReader::open(std::io::BufReader::new(f)).unwrap().get_header();
        if hdr.var_count != n_vtr.0 as u64 {
            parity_errors += 1;
            eprintln!("parity: var count differs fst {} vs vtr {} (wellen lists {})", hdr.var_count, n_vtr.0, n_wl.0);
        }
    }
    out.insert("hierarchy".into(), json!({"fst_wellen": t_wl, "vtr": t_vtr, "vars": n_vtr.0}));

    // ---- load N signals (fresh instance each time, like a one-shot CLI) ----
    for &k in &[1usize, 10, 100, 1000] {
        let ids: Vec<u32> = plan.sigs[..k.min(plan.sigs.len())].to_vec();
        let refs: Vec<SignalRef> = ids.iter().map(|&i| SignalRef::from_index(i as usize).unwrap()).collect();
        let vids: Vec<SignalId> = ids.iter().map(|&i| SignalId(i)).collect();
        let (t_wl, changes_wl) = best_of(3, || {
            let mut w = wellen::simple::read(fst_path).unwrap();
            w.load_signals(&refs);
            refs.iter().map(|r| w.get_signal(*r).unwrap().time_indices().len()).sum::<usize>()
        });
        let (t_vtr, changes_vtr) = best_of(3, || {
            let r = Reader::open(vtr_path).unwrap();
            r.load_signals(&vids).unwrap().iter().map(|d| d.len()).sum::<usize>()
        });
        // Parity: values of the first signal's first 50 changes.
        {
            let mut w = wellen::simple::read(fst_path).unwrap();
            w.load_signals(&refs[..1]);
            let s = w.get_signal(refs[0]).unwrap();
            let d = vtr.load_signal(vids[0]).unwrap();
            let tt = w.time_table();
            for (i, (tidx, v)) in s.iter_changes().take(50).enumerate() {
                let a = v.to_bit_string().unwrap_or_default();
                if i < d.len() {
                    let b = d.get(i).to_ascii();
                    let tv = d.times()[i];
                    if (a != b && !a.is_empty()) || tt[tidx as usize] != tv {
                        parity_errors += 1;
                        if parity_errors < 5 {
                            eprintln!("parity: sig {} change {i}: wellen {}@{} vs vtr {}@{}", ids[0], a, tt[tidx as usize], b, tv);
                        }
                    }
                }
            }
        }
        out.insert(format!("load_{k}"), json!({"fst_wellen": t_wl, "vtr": t_vtr, "changes": changes_vtr, "changes_wellen": changes_wl}));
    }

    // ---- value at time: 10 signals x 100 times, one-shot process model ----
    {
        let ids: Vec<u32> = plan.sigs[..10].to_vec();
        let refs: Vec<SignalRef> = ids.iter().map(|&i| SignalRef::from_index(i as usize).unwrap()).collect();
        let (t_wl, vals_wl) = best_of(3, || {
            let mut w = wellen::simple::read(fst_path).unwrap();
            w.load_signals(&refs);
            let tt = w.time_table();
            let mut vals = Vec::new();
            for &t in &plan.times {
                let idx = tt.partition_point(|&x| x <= t);
                for r in &refs {
                    let s = w.get_signal(*r).unwrap();
                    let v = if idx == 0 { None } else { s.get_offset((idx - 1) as u32) };
                    vals.push(v.map(|o| s.get_value_at(&o, o.elements - 1).to_bit_string().unwrap_or_default()).unwrap_or_default());
                }
            }
            vals
        });
        let (t_vtr, vals_vtr) = best_of(3, || {
            let r = Reader::open(vtr_path).unwrap();
            let mut vals = Vec::new();
            for &t in &plan.times {
                for &s in &ids {
                    vals.push(r.value_at(SignalId(s), t).unwrap().to_ascii());
                }
            }
            vals
        });
        // Warm (reader already open) random access cost per query.
        let t = Instant::now();
        for &tq in &plan.times {
            for &s in &ids {
                std::hint::black_box(vtr.value_at(SignalId(s), tq).unwrap());
            }
        }
        let warm = t.elapsed().as_secs_f64();
        for (a, b) in vals_wl.iter().zip(vals_vtr.iter()) {
            if a != b && !a.is_empty() {
                parity_errors += 1;
                if parity_errors < 5 {
                    eprintln!("parity: value_at wellen {a} vs vtr {b}");
                }
            }
        }
        out.insert("value_at_10x100".into(), json!({"fst_wellen": t_wl, "vtr": t_vtr, "vtr_warm": warm, "queries": vals_vtr.len()}));
    }

    // ---- changes in a 1% window for one signal ----
    {
        let s = plan.scan_sig;
        let r = SignalRef::from_index(s as usize).unwrap();
        let (a, b) = plan.window;
        let (t_wl, n_wl) = best_of(3, || {
            let mut w = wellen::simple::read(fst_path).unwrap();
            w.load_signals(&[r]);
            let tt = w.time_table();
            let sg = w.get_signal(r).unwrap();
            sg.time_indices().iter().filter(|&&i| tt[i as usize] >= a && tt[i as usize] <= b).count()
        });
        let (t_vtr, n_vtr) = best_of(3, || {
            let rd = Reader::open(vtr_path).unwrap();
            rd.changes(SignalId(s), a, b).unwrap().len()
        });
        if n_wl != n_vtr {
            parity_errors += 1;
            eprintln!("parity: window change count {n_wl} vs {n_vtr}");
        }
        out.insert("changes_window_1pct".into(), json!({"fst_wellen": t_wl, "vtr": t_vtr, "changes": n_vtr}));
    }

    // ---- condition search: posedge clk && bus == value of bus at the middle of the run ----
    {
        let (clk, bus) = (plan.clk, plan.bus);
        let rc = SignalRef::from_index(clk as usize).unwrap();
        let rb = SignalRef::from_index(bus as usize).unwrap();
        let mid = plan.window.0;
        let target = vtr.value_at(SignalId(bus), mid).unwrap().to_ascii();
        let (t_wl, n_wl) = best_of(3, || {
            let mut w = wellen::simple::read(fst_path).unwrap();
            w.load_signals(&[rc, rb]);
            let sc = w.get_signal(rc).unwrap();
            let sb = w.get_signal(rb).unwrap();
            let mut hits = 0usize;
            for (tidx, v) in sc.iter_changes() {
                if v.to_bit_string().as_deref() == Some("1") {
                    if let Some(o) = sb.get_offset(tidx) {
                        if sb.get_value_at(&o, o.elements - 1).to_bit_string().as_deref() == Some(target.as_str()) {
                            hits += 1;
                        }
                    }
                }
            }
            hits
        });
        let (t_vtr, n_vtr) = best_of(3, || {
            let rd = Reader::open(vtr_path).unwrap();
            let d = rd.load_signals(&[SignalId(clk), SignalId(bus)]).unwrap();
            let (dc, db) = (&d[0], &d[1]);
            let mut hits = 0usize;
            for i in 0..dc.len() {
                if dc.get(i).as_u64() == Some(1) {
                    let t = dc.times()[i];
                    if db.value_at(t).to_ascii() == target {
                        hits += 1;
                    }
                }
            }
            hits
        });
        if n_wl != n_vtr {
            parity_errors += 1;
            eprintln!("parity: condition hits {n_wl} vs {n_vtr}");
        }
        out.insert("condition_search".into(), json!({"fst_wellen": t_wl, "vtr": t_vtr, "hits": n_vtr}));
    }

    // ---- full streaming read of every change ----
    {
        let (t_fst, n_fst) = best_of(2, || {
            let f = std::fs::File::open(fst_path).unwrap();
            let mut rd = fst_reader::FstReader::open(std::io::BufReader::with_capacity(1 << 20, f)).unwrap();
            let mut n = 0u64;
            rd.read_signals(&fst_reader::FstFilter::all(), |_t, _h, _v| -> Result<(), ()> {
                n += 1;
                Ok(())
            })
            .unwrap();
            n
        });
        let (t_vtr, n_vtr) = best_of(2, || {
            let rd = Reader::open(vtr_path).unwrap();
            let mut n = 0u64;
            rd.for_each_change(0, u64::MAX, |_, _, _| n += 1).unwrap();
            n
        });
        out.insert("stream_all".into(), json!({"fst_reader": t_fst, "vtr": t_vtr, "changes_fst": n_fst, "changes_vtr": n_vtr}));
    }
    out.insert("parity_errors".into(), json!(parity_errors));
    out.insert("signals".into(), json!(vtr.signal_count()));
    serde_json::Value::Object(out)
}
