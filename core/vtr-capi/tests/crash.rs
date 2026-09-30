//! The crash matrix (docs/crash-safe-vtr.html): every way a simulation can
//! end, run in a child process against the static library, with the file it
//! leaves read back by the Rust reader.
//!
//! `tests/crash_harness.cpp` writes value changes and log records and then
//! ends as asked. Each case states what the file must hold afterwards:
//! `Full` is a complete file with every accepted change and log record;
//! `Blocks` is a file recovered by scanning that keeps only completed signal
//! blocks and no log records; `Written` is `Blocks` with every completed
//! block present, which the inline encoder makes exact.
//! `bench/crashlab/run.py` is the full-size measurement of the same endings.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vtr::{LogQuery, Reader};

mod support;

const BLOCK_RECORDS: u64 = 1 << 20;
const SIGSEGV: i32 = 11;
const SIGABRT: i32 = 6;
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Keeps {
    /// A complete file with everything the writer accepted.
    Full,
    /// Recovered by scanning: completed signal blocks only, no log records.
    Blocks,
    /// `Blocks`, and every block completed before the end is in the file.
    Written,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ends {
    Status(i32),
    Signal(i32),
}

struct Case {
    name: &'static str,
    mode: &'static str,
    args: &'static [&'static str],
    ends: Ends,
    keeps: Keeps,
}

/// Today's outcomes: without a guard, only a normal close keeps everything.
const CASES: &[Case] = &[
    Case { name: "none", mode: "none", args: &[], ends: Ends::Status(0), keeps: Keeps::Full },
    Case { name: "segv", mode: "segv", args: &[], ends: Ends::Signal(SIGSEGV), keeps: Keeps::Blocks },
    Case { name: "abort", mode: "abort", args: &[], ends: Ends::Signal(SIGABRT), keeps: Keeps::Blocks },
    Case { name: "throw", mode: "throw", args: &[], ends: Ends::Signal(SIGABRT), keeps: Keeps::Blocks },
    Case { name: "stack", mode: "stack", args: &[], ends: Ends::Signal(SIGSEGV), keeps: Keeps::Blocks },
    Case { name: "heap", mode: "heap", args: &[], ends: Ends::Signal(SIGABRT), keeps: Keeps::Blocks },
    Case { name: "heaplock", mode: "heaplock", args: &[], ends: Ends::Signal(SIGABRT), keeps: Keeps::Blocks },
    Case { name: "term", mode: "term", args: &[], ends: Ends::Signal(SIGTERM), keeps: Keeps::Blocks },
    Case { name: "exit", mode: "exit", args: &[], ends: Ends::Status(3), keeps: Keeps::Blocks },
    Case { name: "kill", mode: "kill", args: &[], ends: Ends::Signal(SIGKILL), keeps: Keeps::Blocks },
    // Small blocks on the simulation thread: each finished section is in the file at once.
    Case { name: "kill-inline", mode: "kill", args: &["--inline", "--block-records", "65536"], ends: Ends::Signal(SIGKILL), keeps: Keeps::Written },
    Case { name: "thread", mode: "thread", args: &[], ends: Ends::Signal(SIGSEGV), keeps: Keeps::Blocks },
];

/// What the harness reported before it died.
#[derive(Debug, Default)]
struct Emitted {
    changes: u64,
    logs: u64,
}

/// What the file holds.
#[derive(Debug)]
struct Kept {
    recovered: bool,
    changes: u64,
    logs: u64,
}

fn build_harness(dir: &Path) -> PathBuf {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    assert!(Command::new(&cxx).arg("--version").output().expect("C++ compiler is required").status.success());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe = dir.join("crash_harness");
    let st = Command::new(&cxx)
        .args(["-std=c++17", "-Wall", "-Wextra", "-O1", "-g"])
        .arg("-I")
        .arg(root.join("include"))
        .arg(root.join("tests/crash_harness.cpp"))
        .arg(support::static_library())
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(st.success(), "C++ compilation failed");
    exe
}

/// Runs one ending with a hard timeout; returns how it ended and its stderr.
fn run(exe: &Path, mode: &str, out: &Path, args: &[&str]) -> (Ends, String) {
    let mut child = Command::new(exe).arg(mode).arg(out).args(args).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || std::io::read_to_string(stderr).unwrap_or_default());
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{mode}: the run did not end within 60 s");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let ends = match (status.code(), status.signal()) {
        (Some(c), _) => Ends::Status(c),
        (None, Some(s)) => Ends::Signal(s),
        _ => unreachable!(),
    };
    (ends, reader.join().unwrap())
}

fn emitted(stderr: &str) -> Emitted {
    let line = stderr.lines().find(|l| l.starts_with("emitted ")).unwrap_or_else(|| panic!("no emitted line in:\n{stderr}"));
    let field = |name: &str| -> u64 {
        let v = line.split_whitespace().find_map(|w| w.strip_prefix(name)).unwrap();
        v.parse().unwrap()
    };
    Emitted { changes: field("changes="), logs: field("logs=") }
}

fn kept(path: &Path) -> Kept {
    let r = Reader::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut changes = 0u64;
    r.for_each_change(0, u64::MAX, |_, _, _| changes += 1).unwrap();
    let mut logs = 0u64;
    r.visit_log(&LogQuery::default(), |_| {
        logs += 1;
        true
    })
    .unwrap();
    Kept { recovered: r.recovered().is_some(), changes, logs }
}

#[test]
fn crash_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let exe = build_harness(dir.path());
    let mut failures = Vec::new();
    for case in CASES {
        let out = dir.path().join(format!("{}.vtr", case.name));
        let (ends, stderr) = run(&exe, case.mode, &out, case.args);
        let e = emitted(&stderr);
        let k = kept(&out);
        eprintln!("{:11} {:?} emitted {:?} kept {:?}", case.name, ends, e, k);
        let mut problems = Vec::new();
        if ends != case.ends {
            problems.push(format!("ended {ends:?}, expected {:?}", case.ends));
        }
        match case.keeps {
            Keeps::Full => {
                if k.recovered || k.changes != e.changes || k.logs != e.logs {
                    problems.push(format!("expected every change and log record in a complete file, kept {k:?} of {e:?}"));
                }
            }
            Keeps::Blocks | Keeps::Written => {
                let block = case.args.iter().position(|a| *a == "--block-records").map_or(BLOCK_RECORDS, |i| case.args[i + 1].parse().unwrap());
                // The thread case keeps writing after it reports, so only the file's own shape is checked.
                let fewer = case.mode == "thread" || k.changes < e.changes;
                if !k.recovered || !fewer || k.changes % block != 0 || k.logs != 0 {
                    problems.push(format!("expected a recovered file of completed blocks, kept {k:?} of {e:?}"));
                }
                if case.keeps == Keeps::Written && k.changes != e.changes / block * block {
                    problems.push(format!("expected all {} completed blocks, kept {k:?} of {e:?}", e.changes / block));
                }
            }
        }
        if !problems.is_empty() {
            failures.push(format!("{}: {}\n{stderr}", case.name, problems.join("; ")));
        }
        std::fs::remove_file(&out).unwrap();
    }
    assert!(failures.is_empty(), "crash matrix:\n{}", failures.join("\n"));
}
