//! Incremental raw hierarchy storage for query-backed documents. Pages retain
//! their admitted text payloads; names are not copied or replaced with labels.
use std::sync::Arc;
use vtr_query::{
    Budget, Error, Reservation, Result,
    metadata::{Declaration, DeclarationPage},
    session::{Continuation, Delivery, Reply, SessionInfo, SnapshotId},
};

#[derive(Clone, Copy)]
struct Index {
    id: u32,
    page: usize,
    declaration: usize,
}
struct Children {
    parent: Option<u32>,
    received: u64,
    last: Arc<Delivery>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChildrenState {
    pub received: u64,
    pub complete: bool,
    pub next: Option<Continuation>,
}

/// One immutable recording, populated only by demanded child pages. Capacity
/// includes all retained declarations, including scopes and non-wave records.
/// Page payloads retain their originating query-budget reservations; this
/// object's budget accounts for its own fixed index/reference arrays.
/// A capacity refusal leaves accepted data and its continuation unchanged.
pub struct QueryHierarchy {
    snapshot: SnapshotId,
    declarations: u64,
    capacity: usize,
    groups: Vec<Children>,
    pages: Vec<Arc<DeclarationPage>>,
    index: Vec<Index>,
    texts: Vec<QueryText>,
    _charge: Reservation,
}
impl QueryHierarchy {
    pub fn new(info: &SessionInfo, capacity: usize, budget: &Budget) -> Result<Self> {
        if capacity == 0 || capacity > 1_000_000 {
            return Err(Error::Invalid(
                "hierarchy capacity must be 1..1000000 declarations",
            ));
        }
        // A nonempty page consumes at least one declaration. Empty work slices
        // retain only their group's last envelope, not an ever-growing list.
        // There can be at most one children group per declaration, plus roots.
        let bytes = capacity
            .checked_mul(
                std::mem::size_of::<Index>()
                    + std::mem::size_of::<Arc<DeclarationPage>>()
                    + std::mem::size_of::<Children>()
                    + std::mem::size_of::<QueryText>(),
            )
            .and_then(|n| n.checked_add(std::mem::size_of::<Children>()))
            .ok_or(Error::ResourceLimit)?;
        let charge = budget.reserve(bytes)?;
        let mut pages = Vec::new();
        let mut index = Vec::new();
        let mut groups = Vec::new();
        let mut texts = Vec::new();
        texts
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        pages
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        index
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        groups
            .try_reserve_exact(capacity + 1)
            .map_err(|_| Error::ResourceLimit)?;
        Ok(Self {
            snapshot: info.snapshot,
            declarations: info.declarations,
            capacity,
            groups,
            texts,
            pages,
            index,
            _charge: charge,
        })
    }
    /// Resolve a name without constructing a presentation copy. Unfetched
    /// references remain None and must not be treated as empty names.
    pub fn text<'a>(&'a self, text: &'a vtr_query::metadata::Text) -> Option<&'a str> {
        match text {
            vtr_query::metadata::Text::Inline(bytes) => std::str::from_utf8(bytes.as_slice()).ok(),
            vtr_query::metadata::Text::Reference { id, bytes } => self
                .texts
                .binary_search_by_key(id, |text| text.id)
                .ok()
                .filter(|&index| self.texts[index].total as u64 == *bytes)
                .and_then(|index| self.texts[index].text()),
        }
    }
    /// Install one complete referenced name; its assembly reservation follows
    /// the string into this tree. Only names referenced by loaded nodes qualify.
    pub fn install_text(&mut self, text: QueryText) -> Result<()> {
        if text.snapshot != self.snapshot || text.text().is_none()
            || !self.declarations().any(|node| matches!(node.name,
                vtr_query::metadata::Text::Reference { id, bytes } if id == text.id && bytes == text.total as u64)) {
            return Err(Error::Invalid("text is not a completed name for this hierarchy"));
        }
        match self.texts.binary_search_by_key(&text.id, |text| text.id) {
            Ok(index) if self.texts[index].text() == text.text() => Ok(()),
            Ok(_) => Err(Error::Invalid("metadata name changed within a snapshot")),
            Err(index) => {
                if self.texts.len() == self.capacity {
                    return Err(Error::ResourceLimit);
                }
                self.texts.insert(index, text);
                Ok(())
            }
        }
    }
    pub fn snapshot(&self) -> SnapshotId {
        self.snapshot
    }
    pub fn len(&self) -> usize {
        self.index.len()
    }
    pub fn is_fully_loaded(&self) -> bool {
        self.index.len() as u64 == self.declarations
    }
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
    pub fn declaration(&self, id: u32) -> Option<&Declaration> {
        let i = self.index.binary_search_by_key(&id, |item| item.id).ok()?;
        let item = &self.index[i];
        self.pages[item.page].declarations().get(item.declaration)
    }
    pub fn declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.index
            .iter()
            .map(|item| &self.pages[item.page].declarations()[item.declaration])
    }
    pub fn children(&self, parent: Option<u32>) -> impl Iterator<Item = &Declaration> {
        self.pages
            .iter()
            .filter(move |page| page.parent == parent)
            .flat_map(|page| page.declarations())
    }
    /// None means this parent has not been queried, even if it has no children.
    pub fn state(&self, parent: Option<u32>) -> Option<ChildrenState> {
        self.groups
            .iter()
            .find(|group| group.parent == parent)
            .map(|group| ChildrenState {
                received: group.received,
                complete: group.last.next.is_none(),
                next: group.last.next,
            })
    }
    /// Accept a page from the exact continuation chain for its parent. Native
    /// and RPC sessions replay retries as the same immutable delivery owner.
    pub fn append(&mut self, delivery: Arc<Delivery>) -> Result<bool> {
        if delivery.request.snapshot != self.snapshot {
            return Err(Error::Invalid("hierarchy page belongs to another snapshot"));
        }
        let Reply::Children(page) = &delivery.reply else {
            return Err(Error::Invalid("expected a hierarchy children page"));
        };
        let group = self
            .groups
            .iter()
            .position(|group| group.parent == page.parent);
        let expected_offset = if let Some(group) = group {
            let state = &self.groups[group];
            if Arc::ptr_eq(&state.last, &delivery) {
                return Ok(false);
            }
            if state.last.next != Some(delivery.request) {
                return Err(Error::Invalid(
                    "hierarchy continuation does not follow accepted page",
                ));
            }
            state.received
        } else {
            if delivery.request.step != 0 {
                return Err(Error::Invalid(
                    "first hierarchy page must start at step zero",
                ));
            }
            0
        };
        if page.offset != expected_offset || page.complete != delivery.next.is_none() {
            return Err(Error::Invalid("hierarchy page has inconsistent coverage"));
        }
        if let Some(next) = delivery.next
            && (next.snapshot != self.snapshot
                || next.operation != delivery.request.operation
                || delivery.request.step.checked_add(1) != Some(next.step))
        {
            return Err(Error::Invalid("invalid hierarchy continuation"));
        }
        let records = page.declarations();
        let received = expected_offset
            .checked_add(records.len() as u64)
            .ok_or(Error::ResourceLimit)?;
        if let Some(parent) = page.parent {
            let parent = self
                .declaration(parent)
                .ok_or(Error::Invalid("hierarchy parent is not loaded"))?;
            if received > parent.children || (page.complete && received != parent.children) {
                return Err(Error::Invalid(
                    "hierarchy child count differs from parent declaration",
                ));
            }
        }
        if self
            .index
            .len()
            .checked_add(records.len())
            .is_none_or(|n| n > self.capacity)
        {
            return Err(Error::ResourceLimit);
        }
        let mut previous = self.children(page.parent).last().map(|node| node.id);
        for node in records {
            node.name.as_str()?;
            if node.parent != page.parent
                || u64::from(node.id) >= self.declarations
                || previous.is_some_and(|id| id >= node.id)
                || self.declaration(node.id).is_some()
            {
                return Err(Error::Invalid("invalid or repeated hierarchy declaration"));
            }
            previous = Some(node.id);
        }
        // All admission and validation precede mutation. The arrays were
        // reserved at construction and each nonempty page owns >=1 record.
        if !records.is_empty() {
            let page_index = self.pages.len();
            // Both sequences are sorted. Merge backwards in the admitted
            // array rather than shifting the index separately for every node.
            let mut old = self.index.len();
            let mut incoming = records.len();
            self.index.resize(
                old + incoming,
                Index {
                    id: 0,
                    page: 0,
                    declaration: 0,
                },
            );
            while incoming > 0 {
                let output = old + incoming - 1;
                if old > 0 && self.index[old - 1].id > records[incoming - 1].id {
                    self.index[output] = self.index[old - 1];
                    old -= 1;
                } else {
                    incoming -= 1;
                    self.index[output] = Index {
                        id: records[incoming].id,
                        page: page_index,
                        declaration: incoming,
                    };
                }
            }
            self.pages.push(page.clone());
        }
        let state = Children {
            parent: page.parent,
            received,
            last: delivery,
        };
        if let Some(group) = group {
            self.groups[group] = state;
        } else {
            self.groups.push(state);
        }
        Ok(true)
    }
    /// Release all retained pages while keeping the admitted arrays reusable.
    pub fn clear(&mut self) {
        self.groups.clear();
        self.texts.clear();
        self.pages.clear();
        self.index.clear();
    }
}

/// Bounded assembly of one referenced metadata string. A TextPart may split
/// UTF-8; no text is exposed until the entire original string is validated.
/// Each part is copied once into the admitted final buffer, then its query
/// delivery can be released. No per-part list grows with the string length.
pub struct QueryText {
    snapshot: SnapshotId,
    id: u32,
    total: usize,
    bytes: Vec<u8>,
    last: Option<(Continuation, usize, usize)>,
    complete: bool,
    _charge: Reservation,
}
impl QueryText {
    pub fn new(
        snapshot: SnapshotId,
        id: u32,
        bytes: u64,
        max_bytes: usize,
        budget: &Budget,
    ) -> Result<Self> {
        let total = usize::try_from(bytes).map_err(|_| Error::ResourceLimit)?;
        if total > max_bytes {
            return Err(Error::ResourceLimit);
        }
        let charge = budget.reserve(total)?;
        let mut data = Vec::new();
        data.try_reserve_exact(total)
            .map_err(|_| Error::ResourceLimit)?;
        Ok(Self {
            snapshot,
            id,
            total,
            bytes: data,
            last: None,
            complete: total == 0,
            _charge: charge,
        })
    }
    pub fn offset(&self) -> u64 {
        self.bytes.len() as u64
    }
    pub fn remaining(&self) -> usize {
        self.total - self.bytes.len()
    }
    pub fn text(&self) -> Option<&str> {
        self.complete
            .then(|| std::str::from_utf8(&self.bytes).expect("validated completed metadata text"))
    }
    pub fn append(&mut self, delivery: &Delivery) -> Result<bool> {
        let Reply::Text(part) = &delivery.reply else {
            return Err(Error::Invalid("expected a metadata text part"));
        };
        if delivery.request.snapshot != self.snapshot
            || delivery.next.is_some()
            || part.id != self.id
            || part.total_bytes != self.total as u64
        {
            return Err(Error::Invalid(
                "metadata text part belongs to another string or snapshot",
            ));
        }
        let start = usize::try_from(part.offset).map_err(|_| Error::ResourceLimit)?;
        let data = part.bytes.as_slice();
        if let Some((request, offset, length)) = self.last
            && delivery.request == request
        {
            if offset == start
                && length == data.len()
                && self.bytes.get(start..start + length) == Some(data)
            {
                return Ok(false);
            }
            return Err(Error::Invalid("metadata text retry changed its bytes"));
        }
        if self.complete
            || start != self.bytes.len()
            || data.is_empty()
            || data.len() > self.remaining()
        {
            return Err(Error::Invalid(
                "metadata text part has inconsistent coverage",
            ));
        }
        self.bytes.extend_from_slice(data);
        if self.bytes.len() == self.total {
            if std::str::from_utf8(&self.bytes).is_err() {
                self.bytes.truncate(start);
                return Err(Error::Invalid("completed metadata text is not UTF-8"));
            }
            self.complete = true;
        }
        self.last = Some((delivery.request, start, data.len()));
        Ok(true)
    }
}
