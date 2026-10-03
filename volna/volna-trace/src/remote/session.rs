//! Resident metadata of one remote recording. Complete histories and tracks
//! are delivered asynchronously to their consumers; this object never
//! owns a second data cache or performs I/O.

use super::objects::Metadata;
use crate::data::transactions::{Track, TrackRef};
use crate::data::{Hierarchy, SignalHistory, SignalRef, TraceInfo};
use crate::session::{Capabilities, Session};
use std::sync::Arc;

/// Validated remote metadata with a nonzero server identity. Its blocking
/// `load_*` methods fail: histories and tracks come through
/// [`super::client::RemoteClient`].
pub struct RemoteSession {
    id: u64,
    metadata: Metadata,
    sizes: Arc<crate::data::ScopeSizes>,
    reservations: Arc<Vec<super::memory::Reservation>>,
    activity: std::sync::OnceLock<Arc<crate::data::ActivityIndex>>,
}

impl RemoteSession {
    /// Install only a complete, validated metadata object. Zero is reserved for
    /// the Open request before the server assigns an identity.
    pub fn new(id: u64, metadata: Metadata) -> anyhow::Result<Self> {
        anyhow::ensure!(id != 0, "invalid remote session identity");
        metadata.validate()?;
        Ok(Self {
            id,
            sizes: Arc::new(crate::data::ScopeSizes::count(&metadata.hierarchy)),
            metadata,
            reservations: Arc::new(vec![]),
            activity: std::sync::OnceLock::new(),
        })
    }

    pub(super) fn from_parts(
        id: u64,
        metadata: Metadata,
        sizes: Arc<crate::data::ScopeSizes>,
        reservations: Arc<Vec<super::memory::Reservation>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(id != 0, "invalid remote session identity");
        Ok(Self {
            id,
            metadata,
            sizes,
            reservations,
            activity: std::sync::OnceLock::new(),
        })
    }
}

impl Session for RemoteSession {
    fn resident_bytes(&self) -> u64 {
        self.reservations
            .iter()
            .map(super::memory::Reservation::bytes)
            .sum::<u64>()
            + self.activity.get().map_or(0, |i| i.memory_bytes())
    }
    fn memory_budget(&self) -> Option<super::memory::MemoryBudget> {
        self.reservations
            .first()
            .map(|reservation| reservation.budget())
    }

    fn remote_id(&self) -> Option<u64> {
        Some(self.id)
    }
    fn capabilities(&self) -> Capabilities {
        self.metadata.capabilities
    }
    fn info(&self) -> &TraceInfo {
        &self.metadata.info
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.metadata.hierarchy
    }
    fn scope_sizes(&self) -> Option<Arc<crate::data::ScopeSizes>> {
        Some(self.sizes.clone())
    }
    fn activity(&self) -> Option<Arc<crate::data::ActivityIndex>> {
        self.activity.get().cloned()
    }
    fn activity_identity(&self) -> Option<vtr::activity::Identity> {
        self.metadata.activity.as_ref()?.identity().ok()
    }
    fn activity_build_info(&self) -> Option<crate::data::ActivityBuildInfo> {
        let id = self.activity_identity()?;
        Some(crate::data::ActivityBuildInfo {
            bytes: id.length,
            format: id.format,
        })
    }
    fn activity_available(&self) -> bool {
        self.activity.get().is_some()
            || self.metadata.activity.as_ref().is_some_and(|a| a.available)
    }
    fn activity_host(&self) -> Option<&str> {
        Some(&self.metadata.server)
    }
    fn install_activity(&self, index: Arc<crate::data::ActivityIndex>) -> anyhow::Result<()> {
        anyhow::ensure!(
            Some(index.identity()) == self.activity_identity(),
            "activity identity mismatch"
        );
        anyhow::ensure!(
            index.signal_count() as usize == self.info().signal_count,
            "activity signal count mismatch"
        );
        let _ = self.activity.set(index);
        Ok(())
    }
    fn tracks(&self) -> &[Track] {
        &self.metadata.tracks
    }
    fn load_signal(&self, _: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        anyhow::bail!("remote signals require the asynchronous executor")
    }
    fn load_track(&self, _: TrackRef) -> anyhow::Result<crate::data::loaded_tracks::LoadedTrack> {
        anyhow::bail!("remote tracks require the asynchronous executor")
    }
}
