#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "linux")]
#[derive(Default)]
enum State {
    #[default]
    Pending,
    Running {
        clock: libc::clockid_t,
        start: u64,
    },
    Finished(Option<u64>),
}

#[derive(Clone, Default)]
pub(super) struct WorkerCpu {
    #[cfg(target_os = "linux")]
    state: Arc<Mutex<State>>,
}
pub(super) struct Measurement(WorkerCpu);
impl Drop for Measurement {
    fn drop(&mut self) {
        self.0.finish();
    }
}
impl WorkerCpu {
    pub fn measure(&self) -> Measurement {
        #[cfg(target_os = "linux")]
        {
            let mut state = self.state.lock().unwrap();
            let mut clock = 0;
            // SAFETY: pthread_self names this live thread and clock is writable.
            let result =
                unsafe { libc::pthread_getcpuclockid(libc::pthread_self(), &raw mut clock) };
            *state = if result == 0 {
                read(clock).map_or(State::Finished(None), |start| State::Running {
                    clock,
                    start,
                })
            } else {
                State::Finished(None)
            };
        }
        Measurement(self.clone())
    }
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            clippy::unused_self,
            reason = "worker clocks are only available on Linux"
        )
    )]
    pub fn nanoseconds(&self) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            let state = self.state.lock().unwrap();
            match *state {
                State::Pending => Some(0),
                State::Running { clock, start } => read(clock)?.checked_sub(start),
                State::Finished(total) => total,
            }
        }
        #[cfg(not(target_os = "linux"))]
        None
    }
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            clippy::unused_self,
            reason = "worker clocks are only available on Linux"
        )
    )]
    fn finish(&self) {
        #[cfg(target_os = "linux")]
        {
            // Serialize clock reads with exit: the OS may reuse a departed thread's clock.
            let mut state = self.state.lock().unwrap();
            if let State::Running { clock, start } = *state {
                *state = State::Finished(read(clock).and_then(|now| now.checked_sub(start)));
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn read(clock: libc::clockid_t) -> Option<u64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock comes from a live worker, whose exit is synchronized with this read.
    if unsafe { libc::clock_gettime(clock, &raw mut value) } != 0 {
        return None;
    }
    u64::try_from(value.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(value.tv_nsec).ok()?)
}

#[cfg(all(test, target_os = "linux", not(miri)))]
mod tests {
    use super::*;
    use std::{
        hint::black_box,
        sync::mpsc,
        time::{Duration, Instant},
    };
    #[test]
    fn worker_cpu_counts_work_not_sleep_and_survives_normal_or_panicked_exit() {
        for panic in [false, true] {
            let counter = WorkerCpu::default();
            let worker_counter = counter.clone();
            let (ready, started) = mpsc::channel();
            let (finish, done) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let _measurement = worker_counter.measure();
                let deadline = Instant::now() + Duration::from_secs(2);
                while worker_counter.nanoseconds().unwrap() < 10_000_000 {
                    for value in 0..10_000 {
                        black_box(value * 7);
                    }
                    assert!(Instant::now() < deadline);
                }
                ready.send(()).unwrap();
                done.recv().unwrap();
                assert!(!panic, "injected worker panic");
            });
            started.recv_timeout(Duration::from_secs(3)).unwrap();
            let before = counter.nanoseconds().unwrap();
            assert!(before >= 10_000_000);
            std::thread::sleep(Duration::from_millis(30));
            let waiting = counter.nanoseconds().unwrap();
            assert!(waiting >= before && waiting - before < 10_000_000);
            finish.send(()).unwrap();
            assert_eq!(worker.join().is_err(), panic);
            let final_value = counter.nanoseconds().unwrap();
            assert!(final_value >= waiting);
            assert_eq!(counter.nanoseconds(), Some(final_value));
        }
    }
}
