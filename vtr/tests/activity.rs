//! Exact activity queries, source binding, and bounded decoding on current traces.

use std::collections::BTreeSet;
use vtr::{
    Codec, Direction, SignalId, SignalKind, VarType,
    activity::{self, BuildOptions, Identity, Index, Sidecar},
    framing::RecordingId,
    recording::{ExecutionMode, Recorder, SourceOptions, Writer, WriterOptions},
    trace::{Reader, TimeWindow},
};

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
        let index = Index::decode_for(&sidecar, &reader).unwrap();
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
    let mut legacy = bytes.clone();
    legacy[8..10].copy_from_slice(&2u16.to_le_bytes());
    assert!(Index::decode(&legacy, &first).is_err());
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
    }
    for position in [12, 16, 32, 48, bytes.len() - 24, bytes.len() - 1] {
        let mut corrupt = bytes.clone();
        corrupt[position] ^= 1;
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
