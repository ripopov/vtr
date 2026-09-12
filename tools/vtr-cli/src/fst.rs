//! FST -> VTR conversion (lossless with respect to the FST hierarchy and values).

use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalHandle, FstSignalValue};
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek, SeekFrom};
use vtr::{Direction, FileType, NodeId, ScopeType, SignalId, SignalKind, Value, VarType, Writer};

pub struct FstConvertOptions {
    /// States per bit for vector signals (4 for Verilog, 9 for VHDL).
    pub states: Option<u8>,
    pub progress: bool,
}

/// Reads the fields fst-reader does not expose: file type, time zero and the blackout list.
fn scan_fst_extras(path: &str) -> std::io::Result<(u8, i64, Vec<(bool, u64)>)> {
    let mut f = std::fs::File::open(path)?;
    let mut tag = [0u8; 1];
    let mut pos = 0u64;
    let len = f.metadata()?.len();
    let mut file_type = 0u8;
    let mut time_zero = 0i64;
    let mut blackout = Vec::new();
    while pos + 9 <= len {
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut tag)?;
        let mut l = [0u8; 8];
        f.read_exact(&mut l)?;
        let sec_len = u64::from_be_bytes(l);
        if tag[0] == 254 {
            break; // gzip wrapped: give up on extras
        }
        match tag[0] {
            0 => {
                f.seek(SeekFrom::Start(pos + 321))?;
                let mut b = [0u8; 9];
                if f.read_exact(&mut b).is_ok() {
                    file_type = b[0];
                    time_zero = i64::from_be_bytes(b[1..9].try_into().unwrap());
                }
            }
            2 => {
                let mut payload = vec![0u8; (sec_len - 8) as usize];
                f.read_exact(&mut payload)?;
                let mut r = vtr::varint::Reader::new(&payload);
                let n = r.u64().unwrap_or(0);
                let mut t = 0u64;
                for _ in 0..n {
                    let active = match r.u8() {
                        Ok(a) => a != 0,
                        Err(_) => break,
                    };
                    t = t.wrapping_add(r.u64().unwrap_or(0));
                    blackout.push((active, t));
                }
            }
            _ => {}
        }
        if sec_len < 8 {
            break;
        }
        pos += 1 + sec_len;
    }
    Ok((file_type, time_zero, blackout))
}

fn map_scope(t: fst_reader::FstScopeType) -> ScopeType {
    ScopeType::from_code(t as u16)
}

fn map_var(t: fst_reader::FstVarType) -> VarType {
    VarType::from_code(t as u16)
}

fn map_dir(d: fst_reader::FstVarDirection) -> Direction {
    Direction::from_u8(d as u8)
}

pub fn convert_fst(input: &str, w: &mut Writer, opts: &FstConvertOptions) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(input)?;
    let mut reader = FstReader::open(BufReader::with_capacity(1 << 20, file)).map_err(|e| format!("{e:?}"))?;
    let header = reader.get_header();
    let (file_type, time_zero, blackout) = scan_fst_extras(input).unwrap_or((0, 0, Vec::new()));
    w.set_timescale(header.timescale_exponent)?;
    w.set_time_zero(time_zero)?;
    w.set_file_type(FileType::from_u8(file_type))?;
    w.set_writer_name(&format!("vtr {} (from FST written by {})", env!("CARGO_PKG_VERSION"), header.version.trim()))?;
    w.set_date(header.date.trim())?;
    let states = opts.states.unwrap_or(if file_type == 0 { 4 } else { 9 });

    // Hierarchy.
    let mut handles: HashMap<usize, SignalId> = HashMap::new();
    let mut pending: Vec<(String, Value)> = Vec::new();
    let mut enum_tables: HashMap<u64, NodeId> = HashMap::new();
    let mut paths: HashMap<u64, String> = HashMap::new();
    let mut last_node: Option<NodeId> = None;
    let mut comments = Vec::new();
    let mut err: Option<vtr::Error> = None;
    let mut widths: Vec<u32> = Vec::new();
    let mut kinds: Vec<SignalKind> = Vec::new();
    reader
        .read_hierarchy(|e| {
            if err.is_some() {
                return;
            }
            let r: vtr::Result<()> = (|| {
                match e {
                    FstHierarchyEntry::Scope { tpe, name, component } => {
                        let n = w.begin_scope(&name, map_scope(tpe), &component);
                        for (k, v) in pending.drain(..) {
                            w.node_attr(n, &k, v)?;
                        }
                        last_node = Some(n);
                    }
                    FstHierarchyEntry::UpScope => {
                        w.end_scope()?;
                    }
                    FstHierarchyEntry::Var { tpe, direction, name, length, handle, is_alias } => {
                        let idx = handle.get_index();
                        let n = if is_alias {
                            let sig = *handles.get(&idx).ok_or(vtr::Error::Corrupt("alias to unknown handle"))?;
                            w.add_alias(&name, map_var(tpe), map_dir(direction), sig)?
                        } else {
                            let kind = if tpe.is_real() {
                                SignalKind::Real
                            } else if tpe == fst_reader::FstVarType::GenericString {
                                SignalKind::VarLen
                            } else {
                                SignalKind::Bits { width: length.max(1), states }
                            };
                            let (n, sig) = w.add_var(&name, map_var(tpe), map_dir(direction), kind);
                            handles.insert(idx, sig);
                            if idx >= widths.len() {
                                widths.resize(idx + 1, 0);
                                kinds.resize(idx + 1, SignalKind::VarLen);
                            }
                            widths[idx] = length;
                            kinds[idx] = kind;
                            n
                        };
                        for (k, v) in pending.drain(..) {
                            w.node_attr(n, &k, v)?;
                        }
                        last_node = Some(n);
                    }
                    FstHierarchyEntry::PathName { id, name } => {
                        paths.insert(id, name);
                    }
                    FstHierarchyEntry::SourceStem { is_instantiation, path_id, line } => {
                        let file = paths.get(&path_id).cloned().unwrap_or_default();
                        let fs = w.intern(&file);
                        let key = if is_instantiation { "fst.source_istem" } else { "fst.source_stem" };
                        let kf = w.intern("file");
                        let kl = w.intern("line");
                        pending.push((key.into(), Value::Map(vec![(kf, Value::Str(fs)), (kl, Value::U64(line))])));
                    }
                    FstHierarchyEntry::Comment { string } => comments.push(string),
                    FstHierarchyEntry::EnumTable { name, handle, mapping } => {
                        let entries: Vec<(&str, &str)> = mapping.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                        let n = w.add_enum_table(&name, &entries);
                        enum_tables.insert(handle, n);
                        last_node = Some(n);
                    }
                    FstHierarchyEntry::EnumTableRef { handle } => {
                        if let Some(n) = enum_tables.get(&handle) {
                            pending.push(("enum_table".into(), Value::U64(n.0 as u64)));
                        }
                    }
                    FstHierarchyEntry::VhdlVarInfo { type_name, var_type, data_type } => {
                        let t = w.intern(&type_name);
                        let k1 = w.intern("type_name");
                        let k2 = w.intern("var_type");
                        let k3 = w.intern("data_type");
                        pending.push((
                            "fst.vhdl".into(),
                            Value::Map(vec![(k1, Value::Str(t)), (k2, Value::U64(var_type as u64)), (k3, Value::U64(data_type as u64))]),
                        ));
                    }
                    FstHierarchyEntry::Array { name, array_type, left, right } => {
                        let nm = w.intern(&name);
                        let k1 = w.intern("name");
                        let k2 = w.intern("array_type");
                        let k3 = w.intern("left");
                        let k4 = w.intern("right");
                        pending.push((
                            "fst.array".into(),
                            Value::Map(vec![
                                (k1, Value::Str(nm)),
                                (k2, Value::U64(array_type as u64)),
                                (k3, Value::I64(left as i64)),
                                (k4, Value::I64(right as i64)),
                            ]),
                        ));
                    }
                    FstHierarchyEntry::Pack { name, pack_type, value } => {
                        let nm = w.intern(&name);
                        let k1 = w.intern("name");
                        let k2 = w.intern("pack_type");
                        let k3 = w.intern("arg");
                        pending.push((
                            "fst.pack".into(),
                            Value::Map(vec![(k1, Value::Str(nm)), (k2, Value::U64(pack_type as u64)), (k3, Value::U64(value))]),
                        ));
                    }
                    FstHierarchyEntry::SVEnum { name, enum_type, value } => {
                        let nm = w.intern(&name);
                        let k1 = w.intern("name");
                        let k2 = w.intern("enum_type");
                        let k3 = w.intern("arg");
                        pending.push((
                            "fst.sv_enum".into(),
                            Value::Map(vec![(k1, Value::Str(nm)), (k2, Value::U64(enum_type as u64)), (k3, Value::U64(value))]),
                        ));
                    }
                    FstHierarchyEntry::AttributeEnd => {
                        if let Some(n) = last_node {
                            let _ = w.node_attr(n, "fst.attr_end", Value::Null);
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = r {
                err = Some(e);
            }
        })
        .map_err(|e| format!("{e:?}"))?;
    if let Some(e) = err {
        return Err(Box::new(e));
    }
    if !comments.is_empty() {
        w.set_comment(&comments.join("\n"))?;
    }
    w.set_file_attr("fst.var_count", Value::U64(header.var_count))?;

    // Values.
    let mut err: Option<vtr::Error> = None;
    let total = header.end_time.saturating_sub(header.start_time).max(1);
    let mut last_pct = 0u64;
    let mut n = 0u64;
    let mut buf = Vec::new();
    reader
        .read_signals(&FstFilter::all(), |time, handle: FstSignalHandle, value| -> Result<(), ()> {
            if err.is_some() {
                return Err(());
            }
            let sig = handles[&handle.get_index()];
            let r = w.set_time(time).and_then(|_| match value {
                FstSignalValue::String(s) => match kinds[handle.get_index()] {
                    SignalKind::VarLen => w.emit_varlen(sig, s),
                    SignalKind::Real => {
                        buf.clear();
                        buf.extend_from_slice(s);
                        w.emit_logic_str(sig, &buf)
                    }
                    _ => w.emit_logic_str(sig, s),
                },
                FstSignalValue::Real(r) => w.emit_real(sig, r),
            });
            if let Err(e) = r {
                err = Some(e);
                return Err(());
            }
            n += 1;
            if opts.progress && n % (1 << 20) == 0 {
                let pct = (time.saturating_sub(header.start_time)) * 100 / total;
                if pct != last_pct {
                    eprintln!("  {pct}% ({n} changes)");
                    last_pct = pct;
                }
            }
            Ok(())
        })
        .map_err(|e| format!("{e:?}"))?;
    if let Some(e) = err {
        return Err(Box::new(e));
    }
    // Make sure the end time is recorded even if nothing changed at it.
    w.set_time(header.end_time.max(w.current_time()))?;
    for (active, t) in blackout {
        w.blackout_at(t, active);
    }
    let _ = widths;
    Ok(())
}
