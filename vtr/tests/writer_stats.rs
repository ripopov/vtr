use vtr::{
    Compression, Direction, ExecutionMode, Reader, Recorder, SignalKind, SourceOptions, Status,
    Value, VarType, Writer, WriterOptions,
};

#[test]
fn live_payload_buffer_queue_and_published_block_statistics_match_the_recording() {
    for (mode, workers) in [
        (ExecutionMode::Inline, 0),
        (ExecutionMode::Threaded, 0),
        (ExecutionMode::Threaded, 2),
    ] {
        for compression in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("stats.vtr");
            let mut options = WriterOptions {
                mode,
                ..WriterOptions::default()
            };
            options.buffer.queue_bytes = 256;
            options.buffer.pending_bytes = 256;
            options.encoding.compression = compression;
            options.encoding.workers = workers;
            options.encoding.block_records = 2;
            options.encoding.waveform_records = 2;
            let mut writer = Writer::create_with(&path, options).unwrap();
            let logger = writer
                .logger(SourceOptions::new("diagnostics", None))
                .unwrap();
            let owners = writer
                .transactions::<String>(SourceOptions::new("owners", None), "OWNER")
                .unwrap();
            let stages = writer
                .stages::<String>(SourceOptions::new("stage", None))
                .unwrap();
            let logs = vtr::logging::declare::<String>(
                &mut writer,
                vtr::logging::CallsiteOptions::new("{}"),
            )
            .unwrap();
            let property = writer.intern("note").unwrap();
            let signal = writer
                .signal(
                    SourceOptions::new("counter", None),
                    SignalKind::Bits {
                        width: 8,
                        states: 2,
                    },
                    VarType::Wire,
                    Direction::Implicit,
                )
                .unwrap();
            let owner = writer.begin(owners, &"x".repeat(4096), None).unwrap();
            writer
                .begin_stage(stages, owner.owner(), &"s".repeat(128))
                .unwrap();
            writer
                .attribute(
                    owner.owner(),
                    property,
                    Value::List(vec![Value::Text("a".repeat(512))]),
                )
                .unwrap();
            let live = writer.stats().unwrap();
            assert_eq!((live.live_owners, live.live_stages), (1, 1));
            assert!(live.active_payload_bytes >= 4096 + 128 + 512);
            writer.payload(owner, &"replaced".to_owned()).unwrap();
            let replaced = writer.stats().unwrap();
            assert!(live.active_payload_bytes > replaced.active_payload_bytes + 4000);
            for time in 0..20 {
                writer.advance_time(time).unwrap();
                writer.emit_u64(signal, time).unwrap();
                let transaction = writer
                    .begin(owners, &format!("request {time}"), None)
                    .unwrap();
                writer.end(transaction, Status::Ok).unwrap();
                writer
                    .log(
                        logger,
                        logs,
                        &format!("message {time}"),
                        Some(owner.owner()),
                    )
                    .unwrap();
            }
            writer.advance_time(30).unwrap();
            writer.end(owner, Status::Ok).unwrap();
            writer.flush().unwrap();
            let drained = writer.stats().unwrap();
            assert_eq!(
                (
                    drained.live_owners,
                    drained.live_stages,
                    drained.active_payload_bytes
                ),
                (0, 0, 0)
            );
            assert_eq!(drained.transport.submitted, drained.transport.completed);
            assert_eq!(drained.encoding.submitted, drained.encoding.completed);
            assert!(
                drained.peak_transaction_bytes > 0
                    && drained.peak_record_bytes > 0
                    && drained.peak_waveform_bytes > 0
            );
            if mode == ExecutionMode::Threaded {
                assert!(drained.transport.submitted > 0 && drained.transport.peak_bytes > 0);
            }
            if workers != 0 {
                assert!(drained.encoding.submitted > 0 && drained.encoding.peak_bytes > 0);
            }
            writer.close().unwrap();
            let finished = writer.stats().unwrap();
            assert_eq!(finished.transport.submitted, drained.transport.submitted);
            assert_eq!(finished.encoding.submitted, finished.encoding.completed);
            assert_eq!(
                finished.backend_cpu_nanoseconds,
                writer.backend_cpu_nanoseconds()
            );
            if mode == ExecutionMode::Inline {
                assert_eq!(finished.backend_cpu_nanoseconds, Some(0));
            }
            #[cfg(target_os = "linux")]
            if mode == ExecutionMode::Threaded {
                assert!(finished.backend_cpu_nanoseconds.unwrap() > 0);
            }
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(finished.output_bytes, bytes.len() as u64);
            let container = vtr::framing::Container::from_bytes(bytes, true).unwrap();
            for (kind, count) in [
                (4, finished.waveform_blocks),
                (5, finished.transaction_blocks),
                (9, finished.record_blocks),
            ] {
                assert_eq!(
                    count,
                    container
                        .sections()
                        .iter()
                        .filter(|section| section.kind == kind)
                        .count() as u64
                );
                assert!(count > 0);
            }
            Reader::open(path).unwrap().validate().unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn owned_worker_cpu_sampling_does_not_wait_for_blocked_output() {
    use std::{
        io::Write,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };
    struct Output {
        file: std::fs::File,
        armed: Arc<AtomicBool>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: mpsc::Sender<()>,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.armed.swap(false, Ordering::Relaxed) {
                self.entered.send(()).unwrap();
                let (lock, changed) = &*self.gate;
                drop(
                    changed
                        .wait_while(lock.lock().unwrap(), |released| !*released)
                        .unwrap(),
                );
            }
            self.file.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    for workers in [0, 2] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked.vtr");
        let armed = Arc::new(AtomicBool::new(false));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered, waiting) = mpsc::channel();
        let output = Output {
            file: std::fs::File::create(&path).unwrap(),
            armed: armed.clone(),
            gate: gate.clone(),
            entered,
        };
        let mut options = WriterOptions::default();
        options.encoding.workers = workers;
        options.encoding.block_records = 1;
        let mut writer = Writer::from_writer(Box::new(output), options).unwrap();
        let logger = writer
            .logger(SourceOptions::new("diagnostics", None))
            .unwrap();
        let logs =
            vtr::logging::declare::<String>(&mut writer, vtr::logging::CallsiteOptions::new("{}"))
                .unwrap();
        armed.store(true, Ordering::Relaxed);
        writer
            .log(logger, logs, &"owned worker".repeat(100), None)
            .unwrap();
        waiting.recv_timeout(Duration::from_secs(3)).unwrap();
        let release = gate.clone();
        let watchdog = std::thread::spawn(move || {
            let (lock, changed) = &*release;
            let (mut released, _) = changed
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(2), |released| {
                    !*released
                })
                .unwrap();
            *released = true;
            changed.notify_all();
        });
        let start = Instant::now();
        let mut previous = writer.backend_cpu_nanoseconds().unwrap();
        for _ in 0..100 {
            let current = writer.backend_cpu_nanoseconds().unwrap();
            assert!(current >= previous);
            previous = current;
        }
        let elapsed = start.elapsed();
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        watchdog.join().unwrap();
        assert!(
            elapsed < Duration::from_secs(1),
            "sampling waited for backend output"
        );
        writer.close().unwrap();
        assert!(writer.backend_cpu_nanoseconds().unwrap() >= previous);
        Reader::open(path).unwrap().validate().unwrap();
    }
}

#[test]
fn capture_and_encoding_budgets_charge_nested_attributes_and_unused_capacity() {
    for (mode, workers) in [
        (ExecutionMode::Inline, 0),
        (ExecutionMode::Threaded, 0),
        (ExecutionMode::Threaded, 2),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capacity.vtr");
        let mut options = WriterOptions {
            mode,
            ..WriterOptions::default()
        };
        options.buffer.queue_bytes = 1024;
        options.encoding.queue_bytes = 1024;
        options.encoding.workers = workers;
        let mut writer = Writer::create_with(&path, options).unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("owners", None), "OWNER")
            .unwrap();
        let key = writer.intern("reserved").unwrap();
        let owner = writer.begin(source, &(), None).unwrap();
        let values = Vec::<vtr::Attribute>::with_capacity(4096);
        let retained = values.capacity() * std::mem::size_of::<vtr::Attribute>();
        writer
            .attribute(owner.owner(), key, Value::Map(values))
            .unwrap();
        assert!(writer.stats().unwrap().active_payload_bytes >= retained);
        writer.end(owner, Status::Ok).unwrap();
        writer.close().unwrap();
        let stats = writer.stats().unwrap();
        if mode == ExecutionMode::Threaded {
            assert!(stats.transport.peak_bytes >= retained);
        }
        if workers > 0 {
            assert!(stats.encoding.peak_bytes >= retained);
        }
        assert!(stats.peak_transaction_bytes >= retained);
        let reader = Reader::open(path).unwrap();
        reader.validate().unwrap();
        let record = reader.lookup(owner.id()).unwrap().unwrap();
        assert!(matches!(&record.attributes()[0].value, Value::Map(values) if values.is_empty()));
    }
}

struct ManyUnits;
impl vtr::Payload for ManyUnits {
    fn bind(types: &mut vtr::Types) -> vtr::Result<u32> {
        types.structure(
            "ManyUnits",
            (0..1000)
                .map(|index| vtr::schema::Field::new(format!("f{index}"), 0))
                .collect(),
        )
    }
    fn encode(&self, _: &mut Vec<u8>) -> vtr::Result<()> {
        Ok(())
    }
    fn decode(_: &mut vtr::PayloadReader<'_>) -> vtr::Result<Self> {
        Ok(Self)
    }
}

#[test]
fn encoding_admission_includes_retained_schema_even_for_an_empty_payload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema.vtr");
    let mut options = WriterOptions::default();
    options.encoding.workers = 2;
    options.encoding.queue_bytes = 1024;
    let mut writer = Writer::create_with(&path, options).unwrap();
    let source = writer
        .transactions::<ManyUnits>(SourceOptions::new("many", None), "OWNER")
        .unwrap();
    let owner = writer.begin(source, &ManyUnits, None).unwrap();
    writer.end(owner, Status::Ok).unwrap();
    writer.close().unwrap();
    assert!(
        writer.stats().unwrap().encoding.peak_bytes
            >= 1000 * std::mem::size_of::<vtr::schema::Field>()
    );
    let reader = Reader::open(path).unwrap();
    reader.validate().unwrap();
    assert!(
        reader
            .lookup(owner.id())
            .unwrap()
            .unwrap()
            .payload()
            .bytes()
            .is_empty()
    );
}
