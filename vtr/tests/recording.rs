use vtr::{
    Compression, Direction, SignalKind, VarType,
    attributes::{Attribute, Value},
    framing::Completion,
    recording::{ExecutionMode, Recorder, SourceOptions, Status, Writer, WriterOptions},
    records::{RecordKind, RecordQuery},
    trace::{Reader, TimeWindow},
    transactions::TransactionQuery,
};

fn options(mode: ExecutionMode, compression: Compression) -> WriterOptions {
    let mut options = WriterOptions {
        mode,
        ..WriterOptions::default()
    };
    options.encoding.compression = compression;
    options.encoding.block_bytes = 512;
    options.encoding.page_bytes = 128;
    options.encoding.waveform_records = 4;
    options
}

#[test]
fn impossible_capture_ring_budgets_fail_before_creating_an_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized-ring.vtr");
    for bytes in [usize::MAX, isize::MAX as usize, isize::MAX as usize / 2] {
        let mut config = options(ExecutionMode::Threaded, Compression::LZ4);
        config.buffer.queue_bytes = bytes;
        assert!(Writer::create_with(&path, config).is_err());
        assert!(!path.exists());
    }
}

#[test]
fn stale_handles_cannot_address_a_new_recording_with_the_same_source_and_record_ids() {
    let directory = tempfile::tempdir().unwrap();
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let path = directory.path().join(format!("stale-{mode:?}.vtr"));
        let (stale_source, stale_record) = {
            let mut writer = Writer::create_with(&path, options(mode, Compression::LZ4)).unwrap();
            let source = writer
                .transactions::<u64>(SourceOptions::new("requests", None), "request")
                .unwrap();
            let record = writer.begin(source, &1, None).unwrap();
            writer.end(record, Status::Ok).unwrap();
            writer.close().unwrap();
            (source, record)
        };
        let mut writer = Writer::create_with(&path, options(mode, Compression::LZ4)).unwrap();
        let source = writer
            .transactions::<u64>(SourceOptions::new("requests", None), "request")
            .unwrap();
        assert_eq!(source.id(), stale_source.id());
        let record = writer.begin(source, &2, None).unwrap();
        assert_eq!(record.id(), stale_record.id());
        assert!(writer.begin(stale_source, &3, None).is_err());
        assert!(
            writer
                .begin(source, &3, Some(stale_record.owner()))
                .is_err()
        );
        assert!(writer.end(stale_record, Status::Ok).is_err());
        writer.end(record, Status::Ok).unwrap();
        let identity = writer.identity();
        writer.close().unwrap();
        let reader = Reader::open(&path).unwrap();
        reader.validate().unwrap();
        assert_eq!(reader.identity(), identity);
        let mut values = Vec::new();
        reader
            .transactions(TransactionQuery::default(), |record| {
                values.push(record.decode::<u64>().unwrap());
                true
            })
            .unwrap();
        assert_eq!(values, [2]);
    }
}

#[test]
fn typed_recording_round_trips_every_codec_mode_and_lifetime() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("trace.vtr");
            let mut writer = Writer::create_with(&path, options(mode, compression)).unwrap();
            let label = writer.intern("vtr.label").unwrap();
            let event_name = writer.intern("embedded").unwrap();
            let lane = writer.intern("lane").unwrap();
            let relation = writer.intern("causes").unwrap();
            let core = writer.scope("core", None).unwrap();
            let source = writer
                .transactions::<[u64; 2]>(SourceOptions::new("requests", Some(core)), "REQUEST")
                .unwrap();
            let stages = writer
                .stages::<Vec<String>>(SourceOptions::new("execute", Some(core)))
                .unwrap();
            let events = writer
                .events::<()>(SourceOptions::new("retire", Some(core)))
                .unwrap();
            let logs = writer
                .logs::<Vec<String>>(SourceOptions::new("result {}", Some(core)))
                .unwrap();
            let clock = writer
                .clock(SourceOptions::new("clock", Some(core)))
                .unwrap();
            writer.associate_clock(source, clock).unwrap();
            let signal = writer
                .signal(
                    SourceOptions::new("counter", Some(core)),
                    SignalKind::Bits {
                        width: 8,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Output,
                )
                .unwrap();
            writer
                .alias(
                    signal,
                    SourceOptions::new("alias", Some(core)),
                    VarType::Wire,
                    Direction::Output,
                )
                .unwrap();
            let packed = writer
                .signal(
                    SourceOptions::new("logic", Some(core)),
                    SignalKind::Bits {
                        width: 4,
                        states: 4,
                    },
                    VarType::Wire,
                    Direction::Input,
                )
                .unwrap();
            let real = writer
                .signal(
                    SourceOptions::new("real", Some(core)),
                    SignalKind::Real,
                    VarType::Real,
                    Direction::Implicit,
                )
                .unwrap();
            let pulse = writer
                .signal(
                    SourceOptions::new("pulse", Some(core)),
                    SignalKind::Bits {
                        width: 1,
                        states: 2,
                    },
                    VarType::Event,
                    Direction::Implicit,
                )
                .unwrap();
            writer.advance_time(0).unwrap();
            writer.clock_run(clock, 0, 2).unwrap();
            writer.emit_u64(signal, 1).unwrap();
            writer.emit_packed(packed, 4, &[0xe4]).unwrap();
            writer
                .emit_real(real, f64::from_bits(0x7ff8_0000_0000_beef))
                .unwrap();
            writer.emit_u64(pulse, 1).unwrap();
            writer.emit_u64(pulse, 1).unwrap();
            writer.advance_time(10).unwrap();
            let owner = writer.begin(source, &[u64::MAX, 42], None).unwrap();
            writer
                .attribute(owner.owner(), label, Value::Text("instruction".into()))
                .unwrap();
            let embedded = writer
                .embedded_stage(owner.owner(), 100, event_name, lane, None, vec![])
                .unwrap();
            let child = writer
                .begin_at(source, 12, &[1, 2], Some(owner.owner()))
                .unwrap();
            let visit = writer
                .begin_stage_at(stages, owner.owner(), 11, &vec!["borrowed Δ".into()])
                .unwrap();
            writer.emit_u64(signal, 2).unwrap();
            writer.clock_stop(clock, 10).unwrap();
            writer.advance_time(14).unwrap();
            writer.end_stage(visit).unwrap();
            let first = writer.event(events, owner.owner(), &()).unwrap();
            let second = writer.event(events, owner.owner(), &()).unwrap();
            assert_ne!(first, second);
            let mut message = vec!["copied".to_owned()];
            let log = writer.log(logs, &message, Some(owner.owner())).unwrap();
            message[0].clear();
            writer
                .relate(
                    relation,
                    owner.id(),
                    child.id(),
                    vec![Attribute {
                        key: label,
                        value: Value::Text("dependency".into()),
                    }],
                )
                .unwrap();
            writer.advance_time(15).unwrap();
            writer.end(owner, Status::Ok).unwrap();
            writer.flush().unwrap();
            let partial = Reader::open(&path).unwrap();
            assert_eq!(partial.completion(), Completion::Unfinished);
            assert!(partial.validate().unwrap().incomplete);
            writer.advance_time(20).unwrap();
            writer.end(child, Status::Ok).unwrap();
            let late = writer
                .signal(
                    SourceOptions::new("late", Some(core)),
                    SignalKind::Bits {
                        width: 1,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Input,
                )
                .unwrap();
            writer.emit_u64(late, 1).unwrap();
            writer.advance_time(100).unwrap();
            let fingerprint = writer.close().unwrap();
            let reader = Reader::open(&path).unwrap();
            assert_eq!(reader.fingerprint(), Some(fingerprint));
            let report = reader.validate().unwrap();
            assert!(report.missing.is_empty());
            assert_eq!(report.relations, 1);
            assert_eq!(report.records, 8);
            let retained = reader.lookup(owner.id()).unwrap().unwrap();
            assert_eq!(
                retained.payload().decode::<[u64; 2]>().unwrap(),
                [u64::MAX, 42]
            );
            assert_eq!(
                reader.lookup(embedded).unwrap().unwrap().kind(),
                RecordKind::EmbeddedStage
            );
            assert_eq!(reader.lookup(embedded).unwrap().unwrap().end(), 100);
            let expansion = reader.expand(owner.id()).unwrap();
            assert_eq!(expansion.located.len(), 3);
            assert!(expansion.complete);
            assert_eq!(
                reader
                    .lookup(log)
                    .unwrap()
                    .unwrap()
                    .payload()
                    .decode::<Vec<String>>()
                    .unwrap(),
                vec!["copied"]
            );
            assert_eq!(reader.clock_for(source.id()).unwrap(), Some(clock.id()));
            assert_eq!(reader.cycle_at(clock.id(), 100).unwrap(), Some(5));
            assert_eq!(reader.find_signal("core.alias"), Some(signal));
            assert_eq!(
                reader.value_at(signal, 9).unwrap().borrow().as_u64(),
                Some(1)
            );
            assert_eq!(
                reader.value_at(signal, 10).unwrap().borrow().as_u64(),
                Some(2)
            );
            assert_eq!(reader.value_at(packed, 99).unwrap().to_ascii(), "zx10");
            assert!(
                matches!(reader.value_at(real, 99).unwrap(), vtr::OwnedSignalValue::Real(value) if value.to_bits() == 0x7ff8_0000_0000_beef)
            );
            let mut pulses = 0;
            reader
                .changes(Some(pulse), TimeWindow::default(), |_, _, _| {
                    pulses += 1;
                    true
                })
                .unwrap();
            assert_eq!(pulses, 2);
            let mut logs_seen = 0;
            reader
                .records(
                    RecordQuery {
                        kind: Some(RecordKind::Log),
                        ..RecordQuery::default()
                    },
                    |_| {
                        logs_seen += 1;
                        true
                    },
                )
                .unwrap();
            assert_eq!(logs_seen, 1);
            let mut children = 0;
            reader
                .transactions(
                    TransactionQuery {
                        parent: Some(owner.id()),
                        ..TransactionQuery::default()
                    },
                    |_| {
                        children += 1;
                        true
                    },
                )
                .unwrap();
            assert_eq!(children, 1);
            drop(reader);
            assert_eq!(
                retained.payload().at(1).unwrap().decode::<u64>().unwrap(),
                42
            );
        }
    }
}

#[test]
fn failure_poisons_recording_but_sealing_preserves_available_evidence() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("failed.vtr");
        let mut writer = Writer::create_with(&path, options(mode, Compression::LZ4)).unwrap();
        let key = writer.intern("key").unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("owners", None), "TRANSACTION")
            .unwrap();
        let owner = writer.begin(source, &(), None).unwrap();
        writer.attribute(owner.owner(), key, Value::U64(1)).unwrap();
        writer.flush().unwrap();
        let result = writer.attribute(owner.owner(), key, Value::U64(2));
        if mode == ExecutionMode::Inline {
            assert!(result.is_err());
        }
        assert!(writer.flush().is_err());
        assert!(writer.begin(source, &(), None).is_err());
        writer.seal().unwrap();
        let reader = Reader::open(&path).unwrap();
        assert_eq!(reader.completion(), Completion::Incomplete);
        assert_eq!(
            reader
                .lookup(owner.id())
                .unwrap()
                .unwrap()
                .attributes()
                .len(),
            1
        );
        reader.validate().unwrap();
    }
}

#[test]
fn parallel_producers_preserve_skewed_owned_payloads_and_bounded_spilling() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("parallel.vtr");
    let mut config = options(ExecutionMode::Threaded, Compression::LZ4);
    config.buffer.queue_bytes = 256;
    config.buffer.pending_bytes = 512;
    let mut writer = Writer::create_with(&path, config).unwrap();
    let source = writer
        .transactions::<Vec<String>>(SourceOptions::new("requests", None), "REQUEST")
        .unwrap();
    let mut producer = writer.add_producer().unwrap();
    let thread = std::thread::spawn(move || {
        for time in 1..101 {
            producer.advance_time(time).unwrap();
            let mut value = vec![format!("{time}:{}", "x".repeat(4096))];
            let transaction = producer.begin(source, &value, None).unwrap();
            value[0].clear();
            producer.end(transaction, Status::Ok).unwrap();
        }
        producer.finish().unwrap();
    });
    thread.join().unwrap();
    writer.advance_time(101).unwrap();
    writer.close().unwrap();
    let stats = writer.stats().unwrap();
    assert!(stats.pending_peak_bytes <= 512);
    assert!(stats.spilled_bytes > 409_600);
    let reader = Reader::open(&path).unwrap();
    reader.validate().unwrap();
    let mut count = 0;
    reader
        .transactions(TransactionQuery::default(), |record| {
            count += 1;
            let value = record.payload().decode::<Vec<String>>().unwrap();
            assert_eq!(value[0], format!("{}:{}", record.begin(), "x".repeat(4096)));
            true
        })
        .unwrap();
    assert_eq!(count, 100);
}

#[test]
fn encoding_workers_preserve_exact_wire_output() {
    let directory = tempfile::tempdir().unwrap();
    for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
        let mut expected = None;
        for workers in [0, 1, 3] {
            let path = directory.path().join(format!("encoded-{workers}.vtr"));
            let mut config = options(ExecutionMode::Threaded, compression);
            config.encoding.workers = workers;
            config.encoding.queue_jobs = 2;
            config.encoding.queue_bytes = 1024;
            let mut writer = Writer::create_with(&path, config).unwrap();
            let source = writer
                .transactions::<[u64; 2]>(SourceOptions::new("requests", None), "REQUEST")
                .unwrap();
            let signal = writer
                .signal(
                    SourceOptions::new("value", None),
                    SignalKind::Bits {
                        width: 32,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Output,
                )
                .unwrap();
            for time in 0..100 {
                writer.advance_time(time).unwrap();
                writer.emit_u64(signal, time).unwrap();
                let transaction = writer.begin(source, &[time, time * 2], None).unwrap();
                writer.end(transaction, Status::Ok).unwrap();
            }
            writer.close().unwrap();
            Reader::open(&path).unwrap().validate().unwrap();
            let mut bytes = std::fs::read(&path).unwrap();
            bytes[16..32].fill(0);
            if let Some(expected) = &expected {
                assert_eq!(&bytes, expected);
            } else {
                expected = Some(bytes);
            }
        }
    }
}
