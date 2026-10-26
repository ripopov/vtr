#![cfg(target_os = "linux")]

use std::{
    os::unix::process::ExitStatusExt,
    process::Command,
    time::{Duration, Instant},
};
use vtr::{
    framing::Completion,
    recording::{Ending, ExecutionMode, Recorder, SourceOptions, Writer, WriterOptions},
    schema::{Payload, PayloadReader, Types},
    trace::Reader,
};

struct FaultPayload;
impl Payload for FaultPayload {
    fn bind(_: &mut Types) -> vtr::Result<u32> {
        Ok(0)
    }
    fn encode(&self, _: &mut Vec<u8>) -> vtr::Result<()> {
        // SAFETY: This isolated child deliberately delivers the installed fatal signal.
        unsafe {
            libc::raise(libc::SIGSEGV);
        }
        unreachable!()
    }
    fn decode(_: &mut PayloadReader<'_>) -> vtr::Result<Self> {
        Ok(Self)
    }
}

#[test]
fn signal_child() {
    let Ok(scenario) = std::env::var("VTR_GUARD_SCENARIO") else {
        return;
    };
    let path = std::env::var_os("VTR_GUARD_TRACE").unwrap();
    // SAFETY: The initialized rlimit disables core dumps only in this subprocess.
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(libc::setrlimit(libc::RLIMIT_CORE, &raw const limit), 0);
        libc::signal(libc::SIGSEGV, libc::SIG_DFL);
    }
    assert!(
        vtr_guard::install(&vtr_guard::Options {
            deadline: Duration::from_secs(3),
            stop_grace: Duration::from_millis(100),
            ..vtr_guard::Options::default()
        })
        .unwrap()
    );
    let mode = if scenario.contains("inline") {
        ExecutionMode::Inline
    } else {
        ExecutionMode::Threaded
    };
    let mut writer = Box::new(
        Writer::create_with(
            path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap(),
    );
    let logger = writer
        .logger(SourceOptions::new("diagnostics", None))
        .unwrap();
    let source = writer
        .callsite::<u64>(vtr::CallsiteOptions::new("step {}"))
        .unwrap();
    let fault = writer
        .callsite::<FaultPayload>(vtr::CallsiteOptions::new("interrupted capture"))
        .unwrap();
    writer.advance_time(17).unwrap();
    writer.log(logger, source, &42, None).unwrap();
    writer.flush().unwrap();
    // SAFETY: Box keeps the writer live and immovable on this thread until process exit or unwatch.
    unsafe {
        vtr_guard::watch(&raw mut *writer).unwrap();
    }
    match scenario.as_str() {
        "busy-threaded" | "busy-inline" => {
            writer.log(logger, fault, &FaultPayload, None).unwrap();
        }
        "exit-threaded" => {
            // SAFETY: The subprocess intentionally exercises libc's registered on_exit handler.
            unsafe {
                libc::exit(7);
            }
        }
        "stop-threaded" => {
            // SAFETY: The stop handler is installed and this signal only requests shutdown.
            unsafe {
                libc::raise(libc::SIGTERM);
            }
            assert_eq!(vtr_guard::stop_requested(), libc::SIGTERM);
            vtr_guard::unwatch(&raw mut *writer);
            writer.close_with(vtr_guard::ending()).unwrap();
            return;
        }
        "escalate-threaded" => {
            // SAFETY: The installed stop handler will ask the rescue thread to park this owner.
            unsafe {
                libc::raise(libc::SIGTERM);
            }
            loop {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "idle-threaded" | "idle-inline" => {
            // SAFETY: This subprocess intentionally exercises fatal-signal rescue.
            unsafe {
                libc::raise(libc::SIGSEGV);
            }
        }
        _ => panic!("unknown scenario"),
    }
    panic!("fatal rescue returned to the child");
}

#[test]
fn native_signals_finish_idle_and_busy_writers() {
    let directory = std::env::temp_dir().join(format!("vtr-guard-signals-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for scenario in [
        "idle-threaded",
        "busy-threaded",
        "idle-inline",
        "busy-inline",
        "exit-threaded",
        "stop-threaded",
        "escalate-threaded",
    ] {
        let path = directory.join(format!("{scenario}.vtr"));
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "signal_child", "--nocapture"])
            .env("VTR_GUARD_SCENARIO", scenario)
            .env("VTR_GUARD_TRACE", &path)
            .env_remove("VTR_GUARD")
            .env_remove("VTR_GUARD_DEADLINE_MS")
            .env_remove("VTR_GUARD_STOP_GRACE_MS")
            .env_remove("VTR_GUARD_TEST_STALL_MS")
            .env_remove("VTR_GUARD_TEST_RESCUE_FAULT")
            .spawn()
            .unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() > Duration::from_secs(8) {
                child.kill().unwrap();
                drop(child.wait());
                panic!("{scenario} exceeded the rescue deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if scenario.starts_with("exit") {
            assert_eq!(status.code(), Some(7));
        } else if scenario.starts_with("stop") || scenario.starts_with("escalate") {
            assert_eq!(status.signal(), Some(libc::SIGTERM));
        } else {
            assert_eq!(status.signal(), Some(libc::SIGSEGV), "{scenario}: {status}");
        }
        let reader = Reader::open(&path).unwrap();
        assert_ne!(reader.completion(), Completion::Complete, "{scenario}");
        assert_eq!(
            reader
                .lookup(1)
                .unwrap()
                .unwrap()
                .payload()
                .decode::<u64>()
                .unwrap(),
            42
        );
        let (ending, time) = reader.ending().unwrap();
        match scenario {
            "busy-inline" => assert!(matches!(ending, Ending::Recovered { .. })),
            "exit-threaded" => assert_eq!(ending, Ending::Exited { status: 7 }),
            "stop-threaded" | "escalate-threaded" => assert_eq!(
                ending,
                Ending::Stopped {
                    signal: libc::SIGTERM
                }
            ),
            _ => assert!(
                matches!(ending, Ending::Crashed { signal: libc::SIGSEGV, sealed, .. }
                if sealed == scenario.starts_with("busy")),
                "{scenario}: {ending:?}"
            ),
        }
        assert_eq!(time, (scenario != "busy-inline").then_some(17));
        reader.validate().unwrap();
    }
    std::fs::remove_dir_all(directory).unwrap();
}
