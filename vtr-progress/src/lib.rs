//! Conversion work counters and the progress messages shared by import hosts.

use std::io::Read;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Preparing,
    Reading,
    Writing,
    Finalizing,
    Complete,
    Failed,
}

impl Phase {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing input",
            Self::Reading => "Reading input",
            Self::Writing => "Writing VTR",
            Self::Finalizing => "Finalizing VTR",
            Self::Complete => "Complete",
            Self::Failed => "Conversion failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub phase: Phase,
    pub completed: u64,
    pub total: Option<u64>,
    pub input_bytes: u64,
    pub output_bytes: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: VERSION,
            phase: Phase::Preparing,
            completed: 0,
            total: None,
            input_bytes: 0,
            output_bytes: 0,
        }
    }
}

impl Snapshot {
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        if self.phase == Phase::Complete {
            return Some(1.0);
        }
        self.total
            .filter(|&n| n > 0)
            .map(|n| (self.completed as f64 / n as f64).clamp(0.0, 1.0) as f32)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.version == VERSION
            && self
                .total
                .is_none_or(|total| total > 0 && self.completed <= total)
    }
}

type Observer = Arc<dyn Fn(Phase, u64, Option<u64>) + Send + Sync>;

#[derive(Default)]
struct Count {
    value: AtomicU64,
    step: u64,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[derive(Clone, Default)]
pub struct Counter(Arc<Count>);

impl Counter {
    pub fn set(&self, value: u64) {
        let previous = self.0.value.fetch_max(value, Ordering::Relaxed);
        self.notify(previous, previous.max(value));
    }

    pub fn advance(&self, amount: u64) {
        let previous = self.0.value.fetch_add(amount, Ordering::Relaxed);
        self.notify(previous, previous.saturating_add(amount));
    }

    fn notify(&self, previous: u64, completed: u64) {
        if let Some(notify) = &self.0.notify {
            let step = self.0.step.max(1);
            if previous == 0 && completed > 0 || previous / step != completed / step {
                notify();
            }
        }
    }
}

#[derive(Default)]
struct Work {
    phase: Phase,
    counter: Counter,
    total: Option<u64>,
}

#[derive(Clone, Default)]
pub struct Reporter {
    work: Arc<Mutex<Work>>,
    observer: Option<Observer>,
}

impl Reporter {
    /// Delivers phase changes and coalesced counter updates on the calling thread.
    /// The callback runs without holding the progress lock.
    pub fn with_observer(
        observer: impl Fn(Phase, u64, Option<u64>) + Send + Sync + 'static,
    ) -> Self {
        Self {
            work: Arc::default(),
            observer: Some(Arc::new(observer)),
        }
    }

    #[expect(
        clippy::must_use_candidate,
        reason = "Starting an indeterminate phase does not need a work counter."
    )]
    pub fn begin(&self, phase: Phase, total: Option<u64>) -> Counter {
        let total = total.filter(|&n| n > 0);
        let notify = self.observer.as_ref().map(|observer| {
            let observer = observer.clone();
            let work = Arc::downgrade(&self.work);
            Arc::new(move || {
                if let Some(work) = work.upgrade() {
                    let (phase, completed, total) = {
                        let work = work.lock().expect("progress lock");
                        (
                            work.phase,
                            work.counter.0.value.load(Ordering::Relaxed),
                            work.total,
                        )
                    };
                    observer(phase, total.map_or(completed, |n| completed.min(n)), total);
                }
            }) as Arc<dyn Fn() + Send + Sync>
        });
        let counter = Counter(Arc::new(Count {
            value: AtomicU64::new(0),
            step: total.map_or(4096, |total| total.div_ceil(1000)),
            notify,
        }));
        *self.work.lock().expect("progress lock") = Work {
            phase,
            counter: counter.clone(),
            total,
        };
        if let Some(observer) = &self.observer {
            observer(phase, 0, total);
        }
        counter
    }

    #[must_use]
    pub fn snapshot(&self, input_bytes: u64, output_bytes: u64) -> Snapshot {
        let work = self.work.lock().expect("progress lock");
        let completed = work.counter.0.value.load(Ordering::Relaxed);
        Snapshot {
            version: VERSION,
            phase: work.phase,
            completed: work.total.map_or(completed, |total| completed.min(total)),
            total: work.total,
            input_bytes,
            output_bytes,
        }
    }
}

pub struct CountingReader<R> {
    inner: R,
    counter: Counter,
}

impl<R> CountingReader<R> {
    pub fn new(inner: R, counter: Counter) -> Self {
        Self { inner, counter }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(bytes)?;
        self.counter.advance(read as u64);
        Ok(read)
    }
}

#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    let mut size = bytes as f64;
    let mut unit = "B";
    for next in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        if size < 1024.0 {
            break;
        }
        size /= 1024.0;
        unit = next;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_units_do_not_turn_every_sample_into_a_progress_message() {
        let events = Arc::new(AtomicU64::new(0));
        let observed = events.clone();
        let reporter = Reporter::with_observer(move |_, _, _| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        let counter = reporter.begin(Phase::Reading, Some(10_000_000_000_000));
        for tick in 1..=10_000 {
            counter.set(tick * 1_000_000_000);
        }
        assert!(events.load(Ordering::Relaxed) <= 1002);
        assert_eq!(reporter.snapshot(0, 0).fraction(), Some(1.0));
    }

    #[test]
    fn observers_can_read_snapshots_and_coalesce_updates_without_retaining_the_reporter() {
        let slot: Arc<Mutex<Option<Reporter>>> = Arc::default();
        let weak = Arc::downgrade(&slot);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let events = observed.clone();
        let reporter = Reporter::with_observer(move |phase, completed, total| {
            let reporter = weak.upgrade().unwrap().lock().unwrap().clone().unwrap();
            let snapshot = reporter.snapshot(12, 34);
            assert_eq!(
                (snapshot.phase, snapshot.completed, snapshot.total),
                (phase, completed, total)
            );
            events.lock().unwrap().push(snapshot);
        });
        *slot.lock().unwrap() = Some(reporter.clone());
        let reading = reporter.begin(Phase::Reading, Some(10000));
        for _ in 0..10000 {
            reading.advance(1);
        }
        let writing = reporter.begin(Phase::Writing, Some(10));
        writing.advance(100);
        reading.advance(4096);
        let snapshots = observed.lock().unwrap();
        assert!(snapshots.len() <= 1005);
        assert_eq!(snapshots.last().unwrap().phase, Phase::Writing);
        assert_eq!(snapshots.last().unwrap().completed, 10);
        assert!(snapshots.iter().all(Snapshot::valid));
        drop(snapshots);
        slot.lock().unwrap().take();
        let work = Arc::downgrade(&reporter.work);
        drop(reporter);
        assert!(work.upgrade().is_none());
    }

    #[test]
    fn phase_changes_detach_old_counters_and_bound_progress() {
        let reporter = Reporter::default();
        let reading = reporter.begin(Phase::Reading, Some(100));
        reading.set(75);
        reading.set(50);
        assert_eq!(reporter.snapshot(100, 10).completed, 75);
        let writing = reporter.begin(Phase::Writing, Some(20));
        reading.advance(100);
        assert_eq!(reporter.snapshot(100, 10).completed, 0);
        writing.advance(30);
        let snapshot = reporter.snapshot(100, 1000);
        assert_eq!(snapshot.completed, 20);
        assert_eq!(snapshot.fraction(), Some(1.0));
        assert!(snapshot.valid());
        reporter.begin(Phase::Finalizing, None);
        assert_eq!(reporter.snapshot(100, 1000).fraction(), None);
        reporter.begin(Phase::Complete, None);
        assert_eq!(reporter.snapshot(100, 1100).fraction(), Some(1.0));
    }
}
