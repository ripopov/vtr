#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use vtr_query::{native_history::Build, wave::Direction, Budget, Cancellation, Error};

#[test]
fn admission_follows_completed_histories_and_releases_failed_builds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            block_records: 4,
            background: false,
            dedup: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_bits("clock", 1, 2);
    for time in 0..100 {
        writer.set_time(time).unwrap();
        writer.emit_u64(signal, time % 2).unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(8192);
    let pinned = budget.reserve(8192).unwrap();
    assert!(matches!(
        Build::new(&reader, signal.0, 4096, &budget, Cancellation::default()),
        Err(Error::ResourceLimit)
    ));
    drop(pinned);
    let cancellation = Cancellation::default();
    let mut build = Build::new(&reader, signal.0, 4096, &budget, cancellation.clone()).unwrap();
    assert!(budget.used() >= 4096);
    assert!(build.advance(1).unwrap().is_none());
    cancellation.cancel();
    assert!(matches!(build.advance(1), Err(Error::Cancelled)));
    assert_eq!(budget.used(), 0);
    let mut build = Build::new(&reader, signal.0, 200, &budget, Cancellation::default()).unwrap();
    loop {
        match build.advance(1) {
            Ok(None) => {}
            Err(Error::ResourceLimit) => break,
            _ => panic!("oversize construction accepted"),
        }
    }
    assert_eq!(budget.used(), 0);
    let mut build = Build::new(&reader, signal.0, 4096, &budget, Cancellation::default()).unwrap();
    let history = loop {
        if let Some(history) = build.advance(1).unwrap() {
            break history;
        }
    };
    assert!(budget.used() < 4096);
    let retained = budget.used();
    let clone = build.advance(1).unwrap().unwrap();
    assert!(std::sync::Arc::ptr_eq(&history, &clone));
    drop(build);
    drop(reader);
    assert_eq!(history.index().find_change(50, Direction::Next), Some(51));
    assert_eq!(budget.used(), retained);
    drop(history);
    assert_eq!(budget.used(), retained);
    drop(clone);
    assert_eq!(budget.used(), 0);
}
