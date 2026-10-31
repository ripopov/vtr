//! Ordinary owners, embedded detail, relations, and scalar or general TX pages.

mod page;
mod scalar;
mod time;

use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet},
};

use crate::{
    Error, Result,
    attributes::{Attributes, validate_attributes},
    records::{
        Generator, GeneratorId, Generators, RecordId, RecordKind, RecordPolicy, checked_u32,
    },
    schema::{DEFAULT_PAYLOAD_LIMIT, Types},
    wire::{self, Cursor},
};

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedEvent {
    pub id: RecordId,
    pub time: u64,
    pub name: u32,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedStage {
    pub id: RecordId,
    pub name: u32,
    pub lane: u32,
    pub begin: u64,
    pub end: Option<u64>,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub id: RecordId,
    pub parent: RecordId,
    pub generator: GeneratorId,
    pub begin: u64,
    pub end: u64,
    pub status: u8,
    pub span_kind: u8,
    pub payload: Vec<u8>,
    pub attributes: Attributes,
    pub events: Vec<EmbeddedEvent>,
    pub stages: Vec<EmbeddedStage>,
    pub child_blocks: Vec<u64>,
}

impl Default for Transaction {
    fn default() -> Self {
        Self {
            id: 0,
            parent: 0,
            generator: u32::MAX,
            begin: 0,
            end: 0,
            status: 0,
            span_kind: 0,
            payload: vec![],
            attributes: vec![],
            events: vec![],
            stages: vec![],
            child_blocks: vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    pub kind: u32,
    pub source: RecordId,
    pub destination: RecordId,
    pub attributes: Attributes,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TransactionBounds {
    pub transactions: u64,
    pub relations: u64,
    pub min_id: u64,
    pub max_id: u64,
    pub begin: u64,
    pub end: u64,
    pub min_parent: u64,
    pub max_parent: u64,
    pub min_embedded: u64,
    pub max_embedded: u64,
    pub min_from: u64,
    pub max_from: u64,
    pub min_to: u64,
    pub max_to: u64,
}

fn extend(minimum: &mut u64, maximum: &mut u64, value: u64, first: bool) {
    *minimum = if first { value } else { (*minimum).min(value) };
    *maximum = (*maximum).max(value);
}

impl TransactionBounds {
    fn add_embedded(&mut self, id: RecordId) {
        let first = self.max_embedded == 0;
        extend(&mut self.min_embedded, &mut self.max_embedded, id, first);
    }

    fn add_transaction(&mut self, transaction: &Transaction) {
        let first = self.transactions == 0;
        self.transactions += 1;
        extend(&mut self.min_id, &mut self.max_id, transaction.id, first);
        if first {
            self.begin = transaction.begin;
            self.end = transaction.end;
        } else {
            self.begin = self.begin.min(transaction.begin);
            self.end = self.end.max(transaction.end);
        }
        if transaction.parent != 0 {
            let first = self.max_parent == 0;
            extend(
                &mut self.min_parent,
                &mut self.max_parent,
                transaction.parent,
                first,
            );
        }
        for id in transaction
            .events
            .iter()
            .map(|e| e.id)
            .chain(transaction.stages.iter().map(|s| s.id))
        {
            let first = self.max_embedded == 0;
            extend(&mut self.min_embedded, &mut self.max_embedded, id, first);
        }
    }

    fn add_relation(&mut self, relation: &Relation) {
        let first = self.relations == 0;
        self.relations += 1;
        extend(
            &mut self.min_from,
            &mut self.max_from,
            relation.source,
            first,
        );
        extend(
            &mut self.min_to,
            &mut self.max_to,
            relation.destination,
            first,
        );
    }

    fn merge(&mut self, other: Self) -> Result<()> {
        if other.transactions != 0 {
            let first = self.transactions == 0;
            extend(&mut self.min_id, &mut self.max_id, other.min_id, first);
            self.max_id = self.max_id.max(other.max_id);
            if first {
                self.begin = other.begin;
                self.end = other.end;
            } else {
                self.begin = self.begin.min(other.begin);
                self.end = self.end.max(other.end);
            }
            if other.max_parent != 0 {
                let first = self.max_parent == 0;
                extend(
                    &mut self.min_parent,
                    &mut self.max_parent,
                    other.min_parent,
                    first,
                );
                self.max_parent = self.max_parent.max(other.max_parent);
            }
            if other.max_embedded != 0 {
                let first = self.max_embedded == 0;
                extend(
                    &mut self.min_embedded,
                    &mut self.max_embedded,
                    other.min_embedded,
                    first,
                );
                self.max_embedded = self.max_embedded.max(other.max_embedded);
            }
            self.transactions = self
                .transactions
                .checked_add(other.transactions)
                .ok_or_else(|| Error::corrupt("transaction count overflow"))?;
        }
        if other.relations != 0 {
            let first = self.relations == 0;
            extend(
                &mut self.min_from,
                &mut self.max_from,
                other.min_from,
                first,
            );
            self.max_from = self.max_from.max(other.max_from);
            extend(&mut self.min_to, &mut self.max_to, other.min_to, first);
            self.max_to = self.max_to.max(other.max_to);
            self.relations = self
                .relations
                .checked_add(other.relations)
                .ok_or_else(|| Error::corrupt("relation count overflow"))?;
        }
        Ok(())
    }

    fn write(self, out: &mut Vec<u8>) {
        for value in [
            self.transactions,
            self.relations,
            self.min_id,
            self.max_id,
            self.begin,
            self.end,
            self.min_parent,
            self.max_parent,
            self.min_embedded,
            self.max_embedded,
            self.min_from,
            self.max_from,
            self.min_to,
            self.max_to,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }

    fn read(cursor: &mut Cursor<'_>) -> Result<Self> {
        let bounds = Self {
            transactions: cursor.u64()?,
            relations: cursor.u64()?,
            min_id: cursor.u64()?,
            max_id: cursor.u64()?,
            begin: cursor.u64()?,
            end: cursor.u64()?,
            min_parent: cursor.u64()?,
            max_parent: cursor.u64()?,
            min_embedded: cursor.u64()?,
            max_embedded: cursor.u64()?,
            min_from: cursor.u64()?,
            max_from: cursor.u64()?,
            min_to: cursor.u64()?,
            max_to: cursor.u64()?,
        };
        if (bounds.transactions == 0 && bounds.relations == 0)
            || bounds.min_id > bounds.max_id
            || bounds.begin > bounds.end
            || bounds.min_parent > bounds.max_parent
            || bounds.min_embedded > bounds.max_embedded
            || bounds.min_from > bounds.max_from
            || bounds.min_to > bounds.max_to
            || (bounds.transactions != 0 && bounds.min_id == 0)
            || (bounds.transactions == 0
                && [
                    bounds.min_id,
                    bounds.max_id,
                    bounds.begin,
                    bounds.end,
                    bounds.min_parent,
                    bounds.max_parent,
                    bounds.min_embedded,
                    bounds.max_embedded,
                ]
                .iter()
                .any(|v| *v != 0))
            || (bounds.relations == 0
                && [
                    bounds.min_from,
                    bounds.max_from,
                    bounds.min_to,
                    bounds.max_to,
                ]
                .iter()
                .any(|v| *v != 0))
            || ((bounds.min_parent == 0) != (bounds.max_parent == 0))
            || ((bounds.min_embedded == 0) != (bounds.max_embedded == 0))
        {
            return Err(Error::Corrupt("invalid transaction bounds"));
        }
        Ok(bounds)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TransactionPage {
    pub bounds: TransactionBounds,
    pub blob_offset: u32,
    pub blob_length: u32,
    pub raw_length: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct TransactionQuery {
    pub generator: Option<GeneratorId>,
    pub parent: Option<RecordId>,
    pub id: Option<RecordId>,
    pub embedded_id: Option<RecordId>,
    pub begin: u64,
    pub end: u64,
}

impl Default for TransactionQuery {
    fn default() -> Self {
        Self {
            generator: None,
            parent: None,
            id: None,
            embedded_id: None,
            begin: 0,
            end: u64::MAX,
        }
    }
}

impl TransactionQuery {
    fn matches(self, tx: &Transaction) -> bool {
        self.matches_owner(tx)
            && self.embedded_id.is_none_or(|id| {
                tx.events.iter().any(|event| event.id == id)
                    || tx.stages.iter().any(|stage| stage.id == id)
            })
    }

    fn matches_owner(self, tx: &Transaction) -> bool {
        tx.begin <= self.end
            && tx.end >= self.begin
            && self.generator.is_none_or(|id| id == tx.generator)
            && self.parent.is_none_or(|id| id == tx.parent)
            && self.id.is_none_or(|id| id == tx.id)
    }
}

fn validate_transaction(tx: &Transaction, generator: &Generator, strings: u64) -> Result<()> {
    if tx.id == 0
        || tx.parent >= tx.id
        || tx.end < tx.begin
        || tx.status > 4
        || tx.span_kind > 5
        || !matches!(generator.kind, RecordKind::Transaction | RecordKind::Clock)
        || tx
            .events
            .iter()
            .any(|e| e.id <= tx.id || u64::from(e.name) >= strings)
        || tx.stages.iter().any(|s| {
            s.id <= tx.id
                || u64::from(s.name) >= strings
                || u64::from(s.lane) >= strings
                || s.end.is_some_and(|end| end < s.begin)
        })
        || tx.child_blocks.first() == Some(&0)
        || tx.child_blocks.windows(2).any(|w| w[0] >= w[1])
        || (generator.kind == RecordKind::Clock
            && (generator.payload_type != 0
                || tx.parent != 0
                || tx.span_kind != 0
                || !matches!(tx.status, 0 | 4)
                || !tx.events.is_empty()
                || !tx.stages.is_empty()
                || !tx.child_blocks.is_empty()))
    {
        return Err(Error::Corrupt(
            "invalid transaction identity, shape, or lifetime",
        ));
    }
    validate_attributes(&tx.attributes, strings)?;
    for attrs in tx
        .events
        .iter()
        .map(|e| &e.attributes)
        .chain(tx.stages.iter().map(|s| &s.attributes))
    {
        validate_attributes(attrs, strings)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub struct TransactionPolicy<'a> {
    pub pages: RecordPolicy,
    pub string_count: u64,
    pub shared_text: Option<&'a std::collections::HashMap<String, u32>>,
}

impl Default for TransactionPolicy<'_> {
    fn default() -> Self {
        Self {
            pages: RecordPolicy::default(),
            string_count: u64::MAX,
            shared_text: None,
        }
    }
}

fn sorted_transactions(transactions: &[Transaction]) -> Result<Vec<&Transaction>> {
    let mut sorted = Vec::with_capacity(transactions.len());
    let Some(first) = transactions.first() else {
        return Ok(sorted);
    };
    let (minimum, maximum) = transactions
        .iter()
        .fold((first.id, first.id), |(min, max), tx| {
            (min.min(tx.id), max.max(tx.id))
        });
    if let Ok(span) = usize::try_from(maximum - minimum)
        && span < transactions.len().saturating_mul(4)
    {
        let mut slots = vec![None; span + 1];
        for tx in transactions {
            if slots[(tx.id - minimum) as usize].replace(tx).is_some() {
                return Err(Error::invalid("duplicate owner identifier"));
            }
        }
        sorted.extend(slots.into_iter().flatten());
    } else {
        sorted.extend(transactions);
        sorted.sort_unstable_by_key(|tx| tx.id);
        if sorted.windows(2).any(|w| w[0].id == w[1].id) {
            return Err(Error::invalid("duplicate owner identifier"));
        }
    }
    Ok(sorted)
}

pub fn encode_transaction_block(
    number: u64,
    types: &Types,
    generators: &Generators,
    transactions: &[Transaction],
    relations: &[Relation],
    options: TransactionPolicy<'_>,
) -> Result<Vec<u8>> {
    let TransactionPolicy {
        pages: policy,
        string_count: strings,
        shared_text,
    } = options;
    if number == 0
        || (transactions.is_empty() && relations.is_empty())
        || policy.page_bytes == 0
        || policy.page_bytes >= wire::MAX_BLOCK_BYTES
    {
        return Err(Error::invalid("invalid transaction block or page policy"));
    }
    let sorted = sorted_transactions(transactions)?;
    let ids: Vec<_> = transactions
        .iter()
        .map(|tx| tx.generator)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for tx in &sorted {
        let generator = generators.get(tx.generator)?;
        validate_transaction(tx, generator, strings)?;
        types.validate(generator.payload_type, &tx.payload, strings)?;
    }
    for relation in relations {
        if u64::from(relation.kind) >= strings {
            return Err(Error::Corrupt("unknown relation kind"));
        }
        validate_attributes(&relation.attributes, strings)?;
    }
    let mut pages = Vec::new();
    let mut blobs = Vec::new();
    let mut bounds = TransactionBounds::default();
    let mut save = |raw: Vec<u8>, page_bounds: TransactionBounds| -> Result<()> {
        let packed = wire::compress(&raw, policy.compression)?;
        pages.push(TransactionPage {
            bounds: page_bounds,
            blob_offset: checked_u32(blobs.len())?,
            blob_length: checked_u32(packed.len())?,
            raw_length: checked_u32(raw.len())?,
        });
        blobs.extend_from_slice(&packed);
        bounds.merge(page_bounds)
    };
    if let Some(datatype) =
        scalar::eligible(types, generators, transactions).filter(|_| relations.is_empty())
    {
        let columns = 6 + types.get(datatype)?.unsigned_words().unwrap();
        let per_page = (policy.page_bytes.saturating_sub(9 + columns) / (8 * columns)).max(1);
        for rows in sorted.chunks(per_page) {
            let mut page_bounds = TransactionBounds::default();
            for row in rows {
                page_bounds.add_transaction(row);
            }
            save(scalar::encode(types, datatype, rows, &ids)?, page_bounds)?;
        }
    } else {
        let mut index = 0;
        while index < sorted.len() + relations.len() {
            let mut encoder = page::Encoder::new(strings, shared_text);
            let mut page_bounds = TransactionBounds::default();
            let first = index;
            while index < sorted.len() + relations.len() {
                let checkpoint = encoder.checkpoint();
                if index < sorted.len() {
                    let tx = sorted[index];
                    encoder.transaction(
                        types,
                        generators.get(tx.generator)?,
                        ids.binary_search(&tx.generator).unwrap() as u32,
                        tx,
                    )?;
                } else {
                    encoder.relation(&relations[index - sorted.len()])?;
                }
                let size = encoder.size();
                if index != first && size > policy.page_bytes {
                    encoder.restore(checkpoint);
                    break;
                }
                if index < sorted.len() {
                    page_bounds.add_transaction(sorted[index]);
                } else {
                    page_bounds.add_relation(&relations[index - sorted.len()]);
                }
                index += 1;
                if size >= policy.page_bytes {
                    break;
                }
            }
            save(encoder.finish()?, page_bounds)?;
        }
    }
    let mut out = number.to_le_bytes().to_vec();
    bounds.write(&mut out);
    out.extend_from_slice(&checked_u32(ids.len())?.to_le_bytes());
    out.extend_from_slice(&checked_u32(pages.len())?.to_le_bytes());
    for id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    for page in pages {
        page.bounds.write(&mut out);
        for value in [page.blob_offset, page.blob_length, page.raw_length] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out.extend_from_slice(&blobs);
    Ok(out)
}

#[derive(Clone)]
pub(crate) struct TransactionDirectory {
    number: u64,
    bounds: TransactionBounds,
    strings: u64,
    ids: Vec<GeneratorId>,
    pages: Vec<TransactionPage>,
    blob_offset: usize,
}

impl TransactionDirectory {
    pub(crate) fn matches(&self, query: TransactionQuery) -> bool {
        self.bounds.transactions != 0
            && query
                .generator
                .is_none_or(|id| self.ids.binary_search(&id).is_ok())
            && query
                .id
                .is_none_or(|id| self.bounds.min_id <= id && id <= self.bounds.max_id)
            && query
                .embedded_id
                .is_none_or(|id| self.bounds.min_embedded <= id && id <= self.bounds.max_embedded)
    }
}

pub struct TransactionBlockView<'a> {
    directory: Cow<'a, TransactionDirectory>,
    blobs: &'a [u8],
    types: &'a Types,
    generators: &'a Generators,
    max_payload_bytes: usize,
    shared_text: &'a [String],
}

impl<'a> TransactionBlockView<'a> {
    pub fn new(
        encoded: &'a [u8],
        types: &'a Types,
        generators: &'a Generators,
        strings: u64,
        max_payload_bytes: usize,
        shared_text: &'a [String],
    ) -> Result<Self> {
        let mut cursor = Cursor::new(encoded);
        let number = cursor.u64()?;
        let bounds = TransactionBounds::read(&mut cursor)?;
        if number == 0 {
            return Err(Error::Corrupt("invalid transaction block number"));
        }
        let generator_count = cursor.u32()?;
        let page_count = cursor.u32()?;
        if generator_count as usize > cursor.remaining() / 4 || page_count == 0 {
            return Err(Error::Corrupt("invalid transaction table count"));
        }
        let mut ids = Vec::new();
        for _ in 0..generator_count {
            let id = cursor.u32()?;
            let generator = generators.get(id)?;
            types.get(generator.payload_type)?;
            if !matches!(generator.kind, RecordKind::Transaction | RecordKind::Clock)
                || ids.last().is_some_and(|previous| *previous >= id)
            {
                return Err(Error::Corrupt("invalid transaction generator table"));
            }
            ids.push(id);
        }
        if (bounds.transactions != 0) == ids.is_empty()
            || page_count as usize > cursor.remaining() / 124
        {
            return Err(Error::Corrupt("invalid transaction page count"));
        }
        let mut pages = Vec::new();
        let mut aggregate = TransactionBounds::default();
        let mut previous_id = 0;
        let mut relation_only = false;
        let mut blob_end = 0u64;
        for _ in 0..page_count {
            let page = TransactionPage {
                bounds: TransactionBounds::read(&mut cursor)?,
                blob_offset: cursor.u32()?,
                blob_length: cursor.u32()?,
                raw_length: cursor.u32()?,
            };
            if page.blob_length == 0
                || page.raw_length == 0
                || u64::from(page.blob_offset) != blob_end
                || page.raw_length as usize >= wire::MAX_BLOCK_BYTES
            {
                return Err(Error::Corrupt("invalid transaction page partition"));
            }
            if page.bounds.transactions != 0 {
                if relation_only || page.bounds.min_id <= previous_id {
                    return Err(Error::Corrupt("unordered transaction page ID ranges"));
                }
                previous_id = page.bounds.max_id;
            } else {
                relation_only = true;
            }
            blob_end += u64::from(page.blob_length);
            aggregate.merge(page.bounds)?;
            pages.push(page);
        }
        if aggregate != bounds {
            return Err(Error::Corrupt("transaction block/page bounds disagree"));
        }
        let blobs = cursor.take(
            usize::try_from(blob_end)
                .map_err(|_| Error::Corrupt("transaction blobs exceed usize"))?,
        )?;
        cursor.done()?;
        Ok(Self {
            directory: Cow::Owned(TransactionDirectory {
                number,
                bounds,
                strings,
                ids,
                pages,
                blob_offset: encoded.len() - blobs.len(),
            }),
            blobs,
            types,
            generators,
            max_payload_bytes,
            shared_text,
        })
    }

    pub(crate) fn into_directory(self) -> TransactionDirectory {
        self.directory.into_owned()
    }

    pub(crate) fn from_directory(
        encoded: &'a [u8],
        directory: &'a TransactionDirectory,
        types: &'a Types,
        generators: &'a Generators,
        max_payload_bytes: usize,
        shared_text: &'a [String],
    ) -> Self {
        Self {
            directory: Cow::Borrowed(directory),
            blobs: &encoded[directory.blob_offset..],
            types,
            generators,
            max_payload_bytes,
            shared_text,
        }
    }

    #[must_use]
    pub fn number(&self) -> u64 {
        self.directory.number
    }

    #[must_use]
    pub fn bounds(&self) -> &TransactionBounds {
        &self.directory.bounds
    }

    pub fn with_defaults(
        encoded: &'a [u8],
        types: &'a Types,
        generators: &'a Generators,
    ) -> Result<Self> {
        Self::new(
            encoded,
            types,
            generators,
            u64::MAX,
            DEFAULT_PAYLOAD_LIMIT,
            &[],
        )
    }

    #[must_use]
    pub fn pages(&self) -> &[TransactionPage] {
        &self.directory.pages
    }

    #[must_use]
    pub fn generators(&self) -> &[GeneratorId] {
        &self.directory.ids
    }

    pub fn time_range(&self) -> Result<Option<(u64, u64)>> {
        if self.directory.bounds.transactions == 0 {
            return Ok(None);
        }
        let mut range = (self.directory.bounds.begin, self.directory.bounds.end);
        for page in &self.directory.pages {
            if page.bounds.max_embedded == 0 {
                continue;
            }
            let start = page.blob_offset as usize;
            let raw = wire::decompress(
                &self.blobs[start..start + page.blob_length as usize],
                page.raw_length as usize,
            )?;
            let (begin, end) = time::range(&raw, page.bounds)?;
            range = (range.0.min(begin), range.1.max(end));
        }
        Ok(Some(range))
    }

    pub(crate) fn activity(
        &self,
        mut admit: impl FnMut(usize) -> Result<()>,
        mut visit: impl FnMut(GeneratorId, u64, u64) -> Result<()>,
    ) -> Result<()> {
        for page in &self.directory.pages {
            admit(page.raw_length as usize)?;
            let start = page.blob_offset as usize;
            let raw = wire::decompress(
                &self.blobs[start..start + page.blob_length as usize],
                page.raw_length as usize,
            )?;
            if raw.first() == Some(&2) {
                let mut cursor = Cursor::new(&raw);
                cursor.byte()?;
                let datatype = cursor.u32()?;
                let count = cursor.u32()? as usize;
                let words = self
                    .types
                    .get(datatype)?
                    .unsigned_words()
                    .ok_or(Error::Corrupt("invalid scalar activity datatype"))?;
                if count == 0 || count as u64 != page.bounds.transactions || count > raw.len() {
                    return Err(Error::Corrupt("invalid scalar activity count"));
                }
                let mut planes = [&[][..]; 4];
                for column in 0..6 + words {
                    let width = cursor.byte()? as usize;
                    if width > 8 {
                        return Err(Error::Corrupt("invalid scalar activity width"));
                    }
                    let data = cursor.take(
                        width
                            .checked_mul(count)
                            .ok_or(Error::Corrupt("scalar activity size overflow"))?,
                    )?;
                    if let Some(plane) = planes.get_mut(column) {
                        *plane = data;
                    }
                }
                cursor.done()?;
                let value = |column: usize, row: usize| -> u64 {
                    planes[column]
                        .chunks_exact(count)
                        .enumerate()
                        .fold(0, |value, (byte, plane)| {
                            value | (u64::from(plane[row]) << (byte * 8))
                        })
                };
                for row in 0..count {
                    let generator = *self
                        .directory
                        .ids
                        .get(
                            usize::try_from(value(1, row))
                                .map_err(|_| Error::Corrupt("invalid activity generator"))?,
                        )
                        .ok_or(Error::Corrupt("invalid activity generator"))?;
                    let begin = value(2, row);
                    let end = begin
                        .checked_add(value(3, row))
                        .ok_or(Error::Corrupt("activity time overflow"))?;
                    visit(generator, begin, end)?;
                }
            } else {
                let columns = wire::unpack_columns::<33>(&raw)?;
                let mut generators = Cursor::new(columns[1]);
                let mut begins = Cursor::new(columns[2]);
                let mut lengths = Cursor::new(columns[3]);
                if page.bounds.transactions > columns[2].len() as u64 {
                    return Err(Error::Corrupt("invalid activity transaction count"));
                }
                let mut begin = 0u64;
                for _ in 0..page.bounds.transactions {
                    let generator = *self
                        .directory
                        .ids
                        .get(generators.id()? as usize)
                        .ok_or(Error::Corrupt("invalid activity generator"))?;
                    begin = begin.wrapping_add(crate::varint::unzigzag(begins.var()?) as u64);
                    let end = begin
                        .checked_add(lengths.var()?)
                        .ok_or(Error::Corrupt("activity time overflow"))?;
                    visit(generator, begin, end)?;
                }
                generators.done()?;
                begins.done()?;
                lengths.done()?;
            }
        }
        Ok(())
    }

    fn decode_page(
        &self,
        page: &TransactionPage,
        tx: &mut impl FnMut(Transaction) -> bool,
        relation: &mut impl FnMut(Relation) -> bool,
        query: Option<TransactionQuery>,
    ) -> Result<bool> {
        let start = page.blob_offset as usize;
        let raw = wire::decompress(
            &self.blobs[start..start + page.blob_length as usize],
            page.raw_length as usize,
        )?;
        if raw.first() == Some(&2) {
            if page.bounds.relations != 0 {
                return Err(Error::Corrupt("scalar page contains relations"));
            }
            let mut bounds = TransactionBounds::default();
            let mut previous = 0;
            let mut error = None;
            let complete = scalar::decode(
                &raw,
                self.types,
                self.generators,
                &self.directory.ids,
                page.bounds.transactions,
                self.max_payload_bytes,
                |record| {
                    if record.id <= previous {
                        error = Some(Error::Corrupt("unordered scalar IDs"));
                        return false;
                    }
                    previous = record.id;
                    bounds.add_transaction(&record);
                    query.is_some_and(|q| !q.matches(&record)) || tx(record)
                },
            )?;
            if let Some(error) = error {
                return Err(error);
            }
            if complete && bounds != page.bounds {
                return Err(Error::Corrupt("scalar page bounds disagree"));
            }
            Ok(complete)
        } else {
            page::decode(&raw, page.bounds, self, tx, relation, query)
        }
    }

    pub fn transactions(
        &self,
        query: TransactionQuery,
        mut visit: impl FnMut(Transaction) -> bool,
    ) -> Result<()> {
        if query.begin > query.end {
            return Err(Error::invalid("invalid transaction window"));
        }
        if !self.directory.matches(query) {
            return Ok(());
        }
        for page in &self.directory.pages {
            let bounds = page.bounds;
            if bounds.transactions == 0
                || bounds.begin > query.end
                || bounds.end < query.begin
                || query
                    .parent
                    .is_some_and(|id| id != 0 && (id < bounds.min_parent || id > bounds.max_parent))
                || query
                    .id
                    .is_some_and(|id| id < bounds.min_id || id > bounds.max_id)
                || query
                    .embedded_id
                    .is_some_and(|id| id < bounds.min_embedded || id > bounds.max_embedded)
            {
                continue;
            }
            if !self.decode_page(page, &mut visit, &mut |_| true, Some(query))? {
                return Ok(());
            }
        }
        Ok(())
    }

    fn decode_relations(
        &self,
        page: &TransactionPage,
        visit: &mut impl FnMut(Relation) -> bool,
    ) -> Result<bool> {
        let start = page.blob_offset as usize;
        let raw = wire::decompress(
            &self.blobs[start..start + page.blob_length as usize],
            page.raw_length as usize,
        )?;
        page::relations(&raw, page.bounds, self, visit)
    }

    pub fn scan_relations(&self, mut visit: impl FnMut(Relation) -> bool) -> Result<()> {
        for page in &self.directory.pages {
            if page.bounds.relations != 0 && !self.decode_relations(page, &mut visit)? {
                return Ok(());
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
        for page in &self.directory.pages {
            let bounds = page.bounds;
            let (min, max) = if incoming {
                (bounds.min_to, bounds.max_to)
            } else {
                (bounds.min_from, bounds.max_from)
            };
            if bounds.relations == 0 || endpoint < min || endpoint > max {
                continue;
            }
            if !self.decode_relations(page, &mut |relation| {
                if (if incoming {
                    relation.destination
                } else {
                    relation.source
                }) == endpoint
                {
                    visit(relation)
                } else {
                    true
                }
            })? {
                return Ok(());
            }
        }
        Ok(())
    }

    pub fn scan(
        &self,
        mut tx: impl FnMut(Transaction) -> bool,
        mut relation: impl FnMut(Relation) -> bool,
    ) -> Result<()> {
        let mut used = HashSet::new();
        for page in &self.directory.pages {
            if !self.decode_page(
                page,
                &mut |record| {
                    used.insert(record.generator);
                    tx(record)
                },
                &mut relation,
                None,
            )? {
                return Ok(());
            }
        }
        if used.len() != self.directory.ids.len() {
            return Err(Error::Corrupt("unused transaction generator table entry"));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        let mut duplicate = false;
        self.scan(
            |tx| {
                for id in std::iter::once(tx.id)
                    .chain(tx.events.iter().map(|e| e.id))
                    .chain(tx.stages.iter().map(|s| s.id))
                {
                    duplicate |= !ids.insert(id);
                }
                true
            },
            |_| true,
        )?;
        if duplicate {
            Err(Error::Corrupt("duplicate global record identifier"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::schema::{Payload, Primitive};

    #[test]
    fn page_visitors_receive_only_complete_selected_records() {
        for general in [false, true] {
            let types = Types::new();
            let mut generators = Generators::default();
            for id in [10, 11] {
                generators
                    .declare(Generator {
                        id,
                        kind: RecordKind::Transaction,
                        payload_type: Primitive::U64.id(),
                    })
                    .unwrap();
            }
            let rows: Vec<_> = (1..=3u64)
                .map(|id| {
                    let mut payload = Vec::new();
                    id.encode(&mut payload).unwrap();
                    Transaction {
                        id,
                        generator: if id == 2 { 11 } else { 10 },
                        begin: id,
                        end: id,
                        payload,
                        child_blocks: if general { vec![7] } else { Vec::new() },
                        ..Default::default()
                    }
                })
                .collect();
            let encoded =
                encode_transaction_block(1, &types, &generators, &rows, &[], Default::default())
                    .unwrap();
            let view = TransactionBlockView::with_defaults(&encoded, &types, &generators).unwrap();
            assert_eq!(view.pages().len(), 1);
            let mut seen = Vec::new();
            let complete = view
                .decode_page(
                    &view.pages()[0],
                    &mut |record| {
                        assert_eq!(record, rows[1]);
                        seen.push(record.id);
                        false
                    },
                    &mut |_| true,
                    Some(TransactionQuery {
                        generator: Some(11),
                        ..Default::default()
                    }),
                )
                .unwrap();
            assert!(!complete);
            assert_eq!(seen, [2]);
        }
    }
}
