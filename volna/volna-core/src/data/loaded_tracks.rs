//! Viewer preparation of immutable raw transaction owners: stacking, palettes and placement.
use std::collections::HashMap;
use std::sync::Arc;
use volna_trace::data::loaded_tracks::{LoadedGenerator as RawGenerator, LoadedTrack as RawTrack};
use volna_trace::data::loaded_tracks::{LoadedRelation, TransactionLocation};
use volna_trace::data::transactions::{TrackRef, Transaction, TransactionRef, TransactionStage};
/// The stage names of a generator by lane, counted in one pass at load so a
/// stage palette (`pipeline::palette`) never scans records. Lanes and the
/// names within a lane keep their first appearance in record order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StageCensus {
    pub lanes: Vec<LaneCensus>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaneCensus {
    pub lane: String,
    /// Stages recorded on this lane.
    pub stages: u64,
    pub names: Vec<NameCensus>,
}

/// One stage name on one lane: how often it occurs and the sum of its
/// positions among a record's stages on that lane (first stage is 0).
#[derive(Clone, Debug, PartialEq)]
pub struct NameCensus {
    pub name: String,
    pub count: u64,
    pub position_sum: u64,
}

impl StageCensus {
    /// Count one record's stages. `cursors` is scratch space reused across
    /// records: per lane, the next position and the name after the last one
    /// matched. Stages mostly arrive in pipeline order, so that name and the
    /// previous stage's lane are tried before a search.
    async fn add<F: std::future::Future<Output = ()>>(
        &mut self,
        stages: &[TransactionStage],
        cursors: &mut Vec<(u64, usize)>,
        checkpoint: &mut impl FnMut() -> F,
    ) {
        cursors.iter_mut().for_each(|cursor| *cursor = (0, 0));
        let mut lane = 0;
        for stage in stages {
            checkpoint().await;
            if self.lanes.get(lane).is_none_or(|l| l.lane != stage.lane) {
                lane = match self.lanes.iter().position(|l| l.lane == stage.lane) {
                    Some(lane) => lane,
                    None => {
                        self.lanes.push(LaneCensus {
                            lane: stage.lane.clone(),
                            stages: 0,
                            names: Vec::new(),
                        });
                        cursors.push((0, 0));
                        self.lanes.len() - 1
                    }
                };
            }
            let census = &mut self.lanes[lane];
            census.stages += 1;
            let (position, hint) = cursors[lane];
            let name = if census.names.get(hint).is_some_and(|n| n.name == stage.name) {
                hint
            } else if let Some(name) = census.names.iter().position(|n| n.name == stage.name) {
                name
            } else {
                census.names.push(NameCensus {
                    name: stage.name.clone(),
                    count: 0,
                    position_sum: 0,
                });
                census.names.len() - 1
            };
            census.names[name].count += 1;
            census.names[name].position_sum += position;
            cursors[lane] = (position + 1, name + 1);
        }
    }

    fn bytes(&self) -> u64 {
        let lane = std::mem::size_of::<LaneCensus>() as u64;
        let name = std::mem::size_of::<NameCensus>() as u64;
        self.lanes.iter().fold(0, |bytes, l| {
            l.names.iter().fold(
                bytes.saturating_add(lane + l.lane.capacity() as u64),
                |bytes, n| bytes.saturating_add(name + n.name.capacity() as u64),
            )
        })
    }
}

#[derive(Debug)]
pub struct LoadedGenerator {
    raw: Arc<RawGenerator>,
    sub_rows: Vec<u16>,
    depth: u16,
    median_lifetime: u64,
    stage_census: StageCensus,
    _reservation: Option<volna_trace::remote::memory::Reservation>,
    _placed_reservation: Option<volna_trace::remote::memory::Reservation>,
}
impl std::ops::Deref for LoadedGenerator {
    type Target = RawGenerator;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
impl LoadedGenerator {
    /// Build client-side lane and palette indexes over shared raw records.
    pub fn prepare(
        raw: Arc<RawGenerator>,
        budget: Option<&volna_trace::remote::memory::MemoryBudget>,
    ) -> anyhow::Result<Self> {
        let mut prepare =
            std::pin::pin!(Self::prepare_with(raw, budget, || std::future::ready(())));
        match std::future::Future::poll(
            prepare.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        ) {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("blocking preparation never yields"),
        }
    }
    pub(crate) async fn prepare_with<F: std::future::Future<Output = ()>>(
        raw: Arc<RawGenerator>,
        budget: Option<&volna_trace::remote::memory::MemoryBudget>,
        mut checkpoint: impl FnMut() -> F,
    ) -> anyhow::Result<Self> {
        let transactions = raw.transactions();
        let mut upper = (transactions.len() as u64).saturating_mul(128);
        for tx in transactions {
            checkpoint().await;
            for stage in &tx.stages {
                checkpoint().await;
                upper = upper.saturating_add((stage.lane.len() + stage.name.len()) as u64 + 128);
            }
        }
        let mut reservation = budget
            .map(|b| b.reserve_object("the generator view", upper))
            .transpose()?;
        // Greedy interval partitioning in begin order over half-open
        // lifetimes: each record takes the lowest sub-row free at its begin.
        // A zero-length record holds its instant, so records at one time
        // stack instead of hiding each other.
        let mut sub_rows = Vec::new();
        sub_rows.try_reserve_exact(transactions.len())?;
        let mut open = std::collections::BinaryHeap::new();
        let mut free = std::collections::BinaryHeap::new();
        let mut depth = 0u16;
        for tx in transactions {
            checkpoint().await;
            while let Some(&std::cmp::Reverse((end, row))) = open.peek() {
                if end > tx.begin {
                    break;
                }
                open.pop();
                free.push(std::cmp::Reverse(row));
            }
            let row = match free.pop() {
                Some(std::cmp::Reverse(row)) => row,
                None => {
                    // Past u16::MAX sub-rows, the deepest one is shared.
                    let row = depth;
                    depth = depth.saturating_add(1);
                    row.min(u16::MAX - 1)
                }
            };
            sub_rows.push(row);
            open.push(std::cmp::Reverse((
                tx.end.max(tx.begin.saturating_add(1)),
                row,
            )));
        }
        let mut stage_census = StageCensus::default();
        let mut cursors = Vec::new();
        for tx in transactions {
            checkpoint().await;
            stage_census
                .add(&tx.stages, &mut cursors, &mut checkpoint)
                .await;
        }
        let median_lifetime = {
            let mut lifetimes = Vec::with_capacity(transactions.len());
            for tx in transactions {
                checkpoint().await;
                lifetimes.push(tx.end - tx.begin);
            }
            let mid = lifetimes.len() / 2;
            if lifetimes.is_empty() {
                0
            } else {
                *lifetimes.select_nth_unstable(mid).1
            }
        };

        let bytes =
            (sub_rows.capacity() * std::mem::size_of::<u16>()) as u64 + stage_census.bytes();
        if let Some(r) = &mut reservation {
            r.shrink(r.bytes().saturating_sub(bytes))?;
        }
        Ok(Self {
            raw,
            sub_rows,
            depth,
            median_lifetime,
            stage_census,
            _reservation: reservation,
            _placed_reservation: None,
        })
    }
    pub(crate) fn scale_times(&mut self, scale: u64) -> anyhow::Result<()> {
        let raw = &self.raw;
        let budget = self
            ._placed_reservation
            .as_ref()
            .map(|r| r.budget())
            .or_else(|| raw.memory_budget())
            .or_else(|| self._reservation.as_ref().map(|r| r.budget()));
        let reservation = budget
            .as_ref()
            .map(|b| b.reserve_object("the placed generator", raw.resident_bytes()))
            .transpose()?;
        let mut transactions = raw.transactions().to_vec();
        let mut relations = raw.relations().to_vec();
        let parents = transactions
            .iter()
            .filter_map(|tx| raw.parent(tx.id).map(|p| (tx.id, p)))
            .collect();
        let time = |t: &mut u64| *t = t.saturating_mul(scale);
        let attributes = |attrs: &mut volna_trace::data::transactions::Attributes| {
            for (key, value) in attrs {
                scale_attribute(key, value, scale);
            }
        };
        for tx in &mut transactions {
            time(&mut tx.begin);
            time(&mut tx.end);
            for a in &mut tx.attributes {
                scale_attribute(&a.key, &mut a.value, scale);
            }
            for event in &mut tx.events {
                time(&mut event.time);
                attributes(&mut event.attributes);
            }
            for stage in &mut tx.stages {
                time(&mut stage.begin);
                if let Some(end) = &mut stage.end {
                    time(end);
                }
                attributes(&mut stage.attributes);
            }
        }
        for relation in &mut relations {
            attributes(&mut relation.relation.attributes);
        }

        self.raw = Arc::new(RawGenerator::new(
            raw.generator(),
            transactions,
            parents,
            relations,
        )?);
        self.median_lifetime = self.median_lifetime.saturating_mul(scale);
        self._placed_reservation = reservation;
        Ok(())
    }
    /// Construct a prepared owner for headless viewer fixtures.
    pub fn new(
        generator: TrackRef,
        transactions: Vec<Transaction>,
        parents: HashMap<TransactionRef, TransactionLocation>,
        relations: Vec<LoadedRelation>,
    ) -> anyhow::Result<Self> {
        Self::prepare(
            Arc::new(RawGenerator::new(
                generator,
                transactions,
                parents,
                relations,
            )?),
            None,
        )
    }
    /// The sub-row of the record at canonical `ordinal` when overlapping
    /// records stack: no two records on one sub-row overlap (a record may
    /// begin where the previous one ends).
    pub fn sub_row(&self, ordinal: usize) -> u16 {
        self.sub_rows.get(ordinal).copied().unwrap_or(0)
    }

    /// Sub-rows the stacking uses; 0 for an empty generator.
    pub fn depth(&self) -> u16 {
        self.depth
    }

    /// Median record lifetime, in trace time units.
    pub fn median_lifetime(&self) -> u64 {
        self.median_lifetime
    }

    /// Stage names per lane, counted at load.
    pub fn stage_census(&self) -> &StageCensus {
        &self.stage_census
    }
}
#[derive(Clone, Debug)]
pub struct LoadedTrack {
    pub track: TrackRef,
    pub generators: Vec<Arc<LoadedGenerator>>,
}
impl LoadedTrack {
    /// Prepare a track on a blocking loader executor.
    pub fn prepare(
        raw: RawTrack,
        budget: Option<&volna_trace::remote::memory::MemoryBudget>,
    ) -> anyhow::Result<Self> {
        let mut prepare =
            std::pin::pin!(Self::prepare_with(raw, budget, || std::future::ready(())));
        match std::future::Future::poll(
            prepare.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        ) {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("blocking preparation never yields"),
        }
    }
    pub(crate) async fn prepare_with<F: std::future::Future<Output = ()>>(
        raw: RawTrack,
        budget: Option<&volna_trace::remote::memory::MemoryBudget>,
        mut checkpoint: impl FnMut() -> F,
    ) -> anyhow::Result<Self> {
        let mut generators = Vec::with_capacity(raw.generators.len());
        for g in raw.generators {
            generators.push(Arc::new(
                LoadedGenerator::prepare_with(g, budget, &mut checkpoint).await?,
            ));
        }
        Ok(Self {
            track: raw.track,
            generators,
        })
    }
}
/// Scale a time attribute. A clock's period may be written as a plain
/// integer ([`crate::clock::PERIOD_ATTRIBUTE`]); it is a time all the same.
fn scale_attribute(
    key: &str,
    value: &mut volna_trace::data::transactions::AttributeValue,
    scale: u64,
) {
    use volna_trace::data::transactions::AttributeValue;
    match value {
        AttributeValue::Time(t) => *t = t.saturating_mul(scale),
        AttributeValue::U64(t) if key == crate::clock::PERIOD_ATTRIBUTE => {
            *t = t.saturating_mul(scale)
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use volna_trace::data::transactions::{TxKind, TxStatus};
    fn tx(id: u64, begin: u64, end: u64) -> Transaction {
        Transaction {
            id: TransactionRef(id),
            generator: TrackRef(1),
            begin,
            end,
            status: TxStatus::Ok,
            kind: TxKind::Producer,
            parent: None,
            attributes: vec![],
            events: vec![],
            stages: vec![],
        }
    }

    #[test]
    fn stage_census_counts_lanes_names_and_positions_in_any_order() {
        let stage = |name: &str, lane: &str| TransactionStage {
            name: name.into(),
            lane: lane.into(),
            begin: 0,
            end: Some(1),
            attributes: vec![],
        };
        let mut a = tx(0, 0, 4);
        a.stages = vec![
            stage("F", ""),
            stage("s", "x"),
            stage("E", ""),
            stage("W", ""),
        ];
        // Out of pipeline order, so the next-name guess misses.
        let mut b = tx(1, 1, 5);
        b.stages = vec![stage("F", ""), stage("W", ""), stage("E", "")];
        let loaded = LoadedGenerator::new(TrackRef(1), vec![a, b], HashMap::new(), vec![]).unwrap();
        let census = loaded.stage_census();
        let lanes: Vec<_> = census
            .lanes
            .iter()
            .map(|l| (l.lane.as_str(), l.stages))
            .collect();
        assert_eq!(lanes, [("", 6), ("x", 1)]);
        let names: Vec<_> = census.lanes[0]
            .names
            .iter()
            .map(|n| (n.name.as_str(), n.count, n.position_sum))
            .collect();
        assert_eq!(names, [("F", 2, 0), ("E", 2, 3), ("W", 2, 3)]);
        assert_eq!(census.lanes[1].names[0].position_sum, 0);
    }

    #[test]
    fn records_at_one_instant_stack() {
        let records = vec![tx(1, 5, 5), tx(2, 5, 5), tx(3, 5, 9), tx(4, 6, 6)];
        let loaded = LoadedGenerator::new(TrackRef(1), records, HashMap::new(), vec![]).unwrap();
        let rows: Vec<u16> = (0..4).map(|i| loaded.sub_row(i)).collect();
        assert_eq!(rows, [0, 1, 2, 0]);
    }

    #[test]
    fn stacking_boundaries_and_median_match_a_scan() {
        let mut records = vec![tx(0, 0, 400)];
        for id in 1..300 {
            let begin = (id * 37) % 250;
            records.push(tx(id, begin, begin + (id * 13) % 40));
        }
        let loaded = LoadedGenerator::new(TrackRef(1), records, HashMap::new(), vec![]).unwrap();
        let txs = loaded.transactions();
        // No two records on one sub-row overlap, and the depth is the
        // deepest overlap of half-open lifetimes.
        let mut deepest = 0;
        for (i, a) in txs.iter().enumerate() {
            let open = txs
                .iter()
                .filter(|b| b.begin <= a.begin && a.begin < b.end.max(b.begin + 1))
                .count();
            deepest = deepest.max(open);
            for (j, b) in txs.iter().enumerate().skip(i + 1) {
                if loaded.sub_row(i) == loaded.sub_row(j) {
                    // A zero-length record holds its instant.
                    let end = |t: &Transaction| t.end.max(t.begin + 1);
                    assert!(end(a) <= b.begin || end(b) <= a.begin, "{a:?} {b:?}");
                }
            }
        }
        assert!(usize::from(loaded.depth()) <= deepest);
        assert!(
            txs.iter()
                .enumerate()
                .all(|(i, _)| loaded.sub_row(i) < loaded.depth())
        );
        let mut lifetimes: Vec<_> = txs.iter().map(|t| t.end - t.begin).collect();
        lifetimes.sort();
        assert_eq!(loaded.median_lifetime(), lifetimes[lifetimes.len() / 2]);
        let mut bounds: Vec<u64> = txs.iter().flat_map(|t| [t.begin, t.end]).collect();
        bounds.sort();
        bounds.dedup();
        for time in 0..420 {
            assert_eq!(
                loaded.next_boundary(time),
                bounds.iter().copied().find(|b| *b > time),
                "next after {time}"
            );
            assert_eq!(
                loaded.prev_boundary(time),
                bounds.iter().copied().rev().find(|b| *b < time),
                "prev before {time}"
            );
        }
        let mut ordinals = vec![];
        loaded
            .visit_window_ordinals(100, 120, |ordinal, tx| {
                assert_eq!(&txs[ordinal], tx);
                ordinals.push(ordinal);
                true
            })
            .unwrap();
        assert!(!ordinals.is_empty());
        let empty = LoadedGenerator::new(TrackRef(1), vec![], HashMap::new(), vec![]).unwrap();
        assert_eq!((empty.depth(), empty.median_lifetime()), (0, 0));
        assert_eq!(empty.next_boundary(0), None);
        assert_eq!(empty.prev_boundary(5), None);
    }
    #[test]
    fn preparation_yields_shares_raw_storage_and_charges_only_its_owners() {
        use std::future::{Future, poll_fn};
        use std::task::{Context, Poll, Waker};
        let raw = Arc::new(
            RawGenerator::new(
                TrackRef(1),
                (0..5000).map(|id| tx(id, id * 10, id * 10 + 5)).collect(),
                HashMap::new(),
                vec![],
            )
            .unwrap(),
        );
        let budget = volna_trace::remote::memory::MemoryBudget::new(4 << 20);
        let credits = std::cell::Cell::new(0);
        let checkpoint = || {
            poll_fn(|_| {
                if credits.get() == 0 {
                    credits.set(128);
                    Poll::Pending
                } else {
                    credits.set(credits.get() - 1);
                    Poll::Ready(())
                }
            })
        };
        let mut work = std::pin::pin!(LoadedGenerator::prepare_with(
            raw.clone(),
            Some(&budget),
            checkpoint
        ));
        let mut polls = 0;
        let mut prepared = loop {
            polls += 1;
            match work.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Pending => {}
                Poll::Ready(result) => break result.unwrap(),
            }
        };
        assert!(polls > 100, "viewer analysis stays cooperative");
        assert!(
            Arc::ptr_eq(&prepared.raw, &raw),
            "identity placement shares raw records"
        );
        assert!(budget.used() > 0);
        let original = raw.transactions().to_vec();
        prepared.scale_times(1000).unwrap();
        assert_eq!(raw.transactions(), original);
        assert_eq!(prepared.transactions()[3].begin, 30_000);
        assert_eq!(prepared.next_boundary(30_000), Some(35_000));
        assert_eq!(prepared.median_lifetime(), 5000);
        drop(prepared);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn refused_or_dropped_preparation_releases_private_admission() {
        use std::future::{Future, poll_fn};
        use std::task::{Context, Poll, Waker};
        let raw = Arc::new(
            RawGenerator::new(
                TrackRef(1),
                (0..5000).map(|id| tx(id, id, id + 5)).collect(),
                HashMap::new(),
                vec![],
            )
            .unwrap(),
        );
        let budget = volna_trace::remote::memory::MemoryBudget::new(0);
        assert!(LoadedGenerator::prepare(raw.clone(), Some(&budget)).is_err());
        assert_eq!(budget.used(), 0);
        budget.set_limit(4 << 20);
        let steps = std::cell::Cell::new(0);
        let checkpoint = || {
            poll_fn(|_| {
                steps.set(steps.get() + 1);
                if steps.get() > 5100 {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
        };
        let mut work = Box::pin(LoadedGenerator::prepare_with(
            raw,
            Some(&budget),
            checkpoint,
        ));
        assert!(matches!(
            work.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        assert!(
            budget.used() > 0,
            "private preparation was admitted before allocation"
        );
        drop(work);
        assert_eq!(budget.used(), 0);
    }
}
