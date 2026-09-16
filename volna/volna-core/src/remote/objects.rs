//! Serializable complete objects, separate from reader internals and UI state.

use crate::data::loaded_tracks::{
    LoadedGenerator, LoadedRelation, LoadedTrack, TransactionLocation,
};
use crate::data::transactions::{Track, TrackKind, TrackRef, Transaction, TransactionRef};
use crate::data::{Hierarchy, TraceInfo};
use crate::session::{Capabilities, Session};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub info: TraceInfo,
    pub hierarchy: Hierarchy,
    pub capabilities: Capabilities,
    pub tracks: Vec<Track>,
}

impl Metadata {
    pub fn from_session(session: &dyn Session) -> Self {
        Self {
            info: session.info().clone(),
            hierarchy: session.hierarchy().clone(),
            capabilities: session.capabilities(),
            tracks: session.tracks().to_vec(),
        }
    }

    /// Validate all references before installing resident metadata. A tree
    /// traversal proves acyclicity as well as unique membership.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut validation = std::pin::pin!(self.validate_with(|| std::future::ready(())));
        match std::future::Future::poll(
            validation.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        ) {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("synchronous validation never yields"),
        }
    }

    pub(super) async fn validate_with<F: std::future::Future<Output = ()>>(
        &self,
        mut checkpoint: impl FnMut() -> F,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.info.time_range.0 <= self.info.time_range.1,
            "reversed trace time range"
        );
        let h = &self.hierarchy;
        let mut seen = HashSet::new();
        let mut vars = HashSet::new();
        let mut pending = Vec::new();
        for &id in &h.roots {
            checkpoint().await;
            pending.push((id, None));
        }
        while let Some((id, parent)) = pending.pop() {
            checkpoint().await;
            anyhow::ensure!(seen.insert(id), "repeated or cyclic scope");
            let scope = h
                .scopes
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("invalid scope reference"))?;
            anyhow::ensure!(scope.parent == parent, "inconsistent scope parent");
            for &var in &scope.vars {
                checkpoint().await;
                anyhow::ensure!(vars.insert(var), "duplicate variable declaration");
                let declaration = h
                    .vars
                    .get(var)
                    .ok_or_else(|| anyhow::anyhow!("invalid variable reference"))?;
                anyhow::ensure!(declaration.scope == id, "inconsistent variable scope");
            }
            for &child in &scope.children {
                checkpoint().await;
                pending.push((child, Some(id)));
            }
        }
        anyhow::ensure!(
            seen.len() == h.scopes.len() && vars.len() == h.vars.len(),
            "unreachable declarations"
        );
        let mut signals = HashMap::new();
        for var in &h.vars {
            checkpoint().await;
            anyhow::ensure!(
                !matches!(var.shape, crate::data::SignalShape::Vector { width: 0 | 1 }),
                "invalid vector width"
            );
            if let Some(shape) = signals.insert(var.signal, var.shape) {
                anyhow::ensure!(shape == var.shape, "inconsistent alias shape");
            }
        }
        let mut tracks = HashMap::new();
        for track in &self.tracks {
            checkpoint().await;
            anyhow::ensure!(
                tracks.insert(track.id, &track.kind).is_none(),
                "duplicate track identity"
            );
        }
        for track in &self.tracks {
            checkpoint().await;
            if let TrackKind::Generator { stream } = track.kind {
                anyhow::ensure!(
                    matches!(tracks.get(&stream), Some(TrackKind::Stream { .. })),
                    "invalid generator stream"
                );
            }
        }
        anyhow::ensure!(
            self.capabilities.transactions || self.tracks.is_empty(),
            "tracks without transaction capability"
        );
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeneratorPayload {
    #[serde(skip)]
    pub(super) reservation: Option<super::memory::Reservation>,
    pub generator: TrackRef,
    pub transactions: Vec<Transaction>,
    pub parents: Vec<(TransactionRef, TransactionLocation)>,
    pub relations: Vec<LoadedRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TrackPayload {
    pub track: TrackRef,
    pub generators: Vec<GeneratorPayload>,
}

impl TrackPayload {
    /// Borrow the backend's complete records for serialization. Only the
    /// parent-location list is projected; records, attributes and relations
    /// stay in their original immutable storage throughout the response.
    pub fn from_loaded(track: &LoadedTrack) -> impl Serialize + '_ {
        #[derive(Serialize)]
        struct Generator<'a> {
            generator: TrackRef,
            transactions: &'a [Transaction],
            parents: Vec<(TransactionRef, TransactionLocation)>,
            relations: &'a [LoadedRelation],
        }
        #[derive(Serialize)]
        struct Track<'a> {
            track: TrackRef,
            generators: Vec<Generator<'a>>,
        }
        Track {
            track: track.track,
            generators: track
                .generators
                .iter()
                .map(|generator| Generator {
                    generator: generator.generator(),
                    transactions: generator.transactions(),
                    parents: generator
                        .transactions()
                        .iter()
                        .filter_map(|tx| generator.parent(tx.id).map(|p| (tx.id, p)))
                        .collect(),
                    relations: generator.relations(),
                })
                .collect(),
        }
    }

    pub fn into_loaded(self, catalog: &[Track]) -> anyhow::Result<LoadedTrack> {
        let mut build = std::pin::pin!(self.into_loaded_with(catalog, || std::future::ready(())));
        match std::future::Future::poll(
            build.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        ) {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("synchronous track construction never yields"),
        }
    }

    pub(super) async fn into_loaded_with<F: std::future::Future<Output = ()>>(
        self,
        catalog: &[Track],
        mut checkpoint: impl FnMut() -> F,
    ) -> anyhow::Result<LoadedTrack> {
        let mut declaration = None;
        let mut expected = HashSet::new();
        let mut catalog_generators = HashSet::new();
        for track in catalog {
            checkpoint().await;
            if track.id == self.track {
                declaration = Some(track);
            }
            if let TrackKind::Generator { stream } = track.kind {
                catalog_generators.insert(track.id);
                if stream == self.track {
                    expected.insert(track.id);
                }
            }
        }
        let declaration = declaration.ok_or_else(|| anyhow::anyhow!("unknown loaded track"))?;
        if matches!(declaration.kind, TrackKind::Generator { .. }) {
            expected = HashSet::from([self.track]);
        }
        let mut seen = HashSet::new();
        let mut result = vec![];
        for payload in self.generators {
            checkpoint().await;
            anyhow::ensure!(
                expected.contains(&payload.generator) && seen.insert(payload.generator),
                "unexpected or duplicate generator"
            );
            let mut parents = HashMap::new();
            for (child, parent) in payload.parents {
                checkpoint().await;
                anyhow::ensure!(
                    parents.insert(child, parent).is_none(),
                    "duplicate parent reference"
                );
            }
            let mut generator = LoadedGenerator::from_sorted(
                payload.generator,
                payload.transactions,
                parents,
                payload.relations,
                &mut checkpoint,
            )
            .await?;
            generator.reservation = payload.reservation;
            result.push(Arc::new(generator));
        }
        anyhow::ensure!(seen == expected, "incomplete stream payload");
        let mut transactions = HashMap::new();
        let mut relations = HashMap::new();
        for generator in &result {
            for tx in generator.transactions() {
                checkpoint().await;
                anyhow::ensure!(
                    transactions.insert(tx.id, generator.generator()).is_none(),
                    "duplicate transaction across generators"
                );
            }
        }
        let endpoint = |id: TransactionRef, owner: TrackRef| -> anyhow::Result<()> {
            anyhow::ensure!(
                catalog_generators.contains(&owner),
                "unknown endpoint generator"
            );
            if expected.contains(&owner) {
                anyhow::ensure!(
                    transactions.get(&id) == Some(&owner),
                    "missing or misplaced endpoint in complete track"
                );
            }
            Ok(())
        };
        for generator in &result {
            for tx in generator.transactions() {
                checkpoint().await;
                if let Some(parent) = generator.parent(tx.id) {
                    endpoint(parent.transaction, parent.generator)?;
                }
            }
            for edge in generator.relations() {
                checkpoint().await;
                endpoint(edge.relation.from, edge.from_generator)?;
                endpoint(edge.relation.to, edge.to_generator)?;
                if let Some(previous) = relations.insert(edge.id, edge) {
                    // Bitwise wire equality preserves NaNs and signed zero in
                    // typed attributes; derived float equality does not.
                    anyhow::ensure!(
                        super::equality::relation(previous, edge, &mut checkpoint).await,
                        "inconsistent relation identity"
                    );
                }
            }
        }
        Ok(LoadedTrack {
            track: self.track,
            generators: result,
        })
    }
}
