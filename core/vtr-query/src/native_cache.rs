//! Per-reader warm histories. Only active consumers keep construction alive.
use crate::{
    native_history::{Build, History},
    Budget, Cancellation, Error, Reservation, Result,
};
use std::sync::Arc;

enum State<'a> {
    Building(Build<'a>),
    Ready(Arc<History>),
    Refused,
}

struct Entry<'a> {
    signal: u32,
    users: usize,
    touched: u64,
    state: State<'a>,
}
pub(crate) enum Lookup {
    Pending,
    Ready(Arc<History>),
    Cold,
}
pub(crate) struct Cache<'a> {
    reader: &'a vtr::Reader,
    budget: Budget,
    entries: Vec<Entry<'a>>,
    capacity: usize,
    history_bytes: usize,
    clock: u64,
    _charge: Reservation,
}
impl<'a> Cache<'a> {
    pub(crate) fn new(reader: &'a vtr::Reader, parent: &Budget) -> Result<Self> {
        // Reserve a separate quota so histories cannot consume reply headroom.
        let bytes = parent.limit().saturating_sub(parent.used()) / 2;
        let budget = parent.child(bytes)?;
        // Concurrency bounds active work, not the reusable working set. A
        // visible set larger than the operation count must remain warm across
        // pans. Bound entry storage by this cache's byte quota and the number
        // of canonical signals in the immutable reader.
        let capacity =
            (reader.signal_count() as usize).min(bytes / (std::mem::size_of::<Entry<'a>>() * 4));
        let charge = budget.reserve(capacity * std::mem::size_of::<Entry<'a>>())?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        if entries.capacity() != capacity {
            return Err(Error::ResourceLimit);
        }
        let history_bytes = (bytes.saturating_sub(charge.bytes()) / 2)
            .saturating_sub(std::mem::size_of::<History>())
            .min(64 << 20);
        Ok(Self {
            reader,
            budget,
            entries,
            capacity,
            history_bytes,
            clock: 0,
            _charge: charge,
        })
    }
    fn tick(&mut self) -> u64 {
        // Saturation preserves safety; ties may change which idle entry is evicted.
        self.clock = self.clock.saturating_add(1);
        self.clock
    }
    /// Pin an already-built history without starting or waiting for construction.
    /// Point and edge requests stay cheap on a cold signal; summary demand owns
    /// warming. The returned owner retains admission even if its entry is evicted.
    pub(crate) fn ready(&mut self, signal: u32) -> Option<Arc<History>> {
        let touched = self.tick();
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.signal == signal)?;
        let State::Ready(history) = &entry.state else {
            return None;
        };
        entry.touched = touched;
        Some(history.clone())
    }
    fn evict_idle(&mut self) -> bool {
        let index = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.users == 0)
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(index, _)| index);
        if let Some(index) = index {
            self.entries.swap_remove(index);
            true
        } else {
            false
        }
    }
    /// True grants a consumer lease. False selects cold execution for this query.
    pub(crate) fn acquire(&mut self, signal: u32) -> Result<bool> {
        let touched = self.tick();
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.signal == signal) {
            entry.users = entry.users.checked_add(1).ok_or(Error::ResourceLimit)?;
            entry.touched = touched;
            return Ok(true);
        }
        if self.capacity == 0 {
            return Ok(false);
        }
        if self.entries.len() == self.capacity && !self.evict_idle() {
            return Ok(false);
        }
        let build = loop {
            match Build::new(
                self.reader,
                signal,
                self.history_bytes,
                &self.budget,
                Cancellation::default(),
            ) {
                Ok(build) => break build,
                Err(Error::ResourceLimit) if self.evict_idle() => {}
                Err(Error::ResourceLimit) => return Ok(false),
                Err(error) => return Err(error),
            }
        };
        self.entries.push(Entry {
            signal,
            users: 1,
            touched,
            state: State::Building(build),
        });
        Ok(true)
    }
    pub(crate) fn advance(&mut self, signal: u32) -> Result<Lookup> {
        let touched = self.tick();
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.signal == signal)
            .ok_or(Error::Invalid("missing history lease"))?;
        entry.touched = touched;
        if let State::Building(build) = &mut entry.state {
            // One block per call keeps construction cooperative. All consumers
            // of this signal advance the same cursor rather than duplicate work.
            match build.advance(1) {
                Ok(Some(history)) => {
                    entry.state = State::Ready(history);
                    return Ok(Lookup::Pending);
                }
                Ok(None) => return Ok(Lookup::Pending),
                Err(Error::ResourceLimit) => entry.state = State::Refused,
                Err(error) => return Err(error),
            }
        }
        Ok(match &entry.state {
            State::Ready(history) => Lookup::Ready(history.clone()),
            State::Refused => Lookup::Cold,
            State::Building(_) => unreachable!(),
        })
    }
    pub(crate) fn release(&mut self, signal: u32) {
        if let Some(index) = self.entries.iter().position(|entry| entry.signal == signal) {
            let entry = &mut self.entries[index];
            entry.users -= 1;
            if entry.users == 0 && matches!(entry.state, State::Building(_)) {
                self.entries.swap_remove(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscribers_share_builds_and_idle_eviction_respects_pins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.vtr");
        let mut writer = vtr::Writer::create_with(
            &path,
            vtr::WriterOptions {
                block_records: 8,
                background: false,
                dedup: false,
                ..Default::default()
            },
        )
        .unwrap();
        let signals: Vec<_> = (0..6)
            .map(|i| writer.add_bits(&format!("s{i}"), 1, 2).1 .0)
            .collect();
        for time in 0..32 {
            writer.set_time(time).unwrap();
            for &signal in &signals {
                writer.emit_u64(vtr::SignalId(signal), time % 2).unwrap();
            }
        }
        writer.close().unwrap();
        let reader = vtr::Reader::open(path).unwrap();
        let parent = Budget::new(65536);
        let mut cache = Cache::new(&reader, &parent).unwrap();
        // Isolate entry-capacity eviction from construction-byte pressure.
        cache.capacity = 4;
        cache.history_bytes = 2048;
        assert!(cache.acquire(signals[0]).unwrap());
        assert!(cache.acquire(signals[0]).unwrap());
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.entries[0].users, 2);
        assert!(matches!(
            cache.advance(signals[0]).unwrap(),
            Lookup::Pending
        ));
        cache.release(signals[0]);
        assert_eq!(cache.entries[0].users, 1);
        let history = loop {
            match cache.advance(signals[0]).unwrap() {
                Lookup::Pending => {}
                Lookup::Ready(history) => break history,
                Lookup::Cold => panic!("small history refused"),
            }
        };
        let Lookup::Ready(same) = cache.advance(signals[0]).unwrap() else {
            panic!("cache miss")
        };
        assert!(Arc::ptr_eq(&history, &same));
        let edge_budget = Budget::new(4096);
        let mut edge = crate::native_navigation::FindChange::new(
            &reader,
            signals[0],
            7,
            crate::wave::Direction::Next,
            crate::wave::Limits {
                bytes: 4096,
                records: 1,
                work: 1,
            },
            edge_budget.clone(),
            Cancellation::default(),
        )
        .unwrap()
        .with_history(cache.ready(signals[0]).unwrap());
        for &signal in &signals[1..4] {
            assert!(cache.acquire(signal).unwrap());
        }
        assert!(
            !cache.acquire(signals[4]).unwrap(),
            "active entries cannot be evicted"
        );
        cache.release(signals[0]);
        assert!(cache.acquire(signals[4]).unwrap());
        assert!(!cache.entries.iter().any(|entry| entry.signal == signals[0]));
        assert_eq!(
            history.index().find_change(7, crate::wave::Direction::Next),
            Some(8)
        );
        cache.release(signals[4]);
        assert!(
            !cache.entries.iter().any(|entry| entry.signal == signals[4]),
            "abandoned construction releases its slot"
        );
        drop(cache);
        assert!(
            parent.used() > 0,
            "evicted but externally pinned history retains admission"
        );
        drop(history);
        drop(same);
        assert!(
            parent.used() > 0,
            "navigation keeps its evicted history pinned"
        );
        let pressure = edge_budget.reserve(edge_budget.limit()).unwrap();
        assert!(matches!(edge.next_page(), Err(Error::ResourceLimit)));
        assert!(
            parent.used() > 0,
            "refused reply must retain the query's history"
        );
        drop(pressure);
        let result = edge.next_page().unwrap();
        assert_eq!(result.result, crate::wave::ChangeSearchResult::Found(8));
        assert_eq!(parent.used(), 0);
        let mut refused = Cache::new(&reader, &parent).unwrap();
        refused.history_bytes = 200;
        assert!(refused.acquire(signals[0]).unwrap());
        while matches!(refused.advance(signals[0]).unwrap(), Lookup::Pending) {}
        refused.release(signals[0]);
        assert!(refused.acquire(signals[0]).unwrap());
        assert!(matches!(refused.advance(signals[0]).unwrap(), Lookup::Cold));
        refused.release(signals[0]);
    }
}
