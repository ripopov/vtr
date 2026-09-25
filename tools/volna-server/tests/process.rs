use bincode::Options;
use serde::de::DeserializeOwned;
use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command as Process, Stdio};
use volna_core::data::WaveValue;
use volna_core::data::transactions::TrackRef;
use volna_core::remote::history::PackedHistory;
use volna_core::remote::objects::Metadata;
use volna_core::remote::transport::*;
use volna_core::remote::{ClientStep, signals::SignalTransfer};
use volna_core::remote::{client::RemoteClient, memory::MemoryBudget};
use volna_core::session::LoadResult;
use volna_core::session::OpenSpec;

struct Server {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    session: u64,
    request: u64,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn drive(&mut self, client: &mut RemoteClient) -> LoadResult {
        let command = client.take_command().unwrap().expect("queued command");
        self.request = command.request;
        write_packet(&mut self.input, &command).unwrap();
        loop {
            let packet = read_packet(&mut self.output)
                .unwrap()
                .expect("client response");
            self.session = packet.session;
            let mut step = client.accept(packet).unwrap();
            loop {
                match step {
                    ClientStep::Yield => {
                        std::thread::yield_now();
                        step = client.step().unwrap();
                    }
                    ClientStep::Ack(ack) => {
                        write_packet(&mut self.input, &ack).unwrap();
                        break;
                    }
                    ClientStep::Complete { ack, result } => {
                        write_packet(&mut self.input, &ack).unwrap();
                        return result;
                    }
                }
            }
        }
    }
    fn start(path: &Path) -> Self {
        let mut child = Process::new(env!("CARGO_BIN_EXE_volna-server"))
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: child.stdout.take().unwrap(),
            child,
            session: 0,
            request: 0,
        }
    }
    fn command(&mut self, command: Command) {
        self.request += 1;
        write_packet(
            &mut self.input,
            &Packet {
                session: self.session,
                request: self.request,
                sequence: 0,
                body: Body::Command(command),
            },
        )
        .unwrap();
    }
    fn receive<T: DeserializeOwned>(&mut self, objects: Vec<ObjectId>) -> Vec<Result<T, String>> {
        let first = read_packet(&mut self.output).unwrap().expect("response");
        if self.session == 0 {
            self.session = first.session;
        }
        let mut receiver =
            Receiver::new(self.session, self.request, objects, 64 * 1024 * 1024).unwrap();
        let mut packet = first;
        let mut buffer = vec![];
        let mut result = vec![];
        loop {
            let ack = acknowledgement(&packet);
            match receiver.accept(packet).unwrap() {
                Receive::Begin { .. } => buffer.clear(),
                Receive::Data(bytes) => buffer.extend(bytes),
                Receive::Complete(_) => result.push(Ok(bincode::DefaultOptions::new()
                    .with_fixint_encoding()
                    .with_little_endian()
                    .reject_trailing_bytes()
                    .with_limit(buffer.len() as u64)
                    .deserialize(&buffer)
                    .unwrap())),
                Receive::Failed { message, .. } => result.push(Err(message)),
            }
            write_packet(&mut self.input, &ack).unwrap();
            if receiver.is_complete() {
                break;
            }
            packet = read_packet(&mut self.output)
                .unwrap()
                .expect("next response");
        }
        receiver.finish().unwrap();
        result
    }
    fn open(&mut self, limit: u64) -> Result<Metadata, String> {
        use volna_core::remote::{memory::MemoryBudget, open::OpenTransfer};
        self.request += 1;
        let mut transfer = OpenTransfer::new(
            self.request,
            19,
            limit,
            MemoryBudget::new(256 * 1024 * 1024),
        )
        .unwrap();
        write_packet(&mut self.input, &transfer.command()).unwrap();
        loop {
            let packet = read_packet(&mut self.output)
                .unwrap()
                .expect("Open response");
            let mut step = transfer.accept(packet).unwrap();
            loop {
                match step {
                    ClientStep::Ack(ack) => {
                        write_packet(&mut self.input, &ack).unwrap();
                        break;
                    }
                    ClientStep::Yield => {
                        std::thread::yield_now();
                        step = transfer.step().unwrap();
                    }
                    ClientStep::Complete { ack, result } => {
                        write_packet(&mut self.input, &ack).unwrap();
                        transfer.finish().unwrap();
                        let volna_core::session::LoadResult::Opened { generation, result } = result
                        else {
                            panic!("Open completion");
                        };
                        assert_eq!(generation, 19);
                        self.session = ack.session;
                        return result
                            .map(|session| {
                                assert_eq!(session.remote_id(), Some(self.session));
                                Metadata::from_session(session.as_ref())
                            })
                            .map_err(|error| format!("{error:#}"));
                    }
                }
            }
        }
    }
    fn histories(
        &mut self,
        signals: &[volna_core::data::SignalRef],
    ) -> volna_core::session::SignalLoads {
        let limit = 64 * 1024 * 1024;
        let mut receiver = SignalTransfer::new(
            self.session,
            self.request,
            73,
            signals,
            limit,
            volna_core::remote::memory::MemoryBudget::new(256 * 1024 * 1024),
        )
        .unwrap();
        let mut results = Vec::new();
        while !receiver.is_complete() {
            let packet = read_packet(&mut self.output)
                .unwrap()
                .expect("history response");
            let mut step = receiver.accept(packet).unwrap();
            let ack = loop {
                match step {
                    ClientStep::Ack(ack) => break ack,
                    ClientStep::Complete { ack, result } => {
                        let volna_core::session::LoadResult::Signals {
                            generation,
                            results: loaded,
                        } = result
                        else {
                            panic!("expected signal completion");
                        };
                        assert_eq!(generation, 73);
                        results.extend(loaded);
                        break ack;
                    }
                    ClientStep::Yield => {
                        std::thread::yield_now();
                        step = receiver.step().unwrap();
                    }
                }
            };
            write_packet(&mut self.input, &ack).unwrap();
        }
        receiver.finish().unwrap();
        results
    }
    fn close(&mut self) {
        self.command(Command::Close);
        assert!(self.child.wait().unwrap().success());
    }
}

fn same_value(a: WaveValue, b: WaveValue) {
    match (a, b) {
        (WaveValue::Real(a), WaveValue::Real(b)) => assert_eq!(a.to_bits(), b.to_bits()),
        (a, b) => assert_eq!(a, b),
    }
}

#[test]
fn vtr_and_fst_process_histories_match_local_values_and_aliases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in [
        "volna/volna/examples/picorv32.vtr",
        "ext/surfer/examples/verilator/features.fst",
    ] {
        let path = root.join(file);
        let local = OpenSpec::Path(path.clone()).open().unwrap();
        let mut server = Server::start(&path);
        let metadata = server.open(64 * 1024 * 1024).unwrap();
        metadata.validate().unwrap();
        assert_eq!(metadata.hierarchy.vars.len(), local.hierarchy().vars.len());
        assert_eq!(metadata.info.time_range, local.info().time_range);
        let mut ids = vec![];
        for variable in local.hierarchy().vars.iter().take(32) {
            if !ids.contains(&variable.signal.0) {
                ids.push(variable.signal.0);
            }
        }
        let mut request = ids.clone();
        request.push(ids[0]);
        server.command(Command::Signals(request));
        let histories = server.histories(
            &ids.iter()
                .copied()
                .map(volna_core::data::SignalRef)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            histories.len(),
            ids.len(),
            "duplicate IDs only transfer once"
        );
        for (id, (returned, result)) in ids.into_iter().zip(histories) {
            assert_eq!(returned.0, id);
            let remote = result.unwrap();
            let local = local.load_signal(volna_core::data::SignalRef(id)).unwrap();
            assert_eq!(remote.shape(), local.shape());
            assert_eq!(remote.len(), local.len());
            same_value(remote.value(None), local.value(None));
            for i in 0..local.len() {
                assert_eq!(remote.time(i), local.time(i));
                same_value(remote.value(Some(i)), local.value(Some(i)));
            }
        }
        server.close();
    }
}

#[test]
fn process_preserves_full_tracks_and_parallel_relations() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    let stream = w.add_stream(None, "stream", "transaction").unwrap();
    let a = w.add_generator(stream, "a").unwrap();
    let b = w.add_generator(stream, "b").unwrap();
    w.add_generator(stream, "empty").unwrap();
    let x = w.begin_tx(a, 1).unwrap();
    w.end_tx(x, 100, vtr::TxStatus::Ok).unwrap();
    let y = w.begin_tx(b, 2).unwrap();
    w.set_tx_parent(y, x).unwrap();
    let key = w.intern("bytes");
    w.tx_attr(
        y,
        key,
        &vtr::Value::Bytes(vec![0, 255]),
    )
    .unwrap();
    w.end_tx(y, 2, vtr::TxStatus::Aborted).unwrap();
    let kind = w.intern("cause");
    for _ in 0..2 {
        w.relate(kind, x, y, &[(key, vtr::Value::F64(f64::NAN))])
            .unwrap();
    }
    w.close().unwrap();
    let mut server = Server::start(file.path());
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut client = RemoteClient::new(10, 32 * 1024 * 1024, budget.clone()).unwrap();
    let LoadResult::Opened {
        generation: 10,
        result,
    } = server.drive(&mut client)
    else {
        panic!("Open result");
    };
    let session = result.unwrap();
    assert!(
        client
            .submit(volna_core::session::LoadRequest::Track {
                session: session.clone(),
                generation: 11,
                request_id: 71,
                track: TrackRef(stream.0)
            })
            .is_ok()
    );
    let LoadResult::Track {
        generation: 11,
        request_id: 71,
        track,
        result,
    } = server.drive(&mut client)
    else {
        panic!("track result");
    };
    assert_eq!(track, TrackRef(stream.0));
    let loaded = result.unwrap();
    let local = OpenSpec::Path(file.path().into())
        .open()
        .unwrap()
        .load_track(volna_core::data::transactions::TrackRef(stream.0))
        .unwrap();
    assert_eq!(loaded.generators.len(), 3);
    for (a, b) in loaded.generators.iter().zip(&local.generators) {
        assert_eq!(a.transactions(), b.transactions());
        assert_eq!(
            bincode::serialize(a.relations()).unwrap(),
            bincode::serialize(b.relations()).unwrap()
        );
        for tx in a.transactions() {
            assert_eq!(a.parent(tx.id), b.parent(tx.id));
        }
    }
    assert!(
        client
            .submit(volna_core::session::LoadRequest::Track {
                session: session.clone(),
                generation: 11,
                request_id: 72,
                track: TrackRef(u32::MAX)
            })
            .is_ok()
    );
    assert!(
        client
            .submit(volna_core::session::LoadRequest::Track {
                session: session.clone(),
                generation: 11,
                request_id: 73,
                track: TrackRef(a.0)
            })
            .is_ok()
    );
    let LoadResult::Track {
        request_id: 72,
        result,
        ..
    } = server.drive(&mut client)
    else {
        panic!("failed track");
    };
    assert!(result.is_err());
    let LoadResult::Track {
        request_id: 73,
        result,
        ..
    } = server.drive(&mut client)
    else {
        panic!("next track");
    };
    assert_eq!(result.unwrap().generators[0].transactions().len(), 1);
    assert!(
        client
            .submit(volna_core::session::LoadRequest::Track {
                session: session.clone(),
                generation: 11,
                request_id: 74,
                track: TrackRef(a.0)
            })
            .is_ok()
    );
    assert!(
        client
            .submit(volna_core::session::LoadRequest::Track {
                session: session.clone(),
                generation: 11,
                request_id: 75,
                track: TrackRef(b.0)
            })
            .is_ok()
    );
    assert!(client.take_command().unwrap().is_some());
    let failed = client.disconnect("test disconnect");
    assert_eq!(failed.len(), 2);
    for (result, expected) in failed.into_iter().zip([74, 75]) {
        let LoadResult::Track {
            generation: 11,
            request_id,
            result,
            ..
        } = result
        else {
            panic!("disconnect failure");
        };
        assert_eq!(request_id, expected);
        assert!(result.is_err());
    }
    drop(client);
    drop(session);
    assert_eq!(loaded.generators[0].transactions().len(), 1);
    assert!(budget.used() > 0);
    drop(loaded);
    assert_eq!(budget.used(), 0);
    server.close();
}

#[test]
fn metadata_limit_and_per_signal_failure_are_explicit() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../volna/volna/examples/picorv32.vtr");
    let mut server = Server::start(&path);
    assert!(server.open(1).unwrap_err().contains("limit"));
    server.close();
    let mut server = Server::start(&path);
    let meta = server.open(64 * 1024 * 1024).unwrap();
    let id = meta.hierarchy.vars[0].signal.0;
    server.command(Command::Signals(vec![id, u32::MAX]));
    let replies =
        server.receive::<PackedHistory>(vec![ObjectId::Signal(id), ObjectId::Signal(u32::MAX)]);
    assert!(replies[0].is_ok());
    assert!(replies[1].is_err());
    server.close();
}

#[test]
fn changed_recording_invalidates_process() {
    let file = tempfile::NamedTempFile::new().unwrap();
    vtr::Writer::create(file.path()).unwrap().close().unwrap();
    let mut server = Server::start(file.path());
    server.open(64 * 1024 * 1024).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(file.path())
        .unwrap()
        .write_all(&[0])
        .unwrap();
    server.command(Command::Track(0));
    assert!(read_packet(&mut server.output).unwrap().is_none());
    assert!(!server.child.wait().unwrap().success());
}
