//! Crash guard for VTR writers (docs/crash-safe-vtr.html).
//!
//! A simulation that records a trace can die in many ways: a fault in model
//! code, `abort()`, an uncaught exception, a stack overflow, heap corruption,
//! a job scheduler's SIGTERM, or `exit()` from deep code. Without a guard the
//! file then keeps only what its encoder had written. With one, every writer
//! the program [`watch`]es is finished with everything it accepted, and the
//! file records how the run ended ([`vtr::Ending`]); the process still ends
//! the way it would have, with the same signal, exit status and core dump.
//!
//! This is the one piece of VTR with process-wide state: signal dispositions
//! belong to the process. It exists only after an explicit [`install`].
//!
//! How it works:
//!
//! * **Crashes** (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT, SIGTRAP, SIGSYS)
//!   run a handler that does only async-signal-safe work: it claims the crash
//!   with a compare-and-swap, records the signal, wakes the rescue thread
//!   through an eventfd and waits with `clock_gettime` and `poll` until the
//!   rescue is done or the deadline passes. It then restores the disposition
//!   it replaced and re-raises: a hardware fault returns and faults again, any
//!   other signal is sent to the same thread with `tgkill`. A previous
//!   handler (AddressSanitizer's, a crash reporter's) therefore still runs,
//!   after the rescue.
//! * The **rescue thread** is started by [`install`] with every signal
//!   blocked. It stops the owners of the watched writers that are still
//!   running, then finishes each writer through the ordinary close path when
//!   its owner is at rest, or seals it ([`vtr::Sealer`]) when the owner was
//!   inside the writer or a call panicked.
//! * An owner at rest is **parked** by a signal whose handler sleeps; an
//!   owner inside the writer is asked to park when it leaves
//!   ([`vtr::CrashState::request_park`]).
//! * **Stop requests** (SIGTERM, SIGINT, SIGHUP, SIGXCPU) only set a flag
//!   ([`stop_requested`]): the program finishes its time step and closes
//!   with [`ending`], and when it exits the guard re-raises the signal so the
//!   exit status still says so. A second request, or the grace period,
//!   escalates to a rescue. A stop signal that already has a handler is left
//!   to it.
//! * **`exit()`** with watched writers closes them with
//!   [`Ending::Exited`](vtr::Ending::Exited) from an `on_exit` hook.
//! * Every watched writer's thread, and every thread that calls
//!   [`thread_init`], gets a 64 KiB alternate signal stack with a guard page,
//!   so a stack overflow is handled like any other fault.
//!
//! `VTR_GUARD=0` in the environment makes [`install`] do nothing;
//! `VTR_GUARD_DEADLINE_MS` and `VTR_GUARD_STOP_GRACE_MS` override the
//! deadline and the grace period. Linux only.

use std::time::Duration;

/// Guard settings. [`Options::default`] is what the Verilator fork uses.
#[derive(Clone, Debug)]
pub struct Options {
    /// How long a crashed thread waits for the rescue before it gives up and
    /// lets the process die with whatever the encoder had written. Default 10 s.
    pub deadline: Duration,
    /// Handle fatal signals. Default true.
    pub crashes: bool,
    /// Handle stop requests. Default true.
    pub stops: bool,
    /// How long a stop request waits for the program to close before the
    /// guard closes the watched writers itself. Default 10 s.
    pub stop_grace: Duration,
    /// Close watched writers on `exit()`. Default true.
    pub exit: bool,
    /// Signal that parks an owner thread; 0 chooses `SIGRTMAX - 3`.
    pub park_signal: i32,
}

impl Default for Options {
    fn default() -> Self {
        Options { deadline: Duration::from_secs(10), crashes: true, stops: true, stop_grace: Duration::from_secs(10), exit: true, park_signal: 0 }
    }
}

/// Why [`install`] or [`watch`] failed.
#[derive(Debug)]
pub enum Error {
    /// Not supported on this platform.
    Unsupported,
    /// All watch slots are in use.
    Full,
    /// A system call failed.
    Os(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => f.write_str("the crash guard is not supported on this platform"),
            Error::Full => f.write_str("the crash guard watches at most 64 writers"),
            Error::Os(e) => write!(f, "crash guard: {e}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{ending, install, stop_requested, thread_init, unwatch, watch};

#[cfg(not(target_os = "linux"))]
mod other {
    use super::*;
    use vtr::{Ending, Writer};
    pub fn install(_: &Options) -> Result<bool, Error> {
        Err(Error::Unsupported)
    }
    pub unsafe fn watch(_: *mut Writer) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    pub fn unwatch(_: *mut Writer) {}
    pub fn thread_init() {}
    pub fn stop_requested() -> i32 {
        0
    }
    pub fn ending() -> Ending {
        Ending::Closed
    }
}

#[cfg(not(target_os = "linux"))]
pub use other::{ending, install, stop_requested, thread_init, unwatch, watch};
