use rand::{Rng, SeedableRng, rngs::StdRng};
use vtr::*;

#[test]
fn named_lookups_distinguish_waveforms_clocks_and_grouping_scopes() {
    let directory = tempfile::tempdir().unwrap();
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for clock_first in [false, true] {
            let path = directory.path().join(format!("{mode:?}-{clock_first}.vtr"));
            let mut writer = Writer::create_with(
                &path,
                WriterOptions {
                    mode,
                    ..Default::default()
                },
            )
            .unwrap();
            let root = writer.scope("top", None).unwrap();
            let options = || SourceOptions::new("clk", Some(root));
            let (signal, clock) = if clock_first {
                let clock = writer.clock(options()).unwrap();
                let signal = writer
                    .signal(
                        options(),
                        SignalKind::Bits {
                            width: 1,
                            states: 2,
                        },
                        VarType::Wire,
                        Direction::Input,
                    )
                    .unwrap();
                (signal, clock)
            } else {
                let signal = writer
                    .signal(
                        options(),
                        SignalKind::Bits {
                            width: 1,
                            states: 2,
                        },
                        VarType::Wire,
                        Direction::Input,
                    )
                    .unwrap();
                let clock = writer.clock(options()).unwrap();
                (signal, clock)
            };
            let grouped_signal = writer
                .signal(
                    SourceOptions::new("group", Some(root)),
                    SignalKind::Real,
                    VarType::Real,
                    Direction::Implicit,
                )
                .unwrap();
            let group = writer.scope("group", Some(root)).unwrap();
            let source = writer
                .transactions::<u64>(SourceOptions::new("requests", Some(group)), "REQUEST")
                .unwrap();
            writer.associate_clock(source, clock).unwrap();
            writer.clock_run(clock, 0, 2).unwrap();
            writer.emit_u64(signal, 1).unwrap();
            writer.emit_real(grouped_signal, 3.5).unwrap();
            let owner = writer.begin(source, &42, None).unwrap();
            writer.advance_time(8).unwrap();
            writer.end(owner, Status::Ok).unwrap();
            writer.close().unwrap();
            let reader = Reader::open(path).unwrap();
            assert_eq!(reader.find_generator("top.clk"), Some(clock.id()));
            assert_eq!(reader.find_signal("top.clk"), Some(signal));
            assert_eq!(reader.find_signal("top.group"), Some(grouped_signal));
            assert_eq!(
                reader.find_generator("top.group.requests"),
                Some(source.id())
            );
            assert_eq!(reader.find_generator("top.group"), None);
            assert_eq!(reader.find_signal("top.group.requests"), None);
            assert_eq!(
                reader.value_at(signal, 8).unwrap().borrow().as_u64(),
                Some(1)
            );
            assert_eq!(
                reader
                    .lookup(owner.id())
                    .unwrap()
                    .unwrap()
                    .payload()
                    .decode::<u64>()
                    .unwrap(),
                42
            );
            assert_eq!(reader.cycle_at(clock.id(), 8).unwrap(), Some(4));
            reader.validate().unwrap();
        }
    }
}

#[test]
fn random_gated_clock_runs_match_brute_force_cycles() {
    let directory = tempfile::tempdir().unwrap();
    let mut rng = StdRng::seed_from_u64(19);
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for round in 0..12 {
            let path = directory.path().join(format!("{mode:?}-{round}.vtr"));
            let mut options = WriterOptions {
                mode,
                ..WriterOptions::default()
            };
            options.encoding.block_bytes = 128;
            let mut writer = Writer::create_with(&path, options).unwrap();
            let clock = writer.clock(SourceOptions::new("clock", None)).unwrap();
            let transactions = writer
                .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
                .unwrap();
            writer.associate_clock(transactions, clock).unwrap();
            let mut time = 0;
            let mut edges = Vec::new();
            let mut ids = Vec::new();
            for run in 0..12 {
                let first = time + rng.gen_range(1..=9);
                let period = rng.gen_range(1..=7);
                let stop = first + rng.gen_range(0..=13);
                let id = writer.clock_run(clock, first, period).unwrap();
                ids.push(id);
                if run == 11 {
                    time = stop;
                    writer.advance_time(time).unwrap();
                } else {
                    writer.clock_stop(clock, stop).unwrap();
                    time = stop;
                }
                let mut edge = first;
                while edge <= stop {
                    edges.push(edge);
                    edge += period;
                }
            }
            writer.close().unwrap();
            let reader = Reader::open(path).unwrap();
            assert_eq!(
                reader.clock_for(transactions.id()).unwrap(),
                Some(clock.id())
            );
            for query in 0..=time + 5 {
                let count = edges.partition_point(|edge| *edge <= query);
                assert_eq!(
                    reader.cycle_at(clock.id(), query).unwrap(),
                    count.checked_sub(1).map(|index| index as u64)
                );
            }
            for id in ids {
                let run = reader.lookup(id).unwrap().unwrap();
                assert_eq!(run.kind(), RecordKind::Clock);
                assert!(edges.contains(&run.begin()));
                assert!(edges.contains(&run.end()));
            }
            reader.validate().unwrap();
        }
    }
}

#[test]
fn changing_an_active_period_closes_the_previous_run_without_duplicate_edges() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("change.vtr");
    let mut writer = Writer::create(&path).unwrap();
    let clock = writer.clock(SourceOptions::new("clock", None)).unwrap();
    let first = writer.clock_run(clock, 1, 3).unwrap();
    let second = writer.clock_run(clock, 9, 4).unwrap();
    writer.advance_time(21).unwrap();
    writer.close().unwrap();
    let reader = Reader::open(path).unwrap();
    assert_eq!(reader.lookup(first).unwrap().unwrap().end(), 7);
    assert_eq!(reader.lookup(second).unwrap().unwrap().end(), 21);
    for (time, cycle) in [
        (0, None),
        (1, Some(0)),
        (8, Some(2)),
        (9, Some(3)),
        (21, Some(6)),
        (25, Some(6)),
    ] {
        assert_eq!(reader.cycle_at(clock.id(), time).unwrap(), cycle);
    }
}
