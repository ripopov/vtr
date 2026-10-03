use volna_core::testing::a as in_a;
use volna_core::trace::TraceId;
use volna_trace::data::transactions::*;
use volna_trace::session::OpenSpec;

#[test]
fn remote_metadata_queues_complete_tracks() {
    use std::sync::Arc;
    use volna_core::document::{Document, TrackLoadState};
    use volna_core::session::{LoadRequest, LoadResult};
    use volna_trace::remote::{objects::Metadata, session::RemoteSession};
    use volna_trace::session::Session;

    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let stream = writer.add_stream(None, "bus", "tlm").unwrap();
    let generator = writer.add_generator(stream, "requests").unwrap();
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
    // Viewer metadata installation does not affect raw loading.
    for request in document.take_requests() {
        document.deliver(request.perform());
    }
    let track = TrackRef(stream.0);
    document.retain_track(in_a(track)).unwrap();
    document.retain_track(in_a(track)).unwrap();
    let mut requests = document.take_requests();
    assert_eq!(requests.len(), 1);
    let LoadRequest::Track {
        generation,
        request_id,
        session,
        track,
        ..
    } = requests.pop().unwrap()
    else {
        panic!("expected track load");
    };
    assert_eq!(session.remote_id(), Some(41));
    assert!(session.load_track(track).is_err());
    assert!(matches!(
        document.track(in_a(track)),
        Some(TrackLoadState::Loading)
    ));
    let loaded = local.load_track(track).unwrap();
    let weak = Arc::downgrade(&loaded.generators[0]);
    assert!(
        document
            .deliver(LoadResult::Track {
                trace: TraceId::A,
                generation,
                request_id,
                track,
                result: volna_core::data::loaded_tracks::LoadedTrack::prepare(loaded, None),
            })
            .is_some()
    );
    document.retain_track(in_a(TrackRef(generator.0))).unwrap();
    assert!(document.take_requests().is_empty());
    document.release_track(in_a(track));
    document.release_track(in_a(track));
    assert!(weak.upgrade().is_some());
    document.release_track(in_a(TrackRef(generator.0)));
    assert!(weak.upgrade().is_none());
}

#[test]
fn document_track_loads_coalesce_share_release_retry_and_reject_stale_results() {
    use std::sync::Arc;
    use volna_core::document::{Document, TrackLoadState};

    use volna_core::session::{LoadRequest, LoadResult};
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let stream = writer.add_stream(None, "stream", "transactions").unwrap();
    let generator = writer.add_generator(stream, "generator").unwrap();
    writer.close().unwrap();
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    let track = TrackRef(generator.0);
    let stream = TrackRef(stream.0);
    let mut doc = Document::new();
    doc.set_session(session.clone());
    // The open's own work: count scope sizes.
    for request in doc.take_requests() {
        doc.deliver(request.perform());
    }
    doc.retain_track(in_a(stream)).unwrap();
    doc.retain_track(in_a(stream)).unwrap();
    let requests = doc.take_requests();
    assert_eq!(requests.len(), 1);
    for request in requests {
        doc.deliver(request.perform());
    }
    doc.retain_track(in_a(track)).unwrap();
    assert!(
        doc.take_requests().is_empty(),
        "reuse generator loaded by stream"
    );
    let (Some(TrackLoadState::Ready(a)), Some(TrackLoadState::Ready(b))) =
        (doc.track(in_a(stream)), doc.track(in_a(track)))
    else {
        panic!("ready");
    };
    assert!(Arc::ptr_eq(&a.generators[0], &b.generators[0]));
    let weak = Arc::downgrade(&a.generators[0]);
    doc.release_track(in_a(stream));
    assert!(doc.track(in_a(stream)).is_some());
    doc.release_track(in_a(stream));
    doc.release_track(in_a(track));
    assert!(weak.upgrade().is_none());

    doc.retain_track(in_a(track)).unwrap();
    let old = doc.take_requests().pop().unwrap();
    doc.release_track(in_a(track));
    doc.retain_track(in_a(track)).unwrap();
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
        trace: TraceId::A,
        generation,
        request_id,
        track,
        result: Err(anyhow::anyhow!("disconnected")),
    });
    assert!(matches!(
        doc.track(in_a(track)),
        Some(TrackLoadState::Failed(_))
    ));
    assert!(doc.retry_track(in_a(track)));
    assert!(!doc.retry_track(in_a(track)));
    assert!(
        doc.deliver(current.perform()).is_none(),
        "old completion cannot overwrite retry"
    );
    for request in doc.take_requests() {
        doc.deliver(request.perform());
    }
    assert!(matches!(
        doc.track(in_a(track)),
        Some(TrackLoadState::Ready(_))
    ));
    doc.release_track(in_a(track));
    doc.retain_track(in_a(track)).unwrap();
    doc.release_track(in_a(track));
    assert!(doc.take_requests().is_empty(), "removed queued demand");
    doc.retain_track(in_a(track)).unwrap();
    let stale = doc.take_requests().pop().unwrap();
    doc.set_session(session);
    assert!(doc.deliver(stale.perform()).is_none());
    assert!(doc.track(in_a(track)).is_none());
}
