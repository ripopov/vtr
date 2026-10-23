use vtr::{
    Compression, Direction, ExecutionMode, Reader, Recorder, SignalId, SignalKind, SignalUpdate,
    SourceOptions, TimeWindow, VarType, Writer, WriterOptions,
};

fn signal(writer: &mut Writer, name: &str, width: u32, states: u8, var_type: VarType) -> SignalId {
    writer
        .signal(
            SourceOptions::new(name, None),
            SignalKind::Bits { width, states },
            var_type,
            Direction::Implicit,
        )
        .unwrap()
}

#[test]
fn batches_match_individual_emissions_including_events_dumping_codecs_and_spill() {
    let dir = tempfile::tempdir().unwrap();
    for (mode, workers) in [
        (ExecutionMode::Inline, 0),
        (ExecutionMode::Threaded, 0),
        (ExecutionMode::Threaded, 2),
    ] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            for deduplicate in [false, true] {
                let mut expected = None;
                for batch in [false, true] {
                    let path = dir.path().join(format!(
                        "{mode:?}-{workers}-{compression:?}-{deduplicate}-{batch}.vtr"
                    ));
                    let mut options = WriterOptions {
                        mode,
                        ..WriterOptions::default()
                    };
                    options.buffer.queue_bytes = 256;
                    options.buffer.pending_bytes = 256;
                    options.encoding.compression = compression;
                    options.encoding.workers = workers;
                    options.encoding.waveform_records = 5;
                    options.encoding.waveform_deduplicate = deduplicate;
                    let mut writer = Writer::create_with(&path, options).unwrap();
                    let signals = [
                        signal(&mut writer, "bits", 8, 2, VarType::Wire),
                        signal(&mut writer, "logic", 4, 4, VarType::Wire),
                        signal(&mut writer, "wide", 64, 2, VarType::Wire),
                        signal(&mut writer, "pulse", 1, 2, VarType::Event),
                    ];
                    if mode == ExecutionMode::Threaded {
                        let mut producer = writer.add_producer().unwrap();
                        record(&mut producer, signals, batch);
                        producer.finish().unwrap();
                        assert!(writer.stats().unwrap().spilled_bytes > 1000);
                    } else {
                        record(&mut writer, signals, batch);
                    }
                    writer.advance_time(20).unwrap();
                    writer.close().unwrap();
                    let reader = Reader::open(&path).unwrap();
                    reader.validate().unwrap();
                    assert_eq!(
                        reader.value_at(signals[0], 7).unwrap().borrow().as_u64(),
                        Some(4)
                    );
                    assert_eq!(
                        reader.value_at(signals[0], 20).unwrap().borrow().as_u64(),
                        Some(16)
                    );
                    assert_eq!(reader.value_at(signals[1], 20).unwrap().to_ascii(), "0000");
                    assert_eq!(
                        reader.value_at(signals[2], 20).unwrap().borrow().as_u64(),
                        Some(u64::MAX - 16)
                    );
                    let mut pulses = 0;
                    reader
                        .changes(Some(signals[3]), TimeWindow::default(), |_, _, _| {
                            pulses += 1;
                            true
                        })
                        .unwrap();
                    assert_eq!(pulses, 28);
                    let mut bytes = std::fs::read(path).unwrap();
                    bytes[16..32].fill(0);
                    if let Some(expected) = &expected {
                        assert_eq!(&bytes, expected);
                    } else {
                        expected = Some(bytes);
                    }
                }
            }
        }
    }
}

fn record(recorder: &mut impl Recorder, signals: [SignalId; 4], batch: bool) {
    recorder.emit_batch(&[]).unwrap();
    for time in 0..=16 {
        recorder.advance_time(time).unwrap();
        if time == 5 {
            recorder.dump(false).unwrap();
        }
        if time == 8 {
            recorder.dump(true).unwrap();
        }
        let mut updates = vec![
            SignalUpdate {
                signal: signals[0],
                value: time,
            },
            SignalUpdate {
                signal: signals[1],
                value: time & 15,
            },
            SignalUpdate {
                signal: signals[2],
                value: u64::MAX - time,
            },
            SignalUpdate {
                signal: signals[3],
                value: 1,
            },
            SignalUpdate {
                signal: signals[3],
                value: 1,
            },
            SignalUpdate {
                signal: signals[0],
                value: time,
            },
        ];
        updates.extend(std::iter::repeat_n(
            SignalUpdate {
                signal: signals[0],
                value: time,
            },
            70,
        ));
        if batch {
            recorder.emit_batch(&updates).unwrap();
        } else {
            for update in &updates {
                recorder.emit_u64(update.signal, update.value).unwrap();
            }
        }
        updates.fill(SignalUpdate {
            signal: SignalId(u32::MAX),
            value: 0,
        });
    }
}

#[test]
fn invalid_batches_record_no_prefix_even_while_dumping_is_disabled() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for disabled in [false, true] {
            for case in 0..3 {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("invalid.vtr");
                let mut writer = Writer::create_with(
                    &path,
                    WriterOptions {
                        mode,
                        ..WriterOptions::default()
                    },
                )
                .unwrap();
                let valid = signal(&mut writer, "valid", 8, 2, VarType::Wire);
                let wide = signal(&mut writer, "wide", 65, 2, VarType::Wire);
                let real = writer
                    .signal(
                        SourceOptions::new("real", None),
                        SignalKind::Real,
                        VarType::Real,
                        Direction::Implicit,
                    )
                    .unwrap();
                writer.emit_u64(valid, 1).unwrap();
                writer.flush().unwrap();
                writer.advance_time(1).unwrap();
                if disabled {
                    writer.dump(false).unwrap();
                }
                let bad = match case {
                    0 => SignalId(u32::MAX),
                    1 => wide,
                    _ => real,
                };
                let result = writer.emit_batch(&[
                    SignalUpdate {
                        signal: valid,
                        value: 2,
                    },
                    SignalUpdate {
                        signal: bad,
                        value: 1,
                    },
                ]);
                assert!(result.and_then(|()| writer.flush()).is_err());
                writer.seal().unwrap();
                let reader = Reader::open(path).unwrap();
                reader.validate().unwrap();
                let mut samples = Vec::new();
                reader
                    .changes(Some(valid), TimeWindow::default(), |time, _, value| {
                        samples.push((time, value.as_u64().unwrap()));
                        true
                    })
                    .unwrap();
                assert_eq!(samples, [(0, 1)]);
            }
        }
    }
}

#[test]
fn a_conflicting_producer_rejects_the_entire_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ownership.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let owned = signal(&mut writer, "owned", 8, 2, VarType::Wire);
    let untouched = signal(&mut writer, "untouched", 8, 2, VarType::Wire);
    writer.emit_u64(owned, 1).unwrap();
    let mut producer = writer.add_producer().unwrap();
    producer
        .emit_batch(&[
            SignalUpdate {
                signal: untouched,
                value: 2,
            },
            SignalUpdate {
                signal: owned,
                value: 3,
            },
        ])
        .unwrap();
    assert!(producer.finish().is_err());
    drop(producer);
    writer.seal().unwrap();
    let reader = Reader::open(path).unwrap();
    reader.validate().unwrap();
    let mut samples = Vec::new();
    reader
        .changes(None, TimeWindow::default(), |time, signal, value| {
            samples.push((time, signal, value.as_u64().unwrap()));
            true
        })
        .unwrap();
    assert_eq!(samples, [(0, owned, 1)]);
}
