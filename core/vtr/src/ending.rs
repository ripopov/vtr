//! How a run ended (SPEC section 8.5).
//!
//! A writer closed by [`Writer::close`](crate::Writer::close) writes nothing
//! extra. Any other ending is one FATAL log record in the reserved root log
//! stream [`STREAM`], written last by
//! [`Writer::close_with`](crate::Writer::close_with); every reader that shows
//! logs shows it, and [`Reader::ending`](crate::Reader::ending) decodes it.

use crate::logblock::{LogArg, LogArgType};
use std::fmt;

/// Name of the reserved root log stream holding the ending record.
pub const STREAM: &str = "vtr.run";

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Closed normally. Not recorded: a complete file without a record.
    Closed,
    /// A stop request (SIGTERM, SIGINT, ...) ended the run, and the writer
    /// was closed normally afterwards.
    Stopped { signal: i32 },
    /// The program called `exit(status)` without closing the writer.
    Exited { status: i32 },
    /// A fatal signal: its number, `si_code`, fault address and the kernel
    /// thread id that received it. `sealed` is true when the writer was
    /// interrupted mid-call and only what had reached its encoder was kept.
    Crashed { signal: i32, code: i32, address: u64, thread: u64, sealed: bool },
    /// The file was recovered by scanning; `dropped` bytes followed the last
    /// verified section.
    Recovered { dropped: u64 },
    /// A writer method panicked (a VTR bug), and the file was sealed with
    /// what had reached the encoder.
    Poisoned,
}

/// Recorded endings: site format, argument names and types, in the order the
/// variants of [`Ending`] after `Closed` are declared.
pub(crate) const SITES: [(&str, &[&str], &[LogArgType]); 5] = [
    ("run stopped by signal {}", &["signal"], &[LogArgType::I64]),
    ("run exited with status {}", &["status"], &[LogArgType::I64]),
    (
        "run crashed by signal {} (code {}, address {}, thread {}, sealed {})",
        &["signal", "code", "address", "thread", "sealed"],
        &[LogArgType::I64, LogArgType::I64, LogArgType::Pointer, LogArgType::U64, LogArgType::Bool],
    ),
    ("run recovered by scanning, {} bytes dropped", &["dropped"], &[LogArgType::U64]),
    ("run sealed after a panic in the writer", &[], &[]),
];

impl Ending {
    /// Index into [`SITES`] and the record's arguments; `None` for `Closed`.
    pub(crate) fn record(&self) -> Option<(usize, Vec<LogArg<'static>>)> {
        Some(match *self {
            Ending::Closed => return None,
            Ending::Stopped { signal } => (0, vec![LogArg::I64(signal as i64)]),
            Ending::Exited { status } => (1, vec![LogArg::I64(status as i64)]),
            Ending::Crashed { signal, code, address, thread, sealed } => {
                (2, vec![LogArg::I64(signal as i64), LogArg::I64(code as i64), LogArg::Pointer(address), LogArg::U64(thread), LogArg::Bool(sealed)])
            }
            Ending::Recovered { dropped } => (3, vec![LogArg::U64(dropped)]),
            Ending::Poisoned => (4, vec![]),
        })
    }

    /// The ending a record of site `fmt` with `args` describes.
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
                [_, _, LogArg::Pointer(address), LogArg::U64(thread), LogArg::Bool(sealed)] => {
                    Ending::Crashed { signal: i(0)?, code: i(1)?, address: *address, thread: *thread, sealed: *sealed }
                }
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

/// Linux name of a signal number (`"SIGSEGV"`), or `None`.
pub fn signal_name(signal: i32) -> Option<&'static str> {
    const NAMES: [&str; 31] = [
        "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGBUS", "SIGFPE", "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM",
        "SIGSTKFLT", "SIGCHLD", "SIGCONT", "SIGSTOP", "SIGTSTP", "SIGTTIN", "SIGTTOU", "SIGURG", "SIGXCPU", "SIGXFSZ", "SIGVTALRM", "SIGPROF", "SIGWINCH", "SIGIO",
        "SIGPWR", "SIGSYS",
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

/// `closed`, `stopped by SIGTERM`, `exited with status 3`,
/// `crashed by SIGSEGV (address 0x0, thread 41137)`, `recovered by scanning (12 bytes dropped)`.
impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Ending::Closed => f.write_str("closed"),
            Ending::Stopped { signal } => write!(f, "stopped by {}", Sig(signal)),
            Ending::Exited { status } => write!(f, "exited with status {status}"),
            Ending::Crashed { signal, address, thread, sealed, .. } => {
                write!(f, "crashed by {} (address {address:#x}, thread {thread}{})", Sig(signal), if sealed { ", sealed mid-call" } else { "" })
            }
            Ending::Recovered { dropped } => write!(f, "recovered by scanning ({dropped} bytes dropped)"),
            Ending::Poisoned => f.write_str("sealed after a panic in the writer"),
        }
    }
}
