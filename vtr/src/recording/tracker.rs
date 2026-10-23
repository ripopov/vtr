//! Keyed transaction lifetimes with reusable storage and independent stage lanes.

use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{OwnerRef, Recorder, Source, Status, Transaction, Writer};
use crate::{Attribute, Error, RecordingId, Result, Value};

static NEXT_TRACKER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyspace {
    tracker: u64,
    index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackerDiagnostic {
    pub count: u64,
    pub first: Option<String>,
}

#[derive(Default)]
struct Lane {
    name: u32,
    stage: u32,
    visit: Option<u64>,
}

#[derive(Default)]
struct Item {
    record: Option<Transaction>,
    names: Vec<(usize, u64)>,
    lanes: Vec<Lane>,
    attributes: Vec<Attribute>,
}

impl Item {
    fn record(&self) -> Transaction {
        self.record.expect("tracker item is active")
    }
}

struct Space {
    name: String,
    keys: HashMap<u64, usize>,
    groups: Vec<Vec<usize>>,
    free_groups: Vec<usize>,
    order: Vec<usize>,
}

impl Space {
    fn members(&self, key: u64) -> Option<&[usize]> {
        self.keys
            .get(&key)
            .map(|&group| self.groups[group].as_slice())
    }

    fn group(&mut self, key: u64) -> &mut Vec<usize> {
        let group = *self.keys.entry(key).or_insert_with(|| {
            self.free_groups.pop().unwrap_or_else(|| {
                self.groups.push(Vec::new());
                self.groups.len() - 1
            })
        });
        &mut self.groups[group]
    }

    fn remove(&mut self, key: u64, item: usize) {
        let group = self.keys[&key];
        self.groups[group].retain(|&current| current != item);
        if self.groups[group].is_empty() {
            self.keys.remove(&key);
            self.free_groups.push(group);
        }
        self.order.retain(|&current| current != item);
    }
}

/// One logical producer's keyed owners. Advance writer time explicitly.
/// Close, abort, or detach live items before dropping the tracker to publish
/// their pending attributes. Dropping a tracker does not end its records.
pub struct Tracker {
    id: u64,
    origin: RecordingId,
    source: Source<()>,
    name: String,
    label: u32,
    empty: u32,
    spaces: Vec<Space>,
    pool: Vec<Item>,
    free: Vec<usize>,
    open: Vec<usize>,
    strings: Vec<Option<(String, u32)>>,
    diagnostics: BTreeMap<String, TrackerDiagnostic>,
}

impl Tracker {
    /// Track an ordinary unit-payload source.
    ///
    /// ```compile_fail
    /// use vtr::{SourceOptions, Tracker, Writer};
    /// fn wrong_source(writer: &mut Writer) -> vtr::Result<()> {
    ///     let events = writer.events::<()>(SourceOptions::new("events", None))?;
    ///     let tracker = Tracker::new(writer, events, "events")?;
    ///     Ok(())
    /// }
    /// ```
    pub fn new(writer: &mut Writer, source: Source<()>, name: impl Into<String>) -> Result<Self> {
        let _guard = writer.crash.guard();
        writer.front.check(source.origin)?;
        writer.usable()?;
        let id = NEXT_TRACKER
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| Error::invalid("tracker identifiers exhausted"))?;
        Ok(Self {
            id,
            origin: writer.identity(),
            source,
            name: name.into(),
            label: writer.intern("vtr.label")?,
            empty: writer.intern("")?,
            spaces: Vec::new(),
            pool: Vec::new(),
            free: Vec::new(),
            open: Vec::new(),
            strings: vec![None; 512],
            diagnostics: BTreeMap::new(),
        })
    }

    pub fn keyspace(&mut self, name: impl Into<String>) -> Result<Keyspace> {
        if self.spaces.len() == u32::MAX as usize {
            return Err(Error::invalid("tracker key spaces exhausted"));
        }
        let index = self.spaces.len();
        self.spaces.push(Space {
            name: name.into(),
            keys: HashMap::new(),
            groups: Vec::new(),
            free_groups: Vec::new(),
            order: Vec::new(),
        });
        Ok(Keyspace {
            tracker: self.id,
            index,
        })
    }

    #[must_use]
    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    #[must_use]
    pub fn keyspace_count(&self) -> usize {
        self.spaces.len()
    }

    fn check(&self, writer: &impl Recorder) -> Result<()> {
        writer.state().check(self.origin)?;
        if writer.state().finished {
            return Err(Error::State("recording is finalized"));
        }
        Ok(())
    }

    pub fn open(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        stage: &str,
        time: u64,
        parent: Option<OwnerRef>,
    ) -> Result<Option<u64>> {
        self.check(writer)?;
        if !self.valid(space, "open") {
            return Ok(None);
        }
        // The replaced owner can also be the new owner's parent.
        let record = writer.begin_at(self.source, time, &(), parent)?;
        if let Some(previous) = self.spaces[space.index].members(key).map(<[usize]>::to_vec) {
            self.diag(
                "open on a key that still names an item; that item is aborted",
                Some((space.index, key)),
            );
            self.end_items(writer, &previous, Status::Aborted, time)?;
        }
        let item = self.free.pop().unwrap_or_else(|| {
            self.pool.push(Item::default());
            self.pool.len() - 1
        });
        self.pool[item].record = Some(record);
        self.open.push(item);
        let stage = self.intern(writer, stage)?;
        self.enter(writer, item, self.empty, stage, time)?;
        self.name(item, space.index, key);
        Ok(Some(record.id()))
    }

    pub fn owner(&mut self, space: Keyspace, key: u64) -> Option<OwnerRef> {
        self.lookup(space, key, "owner").map(|group| {
            let item = self.spaces[space.index].groups[group][0];
            self.pool[item].record().owner()
        })
    }

    pub fn records(&mut self, space: Keyspace, key: u64) -> Vec<u64> {
        self.lookup(space, key, "records")
            .map_or_else(Vec::new, |group| {
                self.spaces[space.index].groups[group]
                    .iter()
                    .map(|&item| self.pool[item].record().id())
                    .collect()
            })
    }

    pub fn bind(
        &mut self,
        writer: &mut impl Recorder,
        from: Keyspace,
        key: u64,
        to: Keyspace,
        target: u64,
        time: u64,
    ) -> Result<()> {
        self.check(writer)?;
        if !self.valid(to, "bind") {
            return Ok(());
        }
        let Some(moved) = self
            .lookup(from, key, "bind")
            .map(|group| self.spaces[from.index].groups[group].clone())
        else {
            return Ok(());
        };
        self.evict(writer, to.index, target, &moved, time)?;
        for item in moved {
            self.name(item, to.index, target);
        }
        Ok(())
    }

    pub fn bind_oldest(
        &mut self,
        writer: &mut impl Recorder,
        from: Keyspace,
        count: usize,
        to: Keyspace,
        target: u64,
        time: u64,
    ) -> Result<usize> {
        self.check(writer)?;
        if !self.valid(from, "bind_oldest") || !self.valid(to, "bind_oldest") {
            return Ok(0);
        }
        let n = count.min(self.spaces[from.index].order.len());
        if n != count {
            self.diag(
                "bind_oldest found fewer items than requested",
                Some((to.index, target)),
            );
        }
        let moved = self.spaces[from.index].order[..n].to_vec();
        self.evict(writer, to.index, target, &moved, time)?;
        for item in moved {
            self.unname(item, from.index);
            self.name(item, to.index, target);
        }
        Ok(n)
    }

    pub fn stage(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        stage: &str,
        time: u64,
    ) -> Result<()> {
        self.lane(writer, space, key, "", stage, time)
    }

    pub fn lane(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        lane: &str,
        stage: &str,
        time: u64,
    ) -> Result<()> {
        self.check(writer)?;
        let Some(group) = self.lookup(space, key, "stage") else {
            return Ok(());
        };
        let lane = self.intern(writer, lane)?;
        let stage = self.intern(writer, stage)?;
        for index in 0..self.spaces[space.index].groups[group].len() {
            let item = self.spaces[space.index].groups[group][index];
            self.enter(writer, item, lane, stage, time)?;
        }
        Ok(())
    }

    pub fn event(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        name: &str,
        time: u64,
    ) -> Result<()> {
        self.check(writer)?;
        let Some(group) = self.lookup(space, key, "event") else {
            return Ok(());
        };
        let name = self.intern(writer, name)?;
        for &item in &self.spaces[space.index].groups[group] {
            writer.embedded_event(self.pool[item].record().owner(), time, name, vec![])?;
        }
        Ok(())
    }

    pub fn attribute(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        name: &str,
        value: Value,
    ) -> Result<()> {
        self.check(writer)?;
        let Some(group) = self.lookup(space, key, "attribute") else {
            return Ok(());
        };
        let name = self.intern(writer, name)?;
        Self::set_attribute(
            &mut self.pool,
            &self.spaces[space.index].groups[group],
            name,
            &value,
        );
        Ok(())
    }

    pub fn label(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        text: &str,
    ) -> Result<()> {
        self.check(writer)?;
        let Some(group) = self.lookup(space, key, "label") else {
            return Ok(());
        };
        let text = self.intern(writer, text)?;
        Self::set_attribute(
            &mut self.pool,
            &self.spaces[space.index].groups[group],
            self.label,
            &Value::Str(text),
        );
        Ok(())
    }

    pub fn close(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        status: Status,
        time: u64,
    ) -> Result<usize> {
        self.check(writer)?;
        let Some(items) = self
            .lookup(space, key, "close")
            .map(|group| self.spaces[space.index].groups[group].clone())
        else {
            return Ok(0);
        };
        self.end_items(writer, &items, status, time)
    }

    pub fn abort_younger(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        key: u64,
        time: u64,
    ) -> Result<usize> {
        self.check(writer)?;
        let Some(group) = self.lookup(space, key, "abort_younger") else {
            return Ok(0);
        };
        let item = *self.spaces[space.index].groups[group].last().unwrap();
        let last = self.pool[item].record().id();
        let first = self
            .open
            .partition_point(|&item| self.pool[item].record().id() <= last);
        let doomed = self.open[first..].to_vec();
        self.end_items(writer, &doomed, Status::Aborted, time)
    }

    pub fn abort_keyspace(
        &mut self,
        writer: &mut impl Recorder,
        space: Keyspace,
        time: u64,
    ) -> Result<usize> {
        self.check(writer)?;
        if !self.valid(space, "abort_keyspace") {
            return Ok(0);
        }
        self.end_items(
            writer,
            &self.spaces[space.index].order.clone(),
            Status::Aborted,
            time,
        )
    }

    pub fn abort_all(&mut self, writer: &mut impl Recorder, time: u64) -> Result<usize> {
        self.check(writer)?;
        self.end_items(writer, &self.open.clone(), Status::Aborted, time)
    }

    pub fn relate(
        &mut self,
        writer: &mut impl Recorder,
        kind: &str,
        from: (Keyspace, u64),
        to: (Keyspace, u64),
    ) -> Result<usize> {
        self.check(writer)?;
        let from = self.records(from.0, from.1);
        let to = self.records(to.0, to.1);
        Self::relations(writer, kind, &from, &to)
    }

    pub fn relate_to(
        &mut self,
        writer: &mut impl Recorder,
        kind: &str,
        from: (Keyspace, u64),
        destination: &mut Self,
        to: (Keyspace, u64),
    ) -> Result<usize> {
        self.check(writer)?;
        destination.check(writer)?;
        let from = self.records(from.0, from.1);
        let to = destination.records(to.0, to.1);
        Self::relations(writer, kind, &from, &to)
    }

    fn relations(
        writer: &mut impl Recorder,
        kind: &str,
        from: &[u64],
        to: &[u64],
    ) -> Result<usize> {
        if from.is_empty() || to.is_empty() {
            return Ok(0);
        }
        let kind = writer.intern(kind)?;
        let count = from
            .len()
            .checked_mul(to.len())
            .ok_or(Error::invalid("relation count overflow"))?;
        for &from in from {
            for &to in to {
                writer.relate(kind, from, to, vec![])?;
            }
        }
        Ok(count)
    }

    /// Publish pending attributes while leaving records and stage visits unfinished.
    pub fn detach(&mut self, writer: &mut impl Recorder) -> Result<()> {
        self.check(writer)?;
        while let Some(&item) = self.open.last() {
            self.write_attributes(writer, item)?;
            self.release(item);
        }
        Ok(())
    }

    #[must_use]
    pub fn diagnostics(&self) -> &BTreeMap<String, TrackerDiagnostic> {
        &self.diagnostics
    }

    pub fn take_diagnostics(&mut self) -> Vec<String> {
        std::mem::take(&mut self.diagnostics)
            .into_iter()
            .map(|(reason, diagnostic)| {
                format!(
                    "{}: {reason} ({} time{}{})",
                    self.name,
                    diagnostic.count,
                    if diagnostic.count == 1 { "" } else { "s" },
                    diagnostic
                        .first
                        .map_or_else(String::new, |first| format!(", first {first}"))
                )
            })
            .collect()
    }

    fn intern(&mut self, writer: &mut impl Recorder, text: &str) -> Result<u32> {
        if text.is_empty() {
            return Ok(self.empty);
        }
        let hash = text
            .bytes()
            .fold(1_469_598_103_934_665_603u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
            });
        let entry = &mut self.strings[hash as usize & 511];
        if let Some((previous, id)) = entry
            && previous == text
        {
            return Ok(*id);
        }
        let id = writer.intern(text)?;
        if let Some((previous, stored)) = entry {
            previous.clear();
            previous.push_str(text);
            *stored = id;
        } else {
            *entry = Some((text.to_owned(), id));
        }
        Ok(id)
    }

    fn valid(&mut self, space: Keyspace, operation: &str) -> bool {
        if space.tracker == self.id && space.index < self.spaces.len() {
            return true;
        }
        self.diag(
            &format!("{operation} with a key space of another tracker"),
            None,
        );
        false
    }

    fn lookup(&mut self, space: Keyspace, key: u64, operation: &str) -> Option<usize> {
        if !self.valid(space, operation) {
            return None;
        }
        if !self.spaces[space.index].keys.contains_key(&key) {
            self.diag(
                &format!("{operation} on a key that names no item"),
                Some((space.index, key)),
            );
            return None;
        }
        self.spaces[space.index].keys.get(&key).copied()
    }

    fn name(&mut self, item: usize, space: usize, key: u64) {
        if let Some(&(_, previous)) = self.pool[item]
            .names
            .iter()
            .find(|&&(index, _)| index == space)
        {
            if previous == key {
                return;
            }
            self.unname(item, space);
        }
        self.pool[item].names.push((space, key));
        let sequence = self.pool[item].record().id();
        let pool = &self.pool;
        let target = &mut self.spaces[space];
        let group = target.group(key);
        let at = group.partition_point(|&other| pool[other].record().id() < sequence);
        group.insert(at, item);
        let at = target
            .order
            .partition_point(|&other| pool[other].record().id() < sequence);
        target.order.insert(at, item);
    }

    fn unname(&mut self, item: usize, space: usize) {
        if let Some(at) = self.pool[item]
            .names
            .iter()
            .position(|&(index, _)| index == space)
        {
            let (_, key) = self.pool[item].names.remove(at);
            self.spaces[space].remove(key, item);
        }
    }

    fn evict(
        &mut self,
        writer: &mut impl Recorder,
        space: usize,
        key: u64,
        moved: &[usize],
        time: u64,
    ) -> Result<()> {
        let doomed: Vec<_> = self.spaces[space]
            .members(key)
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|item| !moved.contains(item))
            .collect();
        if !doomed.is_empty() {
            self.diag(
                "bind to a key that still names another item; that item is aborted",
                Some((space, key)),
            );
            self.end_items(writer, &doomed, Status::Aborted, time)?;
        }
        Ok(())
    }

    fn enter(
        &mut self,
        writer: &mut impl Recorder,
        item: usize,
        lane: u32,
        stage: u32,
        time: u64,
    ) -> Result<()> {
        let item = &mut self.pool[item];
        let record = item.record().owner();
        if let Some(current) = item.lanes.iter_mut().find(|current| current.name == lane) {
            if current.visit.is_some() && current.stage == stage && stage != self.empty {
                return Ok(());
            }
            if let Some(visit) = current.visit {
                writer.end_embedded_stage(record, visit, time)?;
                current.visit = None;
            }
            if stage != self.empty {
                current.visit =
                    Some(writer.embedded_stage(record, time, stage, lane, None, vec![])?);
                current.stage = stage;
            }
        } else if stage != self.empty {
            item.lanes.push(Lane {
                name: lane,
                stage,
                visit: Some(writer.embedded_stage(record, time, stage, lane, None, vec![])?),
            });
        }
        Ok(())
    }

    fn set_attribute(pool: &mut [Item], items: &[usize], key: u32, value: &Value) {
        for &item in items {
            let attributes = &mut pool[item].attributes;
            if let Some(attribute) = attributes.iter_mut().find(|attribute| attribute.key == key) {
                attribute.value = value.clone();
            } else {
                attributes.push(Attribute {
                    key,
                    value: value.clone(),
                });
            }
        }
    }

    fn write_attributes(&mut self, writer: &mut impl Recorder, item: usize) -> Result<()> {
        let item = &mut self.pool[item];
        let owner = item.record().owner();
        for attribute in &item.attributes {
            writer.attribute(owner, attribute.key, attribute.value.clone())?;
        }
        item.attributes.clear();
        Ok(())
    }

    fn release(&mut self, item: usize) {
        while let Some(&(space, _)) = self.pool[item].names.last() {
            self.unname(item, space);
        }
        self.open.retain(|&other| other != item);
        let value = &mut self.pool[item];
        value.record = None;
        value.lanes.clear();
        value.attributes.clear();
        self.free.push(item);
    }

    fn end_items(
        &mut self,
        writer: &mut impl Recorder,
        items: &[usize],
        status: Status,
        time: u64,
    ) -> Result<usize> {
        for &item in items {
            self.write_attributes(writer, item)?;
            writer.end_at(self.pool[item].record(), time, status)?;
            self.release(item);
        }
        Ok(items.len())
    }

    fn diag(&mut self, reason: &str, location: Option<(usize, u64)>) {
        let diagnostic = self.diagnostics.entry(reason.to_owned()).or_default();
        if diagnostic.count == 0 {
            diagnostic.first =
                location.map(|(space, key)| format!("{} {key}", self.spaces[space].name));
        }
        diagnostic.count = diagnostic.count.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionMode, SourceOptions, WriterOptions};

    #[test]
    fn item_and_group_storage_stays_bounded_under_key_churn() {
        let mut writer = Writer::from_writer(
            Box::new(std::io::sink()),
            WriterOptions {
                mode: ExecutionMode::Inline,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let source = writer
            .transactions::<()>(SourceOptions::new("requests", None), "REQUEST")
            .unwrap();
        let mut tracker = Tracker::new(&mut writer, source, "requests").unwrap();
        let input = tracker.keyspace("input").unwrap();
        let output = tracker.keyspace("output").unwrap();
        for round in 0..512 {
            writer.advance_time(round * 3).unwrap();
            for key in 0..8 {
                tracker
                    .open(&mut writer, input, round * 8 + key, "A", round * 3, None)
                    .unwrap();
            }
            tracker
                .bind_oldest(&mut writer, input, 8, output, round, round * 3)
                .unwrap();
            tracker
                .stage(&mut writer, output, round, "B", round * 3 + 1)
                .unwrap();
            tracker
                .close(&mut writer, output, round, Status::Ok, round * 3 + 2)
                .unwrap();
        }
        assert_eq!(tracker.open_count(), 0);
        assert_eq!(tracker.pool.len(), 8);
        assert_eq!(tracker.free.len(), 8);
        assert!(tracker.spaces.iter().all(|space| space.keys.is_empty()));
        assert!(
            tracker
                .spaces
                .iter()
                .map(|space| space.groups.len())
                .sum::<usize>()
                <= 16
        );
        assert!(tracker.diagnostics().is_empty());
        writer.close().unwrap();
    }
}
