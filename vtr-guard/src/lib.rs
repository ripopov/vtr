//! Crash guard for VTR writers.

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Options {
    pub deadline: Duration,
    pub crashes: bool,
    pub stops: bool,
    pub stop_grace: Duration,
    pub exit: bool,
    pub park_signal: i32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            deadline: Duration::from_secs(10),
            crashes: true,
            stops: true,
            stop_grace: Duration::from_secs(10),
            exit: true,
            park_signal: 0,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Unsupported,
    Full,
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
pub use linux::{
    add_stop_callback, ending, install, remove_stop_callback, stop_requested, thread_init, unwatch,
    watch,
};

#[cfg(not(target_os = "linux"))]
mod other {
    use super::{Error, Options};
    use vtr::recording::{Ending, Writer};
    pub fn install(_: &Options) -> Result<bool, Error> {
        Err(Error::Unsupported)
    }
    /// # Safety
    /// The writer must remain live and immovable on its registering thread until unwatched.
    pub unsafe fn watch(_: *mut Writer) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    pub fn unwatch(_: *mut Writer) {}
    pub fn thread_init() {}
    #[must_use]
    pub fn stop_requested() -> i32 {
        0
    }
    #[must_use]
    pub fn ending() -> Ending {
        Ending::Closed
    }
    #[must_use]
    pub fn add_stop_callback(_: Box<dyn Fn(i32) + Send + Sync>) -> u64 {
        0
    }
    pub fn remove_stop_callback(_: u64) {}
}

#[cfg(not(target_os = "linux"))]
pub use other::{
    add_stop_callback, ending, install, remove_stop_callback, stop_requested, thread_init, unwatch,
    watch,
};
