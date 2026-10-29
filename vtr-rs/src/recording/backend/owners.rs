//! Bounded direct slots keep recently allocated owners out of the overflow map.

use super::{ActiveOwner, IdMap};
use crate::{Error, Result, transactions::Transaction};

const SLOTS: usize = 1024;

#[derive(Default)]
pub(super) struct Owners {
    slots: Vec<Option<ActiveOwner>>,
    overflow: IdMap<u64, ActiveOwner>,
    len: usize,
}

impl Owners {
    #[inline]
    pub fn get(&self, id: u64) -> Option<&ActiveOwner> {
        if let Some(Some(owner)) = self.slots.get(id as usize & (SLOTS - 1))
            && owner.value.id == id
        {
            return Some(owner);
        }
        self.overflow.get(&id)
    }

    #[inline]
    pub fn get_mut(&mut self, id: u64) -> Option<&mut ActiveOwner> {
        if let Some(Some(owner)) = self.slots.get_mut(id as usize & (SLOTS - 1))
            && owner.value.id == id
        {
            return Some(owner);
        }
        self.overflow.get_mut(&id)
    }

    #[inline]
    pub fn contains_key(&self, id: u64) -> bool {
        self.get(id).is_some()
    }

    pub fn begin(
        &mut self,
        id: u64,
        generator: u32,
        time: u64,
        parent: u64,
        payload: Vec<u8>,
        payload_ready: bool,
    ) -> Result<()> {
        if self.get(id).is_some() {
            return Err(Error::invalid("record identifier reused"));
        }
        if self.slots.is_empty() {
            self.slots.resize_with(SLOTS, || None);
        }
        let index = id as usize & (SLOTS - 1);
        if self.slots[index].is_some() {
            self.displace(index);
        }
        let owner = self.slots[index].insert(ActiveOwner {
            value: Transaction::default(),
            stages: Vec::new(),
            payload_ready,
        });
        owner.value.id = id;
        owner.value.generator = generator;
        owner.value.begin = time;
        owner.value.parent = parent;
        owner.value.payload = payload;
        self.len += 1;
        Ok(())
    }

    #[cold]
    fn displace(&mut self, index: usize) {
        if let Some(displaced) = self.slots[index].take() {
            self.overflow.insert(displaced.value.id, displaced);
        }
    }

    #[inline]
    pub fn remove(&mut self, id: u64) -> Option<ActiveOwner> {
        let slot = self.slots.get_mut(id as usize & (SLOTS - 1));
        let owner = if let Some(slot) = slot
            && slot.as_ref().is_some_and(|owner| owner.value.id == id)
        {
            slot.take()
        } else {
            self.overflow.remove(&id)
        };
        if owner.is_some() {
            self.len -= 1;
        }
        owner
    }

    pub fn finish_into(&mut self, id: u64, transactions: &mut Vec<Transaction>) {
        if let Some(slot) = self.slots.get_mut(id as usize & (SLOTS - 1))
            && slot.as_ref().is_some_and(|owner| owner.value.id == id)
        {
            transactions.push(std::mem::take(&mut slot.as_mut().unwrap().value));
            *slot = None;
        } else {
            transactions.push(
                self.overflow
                    .remove(&id)
                    .expect("validated active owner")
                    .value,
            );
        }
        self.len -= 1;
    }

    pub fn values(&self) -> impl Iterator<Item = &ActiveOwner> {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .chain(self.overflow.values())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u64, &ActiveOwner)> {
        self.values().map(|owner| (&owner.value.id, owner))
    }

    pub fn keys(&self) -> impl Iterator<Item = &u64> {
        self.values().map(|owner| &owner.value.id)
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colliding_live_owners_preserve_updates_reject_reuse_and_complete() {
        let mut owners = Owners::default();
        for id in [1, 1025, 2049, u64::MAX, u64::MAX - 1024] {
            owners
                .begin(id, 0, id, 0, id.to_le_bytes().to_vec(), true)
                .unwrap();
        }
        owners.get_mut(1).unwrap().value.begin = 7;
        assert!(owners.begin(1025, 0, 8, 0, vec![], true).is_err());
        assert_eq!(owners.len(), 5);
        assert_eq!(owners.remove(2049).unwrap().value.begin, 2049);
        assert!(owners.remove(2049).is_none());
        assert_eq!(owners.get(1).unwrap().value.begin, 7);
        assert_eq!(owners.get(1025).unwrap().value.begin, 1025);
        assert_eq!(owners.remove(u64::MAX).unwrap().value.begin, u64::MAX);
        assert_eq!(owners.len(), 3);
        let mut ids: Vec<_> = owners.keys().copied().collect();
        ids.sort_unstable();
        assert_eq!(ids, [1, 1025, u64::MAX - 1024]);
        let mut transactions = Vec::new();
        for &id in &ids {
            owners.finish_into(id, &mut transactions);
        }
        assert_eq!(transactions.iter().map(|tx| tx.id).collect::<Vec<_>>(), ids);
        assert_eq!(
            transactions.iter().map(|tx| tx.begin).collect::<Vec<_>>(),
            [7, 1025, u64::MAX - 1024]
        );
        for tx in &transactions {
            assert_eq!(tx.payload, tx.id.to_le_bytes());
        }
        assert_eq!(owners.len(), 0);
        assert_eq!(owners.iter().count(), 0);
        owners.begin(1, 0, 9, 0, vec![], true).unwrap();
        assert_eq!(owners.remove(1).unwrap().value.begin, 9);
    }
}
