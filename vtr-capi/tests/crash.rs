//! Crash guard integration tests across simulation termination modes.

#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vtr::{Ending, LogQuery, Reader};

mod support;

const BLOCK_RECORDS: u64 = 1 << 20;
const SIGSEGV: i32 = 11;
const SIGABRT: i32 = 6;
const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Keeps {
    Full(Ending),
    Blocks,
    Written,
    Committed(u64),
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
    env: &'static [(&'static str, &'static str)],
    ends: Ends,
    keeps: Option<Keeps>,
    guarded: Keeps,
}

const fn crashed(signal: i32) -> Keeps {
    Keeps::Full(Ending::Crashed {
        signal,
        code: 0,
        address: 0,
        thread: 0,
        sealed: false,
    })
}

const NO_ENV: &[(&str, &str)] = &[];
const SLOW_1200: &[&str] = &[
    "--records",
    "1000000000",
    "--pace-us",
    "500",
    "--commit-ms",
    "100",
    "--kill-ms",
    "1200",
];
const SLOW_1700: &[&str] = &[
    "--records",
    "1000000000",
    "--pace-us",
    "500",
    "--commit-ms",
    "100",
    "--kill-ms",
    "1700",
];
const SLOW_OFF: &[&str] = &[
    "--records",
    "1000000000",
    "--pace-us",
    "500",
    "--commit-ms",
    "0",
    "--kill-ms",
    "1200",
];

const CASES: &[Case] = &[
    Case {
        name: "none",
        mode: "none",
        args: &[],
        env: NO_ENV,
        ends: Ends::Status(0),
        keeps: Some(Keeps::Full(Ending::Closed)),
        guarded: Keeps::Full(Ending::Closed),
    },
    Case {
        name: "segv",
        mode: "segv",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGSEGV),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGSEGV),
    },
    Case {
        name: "abort",
        mode: "abort",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGABRT),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGABRT),
    },
    Case {
        name: "throw",
        mode: "throw",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGABRT),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGABRT),
    },
    Case {
        name: "stack",
        mode: "stack",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGSEGV),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGSEGV),
    },
    Case {
        name: "heap",
        mode: "heap",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGABRT),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGABRT),
    },
    Case {
        name: "heaplock",
        mode: "heaplock",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGABRT),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGABRT),
    },
    Case {
        name: "term",
        mode: "term",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGTERM),
        keeps: Some(Keeps::Blocks),
        guarded: Keeps::Full(Ending::Stopped { signal: SIGTERM }),
    },
    Case {
        name: "deaf",
        mode: "deaf",
        args: &[],
        env: &[("VTR_GUARD_STOP_GRACE_MS", "300")],
        ends: Ends::Signal(SIGTERM),
        keeps: Some(Keeps::Blocks),
        guarded: Keeps::Full(Ending::Stopped { signal: SIGTERM }),
    },
    Case {
        name: "exit",
        mode: "exit",
        args: &[],
        env: NO_ENV,
        ends: Ends::Status(3),
        keeps: Some(Keeps::Blocks),
        guarded: Keeps::Full(Ending::Exited { status: 3 }),
    },
    Case {
        name: "kill",
        mode: "kill",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGKILL),
        keeps: Some(Keeps::Blocks),
        guarded: Keeps::Blocks,
    },
    Case {
        name: "kill-inline",
        mode: "kill",
        args: &["--inline", "--block-records", "65536"],
        env: NO_ENV,
        ends: Ends::Signal(SIGKILL),
        keeps: Some(Keeps::Written),
        guarded: Keeps::Written,
    },
    Case {
        name: "thread",
        mode: "thread",
        args: &[],
        env: NO_ENV,
        ends: Ends::Signal(SIGSEGV),
        keeps: Some(Keeps::Blocks),
        guarded: crashed(SIGSEGV),
    },
    Case {
        name: "slowkill-1200",
        mode: "slowkill",
        args: SLOW_1200,
        env: NO_ENV,
        ends: Ends::Signal(SIGKILL),
        keeps: Some(Keeps::Committed(100)),
        guarded: Keeps::Committed(100),
    },
    Case {
        name: "slowkill-1700",
        mode: "slowkill",
        args: SLOW_1700,
        env: NO_ENV,
        ends: Ends::Signal(SIGKILL),
        keeps: Some(Keeps::Committed(100)),
        guarded: Keeps::Committed(100),
    },
    Case {
        name: "slowkill-off",
        mode: "slowkill",
        args: SLOW_OFF,
        env: NO_ENV,
        ends: Ends::Signal(SIGKILL),
        keeps: Some(Keeps::Blocks),
        guarded: Keeps::Blocks,
    },
    Case {
        name: "stall",
        mode: "segv",
        args: &[],
        env: &[
            ("VTR_GUARD_TEST_STALL_MS", "5000"),
            ("VTR_GUARD_DEADLINE_MS", "1000"),
        ],
        ends: Ends::Signal(SIGSEGV),
        keeps: None,
        guarded: Keeps::Blocks,
    },
    Case {
        name: "rescue-fault",
        mode: "abort",
        args: &[],
        env: &[("VTR_GUARD_TEST_RESCUE_FAULT", "1")],
        ends: Ends::Signal(SIGSEGV),
        keeps: None,
        guarded: Keeps::Blocks,
    },
];

#[derive(Debug, Default)]
struct Emitted {
    changes: u64,
    logs: u64,
    dying_ms: Option<u64>,
}

#[derive(Debug)]
struct Kept {
    recovered: bool,
    changes: u64,
    logs: u64,
    ending: Ending,
}

fn build_harness(dir: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let cxx = std::env::var("CXX").unwrap_or_else(|_| "c++".into());
    assert!(
        Command::new(&cxx)
            .arg("--version")
            .output()
            .expect("C++ compiler is required")
            .status
            .success()
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe = dir.join(name);
    let st = Command::new(&cxx)
        .args(["-std=c++17", "-Wall", "-Wextra", "-O1", "-g"])
        .args(extra)
        .arg("-I")
        .arg(root.join("include"))
        .arg(root.join("tests/crash_harness.cpp"))
        .arg(support::static_library_with(&["private-heap"]))
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(st.success(), "C++ compilation failed");
    exe
}

fn monotonic_ms() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a live, aligned timespec output for CLOCK_MONOTONIC.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
}

fn run(exe: &Path, case: &Case, out: &Path, guard: bool) -> (Ends, String, Vec<(u64, String)>) {
    let mut cmd = Command::new(exe);
    cmd.arg(case.mode)
        .arg(out)
        .args(case.args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if guard {
        cmd.arg("--guard");
    }
    for (k, v) in case.env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .map(|l| (monotonic_ms(), l))
            .collect::<Vec<_>>()
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            drop(child.kill());
            drop(child.wait());
            panic!("{}: the run did not end within 90 s", case.name);
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let ends = match (status.code(), status.signal()) {
        (Some(c), _) => Ends::Status(c),
        (None, Some(s)) => Ends::Signal(s),
        _ => unreachable!(),
    };
    let lines = reader.join().unwrap();
    let text = lines
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (ends, text, lines)
}

fn emitted(stderr: &str) -> Emitted {
    let line = stderr
        .lines()
        .rfind(|l| l.starts_with("emitted "))
        .unwrap_or_else(|| panic!("no emitted line in:\n{stderr}"));
    let field = |name: &str| -> u64 {
        line.split_whitespace()
            .find_map(|w| w.strip_prefix(name))
            .unwrap()
            .parse()
            .unwrap()
    };
    let dying_ms = stderr
        .lines()
        .find_map(|l| l.strip_prefix("dying at "))
        .map(|v| v.trim().parse().unwrap());
    Emitted {
        changes: field("changes="),
        logs: field("logs="),
        dying_ms,
    }
}

fn kept(path: &Path) -> Kept {
    let r = Reader::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut changes = 0u64;
    r.for_each_change(0, u64::MAX, |_, _, _| changes += 1)
        .unwrap();
    let mut logs = 0u64;
    r.visit_log(&LogQuery::default(), |_| {
        logs += 1;
        true
    })
    .unwrap();
    Kept {
        recovered: r.recovered().is_some(),
        changes,
        logs,
        ending: r.ending().unwrap().0,
    }
}

fn same_ending(got: Ending, want: Ending) -> bool {
    match (got, want) {
        (
            Ending::Crashed {
                signal: a,
                sealed: x,
                ..
            },
            Ending::Crashed {
                signal: b,
                sealed: y,
                ..
            },
        ) => a == b && x == y,
        (a, b) => a == b,
    }
}

fn check(case: &Case, keeps: Keeps, ends: Ends, e: &Emitted, k: &Kept) -> Vec<String> {
    let mut problems = Vec::new();
    if ends != case.ends {
        problems.push(format!("ended {ends:?}, expected {:?}", case.ends));
    }
    let grows = matches!(case.mode, "thread" | "deaf");
    match keeps {
        Keeps::Full(want) => {
            let record = u64::from(want != Ending::Closed);
            let all = if grows {
                k.changes >= e.changes && k.logs >= e.logs + record
            } else {
                k.changes == e.changes && k.logs == e.logs + record
            };
            if k.recovered || !all || !same_ending(k.ending, want) {
                problems.push(format!("expected every change and log record in a complete file ended {want:?}, kept {k:?} of {e:?}"));
            }
        }
        Keeps::Committed(_) => {
            if !k.recovered || k.changes > e.changes || k.logs > e.logs {
                problems.push(format!("expected a recovered file, kept {k:?} of {e:?}"));
            }
        }
        Keeps::Blocks | Keeps::Written => {
            let block = case
                .args
                .iter()
                .position(|a| *a == "--block-records")
                .map_or(BLOCK_RECORDS, |i| case.args[i + 1].parse().unwrap());
            let fewer = grows || k.changes < e.changes;
            if !k.recovered || !fewer || !k.changes.is_multiple_of(block) || k.logs != 0 {
                problems.push(format!(
                    "expected a recovered file of completed blocks, kept {k:?} of {e:?}"
                ));
            }
            if keeps == Keeps::Written && k.changes != e.changes / block * block {
                problems.push(format!(
                    "expected all {} completed blocks, kept {k:?} of {e:?}",
                    e.changes / block
                ));
            }
        }
    }
    problems
}

#[test]
fn crash_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let exe = build_harness(dir.path(), "crash_harness", &[]);
    let mut failures = Vec::new();
    for case in CASES {
        for (guard, keeps) in [(false, case.keeps), (true, Some(case.guarded))] {
            let Some(keeps) = keeps else { continue };
            let out = dir.path().join(format!("{}-{guard}.vtr", case.name));
            let (ends, stderr, lines) = run(&exe, case, &out, guard);
            let e = emitted(&stderr);
            let k = kept(&out);
            let label = format!("{}{}", case.name, if guard { "/guard" } else { "" });
            eprintln!("{label:18} {ends:?} emitted {e:?} kept {k:?}");
            let mut problems = check(case, keeps, ends, &e, &k);
            if let Keeps::Committed(interval) = keeps {
                let field = |l: &str, name: &str| -> u64 {
                    l.split_whitespace()
                        .find_map(|w| w.strip_prefix(name))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap()
                };
                let killed = lines
                    .iter()
                    .find_map(|(_, l)| l.strip_prefix("killed at ms="))
                    .map(|v| v.trim().parse::<u64>().unwrap())
                    .unwrap();
                let settled = lines
                    .iter()
                    .map(|(_, l)| l)
                    .rfind(|l| {
                        l.starts_with("progress ") && field(l, "ms=") + interval + 400 <= killed
                    })
                    .unwrap();
                let (changes, logs) = (field(settled, "changes="), field(settled, "logs="));
                if k.changes < changes || k.logs < logs || changes == 0 {
                    problems.push(format!("kept {} changes and {} log records; {changes} and {logs} were emitted {interval} ms + 400 ms before SIGKILL at {killed} ms", k.changes, k.logs));
                }
            }
            if case.name == "stall" {
                match lines
                    .iter()
                    .find(|(_, l)| l.contains("missed its deadline"))
                {
                    Some((at, _)) => {
                        let waited = at.saturating_sub(e.dying_ms.unwrap());
                        if !(800..=1200).contains(&waited) {
                            problems.push(format!(
                                "gave up after {waited} ms, expected the 1000 ms deadline"
                            ));
                        }
                    }
                    None => problems.push("no deadline message".into()),
                }
            }
            if !problems.is_empty() {
                failures.push(format!("{label}: {}\n{stderr}", problems.join("; ")));
            }
            std::fs::remove_file(&out).unwrap();
        }
    }
    assert!(
        failures.is_empty(),
        "crash matrix:\n{}",
        failures.join("\n")
    );
}

#[test]
fn guard_chains_to_address_sanitizer() {
    let dir = tempfile::tempdir().unwrap();
    let exe = build_harness(
        dir.path(),
        "crash_harness_asan",
        &["-fsanitize=address", "-fno-omit-frame-pointer"],
    );
    let case = Case {
        name: "asan",
        mode: "segv",
        args: &[],
        env: &[("ASAN_OPTIONS", "detect_leaks=0")],
        ends: Ends::Status(1),
        keeps: None,
        guarded: crashed(SIGSEGV),
    };
    let out = dir.path().join("asan.vtr");
    let (ends, stderr, _) = run(&exe, &case, &out, true);
    let e = emitted(&stderr);
    let k = kept(&out);
    eprintln!("asan/guard {ends:?} emitted {e:?} kept {k:?}");
    assert!(
        stderr.contains("AddressSanitizer: SEGV"),
        "ASan's report is missing:\n{stderr}"
    );
    assert!(
        stderr.contains("trace(s) finished"),
        "the guard did not run first:\n{stderr}"
    );
    let problems = check(&case, case.guarded, ends, &e, &k);
    assert!(problems.is_empty(), "{problems:?}");
}
