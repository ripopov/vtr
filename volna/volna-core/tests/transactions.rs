use volna_core::data::transactions::*;
use volna_core::session::OpenSpec;

#[test]
fn capabilities_distinguish_empty_from_unsupported() {
    let fst = OpenSpec::Bytes {
        name: "features.fst".into(),
        bytes: include_bytes!("../../../ext/surfer/examples/verilator/features.fst").to_vec(),
    }
    .open()
    .unwrap();
    assert!(fst.capabilities().waveforms);
    assert!(!fst.capabilities().transactions);
    assert!(!fst.capabilities().relations);
    assert!(fst.transactions().is_none());
    assert!(fst.relations().is_none());

    let file = tempfile::NamedTempFile::new().unwrap();
    vtr::Writer::create(file.path()).unwrap().close().unwrap();
    let vtr = OpenSpec::Path(file.path().into()).open().unwrap();
    assert!(vtr.capabilities().transactions);
    assert!(vtr.capabilities().relations);
    let query = vtr.transactions().unwrap();
    assert!(query.tracks().is_empty());
    query
        .visit_transactions(&TransactionQuery::default(), &mut |_| panic!("empty trace"))
        .unwrap();
    assert!(query.transaction(TransactionRef(1)).unwrap().is_none());
    assert!(
        vtr.relations()
            .unwrap()
            .relations_from(TransactionRef(1))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn vtr_transaction_semantics_survive_the_common_contract() {
    use vtr::Value;
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    let scope = w.begin_scope("top", vtr::ScopeType::Module, "top_type");
    let stream = w.add_stream(Some(scope), "cpu", "pipeline");
    let gen_a = w.add_generator(stream, "instructions");
    let stream_b = w.add_stream(None, "bus", "tlm");
    let gen_b = w.add_generator(stream_b, "reads");
    let key = w.intern("payload");
    let text = w.intern("decoded");
    let event = w.intern("issue");
    let stage = w.intern("execute");
    let lane = w.intern("alu");
    let edge = w.intern("causes");
    let parent = w.begin_tx(gen_a, 10).unwrap();
    w.set_tx_kind(parent, TxKind::Producer).unwrap();
    for phase in [AttrPhase::Begin, AttrPhase::Record, AttrPhase::End] {
        w.tx_attr(
            parent,
            key,
            phase,
            &Value::Map(vec![(
                key,
                Value::List(vec![Value::Str(text), Value::Bytes(vec![0, 255])]),
            )]),
        )
        .unwrap();
    }
    w.tx_event(parent, 12, event, &[(key, Value::U64(42))])
        .unwrap();
    w.tx_stage(parent, stage, lane, 12, 18, &[(key, Value::Bool(true))])
        .unwrap();
    w.end_tx(parent, 20, TxStatus::Aborted).unwrap();
    let child = w.begin_tx(gen_b, 20).unwrap();
    w.set_tx_parent(child, parent).unwrap();
    w.end_tx(child, 20, TxStatus::Ok).unwrap();
    w.relate(
        edge,
        parent,
        child,
        &[(
            key,
            Value::Enum {
                value: 3,
                name: text,
            },
        )],
    )
    .unwrap();
    w.close().unwrap();

    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    let txs = session.transactions().unwrap();
    assert_eq!(txs.tracks().len(), 4);
    let generator = txs
        .tracks()
        .iter()
        .find(|t| t.path == ["top", "cpu", "instructions"])
        .unwrap();
    assert_eq!(
        generator.kind,
        TrackKind::Generator {
            stream: TrackRef(stream.0)
        }
    );
    let p = txs.transaction(TransactionRef(parent)).unwrap().unwrap();
    assert_eq!(
        (p.begin, p.end, p.status, p.kind),
        (10, 20, TxStatus::Aborted, TxKind::Producer)
    );
    assert_eq!(
        p.attributes.iter().map(|a| a.phase).collect::<Vec<_>>(),
        [AttrPhase::Begin, AttrPhase::Record, AttrPhase::End]
    );
    assert_eq!(
        p.attributes[0].value,
        AttributeValue::Map(vec![(
            "payload".into(),
            AttributeValue::List(vec![
                AttributeValue::Text("decoded".into()),
                AttributeValue::Bytes(vec![0, 255])
            ])
        )])
    );
    assert_eq!(p.events[0].name, "issue");
    assert_eq!(p.events[0].time, 12);
    assert_eq!(p.stages[0].lane, "alu");
    assert_eq!(p.stages[0].end, Some(18));
    assert_eq!(
        txs.transaction(TransactionRef(child))
            .unwrap()
            .unwrap()
            .parent,
        Some(TransactionRef(parent))
    );
    let mut ids = Vec::new();
    txs.visit_transactions(
        &TransactionQuery {
            window: Some((20, 20)),
            ..Default::default()
        },
        &mut |tx| {
            ids.push(tx.id);
            true
        },
    )
    .unwrap();
    assert_eq!(
        ids.len(),
        2,
        "inclusive overlap includes ending and point transactions"
    );
    ids.clear();
    txs.visit_transactions(
        &TransactionQuery {
            generator: Some(generator.id),
            ..Default::default()
        },
        &mut |tx| {
            ids.push(tx.id);
            true
        },
    )
    .unwrap();
    assert_eq!(ids, [TransactionRef(parent)]);
    let mut visits = 0;
    txs.visit_transactions(&TransactionQuery::default(), &mut |_| {
        visits += 1;
        false
    })
    .unwrap();
    assert_eq!(visits, 1);
    assert!(
        txs.visit_transactions(
            &TransactionQuery {
                window: Some((2, 1)),
                ..Default::default()
            },
            &mut |_| true
        )
        .is_err()
    );
    assert!(
        txs.visit_transactions(
            &TransactionQuery {
                generator: Some(TrackRef(u32::MAX)),
                ..Default::default()
            },
            &mut |_| true
        )
        .is_err()
    );
    let relations = session.relations().unwrap();
    let from = relations.relations_from(TransactionRef(parent)).unwrap();
    assert_eq!(from, relations.relations_to(TransactionRef(child)).unwrap());
    assert_eq!(from[0].kind, "causes");
    assert_eq!(
        from[0].attributes[0].1,
        AttributeValue::Enum {
            value: 3,
            name: "decoded".into()
        }
    );
    drop(session);
    assert_eq!(
        p.events[0].attributes[0].1,
        AttributeValue::U64(42),
        "results outlive their session"
    );
}
