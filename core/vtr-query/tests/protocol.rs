#![cfg(feature = "wire")]
use prost::Message;
use vtr_query::{
    session::Query,
    wire::{proto as p, MAX_DECODED_BYTES},
    wire_request::{decode, RequestBody, VERSION},
    Budget, Error, TimeBound,
};

fn envelope(body: p::envelope::Body) -> p::Envelope {
    p::Envelope {
        version: VERSION,
        request_id: u64::MAX,
        snapshot: vec![7; 16],
        body: Some(body),
    }
}
fn query(operation: p::query::Operation) -> p::Envelope {
    envelope(p::envelope::Body::Query(p::Query {
        limits: Some(p::Limits {
            bytes: 4096,
            records: 64,
            work: 1024,
        }),
        operation: Some(operation),
    }))
}
#[test]
fn edge_request_roundtrips_exact_time_and_rejects_unknown_direction() {
    let budget = Budget::new(MAX_DECODED_BYTES);
    for direction in [1, 2, 0, 3, -1] {
        let envelope = query(p::query::Operation::FindChange(p::FindChange {
            signal: u32::MAX,
            from: u64::MAX,
            direction,
        }));
        let decoded = decode(&envelope.encode_to_vec(), &budget);
        if direction == 1 || direction == 2 {
            let request = decoded.unwrap();
            let encoded = vtr_query::wire_encode::request(
                request.request_id,
                request.snapshot,
                &request.body,
                &budget,
            )
            .unwrap();
            assert_eq!(p::Envelope::decode(encoded.bytes()).unwrap(), envelope);
        } else {
            assert!(decoded.is_err());
        }
    }
    assert_eq!(budget.used(), 0);
}
#[test]
fn request_decoding_preserves_maximum_time_and_integer_identities() {
    let encoded = query(p::query::Operation::Window(p::WaveWindow {
        signal: u32::MAX,
        interval: Some(p::Interval {
            start: u64::MAX,
            end: Some(p::Bound {
                value: Some(p::bound::Value::AfterMax(p::Empty {})),
            }),
        }),
    }))
    .encode_to_vec();
    let budget = Budget::new(MAX_DECODED_BYTES);
    let request = decode(&encoded, &budget).unwrap();
    assert_eq!(request.request_id, u64::MAX);
    let RequestBody::Query {
        query: Query::Window { signal, interval },
        ..
    } = &request.body
    else {
        panic!("expected window");
    };
    assert_eq!(*signal, u32::MAX);
    assert_eq!(interval.start(), u64::MAX);
    assert_eq!(interval.end(), TimeBound::AfterMax);
    assert!(budget.used() > 0);
    drop(request);
    assert_eq!(budget.used(), 0);
}
#[test]
fn reject_invalid_grids_missing_fields_and_snapshot_conflicts() {
    let budget = Budget::new(MAX_DECODED_BYTES);
    let cases = [
        query(p::query::Operation::Summary(p::WaveSummary {
            signal: 0,
            grid: Some(p::Grid {
                start: 1,
                level: 4,
                count: 2,
            }),
        })),
        query(p::query::Operation::Window(p::WaveWindow {
            signal: 0,
            interval: None,
        })),
        envelope(p::envelope::Body::Next(p::Continuation {
            snapshot: vec![8; 16],
            operation: 1,
            step: 0,
        })),
        p::Envelope {
            version: 2,
            ..envelope(p::envelope::Body::Hello(p::Empty {}))
        },
        p::Envelope {
            request_id: 0,
            ..envelope(p::envelope::Body::Hello(p::Empty {}))
        },
        envelope(p::envelope::Body::Reply(p::Delivery::default())),
    ];
    for case in cases {
        assert!(decode(&case.encode_to_vec(), &budget).is_err());
        assert_eq!(budget.used(), 0);
    }
}
#[test]
fn framing_size_does_not_authorize_unbounded_decoded_repetitions() {
    let encoded = query(p::query::Operation::Resolve(p::Resolve {
        paths: (0..5000).map(|_| p::Path::default()).collect(),
    }))
    .encode_to_vec();
    assert!(encoded.len() < 65536);
    let budget = Budget::new(MAX_DECODED_BYTES);
    assert!(matches!(
        decode(&encoded, &budget),
        Err(Error::ResourceLimit)
    ));
    assert_eq!(budget.used(), 0);
    let encoded = query(p::query::Operation::Resolve(p::Resolve {
        paths: vec![p::Path {
            segments: vec!["literal.root".into(), "\\sig.with.dots ".into()],
            occurrence: Some(1),
            kind: Some(2),
        }],
    }))
    .encode_to_vec();
    let decoded = decode(&encoded, &budget).unwrap();
    let RequestBody::Query {
        query: Query::Resolve { paths },
        ..
    } = &decoded.body
    else {
        panic!("expected paths");
    };
    assert_eq!(paths[0].segments, ["literal.root", "\\sig.with.dots "]);
    assert_eq!(paths[0].occurrence, Some(1));
}
#[test]
fn malformed_wire_is_rejected_before_any_decode_allocation() {
    let valid = envelope(p::envelope::Body::Hello(p::Empty {})).encode_to_vec();
    let budget = Budget::new(MAX_DECODED_BYTES);
    let mut duplicate = valid.clone();
    duplicate.extend([8, 1]);
    let mut conflicting_body = valid.clone();
    conflicting_body.extend([98, 0]);
    let mut overflow_u32 = vec![8, 0x81, 0x80, 0x80, 0x80, 0x10];
    overflow_u32.extend(&valid[2..]);
    for bytes in [
        vec![0xff; 10],
        vec![0],
        vec![10, 0],
        vec![26, 255],
        duplicate,
        conflicting_body,
        overflow_u32,
    ] {
        assert!(decode(&bytes, &budget).is_err());
        assert_eq!(budget.used(), 0);
    }
    for end in 1..valid.len() {
        assert!(decode(&valid[..end], &budget).is_err());
        assert_eq!(budget.used(), 0);
    }
    let tiny = Budget::new(1);
    assert!(matches!(decode(&valid, &tiny), Err(Error::ResourceLimit)));
    assert_eq!(tiny.used(), 0);
}
#[test]
fn generated_request_objects_fit_the_preflight_allowance() {
    // The preflight reserves 1024 per message, including conversion/spare
    // capacity. A schema change must not silently invalidate that allowance.
    macro_rules! fits { ($($ty:ty),*) => { $(assert!(std::mem::size_of::<$ty>() <= 512, stringify!($ty));)* } }
    fits!(
        p::Envelope,
        p::Query,
        p::Limits,
        p::WaveWindow,
        p::WaveSummary,
        p::Children,
        p::Search,
        p::Resolve,
        p::Path,
        p::TextRequest,
        p::Continuation,
        p::Cancel,
        p::Interval,
        p::Bound,
        p::Grid
    );
}

#[test]
fn borrowed_request_encoder_matches_strict_decoder_for_every_operation() {
    use vtr_query::{
        metadata::Path,
        session::{Continuation, SnapshotId},
        wave::Limits,
        wire_encode, Grid, Interval,
    };
    let snapshot = SnapshotId([7; 16]);
    let cursor = Continuation {
        snapshot,
        operation: u64::MAX,
        step: u64::MAX,
    };
    let queries = vec![
        Query::Window {
            signal: u32::MAX,
            interval: Interval::new(u64::MAX, TimeBound::AfterMax).unwrap(),
        },
        Query::Summary {
            signal: 0,
            grid: Grid::new(0, 64, 1).unwrap(),
        },
        Query::Children { parent: None },
        Query::Children { parent: Some(0) },
        Query::Search {
            scope: Some(0),
            needle: "ß.\\x[0]".into(),
        },
        Query::Resolve {
            paths: vec![Path {
                segments: vec!["".into(), "ß.\\x[0]".into()],
                occurrence: Some(0),
                kind: Some(2),
            }],
        },
        Query::Text {
            id: u32::MAX,
            offset: u64::MAX,
            length: 0,
        },
    ];
    let mut bodies = vec![
        RequestBody::Hello,
        RequestBody::Open,
        RequestBody::Close,
        RequestBody::Next(cursor),
        RequestBody::Release(cursor),
        RequestBody::Cancel(u64::MAX),
    ];
    bodies.extend(queries.into_iter().map(|query| RequestBody::Query {
        query,
        limits: Limits::default(),
    }));
    let budget = Budget::new(MAX_DECODED_BYTES);
    for body in bodies {
        let bytes = wire_encode::request(u64::MAX, Some(snapshot), &body, &budget).unwrap();
        let decoded = decode(bytes.bytes(), &budget).unwrap();
        let again =
            wire_encode::request(decoded.request_id, decoded.snapshot, &decoded.body, &budget)
                .unwrap();
        assert_eq!(again.bytes(), bytes.bytes());
    }
    assert_eq!(budget.used(), 0);
    let oversized = RequestBody::Query {
        query: Query::Search {
            scope: None,
            needle: "x".repeat(65536),
        },
        limits: Limits::default(),
    };
    assert!(matches!(
        wire_encode::request(1, Some(snapshot), &oversized, &budget),
        Err(Error::ResourceLimit)
    ));
    assert_eq!(budget.used(), 0);
}
