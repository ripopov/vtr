//! Files whose writer never closed them (docs/crash-safe-vtr.html, stage 2):
//! finished sections reach the file at once, and a recovering reader keeps
//! exactly the complete, verified sections before a cut, a torn payload or a
//! zero-filled tail.

use vtr::*;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vtr-recovery-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A closed file with every kind of section, many of each.
fn sample() -> (Vec<u8>, SignalId) {
    let path = tmp("sample.vtr");
    let opts = WriterOptions { block_records: 64, tx_block_bytes: 512, background: false, dedup: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let top = w.add_scope(None, "top", ScopeType::Module, "").unwrap();
    let (_, a) = w.add_var(Some(top), "a", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 16, states: 2 }).unwrap();
    let (_, b) = w.add_var(Some(top), "b", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 4, states: 4 }).unwrap();
    let stream = w.add_stream(Some(top), "bus", "TRANSACTOR").unwrap();
    let gen = w.add_generator(stream, "read").unwrap();
    let log = w.add_stream(None, "log", LOG_STREAM_KIND).unwrap();
    let site = w.add_log_site(&LogSiteSpec::new(log, Severity::Info, "step {}", &[LogArgType::U64])).unwrap();
    let clk = w.add_clock(Some(top), "clk").unwrap();
    let key = w.intern("addr");
    for t in 0..2000u64 {
        w.set_time(t).unwrap();
        w.emit_u64(a, t * 7).unwrap();
        if t % 3 == 0 {
            w.emit_logic_str(b, if t % 2 == 0 { b"10xz" } else { b"0110" }).unwrap();
        }
        if t % 10 == 0 {
            let tx = w.begin_tx(gen, t).unwrap();
            w.tx_attr(tx, key, &Value::U64(t)).unwrap();
            w.end_tx(tx, t + 4, TxStatus::Ok).unwrap();
            w.log(site, t, &[LogArg::U64(t)]).unwrap();
        }
        if t % 500 == 0 {
            w.clock_stop(clk, t).unwrap();
            w.clock_run(clk, t + 1, 2).unwrap();
            // A few extra sections of the other kinds: hierarchy and strings grow mid-run.
            w.add_var(Some(top), &format!("late{t}"), VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 1, states: 2 }).unwrap();
            w.flush().unwrap();
        }
    }
    w.close().unwrap();
    (std::fs::read(&path).unwrap(), a)
}

/// Everything a reader can decode, in a comparable form.
fn contents(rd: &Reader, a: SignalId) -> (Vec<(u64, Option<u64>)>, Vec<u64>, Vec<u64>) {
    let d = rd.load_signal(a).unwrap();
    let values = (0..d.len()).map(|i| (d.times()[i], d.get(i).as_u64())).collect();
    let mut txs: Vec<u64> = rd.transactions(&TxQuery::default()).unwrap().iter().map(|t| t.id).collect();
    txs.sort();
    let mut logs = Vec::new();
    rd.visit_log(&LogQuery::default(), |r| {
        logs.push(r.id);
        true
    })
    .unwrap();
    for c in 0..rd.clocks().len() {
        rd.clock(ClockId(c as u32)).unwrap();
    }
    (values, txs, logs)
}

/// A recovered prefix must be a prefix of the complete file's contents.
fn assert_prefix(cut: &Reader, full: &Reader, a: SignalId) {
    let (cv, ct, cl) = contents(cut, a);
    let (fv, ft, fl) = contents(full, a);
    assert_eq!(cv[..], fv[..cv.len()]);
    assert!(ct.iter().all(|id| ft.contains(id)));
    assert!(cl.iter().all(|id| fl.contains(id)));
}

#[test]
fn cut_at_every_section_boundary() {
    let (full, a) = sample();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    assert_eq!(rd.recovered(), None);
    let sections = rd.sections().to_vec();
    assert!(sections.len() > 40, "{} sections", sections.len());
    let ends = sections.iter().map(|e| e.offset).chain(std::iter::once(sections.last().unwrap().payload_offset() + sections.last().unwrap().len));
    for (k, cut) in ends.enumerate() {
        let r = Reader::from_bytes(full[..cut as usize].to_vec()).unwrap();
        assert_eq!(r.recovered(), Some(0), "cut at section {k}");
        assert_eq!(r.sections(), &sections[..k], "cut at section {k}: the directory is rebuilt exactly");
        if k > 3 {
            assert_prefix(&r, &rd, a);
        }
    }
}

#[test]
fn cut_inside_every_payload() {
    let (full, a) = sample();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    let sections = rd.sections().to_vec();
    for (k, e) in sections.iter().enumerate() {
        for cut in [e.offset + 10, e.payload_offset() + e.len / 2, e.payload_offset() + e.len - 1] {
            if cut >= e.payload_offset() + e.len {
                continue;
            }
            let r = Reader::from_bytes(full[..cut as usize].to_vec()).unwrap();
            assert_eq!(r.recovered(), Some(cut - e.offset), "cut in section {k} at {cut}");
            assert_eq!(r.sections(), &sections[..k]);
            if k > 3 {
                assert_prefix(&r, &rd, a);
            }
        }
    }
}

#[test]
fn zero_filled_tail_is_dropped() {
    // A header that made it to disk with its payload unwritten (the file was
    // extended, the data never landed) reads back as zeros of the right length.
    let (full, a) = sample();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    let sections = rd.sections().to_vec();
    for (k, e) in sections.iter().enumerate().filter(|(_, e)| e.len > 1) {
        let end = (e.payload_offset() + e.len) as usize;
        let mut torn = full[..end].to_vec();
        let half = (e.payload_offset() + e.len / 2) as usize;
        torn[half..end].fill(0);
        if torn[..end] == full[..end] {
            continue; // the payload's tail was zeros already
        }
        let r = Reader::from_bytes(torn).unwrap();
        assert_eq!(r.recovered(), Some(end as u64 - e.offset), "section {k}");
        assert_eq!(r.sections(), &sections[..k]);
        if k > 3 {
            assert_prefix(&r, &rd, a);
        }
        // Zeros past the last complete section read as the end.
        let mut padded = full[..e.offset as usize].to_vec();
        padded.resize(end + 4096, 0);
        let r = Reader::from_bytes(padded).unwrap();
        assert_eq!(r.recovered(), Some(end as u64 + 4096 - e.offset));
        assert_eq!(r.sections(), &sections[..k]);
    }
}

#[test]
fn finished_sections_reach_the_file_at_once() {
    // A writer that is never closed, as when its process dies: what it has
    // written is in the file, not in a buffer of the dead process.
    let path = tmp("unclosed.vtr");
    let opts = WriterOptions { block_records: 1000, background: false, dedup: false, ..Default::default() };
    let mut w = Writer::create_with(&path, opts).unwrap();
    let (_, a) = w.add_var(None, "a", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 8, states: 2 }).unwrap();
    let log = w.add_stream(None, "log", LOG_STREAM_KIND).unwrap();
    let site = w.add_log_site(&LogSiteSpec::new(log, Severity::Info, "step {}", &[LogArgType::U64])).unwrap();
    for t in 0..2500u64 {
        w.set_time(t).unwrap();
        w.emit_u64(a, t).unwrap();
    }
    w.log(site, 7, &[LogArg::U64(7)]).unwrap();
    // Two blocks completed on their own; flush() finishes the third and the log block.
    let on_disk = Reader::from_bytes(std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(on_disk.recovered(), Some(0));
    assert_eq!(on_disk.load_signal(a).unwrap().len(), 2000);
    w.flush().unwrap();
    let on_disk = Reader::from_bytes(std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(on_disk.recovered(), Some(0));
    assert_eq!(on_disk.load_signal(a).unwrap().len(), 2500);
    assert_eq!(on_disk.log_count(), 1);
    w.close().unwrap();
    assert_eq!(Reader::open(&path).unwrap().recovered(), None);
}

/// Every cut file of the tests above, as (label, bytes).
fn cut_files(full: &[u8], sections: &[vtr::container::DirEntry]) -> Vec<(String, Vec<u8>)> {
    let mut cuts = Vec::new();
    let last_end = sections.last().unwrap().payload_offset() + sections.last().unwrap().len;
    for (k, cut) in sections.iter().map(|e| e.offset).chain(std::iter::once(last_end)).enumerate() {
        cuts.push((format!("boundary {k}"), full[..cut as usize].to_vec()));
    }
    for (k, e) in sections.iter().enumerate().filter(|(_, e)| e.len > 1) {
        cuts.push((format!("inside {k}"), full[..(e.payload_offset() + e.len / 2) as usize].to_vec()));
        let end = (e.payload_offset() + e.len) as usize;
        let mut torn = full[..end].to_vec();
        torn[(e.payload_offset() + e.len / 2) as usize..end].fill(0);
        cuts.push((format!("zero-filled {k}"), torn));
    }
    cuts
}

#[test]
fn recover_every_cut_file() {
    let (full, a) = sample();
    let rd = Reader::from_bytes(full.clone()).unwrap();
    let sections = rd.sections().to_vec();
    let (input, output) = (tmp("cut-in.vtr"), tmp("cut-out.vtr"));
    for (label, bytes) in cut_files(&full, &sections) {
        std::fs::write(&input, &bytes).unwrap();
        let cut = Reader::from_bytes(bytes).unwrap();
        let dropped = cut.recovered().unwrap();
        assert_eq!(vtr::recover(&input, &output).unwrap(), Some(dropped), "{label}");
        let r = Reader::open(&output).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(r.recovered(), None, "{label}: the rewritten file is complete");
        assert_eq!(r.ending().unwrap().0, Ending::Recovered { dropped }, "{label}");
        // Every verified section is kept unchanged, then the ending's strings, nodes and record.
        let kept: Vec<_> = r.sections()[..cut.sections().len()].iter().map(|e| (e.kind, e.offset, e.len, e.aux0, e.aux1)).collect();
        let want: Vec<_> = cut.sections().iter().map(|e| (e.kind, e.offset, e.len, e.aux0, e.aux1)).collect();
        assert_eq!(kept, want, "{label}");
        if cut.signal_count() > 0 {
            let (cv, ct, mut cl) = contents(&cut, a);
            let (rv, mut rt, rl) = contents(&r, a);
            // Log records are also zero-duration transactions; the ending record is the new one.
            let end = *rl.last().unwrap();
            rt.retain(|&id| id != end);
            assert_eq!((rv, rt), (cv, ct), "{label}");
            cl.push(end);
            assert_eq!(rl, cl, "{label}: the same log records and the ending record");
        }
    }
    // A complete file is copied as it is.
    std::fs::write(&input, &full).unwrap();
    assert_eq!(vtr::recover(&input, &output).unwrap(), None);
    assert_eq!(std::fs::read(&output).unwrap(), full);
}

#[test]
fn commit_interval_bounds_what_waits_in_memory() {
    use std::time::{Duration, Instant};
    // A slow run (a time step every 200 us) that is never closed, as when it is killed:
    // with a commit interval the file keeps everything older than the interval.
    for interval in [Duration::ZERO, Duration::from_millis(100)] {
        let path = tmp(&format!("commit{}.vtr", interval.as_millis()));
        let opts = WriterOptions { commit_interval: interval, ..Default::default() };
        let mut w = Writer::create_with(&path, opts).unwrap();
        let (_, a) = w.add_var(None, "a", VarType::Wire, Direction::Implicit, SignalKind::Bits { width: 32, states: 2 }).unwrap();
        let log = w.add_stream(None, "log", LOG_STREAM_KIND).unwrap();
        let site = w.add_log_site(&LogSiteSpec::new(log, Severity::Info, "step {}", &[LogArgType::U64])).unwrap();
        let start = Instant::now();
        let mut progress = Vec::new();
        let mut t = 0u64;
        while start.elapsed() < Duration::from_millis(1500) {
            t += 1;
            w.set_time(t).unwrap();
            w.emit_u64(a, t).unwrap();
            if t % 10 == 0 {
                w.log(site, t, &[LogArg::U64(t)]).unwrap();
            }
            progress.push((start.elapsed(), t));
            std::thread::sleep(Duration::from_micros(200));
        }
        // What had been emitted half a second before the end must be in the file.
        let settled = progress.iter().rev().find(|(at, _)| *at + Duration::from_millis(500) + interval <= start.elapsed()).unwrap().1;
        std::thread::sleep(Duration::from_millis(100));
        let on_disk = Reader::from_bytes(std::fs::read(&path).unwrap()).unwrap();
        let kept = if on_disk.signal_count() > 0 { on_disk.load_signal(a).unwrap().len() as u64 } else { 0 };
        if interval.is_zero() {
            assert_eq!((kept, on_disk.log_count()), (0, 0), "without an interval everything waits for the block");
        } else {
            assert!(kept >= settled, "kept {kept} changes of the {settled} emitted {} ms before the end", 500 + interval.as_millis());
            assert!(on_disk.log_count() >= settled / 10, "kept {} log records", on_disk.log_count());
        }
        drop(w);
    }
}
