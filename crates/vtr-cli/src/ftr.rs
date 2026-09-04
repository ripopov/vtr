//! FTR (LWTR4SC Fast Transaction Recording, CBOR) -> VTR conversion.

use std::collections::HashMap;
use vtr::{AttrPhase, FileType, NodeId, StrId, TxId, TxStatus, Value, Writer};

#[derive(Debug, Clone)]
enum Cbor {
    U(u64),
    I(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Tag(u64, Box<Cbor>),
    Bool(bool),
    Null,
    F(f64),
    Break,
}

struct P<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> P<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.p).ok_or("truncated CBOR")?;
        self.p += 1;
        Ok(v)
    }
    fn arg(&mut self, ai: u8) -> Result<Option<u64>, String> {
        Ok(Some(match ai {
            0..=23 => ai as u64,
            24 => self.byte()? as u64,
            25 => {
                let b = self.take(2)?;
                u16::from_be_bytes([b[0], b[1]]) as u64
            }
            26 => {
                let b = self.take(4)?;
                u32::from_be_bytes(b.try_into().unwrap()) as u64
            }
            27 => {
                let b = self.take(8)?;
                u64::from_be_bytes(b.try_into().unwrap())
            }
            31 => return Ok(None),
            _ => return Err("reserved CBOR additional info".into()),
        }))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self.b.get(self.p..self.p + n).ok_or("truncated CBOR")?;
        self.p += n;
        Ok(s)
    }
    fn item(&mut self) -> Result<Cbor, String> {
        let ib = self.byte()?;
        let (mt, ai) = (ib >> 5, ib & 31);
        Ok(match mt {
            0 => Cbor::U(self.arg(ai)?.ok_or("bad uint")?),
            1 => Cbor::I(-1 - self.arg(ai)?.ok_or("bad nint")? as i64),
            2 | 3 => {
                let mut data = Vec::new();
                match self.arg(ai)? {
                    Some(n) => data.extend_from_slice(self.take(n as usize)?),
                    None => loop {
                        match self.item()? {
                            Cbor::Break => break,
                            Cbor::Bytes(b) => data.extend_from_slice(&b),
                            Cbor::Text(t) => data.extend_from_slice(t.as_bytes()),
                            _ => return Err("bad indefinite string".into()),
                        }
                    },
                }
                if mt == 2 {
                    Cbor::Bytes(data)
                } else {
                    Cbor::Text(String::from_utf8_lossy(&data).into_owned())
                }
            }
            4 => {
                let mut v = Vec::new();
                match self.arg(ai)? {
                    Some(n) => {
                        for _ in 0..n {
                            v.push(self.item()?);
                        }
                    }
                    None => loop {
                        match self.item()? {
                            Cbor::Break => break,
                            x => v.push(x),
                        }
                    },
                }
                Cbor::Array(v)
            }
            5 => {
                let mut v = Vec::new();
                match self.arg(ai)? {
                    Some(n) => {
                        for _ in 0..n {
                            let k = self.item()?;
                            let val = self.item()?;
                            v.push((k, val));
                        }
                    }
                    None => loop {
                        match self.item()? {
                            Cbor::Break => break,
                            k => {
                                let val = self.item()?;
                                v.push((k, val));
                            }
                        }
                    },
                }
                Cbor::Map(v)
            }
            6 => {
                let t = self.arg(ai)?.ok_or("bad tag")?;
                Cbor::Tag(t, Box::new(self.item()?))
            }
            7 => match ai {
                20 => Cbor::Bool(false),
                21 => Cbor::Bool(true),
                22 | 23 => Cbor::Null,
                25 => {
                    let b = self.take(2)?;
                    Cbor::F(half_to_f64(u16::from_be_bytes([b[0], b[1]])))
                }
                26 => {
                    let b = self.take(4)?;
                    Cbor::F(f32::from_be_bytes(b.try_into().unwrap()) as f64)
                }
                27 => {
                    let b = self.take(8)?;
                    Cbor::F(f64::from_be_bytes(b.try_into().unwrap()))
                }
                31 => Cbor::Break,
                24 => {
                    self.byte()?;
                    Cbor::Null
                }
                _ => Cbor::Null,
            },
            _ => unreachable!(),
        })
    }
}

fn half_to_f64(h: u16) -> f64 {
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f64;
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let v = match exp {
        0 => mant * 2f64.powi(-24),
        31 => {
            if mant == 0.0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1.0 + mant / 1024.0) * 2f64.powi(exp - 15),
    };
    sign * v
}

fn as_u64(c: &Cbor) -> u64 {
    match c {
        Cbor::U(u) => *u,
        Cbor::I(i) => *i as u64,
        Cbor::F(f) => *f as u64,
        _ => 0,
    }
}

fn as_i64(c: &Cbor) -> i64 {
    match c {
        Cbor::U(u) => *u as i64,
        Cbor::I(i) => *i,
        Cbor::F(f) => *f as i64,
        _ => 0,
    }
}

/// Unwraps a chunk envelope: raw bytes or `[size, lz4 bytes]`.
fn chunk_payload(c: &Cbor) -> Result<Vec<u8>, String> {
    match c {
        Cbor::Bytes(b) => Ok(b.clone()),
        Cbor::Array(a) if a.len() == 2 => {
            let size = as_u64(&a[0]) as usize;
            match &a[1] {
                Cbor::Bytes(b) => lz4_decompress(b, size),
                _ => Err("bad compressed chunk".into()),
            }
        }
        _ => Err("bad chunk envelope".into()),
    }
}

fn lz4_decompress(input: &[u8], size: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; size];
    let n = lz4_flex::block::decompress_into(input, &mut out).map_err(|e| e.to_string())?;
    out.truncate(n);
    Ok(out)
}

pub fn convert_ftr(input: &str, w: &mut Writer) -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(input)?;
    let mut p = P { b: &data, p: 0 };
    // Self-described CBOR tag + indefinite array.
    let head = p.item()?;
    let mut chunks: Vec<Cbor> = match head {
        Cbor::Tag(55799, inner) => match *inner {
            Cbor::Array(v) => v,
            _ => return Err("FTR: expected array".into()),
        },
        Cbor::Array(v) => v,
        _ => return Err("FTR: not a CBOR-tagged FTR file".into()),
    };
    w.set_file_type(FileType::SystemC)?;
    let mut dict: HashMap<u64, String> = HashMap::new();
    dict.insert(0, String::new());
    let mut streams: HashMap<u64, NodeId> = HashMap::new();
    let mut gens: HashMap<u64, NodeId> = HashMap::new();
    let mut txids: HashMap<u64, TxId> = HashMap::new();
    let mut relations: Vec<(u64, u64, u64, u64, u64)> = Vec::new();
    let mut strs: HashMap<u64, StrId> = HashMap::new();
    let mut timescale_set = false;
    let k_ftr_id = w.intern("ftr.id");
    let mut last_time = 0u64;
    for chunk in chunks.drain(..) {
        let (tag, body) = match chunk {
            Cbor::Tag(t, b) => (t, *b),
            _ => continue,
        };
        match tag {
            6 => {
                let payload = chunk_payload(&body).or_else(|_| Ok::<_, String>(Vec::new()))?;
                let info = if payload.is_empty() { body.clone() } else { P { b: &payload, p: 0 }.item()? };
                if let Cbor::Array(a) = info {
                    if let Some(ts) = a.first() {
                        w.set_timescale(as_i64(ts) as i8)?;
                        timescale_set = true;
                    }
                    if let Some(Cbor::Tag(1, e)) = a.get(1) {
                        w.set_file_attr("ftr.epoch", Value::U64(as_u64(e)))?;
                    }
                }
            }
            8 | 9 => {
                let payload = chunk_payload(&body)?;
                if let Cbor::Map(m) = (P { b: &payload, p: 0 }).item()? {
                    for (k, v) in m {
                        if let Cbor::Text(t) = v {
                            dict.insert(as_u64(&k), t);
                        }
                    }
                }
            }
            10 | 11 => {
                let payload = chunk_payload(&body)?;
                if let Cbor::Array(entries) = (P { b: &payload, p: 0 }).item()? {
                    for e in entries {
                        if let Cbor::Tag(t, inner) = e {
                            if let Cbor::Array(a) = *inner {
                                let id = as_u64(&a[0]);
                                let name = dict.get(&as_u64(&a[1])).cloned().unwrap_or_default();
                                match t {
                                    16 => {
                                        let kind = dict.get(&as_u64(&a[2])).cloned().unwrap_or_default();
                                        let n = w.add_stream(None, &name, &kind);
                                        w.node_attr(n, "ftr.id", Value::U64(id))?;
                                        streams.insert(id, n);
                                    }
                                    17 => {
                                        let sid = as_u64(&a[2]);
                                        let s = *streams.get(&sid).ok_or("FTR: generator of unknown stream")?;
                                        let n = w.add_generator(s, &name);
                                        w.node_attr(n, "ftr.id", Value::U64(id))?;
                                        gens.insert(id, n);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            12 | 13 => {
                let a = match body {
                    Cbor::Array(a) => a,
                    _ => continue,
                };
                let payload = if tag == 12 {
                    match a.get(3) {
                        Some(Cbor::Bytes(b)) => b.clone(),
                        _ => continue,
                    }
                } else {
                    let size = as_u64(&a[3]) as usize;
                    match a.get(4) {
                        Some(Cbor::Bytes(b)) => lz4_decompress(b, size)?,
                        _ => continue,
                    }
                };
                if let Cbor::Array(txs) = (P { b: &payload, p: 0 }).item()? {
                    for tx in txs {
                        let items = match tx {
                            Cbor::Array(v) => v,
                            _ => continue,
                        };
                        let mut cur: Option<(TxId, u64)> = None;
                        for it in items {
                            if let Cbor::Tag(t, inner) = it {
                                if let Cbor::Array(f) = *inner {
                                    match t {
                                        6 => {
                                            let id = as_u64(&f[0]);
                                            let gen = *gens.get(&as_u64(&f[1])).ok_or("FTR: tx of unknown generator")?;
                                            let start = as_u64(&f[2]);
                                            let end = as_u64(&f[3]).max(start);
                                            let vid = w.begin_tx(gen, start)?;
                                            w.tx_attr(vid, k_ftr_id, AttrPhase::Begin, &Value::U64(id))?;
                                            txids.insert(id, vid);
                                            last_time = last_time.max(end);
                                            cur = Some((vid, end));
                                        }
                                        7 | 8 | 9 => {
                                            if let Some((vid, _)) = cur {
                                                let name_id = as_u64(&f[0]);
                                                let key = *strs.entry(name_id).or_insert_with(|| {
                                                    let n = dict.get(&name_id).cloned().unwrap_or_default();
                                                    w.intern(&n)
                                                });
                                                let phase = match t {
                                                    7 => AttrPhase::Begin,
                                                    8 => AttrPhase::Record,
                                                    _ => AttrPhase::End,
                                                };
                                                let value = ftr_value(w, as_u64(&f[1]), &f[2], &dict, &mut strs);
                                                w.tx_attr(vid, key, phase, &value)?;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        if let Some((vid, end)) = cur {
                            w.end_tx(vid, end, TxStatus::Unset)?;
                        }
                    }
                }
            }
            14 | 15 => {
                let payload = chunk_payload(&body)?;
                if let Cbor::Array(rels) = (P { b: &payload, p: 0 }).item()? {
                    for r in rels {
                        if let Cbor::Array(f) = r {
                            if f.len() >= 3 {
                                relations.push((as_u64(&f[0]), as_u64(&f[1]), as_u64(&f[2]), f.get(3).map(as_u64).unwrap_or(0), f.get(4).map(as_u64).unwrap_or(0)));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !timescale_set {
        w.set_timescale(-12)?;
    }
    let k_from_stream = w.intern("ftr.from_stream");
    let k_to_stream = w.intern("ftr.to_stream");
    let mut missing = 0u64;
    for (name_id, from, to, fs, ts) in relations {
        let kind = *strs.entry(name_id).or_insert_with(|| {
            let n = dict.get(&name_id).cloned().unwrap_or_default();
            w.intern(&n)
        });
        match (txids.get(&from), txids.get(&to)) {
            (Some(&f), Some(&t)) => {
                let attrs = [(k_from_stream, Value::U64(fs)), (k_to_stream, Value::U64(ts))];
                w.relate(kind, f, t, &attrs)?;
            }
            _ => missing += 1,
        }
    }
    if missing > 0 {
        eprintln!("ftr: {missing} relations reference unknown transactions");
    }
    w.set_time(last_time)?;
    Ok(())
}

fn ftr_value(w: &mut Writer, type_code: u64, v: &Cbor, dict: &HashMap<u64, String>, strs: &mut HashMap<u64, StrId>) -> Value {
    let dict_str = |w: &mut Writer, strs: &mut HashMap<u64, StrId>, id: u64| -> StrId {
        *strs.entry(id).or_insert_with(|| {
            let s = dict.get(&id).cloned().unwrap_or_default();
            w.intern(&s)
        })
    };
    match type_code {
        0 => match v {
            Cbor::Bool(b) => Value::Bool(*b),
            other => Value::Bool(as_u64(other) != 0),
        },
        1 => {
            let id = as_u64(v);
            let name = dict_str(w, strs, id);
            Value::Enum { value: 0, name }
        }
        2 => Value::I64(as_i64(v)),
        3 => Value::U64(as_u64(v)),
        4 | 7 | 8 => match v {
            Cbor::F(f) => Value::F64(*f),
            other => Value::F64(as_i64(other) as f64),
        },
        5 | 6 => {
            let id = as_u64(v);
            let s = dict.get(&id).cloned().unwrap_or_default();
            let width = s.len() as u32;
            let states = if type_code == 5 { 2 } else { 4 };
            let mut data = vec![0u8; vtr::value::packed_len(width, states)];
            vtr::signal::pack_ascii(s.as_bytes(), width, states, &mut data);
            if states == 2 {
                Value::Bits { width, data }
            } else {
                Value::Logic { width, data }
            }
        }
        9 => Value::Pointer(as_u64(v)),
        10 => Value::Str(dict_str(w, strs, as_u64(v))),
        11 => Value::Time(as_u64(v)),
        _ => match v {
            Cbor::U(u) => Value::U64(*u),
            Cbor::I(i) => Value::I64(*i),
            Cbor::F(f) => Value::F64(*f),
            Cbor::Bool(b) => Value::Bool(*b),
            Cbor::Text(t) => Value::Str(w.intern(t)),
            _ => Value::Null,
        },
    }
}
