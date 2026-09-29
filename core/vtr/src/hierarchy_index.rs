//! Immutable dense identities over the reader's original declaration columns.
use crate::hierarchy::{Hierarchy, NodeId, NodeKind};

/// Dense declaration identities for selected node kinds, in declaration order.
/// The node IDs support selection; a bitmap and prefix popcounts map a reader
/// node back to its dense identity without a per-node reverse map.
#[derive(Debug, Default)]
pub struct NodeIndex {
    nodes: Vec<NodeId>,
    bits: Vec<u64>,
    rank: Vec<u32>,
}
impl NodeIndex {
    /// Build an immutable index. Does not copy declaration fields or names.
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
    /// Number of matching declarations.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    /// Whether this selection contains no declarations.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    /// Reader node at a dense identity. Panics out of range.
    pub fn node(&self, id: usize) -> NodeId {
        self.nodes[id]
    }
    /// Dense identity of a reader node, or `None` for an unselected kind.
    pub fn ordinal(&self, node: NodeId) -> Option<usize> {
        let word = node.0 as usize / 64;
        let offset = node.0 % 64;
        let bits = *self.bits.get(word)?;
        (bits & (1 << offset) != 0)
            .then(|| (self.rank[word] + (bits & ((1u64 << offset) - 1)).count_ones()) as usize)
    }
    /// Allocated bytes of IDs and rank/select support, excluding the reader.
    pub fn resident_bytes(&self) -> u64 {
        (self.nodes.capacity() * 4 + self.bits.capacity() * 8 + self.rank.capacity() * 4) as u64
    }
    /// Borrow children, filtering this index's selected kinds. The hierarchy
    /// must be the indexed hierarchy used to construct this selection.
    pub fn children<'a>(&'a self, h: &'a Hierarchy, node: NodeId) -> IndexedNodes<'a> {
        IndexedNodes {
            index: self,
            nodes: h.child_ids(node),
        }
    }
}

/// Borrowed membership over the reader's existing child column.
#[derive(Clone, Copy, Debug)]
pub struct IndexedNodes<'a> {
    index: &'a NodeIndex,
    nodes: &'a [u32],
}
impl<'a> IndexedNodes<'a> {
    /// Dense identities in original child order; iteration allocates nothing.
    pub fn iter(self) -> IndexedIter<'a> {
        IndexedIter {
            index: self.index,
            nodes: self.nodes,
            front: 0,
            back: self.nodes.len(),
        }
    }
    /// Number of selected children, scanned without allocating.
    pub fn len(self) -> usize {
        self.iter().count()
    }
    /// Whether any selected child exists.
    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }
}

/// Allocation-free cursor over a selected child column.
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
