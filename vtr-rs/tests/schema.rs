use std::collections::HashMap;

use vtr::{
    arguments::{ArgumentColumns, ArgumentReader},
    schema::{self, Enumerator, FIRST_DECLARED_TYPE, Field, Payload, Primitive, Types},
};

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
fn numeric_slice_encoding_preserves_prefixes_sequence_lengths_and_float_bits() {
    let words = [0x1234u16, 0xabcd];
    let mut prefixed = vec![0xaa];
    u16::encode_slice(&words, &mut prefixed).unwrap();
    assert_eq!(prefixed, unhex("aa3412cdab"));
    assert_eq!(schema::capture(&words).unwrap(), unhex("3412cdab"));
    assert_eq!(
        schema::capture(&words.to_vec()).unwrap(),
        unhex("020000003412cdab")
    );
    assert_eq!(schema::capture(&Vec::<i64>::new()).unwrap(), [0, 0, 0, 0]);
    i64::encode_slice(&[], &mut prefixed).unwrap();
    assert_eq!(prefixed, unhex("aa3412cdab"));
    let floats = [f64::from_bits(0x7ff8_0012_3456_789a), -0.0];
    let bytes = schema::capture(&floats).unwrap();
    assert_eq!(bytes, unhex("9a7856341200f87f0000000000000080"));
    let decoded = schema::decode::<[f64; 2]>(&bytes, 16).unwrap();
    assert_eq!(decoded.map(f64::to_bits), floats.map(f64::to_bits));
}

#[test]
fn reference_cpp_payloads_match_in_both_directions() {
    for source in [
        include_str!("fixtures/schema-local.json"),
        include_str!("fixtures/schema-shared.json"),
    ] {
        let fixture: serde_json::Value = serde_json::from_str(source).unwrap();
        let declarations = unhex(fixture["declarations"].as_str().unwrap());
        let mut types = Types::new();
        types.read_declarations(&declarations).unwrap();
        assert_eq!(
            types.declarations(FIRST_DECLARED_TYPE).unwrap(),
            declarations
        );
        let columns: [Vec<u8>; 5] =
            std::array::from_fn(|i| unhex(fixture["columns"][i].as_str().unwrap()));
        let global: Vec<String> = fixture["global_text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().into())
            .collect();
        let references: HashMap<String, u32> = global
            .iter()
            .enumerate()
            .map(|(id, text)| (text.clone(), id as u32))
            .collect();
        let mut reader = ArgumentReader::new(
            columns.each_ref().map(Vec::as_slice),
            u64::MAX,
            1 << 20,
            &global,
        )
        .unwrap();
        let mut selective = ArgumentReader::new(
            columns.each_ref().map(Vec::as_slice),
            u64::MAX,
            1 << 20,
            &global,
        )
        .unwrap();
        let mut encoder =
            ArgumentColumns::new(u64::MAX, (!global.is_empty()).then_some(&references));
        for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
            let datatype = row[0].as_u64().unwrap() as u32;
            let expected = unhex(row[1].as_str().unwrap());
            types.validate(datatype, &expected, u64::MAX).unwrap();
            assert_eq!(reader.read(&types, datatype).unwrap(), expected);
            if index % 2 == 0 {
                selective.skip(&types, datatype).unwrap();
            } else {
                assert_eq!(selective.read(&types, datatype).unwrap(), expected);
            }
            encoder.append(&types, datatype, &expected).unwrap();
            for end in 0..expected.len() {
                assert!(
                    types
                        .validate(datatype, &expected[..end], u64::MAX)
                        .is_err()
                );
            }
        }
        reader.done().unwrap();
        selective.done().unwrap();
        assert_eq!(encoder.finish(), columns);
    }
}

#[test]
fn malformed_declarations_are_atomic_and_bounded() {
    let mut types = Types::new();
    let id = types
        .structure("root", vec![Field::new("number", Primitive::U64.id())])
        .unwrap();
    let original = types.declarations(FIRST_DECLARED_TYPE).unwrap();
    for end in 0..original.len() {
        let mut target = Types::new();
        assert!(target.read_declarations(&original[..end]).is_err());
        assert_eq!(target.len(), FIRST_DECLARED_TYPE as usize);
    }
    assert!(
        types
            .structure("duplicates", vec![Field::new("a", id), Field::new("a", id)])
            .is_err()
    );
    assert!(
        types
            .structure("forward", vec![Field::new("a", id + 1)])
            .is_err()
    );
    assert!(types.array("overflow", id, u32::MAX).is_err());
    assert!(
        types
            .enumeration(
                "narrow",
                Primitive::I8.id(),
                vec![Enumerator {
                    value: 128,
                    name: "bad".into()
                }]
            )
            .is_err()
    );
    assert!(
        types
            .enumeration("float", Primitive::F64.id(), vec![])
            .is_err()
    );
    assert_eq!(types.declarations(FIRST_DECLARED_TYPE).unwrap(), original);
    let mut tail = id;
    for _ in 2..64 {
        tail = types
            .structure("nested", vec![Field::new("item", tail)])
            .unwrap();
    }
    assert!(
        types
            .structure("too deep", vec![Field::new("item", tail)])
            .is_err()
    );
    let empty = types.structure("empty", vec![]).unwrap();
    let empty_array = types.array("huge empty", empty, u32::MAX).unwrap();
    types.validate(empty_array, &[], 0).unwrap();
    assert_eq!(types.get(empty_array).unwrap().unsigned_words(), Some(0));
}

#[test]
fn invalid_canonical_values_and_column_values_fail() {
    let types = Types::new();
    assert!(types.validate(Primitive::Bool.id(), &[2], 0).is_err());
    assert!(
        types
            .validate(Primitive::Str.id(), &1u32.to_le_bytes(), 1)
            .is_err()
    );
    assert!(types.validate(Primitive::Enum.id(), &[], 0).is_err());
    for (primitive, bits, packed) in [
        (Primitive::Bits, 1u64, vec![2]),
        (Primitive::Logic, 1, vec![4]),
        (Primitive::Logic9, 2, vec![0x90]),
        (Primitive::Logic9, 1, vec![9]),
    ] {
        let mut bytes = bits.to_le_bytes().to_vec();
        bytes.extend_from_slice(&packed);
        assert!(types.validate(primitive.id(), &bytes, 0).is_err());
    }
    for numeric in [
        &[0x80, 0][..],
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2],
        &[128, 2],
    ] {
        let mut reader = ArgumentReader::with_defaults([numeric, &[], &[], &[0], &[]]).unwrap();
        assert!(reader.read(&types, Primitive::U8.id()).is_err());
    }
    let mut reader =
        ArgumentReader::with_defaults([&[], &[], &[1], &[4, 1, b'a', 1, b'b'], &[]]).unwrap();
    assert!(reader.read(&types, Primitive::Text.id()).is_err());
    assert!(reader.done().is_err());
    assert!(ArgumentReader::with_defaults([&[], &[], &[], &[4, 1, b'a', 1, b'a'], &[]]).is_err());
    assert!(ArgumentReader::with_defaults([&[], &[], &[], &[1], &[]]).is_err());
}

#[test]
fn fixed_layout_validation_preserves_value_constraints_and_exact_sizes() {
    let mut types = Types::new();
    let numbers = types.array("numbers", Primitive::U64.id(), 5).unwrap();
    let constrained = types
        .structure(
            "constrained",
            vec![
                Field::new("numbers", numbers),
                Field::new("enabled", Primitive::Bool.id()),
                Field::new("name", Primitive::Str.id()),
            ],
        )
        .unwrap();
    let mut restored = Types::new();
    restored
        .read_declarations(&types.declarations(FIRST_DECLARED_TYPE).unwrap())
        .unwrap();
    for types in [&types, &restored] {
        let mut bytes = vec![255; 40];
        types.validate(numbers, &bytes, 0).unwrap();
        for length in 0..40 {
            assert!(types.validate(numbers, &bytes[..length], 0).is_err());
        }
        bytes.push(1);
        assert!(types.validate(numbers, &bytes, 0).is_err());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        types.validate(constrained, &bytes, 1).unwrap();
        bytes[40] = 2;
        assert!(types.validate(constrained, &bytes, 1).is_err());
        bytes[40] = 1;
        bytes[41] = 1;
        assert!(types.validate(constrained, &bytes, 1).is_err());
        bytes[41] = 0;
        bytes.push(0);
        assert!(types.validate(constrained, &bytes, 1).is_err());
    }
}

#[test]
fn column_limits_and_failed_reads_preserve_position() {
    let types = Types::new();
    let payload = schema::capture(&"hello".to_owned()).unwrap();
    let mut encoder = ArgumentColumns::default();
    encoder
        .append(&types, Primitive::Text.id(), &payload)
        .unwrap();
    let columns = encoder.finish();
    let views = columns.each_ref().map(Vec::as_slice);
    let mut reader = ArgumentReader::new(views, 0, 8, &[]).unwrap();
    assert!(reader.read(&types, Primitive::Text.id()).is_err());
    assert!(reader.read(&types, Primitive::Text.id()).is_err());
    let mut reader = ArgumentReader::new(views, 0, 9, &[]).unwrap();
    assert_eq!(reader.read(&types, Primitive::Text.id()).unwrap(), payload);
    reader.done().unwrap();
    let mut types = Types::new();
    let sequence = types.sequence("untrusted", Primitive::U64.id()).unwrap();
    let mut reader =
        ArgumentReader::with_defaults([&[0xff, 0xff, 0xff, 0xff, 15], &[], &[], &[0], &[]])
            .unwrap();
    assert!(reader.read(&types, sequence).is_err());
}

#[test]
fn typed_payloads_retain_float_bits_and_bound_allocations() {
    let value = vec![
        vec![0.0f64, -0.0, f64::from_bits(0x7ff8_0000_0000_beef)],
        vec![],
    ];
    let bytes = schema::capture(&value).unwrap();
    let restored: Vec<Vec<f64>> = schema::decode(&bytes, 1024).unwrap();
    for (before, after) in value.iter().flatten().zip(restored.iter().flatten()) {
        assert_eq!(before.to_bits(), after.to_bits());
    }
    assert!(schema::decode::<Vec<Vec<f64>>>(&bytes, 8).is_err());
    assert!(schema::decode::<Vec<()>>(&u32::MAX.to_le_bytes(), 1024).is_err());
    assert!(schema::decode::<bool>(&[2], 0).is_err());
    assert!(schema::decode::<u64>(&[0; 9], 0).is_err());
    let mut types = Types::new();
    let id = Vec::<Vec<f64>>::bind(&mut types).unwrap();
    types.validate(id, &bytes, 0).unwrap();
}

#[test]
fn semantic_bindings_decode_and_reencode_cpp_reference_values() {
    use schema::{Address, Bits, Bytes, Fixed, Logic4, Logic9, StringId, TimeValue, UFixed};

    fn check<T: Payload>(bytes: &[u8], expected: schema::TypeId) {
        let value = schema::decode::<T>(bytes, 1024).unwrap();
        assert_eq!(schema::capture(&value).unwrap(), bytes);
        assert_eq!(T::bind(&mut Types::new()).unwrap(), expected);
        for end in 0..bytes.len() {
            assert!(schema::decode::<T>(&bytes[..end], 1024).is_err());
        }
    }
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/schema-local.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let datatype = row[0].as_u64().unwrap() as u32;
        let bytes = unhex(row[1].as_str().unwrap());
        match datatype {
            12 => check::<StringId>(&bytes, datatype),
            14 => check::<Bytes>(&bytes, datatype),
            15 => check::<Bits>(&bytes, datatype),
            16 => check::<Logic4>(&bytes, datatype),
            17 => check::<Logic9>(&bytes, datatype),
            18 => check::<TimeValue>(&bytes, datatype),
            20 => check::<Address>(&bytes, datatype),
            21 => check::<Fixed>(&bytes, datatype),
            22 => check::<UFixed>(&bytes, datatype),
            _ => (),
        }
    }
}

#[test]
fn semantic_payloads_reject_invalid_states_padding_and_allocations() {
    use schema::{Bits, Bytes, Logic4, Logic9, PackedVector};

    for value in [
        Bits {
            width: 1,
            packed: vec![2],
        },
        Bits {
            width: 9,
            packed: vec![1],
        },
        Bits {
            width: 0,
            packed: vec![0],
        },
    ] {
        assert!(schema::capture(&value).is_err());
    }
    assert!(
        schema::capture(&Logic4 {
            width: 1,
            packed: vec![4]
        })
        .is_err()
    );
    assert!(
        schema::capture(&Logic9 {
            width: 2,
            packed: vec![0x90]
        })
        .is_err()
    );
    assert!(
        schema::capture(&PackedVector::<3> {
            width: 0,
            packed: vec![]
        })
        .is_err()
    );
    assert!(PackedVector::<3>::bind(&mut Types::new()).is_err());
    let mut invalid = 2u64.to_le_bytes().to_vec();
    invalid.push(0x90);
    assert!(schema::decode::<Logic9>(&invalid, 1).is_err());
    for (width, packed) in [(0, vec![]), (1, vec![1]), (9, vec![0x81, 1])] {
        let value = Bits { width, packed };
        let bytes = schema::capture(&value).unwrap();
        assert_eq!(schema::decode::<Bits>(&bytes, 2).unwrap(), value);
    }
    let bytes = schema::capture(&Bits {
        width: 9,
        packed: vec![0x81, 1],
    })
    .unwrap();
    assert!(schema::decode::<Bits>(&bytes, 1).is_err());
    assert!(schema::decode::<Bits>(&u64::MAX.to_le_bytes(), usize::MAX).is_err());
    let bytes = schema::capture(&Bytes(vec![0, 255])).unwrap();
    assert!(schema::decode::<Bytes>(&bytes, 1).is_err());
    assert!(schema::decode::<Bytes>(&u32::MAX.to_le_bytes(), 1024).is_err());
}
