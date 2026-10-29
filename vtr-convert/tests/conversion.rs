use std::path::{Path, PathBuf};
use vtr::{
    attributes::Value,
    recording::{ExecutionMode, Status, Writer, WriterOptions},
    trace::{Reader, Record},
    transactions::TransactionQuery,
};
use vtr_convert::kanata::Instruction;
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn writer(path: &Path) -> Writer {
    Writer::create_with(
        path,
        WriterOptions {
            mode: ExecutionMode::Inline,
            ..Default::default()
        },
    )
    .unwrap()
}
fn attr<'a>(r: &Reader, tx: &'a Record, name: &str) -> &'a Value {
    &tx.attributes()
        .iter()
        .find(|a| r.string(a.key).unwrap() == name)
        .unwrap()
        .value
}
#[test]
fn kanata_cycles_threads_stages_labels_relations_and_open_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.vtr");
    let mut w = writer(&path);
    vtr_convert::kanata::convert_kanata(fixture("pipeline.kanata"), &mut w, &Default::default())
        .unwrap();
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    r.validate().unwrap();
    assert_eq!(r.metadata().timescale, 0);
    assert!(
        r.metadata()
            .attributes
            .iter()
            .any(|a| r.string(a.key).unwrap() == "time.unit"
                && a.value == Value::Text("cycle".into()))
    );
    let clock = r.find_generator("cpu.cycle").unwrap();
    let mut stretches = Vec::new();
    r.transactions(
        TransactionQuery {
            generator: Some(clock),
            ..Default::default()
        },
        |record| {
            stretches.push(record);
            true
        },
    )
    .unwrap();
    assert_eq!((stretches[0].begin(), stretches[0].end()), (100, 106));
    assert_eq!(r.cycle_at(clock, 106).unwrap(), Some(6));
    let mut all = Vec::new();
    r.transactions(TransactionQuery::default(), |record| {
        if record.generator() != Some(clock) {
            all.push(record);
        }
        true
    })
    .unwrap();
    let find = |id| {
        all.iter()
            .find(|record| {
                record
                    .payload()
                    .decode::<Instruction>()
                    .unwrap()
                    .simulation_id
                    == id
            })
            .unwrap()
    };
    let (a, b, c) = (find(10), find(11), find(12));
    assert_eq!(all.len(), 3);
    for record in &all {
        assert_eq!(
            r.clock_for(record.generator().unwrap()).unwrap(),
            Some(clock)
        );
    }
    assert_eq!((a.begin(), a.end(), a.status()), (100, 103, Status::Unset));
    assert_eq!(
        (b.begin(), b.end(), b.status()),
        (101, 103, Status::Aborted)
    );
    assert_eq!((c.begin(), c.end(), c.status()), (103, 106, Status::Open));
    assert_eq!(
        attr(&r, a, "vtr.label"),
        &Value::Text("load\nr1\nretired".into())
    );
    assert_eq!(attr(&r, a, "detail"), &Value::Text("address\n0x100".into()));
    assert_eq!((a.stages()[0].begin, a.stages()[0].end), (100, Some(101)));
    assert_eq!(r.string(a.stages()[1].name).unwrap(), "X");
    assert!(
        a.stages()[0]
            .attributes
            .iter()
            .any(|a| r.string(a.key).unwrap() == "vtr.label"
                && a.value == Value::Text("fetch".into()))
    );
    let mut relations = Vec::new();
    r.relations(a.id(), false, |relation| {
        relations.push(relation);
        true
    })
    .unwrap();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].destination, b.id());
    assert_eq!(r.string(relations[0].kind).unwrap(), "wakeup");
}
#[test]
fn kanata_gzip_and_malformed_input() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let gzip = dir.path().join("input");
    let mut encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&gzip).unwrap(),
        flate2::Compression::default(),
    );
    encoder
        .write_all(&std::fs::read(fixture("pipeline.kanata")).unwrap())
        .unwrap();
    encoder.finish().unwrap();
    let mut w = writer(&dir.path().join("gzip.vtr"));
    vtr_convert::kanata::convert_kanata(gzip, &mut w, &Default::default()).unwrap();
    w.close().unwrap();
    for input in [
        "",
        "Kanata\t0003\n",
        "Kanata\t0004\nC\t-1\n",
        "Kanata\t0004\nC=\t-1\nC=\t-2\n",
        "Kanata\t0004\nC=\t9\nC=\t8\n",
        "Kanata\t0004\nI\t0\n",
        "Kanata\t0004\nL\t8\t0\tmissing\n",
        "Kanata\t0004\nI\t0\t0\t0\nI\t0\t0\t0\n",
        "Kanata\t0004\nC=\t9223372036854775807\nC\t1\n",
        "Kanata\t0004\nunknown\n",
    ] {
        let source = dir.path().join("bad.kanata");
        std::fs::write(&source, input).unwrap();
        let mut w = writer(&dir.path().join("bad.vtr"));
        assert!(
            vtr_convert::kanata::convert_kanata(source, &mut w, &Default::default()).is_err(),
            "{input}"
        );
    }
}
#[test]
fn kanata_sample_with_negative_start_cycle_converts() {
    // Published with the Kanata format documentation: Dhrystone on RSD, starting at cycle -1.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.vtr");
    let mut w = writer(&path);
    vtr_convert::kanata::convert_kanata(
        fixture("kanata-sample-2.log.gz"),
        &mut w,
        &Default::default(),
    )
    .unwrap();
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    r.validate().unwrap();
    assert!(r.metadata().attributes.iter().any(|a| {
        r.string(a.key).unwrap() == "kanata.start_cycle" && a.value == Value::I64(-1)
    }));
    let clock = r.find_generator("cpu.cycle").unwrap();
    let mut stretches = Vec::new();
    r.transactions(
        TransactionQuery {
            generator: Some(clock),
            ..Default::default()
        },
        |record| {
            stretches.push(record);
            true
        },
    )
    .unwrap();
    assert_eq!(
        stretches
            .iter()
            .map(|record| (record.begin(), record.end()))
            .collect::<Vec<_>>(),
        [(0, 4542)]
    );
    assert_eq!(r.cycle_at(clock, 0).unwrap(), Some(0));
    // The sample ends with 41 instructions still in flight, 374 flushed, and 3626 retired.
    let mut counts = [0usize; 3];
    let mut records = 0;
    r.transactions(TransactionQuery::default(), |record| {
        if record.generator() != Some(clock) {
            records += 1;
            counts[match record.status() {
                Status::Open => 0,
                Status::Aborted => 1,
                _ => 2,
            }] += 1;
        }
        true
    })
    .unwrap();
    assert_eq!(records, 4041);
    assert_eq!(counts, [41, 374, 3626]);
}
#[test]
fn vcd_short_vectors_strings_and_dump_intervals() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("values.vcd");
    std::fs::write(&input, "$var wire 4 ! bus $end\n$var string 1 \" text $end\n$enddefinitions $end\n#0\nb1 !\nshello \"\n#2\n$dumpoff\nbx !\n$end\n#4\n$dumpon\nbz !\n$end\n").unwrap();
    let path = dir.path().join("values.vtr");
    let mut w = writer(&path);
    vtr_convert::vcd::convert_vcd(input, &mut w, &Default::default()).unwrap();
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    assert_eq!(r.metadata().timescale, -9);
    let bus = r.find_signal("bus").unwrap();
    for (time, want) in [(0, "0001"), (2, "xxxx"), (4, "zzzz")] {
        assert_eq!(r.value_at(bus, time).unwrap().to_ascii(), want);
    }
    let text = r.find_signal("text").unwrap();
    assert!(
        matches!(r.value_at(text,0).unwrap(), vtr::OwnedSignalValue::VarLen(bytes) if bytes == b"hello")
    );
    assert_eq!(
        r.blackouts()
            .iter()
            .map(|b| (b.time, b.active))
            .collect::<Vec<_>>(),
        [(2, false), (4, true)]
    );
}

#[test]
fn vcd_malformed_commands_and_aliases_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let header = "$var wire 1 ! a $end\n$enddefinitions $end\n";
    for input in [
        format!("{header}#0\n1&\n"),
        format!("{header}#10\n0!\n#9\n1!\n"),
        format!("{header}#0\nb"),
        format!("{header}#0\nk!\n"),
        format!("{header}#0\nb11 !\n"),
        format!("{header}$dumpvars\n0!\n"),
        "$var wire 1 ! a $end\n$var wire 2 ! alias $end\n$enddefinitions $end\n".into(),
        "$var wire 0 ! a $end\n$enddefinitions $end\n".into(),
        "$timescale 2 ns $end\n$enddefinitions $end\n".into(),
    ] {
        let path = dir.path().join("bad.vcd");
        std::fs::write(&path, &input).unwrap();
        let mut w = writer(&dir.path().join("out.vtr"));
        assert!(
            vtr_convert::vcd::convert_vcd(path, &mut w, &Default::default()).is_err(),
            "{input}"
        );
    }
}
