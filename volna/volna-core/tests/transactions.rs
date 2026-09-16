use volna_core::data::transactions::*;
use volna_core::session::OpenSpec;

#[test]
fn remote_metadata_queues_complete_tracks() {
    use std::sync::Arc;
    use volna_core::document::{Document, TrackLoadState};
    use volna_core::remote::{objects::Metadata, session::RemoteSession};
    use volna_core::session::{LoadRequest, LoadResult, Session};

    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let stream = writer.add_stream(None, "bus", "tlm");
    let generator = writer.add_generator(stream, "requests");
    let tx = writer.begin_tx(generator, 1).unwrap();
    writer.end_tx(tx, 100, TxStatus::Ok).unwrap();
    writer.close().unwrap();
    let local = OpenSpec::Path(file.path().into()).open().unwrap();
    assert_eq!(local.remote_id(), None);
    let metadata = Metadata::from_session(local.as_ref());
    assert!(RemoteSession::new(0, metadata.clone()).is_err());
    let remote = Arc::new(RemoteSession::new(41, metadata).unwrap());
    assert!(remote.capabilities().transactions);
    assert_eq!(remote.tracks(), local.tracks());
    assert_eq!(
        Metadata::from_session(remote.as_ref()).tracks,
        local.tracks()
    );
    let mut document = Document::new();
    document.set_session(remote);
    let track = TrackRef(stream.0);
    document.retain_track(track).unwrap();
    document.retain_track(track).unwrap();
    let mut requests = document.take_requests();
    assert_eq!(requests.len(), 1);
    let LoadRequest::Track {
        generation,
        request_id,
        session,
        track,
    } = requests.pop().unwrap()
    else {
        panic!("expected track load");
    };
    assert_eq!(session.remote_id(), Some(41));
    assert!(session.load_track(track).is_err());
    assert!(matches!(
        document.track(track),
        Some(TrackLoadState::Loading)
    ));
    let loaded = local.load_track(track).unwrap();
    let weak = Arc::downgrade(&loaded.generators[0]);
    assert!(
        document
            .deliver(LoadResult::Track {
                generation,
                request_id,
                track,
                result: Ok(loaded),
            })
            .is_some()
    );
    document.retain_track(TrackRef(generator.0)).unwrap();
    assert!(document.take_requests().is_empty());
    document.release_track(track);
    document.release_track(track);
    assert!(weak.upgrade().is_some());
    document.release_track(TrackRef(generator.0));
    assert!(weak.upgrade().is_none());
}

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
    assert!(fst.tracks().is_empty());
    assert!(fst.load_track(TrackRef(0)).is_err());

    let file = tempfile::NamedTempFile::new().unwrap();
    vtr::Writer::create(file.path()).unwrap().close().unwrap();
    let vtr = OpenSpec::Path(file.path().into()).open().unwrap();
    assert!(vtr.capabilities().transactions);
    assert!(vtr.capabilities().relations);
    assert!(vtr.tracks().is_empty());
    assert!(vtr.load_track(TrackRef(0)).is_err());
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
    assert_eq!(session.tracks().len(), 4);
    let generator = session
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
    let loaded_a = session.load_track(generator.id).unwrap();
    let loaded_b = session.load_track(TrackRef(stream_b.0)).unwrap();
    let a = &loaded_a.generators[0];
    let b = &loaded_b.generators[0];
    let p = a.transaction(TransactionRef(parent)).unwrap();
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
        b.transaction(TransactionRef(child)).unwrap().parent,
        Some(TransactionRef(parent))
    );
    assert!(a.transaction(TransactionRef(child)).is_none());
    let mut ids = Vec::new();
    for records in [a, b] {
        records
            .visit_window(20, 20, |tx| {
                ids.push(tx.id);
                true
            })
            .unwrap();
    }
    assert_eq!(
        ids.len(),
        2,
        "inclusive overlap includes ending and point transactions"
    );
    assert_eq!(
        a.transactions().iter().map(|tx| tx.id).collect::<Vec<_>>(),
        [TransactionRef(parent)]
    );
    let mut visits = 0;
    assert!(
        !a.visit_window(0, u64::MAX, |_| {
            visits += 1;
            false
        })
        .unwrap()
    );
    assert_eq!(visits, 1);
    assert!(a.visit_window(2, 1, |_| true).is_err());
    assert!(session.load_track(TrackRef(u32::MAX)).is_err());
    assert_eq!(a.relations(), b.relations());
    let from: Vec<_> = a.relations().iter().map(|edge| &edge.relation).collect();
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

#[test]
fn complete_tracks_include_empty_generators_parents_and_parallel_relations() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let stream = writer.add_stream(None, "stream", "transactions");
    let generator = writer.add_generator(stream, "generator");
    let empty = writer.add_generator(stream, "empty");
    let other_stream = writer.add_stream(None, "other", "transactions");
    let other = writer.add_generator(other_stream, "other-generator");
    let parent = writer.begin_tx(other, 0).unwrap();
    writer.end_tx(parent, 100, TxStatus::Ok).unwrap();
    let child = writer.begin_tx(generator, 50).unwrap();
    writer.set_tx_parent(child, parent).unwrap();
    writer.end_tx(child, 50, TxStatus::Ok).unwrap();
    let kind = writer.intern("parallel");
    for _ in 0..2 {
        writer.relate(kind, parent, child, &[]).unwrap();
    }
    writer.close().unwrap();

    let reader = vtr::Reader::open(file.path()).unwrap();
    assert_eq!(reader.transaction_generator(parent).unwrap(), Some(other));
    assert_eq!(
        reader.transaction_generator(child).unwrap(),
        Some(generator)
    );
    assert_eq!(reader.transaction_generator(u64::MAX).unwrap(), None);
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    let loaded = session.load_track(TrackRef(stream.0)).unwrap();
    assert_eq!(loaded.generators.len(), 2);
    let records = loaded
        .generators
        .iter()
        .find(|g| g.generator() == TrackRef(generator.0))
        .unwrap();
    assert_eq!(records.transactions().len(), 1);
    assert_eq!(
        records.parent(TransactionRef(child)).unwrap().generator,
        TrackRef(other.0)
    );
    assert_eq!(records.relations().len(), 2);
    assert_ne!(records.relations()[0].id, records.relations()[1].id);
    assert_eq!(
        records.relations()[0].relation,
        records.relations()[1].relation
    );
    assert!(
        loaded
            .generators
            .iter()
            .find(|g| g.generator() == TrackRef(empty.0))
            .unwrap()
            .transactions()
            .is_empty()
    );
    let opposite = session.load_track(TrackRef(other.0)).unwrap();
    assert_eq!(records.relations(), opposite.generators[0].relations());
    assert!(session.load_track(TrackRef(u32::MAX)).is_err());
    drop(session);
    let mut hits = vec![];
    opposite.generators[0]
        .visit_window(50, 50, |tx| {
            hits.push(tx.id);
            true
        })
        .unwrap();
    assert_eq!(hits, [TransactionRef(parent)]);
}

#[test]
fn document_track_loads_coalesce_share_release_retry_and_reject_stale_results() {
    use std::sync::Arc;
    use volna_core::document::{Document, TrackLoadState};
    use volna_core::session::{LoadRequest, LoadResult};
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let stream = writer.add_stream(None, "stream", "transactions");
    let generator = writer.add_generator(stream, "generator");
    writer.close().unwrap();
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    let track = TrackRef(generator.0);
    let stream = TrackRef(stream.0);
    let mut doc = Document::new();
    doc.set_session(session.clone());
    doc.retain_track(stream).unwrap();
    doc.retain_track(stream).unwrap();
    let requests = doc.take_requests();
    assert_eq!(requests.len(), 1);
    for request in requests {
        doc.deliver(request.perform());
    }
    doc.retain_track(track).unwrap();
    assert!(
        doc.take_requests().is_empty(),
        "reuse generator loaded by stream"
    );
    let (Some(TrackLoadState::Ready(a)), Some(TrackLoadState::Ready(b))) =
        (doc.track(stream), doc.track(track))
    else {
        panic!("ready");
    };
    assert!(Arc::ptr_eq(&a.generators[0], &b.generators[0]));
    let weak = Arc::downgrade(&a.generators[0]);
    doc.release_track(stream);
    assert!(doc.track(stream).is_some());
    doc.release_track(stream);
    doc.release_track(track);
    assert!(weak.upgrade().is_none());

    doc.retain_track(track).unwrap();
    let old = doc.take_requests().pop().unwrap();
    doc.release_track(track);
    doc.retain_track(track).unwrap();
    let current = doc.take_requests().pop().unwrap();
    assert!(doc.deliver(old.perform()).is_none());
    let LoadRequest::Track {
        generation,
        request_id,
        ..
    } = &current
    else {
        panic!("track");
    };
    let (generation, request_id) = (*generation, *request_id);
    doc.deliver(LoadResult::Track {
        generation,
        request_id,
        track,
        result: Err(anyhow::anyhow!("disconnected")),
    });
    assert!(matches!(doc.track(track), Some(TrackLoadState::Failed(_))));
    assert!(doc.retry_track(track));
    assert!(!doc.retry_track(track));
    assert!(
        doc.deliver(current.perform()).is_none(),
        "old completion cannot overwrite retry"
    );
    for request in doc.take_requests() {
        doc.deliver(request.perform());
    }
    assert!(matches!(doc.track(track), Some(TrackLoadState::Ready(_))));
    doc.release_track(track);
    doc.retain_track(track).unwrap();
    doc.release_track(track);
    assert!(doc.take_requests().is_empty(), "removed queued demand");
    doc.retain_track(track).unwrap();
    let stale = doc.take_requests().pop().unwrap();
    doc.set_session(session);
    assert!(doc.deliver(stale.perform()).is_none());
    assert!(doc.track(track).is_none());
}
