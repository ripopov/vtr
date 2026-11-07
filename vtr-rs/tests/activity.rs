//! Exact activity queries, source binding, and bounded decoding on current traces.

use std::collections::BTreeSet;
use vtr::{
    Codec, Direction, SignalId, SignalKind, VarType,
    activity::{self, BuildOptions, Identity, Index, Sidecar},
    framing::RecordingId,
    recording::{ExecutionMode, Recorder, SourceOptions, Writer, WriterOptions},
    trace::{Reader, TimeWindow},
};

fn validate(bytes: &[u8], identity: &Identity, limit: u64) -> vtr::Result<()> {
    use std::future::Future;
    let mut validation = std::pin::pin!(Index::validate_with(
        bytes,
        identity,
        |used| if used <= limit {
            Ok(())
        } else {
            Err(vtr::Error::State("validation budget exhausted"))
        },
        || std::future::ready(Ok(()))
    ));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match validation.as_mut().poll(&mut context) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => panic!("synchronous validation yielded"),
    }
}

fn recording(path: &std::path::Path, codec: Codec) -> Reader {
    let mut options = WriterOptions {
        mode: ExecutionMode::Inline,
        ..Default::default()
    };
    options.encoding.compression.codec = codec;
    options.encoding.waveform_records = 7;
    options.encoding.waveform_deduplicate = false;
    options.metadata.group_size = 4;
    let mut writer = Writer::create_with(path, options).unwrap();
    let signals: Vec<_> = (0..13)
        .map(|index| {
            writer
                .signal(
                    SourceOptions::new(format!("s{index}"), None),
                    SignalKind::Bits {
                        width: if index % 2 == 0 { 1 } else { 33 },
                        states: 4,
                    },
                    VarType::Wire,
                    Direction::Implicit,
                )
                .unwrap()
        })
        .collect();
    writer
        .alias(
            signals[0],
            SourceOptions::new("alias", None),
            VarType::Wire,
            Direction::Implicit,
        )
        .unwrap();
    let event = writer
        .signal(
            SourceOptions::new("event", None),
            SignalKind::Bits {
                width: 1,
                states: 2,
            },
            VarType::Event,
            Direction::Implicit,
        )
        .unwrap();
    for time in 40..=140 {
        writer.advance_time(time).unwrap();
        for (index, signal) in signals.iter().enumerate() {
            if (time * (index as u64 + 3)) % (index as u64 + 5) <= 1 {
                writer.emit_u64(*signal, time & 1).unwrap();
            }
        }
        if time % 7 == 0 {
            writer.emit_u64(event, 1).unwrap();
            writer.emit_u64(event, 1).unwrap();
        }
        if time % 19 == 0 {
            writer.flush().unwrap();
        }
    }
    let late = writer
        .signal(
            SourceOptions::new("late", None),
            SignalKind::Real,
            VarType::Real,
            Direction::Implicit,
        )
        .unwrap();
    writer.advance_time(200).unwrap();
    writer.emit_real(late, 1.0).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    reader.validate().unwrap();
    reader
}

#[test]
fn dynamic_aliases_share_scanned_ranges_across_groups_under_a_small_budget() {
    let directory = tempfile::tempdir().unwrap();
    let mut options = WriterOptions {
        mode: ExecutionMode::Inline,
        ..Default::default()
    };
    options.metadata.group_size = 8;
    let path = directory.path().join("aliases.vtr");
    let mut writer = Writer::create_with(&path, options).unwrap();
    let signals: Vec<_> = (0..64)
        .map(|id| {
            writer
                .signal(
                    SourceOptions::new(format!("s{id}"), None),
                    SignalKind::Bits {
                        width: 64,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Implicit,
                )
                .unwrap()
        })
        .collect();
    for step in 0..4096 {
        writer.advance_time((step / 2) * 3 + step % 2).unwrap();
        for &signal in &signals {
            writer.emit_u64(signal, step).unwrap();
        }
    }
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    let mut reference = Vec::new();
    let expected = activity::build(&reader, &mut reference, &BuildOptions::default()).unwrap();
    for threads in [1, 4] {
        let mut bytes = Vec::new();
        let result = activity::build(
            &reader,
            &mut bytes,
            &BuildOptions {
                memory: 4 << 20,
                threads,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.changes, 64 * 4095);
        assert_eq!(result.changes, expected.changes);
        assert_eq!(bytes, reference);
        let index = Index::decode_for(&bytes, &reader).unwrap();
        assert_eq!(index.classify(3, 3), signals);
        assert!(index.classify(2, 2).is_empty());
    }
    let error = activity::build(
        &reader,
        std::io::sink(),
        &BuildOptions {
            memory: 512 << 10,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("memory limit"), "{error}");
}

#[test]
fn dense_aliases_remain_bounded_across_parallel_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("parallel-aliases.vtr");
    let mut options = WriterOptions {
        mode: ExecutionMode::Inline,
        ..Default::default()
    };
    options.metadata.group_size = 32;
    options.encoding.waveform_records = 400_000;
    let mut writer = Writer::create_with(&path, options).unwrap();
    let signals: Vec<_> = (0..256)
        .map(|id| {
            writer
                .signal(
                    SourceOptions::new(format!("s{id}"), None),
                    SignalKind::Bits {
                        width: 64,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Implicit,
                )
                .unwrap()
        })
        .collect();
    for time in 0..30_000 {
        writer.advance_time(time * 3).unwrap();
        for &signal in &signals {
            writer.emit_u64(signal, time).unwrap();
        }
    }
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    assert_eq!(reader.block_count(), 20);
    let mut expected = Vec::new();
    activity::build(&reader, &mut expected, &BuildOptions::default()).unwrap();
    for threads in [1, 4, 8] {
        let mut actual = Vec::new();
        let control = std::sync::Arc::new(activity::BuildControl::default());
        let summary = activity::build(
            &reader,
            &mut actual,
            &BuildOptions {
                memory: 6 << 20,
                threads,
                control: Some(control.clone()),
            },
        )
        .unwrap();
        assert_eq!(summary.changes, 256 * 29_999);
        assert_eq!(actual, expected, "{threads} workers");
        assert_eq!(control.progress().completed, reader.block_count());
        let index = Index::decode_for(&actual, &reader).unwrap();
        assert_eq!(index.classify(3, 3), signals);
        assert!(index.classify(4, 5).is_empty());
    }
}

#[test]
fn wide_values_with_few_changes_fit_despite_an_oversized_scan_estimate() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("wide.vtr");
    let mut writer = Writer::create_with(
        &path,
        WriterOptions {
            mode: ExecutionMode::Inline,
            ..Default::default()
        },
    )
    .unwrap();
    let mut value = vec![0u8; 256 << 10];
    let signal = writer
        .signal(
            SourceOptions::new("wide", None),
            SignalKind::Bits {
                width: value.len() as u32 * 8,
                states: 2,
            },
            VarType::Wire,
            Direction::Implicit,
        )
        .unwrap();
    for time in [0, 1, 2, u64::MAX - 1, u64::MAX] {
        writer.advance_time(time).unwrap();
        value[0] = value[0].wrapping_add(1);
        writer.emit_packed(signal, 2, &value).unwrap();
    }
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    let mut expected = Vec::new();
    activity::build(&reader, &mut expected, &BuildOptions::default()).unwrap();
    let mut actual = Vec::new();
    let summary = activity::build(
        &reader,
        &mut actual,
        &BuildOptions {
            memory: 4 << 20,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(summary.changes, 4);
    assert_eq!(actual, expected);
    let index = Index::decode_for(&actual, &reader).unwrap();
    assert_eq!(index.classify(1, 2), vec![signal]);
    assert!(index.classify(3, u64::MAX - 2).is_empty());
    assert_eq!(index.classify(u64::MAX - 1, u64::MAX), vec![signal]);
    let error = activity::build(
        &reader,
        std::io::sink(),
        &BuildOptions {
            memory: 512 << 10,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("memory limit"), "{error}");
}

#[test]
fn every_window_matches_waveforms_across_blocks_aliases_and_late_declarations() {
    let directory = tempfile::tempdir().unwrap();
    for codec in [Codec::None, Codec::Lz4, Codec::Zstd] {
        let reader = recording(&directory.path().join("trace.vtr"), codec);
        let mut sidecar = Vec::new();
        let summary = activity::build(
            &reader,
            &mut sidecar,
            &BuildOptions {
                threads: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(summary.delta, Some((1, 1)));
        let mut parallel = Vec::new();
        activity::build(
            &reader,
            &mut parallel,
            &BuildOptions {
                threads: 3,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(sidecar, parallel);
        let estimate = Index::estimate_memory(&sidecar, &Identity::of(&reader).unwrap()).unwrap();
        let index = Index::decode_for(&sidecar, &reader).unwrap();
        assert!(index.memory_bytes() <= estimate.resident);
        assert_eq!(index.blocks().len(), reader.block_count());
        let minimum = reader.waveform_time_range().unwrap().first;
        let mut histories = vec![BTreeSet::new(); reader.signal_count() as usize];
        reader
            .changes(None, TimeWindow::default(), |time, signal, _| {
                if time > minimum {
                    histories[signal.0 as usize].insert(time);
                }
                true
            })
            .unwrap();
        for first in 0..=205 {
            for width in [0, 1, 2, 5, 13, 31, 99, 300] {
                let last = first + width;
                let expected: Vec<_> = histories
                    .iter()
                    .enumerate()
                    .filter_map(|(signal, times)| {
                        times
                            .range(first..=last)
                            .next()
                            .map(|_| SignalId(signal as u32))
                    })
                    .collect();
                let classification = index.classify(first, last);
                assert_eq!(classification, expected, "{codec:?} {first}..{last}");
            }
        }
        for signal in 0..reader.signal_count() {
            let points: BTreeSet<_> = index
                .stretches(SignalId(signal))
                .flat_map(|stretch| stretch.start..=stretch.end)
                .collect();
            assert_eq!(points, histories[signal as usize]);
        }
        assert!(index.classify(100, 99).is_empty());
    }
}

#[test]
fn recording_identity_binds_sidecars_even_when_sizes_and_directory_crc_match() {
    let directory = tempfile::tempdir().unwrap();
    let reader = recording(&directory.path().join("first.vtr"), Codec::None);
    let other = recording(&directory.path().join("second.vtr"), Codec::None);
    let first = Identity::of(&reader).unwrap();
    let second = Identity::of(&other).unwrap();
    assert_eq!(
        (first.length, first.toc_crc),
        (second.length, second.toc_crc)
    );
    assert_ne!(first.recording, second.recording);
    let mut bytes = Vec::new();
    activity::build(&reader, &mut bytes, &Default::default()).unwrap();
    assert!(Index::decode_for(&bytes, &other).is_err());
    let left = Sidecar::new(
        &directory.path().join("left"),
        &first,
        Some(directory.path()),
    );
    let right = Sidecar::new(
        &directory.path().join("right"),
        &second,
        Some(directory.path()),
    );
    assert_ne!(left.cached, right.cached);
    let mut unfinished = std::fs::read(directory.path().join("first.vtr")).unwrap();
    unfinished.truncate(unfinished.len() - 24);
    let unfinished = Reader::from_bytes(unfinished).unwrap();
    assert!(Identity::of(&unfinished).is_err());
    for (major, minor) in [(2u16, 0u16), (3, 0), (4, 1)] {
        let mut wrong_version = bytes.clone();
        wrong_version[8..10].copy_from_slice(&major.to_le_bytes());
        wrong_version[10..12].copy_from_slice(&minor.to_le_bytes());
        let error = Index::decode(&wrong_version, &first).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("unsupported VTI version {major}.{minor} (expected 4.0)")
        );
    }
    let mut wrong_id = first;
    wrong_id.recording = RecordingId([1; 16]);
    assert!(Index::decode(&bytes, &wrong_id).is_err());
}

#[test]
fn decoding_checks_framing_and_honors_memory_admission_and_yield_points() {
    use std::future::Future;
    let directory = tempfile::tempdir().unwrap();
    let reader = recording(&directory.path().join("trace.vtr"), Codec::Lz4);
    let identity = Identity::of(&reader).unwrap();
    let mut bytes = Vec::new();
    activity::build(&reader, &mut bytes, &Default::default()).unwrap();
    for end in [0, 31, bytes.len() - 1, bytes.len() - 24] {
        assert!(Index::decode(&bytes[..end], &identity).is_err());
        assert!(validate(&bytes[..end], &identity, u64::MAX).is_err());
    }
    for position in [12, 16, 32, 48, bytes.len() - 24, bytes.len() - 1] {
        let mut corrupt = bytes.clone();
        corrupt[position] ^= 1;
        assert!(validate(&corrupt, &identity, u64::MAX).is_err());
        assert!(
            Index::decode(&corrupt, &identity).is_err(),
            "byte {position}"
        );
    }
    let mut denied = std::pin::pin!(Index::decode_with(
        &bytes,
        &identity,
        |_| Err(vtr::Error::State("budget exhausted")),
        || std::future::ready(Ok(()))
    ));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(matches!(
        denied.as_mut().poll(&mut context),
        std::task::Poll::Ready(Err(_))
    ));
    let estimate = Index::estimate_memory(&bytes, &identity).unwrap();
    let mut peak = 0;
    let mut checkpoints = 0;
    {
        let mut decoding = std::pin::pin!(Index::decode_with(
            &bytes,
            &identity,
            |used| {
                peak = peak.max(used);
                Ok(())
            },
            || {
                checkpoints += 1;
                std::future::ready(Ok(()))
            }
        ));
        assert!(matches!(
            decoding.as_mut().poll(&mut context),
            std::task::Poll::Ready(Ok(_))
        ));
    }
    assert!(peak > bytes.len() as u64);
    assert!(
        peak <= estimate.peak,
        "preflight must cover the loading workspace"
    );
    assert!(checkpoints > 100);
    assert!(
        activity::build(
            &reader,
            Vec::new(),
            &BuildOptions {
                memory: 1,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn generator_windows_match_records_with_gaps_points_and_out_of_order_completion() {
    use vtr::{
        Status,
        records::{RecordKind, RecordQuery},
        transactions::TransactionQuery,
    };
    let dir = tempfile::tempdir().unwrap();
    for general in [false, true] {
        let path = dir.path().join("generators.vtr");
        let mut options = WriterOptions::default();
        options.encoding.block_records = 13;
        options.encoding.page_bytes = 128;
        let mut w = Writer::create_with(&path, options).unwrap();
        let scope = w.scope("top", None).unwrap();
        let source = w
            .transactions::<u64>(SourceOptions::new("tx", Some(scope)), "TX")
            .unwrap();
        w.transactions::<()>(SourceOptions::new("empty", Some(scope)), "TX")
            .unwrap();
        let events = w
            .events::<()>(SourceOptions::new("event", Some(scope)))
            .unwrap();
        let stages = w
            .stages::<()>(SourceOptions::new("stage", Some(scope)))
            .unwrap();
        let logger = w.logger(SourceOptions::new("log", Some(scope))).unwrap();
        let callsite =
            vtr::logging::declare::<()>(&mut w, vtr::logging::CallsiteOptions::new("message"))
                .unwrap();
        let key = w.intern("label").unwrap();
        let owner = w.begin_at(source, 0, &0, None).unwrap();
        w.end_at(owner, 0, Status::Ok).unwrap();
        for i in 0..500 {
            let begin = (i * 117) % 900;
            let tx = w.begin_at(source, begin, &i, None).unwrap();
            if general {
                w.attribute(tx.owner(), key, vtr::Value::U64(i)).unwrap();
            }
            let stage = w.begin_stage_at(stages, tx.owner(), begin, &()).unwrap();
            w.end_stage_at(stage, begin + i % 7).unwrap();
            w.event_at(events, tx.owner(), begin, &()).unwrap();
            w.end_at(tx, begin + i % 11, Status::Ok).unwrap();
        }
        let long = w.begin_at(source, 1000, &0, None).unwrap();
        w.advance_time(1050).unwrap();
        w.log(logger, callsite, &(), None).unwrap();
        w.end_at(long, 2000, Status::Ok).unwrap();
        let late = w
            .transactions::<u64>(SourceOptions::new("late", Some(scope)), "TX")
            .unwrap();
        for t in [3000, u64::MAX] {
            let tx = w.begin_at(late, t, &0, None).unwrap();
            w.end_at(tx, t, Status::Ok).unwrap();
        }
        w.close().unwrap();
        let r = Reader::open(&path).unwrap();
        r.validate().unwrap();
        let mut expected = Vec::new();
        r.transactions(TransactionQuery::default(), |tx| {
            expected.push((tx.generator().unwrap(), tx.begin(), tx.end()));
            true
        })
        .unwrap();
        r.records(RecordQuery::default(), |record| {
            let end = if record.kind() == RecordKind::Stage && record.end() > record.begin() {
                record.end() - 1
            } else {
                record.end()
            };
            expected.push((record.generator().unwrap(), record.begin(), end));
            true
        })
        .unwrap();
        let mut bytes = Vec::new();
        activity::build(&r, &mut bytes, &Default::default()).unwrap();
        validate(&bytes, &Identity::of(&r).unwrap(), u64::MAX).unwrap();
        let estimate = Index::estimate_memory(&bytes, &Identity::of(&r).unwrap()).unwrap();
        let index = Index::decode_for(&bytes, &r).unwrap();
        assert!(index.memory_bytes() <= estimate.resident);
        for begin in (0u64..3100).chain([u64::MAX]) {
            for length in [0, 1, 17, 500] {
                let end = begin.saturating_add(length);
                let wanted: BTreeSet<_> = expected
                    .iter()
                    .filter(|&&(_, a, b)| a <= end && b >= begin)
                    .map(|&(id, _, _)| id)
                    .collect();
                assert_eq!(
                    index.classify_generators(begin, end),
                    wanted.into_iter().collect::<Vec<_>>(),
                    "general={general}, [{begin},{end}]"
                );
            }
        }
        assert!(index.classify_generators(2, 1).is_empty());
        assert!(
            activity::build(
                &r,
                std::io::sink(),
                &BuildOptions {
                    memory: 128,
                    ..Default::default()
                }
            )
            .is_err()
        );
        // Payload limits must not prevent metadata-only indexing.
        let limited = Reader::open_with(
            &path,
            vtr::trace::ReadOptions {
                max_payload_bytes: 0,
                ..Default::default()
            },
        )
        .unwrap();
        let mut second = Vec::new();
        activity::build(&limited, &mut second, &Default::default()).unwrap();
        assert_eq!(bytes, second);
    }
}

#[test]
fn sidecars_validate_in_a_small_workspace_without_materializing_activity_ranges() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("busy.vtr");
    let mut options = WriterOptions::default();
    options.encoding.waveform_records = 256;
    let mut writer = Writer::create_with(&path, options).unwrap();
    let signal = writer
        .signal(
            SourceOptions::new("busy", None),
            SignalKind::Bits {
                width: 1,
                states: 2,
            },
            VarType::Wire,
            Direction::Implicit,
        )
        .unwrap();
    for step in 0..50_000 {
        writer.advance_time(step * 3).unwrap();
        writer.emit_u64(signal, step & 1).unwrap();
    }
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    let identity = Identity::of(&reader).unwrap();
    let mut image = Vec::new();
    activity::build(&reader, &mut image, &BuildOptions::default()).unwrap();
    let limit = 256 << 10;
    validate(&image, &identity, limit).unwrap();
    assert!(validate(&image, &identity, 1).is_err());
    let index = Index::decode_for(&image, &reader).unwrap();
    assert!(index.memory_bytes() > limit);
    assert_eq!(index.classify(15, 15), vec![signal]);
    assert!(index.classify(16, 17).is_empty());
}
