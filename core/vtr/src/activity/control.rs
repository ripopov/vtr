//! Progress and cooperative cancellation of one sidecar build.
use std::sync::Mutex;

use crate::{Error, Result};

/// Progress of one activity-index build, counted in stitched source blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildProgress {
    /// Source blocks scanned and stitched into the output.
    pub completed: usize,
    /// Source blocks in this build; zero before the builder starts.
    pub total: usize,
}

/// Shared control for one build. Workers check cancellation between blocks;
/// a cancelled build returns after blocks already scanning return. Publishing
/// through [`super::Sidecar::write_with_control`] serializes cancellation
/// with the final rename, so an accepted cancellation never publishes.
#[derive(Debug, Default)]
pub struct BuildControl {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    progress: BuildProgress,
    cancelled: bool,
    published: bool,
}

impl BuildControl {
    /// Cancels unless publication or non-file completion has already succeeded. Returns whether
    /// cancellation was accepted. Repeated cancellation is harmless.
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.published {
            return false;
        }
        state.cancelled = true;
        true
    }

    /// Claims completion of a non-file result, serialized against cancellation.
    /// After success, cancellation is refused. A previously accepted
    /// cancellation returns an error. File writers use the same terminal state
    /// when they publish their sidecar. Repeated completion is harmless.
    pub fn complete(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.cancelled { return Err(Error::State("activity index build cancelled")) }
        state.published = true;
        Ok(())
    }

    /// Whether cancellation was accepted.
    pub fn is_cancelled(&self) -> bool {
        self.state.lock().unwrap().cancelled
    }

    /// A consistent snapshot of stitched-block progress.
    pub fn progress(&self) -> BuildProgress {
        self.state.lock().unwrap().progress
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::State("activity index build cancelled"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn begin(&self, total: usize) -> Result<()> {
        self.check()?;
        self.state.lock().unwrap().progress = BuildProgress {
            completed: 0,
            total,
        };
        Ok(())
    }

    pub(crate) fn advance(&self, completed: usize) {
        self.state.lock().unwrap().progress.completed = completed;
    }

    pub(crate) fn publish<T>(&self, write: impl FnOnce() -> Result<T>) -> Result<T> {
        let mut state = self.state.lock().unwrap();
        if state.cancelled {
            return Err(Error::State("activity index build cancelled"));
        }
        let result = write()?;
        state.published = true;
        Ok(result)
    }
}
