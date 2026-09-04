//! VCD output and VCD change accounting, used to check that a VTR file and the
//! FST it was made from carry the same value changes.
//!
//! * [`vtr_to_vcd`] streams a VTR file into a VCD (hierarchy, then every change in
//!   time order through `Reader::for_each_change`).
//! * [`fst_to_vcd`] does the same for an FST through `fst-reader`'s streaming API.
//! * [`vcd_stats`] counts the value changes of a VCD per signal (VCD id code), and
//!   [`compare`] checks two VCDs for identical counts, in total and per signal.
//!
//! Both writers use the same conventions (one id code per signal, shared by
//! aliases; names copied verbatim; full-width vectors), so the VCDs of a matching
//! pair differ only by header text and id codes.

use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalValue, FstVarType};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use vtr::{NodeData, NodeId, Reader, ScopeType, SignalKind, SignalValue};

type BoxError = Box<dyn std::error::Error>;

/// VCD identifier code for signal `i`: base-94 digits over the printable ASCII range.
fn id_code(mut i: usize) -> String {
    let mut s = String::new();
    loop {
        s.push((b'!' + (i % 94) as u8) as char);
        i /= 94;
        if i == 0 {
            return s;
        }
    }
}

fn timescale_line(exp: i8) -> String {
    // 10^exp seconds as "<1|10|100><unit>".
    let units = [(0, "s"), (-3, "ms"), (-6, "us"), (-9, "ns"), (-12, "ps"), (-15, "fs")];
    let (uexp, unit) = units.iter().copied().find(|(u, _)| exp >= *u).unwrap_or((-15, "fs"));
    let mult = 10u32.pow((exp - uexp) as u32);
    format!("$timescale {mult}{unit} $end")
}

fn scope_keyword(t: ScopeType) -> &'static str {
    match t {
        ScopeType::Module => "module",
        ScopeType::Task => "task",
        ScopeType::Function => "function",
        ScopeType::Begin => "begin",
        ScopeType::Fork => "fork",
        _ => "module",
    }
}

/// Writes `path` (a VTR file) as VCD to `out`. Returns the number of value changes written.
pub fn vtr_to_vcd(path: &str, out: &str) -> Result<u64, BoxError> {
    let r = Reader::open(path)?;
    let h = r.hierarchy();
    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);
    writeln!(w, "$date {} $end", r.meta().date)?;
    writeln!(w, "$version VTR vtr2vcd $end")?;
    writeln!(w, "{}", timescale_line(r.meta().timescale))?;
    // Depth-first over the hierarchy, scopes and vars only.
    fn walk(r: &Reader, n: NodeId, w: &mut impl Write) -> io::Result<()> {
        let h = r.hierarchy();
        match h.node(n).data {
            NodeData::Scope { scope_type, .. } => {
                writeln!(w, "$scope {} {} $end", scope_keyword(scope_type), r.name(n))?;
                for c in h.children(n) {
                    walk(r, c, w)?;
                }
                writeln!(w, "$upscope $end")?;
            }
            NodeData::Var { signal, .. } => {
                let (kw, width) = match h.signal_kind(signal) {
                    Some(SignalKind::Bits { width, .. }) => ("wire", width),
                    Some(SignalKind::Real) => ("real", 64),
                    _ => ("string", 1),
                };
                writeln!(w, "$var {kw} {width} {} {} $end", id_code(signal.0 as usize), r.name(n))?;
            }
            _ => {}
        }
        Ok(())
    }
    for root in h.roots() {
        walk(&r, root, &mut w)?;
    }
    writeln!(w, "$enddefinitions $end")?;
    let ids: Vec<String> = (0..h.signals.len()).map(id_code).collect();
    let mut last = u64::MAX;
    let mut n = 0u64;
    let mut err = None;
    r.for_each_change(0, u64::MAX, |t, s, v| {
        if err.is_some() {
            return;
        }
        let res = (|| -> io::Result<()> {
            if t != last {
                writeln!(w, "#{t}")?;
                last = t;
            }
            let id = &ids[s.0 as usize];
            match v {
                SignalValue::Bits { width: 1, .. } => writeln!(w, "{}{id}", v.to_ascii()),
                SignalValue::Bits { .. } => writeln!(w, "b{} {id}", v.to_ascii()),
                SignalValue::Real(x) => writeln!(w, "r{x} {id}"),
                SignalValue::VarLen(b) => writeln!(w, "s{} {id}", String::from_utf8_lossy(b)),
            }
        })();
        if let Err(e) = res {
            err = Some(e);
        }
        n += 1;
    })?;
    if let Some(e) = err {
        return Err(e.into());
    }
    w.flush()?;
    Ok(n)
}

/// Writes `path` (an FST file) as VCD to `out` through `fst-reader`. Returns the number of value changes.
pub fn fst_to_vcd(path: &str, out: &str) -> Result<u64, BoxError> {
    let mut rd = FstReader::open(BufReader::with_capacity(1 << 20, File::open(path)?)).map_err(|e| format!("{e:?}"))?;
    let hdr = rd.get_header();
    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);
    writeln!(w, "$date {} $end", hdr.date)?;
    writeln!(w, "$version {} fst2vcd $end", hdr.version.trim())?;
    writeln!(w, "{}", timescale_line(hdr.timescale_exponent))?;
    // Width per signal handle (1 = scalar), filled while emitting the header.
    let mut widths: Vec<u32> = Vec::new();
    let mut herr: Option<io::Error> = None;
    rd.read_hierarchy(|e| {
        if herr.is_some() {
            return;
        }
        let res = match e {
            FstHierarchyEntry::Scope { tpe, name, .. } => {
                let kw = match tpe {
                    fst_reader::FstScopeType::Task => "task",
                    fst_reader::FstScopeType::Function => "function",
                    fst_reader::FstScopeType::Begin => "begin",
                    fst_reader::FstScopeType::Fork => "fork",
                    _ => "module",
                };
                writeln!(w, "$scope {kw} {name} $end")
            }
            FstHierarchyEntry::UpScope => writeln!(w, "$upscope $end"),
            FstHierarchyEntry::Var { tpe, name, length, handle, .. } => {
                let i = handle.get_index();
                if widths.len() <= i {
                    widths.resize(i + 1, 0);
                }
                widths[i] = length;
                let kw = match tpe {
                    FstVarType::Real | FstVarType::RealParameter | FstVarType::RealTime | FstVarType::ShortReal => "real",
                    FstVarType::GenericString => "string",
                    _ => "wire",
                };
                writeln!(w, "$var {kw} {length} {} {name} $end", id_code(i))
            }
            _ => Ok(()),
        };
        if let Err(e) = res {
            herr = Some(e);
        }
    })
    .map_err(|e| format!("{e:?}"))?;
    if let Some(e) = herr {
        return Err(e.into());
    }
    writeln!(w, "$enddefinitions $end")?;
    let ids: Vec<String> = (0..widths.len()).map(id_code).collect();
    let mut last = u64::MAX;
    let mut n = 0u64;
    rd.read_signals(&FstFilter::all(), |t, handle, v| -> io::Result<()> {
        if t != last {
            writeln!(w, "#{t}")?;
            last = t;
        }
        let i = handle.get_index();
        let id = &ids[i];
        n += 1;
        match v {
            FstSignalValue::String(bytes) => {
                if widths[i] == 1 {
                    w.write_all(bytes)?;
                    writeln!(w, "{id}")
                } else {
                    w.write_all(b"b")?;
                    w.write_all(bytes)?;
                    writeln!(w, " {id}")
                }
            }
            FstSignalValue::Real(x) => writeln!(w, "r{x} {id}"),
        }
    })
    .map_err(|e| format!("{e:?}"))?;
    w.flush()?;
    Ok(n)
}

/// Change counts of a VCD file.
pub struct VcdStats {
    /// Total value changes (every scalar/vector/real/string value line, `$dumpvars` included).
    pub changes: u64,
    /// Per signal: full hierarchical name (first var declared with the id code,
    /// whitespace removed) -> number of changes. Aliases share the id code.
    pub per_signal: HashMap<String, u64>,
}

/// Packs a VCD id code (at most 8 bytes) into a map key.
fn id_key(id: &[u8]) -> Result<u64, BoxError> {
    if id.is_empty() || id.len() > 8 {
        return Err(format!("unsupported VCD id code {:?}", String::from_utf8_lossy(id)).into());
    }
    let mut k = [0u8; 8];
    k[..id.len()].copy_from_slice(id);
    Ok(u64::from_le_bytes(k))
}

fn trim_end(b: &[u8]) -> &[u8] {
    let mut e = b.len();
    while e > 0 && (b[e - 1] == b'\n' || b[e - 1] == b'\r' || b[e - 1] == b' ' || b[e - 1] == b'\t') {
        e -= 1;
    }
    &b[..e]
}

/// Counts the value changes of `path`, in total and per signal.
pub fn vcd_stats(path: &str) -> Result<VcdStats, BoxError> {
    let mut rd = BufReader::with_capacity(4 << 20, File::open(path)?);
    let mut line = Vec::with_capacity(256);
    let mut scopes: Vec<String> = Vec::new();
    let mut names: HashMap<u64, String> = HashMap::new();
    let mut counts: HashMap<u64, u64> = HashMap::new();
    let mut in_header = true;
    let mut total = 0u64;
    loop {
        line.clear();
        if rd.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let l = trim_end(&line);
        if l.is_empty() {
            continue;
        }
        if in_header {
            let text = String::from_utf8_lossy(l);
            let toks: Vec<&str> = text.split_whitespace().collect();
            match toks.first().copied() {
                Some("$scope") if toks.len() >= 3 => scopes.push(toks[2].to_string()),
                Some("$upscope") => {
                    scopes.pop();
                }
                Some("$var") if toks.len() >= 5 => {
                    let key = id_key(toks[3].as_bytes())?;
                    let end = if toks.last() == Some(&"$end") { toks.len() - 1 } else { toks.len() };
                    let name: String = toks[4..end].concat();
                    let full = if scopes.is_empty() { name } else { format!("{}.{}", scopes.join("."), name) };
                    names.entry(key).or_insert(full);
                }
                Some("$enddefinitions") => in_header = false,
                _ => {}
            }
            continue;
        }
        let id = match l[0] {
            b'0' | b'1' | b'x' | b'X' | b'z' | b'Z' | b'u' | b'U' | b'w' | b'W' | b'l' | b'L' | b'h' | b'H' | b'-' => &l[1..],
            b'b' | b'B' | b'r' | b'R' | b's' | b'S' => match l.iter().position(|&c| c == b' ') {
                Some(p) => &l[p + 1..],
                None => return Err(format!("malformed VCD value line {:?}", String::from_utf8_lossy(l)).into()),
            },
            _ => continue, // '#' time stamps, $dumpvars / $end and other keywords
        };
        *counts.entry(id_key(id)?).or_insert(0) += 1;
        total += 1;
    }
    let mut per_signal = HashMap::with_capacity(names.len());
    for (key, name) in names {
        per_signal.insert(name, counts.get(&key).copied().unwrap_or(0));
    }
    Ok(VcdStats { changes: total, per_signal })
}

/// Result of comparing two VCDs.
pub struct Comparison {
    pub changes_a: u64,
    pub changes_b: u64,
    pub signals_a: usize,
    pub signals_b: usize,
    /// Signals whose change counts differ or that exist on one side only (name, a, b).
    pub mismatches: Vec<(String, u64, u64)>,
}

impl Comparison {
    pub fn identical(&self) -> bool {
        self.changes_a == self.changes_b && self.signals_a == self.signals_b && self.mismatches.is_empty()
    }
}

/// Compares the change counts of two VCDs, in total and per signal name.
pub fn compare(a: &VcdStats, b: &VcdStats) -> Comparison {
    let mut mismatches = Vec::new();
    for (name, &na) in &a.per_signal {
        let nb = b.per_signal.get(name).copied().unwrap_or(0);
        if na != nb {
            mismatches.push((name.clone(), na, nb));
        }
    }
    for (name, &nb) in &b.per_signal {
        if !a.per_signal.contains_key(name) {
            mismatches.push((name.clone(), 0, nb));
        }
    }
    mismatches.sort();
    Comparison { changes_a: a.changes, changes_b: b.changes, signals_a: a.per_signal.len(), signals_b: b.per_signal.len(), mismatches }
}
