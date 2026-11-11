//! Incremental strings, metadata, hierarchy, and typed source declarations.

use std::collections::HashMap;

use crate::{
    Error, Result,
    attributes::{
        Attributes, Value, ValueId, encode_attributes, read_attributes, validate_attribute_refs,
    },
    framing::{Container, Section},
    hierarchy::{NodeId, SignalId, SignalKind},
    records::{Generator, Generators, RecordKind},
    schema::{DEFAULT_PAYLOAD_LIMIT, Types},
    varint::{put_blob, put_i64, put_u64, unzigzag},
    wire::{self, Cursor},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    pub timescale: i8,
    pub time_zero: i64,
    pub file_type: u8,
    pub writer: String,
    pub date: String,
    pub comment: String,
    pub group_size: u32,
    pub attributes: Attributes,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            timescale: -12,
            time_zero: 0,
            file_type: 0,
            writer: String::new(),
            date: String::new(),
            comment: String::new(),
            group_size: 256,
            attributes: vec![],
        }
    }
}

impl Metadata {
    pub fn encode(&self, strings: u64) -> Result<Vec<u8>> {
        check_group_size(self.group_size)?;
        let mut out = vec![1];
        put_i64(&mut out, i64::from(self.timescale));
        put_i64(&mut out, self.time_zero);
        out.push(self.file_type);
        for text in [&self.writer, &self.date, &self.comment] {
            put_blob(&mut out, text.as_bytes());
        }
        put_u64(&mut out, u64::from(self.group_size));
        encode_attributes(&mut out, &self.attributes, strings, 0)?;
        Ok(out)
    }

    pub fn decode(payload: &[u8], strings: u64) -> Result<Self> {
        let mut cursor = Cursor::new(payload);
        if cursor.var()? != 1 {
            return Err(Error::Corrupt("unknown metadata layout"));
        }
        let timescale = i8::try_from(unzigzag(cursor.var()?))
            .map_err(|_| Error::Corrupt("invalid timescale"))?;
        let time_zero = unzigzag(cursor.var()?);
        let file_type = cursor.byte()?;
        let writer = cursor.text()?.to_owned();
        let date = cursor.text()?.to_owned();
        let comment = cursor.text()?.to_owned();
        let group_size = cursor.id()?;
        check_group_size(group_size)?;
        let mut budget = DEFAULT_PAYLOAD_LIMIT;
        let attributes = read_attributes(&mut cursor, strings, 0, &mut budget)?;
        cursor.done()?;
        Ok(Self {
            timescale,
            time_zero,
            file_type,
            writer,
            date,
            comment,
            group_size,
            attributes,
        })
    }
}

fn check_group_size(size: u32) -> Result<()> {
    if size == 0 || !size.is_power_of_two() || size > 1 << 20 {
        Err(Error::Corrupt("invalid signal group size"))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeData {
    Scope {
        scope_type: u16,
        component: u32,
    },
    Signal {
        var_type: u16,
        direction: u8,
        signal: SignalId,
        alias: bool,
    },
    Generator {
        kind: u32,
        payload_type: u32,
    },
    EnumTable {
        entries: Vec<(u32, u32)>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub parent: Option<NodeId>,
    pub name: u32,
    pub data: NodeData,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, Default)]
pub struct Declarations {
    pub strings: Vec<String>,
    pub nodes: Vec<Node>,
    pub signals: Vec<SignalKind>,
    pub types: Types,
    pub generators: Generators,
    interned: HashMap<String, u32>,
    indexed_strings: usize,
    values: Vec<Value>,
    interned_values: HashMap<Vec<u8>, ValueId>,
    indexed_values: usize,
}

impl Declarations {
    #[must_use]
    pub fn new() -> Self {
        let mut out = Self::default();
        out.intern("").unwrap();
        out
    }

    pub fn string(&self, id: u32) -> Result<&str> {
        self.strings
            .get(id as usize)
            .map(String::as_str)
            .ok_or_else(|| Error::corrupt("unknown string identifier"))
    }

    pub fn intern(&mut self, text: &str) -> Result<u32> {
        if self.indexed_strings < self.strings.len() {
            self.interned
                .reserve(self.strings.len() - self.indexed_strings);
            for (id, value) in self.strings.iter().enumerate().skip(self.indexed_strings) {
                self.interned.entry(value.clone()).or_insert(id as u32);
            }
            self.indexed_strings = self.strings.len();
        }
        if let Some(id) = self.interned.get(text) {
            return Ok(*id);
        }
        let id = u32::try_from(self.strings.len())
            .ok()
            .filter(|id| *id != u32::MAX)
            .ok_or_else(|| Error::invalid("string identifier exhaustion"))?;
        self.strings.push(text.into());
        self.interned.insert(text.into(), id);
        self.indexed_strings = self.strings.len();
        Ok(id)
    }

    pub fn value(&self, id: ValueId) -> Result<&Value> {
        self.values
            .get(id.0 as usize)
            .ok_or_else(|| Error::corrupt("unknown interned value identifier"))
    }

    pub fn resolve_value<'a>(&'a self, mut value: &'a Value) -> Result<&'a Value> {
        while let Value::Ref(id) = value {
            value = self.value(*id)?;
        }
        Ok(value)
    }

    #[must_use]
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    /// Owned table entries and their buffers, excluding the writer's deduplication index.
    #[must_use]
    pub fn value_table_bytes(&self) -> usize {
        self.values.capacity() * std::mem::size_of::<Value>()
            + self
                .values
                .iter()
                .map(crate::attributes::value_charge)
                .sum::<usize>()
    }

    /// Deduplicate encoded typed content. Map and list order, float bits, and
    /// reference IDs participate in identity.
    pub fn intern_value(&mut self, value: Value) -> Result<ValueId> {
        let mut encoded = Vec::new();
        value.encode(&mut encoded, self.strings.len() as u64)?;
        value.validate_refs(self.values.len())?;
        for id in self.indexed_values..self.values.len() {
            let mut key = Vec::new();
            self.values[id].encode(&mut key, self.strings.len() as u64)?;
            self.interned_values
                .entry(key)
                .or_insert(ValueId(id as u32));
        }
        self.indexed_values = self.values.len();
        if let Some(id) = self.interned_values.get(&encoded) {
            return Ok(*id);
        }
        let id = u32::try_from(self.values.len())
            .ok()
            .filter(|id| *id != u32::MAX)
            .ok_or_else(|| Error::invalid("value identifier exhaustion"))?;
        let id = ValueId(id);
        self.values.push(value);
        self.interned_values.insert(encoded, id);
        self.indexed_values = self.values.len();
        Ok(id)
    }

    pub fn values_chunk(&self, first: ValueId) -> Result<Vec<u8>> {
        let values = self
            .values
            .get(first.0 as usize..)
            .ok_or_else(|| Error::invalid("invalid value chunk start"))?;
        let mut out = Vec::new();
        put_u64(&mut out, u64::from(first.0));
        put_u64(&mut out, values.len() as u64);
        for value in values {
            let mut encoded = Vec::new();
            value.encode(&mut encoded, self.strings.len() as u64)?;
            put_blob(&mut out, &encoded);
        }
        Ok(out)
    }

    pub fn read_values(&mut self, payload: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(payload);
        if cursor.id()? as usize != self.values.len() {
            return Err(Error::Corrupt("noncontiguous value identifiers"));
        }
        let count = cursor.id()? as usize;
        if count > cursor.remaining() / 2
            || self
                .values
                .len()
                .checked_add(count)
                .is_none_or(|end| end > u32::MAX as usize)
        {
            return Err(Error::Corrupt("invalid value count"));
        }
        let mut values = Vec::new();
        for _ in 0..count {
            let value = Value::decode(
                cursor.blob()?,
                self.strings.len() as u64,
                DEFAULT_PAYLOAD_LIMIT,
            )?;
            value.validate_refs(self.values.len() + values.len())?;
            values.push(value);
        }
        cursor.done()?;
        self.values.extend(values);
        Ok(())
    }

    #[must_use]
    pub fn find(&self, path: &str) -> Option<NodeId> {
        self.find_matching(path, |_| true)
    }

    pub(crate) fn find_matching(
        &self,
        path: &str,
        accept: impl Fn(&NodeData) -> bool,
    ) -> Option<NodeId> {
        let mut parent = None;
        let mut components = path.split('.').peekable();
        while let Some(component) = components.next() {
            let leaf = components.peek().is_none();
            parent = Some(NodeId(
                self.nodes
                    .iter()
                    .enumerate()
                    .find(|(_, node)| {
                        node.parent == parent
                            && self.string(node.name).is_ok_and(|name| name == component)
                            && if leaf {
                                accept(&node.data)
                            } else {
                                matches!(node.data, NodeData::Scope { .. })
                            }
                    })?
                    .0 as u32,
            ));
        }
        parent
    }

    pub fn strings_chunk(&self, first: u32) -> Result<Vec<u8>> {
        let strings = self
            .strings
            .get(first as usize..)
            .ok_or_else(|| Error::invalid("invalid string chunk start"))?;
        let mut out = Vec::new();
        put_u64(&mut out, u64::from(first));
        put_u64(&mut out, strings.len() as u64);
        for string in strings {
            put_blob(&mut out, string.as_bytes());
        }
        Ok(out)
    }

    pub fn read_strings(&mut self, payload: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(payload);
        if cursor.id()? as usize != self.strings.len() {
            return Err(Error::Corrupt("noncontiguous string identifiers"));
        }
        let count = cursor.id()?;
        if count as usize > cursor.remaining() {
            return Err(Error::Corrupt("invalid string count"));
        }
        let mut strings = Vec::new();
        for _ in 0..count {
            strings.push(cursor.text()?.to_owned());
        }
        cursor.done()?;
        if self.strings.is_empty() && strings.first().is_none_or(|text| !text.is_empty()) {
            return Err(Error::Corrupt("first string is not empty"));
        }
        for text in strings {
            u32::try_from(self.strings.len())
                .map_err(|_| Error::Corrupt("string identifier exhaustion"))?;
            self.strings.push(text);
        }
        Ok(())
    }

    pub fn hierarchy_chunk(&self, first: u32) -> Result<Vec<u8>> {
        let nodes = self
            .nodes
            .get(first as usize..)
            .ok_or_else(|| Error::invalid("invalid hierarchy chunk start"))?;
        let mut signals = self.nodes[..first as usize]
            .iter()
            .filter(|node| matches!(node.data, NodeData::Signal { alias: false, .. }))
            .count() as u32;
        let mut out = Vec::new();
        put_u64(&mut out, u64::from(first));
        put_u64(&mut out, nodes.len() as u64);
        for (offset, node) in nodes.iter().enumerate() {
            let index = first as usize + offset;
            out.push(match node.data {
                NodeData::Scope { .. } => 1,
                NodeData::Signal { .. } => 2,
                NodeData::Generator { .. } => 3,
                NodeData::EnumTable { .. } => 4,
            });
            let parent = match node.parent {
                None => 0,
                Some(parent)
                    if parent.0 < index as u32
                        && matches!(self.nodes[parent.0 as usize].data, NodeData::Scope { .. }) =>
                {
                    index as u32 - parent.0
                }
                _ => return Err(Error::invalid("hierarchy parent must be an earlier scope")),
            };
            put_u64(&mut out, u64::from(parent));
            self.string(node.name)?;
            put_u64(&mut out, u64::from(node.name));
            match &node.data {
                NodeData::Scope {
                    scope_type,
                    component,
                } => {
                    self.string(*component)?;
                    put_u64(&mut out, u64::from(*scope_type));
                    put_u64(&mut out, u64::from(*component));
                }
                NodeData::Generator { kind, payload_type } => {
                    self.string(*kind)?;
                    self.types.get(*payload_type)?;
                    put_u64(&mut out, u64::from(*kind));
                    put_u64(&mut out, u64::from(*payload_type));
                }
                NodeData::EnumTable { entries } => {
                    put_u64(&mut out, entries.len() as u64);
                    for (name, bits) in entries {
                        self.string(*name)?;
                        self.string(*bits)?;
                        put_u64(&mut out, u64::from(*name));
                        put_u64(&mut out, u64::from(*bits));
                    }
                }
                NodeData::Signal {
                    var_type,
                    direction,
                    signal,
                    alias,
                } => {
                    put_u64(&mut out, u64::from(*var_type));
                    out.push(*direction);
                    if signal.0 > signals
                        || (*alias && signal.0 == signals)
                        || (!alias && signal.0 != signals)
                    {
                        return Err(Error::invalid("invalid signal declaration order"));
                    }
                    put_u64(&mut out, u64::from(signals - signal.0));
                    out.push(u8::from(!alias));
                    if !alias {
                        let kind = *self
                            .signals
                            .get(signal.0 as usize)
                            .ok_or_else(|| Error::invalid("unknown signal"))?;
                        encode_signal_kind(&mut out, kind)?;
                        signals += 1;
                    }
                }
            }
            encode_attributes(&mut out, &node.attributes, self.strings.len() as u64, 0)?;
        }
        Ok(out)
    }

    pub fn read_hierarchy(&mut self, payload: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(payload);
        if cursor.id()? as usize != self.nodes.len() {
            return Err(Error::Corrupt("noncontiguous hierarchy identifiers"));
        }
        let count = cursor.id()?;
        if count as usize > cursor.remaining() / 4 {
            return Err(Error::Corrupt("invalid hierarchy count"));
        }
        let old_nodes = self.nodes.len();
        let old_signals = self.signals.len();
        let old_generators = self.generators.clone();
        let result = (|| {
            for _ in 0..count {
                let tag = cursor.byte()?;
                let distance = cursor.id()? as usize;
                if distance > self.nodes.len() {
                    return Err(Error::Corrupt("invalid parent distance"));
                }
                let parent = if distance == 0 {
                    None
                } else {
                    let id = self.nodes.len() - distance;
                    if !matches!(self.nodes[id].data, NodeData::Scope { .. }) {
                        return Err(Error::Corrupt("parent is not a scope"));
                    }
                    Some(NodeId(id as u32))
                };
                let name = cursor.id()?;
                self.string(name)?;
                let data = match tag {
                    1 => {
                        let scope_type = u16::try_from(cursor.id()?)
                            .map_err(|_| Error::Corrupt("scope type exceeds u16"))?;
                        let component = cursor.id()?;
                        self.string(component)?;
                        NodeData::Scope {
                            scope_type,
                            component,
                        }
                    }
                    2 => {
                        let var_type = u16::try_from(cursor.id()?)
                            .map_err(|_| Error::Corrupt("variable type exceeds u16"))?;
                        let direction = cursor.byte()?;
                        let back = cursor.id()? as usize;
                        let declares = cursor.byte()?;
                        if back > self.signals.len()
                            || declares > 1
                            || (declares == 0 && back == 0)
                            || (declares == 1 && back != 0)
                        {
                            return Err(Error::Corrupt("invalid signal declaration"));
                        }
                        let signal = SignalId((self.signals.len() - back) as u32);
                        if declares == 1 {
                            self.signals.push(decode_signal_kind(&mut cursor)?);
                        } else if self
                            .nodes
                            .iter()
                            .find_map(|node| match node.data {
                                NodeData::Signal {
                                    signal: id,
                                    var_type,
                                    alias: false,
                                    ..
                                } if id == signal => Some(var_type),
                                _ => None,
                            })
                            .is_some_and(|original| (original == 0) != (var_type == 0))
                        {
                            return Err(Error::Corrupt("conflicting event alias"));
                        }
                        NodeData::Signal {
                            var_type,
                            direction,
                            signal,
                            alias: declares == 0,
                        }
                    }
                    3 => {
                        let kind = cursor.id()?;
                        let payload_type = cursor.id()?;
                        let kind_name = self.string(kind)?;
                        self.types.get(payload_type)?;
                        self.generators.declare(Generator {
                            id: self.nodes.len() as u32,
                            payload_type,
                            kind: match kind_name {
                                "CLOCK" => RecordKind::Clock,
                                "EVENT" => RecordKind::Event,
                                "STAGE" => RecordKind::Stage,
                                "LOG" => RecordKind::Log,
                                _ => RecordKind::Transaction,
                            },
                        })?;
                        NodeData::Generator { kind, payload_type }
                    }
                    4 => {
                        let count = cursor.id()?;
                        if count as usize > cursor.remaining() / 2 {
                            return Err(Error::Corrupt("invalid enumeration count"));
                        }
                        let mut entries = Vec::new();
                        for _ in 0..count {
                            let name = cursor.id()?;
                            let bits = cursor.id()?;
                            self.string(name)?;
                            self.string(bits)?;
                            entries.push((name, bits));
                        }
                        NodeData::EnumTable { entries }
                    }
                    _ => return Err(Error::Corrupt("unknown hierarchy node kind")),
                };
                let mut budget = DEFAULT_PAYLOAD_LIMIT;
                let attributes =
                    read_attributes(&mut cursor, self.strings.len() as u64, 0, &mut budget)?;
                validate_attribute_refs(&attributes, self.values.len())?;
                self.nodes.push(Node {
                    parent,
                    name,
                    data,
                    attributes,
                });
            }
            cursor.done()
        })();
        if result.is_err() {
            self.nodes.truncate(old_nodes);
            self.signals.truncate(old_signals);
            self.generators = old_generators;
        }
        result
    }

    pub fn from_container(container: &Container) -> Result<(Self, Metadata)> {
        let mut declarations = Self::default();
        let mut metadata = None;
        for section in container.sections() {
            let payload = container.payload(section, false)?;
            match section.kind {
                1 => {
                    if metadata.is_some() {
                        return Err(Error::Corrupt("duplicate metadata section"));
                    }
                    metadata = Some(Metadata::decode(
                        payload,
                        declarations.strings.len() as u64,
                    )?);
                    validate_attribute_refs(
                        &metadata.as_ref().unwrap().attributes,
                        declarations.values.len(),
                    )?;
                }
                2 => {
                    let raw = wire::decompress_any(payload)?;
                    check_chunk_bounds(section, &raw)?;
                    declarations.read_strings(&raw)?;
                }
                3 => {
                    if metadata.is_none() {
                        return Err(Error::Corrupt("hierarchy precedes metadata"));
                    }
                    let raw = wire::decompress_any(payload)?;
                    check_chunk_bounds(section, &raw)?;
                    declarations.read_hierarchy(&raw)?;
                }
                10 => {
                    if metadata.is_none() {
                        return Err(Error::Corrupt("datatype section precedes metadata"));
                    }
                    check_chunk_bounds(section, payload)?;
                    declarations.types.read_declarations_with_strings(
                        payload,
                        declarations.strings.len() as u64,
                    )?;
                    for id in section.aux0..section.aux0 + section.aux1 {
                        validate_attribute_refs(
                            &declarations.types.get(id as u32)?.attributes,
                            declarations.values.len(),
                        )?;
                    }
                }
                11 => {
                    let raw = wire::decompress_any(payload)?;
                    check_chunk_bounds(section, &raw)?;
                    declarations.read_values(&raw)?;
                }
                4..=6 | 9 => {
                    if metadata.is_none() {
                        return Err(Error::Corrupt("recording precedes metadata"));
                    }
                }
                _ if section.flags & 1 != 0 => (),
                _ => return Err(Error::Corrupt("unknown required section")),
            }
        }
        if declarations.strings.is_empty() {
            return Err(Error::Corrupt("missing string declarations"));
        }
        Ok((
            declarations,
            metadata.ok_or_else(|| Error::corrupt("missing metadata"))?,
        ))
    }
}

fn check_chunk_bounds(section: &Section, payload: &[u8]) -> Result<()> {
    let mut cursor = Cursor::new(payload);
    if cursor.var()? != section.aux0 || cursor.var()? != section.aux1 {
        return Err(Error::Corrupt("declaration directory bounds disagree"));
    }
    Ok(())
}

fn encode_signal_kind(out: &mut Vec<u8>, kind: SignalKind) -> Result<()> {
    match kind {
        SignalKind::Bits { width, states } => {
            if width == 0 || width >= 1 << 31 {
                return Err(Error::invalid("invalid signal width"));
            }
            out.push(match states {
                2 => 0,
                4 => 1,
                9 => 2,
                _ => return Err(Error::invalid("invalid signal alphabet")),
            });
            put_u64(out, u64::from(width));
        }
        SignalKind::Real => out.push(3),
        SignalKind::VarLen => out.push(4),
    }
    Ok(())
}

fn decode_signal_kind(cursor: &mut Cursor<'_>) -> Result<SignalKind> {
    match cursor.byte()? {
        code @ 0..=2 => {
            let width = cursor.id()?;
            if width == 0 || width >= 1 << 31 {
                return Err(Error::Corrupt("invalid signal width"));
            }
            Ok(SignalKind::Bits {
                width,
                states: [2, 4, 9][code as usize],
            })
        }
        3 => Ok(SignalKind::Real),
        4 => Ok(SignalKind::VarLen),
        _ => Err(Error::Corrupt("unknown signal kind")),
    }
}
