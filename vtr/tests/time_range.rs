use vtr::{
    Attribute, Compression, ExecutionMode, ReadOptions, Reader, Recorder, SourceOptions, Status,
    TimeWindow, Value, Writer, WriterOptions,
};

#[test]
fn range_includes_detail_outside_owners_without_decoding_payloads_or_filling_owner_cache() {
    let directory = tempfile::tempdir().unwrap();
    for (mode, workers) in [(ExecutionMode::Inline, 0), (ExecutionMode::Threaded, 2)] {
        for compression in [
            Compression::NONE,
            Compression::LZ4,
            Compression::ZSTD_DEFAULT,
        ] {
            let path = directory
                .path()
                .join(format!("{mode:?}-{compression:?}.vtr"));
            let mut options = WriterOptions {
                mode,
                ..Default::default()
            };
            options.encoding.compression = compression;
            options.encoding.workers = workers;
            options.encoding.page_bytes = 1;
            options.encoding.block_records = 1;
            let mut writer = Writer::create_with(&path, options).unwrap();
            let owners = writer
                .transactions::<String>(SourceOptions::new("owners", None), "OWNER")
                .unwrap();
            let stage_name = writer.intern("execute").unwrap();
            let lane = writer.intern("main").unwrap();
            let key = writer.intern("detail").unwrap();
            let owner = writer
                .begin_at(owners, 100, &"payload".repeat(4_000), None)
                .unwrap();
            writer
                .attribute(owner.owner(), key, Value::Bytes(vec![7; 32_000]))
                .unwrap();
            writer
                .embedded_stage(owner.owner(), 50, stage_name, lane, Some(160), vec![])
                .unwrap();
            writer
                .embedded_event(
                    owner.owner(),
                    180,
                    stage_name,
                    vec![Attribute {
                        key,
                        value: Value::Text("event".repeat(4_000)),
                    }],
                )
                .unwrap();
            writer.end_at(owner, 110, Status::Ok).unwrap();
            writer.close().unwrap();
            Reader::open(&path).unwrap().validate().unwrap();
            let reader = Reader::open_with(
                &path,
                ReadOptions {
                    max_payload_bytes: 1,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                reader.time_range().unwrap(),
                Some(TimeWindow {
                    first: 50,
                    last: 180
                })
            );
            assert_eq!(reader.cache_stats().owner_entries, 0);
            assert!(
                reader.lookup(owner.id()).is_err(),
                "payload admission still applies to record reads"
            );
            assert_eq!(
                reader.time_range().unwrap(),
                Some(TimeWindow {
                    first: 50,
                    last: 180
                })
            );
            assert_eq!(reader.cache_stats().owner_entries, 0);
        }
    }
}

#[test]
fn empty_relation_only_owner_located_waveform_and_full_domain_ranges_remain_distinct() {
    let directory = tempfile::tempdir().unwrap();
    for case in 0..7 {
        let path = directory.path().join(format!("range-{case}.vtr"));
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode: ExecutionMode::Inline,
                ..Default::default()
            },
        )
        .unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("owners", None), "OWNER")
            .unwrap();
        let name = writer.intern("sample").unwrap();
        let expected = match case {
            0 => None,
            1 => {
                writer.relate(name, 0, 0, vec![]).unwrap();
                None
            }
            2 => {
                let owner = writer.begin_at(source, 0, &(), None).unwrap();
                writer.end_at(owner, 0, Status::Ok).unwrap();
                Some((0, 0))
            }
            3 => {
                let owner = writer.begin_at(source, 100, &(), None).unwrap();
                writer
                    .embedded_stage(owner.owner(), 0, name, name, Some(u64::MAX), vec![])
                    .unwrap();
                writer.end_at(owner, 110, Status::Ok).unwrap();
                Some((0, u64::MAX))
            }
            4 => {
                let owner = writer.begin_at(source, 100, &(), None).unwrap();
                writer
                    .embedded_stage(owner.owner(), 200, name, name, None, vec![])
                    .unwrap();
                writer.end_at(owner, 110, Status::Ok).unwrap();
                Some((100, 200))
            }
            5 => {
                let owner = writer.begin_at(source, 100, &(), None).unwrap();
                let events = writer
                    .events::<()>(SourceOptions::new("events", None))
                    .unwrap();
                writer.event_at(events, owner.owner(), 20, &()).unwrap();
                let stages = writer
                    .stages::<()>(SourceOptions::new("stages", None))
                    .unwrap();
                let stage = writer
                    .begin_stage_at(stages, owner.owner(), 200, &())
                    .unwrap();
                writer.end_stage_at(stage, 250).unwrap();
                writer.end_at(owner, 110, Status::Ok).unwrap();
                Some((20, 250))
            }
            6 => {
                writer.advance_time(500).unwrap();
                let owner = writer.begin_at(source, 100, &(), None).unwrap();
                writer
                    .embedded_event(owner.owner(), 20, name, vec![])
                    .unwrap();
                writer.end_at(owner, 110, Status::Ok).unwrap();
                Some((20, 500))
            }
            _ => unreachable!(),
        };
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        assert_eq!(
            reader.time_range().unwrap().map(|w| (w.first, w.last)),
            expected,
            "case {case}"
        );
    }
}
