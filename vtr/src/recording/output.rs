use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
};

use super::{QueueStats, cpu::WorkerCpu};
use crate::{
    Error, Result,
    framing::{Fingerprint, FramedWriter, RecordingId},
};

#[derive(Clone, Copy)]
pub(super) struct Section {
    pub kind: u32,
    pub begin: u64,
    pub end: u64,
    pub block: u64,
}
impl Section {
    pub fn new(kind: u32, begin: u64, end: u64, block: u64) -> Self {
        Self {
            kind,
            begin,
            end,
            block,
        }
    }
}

type Encode = Box<dyn FnOnce() -> Result<Vec<u8>> + Send>;
struct Job {
    sequence: u64,
    bytes: usize,
    section: Section,
    encode: Encode,
}
struct Encoded {
    sequence: u64,
    bytes: usize,
    section: Section,
    payload: Result<Vec<u8>>,
}

struct Pool {
    sender: Option<mpsc::SyncSender<Job>>,
    receiver: mpsc::Receiver<Encoded>,
    threads: Vec<JoinHandle<()>>,
    ready: BTreeMap<u64, Encoded>,
    next: u64,
    written: u64,
    pending_bytes: usize,
    max_jobs: usize,
    max_bytes: usize,
    waits: u64,
    wait_nanoseconds: u64,
    peak_bytes: usize,
}
impl Pool {
    fn new(counters: &[WorkerCpu], max_jobs: usize, max_bytes: usize) -> Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(max_jobs);
        let jobs = Arc::new(Mutex::new(jobs));
        let (results, receiver) = mpsc::channel();
        let mut threads = Vec::new();
        for (id, cpu) in counters.iter().cloned().enumerate() {
            let jobs = jobs.clone();
            let results = results.clone();
            match std::thread::Builder::new()
                .name(format!("vtr-encode-{id}"))
                .spawn(move || {
                    let _measurement = cpu.measure();
                    loop {
                        let job = jobs.lock().unwrap().recv();
                        let Ok(job) = job else {
                            break;
                        };
                        let payload =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.encode))
                                .unwrap_or_else(|_| {
                                    Err(Error::Recording("encoding worker panicked".into()))
                                });
                        if results
                            .send(Encoded {
                                sequence: job.sequence,
                                bytes: job.bytes,
                                section: job.section,
                                payload,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }) {
                Ok(thread) => threads.push(thread),
                Err(error) => {
                    drop(sender);
                    for thread in threads {
                        drop(thread.join());
                    }
                    return Err(error.into());
                }
            }
        }
        drop(results);
        Ok(Self {
            sender: Some(sender),
            receiver,
            threads,
            ready: BTreeMap::new(),
            next: 0,
            written: 0,
            pending_bytes: 0,
            max_jobs,
            max_bytes,
            waits: 0,
            wait_nanoseconds: 0,
            peak_bytes: 0,
        })
    }
    fn stats(&self) -> QueueStats {
        QueueStats {
            submitted: self.next,
            completed: self.written,
            waits: self.waits,
            wait_nanoseconds: self.wait_nanoseconds,
            peak_bytes: self.peak_bytes,
        }
    }
    fn pending(&self) -> u64 {
        self.next - self.written
    }
    fn receive(&mut self, blocking: bool) -> Result<()> {
        let result = if blocking {
            Some(
                self.receiver
                    .recv()
                    .map_err(|_| Error::State("encoding workers stopped"))?,
            )
        } else {
            match self.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) if self.pending() == 0 => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(Error::State("encoding workers stopped"));
                }
            }
        };
        if let Some(result) = result {
            self.ready.insert(result.sequence, result);
        }
        Ok(())
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        self.sender.take();
        for thread in self.threads.drain(..) {
            drop(thread.join());
        }
    }
}

pub(super) struct Output {
    writer: FramedWriter<Box<dyn Write + Send>>,
    pool: Option<Pool>,
    failure: Option<(String, bool)>,
    worker_cpu: Vec<WorkerCpu>,
    encoding_stats: QueueStats,
    pub transaction_blocks: u64,
    pub record_blocks: u64,
    pub waveform_blocks: u64,
}
impl Output {
    pub fn new(output: Box<dyn Write + Send>, options: &super::EncodingOptions) -> Result<Self> {
        let writer = FramedWriter::new(output)?;
        let worker_cpu = (0..options.workers)
            .map(|_| WorkerCpu::default())
            .collect::<Vec<_>>();
        let pool = if options.workers == 0 {
            None
        } else {
            Some(Pool::new(
                &worker_cpu,
                options.queue_jobs,
                options.queue_bytes,
            )?)
        };
        Ok(Self {
            writer,
            pool,
            failure: None,
            worker_cpu,
            encoding_stats: QueueStats::default(),
            transaction_blocks: 0,
            record_blocks: 0,
            waveform_blocks: 0,
        })
    }
    pub fn worker_cpu(&self) -> Vec<WorkerCpu> {
        self.worker_cpu.clone()
    }
    pub fn stats(&self) -> QueueStats {
        self.pool.as_ref().map_or(self.encoding_stats, Pool::stats)
    }
    fn stop_pool(&mut self) {
        if let Some(pool) = self.pool.take() {
            self.encoding_stats = pool.stats();
            drop(pool);
        }
    }
    pub fn identity(&self) -> RecordingId {
        self.writer.identity()
    }
    pub fn position(&self) -> u64 {
        self.writer.position()
    }
    pub fn failed(&self) -> bool {
        self.failure.is_some()
    }
    fn usable(&self) -> Result<()> {
        if let Some((message, _)) = &self.failure {
            Err(Error::Recording(message.clone()))
        } else {
            Ok(())
        }
    }
    fn write(&mut self, section: Section, payload: Vec<u8>) -> Result<()> {
        let result = self.writer.section(
            section.kind,
            0,
            &payload,
            section.begin,
            section.end,
            section.block,
        );
        if let Err(error) = &result {
            self.failure = Some((error.to_string(), false));
        } else {
            match section.kind {
                4 => self.waveform_blocks += 1,
                5 => self.transaction_blocks += 1,
                9 => self.record_blocks += 1,
                _ => (),
            }
        }
        result
    }
    fn drain_ready(&mut self) -> Result<()> {
        loop {
            let result = self
                .pool
                .as_mut()
                .and_then(|pool| pool.ready.remove(&pool.written));
            let Some(result) = result else {
                return Ok(());
            };
            let payload = match result.payload {
                Ok(payload) => payload,
                Err(error) => {
                    self.failure = Some((error.to_string(), true));
                    return Err(error);
                }
            };
            self.write(result.section, payload)?;
            let pool = self.pool.as_mut().unwrap();
            pool.pending_bytes -= result.bytes;
            pool.written += 1;
        }
    }
    pub fn poll(&mut self) -> Result<()> {
        self.usable()?;
        if self.pool.is_none() {
            return Ok(());
        }
        loop {
            let pool = self.pool.as_mut().unwrap();
            let before = pool.ready.len();
            pool.receive(false)?;
            let changed = pool.ready.len() != before;
            self.drain_ready()?;
            if !changed {
                return Ok(());
            }
        }
    }
    pub fn encode(
        &mut self,
        section: Section,
        bytes: usize,
        encode: impl FnOnce() -> Result<Vec<u8>> + Send + 'static,
    ) -> Result<()> {
        self.usable()?;
        if self.pool.is_none() {
            let payload = match encode() {
                Ok(payload) => payload,
                Err(error) => {
                    self.failure = Some((error.to_string(), true));
                    return Err(error);
                }
            };
            return self.write(section, payload);
        }
        while self.pool.as_ref().is_some_and(|pool| {
            pool.pending() != 0
                && (pool.pending() >= pool.max_jobs as u64
                    || bytes > pool.max_bytes.saturating_sub(pool.pending_bytes))
        }) {
            let pool = self.pool.as_mut().unwrap();
            pool.waits += 1;
            let began = std::time::Instant::now();
            let result = pool.receive(true);
            pool.wait_nanoseconds = pool
                .wait_nanoseconds
                .saturating_add(u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX));
            result?;
            self.drain_ready()?;
        }
        let pool = self.pool.as_mut().unwrap();
        let sequence = pool.next;
        pool.sender
            .as_ref()
            .unwrap()
            .send(Job {
                sequence,
                bytes,
                section,
                encode: Box::new(encode),
            })
            .map_err(|_| Error::State("encoding workers stopped"))?;
        pool.next = pool
            .next
            .checked_add(1)
            .ok_or(Error::invalid("encoding sequence exhausted"))?;
        pool.pending_bytes = pool
            .pending_bytes
            .checked_add(bytes)
            .ok_or(Error::invalid("encoding byte count overflow"))?;
        pool.peak_bytes = pool.peak_bytes.max(pool.pending_bytes);
        self.poll()
    }
    pub fn section(&mut self, section: Section, payload: Vec<u8>) -> Result<()> {
        self.encode(section, payload.capacity(), move || Ok(payload))
    }
    pub fn flush(&mut self) -> Result<()> {
        self.usable()?;
        while self.pool.as_ref().is_some_and(|pool| pool.pending() != 0) {
            self.pool.as_mut().unwrap().receive(true)?;
            self.drain_ready()?;
        }
        let result = self.writer.flush();
        if let Err(error) = &result {
            self.failure = Some((error.to_string(), false));
        }
        result
    }
    pub fn finish(&mut self, incomplete: bool) -> Result<Fingerprint> {
        self.flush()?;
        self.stop_pool();
        self.writer.finish(incomplete, 0)
    }
    pub fn seal_failed(&mut self) -> Result<Fingerprint> {
        if !self
            .failure
            .as_ref()
            .is_some_and(|(_, recoverable)| *recoverable)
        {
            return Err(Error::State("output cannot be sealed after an I/O failure"));
        }
        self.stop_pool();
        self.writer.finish(true, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    use std::time::Duration;

    #[derive(Clone)]
    struct Bytes(Arc<Mutex<Vec<u8>>>);
    impl Write for Bytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn concurrent_encoders_publish_in_submission_order() {
        let bytes = Bytes(Arc::default());
        let options = super::super::EncodingOptions {
            workers: 2,
            queue_jobs: 2,
            queue_bytes: 16,
            ..super::super::EncodingOptions::default()
        };
        let mut output = Output::new(Box::new(bytes.clone()), &options).unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let first = gate.clone();
        output
            .encode(Section::new(1, 0, 0, 0), 8, move || {
                let (lock, changed) = &*first;
                let guard = changed
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(2), |done| !*done)
                    .unwrap();
                if !*guard.0 {
                    return Err(Error::State("second encoder never ran concurrently"));
                }
                Ok(b"first".to_vec())
            })
            .unwrap();
        output
            .encode(Section::new(2, 0, 0, 0), 8, move || {
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
                Ok(b"second".to_vec())
            })
            .unwrap();
        output.finish(false).unwrap();
        #[cfg(target_os = "linux")]
        assert!(
            output
                .worker_cpu()
                .iter()
                .all(|counter| counter.nanoseconds().unwrap() > 0)
        );
        let container =
            crate::framing::Container::from_bytes(bytes.0.lock().unwrap().clone(), true).unwrap();
        assert_eq!(
            container.payload(&container.sections()[0], true).unwrap(),
            b"first"
        );
        assert_eq!(
            container.payload(&container.sections()[1], true).unwrap(),
            b"second"
        );
    }

    #[test]
    fn encoding_failure_seals_only_the_published_prefix() {
        let bytes = Bytes(Arc::default());
        let options = super::super::EncodingOptions {
            workers: 2,
            queue_jobs: 3,
            ..super::super::EncodingOptions::default()
        };
        let mut output = Output::new(Box::new(bytes.clone()), &options).unwrap();
        output
            .section(Section::new(1, 0, 0, 0), b"metadata".to_vec())
            .unwrap();
        output.flush().unwrap();
        let _accepted = output.encode(Section::new(2, 0, 0, 0), 1, || {
            Err(Error::Codec("injected encoder failure".into()))
        });
        let _accepted = output.section(Section::new(3, 0, 0, 0), b"must not publish".to_vec());
        assert!(output.flush().is_err());
        output.seal_failed().unwrap();
        assert!(output.stats().submitted > output.stats().completed);
        let container =
            crate::framing::Container::from_bytes(bytes.0.lock().unwrap().clone(), true).unwrap();
        assert_eq!(container.completion, crate::framing::Completion::Incomplete);
        assert_eq!(container.sections().len(), 1);
        assert_eq!(
            container.payload(&container.sections()[0], true).unwrap(),
            b"metadata"
        );
    }

    #[test]
    fn encoding_queue_keeps_backpressure_and_completion_stats_after_shutdown() {
        let bytes = Bytes(Arc::default());
        let options = super::super::EncodingOptions {
            workers: 1,
            queue_jobs: 1,
            queue_bytes: 8,
            ..Default::default()
        };
        let mut output = Output::new(Box::new(bytes), &options).unwrap();
        output
            .encode(Section::new(1, 0, 0, 0), 8, || {
                std::thread::sleep(Duration::from_millis(10));
                Ok(b"first".to_vec())
            })
            .unwrap();
        output
            .encode(Section::new(2, 0, 0, 0), 8, || Ok(b"second".to_vec()))
            .unwrap();
        output.finish(false).unwrap();
        let stats = output.stats();
        assert_eq!((stats.submitted, stats.completed), (2, 2));
        assert_eq!(stats.peak_bytes, 8);
        assert!(stats.waits >= 1 && stats.wait_nanoseconds > 0);
        assert_eq!(output.stats().submitted, stats.submitted);
    }
}
