use std::collections::{BTreeMap, HashMap};

use vtr::{
    Compression,
    attributes::Attribute,
    records::{Generator, Generators, RecordKind, RecordPolicy},
    schema::{self, Primitive, Types},
    transactions::{
        Relation, Transaction, TransactionBlockView, TransactionPolicy, TransactionQuery,
        encode_transaction_block,
    },
    varint::{put_blob, put_u64},
};

fn unhex(value: &str) -> Vec<u8> {
    value
        .trim()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn attributes(out: &mut Vec<u8>, values: &[Attribute]) {
    put_u64(out, values.len() as u64);
    for value in values {
        put_u64(out, u64::from(value.key));
        value.value.encode(out, u64::MAX).unwrap();
    }
}

fn canonical_tx(tx: &Transaction) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [tx.id, tx.parent, tx.begin, tx.end] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&tx.generator.to_le_bytes());
    out.extend_from_slice(&[tx.status, tx.span_kind]);
    put_blob(&mut out, &tx.payload);
    attributes(&mut out, &tx.attributes);
    put_u64(&mut out, tx.events.len() as u64);
    for event in &tx.events {
        out.extend_from_slice(&event.id.to_le_bytes());
        out.extend_from_slice(&event.time.to_le_bytes());
        out.extend_from_slice(&event.name.to_le_bytes());
        attributes(&mut out, &event.attributes);
    }
    put_u64(&mut out, tx.stages.len() as u64);
    for stage in &tx.stages {
        out.extend_from_slice(&stage.id.to_le_bytes());
        out.extend_from_slice(&stage.name.to_le_bytes());
        out.extend_from_slice(&stage.lane.to_le_bytes());
        out.extend_from_slice(&stage.begin.to_le_bytes());
        out.push(u8::from(stage.end.is_some()));
        if let Some(end) = stage.end {
            out.extend_from_slice(&end.to_le_bytes());
        }
        attributes(&mut out, &stage.attributes);
    }
    put_u64(&mut out, tx.child_blocks.len() as u64);
    for block in &tx.child_blocks {
        out.extend_from_slice(&block.to_le_bytes());
    }
    out
}

fn canonical_relation(relation: &Relation) -> Vec<u8> {
    let mut out = relation.kind.to_le_bytes().to_vec();
    out.extend_from_slice(&relation.source.to_le_bytes());
    out.extend_from_slice(&relation.destination.to_le_bytes());
    attributes(&mut out, &relation.attributes);
    out
}

#[test]
fn cpp_general_pages_preserve_payloads_embedded_ids_relations_and_queries() {
    for source in [
        include_str!("fixtures/transactions-0.json"),
        include_str!("fixtures/transactions-1.json"),
        include_str!("fixtures/transactions-2.json"),
    ] {
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
        let global: Vec<String> = fixture["global_text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().into())
            .collect();
        let block = unhex(fixture["block"].as_str().unwrap());
        let view =
            TransactionBlockView::new(&block, &types, &generators, 64, 1 << 20, &global).unwrap();
        view.validate().unwrap();
        let mut actual = Vec::new();
        let mut relations = Vec::new();
        view.scan(
            |tx| {
                actual.push(tx);
                true
            },
            |relation| {
                relations.push(relation);
                true
            },
        )
        .unwrap();
        let expected: BTreeMap<_, _> = fixture["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let bytes = unhex(v.as_str().unwrap());
                (u64::from_le_bytes(bytes[..8].try_into().unwrap()), bytes)
            })
            .collect();
        assert_eq!(
            actual
                .iter()
                .map(|tx| (tx.id, canonical_tx(tx)))
                .collect::<BTreeMap<_, _>>(),
            expected
        );
        assert_eq!(
            relations.iter().map(canonical_relation).collect::<Vec<_>>(),
            fixture["relations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| unhex(v.as_str().unwrap()))
                .collect::<Vec<_>>()
        );
        for begin in [0, 1, 10, 99, 100, 101, u64::MAX] {
            for end in [begin, begin.saturating_add(10)] {
                for generator in [None, Some(100), Some(101), Some(102)] {
                    let mut found = Vec::new();
                    view.transactions(
                        TransactionQuery {
                            begin,
                            end,
                            generator,
                            ..TransactionQuery::default()
                        },
                        |tx| {
                            found.push(tx.id);
                            true
                        },
                    )
                    .unwrap();
                    assert_eq!(
                        found,
                        actual
                            .iter()
                            .filter(|tx| tx.begin <= end
                                && tx.end >= begin
                                && generator.is_none_or(|id| id == tx.generator))
                            .map(|tx| tx.id)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        for tx in &actual {
            for id in tx
                .events
                .iter()
                .map(|e| e.id)
                .chain(tx.stages.iter().map(|s| s.id))
            {
                let mut found = Vec::new();
                view.transactions(
                    TransactionQuery {
                        embedded_id: Some(id),
                        ..TransactionQuery::default()
                    },
                    |tx| {
                        found.push(tx.id);
                        true
                    },
                )
                .unwrap();
                assert_eq!(found, vec![tx.id]);
            }
        }
        for incoming in [false, true] {
            for endpoint in [0, 2, 13, 99, u64::MAX] {
                let mut found = Vec::new();
                view.relations(endpoint, incoming, |relation| {
                    found.push(canonical_relation(&relation));
                    true
                })
                .unwrap();
                assert_eq!(
                    found,
                    relations
                        .iter()
                        .filter(|r| if incoming {
                            r.destination == endpoint
                        } else {
                            r.source == endpoint
                        })
                        .map(canonical_relation)
                        .collect::<Vec<_>>()
                );
            }
        }
        let shared = HashMap::from([("payload Δ".to_owned(), 1)]);
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let encoded = encode_transaction_block(
                7,
                &types,
                &generators,
                &actual,
                &relations,
                TransactionPolicy {
                    pages: RecordPolicy {
                        page_bytes: 1024,
                        compression,
                        separate_logs: true,
                    },
                    string_count: 64,
                    shared_text: Some(&shared),
                },
            )
            .unwrap();
            let decoded =
                TransactionBlockView::new(&encoded, &types, &generators, 64, 1 << 20, &global)
                    .unwrap();
            decoded.validate().unwrap();
            let mut encoded_rows = BTreeMap::new();
            decoded
                .scan(
                    |tx| {
                        encoded_rows.insert(tx.id, canonical_tx(&tx));
                        true
                    },
                    |_| true,
                )
                .unwrap();
            assert_eq!(encoded_rows, expected);
            if compression == Compression::NONE
                && source == include_str!("fixtures/transactions-0.json")
            {
                assert_eq!(encoded, block);
            }
        }
    }
}

fn scalar_source() -> (Types, Generators, Vec<Transaction>) {
    let mut types = Types::new();
    let datatype = types.array("", Primitive::U64.id(), 3).unwrap();
    let mut generators = Generators::default();
    for id in [4, 9] {
        generators
            .declare(Generator {
                id,
                kind: RecordKind::Transaction,
                payload_type: datatype,
            })
            .unwrap();
    }
    let rows = (0u64..2000)
        .map(|i| Transaction {
            id: i * 2 + 1,
            generator: if i % 2 == 0 { 9 } else { 4 },
            begin: (i * 117) % 9000,
            end: (i * 117) % 9000 + i % 99,
            parent: u64::from(i != 0),
            status: (i % 5) as u8,
            span_kind: (i % 6) as u8,
            payload: schema::capture(&[i, i % 19, if i % 3 == 0 { u64::MAX } else { 0 }]).unwrap(),
            ..Transaction::default()
        })
        .collect();
    (types, generators, rows)
}

#[test]
fn cpp_scalar_pages_match_rust_bytes_and_stream_full_u64_values() {
    let (types, generators, source) = scalar_source();
    for (fixture, compression) in [
        (include_str!("fixtures/scalar-0.hex"), Compression::NONE),
        (include_str!("fixtures/scalar-1.hex"), Compression::LZ4),
        (
            include_str!("fixtures/scalar-2.hex"),
            Compression::ZSTD_FAST,
        ),
    ] {
        let block = unhex(fixture);
        let view = TransactionBlockView::with_defaults(&block, &types, &generators).unwrap();
        view.validate().unwrap();
        let mut actual = Vec::new();
        view.transactions(TransactionQuery::default(), |tx| {
            actual.push(tx);
            true
        })
        .unwrap();
        assert_eq!(actual, source);
        let encoded = encode_transaction_block(
            7,
            &types,
            &generators,
            &source,
            &[],
            TransactionPolicy {
                pages: RecordPolicy {
                    page_bytes: 4096,
                    compression,
                    separate_logs: true,
                },
                ..TransactionPolicy::default()
            },
        )
        .unwrap();
        if compression == Compression::NONE {
            assert_eq!(encoded, block);
        }
        let decoded = TransactionBlockView::with_defaults(&encoded, &types, &generators).unwrap();
        decoded.validate().unwrap();
        let mut count = 0;
        decoded
            .transactions(TransactionQuery::default(), |_| {
                count += 1;
                false
            })
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[test]
fn transaction_blocks_reject_truncation_bounds_duplicate_ids_and_invalid_shapes() {
    let (types, generators, mut source) = scalar_source();
    source.truncate(3);
    let encoded = encode_transaction_block(
        1,
        &types,
        &generators,
        &source,
        &[],
        TransactionPolicy {
            string_count: 0,
            ..TransactionPolicy::default()
        },
    )
    .unwrap();
    for end in 0..encoded.len() {
        assert!(TransactionBlockView::with_defaults(&encoded[..end], &types, &generators).is_err());
    }
    for offset in [0, 8, 24, 40, 48, 136, 136 + 16, 136 + 112] {
        let mut corrupt = encoded.clone();
        corrupt[offset] ^= 1;
        assert!(
            TransactionBlockView::with_defaults(&corrupt, &types, &generators)
                .and_then(|v| v.validate())
                .is_err(),
            "offset {offset}"
        );
    }
    let original = source[1].id;
    source[1].id = source[0].id;
    assert!(
        encode_transaction_block(
            1,
            &types,
            &generators,
            &source,
            &[],
            TransactionPolicy {
                string_count: 0,
                ..TransactionPolicy::default()
            }
        )
        .is_err()
    );
    source[1].id = original;
    for status in [5, 15, 255] {
        source[1].status = status;
        assert!(
            encode_transaction_block(
                1,
                &types,
                &generators,
                &source,
                &[],
                TransactionPolicy {
                    string_count: 0,
                    ..TransactionPolicy::default()
                }
            )
            .is_err()
        );
    }
}

#[test]
fn transaction_blocks_order_dense_and_sparse_ids_and_reject_duplicates() {
    let (types, generators, mut rows) = scalar_source();
    rows.truncate(3);
    for ids in [
        [19, 17, 18],
        [u64::MAX - 1, 1, 1 << 48],
        [u64::MAX - 1, u64::MAX - 3, u64::MAX - 2],
    ] {
        for (row, id) in rows.iter_mut().zip(ids) {
            row.id = id;
            row.parent = 0;
        }
        let encoded = encode_transaction_block(
            1,
            &types,
            &generators,
            &rows,
            &[],
            TransactionPolicy::default(),
        )
        .unwrap();
        let view = TransactionBlockView::with_defaults(&encoded, &types, &generators).unwrap();
        view.validate().unwrap();
        let mut actual = Vec::new();
        view.scan(
            |tx| {
                actual.push((tx.id, tx.payload));
                true
            },
            |_| true,
        )
        .unwrap();
        let mut expected: Vec<_> = rows.iter().map(|tx| (tx.id, tx.payload.clone())).collect();
        expected.sort_unstable_by_key(|row| row.0);
        assert_eq!(actual, expected);
        rows[2].id = rows[0].id;
        assert!(
            encode_transaction_block(
                1,
                &types,
                &generators,
                &rows,
                &[],
                TransactionPolicy::default(),
            )
            .is_err()
        );
    }
}
