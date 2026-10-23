use std::panic::{AssertUnwindSafe, catch_unwind};
use vtr::{
    Compression, Direction, SignalKind, VarType,
    framing::Completion,
    recording::{Ending, ExecutionMode, Recorder, SourceOptions, Status, Writer, WriterOptions},
    records::{RecordKind, RecordQuery},
    schema::{Payload, PayloadReader, Types},
    trace::{Reader, TimeWindow},
};

const CRASH: Ending = Ending::Crashed {
    signal: 11,
    code: 1,
    address: 0xdead_beef,
    thread: 7,
    sealed: true,
};

fn options(mode: ExecutionMode) -> WriterOptions {
    let mut options = WriterOptions {
        mode,
        ..WriterOptions::default()
    };
    options.buffer.queue_bytes = 256;
    options.buffer.pending_bytes = 256;
    options.encoding.waveform_records = 8;
    options.encoding.block_records = 8;
    options.encoding.block_bytes = 256;
    options
}

#[test]
fn independent_sealing_drains_accepted_captures_with_live_producers() {
    for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
        for workers in [0, 2] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("seal.vtr");
            let mut opts = options(ExecutionMode::Threaded);
            opts.encoding.compression = compression;
            opts.encoding.workers = workers;
            let mut writer = Writer::create_with(&path, opts).unwrap();
            let source = writer
                .transactions::<u64>(SourceOptions::new("requests", None), "REQUEST")
                .unwrap();
            let logs = writer
                .logs::<String>(SourceOptions::new("message {}", None))
                .unwrap();
            let clock = writer.clock(SourceOptions::new("clock", None)).unwrap();
            let signal = writer
                .signal(
                    SourceOptions::new("counter", None),
                    SignalKind::Bits {
                        width: 16,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Output,
                )
                .unwrap();
            let mut producer = writer.add_producer().unwrap();
            let mut stalled = writer.add_producer().unwrap();
            let sealer = writer.sealer();
            let mut expected = Vec::new();
            // The stalled producer keeps the frontier at zero and forces spill recovery.
            for time in 1..=300 {
                producer.advance_time(time).unwrap();
                let owner = producer.begin(source, &time, None).unwrap();
                expected.push((owner.id(), time));
                if time % 3 != 0 {
                    producer.end(owner, Status::Ok).unwrap();
                }
                writer.advance_time(time).unwrap();
                writer.emit_u64(signal, time).unwrap();
                writer.log(logs, &format!("step {time}"), None).unwrap();
                if time == 1 {
                    writer.clock_run(clock, 0, 2).unwrap();
                }
                if time == 200 {
                    writer.scope("late", None).unwrap();
                }
            }
            writer.guarded(|_| {
                assert!(sealer.state().busy());
                std::thread::spawn(move || sealer.seal(CRASH))
                    .join()
                    .unwrap()
                    .unwrap();
            });
            assert!(writer.advance_time(301).is_err());
            assert!(producer.begin(source, &301, None).is_err());
            assert!(producer.intern("after sealing").is_err());
            assert!(stalled.advance_time(1).is_err());
            assert!(writer.add_producer().is_err());
            let reader = Reader::open(&path).unwrap();
            assert_eq!(reader.completion(), Completion::Incomplete);
            assert_eq!(reader.ending().unwrap(), (CRASH, Some(300)));
            reader.validate().unwrap();
            for (id, payload) in expected {
                let owner = reader.lookup(id).unwrap().unwrap();
                assert_eq!(owner.payload().decode::<u64>().unwrap(), payload);
                assert_eq!(
                    owner.status(),
                    if payload % 3 == 0 {
                        Status::Open
                    } else {
                        Status::Ok
                    }
                );
            }
            let mut values = Vec::new();
            reader
                .changes(Some(signal), TimeWindow::default(), |time, _, value| {
                    values.push((time, value.as_u64().unwrap()));
                    true
                })
                .unwrap();
            assert_eq!(
                values,
                (1..=300).map(|time| (time, time)).collect::<Vec<_>>()
            );
            let mut count = 0;
            reader
                .records(
                    RecordQuery {
                        kind: Some(RecordKind::Log),
                        ..RecordQuery::default()
                    },
                    |_| {
                        count += 1;
                        true
                    },
                )
                .unwrap();
            assert_eq!(count, 301);
            assert!(reader.find("late").is_some());
        }
    }
}

struct PanicPayload;
impl Payload for PanicPayload {
    fn bind(_: &mut Types) -> vtr::Result<u32> {
        Ok(0)
    }
    fn encode(&self, _: &mut Vec<u8>) -> vtr::Result<()> {
        panic!("payload capture interrupted")
    }
    fn decode(_: &mut PayloadReader<'_>) -> vtr::Result<Self> {
        Ok(Self)
    }
}

struct PanicSchema;
impl Payload for PanicSchema {
    fn bind(_: &mut Types) -> vtr::Result<u32> {
        panic!("schema binding interrupted")
    }
    fn encode(&self, _: &mut Vec<u8>) -> vtr::Result<()> {
        Ok(())
    }
    fn decode(_: &mut PayloadReader<'_>) -> vtr::Result<Self> {
        Ok(Self)
    }
}

#[test]
fn schema_panics_never_finalize_a_partly_mutated_backend() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("schema-panic.vtr");
        let mut writer = Writer::create_with(&path, options(mode)).unwrap();
        let logs = writer
            .logs::<u64>(SourceOptions::new("good {}", None))
            .unwrap();
        let id = writer.log(logs, &42, None).unwrap();
        writer.flush().unwrap();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            writer.transactions::<PanicSchema>(SourceOptions::new("bad", None), "REQUEST")
        }));
        match mode {
            ExecutionMode::Inline => {
                assert!(outcome.is_err());
                assert!(writer.is_poisoned());
            }
            ExecutionMode::Threaded => {
                assert!(outcome.unwrap().is_err());
            }
        }
        assert!(writer.intern("after panic").is_err());
        assert!(writer.close().is_err());
        drop(writer);
        let reader = Reader::open(path).unwrap();
        assert_ne!(reader.completion(), Completion::Complete);
        assert_eq!(
            reader
                .lookup(id)
                .unwrap()
                .unwrap()
                .payload()
                .decode::<u64>()
                .unwrap(),
            42
        );
        reader.validate().unwrap();
    }
}

#[test]
fn payload_panics_poison_the_entire_frontend_call_and_preserve_owned_records() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panic.vtr");
        let mut writer = Writer::create_with(&path, options(mode)).unwrap();
        let good = writer
            .logs::<u64>(SourceOptions::new("good {}", None))
            .unwrap();
        let bad = writer
            .logs::<PanicPayload>(SourceOptions::new("bad", None))
            .unwrap();
        let id = writer.log(good, &42, None).unwrap();
        writer.flush().unwrap();
        assert!(catch_unwind(AssertUnwindSafe(|| writer.log(bad, &PanicPayload, None))).is_err());
        assert!(writer.is_poisoned());
        assert!(writer.log(good, &43, None).is_err());
        let result = writer.close();
        assert_eq!(result.is_ok(), mode == ExecutionMode::Threaded);
        drop(writer);
        let reader = Reader::open(path).unwrap();
        assert_ne!(reader.completion(), Completion::Complete);
        assert_eq!(
            reader
                .lookup(id)
                .unwrap()
                .unwrap()
                .payload()
                .decode::<u64>()
                .unwrap(),
            42
        );
        if mode == ExecutionMode::Threaded {
            assert_eq!(reader.ending().unwrap(), (Ending::Poisoned, Some(0)));
        }
        reader.validate().unwrap();
    }
}

#[test]
fn abnormal_endings_are_typed_and_sealed_in_both_modes() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        for ending in [
            Ending::Closed,
            Ending::Stopped { signal: 15 },
            Ending::Exited { status: -7 },
            CRASH,
            Ending::Recovered { dropped: u64::MAX },
            Ending::Poisoned,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("ending.vtr");
            let mut writer = Writer::create_with(&path, options(mode)).unwrap();
            assert!(writer.scope("vtr.run", None).is_err());
            writer.advance_time(5).unwrap();
            writer.close_with(ending).unwrap();
            assert!(writer.close_with(ending).is_err());
            let reader = Reader::open(path).unwrap();
            assert_eq!(
                reader.ending().unwrap(),
                (ending, (ending != Ending::Closed).then_some(5))
            );
            assert_eq!(
                reader.completion() == Completion::Complete,
                ending == Ending::Closed
            );
            reader
                .records(RecordQuery::default(), |record| {
                    assert_eq!(reader.format_log(&record).unwrap(), format!("run {ending}"));
                    true
                })
                .unwrap();
            reader.validate().unwrap();
        }
    }
}

#[test]
fn rescue_wakes_capture_waiting_for_queue_space_before_output_resumes() {
    use std::{
        io::Write,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };
    struct Output {
        file: std::fs::File,
        armed: Arc<AtomicBool>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: mpsc::Sender<()>,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.armed.swap(false, Ordering::AcqRel) {
                self.entered.send(()).unwrap();
                let (lock, changed) = &*self.gate;
                let (released, _) = changed
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(3), |released| {
                        !*released
                    })
                    .unwrap();
                if !*released {
                    return Err(std::io::Error::other("test output gate timed out"));
                }
            }
            self.file.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("waiting.vtr");
    let armed = Arc::new(AtomicBool::new(false));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered, output_blocked) = mpsc::channel();
    let output = Output {
        file: std::fs::File::create(&path).unwrap(),
        armed: armed.clone(),
        gate: gate.clone(),
        entered,
    };
    let mut writer =
        Writer::from_writer(Box::new(output), options(ExecutionMode::Threaded)).unwrap();
    let source = writer
        .logs::<String>(SourceOptions::new("message {}", None))
        .unwrap();
    writer.flush().unwrap();
    let mut producer = writer.add_producer().unwrap();
    writer.finish_producer().unwrap();
    let sealer = writer.sealer();
    armed.store(true, Ordering::Release);
    let (done, capture_stopped) = mpsc::channel();
    let capture = std::thread::spawn(move || {
        let payload = "x".repeat(4096);
        let mut accepted = 0;
        loop {
            if producer.log(source, &payload, None).is_err() {
                break;
            }
            accepted += 1;
        }
        done.send(accepted).unwrap();
    });
    output_blocked.recv_timeout(Duration::from_secs(2)).unwrap();
    let rescue = std::thread::spawn(move || sealer.seal(CRASH));
    let accepted = capture_stopped
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert!(accepted > 0);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    rescue.join().unwrap().unwrap();
    capture.join().unwrap();
    let reader = Reader::open(path).unwrap();
    let mut count = 0;
    reader
        .records(
            RecordQuery {
                generator: Some(source.id()),
                ..RecordQuery::default()
            },
            |_| {
                count += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(count, accepted);
    reader.validate().unwrap();
}

#[test]
fn output_panics_wake_producers_and_never_finalize_a_damaged_backend() {
    use std::{
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    struct Output {
        file: std::fs::File,
        fail: Arc<AtomicBool>,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            assert!(!self.fail.load(Ordering::Acquire), "output interrupted");
            self.file.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("panic-output.vtr");
    let fail = Arc::new(AtomicBool::new(false));
    let output = Output {
        file: std::fs::File::create(&path).unwrap(),
        fail: fail.clone(),
    };
    let mut options = options(ExecutionMode::Threaded);
    options.encoding.workers = 2;
    let mut writer = Writer::from_writer(Box::new(output), options).unwrap();
    let source = writer
        .logs::<String>(SourceOptions::new("{}", None))
        .unwrap();
    let good = writer.log(source, &"published".into(), None).unwrap();
    writer.flush().unwrap();
    fail.store(true, Ordering::Release);
    for _ in 0..50 {
        if writer.log(source, &"x".repeat(4096), None).is_err() {
            break;
        }
    }
    assert!(writer.flush().is_err());
    assert!(writer.seal().is_err());
    drop(writer);
    let reader = Reader::open(path).unwrap();
    assert_eq!(reader.completion(), Completion::Unfinished);
    assert_eq!(
        reader
            .lookup(good)
            .unwrap()
            .unwrap()
            .payload()
            .decode::<String>()
            .unwrap(),
        "published"
    );
    reader.validate().unwrap();
}
