//! VTR write benchmark: replays a workload into the Rust writer.

use crate::replay::{HierOp, Replay};
use crate::util::{file_size, Stopwatch};
use serde_json::json;
use vtr::{Direction, ScopeType, SignalId, SignalKind, VarType, Writer, WriterOptions};

pub fn declare(w: &mut Writer, rp: &Replay) -> Vec<SignalId> {
    let mut map: Vec<Option<SignalId>> = vec![None; rp.signals.len()];
    for op in &rp.hier {
        match op {
            HierOp::Scope(n) => {
                w.begin_scope(n, ScopeType::Module, "");
            }
            HierOp::Up => w.end_scope().unwrap(),
            HierOp::Var(s, n) => {
                let d = rp.signals[*s as usize];
                let kind = match d.kind {
                    0 => SignalKind::Bits { width: d.width, states: d.states },
                    1 => SignalKind::Real,
                    _ => SignalKind::VarLen,
                };
                let (_, sig) = w.add_var(n, VarType::Wire, Direction::Implicit, kind);
                map[*s as usize] = Some(sig);
            }
            HierOp::Alias(s, n) => {
                w.add_alias(n, VarType::Wire, Direction::Implicit, map[*s as usize].unwrap()).unwrap();
            }
        }
    }
    // Signals never mentioned in the hierarchy (synthetic) get declared at top level.
    for (i, m) in map.iter_mut().enumerate() {
        if m.is_none() {
            let d = rp.signals[i];
            let kind = match d.kind {
                0 => SignalKind::Bits { width: d.width, states: d.states },
                1 => SignalKind::Real,
                _ => SignalKind::VarLen,
            };
            *m = Some(w.add_var(&format!("s{i}"), VarType::Wire, Direction::Implicit, kind).1);
        }
    }
    map.into_iter().map(|m| m.unwrap()).collect()
}

/// Replays `rp` into `w` (timed section). Returns the number of records.
pub fn replay_into(w: &mut Writer, rp: &Replay, map: &[SignalId]) -> vtr::Result<u64> {
    let mut last_time = u64::MAX;
    for r in &rp.recs {
        if r.time != last_time {
            w.set_time(r.time)?;
            last_time = r.time;
        }
        let sig = map[r.sig as usize];
        let p = rp.payload(r);
        match r.form {
            0 => {
                let w_bits = rp.signals[r.sig as usize].width;
                if w_bits <= 64 {
                    let mut b = [0u8; 8];
                    b[..p.len()].copy_from_slice(p);
                    w.emit_u64(sig, u64::from_le_bytes(b))?;
                } else {
                    w.emit_packed(sig, 2, p)?;
                }
            }
            1 => w.emit_logic_str(sig, p)?,
            2 => w.emit_real(sig, f64::from_le_bytes(p.try_into().unwrap()))?,
            _ => w.emit_varlen(sig, p)?,
        }
    }
    w.set_time(rp.end_time.max(last_time.min(rp.end_time)))?;
    Ok(rp.recs.len() as u64)
}

pub fn run(rp: &Replay, out: &str, opts: WriterOptions, label: &str) -> serde_json::Value {
    let sw = Stopwatch::start();
    let mut w = Writer::create_with(out, opts.clone()).expect("create");
    w.set_timescale(rp.timescale).unwrap();
    let map = declare(&mut w, rp);
    let n = replay_into(&mut w, rp, &map).expect("replay");
    let stats = w.stats();
    w.close().expect("close");
    let (wall, cpu) = sw.stop();
    let size = file_size(out);
    eprintln!("{label}: {n} changes in {wall:.3}s wall / {cpu:.3}s cpu -> {size} bytes ({:.1} Mchg/s)", n as f64 / wall / 1e6);
    json!({
        "writer": label,
        "records": n,
        "wall_s": wall,
        "cpu_s": cpu,
        "bytes": size,
        "blocks": stats.blocks,
        "changes_per_s": n as f64 / wall,
        "codec": format!("{:?}", opts.compression.codec).to_lowercase(),
        "level": opts.compression.level,
        "background": opts.background,
    })
}
