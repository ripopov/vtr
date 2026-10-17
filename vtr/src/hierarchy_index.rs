//! Immutable dense identities over the reader's original declaration columns.

use crate::hierarchy::{Hierarchy, NodeId, NodeKind};

#[derive(Debug, Default)]
pub struct NodeIndex {
    nodes: Vec<NodeId>,
    bits: Vec<u64>,
    rank: Vec<u32>,
}
impl NodeIndex {
    #[must_use]
    pub fn new(h: &Hierarchy, kinds: &[NodeKind]) -> Self {
        let count = h.ids().filter(|&id| kinds.contains(&h.kind(id))).count();
        if count == 0 {
            return Self::default();
        }
        let mut out = Self {
            nodes: Vec::with_capacity(count),
            bits: vec![0; h.len().div_ceil(64)],
            rank: Vec::with_capacity(h.len().div_ceil(64)),
        };
        for id in h.ids() {
            if kinds.contains(&h.kind(id)) {
                out.nodes.push(id);
                out.bits[id.0 as usize / 64] |= 1 << (id.0 % 64);
            }
        }
        let mut rank = 0;
        for &bits in &out.bits {
            out.rank.push(rank);
            rank += bits.count_ones();
        }
        out
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    #[must_use]
    pub fn node(&self, id: usize) -> NodeId {
        self.nodes[id]
    }
    #[must_use]
    pub fn ordinal(&self, node: NodeId) -> Option<usize> {
        let word = node.0 as usize / 64;
        let offset = node.0 % 64;
        let bits = *self.bits.get(word)?;
        (bits & (1 << offset) != 0)
            .then(|| (self.rank[word] + (bits & ((1u64 << offset) - 1)).count_ones()) as usize)
    }
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        (self.nodes.capacity() * 4 + self.bits.capacity() * 8 + self.rank.capacity() * 4) as u64
    }
    #[must_use]
    pub fn children<'a>(&'a self, h: &'a Hierarchy, node: NodeId) -> IndexedNodes<'a> {
        IndexedNodes {
            index: self,
            nodes: h.child_ids(node),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IndexedNodes<'a> {
    index: &'a NodeIndex,
    nodes: &'a [u32],
}
impl<'a> IndexedNodes<'a> {
    #[must_use]
    pub fn iter(self) -> IndexedIter<'a> {
        IndexedIter {
            index: self.index,
            nodes: self.nodes,
            front: 0,
            back: self.nodes.len(),
        }
    }
    #[must_use]
    pub fn len(self) -> usize {
        self.iter().count()
    }
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }
}

pub struct IndexedIter<'a> {
    index: &'a NodeIndex,
    nodes: &'a [u32],
    front: usize,
    back: usize,
}
impl Iterator for IndexedIter<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        while self.front < self.back {
            let node = self.nodes[self.front];
            self.front += 1;
            if let Some(id) = self.index.ordinal(NodeId(node)) {
                return Some(id);
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.back - self.front))
    }
}
impl DoubleEndedIterator for IndexedIter<'_> {
    fn next_back(&mut self) -> Option<usize> {
        while self.front < self.back {
            self.back -= 1;
            if let Some(id) = self.index.ordinal(NodeId(self.nodes[self.back])) {
                return Some(id);
            }
        }
        None
    }
}
