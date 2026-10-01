//! Progress, cancellation and concurrent publication of activity sidecars.
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use vtr::activity::{
    self, Block, BlockScan, Budget, BuildControl, BuildOptions, Identity, Sidecar, Source,
    SourceFormat,
};
use vtr::{Error, Result};

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
    fn cost(&self, _: usize, _: Budget) -> Result<u64> {
        Ok(1024)
    }
    fn worker(&self) {}
    fn scan(&self, _: &mut (), i: usize, scan: &mut BlockScan) -> Result<()> {
        if i == 1 {
            if let Some((entered, resume)) = &self.pause {
                entered.send(()).unwrap();
                resume
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .expect("resume the scan");
            }
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
