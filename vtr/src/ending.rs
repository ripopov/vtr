//! How a run ended (SPEC section 8.5).

use crate::logblock::{LogArg, LogArgType};
use std::fmt;

pub const SCOPE: &str = "vtr.run";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Closed,
    Stopped {
        signal: i32,
    },
    Exited {
        status: i32,
    },
    Crashed {
        signal: i32,
        code: i32,
        address: u64,
        thread: u64,
        sealed: bool,
    },
    Recovered {
        dropped: u64,
    },
    Poisoned,
}

pub(crate) const SITES: [(&str, &[&str], &[LogArgType]); 5] = [
    ("run stopped by signal {}", &["signal"], &[LogArgType::I64]),
    ("run exited with status {}", &["status"], &[LogArgType::I64]),
    (
        "run crashed by signal {} (code {}, address {}, thread {}, sealed {})",
        &["signal", "code", "address", "thread", "sealed"],
        &[
            LogArgType::I64,
            LogArgType::I64,
            LogArgType::Pointer,
            LogArgType::U64,
            LogArgType::Bool,
        ],
    ),
    (
        "run recovered by scanning, {} bytes dropped",
        &["dropped"],
        &[LogArgType::U64],
    ),
    ("run sealed after a panic in the writer", &[], &[]),
];

impl Ending {
    pub(crate) fn record(&self) -> Option<(usize, Vec<LogArg<'static>>)> {
        Some(match *self {
            Ending::Closed => return None,
            Ending::Stopped { signal } => (0, vec![LogArg::I64(i64::from(signal))]),
            Ending::Exited { status } => (1, vec![LogArg::I64(i64::from(status))]),
            Ending::Crashed {
                signal,
                code,
                address,
                thread,
                sealed,
            } => (
                2,
                vec![
                    LogArg::I64(i64::from(signal)),
                    LogArg::I64(i64::from(code)),
                    LogArg::Pointer(address),
                    LogArg::U64(thread),
                    LogArg::Bool(sealed),
                ],
            ),
            Ending::Recovered { dropped } => (3, vec![LogArg::U64(dropped)]),
            Ending::Poisoned => (4, vec![]),
        })
    }

    pub(crate) fn from_record(fmt: &str, args: &[LogArg]) -> Option<Ending> {
        let site = SITES.iter().position(|s| s.0 == fmt)?;
        let i = |k: usize| match args.get(k) {
            Some(LogArg::I64(v)) => Some(*v as i32),
            _ => None,
        };
        Some(match site {
            0 => Ending::Stopped { signal: i(0)? },
            1 => Ending::Exited { status: i(0)? },
            2 => match args {
                [
                    _,
                    _,
                    LogArg::Pointer(address),
                    LogArg::U64(thread),
                    LogArg::Bool(sealed),
                ] => Ending::Crashed {
                    signal: i(0)?,
                    code: i(1)?,
                    address: *address,
                    thread: *thread,
                    sealed: *sealed,
                },
                _ => return None,
            },
            3 => match args {
                [LogArg::U64(dropped)] => Ending::Recovered { dropped: *dropped },
                _ => return None,
            },
            _ => Ending::Poisoned,
        })
    }
}

pub(crate) fn strings() -> impl Iterator<Item = &'static str> {
    [
        SCOPE,
        crate::logblock::GENERATOR_KIND,
        crate::logblock::KEY_SEVERITY,
        crate::logblock::KEY_ARGS,
        crate::logblock::KEY_NAMES,
    ]
    .into_iter()
    .chain(
        SITES
            .iter()
            .flat_map(|(fmt, names, _)| std::iter::once(*fmt).chain(names.iter().copied())),
    )
}

#[must_use]
pub fn signal_name(signal: i32) -> Option<&'static str> {
    const NAMES: [&str; 31] = [
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
    ];
    NAMES.get((signal as usize).wrapping_sub(1)).copied()
}

struct Sig(i32);

impl fmt::Display for Sig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match signal_name(self.0) {
            Some(n) => f.write_str(n),
            None => write!(f, "signal {}", self.0),
        }
    }
}

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Ending::Closed => f.write_str("closed"),
            Ending::Stopped { signal } => write!(f, "stopped by {}", Sig(signal)),
            Ending::Exited { status } => write!(f, "exited with status {status}"),
            Ending::Crashed {
                signal,
                address,
                thread,
                sealed,
                ..
            } => {
                write!(
                    f,
                    "crashed by {} (address {address:#x}, thread {thread}{})",
                    Sig(signal),
                    if sealed { ", sealed mid-call" } else { "" }
                )
            }
            Ending::Recovered { dropped } => {
                write!(f, "recovered by scanning ({dropped} bytes dropped)")
            }
            Ending::Poisoned => f.write_str("sealed after a panic in the writer"),
        }
    }
}
