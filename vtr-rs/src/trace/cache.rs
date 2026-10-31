//! FIFO caches bounded by retained storage and entry count.

use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::Arc,
};

pub(super) type PageCache = Cache<(u64, u32), Arc<Vec<u8>>>;

pub(super) struct Cache<K, V> {
    entries: HashMap<K, (usize, V)>,
    order: VecDeque<K>,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl<K, V> Cache<K, V> {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_entries,
            max_bytes,
        }
    }
    pub fn clear(&mut self) {
        *self = Self::new(self.max_entries, self.max_bytes);
    }
}

impl<K: Copy + Eq + Hash, V: Clone> Cache<K, V> {
    pub fn get(&self, key: K) -> Option<V> {
        self.entries.get(&key).map(|(_, value)| value.clone())
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        if self.max_entries == 0
            || self.max_bytes == 0
            || bytes > self.max_bytes
            || self.entries.contains_key(&key)
        {
            return;
        }
        while self.entries.len() >= self.max_entries || bytes > self.max_bytes - self.bytes {
            let oldest = self.order.pop_front().expect("cache eviction order");
            self.bytes -= self.entries.remove(&oldest).expect("cached entry").0;
        }
        self.bytes += bytes;
        self.order.push_back(key);
        self.entries.insert(key, (bytes, value));
    }
}
