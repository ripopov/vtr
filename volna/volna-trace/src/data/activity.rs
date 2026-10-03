//! Immutable activity indexes and reader-side sidecar access.
use crate::remote::memory::{MemoryBudget, Reservation};

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
