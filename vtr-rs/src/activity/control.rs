//! Progress and cooperative cancellation of one sidecar build.

use std::sync::Mutex;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildProgress {
    pub completed: usize,
    pub total: usize,
}

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
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.published {
            return false;
        }
        state.cancelled = true;
        true
    }

    pub fn complete(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.cancelled {
            return Err(Error::State("activity index build cancelled"));
        }
        state.published = true;
        Ok(())
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.lock().unwrap().cancelled
    }

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
