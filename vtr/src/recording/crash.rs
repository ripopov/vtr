//! Crash-state coordination; independent sealers never borrow a writer frontend.

use super::{Ending, transport::Rescue};
use crate::{Error, Result, framing::Fingerprint};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering, compiler_fence},
};

#[derive(Default)]
pub struct CrashState {
    busy: AtomicU32,
    poisoned: AtomicBool,
    watched: AtomicBool,
    park: AtomicUsize,
}
impl CrashState {
    #[inline]
    fn enter(&self) -> u32 {
        // Only the exclusive frontend updates nesting. Signal handlers and sealers
        // observe it atomically, but never enter or leave its capture scopes.
        let previous = self.busy.load(Ordering::Relaxed);
        self.busy.store(1, Ordering::Relaxed);
        compiler_fence(Ordering::SeqCst);
        previous
    }
    #[inline]
    fn leave(&self, previous: u32) {
        compiler_fence(Ordering::SeqCst);
        self.busy.store(previous, Ordering::Relaxed);
        if previous == 0 {
            let park = self.park.load(Ordering::Relaxed);
            if park != 0 {
                // SAFETY: request_park stores only fn() pointers and zero is excluded.
                let park: fn() = unsafe { std::mem::transmute(park) };
                park();
            }
        }
    }
    pub fn request_park(&self, park: fn()) {
        self.park.store(park as usize, Ordering::Relaxed);
    }
    pub fn clear_park_request(&self) {
        self.park.store(0, Ordering::Relaxed);
    }
    pub fn poison(&self) {
        self.poisoned.store(true, Ordering::Relaxed);
    }
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed) != 0 || self.poisoned()
    }
    pub fn poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }
    pub fn watched(&self) -> bool {
        self.watched.load(Ordering::Relaxed)
    }
    pub fn set_watched(&self, watched: bool) {
        self.watched.store(watched, Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn scoped(&self) -> Scoped<'_> {
        Scoped {
            state: self,
            previous: self.enter(),
        }
    }
    #[inline]
    pub(crate) fn guard(self: &Arc<Self>) -> Guarded {
        Guarded {
            state: self.clone(),
            previous: self.enter(),
        }
    }
}

pub(crate) struct Scoped<'a> {
    state: &'a CrashState,
    previous: u32,
}
impl Drop for Scoped<'_> {
    #[inline]
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.state.poison();
        } else {
            self.state.leave(self.previous);
        }
    }
}

pub struct Guarded {
    state: Arc<CrashState>,
    previous: u32,
}
impl Drop for Guarded {
    #[inline]
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.state.poison();
        } else {
            self.state.leave(self.previous);
        }
    }
}

#[derive(Clone)]
pub struct Sealer {
    pub(super) state: Arc<CrashState>,
    pub(super) rescue: Option<Rescue>,
}
impl Sealer {
    #[must_use]
    pub fn state(&self) -> &CrashState {
        &self.state
    }
    pub fn seal(&self, ending: Ending) -> Result<()> {
        self.finish(ending).map(|_| ())
    }
    pub(super) fn finish(&self, ending: Ending) -> Result<Fingerprint> {
        self.rescue
            .as_ref()
            .ok_or(Error::State(
                "a busy inline writer cannot be sealed independently",
            ))?
            .seal(ending)
    }
}
