//! Record identities, states, and independently selectable located-record pages.

use std::collections::{BTreeMap, HashSet};

use crate::{
    Compression, Error, Result,
    arguments::{ArgumentColumns, ArgumentReader},
    schema::{DEFAULT_PAYLOAD_LIMIT, TypeId, Types},
    varint::{len_u64, put_i64, put_u64, unzigzag},
    wire::{self, Cursor},
};

pub type RecordId = u64;
pub type GeneratorId = u32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    #[default]
    Unset,
    Ok,
    Error,
    Aborted,
    Open,
}

impl TryFrom<u8> for Status {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Unset),
            1 => Ok(Self::Ok),
            2 => Ok(Self::Error),
            3 => Ok(Self::Aborted),
            4 => Ok(Self::Open),
            _ => Err(Error::Corrupt("invalid record status")),
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unset => "unset",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Aborted => "aborted",
            Self::Open => "open",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum SpanKind {
    #[default]
    Unspecified,
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

impl TryFrom<u8> for SpanKind {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Unspecified),
            1 => Ok(Self::Internal),
            2 => Ok(Self::Server),
            3 => Ok(Self::Client),
            4 => Ok(Self::Producer),
            5 => Ok(Self::Consumer),
            _ => Err(Error::Corrupt("invalid record span kind")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RecordKind {
    Transaction,
    Clock,
    Event,
    Stage,
    Log,
    EmbeddedEvent,
    EmbeddedStage,
}

impl RecordKind {
    pub fn from_u8(kind: u8) -> Result<Self> {
        Ok(match kind {
            0 => Self::Transaction,
            1 => Self::Clock,
            2 => Self::Event,
            3 => Self::Stage,
            4 => Self::Log,
            5 => Self::EmbeddedEvent,
            6 => Self::EmbeddedStage,
            _ => return Err(Error::Corrupt("unknown record kind")),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generator {
    pub id: GeneratorId,
    pub kind: RecordKind,
    pub payload_type: TypeId,
}

#[derive(Clone, Default, Debug)]
pub struct Generators {
    prefix: Vec<Option<Generator>>,
    generators: BTreeMap<GeneratorId, Generator>,
}

impl Generators {
    pub(crate) fn memory_charge(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.prefix.capacity() * std::mem::size_of::<Option<Generator>>()
            + self.generators.len() * std::mem::size_of::<(GeneratorId, Generator)>()
    }
    pub fn payload_type(&self, generator: GeneratorId, datatype: Option<u32>) -> Result<TypeId> {
        let source = self.get(generator)?;
        match (source.kind, datatype) {
            (RecordKind::Log, Some(id)) => Ok(id),
            (RecordKind::Log, None) => Err(Error::Corrupt("log requires a datatype")),
            (_, None) => Ok(source.payload_type),
            (_, Some(_)) => Err(Error::Corrupt("fixed-schema record has a datatype")),
        }
    }

    pub fn declare(&mut self, generator: Generator) -> Result<()> {
        if generator.id == u32::MAX
            || generator.kind > RecordKind::Log
            || (matches!(generator.kind, RecordKind::Clock | RecordKind::Log)
                && generator.payload_type != 0)
        {
            return Err(Error::Corrupt("invalid generator declaration"));
        }
        if generator.id < 256 {
            self.prefix
                .resize(self.prefix.len().max(generator.id as usize + 1), None);
            let slot = &mut self.prefix[generator.id as usize];
            if slot.is_some() {
                return Err(Error::Corrupt("generator redeclared"));
            }
            *slot = Some(generator);
        } else {
            match self.generators.entry(generator.id) {
                std::collections::btree_map::Entry::Occupied(_) => {
                    return Err(Error::Corrupt("generator redeclared"));
                }
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(generator);
                }
            }
        }
        Ok(())
    }

    #[inline]
    pub fn get(&self, id: GeneratorId) -> Result<&Generator> {
        if id < 256 {
            return self
                .prefix
                .get(id as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| Error::corrupt("undeclared generator"));
        }
        self.generators
            .get(&id)
            .ok_or_else(|| Error::corrupt("undeclared generator"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedRecord {
    // A reserved absent ID preserves fixed-schema buffer budgets without an optional tag.
    pub datatype_id: u32,
    pub id: RecordId,
    pub generator: GeneratorId,
    pub begin: u64,
    pub end: u64,
    pub parent: RecordId,
    pub payload: Vec<u8>,
}

impl LocatedRecord {
    #[must_use]
    pub fn datatype(&self) -> Option<u32> {
        (self.datatype_id != u32::MAX).then_some(self.datatype_id)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RecordPolicy {
    pub page_bytes: usize,
    pub compression: Compression,
    pub separate_logs: bool,
}

impl Default for RecordPolicy {
    fn default() -> Self {
        Self {
            page_bytes: 64 << 10,
            compression: Compression::default(),
            separate_logs: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentHeader {
    pub datatype: Option<u32>,
    pub generator: GeneratorId,
    pub page: u32,
    pub count: u32,
    pub raw_offset: u32,
    pub raw_length: u32,
    pub min_id: RecordId,
    pub max_id: RecordId,
    pub min_parent: RecordId,
    pub max_parent: RecordId,
    pub begin: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageHeader {
    pub blob_offset: u32,
    pub blob_length: u32,
    pub raw_length: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct RecordQuery {
    pub datatype: Option<u32>,
    pub generator: Option<GeneratorId>,
    pub parent: Option<RecordId>,
    pub id: Option<RecordId>,
    pub kind: Option<RecordKind>,
    pub begin: u64,
    pub end: u64,
    pub include_logs: bool,
}

impl Default for RecordQuery {
    fn default() -> Self {
        Self {
            datatype: None,
            generator: None,
            parent: None,
            id: None,
            kind: None,
            begin: 0,
            end: u64::MAX,
            include_logs: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    count: u64,
    min_id: u64,
    max_id: u64,
    min_parent: u64,
    max_parent: u64,
    begin: u64,
    end: u64,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            count: 0,
            min_id: u64::MAX,
            max_id: 0,
            min_parent: u64::MAX,
            max_parent: 0,
            begin: u64::MAX,
            end: 0,
        }
    }
}

impl Bounds {
    fn add(&mut self, record: &LocatedRecord) {
        self.count += 1;
        self.min_id = self.min_id.min(record.id);
        self.max_id = self.max_id.max(record.id);
        if record.parent != 0 {
            self.min_parent = self.min_parent.min(record.parent);
            self.max_parent = self.max_parent.max(record.parent);
        }
        self.begin = self.begin.min(record.begin);
        self.end = self.end.max(record.end);
    }

    fn parent_min(self) -> u64 {
        if self.max_parent == 0 {
            0
        } else {
            self.min_parent
        }
    }

    fn matches(self, segment: &SegmentHeader) -> bool {
        self.count == u64::from(segment.count)
            && self.min_id == segment.min_id
            && self.max_id == segment.max_id
            && self.parent_min() == segment.min_parent
            && self.max_parent == segment.max_parent
            && self.begin == segment.begin
            && self.end == segment.end
    }
}

fn valid_record(record: &LocatedRecord, generator: &Generator) -> Result<()> {
    if record.id == 0
        || record.parent >= record.id
        || !(RecordKind::Event..=RecordKind::Log).contains(&generator.kind)
        || (generator.kind != RecordKind::Log && record.parent == 0)
        || record.begin > record.end
        || (generator.kind != RecordKind::Stage && record.begin != record.end)
    {
        return Err(Error::Corrupt("invalid located record"));
    }
    Ok(())
}

pub(crate) fn checked_u32(size: usize) -> Result<u32> {
    u32::try_from(size).map_err(|_| Error::invalid("block field exceeds u32"))
}

struct SegmentEncoder<'a> {
    columns: [Vec<u8>; 4],
    arguments: ArgumentColumns<'a>,
    previous_id: u64,
    previous_parent: u64,
    previous_end: u64,
}

impl SegmentEncoder<'_> {
    fn new(strings: u64) -> Self {
        Self {
            columns: std::array::from_fn(|_| Vec::new()),
            arguments: ArgumentColumns::new(strings, None),
            previous_id: 0,
            previous_parent: 0,
            previous_end: 0,
        }
    }

    fn append(
        &mut self,
        types: &Types,
        generator: &Generator,
        record: &LocatedRecord,
        datatype: TypeId,
    ) -> Result<()> {
        put_i64(
            &mut self.columns[0],
            record.begin.wrapping_sub(self.previous_end) as i64,
        );
        put_i64(
            &mut self.columns[1],
            record.id.wrapping_sub(self.previous_id) as i64,
        );
        if generator.kind == RecordKind::Log {
            put_u64(
                &mut self.columns[2],
                if record.parent == 0 {
                    0
                } else {
                    record.id - record.parent
                },
            );
        } else {
            put_i64(
                &mut self.columns[2],
                record.parent.wrapping_sub(self.previous_parent) as i64,
            );
        }
        if generator.kind == RecordKind::Stage {
            put_u64(&mut self.columns[3], record.end - record.begin);
        }
        self.arguments.append(types, datatype, &record.payload)?;
        self.previous_id = record.id;
        self.previous_parent = record.parent;
        self.previous_end = record.end;
        Ok(())
    }

    fn size(&self) -> usize {
        1 + self
            .columns
            .iter()
            .map(Vec::len)
            .chain(self.arguments.encoded_lengths())
            .map(|length| len_u64(length as u64) + length)
            .sum::<usize>()
    }

    fn finish(self) -> Result<Vec<u8>> {
        let mut columns = self.columns.into_iter().chain(self.arguments.finish());
        wire::pack_columns(&std::array::from_fn::<_, 9, _>(|_| columns.next().unwrap()))
    }
}

fn flush_page(
    raw: &mut Vec<u8>,
    pages: &mut Vec<PageHeader>,
    blobs: &mut Vec<u8>,
    compression: Compression,
) -> Result<()> {
    if raw.is_empty() {
        return Ok(());
    }
    let packed = wire::compress(raw, compression)?;
    pages.push(PageHeader {
        blob_offset: checked_u32(blobs.len())?,
        blob_length: checked_u32(packed.len())?,
        raw_length: checked_u32(raw.len())?,
    });
    blobs.extend_from_slice(&packed);
    raw.clear();
    Ok(())
}

pub fn encode_record_block(
    number: u64,
    types: &Types,
    generators: &Generators,
    records: &[LocatedRecord],
    policy: RecordPolicy,
    string_count: u64,
) -> Result<Vec<u8>> {
    if number == 0
        || records.is_empty()
        || policy.page_bytes == 0
        || policy.page_bytes >= wire::MAX_BLOCK_BYTES
    {
        return Err(Error::invalid("empty record block or invalid policy"));
    }
    let mut buckets: Vec<(Generator, Option<u32>, TypeId, Vec<&LocatedRecord>)> = Vec::new();
    let mut indices = BTreeMap::new();
    let mut ids = HashSet::new();
    for record in records {
        let generator = *generators.get(record.generator)?;
        valid_record(record, &generator)?;
        let datatype = generators.payload_type(record.generator, record.datatype())?;
        types.validate(datatype, &record.payload, string_count)?;
        if !ids.insert(record.id) {
            return Err(Error::invalid("duplicate global record identifier"));
        }
        let index = *indices
            .entry((record.generator, record.datatype()))
            .or_insert_with(|| {
                let index = buckets.len();
                buckets.push((generator, record.datatype(), datatype, Vec::new()));
                index
            });
        buckets[index].3.push(record);
    }
    if policy.separate_logs {
        buckets.sort_by_key(|(generator, ..)| generator.kind == RecordKind::Log);
    }
    let mut segments = Vec::new();
    let mut pages = Vec::new();
    let mut raw_page = Vec::new();
    let mut blobs = Vec::new();
    let mut all = Bounds::default();
    let mut had_activity = false;
    for (generator, selection, datatype, bucket) in buckets {
        if policy.separate_logs && generator.kind == RecordKind::Log && had_activity {
            flush_page(&mut raw_page, &mut pages, &mut blobs, policy.compression)?;
            had_activity = false;
        } else if generator.kind != RecordKind::Log {
            had_activity = true;
        }
        let mut first = 0;
        while first < bucket.len() {
            let mut encoder = SegmentEncoder::new(string_count);
            let mut bounds = Bounds::default();
            let mut index = first;
            while index < bucket.len() {
                let lengths = encoder.columns.each_ref().map(Vec::len);
                let checkpoint = encoder.arguments.checkpoint();
                encoder.append(types, &generator, bucket[index], datatype)?;
                let size = encoder.size();
                if index != first && size > policy.page_bytes {
                    for (column, length) in encoder.columns.iter_mut().zip(lengths) {
                        column.truncate(length);
                    }
                    encoder.arguments.restore(checkpoint);
                    break;
                }
                bounds.add(bucket[index]);
                all.add(bucket[index]);
                index += 1;
                if size >= policy.page_bytes {
                    break;
                }
            }
            let raw = encoder.finish()?;
            if !raw_page.is_empty() && raw.len() > policy.page_bytes.saturating_sub(raw_page.len())
            {
                flush_page(&mut raw_page, &mut pages, &mut blobs, policy.compression)?;
            }
            segments.push(SegmentHeader {
                datatype: selection,
                generator: generator.id,
                page: checked_u32(pages.len())?,
                count: checked_u32(index - first)?,
                raw_offset: checked_u32(raw_page.len())?,
                raw_length: checked_u32(raw.len())?,
                min_id: bounds.min_id,
                max_id: bounds.max_id,
                min_parent: bounds.parent_min(),
                max_parent: bounds.max_parent,
                begin: bounds.begin,
                end: bounds.end,
            });
            raw_page.extend_from_slice(&raw);
            if raw_page.len() >= policy.page_bytes {
                flush_page(&mut raw_page, &mut pages, &mut blobs, policy.compression)?;
            }
            first = index;
        }
    }
    flush_page(&mut raw_page, &mut pages, &mut blobs, policy.compression)?;
    let mut out = Vec::new();
    for value in [
        number, all.count, all.min_id, all.max_id, all.begin, all.end,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [checked_u32(segments.len())?, checked_u32(pages.len())?] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for segment in segments {
        out.extend_from_slice(&segment.generator.to_le_bytes());
        if let Some(datatype) = segment.datatype {
            out.extend_from_slice(&datatype.to_le_bytes());
        }
        for value in [
            segment.page,
            segment.count,
            segment.raw_offset,
            segment.raw_length,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [
            segment.min_id,
            segment.max_id,
            segment.min_parent,
            segment.max_parent,
            segment.begin,
            segment.end,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    for page in pages {
        for value in [page.blob_offset, page.blob_length, page.raw_length] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.extend_from_slice(&blobs);
    Ok(out)
}

pub struct RecordBlockView<'a> {
    pub number: u64,
    pub count: u64,
    pub min_id: u64,
    pub max_id: u64,
    pub begin: u64,
    pub end: u64,
    segments: Vec<SegmentHeader>,
    pages: Vec<PageHeader>,
    blobs: &'a [u8],
    types: &'a Types,
    generators: &'a Generators,
    string_count: u64,
    max_payload_bytes: usize,
}

impl<'a> RecordBlockView<'a> {
    pub fn new(
        encoded: &'a [u8],
        types: &'a Types,
        generators: &'a Generators,
        string_count: u64,
        max_payload_bytes: usize,
    ) -> Result<Self> {
        let mut cursor = Cursor::new(encoded);
        let number = cursor.u64()?;
        let count = cursor.u64()?;
        let min_id = cursor.u64()?;
        let max_id = cursor.u64()?;
        let begin = cursor.u64()?;
        let end = cursor.u64()?;
        if number == 0 || count == 0 || min_id == 0 || min_id > max_id || begin > end {
            return Err(Error::Corrupt("invalid record block bounds"));
        }
        let segment_count = cursor.u32()?;
        let page_count = cursor.u32()?;
        if segment_count == 0 || page_count == 0 || segment_count as usize > cursor.remaining() / 68
        {
            return Err(Error::Corrupt("invalid record segment count"));
        }
        let mut segments = Vec::new();
        for _ in 0..segment_count {
            let generator_id = cursor.u32()?;
            let generator = generators.get(generator_id)?;
            let datatype = if generator.kind == RecordKind::Log {
                Some(cursor.u32()?)
            } else {
                None
            };
            let segment = SegmentHeader {
                generator: generator_id,
                datatype,
                page: cursor.u32()?,
                count: cursor.u32()?,
                raw_offset: cursor.u32()?,
                raw_length: cursor.u32()?,
                min_id: cursor.u64()?,
                max_id: cursor.u64()?,
                min_parent: cursor.u64()?,
                max_parent: cursor.u64()?,
                begin: cursor.u64()?,
                end: cursor.u64()?,
            };
            let generator = generators.get(segment.generator)?;
            types.get(generators.payload_type(segment.generator, segment.datatype)?)?;
            if !(RecordKind::Event..=RecordKind::Log).contains(&generator.kind)
                || segment.page >= page_count
                || segment.count == 0
                || segment.raw_length == 0
                || segment.min_id == 0
                || segment.min_id > segment.max_id
                || segment.min_parent > segment.max_parent
                || (segment.min_parent == 0 && segment.max_parent != 0)
                || segment.begin > segment.end
                || (generator.kind != RecordKind::Log && segment.min_parent == 0)
            {
                return Err(Error::Corrupt("invalid record segment"));
            }
            segments.push(segment);
        }
        if page_count as usize > cursor.remaining() / 12 {
            return Err(Error::Corrupt("invalid record page count"));
        }
        let mut pages = Vec::new();
        let mut blob_end = 0u64;
        for _ in 0..page_count {
            let page = PageHeader {
                blob_offset: cursor.u32()?,
                blob_length: cursor.u32()?,
                raw_length: cursor.u32()?,
            };
            if page.blob_length == 0
                || page.raw_length == 0
                || u64::from(page.blob_offset) != blob_end
                || page.raw_length as usize >= wire::MAX_BLOCK_BYTES
            {
                return Err(Error::Corrupt("invalid record page partition"));
            }
            blob_end += u64::from(page.blob_length);
            pages.push(page);
        }
        let blobs = cursor.take(
            usize::try_from(blob_end).map_err(|_| Error::Corrupt("record blobs exceed usize"))?,
        )?;
        cursor.done()?;
        let mut rows = 0u64;
        let mut page = 0usize;
        let mut offset = 0u64;
        for segment in &segments {
            if segment.page as usize != page {
                if segment.page as usize != page + 1 || offset != u64::from(pages[page].raw_length)
                {
                    return Err(Error::Corrupt("invalid record segment partition"));
                }
                page += 1;
                offset = 0;
            }
            if u64::from(segment.raw_offset) != offset {
                return Err(Error::Corrupt("invalid record segment offset"));
            }
            offset += u64::from(segment.raw_length);
            if offset > u64::from(pages[page].raw_length) {
                return Err(Error::Corrupt("record segment exceeds page"));
            }
            rows += u64::from(segment.count);
        }
        if page + 1 != pages.len()
            || offset != u64::from(pages[page].raw_length)
            || rows != count
            || segments.iter().map(|s| s.min_id).min() != Some(min_id)
            || segments.iter().map(|s| s.max_id).max() != Some(max_id)
            || segments.iter().map(|s| s.begin).min() != Some(begin)
            || segments.iter().map(|s| s.end).max() != Some(end)
        {
            return Err(Error::Corrupt("record block aggregate bounds mismatch"));
        }
        Ok(Self {
            number,
            count,
            min_id,
            max_id,
            begin,
            end,
            segments,
            pages,
            blobs,
            types,
            generators,
            string_count,
            max_payload_bytes,
        })
    }

    pub fn with_defaults(
        encoded: &'a [u8],
        types: &'a Types,
        generators: &'a Generators,
    ) -> Result<Self> {
        Self::new(encoded, types, generators, u64::MAX, DEFAULT_PAYLOAD_LIMIT)
    }

    #[must_use]
    pub fn segments(&self) -> &[SegmentHeader] {
        &self.segments
    }
    #[must_use]
    pub fn pages(&self) -> &[PageHeader] {
        &self.pages
    }

    pub(crate) fn activity(
        &self,
        mut admit: impl FnMut(usize) -> Result<()>,
        mut visit: impl FnMut(GeneratorId, u64, u64) -> Result<()>,
    ) -> Result<()> {
        let mut cached = None;
        let mut raw = Vec::new();
        for segment in &self.segments {
            if cached != Some(segment.page) {
                let page = &self.pages[segment.page as usize];
                admit(page.raw_length as usize)?;
                let start = page.blob_offset as usize;
                raw = wire::decompress(
                    &self.blobs[start..start + page.blob_length as usize],
                    page.raw_length as usize,
                )?;
                cached = Some(segment.page);
            }
            let start = segment.raw_offset as usize;
            let columns =
                wire::unpack_columns::<9>(&raw[start..start + segment.raw_length as usize])?;
            let mut begins = Cursor::new(columns[0]);
            let mut lengths = Cursor::new(columns[3]);
            let stage = self.generators.get(segment.generator)?.kind == RecordKind::Stage;
            let mut previous_end = 0u64;
            for _ in 0..segment.count {
                let begin = previous_end.wrapping_add(unzigzag(begins.var()?) as u64);
                let end = begin
                    .checked_add(if stage { lengths.var()? } else { 0 })
                    .ok_or(Error::Corrupt("activity time overflow"))?;
                visit(
                    segment.generator,
                    begin,
                    if stage && end > begin { end - 1 } else { end },
                )?;
                previous_end = end;
            }
            begins.done()?;
            lengths.done()?;
        }
        Ok(())
    }

    /// Visitors return false to stop; records own their payload after this call.
    pub fn query(
        &self,
        query: RecordQuery,
        mut visit: impl FnMut(LocatedRecord) -> bool,
    ) -> Result<()> {
        if query.begin > query.end {
            return Err(Error::invalid("invalid record query window"));
        }
        let mut cached_page = None;
        let mut raw = Vec::new();
        for segment in &self.segments {
            let generator = self.generators.get(segment.generator)?;
            let datatype = self
                .generators
                .payload_type(generator.id, segment.datatype)?;
            if query.datatype.is_some_and(|id| id != datatype)
                || query.generator.is_some_and(|id| id != segment.generator)
                || (!query.include_logs && generator.kind == RecordKind::Log)
                || query.kind.is_some_and(|kind| kind != generator.kind)
                || query
                    .id
                    .is_some_and(|id| id < segment.min_id || id > segment.max_id)
                || query.parent.is_some_and(|id| {
                    id != 0 && (id < segment.min_parent || id > segment.max_parent)
                })
                || query.end < segment.begin
                || query.begin > segment.end
            {
                continue;
            }
            if cached_page != Some(segment.page) {
                let page = &self.pages[segment.page as usize];
                let start = page.blob_offset as usize;
                raw = wire::decompress(
                    &self.blobs[start..start + page.blob_length as usize],
                    page.raw_length as usize,
                )?;
                cached_page = Some(segment.page);
            }
            let start = segment.raw_offset as usize;
            let columns =
                wire::unpack_columns::<9>(&raw[start..start + segment.raw_length as usize])?;
            let mut metadata: [Cursor<'_>; 4] = std::array::from_fn(|i| Cursor::new(columns[i]));
            let mut arguments = ArgumentReader::new(
                std::array::from_fn(|i| columns[i + 4]),
                self.string_count,
                self.max_payload_bytes,
                &[],
            )?;
            let mut previous_id = 0u64;
            let mut previous_parent = 0u64;
            let mut previous_end = 0u64;
            let mut bounds = Bounds::default();
            for _ in 0..segment.count {
                let begin = previous_end.wrapping_add(unzigzag(metadata[0].var()?) as u64);
                let id = previous_id.wrapping_add(unzigzag(metadata[1].var()?) as u64);
                let parent = if generator.kind == RecordKind::Log {
                    let distance = metadata[2].var()?;
                    if distance >= id && distance != 0 {
                        return Err(Error::Corrupt("invalid log parent distance"));
                    }
                    if distance == 0 { 0 } else { id - distance }
                } else {
                    previous_parent.wrapping_add(unzigzag(metadata[2].var()?) as u64)
                };
                let duration = if generator.kind == RecordKind::Stage {
                    metadata[3].var()?
                } else {
                    0
                };
                let end = begin
                    .checked_add(duration)
                    .ok_or_else(|| Error::corrupt("stage end overflow"))?;
                let record = LocatedRecord {
                    datatype_id: segment.datatype.unwrap_or(u32::MAX),
                    id,
                    generator: generator.id,
                    begin,
                    end,
                    parent,
                    payload: arguments.read(self.types, datatype)?,
                };
                valid_record(&record, generator)?;
                bounds.add(&record);
                previous_id = id;
                previous_parent = parent;
                previous_end = end;
                let matches_time = begin <= query.end
                    && if begin == end {
                        begin >= query.begin
                    } else {
                        end > query.begin
                    };
                if matches_time
                    && query.parent.is_none_or(|wanted| wanted == parent)
                    && query.id.is_none_or(|wanted| wanted == id)
                    && !visit(record)
                {
                    return Ok(());
                }
            }
            for column in metadata {
                column.done()?;
            }
            arguments.done()?;
            if !bounds.matches(segment) {
                return Err(Error::Corrupt("record segment bounds mismatch"));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        let mut count = 0;
        let mut duplicate = false;
        self.query(RecordQuery::default(), |record| {
            count += 1;
            duplicate |= !ids.insert(record.id);
            true
        })?;
        if duplicate || count != self.count {
            return Err(Error::Corrupt(
                "duplicate record identifier or count mismatch",
            ));
        }
        Ok(())
    }
}
