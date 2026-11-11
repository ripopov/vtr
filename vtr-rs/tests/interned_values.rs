use vtr::{
    Attribute, Compression, ExecutionMode, Reader, Recorder, SourceOptions, Status, Value, ValueId,
    Writer, WriterOptions,
};

#[test]
fn shared_instruction_descriptions_survive_flushes_and_recovery() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("instructions.vtr");
            let mut options = WriterOptions {
                mode,
                ..Default::default()
            };
            options.encoding.compression = compression;
            options.encoding.block_records = 2;
            let mut writer = Writer::create_with(&path, options).unwrap();
            let mnemonic = writer.intern("mnemonic").unwrap();
            let operation_key = writer.intern("operation").unwrap();
            let instruction_key = writer.intern("instruction").unwrap();
            let mask = writer.intern("exec_mask").unwrap();
            let operation = writer
                .intern_value(Value::Map(vec![Attribute {
                    key: mnemonic,
                    value: Value::Text("FADD".into()),
                }]))
                .unwrap();
            let description = Value::List(vec![
                Value::Pointer(0x1040),
                Value::Bytes(vec![1, 2, 3, 4]),
                Value::Map(vec![Attribute {
                    key: operation_key,
                    value: Value::Ref(operation),
                }]),
            ]);
            let instruction = writer.intern_value(description.clone()).unwrap();
            assert_eq!(writer.intern_value(description).unwrap(), instruction);
            let source = writer
                .transactions::<()>(SourceOptions::new("instructions", None), "INSTRUCTION")
                .unwrap();
            for time in 0..4 {
                writer.advance_time(time).unwrap();
                let tx = writer.begin(source, &(), None).unwrap();
                writer
                    .attribute(tx.owner(), instruction_key, Value::Ref(instruction))
                    .unwrap();
                writer
                    .attribute(tx.owner(), mask, Value::U64(1 << time))
                    .unwrap();
                let shared = vec![Attribute {
                    key: instruction_key,
                    value: Value::Ref(instruction),
                }];
                writer
                    .embedded_event(tx.owner(), time, mnemonic, shared.clone())
                    .unwrap();
                writer
                    .embedded_stage(
                        tx.owner(),
                        time,
                        operation_key,
                        0,
                        Some(time + 1),
                        shared.clone(),
                    )
                    .unwrap();
                writer.end(tx, Status::Ok).unwrap();
                writer
                    .relate(operation_key, tx.id(), tx.id(), shared)
                    .unwrap();
            }
            writer.flush().unwrap();
            let updated_description = Value::List(vec![
                Value::Pointer(0x1040),
                Value::Bytes(vec![5, 6, 7, 8]),
                Value::Map(vec![Attribute {
                    key: operation_key,
                    value: Value::Ref(operation),
                }]),
            ]);
            let updated = writer.intern_value(updated_description.clone()).unwrap();
            assert_ne!(instruction, updated);
            writer.advance_time(5).unwrap();
            let tx = writer.begin(source, &(), None).unwrap();
            writer
                .attribute(tx.owner(), instruction_key, Value::Ref(updated))
                .unwrap();
            writer.end(tx, Status::Ok).unwrap();
            writer.close().unwrap();
            let bytes = std::fs::read(path).unwrap();
            for input in [bytes.clone(), bytes[..bytes.len() - 1].to_vec()] {
                let reader = Reader::from_bytes(input).unwrap();
                reader.validate().unwrap();
                let mut references = Vec::new();
                let mut masks = Vec::new();
                reader
                    .transactions(Default::default(), |tx| {
                        for attr in tx.attributes() {
                            if attr.key == instruction_key {
                                let Value::Ref(id) = attr.value else {
                                    panic!("reference lost")
                                };
                                references.push(id);
                            } else if attr.key == mask {
                                masks.push(attr.value.clone());
                            }
                        }
                        if tx.begin() < 4 {
                            assert_eq!(tx.events()[0].attributes[0].value, Value::Ref(instruction));
                            assert_eq!(tx.stages()[0].attributes[0].value, Value::Ref(instruction));
                        }
                        true
                    })
                    .unwrap();
                assert_eq!(
                    references,
                    [instruction, instruction, instruction, instruction, updated]
                );
                assert_eq!(
                    masks,
                    [Value::U64(1), Value::U64(2), Value::U64(4), Value::U64(8)]
                );
                let mut relations = 0;
                reader
                    .scan_relations(|relation| {
                        assert_eq!(relation.attributes[0].value, Value::Ref(instruction));
                        relations += 1;
                        true
                    })
                    .unwrap();
                assert_eq!(relations, 4);
                assert_eq!(
                    reader
                        .format_value(reader.value(instruction).unwrap())
                        .unwrap(),
                    "[4160, 01020304, {operation: {mnemonic: \"FADD\"}}]"
                );
                assert_eq!(
                    reader.value(operation).unwrap(),
                    &Value::Map(vec![Attribute {
                        key: mnemonic,
                        value: Value::Text("FADD".into())
                    }])
                );
                assert_eq!(reader.value(updated).unwrap(), &updated_description);
            }
        }
    }
}

#[test]
fn interning_preserves_types_float_bits_and_order_and_rejects_unknown_references() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = Writer::create(dir.path().join("typed.vtr")).unwrap();
    let values = [
        Value::U8(1),
        Value::U64(1),
        Value::I8(1),
        Value::F32(0.0),
        Value::F32(-0.0),
        Value::F32(f32::from_bits(0x7fc0_0001)),
        Value::F32(f32::from_bits(0x7fc0_0002)),
        Value::Text("1".into()),
        Value::List(vec![Value::U8(1), Value::U8(2)]),
        Value::List(vec![Value::U8(2), Value::U8(1)]),
    ];
    let mut ids = Vec::new();
    for value in &values {
        let id = writer.intern_value(value.clone()).unwrap();
        assert_eq!(writer.intern_value(value.clone()).unwrap(), id);
        assert!(!ids.contains(&id));
        ids.push(id);
    }

    writer.close().unwrap();
    let reader = Reader::open(dir.path().join("typed.vtr")).unwrap();
    for (id, original) in ids.into_iter().zip(values) {
        match (original, reader.value(id).unwrap()) {
            (Value::F32(expected), Value::F32(actual)) => {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
            (expected, actual) => assert_eq!(actual, &expected),
        }
    }
    for value in [
        Value::Ref(ValueId(0)),
        Value::List(vec![Value::Ref(ValueId(u32::MAX))]),
    ] {
        let mut writer = Writer::create(dir.path().join("invalid.vtr")).unwrap();
        assert!(writer.intern_value(value).is_err());
    }
}

#[test]
fn later_value_declarations_cannot_repair_transaction_or_relation_references() {
    use vtr::framing::{Container, FramedWriter};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ordered.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let key = writer.intern("instruction").unwrap();
    let id = writer.intern_value(Value::Text("FADD".into())).unwrap();
    let source = writer
        .transactions::<()>(SourceOptions::new("instructions", None), "INSTRUCTION")
        .unwrap();
    let tx = writer.begin(source, &(), None).unwrap();
    writer
        .attribute(tx.owner(), key, Value::List(vec![Value::Ref(id)]))
        .unwrap();
    writer.end(tx, Status::Ok).unwrap();
    writer
        .relate(
            key,
            tx.id(),
            tx.id(),
            vec![Attribute {
                key,
                value: Value::List(vec![Value::Ref(id)]),
            }],
        )
        .unwrap();
    writer.close().unwrap();
    let container = Container::from_bytes(std::fs::read(path).unwrap(), true).unwrap();
    let copy = |out: &mut FramedWriter<Vec<u8>>, section: &vtr::framing::Section| {
        out.section(
            section.kind,
            section.flags,
            container.payload(section, true).unwrap(),
            section.aux0,
            section.aux1,
            section.block_number,
        )
        .unwrap();
    };
    let mut out = FramedWriter::with_identity(Vec::new(), container.identity).unwrap();
    for section in container
        .sections()
        .iter()
        .filter(|section| section.kind != 11)
    {
        copy(&mut out, section);
    }
    for section in container
        .sections()
        .iter()
        .filter(|section| section.kind == 11)
    {
        copy(&mut out, section);
    }
    out.finish(false, 0).unwrap();
    let reader = Reader::from_bytes(out.into_inner()).unwrap();
    assert!(reader.validate().is_err());
    assert!(reader.transactions(Default::default(), |_| true).is_err());
    assert!(reader.scan_relations(|_| true).is_err());

    let mut declarations = vtr::Declarations::new();
    for tagged in [vec![25, 0], vec![23, 1, 25, 0]] {
        let mut chunk = vec![0, 1];
        vtr::varint::put_blob(&mut chunk, &tagged);
        assert!(declarations.read_values(&chunk).is_err());
        assert!(declarations.values().is_empty());
    }
}
