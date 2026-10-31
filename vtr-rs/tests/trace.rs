use vtr::{
    attributes::Value,
    records::{RecordKind, RecordQuery},
    schema::{Field, Payload, PayloadReader, Primitive, TypeId, Types},
    trace::{ReadOptions, Reader},
    transactions::TransactionQuery,
};

#[test]
fn nested_expansion_survives_cache_eviction_clearing_and_disabled_caches() {
    use vtr::{ExecutionMode, Recorder, SourceOptions, Status, Writer, WriterOptions};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested.vtr");
    let mut options = WriterOptions {
        mode: ExecutionMode::Inline,
        ..Default::default()
    };
    options.encoding.page_bytes = 256;
    let mut writer = Writer::create_with(&path, options).unwrap();
    let source = writer
        .transactions::<u64>(SourceOptions::new("owners", None), "OWNER")
        .unwrap();
    let stages = writer
        .stages::<String>(SourceOptions::new("stages", None))
        .unwrap();
    let mut expected = Vec::new();
    for i in 0..12u64 {
        let owner = writer.begin_at(source, i * 10, &i, None).unwrap();
        let payload = format!("stage {i}: {}", "x".repeat(80));
        let stage = writer
            .begin_stage_at(stages, owner.owner(), i * 10 + 1, &payload)
            .unwrap();
        writer.end_stage_at(stage, i * 10 + 5).unwrap();
        writer.end_at(owner, i * 10 + 9, Status::Ok).unwrap();
        writer.flush().unwrap();
        expected.push((owner.id(), stage.id(), payload));
    }
    writer.close().unwrap();
    for owner_limit in [0, 1, 2] {
        for page_limit in [0, 1, 256, 1 << 20] {
            let reader = Reader::open_with(
                &path,
                ReadOptions {
                    owner_cache_entries: owner_limit,
                    located_page_cache_bytes: page_limit,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut count = 0;
            let mut retained = None;
            reader
                .transactions(
                    TransactionQuery {
                        generator: Some(source.id()),
                        ..Default::default()
                    },
                    |owner| {
                        if owner_limit != 0 {
                            assert!(reader.cache_stats().owner_entries > 0);
                        }
                        let (id, stage, payload) = &expected[count];
                        assert_eq!(owner.id(), *id);
                        let expansion = reader.expand(*id).unwrap();
                        assert!(expansion.complete);
                        assert_eq!(expansion.located.len(), 1);
                        assert_eq!(expansion.located[0].id(), *stage);
                        assert_eq!(expansion.located[0].decode::<String>().unwrap(), *payload);
                        retained = Some(expansion.located[0].clone());
                        if count % 3 == 0 {
                            reader.clear_cache();
                        }
                        let stats = reader.cache_stats();
                        assert!(stats.owner_entries <= owner_limit);
                        assert!(stats.located_page_bytes <= page_limit);
                        count += 1;
                        true
                    },
                )
                .unwrap();
            assert_eq!(count, expected.len());
            let stats = reader.cache_stats();
            if page_limit <= 1 {
                assert_eq!(stats.located_pages, 0);
            } else {
                assert!(stats.located_pages > 0);
            }
            reader.clear_cache();
            assert_eq!(reader.cache_stats().located_page_bytes, 0);
            drop(reader);
            assert_eq!(
                retained.unwrap().decode::<String>().unwrap(),
                expected.last().unwrap().2
            );
        }
    }
}

#[test]
fn relation_scan_preserves_duplicates_outside_endpoints_and_cancellation_across_blocks() {
    use vtr::{ExecutionMode, Recorder, SourceOptions, Status, Writer, WriterOptions};
    for (mode, workers) in [
        (ExecutionMode::Inline, 0),
        (ExecutionMode::Threaded, 0),
        (ExecutionMode::Threaded, 2),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("relations.vtr");
        let mut options = WriterOptions {
            mode,
            ..WriterOptions::default()
        };
        options.encoding.block_records = 2;
        options.encoding.workers = workers;
        let mut writer = Writer::create_with(&path, options).unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("owners", None), "OWNER")
            .unwrap();
        let first = writer.begin(source, &(), None).unwrap();
        let second = writer.begin(source, &(), None).unwrap();
        let kind = writer.intern("depends").unwrap();
        let endpoints = [
            (first.id(), second.id()),
            (first.id(), second.id()),
            (0, first.id()),
            (second.id(), 0),
            (second.id(), first.id()),
        ];
        for (index, &(from, to)) in endpoints.iter().enumerate() {
            writer.relate(kind, from, to, vec![]).unwrap();
            if index == 1 {
                writer.flush().unwrap();
            }
        }
        writer.end(first, Status::Ok).unwrap();
        writer.end(second, Status::Ok).unwrap();
        writer.close().unwrap();
        let reader = Reader::open(&path).unwrap();
        reader.validate().unwrap();
        let mut seen = Vec::new();
        reader
            .scan_relations(|edge| {
                seen.push((edge.source, edge.destination));
                true
            })
            .unwrap();
        assert_eq!(seen, endpoints);
        for stop in [1, 3, endpoints.len()] {
            let mut count = 0;
            reader
                .scan_relations(|_| {
                    count += 1;
                    count < stop
                })
                .unwrap();
            assert_eq!(count, stop);
        }
        let mut outgoing = Vec::new();
        reader
            .relations(first.id(), false, |edge| {
                outgoing.push((edge.source, edge.destination));
                true
            })
            .unwrap();
        assert_eq!(outgoing, endpoints[..2]);
    }
}

#[derive(Debug, PartialEq)]
struct Instruction {
    pc: u64,
    opcode: u64,
}

impl Payload for Instruction {
    fn bind(types: &mut Types) -> vtr::Result<TypeId> {
        types.structure(
            "Instruction",
            vec![
                Field::new("pc", Primitive::U64.id()),
                Field::new("opcode", Primitive::U64.id()),
            ],
        )
    }
    fn encode(&self, out: &mut Vec<u8>) -> vtr::Result<()> {
        self.pc.encode(out)?;
        self.opcode.encode(out)
    }
    fn decode(reader: &mut PayloadReader<'_>) -> vtr::Result<Self> {
        Ok(Self {
            pc: u64::decode(reader)?,
            opcode: u64::decode(reader)?,
        })
    }
}

#[test]
fn cpp_typed_trace_retains_schema_and_records_after_reader_and_cache_destruction() {
    let reader = Reader::from_bytes_with(
        include_bytes!("fixtures/typed.vtr").to_vec(),
        ReadOptions {
            owner_cache_entries: 1,
            owner_cache_bytes: 4096,
            ..ReadOptions::default()
        },
    )
    .unwrap();
    let generator = reader.find_generator("core0.instructions").unwrap();
    let execute = reader.find_generator("core0.execute").unwrap();
    let mut owners = Vec::new();
    reader
        .transactions(TransactionQuery::default(), |record| {
            owners.push(record);
            true
        })
        .unwrap();
    assert_eq!(owners.len(), 1);
    let owner = owners.pop().unwrap();
    assert_eq!(owner.generator(), Some(generator));
    assert_eq!((owner.begin(), owner.end()), (10, 15));
    assert_eq!(
        owner.payload().decode::<Instruction>().unwrap(),
        Instruction {
            pc: 0x8000,
            opcode: 0x13
        }
    );
    assert!(owner.payload().decode::<[u64; 2]>().is_err());
    assert_eq!(
        owner.payload().field("pc").unwrap().scalar().unwrap(),
        Value::U64(0x8000)
    );
    let expansion = reader.expand(owner.id()).unwrap();
    assert!(expansion.complete);
    assert_eq!(expansion.located.len(), 2);
    assert_eq!(expansion.located[0].kind(), RecordKind::Stage);
    assert_eq!(expansion.located[0].generator(), Some(execute));
    assert_eq!(
        (expansion.located[0].begin(), expansion.located[0].end()),
        (11, 14)
    );
    assert_eq!(expansion.located[1].kind(), RecordKind::Event);
    let mut at_end = 0;
    reader
        .records(
            RecordQuery {
                generator: Some(execute),
                begin: 14,
                end: 14,
                ..RecordQuery::default()
            },
            |_| {
                at_end += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(at_end, 0);
    let mut at_begin = 0;
    reader
        .records(
            RecordQuery {
                begin: 11,
                end: 11,
                ..RecordQuery::default()
            },
            |_| {
                at_begin += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(at_begin, 1);
    let retained = reader.lookup(owner.id()).unwrap().unwrap();
    let payload = retained.payload().field("opcode").unwrap();
    reader.clear_cache();
    assert_eq!(reader.cache_stats().owner_entries, 0);
    let report = reader.validate().unwrap();
    assert_eq!(report.records, 3);
    assert_eq!(report.relations, 0);
    assert!(report.missing.is_empty());
    drop(reader);
    assert_eq!(payload.decode::<u64>().unwrap(), 0x13);
    assert_eq!(
        retained.payload().decode::<Instruction>().unwrap().pc,
        0x8000
    );
}
