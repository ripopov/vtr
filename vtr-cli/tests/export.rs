use std::path::{Path, PathBuf};
use vtr::{Reader, Writer, WriterOptions};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../vtr-convert/tests/fixtures")
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
#[test]
fn export_rejects_unrepresentable_data_and_propagates_write_errors() {
    let dir = tempfile::tempdir().unwrap();
    for kind in [
        vtr::SignalKind::VarLen,
        vtr::SignalKind::bits(1, vtr::LogicStates::Nine),
    ] {
        let path = dir.path().join("trace.vtr");
        let mut w = writer(&path);
        w.add_var(
            None,
            "a",
            vtr::VarType::Wire,
            vtr::Direction::Implicit,
            kind,
        )
        .unwrap();
        w.close().unwrap();
        let r = Reader::open(path).unwrap();
        assert!(vtr_cli::vcdout::write_vcd(&r, Vec::new()).is_err());
    }
    let path = dir.path().join("trace.vtr");
    let mut w = writer(&path);
    vtr_convert::vcd::convert_vcd(fixture("waves.vcd"), &mut w, &Default::default()).unwrap();
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    let mut tiny = [0u8; 8];
    assert!(vtr_cli::vcdout::write_vcd(&r, &mut tiny[..]).is_err());
}

#[test]
fn vcd_timebase_aliases_values_events_and_export_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.vtr");
    let mut w = writer(&path);
    vtr_convert::vcd::convert_vcd(fixture("waves.vcd"), &mut w, &Default::default()).unwrap();
    w.close().unwrap();
    let r = Reader::open(&path).unwrap();
    assert_eq!(r.meta().timescale, -8);
    assert_eq!(r.time_range().unwrap().1, 10);
    let clk = r.find_signal("top.clk", '.').unwrap();
    assert_eq!(r.find_signal("top.alias", '.'), Some(clk));
    let bus = r.find_signal("top.bus [3:0]", '.').unwrap();
    assert_eq!(r.value_at(bus, 0).unwrap().to_ascii(), "10xz");
    assert_eq!(r.value_at(bus, 5).unwrap().to_ascii(), "z001");
    let real = r.find_signal("top.temperature", '.').unwrap();
    assert_eq!(r.value_at(real, 5).unwrap().to_ascii(), "-2.5");
    let event = r.find_signal("top.pulse", '.').unwrap();
    assert_eq!(
        r.changes(event, 0, 10)
            .unwrap()
            .iter()
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        [5, 5, 7]
    );
    let out = dir.path().join("export.vcd");
    vtr_cli::vcdout::write_vcd(&r, std::fs::File::create(&out).unwrap()).unwrap();
    let again = dir.path().join("again.vtr");
    let mut w = writer(&again);
    vtr_convert::vcd::convert_vcd(out, &mut w, &Default::default()).unwrap();
    w.close().unwrap();
    let rr = Reader::open(again).unwrap();
    for name in ["top.clk", "top.bus [3:0]", "top.temperature", "top.pulse"] {
        let changes = |reader: &Reader| {
            reader
                .changes(reader.find_signal(name, '.').unwrap(), 0, 10)
                .unwrap()
                .iter()
                .map(|(t, v)| (*t, v.to_ascii()))
                .collect::<Vec<_>>()
        };
        assert_eq!(changes(&r), changes(&rr), "{name}");
    }
    assert_eq!(rr.time_range(), r.time_range());
}
