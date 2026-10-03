//! Flat resident hierarchy storage and its streaming builder.
use super::source::*;
use super::value::SignalShape;
use std::collections::HashMap;

pub(crate) const NONE: u32 = u32::MAX;
pub(crate) const SCOPE_WORDS: usize = 6;
pub(crate) const VAR_WORDS: usize = 8;

/// UTF-8 dictionary, with no allocation per name in the resident model.
#[derive(Debug, Default)]
pub(crate) struct Names {
    pub bytes: Vec<u8>,
    pub offsets: Vec<u32>,
}
impl Names {
    pub fn get(&self, id: u32) -> &str {
        let i = id as usize;
        let bytes = &self.bytes[self.offsets[i] as usize..self.offsets[i + 1] as usize];
        // SAFETY: Only ColumnBuilder creates this dictionary, from valid str
        // values. The immutable owner never exposes its buffers for mutation.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }
    pub fn resident_bytes(&self) -> u64 {
        (self.bytes.capacity() + 4 * self.offsets.capacity()) as u64
    }
}

/// Compressed sparse row membership, shared by reader and column views.
#[derive(Debug, Default)]
pub(crate) struct Links {
    pub roots: Vec<u32>,
    pub children: Vec<u32>,
    pub child_offsets: Vec<u32>,
    pub vars: Vec<u32>,
    pub var_offsets: Vec<u32>,
    pub generators: Vec<u32>,
    pub generator_offsets: Vec<u32>,
}
fn csr(scopes: usize, owners: impl Iterator<Item = Option<usize>> + Clone) -> (Vec<u32>, Vec<u32>) {
    let mut offsets = vec![0u32; scopes + 1];
    for owner in owners.clone().flatten() {
        offsets[owner + 1] += 1;
    }
    for i in 0..scopes {
        offsets[i + 1] += offsets[i];
    }
    let mut ids = vec![0; *offsets.last().unwrap() as usize];
    let mut cursor = offsets[..scopes].to_vec();
    for (id, owner) in owners.enumerate() {
        if let Some(owner) = owner {
            ids[cursor[owner] as usize] = id as u32;
            cursor[owner] += 1;
        }
    }
    (offsets, ids)
}
impl Links {
    pub fn build(
        parents: &[u32],
        vars: impl Iterator<Item = usize> + Clone,
        generators: &[Generator],
    ) -> Self {
        let n = parents.len();
        let (child_offsets, children) = csr(
            n,
            parents.iter().map(|&p| (p != NONE).then_some(p as usize)),
        );
        let (var_offsets, vars) = csr(n, vars.map(Some));
        let (generator_offsets, generators) = csr(n, generators.iter().map(|g| Some(g.stream)));
        Self {
            roots: parents
                .iter()
                .enumerate()
                .filter_map(|(i, &p)| (p == NONE).then_some(i as u32))
                .collect(),
            children,
            child_offsets,
            vars,
            var_offsets,
            generators,
            generator_offsets,
        }
    }
    fn range<'a>(offsets: &[u32], ids: &'a [u32], id: usize) -> Ids<'a> {
        Ids::from_slice(&ids[offsets[id] as usize..offsets[id + 1] as usize])
    }
    pub fn children(&self, id: usize) -> Ids<'_> {
        Self::range(&self.child_offsets, &self.children, id)
    }
    pub fn vars(&self, id: usize) -> Ids<'_> {
        Self::range(&self.var_offsets, &self.vars, id)
    }
    pub fn generators(&self, id: usize) -> Ids<'_> {
        Self::range(&self.generator_offsets, &self.generators, id)
    }
    pub fn resident_bytes(&self) -> u64 {
        [
            &self.roots,
            &self.children,
            &self.child_offsets,
            &self.vars,
            &self.var_offsets,
            &self.generators,
            &self.generator_offsets,
        ]
        .iter()
        .map(|v| (v.capacity() * 4) as u64)
        .sum()
    }
}

#[derive(Debug, Default)]
pub(crate) struct Columns {
    pub scopes: [Vec<u32>; SCOPE_WORDS],
    pub vars: [Vec<u32>; VAR_WORDS],
    pub names: Names,
    pub links: Links,
}
pub(crate) fn shape_words(shape: SignalShape) -> (u32, u32) {
    match shape {
        SignalShape::Event => (0, 0),
        SignalShape::Bit => (1, 1),
        SignalShape::Vector { width } => (2, width),
        SignalShape::Real => (3, 0),
        SignalShape::Text => (4, 0),
    }
}
pub(crate) fn words_shape(kind: u32, width: u32) -> anyhow::Result<SignalShape> {
    Ok(match (kind, width) {
        (0, 0) => SignalShape::Event,
        (1, 1) => SignalShape::Bit,
        (2, 2..) => SignalShape::Vector { width },
        (3, 0) => SignalShape::Real,
        (4, 0) => SignalShape::Text,
        _ => anyhow::bail!("invalid hierarchy signal shape"),
    })
}
pub(crate) fn direction(code: u32) -> anyhow::Result<Direction> {
    anyhow::ensure!(code <= 7, "invalid hierarchy direction flags");
    Ok(match code & 3 {
        0 => Direction::None,
        1 => Direction::Input,
        2 => Direction::Output,
        3 => Direction::InOut,
        _ => anyhow::bail!("invalid hierarchy direction"),
    })
}
impl HierarchySource for Columns {
    fn scope_count(&self) -> usize {
        self.scopes[0].len()
    }
    fn var_count(&self) -> usize {
        self.vars[0].len()
    }
    fn roots(&self) -> Ids<'_> {
        Ids::from_slice(&self.links.roots)
    }
    fn scope(&self, id: usize) -> ScopeView<'_> {
        let c = &self.scopes;
        ScopeView {
            name: self.names.get(c[0][id]),
            kind: self.names.get(c[1][id]),
            component: self.names.get(c[2][id]),
            parent: (c[3][id] != NONE).then_some(c[3][id] as usize),
            role: if c[4][id] == 0 {
                ScopeRole::Scope
            } else {
                ScopeRole::Stream {
                    track: super::transactions::TrackRef(c[5][id]),
                }
            },
            children: self.links.children(id),
            vars: self.links.vars(id),
            generators: self.links.generators(id),
        }
    }
    fn var(&self, id: usize) -> VariableView<'_> {
        let c = &self.vars;
        VariableView {
            name: self.names.get(c[0][id]),
            scope: c[1][id] as usize,
            shape: words_shape(c[2][id], c[3][id]).expect("validated shape"),
            var_type: self.names.get(c[4][id]),
            direction: direction(c[5][id]).expect("validated direction"),
            signal: SignalRef(c[6][id]),
            enum_table: (c[5][id] & 4 != 0).then_some(c[7][id]),
        }
    }
    fn var_name(&self, id: usize) -> &str {
        self.names.get(self.vars[0][id])
    }
    fn resident_bytes(&self) -> u64 {
        self.scopes
            .iter()
            .chain(&self.vars)
            .map(|v| (v.capacity() * 4) as u64)
            .sum::<u64>()
            + self.names.resident_bytes()
            + self.links.resident_bytes()
    }
}

#[derive(Default, Debug)]
struct IdentityHash(u64);
impl std::hash::Hasher for IdentityHash {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.wrapping_mul(257).wrapping_add(byte as u64);
        }
    }
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}
/// Streaming column builder. The dictionary's hash index is discarded at finish.
#[derive(Debug, Default)]
pub(crate) struct ColumnBuilder {
    pub columns: Columns,
    names: HashMap<u64, u32, std::hash::BuildHasherDefault<IdentityHash>>,
    pub generators: Vec<Generator>,
}
impl ColumnBuilder {
    fn intern(&mut self, text: &str) -> u32 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        let mut hash = hasher.finish();
        while let Some(&id) = self.names.get(&hash) {
            if self.columns.names.get(id) == text {
                return id;
            }
            hash = hash.wrapping_add(1);
        }
        let table = &mut self.columns.names;
        if table.offsets.is_empty() {
            table.offsets.push(0);
        }
        let id = table.len() as u32;
        table.bytes.extend_from_slice(text.as_bytes());
        table.offsets.push(
            u32::try_from(table.bytes.len()).expect("hierarchy name dictionary exceeds 4 GiB"),
        );
        self.names.insert(hash, id);
        id
    }
    pub fn push_scope(
        &mut self,
        name: &str,
        kind: &str,
        component: &str,
        parent: Option<usize>,
        role: ScopeRole,
    ) -> usize {
        let id = self.columns.scope_count();
        let (role, track) = match role {
            ScopeRole::Scope => (0, 0),
            ScopeRole::Stream { track } => (1, track.0),
        };
        let row = [
            self.intern(name),
            self.intern(kind),
            self.intern(component),
            parent.map_or(NONE, |p| p as u32),
            role,
            track,
        ];
        for (column, value) in self.columns.scopes.iter_mut().zip(row) {
            column.push(value);
        }
        id
    }
    pub fn push_var(&mut self, v: VariableView<'_>) -> usize {
        let id = self.columns.var_count();
        let (kind, width) = shape_words(v.shape);
        let row = [
            self.intern(v.name),
            v.scope as u32,
            kind,
            width,
            self.intern(v.var_type),
            v.direction as u32 | (u32::from(v.enum_table.is_some()) << 2),
            v.signal.0,
            v.enum_table.unwrap_or(0),
        ];
        for (column, value) in self.columns.vars.iter_mut().zip(row) {
            column.push(value);
        }
        id
    }
    pub fn finish(mut self) -> Hierarchy {
        self.columns.links = Links::build(
            &self.columns.scopes[3],
            self.columns.vars[1].iter().map(|&s| s as usize),
            &self.generators,
        );
        for c in self.columns.scopes.iter_mut().chain(&mut self.columns.vars) {
            c.shrink_to_fit();
        }
        self.columns.names.bytes.shrink_to_fit();
        self.columns.names.offsets.shrink_to_fit();
        Hierarchy {
            source: std::sync::Arc::new(self.columns),
            generators: std::sync::Arc::new(self.generators),
        }
    }
}
impl HierarchyBuilder {
    /// Freeze owned fixture declarations into the production column model.
    pub fn finish(self) -> Hierarchy {
        let mut out = ColumnBuilder::default();
        for scope in self.scopes {
            out.push_scope(
                &scope.name,
                &scope.kind,
                &scope.component,
                scope.parent,
                scope.role,
            );
        }
        for v in self.vars {
            out.push_var(VariableView {
                name: &v.name,
                scope: v.scope,
                shape: v.shape,
                var_type: &v.var_type,
                direction: v.direction,
                signal: v.signal,
                enum_table: v.enum_table,
            });
        }
        out.generators = self.generators;
        out.finish()
    }
}
impl From<HierarchyBuilder> for Hierarchy {
    fn from(builder: HierarchyBuilder) -> Self {
        builder.finish()
    }
}
