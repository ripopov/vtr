//! VCD (and any other wellen-supported format) -> VTR via the wellen reader.
//! Not bit-exact for FST metadata (use the native FST path for that).

use std::collections::HashMap;
use vtr::{Direction, ScopeType, SignalId, SignalKind, VarType, Writer};
use wellen::{ScopeRef, SignalRef, VarRef};

fn map_var(t: wellen::VarType) -> VarType {
    use wellen::VarType as W;
    match t {
        W::Event => VarType::Event,
        W::Integer => VarType::Integer,
        W::Parameter => VarType::Parameter,
        W::Real => VarType::Real,
        W::Reg => VarType::Reg,
        W::Supply0 => VarType::Supply0,
        W::Supply1 => VarType::Supply1,
        W::Time => VarType::Time,
        W::Tri => VarType::Tri,
        W::TriAnd => VarType::TriAnd,
        W::TriOr => VarType::TriOr,
        W::TriReg => VarType::TriReg,
        W::Tri0 => VarType::Tri0,
        W::Tri1 => VarType::Tri1,
        W::WAnd => VarType::WAnd,
        W::Wire => VarType::Wire,
        W::WOr => VarType::WOr,
        W::String => VarType::String,
        W::Port => VarType::Port,
        W::SparseArray => VarType::SparseArray,
        W::RealTime => VarType::RealTime,
        W::Bit => VarType::Bit,
        W::Logic => VarType::Logic,
        W::Int => VarType::Int,
        W::ShortInt => VarType::ShortInt,
        W::LongInt => VarType::LongInt,
        W::Byte => VarType::Byte,
        W::Enum => VarType::Enum,
        W::ShortReal => VarType::ShortReal,
        W::RealParameter => VarType::RealParameter,
        W::EventParameter => VarType::Event,
        W::Boolean => VarType::Other(100),
        W::BitVector => VarType::Other(101),
        W::StdLogic => VarType::Other(102),
        W::StdLogicVector => VarType::Other(103),
        W::StdULogic => VarType::Other(104),
        W::StdULogicVector => VarType::Other(105),
    }
}

fn map_scope(t: wellen::ScopeType) -> ScopeType {
    use wellen::ScopeType as W;
    match t {
        W::Module => ScopeType::Module,
        W::Task => ScopeType::Task,
        W::Function => ScopeType::Function,
        W::Begin => ScopeType::Begin,
        W::Fork => ScopeType::Fork,
        W::Generate => ScopeType::Generate,
        W::Struct => ScopeType::Struct,
        W::Union => ScopeType::Union,
        W::Class => ScopeType::Class,
        W::Interface => ScopeType::Interface,
        W::Package => ScopeType::Package,
        W::Program => ScopeType::Program,
        W::VhdlArchitecture => ScopeType::VhdlArchitecture,
        W::VhdlProcedure => ScopeType::VhdlProcedure,
        W::VhdlFunction => ScopeType::VhdlFunction,
        W::VhdlRecord => ScopeType::VhdlRecord,
        W::VhdlProcess => ScopeType::VhdlProcess,
        W::VhdlBlock => ScopeType::VhdlBlock,
        W::VhdlForGenerate => ScopeType::VhdlForGenerate,
        W::VhdlIfGenerate => ScopeType::VhdlIfGenerate,
        W::VhdlGenerate => ScopeType::VhdlGenerate,
        W::VhdlPackage => ScopeType::VhdlPackage,
        W::GhwGeneric => ScopeType::Generic,
        W::VhdlArray => ScopeType::SvArray,
        W::Unknown => ScopeType::Generic,
        _ => ScopeType::Generic,
    }
}

fn map_dir(d: wellen::VarDirection) -> Direction {
    use wellen::VarDirection as W;
    match d {
        W::Unknown => Direction::Implicit,
        W::Implicit => Direction::Implicit,
        W::Input => Direction::Input,
        W::Output => Direction::Output,
        W::InOut => Direction::InOut,
        W::Buffer => Direction::Buffer,
        W::Linkage => Direction::Linkage,
    }
}

pub fn convert_wellen(input: &str, w: &mut Writer, states: u8) -> Result<(), Box<dyn std::error::Error>> {
    let wave = wellen::simple::read(input).map_err(|e| format!("{e}"))?;
    let h = wave.hierarchy();
    let ts = h.timescale();
    let exp = match ts {
        Some(t) => {
            let base = match t.unit {
                wellen::TimescaleUnit::ZeptoSeconds => -21,
                wellen::TimescaleUnit::AttoSeconds => -18,
                wellen::TimescaleUnit::FemtoSeconds => -15,
                wellen::TimescaleUnit::PicoSeconds => -12,
                wellen::TimescaleUnit::NanoSeconds => -9,
                wellen::TimescaleUnit::MicroSeconds => -6,
                wellen::TimescaleUnit::MilliSeconds => -3,
                wellen::TimescaleUnit::Seconds => 0,
                wellen::TimescaleUnit::Unknown => -9,
            };
            let f = match t.factor {
                10 => 1,
                100 => 2,
                _ => 0,
            };
            base + f
        }
        None => -9,
    };
    w.set_timescale(exp)?;
    let mut sigs: HashMap<SignalRef, SignalId> = HashMap::new();
    let mut kinds: HashMap<SignalRef, SignalKind> = HashMap::new();
    fn walk(
        w: &mut Writer,
        h: &wellen::Hierarchy,
        vars: impl Iterator<Item = VarRef>,
        scopes: impl Iterator<Item = ScopeRef>,
        sigs: &mut HashMap<SignalRef, SignalId>,
        kinds: &mut HashMap<SignalRef, SignalKind>,
        states: u8,
    ) -> vtr::Result<()> {
        for v in vars {
            let var = &h[v];
            let name = var.name(h);
            let r = var.signal_ref();
            if let Some(&s) = sigs.get(&r) {
                w.add_alias(name, map_var(var.var_type()), map_dir(var.direction()), s)?;
                continue;
            }
            let kind = match var.signal_encoding(h) {
                wellen::SignalEncoding::Real => SignalKind::Real,
                wellen::SignalEncoding::String => SignalKind::VarLen,
                wellen::SignalEncoding::BitVector(n) => SignalKind::Bits { width: n, states },
            };
            let (_, s) = w.add_var(name, map_var(var.var_type()), map_dir(var.direction()), kind);
            sigs.insert(r, s);
            kinds.insert(r, kind);
        }
        for sc in scopes {
            let scope = &h[sc];
            w.begin_scope(scope.name(h), map_scope(scope.scope_type()), scope.component(h).unwrap_or(""));
            walk(w, h, scope.vars(h), scope.scopes(h), sigs, kinds, states)?;
            w.end_scope()?;
        }
        Ok(())
    }
    walk(w, h, h.vars(), h.scopes(), &mut sigs, &mut kinds, states)?;
    let mut wave = wave;
    let all: Vec<SignalRef> = sigs.keys().copied().collect();
    wave.load_signals(&all);
    let tt = wave.time_table().to_vec();
    // k-way merge by time index.
    struct Cur {
        r: SignalRef,
        i: usize,
    }
    let mut heap = std::collections::BinaryHeap::new();
    let mut curs: Vec<Cur> = Vec::new();
    for r in &all {
        let s = wave.get_signal(*r).unwrap();
        if !s.time_indices().is_empty() {
            heap.push(std::cmp::Reverse((s.time_indices()[0], curs.len())));
            curs.push(Cur { r: *r, i: 0 });
        }
    }
    let mut buf = String::new();
    while let Some(std::cmp::Reverse((tidx, ci))) = heap.pop() {
        let cur = &mut curs[ci];
        let s = wave.get_signal(cur.r).unwrap();
        let sig = sigs[&cur.r];
        let t = tt[tidx as usize];
        w.set_time(t)?;
        let ti = s.time_indices();
        // All entries with this time index.
        let off = s.get_offset(tidx).unwrap();
        for e in 0..off.elements {
            let v = s.get_value_at(&off, e);
            match v {
                wellen::SignalValueRef::Real(f) => w.emit_real(sig, f)?,
                wellen::SignalValueRef::String(st) => w.emit_varlen(sig, st.as_bytes())?,
                wellen::SignalValueRef::Event => w.emit_bit(sig, 1)?,
                other => {
                    buf.clear();
                    if let Some(bs) = other.to_bit_string() {
                        buf.push_str(&bs);
                    }
                    w.emit_logic_str(sig, buf.as_bytes())?;
                }
            }
        }
        cur.i += off.elements as usize;
        if cur.i < ti.len() {
            heap.push(std::cmp::Reverse((ti[cur.i], ci)));
        }
    }
    if let Some(&last) = tt.last() {
        w.set_time(last)?;
    }
    Ok(())
}
