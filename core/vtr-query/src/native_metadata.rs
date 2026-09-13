use crate::{
    metadata::{Declaration, DeclarationData, DeclarationPage, SharedDeclarations, Text, TextPart},
    wave::{Bytes, Limits},
    Budget, Cancellation, Error, Result,
};
use std::sync::Arc;
use vtr::{NodeDataRef, NodeId, Reader, StrId};

/// Page direct children from the reader's existing adjacency index. No full
/// hierarchy copy or pathname table is constructed for the query layer.
pub struct Children<'a> {
    reader: &'a Reader,
    parent: Option<NodeId>,
    offset: usize,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
}
impl<'a> Children<'a> {
    pub fn new(
        reader: &'a Reader,
        parent: Option<u32>,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        if parent.is_some_and(|id| id as usize >= reader.hierarchy().len()) {
            return Err(Error::Invalid("unknown hierarchy parent"));
        }
        if limits.records == 0 || limits.work == 0 {
            return Err(Error::Invalid("zero metadata record or work limit"));
        }
        Ok(Self {
            reader,
            parent: parent.map(NodeId),
            offset: 0,
            limits,
            budget,
            cancellation,
        })
    }
    fn count(&self) -> usize {
        match self.parent {
            Some(parent) => self.reader.hierarchy().children(parent).count(),
            None => self.reader.hierarchy().roots().count(),
        }
    }
    pub fn next_page(&mut self) -> Result<SharedDeclarations> {
        self.cancellation.check()?;
        let available = self
            .limits
            .bytes
            .checked_sub(std::mem::size_of::<DeclarationPage>())
            .ok_or(Error::ResourceLimit)?;
        let count = self.count();
        let capacity = self
            .limits
            .records
            .min(self.limits.work)
            .min(count - self.offset)
            .min(available / std::mem::size_of::<Declaration>());
        if capacity == 0 && self.offset < count {
            return Err(Error::ResourceLimit);
        }
        let rows = capacity * std::mem::size_of::<Declaration>();
        let charge = self
            .budget
            .reserve(rows + std::mem::size_of::<DeclarationPage>())?;
        let mut declarations = Vec::new();
        declarations
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        if declarations.capacity() != capacity {
            return Err(Error::ResourceLimit);
        }
        let first = self.offset;
        let mut remaining = available - rows;
        for index in first..first + capacity {
            self.cancellation.check()?;
            let id = match self.parent {
                Some(parent) => self.reader.hierarchy().children(parent).nth(index),
                None => self.reader.hierarchy().roots().nth(index),
            }
            .ok_or(Error::Invalid("hierarchy cursor out of range"))?;
            declarations.push(declaration(self.reader, id, &self.budget, &mut remaining)?);
        }
        // Commit the cursor only after the whole page succeeds, so cancellation
        // or allocation failure cannot consume declarations never delivered.
        self.offset += declarations.len();
        Ok(Arc::new(DeclarationPage {
            parent: self.parent.map(|p| p.0),
            offset: first as u64,
            complete: self.offset == count,
            declarations,
            _charge: charge,
        }))
    }
}
fn text(reader: &Reader, id: StrId, budget: &Budget, remaining: &mut usize) -> Result<Text> {
    let raw = reader.str(id).as_bytes();
    let allocation = Bytes::retained_size(raw.len())?;
    if raw.len() > 1024 || allocation > *remaining {
        return Ok(Text::Reference {
            id: id.0,
            bytes: raw.len() as u64,
        });
    }
    // A string reference is also valid when another pinned page occupies the
    // global budget. Do not turn available metadata into an allocation failure.
    match Bytes::from_slice(raw, budget) {
        Ok(bytes) => {
            *remaining -= allocation;
            Ok(Text::Inline(bytes))
        }
        Err(Error::ResourceLimit) => Ok(Text::Reference {
            id: id.0,
            bytes: raw.len() as u64,
        }),
        Err(error) => Err(error),
    }
}
fn declaration(
    reader: &Reader,
    id: NodeId,
    budget: &Budget,
    remaining: &mut usize,
) -> Result<Declaration> {
    let hierarchy = reader.hierarchy();
    let node = hierarchy.node_ref(id);
    let name = text(reader, node.name, budget, remaining)?;
    let data = match node.data {
        NodeDataRef::Scope {
            scope_type,
            component,
        } => DeclarationData::Scope {
            type_code: scope_type.code(),
            component: text(reader, component, budget, remaining)?,
        },
        NodeDataRef::Var {
            var_type,
            direction,
            signal,
            declares,
        } => DeclarationData::Variable {
            type_code: var_type.code(),
            direction: direction as u8,
            signal: signal.0,
            kind: super::native::kind(
                hierarchy
                    .signal_kind(signal)
                    .ok_or(Error::Invalid("missing signal kind"))?,
            ),
            alias: declares.is_none(),
        },
        NodeDataRef::Stream { kind } => DeclarationData::Stream {
            kind: text(reader, kind, budget, remaining)?,
        },
        NodeDataRef::Generator => DeclarationData::Generator,
        NodeDataRef::EnumTable { entries } => DeclarationData::EnumTable {
            entries: entries.len() as u64,
        },
    };
    Ok(Declaration {
        id: id.0,
        parent: node.parent.map(|p| p.0),
        name,
        data,
        children: hierarchy.children(id).count() as u64,
        attributes: node.attrs.len() as u64,
    })
}

/// Retrieve a bounded byte range of an interned string on this same snapshot.
pub fn text_part(
    reader: &Reader,
    id: u32,
    offset: u64,
    length: usize,
    budget: &Budget,
    cancellation: &Cancellation,
) -> Result<Arc<TextPart>> {
    cancellation.check()?;
    if id as usize >= reader.strings().len() {
        return Err(Error::Invalid("unknown string reference"));
    }
    let raw = reader.str(StrId(id)).as_bytes();
    let offset_usize =
        usize::try_from(offset).map_err(|_| Error::Invalid("string offset out of range"))?;
    if offset_usize > raw.len() || length == 0 {
        return Err(Error::Invalid("invalid string byte range"));
    }
    let end = offset_usize.saturating_add(length).min(raw.len());
    let charge = budget.reserve(std::mem::size_of::<TextPart>())?;
    let bytes = Bytes::from_slice(&raw[offset_usize..end], budget)?;
    Ok(Arc::new(TextPart {
        id,
        offset,
        total_bytes: raw.len() as u64,
        bytes,
        _charge: charge,
    }))
}

type Folded<'a> = std::iter::FlatMap<
    std::str::Chars<'a>,
    std::char::ToLowercase,
    fn(char) -> std::char::ToLowercase,
>;

/// Declaration-order substring search using Unicode lowercase expansion (no
/// normalization). The scoped node itself and all its descendants are eligible.
/// KMP comparisons and ancestry edges consume explicit work units; large names
/// are searched in place without allocating a lowercase copy of the hierarchy.
pub struct Search<'a> {
    reader: &'a Reader,
    scope: Option<NodeId>,
    needle: Vec<char>,
    prefix: Vec<usize>,
    _pattern_charge: crate::Reservation,
    node: u32,
    ancestor: Option<NodeId>,
    scoped: bool,
    name: Option<Folded<'a>>,
    pending: Option<char>,
    matched: usize,
    emit: bool,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
}
impl<'a> Search<'a> {
    pub fn new(
        reader: &'a Reader,
        scope: Option<u32>,
        needle: &str,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        if scope.is_some_and(|id| id as usize >= reader.hierarchy().len())
            || needle.len() > 64 * 1024
            || limits.work == 0
            || limits.records == 0
        {
            return Err(Error::Invalid("invalid hierarchy search"));
        }
        let count = needle.chars().flat_map(char::to_lowercase).count();
        let charge =
            budget.reserve(count * (std::mem::size_of::<char>() + std::mem::size_of::<usize>()))?;
        let mut pattern = Vec::new();
        let mut prefix = Vec::new();
        pattern
            .try_reserve_exact(count)
            .map_err(|_| Error::ResourceLimit)?;
        prefix
            .try_reserve_exact(count)
            .map_err(|_| Error::ResourceLimit)?;
        pattern.extend(needle.chars().flat_map(char::to_lowercase));
        prefix.resize(count, 0);
        for i in 1..count {
            if i % 256 == 0 {
                cancellation.check()?;
            }
            let mut matched = prefix[i - 1];
            while matched > 0 && pattern[i] != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if pattern[i] == pattern[matched] {
                matched += 1;
            }
            prefix[i] = matched;
        }
        Ok(Self {
            reader,
            scope: scope.map(NodeId),
            needle: pattern,
            prefix,
            _pattern_charge: charge,
            node: 0,
            ancestor: Some(NodeId(0)),
            scoped: false,
            name: None,
            pending: None,
            matched: 0,
            emit: false,
            limits,
            budget,
            cancellation,
        })
    }
    fn advance(&mut self) {
        self.node += 1;
        self.ancestor = Some(NodeId(self.node));
        self.scoped = false;
        self.name = None;
        self.pending = None;
        self.matched = 0;
        self.emit = false;
    }
    pub fn next_page(&mut self) -> Result<Arc<crate::metadata::SearchPage>> {
        use crate::metadata::SearchPage;
        self.cancellation.check()?;
        let available = self
            .limits
            .bytes
            .checked_sub(std::mem::size_of::<SearchPage>())
            .ok_or(Error::ResourceLimit)?;
        let capacity = self
            .limits
            .records
            .min(self.limits.work)
            .min(available / std::mem::size_of::<Declaration>());
        if capacity == 0 {
            return Err(Error::ResourceLimit);
        }
        let rows = capacity * std::mem::size_of::<Declaration>();
        let charge = self
            .budget
            .reserve(rows + std::mem::size_of::<SearchPage>())?;
        let mut declarations = Vec::new();
        declarations
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        let first = self.node;
        let mut remaining = available - rows;
        let mut work = self.limits.work;
        while (self.node as usize) < self.reader.hierarchy().len()
            && work > 0
            && declarations.len() < capacity
        {
            self.cancellation.check()?;
            work -= 1;
            if self.emit {
                declarations.push(declaration(
                    self.reader,
                    NodeId(self.node),
                    &self.budget,
                    &mut remaining,
                )?);
                self.advance();
                continue;
            }
            if !self.scoped {
                if self.scope.is_none() || self.ancestor == self.scope {
                    self.scoped = true;
                    if self.needle.is_empty() {
                        self.emit = true;
                    }
                } else if let Some(ancestor) = self.ancestor {
                    self.ancestor = self.reader.hierarchy().parent(ancestor);
                } else {
                    self.advance();
                }
                continue;
            }
            if self.name.is_none() {
                let name: &'a str = self.reader.name(NodeId(self.node));
                self.name = Some(
                    name.chars()
                        .flat_map(char::to_lowercase as fn(char) -> std::char::ToLowercase),
                );
            }
            let character = match self.pending.or_else(|| self.name.as_mut().unwrap().next()) {
                Some(character) => character,
                None => {
                    self.advance();
                    continue;
                }
            };
            self.pending = Some(character);
            if self.needle[self.matched] == character {
                self.matched += 1;
                self.pending = None;
                if self.matched == self.needle.len() {
                    self.emit = true;
                }
            } else if self.matched > 0 {
                self.matched = self.prefix[self.matched - 1];
            } else {
                self.pending = None;
            }
        }
        Ok(Arc::new(SearchPage {
            start_node: first,
            complete: self.node as usize == self.reader.hierarchy().len(),
            declarations,
            _charge: charge,
        }))
    }
}

/// Batched path resolution with resumable sibling traversal. The client sends
/// the complete paths once; it need not request one hierarchy level per RTT.
pub struct ResolvePaths<'a> {
    reader: &'a Reader,
    paths: Vec<crate::metadata::Path>,
    _request_charge: crate::Reservation,
    path: usize,
    segment: usize,
    parent: Option<NodeId>,
    child: usize,
    matches: u32,
    selected: Option<NodeId>,
    limits: Limits,
    budget: Budget,
    cancellation: Cancellation,
}
impl<'a> ResolvePaths<'a> {
    pub fn new(
        reader: &'a Reader,
        paths: Vec<crate::metadata::Path>,
        limits: Limits,
        budget: Budget,
        cancellation: Cancellation,
    ) -> Result<Self> {
        cancellation.check()?;
        if limits.records == 0 || limits.work == 0 {
            return Err(Error::Invalid("zero path-resolution limit"));
        }
        let mut bytes = paths
            .capacity()
            .checked_mul(std::mem::size_of::<crate::metadata::Path>())
            .ok_or(Error::ResourceLimit)?;
        for path in &paths {
            if path.kind.is_some_and(|kind| !(1..=5).contains(&kind)) {
                return Err(Error::Invalid("unknown node kind"));
            }
            bytes = bytes
                .checked_add(
                    path.segments
                        .capacity()
                        .checked_mul(std::mem::size_of::<String>())
                        .ok_or(Error::ResourceLimit)?,
                )
                .ok_or(Error::ResourceLimit)?;
            for name in &path.segments {
                bytes = bytes
                    .checked_add(name.capacity())
                    .ok_or(Error::ResourceLimit)?;
            }
        }
        let charge = budget.reserve(bytes)?;
        Ok(Self {
            reader,
            paths,
            _request_charge: charge,
            path: 0,
            segment: 0,
            parent: None,
            child: 0,
            matches: 0,
            selected: None,
            limits,
            budget,
            cancellation,
        })
    }
    fn finish_path(&mut self) {
        self.path += 1;
        self.segment = 0;
        self.parent = None;
        self.child = 0;
        self.matches = 0;
        self.selected = None;
    }
    pub fn next_page(&mut self) -> Result<Arc<crate::metadata::ResolvePage>> {
        use crate::metadata::{Resolution, ResolvePage};
        self.cancellation.check()?;
        let available = self
            .limits
            .bytes
            .checked_sub(std::mem::size_of::<ResolvePage>())
            .ok_or(Error::ResourceLimit)?;
        let capacity = self
            .limits
            .records
            .min(self.paths.len() - self.path)
            .min(available / std::mem::size_of::<Resolution>());
        if capacity == 0 && self.path < self.paths.len() {
            return Err(Error::ResourceLimit);
        }
        let charge = self.budget.reserve(
            std::mem::size_of::<ResolvePage>() + capacity * std::mem::size_of::<Resolution>(),
        )?;
        let mut results = Vec::new();
        results
            .try_reserve_exact(capacity)
            .map_err(|_| Error::ResourceLimit)?;
        let first = self.path;
        for _ in 0..self.limits.work {
            self.cancellation.check()?;
            if self.path == self.paths.len() || results.len() == capacity {
                break;
            }
            let path = &self.paths[self.path];
            if path.segments.is_empty() {
                results.push(Resolution::Missing);
                self.finish_path();
                continue;
            }
            let terminal = self.segment + 1 == path.segments.len();
            let child = match self.parent {
                Some(parent) => self.reader.hierarchy().children(parent).nth(self.child),
                None => self.reader.hierarchy().roots().nth(self.child),
            };
            if let Some(child) = child {
                self.child += 1;
                let kind = self.reader.hierarchy().kind(child) as u8;
                let kind_matches = if terminal {
                    path.kind.is_none_or(|expected| expected == kind)
                } else {
                    kind == vtr::NodeKind::Scope as u8
                };
                if kind_matches && self.reader.name(child) == path.segments[self.segment] {
                    if self.matches == path.occurrence.filter(|_| terminal).unwrap_or(0) {
                        self.selected = Some(child);
                    }
                    self.matches += 1;
                    if self.matches > 1 && (!terminal || path.occurrence.is_none()) {
                        results.push(Resolution::Ambiguous);
                        self.finish_path();
                    }
                }
                continue;
            }
            match self.selected {
                None => {
                    results.push(Resolution::Missing);
                    self.finish_path();
                }
                Some(node) if terminal => {
                    results.push(Resolution::Found(node.0));
                    self.finish_path();
                }
                Some(node) => {
                    self.parent = Some(node);
                    self.segment += 1;
                    self.child = 0;
                    self.matches = 0;
                    self.selected = None;
                }
            }
        }
        Ok(Arc::new(ResolvePage {
            offset: first,
            complete: self.path == self.paths.len(),
            results,
            _charge: charge,
        }))
    }
}
