//! Kanata version 0004 instruction lifecycles, stages, labels, and wakeup relations.

use anyhow::{Context, Result, bail, ensure};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use vtr::{FileType, NodeId, TxId, TxStatus, Value, Writer};

#[derive(Default)]
struct Stage {
    name: String,
    lane: String,
    begin: u64,
    end: Option<u64>,
    label: String,
}
struct Op {
    tx: TxId,
    label: String,
    detail: String,
    retired: Option<(u64, i64, bool)>,
    stages: Vec<Stage>,
}
fn append(dst: &mut String, text: &str) {
    if !dst.is_empty() {
        dst.push('\n');
    }
    dst.push_str(text);
}

pub fn convert_kanata(
    input: impl AsRef<Path>,
    w: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    let input = input.as_ref();
    let file = std::fs::File::open(input).with_context(|| input.display().to_string())?;
    let length = file.metadata()?.len();
    convert_input(file, length, w, progress)
}

pub fn convert_input(
    input: impl Read + 'static,
    length: u64,
    w: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    let counter = progress.begin(vtr_progress::Phase::Reading, Some(length));
    let mut reader = BufReader::new(vtr_progress::CountingReader::new(input, counter));
    let compressed = reader.fill_buf()?.starts_with(&[0x1f, 0x8b]);
    let reader: Box<dyn Read> = if compressed {
        Box::new(flate2::read::MultiGzDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    convert_reader(BufReader::new(reader), w, progress)
}

fn convert_reader(
    reader: impl BufRead,
    w: &mut Writer,
    progress: &vtr_progress::Reporter,
) -> Result<()> {
    let mut lines = reader.lines();
    ensure!(
        lines.next().transpose()?.as_deref() == Some("Kanata\t0004"),
        "expected Kanata version 0004 header"
    );
    w.set_timescale(vtr::Timescale::Seconds)?;
    w.set_file_type(FileType::Architectural)?;
    let unit = w.intern("cycle");
    w.set_file_attr("time.unit", Value::Str(unit))?;
    let version = w.intern("0004");
    w.set_file_attr("kanata.version", Value::Str(version))?;
    let core = w.add_scope(None, "cpu", vtr::ScopeType::Core, "")?;
    let clock = w.add_clock(Some(core), "cycle")?;
    let clock_path = w.intern("cpu.cycle");
    let mut clock_running = false;
    let mut threads: HashMap<u64, NodeId> = HashMap::new();
    let mut ops: BTreeMap<u64, Op> = BTreeMap::new();
    let mut cycle = 0u64;
    let mut start_set = false;
    for (line_index, line) in lines.enumerate() {
        let line = line?;
        let fields: Vec<_> = line.split('\t').collect();
        let parse = |w: &mut Writer, ops: &mut BTreeMap<u64, Op>| -> Result<()> {
            let arg = |i: usize| fields.get(i).copied().context("missing argument");
            let num = |i| -> Result<u64> { Ok(arg(i)?.parse()?) };
            let cmd = fields[0];
            let expected = match cmd {
                "" => return Ok(()),
                "C" | "C=" => 2,
                "I" | "L" | "S" | "E" | "R" | "W" => 4,
                _ => bail!("unknown command {cmd:?}"),
            };
            ensure!(
                fields.len() == expected,
                "{cmd} expects {} arguments",
                expected - 1
            );
            match cmd {
                "C" | "C=" => {
                    let n = num(1)?;
                    let next = if cmd == "C" {
                        cycle.checked_add(n).context("cycle overflow")?
                    } else {
                        n
                    };
                    ensure!(next >= cycle, "cycles must not go backwards");
                    cycle = next;
                    if !start_set {
                        w.set_file_attr("kanata.start_cycle", Value::U64(cycle))?;
                        start_set = true;
                    }
                    if !clock_running {
                        w.clock_run(clock, cycle, 1)?;
                        clock_running = true;
                    }
                }
                "I" => {
                    let id = num(1)?;
                    let gid: i64 = arg(2)?.parse()?;
                    let tid = num(3)?;
                    ensure!(!ops.contains_key(&id), "duplicate instruction {id}");
                    let r#gen = match threads.entry(tid) {
                        std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                        std::collections::hash_map::Entry::Vacant(e) => {
                            let generator =
                                w.add_generator(Some(core), &format!("thread{tid}"), "PIPELINE")?;
                            w.node_attr(generator, vtr::clock::KEY_CLOCK, Value::Str(clock_path))?;
                            *e.insert(generator)
                        }
                    };
                    if !start_set {
                        w.set_file_attr("kanata.start_cycle", Value::U64(cycle))?;
                        start_set = true;
                    }
                    if !clock_running {
                        w.clock_run(clock, cycle, 1)?;
                        clock_running = true;
                    }
                    let tx = w.begin_tx(r#gen, cycle)?;
                    let key = w.intern("insn_id_in_sim");
                    w.tx_attr(tx, key, &Value::I64(gid))?;
                    let key = w.intern("line");
                    w.tx_attr(tx, key, &Value::U64(line_index as u64 + 2))?;
                    ops.insert(
                        id,
                        Op {
                            tx,
                            label: String::new(),
                            detail: String::new(),
                            retired: None,
                            stages: Vec::new(),
                        },
                    );
                }
                "W" => {
                    let consumer = ops.get(&num(1)?).context("unknown consumer instruction")?;
                    let producer = ops.get(&num(2)?).context("unknown producer instruction")?;
                    let kind = num(3)?;
                    let attrs = if kind == 0 {
                        vec![]
                    } else {
                        vec![(w.intern("type"), Value::U64(kind))]
                    };
                    let key = w.intern("wakeup");
                    w.relate(key, producer.tx, consumer.tx, &attrs)?;
                }
                _ => {
                    let id = num(1)?;
                    let op = ops
                        .get_mut(&id)
                        .with_context(|| format!("unknown instruction {id}"))?;
                    match cmd {
                        "L" => {
                            let text = arg(3)?.replace("\\n", "\n");
                            match num(2)? {
                                0 => append(&mut op.label, &text),
                                1 => append(&mut op.detail, &text),
                                2 => append(
                                    &mut op
                                        .stages
                                        .last_mut()
                                        .context("stage label without a stage")?
                                        .label,
                                    &text,
                                ),
                                n => bail!("unknown label type {n}"),
                            }
                        }
                        "S" | "E" => {
                            ensure!(op.retired.is_none(), "stage after retirement");
                            let lane = arg(2)?;
                            let name = arg(3)?;
                            ensure!(!lane.is_empty() && !name.is_empty(), "empty stage or lane");
                            let active = op
                                .stages
                                .iter_mut()
                                .rev()
                                .find(|s| s.lane == lane && s.name == name && s.end.is_none());
                            if cmd == "S" {
                                ensure!(active.is_none(), "stage already open on this lane");
                                if let Some(previous) = op
                                    .stages
                                    .iter_mut()
                                    .rev()
                                    .find(|s| s.lane == lane && s.end.is_none())
                                {
                                    previous.end = Some(cycle);
                                }
                                op.stages.push(Stage {
                                    name: name.into(),
                                    lane: lane.into(),
                                    begin: cycle,
                                    ..Default::default()
                                });
                            } else {
                                active.context("ending a stage that is not open")?.end =
                                    Some(cycle);
                            }
                        }
                        "R" => {
                            ensure!(op.retired.is_none(), "instruction retired twice");
                            let rid = arg(2)?.parse::<i64>()?;
                            let flushed = num(3)?;
                            ensure!(flushed <= 1, "retirement type must be 0 or 1");
                            op.retired = Some((cycle, rid, flushed == 1));
                        }
                        _ => unreachable!(),
                    }
                }
            }
            Ok(())
        };
        let mut parse = parse;
        parse(w, &mut ops).with_context(|| format!("Kanata line {}", line_index + 2))?;
    }
    w.set_time(cycle)?;
    let counter = progress.begin(vtr_progress::Phase::Writing, Some(ops.len() as u64));
    for op in ops.values() {
        for (key, text) in [("vtr.label", &op.label), ("detail", &op.detail)] {
            if !text.is_empty() {
                let key = w.intern(key);
                let value = Value::Str(w.intern(text));
                w.tx_attr(op.tx, key, &value)?;
            }
        }
        for stage in &op.stages {
            let name = w.intern(&stage.name);
            let lane = w.intern(&stage.lane);
            let attrs = if stage.label.is_empty() {
                vec![]
            } else {
                vec![(w.intern("vtr.label"), Value::Str(w.intern(&stage.label)))]
            };
            w.tx_stage(
                op.tx,
                name,
                lane,
                stage.begin,
                stage.end.unwrap_or(op.retired.map_or(cycle, |r| r.0)),
                &attrs,
            )?;
        }
        if let Some((end, rid, flushed)) = op.retired {
            let key = w.intern("retire_id");
            w.tx_attr(op.tx, key, &Value::I64(rid))?;
            w.end_tx(
                op.tx,
                end,
                if flushed {
                    TxStatus::Aborted
                } else {
                    TxStatus::Unset
                },
            )?;
        }
        counter.advance(1);
    }
    Ok(())
}
