//! VCD export of VTR waveforms.

use anyhow::{Result, bail, ensure};
use std::io::{self, Write};
use vtr::{NodeData, NodeId, Reader, ScopeType, SignalKind, SignalValue};

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

fn timescale_line(exp: i8) -> Result<String> {
    ensure!(
        (-15..=2).contains(&exp),
        "VCD requires a timescale between 1 fs and 100 s"
    );
    let units = [
        (0, "s"),
        (-3, "ms"),
        (-6, "us"),
        (-9, "ns"),
        (-12, "ps"),
        (-15, "fs"),
    ];
    let (base, unit) = units.into_iter().find(|(base, _)| exp >= *base).unwrap();
    Ok(format!(
        "$timescale {}{unit} $end",
        10u32.pow((exp - base) as u32)
    ))
}

fn scope_keyword(t: ScopeType) -> &'static str {
    match t {
        ScopeType::Task => "task",
        ScopeType::Function => "function",
        ScopeType::Begin => "begin",
        ScopeType::Fork => "fork",
        _ => "module",
    }
}

pub fn write_vcd(r: &Reader, mut w: impl Write) -> Result<u64> {
    ensure!(
        r.meta().time_zero == 0,
        "VCD export does not support time-zero offsets"
    );
    ensure!(
        r.blackout().is_empty(),
        "VCD export does not support blackout intervals"
    );
    let h = r.hierarchy();
    for signal in &h.signals {
        match signal {
            SignalKind::Bits { states: 9, .. } | SignalKind::VarLen => {
                bail!("VCD export supports only 2/4-state bits, events, and reals")
            }
            _ => {}
        }
    }
    for id in h.ids() {
        if matches!(h.kind(id), vtr::NodeKind::Scope | vtr::NodeKind::Var) {
            let name = r.name(id);
            let parts: Vec<_> = name.split_whitespace().collect();
            ensure!(
                !name.contains("$end")
                    && !parts.is_empty()
                    && (parts.len() == 1
                        || (parts.len() == 2
                            && parts[1].starts_with('[')
                            && parts[1].ends_with(']')
                            && h.kind(id) == vtr::NodeKind::Var)),
                "name {name:?} cannot be represented in VCD"
            );
        }
    }
    writeln!(w, "$version Volna vtr $end")?;
    writeln!(w, "{}", timescale_line(r.meta().timescale)?)?;
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
            NodeData::Var {
                signal, var_type, ..
            } => {
                let (kw, width) = match h.signal_kind(signal) {
                    Some(SignalKind::Bits { width, .. }) => (
                        if var_type == vtr::VarType::Event {
                            "event"
                        } else {
                            "wire"
                        },
                        width,
                    ),
                    Some(SignalKind::Real) => ("real", 64),
                    _ => ("string", 1),
                };
                writeln!(
                    w,
                    "$var {kw} {width} {} {} $end",
                    id_code(signal.0 as usize),
                    r.name(n)
                )?;
            }
            _ => {}
        }
        Ok(())
    }
    for root in h.roots() {
        walk(r, root, &mut w)?;
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
    if let Some((_, end)) = r.time_range()
        && last != end
    {
        writeln!(w, "#{end}")?;
    }
    w.flush()?;
    Ok(n)
}
