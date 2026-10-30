#[path = "../tests/common/tracker.rs"]
mod tracker_case;

use vtr::{
    Direction, SignalKind, SignalUpdate, VarType,
    attributes::Value,
    recording::{ExecutionMode, Recorder, SourceOptions, Status, Writer, WriterOptions},
    trace::Reader,
    transactions::TransactionQuery,
};

#[derive(Debug, PartialEq)]
struct Packet {
    pc: u64,
    description: String,
    data: Vec<i16>,
}
vtr::impl_payload!(Packet {
    pc,
    description,
    data
});
fn packet() -> Packet {
    Packet {
        pc: 0x8000,
        description: "packet Δ".into(),
        data: vec![-32768, 0, 32767],
    }
}

fn write(path: &str, codec: u8, workers: usize, inline: bool) -> vtr::Result<()> {
    let mut options = WriterOptions {
        mode: if inline {
            ExecutionMode::Inline
        } else {
            ExecutionMode::Threaded
        },
        ..WriterOptions::default()
    };
    options.encoding.compression.codec = vtr::Codec::from_u8(codec)?;
    options.encoding.workers = workers;
    options.encoding.block_bytes = 512;
    options.encoding.page_bytes = 128;
    options.encoding.waveform_records = 4;
    options.metadata.timescale = -9;
    let mut writer = Writer::create_with(path, options)?;
    let logger = writer.logger(SourceOptions::new("diagnostics", None))?;
    let label = writer.intern("vtr.label")?;
    let name = writer.intern("embedded")?;
    let relation = writer.intern("causes")?;
    let core = writer.scope("core", None)?;
    let requests =
        writer.transactions::<Packet>(SourceOptions::new("requests", Some(core)), "REQUEST")?;
    let scalar =
        writer.transactions::<[u64; 3]>(SourceOptions::new("scalar", Some(core)), "SCALAR")?;
    let execute = writer.stages::<String>(SourceOptions::new("execute", Some(core)))?;
    let events = writer.events::<()>(SourceOptions::new("retired", Some(core)))?;
    let logs = vtr::logging::declare::<Packet>(
        &mut writer,
        vtr::logging::CallsiteOptions::new("packet {}"),
    )?;
    let retry = vtr::logging::declare::<String>(
        &mut writer,
        vtr::logging::CallsiteOptions {
            severity: 3,
            file: Some("shared.cpp".into()),
            line: Some(17),
            ..vtr::logging::CallsiteOptions::new("retry {}")
        },
    )?;
    let memory_logger = writer.logger(SourceOptions::new("memory", None))?;
    let clock = writer.clock(SourceOptions::new("clock", Some(core)))?;
    writer.associate_clock(requests, clock)?;
    let counter = writer.signal(
        SourceOptions::new("counter", Some(core)),
        SignalKind::Bits {
            width: 32,
            states: 2,
        },
        VarType::Wire,
        Direction::Implicit,
    )?;
    writer.alias(
        counter,
        SourceOptions::new("alias", Some(core)),
        VarType::Wire,
        Direction::Implicit,
    )?;
    let logic = writer.signal(
        SourceOptions::new("logic", Some(core)),
        SignalKind::Bits {
            width: 4,
            states: 4,
        },
        VarType::Wire,
        Direction::Implicit,
    )?;
    let real = writer.signal(
        SourceOptions::new("real", Some(core)),
        SignalKind::Real,
        VarType::Real,
        Direction::Implicit,
    )?;
    let wide = writer.signal(
        SourceOptions::new("wide", Some(core)),
        SignalKind::Bits {
            width: 129,
            states: 2,
        },
        VarType::Wire,
        Direction::Implicit,
    )?;
    writer.advance_time(0)?;
    writer.clock_run(clock, 0, 2)?;
    writer.emit_batch(&[
        SignalUpdate {
            signal: counter,
            value: 0,
        },
        SignalUpdate {
            signal: counter,
            value: 1,
        },
    ])?;
    writer.emit_packed(logic, 4, &[0xe4])?;
    writer.emit_real(real, f64::from_bits(0x7ff8_0000_0000_beef))?;
    writer.emit_words(
        wide,
        129,
        &[0x1234_5678, 0x90ab_cdef, 0x7654_3210, 0xfedc_ba98, u32::MAX],
    )?;
    writer.advance_time(10)?;
    writer.emit_u64(counter, 2)?;
    writer.emit_real(real, -0.0)?;
    let owner = writer.begin_deferred(requests, None)?;
    writer.payload(owner, &packet())?;
    writer.attribute(owner.owner(), label, Value::Text("instruction Δ".into()))?;
    let visit = writer.begin_stage_deferred_at(execute, owner.owner(), 11)?;
    writer.payload(visit, &"replaced stage".to_owned())?;
    writer.embedded_event(owner.owner(), 12, name, vec![])?;
    writer.embedded_stage(owner.owner(), 100, name, 0, None, vec![])?;
    let child = writer.begin_at(requests, 12, &packet(), Some(owner.owner()))?;
    writer.payload(
        child,
        &Packet {
            pc: 1,
            description: "replaced owner".into(),
            data: vec![],
        },
    )?;
    writer.advance_time(12)?;
    writer.dump(false)?;
    writer.dump(false)?;
    writer.emit_u64(counter, 999)?;
    writer.emit_packed(logic, 4, &[0])?;
    writer.emit_real(real, 1.0)?;
    writer.emit_words(wide, 129, &[0; 5])?;
    writer.advance_time(14)?;
    writer.emit_u64(counter, 3)?;
    writer.event(events, owner.owner(), &())?;
    writer.event(events, owner.owner(), &())?;
    writer.log(logger, logs, &packet(), Some(owner.owner()))?;
    writer.clock_stop(clock, 10)?;
    writer.end_stage_with_payload(visit, &"execute Δ".to_owned())?;
    writer.advance_time(15)?;
    writer.dump(true)?;
    writer.dump(true)?;
    writer.emit_u64(counter, 2)?;
    writer.end(owner, Status::Ok)?;
    writer.relate(relation, owner.id(), child.id(), vec![])?;
    writer.advance_time(20)?;
    writer.emit_u64(counter, 4)?;
    writer.end_with_payload(child, &packet(), Status::Ok)?;
    writer.flush()?;
    for time in 21..37 {
        writer.advance_time(time)?;
        writer.emit_u64(counter, time)?;
        let tx = writer.begin_deferred(scalar, None)?;
        if time % 2 == 0 {
            writer.payload(tx, &[time, time * 2, u64::MAX])?;
            writer.end(tx, Status::Ok)?;
        } else {
            writer.end_with_payload(tx, &[time, time * 2, u64::MAX], Status::Ok)?;
        }
    }
    writer.advance_time(40)?;
    writer.log(logger, retry, &"bank 3".into(), None)?;
    writer.log(logger, retry, &"bank 3".into(), None)?;
    writer.log(memory_logger, logs, &packet(), None)?;
    writer.log(logger, logs, &packet(), None)?;
    writer.close()?;
    Ok(())
}

fn read(path: &str) -> vtr::Result<()> {
    let reader = Reader::open(path)?;
    let report = reader.validate()?;
    assert_eq!(report.records, 29);
    assert_eq!(report.relations, 1);
    assert!(report.missing.is_empty());
    let owner = reader.lookup(2)?.unwrap();
    assert_eq!(owner.decode::<Packet>()?, packet());
    assert_eq!((owner.begin(), owner.end()), (10, 15));
    assert_eq!(reader.expand(2)?.located.len(), 3);
    assert_eq!(reader.lookup(3)?.unwrap().decode::<String>()?, "execute Δ");
    assert_eq!(reader.lookup(5)?.unwrap().end(), 100);
    assert_eq!(
        reader.string(reader.lookup(5)?.unwrap().lane().unwrap())?,
        ""
    );
    assert_eq!(reader.lookup(6)?.unwrap().parent(), Some(2));
    assert_eq!(
        reader
            .lookup(9)?
            .unwrap()
            .payload()
            .field("value")?
            .decode::<Packet>()?,
        packet()
    );
    assert_eq!(
        vtr::logging::format_log(&reader, &reader.lookup(9)?.unwrap())?,
        "packet 32768"
    );
    assert_eq!(vtr::logging::callsites(&reader)?.len(), 2);
    let retry = reader.lookup(26)?.unwrap();
    let duplicate = reader.lookup(27)?.unwrap();
    assert_eq!(retry.generator(), reader.find_generator("diagnostics"));
    assert_eq!(
        retry.payload().field("value")?.decode::<String>()?,
        "bank 3"
    );
    assert_eq!(retry.datatype(), duplicate.datatype());
    assert_ne!(retry.id(), duplicate.id());
    assert_eq!(
        reader.lookup(28)?.unwrap().generator(),
        reader.find_generator("memory")
    );
    assert_eq!(
        reader.lookup(28)?.unwrap().datatype(),
        reader.lookup(9)?.unwrap().datatype()
    );
    let mut selected = 0;
    reader.records(
        vtr::RecordQuery {
            generator: retry.generator(),
            datatype: Some(retry.datatype()),
            ..Default::default()
        },
        |record| {
            assert_eq!(
                vtr::logging::format_log(&reader, &record).unwrap(),
                "retry bank 3"
            );
            selected += 1;
            true
        },
    )?;
    assert_eq!(selected, 2);
    let scalar = reader.find_generator("core.scalar").unwrap();
    let mut count = 0;
    reader.transactions(
        TransactionQuery {
            generator: Some(scalar),
            ..TransactionQuery::default()
        },
        |record| {
            let time = record.begin();
            assert_eq!(
                record.decode::<[u64; 3]>().unwrap(),
                [time, time * 2, u64::MAX]
            );
            count += 1;
            true
        },
    )?;
    assert_eq!(count, 16);
    let counter = reader.find_signal("core.counter").unwrap();
    assert_eq!(reader.find_signal("core.alias"), Some(counter));
    for (time, expected) in [
        (0, 1),
        (9, 1),
        (10, 2),
        (14, 2),
        (20, 4),
        (30, 30),
        (40, 36),
    ] {
        assert_eq!(
            reader.value_at(counter, time)?.borrow().as_u64(),
            Some(expected)
        );
    }
    assert_eq!(
        reader
            .blackouts()
            .iter()
            .map(|gap| (gap.time, gap.active))
            .collect::<Vec<_>>(),
        [(12, false), (15, true)]
    );
    let mut counter_changes = Vec::new();
    reader.changes(
        Some(counter),
        vtr::trace::TimeWindow::default(),
        |time, _, value| {
            counter_changes.push((time, value.as_u64().unwrap()));
            true
        },
    )?;
    assert_eq!(
        counter_changes,
        [(0, 0), (0, 1), (10, 2), (20, 4)]
            .into_iter()
            .chain((21..37).map(|time| (time, time)))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        reader
            .value_at(reader.find_signal("core.logic").unwrap(), 40)?
            .to_ascii(),
        "zx10"
    );
    assert!(
        matches!(reader.value_at(reader.find_signal("core.real").unwrap(), 0)?, vtr::OwnedSignalValue::Real(value) if value.to_bits() == 0x7ff8_0000_0000_beef)
    );
    assert!(
        matches!(reader.value_at(reader.find_signal("core.real").unwrap(), 10)?, vtr::OwnedSignalValue::Real(value) if value.to_bits() == (-0.0f64).to_bits())
    );
    let wide = reader.value_at(reader.find_signal("core.wide").unwrap(), 40)?;
    assert!(
        matches!(wide, vtr::OwnedSignalValue::Bits { ref data, .. } if data.len() == 17 && data[0..4] == 0x1234_5678u32.to_le_bytes() && data[16] == 1)
    );
    let payload = owner.payload();
    drop(reader);
    assert_eq!(payload.decode::<Packet>()?, packet());
    println!(
        "25 records, typed payloads, ownership, clocks, relations, waveforms and retained schemas verified"
    );
    Ok(())
}

fn index(command: &str, trace_path: &str, index_path: &str) -> vtr::Result<()> {
    let reader = Reader::open(trace_path)?;
    if command == "index-write" {
        vtr::activity::build(
            &reader,
            std::fs::File::create(index_path)?,
            &vtr::activity::BuildOptions::default(),
        )?;
    } else {
        let loaded = vtr::activity::Index::decode_for(&std::fs::read(index_path)?, &reader)?;
        let mut ranges = Vec::new();
        reader.transactions(Default::default(), |record| {
            ranges.push((record.generator().unwrap(), record.begin(), record.end()));
            true
        })?;
        reader.records(Default::default(), |record| {
            let end = if record.kind() == vtr::records::RecordKind::Stage
                && record.end() > record.begin()
            {
                record.end() - 1
            } else {
                record.end()
            };
            ranges.push((record.generator().unwrap(), record.begin(), end));
            true
        })?;
        for first in 0..=42 {
            for last in first..=42 {
                let generators: std::collections::BTreeSet<_> = ranges
                    .iter()
                    .filter(|&&(_, begin, end)| begin <= last && end >= first)
                    .map(|&(id, _, _)| id)
                    .collect();
                assert_eq!(
                    loaded.classify_generators(first, last),
                    generators.into_iter().collect::<Vec<_>>()
                );
                let mut expected = std::collections::BTreeSet::new();
                reader.changes(
                    None,
                    vtr::trace::TimeWindow {
                        first: first.max(1),
                        last: last.max(1),
                    },
                    |time, signal, _| {
                        if time >= first.max(1) && time <= last {
                            expected.insert(signal);
                        }
                        true
                    },
                )?;
                assert_eq!(
                    loaded.classify(first, last),
                    expected.into_iter().collect::<Vec<_>>()
                );
            }
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    match arguments.get(1).map(String::as_str) {
        Some(command @ ("index-write" | "index-read")) if arguments.len() == 4 => {
            index(command, &arguments[2], &arguments[3])?;
        }
        Some("recover") if arguments.len() == 4 => {
            vtr::recover(&arguments[2], &arguments[3])?;
        }
        Some("tracker-write") if arguments.len() == 4 => {
            let mode = if arguments[3] == "inline" {
                ExecutionMode::Inline
            } else {
                ExecutionMode::Threaded
            };
            tracker_case::write(std::path::Path::new(&arguments[2]), mode)?;
        }
        Some("tracker-read") if arguments.len() == 3 => {
            tracker_case::read(std::path::Path::new(&arguments[2]))?;
        }
        Some("write") if arguments.len() == 6 => write(
            &arguments[2],
            arguments[3].parse()?,
            arguments[4].parse()?,
            arguments[5] == "inline",
        )?,
        Some("read") if arguments.len() == 3 => read(&arguments[2])?,
        _ => return Err("usage: typed_interop read PATH | write PATH CODEC WORKERS MODE".into()),
    }
    Ok(())
}
