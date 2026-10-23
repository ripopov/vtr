//! VCD export of VTR waveforms.

use anyhow::{Result, bail, ensure};
use std::io::{self, Write};
use vtr::{
    ScopeType, SignalKind, SignalValue, VarType,
    declarations::NodeData,
    trace::{Reader, TimeWindow},
};

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
        r.metadata().time_zero == 0,
        "VCD export does not support time-zero offsets"
    );
    ensure!(
        r.blackouts().is_empty(),
        "VCD export does not support blackout intervals"
    );
    let h = r.declarations();
    for signal in &h.signals {
        match signal {
            SignalKind::Bits { states: 9, .. } | SignalKind::VarLen => {
                bail!("VCD export supports only 2/4-state bits, events, and reals")
            }
            _ => {}
        }
    }
    for node in &h.nodes {
        if matches!(node.data, NodeData::Scope { .. } | NodeData::Signal { .. }) {
            let name = r.string(node.name)?;
            let parts: Vec<_> = name.split_whitespace().collect();
            ensure!(
                !name.contains("$end")
                    && !parts.is_empty()
                    && (parts.len() == 1
                        || (parts.len() == 2
                            && parts[1].starts_with('[')
                            && parts[1].ends_with(']')
                            && matches!(node.data, NodeData::Signal { .. }))),
                "name {name:?} cannot be represented in VCD"
            );
        }
    }
    writeln!(w, "$version Volna vtr $end")?;
    writeln!(w, "{}", timescale_line(r.metadata().timescale)?)?;
    let mut pending: Vec<_> = r.children(None).iter().rev().copied().map(Some).collect();
    while let Some(entry) = pending.pop() {
        let Some(id) = entry else {
            writeln!(w, "$upscope $end")?;
            continue;
        };
        let node = r.node(id)?;
        let name = r.name(id)?;
        match node.data {
            NodeData::Scope { scope_type, .. } => {
                writeln!(
                    w,
                    "$scope {} {name} $end",
                    scope_keyword(ScopeType::from_code(scope_type))
                )?;
                pending.push(None);
                pending.extend(r.children(Some(id)).iter().rev().copied().map(Some));
            }
            NodeData::Signal {
                signal, var_type, ..
            } => {
                let (keyword, width) = match r.signal_kind(signal)? {
                    SignalKind::Bits { width, .. } => (
                        if var_type == VarType::Event.code() {
                            "event"
                        } else {
                            "wire"
                        },
                        width,
                    ),
                    SignalKind::Real => ("real", 64),
                    SignalKind::VarLen => unreachable!("validated VCD signal kinds"),
                };
                writeln!(
                    w,
                    "$var {keyword} {width} {} {name} $end",
                    id_code(signal.0 as usize)
                )?;
            }
            _ => (),
        }
    }
    writeln!(w, "$enddefinitions $end")?;
    let ids: Vec<String> = (0..h.signals.len()).map(id_code).collect();
    let mut last = u64::MAX;
    let mut n = 0u64;
    let mut err = None;
    r.ordered_changes(TimeWindow::default(), |t, s, v| {
        if err.is_some() {
            return false;
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
        true
    })?;
    if let Some(e) = err {
        return Err(e.into());
    }
    if let Some(window) = r.waveform_time_range()
        && last != window.last
    {
        writeln!(w, "#{}", window.last)?;
    }
    w.flush()?;
    Ok(n)
}
