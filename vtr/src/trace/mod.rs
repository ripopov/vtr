//! Retained trace records, selective navigation, and independent full-file validation.

mod format;
mod hierarchy;
mod payload;
pub use format::FormatLocale;
mod validation;
mod waveform;

pub use payload::PayloadView;
pub use validation::{MissingKind, MissingReference, ValidationReport};
pub use waveform::{SignalData, TimeWindow};

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, Mutex},
};

use crate::{
    Error, Result,
    attributes::{Attribute, Value},
    declarations::{Declarations, Metadata, NodeData},
    framing::{Completion, Container, Fingerprint, RecordingId, Section},
    hierarchy::{NodeId, SignalId},
    records::{
        GeneratorId, LocatedRecord, RecordBlockView, RecordId, RecordKind, RecordQuery, SpanKind,
        Status,
    },
    schema::{DEFAULT_PAYLOAD_LIMIT, Payload, Types},
    transactions::{
        EmbeddedEvent, EmbeddedStage, Relation, Transaction, TransactionBlockView, TransactionQuery,
    },
};

#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub verify_checksums: bool,
    pub max_payload_bytes: usize,
    pub owner_cache_entries: usize,
    pub owner_cache_bytes: usize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            verify_checksums: true,
            max_payload_bytes: DEFAULT_PAYLOAD_LIMIT,
            owner_cache_entries: 4096,
            owner_cache_bytes: 32 << 20,
        }
    }
}

#[derive(Debug)]
struct RecordTypes {
    types: Types,
    origin: RecordingId,
    allocation_limit: usize,
}

impl std::ops::Deref for RecordTypes {
    type Target = Types;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.types
    }
}

#[derive(Clone, Debug)]
struct RecordData {
    raw: Transaction,
    kind: RecordKind,
    types: Arc<RecordTypes>,
    datatype: crate::schema::TypeId,
}

#[derive(Clone, Copy, Debug)]
enum Embedded {
    Event(usize),
    Stage(usize),
}

#[derive(Clone, Debug)]
pub struct Record {
    data: Arc<RecordData>,
    embedded: Option<Embedded>,
}

impl Record {
    #[must_use]
    #[inline]
    pub fn id(&self) -> RecordId {
        match self.embedded {
            None => self.data.raw.id,
            Some(Embedded::Event(i)) => self.data.raw.events[i].id,
            Some(Embedded::Stage(i)) => self.data.raw.stages[i].id,
        }
    }
    #[must_use]
    #[inline]
    pub fn kind(&self) -> RecordKind {
        match self.embedded {
            None => self.data.kind,
            Some(Embedded::Event(_)) => RecordKind::EmbeddedEvent,
            Some(Embedded::Stage(_)) => RecordKind::EmbeddedStage,
        }
    }
    #[must_use]
    #[inline]
    pub fn parent(&self) -> Option<RecordId> {
        let parent = if self.embedded.is_some() {
            self.data.raw.id
        } else {
            self.data.raw.parent
        };
        (parent != 0).then_some(parent)
    }
    #[must_use]
    #[inline]
    pub fn datatype(&self) -> crate::TypeId {
        if self.embedded.is_some() {
            0
        } else {
            self.data.datatype
        }
    }
    #[must_use]
    #[inline]
    pub fn generator(&self) -> Option<GeneratorId> {
        self.embedded.is_none().then_some(self.data.raw.generator)
    }
    #[must_use]
    #[inline]
    pub fn begin(&self) -> u64 {
        match self.embedded {
            None => self.data.raw.begin,
            Some(Embedded::Event(i)) => self.data.raw.events[i].time,
            Some(Embedded::Stage(i)) => self.data.raw.stages[i].begin,
        }
    }
    #[must_use]
    #[inline]
    pub fn end(&self) -> u64 {
        match self.embedded {
            None => self.data.raw.end,
            Some(Embedded::Event(i)) => self.data.raw.events[i].time,
            Some(Embedded::Stage(i)) => {
                let stage = &self.data.raw.stages[i];
                stage.end.unwrap_or(stage.begin.max(self.data.raw.end))
            }
        }
    }
    #[must_use]
    #[inline]
    pub fn status(&self) -> Status {
        if self.embedded.is_none() {
            Status::try_from(self.data.raw.status).expect("validated record status")
        } else {
            Status::Unset
        }
    }
    #[must_use]
    pub fn span_kind(&self) -> SpanKind {
        if self.embedded.is_none() {
            SpanKind::try_from(self.data.raw.span_kind).expect("validated record span kind")
        } else {
            SpanKind::Unspecified
        }
    }
    #[must_use]
    pub fn attributes(&self) -> &[Attribute] {
        match self.embedded {
            None => &self.data.raw.attributes,
            Some(Embedded::Event(i)) => &self.data.raw.events[i].attributes,
            Some(Embedded::Stage(i)) => &self.data.raw.stages[i].attributes,
        }
    }
    /// Decode the payload without retaining a separate payload view.
    pub fn decode<T: Payload>(&self) -> Result<T> {
        let (datatype, bytes) = if self.embedded.is_some() {
            (0, &[][..])
        } else {
            (self.data.datatype, self.data.raw.payload.as_slice())
        };
        payload::decode_payload(
            &self.data.types,
            datatype,
            bytes,
            self.data.types.allocation_limit,
        )
    }
    #[must_use]
    #[inline]
    pub fn payload(&self) -> PayloadView {
        let (datatype, length) = if self.embedded.is_some() {
            (0, 0)
        } else {
            (self.data.datatype, self.data.raw.payload.len())
        };
        PayloadView {
            types: self.data.types.clone(),
            storage: self.data.clone(),
            datatype,
            range: 0..length,
            allocation_limit: self.data.types.allocation_limit,
        }
    }
    #[must_use]
    pub fn events(&self) -> &[EmbeddedEvent] {
        if self.embedded.is_none() {
            &self.data.raw.events
        } else {
            &[]
        }
    }
    #[must_use]
    pub fn stages(&self) -> &[EmbeddedStage] {
        if self.embedded.is_none() {
            &self.data.raw.stages
        } else {
            &[]
        }
    }
    #[must_use]
    pub fn child_blocks(&self) -> &[u64] {
        if self.embedded.is_none() {
            &self.data.raw.child_blocks
        } else {
            &[]
        }
    }
    #[must_use]
    pub fn name(&self) -> Option<u32> {
        match self.embedded {
            None => None,
            Some(Embedded::Event(i)) => Some(self.data.raw.events[i].name),
            Some(Embedded::Stage(i)) => Some(self.data.raw.stages[i].name),
        }
    }
    #[must_use]
    pub fn lane(&self) -> Option<u32> {
        match self.embedded {
            Some(Embedded::Stage(i)) => Some(self.data.raw.stages[i].lane),
            _ => None,
        }
    }
    #[must_use]
    pub fn explicit_end(&self) -> Option<u64> {
        match self.embedded {
            Some(Embedded::Stage(i)) => self.data.raw.stages[i].end,
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Expansion {
    pub owner: Record,
    pub located: Vec<Record>,
    pub missing_blocks: Vec<u64>,
    pub complete: bool,
}

#[derive(Clone, Copy)]
struct Availability {
    strings: u64,
    types: usize,
    nodes: usize,
}

#[derive(Default)]
struct OwnerCache {
    entries: BTreeMap<RecordId, (u64, usize, Record)>,
    tick: u64,
    bytes: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct CacheStats {
    pub owner_entries: usize,
    pub owner_bytes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blackout {
    pub time: u64,
    pub active: bool,
}

pub struct Reader {
    container: Container,
    declarations: Declarations,
    metadata: Metadata,
    types: Arc<RecordTypes>,
    blocks: BTreeMap<u64, usize>,
    availability: HashMap<u64, Availability>,
    tree: hierarchy::Tree,
    waves: Vec<waveform::WaveBlock>,
    blackouts: Vec<Blackout>,
    options: ReadOptions,
    cache: Mutex<OwnerCache>,
}

impl Reader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, ReadOptions::default())
    }
    pub fn open_with(path: impl AsRef<Path>, options: ReadOptions) -> Result<Self> {
        Self::from_container(Container::open(path, options.verify_checksums)?, options)
    }
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::from_bytes_with(bytes, ReadOptions::default())
    }
    pub fn from_bytes_with(bytes: Vec<u8>, options: ReadOptions) -> Result<Self> {
        Self::from_container(
            Container::from_bytes(bytes, options.verify_checksums)?,
            options,
        )
    }
    fn from_container(container: Container, options: ReadOptions) -> Result<Self> {
        let (declarations, metadata) = Declarations::from_container(&container)?;
        let types = Arc::new(RecordTypes {
            origin: container.identity,
            types: declarations.types.clone(),
            allocation_limit: options.max_payload_bytes,
        });
        let mut available = Availability {
            strings: 0,
            types: crate::schema::FIRST_DECLARED_TYPE as usize,
            nodes: 0,
        };
        let mut blocks = BTreeMap::new();
        let mut availability = HashMap::new();
        for (index, section) in container.sections().iter().enumerate() {
            match section.kind {
                2 => {
                    available.strings = section
                        .aux0
                        .checked_add(section.aux1)
                        .ok_or_else(|| Error::corrupt("string count overflow"))?;
                }
                3 => {
                    available.nodes = usize::try_from(section.aux0 + section.aux1)
                        .map_err(|_| Error::Corrupt("node count overflow"))?;
                }
                10 => {
                    available.types = usize::try_from(section.aux0 + section.aux1)
                        .map_err(|_| Error::Corrupt("datatype count overflow"))?;
                }
                5 | 9 => {
                    if blocks.insert(section.block_number, index).is_some() {
                        return Err(Error::Corrupt("duplicate block number"));
                    }
                    availability.insert(section.block_number, available);
                }
                _ => (),
            }
        }
        let waves = waveform::parse_blocks(&container, &declarations, &metadata)?;
        let mut blackouts = Vec::new();
        for section in container
            .sections()
            .iter()
            .filter(|section| section.kind == 6)
        {
            let mut cursor = crate::wire::Cursor::new(container.payload(section, true)?);
            let count = cursor.var()?;
            if count > (cursor.remaining() / 2) as u64 {
                return Err(Error::Corrupt("invalid blackout count"));
            }
            let mut time = 0u64;
            for _ in 0..count {
                let active = match cursor.byte()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Corrupt("invalid blackout state")),
                };
                time = time
                    .checked_add(cursor.var()?)
                    .ok_or_else(|| Error::corrupt("blackout time overflow"))?;
                if blackouts
                    .last()
                    .is_some_and(|last: &Blackout| last.time > time)
                {
                    return Err(Error::Corrupt("blackout time moved backwards"));
                }
                blackouts.push(Blackout { time, active });
            }
            cursor.done()?;
        }
        let tree = hierarchy::Tree::new(&declarations.nodes);
        let reader = Self {
            tree,
            container,
            declarations,
            metadata,
            types,
            blocks,
            availability,
            waves,
            blackouts,
            options,
            cache: Mutex::default(),
        };
        for section in reader.container.sections() {
            if section.kind == 5 {
                reader.transaction_block(section)?;
            }
            if section.kind == 9 {
                reader.record_block(section)?;
            }
        }
        Ok(reader)
    }

    pub fn blackouts(&self) -> &[Blackout] {
        &self.blackouts
    }
    pub fn identity(&self) -> RecordingId {
        self.container.identity
    }
    pub fn fingerprint(&self) -> Option<Fingerprint> {
        self.container.fingerprint
    }
    pub fn completion(&self) -> Completion {
        self.container.completion
    }
    pub fn dropped_bytes(&self) -> u64 {
        self.container.dropped
    }
    /// Return the recorded run ending, or recovery provenance for an unfinished trace.
    pub fn ending(&self) -> Result<(crate::recording::Ending, Option<u64>)> {
        use crate::recording::{
            Ending,
            ending::{MARKER, RunEnding, SCOPE},
        };
        let mut sources = Vec::new();
        for (id, node) in self.declarations.nodes.iter().enumerate() {
            if !matches!(node.data, NodeData::Generator { .. })
                || self.declarations.generators.get(id as u32)?.kind != RecordKind::Log
            {
                continue;
            }
            if node
                .attributes
                .iter()
                .any(|attribute| self.string(attribute.key).ok() == Some(MARKER))
            {
                let valid_marker = node.attributes.iter().any(|attribute| {
                    self.string(attribute.key).ok() == Some(MARKER)
                        && attribute.value == Value::Bool(true)
                });
                if !valid_marker
                    || node.parent.is_none_or(|parent| {
                        let parent = &self.declarations.nodes[parent.0 as usize];
                        parent.parent.is_some()
                            || self.string(parent.name).ok() != Some(SCOPE)
                            || !matches!(parent.data, NodeData::Scope { .. })
                    })
                {
                    return Err(Error::Corrupt("invalid run-ending source"));
                }
                sources.push(id as u32);
            }
        }
        let mut ending = None;
        self.records(
            RecordQuery {
                kind: Some(RecordKind::Log),
                ..RecordQuery::default()
            },
            |record| {
                if record
                    .generator()
                    .is_some_and(|generator| sources.contains(&generator))
                {
                    if ending.is_some() {
                        ending = Some(Err(Error::Corrupt("multiple run-ending records")));
                    } else {
                        ending = Some(
                            record
                                .payload()
                                .field("value")
                                .and_then(|value| value.decode::<RunEnding>())
                                .and_then(|data| data.ending())
                                .map(|end| (end, Some(record.begin()))),
                        );
                    }
                }
                true
            },
        )?;
        ending.unwrap_or_else(|| {
            Ok((
                if self.completion() == Completion::Complete {
                    Ending::Closed
                } else {
                    Ending::Recovered {
                        dropped: self.dropped_bytes(),
                    }
                },
                None,
            ))
        })
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn declarations(&self) -> &Declarations {
        &self.declarations
    }
    pub fn types(&self) -> &Types {
        &self.types
    }
    pub fn sections(&self) -> &[Section] {
        self.container.sections()
    }

    /// Bounds of waveforms, owners, located records, and embedded detail.
    /// Pages containing embedded detail are decompressed to read their time columns;
    /// payloads and attributes are not materialized, and owner caches stay unchanged.
    pub fn time_range(&self) -> Result<Option<TimeWindow>> {
        let mut range = self.waveform_time_range();
        for section in self.sections() {
            let next = match section.kind {
                5 => self.transaction_block(section)?.time_range()?,
                9 => Some((section.aux0, section.aux1)),
                _ => None,
            };
            if let Some((first, last)) = next {
                range = Some(match range {
                    Some(range) => TimeWindow {
                        first: range.first.min(first),
                        last: range.last.max(last),
                    },
                    None => TimeWindow { first, last },
                });
            }
        }
        Ok(range)
    }
    pub fn string(&self, id: u32) -> Result<&str> {
        self.declarations.string(id)
    }
    pub fn find(&self, path: &str) -> Option<NodeId> {
        self.declarations.find(path)
    }
    pub fn find_generator(&self, path: &str) -> Option<GeneratorId> {
        self.declarations
            .find_matching(path, |data| matches!(data, NodeData::Generator { .. }))
            .map(|id| id.0)
    }
    pub fn find_signal(&self, path: &str) -> Option<SignalId> {
        match self
            .declarations
            .nodes
            .get(
                self.declarations
                    .find_matching(path, |data| matches!(data, NodeData::Signal { .. }))?
                    .0 as usize,
            )?
            .data
        {
            NodeData::Signal { signal, .. } => Some(signal),
            _ => None,
        }
    }

    fn available_generator(&self, id: GeneratorId, available: Availability) -> Result<()> {
        if id as usize >= available.nodes
            || self.declarations.generators.get(id)?.payload_type as usize >= available.types
        {
            return Err(Error::Corrupt(
                "record uses a source or datatype before its declaration",
            ));
        }
        Ok(())
    }
    fn transaction_block(&self, section: &Section) -> Result<TransactionBlockView<'_>> {
        let available = self.availability[&section.block_number];
        let view = TransactionBlockView::new(
            self.container.payload(section, false)?,
            &self.types,
            &self.declarations.generators,
            available.strings,
            self.options.max_payload_bytes,
            &self.declarations.strings,
        )?;
        if view.number != section.block_number
            || view.bounds.begin != section.aux0
            || view.bounds.end != section.aux1
        {
            return Err(Error::Corrupt("transaction directory bounds disagree"));
        }
        for id in view.generators() {
            self.available_generator(*id, available)?;
        }
        Ok(view)
    }
    fn record_block(&self, section: &Section) -> Result<RecordBlockView<'_>> {
        let available = self.availability[&section.block_number];
        let view = RecordBlockView::new(
            self.container.payload(section, false)?,
            &self.types,
            &self.declarations.generators,
            available.strings,
            self.options.max_payload_bytes,
        )?;
        if view.number != section.block_number
            || view.begin != section.aux0
            || view.end != section.aux1
        {
            return Err(Error::Corrupt("record directory bounds disagree"));
        }
        for segment in view.segments() {
            self.available_generator(segment.generator, available)?;
            if let Some(id) = segment.datatype
                && id as usize >= available.types
            {
                return Err(Error::Corrupt(
                    "record uses a datatype before its declaration",
                ));
            }
        }
        Ok(view)
    }

    #[inline]
    fn retain(&self, raw: Transaction, kind: RecordKind) -> Record {
        let NodeData::Generator { payload_type, .. } =
            self.declarations.nodes[raw.generator as usize].data
        else {
            unreachable!("validated record generator");
        };
        self.retain_payload(raw, kind, payload_type)
    }
    #[inline]
    fn retain_payload(
        &self,
        raw: Transaction,
        kind: RecordKind,
        datatype: crate::TypeId,
    ) -> Record {
        Record {
            data: Arc::new(RecordData {
                raw,
                kind,
                types: self.types.clone(),
                datatype,
            }),
            embedded: None,
        }
    }
    #[inline]
    fn retain_located(&self, raw: LocatedRecord) -> Record {
        let source = self
            .declarations
            .generators
            .get(raw.generator)
            .expect("validated generator");
        let datatype = raw.datatype().unwrap_or(source.payload_type);
        self.retain_payload(
            Transaction {
                id: raw.id,
                parent: raw.parent,
                generator: raw.generator,
                begin: raw.begin,
                end: raw.end,
                payload: raw.payload,
                ..Transaction::default()
            },
            source.kind,
            datatype,
        )
    }

    pub fn transactions(
        &self,
        query: TransactionQuery,
        mut visit: impl FnMut(Record) -> bool,
    ) -> Result<()> {
        if query.begin > query.end {
            return Err(Error::invalid("invalid transaction query window"));
        }
        let mut running = true;
        for section in self.sections().iter().filter(|section| section.kind == 5) {
            if section.aux0 > query.end || section.aux1 < query.begin {
                continue;
            }
            self.transaction_block(section)?
                .transactions(query, |raw| {
                    let NodeData::Generator { kind, .. } =
                        self.declarations.nodes[raw.generator as usize].data
                    else {
                        unreachable!("validated transaction generator");
                    };
                    let kind = if self.declarations.strings[kind as usize] == "CLOCK" {
                        RecordKind::Clock
                    } else {
                        RecordKind::Transaction
                    };
                    running = visit(self.retain(raw, kind));
                    running
                })?;
            if !running {
                break;
            }
        }
        Ok(())
    }
    /// Ordinary child transactions, excluding located records and embedded children.
    pub fn child_transactions(
        &self,
        parent: RecordId,
        mut visit: impl FnMut(Record) -> bool,
    ) -> Result<()> {
        self.transactions(
            TransactionQuery {
                parent: Some(parent),
                ..TransactionQuery::default()
            },
            |record| record.kind() != RecordKind::Transaction || visit(record),
        )
    }

    /// Logs belonging to one owner within inclusive time bounds.
    pub fn logs(
        &self,
        parent: RecordId,
        time: TimeWindow,
        visit: impl FnMut(Record) -> bool,
    ) -> Result<()> {
        self.records(
            RecordQuery {
                parent: Some(parent),
                kind: Some(RecordKind::Log),
                begin: time.first,
                end: time.last,
                ..RecordQuery::default()
            },
            visit,
        )
    }

    pub fn records(&self, query: RecordQuery, mut visit: impl FnMut(Record) -> bool) -> Result<()> {
        if query.begin > query.end {
            return Err(Error::invalid("invalid record query window"));
        }
        let mut running = true;
        for section in self.sections().iter().filter(|section| section.kind == 9) {
            if section.aux0 > query.end || section.aux1 < query.begin {
                continue;
            }
            self.record_block(section)?.query(query, |raw| {
                running = visit(self.retain_located(raw));
                running
            })?;
            if !running {
                break;
            }
        }
        Ok(())
    }
    pub fn relations(
        &self,
        endpoint: RecordId,
        incoming: bool,
        mut visit: impl FnMut(Relation) -> bool,
    ) -> Result<()> {
        let mut running = true;
        for section in self.sections().iter().filter(|section| section.kind == 5) {
            self.transaction_block(section)?
                .relations(endpoint, incoming, |relation| {
                    running = visit(relation);
                    running
                })?;
            if !running {
                break;
            }
        }
        Ok(())
    }

    /// Visits every stored relation in block/page order, preserving duplicates.
    pub fn scan_relations(&self, mut visit: impl FnMut(Relation) -> bool) -> Result<()> {
        let mut running = true;
        for section in self.sections().iter().filter(|section| section.kind == 5) {
            self.transaction_block(section)?.scan(
                |_| true,
                |relation| {
                    running = visit(relation);
                    running
                },
            )?;
            if !running {
                break;
            }
        }
        Ok(())
    }

    fn cache_owner(&self, record: Record) {
        let bytes = record.data.raw.payload.capacity()
            + std::mem::size_of::<RecordData>()
            + record.child_blocks().len() * 8
            + std::mem::size_of_val(record.events())
            + std::mem::size_of_val(record.stages())
            + record
                .attributes()
                .iter()
                .map(|a| attribute_bytes(&a.value))
                .sum::<usize>()
            + record
                .events()
                .iter()
                .flat_map(|e| &e.attributes)
                .chain(record.stages().iter().flat_map(|s| &s.attributes))
                .map(|a| attribute_bytes(&a.value))
                .sum::<usize>();
        if self.options.owner_cache_entries == 0 || bytes > self.options.owner_cache_bytes {
            return;
        }
        let mut cache = self.cache.lock().unwrap();
        if cache.entries.contains_key(&record.id()) {
            return;
        }
        while !cache.entries.is_empty()
            && (cache.entries.len() >= self.options.owner_cache_entries
                || bytes > self.options.owner_cache_bytes.saturating_sub(cache.bytes))
        {
            let id = *cache
                .entries
                .iter()
                .min_by_key(|(_, (tick, _, _))| *tick)
                .unwrap()
                .0;
            let (_, removed, _) = cache.entries.remove(&id).unwrap();
            cache.bytes -= removed;
        }
        cache.tick = cache.tick.wrapping_add(1);
        let tick = cache.tick;
        cache.bytes += bytes;
        cache.entries.insert(record.id(), (tick, bytes, record));
    }
    fn cached(&self, id: RecordId) -> Option<Record> {
        let mut cache = self.cache.lock().unwrap();
        cache.tick = cache.tick.wrapping_add(1);
        let tick = cache.tick;
        let entry = cache.entries.get_mut(&id)?;
        entry.0 = tick;
        Some(entry.2.clone())
    }
    pub fn lookup(&self, id: RecordId) -> Result<Option<Record>> {
        if id == 0 {
            return Ok(None);
        }
        if let Some(record) = self.cached(id) {
            return Ok(Some(record));
        }
        let mut found = None;
        self.transactions(
            TransactionQuery {
                id: Some(id),
                ..TransactionQuery::default()
            },
            |record| {
                found = Some(record);
                false
            },
        )?;
        if let Some(owner) = found {
            self.cache_owner(owner.clone());
            return Ok(Some(owner));
        }
        self.records(
            RecordQuery {
                id: Some(id),
                ..RecordQuery::default()
            },
            |record| {
                found = Some(record);
                false
            },
        )?;
        if found.is_some() {
            return Ok(found);
        }
        self.transactions(
            TransactionQuery {
                embedded_id: Some(id),
                ..TransactionQuery::default()
            },
            |owner| {
                let embedded = owner
                    .events()
                    .iter()
                    .position(|event| event.id == id)
                    .map(Embedded::Event)
                    .or_else(|| {
                        owner
                            .stages()
                            .iter()
                            .position(|stage| stage.id == id)
                            .map(Embedded::Stage)
                    });
                found = Some(Record {
                    embedded,
                    ..owner.clone()
                });
                self.cache_owner(owner);
                false
            },
        )?;
        Ok(found)
    }
    pub fn expand(&self, id: RecordId) -> Result<Expansion> {
        let owner = self
            .lookup(id)?
            .filter(|record| record.kind() == RecordKind::Transaction)
            .ok_or_else(|| Error::invalid("expansion requires an ordinary owner"))?;
        let mut located = Vec::new();
        let mut missing_blocks = Vec::new();
        for number in owner.child_blocks() {
            let Some(index) = self.blocks.get(number) else {
                missing_blocks.push(*number);
                continue;
            };
            let section = &self.sections()[*index];
            if section.kind != 9 {
                return Err(Error::Corrupt("child locator references wrong block kind"));
            }
            self.record_block(section)?.query(
                RecordQuery {
                    parent: Some(id),
                    include_logs: false,
                    ..RecordQuery::default()
                },
                |raw| {
                    located.push(self.retain_located(raw));
                    true
                },
            )?;
        }
        located.sort_unstable_by_key(Record::id);
        let complete = self.completion() == Completion::Complete && missing_blocks.is_empty();
        Ok(Expansion {
            owner,
            located,
            missing_blocks,
            complete,
        })
    }
    pub fn cache_stats(&self) -> CacheStats {
        let cache = self.cache.lock().unwrap();
        CacheStats {
            owner_entries: cache.entries.len(),
            owner_bytes: cache.bytes,
        }
    }
    pub fn clear_cache(&self) {
        *self.cache.lock().unwrap() = OwnerCache::default();
    }
    pub fn clock_for(&self, generator: GeneratorId) -> Result<Option<GeneratorId>> {
        self.declarations.generators.get(generator)?;
        for attribute in &self.declarations.nodes[generator as usize].attributes {
            if self.string(attribute.key)? == "vtr.clock" {
                let Value::U32(clock) = attribute.value else {
                    return Err(Error::Corrupt("clock association is not u32"));
                };
                match self.declarations.generators.get(clock) {
                    Ok(generator) if generator.kind == RecordKind::Clock => return Ok(Some(clock)),
                    Err(_)
                        if clock as usize >= self.declarations.nodes.len()
                            && self.completion() != Completion::Complete =>
                    {
                        return Ok(None);
                    }
                    _ => return Err(Error::Corrupt("invalid clock association")),
                }
            }
        }
        Ok(None)
    }
    pub fn cycle_at(&self, generator: GeneratorId, time: u64) -> Result<Option<u64>> {
        if self.declarations.generators.get(generator)?.kind != RecordKind::Clock {
            return Err(Error::invalid("cycle query requires a clock"));
        }
        let mut stretches = Vec::new();
        self.transactions(
            TransactionQuery {
                generator: Some(generator),
                ..TransactionQuery::default()
            },
            |record| {
                stretches.push(record);
                true
            },
        )?;
        stretches.sort_unstable_by_key(Record::begin);
        let mut previous_end = None;
        let mut next_cycle = Some(0u64);
        let mut answer = None;
        for stretch in stretches {
            if previous_end.is_some_and(|end| stretch.begin() <= end) {
                return Err(Error::Corrupt("clock stretches overlap"));
            }
            let next_cycle_value =
                next_cycle.ok_or_else(|| Error::corrupt("clock cycle exhaustion"))?;
            let period = self.clock_period(&stretch.data.raw)?;
            let count = (stretch.end() - stretch.begin()) / period;
            let last = next_cycle_value
                .checked_add(count)
                .ok_or_else(|| Error::corrupt("clock cycle overflow"))?;
            if time >= stretch.begin() {
                answer =
                    Some(next_cycle_value + (time.min(stretch.end()) - stretch.begin()) / period);
            }
            previous_end = Some(stretch.end());
            next_cycle = last.checked_add(1);
        }
        Ok(answer)
    }
    fn clock_period(&self, tx: &Transaction) -> Result<u64> {
        let mut period = None;
        for attribute in &tx.attributes {
            if self.string(attribute.key)? == "vtr.period" {
                let Value::Time(step) = attribute.value else {
                    return Err(Error::Corrupt("clock period is not a time"));
                };
                if step == 0 {
                    return Err(Error::Corrupt("zero clock period"));
                }
                period = Some(step);
            }
        }
        if tx.begin == tx.end {
            if period.is_some() {
                return Err(Error::Corrupt("single-edge clock has a period"));
            }
            return Ok(1);
        }
        let period = period.ok_or_else(|| Error::corrupt("missing clock period"))?;
        if !(tx.end - tx.begin).is_multiple_of(period) {
            return Err(Error::Corrupt("clock stretch does not match period"));
        }
        Ok(period)
    }
}

fn attribute_bytes(value: &Value) -> usize {
    std::mem::size_of::<Attribute>()
        + match value {
            Value::Text(text) => text.len(),
            Value::Bytes(bytes)
            | Value::Bits { data: bytes, .. }
            | Value::Logic { data: bytes, .. }
            | Value::Logic9 { data: bytes, .. } => bytes.len(),
            Value::List(values) => values.iter().map(attribute_bytes).sum(),
            Value::Map(values) => values.iter().map(|a| attribute_bytes(&a.value)).sum(),
            _ => 0,
        }
}
