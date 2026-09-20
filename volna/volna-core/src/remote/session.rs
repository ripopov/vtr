//! Resident metadata of one remote recording. Complete histories and tracks
//! are delivered asynchronously to their document consumers; this object never
//! owns a second data cache or performs I/O.

use super::objects::Metadata;
use crate::data::transactions::{Track, TrackRef};
use crate::data::{Hierarchy, SignalHistory, SignalRef, TraceInfo};
use crate::session::{Capabilities, Session};
use std::sync::Arc;

pub struct RemoteSession {
    id: u64,
    metadata: Metadata,
    _reservation: Option<super::memory::Reservation>,
}

impl RemoteSession {
    /// Install only a complete, validated metadata object. Zero is reserved for
    /// the Open request before the server assigns an identity.
    pub fn new(id: u64, metadata: Metadata) -> anyhow::Result<Self> {
        anyhow::ensure!(id != 0, "invalid remote session identity");
        metadata.validate()?;
        Ok(Self {
            id,
            metadata,
            _reservation: None,
        })
    }

    pub(super) fn from_decoded(
        id: u64,
        decoded: super::metadata::ValidatedMetadata,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(id != 0, "invalid remote session identity");
        Ok(Self {
            id,
            metadata: decoded.metadata,
            _reservation: Some(decoded.reservation),
        })
    }
}

impl Session for RemoteSession {
    fn memory_budget(&self) -> Option<super::memory::MemoryBudget> {
        self._reservation
            .as_ref()
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
