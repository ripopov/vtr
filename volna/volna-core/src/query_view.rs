//! Executor-neutral bridge from the app's visible rows to bounded queries.
//! Hosts call `poll` after layout/input and on task wakeups, outside painting.
use crate::{
    App,
    panels::PanelId,
    wave::demand::{Demand, Progress, WaveDemands},
};
use std::task::{Context, Poll};
use vtr_query::{
    Budget, Error, Grid, Reservation, Result,
    session::{AsyncSession, SnapshotId},
    wave::Limits,
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Row {
    panel: PanelId,
    index: usize,
    signal: u32,
}
struct Binding {
    row: Row,
    revision: Option<u64>,
}

pub struct QueryView<S: AsyncSession> {
    metadata: crate::query_metadata::MetadataQueries<S::Task>,
    navigation: crate::query_navigation::Navigation<S::Task>,
    cursor: crate::query_cursor::CursorQueries<S::Task>,
    demands: Option<WaveDemands<S>>,
    generation: u64,
    snapshot: SnapshotId,
    bindings: Vec<Option<Binding>>,
    intents: Vec<(Row, Demand)>,
    max_items: u32,
    _charge: Reservation,
}
impl<S: AsyncSession> QueryView<S> {
    /// Attach a query session to the already-opened matching trace metadata.
    /// A failed attachment does not bind or alter the document.
    pub fn attach(
        app: &mut App,
        session: S,
        limits: Limits,
        max_items: u32,
        max_rows: usize,
        budget: &Budget,
    ) -> Result<Self> {
        if max_rows == 0 || max_rows > 4096 || max_items == 0 || max_items > 4096 {
            session.close();
            return Err(Error::Invalid("query view needs 1..4096 rows and items"));
        }
        let generation = app.doc.generation();
        let snapshot = session.info().snapshot;
        let charge = match budget.reserve(
            max_rows
                .checked_mul(
                    std::mem::size_of::<Option<Binding>>() + std::mem::size_of::<(Row, Demand)>(),
                )
                .ok_or(Error::ResourceLimit)?,
        ) {
            Ok(charge) => charge,
            Err(error) => {
                session.close();
                return Err(error);
            }
        };
        let mut bindings = Vec::new();
        let mut intents = Vec::new();
        if bindings.try_reserve_exact(max_rows).is_err()
            || intents.try_reserve_exact(max_rows).is_err()
        {
            session.close();
            return Err(Error::ResourceLimit);
        }
        bindings.resize_with(max_rows, || None);
        let demands = WaveDemands::new(session, limits, max_items, max_rows, 4, budget)?;
        let cursor = crate::query_cursor::CursorQueries::new(max_rows, limits, budget)?;
        if !app.doc.bind_query_session(generation, snapshot) {
            return Err(Error::Invalid(
                "query session does not belong to the active document",
            ));
        }
        Ok(Self {
            metadata: crate::query_metadata::MetadataQueries::new(limits, budget),
            navigation: crate::query_navigation::Navigation::new(limits),
            demands: Some(demands),
            cursor,
            generation,
            snapshot,
            bindings,
            intents,
            max_items,
            _charge: charge,
        })
    }
    pub fn is_closed(&self) -> bool {
        self.demands.is_none()
    }
    fn synchronize(&mut self, app: &mut App) -> Result<()> {
        self.intents.clear();
        for panel in app.panels.layout().visible() {
            let Some(waves) = app.panels.waves(panel) else {
                continue;
            };
            let layout = waves.last_layout();
            let Some((interval, _)) = waves.viewport(&app.doc).integer_projection(layout.waves)
            else {
                continue;
            };
            let columns = (layout.waves.width().ceil() as u32).clamp(1, self.max_items);
            let Some(grid) = Grid::covering(interval, columns)? else {
                continue;
            };
            for index in layout.rows.clone() {
                let Some(signal) = waves.items.get(index).and_then(|row| row.source.signal())
                else {
                    continue;
                };
                if self.intents.len() == self.bindings.len() {
                    return Err(Error::ResourceLimit);
                }
                self.intents.push((
                    Row {
                        panel,
                        index,
                        signal: signal.0,
                    },
                    Demand::Summary {
                        signal: signal.0,
                        grid,
                    },
                ));
            }
        }
        let demands = self.demands.as_mut().unwrap();
        for (slot, binding) in self.bindings.iter_mut().enumerate() {
            if binding
                .as_ref()
                .is_some_and(|binding| !self.intents.iter().any(|(row, _)| *row == binding.row))
            {
                let old = binding.take().unwrap().row;
                demands.set(slot, None)?;
                if let Some(item) = app
                    .panels
                    .waves_mut(old.panel)
                    .and_then(|waves| waves.items.get_mut(old.index))
                    && item
                        .source
                        .signal()
                        .is_some_and(|signal| signal.0 == old.signal)
                    && item
                        .query
                        .as_ref()
                        .is_some_and(|data| data.snapshot() == self.snapshot)
                {
                    item.query = None;
                }
            }
        }
        for &(row, demand) in &self.intents {
            let slot = self
                .bindings
                .iter()
                .position(|binding| binding.as_ref().is_some_and(|binding| binding.row == row))
                .or_else(|| self.bindings.iter().position(Option::is_none))
                .ok_or(Error::ResourceLimit)?;
            if self.bindings[slot].is_none() {
                self.bindings[slot] = Some(Binding {
                    row,
                    revision: None,
                });
            }
            demands.set(slot, Some(demand))?;
        }
        Ok(())
    }
    pub fn poll(&mut self, app: &mut App, cx: &mut Context<'_>) -> Poll<Result<Progress>> {
        if app.doc.query_snapshot() != Some(self.snapshot) {
            self.demands = None; // a different recording closes the old worker
            self.metadata.stop();
            self.navigation.stop();
            self.cursor.stop();
        } else if self.generation != app.doc.generation() {
            // A workspace restore changes row identity without changing the trace.
            self.generation = app.doc.generation();
            for binding in self.bindings.iter_mut().flatten() {
                binding.revision = None;
            }
        }
        if self.demands.is_none() {
            return Poll::Ready(Ok(Progress::Idle));
        }
        if let Err(error) = self.synchronize(app) {
            return Poll::Ready(Err(error));
        }
        let demands = self.demands.as_mut().unwrap();
        let navigation = self.navigation.poll(demands.session(), app, cx);
        let cursor = self.cursor.poll(demands.session(), app, cx);
        let metadata = self.metadata.poll(demands.session(), app, cx);
        let progress = demands.poll(cx);
        let mut changed = false;
        for (slot, binding) in self.bindings.iter_mut().enumerate() {
            let Some(binding) = binding else {
                continue;
            };
            let state = demands.slot(slot).unwrap();
            let revision = state.revision();
            if binding.revision == Some(revision)
                && app
                    .panels
                    .waves(binding.row.panel)
                    .and_then(|waves| waves.items.get(binding.row.index))
                    .is_some_and(|item| item.query.is_some())
            {
                continue;
            }
            let data = match demands.snapshot(slot) {
                Ok(data) => data,
                Err(error) => return Poll::Ready(Err(error)),
            };
            if let Some(waves) = app.panels.waves_mut(binding.row.panel) {
                if let Some(data) = data {
                    changed |=
                        waves.install_query(&app.doc, self.generation, binding.row.index, data);
                }
                if let Some(item) = waves.items.get_mut(binding.row.index) {
                    let error = state.error().map(ToString::to_string);
                    changed |= item.error != error;
                    item.error = error;
                }
            }
            binding.revision = Some(revision);
        }
        if changed {
            app.changed();
        }
        let mut advanced = changed;
        let mut pending = false;
        let mut backpressure = false;
        for result in [navigation, cursor, metadata, progress.map(Ok)] {
            match result {
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(Progress::Advanced)) => advanced = true,
                Poll::Ready(Ok(Progress::Backpressure)) => backpressure = true,
                Poll::Pending => pending = true,
                Poll::Ready(Ok(Progress::Idle)) => {}
            }
        }
        if advanced {
            Poll::Ready(Ok(Progress::Advanced))
        } else if pending {
            Poll::Pending
        } else if backpressure {
            Poll::Ready(Ok(Progress::Backpressure))
        } else {
            Poll::Ready(Ok(Progress::Idle))
        }
    }
}
