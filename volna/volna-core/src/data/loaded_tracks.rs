//! Complete immutable transaction data. Local and remote loaders publish the
//! same objects; interval queries below never access a reader or transport.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::transactions::{Relation, TrackRef, Transaction, TransactionRef};

/// A reference remains useful when its generator has not been loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransactionLocation {
    pub transaction: TransactionRef,
    pub generator: TrackRef,
}

/// Identity is the relation's ordinal in the immutable recording, not a hash
/// of its contents: identical parallel edges are distinct relations.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoadedRelation {
    pub id: u64,
    pub from_generator: TrackRef,
    pub to_generator: TrackRef,
    pub relation: Relation,
}

/// Complete records of one generator. A loaded stream retains these same
/// objects, so a generator displayed separately need not copy its records.
#[derive(Debug)]
pub struct LoadedGenerator {
    pub(crate) reservation: Option<crate::remote::memory::Reservation>,
    generator: TrackRef,
    transactions: Vec<Transaction>,
    by_id: HashMap<TransactionRef, usize>,
    parents: HashMap<TransactionRef, TransactionLocation>,
    relations: Vec<LoadedRelation>,
    // Balanced max-end tree over records ordered by begin time. Unlike a
    // begin-time binary search, this retains long overlapping transactions.
    max_end: Vec<u64>,
    leaves: usize,
}

impl LoadedGenerator {
    /// Validate and index a complete payload before publishing it. Parent
    /// locations are keyed by child ID; every parent reference must resolve
    /// to a generator, even if that generator is not part of this load.
    pub fn new(
        generator: TrackRef,
        mut transactions: Vec<Transaction>,
        parents: HashMap<TransactionRef, TransactionLocation>,
        relations: Vec<LoadedRelation>,
    ) -> anyhow::Result<Self> {
        transactions.sort_by_key(|tx| (tx.begin, tx.end, tx.id.0));
        let mut build = std::pin::pin!(Self::from_sorted(
            generator,
            transactions,
            parents,
            relations,
            || std::future::ready(()),
        ));
        match std::future::Future::poll(
            build.as_mut(),
            &mut std::task::Context::from_waker(std::task::Waker::noop()),
        ) {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => unreachable!("local index construction never yields"),
        }
    }

    /// The server sends records in canonical begin/end/ID order. Validate that
    /// order and build client indexes in steps, avoiding another large sort.
    pub(crate) async fn from_sorted<F: std::future::Future<Output = ()>>(
        generator: TrackRef,
        transactions: Vec<Transaction>,
        parents: HashMap<TransactionRef, TransactionLocation>,
        relations: Vec<LoadedRelation>,
        mut checkpoint: impl FnMut() -> F,
    ) -> anyhow::Result<Self> {
        let mut by_id = HashMap::with_capacity(transactions.len());
        let mut previous = None;
        for (index, tx) in transactions.iter().enumerate() {
            checkpoint().await;
            let key = (tx.begin, tx.end, tx.id.0);
            anyhow::ensure!(
                previous.is_none_or(|last| last <= key),
                "unordered transaction payload"
            );
            previous = Some(key);
            anyhow::ensure!(
                tx.generator == generator,
                "transaction belongs to another generator"
            );
            anyhow::ensure!(tx.begin <= tx.end, "reversed transaction interval");
            anyhow::ensure!(
                by_id.insert(tx.id, index).is_none(),
                "duplicate transaction identity"
            );
            anyhow::ensure!(
                tx.parent == parents.get(&tx.id).map(|p| p.transaction),
                "parent location does not match transaction"
            );
        }
        for id in parents.keys() {
            checkpoint().await;
            anyhow::ensure!(by_id.contains_key(id), "unknown child in parent locations");
        }
        for parent in parents.values() {
            checkpoint().await;
            if parent.generator == generator {
                anyhow::ensure!(
                    by_id.contains_key(&parent.transaction),
                    "missing parent in complete generator"
                );
            }
        }
        let mut relation_ids = HashSet::with_capacity(relations.len());
        for edge in &relations {
            checkpoint().await;
            anyhow::ensure!(relation_ids.insert(edge.id), "duplicate relation identity");
            let from_here = edge.from_generator == generator;
            let to_here = edge.to_generator == generator;
            anyhow::ensure!(from_here || to_here, "relation does not touch generator");
            anyhow::ensure!(
                !from_here || by_id.contains_key(&edge.relation.from),
                "missing relation source"
            );
            anyhow::ensure!(
                !to_here || by_id.contains_key(&edge.relation.to),
                "missing relation target"
            );
        }
        let leaves = transactions
            .len()
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(|| anyhow::anyhow!("transaction index too large"))?;
        let tree_len = leaves
            .checked_mul(2)
            .ok_or_else(|| anyhow::anyhow!("transaction index too large"))?;
        let mut max_end = Vec::new();
        max_end.try_reserve_exact(tree_len)?;
        for _ in 0..tree_len {
            checkpoint().await;
            max_end.push(0);
        }
        for (i, tx) in transactions.iter().enumerate() {
            checkpoint().await;
            max_end[leaves + i] = tx.end;
        }
        for i in (1..leaves).rev() {
            checkpoint().await;
            max_end[i] = max_end[i * 2].max(max_end[i * 2 + 1]);
        }
        Ok(Self {
            reservation: None,
            generator,
            transactions,
            by_id,
            parents,
            relations,
            max_end,
            leaves,
        })
    }

    pub fn generator(&self) -> TrackRef {
        self.generator
    }
    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }
    pub fn relations(&self) -> &[LoadedRelation] {
        &self.relations
    }
    pub fn parent(&self, child: TransactionRef) -> Option<TransactionLocation> {
        self.parents.get(&child).copied()
    }
    pub fn transaction(&self, id: TransactionRef) -> Option<&Transaction> {
        self.by_id.get(&id).map(|&index| &self.transactions[index])
    }

    /// Inclusive overlap, including point events at either boundary. Returns
    /// false if the visitor stopped early. Results are in begin/end/ID order.
    pub fn visit_window(
        &self,
        start: u64,
        end: u64,
        mut visitor: impl FnMut(&Transaction) -> bool,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(start <= end, "transaction window is reversed");
        let limit = self.transactions.partition_point(|tx| tx.begin <= end);
        Ok(self.visit_node(1, 0, self.leaves, start, limit, &mut visitor))
    }

    fn visit_node(
        &self,
        node: usize,
        lo: usize,
        hi: usize,
        start: u64,
        limit: usize,
        visitor: &mut impl FnMut(&Transaction) -> bool,
    ) -> bool {
        if lo >= limit || self.max_end[node] < start {
            return true;
        }
        if hi - lo == 1 {
            return visitor(&self.transactions[lo]);
        }
        let mid = lo + (hi - lo) / 2;
        self.visit_node(node * 2, lo, mid, start, limit, visitor)
            && self.visit_node(node * 2 + 1, mid, hi, start, limit, visitor)
    }
}

/// An atomic successful load of a stream or generator, including empty
/// generators. Consumers retain these handles for as long as they need them.
#[derive(Clone, Debug)]
pub struct LoadedTrack {
    pub track: TrackRef,
    pub generators: Vec<Arc<LoadedGenerator>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::transactions::{TxKind, TxStatus};

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
    fn interval_index_matches_scan_including_long_and_point_records() {
        let mut records = vec![tx(0, 0, u64::MAX), tx(1, u64::MAX, u64::MAX)];
        for id in 2..202 {
            let begin = (id * 37) % 100;
            records.push(tx(id, begin, begin + (id * 19) % 50));
        }
        let loaded = LoadedGenerator::new(TrackRef(1), records, HashMap::new(), vec![]).unwrap();
        for start in 0..160 {
            for end in [start, start + 7, u64::MAX] {
                let expected: Vec<_> = loaded
                    .transactions()
                    .iter()
                    .filter(|tx| tx.begin <= end && tx.end >= start)
                    .map(|tx| tx.id)
                    .collect();
                let mut actual = vec![];
                assert!(
                    loaded
                        .visit_window(start, end, |tx| {
                            actual.push(tx.id);
                            true
                        })
                        .unwrap()
                );
                assert_eq!(actual, expected);
            }
        }
        let mut count = 0;
        assert!(
            !loaded
                .visit_window(0, u64::MAX, |_| {
                    count += 1;
                    false
                })
                .unwrap()
        );
        assert_eq!(count, 1);
        assert!(loaded.visit_window(2, 1, |_| true).is_err());
    }

    #[test]
    fn cooperative_index_yields_and_matches_local_queries() {
        use std::cell::Cell;
        use std::future::{Future, poll_fn};
        use std::task::{Context, Poll, Waker};
        let records: Vec<_> = (0..2000)
            .map(|id| tx(id, id, if id == 0 { 10000 } else { id + 3 }))
            .collect();
        let local =
            LoadedGenerator::new(TrackRef(1), records.clone(), HashMap::new(), vec![]).unwrap();
        let credits = Cell::new(0);
        let checkpoint = || {
            poll_fn(|_| {
                if credits.get() == 0 {
                    Poll::Pending
                } else {
                    credits.set(credits.get() - 1);
                    Poll::Ready(())
                }
            })
        };
        let mut build = std::pin::pin!(LoadedGenerator::from_sorted(
            TrackRef(1),
            records,
            HashMap::new(),
            vec![],
            checkpoint
        ));
        let mut polls = 0;
        let remote = loop {
            credits.set(32);
            polls += 1;
            match build.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Pending => assert_eq!(credits.get(), 0),
                Poll::Ready(result) => break result.unwrap(),
            }
        };
        assert!(polls > 100);
        assert_eq!(local.transactions(), remote.transactions());
        assert_eq!(local.by_id, remote.by_id);
        assert_eq!(local.max_end, remote.max_end);
        let mut overlap = Vec::new();
        remote
            .visit_window(1000, 1000, |tx| {
                overlap.push(tx.id);
                true
            })
            .unwrap();
        assert!(overlap.contains(&TransactionRef(0)));
        assert!(overlap.contains(&TransactionRef(1000)));

        let mut unordered = std::pin::pin!(LoadedGenerator::from_sorted(
            TrackRef(1),
            vec![tx(1, 2, 3), tx(2, 1, 2)],
            HashMap::new(),
            vec![],
            || std::future::ready(())
        ));
        assert!(matches!(
            unordered
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(_))
        ));
    }

    #[test]
    fn validates_complete_objects_and_keeps_parallel_relations() {
        let mut child = tx(2, 10, 10);
        child.parent = Some(TransactionRef(9));
        let parent = TransactionLocation {
            transaction: TransactionRef(9),
            generator: TrackRef(3),
        };
        let edge = LoadedRelation {
            id: 0,
            from_generator: TrackRef(3),
            to_generator: TrackRef(1),
            relation: Relation {
                from: parent.transaction,
                to: child.id,
                kind: "causes".into(),
                attributes: vec![],
            },
        };
        let parents = HashMap::from([(child.id, parent)]);
        assert!(
            LoadedGenerator::new(TrackRef(1), vec![child.clone()], HashMap::new(), vec![]).is_err()
        );
        assert!(
            LoadedGenerator::new(
                TrackRef(1),
                vec![child.clone(), child.clone()],
                parents.clone(),
                vec![]
            )
            .is_err()
        );
        assert!(
            LoadedGenerator::new(
                TrackRef(1),
                vec![child.clone()],
                parents.clone(),
                vec![edge.clone(), edge.clone()]
            )
            .is_err()
        );
        let mut parallel = edge.clone();
        parallel.id = 1;
        let loaded = Arc::new(
            LoadedGenerator::new(TrackRef(1), vec![child], parents, vec![edge, parallel]).unwrap(),
        );
        assert_eq!(loaded.relations().len(), 2);
        assert_eq!(loaded.parent(TransactionRef(2)), Some(parent));
        assert!(loaded.transaction(TransactionRef(9)).is_none());
        let stream = LoadedTrack {
            track: TrackRef(0),
            generators: vec![loaded.clone()],
        };
        let generator = LoadedTrack {
            track: TrackRef(1),
            generators: vec![loaded.clone()],
        };
        assert!(Arc::ptr_eq(&stream.generators[0], &generator.generators[0]));
        let weak = Arc::downgrade(&loaded);
        drop(loaded);
        drop(stream);
        drop(generator);
        assert!(weak.upgrade().is_none());
        let empty = LoadedGenerator::new(TrackRef(1), vec![], HashMap::new(), vec![]).unwrap();
        assert!(
            empty
                .visit_window(0, u64::MAX, |_| panic!("empty"))
                .unwrap()
        );
    }
}
