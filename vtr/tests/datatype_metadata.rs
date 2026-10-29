use vtr::schema::{Field, TypeKind, Types};
use vtr::{Attribute, Value};

#[test]
fn logging_key_lookup_uses_the_first_text_match_across_string_aliases() {
    use vtr::{Declarations, Metadata, Reader, framing::FramedWriter, logging::Severity};
    let mut declarations = Declarations::new();
    declarations.strings = [
        "",
        "kind",
        "kind",
        "log.format",
        "log.format",
        "log.severity",
        "log.severity",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let attributes = [
        Value::Text("callsite".into()),
        Value::Text("other".into()),
        Value::Text("first {}".into()),
        Value::Text("second {}".into()),
        Value::Enum { value: 2, name: 0 },
        Value::Enum { value: 4, name: 0 },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, value)| Attribute {
        key: index as u32 + 1,
        value,
    })
    .collect();
    let datatype = declarations
        .types
        .declare_with_attributes(
            "Statement".into(),
            TypeKind::Struct(vec![Field::new("value", 9)]),
            attributes,
        )
        .unwrap();
    let strings = declarations.strings_chunk(0).unwrap();
    let mut stored = vec![0];
    vtr::varint::put_u64(&mut stored, strings.len() as u64);
    stored.extend(strings);
    let mut framed = FramedWriter::new(Vec::new()).unwrap();
    framed.section(2, 0, &stored, 0, 7, 0).unwrap();
    framed
        .section(1, 0, &Metadata::default().encode(7).unwrap(), 0, 0, 0)
        .unwrap();
    framed
        .section(
            10,
            0,
            &declarations.types.declarations(23).unwrap(),
            23,
            1,
            0,
        )
        .unwrap();
    framed.finish(false, 0).unwrap();
    let reader = Reader::from_bytes(framed.into_inner()).unwrap();
    reader.validate().unwrap();
    let metadata = vtr::logging::metadata(&reader, datatype).unwrap().unwrap();
    assert_eq!(metadata.format, "first {}");
    assert_eq!(metadata.severity, Severity::Info);
}

#[test]
fn composed_datatypes_preserve_independent_metadata_and_canonical_payloads() {
    let mut types = Types::default();
    let arguments = types
        .structure("Arguments", vec![Field::new("request", 9)])
        .unwrap();
    let metadata = |message: &str| {
        vec![
            Attribute {
                key: 0,
                value: Value::Str(1),
            },
            Attribute {
                key: 2,
                value: Value::Text(message.into()),
            },
            Attribute {
                key: 3,
                value: Value::Map(vec![Attribute {
                    key: 4,
                    value: Value::List(vec![Value::I64(-7), Value::Bytes(vec![0, 255])]),
                }]),
            },
        ]
    };
    let kind = TypeKind::Struct(vec![Field::new("args", arguments)]);
    let first = types
        .declare_with_attributes("Message".into(), kind.clone(), metadata("started {}"))
        .unwrap();
    let second = types
        .declare_with_attributes("Message".into(), kind, metadata("failed {}"))
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(types.get(first).unwrap().fixed_size, Some(8));
    assert_eq!(types.get(second).unwrap().fixed_size, Some(8));
    let bytes = 42u64.to_le_bytes();
    for id in [arguments, first, second] {
        types.validate(id, &bytes, 5).unwrap();
    }
    let encoded = types.declarations(23).unwrap();
    let mut decoded = Types::default();
    decoded.read_declarations_with_strings(&encoded, 5).unwrap();
    for id in [arguments, first, second] {
        assert_eq!(
            types.get(id).unwrap().attributes,
            decoded.get(id).unwrap().attributes
        );
        assert_eq!(types.get(id).unwrap().kind, decoded.get(id).unwrap().kind);
    }
    assert_eq!(decoded.declarations(23).unwrap(), encoded);
    let mut other = Types::default();
    assert!(other.read_declarations_with_strings(&encoded, 1).is_err());
    assert_eq!(other.len(), 23);
    for end in 0..encoded.len() {
        assert!(
            other
                .read_declarations_with_strings(&encoded[..end], 5)
                .is_err()
        );
        assert_eq!(other.len(), 23);
    }
}

#[test]
fn metadata_is_generic_and_empty_metadata_preserves_the_declaration_encoding() {
    let mut plain = Types::default();
    let first = plain.structure("A", vec![Field::new("x", 9)]).unwrap();
    let mut explicit = Types::default();
    explicit
        .declare_with_attributes(
            "A".into(),
            TypeKind::Struct(vec![Field::new("x", 9)]),
            vec![],
        )
        .unwrap();
    assert_eq!(
        plain.declarations(23).unwrap(),
        explicit.declarations(23).unwrap()
    );
    let attributes = vec![Attribute {
        key: 0,
        value: Value::Bool(true),
    }];
    for kind in [
        TypeKind::Array {
            element: first,
            count: 2,
        },
        TypeKind::Sequence { element: first },
        TypeKind::Enum {
            underlying: 4,
            values: vec![],
        },
    ] {
        plain
            .declare_with_attributes("custom".into(), kind, attributes.clone())
            .unwrap();
    }
    let bytes = plain.declarations(23).unwrap();
    let mut decoded = Types::default();
    decoded.read_declarations_with_strings(&bytes, 1).unwrap();
    assert_eq!(decoded.declarations(23).unwrap(), bytes);
    assert!(
        decoded
            .declare_with_attributes(
                "bad".into(),
                TypeKind::Struct(vec![]),
                vec![attributes[0].clone(), attributes[0].clone()]
            )
            .is_err()
    );
    let mut empty_flag = explicit.declarations(23).unwrap();
    empty_flag[2] |= 0x80;
    empty_flag.push(0);
    assert!(Types::default().read_declarations(&empty_flag).is_err());
}

#[test]
fn one_logger_selects_generic_metadata_datatypes_in_inline_and_threaded_capture() {
    use vtr::{
        DatatypeOptions, ExecutionMode, Reader, RecordQuery, Recorder, SourceOptions, Writer,
        WriterOptions,
    };
    for execution in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metadata.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode: execution,
                ..Default::default()
            },
        )
        .unwrap();
        let logger = writer.logger(SourceOptions::new("logger", None)).unwrap();
        let key = writer.intern("arbitrary.application.metadata").unwrap();
        let first = writer
            .datatype::<u64>(DatatypeOptions {
                name: "request".into(),
                attributes: vec![Attribute {
                    key,
                    value: Value::Text("started".into()),
                }],
            })
            .unwrap();
        let second = writer
            .datatype::<String>(DatatypeOptions {
                name: "message".into(),
                attributes: vec![Attribute {
                    key,
                    value: Value::List(vec![Value::U64(7)]),
                }],
            })
            .unwrap();
        writer.log(logger, first, &42, None).unwrap();
        let mut borrowed = String::from("original");
        writer.log(logger, second, &borrowed, None).unwrap();
        borrowed.clear();
        writer.close().unwrap();
        let reader = Reader::open(&path).unwrap();
        let mut retained = None;
        reader
            .records(
                RecordQuery {
                    datatype: Some(second.id()),
                    ..Default::default()
                },
                |record| {
                    assert_eq!(record.generator(), Some(logger.id()));
                    assert_eq!(record.datatype(), second.id());
                    retained = Some(record.payload().field("value").unwrap());
                    true
                },
            )
            .unwrap();
        let payload = retained.unwrap();
        drop(reader);
        assert_eq!(payload.decode::<String>().unwrap(), "original");
    }
}
