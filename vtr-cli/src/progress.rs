//! Conversion progress on terminals, redirected diagnostics, and the native host stream.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use vtr_progress::{Phase, Reporter, Snapshot, format_bytes};

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Style {
    Auto,
    Json,
    None,
}

pub struct Display {
    pub reporter: Reporter,
    output: Arc<Mutex<PathBuf>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Display {
    pub fn start(input_bytes: u64, output: &Path, style: Style) -> Self {
        let reporter = Reporter::default();
        let path = Arc::new(Mutex::new(output.to_owned()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = if matches!(style, Style::None) {
            None
        } else {
            let reporter = reporter.clone();
            let path = path.clone();
            let stop = stop.clone();
            Some(std::thread::spawn(move || {
                let terminal = std::io::stderr().is_terminal();
                let mut previous = None;
                let mut last = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
                let mut tick = 0;
                loop {
                    let done = stop.load(Ordering::Acquire);
                    let output_bytes = std::fs::metadata(&*path.lock().expect("output path lock"))
                        .map_or(0, |m| m.len());
                    let snapshot = reporter.snapshot(input_bytes, output_bytes);
                    if done
                        || previous != Some(snapshot.phase)
                        || last.elapsed()
                            >= if terminal || matches!(style, Style::Json) {
                                Duration::from_millis(100)
                            } else {
                                Duration::from_secs(1)
                            }
                    {
                        render(&snapshot, style, terminal, done, tick);
                        previous = Some(snapshot.phase);
                        last = Instant::now();
                    }
                    if done {
                        break;
                    }
                    tick += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }))
        };
        Self {
            reporter,
            output: path,
            stop,
            worker,
        }
    }

    pub fn output(&self, path: &Path) {
        path.clone_into(&mut self.output.lock().expect("output path lock"));
    }

    pub fn complete(&mut self, path: &Path) {
        self.output(path);
        self.reporter.begin(Phase::Complete, None);
        self.stop();
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            drop(worker.join());
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        if !self.stop.load(Ordering::Acquire) {
            self.reporter.begin(Phase::Failed, None);
            self.stop();
        }
    }
}

fn render(snapshot: &Snapshot, style: Style, terminal: bool, done: bool, tick: usize) {
    if matches!(style, Style::Json) {
        let mut stdout = std::io::stdout().lock();
        if serde_json::to_writer(&mut stdout, snapshot).is_ok() {
            drop(writeln!(stdout));
            drop(stdout.flush());
        }
        return;
    }
    let bar = if let Some(fraction) = snapshot.fraction() {
        let filled = (fraction * 20.0).floor() as usize;
        format!(
            "[{}{}] {:3.0}%",
            "=".repeat(filled),
            " ".repeat(20 - filled),
            fraction * 100.0
        )
    } else {
        let mut cells = [b' '; 20];
        cells[tick % 20] = b'>';
        format!("[{}]     ", String::from_utf8_lossy(&cells))
    };
    let mut stderr = std::io::stderr().lock();
    if terminal {
        drop(write!(stderr, "\r\x1b[2K"));
    }
    drop(write!(
        stderr,
        "{} {bar}  Input {}  Output {}",
        snapshot.phase.label(),
        format_bytes(snapshot.input_bytes),
        format_bytes(snapshot.output_bytes)
    ));
    if !terminal || done {
        drop(writeln!(stderr));
    } else {
        drop(stderr.flush());
    }
}
