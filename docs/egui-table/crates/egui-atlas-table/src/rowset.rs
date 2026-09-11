//! Compact, immutable row mapping. Rank/select use a directory every 512 bits;
//! rendering never walks preceding rows, even in a sparse filtered view.
use std::sync::Arc;

#[derive(Clone)]
pub enum RowSet {
    All(u32),
    Bits(Arc<RankedBits>),
}
impl RowSet {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn len(&self) -> u32 {
        match self {
            Self::All(n) => *n,
            Self::Bits(b) => b.len,
        }
    }
    pub fn select(&self, index: u32) -> Option<u32> {
        match self {
            Self::All(n) => (index < *n).then_some(index),
            Self::Bits(b) => b.select(index),
        }
    }
    pub fn contains(&self, row: u32) -> bool {
        match self {
            Self::All(n) => row < *n,
            Self::Bits(b) => b
                .words
                .get(row as usize / 64)
                .is_some_and(|w| w & (1 << (row % 64)) != 0),
        }
    }
    /// Number of selected source rows strictly before `row`.
    pub fn rank_before(&self, row: u32) -> u32 {
        match self {
            Self::All(n) => row.min(*n),
            Self::Bits(b) => b.rank_before(row),
        }
    }
    pub fn bytes(&self) -> usize {
        match self {
            Self::All(_) => 0,
            Self::Bits(b) => b.words.len() * 8 + b.prefix.len() * 4,
        }
    }
}
pub struct RankedBits {
    words: Vec<u64>,
    prefix: Vec<u32>,
    len: u32,
}
impl RankedBits {
    fn select(&self, index: u32) -> Option<u32> {
        if index >= self.len {
            return None;
        }
        let block = self.prefix.partition_point(|&p| p <= index) - 1;
        let mut remaining = index - self.prefix[block];
        for (offset, &word) in self.words[block * 8..].iter().take(8).enumerate() {
            let count = word.count_ones();
            if remaining < count {
                let mut w = word;
                for _ in 0..remaining {
                    w &= w - 1;
                }
                return Some(((block * 8 + offset) * 64) as u32 + w.trailing_zeros());
            }
            remaining -= count;
        }
        unreachable!("rank directory and bits disagree")
    }
    fn rank_before(&self, row: u32) -> u32 {
        let word_index = row as usize / 64;
        if word_index >= self.words.len() {
            return self.len;
        }
        let block = word_index / 8;
        let mut count = self.prefix[block];
        for word in &self.words[block * 8..word_index] {
            count += word.count_ones();
        }
        count + (self.words[word_index] & ((1u64 << (row % 64)) - 1)).count_ones()
    }
}
pub struct Builder {
    words: Vec<u64>,
}
impl Builder {
    pub fn new(rows: u32) -> Self {
        Self {
            words: vec![0; rows.div_ceil(64) as usize],
        }
    }
    pub fn insert(&mut self, row: u32) {
        self.words[row as usize / 64] |= 1 << (row % 64);
    }
    pub fn snapshot(&self) -> RowSet {
        Self::finish(self.words.clone())
    }
    pub fn build(self) -> RowSet {
        Self::finish(self.words)
    }
    fn finish(words: Vec<u64>) -> RowSet {
        let mut prefix = Vec::with_capacity(words.len().div_ceil(8));
        let mut len = 0;
        for chunk in words.chunks(8) {
            prefix.push(len);
            len += chunk.iter().map(|w| w.count_ones()).sum::<u32>();
        }
        RowSet::Bits(Arc::new(RankedBits { words, prefix, len }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rank_select_empty_dense_sparse_and_last_row() {
        for rows in [1, 63, 64, 65, 511, 512, 513, 10_000_000] {
            let mut b = Builder::new(rows);
            let expected: Vec<u32> = (0..rows)
                .filter(|r| r % 499 == 0 || *r == rows - 1)
                .collect();
            for &r in &expected {
                b.insert(r);
            }
            let set = b.build();
            assert_eq!(set.len(), expected.len() as u32);
            for (i, &row) in expected.iter().enumerate() {
                assert_eq!(set.select(i as u32), Some(row));
                assert_eq!(set.rank_before(row), i as u32);
                assert_eq!(set.rank_before(row + 1), i as u32 + 1);
                assert!(set.contains(row));
            }
            assert_eq!(set.select(set.len()), None);
            assert_eq!(Builder::new(rows).build().select(0), None);
        }
        let mut b = Builder::new(2048);
        for r in 0..2048 {
            b.insert(r);
        }
        let set = b.build();
        for r in 0..2048 {
            assert_eq!(set.select(r), Some(r));
        }
    }
}
