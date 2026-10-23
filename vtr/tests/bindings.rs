use vtr::{
    Payload, Recorder, SourceOptions, Types,
    schema::{self, Bytes},
};

#[derive(Debug, PartialEq)]
struct Operation(i8);
vtr::impl_payload_enum!(Operation { "load" => -7, "store" => 9 });

struct Operand {
    index: u32,
    offset: i16,
    weight: f32,
}
vtr::impl_payload!(Operand {
    index,
    offset,
    weight
});

struct Request {
    address: u64,
    operation: Operation,
    operands: [Operand; 3],
    extra: Vec<Operand>,
    description: String,
    data: Bytes,
    estimate: f64,
    flags: Vec<bool>,
}
vtr::impl_payload!(Request {
    address,
    operation,
    operands,
    extra,
    description,
    data,
    estimate,
    flags
});

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn declarative_bindings_match_cpp_aggregate_declarations_and_canonical_rows() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/schema-local.json")).unwrap();
    let mut types = Types::new();
    let datatype = Request::bind(&mut types).unwrap();
    assert_eq!(
        types.declarations(schema::FIRST_DECLARED_TYPE).unwrap(),
        unhex(fixture["declarations"].as_str().unwrap())
    );
    for (index, row) in fixture["rows"].as_array().unwrap()[..3].iter().enumerate() {
        assert_eq!(datatype, row[0].as_u64().unwrap() as u32);
        let bytes = unhex(row[1].as_str().unwrap());
        let request: Request = schema::decode(&bytes, 4096).unwrap();
        assert_eq!(schema::capture(&request).unwrap(), bytes);
        assert_eq!(request.operation.0, [-7, 9, 22][index]);
        assert_eq!(request.address, u64::MAX - index as u64);
        assert_eq!(request.operands[0].weight.to_bits(), 0x8000_0000);
        assert_eq!(request.operands[1].weight.to_bits(), 0x7fc0_beef);
        assert_eq!(request.estimate.to_bits(), 0x7ff8_0000_0000_babe);
        assert_eq!(request.extra.len(), index);
        assert!(schema::decode::<Request>(&bytes, 8).is_err());
        for end in 0..bytes.len() {
            assert!(schema::decode::<Request>(&bytes[..end], 4096).is_err());
        }
    }
}

#[derive(Debug, PartialEq)]
struct WideCode(u64);
vtr::impl_payload_enum!("UnsignedOperation", WideCode { "max" => u64::MAX });

#[derive(Debug, PartialEq)]
struct Empty {}
vtr::impl_payload!(Empty {});

#[derive(Debug, PartialEq)]
struct Counter<T> {
    r#type: T,
    empty: Empty,
}
vtr::impl_payload!("Counter", Counter<WideCode> { r#type, empty });

#[test]
fn renamed_generic_raw_fields_empty_structs_and_unknown_unsigned_enums_retain_schema() {
    for mode in [vtr::ExecutionMode::Inline, vtr::ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bindings.vtr");
        let mut writer = vtr::Writer::create_with(
            &path,
            vtr::WriterOptions {
                mode,
                ..Default::default()
            },
        )
        .unwrap();
        let bound = writer.bind::<Counter<WideCode>>().unwrap();
        assert_eq!(writer.bind::<Counter<WideCode>>().unwrap(), bound);
        let source = writer
            .transactions::<Counter<WideCode>>(SourceOptions::new("counter", None), "COUNTER")
            .unwrap();
        let mut ids = Vec::new();
        for raw in [u64::MAX, 42] {
            let value = Counter {
                r#type: WideCode(raw),
                empty: Empty {},
            };
            let transaction = writer.begin(source, &value, None).unwrap();
            writer.end(transaction, vtr::Status::Ok).unwrap();
            ids.push(transaction.id());
        }
        writer.close().unwrap();
        let reader = vtr::Reader::open(path).unwrap();
        reader.validate().unwrap();
        let retained: Vec<_> = ids
            .iter()
            .map(|&id| reader.lookup(id).unwrap().unwrap().payload())
            .collect();
        assert!(retained.iter().all(|payload| payload.datatype() == bound));
        drop(reader);
        for (payload, raw) in retained.iter().zip([u64::MAX, 42]) {
            assert_eq!(
                payload.decode::<Counter<WideCode>>().unwrap(),
                Counter {
                    r#type: WideCode(raw),
                    empty: Empty {}
                }
            );
            assert_eq!(
                payload.field("type").unwrap().decode::<WideCode>().unwrap(),
                WideCode(raw)
            );
            assert_eq!(
                payload.field("empty").unwrap().decode::<Empty>().unwrap(),
                Empty {}
            );
            assert!(
                payload
                    .field("type")
                    .unwrap()
                    .decode::<Operation>()
                    .is_err()
            );
        }
    }
}

struct Recursive {
    children: Vec<Recursive>,
}
vtr::impl_payload!(Recursive { children });

#[test]
fn recursive_automatic_bindings_fail_without_exhausting_the_stack_or_poisoning_types() {
    let mut types = Types::new();
    assert!(Recursive::bind(&mut types).is_err());
    let valid = Counter::<WideCode>::bind(&mut types).unwrap();
    let value = Counter {
        r#type: WideCode(42),
        empty: Empty {},
    };
    types
        .validate(valid, &schema::capture(&value).unwrap(), 0)
        .unwrap();
    assert_eq!(Counter::<WideCode>::bind(&mut types).unwrap(), valid);
}
