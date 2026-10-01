//! Scope activity (docs/hierarchy-activity.html): how many of each scope's
//! signals change in a window, from the trace's activity index at once, and
//! exactly once the signals the index leaves undecided are read.
//!
//! [`ActivityCounter`] holds every signal's census weights over the trace's
//! scopes, built once per trace off the UI thread. [`ActivityCounts`] is one
//! window's answer per scope; a frame costs one classification of every
//! signal and one pass over the changing signals' weights and the scopes.

use super::SignalRef;
use super::source::{Hierarchy, ScopeId};
use crate::remote::memory::{MemoryBudget, Reservation};

/// Every signal's census weights over one trace's scopes, so that the
/// distinct signals of any set below every scope cost the set's variables
/// plus one pass over the scopes.
#[derive(Debug)]
pub struct ActivityCounter {
    contributions: vtr::Contributions,
    /// Census index → scope.
    order: Vec<ScopeId>,
    reservation: Option<Reservation>,
}

impl ActivityCounter {
    /// One recording census over the hierarchy in preorder.
    pub fn build(h: &Hierarchy) -> Self {
        let mut census = vtr::Census::recording();
        let mut order = Vec::with_capacity(h.scope_count());
        let mut stack: Vec<Option<ScopeId>> = h.roots().iter().rev().map(Some).collect();
        while let Some(entry) = stack.pop() {
            let Some(id) = entry else {
                census.leave();
                continue;
            };
            census.enter();
            order.push(id);
            let scope = h.scope(id);
            for v in scope.vars {
                census.var(h.signal(v).0);
            }
            stack.push(None);
            stack.extend(scope.children.iter().rev().map(Some));
        }
        let (_, contributions) = census.finish_recorded();
        Self {
            contributions,
            order,
            reservation: None,
        }
    }

    pub fn account(mut self, budget: &MemoryBudget) -> anyhow::Result<Self> {
        self.reservation =
            Some(budget.reserve_object("the activity counter", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        self.contributions.memory_bytes()
            + (self.order.len() * std::mem::size_of::<ScopeId>()) as u64
    }

    /// Distinct signals of `signals` at or below every scope, by scope.
    fn count(&self, signals: impl IntoIterator<Item = u32>) -> Vec<u32> {
        let mut by_census = Vec::new();
        self.contributions.count(signals, &mut by_census);
        let mut by_scope = vec![0; self.order.len()];
        for (i, &scope) in self.order.iter().enumerate() {
            by_scope[scope] = by_census[i];
        }
        by_scope
    }
}

/// One window's activity per scope: the signals that change, and at most
/// how many may while some are undecided.
#[derive(Debug)]
pub struct ActivityCounts {
    /// The window in trace times, both ends included.
    pub window: (u64, u64),
    /// Signals below each scope known to change, by scope.
    changing: Vec<u32>,
    /// Changing or undecided, by scope; empty once exact.
    upper: Vec<u32>,
    /// Signals the index cannot decide in this window, to read from the
    /// trace; empty once exact.
    pub undecided: Vec<SignalRef>,
    reservation: Option<Reservation>,
}

impl ActivityCounts {
    /// What the index says about `window` (trace times).
    pub fn classify(
        index: &vtr::activity::Index,
        counter: &ActivityCounter,
        window: (u64, u64),
    ) -> Self {
        let c = index.classify(window.0, window.1);
        let changing = counter.count(c.active.iter().map(|s| s.0));
        let upper = if c.undecided.is_empty() {
            Vec::new()
        } else {
            let may = counter.count(c.undecided.iter().map(|s| s.0));
            changing.iter().zip(may).map(|(a, b)| a + b).collect()
        };
        Self {
            window,
            changing,
            upper,
            undecided: c.undecided.into_iter().map(|s| SignalRef(s.0)).collect(),
            reservation: None,
        }
    }

    /// The exact counts once the undecided signals were read: `changed` are
    /// those of them that change in the window.
    pub fn resolved(&self, counter: &ActivityCounter, changed: &[SignalRef]) -> Self {
        let more = counter.count(changed.iter().map(|s| s.0));
        Self {
            window: self.window,
            changing: self.changing.iter().zip(more).map(|(a, b)| a + b).collect(),
            upper: Vec::new(),
            undecided: Vec::new(),
            reservation: None,
        }
    }

    pub fn account(mut self, budget: &MemoryBudget) -> anyhow::Result<Self> {
        self.reservation =
            Some(budget.reserve_object("the scope activity", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        ((self.changing.len() + self.upper.len()) * 4
            + self.undecided.len() * std::mem::size_of::<SignalRef>()) as u64
    }

    /// Whether every signal is decided.
    pub fn exact(&self) -> bool {
        self.undecided.is_empty()
    }

    /// `(changing, upper)` below `scope`.
    pub fn get(&self, scope: ScopeId) -> Option<(u32, u32)> {
        let changing = *self.changing.get(scope)?;
        Some((changing, self.upper.get(scope).copied().unwrap_or(changing)))
    }
}

/// What a scope row shows of the activity in the viewport: of its `total`
/// signals, `changing` change for sure and at most `upper` may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeActivity {
    pub changing: u32,
    pub upper: u32,
    pub total: u32,
}

impl ScopeActivity {
    /// Nothing is left to read.
    pub fn exact(&self) -> bool {
        self.changing == self.upper
    }

    /// No signal below the scope can change: the row is drawn faint.
    pub fn quiet(&self) -> bool {
        self.total > 0 && self.upper == 0
    }

    /// The count column: `2,483 / 6,779`, or a range while signals are
    /// read: `1,462–2,210 / 6,779`.
    pub fn label(&self) -> String {
        let total = super::sizes::grouped(self.total);
        match self.exact() {
            true => format!("{} / {total}", super::sizes::grouped(self.changing)),
            false => format!(
                "{}–{} / {total}",
                super::sizes::grouped(self.changing),
                super::sizes::grouped(self.upper)
            ),
        }
    }

    /// Shares of the scope's signals that change and that may: the
    /// meter's solid and hatched lengths, in `0..=1`.
    pub fn shares(&self) -> (f32, f32) {
        match self.total {
            0 => (0.0, 0.0),
            t => (
                self.changing as f32 / t as f32,
                self.upper as f32 / t as f32,
            ),
        }
    }

    /// The tooltip line: `2,483 of 6,779 signals change in the view`.
    pub fn detail(&self) -> String {
        let total = super::sizes::grouped(self.total);
        match self.exact() {
            true => format!(
                "{} of {total} signals change in the view",
                super::sizes::grouped(self.changing)
            ),
            false => format!(
                "{} to {} of {total} signals change in the view; reading the rest from the trace",
                super::sizes::grouped(self.changing),
                super::sizes::grouped(self.upper)
            ),
        }
    }
}

/// An immutable activity index and the admission that follows its last shared
/// owner, including classifications queued after its session closes.
#[derive(Debug)]
pub struct ActivityIndex {
    pub(crate) index: vtr::activity::Index,
    pub(crate) _reservation: Option<Reservation>,
}

impl std::ops::Deref for ActivityIndex {
    type Target = vtr::activity::Index;
    fn deref(&self) -> &Self::Target {
        &self.index
    }
}

/// A session's activity index, when one valid for its trace was found
/// beside the trace or in the user cache as the session opened.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn find_index(
    trace: &std::path::Path,
    id: &vtr::activity::Identity,
) -> Option<std::sync::Arc<ActivityIndex>> {
    let sidecar =
        vtr::activity::Sidecar::new(trace, id, vtr::activity::default_cache_dir().as_deref());
    sidecar.load(id).map(|(_, index)| {
        std::sync::Arc::new(ActivityIndex {
            index,
            _reservation: None,
        })
    })
}

/// A recording that can build its missing activity index beside its reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivityBuildInfo {
    pub bytes: u64,
    pub format: vtr::activity::SourceFormat,
}

impl ActivityBuildInfo {
    /// A coarse estimate from compressed size, rounded up to seconds.
    /// VTR scans about 1 GiB/s and FST about 400 MiB/s on C910.
    pub fn estimated_seconds(self) -> u64 {
        let rate = match self.format {
            vtr::activity::SourceFormat::Vtr => 1 << 30,
            vtr::activity::SourceFormat::Fst => 400 << 20,
        };
        self.bytes.div_ceil(rate).max(1)
    }
}

/// The owning session's source identity and shared, immutable activity index.
/// A late-built index pays for itself; an index found at open is included in
/// the session's initial resident-byte admission.
pub(crate) struct ActivitySource {
    pub path: std::path::PathBuf,
    pub identity: vtr::activity::Identity,
    loaded: std::sync::OnceLock<std::sync::Arc<ActivityIndex>>,
}

impl ActivitySource {
    #[cfg(not(target_family = "wasm"))]
    pub fn new(path: &std::path::Path, identity: vtr::activity::Identity) -> Self {
        let loaded = std::sync::OnceLock::new();
        if let Some(index) = find_index(path, &identity) {
            let _ = loaded.set(index);
        }
        Self {
            path: path.to_owned(),
            identity,
            loaded,
        }
    }

    pub fn index(&self) -> Option<std::sync::Arc<ActivityIndex>> {
        self.loaded.get().map(std::sync::Arc::clone)
    }

    pub fn resident_bytes(&self) -> u64 {
        self.loaded.get().map_or(0, |l| l.memory_bytes())
    }

    pub fn info(&self) -> ActivityBuildInfo {
        ActivityBuildInfo {
            bytes: self.identity.length,
            format: self.identity.format,
        }
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn image(&self, cache_dir: Option<&std::path::Path>) -> anyhow::Result<Vec<u8>> {
        let sidecar = vtr::activity::Sidecar::new(&self.path, &self.identity, cache_dir);
        for path in sidecar.paths() {
            if let Ok(bytes) = std::fs::read(path)
                && vtr::activity::Index::decode(&bytes, &self.identity).is_ok()
            {
                return Ok(bytes);
            }
        }
        anyhow::bail!("no valid activity sidecar")
    }

    pub fn build(
        &self,
        options: &vtr::activity::BuildOptions,
        budget: &MemoryBudget,
        cache_dir: Option<&std::path::Path>,
        signals: usize,
        build: impl FnOnce(&mut dyn std::io::Write) -> vtr::Result<vtr::activity::Summary>,
    ) -> anyhow::Result<()> {
        if self.loaded.get().is_some() {
            return Ok(());
        }
        let control = options
            .control
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("activity build needs a control"))?;
        if control.is_cancelled() {
            anyhow::bail!("activity build cancelled");
        }
        let sidecar = vtr::activity::Sidecar::new(&self.path, &self.identity, cache_dir);
        // A persistent sibling lock coordinates separate server processes.
        // Never unlink it: a waiter must lock the same inode as a new client.
        #[cfg(not(target_family = "wasm"))]
        let _lock = {
            let mut locked = None;
            for path in sidecar.paths() {
                let mut name = path.as_os_str().to_owned();
                name.push(".lock");
                if let Some(parent) = std::path::Path::new(&name).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(file) = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(name)
                {
                    loop {
                        match file.try_lock() {
                            Ok(()) => break,
                            Err(std::fs::TryLockError::WouldBlock) => {
                                if control.is_cancelled() {
                                    anyhow::bail!("activity build cancelled");
                                }
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                        }
                    }
                    locked = Some(file);
                    break;
                }
            }
            locked.ok_or_else(|| anyhow::anyhow!("cannot lock the activity sidecar"))?
        };
        // Another process may have published while this one waited.
        let path = if let Some((path, index)) = sidecar.load(&self.identity) {
            drop(index);
            path
        } else {
            let (path, _) = sidecar.write_with_control(control, |w| {
                let scratch = options
                    .memory
                    .saturating_add((signals as u64).saturating_mul(24));
                let _reservation = budget
                    .reserve(scratch)
                    .map_err(|e| vtr::Error::Invalid(e.to_string()))?;
                build(w)
            })?;
            path
        };
        let index = vtr::activity::Index::open(&path, &self.identity)?;
        let reservation = budget.reserve_object("the activity index", index.memory_bytes())?;
        control.complete()?;
        let _ = self.loaded.set(std::sync::Arc::new(ActivityIndex {
            index,
            _reservation: Some(reservation),
        }));
        Ok(())
    }
}
