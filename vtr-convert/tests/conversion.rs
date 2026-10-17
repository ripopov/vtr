use std::path::{Path, PathBuf};
use vtr::{Reader, TxQuery, TxStatus, Value, Writer, WriterOptions};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn writer(path: &Path) -> Writer {
    Writer::create_with(
        path,
        WriterOptions {
            dedup: false,
            background: false,
            ..Default::default()
        },
    )
    .unwrap()
}
fn attr<'a>(r: &Reader, tx: &'a vtr::Transaction, name: &str) -> &'a Value {
    &tx.attrs
        .iter()
        .find(|a| r.str(a.key) == name)
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
    assert_eq!(r.meta().timescale, 0);
    assert!(r.meta().attrs.iter().any(
        |(k, v)| r.str(*k) == "time.unit" && matches!(v, Value::Str(s) if r.str(*s) == "cycle")
    ));
    let clock = &r.clocks()[0];
    assert_eq!(clock.path, "cpu.cycle");
    let timeline = r.clock(clock.id).unwrap();
    let stretch = &timeline.stretches()[0];
    assert_eq!((stretch.begin, stretch.end, stretch.period), (100, 106, 1));
    let streams: Vec<_> = r.generators().filter(|s| *s != clock.generator).collect();
    assert_eq!(streams.len(), 2);
    assert!(
        streams
            .iter()
            .all(|s| r.generator_clock(*s) == Some(clock.id))
    );
    let all = r.transactions(&TxQuery::default()).unwrap();
    let find = |id| {
        all.iter()
            .find(|t| {
                t.attrs
                    .iter()
                    .any(|a| r.str(a.key) == "insn_id_in_sim" && a.value == Value::I64(id))
            })
            .unwrap()
    };
    let (a, b, c) = (find(10), find(11), find(12));
    assert_eq!((a.begin, a.end, a.status), (100, 103, TxStatus::Unset));
    assert_eq!((b.begin, b.end, b.status), (101, 103, TxStatus::Aborted));
    assert_eq!((c.begin, c.end, c.status), (103, 106, TxStatus::Open));
    assert!(matches!(attr(&r,a,"vtr.label"), Value::Str(s) if r.str(*s) == "load\nr1\nretired"));
    assert!(matches!(attr(&r,a,"detail"), Value::Str(s) if r.str(*s) == "address\n0x100"));
    assert_eq!((a.stages[0].begin, a.stages[0].end), (100, Some(101)));
    assert_eq!(r.str(a.stages[1].name), "X");
    assert!(
        a.stages[0]
            .attrs
            .iter()
            .any(|(k, v)| r.str(*k) == "vtr.label"
                && matches!(v,Value::Str(s) if r.str(*s)=="fetch"))
    );
    let relations = r.relations_from(a.id).unwrap();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].to, b.id);
    assert_eq!(r.str(relations[0].kind), "wakeup");
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
        "Kanata\t0004\nC=\t-1\n",
        "Kanata\t0004\nC=\t9\nC=\t8\n",
        "Kanata\t0004\nI\t0\n",
        "Kanata\t0004\nL\t8\t0\tmissing\n",
        "Kanata\t0004\nI\t0\t0\t0\nI\t0\t0\t0\n",
        "Kanata\t0004\nC=\t18446744073709551615\nC\t1\n",
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
fn vcd_short_vectors_strings_and_dump_intervals() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("values.vcd");
    std::fs::write(&input, "$var wire 4 ! bus $end\n$var string 1 \" text $end\n$enddefinitions $end\n#0\nb1 !\nshello \"\n#2\n$dumpoff\nbx !\n$end\n#4\n$dumpon\nbz !\n$end\n").unwrap();
    let path = dir.path().join("values.vtr");
    let mut w = writer(&path);
    vtr_convert::vcd::convert_vcd(input, &mut w, &Default::default()).unwrap();
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    assert_eq!(r.meta().timescale, -9);
    let bus = r.find_signal("bus", '.').unwrap();
    for (time, want) in [(0, "0001"), (2, "xxxx"), (4, "zzzz")] {
        assert_eq!(r.value_at(bus, time).unwrap().to_ascii(), want);
    }
    let text = r.find_signal("text", '.').unwrap();
    assert!(
        matches!(r.value_at(text,0).unwrap(), vtr::OwnedSignalValue::VarLen(bytes) if bytes == b"hello")
    );
    assert_eq!(
        r.blackout()
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
