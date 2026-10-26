use vtr::{
    ExecutionMode, Reader, Recorder, SourceOptions, SpanKind, Status, Writer, WriterOptions,
};

#[test]
fn typed_states_survive_recording_open_completion_and_retained_reads() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("states.vtr");
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
        let name = writer.intern("issued").unwrap();
        let mut expected = Vec::new();
        for (kind, status) in [
            (SpanKind::Unspecified, Status::Unset),
            (SpanKind::Internal, Status::Ok),
            (SpanKind::Server, Status::Error),
            (SpanKind::Client, Status::Aborted),
            (SpanKind::Producer, Status::Open),
            (SpanKind::Consumer, Status::Ok),
        ] {
            let owner = writer.begin(source, &(), None).unwrap();
            writer.span_kind(owner.owner(), kind).unwrap();
            let child = writer
                .embedded_event(owner.owner(), 0, name, vec![])
                .unwrap();
            if status != Status::Open {
                writer.end_at(owner, 5, status).unwrap();
            }
            expected.push((owner.id(), child, kind, status));
        }
        writer.advance_time(10).unwrap();
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        let retained: Vec<_> = expected
            .iter()
            .map(|&(id, child, kind, status)| {
                let record = reader.lookup(id).unwrap().unwrap();
                assert_eq!(record.end(), if status == Status::Open { 10 } else { 5 });
                let child = reader.lookup(child).unwrap().unwrap();
                assert_eq!(child.status(), Status::Unset);
                assert_eq!(child.span_kind(), SpanKind::Unspecified);
                (record, kind, status)
            })
            .collect();
        drop(reader);
        for (record, kind, status) in retained {
            assert_eq!(record.status(), status);
            assert_eq!(record.span_kind(), kind);
        }
    }
}

#[test]
fn runtime_navigation_filters_parent_kind_and_time_and_honors_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("navigation.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let logger = writer
        .logger(SourceOptions::new("diagnostics", None))
        .unwrap();
    let source = writer
        .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
        .unwrap();
    let logs = writer
        .callsite::<u64>(vtr::CallsiteOptions::new("time {}"))
        .unwrap();
    let stages = writer
        .stages::<()>(SourceOptions::new("stage", None))
        .unwrap();
    let parent = writer.begin(source, &(), None).unwrap();
    let unrelated = writer.begin(source, &(), None).unwrap();
    let mut children = Vec::new();
    for _ in 0..3 {
        let child = writer.begin(source, &(), Some(parent.owner())).unwrap();
        children.push(child.id());
        writer.end(child, Status::Ok).unwrap();
    }
    let stage = writer.begin_stage(stages, parent.owner(), &()).unwrap();
    writer.end_stage(stage).unwrap();
    for time in [2, 3, 5, 6] {
        writer
            .log_at(logger, logs, time, &time, Some(parent.owner()))
            .unwrap();
        writer
            .log_at(logger, logs, time, &time, Some(unrelated.owner()))
            .unwrap();
    }
    writer.advance_time(10).unwrap();
    writer.end(parent, Status::Ok).unwrap();
    writer.end(unrelated, Status::Ok).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    reader.validate().unwrap();
    let mut actual = Vec::new();
    reader
        .child_transactions(parent.id(), |record| {
            actual.push(record.id());
            true
        })
        .unwrap();
    actual.sort_unstable();
    assert_eq!(actual, children);
    let mut count = 0;
    reader
        .child_transactions(parent.id(), |_| {
            count += 1;
            false
        })
        .unwrap();
    assert_eq!(count, 1);
    let mut actual = Vec::new();
    let time = vtr::TimeWindow { first: 3, last: 5 };
    reader
        .logs(parent.id(), time, |record| {
            actual.push(record.payload().decode::<u64>().unwrap());
            true
        })
        .unwrap();
    actual.sort_unstable();
    assert_eq!(actual, [3, 5]);
    let mut count = 0;
    reader
        .logs(parent.id(), time, |_| {
            count += 1;
            false
        })
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        reader
            .logs(parent.id(), vtr::TimeWindow { first: 5, last: 3 }, |_| true)
            .is_err()
    );
    reader
        .child_transactions(u64::MAX, |_| panic!("unexpected child"))
        .unwrap();
    reader
        .logs(u64::MAX, time, |_| panic!("unexpected log"))
        .unwrap();
}
