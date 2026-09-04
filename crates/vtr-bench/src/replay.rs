//! Replay files: a writer-neutral, pre-decoded stream of value changes that
//! every writer under test consumes identically.
//!
//! ```text
//! magic "VTRRPL01" | i8 timescale | u32 n_signals | u32 n_hier_ops | u64 n_records | u64 end_time
//! signals: n_signals x { u8 kind (0 bits 1 real 2 varlen), u8 states, u32 width }
//! hier ops: { u8 op (1 scope 2 upscope 3 var 4 alias), [varint sig], varint len, name }
//! records:  { varint dt, varint sig, u8 form (0 packed 2-state, 1 ascii, 2 real, 3 varlen), payload }
//! ```

use std::io::{BufRead, BufReader, BufWriter, Read, Write};

#[derive(Clone, Copy, Debug)]
pub struct SigDecl {
    pub kind: u8,
    pub states: u8,
    pub width: u32,
}

#[derive(Clone, Debug)]
pub enum HierOp {
    Scope(String),
    Up,
    Var(u32, String),
    Alias(u32, String),
}

#[derive(Clone, Copy, Debug)]
pub struct Rec {
    pub time: u64,
    pub sig: u32,
    pub form: u8,
    pub off: u32,
    pub len: u32,
}

pub struct Replay {
    pub timescale: i8,
    pub signals: Vec<SigDecl>,
    pub hier: Vec<HierOp>,
    pub recs: Vec<Rec>,
    pub heap: Vec<u8>,
    pub end_time: u64,
}

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

impl Replay {
    pub fn payload(&self, r: &Rec) -> &[u8] {
        &self.heap[r.off as usize..(r.off + r.len) as usize]
    }

    pub fn push(&mut self, time: u64, sig: u32, form: u8, payload: &[u8]) {
        let off = self.heap.len() as u32;
        self.heap.extend_from_slice(payload);
        self.recs.push(Rec { time, sig, form, off, len: payload.len() as u32 });
        self.end_time = self.end_time.max(time);
    }

    pub fn save(&self, path: &str) -> std::io::Result<()> {
        let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        w.write_all(b"VTRRPL01")?;
        w.write_all(&[self.timescale as u8])?;
        w.write_all(&(self.signals.len() as u32).to_le_bytes())?;
        w.write_all(&(self.hier.len() as u32).to_le_bytes())?;
        w.write_all(&(self.recs.len() as u64).to_le_bytes())?;
        w.write_all(&self.end_time.to_le_bytes())?;
        let mut buf = Vec::new();
        for s in &self.signals {
            buf.push(s.kind);
            buf.push(s.states);
            buf.extend_from_slice(&s.width.to_le_bytes());
        }
        for h in &self.hier {
            match h {
                HierOp::Scope(n) => {
                    buf.push(1);
                    put_varint(&mut buf, n.len() as u64);
                    buf.extend_from_slice(n.as_bytes());
                }
                HierOp::Up => buf.push(2),
                HierOp::Var(s, n) | HierOp::Alias(s, n) => {
                    buf.push(if matches!(h, HierOp::Var(..)) { 3 } else { 4 });
                    put_varint(&mut buf, *s as u64);
                    put_varint(&mut buf, n.len() as u64);
                    buf.extend_from_slice(n.as_bytes());
                }
            }
        }
        w.write_all(&buf)?;
        buf.clear();
        let mut prev = 0u64;
        for r in &self.recs {
            put_varint(&mut buf, r.time - prev);
            prev = r.time;
            put_varint(&mut buf, r.sig as u64);
            buf.push(r.form);
            let p = self.payload(r);
            if r.form == 1 || r.form == 3 {
                put_varint(&mut buf, p.len() as u64);
            }
            buf.extend_from_slice(p);
            if buf.len() > (1 << 20) {
                w.write_all(&buf)?;
                buf.clear();
            }
        }
        w.write_all(&buf)?;
        w.flush()
    }

    pub fn load(path: &str) -> std::io::Result<Replay> {
        let mut f = BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
        let mut hdr = [0u8; 33];
        f.read_exact(&mut hdr)?;
        if &hdr[..8] != b"VTRRPL01" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a replay file"));
        }
        let timescale = hdr[8] as i8;
        let n_sig = u32::from_le_bytes(hdr[9..13].try_into().unwrap()) as usize;
        let n_hier = u32::from_le_bytes(hdr[13..17].try_into().unwrap()) as usize;
        let n_rec = u64::from_le_bytes(hdr[17..25].try_into().unwrap()) as usize;
        let end_time = u64::from_le_bytes(hdr[25..33].try_into().unwrap());
        let mut rest = Vec::new();
        f.read_to_end(&mut rest)?;
        let mut p = 0usize;
        let mut signals = Vec::with_capacity(n_sig);
        for _ in 0..n_sig {
            signals.push(SigDecl { kind: rest[p], states: rest[p + 1], width: u32::from_le_bytes(rest[p + 2..p + 6].try_into().unwrap()) });
            p += 6;
        }
        let mut hier = Vec::with_capacity(n_hier);
        for _ in 0..n_hier {
            let op = rest[p];
            p += 1;
            match op {
                1 => {
                    let l = get_varint(&rest, &mut p) as usize;
                    hier.push(HierOp::Scope(String::from_utf8_lossy(&rest[p..p + l]).into_owned()));
                    p += l;
                }
                2 => hier.push(HierOp::Up),
                _ => {
                    let s = get_varint(&rest, &mut p) as u32;
                    let l = get_varint(&rest, &mut p) as usize;
                    let n = String::from_utf8_lossy(&rest[p..p + l]).into_owned();
                    p += l;
                    hier.push(if op == 3 { HierOp::Var(s, n) } else { HierOp::Alias(s, n) });
                }
            }
        }
        let mut recs = Vec::with_capacity(n_rec);
        let mut heap = Vec::with_capacity(rest.len() - p);
        let mut t = 0u64;
        for _ in 0..n_rec {
            t += get_varint(&rest, &mut p);
            let sig = get_varint(&rest, &mut p) as u32;
            let form = rest[p];
            p += 1;
            let len = match form {
                0 => (signals[sig as usize].width as usize).div_ceil(8),
                2 => 8,
                _ => get_varint(&rest, &mut p) as usize,
            };
            let off = heap.len() as u32;
            heap.extend_from_slice(&rest[p..p + len]);
            p += len;
            recs.push(Rec { time: t, sig, form, off, len: len as u32 });
        }
        Ok(Replay { timescale, signals, hier, recs, heap, end_time })
    }

    /// Converts an FST file into a replay.
    pub fn from_fst(path: &str) -> Result<Replay, String> {
        use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalValue};
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut rd = FstReader::open(BufReader::with_capacity(1 << 20, file)).map_err(|e| format!("{e:?}"))?;
        let header = rd.get_header();
        let mut rp = Replay { timescale: header.timescale_exponent, signals: Vec::new(), hier: Vec::new(), recs: Vec::new(), heap: Vec::new(), end_time: 0 };
        let mut handle_to_sig: Vec<u32> = Vec::new();
        rd.read_hierarchy(|e| match e {
            FstHierarchyEntry::Scope { name, .. } => rp.hier.push(HierOp::Scope(name)),
            FstHierarchyEntry::UpScope => rp.hier.push(HierOp::Up),
            FstHierarchyEntry::Var { tpe, name, length, handle, is_alias, .. } => {
                let idx = handle.get_index();
                if idx >= handle_to_sig.len() {
                    handle_to_sig.resize(idx + 1, u32::MAX);
                }
                if is_alias {
                    rp.hier.push(HierOp::Alias(handle_to_sig[idx], name));
                } else {
                    let sig = rp.signals.len() as u32;
                    let decl = if tpe.is_real() {
                        SigDecl { kind: 1, states: 0, width: 64 }
                    } else if tpe == fst_reader::FstVarType::GenericString {
                        SigDecl { kind: 2, states: 0, width: 0 }
                    } else {
                        SigDecl { kind: 0, states: 4, width: length.max(1) }
                    };
                    rp.signals.push(decl);
                    handle_to_sig[idx] = sig;
                    rp.hier.push(HierOp::Var(sig, name));
                }
            }
            _ => {}
        })
        .map_err(|e| format!("{e:?}"))?;
        let mut packed = Vec::new();
        rd.read_signals(&FstFilter::all(), |time, handle, value| -> Result<(), ()> {
            let sig = handle_to_sig[handle.get_index()];
            let decl = rp.signals[sig as usize];
            match value {
                FstSignalValue::String(s) => match decl.kind {
                    2 => rp.push(time, sig, 3, s),
                    1 => {
                        let v: f64 = std::str::from_utf8(s).ok().and_then(|t| t.parse().ok()).unwrap_or(0.0);
                        rp.push(time, sig, 2, &v.to_le_bytes());
                    }
                    _ => {
                        packed.clear();
                        packed.resize((decl.width as usize).div_ceil(8), 0);
                        if vtr::signal::pack_ascii(s, decl.width, 2, &mut packed) {
                            rp.push(time, sig, 0, &packed);
                        } else {
                            rp.push(time, sig, 1, s);
                        }
                    }
                },
                FstSignalValue::Real(r) => rp.push(time, sig, 2, &r.to_le_bytes()),
            }
            Ok(())
        })
        .map_err(|e| format!("{e:?}"))?;
        rp.end_time = rp.end_time.max(header.end_time);
        Ok(rp)
    }

    /// Builds an `n`-way replica: signals and hierarchy are copied `n` times
    /// under top scopes `copyK`, vectors wider than one bit are XOR-perturbed
    /// per copy so copies are not byte-identical, times are unchanged.
    pub fn replicate(&self, n: usize) -> Replay {
        let s = self.signals.len() as u32;
        let mut out = Replay { timescale: self.timescale, signals: Vec::with_capacity(self.signals.len() * n), hier: Vec::new(), recs: Vec::with_capacity(self.recs.len() * n), heap: Vec::with_capacity(self.heap.len() * n), end_time: self.end_time };
        for k in 0..n {
            out.signals.extend_from_slice(&self.signals);
            out.hier.push(HierOp::Scope(format!("copy{k}")));
            for h in &self.hier {
                out.hier.push(match h {
                    HierOp::Scope(x) => HierOp::Scope(x.clone()),
                    HierOp::Up => HierOp::Up,
                    HierOp::Var(sg, x) => HierOp::Var(sg + k as u32 * s, x.clone()),
                    HierOp::Alias(sg, x) => HierOp::Alias(sg + k as u32 * s, x.clone()),
                });
            }
            out.hier.push(HierOp::Up);
        }
        let mut buf = Vec::new();
        for r in &self.recs {
            for k in 0..n {
                let p = self.payload(r);
                let decl = self.signals[r.sig as usize];
                let sig = r.sig + k as u32 * s;
                if k == 0 || r.form != 0 || decl.width < 2 {
                    out.push(r.time, sig, r.form, p);
                } else {
                    buf.clear();
                    buf.extend_from_slice(p);
                    let h = (k as u64).wrapping_mul(0x9E3779B97F4A7C15);
                    for (i, b) in buf.iter_mut().enumerate() {
                        *b ^= (h >> ((i % 8) * 8)) as u8;
                    }
                    vtr::signal::trim_top_bits(&mut buf, decl.width);
                    out.push(r.time, sig, 0, &buf);
                }
            }
        }
        out
    }

    pub fn n_times(&self) -> usize {
        let mut n = 0;
        let mut last = u64::MAX;
        for r in &self.recs {
            if r.time != last {
                n += 1;
                last = r.time;
            }
        }
        n
    }
}

/// Simple deterministic PRNG.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Synthetic workload generator. `kind`:
/// * `long_sparse`: many time steps, few active signals per step.
/// * `many_active`: many signals, short run, most change each cycle.
/// * `wide_bus`: wide buses (up to 2048 bits) toggling every cycle.
pub fn generate(kind: &str, scale: u64) -> Replay {
    let mut rp = Replay { timescale: -12, signals: Vec::new(), hier: Vec::new(), recs: Vec::new(), heap: Vec::new(), end_time: 0 };
    let mut rng = Rng(0x1234_5678_9abc_def1);
    rp.hier.push(HierOp::Scope("top".into()));
    let add = |rp: &mut Replay, name: String, width: u32| -> u32 {
        let s = rp.signals.len() as u32;
        rp.signals.push(SigDecl { kind: 0, states: 4, width });
        rp.hier.push(HierOp::Var(s, name));
        s
    };
    match kind {
        "long_sparse" => {
            // 20k signals in 200 modules, 100 * scale time steps, ~0.1% of signals change per step.
            let n_sig = 20_000u32;
            let mut widths = Vec::new();
            for m in 0..200 {
                rp.hier.push(HierOp::Scope(format!("mod{m}")));
                for i in 0..100 {
                    let w = match i % 10 {
                        0..=5 => 1,
                        6 | 7 => 8,
                        8 => 32,
                        _ => 64,
                    };
                    add(&mut rp, format!("sig{i}"), w);
                    widths.push(w);
                }
                rp.hier.push(HierOp::Up);
            }
            let clk = add(&mut rp, "clk".into(), 1);
            let steps = 100 * scale;
            let mut counters = vec![0u64; n_sig as usize];
            for t in 0..steps {
                let time = t * 5000;
                rp.push(time, clk, 0, &[(t & 1) as u8]);
                if t & 1 == 1 {
                    for _ in 0..20 {
                        let s = rng.below(n_sig as u64) as usize;
                        counters[s] = counters[s].wrapping_add(1 + rng.below(3));
                        let w = widths[s];
                        let v = counters[s] & if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                        let bytes = v.to_le_bytes();
                        rp.push(time, s as u32, 0, &bytes[..(w as usize).div_ceil(8)]);
                    }
                }
            }
        }
        "many_active" => {
            // 2000 modules x 100 signals = 200k signals, 20 * scale cycles, 30% toggle per cycle.
            let mut widths = Vec::new();
            for m in 0..2000 {
                rp.hier.push(HierOp::Scope(format!("unit{m}")));
                for i in 0..100 {
                    let w = match i % 8 {
                        0..=3 => 1,
                        4 => 4,
                        5 => 16,
                        6 => 32,
                        _ => 128,
                    };
                    add(&mut rp, format!("s{i}"), w);
                    widths.push(w);
                }
                rp.hier.push(HierOp::Up);
            }
            let n_sig = widths.len();
            let clk = add(&mut rp, "clk".into(), 1);
            let mut vals = vec![0u64; n_sig];
            let mut buf = [0u8; 16];
            for t in 0..20 * scale {
                let time = t * 1000;
                rp.push(time, clk, 0, &[(t & 1) as u8]);
                if t & 1 == 0 {
                    continue;
                }
                for s in 0..n_sig {
                    if rng.below(10) < 3 {
                        let w = widths[s];
                        vals[s] = vals[s].wrapping_add(rng.below(1 << 12)) ^ (rng.next() & 0xff);
                        let v = vals[s] & if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
                        buf[..8].copy_from_slice(&v.to_le_bytes());
                        buf[8..].copy_from_slice(&(rng.next()).to_le_bytes());
                        rp.push(time, s as u32, 0, &buf[..(w as usize).div_ceil(8)]);
                    }
                }
            }
        }
        "wide_bus" => {
            // 64 buses of 256..2048 bits + valid strobes, 500 * scale cycles.
            let mut buses = Vec::new();
            for i in 0..64 {
                let w = [256u32, 512, 1024, 2048][i % 4];
                let d = add(&mut rp, format!("data{i}"), w);
                let v = add(&mut rp, format!("valid{i}"), 1);
                buses.push((d, v, w));
            }
            let clk = add(&mut rp, "clk".into(), 1);
            let mut payload = vec![0u8; 256];
            for t in 0..500 * scale {
                let time = t * 1000;
                rp.push(time, clk, 0, &[(t & 1) as u8]);
                if t & 1 == 0 {
                    continue;
                }
                for &(d, v, w) in &buses {
                    let active = rng.below(4) != 0;
                    rp.push(time, v, 0, &[active as u8]);
                    if active {
                        let n = (w as usize) / 8;
                        // Realistic payload: a few random words in a mostly-zero/pattern background.
                        for b in payload[..n].iter_mut() {
                            *b = 0;
                        }
                        for _ in 0..(n / 16) {
                            let o = rng.below(n as u64 - 8) as usize;
                            payload[o..o + 8].copy_from_slice(&rng.next().to_le_bytes());
                        }
                        rp.push(time, d, 0, &payload[..n]);
                    }
                }
            }
        }
        _ => panic!("unknown synthetic workload {kind}"),
    }
    rp.hier.push(HierOp::Up);
    rp
}

/// Reads lines from a possibly gzipped text file.
pub fn text_lines(path: &str) -> std::io::Result<Box<dyn Iterator<Item = String>>> {
    let f = std::fs::File::open(path)?;
    let r: Box<dyn Read> = if path.ends_with(".gz") { Box::new(flate2::read::GzDecoder::new(f)) } else { Box::new(f) };
    Ok(Box::new(BufReader::with_capacity(1 << 20, r).lines().map_while(Result::ok)))
}
