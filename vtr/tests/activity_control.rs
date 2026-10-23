//! Progress, cancellation and concurrent publication of activity sidecars.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use vtr::activity::{
    self, Block, BlockScan, BuildControl, BuildOptions, Identity, Sidecar, Source, SourceFormat,
};
use vtr::{Error, Result};

#[test]
fn empty_signal_tables_yield_and_cancel_during_initialization() {
    use std::{
        cell::Cell,
        future::Future,
        task::{Context, Poll, Waker},
    };
    let identity = Identity {
        format: SourceFormat::Vtr,
        length: 32,
        toc_crc: 7,
        recording: vtr::RecordingId([17; 16]),
    };
    let mut bytes = Vec::new();
    activity::Builder::new(&mut bytes, identity, 4096, 0)
        .unwrap()
        .finish()
        .unwrap();
    for cancel_at in [None, Some(32)] {
        let checkpoints = Cell::new(0);
        let mut decode = std::pin::pin!(activity::Index::decode_with(
            &bytes,
            &identity,
            |_| Ok(()),
            || {
                let count = checkpoints.get() + 1;
                checkpoints.set(count);
                let mut waiting = true;
                std::future::poll_fn(move |_| {
                    if waiting {
                        waiting = false;
                        Poll::Pending
                    } else {
                        Poll::Ready(if cancel_at == Some(count) {
                            Err(Error::State("cancelled"))
                        } else {
                            Ok(())
                        })
                    }
                })
            }
        ));
        let mut pending = 0;
        let result = loop {
            match decode
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Pending => pending += 1,
                Poll::Ready(result) => break result,
            }
        };
        if let Some(limit) = cancel_at {
            assert!(matches!(result, Err(Error::State("cancelled"))));
            assert_eq!(checkpoints.get(), limit);
            assert_eq!(pending, limit);
        } else {
            let index = result.unwrap();
            assert_eq!(index.signal_count(), 4096);
            assert_eq!(index.stretch_count(), 0);
            assert!(pending >= 4096);
            assert_eq!(
                index.memory_bytes(),
                activity::Index::decode(&bytes, &identity)
                    .unwrap()
                    .memory_bytes()
            );
        }
    }
}

struct Trace {
    blocks: Vec<Block>,
    pause: Option<(mpsc::SyncSender<()>, Mutex<mpsc::Receiver<()>>)>,
}

impl Trace {
    fn new() -> Self {
        Self {
            blocks: (0..6)
                .map(|i| Block {
                    start: i * 100,
                    end: (i + 1) * 100 - 1,
                    bytes: 4096,
                })
                .collect(),
            pause: None,
        }
    }
}

impl Source for Trace {
    type Worker = ();
    fn identity(&self) -> Identity {
        Identity {
            format: SourceFormat::Vtr,
            length: 100_000,
            toc_crc: 5,
            recording: vtr::framing::RecordingId([42; 16]),
        }
    }
    fn signal_count(&self) -> u32 {
        1
    }
    fn t_min(&self) -> u64 {
        0
    }
    fn blocks(&self) -> &[Block] {
        &self.blocks
    }
    fn cost(&self, _: usize) -> Result<u64> {
        Ok(1024)
    }
    fn worker(&self) {}
    fn scan(&self, (): &mut (), i: usize, scan: &mut BlockScan) -> Result<()> {
        if i == 1
            && let Some((entered, resume)) = &self.pause
        {
            entered.send(()).unwrap();
            resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .expect("resume the scan");
        }
        for t in (self.blocks[i].start + 1..self.blocks[i].end).step_by(3) {
            scan.push(0, t);
        }
        Ok(())
    }
}

#[test]
fn controlled_builds_have_identical_bytes_and_complete_progress() {
    let trace = Trace::new();
    let mut plain = Vec::new();
    activity::build_from(&trace, &mut plain, &BuildOptions::default()).unwrap();
    for threads in [1, 2, 7] {
        let control = Arc::new(BuildControl::default());
        let options = BuildOptions {
            threads,
            control: Some(control.clone()),
            ..Default::default()
        };
        let mut controlled = Vec::new();
        activity::build_from(&trace, &mut controlled, &options).unwrap();
        assert_eq!(plain, controlled);
        assert_eq!(control.progress().completed, trace.blocks.len());
        assert_eq!(control.progress().total, trace.blocks.len());
    }
}

#[test]
fn cancellation_during_a_scan_stops_and_removes_temporary_output() {
    let dir = tempfile::tempdir().unwrap();
    let (entered, waiting) = mpsc::sync_channel(1);
    let (resume, paused) = mpsc::sync_channel(1);
    let mut trace = Trace::new();
    trace.pause = Some((entered, Mutex::new(paused)));
    let control = Arc::new(BuildControl::default());
    let options = BuildOptions {
        threads: 1,
        control: Some(control.clone()),
        ..Default::default()
    };
    let sidecar = Sidecar::new(&dir.path().join("run.vtr"), &trace.identity(), None);
    std::thread::scope(|scope| {
        let build = scope.spawn(|| {
            sidecar.write_with_control(&control, |w| activity::build_from(&trace, w, &options))
        });
        waiting
            .recv_timeout(Duration::from_secs(5))
            .expect("second block started");
        let deadline = Instant::now() + Duration::from_secs(5);
        while control.progress().completed == 0 {
            assert!(Instant::now() < deadline, "first block must be stitched");
            std::thread::yield_now();
        }
        assert_eq!(control.progress().total, 6);
        assert!(control.cancel());
        resume.send(()).unwrap();
        assert!(matches!(
            build.join().unwrap(),
            Err(Error::State("activity index build cancelled"))
        ));
    });
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    assert!(control.progress().completed < control.progress().total);
}

#[test]
fn cancellation_at_publication_preserves_an_existing_index() {
    let dir = tempfile::tempdir().unwrap();
    let trace = Trace::new();
    let sidecar = Sidecar::new(&dir.path().join("run.vtr"), &trace.identity(), None);
    sidecar
        .write(|w| activity::build_from(&trace, w, &BuildOptions::default()))
        .unwrap();
    let bytes = std::fs::read(&sidecar.beside).unwrap();
    let control = Arc::new(BuildControl::default());
    let options = BuildOptions {
        control: Some(control.clone()),
        ..Default::default()
    };
    let result = sidecar.write_with_control(&control, |w| {
        let summary = activity::build_from(&trace, w, &options)?;
        assert!(control.cancel());
        Ok(summary)
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&sidecar.beside).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn concurrent_builds_do_not_share_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let trace = Trace::new();
    let sidecar = Sidecar::new(&dir.path().join("run.vtr"), &trace.identity(), None);
    let cancelled = BuildControl::default();
    sidecar
        .write(|w| {
            let summary = activity::build_from(&trace, w, &BuildOptions::default())?;
            let second = sidecar.write_with_control(&cancelled, |second| {
                let result = activity::build_from(&trace, second, &BuildOptions::default())?;
                assert_eq!(
                    std::fs::read_dir(dir.path()).unwrap().count(),
                    2,
                    "each build owns its temporary output"
                );
                cancelled.cancel();
                Ok(result)
            });
            assert!(second.is_err());
            Ok(summary)
        })
        .unwrap();
    assert!(sidecar.load(&trace.identity()).is_some());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn signal_tables_are_admitted_before_allocating_even_without_waveform_blocks() {
    struct Empty;
    impl Source for Empty {
        type Worker = ();
        fn identity(&self) -> Identity {
            Trace::new().identity()
        }
        fn signal_count(&self) -> u32 {
            u32::MAX
        }
        fn t_min(&self) -> u64 {
            0
        }
        fn blocks(&self) -> &[Block] {
            &[]
        }
        fn cost(&self, _: usize) -> Result<u64> {
            unreachable!()
        }
        fn worker(&self) {}
        fn scan(&self, (): &mut (), _: usize, _: &mut BlockScan) -> Result<()> {
            unreachable!()
        }
    }
    let mut output = Vec::new();
    let options = BuildOptions {
        memory: 4096,
        ..BuildOptions::default()
    };
    let error = activity::build_from(&Empty, &mut output, &options).unwrap_err();
    assert!(error.to_string().contains("signal tables exceed"));
    assert!(output.is_empty());
}
