#![cfg(feature = "wire")]
use prost::Message;
use vtr_query::{
    wire::{proto as p, MAX_DECODED_BYTES},
    wire_encode,
    wire_reply::{self, Body},
    wire_request::VERSION,
    Budget, Error,
};
fn cursor(step: u64) -> p::Continuation {
    p::Continuation {
        snapshot: vec![7; 16],
        operation: u64::MAX,
        step,
    }
}
fn range() -> p::Interval {
    p::Interval {
        start: u64::MAX,
        end: Some(p::Bound {
            value: Some(p::bound::Value::AfterMax(p::Empty {})),
        }),
    }
}
fn real(bits: u64) -> p::Value {
    p::Value {
        value: Some(p::value::Value::Real(bits)),
    }
}
fn known(v: p::Value) -> p::Sample {
    p::Sample {
        value: Some(p::sample::Value::Known(v)),
    }
}
fn change(v: p::Value) -> p::Change {
    p::Change {
        time: u64::MAX,
        value: Some(v),
    }
}
fn envelope(body: p::envelope::Body) -> p::Envelope {
    p::Envelope {
        version: VERSION,
        request_id: u64::MAX,
        snapshot: vec![7; 16],
        body: Some(body),
    }
}
fn reply(page: p::delivery::Page, complete: bool) -> p::Envelope {
    envelope(p::envelope::Body::Reply(p::Delivery {
        request: Some(cursor(0)),
        next: (!complete).then(|| cursor(1)),
        page: Some(page),
    }))
}
fn window(values: Vec<p::Value>, predecessor: p::Sample) -> p::Envelope {
    reply(
        p::delivery::Page::Window(p::WindowPage {
            interval: Some(range()),
            predecessor: Some(predecessor),
            changes: values.into_iter().map(change).collect(),
            complete: true,
        }),
        true,
    )
}
fn roundtrip(e: p::Envelope) {
    let budget = Budget::new(MAX_DECODED_BYTES);
    let response = wire_reply::decode(&e.encode_to_vec(), &budget).unwrap();
    let Body::Delivery(d) = response.body() else {
        panic!("delivery");
    };
    let detached = d.clone();
    drop(response);
    assert!(budget.used() > 0);
    let encoded = wire_encode::delivery(u64::MAX, &detached, &budget).unwrap();
    assert_eq!(p::Envelope::decode(encoded.bytes()).unwrap(), e);
    drop(detached);
    assert!(budget.used() > 0, "encoded bytes retain independent charge");
    drop(encoded);
    assert_eq!(budget.used(), 0);
}
#[test]
fn waveform_codec_preserves_payload_bits_defaults_events_and_same_time_order() {
    roundtrip(window(
        vec![real(0), real(1u64 << 63), real(0x7ff800000000beef)],
        known(real(0)),
    ));
    for (width, states, data) in [
        (2048, 2, vec![0xff; 256]),
        (3, 4, vec![0x3b]),
        (3, 9, vec![0x87, 0x04]),
    ] {
        let v = p::Value {
            value: Some(p::value::Value::Bits(p::Bits {
                width,
                states,
                data,
            })),
        };
        let s = p::Sample {
            value: Some(p::sample::Value::BackendDefault(p::Kind {
                value: Some(p::kind::Value::Bits(p::BitKind { width, states })),
            })),
        };
        roundtrip(window(vec![v], s));
    }
    let bytes = p::Value {
        value: Some(p::value::Value::Bytes(vec![0, 255, 128])),
    };
    roundtrip(window(vec![bytes.clone()], known(bytes)));
    roundtrip(window(
        vec![real(0), real(0)],
        p::Sample {
            value: Some(p::sample::Value::Event(p::Empty {})),
        },
    ));
    roundtrip(window(
        vec![],
        p::Sample {
            value: Some(p::sample::Value::BackendDefault(p::Kind {
                value: Some(p::kind::Value::Real(p::Empty {})),
            })),
        },
    ));
}
fn txt(s: &str) -> p::Text {
    p::Text {
        value: Some(p::text::Value::Inline(s.into())),
    }
}
fn declarations() -> Vec<p::Declaration> {
    use p::declaration::Data as D;
    [
        D::Scope(p::Scope {
            type_code: 65535,
            component: Some(txt("cpu")),
        }),
        D::Variable(p::Variable {
            type_code: 65535,
            direction: 255,
            signal: u32::MAX,
            kind: Some(p::Kind {
                value: Some(p::kind::Value::Bytes(p::Empty {})),
            }),
            alias: true,
        }),
        D::Stream(p::Stream {
            kind: Some(txt("事务")),
        }),
        D::Generator(p::Empty {}),
        D::EnumTable(p::EnumTable { entries: u64::MAX }),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, data)| p::Declaration {
        id: i as u32 + 1,
        parent: Some(0),
        name: Some(if i == 0 {
            p::Text {
                value: Some(p::text::Value::Reference(p::TextReference {
                    id: u32::MAX,
                    bytes: u64::MAX,
                })),
            }
        } else {
            txt("ß.\\literal[0]")
        }),
        children: u64::MAX,
        attributes: u64::MAX,
        data: Some(data),
    })
    .collect()
}
#[test]
fn metadata_codec_preserves_all_declarations_resolution_outcomes_and_byte_parts() {
    roundtrip(reply(
        p::delivery::Page::Children(p::DeclarationPage {
            parent: Some(0),
            offset: 7,
            complete: true,
            declarations: declarations(),
        }),
        true,
    ));
    roundtrip(reply(
        p::delivery::Page::Search(p::SearchPage {
            start_node: 1,
            complete: false,
            declarations: declarations(),
        }),
        false,
    ));
    roundtrip(reply(
        p::delivery::Page::Resolve(p::ResolvePage {
            offset: 0,
            complete: true,
            results: vec![
                p::Resolution {
                    result: Some(p::resolution::Result::Found(0)),
                },
                p::Resolution {
                    result: Some(p::resolution::Result::Missing(p::Empty {})),
                },
                p::Resolution {
                    result: Some(p::resolution::Result::Ambiguous(p::Empty {})),
                },
            ],
        }),
        true,
    ));
    roundtrip(reply(
        p::delivery::Page::Text(p::TextPart {
            id: u32::MAX,
            offset: u64::MAX - 2,
            total_bytes: u64::MAX,
            bytes: vec![0x80, 0xff],
        }),
        true,
    ));
}
#[test]
fn summary_codec_preserves_nonfinite_counts_signed_zero_and_after_max() {
    roundtrip(reply(
        p::delivery::Page::Summary(p::SummaryPage {
            grid: Some(p::Grid {
                start: u64::MAX,
                level: 0,
                count: 1,
            }),
            offset: 0,
            complete: true,
            bins: vec![p::WaveBin {
                interval: Some(range()),
                entry: Some(known(real(1u64 << 63))),
                exit: Some(known(real(0x7ff800000000beef))),
                changes: 3,
                first: Some(change(real(0))),
                last: Some(change(real(0x7ff800000000beef))),
                real: Some(p::RealSummary {
                    finite_min: Some(1u64 << 63),
                    finite_max: Some(0),
                    nan_changes: 1,
                    positive_infinite_changes: 1,
                    negative_infinite_changes: 0,
                }),
            }],
        }),
        true,
    ));
}
#[test]
fn controls_and_opened_capabilities_match_generated_schema() {
    let b = Budget::new(MAX_DECODED_BYTES);
    let w = wire_encode::welcome(1, &b).unwrap();
    assert!(matches!(
        wire_reply::decode(w.bytes(), &b).unwrap().body(),
        Body::Welcome(_)
    ));
    let info = vtr_query::session::SessionInfo {
        snapshot: vtr_query::session::SnapshotId([7; 16]),
        timescale: -128,
        time_range: Some(
            vtr_query::Interval::new(u64::MAX, vtr_query::TimeBound::AfterMax).unwrap(),
        ),
        signals: u32::MAX,
        declarations: u64::MAX,
    };
    let opened = wire_encode::opened(u64::MAX, &info, &b).unwrap();
    let decoded = wire_reply::decode(opened.bytes(), &b).unwrap();
    let Body::Opened {
        info: got,
        operations,
    } = decoded.body()
    else {
        panic!("opened")
    };
    assert_eq!(got.timescale, -128);
    assert_eq!(got.time_range, info.time_range);
    assert_eq!(operations.len(), 6);
    // Prost emits packed enums; the borrowed encoder emits unpacked enums.
    let packed = p::Envelope::decode(opened.bytes()).unwrap().encode_to_vec();
    assert!(wire_reply::decode(&packed, &b).is_ok());
    let ack = wire_encode::acknowledged(3, Some(info.snapshot), &b).unwrap();
    assert!(matches!(
        wire_reply::decode(ack.bytes(), &b).unwrap().body(),
        Body::Acknowledged
    ));
    let failure = wire_encode::failure(
        4,
        Some(info.snapshot),
        p::failure::Code::ResourceLimit,
        "quota",
        &b,
    )
    .unwrap();
    assert!(
        matches!(wire_reply::decode(failure.bytes(),&b).unwrap().body(),Body::Failure { code:p::failure::Code::ResourceLimit, message } if message == "quota")
    );
}
#[test]
fn invalid_reply_semantics_and_allocation_failure_release_every_charge() {
    let good = window(vec![real(0)], known(real(0)));
    let mut cases = vec![];
    let mut e = good.clone();
    e.snapshot[0] = 6;
    cases.push(e);
    let mut e = good.clone();
    e.version = 99;
    cases.push(e);
    let mut e = good.clone();
    e.request_id = 0;
    cases.push(e);
    for mode in 0..6 {
        let mut e = good.clone();
        let Some(p::envelope::Body::Reply(d)) = &mut e.body else {
            unreachable!()
        };
        let Some(p::delivery::Page::Window(w)) = &mut d.page else {
            unreachable!()
        };
        match mode {
            0 => d.next = Some(cursor(2)),
            1 => w.complete = false,
            2 => w.changes[0].time = 0,
            3 => w.predecessor = None,
            4 => {
                w.changes[0].value = Some(p::Value {
                    value: Some(p::value::Value::Bytes(vec![])),
                })
            }
            _ => d.request.as_mut().unwrap().operation = 0,
        }
        cases.push(e);
    }
    for e in cases {
        let b = Budget::new(MAX_DECODED_BYTES);
        assert!(wire_reply::decode(&e.encode_to_vec(), &b).is_err());
        assert_eq!(b.used(), 0);
    }
    let b = Budget::new(1);
    assert!(matches!(
        wire_reply::decode(&good.encode_to_vec(), &b),
        Err(Error::ResourceLimit)
    ));
    assert_eq!(b.used(), 0);
    let b = Budget::new(MAX_DECODED_BYTES);
    let bytes = good.encode_to_vec();
    for end in 0..bytes.len() {
        assert!(
            wire_reply::decode(&bytes[..end], &b).is_err(),
            "prefix {end}"
        );
        assert_eq!(b.used(), 0);
    }
}
#[test]
fn schema_preflight_rejects_wrong_direction_unknown_duplicate_and_invalid_bool() {
    let mut duplicate = wire_encode::welcome(1, &Budget::new(8192))
        .unwrap()
        .bytes()
        .to_vec();
    duplicate.extend_from_slice(&[8, 1]);
    let wrong = p::Envelope {
        version: VERSION,
        request_id: 1,
        snapshot: vec![],
        body: Some(p::envelope::Body::Hello(p::Empty {})),
    }
    .encode_to_vec();
    let mut unknown = wrong.clone();
    unknown.extend_from_slice(&[0xf8, 1, 0]);
    for bytes in [duplicate, wrong, unknown] {
        let b = Budget::new(MAX_DECODED_BYTES);
        assert!(wire_reply::decode(&bytes, &b).is_err());
        assert_eq!(b.used(), 0);
    }
    // Encoded minimal reply with a non-boolean scalar in WindowPage.complete.
    let mut e = window(vec![], known(real(0))).encode_to_vec();
    assert_eq!(&e[e.len() - 2..], &[32, 1]);
    *e.last_mut().unwrap() = 2;
    assert!(wire_reply::decode(&e, &Budget::new(MAX_DECODED_BYTES)).is_err());
}

#[test]
fn structural_bombs_and_bad_packing_fail_with_no_retained_allocation() {
    let budget = Budget::new(MAX_DECODED_BYTES);
    let bomb = reply(
        p::delivery::Page::Resolve(p::ResolvePage {
            offset: 0,
            complete: true,
            results: (0..5000)
                .map(|_| p::Resolution {
                    result: Some(p::resolution::Result::Missing(p::Empty {})),
                })
                .collect(),
        }),
        true,
    );
    assert!(matches!(
        wire_reply::decode(&bomb.encode_to_vec(), &budget),
        Err(Error::ResourceLimit)
    ));
    for (width, states, data) in [(2048, 2, vec![0]), (1, 9, vec![15]), (1, 3, vec![0])] {
        let v = p::Value {
            value: Some(p::value::Value::Bits(p::Bits {
                width,
                states,
                data,
            })),
        };
        assert!(
            wire_reply::decode(&window(vec![v.clone()], known(v)).encode_to_vec(), &budget)
                .is_err()
        );
        assert_eq!(budget.used(), 0);
    }
    // Padding is not part of the logical vector and must retain backend bytes.
    let v = p::Value {
        value: Some(p::value::Value::Bits(p::Bits {
            width: 1,
            states: 9,
            data: vec![0xf8],
        })),
    };
    roundtrip(window(vec![v.clone()], known(v)));
    let v = p::Value {
        value: Some(p::value::Value::Bits(p::Bits {
            width: 0,
            states: 2,
            data: vec![],
        })),
    };
    roundtrip(window(vec![v.clone()], known(v)));
}
#[test]
fn generated_and_typed_objects_fit_the_shared_preflight_allowance() {
    // Two copies of the larger representation per message fit 1024 bytes;
    // payload capacities have their own factor-of-two charge in the scanner.
    macro_rules! fits { ($($ty:ty),*) => { $(assert!(std::mem::size_of::<$ty>() <= 512, stringify!($ty));)* } }
    fits!(
        p::Envelope,
        p::Welcome,
        p::SessionInfo,
        p::Failure,
        p::Delivery,
        p::WindowPage,
        p::SummaryPage,
        p::DeclarationPage,
        p::SearchPage,
        p::ResolvePage,
        p::TextPart,
        p::Change,
        p::Sample,
        p::Value,
        p::Kind,
        p::BitKind,
        p::Bits,
        p::WaveBin,
        p::RealSummary,
        p::Declaration,
        p::Text,
        p::TextReference,
        p::Scope,
        p::Variable,
        p::Stream,
        p::EnumTable,
        p::Resolution,
        vtr_query::session::Delivery,
        vtr_query::wave::WindowPage,
        vtr_query::wave::Predecessor,
        vtr_query::summary::SummaryPage,
        vtr_query::summary::WaveBin,
        vtr_query::metadata::Declaration,
        vtr_query::metadata::DeclarationPage,
        vtr_query::metadata::SearchPage,
        vtr_query::metadata::ResolvePage,
        vtr_query::metadata::TextPart,
        wire_reply::Response
    );
}

#[test]
fn detached_payload_keeps_its_decode_admission_alive() {
    let v = p::Value {
        value: Some(p::value::Value::Bytes(vec![0xff; 4096])),
    };
    let b = Budget::new(MAX_DECODED_BYTES);
    let r = wire_reply::decode(&window(vec![v.clone()], known(v)).encode_to_vec(), &b).unwrap();
    let Body::Delivery(d) = r.body() else {
        panic!("delivery")
    };
    let vtr_query::session::Reply::Window(w) = &d.reply else {
        panic!("window")
    };
    let vtr_query::wave::Value::Bytes(bytes) = &w.changes()[0].value else {
        panic!("bytes")
    };
    let bytes = bytes.clone();
    let admitted = b.used();
    drop(r);
    assert_eq!(b.used(), admitted);
    assert_eq!(bytes.as_slice(), vec![0xff; 4096]);
    drop(bytes);
    assert_eq!(b.used(), 0);
}
