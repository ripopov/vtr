use vtr::{
    ExecutionMode, Reader, RecordKind, RecordQuery, Recorder, SourceOptions, Status, Writer,
    WriterOptions,
};

#[test]
fn logger_selects_callsite_schemas_and_preserves_fixed_and_embedded_records() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mixed.vtr");
        let mut options = WriterOptions {
            mode,
            ..Default::default()
        };
        options.encoding.page_bytes = 128;
        options.encoding.block_records = 4;
        let mut writer = Writer::create_with(&path, options).unwrap();
        let logger = writer.logger(SourceOptions::new("shader", None)).unwrap();
        let other = writer.logger(SourceOptions::new("memory", None)).unwrap();
        let request = vtr::logging::declare::<u64>(
            &mut writer,
            vtr::logging::CallsiteOptions::new("request {:#x}"),
        )
        .unwrap();
        let retry = vtr::logging::declare::<String>(
            &mut writer,
            vtr::logging::CallsiteOptions {
                severity: 3,
                ..vtr::logging::CallsiteOptions::new("retry {}")
            },
        )
        .unwrap();
        let second_request = vtr::logging::declare::<u64>(
            &mut writer,
            vtr::logging::CallsiteOptions::new("completed {}"),
        )
        .unwrap();
        let owners = writer
            .transactions::<u64>(SourceOptions::new("owners", None), "WORK")
            .unwrap();
        let events = writer
            .events::<u32>(SourceOptions::new("events", None))
            .unwrap();
        let stages = writer
            .stages::<String>(SourceOptions::new("stages", None))
            .unwrap();
        let name = writer.intern("internal").unwrap();
        let owner = writer.begin(owners, &42, None).unwrap();
        let stage = writer
            .begin_stage(stages, owner.owner(), &"execute".into())
            .unwrap();
        let event = writer.event(events, owner.owner(), &7).unwrap();
        let embedded = writer
            .embedded_event(owner.owner(), 0, name, vec![])
            .unwrap();
        let embedded_stage = writer
            .embedded_stage(owner.owner(), 0, name, name, Some(1), vec![])
            .unwrap();
        let mut requests = Vec::new();
        let mut retries = Vec::new();
        for _ in 0..12 {
            requests.push(
                writer
                    .log(logger, request, &128, Some(owner.owner()))
                    .unwrap(),
            );
            retries.push(writer.log(logger, retry, &"bank 3".into(), None).unwrap());
        }
        let shared = writer.log(other, request, &192, None).unwrap();
        let completed = writer.log(logger, second_request, &128, None).unwrap();
        writer.advance_time(2).unwrap();
        writer.end_stage(stage).unwrap();
        writer.end(owner, Status::Ok).unwrap();
        writer.close().unwrap();
        let reader = Reader::open(&path).unwrap();
        assert_eq!(reader.validate().unwrap().records, 31);
        assert_eq!(vtr::logging::callsites(&reader).unwrap().len(), 3);
        let first = reader.declarations().types.get(request.id()).unwrap();
        let second = reader
            .declarations()
            .types
            .get(second_request.id())
            .unwrap();
        assert_ne!(request.id(), second_request.id());
        assert_eq!(first.kind, second.kind);
        let mut selected = Vec::new();
        reader
            .records(
                RecordQuery {
                    generator: Some(logger.id()),
                    datatype: Some(request.id()),
                    ..Default::default()
                },
                |record| {
                    selected.push(record);
                    true
                },
            )
            .unwrap();
        let mut actual: Vec<_> = selected.iter().map(vtr::Record::id).collect();
        actual.sort_unstable();
        assert_eq!(actual, requests);
        assert!(selected.iter().all(|r| r.datatype() == request.id()
            && r.payload().field("value").unwrap().decode::<u64>().unwrap() == 128));
        assert_eq!(
            vtr::logging::format_log(&reader, &reader.lookup(shared).unwrap().unwrap()).unwrap(),
            "request 0xc0"
        );
        assert_eq!(
            vtr::logging::format_log(&reader, &reader.lookup(completed).unwrap().unwrap()).unwrap(),
            "completed 128"
        );
        assert_eq!(
            reader
                .lookup(event)
                .unwrap()
                .unwrap()
                .decode::<u32>()
                .unwrap(),
            7
        );
        assert_eq!(
            reader
                .lookup(stage.id())
                .unwrap()
                .unwrap()
                .decode::<String>()
                .unwrap(),
            "execute"
        );
        for (id, kind) in [
            (embedded, RecordKind::EmbeddedEvent),
            (embedded_stage, RecordKind::EmbeddedStage),
        ] {
            let record = reader.lookup(id).unwrap().unwrap();
            assert_eq!(record.kind(), kind);
            assert_eq!(record.payload().datatype(), 0);
            assert!(record.generator().is_none());
        }
        let retained = reader.lookup(retries[0]).unwrap().unwrap();
        drop(reader);
        assert_eq!(
            retained
                .payload()
                .field("value")
                .unwrap()
                .decode::<String>()
                .unwrap(),
            "bank 3"
        );
        assert_eq!(retained.datatype(), retry.id());
    }
}

#[test]
fn explicit_log_capture_checks_schema_references_and_consumes_all_bytes() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for case in 0..5 {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("raw.vtr");
            let mut writer = Writer::create_with(
                &path,
                WriterOptions {
                    mode,
                    ..Default::default()
                },
            )
            .unwrap();
            let logger = writer.logger(SourceOptions::new("logger", None)).unwrap();
            let datatype = writer.bind::<u64>().unwrap();
            let callsite = vtr::logging::declare_raw(
                &mut writer,
                vtr::logging::CallsiteOptions::new("{}"),
                datatype,
            )
            .unwrap();
            let mut bytes = 42u64.to_le_bytes().to_vec();
            let callsite = match case {
                3 => u32::MAX,
                4 => logger.id(),
                _ => callsite,
            };
            if case == 1 {
                bytes.push(0);
            }
            if case == 2 {
                bytes.pop();
            }
            let result = writer
                .log_raw(logger, callsite, 0, &bytes, None)
                .and_then(|_| writer.close());
            if case != 0 {
                assert!(result.is_err());
            } else {
                result.unwrap();
                let reader = Reader::open(path).unwrap();
                reader.validate().unwrap();
                assert_eq!(
                    reader
                        .lookup(1)
                        .unwrap()
                        .unwrap()
                        .payload()
                        .field("value")
                        .unwrap()
                        .decode::<u64>()
                        .unwrap(),
                    42
                );
            }
        }
    }
}

#[test]
fn datatype_metadata_is_opaque_to_storage_and_invalid_references_are_rejected() {
    use vtr::{
        Attribute, DatatypeOptions, Value,
        framing::{Container, FramedWriter},
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("references.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let logger = writer.logger(SourceOptions::new("logger", None)).unwrap();
    let kind = writer.intern("kind").unwrap();
    let format = writer.intern("log.format").unwrap();
    let datatype = writer
        .datatype::<u64>(DatatypeOptions {
            name: "arbitrary".into(),
            attributes: vec![
                Attribute {
                    key: kind,
                    value: Value::Text("callsite".into()),
                },
                Attribute {
                    key: format,
                    value: Value::U64(42),
                },
            ],
        })
        .unwrap();
    writer.log(logger, datatype, &42, None).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(&path).unwrap();
    reader.validate().unwrap();
    assert!(vtr::logging::metadata(&reader, datatype.id()).is_err());
    let container = Container::open(path, true).unwrap();
    for reference in [u32::MAX, 19] {
        let mut output = FramedWriter::with_identity(Vec::new(), container.identity).unwrap();
        for section in container.sections() {
            let mut payload = container.payload(section, true).unwrap().to_vec();
            if section.kind == 9 {
                payload[60..64].copy_from_slice(&u32::to_le_bytes(reference));
            }
            output
                .section(
                    section.kind,
                    section.flags,
                    &payload,
                    section.aux0,
                    section.aux1,
                    section.block_number,
                )
                .unwrap();
        }
        output.finish(false, 0).unwrap();
        assert!(Reader::from_bytes(output.into_inner()).is_err());
    }
}

#[test]
fn callsite_queries_skip_unselected_pages_before_decompression() {
    use vtr::records::{
        Generator, Generators, LocatedRecord, RecordBlockView, RecordPolicy, encode_record_block,
    };
    let types = vtr::Types::new();
    let mut generators = Generators::default();
    generators
        .declare(Generator {
            id: 0,
            kind: RecordKind::Log,
            payload_type: 0,
        })
        .unwrap();

    let records = [
        LocatedRecord {
            id: 1,
            generator: 0,
            datatype_id: 9,
            begin: 0,
            end: 0,
            parent: 0,
            payload: 42u64.to_le_bytes().to_vec(),
        },
        LocatedRecord {
            id: 2,
            generator: 0,
            datatype_id: 13,
            begin: 0,
            end: 0,
            parent: 0,
            payload: vtr::schema::capture(&"retry".to_string()).unwrap(),
        },
    ];
    let mut encoded = encode_record_block(
        1,
        &types,
        &generators,
        &records,
        RecordPolicy {
            page_bytes: 1,
            compression: vtr::Compression::NONE,
            separate_logs: true,
        },
        0,
    )
    .unwrap();
    let view = RecordBlockView::with_defaults(&encoded, &types, &generators).unwrap();
    assert_eq!(view.pages().len(), 2);
    let blobs_start = encoded.len()
        - view
            .pages()
            .iter()
            .map(|page| page.blob_length as usize)
            .sum::<usize>();
    let unselected = blobs_start + view.pages()[1].blob_offset as usize;
    encoded[unselected] = 255;
    let view = RecordBlockView::with_defaults(&encoded, &types, &generators).unwrap();
    let mut ids = Vec::new();
    view.query(
        RecordQuery {
            datatype: Some(9),
            ..Default::default()
        },
        |record| {
            ids.push(record.id);
            true
        },
    )
    .unwrap();
    assert_eq!(ids, [1]);
    assert!(view.validate().is_err());
}

#[test]
fn later_declarations_cannot_repair_an_earlier_log_block() {
    use vtr::framing::{Container, FramedWriter};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("late.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let logger = writer.logger(SourceOptions::new("logger", None)).unwrap();
    let early =
        vtr::logging::declare::<u64>(&mut writer, vtr::logging::CallsiteOptions::new("early {}"))
            .unwrap();
    writer.log(logger, early, &1, None).unwrap();
    writer.flush().unwrap();
    let late =
        vtr::logging::declare::<String>(&mut writer, vtr::logging::CallsiteOptions::new("late {}"))
            .unwrap();
    writer.log(logger, late, &"retry".into(), None).unwrap();
    writer.close().unwrap();
    let container = Container::open(path, true).unwrap();
    let deferred = container
        .sections()
        .iter()
        .rposition(|section| section.kind == 10)
        .unwrap();
    let mut output = FramedWriter::with_identity(Vec::new(), container.identity).unwrap();
    for section in container
        .sections()
        .iter()
        .enumerate()
        .filter_map(|(index, section)| (index != deferred).then_some(section))
        .chain(std::iter::once(&container.sections()[deferred]))
    {
        output
            .section(
                section.kind,
                section.flags,
                container.payload(section, true).unwrap(),
                section.aux0,
                section.aux1,
                section.block_number,
            )
            .unwrap();
    }
    output.finish(false, 0).unwrap();
    assert!(Reader::from_bytes(output.into_inner()).is_err());
}
