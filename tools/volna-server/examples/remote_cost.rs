//! Native client of the actual child protocol, retaining all completed objects.
//! CPU step timings exclude pipe waits; they are not browser frame measurements.
use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;
use volna_core::data::transactions::{TrackKind, TrackRef};
use volna_core::remote::ClientStep;
use volna_core::remote::client::RemoteClient;
use volna_core::remote::limits::Limits;
use volna_core::remote::memory::MemoryBudget;
use volna_core::remote::transport::{self, MAX_FRAME_BYTES, Packet};
use volna_core::session::LoadResult;

struct Measurement {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    client: RemoteClient,
    budget: MemoryBudget,
    start: Instant,
    received: u64,
    sent: u64,
    frames: usize,
    steps: usize,
    work_ms: f64,
    max_work_ms: f64,
    budget_peak: u64,
    first_ready_ms: Option<f64>,
}

impl Drop for Measurement {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn peak_rss(pid: &str) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find(|line| line.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

impl Measurement {
    fn send(&mut self, packet: Packet) -> anyhow::Result<()> {
        let frame = transport::encode(&packet)?;
        self.sent += frame.len() as u64;
        self.input.write_all(&frame)?;
        Ok(())
    }

    fn record_work(&mut self, begin: Instant) {
        let elapsed = begin.elapsed().as_secs_f64() * 1000.0;
        self.work_ms += elapsed;
        self.max_work_ms = self.max_work_ms.max(elapsed);
        self.steps += 1;
        self.budget_peak = self.budget_peak.max(self.budget.used());
    }

    fn drive(&mut self, count: usize) -> anyhow::Result<Vec<LoadResult>> {
        let mut completed = Vec::new();
        while completed.len() < count {
            if let Some(command) = self.client.take_command()? {
                self.send(command)?;
            }
            // Read the bounded frame first, so waiting for the child does not
            // masquerade as a synchronous decode/install pause.
            let mut header = [0; 12];
            self.output.read_exact(&mut header)?;
            let length = u32::from_le_bytes(header[8..12].try_into()?) as usize;
            anyhow::ensure!(length <= MAX_FRAME_BYTES, "oversize frame");
            let mut frame = Vec::with_capacity(12 + length);
            frame.extend_from_slice(&header);
            frame.resize(12 + length, 0);
            self.output.read_exact(&mut frame[12..])?;
            self.received += frame.len() as u64;
            self.frames += 1;
            let begin = Instant::now();
            let packet = transport::decode(&frame)?;
            let step = self.client.accept(packet);
            self.record_work(begin);
            let mut step = step?;
            loop {
                match step {
                    ClientStep::Yield => {
                        let begin = Instant::now();
                        let next = self.client.step();
                        self.record_work(begin);
                        step = next?;
                    }
                    ClientStep::Ack(ack) => {
                        self.send(ack)?;
                        break;
                    }
                    ClientStep::Complete { ack, result } => {
                        let ready = match &result {
                            LoadResult::Signals { results, .. } => {
                                results.iter().any(|(_, r)| r.is_ok())
                            }
                            LoadResult::Track { result, .. } => result.is_ok(),
                            LoadResult::Opened { .. } => false,
                        };
                        if ready && self.first_ready_ms.is_none() {
                            self.first_ready_ms = Some(self.start.elapsed().as_secs_f64() * 1000.0);
                        }
                        completed.push(result);
                        self.send(ack)?;
                        break;
                    }
                }
            }
        }
        Ok(completed)
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let executable = args
        .next()
        .expect("remote_cost SERVER TRACE signals:N|tracks|track:ID [MEMORY_MIB OBJECT_MIB]");
    let path = args.next().expect("TRACE");
    let selection = args.next().expect("selection");
    let limits = Limits {
        memory_mib: args.next().map(|s| s.parse()).transpose()?.unwrap_or(512),
        object_mib: args.next().map(|s| s.parse()).transpose()?.unwrap_or(256),
    };
    let (memory, object) = limits.bytes()?;
    let budget = MemoryBudget::new(memory);
    let start = Instant::now();
    let mut child = Command::new(executable)
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut run = Measurement {
        input: child.stdin.take().unwrap(),
        output: child.stdout.take().unwrap(),
        child,
        client: RemoteClient::new(1, object, budget.clone())?,
        budget,
        start,
        received: 0,
        sent: 0,
        frames: 0,
        steps: 0,
        work_ms: 0.0,
        max_work_ms: 0.0,
        budget_peak: 0,
        first_ready_ms: None,
    };
    let mut opened = match run.drive(1) {
        Ok(results) => results,
        Err(error) => run.client.disconnect(&format!("{error:#}")),
    };
    let LoadResult::Opened { result, .. } = opened.pop().unwrap() else {
        anyhow::bail!("expected metadata");
    };
    let open_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut errors = Vec::new();
    let mut loaded = Vec::new();
    let mut catalog = Vec::new();
    let mut signals = 0;
    let mut changes = 0;
    let mut transactions = 0;
    let mut relations = 0;
    match &result {
        Err(error) => errors.push(format!("open: {error:#}")),
        Ok(session) => {
            catalog = session
                .tracks()
                .iter()
                .map(|t| serde_json::json!({"id": t.id.0, "path": t.path}))
                .collect();
            let count = if let Some(count) = selection.strip_prefix("signals:") {
                let mut seen = HashSet::new();
                let ids: Vec<_> = session
                    .hierarchy()
                    .vars
                    .iter()
                    .map(|v| v.signal)
                    .filter(|id| seen.insert(*id))
                    .take(count.parse()?)
                    .collect();
                let count = ids.len();
                anyhow::ensure!(
                    run.client
                        .submit(volna_core::session::LoadRequest::Signals {
                            session: session.clone(),
                            generation: 1,
                            signals: ids,
                        })
                        .is_ok(),
                    "signal submission rejected"
                );
                count
            } else {
                let ids: Vec<_> = if let Some(id) = selection.strip_prefix("track:") {
                    vec![TrackRef(id.parse()?)]
                } else {
                    anyhow::ensure!(selection == "tracks", "unknown selection");
                    session
                        .tracks()
                        .iter()
                        .filter(|t| matches!(t.kind, TrackKind::Stream { .. }))
                        .map(|t| t.id)
                        .collect()
                };
                for (index, &track) in ids.iter().enumerate() {
                    anyhow::ensure!(
                        run.client
                            .submit(volna_core::session::LoadRequest::Track {
                                session: session.clone(),
                                generation: 1,
                                request_id: index as u64 + 1,
                                track,
                            })
                            .is_ok(),
                        "track submission rejected"
                    );
                }
                ids.len()
            };
            loaded = run.drive(count)?;
            for item in &loaded {
                match item {
                    LoadResult::Signals { results, .. } => {
                        for (id, r) in results {
                            match r {
                                Ok(history) => {
                                    signals += 1;
                                    changes += history.len();
                                }
                                Err(error) => errors.push(format!("signal {}: {error:#}", id.0)),
                            }
                        }
                    }
                    LoadResult::Track { track, result, .. } => match result {
                        Ok(data) => {
                            for generator in &data.generators {
                                transactions += generator.transactions().len();
                                relations += generator.relations().len();
                            }
                        }
                        Err(error) => errors.push(format!("track {}: {error:#}", track.0)),
                    },
                    _ => anyhow::bail!("unexpected metadata"),
                }
            }
        }
    }
    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        serde_json::json!({
            "path": path, "selection": selection, "memory_mib": limits.memory_mib,
            "object_mib": limits.object_mib, "open_ms": open_ms, "first_ready_ms": run.first_ready_ms,
            "total_ms": total_ms, "server_wire_bytes": run.received, "client_wire_bytes": run.sent,
            "frames": run.frames, "decode_steps": run.steps, "decode_work_ms": run.work_ms,
            "max_decode_step_ms": run.max_work_ms, "budget_peak_bytes": run.budget_peak,
            "budget_resident_bytes": run.budget.used(), "client_peak_rss_kib": peak_rss("self"),
            "server_peak_rss_kib": peak_rss(&run.child.id().to_string()), "signals": signals,
            "changes": changes, "transactions": transactions, "incident_relation_copies": relations,
            "errors": errors, "catalog": catalog,
        })
    );
    std::hint::black_box((&loaded, &result));
    Ok(())
}
