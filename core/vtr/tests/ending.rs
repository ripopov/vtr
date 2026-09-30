//! How a run ended (docs/crash-safe-vtr.html, stage 3; SPEC section 8.5).

use vtr::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-ending-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A run with value changes and log records of its own, ended as given.
fn write(name: &str, end: Option<Ending>, background: bool) -> std::path::PathBuf {
    let path = tmp(name);
    let opts = WriterOptions { background, tx_block_bytes: 256, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, a) = w.add_var(None, "a", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap();
    let log = w.add_stream(None, "sim", LOG_STREAM_KIND).unwrap();
    let site = w.add_log_site(&LogSiteSpec::new(log, Severity::Fatal, "step {}", &[LogArgType::U64])).unwrap();
    for t in 0..300u64 {
        w.set_time(t * 10).unwrap();
        w.emit_u64(a, t).unwrap();
        w.log(site, t * 10, &[LogArg::U64(t)]).unwrap();
    }
    match end {
        Some(e) => w.close_with(e).unwrap(),
        None => w.close().unwrap(),
    }
    path
}

const ENDINGS: [Ending; 6] = [
    Ending::Stopped { signal: 15 },
    Ending::Exited { status: 3 },
    Ending::Crashed { signal: 11, code: 1, address: 0, thread: 41137, sealed: false },
    Ending::Crashed { signal: 6, code: -6, address: 0xdead_beef, thread: 7, sealed: true },
    Ending::Recovered { dropped: 616 },
    Ending::Crashed { signal: 99, code: 0, address: u64::MAX, thread: 0, sealed: false },
];

#[test]
fn every_ending_round_trips() {
    for background in [false, true] {
        for (i, end) in ENDINGS.into_iter().enumerate() {
            let path = write(&format!("end{i}-{background}.vtr"), Some(end), background);
            let r = Reader::open(&path).unwrap();
            assert_eq!(r.recovered(), None);
            assert_eq!(r.ending().unwrap(), (end, Some(2990)), "{end:?}");
            // The record is an ordinary FATAL log record: every log reader shows it, last.
            let mut last = None;
            r.visit_log(&LogQuery { min_severity: Severity::Fatal, ..Default::default() }, |rec| {
                last = Some((r.full_path(rec.site.stream, "."), rec.format(r.strings()), rec.time));
                true
            })
            .unwrap();
            let (stream, text, time) = last.unwrap();
            assert_eq!((stream.as_str(), time), (ending::STREAM, 2990));
            assert!(text.starts_with("run "), "{text}");
            assert_eq!(r.log_count(), 301);
        }
    }
}

#[test]
fn text_of_each_ending() {
    let text: Vec<String> = ENDINGS.iter().map(|e| e.to_string()).collect();
    assert_eq!(
        text,
        [
            "stopped by SIGTERM",
            "exited with status 3",
            "crashed by SIGSEGV (address 0x0, thread 41137)",
            "crashed by SIGABRT (address 0xdeadbeef, thread 7, sealed mid-call)",
            "recovered by scanning (616 bytes dropped)",
            "crashed by signal 99 (address 0xffffffffffffffff, thread 0)",
        ]
    );
    assert_eq!(Ending::Closed.to_string(), "closed");
}

#[test]
fn a_normal_close_writes_no_record() {
    for end in [None, Some(Ending::Closed)] {
        let path = write("closed.vtr", end, true);
        let r = Reader::open(&path).unwrap();
        assert_eq!(r.ending().unwrap(), (Ending::Closed, None));
        assert!(r.find_node(&[ending::STREAM]).is_none());
        assert_eq!(r.log_count(), 300);
    }
}

#[test]
fn a_recovered_file_without_a_record() {
    let full = std::fs::read(write("cut.vtr", None, false)).unwrap();
    let r = Reader::from_bytes(full[..full.len() - 100].to_vec()).unwrap();
    let dropped = r.recovered().unwrap();
    assert_eq!(r.ending().unwrap(), (Ending::Recovered { dropped }, None));
}

#[test]
fn the_stream_is_reserved() {
    let mut w = Writer::create(tmp("reserved.vtr")).unwrap();
    assert!(matches!(w.add_stream(None, ending::STREAM, LOG_STREAM_KIND), Err(Error::Invalid(_))));
    // Only at the root.
    let top = w.add_scope(None, "top", ScopeType::Module, "").unwrap();
    w.add_stream(Some(top), ending::STREAM, LOG_STREAM_KIND).unwrap();
    w.close_with(Ending::Exited { status: 1 }).unwrap();
    // Closing twice keeps the first ending.
    w.close_with(Ending::Exited { status: 2 }).unwrap();
    let r = Reader::open(tmp("reserved.vtr")).unwrap();
    assert_eq!(r.ending().unwrap(), (Ending::Exited { status: 1 }, Some(0)));
}
