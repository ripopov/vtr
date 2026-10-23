use std::collections::BTreeMap;

use vtr::{
    Compression,
    records::{
        Generator, Generators, LocatedRecord, RecordBlockView, RecordKind, RecordPolicy,
        RecordQuery, encode_record_block,
    },
    schema::Types,
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

fn fixture(source: &str) -> (Types, Generators, Vec<LocatedRecord>, Vec<u8>) {
    let fixture: serde_json::Value = serde_json::from_str(source).unwrap();
    let mut types = Types::new();
    types
        .read_declarations(&unhex(fixture["declarations"].as_str().unwrap()))
        .unwrap();
    let mut generators = Generators::default();
    for declaration in fixture["generators"].as_array().unwrap() {
        generators
            .declare(Generator {
                id: declaration[0].as_u64().unwrap() as u32,
                kind: RecordKind::from_u8(declaration[1].as_u64().unwrap() as u8).unwrap(),
                payload_type: declaration[2].as_u64().unwrap() as u32,
            })
            .unwrap();
    }
    let records = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| LocatedRecord {
            id: row[0].as_u64().unwrap(),
            generator: row[1].as_u64().unwrap() as u32,
            begin: row[2].as_u64().unwrap(),
            end: row[3].as_u64().unwrap(),
            parent: row[4].as_u64().unwrap(),
            payload: unhex(row[5].as_str().unwrap()),
        })
        .collect();
    (
        types,
        generators,
        records,
        unhex(fixture["block"].as_str().unwrap()),
    )
}

fn rows(view: &RecordBlockView<'_>, query: RecordQuery) -> Vec<LocatedRecord> {
    let mut out = Vec::new();
    view.query(query, |record| {
        out.push(record);
        true
    })
    .unwrap();
    out
}

#[test]
fn sparse_generator_ids_roundtrip_and_failed_redeclarations_preserve_schema() {
    let types = Types::new();
    let mut generators = Generators::default();
    let mut records = Vec::new();
    for (index, id) in [0, 7, 255, 256, u32::MAX - 1].into_iter().enumerate() {
        let generator = Generator {
            id,
            kind: RecordKind::Log,
            payload_type: vtr::schema::Primitive::U64.id(),
        };
        generators.declare(generator).unwrap();
        assert!(
            generators
                .declare(Generator {
                    kind: RecordKind::Stage,
                    ..generator
                })
                .is_err()
        );
        assert_eq!(*generators.get(id).unwrap(), generator);
        records.push(LocatedRecord {
            id: index as u64 + 1,
            generator: id,
            begin: index as u64,
            end: index as u64,
            parent: 0,
            payload: (index as u64).to_le_bytes().to_vec(),
        });
    }
    assert!(generators.get(254).is_err());
    assert!(generators.get(u32::MAX).is_err());
    let generators = generators.clone();
    let block =
        encode_record_block(1, &types, &generators, &records, RecordPolicy::default(), 0).unwrap();
    let view = RecordBlockView::with_defaults(&block, &types, &generators).unwrap();
    view.validate().unwrap();
    assert_eq!(rows(&view, RecordQuery::default()), records);
}

#[test]
fn decode_cpp_record_pages_with_every_codec_and_exact_queries() {
    for source in [
        include_str!("fixtures/records-0.json"),
        include_str!("fixtures/records-1.json"),
        include_str!("fixtures/records-2.json"),
    ] {
        let (types, generators, records, block) = fixture(source);
        let view = RecordBlockView::with_defaults(&block, &types, &generators).unwrap();
        view.validate().unwrap();
        let expected: BTreeMap<_, _> = records.iter().map(|record| (record.id, record)).collect();
        let all = rows(&view, RecordQuery::default());
        assert_eq!(all.len(), records.len());
        for actual in &all {
            assert_eq!(expected[&actual.id], actual);
        }
        for generator in [10, 11, 12] {
            assert_eq!(
                all.iter()
                    .filter(|r| r.generator == generator)
                    .collect::<Vec<_>>(),
                records
                    .iter()
                    .filter(|r| r.generator == generator)
                    .collect::<Vec<_>>()
            );
        }
        for begin in [0, 1, 17, 101, 399, 400, u64::MAX] {
            for end in [begin, begin.saturating_add(20)] {
                for generator in [None, Some(10), Some(11), Some(12)] {
                    for parent in [None, Some(0), Some(1), Some(u64::MAX - 1)] {
                        let query = RecordQuery {
                            begin,
                            end,
                            generator,
                            parent,
                            ..RecordQuery::default()
                        };
                        let expected: Vec<_> = all
                            .iter()
                            .filter(|r| {
                                r.begin <= end
                                    && if r.begin == r.end {
                                        r.begin >= begin
                                    } else {
                                        r.end > begin
                                    }
                            })
                            .filter(|r| {
                                generator.is_none_or(|id| id == r.generator)
                                    && parent.is_none_or(|id| id == r.parent)
                            })
                            .collect();
                        assert_eq!(rows(&view, query).iter().collect::<Vec<_>>(), expected);
                    }
                }
            }
        }
        let mut count = 0;
        view.query(RecordQuery::default(), |_| {
            count += 1;
            false
        })
        .unwrap();
        assert_eq!(count, 1);
    }
}

#[test]
fn encode_records_matches_stored_cpp_bytes_and_round_trips_page_policies() {
    let (types, generators, records, block) = fixture(include_str!("fixtures/records-0.json"));
    let encoded = encode_record_block(
        7,
        &types,
        &generators,
        &records,
        RecordPolicy {
            page_bytes: 512,
            compression: Compression::NONE,
            separate_logs: false,
        },
        u64::MAX,
    )
    .unwrap();
    assert_eq!(encoded, block);
    for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
        for page_bytes in [1, 512, 65536] {
            for separate_logs in [false, true] {
                let encoded = encode_record_block(
                    7,
                    &types,
                    &generators,
                    &records,
                    RecordPolicy {
                        page_bytes,
                        compression,
                        separate_logs,
                    },
                    u64::MAX,
                )
                .unwrap();
                let view = RecordBlockView::with_defaults(&encoded, &types, &generators).unwrap();
                view.validate().unwrap();
                let actual: BTreeMap<_, _> = rows(&view, RecordQuery::default())
                    .into_iter()
                    .map(|r| (r.id, r))
                    .collect();
                assert_eq!(actual, records.iter().cloned().map(|r| (r.id, r)).collect());
                if separate_logs {
                    for page in 0..view.pages().len() {
                        let kinds: Vec<_> = view
                            .segments()
                            .iter()
                            .filter(|s| s.page as usize == page)
                            .map(|s| generators.get(s.generator).unwrap().kind)
                            .collect();
                        assert!(
                            !kinds.contains(&RecordKind::Log)
                                || kinds.iter().all(|k| *k == RecordKind::Log)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn malformed_record_pages_and_invalid_lifetimes_are_rejected() {
    let (types, generators, mut records, _) = fixture(include_str!("fixtures/records-0.json"));
    records.truncate(3);
    let encoded = encode_record_block(
        1,
        &types,
        &generators,
        &records,
        RecordPolicy::default(),
        u64::MAX,
    )
    .unwrap();
    for end in 0..encoded.len() {
        assert!(RecordBlockView::with_defaults(&encoded[..end], &types, &generators).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(RecordBlockView::with_defaults(&trailing, &types, &generators).is_err());
    for offset in [0, 8, 16, 24, 32, 40, 56 + 20, 56 + 44] {
        let mut corrupt = encoded.clone();
        corrupt[offset] ^= 1;
        assert!(
            RecordBlockView::with_defaults(&corrupt, &types, &generators)
                .and_then(|view| view.validate())
                .is_err(),
            "offset {offset}"
        );
    }
    for (id, parent, begin, end) in [
        (0, 0, 1, 1),
        (3, 3, 1, 1),
        (3, 0, 1, 1),
        (3, 1, 1, 2),
        (3, 1, 2, 1),
    ] {
        records[0].id = id;
        records[0].parent = parent;
        records[0].begin = begin;
        records[0].end = end;
        assert!(
            encode_record_block(
                1,
                &types,
                &generators,
                &records,
                RecordPolicy::default(),
                u64::MAX
            )
            .is_err()
        );
    }
}
