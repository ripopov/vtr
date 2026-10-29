#[path = "common/tracker.rs"]
mod scenario;

use vtr::{
    ExecutionMode, Reader, Recorder, SourceOptions, Status, Tracker, TransactionQuery, Value,
    Writer, WriterOptions,
};

#[test]
fn grouped_lifetimes_aliases_lanes_and_detached_owners_match_reference() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        scenario::write(&directory.path().join("lifecycle.vtr"), mode).unwrap();
    }
}

#[test]
fn key_and_recording_isolation_parent_replacement_and_diagnostics() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("isolation.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
            .unwrap();
        let mut tracker = Tracker::new(&mut writer, source, "requests").unwrap();
        let mut other = Tracker::new(&mut writer, source, "other").unwrap();
        let keys = tracker.keyspace("keys").unwrap();
        let foreign = other.keyspace("keys").unwrap();
        assert!(
            tracker
                .open(&mut writer, foreign, 1, "A", 0, None)
                .unwrap()
                .is_none()
        );
        assert!(tracker.owner(foreign, 1).is_none());
        assert!(tracker.records(foreign, 1).is_empty());
        tracker.bind(&mut writer, keys, 1, foreign, 1, 0).unwrap();
        tracker.bind(&mut writer, foreign, 1, keys, 1, 0).unwrap();
        for _ in 0..4 {
            tracker
                .close(&mut writer, keys, 900, Status::Ok, 0)
                .unwrap();
        }
        let missing = tracker
            .diagnostics()
            .get("close on a key that names no item")
            .unwrap();
        assert_eq!(missing.count, 4);
        assert_eq!(missing.first.as_deref(), Some("keys 900"));
        let parent = tracker
            .open(&mut writer, keys, 1, "A", 1, None)
            .unwrap()
            .unwrap();
        let owner = tracker.owner(keys, 1);
        let child = tracker
            .open(&mut writer, keys, 1, "A", 2, owner)
            .unwrap()
            .unwrap();
        assert_eq!(tracker.records(keys, 1), [child]);
        let external_path = directory.path().join("external.vtr");
        let mut external = Writer::create_with(
            &external_path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let external_source = external
            .transactions::<()>(SourceOptions::new("external", None), "REQUEST")
            .unwrap();
        let mut destination = Tracker::new(&mut external, external_source, "external").unwrap();
        let targets = destination.keyspace("target").unwrap();
        assert!(
            tracker
                .relate_to(
                    &mut writer,
                    "edge",
                    (keys, 1),
                    &mut destination,
                    (targets, 1)
                )
                .is_err()
        );
        assert!(tracker.stage(&mut external, keys, 1, "B", 3).is_err());
        assert_eq!(tracker.records(keys, 1), [child]);
        assert_eq!(
            tracker.close(&mut writer, keys, 1, Status::Ok, 2).unwrap(),
            1
        );
        writer.close().unwrap();
        external.close().unwrap();
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        assert_eq!(
            reader.lookup(parent).unwrap().unwrap().status(),
            Status::Aborted
        );
        assert_eq!(
            reader.lookup(child).unwrap().unwrap().parent(),
            Some(parent)
        );
        assert!(tracker.abort_all(&mut writer, 3).is_err());
        assert!(!tracker.take_diagnostics().is_empty());
        assert!(tracker.diagnostics().is_empty());
    }
}

#[test]
fn large_groups_shuffled_completions_and_repeated_key_reuse() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("population.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
            .unwrap();
        let mut tracker = Tracker::new(&mut writer, source, "requests").unwrap();
        let from = tracker.keyspace("source").unwrap();
        let fused = tracker.keyspace("fused").unwrap();
        for round in 0..4 {
            let time = round * 4;
            writer.advance_time(time).unwrap();
            for key in 0..1000 {
                tracker
                    .open(&mut writer, from, key, "A", time, None)
                    .unwrap();
            }
            writer.advance_time(time + 1).unwrap();
            assert_eq!(
                tracker
                    .bind_oldest(&mut writer, from, 6, fused, 1, time + 1)
                    .unwrap(),
                6
            );
            tracker.stage(&mut writer, fused, 1, "X", time + 1).unwrap();
            tracker
                .attribute(&mut writer, fused, 1, "folded", Value::U64(6))
                .unwrap();
            writer.advance_time(time + 2).unwrap();
            assert_eq!(
                tracker
                    .close(&mut writer, fused, 1, Status::Ok, time + 2)
                    .unwrap(),
                6
            );
            writer.advance_time(time + 3).unwrap();
            for i in 0..994 {
                assert_eq!(
                    tracker
                        .close(
                            &mut writer,
                            from,
                            6 + (i * 7919) % 994,
                            Status::Ok,
                            time + 3
                        )
                        .unwrap(),
                    1
                );
            }
            assert_eq!(tracker.open_count(), 0);
            assert!(tracker.diagnostics().is_empty());
        }
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        let mut count = 0;
        reader
            .transactions(TransactionQuery::default(), |r| {
                assert_eq!(r.status(), Status::Ok);
                count += 1;
                true
            })
            .unwrap();
        assert_eq!(count, 4000);
    }
}

#[test]
fn explicit_producer_tracker_moves_threads_and_interns_shared_names() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("migration.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let source = writer
        .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
        .unwrap();
    let mut tracker = Tracker::new(&mut writer, source, "requests").unwrap();
    let keys = tracker.keyspace("keys").unwrap();
    let mut producer = writer.add_producer().unwrap();
    writer.advance_time(100).unwrap();
    producer.advance_time(5).unwrap();
    let id = tracker
        .open(&mut producer, keys, 1, "A", 5, None)
        .unwrap()
        .unwrap();
    let thread = std::thread::spawn(move || {
        producer.advance_time(6).unwrap();
        tracker.stage(&mut producer, keys, 1, "B", 6).unwrap();
        tracker
            .attribute(&mut producer, keys, 1, "worker", Value::U64(1))
            .unwrap();
        tracker
            .close(&mut producer, keys, 1, Status::Ok, 6)
            .unwrap();
        producer.finish().unwrap();
        assert!(producer.intern("after finish").is_err());
    });
    thread.join().unwrap();
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    reader.validate().unwrap();
    let record = reader.lookup(id).unwrap().unwrap();
    assert_eq!(
        (record.status(), record.end(), record.stages().len()),
        (Status::Ok, 6, 2)
    );
    assert_eq!(record.stages()[0].end, Some(6));
    assert!(
        record
            .attributes()
            .iter()
            .any(|a| reader.string(a.key).unwrap() == "worker" && a.value == Value::U64(1))
    );
}

#[test]
fn constructor_rejects_foreign_recordings() {
    let directory = tempfile::tempdir().unwrap();
    let mut writer = Writer::create(directory.path().join("source.vtr")).unwrap();
    let source = writer
        .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
        .unwrap();
    let mut other = Writer::create(directory.path().join("other.vtr")).unwrap();
    assert!(Tracker::new(&mut other, source, "foreign").is_err());
    other.intern("still usable").unwrap();
    other.close().unwrap();
    writer.close().unwrap();
}
