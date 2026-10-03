//! Protocol v6 bounded hierarchy pages. Names and fixed-width columns are
//! retained directly; no node acquires an owned string or vector.
use super::decode::{Decoder, Reader, Step};
use super::memory::{MemoryBudget, Reservation};
use super::objects::Metadata;
use crate::data::hierarchy_columns::{
    ColumnBuilder, Links, NONE, SCOPE_WORDS, VAR_WORDS, direction, words_shape,
};
use crate::data::source::{HierarchySource, Ids, ScopeView, VariableView};
use crate::data::transactions::Track;
use crate::data::{Generator, Hierarchy, ScopeSize, ScopeSizes, SignalRef, TraceInfo};
use crate::session::{Capabilities, Session};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Maximum scope or variable declarations in one page.
pub const PAGE_ENTRIES: usize = 65_536;
const SCRATCH: u64 = (super::transport::DATA_BYTES + 8192) as u64;

/// Small raw catalog. Hierarchy declarations travel in separate objects.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub info: TraceInfo,
    pub capabilities: Capabilities,
    pub tracks: Vec<Track>,
    pub generators: Vec<Generator>,
    pub scopes: u32,
    pub vars: u32,
    pub activity: Option<super::activity::Descriptor>,
    pub server: String,
}
#[derive(Serialize)]
/// Borrowed catalog serialization; shares recording strings and slices.
pub struct HeaderView<'a> {
    info: &'a TraceInfo,
    capabilities: Capabilities,
    tracks: &'a [Track],
    generators: &'a [Generator],
    scopes: u32,
    vars: u32,
    activity: Option<super::activity::Descriptor>,
    server: String,
}
impl Header {
    /// Project resident raw metadata without copying record storage.
    pub fn borrowed(session: &dyn Session, server: String) -> anyhow::Result<HeaderView<'_>> {
        Ok(HeaderView {
            info: session.info(),
            capabilities: session.capabilities(),
            tracks: session.tracks(),
            generators: &session.hierarchy().generators,
            scopes: session.hierarchy().scope_count().try_into()?,
            vars: session.hierarchy().var_count().try_into()?,
            activity: super::activity::Descriptor::from_session(session),
            server,
        })
    }
    /// Owned catalog projection for fixtures; production serialization borrows.
    pub fn from_session(session: &dyn Session) -> anyhow::Result<Self> {
        Ok(Self {
            info: session.info().clone(),
            capabilities: session.capabilities(),
            tracks: session.tracks().to_vec(),
            generators: session.hierarchy().generators.as_ref().clone(),
            scopes: session.hierarchy().scope_count().try_into()?,
            vars: session.hierarchy().var_count().try_into()?,
            activity: super::activity::Descriptor::from_session(session),
            server: session.activity_host().unwrap_or("remote server").into(),
        })
    }
    /// Number of required scope objects, including a partial last page.
    pub fn scope_pages(&self) -> u32 {
        self.scopes.div_ceil(PAGE_ENTRIES as u32)
    }
    /// Number of required variable objects, including a partial last page.
    pub fn var_pages(&self) -> u32 {
        self.vars.div_ceil(PAGE_ENTRIES as u32)
    }
}

/// A page's column buffers plus its UTF-8 dictionary. Scope pages also carry
/// the server's distinct-signal, variable and subtree-scope totals.
#[derive(Debug, Serialize, Deserialize)]
pub struct Page {
    pub start: u32,
    pub ids: Vec<u32>,
    pub columns: Vec<Vec<u32>>,
    #[serde(serialize_with = "super::serialize_bytes")]
    pub names: Vec<u8>,
    pub offsets: Vec<u32>,
    pub sizes: Vec<ScopeSize>,
}
impl Page {
    /// Project one scope page. Panics if the page is out of range.
    pub fn scopes(h: &Hierarchy, sizes: &ScopeSizes, page: u32) -> Self {
        scope_pages(h, sizes)
            .nth(page as usize)
            .expect("scope page")
    }
    /// Project one variable page. Panics if the page is out of range.
    pub fn vars(h: &Hierarchy, page: u32) -> Self {
        var_pages(h).nth(page as usize).expect("variable page")
    }
    fn name(&self, id: u32) -> &str {
        let bytes =
            &self.names[self.offsets[id as usize] as usize..self.offsets[id as usize + 1] as usize];
        // SAFETY: ColumnBuilder accepts str; PageDecoder validates every name
        // before these private pages enter an immutable hierarchy owner.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
    fn rows(&self) -> usize {
        self.columns[0].len()
    }
}

struct ScopeOrder<'a> {
    hierarchy: &'a Hierarchy,
    pending: Vec<u32>,
}
impl<'a> ScopeOrder<'a> {
    fn new(hierarchy: &'a Hierarchy) -> Self {
        Self {
            hierarchy,
            pending: hierarchy.roots().iter().rev().map(|id| id as u32).collect(),
        }
    }
}
impl Iterator for ScopeOrder<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        let id = self.pending.pop()?;
        self.pending.extend(
            self.hierarchy
                .scope(id as usize)
                .children
                .iter()
                .rev()
                .map(|id| id as u32),
        );
        Some(id)
    }
}
struct VarOrder<'a> {
    scopes: ScopeOrder<'a>,
    vars: crate::data::source::IdsIter<'a>,
}
impl Iterator for VarOrder<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        loop {
            if let Some(id) = self.vars.next() {
                return Some(id as u32);
            }
            self.vars = self
                .scopes
                .hierarchy
                .scope(self.scopes.next()? as usize)
                .vars
                .iter();
        }
    }
}
/// Scope pages in preorder, preserving source-local identities in an ID column.
pub fn scope_pages<'a>(h: &'a Hierarchy, sizes: &'a ScopeSizes) -> impl Iterator<Item = Page> + 'a {
    let mut order = ScopeOrder::new(h);
    let mut start = 0u32;
    std::iter::from_fn(move || {
        let ids: Vec<_> = order.by_ref().take(PAGE_ENTRIES).collect();
        if ids.is_empty() {
            return None;
        }
        let mut builder = ColumnBuilder::default();
        for &id in &ids {
            let s = h.scope(id as usize);
            builder.push_scope(s.name, s.kind, s.component, s.parent, s.role);
        }
        let sizes = ids
            .iter()
            .map(|&id| sizes.get(id as usize).expect("scope size"))
            .collect();
        let page = Page {
            start,
            ids,
            columns: builder.columns.scopes.into(),
            names: builder.columns.names.bytes,
            offsets: builder.columns.names.offsets,
            sizes,
        };
        start += page.rows() as u32;
        Some(page)
    })
}
/// Variable pages grouped by preorder scope, in declaration order within it.
pub fn var_pages(h: &Hierarchy) -> impl Iterator<Item = Page> + '_ {
    let mut order = VarOrder {
        scopes: ScopeOrder::new(h),
        vars: Ids::default().iter(),
    };
    let mut start = 0u32;
    std::iter::from_fn(move || {
        let ids: Vec<_> = order.by_ref().take(PAGE_ENTRIES).collect();
        if ids.is_empty() {
            return None;
        }
        let mut builder = ColumnBuilder::default();
        for &id in &ids {
            builder.push_var(h.var(id as usize));
        }
        let page = Page {
            start,
            ids,
            columns: builder.columns.vars.into(),
            names: builder.columns.names.bytes,
            offsets: builder.columns.names.offsets,
            sizes: vec![],
        };
        start += page.rows() as u32;
        Some(page)
    })
}

pub(super) type Finished = (Metadata, Arc<ScopeSizes>, Arc<Vec<Reservation>>);
#[derive(Clone, Copy)]
pub(super) struct PageSpec {
    pub scopes: bool,
    pub start: u32,
    pub count: usize,
    pub total_scopes: u32,
    pub total_vars: u32,
    pub total_signals: usize,
}
pub(super) struct PageDecoder(Decoder<Page>);
impl PageDecoder {
    pub fn new(
        declared: u64,
        limit: u64,
        budget: &MemoryBudget,
        spec: PageSpec,
    ) -> anyhow::Result<Self> {
        let PageSpec {
            scopes,
            start,
            count,
            total_scopes,
            total_vars,
            total_signals,
        } = spec;
        let mut decoder = Decoder::new(declared, limit, budget, move |r| {
            Box::pin(async move {
                let actual_start = r.u32().await?;
                anyhow::ensure!(actual_start == start, "hierarchy page out of order");
                let ids = r.bounded_vector(4, count..=count, || r.u32()).await?;
                anyhow::ensure!(ids.len() == count, "invalid hierarchy ID column");
                let words = if scopes { SCOPE_WORDS } else { VAR_WORDS };
                let columns = r
                    .bounded_vector(8, words..=words, || async {
                        r.bounded_vector(4, count..=count, || r.u32()).await
                    })
                    .await?;
                anyhow::ensure!(
                    columns.len() == words && columns.iter().all(|c| c.len() == count),
                    "invalid hierarchy page column lengths"
                );
                let names = r.bytes().await?;
                let max_names = count * if scopes { 3 } else { 2 };
                let offsets = r.bounded_vector(4, 2..=max_names + 1, || r.u32()).await?;
                anyhow::ensure!(
                    offsets.len() >= 2
                        && offsets[0] == 0
                        && offsets.last().copied() == Some(names.len().try_into()?),
                    "invalid hierarchy name offsets"
                );
                for pair in offsets.windows(2) {
                    r.checkpoint().await;
                    anyhow::ensure!(
                        pair[0] <= pair[1] && pair[1] as usize <= names.len(),
                        "invalid hierarchy name range"
                    );
                    validate_name(&r, &names[pair[0] as usize..pair[1] as usize]).await?;
                }
                let size_count = if scopes { count } else { 0 };
                let sizes = r
                    .bounded_vector(12, size_count..=size_count, || async {
                        Ok(ScopeSize {
                            signals: r.u32().await?,
                            variables: r.u32().await?,
                            scopes: r.u32().await?,
                        })
                    })
                    .await?;
                anyhow::ensure!(
                    sizes.len() == if scopes { count } else { 0 },
                    "invalid hierarchy size column"
                );
                let page = Page {
                    start,
                    ids,
                    columns,
                    names,
                    offsets,
                    sizes,
                };
                for i in 0..count {
                    r.checkpoint().await;
                    anyhow::ensure!(
                        page.ids[i] < if scopes { total_scopes } else { total_vars },
                        "invalid hierarchy declaration ID"
                    );
                    let c = &page.columns;
                    let fields: &[usize] = if scopes { &[0, 1, 2] } else { &[0, 4] };
                    for &field in fields {
                        anyhow::ensure!(
                            (c[field][i] as usize) < page.offsets.len() - 1,
                            "invalid hierarchy name reference"
                        );
                    }
                    if scopes {
                        anyhow::ensure!(
                            c[3][i] == NONE || c[3][i] < page.ids[i],
                            "invalid hierarchy parent reference"
                        );
                        anyhow::ensure!(
                            c[4][i] <= 1 && (c[4][i] != 0 || c[5][i] == 0),
                            "invalid hierarchy scope role"
                        );
                        let size = page.sizes[i];
                        anyhow::ensure!(
                            size.scopes > 0
                                && size.scopes <= total_scopes
                                && size.variables <= total_vars
                                && size.variables >= size.signals
                                && size.signals as usize <= total_signals,
                            "invalid hierarchy scope size"
                        );
                    } else {
                        anyhow::ensure!(c[1][i] < total_scopes, "invalid hierarchy variable scope");
                        words_shape(c[2][i], c[3][i])?;
                        direction(c[5][i])?;
                        anyhow::ensure!(
                            c[5][i] & 4 != 0 || c[7][i] == 0,
                            "enum identity without presence flag"
                        );
                        anyhow::ensure!(
                            (c[6][i] as usize) < total_signals,
                            "invalid hierarchy signal reference"
                        );
                    }
                }
                Ok(page)
            })
        })?;
        // Wire lengths cover payload buffers. Vec headers in the column
        // directory are larger than their wire lengths, so admit those too
        // before reading even the first length or allocating a page buffer.
        let column_headers =
            if scopes { SCOPE_WORDS } else { VAR_WORDS } * std::mem::size_of::<Vec<u32>>();
        decoder.prepay(
            declared
                .checked_add(column_headers as u64)
                .ok_or_else(|| anyhow::anyhow!("hierarchy page allocation overflow"))?,
        )?;
        Ok(Self(decoder))
    }
    pub fn feed(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.0.feed(bytes)
    }
    pub fn step(&mut self) -> anyhow::Result<Step<Page>> {
        self.0.step()
    }
}

// Validate long strings in bounded byte batches, preserving UTF-8 boundaries.
async fn validate_name(r: &Reader, bytes: &[u8]) -> anyhow::Result<()> {
    let mut start = 0;
    while start < bytes.len() {
        r.checkpoint().await;
        let end = (start + 4096).min(bytes.len());
        match std::str::from_utf8(&bytes[start..end]) {
            Ok(_) => start = end,
            Err(error) if error.error_len().is_none() && end < bytes.len() => {
                // Only the final (up to three) bytes are incomplete. The next
                // batch begins at that character and includes its full tail.
                start += error.valid_up_to();
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RemoteHierarchy {
    scopes: Vec<Page>,
    vars: Vec<Page>,
    links: Links,
    scope_count: usize,
    var_count: usize,
    scope_positions: Vec<u32>,
    var_positions: Vec<u32>,
    _ownership: Arc<Vec<Reservation>>,
}
impl HierarchySource for RemoteHierarchy {
    fn scope_count(&self) -> usize {
        self.scope_count
    }
    fn var_count(&self) -> usize {
        self.var_count
    }
    fn roots(&self) -> Ids<'_> {
        Ids::from_slice(&self.links.roots)
    }
    fn scope(&self, id: usize) -> ScopeView<'_> {
        let position = self.scope_positions[id] as usize;
        let p = &self.scopes[position / PAGE_ENTRIES];
        let i = position % PAGE_ENTRIES;
        let c = &p.columns;
        ScopeView {
            name: p.name(c[0][i]),
            kind: p.name(c[1][i]),
            component: p.name(c[2][i]),
            parent: (c[3][i] != NONE).then_some(c[3][i] as usize),
            role: if c[4][i] == 0 {
                crate::data::ScopeRole::Scope
            } else {
                crate::data::ScopeRole::Stream {
                    track: crate::data::transactions::TrackRef(c[5][i]),
                }
            },
            children: self.links.children(id),
            vars: self.links.vars(id),
            generators: self.links.generators(id),
        }
    }
    fn var(&self, id: usize) -> VariableView<'_> {
        let position = self.var_positions[id] as usize;
        let p = &self.vars[position / PAGE_ENTRIES];
        let i = position % PAGE_ENTRIES;
        let c = &p.columns;
        VariableView {
            name: p.name(c[0][i]),
            scope: c[1][i] as usize,
            shape: words_shape(c[2][i], c[3][i]).expect("shape"),
            var_type: p.name(c[4][i]),
            direction: direction(c[5][i]).expect("direction"),
            signal: SignalRef(c[6][i]),
            enum_table: (c[5][i] & 4 != 0).then_some(c[7][i]),
        }
    }
    fn var_name(&self, id: usize) -> &str {
        let position = self.var_positions[id] as usize;
        let p = &self.vars[position / PAGE_ENTRIES];
        p.name(p.columns[0][position % PAGE_ENTRIES])
    }
    fn resident_bytes(&self) -> u64 {
        let pages = self
            .scopes
            .iter()
            .chain(&self.vars)
            .map(|p| {
                (p.ids.capacity() * 4
                    + p.columns.capacity() * std::mem::size_of::<Vec<u32>>()
                    + p.columns.iter().map(|c| c.capacity() * 4).sum::<usize>()
                    + p.names.capacity()
                    + p.offsets.capacity() * 4) as u64
            })
            .sum::<u64>();
        pages
            + self.links.resident_bytes()
            + ((self.scopes.capacity() + self.vars.capacity()) * std::mem::size_of::<Page>()
                + (self.scope_positions.capacity() + self.var_positions.capacity()) * 4)
                as u64
    }
}

async fn filled<T: Clone>(r: &Reader, len: usize, value: T) -> anyhow::Result<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)?;
    while out.len() < len {
        let end = (out.len() + PAGE_ENTRIES).min(len);
        out.resize(end, value.clone());
        r.yield_now().await;
    }
    Ok(out)
}
async fn copy_offsets(r: &Reader, dest: &mut [u32], source: &[u32]) {
    for (a, b) in dest
        .chunks_mut(PAGE_ENTRIES)
        .zip(source.chunks(PAGE_ENTRIES))
    {
        a.copy_from_slice(b);
        r.yield_now().await;
    }
}

pub(super) struct Assembly {
    pub header: Header,
    scopes: Vec<Page>,
    vars: Vec<Page>,
    reservations: Vec<Reservation>,
}
impl Assembly {
    pub fn new(header: Header, mut reservation: Reservation) -> anyhow::Result<Self> {
        reservation.shrink(SCRATCH)?;
        anyhow::ensure!(
            header.info.time_range.0 <= header.info.time_range.1,
            "reversed trace time range"
        );
        // Small page directories are also admitted before allocation.
        let pages = header.scope_pages() as usize + header.var_pages() as usize;
        reservation.grow(
            (pages * std::mem::size_of::<Page>()
                + (pages + 2) * std::mem::size_of::<Reservation>()
                + (pages + 1) * std::mem::size_of::<super::transport::ObjectId>())
                as u64,
        )?;
        let mut scopes = Vec::new();
        scopes.try_reserve_exact(header.scope_pages() as usize)?;
        let mut vars = Vec::new();
        vars.try_reserve_exact(header.var_pages() as usize)?;
        let mut reservations = Vec::new();
        reservations.try_reserve_exact(pages + 2)?;
        reservations.push(reservation);
        Ok(Self {
            header,
            scopes,
            vars,
            reservations,
        })
    }
    pub fn push(
        &mut self,
        scopes: bool,
        page: Page,
        mut reservation: Reservation,
    ) -> anyhow::Result<()> {
        reservation.shrink(SCRATCH)?;
        if scopes {
            self.scopes.push(page)
        } else {
            self.vars.push(page)
        };
        self.reservations.push(reservation);
        Ok(())
    }
    pub fn finish(self, limit: u64, budget: &MemoryBudget) -> anyhow::Result<Decoder<Finished>> {
        Decoder::new(0, limit, budget, move |r| {
            Box::pin(async move {
                let mut assembly = self;
                let scopes = assembly.header.scopes as usize;
                let vars = assembly.header.vars as usize;
                // CSR offsets, edges, cursors and sizes. Exact lengths, no per-node
                // hash tables; charge before constructing any of the indexes.
                let generators = assembly.header.generators.len();
                let index_charge = (scopes as u64 + 1) * 12
                    + scopes as u64 * 28
                    + vars as u64 * 8
                    + generators as u64 * 4;
                r.charge(usize::try_from(index_charge)?)?;
                let mut scope_positions = filled(&r, scopes, NONE).await?;
                let mut var_positions = filled(&r, vars, NONE).await?;
                let mut links = Links {
                    child_offsets: filled(&r, scopes + 1, 0).await?,
                    var_offsets: filled(&r, scopes + 1, 0).await?,
                    generator_offsets: filled(&r, scopes + 1, 0).await?,
                    ..Default::default()
                };
                let mut roots = 0;
                for p in &assembly.scopes {
                    for &parent in &p.columns[3] {
                        r.checkpoint().await;
                        if parent == NONE {
                            roots += 1;
                        }
                    }
                }
                links.roots.try_reserve_exact(roots)?;
                for p in &assembly.scopes {
                    for i in 0..p.rows() {
                        r.checkpoint().await;
                        let parent = p.columns[3][i];
                        let id = p.ids[i] as usize;
                        anyhow::ensure!(
                            scope_positions[id] == NONE,
                            "duplicate hierarchy scope ID"
                        );
                        scope_positions[id] = p.start + i as u32;
                        if parent == NONE {
                            links.roots.push(id as u32)
                        } else {
                            links.child_offsets[parent as usize + 1] += 1;
                        }
                    }
                }
                let mut previous_var = None;
                for p in &assembly.vars {
                    for i in 0..p.rows() {
                        r.checkpoint().await;
                        let id = p.ids[i] as usize;
                        let owner = scope_positions[p.columns[1][i] as usize];
                        anyhow::ensure!(
                            previous_var.is_none_or(
                                |(scope, var)| owner > scope || (owner == scope && id > var)
                            ),
                            "hierarchy variables out of preorder"
                        );
                        previous_var = Some((owner, id));
                        anyhow::ensure!(
                            var_positions[id] == NONE,
                            "duplicate hierarchy variable ID"
                        );
                        var_positions[id] = p.start + i as u32;
                        links.var_offsets[p.columns[1][i] as usize + 1] += 1;
                    }
                }
                for g in &assembly.header.generators {
                    r.checkpoint().await;
                    anyhow::ensure!(g.stream < scopes, "invalid generator stream");
                    links.generator_offsets[g.stream + 1] += 1;
                }
                for i in 0..scopes {
                    r.checkpoint().await;
                    links.child_offsets[i + 1] += links.child_offsets[i];
                    links.var_offsets[i + 1] += links.var_offsets[i];
                    links.generator_offsets[i + 1] += links.generator_offsets[i];
                }
                links.children = filled(&r, links.child_offsets[scopes] as usize, 0).await?;
                links.vars = filled(&r, vars, 0).await?;
                links.generators = filled(&r, generators, 0).await?;
                let mut cursor = filled(&r, scopes, 0).await?;
                copy_offsets(&r, &mut cursor, &links.child_offsets[..scopes]).await;
                for p in &assembly.scopes {
                    for i in 0..p.rows() {
                        r.checkpoint().await;
                        let parent = p.columns[3][i];
                        if parent != NONE {
                            let pos = &mut cursor[parent as usize];
                            links.children[*pos as usize] = p.ids[i];
                            *pos += 1;
                        }
                    }
                }
                copy_offsets(&r, &mut cursor, &links.var_offsets[..scopes]).await;
                for p in &assembly.vars {
                    for i in 0..p.rows() {
                        r.checkpoint().await;
                        let pos = &mut cursor[p.columns[1][i] as usize];
                        links.vars[*pos as usize] = p.ids[i];
                        *pos += 1;
                    }
                }
                copy_offsets(&r, &mut cursor, &links.generator_offsets[..scopes]).await;
                for (id, g) in assembly.header.generators.iter().enumerate() {
                    r.checkpoint().await;
                    let pos = &mut cursor[g.stream];
                    links.generators[*pos as usize] = id as u32;
                    *pos += 1;
                }
                // Reuse the admitted cursor as a preorder ancestor stack.
                let mut depth = 0;
                for p in &assembly.scopes {
                    for i in 0..p.rows() {
                        r.checkpoint().await;
                        let parent = p.columns[3][i];
                        if parent == NONE {
                            depth = 0;
                        } else {
                            while depth > 0 && cursor[depth - 1] != parent {
                                r.checkpoint().await;
                                depth -= 1;
                            }
                            anyhow::ensure!(depth > 0, "hierarchy scopes out of preorder");
                        }
                        cursor[depth] = p.ids[i];
                        depth += 1;
                    }
                }
                drop(cursor);
                let mut sizes = filled(&r, scopes, ScopeSize::default()).await?;
                // Move size storage into the shared owner, one column-sized buffer.
                for p in &assembly.scopes {
                    for (i, &size) in p.sizes.iter().enumerate() {
                        r.checkpoint().await;
                        sizes[p.ids[i] as usize] = size;
                    }
                }
                for (p, reservation) in assembly
                    .scopes
                    .iter_mut()
                    .zip(assembly.reservations.iter_mut().skip(1))
                {
                    r.checkpoint().await;
                    reservation
                        .shrink((p.sizes.capacity() * std::mem::size_of::<ScopeSize>()) as u64)?;
                    p.sizes = Vec::new();
                }
                let mut index_owner = r.take_since(SCRATCH)?;
                index_owner.shrink(
                    index_charge
                        - links.resident_bytes()
                        - ((sizes.capacity() * 12)
                            + (scope_positions.capacity() + var_positions.capacity()) * 4)
                            as u64,
                )?;
                assembly.reservations.push(index_owner);
                let ownership = Arc::new(assembly.reservations);
                let hierarchy = Hierarchy {
                    source: Arc::new(RemoteHierarchy {
                        scopes: assembly.scopes,
                        vars: assembly.vars,
                        links,
                        scope_count: scopes,
                        var_count: vars,
                        scope_positions,
                        var_positions,
                        _ownership: ownership.clone(),
                    }),
                    generators: Arc::new(assembly.header.generators),
                };
                let metadata = Metadata {
                    info: assembly.header.info,
                    hierarchy,
                    capabilities: assembly.header.capabilities,
                    tracks: assembly.header.tracks,
                    activity: assembly.header.activity,
                    server: assembly.header.server,
                };
                // Alias validation has one slot per signal, rather than per alias.
                r.charge(
                    metadata
                        .info
                        .signal_count
                        .checked_mul(8)
                        .ok_or_else(|| anyhow::anyhow!("signal validation overflow"))?,
                )?;
                let mut shapes = filled(&r, metadata.info.signal_count, (NONE, 0)).await?;
                for v in metadata.hierarchy.vars() {
                    r.checkpoint().await;
                    let shape = crate::data::hierarchy_columns::shape_words(v.shape);
                    let slot = &mut shapes[v.signal.0 as usize];
                    anyhow::ensure!(slot.0 == NONE || *slot == shape, "inconsistent alias shape");
                    *slot = shape;
                }
                // Catalog checks are bounded by streams/generators, not waveforms.
                validate_catalog(&metadata, &r).await?;
                Ok((
                    metadata,
                    Arc::new(ScopeSizes {
                        sizes,
                        _ownership: Some(ownership.clone()),
                    }),
                    ownership,
                ))
            })
        })
    }
}
async fn validate_catalog(metadata: &Metadata, r: &Reader) -> anyhow::Result<()> {
    use crate::data::ScopeRole;
    use crate::data::transactions::TrackKind;
    let mut tracks = std::collections::HashMap::new();
    r.charge((metadata.tracks.len() + metadata.hierarchy.generators.len()) * 128)?;
    for t in &metadata.tracks {
        r.checkpoint().await;
        anyhow::ensure!(
            tracks.insert(t.id, &t.kind).is_none(),
            "duplicate track identity"
        );
    }
    for t in &metadata.tracks {
        r.checkpoint().await;
        if let TrackKind::Generator { stream } = t.kind {
            anyhow::ensure!(
                matches!(tracks.get(&stream), Some(TrackKind::Stream { .. })),
                "invalid generator stream"
            );
        }
    }
    let mut seen = std::collections::HashSet::new();
    for s in metadata.hierarchy.scopes() {
        r.checkpoint().await;
        if let ScopeRole::Stream { track } = s.role {
            anyhow::ensure!(seen.insert(track), "duplicate hierarchy track");
            anyhow::ensure!(
                matches!(tracks.get(&track),Some(TrackKind::Stream {kind}) if kind==s.kind),
                "invalid hierarchy stream"
            );
        }
    }
    for g in metadata.hierarchy.generators.iter() {
        r.checkpoint().await;
        anyhow::ensure!(seen.insert(g.track), "duplicate hierarchy generator");
        let role = metadata.hierarchy.scope(g.stream).role;
        anyhow::ensure!(
            matches!(tracks.get(&g.track),Some(TrackKind::Generator {stream}) if role==ScopeRole::Stream {track:*stream}),
            "invalid hierarchy generator"
        );
    }
    anyhow::ensure!(
        metadata.capabilities.transactions || metadata.tracks.is_empty(),
        "tracks without transaction capability"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(bytes: Vec<u8>) -> (bool, usize) {
        let budget = MemoryBudget::new(1024 * 1024);
        let mut decoder = Decoder::new(0, 0, &budget, move |r| {
            Box::pin(async move { validate_name(&r, &bytes).await })
        })
        .unwrap();
        let mut yields = 0;
        let valid = loop {
            match decoder.step() {
                Ok(Step::Yield) => yields += 1,
                Ok(Step::Ready((), reservation)) => {
                    drop(reservation);
                    break true;
                }
                Err(_) => break false,
                Ok(Step::NeedInput) => panic!("validation requires no input"),
            }
        };
        drop(decoder);
        assert_eq!(budget.used(), 0);
        (valid, yields)
    }

    #[test]
    fn name_validation_preserves_utf8_across_every_batch_boundary() {
        for prefix in 0..8 {
            let text = "a".repeat(4096 - prefix) + "λ中😀tail";
            assert!(check(text.clone().into_bytes()).0);
            let mut bytes = text.into_bytes();
            bytes.pop();
            bytes.push(0xff);
            assert!(!check(bytes).0);
        }
        assert!(
            !check(vec![0xf0, 0x9f, 0x98]).0,
            "incomplete final character"
        );
    }

    #[test]
    fn very_long_names_yield_during_validation() {
        let text = "λ😀".repeat(1_000_000).into_bytes();
        let (valid, yields) = check(text);
        assert!(valid);
        assert!(
            yields > 0,
            "validation work is bounded by bytes as well as rows"
        );
    }
}
