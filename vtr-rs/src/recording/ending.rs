//! Typed run-ending records shared by recording and crash rescue.

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

use crate::{
    Error, Result,
    schema::{Field, Payload, PayloadReader, TypeId, Types},
};

pub(crate) const MARKER: &str = "vtr.ending";

pub(crate) struct RunEnding {
    message: String,
    reason: u8,
    signal: i32,
    code: i32,
    address: u64,
    thread: u64,
    sealed: bool,
    status: i32,
    dropped: u64,
}

impl From<Ending> for RunEnding {
    fn from(end: Ending) -> Self {
        let mut data = Self {
            message: end.to_string(),
            reason: 0,
            signal: 0,
            code: 0,
            address: 0,
            thread: 0,
            sealed: false,
            status: 0,
            dropped: 0,
        };
        match end {
            Ending::Closed => (),
            Ending::Stopped { signal } => {
                data.reason = 1;
                data.signal = signal;
            }
            Ending::Exited { status } => {
                data.reason = 2;
                data.status = status;
            }
            Ending::Crashed {
                signal,
                code,
                address,
                thread,
                sealed,
            } => {
                data.reason = 3;
                data.signal = signal;
                data.code = code;
                data.address = address;
                data.thread = thread;
                data.sealed = sealed;
            }
            Ending::Recovered { dropped } => {
                data.reason = 4;
                data.dropped = dropped;
            }
            Ending::Poisoned => data.reason = 5,
        }
        data
    }
}

impl RunEnding {
    pub(crate) fn ending(&self) -> Result<Ending> {
        let end = match self.reason {
            1 => Ending::Stopped {
                signal: self.signal,
            },
            2 => Ending::Exited {
                status: self.status,
            },
            3 => Ending::Crashed {
                signal: self.signal,
                code: self.code,
                address: self.address,
                thread: self.thread,
                sealed: self.sealed,
            },
            4 => Ending::Recovered {
                dropped: self.dropped,
            },
            5 => Ending::Poisoned,
            _ => return Err(Error::Corrupt("invalid run-ending reason")),
        };
        let canonical = Self::from(end);
        if self.signal != canonical.signal
            || self.code != canonical.code
            || self.address != canonical.address
            || self.thread != canonical.thread
            || self.sealed != canonical.sealed
            || self.status != canonical.status
            || self.dropped != canonical.dropped
            || self.message != canonical.message
        {
            return Err(Error::Corrupt("noncanonical run-ending record"));
        }
        Ok(end)
    }
}

impl Payload for RunEnding {
    fn bind(types: &mut Types) -> Result<TypeId> {
        types.structure(
            "vtr.RunEnding",
            vec![
                Field::new("message", 13),
                Field::new("reason", 6),
                Field::new("signal", 4),
                Field::new("code", 4),
                Field::new("address", 9),
                Field::new("thread", 9),
                Field::new("sealed", 1),
                Field::new("status", 4),
                Field::new("dropped", 9),
            ],
        )
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        self.message.encode(out)?;
        self.reason.encode(out)?;
        self.signal.encode(out)?;
        self.code.encode(out)?;
        self.address.encode(out)?;
        self.thread.encode(out)?;
        self.sealed.encode(out)?;
        self.status.encode(out)?;
        self.dropped.encode(out)
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        Ok(Self {
            message: String::decode(reader)?,
            reason: u8::decode(reader)?,
            signal: i32::decode(reader)?,
            code: i32::decode(reader)?,
            address: u64::decode(reader)?,
            thread: u64::decode(reader)?,
            sealed: bool::decode(reader)?,
            status: i32::decode(reader)?,
            dropped: u64::decode(reader)?,
        })
    }
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
