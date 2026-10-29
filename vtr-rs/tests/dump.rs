use vtr::{
    Compression, Direction, SignalId, SignalKind, VarType,
    recording::{
        ClockSource, ExecutionMode, Recorder, Source, SourceOptions, Status, Writer, WriterOptions,
    },
    trace::{Reader, TimeWindow},
};

fn signal(writer: &mut Writer, name: &str, kind: SignalKind, event: bool) -> SignalId {
    writer
        .signal(
            SourceOptions::new(name, None),
            kind,
            if event { VarType::Event } else { VarType::Wire },
            Direction::Implicit,
        )
        .unwrap()
}

fn samples(reader: &Reader, signal: SignalId) -> Vec<(u64, String)> {
    let mut values = Vec::new();
    reader
        .changes(Some(signal), TimeWindow::default(), |time, _, value| {
            values.push((time, value.to_ascii()));
            true
        })
        .unwrap();
    values
}

#[test]
fn dump_suppresses_only_waves_and_preserves_recorded_state_across_spill() {
    for (mode, workers) in [
        (ExecutionMode::Inline, 0),
        (ExecutionMode::Threaded, 0),
        (ExecutionMode::Threaded, 2),
    ] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            for deduplicate in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("dump.vtr");
                let mut options = WriterOptions {
                    mode,
                    ..WriterOptions::default()
                };
                options.buffer.queue_bytes = 256;
                options.buffer.pending_bytes = 256;
                options.encoding.workers = workers;
                options.encoding.compression = compression;
                options.encoding.waveform_deduplicate = deduplicate;
                options.encoding.waveform_records = 2;
                let mut writer = Writer::create_with(&path, options).unwrap();
                let logger = writer
                    .logger(SourceOptions::new("diagnostics", None))
                    .unwrap();
                let bits = signal(
                    &mut writer,
                    "bits",
                    SignalKind::Bits {
                        width: 8,
                        states: 2,
                    },
                    false,
                );
                let logic = signal(
                    &mut writer,
                    "logic",
                    SignalKind::Bits {
                        width: 4,
                        states: 4,
                    },
                    false,
                );
                let wide = signal(
                    &mut writer,
                    "wide",
                    SignalKind::Bits {
                        width: 129,
                        states: 2,
                    },
                    false,
                );
                let real = signal(&mut writer, "real", SignalKind::Real, false);
                let bytes = signal(&mut writer, "bytes", SignalKind::VarLen, false);
                let pulse = signal(
                    &mut writer,
                    "pulse",
                    SignalKind::Bits {
                        width: 1,
                        states: 2,
                    },
                    true,
                );
                let late = signal(
                    &mut writer,
                    "late",
                    SignalKind::Bits {
                        width: 1,
                        states: 2,
                    },
                    false,
                );
                let owners = writer
                    .transactions::<u64>(SourceOptions::new("owners", None), "OWNER")
                    .unwrap();
                let logs = vtr::logging::declare::<u64>(
                    &mut writer,
                    vtr::logging::CallsiteOptions::new("step {}"),
                )
                .unwrap();
                let clock = writer.clock(SourceOptions::new("clock", None)).unwrap();
                let scenario = Scenario {
                    logger,
                    waves: [bits, logic, wide, real, bytes],
                    pulse,
                    late,
                    owners,
                    logs,
                    clock,
                };
                if mode == ExecutionMode::Threaded {
                    let mut producer = writer.add_producer().unwrap();
                    scenario.record(&mut producer);
                    producer.finish().unwrap();
                    // The writer's stalled frontier forces accepted captures through the spill file.
                    assert!(writer.stats().unwrap().spilled_bytes > 0);
                } else {
                    scenario.record(&mut writer);
                }
                writer.advance_time(6).unwrap();
                writer.close().unwrap();
                let reader = Reader::open(&path).unwrap();
                reader.validate().unwrap();
                assert_eq!(
                    reader
                        .blackouts()
                        .iter()
                        .map(|gap| (gap.time, gap.active))
                        .collect::<Vec<_>>(),
                    [(1, false), (3, true), (5, false)]
                );
                let times = if deduplicate {
                    vec![0, 4]
                } else {
                    vec![0, 3, 4]
                };
                for id in [bits, logic, wide, real, bytes] {
                    assert_eq!(
                        samples(&reader, id)
                            .iter()
                            .map(|(time, _)| *time)
                            .collect::<Vec<_>>(),
                        times
                    );
                }
                assert_eq!(samples(&reader, bits).last().unwrap().1, "00000111");
                assert_eq!(
                    samples(&reader, pulse),
                    [
                        (0, "0".into()),
                        (0, "0".into()),
                        (3, "0".into()),
                        (3, "0".into()),
                        (4, "1".into())
                    ]
                );
                assert_eq!(samples(&reader, late), [(3, "0".into())]);
                let owner = reader.lookup(2).unwrap().unwrap();
                assert_eq!((owner.begin(), owner.end()), (1, 3));
                assert_eq!(owner.status(), Status::Ok);
                assert_eq!(
                    vtr::logging::format_log(&reader, &reader.lookup(3).unwrap().unwrap()).unwrap(),
                    "step 2"
                );
                assert_eq!(reader.cycle_at(clock.id(), 6).unwrap(), Some(3));
            }
        }
    }
}

struct Scenario {
    waves: [SignalId; 5],
    pulse: SignalId,
    late: SignalId,
    owners: Source<u64>,
    logger: vtr::Logger,
    logs: vtr::PayloadType<u64>,
    clock: ClockSource,
}
impl Scenario {
    fn record(&self, recorder: &mut impl Recorder) {
        recorder.dump(true).unwrap();
        recorder.clock_run(self.clock, 0, 2).unwrap();
        self.waves(recorder, false);
        recorder.emit_u64(self.pulse, 0).unwrap();
        recorder.emit_u64(self.pulse, 0).unwrap();
        recorder.advance_time(1).unwrap();
        recorder.dump(false).unwrap();
        recorder.dump(false).unwrap();
        let owner = recorder.begin(self.owners, &42, None).unwrap();
        self.waves(recorder, true);
        recorder.emit_u64(self.pulse, 1).unwrap();
        recorder.emit_u64(self.late, 1).unwrap();
        recorder.advance_time(2).unwrap();
        self.waves(recorder, true);
        recorder
            .log(self.logger, self.logs, &2, Some(owner.owner()))
            .unwrap();
        recorder.advance_time(3).unwrap();
        recorder.dump(true).unwrap();
        recorder.dump(true).unwrap();
        self.waves(recorder, false);
        recorder.emit_u64(self.pulse, 0).unwrap();
        recorder.emit_u64(self.pulse, 0).unwrap();
        recorder.emit_u64(self.late, 0).unwrap();
        recorder.end(owner, Status::Ok).unwrap();
        recorder.advance_time(4).unwrap();
        self.waves(recorder, true);
        recorder.emit_u64(self.pulse, 1).unwrap();
        recorder.advance_time(5).unwrap();
        recorder.dump(false).unwrap();
        self.waves(recorder, false);
        recorder.clock_stop(self.clock, 6).unwrap();
    }
    fn waves(&self, recorder: &mut impl Recorder, changed: bool) {
        let [bits, logic, wide, real, bytes] = self.waves;
        recorder
            .emit_u64(bits, if changed { 7 } else { 0 })
            .unwrap();
        recorder
            .emit_packed(logic, 4, &[if changed { 0xff } else { 0 }])
            .unwrap();
        recorder
            .emit_words(wide, 129, &[u32::from(changed); 5])
            .unwrap();
        recorder
            .emit_real(real, if changed { -0.0 } else { 0.0 })
            .unwrap();
        recorder
            .emit_bytes(bytes, if changed { b"captured" } else { b"" })
            .unwrap();
    }
}

#[test]
fn blackout_annotations_do_not_suppress_capture_and_flush_publishes_transitions() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefix.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let bits = signal(
            &mut writer,
            "bits",
            SignalKind::Bits {
                width: 8,
                states: 2,
            },
            false,
        );
        writer.blackout(1, false).unwrap();
        writer.advance_time(2).unwrap();
        writer.emit_u64(bits, 7).unwrap();
        writer.blackout(3, true).unwrap();
        writer.advance_time(4).unwrap();
        writer.dump(false).unwrap();
        writer.emit_u64(bits, 42).unwrap();
        writer.flush().unwrap();
        {
            let prefix = Reader::open(&path).unwrap();
            assert_eq!(
                prefix
                    .blackouts()
                    .iter()
                    .map(|gap| (gap.time, gap.active))
                    .collect::<Vec<_>>(),
                [(1, false), (3, true), (4, false)]
            );
            assert_eq!(samples(&prefix, bits), [(2, "00000111".into())]);
        }
        writer.advance_time(5).unwrap();
        writer.dump(true).unwrap();
        writer.emit_u64(bits, 42).unwrap();
        writer.close().unwrap();
        let reader = Reader::open(&path).unwrap();
        reader.validate().unwrap();
        assert_eq!(
            reader
                .blackouts()
                .iter()
                .map(|gap| (gap.time, gap.active))
                .collect::<Vec<_>>(),
            [(1, false), (3, true), (4, false), (5, true)]
        );
        assert_eq!(
            samples(&reader, bits),
            [(2, "00000111".into()), (5, "00101010".into())]
        );
    }
}

#[test]
fn disabled_capture_still_rejects_invalid_signals_values_and_backdated_markers() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for case in 0..7 {
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
            let bits = signal(
                &mut writer,
                "bits",
                SignalKind::Bits {
                    width: 2,
                    states: 2,
                },
                false,
            );
            let real = signal(&mut writer, "real", SignalKind::Real, false);
            let wide = signal(
                &mut writer,
                "wide",
                SignalKind::Bits {
                    width: 129,
                    states: 2,
                },
                false,
            );
            writer.advance_time(5).unwrap();
            writer.dump(false).unwrap();
            writer.flush().unwrap();
            let result = match case {
                0 => writer.emit_u64(SignalId(u32::MAX), 1),
                1 => writer.emit_real(bits, 1.0),
                2 => writer.emit_u64(real, 1),
                3 => writer.emit_packed(bits, 2, &[]),
                4 => writer.emit_packed(bits, 4, &[2]),
                5 => writer.emit_u64(wide, 1),
                _ => writer.blackout(4, true),
            };
            assert!(
                result.and_then(|()| writer.flush()).is_err(),
                "accepted invalid case {case} in {mode:?}"
            );
            assert!(writer.close().is_err());
        }
    }
}
