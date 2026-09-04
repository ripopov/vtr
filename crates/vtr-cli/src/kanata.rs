//! Kanata (Konata pipeline log) -> VTR conversion.
//!
//! Mapping (see docs/KDB_APPNOTE.md for the viewer side):
//! * one stream per thread id, generator "instruction"
//! * `I`  -> begin transaction; attrs `insn_id_in_sim` (begin phase), `line`
//! * `L 0/1` -> attrs `label` / `detail` (record phase, appended)
//! * `L 2` -> attr `label` on the most recently started stage
//! * `S`/`E` -> stages on lane `lane`
//! * `R`  -> end; attrs `retire_id`; status Aborted for flush
//! * `W`  -> relation `wakeup` (attr `type` when non-zero) from producer to consumer
//! * `C=` -> file attr `kanata.start_cycle`; cycles map 1:1 to time units.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use vtr::{AttrPhase, FileType, NodeId, ScopeType, StrId, TxId, TxStatus, Value, Writer};

struct Op {
    tx: TxId,
    label: String,
    detail: String,
    retired: bool,
    retire_cycle: u64,
    status: TxStatus,
}

struct Thread {
    #[allow(dead_code)]
    stream: NodeId,
    gen: NodeId,
}

/// Number of retired instructions kept open so that late `L` lines can still attach.
const RETIRE_LAG: usize = 4096;

pub fn convert_kanata(input: &str, w: &mut Writer) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(input)?;
    let reader: Box<dyn Read> = if input.ends_with(".gz") { Box::new(flate2::read::GzDecoder::new(file)) } else { Box::new(file) };
    let mut lines = BufReader::with_capacity(1 << 20, reader).lines();
    let first = lines.next().ok_or("empty file")??;
    if !first.starts_with("Kanata") {
        return Err("not a Kanata log (missing header)".into());
    }
    w.set_timescale(0)?;
    w.set_file_type(FileType::Architectural)?;
    let unit = w.intern("cycle");
    w.set_file_attr("time.unit", Value::Str(unit))?;
    let ver = first.split('\t').nth(1).unwrap_or("").trim().to_string();
    let vs = w.intern(&ver);
    w.set_file_attr("kanata.version", Value::Str(vs))?;
    let core = w.begin_scope("cpu", ScopeType::Core, "");
    w.end_scope()?;

    let k_gid = w.intern("insn_id_in_sim");
    let k_line = w.intern("line");
    let k_label = w.intern("label");
    let k_detail = w.intern("detail");
    let k_rid = w.intern("retire_id");
    let k_type = w.intern("type");
    let k_wakeup = w.intern("wakeup");
    let k_lane = w.intern("lane");
    let mut threads: HashMap<u64, Thread> = HashMap::new();
    let mut ops: HashMap<u64, Op> = HashMap::new();
    let mut retired: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
    let mut lanes: HashMap<String, StrId> = HashMap::new();
    let mut stages: HashMap<String, StrId> = HashMap::new();
    let mut cycle: i64 = 0;
    let mut start_set = false;
    let mut line_no: u64 = 1;
    let mut warnings = 0u64;

    let flush_retired = |w: &mut Writer, ops: &mut HashMap<u64, Op>, id: u64| -> vtr::Result<()> {
        if let Some(op) = ops.remove(&id) {
            if !op.label.is_empty() {
                let s = w.intern(&op.label);
                w.tx_attr(op.tx, k_label, AttrPhase::Record, &Value::Str(s))?;
            }
            if !op.detail.is_empty() {
                let s = w.intern(&op.detail);
                w.tx_attr(op.tx, k_detail, AttrPhase::Record, &Value::Str(s))?;
            }
            w.end_tx(op.tx, op.retire_cycle, op.status)?;
        }
        Ok(())
    };

    for line in lines {
        let line = line?;
        line_no += 1;
        let mut it = line.split('\t');
        let cmd = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        let num = |i: usize| -> Result<i64, String> {
            args.get(i).ok_or_else(|| format!("line {line_no}: missing argument {i}"))?.trim().parse::<i64>().map_err(|e| format!("line {line_no}: {e}"))
        };
        match cmd {
            "C=" => {
                cycle = num(0)?;
                if !start_set {
                    w.set_file_attr("kanata.start_cycle", Value::I64(cycle))?;
                    start_set = true;
                }
            }
            "C" => cycle += num(0)?,
            "I" => {
                let id = num(0)? as u64;
                let gid = num(1)?;
                let tid = num(2)? as u64;
                let th = threads.entry(tid).or_insert_with(|| {
                    let stream = w.add_stream(Some(core), &format!("thread{tid}"), "PIPELINE");
                    let gen = w.add_generator(stream, "instruction");
                    Thread { stream, gen }
                });
                let tx = w.begin_tx(th.gen, cycle.max(0) as u64)?;
                w.tx_attr(tx, k_gid, AttrPhase::Begin, &Value::I64(gid))?;
                w.tx_attr(tx, k_line, AttrPhase::Begin, &Value::U64(line_no))?;
                ops.insert(id, Op { tx, label: String::new(), detail: String::new(), retired: false, retire_cycle: 0, status: TxStatus::Unset });
            }
            "L" => {
                let id = num(0)? as u64;
                let ty = num(1)?;
                let text = args.get(2).copied().unwrap_or("").replace("\\n", "\n");
                let text = text.as_str();
                if let Some(op) = ops.get_mut(&id) {
                    match ty {
                        0 => op.label.push_str(text),
                        1 => op.detail.push_str(text),
                        2 => {
                            let s = w.intern(text);
                            if w.tx_stage_attr(op.tx, k_label, &Value::Str(s)).is_err() {
                                op.detail.push_str(text);
                            }
                        }
                        _ => {}
                    }
                } else {
                    warnings += 1;
                }
            }
            "S" | "E" => {
                let id = num(0)? as u64;
                let lane_s = args.get(1).copied().unwrap_or("0").trim();
                let stage_s = args.get(2).copied().unwrap_or("").trim();
                let lane = *lanes.entry(lane_s.to_string()).or_insert_with(|| w.intern(lane_s));
                let stage = *stages.entry(stage_s.to_string()).or_insert_with(|| w.intern(stage_s));
                if let Some(op) = ops.get(&id) {
                    if op.retired {
                        warnings += 1;
                        continue;
                    }
                    let t = cycle.max(0) as u64;
                    if cmd == "S" {
                        w.tx_stage_begin(op.tx, stage, lane, t)?;
                    } else {
                        w.tx_stage_end(op.tx, stage, lane, t)?;
                    }
                } else {
                    warnings += 1;
                }
            }
            "R" => {
                let id = num(0)? as u64;
                let rid = num(1)?;
                let ty = num(2)?;
                if let Some(op) = ops.get_mut(&id) {
                    op.retired = true;
                    op.retire_cycle = cycle.max(0) as u64;
                    op.status = if ty == 1 { TxStatus::Aborted } else { TxStatus::Unset };
                    w.tx_attr(op.tx, k_rid, AttrPhase::End, &Value::I64(rid))?;
                    if ty != 0 && ty != 1 {
                        w.tx_attr(op.tx, k_type, AttrPhase::End, &Value::I64(ty))?;
                    }
                    retired.push_back(id);
                    if retired.len() > RETIRE_LAG {
                        let old = retired.pop_front().unwrap();
                        flush_retired(w, &mut ops, old)?;
                    }
                } else {
                    warnings += 1;
                }
            }
            "W" => {
                let consumer = num(0)? as u64;
                let producer = num(1)? as u64;
                let ty = num(2)?;
                match (ops.get(&consumer), ops.get(&producer)) {
                    (Some(c), Some(p)) => {
                        let attrs: Vec<(StrId, Value)> = if ty != 0 { vec![(k_type, Value::I64(ty)), (k_lane, Value::U64(cycle.max(0) as u64))] } else { vec![] };
                        w.relate(k_wakeup, p.tx, c.tx, &attrs)?;
                    }
                    _ => warnings += 1,
                }
            }
            "" => {}
            _ => warnings += 1,
        }
    }
    // Unretired ops stay open (status Open, ended at the last cycle).
    let ids: Vec<u64> = retired.drain(..).collect();
    for id in ids {
        flush_retired(w, &mut ops, id)?;
    }
    let rest: Vec<u64> = ops.keys().copied().collect();
    for id in rest {
        let op = ops.get_mut(&id).unwrap();
        if !op.label.is_empty() {
            let s = w.intern(&op.label);
            w.tx_attr(op.tx, k_label, AttrPhase::Record, &Value::Str(s))?;
        }
        if !op.detail.is_empty() {
            let s = w.intern(&op.detail);
            w.tx_attr(op.tx, k_detail, AttrPhase::Record, &Value::Str(s))?;
        }
        // Left open: the writer records status Open at close.
    }
    w.set_time(cycle.max(0) as u64)?;
    if warnings > 0 {
        eprintln!("kanata: {warnings} lines ignored (unknown command or op)");
    }
    Ok(())
}
