use vtr::{
    Compression, ExecutionMode, Reader, Recorder, SourceOptions, Status, Writer, WriterOptions,
    framing::Completion, trace::MissingKind,
};

fn options(mode: ExecutionMode, compression: Compression, workers: usize) -> WriterOptions {
    let mut options = WriterOptions {
        mode,
        ..WriterOptions::default()
    };
    options.buffer.queue_bytes = 256;
    options.buffer.pending_bytes = 256;
    options.encoding.block_records = 2;
    options.encoding.compression = compression;
    options.encoding.workers = workers;
    options
}

#[test]
fn deferred_and_replaced_payloads_are_owned_through_spill_and_final_capture() {
    for (mode, workers) in [
        (ExecutionMode::Inline, 0),
        (ExecutionMode::Threaded, 0),
        (ExecutionMode::Threaded, 2),
    ] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("payloads.vtr");
            let mut writer =
                Writer::create_with(&path, options(mode, compression, workers)).unwrap();
            let owners = writer
                .transactions::<String>(SourceOptions::new("owners", None), "OWNER")
                .unwrap();
            let stages = writer
                .stages::<Vec<String>>(SourceOptions::new("stages", None))
                .unwrap();
            let ids = if mode == ExecutionMode::Threaded {
                let mut producer = writer.add_producer().unwrap();
                let ids = record(&mut producer, owners, stages);
                producer.finish().unwrap();
                assert!(writer.stats().unwrap().spilled_bytes > 4096);
                ids
            } else {
                record(&mut writer, owners, stages)
            };
            writer.advance_time(10).unwrap();
            writer.close().unwrap();
            let reader = Reader::open(path).unwrap();
            reader.validate().unwrap();
            for (id, expected, begin, end, status) in [
                (ids[0], "replacement", 1, 3, Status::Ok),
                (ids[1], "final", 0, 4, Status::Error),
                (ids[2], "open", 5, 10, Status::Open),
            ] {
                let value = reader.lookup(id).unwrap().unwrap();
                assert_eq!(value.payload().decode::<String>().unwrap(), expected);
                assert_eq!(
                    (value.begin(), value.end(), value.status()),
                    (begin, end, status)
                );
            }
            for (id, expected, end) in
                [(ids[3], "final stage", 2), (ids[4], "replacement stage", 3)]
            {
                let stage = reader.lookup(id).unwrap().unwrap();
                assert_eq!(stage.payload().decode::<Vec<String>>().unwrap(), [expected]);
                assert_eq!(stage.end(), end);
            }
        }
    }
}

fn record(
    recorder: &mut impl Recorder,
    owners: vtr::Source<String>,
    stages: vtr::Source<Vec<String>, vtr::recording::source::Stage>,
) -> [u64; 5] {
    recorder.advance_time(1).unwrap();
    let owner = recorder.begin_deferred(owners, None).unwrap();
    let child = recorder
        .begin_deferred_at(owners, 0, Some(owner.owner()))
        .unwrap();
    let stage = recorder
        .begin_stage_deferred(stages, owner.owner())
        .unwrap();
    let auto_stage = recorder
        .begin_stage_at(stages, owner.owner(), 2, &vec!["initial".into()])
        .unwrap();
    let mut value = "x".repeat(4096);
    recorder.payload(owner, &value).unwrap();
    value.clear();
    value.push_str("replacement");
    recorder.payload(owner, &value).unwrap();
    value.clear();
    let mut visit = vec!["initial stage".to_owned()];
    recorder.payload(stage, &visit).unwrap();
    visit[0] = "final stage".into();
    recorder
        .end_stage_at_with_payload(stage, 2, &visit)
        .unwrap();
    visit[0] = "replacement stage".into();
    recorder.payload(auto_stage, &visit).unwrap();
    visit.clear();
    recorder.end_at(owner, 3, Status::Ok).unwrap();
    recorder
        .end_at_with_payload(child, 4, &"final".to_owned(), Status::Error)
        .unwrap();
    recorder.advance_time(5).unwrap();
    let open = recorder
        .begin(owners, &"initial open".into(), None)
        .unwrap();
    recorder.payload(open, &"open".into()).unwrap();
    [
        owner.id(),
        child.id(),
        open.id(),
        stage.id(),
        auto_stage.id(),
    ]
}

#[derive(Debug, PartialEq)]
struct Empty {}
vtr::impl_payload!(Empty {});

#[test]
fn only_zero_size_payloads_are_implicitly_ready() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.vtr");
        let mut writer = Writer::create_with(&path, options(mode, Compression::NONE, 0)).unwrap();
        let empty = writer
            .transactions::<Empty>(SourceOptions::new("empty", None), "OWNER")
            .unwrap();
        let array = writer
            .transactions::<[u64; 0]>(SourceOptions::new("array", None), "OWNER")
            .unwrap();
        let sequence = writer
            .transactions::<Vec<()>>(SourceOptions::new("sequence", None), "OWNER")
            .unwrap();
        let stage = writer
            .stages::<Empty>(SourceOptions::new("stage", None))
            .unwrap();
        let owner = writer.begin_deferred(empty, None).unwrap();
        let visit = writer.begin_stage_deferred(stage, owner.owner()).unwrap();
        writer.end(owner, Status::Ok).unwrap();
        let array = writer.begin_deferred(array, None).unwrap();
        writer.end(array, Status::Ok).unwrap();
        let sequence = writer.begin_deferred(sequence, None).unwrap();
        writer
            .end_with_payload(sequence, &vec![], Status::Ok)
            .unwrap();
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        assert_eq!(
            reader
                .lookup(owner.id())
                .unwrap()
                .unwrap()
                .payload()
                .decode::<Empty>()
                .unwrap(),
            Empty {}
        );
        assert_eq!(
            reader
                .lookup(visit.id())
                .unwrap()
                .unwrap()
                .payload()
                .decode::<Empty>()
                .unwrap(),
            Empty {}
        );
        assert_eq!(
            reader
                .lookup(array.id())
                .unwrap()
                .unwrap()
                .payload()
                .decode::<[u64; 0]>()
                .unwrap(),
            []
        );
        assert_eq!(
            reader
                .lookup(sequence.id())
                .unwrap()
                .unwrap()
                .payload()
                .decode::<Vec<()>>()
                .unwrap(),
            []
        );
        let path = dir.path().join("missing-sequence.vtr");
        let mut writer = Writer::create_with(&path, options(mode, Compression::NONE, 0)).unwrap();
        let source = writer
            .transactions::<Vec<()>>(SourceOptions::new("sequence", None), "OWNER")
            .unwrap();
        let missing = writer.begin_deferred(source, None).unwrap();
        assert!(writer.close().is_err());
        writer.seal().unwrap();
        let reader = Reader::open(path).unwrap();
        assert!(reader.lookup(missing.id()).unwrap().is_none());
        reader.validate().unwrap();
    }
}

#[test]
fn missing_payloads_reject_end_and_normal_close_but_sealing_preserves_valid_evidence() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for case in 0..5 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("missing.vtr");
            let mut writer =
                Writer::create_with(&path, options(mode, Compression::LZ4, 0)).unwrap();
            let logger = writer
                .logger(SourceOptions::new("diagnostics", None))
                .unwrap();
            let owners = writer
                .transactions::<u64>(SourceOptions::new("owners", None), "OWNER")
                .unwrap();
            let stages = writer
                .stages::<String>(SourceOptions::new("stages", None))
                .unwrap();
            let logs =
                vtr::logging::declare::<u64>(&mut writer, vtr::logging::CallsiteOptions::new("{}"))
                    .unwrap();
            let good = writer.begin(owners, &42, None).unwrap();
            let missing = writer.begin_deferred(owners, None).unwrap();
            let child = writer.begin(owners, &7, Some(missing.owner())).unwrap();
            writer.end(child, Status::Ok).unwrap();
            let bad_stage = writer.begin_stage_deferred(stages, good.owner()).unwrap();
            let orphan_stage = writer
                .begin_stage(stages, missing.owner(), &"valid stage".into())
                .unwrap();
            let log = writer
                .log(logger, logs, &99, Some(missing.owner()))
                .unwrap();
            writer.flush().unwrap();
            let result = match case {
                0 => writer.end(missing, Status::Ok),
                1 => writer.end_stage(bad_stage),
                2 => writer.end(good, Status::Ok),
                3 => writer.end_with_payload(good, &123, Status::Ok),
                _ => writer.close().map(|_| ()),
            };
            assert!(result.and_then(|()| writer.flush()).is_err());
            assert!(writer.close().is_err());
            writer.seal().unwrap();
            let reader = Reader::open(path).unwrap();
            assert_eq!(reader.completion(), Completion::Incomplete);
            let report = reader.validate().unwrap();
            assert!(
                report
                    .missing
                    .iter()
                    .any(|gap| gap.kind == MissingKind::Parent && gap.target == missing.id())
            );
            assert!(reader.lookup(missing.id()).unwrap().is_none());
            assert!(reader.lookup(bad_stage.id()).unwrap().is_none());
            assert_eq!(
                reader
                    .lookup(good.id())
                    .unwrap()
                    .unwrap()
                    .payload()
                    .decode::<u64>()
                    .unwrap(),
                42
            );
            assert_eq!(
                reader
                    .lookup(orphan_stage.id())
                    .unwrap()
                    .unwrap()
                    .payload()
                    .decode::<String>()
                    .unwrap(),
                "valid stage"
            );
            assert_eq!(
                reader
                    .lookup(child.id())
                    .unwrap()
                    .unwrap()
                    .payload()
                    .decode::<u64>()
                    .unwrap(),
                7
            );
            assert_eq!(
                reader
                    .lookup(log)
                    .unwrap()
                    .unwrap()
                    .payload()
                    .field("value")
                    .unwrap()
                    .decode::<u64>()
                    .unwrap(),
                99
            );
        }
    }
}

#[test]
fn active_payload_updates_check_recording_identity_and_lifetime() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let dir = tempfile::tempdir().unwrap();
        let mut left = Writer::create_with(
            dir.path().join("left.vtr"),
            options(mode, Compression::NONE, 0),
        )
        .unwrap();
        let mut right = Writer::create_with(
            dir.path().join("right.vtr"),
            options(mode, Compression::NONE, 0),
        )
        .unwrap();
        let source = left
            .transactions::<u64>(SourceOptions::new("owners", None), "OWNER")
            .unwrap();
        let stages = left
            .stages::<u64>(SourceOptions::new("stages", None))
            .unwrap();
        let owner = left.begin_deferred(source, None).unwrap();
        let stage = left.begin_stage_deferred(stages, owner.owner()).unwrap();
        assert!(right.payload(owner, &1).is_err());
        assert!(right.end_with_payload(owner, &1, Status::Ok).is_err());
        assert!(right.payload(stage, &1).is_err());
        assert!(right.end_stage_with_payload(stage, &1).is_err());
        assert!(right.begin_deferred(source, None).is_err());
        assert!(right.begin_stage_deferred(stages, owner.owner()).is_err());
        right.close().unwrap();
        left.end_stage_with_payload(stage, &2).unwrap();
        left.end_with_payload(owner, &1, Status::Ok).unwrap();
        let result = left.payload(owner, &3);
        assert!(result.and_then(|()| left.flush()).is_err());
        left.seal().unwrap();
    }
}

#[test]
fn independent_rescue_drains_deferred_captures_without_fabricating_missing_values() {
    for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rescue.vtr");
        let mut writer =
            Writer::create_with(&path, options(ExecutionMode::Threaded, compression, 2)).unwrap();
        let logger = writer
            .logger(SourceOptions::new("diagnostics", None))
            .unwrap();
        let source = writer
            .transactions::<String>(SourceOptions::new("owners", None), "OWNER")
            .unwrap();
        let stages = writer
            .stages::<String>(SourceOptions::new("stages", None))
            .unwrap();
        let logs =
            vtr::logging::declare::<u64>(&mut writer, vtr::logging::CallsiteOptions::new("{}"))
                .unwrap();
        let mut producer = writer.add_producer().unwrap();
        producer.advance_time(5).unwrap();
        let ready = producer.begin_deferred(source, None).unwrap();
        producer.payload(ready, &"retained".into()).unwrap();
        let missing = producer.begin_deferred(source, None).unwrap();
        let missing_stage = producer
            .begin_stage_deferred(stages, ready.owner())
            .unwrap();
        let log = producer
            .log(logger, logs, &99, Some(missing.owner()))
            .unwrap();
        let sealer = writer.sealer();
        writer.guarded(|_| {
            std::thread::spawn(move || sealer.seal(vtr::Ending::Poisoned))
                .join()
                .unwrap()
                .unwrap();
        });
        assert!(writer.close().is_err());
        assert!(producer.payload(missing, &"late".into()).is_err());
        let reader = Reader::open(path).unwrap();
        assert_eq!(reader.completion(), Completion::Incomplete);
        assert!(reader.lookup(missing.id()).unwrap().is_none());
        assert!(reader.lookup(missing_stage.id()).unwrap().is_none());
        assert_eq!(
            reader
                .lookup(ready.id())
                .unwrap()
                .unwrap()
                .payload()
                .decode::<String>()
                .unwrap(),
            "retained"
        );
        assert_eq!(
            reader
                .lookup(log)
                .unwrap()
                .unwrap()
                .payload()
                .field("value")
                .unwrap()
                .decode::<u64>()
                .unwrap(),
            99
        );
        assert_eq!(reader.ending().unwrap().0, vtr::Ending::Poisoned);
        let report = reader.validate().unwrap();
        assert!(
            report
                .missing
                .iter()
                .any(|gap| gap.kind == MissingKind::Parent && gap.target == missing.id())
        );
    }
}
