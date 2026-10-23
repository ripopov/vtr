use rand::{Rng, SeedableRng, rngs::StdRng};
use vtr::*;

#[test]
fn random_waveforms_preserve_every_sample_and_window_across_blocks() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for codec in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("waves.vtr");
            let mut options = WriterOptions {
                mode,
                ..WriterOptions::default()
            };
            options.metadata.group_size = 4;
            options.encoding.compression = codec;
            options.encoding.waveform_records = 37;
            options.encoding.waveform_deduplicate = false;
            let mut writer = Writer::create_with(&path, options).unwrap();
            let mut rng = StdRng::seed_from_u64(71);
            let mut signals = Vec::new();
            let mut expected = Vec::<Vec<(u64, String)>>::new();
            for time in 0..180 {
                writer.advance_time(time / 2).unwrap();
                if time % 9 == 0 {
                    let index = signals.len();
                    let kind = match index % 5 {
                        0 => SignalKind::Bits {
                            width: [1, 7, 64, 129][index % 4],
                            states: 2,
                        },
                        1 => SignalKind::Bits {
                            width: 37,
                            states: 4,
                        },
                        2 => SignalKind::Bits {
                            width: 17,
                            states: 9,
                        },
                        3 => SignalKind::Real,
                        _ => SignalKind::VarLen,
                    };
                    let var = match kind {
                        SignalKind::Real => VarType::Real,
                        SignalKind::VarLen => VarType::String,
                        SignalKind::Bits { .. } => VarType::Wire,
                    };
                    let signal = writer
                        .signal(
                            SourceOptions::new(format!("s{index}"), None),
                            kind,
                            var,
                            Direction::Implicit,
                        )
                        .unwrap();
                    writer
                        .alias(
                            signal,
                            SourceOptions::new(format!("alias{index}"), None),
                            var,
                            Direction::Implicit,
                        )
                        .unwrap();
                    signals.push((signal, kind));
                    expected.push(Vec::new());
                }
                for (index, &(signal, kind)) in signals.iter().enumerate() {
                    if rng.gen_ratio(1, 3) {
                        continue;
                    }
                    let ascii = match kind {
                        SignalKind::Bits { width, states } => {
                            let stride = match states {
                                2 => 1,
                                4 => 2,
                                _ => 4,
                            };
                            let mut bytes = vec![0; vtr::signal::packed_len(width, states)];
                            let mut chars = Vec::new();
                            for bit in 0..width as usize {
                                let value = rng.gen_range(0..states);
                                bytes[bit * stride / 8] |= value << (bit * stride % 8);
                                chars.push(if states == 9 {
                                    b"01xzuwlh-"[value as usize]
                                } else {
                                    b"01xz"[value as usize]
                                });
                            }
                            writer.emit_packed(signal, states, &bytes).unwrap();
                            chars.reverse();
                            String::from_utf8(chars).unwrap()
                        }
                        SignalKind::Real => {
                            let value = f64::from_bits(rng.r#gen());
                            writer.emit_real(signal, value).unwrap();
                            OwnedSignalValue::Real(value).to_ascii()
                        }
                        SignalKind::VarLen => {
                            let bytes = format!("{index}:{}", rng.r#gen::<u32>());
                            writer.emit_bytes(signal, bytes.as_bytes()).unwrap();
                            bytes
                        }
                    };
                    expected[index].push((time / 2, ascii));
                }
            }
            writer.close().unwrap();
            let reader = Reader::open(&path).unwrap();
            assert!(reader.block_count() > 10);
            for (index, &(signal, _)) in signals.iter().enumerate() {
                assert_eq!(reader.find_signal(&format!("alias{index}")), Some(signal));
                let history = reader.load_signal(signal).unwrap();
                let actual = history
                    .times()
                    .iter()
                    .enumerate()
                    .map(|(i, &time)| (time, history.get(i).to_ascii()))
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual, expected[index],
                    "{mode:?}, {codec:?}, signal {index}"
                );
                for query in [
                    TimeWindow { first: 5, last: 17 },
                    TimeWindow {
                        first: 30,
                        last: 30,
                    },
                    TimeWindow {
                        first: 81,
                        last: 105,
                    },
                ] {
                    let mut actual = Vec::new();
                    reader
                        .changes(Some(signal), query, |time, _, value| {
                            actual.push((time, value.to_ascii()));
                            true
                        })
                        .unwrap();
                    assert_eq!(
                        actual,
                        expected[index]
                            .iter()
                            .filter(|(time, _)| *time >= query.first && *time <= query.last)
                            .cloned()
                            .collect::<Vec<_>>()
                    );
                    for time in query.first..=query.last {
                        if let Some((_, value)) =
                            expected[index].iter().rev().find(|(at, _)| *at <= time)
                        {
                            assert_eq!(&reader.value_at(signal, time).unwrap().to_ascii(), value);
                        }
                    }
                }
            }
            let retained = reader.load_signal(signals[0].0).unwrap();
            drop(reader);
            assert_eq!(retained.times().len(), expected[0].len());
        }
    }
}

#[test]
fn events_keep_identical_repetitions_while_wires_deduplicate() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.vtr");
        let mut options = WriterOptions {
            mode,
            ..WriterOptions::default()
        };
        options.encoding.waveform_records = 3;
        let mut writer = Writer::create_with(&path, options).unwrap();
        let kind = SignalKind::Bits {
            width: 1,
            states: 2,
        };
        let event = writer
            .signal(
                SourceOptions::new("event", None),
                kind,
                VarType::Event,
                Direction::Implicit,
            )
            .unwrap();
        let wire = writer
            .signal(
                SourceOptions::new("wire", None),
                kind,
                VarType::Wire,
                Direction::Implicit,
            )
            .unwrap();
        writer
            .alias(
                event,
                SourceOptions::new("alias", None),
                VarType::Event,
                Direction::Implicit,
            )
            .unwrap();
        for time in 0..20 {
            writer.advance_time(time / 3).unwrap();
            writer.emit_u64(event, 1).unwrap();
            writer.emit_u64(wire, 1).unwrap();
        }
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        assert_eq!(reader.load_signal(event).unwrap().len(), 20);
        assert_eq!(reader.load_signal(wire).unwrap().len(), 1);
        assert_eq!(reader.find_signal("alias"), Some(event));
    }
}
