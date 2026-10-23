//! Streaming VCD conversion with checked identifiers, aliases, and time ordering.

use anyhow::{Context, Result, bail, ensure};
use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;
use vcd::{Command, IdCode, ScopeItem, SimulationCommand};
use vtr::recording::{Recorder, SourceOptions, Writer};
use vtr::{Direction, LogicStates, NodeId, ScopeType, SignalId, SignalKind, VarType};

#[derive(Clone, Copy)]
struct Signal {
    id: SignalId,
    kind: SignalKind,
    event: bool,
}

fn map_var(t: vcd::VarType) -> Result<VarType> {
    use vcd::VarType as V;
    Ok(match t {
        V::Event => VarType::Event,
        V::Integer => VarType::Integer,
        V::Parameter => VarType::Parameter,
        V::Real => VarType::Real,
        V::Reg => VarType::Reg,
        V::Supply0 => VarType::Supply0,
        V::Supply1 => VarType::Supply1,
        V::Time => VarType::Time,
        V::Tri => VarType::Tri,
        V::TriAnd => VarType::TriAnd,
        V::TriOr => VarType::TriOr,
        V::TriReg => VarType::TriReg,
        V::Tri0 => VarType::Tri0,
        V::Tri1 => VarType::Tri1,
        V::WAnd => VarType::WAnd,
        V::Wire => VarType::Wire,
        V::WOr => VarType::WOr,
        V::String => VarType::String,
        _ => bail!("unsupported VCD variable type {t}"),
    })
}

fn declarations(
    w: &mut Writer,
    items: &[ScopeItem],
    parent: Option<NodeId>,
    signals: &mut HashMap<IdCode, Signal>,
) -> Result<()> {
    for item in items {
        match item {
            ScopeItem::Scope(scope) => {
                let kind = match scope.scope_type {
                    vcd::ScopeType::Module => ScopeType::Module,
                    vcd::ScopeType::Task => ScopeType::Task,
                    vcd::ScopeType::Function => ScopeType::Function,
                    vcd::ScopeType::Begin => ScopeType::Begin,
                    vcd::ScopeType::Fork => ScopeType::Fork,
                    _ => bail!("unsupported VCD scope type"),
                };
                let node = w.scope_with(SourceOptions::new(&scope.identifier, parent), kind, "")?;
                declarations(w, &scope.items, Some(node), signals)?;
            }
            ScopeItem::Var(var) => {
                let var_type = map_var(var.var_type)?;
                let kind = match var_type {
                    VarType::Real => SignalKind::Real,
                    VarType::String => SignalKind::VarLen,
                    _ => {
                        ensure!(var.size > 0, "variable {} has zero width", var.reference);
                        ensure!(
                            var_type != VarType::Event || var.size == 1,
                            "event {} must have width 1",
                            var.reference
                        );
                        SignalKind::bits(var.size, LogicStates::Four)
                    }
                };
                let name = match var.index {
                    Some(index) => format!("{} {index}", var.reference),
                    None => var.reference.clone(),
                };
                let event = var_type == VarType::Event;
                match signals.entry(var.code) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        ensure!(
                            e.get().kind == kind && e.get().event == event,
                            "alias {name:?} has an incompatible type or width"
                        );
                        w.alias(
                            e.get().id,
                            SourceOptions::new(&name, parent),
                            var_type,
                            Direction::Implicit,
                        )?;
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let id = w.signal(
                            SourceOptions::new(&name, parent),
                            kind,
                            var_type,
                            Direction::Implicit,
                        )?;
                        e.insert(Signal { id, kind, event });
                    }
                }
            }
            ScopeItem::Comment(_) => {}
            _ => bail!("unsupported VCD declaration"),
        }
    }
    Ok(())
}

pub fn convert_vcd(
    input: impl AsRef<Path>,
    w: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    let file = std::fs::File::open(input)?;
    let length = file.metadata()?.len();
    convert_input(file, length, w, progress)
}

pub fn convert_input(
    input: impl std::io::Read,
    length: u64,
    w: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    let counter = progress.begin(vtr_progress::Phase::Reading, Some(length));
    let mut parser = vcd::Parser::new(BufReader::new(vtr_progress::CountingReader::new(
        input, counter,
    )));
    let header = parser.parse_header()?;
    let (factor, unit) = header.timescale.unwrap_or((1, vcd::TimescaleUnit::NS));
    let base = match unit {
        vcd::TimescaleUnit::S => 0,
        vcd::TimescaleUnit::MS => -3,
        vcd::TimescaleUnit::US => -6,
        vcd::TimescaleUnit::NS => -9,
        vcd::TimescaleUnit::PS => -12,
        vcd::TimescaleUnit::FS => -15,
    };
    let exponent = base
        + match factor {
            1 => 0,
            10 => 1,
            100 => 2,
            _ => bail!("VCD timescale factor must be 1, 10, or 100"),
        };
    let mut metadata = w.metadata()?;
    metadata.timescale = exponent;
    if let Some(date) = header.date {
        metadata.date = date;
    }
    if let Some(version) = header.version {
        let key = w.intern("vcd.version")?;
        metadata.attributes.push(vtr::attributes::Attribute {
            key,
            value: vtr::attributes::Value::Text(version),
        });
    }
    w.set_metadata(metadata)?;
    let mut signals = HashMap::new();
    declarations(w, &header.items, None, &mut signals)?;
    let signal = |code: IdCode| {
        signals
            .get(&code)
            .copied()
            .with_context(|| format!("undeclared VCD identifier {code}"))
    };
    let mut simulation = None;
    while let Some(command) = parser.next() {
        let line = parser.line();
        let result = (|| -> Result<()> {
            match command? {
                Command::Timestamp(time) => w.advance_time(time)?,
                Command::ChangeScalar(code, value) => {
                    let signal = signal(code)?;
                    ensure!(
                        matches!(signal.kind, SignalKind::Bits { width: 1, .. }),
                        "scalar value on non-scalar {code}"
                    );
                    emit_logic(w, signal, value.to_string().as_bytes())?;
                }
                Command::ChangeVector(code, value) => {
                    let signal = signal(code)?;
                    let SignalKind::Bits { width, .. } = signal.kind else {
                        bail!("vector value on non-vector {code}");
                    };
                    ensure!(
                        value.len() > 0 && value.len() <= width as usize,
                        "vector value exceeds declared width for {code}"
                    );
                    emit_logic(w, signal, value.to_string().as_bytes())?;
                }
                Command::ChangeReal(code, value) => w.emit_real(signal(code)?.id, value)?,
                Command::ChangeString(code, value) => {
                    w.emit_bytes(signal(code)?.id, value.as_bytes())?;
                }
                Command::Begin(kind) => {
                    ensure!(simulation.is_none(), "nested VCD dump command");
                    simulation = Some(kind);
                    match kind {
                        SimulationCommand::Dumpoff => w.blackout(w.current_time(), false)?,
                        SimulationCommand::Dumpon => w.blackout(w.current_time(), true)?,
                        _ => {}
                    }
                }
                Command::End(kind) => {
                    ensure!(
                        simulation.take() == Some(kind),
                        "unmatched VCD dump command end"
                    );
                }
                Command::Comment(_) => {}
                other => bail!("unexpected command in VCD body: {other:?}"),
            }
            Ok(())
        })();
        result.with_context(|| format!("VCD line {line}"))?;
    }
    ensure!(simulation.is_none(), "unterminated VCD dump command");
    Ok(())
}

fn emit_logic(writer: &mut Writer, signal: Signal, text: &[u8]) -> Result<()> {
    let SignalKind::Bits { width, states } = signal.kind else {
        bail!("logic value on non-logic signal");
    };
    ensure!(
        !text.is_empty() && text.len() <= width as usize,
        "invalid VCD logic width"
    );
    let mut bytes = vec![0; vtr::signal::packed_len(width, states)];
    vtr::signal::pack_ascii(text, width, states, &mut bytes);
    writer.emit_packed(signal.id, states, &bytes)?;
    Ok(())
}
