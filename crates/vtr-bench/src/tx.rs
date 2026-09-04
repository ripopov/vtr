//! Transaction workloads: a synthetic out-of-order pipeline trace in Kanata
//! format, a synthetic TLM (CPU -> NoC -> slave) transaction replay, and the
//! VTR-side timing drivers.

use crate::replay::{text_lines, Rng};
use crate::util::{file_size, Stopwatch};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use vtr::{AttrPhase, NodeId, Reader, StrId, TxId, TxQuery, TxStatus, Value, Writer, WriterOptions};

// ---------------------------------------------------------------------------
// Kanata generator
// ---------------------------------------------------------------------------

const MNEMONICS: [&str; 12] = ["add", "sub", "and", "or", "xor", "lw", "sw", "beq", "bne", "jal", "mul", "addi"];

/// Writes an `n`-instruction Kanata log modelling a 4-wide out-of-order core:
/// stages F/Dc/Rn/Ds/Is/X/Cm/Rt on lane 0, issue-queue stalls on lane 1,
/// 1-2 wakeup dependencies per op, ~14% branches with 10% mispredict flushes.
pub fn gen_kanata(path: &str, n: u64, seed: u64) -> std::io::Result<()> {
    let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    writeln!(out, "Kanata\t0004")?;
    writeln!(out, "C=\t0")?;
    let mut rng = Rng(seed);
    // Pending events keyed by (cycle, seq).
    let mut events: BTreeMap<(u64, u64), String> = BTreeMap::new();
    let mut seq = 0u64;
    let mut push = |events: &mut BTreeMap<(u64, u64), String>, cycle: u64, s: String| {
        seq += 1;
        events.insert((cycle, seq), s);
    };
    let mut emitted_cycle = 0u64;
    let mut fetch_cycle = 0u64;
    let mut fetched_this_cycle = 0;
    let mut pc = 0x8000_0000u64;
    let mut retire_id = 0u64;
    let mut last_retire_cycle = 0u64;
    let mut flush_until = 0u64; // instructions with id < flush_until are flushed
    let mut recent: Vec<(u64, u64)> = Vec::new(); // (id, complete cycle) of recent producers
    for id in 0..n {
        if fetched_this_cycle == 4 {
            fetch_cycle += 1;
            fetched_this_cycle = 0;
        }
        fetched_this_cycle += 1;
        // Flush pending events that can no longer be preceded.
        while let Some((&(c, s), _)) = events.iter().next() {
            if c >= fetch_cycle {
                break;
            }
            let text = events.remove(&(c, s)).unwrap();
            if c > emitted_cycle {
                writeln!(out, "C\t{}", c - emitted_cycle)?;
                emitted_cycle = c;
            }
            out.write_all(text.as_bytes())?;
        }
        let m = MNEMONICS[rng.below(12) as usize];
        let is_branch = matches!(m, "beq" | "bne" | "jal");
        let is_load = m == "lw";
        let (rd, rs1, rs2) = (rng.below(32), rng.below(32), rng.below(32));
        let gid = id * 4 + 1;
        push(&mut events, fetch_cycle, format!("I\t{id}\t{gid}\t0\n"));
        push(&mut events, fetch_cycle, format!("L\t{id}\t0\t{pc:08x}: {m} x{rd}, x{rs1}, x{rs2}\n"));
        push(&mut events, fetch_cycle, format!("L\t{id}\t1\t(g:{gid}, c0)\\nrs1=x{rs1} rs2=x{rs2}\n"));
        pc += 4;
        let f_len = 1 + rng.below(2);
        let mut c = fetch_cycle;
        push(&mut events, c, format!("S\t{id}\t0\tF\n"));
        c += f_len;
        push(&mut events, c, format!("S\t{id}\t0\tDc\n"));
        c += 1;
        push(&mut events, c, format!("S\t{id}\t0\tRn\n"));
        c += 1;
        push(&mut events, c, format!("S\t{id}\t0\tDs\n"));
        c += 1;
        // Dependencies on recent producers.
        let ndeps = rng.below(3) as usize;
        let mut ready = c;
        for _ in 0..ndeps {
            if !recent.is_empty() {
                let (pid, pcomp) = recent[rng.below(recent.len() as u64) as usize];
                push(&mut events, c, format!("W\t{id}\t{pid}\t0\n"));
                ready = ready.max(pcomp);
            }
        }
        push(&mut events, c, format!("S\t{id}\t0\tIs\n"));
        if ready > c + 1 {
            push(&mut events, c, format!("S\t{id}\t1\tstl\n"));
            push(&mut events, ready, format!("E\t{id}\t1\tstl\n"));
        }
        c = ready.max(c + 1);
        push(&mut events, c, format!("S\t{id}\t0\tX\n"));
        let x_len = if is_load && rng.below(10) == 0 { 20 } else if is_load { 3 } else if m == "mul" { 4 } else { 1 };
        c += x_len;
        push(&mut events, c, format!("L\t{id}\t2\tlat={x_len}\n"));
        push(&mut events, c, format!("S\t{id}\t0\tCm\n"));
        let complete = c;
        c += 1;
        let flushed = id < flush_until;
        if flushed {
            push(&mut events, c, format!("R\t{id}\t{retire_id}\t1\n"));
        } else {
            let rt = c.max(last_retire_cycle);
            push(&mut events, rt, format!("S\t{id}\t0\tRt\n"));
            push(&mut events, rt + 1, format!("R\t{id}\t{retire_id}\t0\n"));
            last_retire_cycle = rt;
            retire_id += 1;
            if is_branch && rng.below(10) == 0 {
                flush_until = id + 8 + rng.below(24);
            }
        }
        recent.push((id, complete));
        if recent.len() > 24 {
            recent.remove(0);
        }
    }
    for ((c, _), text) in events {
        if c > emitted_cycle {
            writeln!(out, "C\t{}", c - emitted_cycle)?;
            emitted_cycle = c;
        }
        out.write_all(text.as_bytes())?;
    }
    out.flush()
}

// ---------------------------------------------------------------------------
// TLM transaction replay
// ---------------------------------------------------------------------------

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(b: &[u8], p: &mut usize) -> u64 {
    let mut r = 0u64;
    let mut s = 0;
    loop {
        let x = b[*p];
        *p += 1;
        r |= ((x & 0x7f) as u64) << s;
        if x < 0x80 {
            return r;
        }
        s += 7;
    }
}

#[derive(Clone, Debug)]
pub enum TxOp {
    Begin { id: u64, gen: u32, time: u64 },
    /// type: 0 bool 1 i64 2 u64 3 f64 4 str
    Attr { id: u64, key: u32, phase: u8, ty: u8, v: u64 },
    End { id: u64, time: u64 },
    Rel { kind: u32, from: u64, to: u64 },
}

pub struct TxReplay {
    pub strings: Vec<String>,
    pub streams: Vec<u32>,
    pub gens: Vec<(u32, u32)>,
    pub ops: Vec<TxOp>,
}

impl TxReplay {
    pub fn save(&self, path: &str) -> std::io::Result<()> {
        let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        w.write_all(b"VTRTXR01")?;
        let mut b = Vec::new();
        put_varint(&mut b, self.strings.len() as u64);
        for s in &self.strings {
            put_varint(&mut b, s.len() as u64);
            b.extend_from_slice(s.as_bytes());
        }
        put_varint(&mut b, self.streams.len() as u64);
        for s in &self.streams {
            put_varint(&mut b, *s as u64);
        }
        put_varint(&mut b, self.gens.len() as u64);
        for (st, n) in &self.gens {
            put_varint(&mut b, *st as u64);
            put_varint(&mut b, *n as u64);
        }
        put_varint(&mut b, self.ops.len() as u64);
        w.write_all(&b)?;
        b.clear();
        for op in &self.ops {
            match op {
                TxOp::Begin { id, gen, time } => {
                    b.push(1);
                    put_varint(&mut b, *id);
                    put_varint(&mut b, *gen as u64);
                    put_varint(&mut b, *time);
                }
                TxOp::Attr { id, key, phase, ty, v } => {
                    b.push(2);
                    put_varint(&mut b, *id);
                    put_varint(&mut b, *key as u64);
                    b.push(*phase);
                    b.push(*ty);
                    if *ty == 3 {
                        b.extend_from_slice(&v.to_le_bytes());
                    } else {
                        put_varint(&mut b, *v);
                    }
                }
                TxOp::End { id, time } => {
                    b.push(3);
                    put_varint(&mut b, *id);
                    put_varint(&mut b, *time);
                }
                TxOp::Rel { kind, from, to } => {
                    b.push(4);
                    put_varint(&mut b, *kind as u64);
                    put_varint(&mut b, *from);
                    put_varint(&mut b, *to);
                }
            }
            if b.len() > (1 << 20) {
                w.write_all(&b)?;
                b.clear();
            }
        }
        w.write_all(&b)?;
        w.flush()
    }

    pub fn load(path: &str) -> std::io::Result<TxReplay> {
        let data = std::fs::read(path)?;
        if &data[..8] != b"VTRTXR01" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a tx replay"));
        }
        let mut p = 8;
        let ns = get_varint(&data, &mut p) as usize;
        let mut strings = Vec::with_capacity(ns);
        for _ in 0..ns {
            let l = get_varint(&data, &mut p) as usize;
            strings.push(String::from_utf8_lossy(&data[p..p + l]).into_owned());
            p += l;
        }
        let nst = get_varint(&data, &mut p) as usize;
        let streams = (0..nst).map(|_| get_varint(&data, &mut p) as u32).collect();
        let ng = get_varint(&data, &mut p) as usize;
        let mut gens = Vec::with_capacity(ng);
        for _ in 0..ng {
            let st = get_varint(&data, &mut p) as u32;
            let n = get_varint(&data, &mut p) as u32;
            gens.push((st, n));
        }
        let nops = get_varint(&data, &mut p) as usize;
        let mut ops = Vec::with_capacity(nops);
        for _ in 0..nops {
            let op = data[p];
            p += 1;
            ops.push(match op {
                1 => {
                    let id = get_varint(&data, &mut p);
                    let gen = get_varint(&data, &mut p) as u32;
                    let time = get_varint(&data, &mut p);
                    TxOp::Begin { id, gen, time }
                }
                2 => {
                    let id = get_varint(&data, &mut p);
                    let key = get_varint(&data, &mut p) as u32;
                    let phase = data[p];
                    let ty = data[p + 1];
                    p += 2;
                    let v = if ty == 3 {
                        let v = u64::from_le_bytes(data[p..p + 8].try_into().unwrap());
                        p += 8;
                        v
                    } else {
                        get_varint(&data, &mut p)
                    };
                    TxOp::Attr { id, key, phase, ty, v }
                }
                3 => {
                    let id = get_varint(&data, &mut p);
                    let time = get_varint(&data, &mut p);
                    TxOp::End { id, time }
                }
                _ => {
                    let kind = get_varint(&data, &mut p) as u32;
                    let from = get_varint(&data, &mut p);
                    let to = get_varint(&data, &mut p);
                    TxOp::Rel { kind, from, to }
                }
            });
        }
        Ok(TxReplay { strings, streams, gens, ops })
    }
}

/// Generates a CPU -> NoC -> slave transaction workload with `n` top-level
/// instructions (3n transactions, ~2.3n relations, ~13n attributes).
pub fn gen_tlm(n: u64, seed: u64) -> TxReplay {
    let mut rng = Rng(seed);
    let mut strings: Vec<String> = Vec::new();
    let intern = |s: &str, strings: &mut Vec<String>| -> u32 {
        if let Some(i) = strings.iter().position(|x| x == s) {
            return i as u32;
        }
        strings.push(s.to_string());
        (strings.len() - 1) as u32
    };
    let n_cpu = 4u64;
    let n_slave = 4u64;
    let mut streams = Vec::new();
    let mut gens = Vec::new();
    for c in 0..n_cpu {
        let s = intern(&format!("top.cpu{c}"), &mut strings);
        streams.push(s);
        let sidx = (streams.len() - 1) as u32;
        gens.push((sidx, intern("load", &mut strings)));
        gens.push((sidx, intern("store", &mut strings)));
    }
    let noc = intern("top.noc", &mut strings);
    streams.push(noc);
    let noc_idx = (streams.len() - 1) as u32;
    gens.push((noc_idx, intern("packet", &mut strings)));
    let g_noc = (gens.len() - 1) as u32;
    let mut g_slave = Vec::new();
    for s in 0..n_slave {
        let st = intern(&format!("top.slave{s}"), &mut strings);
        streams.push(st);
        let sidx = (streams.len() - 1) as u32;
        gens.push((sidx, intern("access", &mut strings)));
        g_slave.push((gens.len() - 1) as u32);
    }
    let k_pc = intern("pc", &mut strings);
    let k_addr = intern("addr", &mut strings);
    let k_size = intern("size", &mut strings);
    let k_data = intern("data", &mut strings);
    let k_resp = intern("response", &mut strings);
    let k_src = intern("src", &mut strings);
    let k_dst = intern("dst", &mut strings);
    let k_lat = intern("latency_ns", &mut strings);
    let k_err = intern("error", &mut strings);
    let k_len = intern("len", &mut strings);
    let s_ok = intern("OK", &mut strings);
    let s_err = intern("ERROR", &mut strings);
    let s_exok = intern("EXOKAY", &mut strings);
    let r_parent = intern("parent_of", &mut strings);
    let r_pred = intern("pred", &mut strings);
    // Generate events with (time, seq) keys, then sort.
    let mut ev: Vec<(u64, u64, TxOp)> = Vec::with_capacity(n as usize * 20);
    let mut seq = 0u64;
    let mut push = |ev: &mut Vec<(u64, u64, TxOp)>, t: u64, op: TxOp| {
        seq += 1;
        ev.push((t, seq, op));
    };
    let mut next_id = 1u64;
    let mut cpu_time = vec![0u64; n_cpu as usize];
    let mut cpu_last = vec![0u64; n_cpu as usize];
    let mut pcs = vec![0x1000u64; n_cpu as usize];
    for i in 0..n {
        let c = (i % n_cpu) as usize;
        let t = cpu_time[c];
        let is_store = rng.below(3) == 0;
        let gen = c as u32 * 2 + is_store as u32;
        let id = next_id;
        next_id += 1;
        let addr = 0x8000_0000 + rng.below(1 << 20) * 64;
        let size = [4u64, 8, 16, 64][rng.below(4) as usize];
        push(&mut ev, t, TxOp::Begin { id, gen, time: t });
        push(&mut ev, t, TxOp::Attr { id, key: k_pc, phase: 0, ty: 2, v: pcs[c] });
        push(&mut ev, t, TxOp::Attr { id, key: k_addr, phase: 0, ty: 2, v: addr });
        push(&mut ev, t, TxOp::Attr { id, key: k_size, phase: 0, ty: 2, v: size });
        pcs[c] += 4;
        if cpu_last[c] != 0 {
            push(&mut ev, t, TxOp::Rel { kind: r_pred, from: cpu_last[c], to: id });
        }
        cpu_last[c] = id;
        // NoC packet
        let pid = next_id;
        next_id += 1;
        let tn = t + 2;
        let slave = (addr >> 18) as u64 % n_slave;
        push(&mut ev, tn, TxOp::Begin { id: pid, gen: g_noc, time: tn });
        push(&mut ev, tn, TxOp::Attr { id: pid, key: k_src, phase: 0, ty: 2, v: c as u64 });
        push(&mut ev, tn, TxOp::Attr { id: pid, key: k_dst, phase: 0, ty: 2, v: slave });
        push(&mut ev, tn, TxOp::Attr { id: pid, key: k_len, phase: 0, ty: 2, v: size });
        push(&mut ev, tn, TxOp::Rel { kind: r_parent, from: id, to: pid });
        // Slave access
        let sid = next_id;
        next_id += 1;
        let ts = tn + 3 + rng.below(3);
        let lat = 3 + rng.below(28);
        push(&mut ev, ts, TxOp::Begin { id: sid, gen: g_slave[slave as usize], time: ts });
        push(&mut ev, ts, TxOp::Attr { id: sid, key: k_addr, phase: 0, ty: 2, v: addr });
        push(&mut ev, ts, TxOp::Attr { id: sid, key: k_len, phase: 0, ty: 2, v: size });
        push(&mut ev, ts, TxOp::Rel { kind: r_parent, from: pid, to: sid });
        let te = ts + lat;
        let data = rng.next();
        let err = rng.below(200) == 0;
        push(&mut ev, te, TxOp::Attr { id: sid, key: k_data, phase: 2, ty: 2, v: data });
        push(&mut ev, te, TxOp::Attr { id: sid, key: k_lat, phase: 2, ty: 3, v: (lat as f64 * 0.5).to_bits() });
        push(&mut ev, te, TxOp::End { id: sid, time: te });
        let tne = te + 2;
        push(&mut ev, tne, TxOp::Attr { id: pid, key: k_resp, phase: 2, ty: 4, v: if err { s_err } else { s_exok } as u64 });
        push(&mut ev, tne, TxOp::End { id: pid, time: tne });
        let tie = tne + 1;
        push(&mut ev, tie, TxOp::Attr { id, key: k_data, phase: 2, ty: 2, v: data });
        push(&mut ev, tie, TxOp::Attr { id, key: k_resp, phase: 2, ty: 4, v: if err { s_err } else { s_ok } as u64 });
        push(&mut ev, tie, TxOp::Attr { id, key: k_err, phase: 1, ty: 0, v: err as u64 });
        push(&mut ev, tie, TxOp::End { id, time: tie });
        cpu_time[c] = t + 1 + rng.below(10);
    }
    ev.sort_by_key(|(t, s, _)| (*t, *s));
    TxReplay { strings, streams, gens, ops: ev.into_iter().map(|(_, _, op)| op).collect() }
}

/// Times the VTR writer on a transaction replay.
pub fn tx_write_vtr(rp: &TxReplay, out: &str, opts: WriterOptions, label: &str) -> serde_json::Value {
    let sw = Stopwatch::start();
    let mut w = Writer::create_with(out, opts).expect("create");
    w.set_timescale(-9).unwrap();
    let strs: Vec<StrId> = rp.strings.iter().map(|s| w.intern(s)).collect();
    let streams: Vec<NodeId> = rp.streams.iter().map(|&s| w.add_stream(None, &rp.strings[s as usize], "TRANSACTOR")).collect();
    let gens: Vec<NodeId> = rp.gens.iter().map(|&(st, n)| w.add_generator(streams[st as usize], &rp.strings[n as usize])).collect();
    let max_id = rp.ops.iter().filter_map(|o| if let TxOp::Begin { id, .. } = o { Some(*id) } else { None }).max().unwrap_or(0);
    let mut ids: Vec<TxId> = vec![0; max_id as usize + 1];
    let (mut ntx, mut nattr, mut nrel) = (0u64, 0u64, 0u64);
    for op in &rp.ops {
        match op {
            TxOp::Begin { id, gen, time } => {
                ids[*id as usize] = w.begin_tx(gens[*gen as usize], *time).unwrap();
                ntx += 1;
            }
            TxOp::Attr { id, key, phase, ty, v } => {
                let val = match ty {
                    0 => Value::Bool(*v != 0),
                    1 => Value::I64(*v as i64),
                    2 => Value::U64(*v),
                    3 => Value::F64(f64::from_bits(*v)),
                    _ => Value::Str(strs[*v as usize]),
                };
                w.tx_attr(ids[*id as usize], strs[*key as usize], AttrPhase::from_u8(*phase), &val).unwrap();
                nattr += 1;
            }
            TxOp::End { id, time } => w.end_tx(ids[*id as usize], *time, TxStatus::Unset).unwrap(),
            TxOp::Rel { kind, from, to } => {
                w.relate(strs[*kind as usize], ids[*from as usize], ids[*to as usize], &[]).unwrap();
                nrel += 1;
            }
        }
    }
    w.close().unwrap();
    let (wall, cpu) = sw.stop();
    let size = file_size(out);
    eprintln!("{label}: {ntx} tx, {nattr} attrs, {nrel} rel in {wall:.3}s wall / {cpu:.3}s cpu -> {size} bytes");
    json!({"writer": label, "transactions": ntx, "attributes": nattr, "relations": nrel, "wall_s": wall, "cpu_s": cpu, "bytes": size})
}

/// Times the Kanata -> VTR conversion (parsing included, as in the FTR harness).
pub fn kanata_write_vtr(log: &str, out: &str, opts: WriterOptions) -> serde_json::Value {
    let sw = Stopwatch::start();
    let mut w = Writer::create_with(out, opts).expect("create");
    vtr_cli::kanata::convert_kanata(log, &mut w).expect("convert");
    let stats = w.stats();
    w.close().unwrap();
    let (wall, cpu) = sw.stop();
    let size = file_size(out);
    // Count stages for the record.
    let lines = text_lines(log).map(|it| it.filter(|l| l.starts_with("S\t")).count()).unwrap_or(0);
    eprintln!("vtr(kanata): {} tx, {lines} stages in {wall:.3}s wall / {cpu:.3}s cpu -> {size} bytes", stats.transactions);
    json!({"writer": "vtr", "transactions": stats.transactions, "stages": lines, "wall_s": wall, "cpu_s": cpu, "bytes": size})
}

/// Transaction navigation benchmark on a VTR file.
pub fn tx_read(path: &str, seed: u64) -> serde_json::Value {
    let mut rng = Rng(seed);
    let (t_open, r) = crate::util::best_of(3, || Reader::open(path).unwrap());
    let (ntx, nrel) = r.tx_counts();
    let t = std::time::Instant::now();
    let mut n = 0u64;
    r.visit_transactions(&TxQuery::default(), |_| {
        n += 1;
        true
    })
    .unwrap();
    let t_scan = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let mut found = 0;
    for _ in 0..1000 {
        let id = 1 + rng.below(ntx.max(1));
        if r.transaction(id).unwrap().is_some() {
            found += 1;
        }
    }
    let t_lookup = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let mut rels = 0usize;
    for _ in 0..1000 {
        let id = 1 + rng.below(ntx.max(1));
        rels += r.relations_from(id).unwrap().len() + r.relations_to(id).unwrap().len();
    }
    let t_rel = t.elapsed().as_secs_f64();
    let (a, b) = r.time_range().unwrap_or((0, 1));
    let mid = a + (b - a) / 2;
    let t = std::time::Instant::now();
    let win = r.transactions(&TxQuery { window: Some((mid, mid + (b - a) / 100)), ..Default::default() }).unwrap().len();
    let t_win = t.elapsed().as_secs_f64();
    json!({"open_s": t_open, "transactions": ntx, "relations": nrel, "scan_all_s": t_scan, "scanned": n, "lookup_1000_s": t_lookup, "found": found, "relations_1000_s": t_rel, "relations_found": rels, "window_1pct_s": t_win, "window_tx": win})
}
