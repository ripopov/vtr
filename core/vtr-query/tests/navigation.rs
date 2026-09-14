#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use vtr_query::{
    native_navigation::FindChange,
    wave::{ChangeSearchResult, Direction, Limits},
    Budget, Cancellation, Error,
};

fn fixture() -> (tempfile::TempDir, vtr::Reader, u32, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("navigation.vtr");
    let mut writer = vtr::Writer::create_with(
        &path,
        vtr::WriterOptions {
            block_records: 2,
            dedup: false,
            background: false,
            ..Default::default()
        },
    )
    .unwrap();
    let (_, signal) = writer.add_bits("s", 64, 2);
    let (_, empty) = writer.add_bits("empty", 1, 2);
    for (i, time) in [0, 3, 3, 3, 9, u64::MAX - 1, u64::MAX]
        .into_iter()
        .enumerate()
    {
        writer.set_time(time).unwrap();
        writer.emit_u64(signal, i as u64).unwrap();
    }
    writer.close().unwrap();
    (dir, vtr::Reader::open(path).unwrap(), signal.0, empty.0)
}

fn limits() -> Limits {
    Limits {
        bytes: 256,
        records: 1,
        work: 1,
    }
}

#[test]
fn strict_navigation_matches_raw_history_at_all_boundaries() {
    let (_dir, reader, signal, empty) = fixture();
    let budget = Budget::new(1024);
    let raw = reader.changes(vtr::SignalId(signal), 0, u64::MAX).unwrap();
    for from in [0, 1, 3, 4, 9, 10, u64::MAX - 1, u64::MAX] {
        for direction in [Direction::Previous, Direction::Next] {
            let expected = match direction {
                Direction::Previous => raw.iter().rev().find(|(time, _)| *time < from),
                Direction::Next => raw.iter().find(|(time, _)| *time > from),
            }
            .map_or(ChangeSearchResult::Exhausted, |(time, _)| {
                ChangeSearchResult::Found(*time)
            });
            let mut search = FindChange::new(
                &reader,
                signal,
                from,
                direction,
                limits(),
                budget.clone(),
                Cancellation::default(),
            )
            .unwrap();
            let mut done = false;
            for _ in 0..100 {
                let page = search.next_page().unwrap();
                if page.result != ChangeSearchResult::Pending {
                    assert_eq!(
                        page.result, expected,
                        "from={from}, direction={direction:?}"
                    );
                    assert_eq!(search.next_page().unwrap().result, expected);
                    done = true;
                    break;
                }
            }
            assert!(done, "search did not finish");
            drop(search);
            assert_eq!(budget.used(), 0);
        }
    }
    let mut search = FindChange::new(
        &reader,
        empty,
        10,
        Direction::Previous,
        Limits {
            work: 100,
            ..limits()
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    assert_eq!(
        search.next_page().unwrap().result,
        ChangeSearchResult::Exhausted
    );
    assert_eq!(budget.used(), 0);
}

#[test]
fn pending_candidates_are_hidden_and_backpressure_does_not_advance() {
    let (_dir, reader, signal, _) = fixture();
    let budget = Budget::new(256);
    let mut search = FindChange::new(
        &reader,
        signal,
        10,
        Direction::Previous,
        limits(),
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    let pinned = search.next_page().unwrap();
    assert_eq!(pinned.result, ChangeSearchResult::Pending);
    let rest = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert_eq!(search.next_page().unwrap_err(), Error::ResourceLimit);
    drop(rest);
    let mut pending = 0;
    loop {
        let page = search.next_page().unwrap();
        match page.result {
            ChangeSearchResult::Pending => pending += 1,
            ChangeSearchResult::Found(time) => {
                assert_eq!(time, 9);
                break;
            }
            ChangeSearchResult::Exhausted => panic!("lost the previous edge"),
        }
        assert!(pending < 100);
    }
    assert!(pending > 1);
    assert!(budget.used() > 0);
    drop(pinned);
    drop(search);
    assert_eq!(budget.used(), 0);
}

#[test]
fn cancellation_and_invalid_inputs_do_not_bypass_boundary_checks() {
    let (_dir, reader, signal, _) = fixture();
    let budget = Budget::new(256);
    let cancel = Cancellation::default();
    let mut search = FindChange::new(
        &reader,
        signal,
        10,
        Direction::Previous,
        limits(),
        budget.clone(),
        cancel.clone(),
    )
    .unwrap();
    search.next_page().unwrap();
    cancel.cancel();
    assert_eq!(search.next_page().unwrap_err(), Error::Cancelled);
    assert!(FindChange::new(
        &reader,
        u32::MAX,
        u64::MAX,
        Direction::Next,
        limits(),
        budget.clone(),
        Cancellation::default(),
    )
    .is_err());
    for invalid in [
        Limits {
            bytes: 0,
            ..limits()
        },
        Limits {
            records: 0,
            ..limits()
        },
        Limits {
            work: 0,
            ..limits()
        },
    ] {
        assert!(FindChange::new(
            &reader,
            signal,
            0,
            Direction::Previous,
            invalid,
            budget.clone(),
            Cancellation::default(),
        )
        .is_err());
    }
    assert_eq!(budget.used(), 0);
}
