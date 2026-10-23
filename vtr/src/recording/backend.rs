use std::{
    any::TypeId as RustType,
    collections::{BTreeMap, HashMap},
    hash::{BuildHasherDefault, Hasher},
    io::Write,
    sync::Arc,
};

mod owners;
use owners::Owners;

use super::{
    EncodingOptions, Operation, ProducerId, SourceOptions, WriterOptions,
    capture::{PayloadOperation, SignalRef},
    output::{Output, Section},
    waveform::{self, Waveform},
};
use crate::{
    Error, Result,
    attributes::{Attribute, Value, validate_attributes},
    declarations::{Declarations, Metadata, Node, NodeData},
    framing::Fingerprint,
    hierarchy::{Direction, NodeId, ScopeType, SignalId, SignalKind, VarType},
    records::{
        Generator, Generators, LocatedRecord, RecordKind, RecordPolicy, encode_record_block,
    },
    schema::{FIRST_DECLARED_TYPE, Payload, TypeKind, Types},
    transactions::{Relation, Transaction, TransactionPolicy, encode_transaction_block},
    wire,
};

// These maps contain generated numeric identifiers, rather than user-supplied strings.
#[derive(Default)]
struct IdHasher(u64);
impl Hasher for IdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }
    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }
}
type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

struct ActiveOwner {
    value: Transaction,
    stages: Vec<u64>,
    payload_ready: bool,
}
struct ActiveStage {
    value: LocatedRecord,
    payload_ready: bool,
}
struct Clock {
    id: u64,
    begin: u64,
    period: u64,
    previous: Option<u64>,
}
struct SchemaSnapshot {
    types_count: usize,
    nodes_count: usize,
    types: Arc<Types>,
    generators: Arc<Generators>,
    charge: usize,
}

pub(super) struct Backend {
    output: Output,
    pub declarations: Declarations,
    pub metadata: Metadata,
    pub encoding: EncodingOptions,
    written_strings: u32,
    written_nodes: u32,
    written_types: u32,
    metadata_written: bool,
    bindings: HashMap<RustType, u32>,
    owners: Owners,
    stages: IdMap<u64, ActiveStage>,
    records: Vec<LocatedRecord>,
    transactions: Vec<Transaction>,
    relations: Vec<Relation>,
    record_bytes: usize,
    tx_bytes: usize,
    peak_record_bytes: usize,
    peak_transaction_bytes: usize,
    next_block: u64,
    max_created: u64,
    max_reference: u64,
    pub captured_time: u64,
    clocks: BTreeMap<u32, Clock>,
    waveform: Waveform,
    wave_encoder: super::waveform::Encoder,
    blackout: Vec<(u64, bool)>,
    last_blackout: Option<u64>,
    dump_active: bool,
    pub finished: bool,
    mode: super::ExecutionMode,
    ownership: IdMap<Resource, (ProducerId, u64)>,
    signal_ownership: Vec<Option<(ProducerId, u64)>>,
    snapshot: Option<SchemaSnapshot>,
    payload_pool: super::payload_pool::Pool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Resource {
    Signal(u32),
    Record(u64),
    Clock(u32),
}

impl Backend {
    pub fn new(output: Box<dyn Write + Send>, options: WriterOptions) -> Result<Self> {
        options.validate()?;
        Ok(Self {
            output: Output::new(output, &options.encoding)?,
            declarations: Declarations::new(),
            metadata: options.metadata,
            payload_pool: super::payload_pool::Pool::new(options.encoding.payload_pool_bytes),
            encoding: options.encoding,
            written_strings: 0,
            written_nodes: 0,
            written_types: FIRST_DECLARED_TYPE,
            metadata_written: false,
            bindings: HashMap::new(),
            owners: Owners::default(),
            stages: IdMap::default(),
            records: vec![],
            transactions: vec![],
            relations: vec![],
            record_bytes: 0,
            tx_bytes: 0,
            peak_record_bytes: 0,
            peak_transaction_bytes: 0,
            next_block: 1,
            max_created: 0,
            max_reference: 0,
            captured_time: 0,
            clocks: BTreeMap::new(),
            waveform: Waveform::default(),
            wave_encoder: super::waveform::Encoder::default(),
            blackout: vec![],
            last_blackout: None,
            dump_active: true,
            finished: false,
            mode: options.mode,
            ownership: IdMap::default(),
            signal_ownership: Vec::new(),
            snapshot: None,
        })
    }
    pub fn identity(&self) -> crate::framing::RecordingId {
        self.output.identity()
    }
    pub fn worker_cpu(&self) -> Vec<super::cpu::WorkerCpu> {
        self.output.worker_cpu()
    }
    fn alive(&self) -> Result<()> {
        if self.finished {
            Err(Error::State("recording is finalized"))
        } else {
            Ok(())
        }
    }
    pub fn bind<T: Payload + 'static>(&mut self) -> Result<u32> {
        self.alive()?;
        let key = RustType::of::<T>();
        if let Some(id) = self.bindings.get(&key) {
            return Ok(*id);
        }
        let id = self.declarations.types.bind::<T>()?;
        self.bindings.insert(key, id);
        Ok(id)
    }
    pub fn intern(&mut self, name: &str) -> Result<u32> {
        self.alive()?;
        self.declarations.intern(name)
    }
    pub fn metadata(&mut self, metadata: Metadata) -> Result<()> {
        self.alive()?;
        if self.metadata_written {
            return Err(Error::State("metadata has been published"));
        }
        metadata.encode(self.declarations.strings.len() as u64)?;
        self.metadata = metadata;
        Ok(())
    }
    fn node(&mut self, options: SourceOptions, data: NodeData) -> Result<NodeId> {
        self.alive()?;
        if options.parent.is_some_and(|parent| {
            self.declarations
                .nodes
                .get(parent.0 as usize)
                .is_none_or(|node| !matches!(node.data, NodeData::Scope { .. }))
        }) {
            return Err(Error::invalid("parent must be a scope"));
        }
        validate_attributes(&options.attributes, self.declarations.strings.len() as u64)?;
        let name = self.intern(&options.name)?;
        let id = u32::try_from(self.declarations.nodes.len())
            .ok()
            .filter(|id| *id != u32::MAX)
            .ok_or_else(|| Error::invalid("node identifier exhausted"))?;
        self.declarations.nodes.push(Node {
            parent: options.parent,
            name,
            data,
            attributes: options.attributes,
        });
        Ok(NodeId(id))
    }
    pub fn scope(
        &mut self,
        options: SourceOptions,
        scope_type: ScopeType,
        component: &str,
    ) -> Result<NodeId> {
        let component = self.intern(component)?;
        self.node(
            options,
            NodeData::Scope {
                scope_type: scope_type.code(),
                component,
            },
        )
    }
    pub fn source<T: Payload + 'static>(
        &mut self,
        options: SourceOptions,
        kind: &str,
    ) -> Result<u32> {
        let payload_type = self.bind::<T>()?;
        let kind_id = self.intern(kind)?;
        let category = match kind {
            "EVENT" => RecordKind::Event,
            "STAGE" => RecordKind::Stage,
            "LOG" => RecordKind::Log,
            "CLOCK" => RecordKind::Clock,
            _ => RecordKind::Transaction,
        };
        if category == RecordKind::Clock && payload_type != 0 {
            return Err(Error::invalid("clock requires unit payload"));
        }
        if let TypeKind::Struct(fields) = &self.declarations.types.get(payload_type)?.kind
            && fields
                .iter()
                .any(|field| field.name == "vtr.label" && !matches!(field.datatype, 12 | 13))
        {
            return Err(Error::invalid("payload label must be string or text"));
        }
        let id = self
            .node(
                options,
                NodeData::Generator {
                    kind: kind_id,
                    payload_type,
                },
            )?
            .0;
        self.declarations.generators.declare(Generator {
            id,
            kind: category,
            payload_type,
        })?;
        if category == RecordKind::Clock {
            self.clocks.insert(
                id,
                Clock {
                    id: 0,
                    begin: 0,
                    period: 0,
                    previous: None,
                },
            );
        }
        Ok(id)
    }
    pub fn signal(
        &mut self,
        options: SourceOptions,
        kind: SignalKind,
        var_type: VarType,
        direction: Direction,
    ) -> Result<SignalId> {
        if options.parent.is_some_and(|parent| {
            self.declarations
                .nodes
                .get(parent.0 as usize)
                .is_none_or(|node| !matches!(node.data, NodeData::Scope { .. }))
        }) {
            return Err(Error::invalid("parent must be a scope"));
        }
        validate_attributes(&options.attributes, self.declarations.strings.len() as u64)?;
        self.flush_waves()?;
        let signal = SignalId(self.waveform.add_signal(kind, var_type == VarType::Event)?);
        self.declarations.signals.push(kind);
        self.signal_ownership.push(None);
        self.node(
            options,
            NodeData::Signal {
                var_type: var_type.code(),
                direction: direction as u8,
                signal,
                alias: false,
            },
        )?;
        Ok(signal)
    }
    pub fn alias(
        &mut self,
        options: SourceOptions,
        signal: SignalId,
        var_type: VarType,
        direction: Direction,
    ) -> Result<NodeId> {
        if signal.0 as usize >= self.declarations.signals.len() {
            return Err(Error::invalid("unknown aliased signal"));
        }
        if self.waveform.event(signal.0) != (var_type == VarType::Event) {
            return Err(Error::invalid(
                "event aliases must retain event classification",
            ));
        }
        self.node(
            options,
            NodeData::Signal {
                var_type: var_type.code(),
                direction: direction as u8,
                signal,
                alias: true,
            },
        )
    }
    pub fn enum_table(
        &mut self,
        options: SourceOptions,
        entries: Vec<(u32, u32)>,
    ) -> Result<NodeId> {
        for (name, bits) in &entries {
            self.declarations.string(*name)?;
            self.declarations.string(*bits)?;
        }
        self.node(options, NodeData::EnumTable { entries })
    }
    pub fn node_attribute(&mut self, node: NodeId, attribute: Attribute) -> Result<()> {
        if node.0 < self.written_nodes {
            return Err(Error::State("node metadata is immutable after publication"));
        }
        validate_attributes(
            std::slice::from_ref(&attribute),
            self.declarations.strings.len() as u64,
        )?;
        let name = self.declarations.string(attribute.key)?;
        if name == "vtr.label" && !matches!(attribute.value, Value::Text(_) | Value::Str(_)) {
            return Err(Error::invalid("invalid label type"));
        }
        if name == "vtr.clock" {
            let Value::U32(clock) = attribute.value else {
                return Err(Error::invalid("clock association must be u32"));
            };
            if self.declarations.generators.get(clock)?.kind != RecordKind::Clock {
                return Err(Error::invalid("associated source is not a clock"));
            }
        }
        let node = self
            .declarations
            .nodes
            .get_mut(node.0 as usize)
            .ok_or_else(|| Error::invalid("unknown node"))?;
        if node.attributes.iter().any(|a| a.key == attribute.key) {
            return Err(Error::invalid("duplicate node attribute"));
        }
        node.attributes.push(attribute);
        Ok(())
    }
    fn publish(&mut self) -> Result<()> {
        if self.written_strings as usize != self.declarations.strings.len() {
            let raw = self.declarations.strings_chunk(self.written_strings)?;
            let count = self.declarations.strings.len() as u64 - u64::from(self.written_strings);
            self.output.section(
                Section::new(2, u64::from(self.written_strings), count, 0),
                wire::compress(&raw, self.encoding.compression)?,
            )?;
            self.written_strings = self.declarations.strings.len() as u32;
        }
        if !self.metadata_written {
            self.output.section(
                Section::new(1, 0, 0, 0),
                self.metadata
                    .encode(self.declarations.strings.len() as u64)?,
            )?;
            self.metadata_written = true;
        }
        if self.written_types as usize != self.declarations.types.len() {
            let raw = self.declarations.types.declarations(self.written_types)?;
            let count = self.declarations.types.len() as u64 - u64::from(self.written_types);
            self.output.section(
                Section::new(10, u64::from(self.written_types), count, 0),
                raw,
            )?;
            self.written_types = self.declarations.types.len() as u32;
        }
        if self.written_nodes as usize != self.declarations.nodes.len() {
            let raw = self.declarations.hierarchy_chunk(self.written_nodes)?;
            let count = self.declarations.nodes.len() as u64 - u64::from(self.written_nodes);
            self.output.section(
                Section::new(3, u64::from(self.written_nodes), count, 0),
                wire::compress(&raw, self.encoding.compression)?,
            )?;
            self.written_nodes = self.declarations.nodes.len() as u32;
        }
        Ok(())
    }
    fn claim(&mut self, resource: Resource, producer: ProducerId, stamp: u64) -> Result<()> {
        if self.mode == super::ExecutionMode::Inline {
            return Ok(());
        }
        if let Resource::Signal(signal) = resource {
            let claim = self
                .signal_ownership
                .get_mut(signal as usize)
                .ok_or_else(|| Error::invalid("unknown signal"))?;
            if claim.is_some_and(|(owner, time)| time == stamp && owner != producer) {
                return Err(Error::invalid(
                    "resource has multiple tracing owners at one producer timestamp",
                ));
            }
            *claim = Some((producer, stamp));
        } else {
            match self.ownership.entry(resource) {
                std::collections::hash_map::Entry::Occupied(mut claim) => {
                    let &(owner, time) = claim.get();
                    if time == stamp && owner != producer {
                        return Err(Error::invalid(
                            "resource has multiple tracing owners at one producer timestamp",
                        ));
                    }
                    claim.insert((producer, stamp));
                }
                std::collections::hash_map::Entry::Vacant(claim) => {
                    claim.insert((producer, stamp));
                }
            }
        }
        Ok(())
    }
    fn check_claim(&self, resource: Resource, producer: ProducerId, stamp: u64) -> Result<()> {
        if self.mode == super::ExecutionMode::Inline {
            return Ok(());
        }
        let claim = match resource {
            Resource::Signal(signal) => self
                .signal_ownership
                .get(signal as usize)
                .ok_or_else(|| Error::invalid("unknown signal"))?
                .as_ref(),
            _ => self.ownership.get(&resource),
        };
        if claim.is_some_and(|(owner, time)| *time == stamp && *owner != producer) {
            return Err(Error::invalid(
                "resource has multiple tracing owners at one producer timestamp",
            ));
        }
        Ok(())
    }
    fn created(&mut self, id: u64) -> Result<()> {
        if id == 0 {
            return Err(Error::invalid("zero record identifier"));
        }
        self.max_created = self.max_created.max(id);
        Ok(())
    }
    fn payload(&self, generator: u32, expected: RecordKind, payload: &[u8]) -> Result<()> {
        let source = self.declarations.generators.get(generator)?;
        if source.kind != expected {
            return Err(Error::invalid(
                "source kind does not match recording operation",
            ));
        }
        self.declarations.types.validate(
            source.payload_type,
            payload,
            self.declarations.strings.len() as u64,
        )
    }
    fn begin_payload(
        &self,
        generator: u32,
        expected: RecordKind,
        payload: Option<&[u8]>,
    ) -> Result<bool> {
        if let Some(payload) = payload {
            self.payload(generator, expected, payload)?;
            return Ok(true);
        }
        let source = self.declarations.generators.get(generator)?;
        if source.kind != expected {
            return Err(Error::invalid(
                "source kind does not match recording operation",
            ));
        }
        Ok(self.declarations.types.get(source.payload_type)?.fixed_size == Some(0))
    }
    fn replace_payload(&mut self, id: u64, payload: Vec<u8>) -> Result<()> {
        if let Some(owner) = self.owners.get(id) {
            self.payload(owner.value.generator, RecordKind::Transaction, &payload)?;
            let owner = self.owners.get_mut(id).unwrap();
            owner.value.payload = payload;
            owner.payload_ready = true;
        } else if let Some(stage) = self.stages.get(&id) {
            self.payload(stage.value.generator, RecordKind::Stage, &payload)?;
            let stage = self.stages.get_mut(&id).unwrap();
            stage.value.payload = payload;
            stage.payload_ready = true;
        } else {
            return Err(Error::invalid("record is not active"));
        }
        Ok(())
    }
    fn attributes(&self, attributes: &[Attribute]) -> Result<()> {
        validate_attributes(attributes, self.declarations.strings.len() as u64)?;
        for attr in attributes {
            if self.declarations.string(attr.key)? == "vtr.label"
                && !matches!(attr.value, Value::Text(_) | Value::Str(_))
            {
                return Err(Error::invalid("label must be string or text"));
            }
        }
        Ok(())
    }
    pub(super) fn payload_buffer(&mut self, length: usize) -> Vec<u8> {
        self.payload_pool.take(length)
    }
    pub(super) fn apply_payload(
        &mut self,
        producer: ProducerId,
        stamp: u64,
        action: PayloadOperation,
        id: u64,
        payload: Option<&[u8]>,
    ) -> Result<()> {
        let payload = payload.map(|bytes| {
            let mut buffer = self.payload_pool.take(bytes.len());
            buffer.extend_from_slice(bytes);
            buffer
        });
        self.apply_owned_payload(producer, stamp, action, id, payload)
    }
    #[inline]
    pub(super) fn apply_owned_payload(
        &mut self,
        producer: ProducerId,
        stamp: u64,
        action: PayloadOperation,
        id: u64,
        payload: Option<Vec<u8>>,
    ) -> Result<()> {
        self.alive()?;
        self.captured_time = self.captured_time.max(stamp);
        match action {
            PayloadOperation::Begin {
                generator,
                time,
                parent,
            } => {
                let payload_ready =
                    self.begin_payload(generator, RecordKind::Transaction, payload.as_deref())?;
                if parent != 0 && !self.owners.contains_key(parent) {
                    return Err(Error::invalid("parent owner is not active"));
                }
                self.created(id)?;
                self.claim(Resource::Record(id), producer, stamp)?;
                self.owners.begin(
                    id,
                    generator,
                    time,
                    parent,
                    payload.unwrap_or_default(),
                    payload_ready,
                )?;
            }
            PayloadOperation::BeginStage {
                generator,
                time,
                parent,
            } => {
                let payload_ready =
                    self.begin_payload(generator, RecordKind::Stage, payload.as_deref())?;
                let owner = self
                    .owners
                    .get_mut(parent)
                    .ok_or_else(|| Error::invalid("stage owner is not active"))?;
                owner.stages.push(id);
                self.created(id)?;
                self.claim(Resource::Record(id), producer, stamp)?;
                self.stages.insert(
                    id,
                    ActiveStage {
                        value: LocatedRecord {
                            id,
                            generator,
                            begin: time,
                            end: time,
                            parent,
                            payload: payload.unwrap_or_default(),
                        },
                        payload_ready,
                    },
                );
            }
            PayloadOperation::Point {
                generator,
                time,
                parent,
                log,
            } => {
                let payload = payload.expect("point payload");
                self.payload(
                    generator,
                    if log {
                        RecordKind::Log
                    } else {
                        RecordKind::Event
                    },
                    &payload,
                )?;
                if parent != 0 && !self.owners.contains_key(parent) {
                    return Err(Error::invalid("point owner is not active"));
                }
                if !log && parent == 0 {
                    return Err(Error::invalid("located event requires an owner"));
                }
                self.created(id)?;
                self.submit_record(LocatedRecord {
                    id,
                    generator,
                    begin: time,
                    end: time,
                    parent,
                    payload,
                })?;
            }
            PayloadOperation::End { id, time, status } => {
                self.claim(Resource::Record(id), producer, stamp)?;
                self.end_owner(id, time, status, payload)?;
            }
            PayloadOperation::EndStage { id, time } => {
                self.claim(Resource::Record(id), producer, stamp)?;
                self.end_stage(id, time, payload)?;
            }
            PayloadOperation::Replace { id } => {
                self.claim(Resource::Record(id), producer, stamp)?;
                self.replace_payload(id, payload.expect("replacement payload"))?;
            }
        }
        Ok(())
    }
    pub fn apply(&mut self, producer: ProducerId, stamp: u64, operation: Operation) -> Result<()> {
        self.alive()?;
        self.captured_time = self.captured_time.max(stamp);
        match operation {
            Operation::Progress(time) => self.waveform.advance(time)?,
            Operation::Emit { signal, value } => {
                self.emit(producer, stamp, signal, value.borrow())?;
            }
            Operation::EmitBatch(updates) => {
                for update in &updates {
                    self.waveform.validate_unsigned(update.signal.0)?;
                    self.check_claim(Resource::Signal(update.signal.0), producer, stamp)?;
                }
                for update in updates {
                    self.emit(
                        producer,
                        stamp,
                        update.signal,
                        SignalRef::Unsigned(update.value),
                    )?;
                }
            }
            Operation::Begin {
                id,
                generator,
                time,
                parent,
                payload,
            } => {
                self.apply_owned_payload(
                    producer,
                    stamp,
                    PayloadOperation::Begin {
                        generator,
                        time,
                        parent,
                    },
                    id,
                    payload,
                )?;
            }
            Operation::BeginStage {
                id,
                generator,
                time,
                parent,
                payload,
            } => {
                self.apply_owned_payload(
                    producer,
                    stamp,
                    PayloadOperation::BeginStage {
                        generator,
                        time,
                        parent,
                    },
                    id,
                    payload,
                )?;
            }
            Operation::Point {
                id,
                generator,
                time,
                parent,
                payload,
                log,
            } => {
                self.apply_owned_payload(
                    producer,
                    stamp,
                    PayloadOperation::Point {
                        generator,
                        time,
                        parent,
                        log,
                    },
                    id,
                    Some(payload),
                )?;
            }
            Operation::End {
                id,
                time,
                status,
                payload,
            } => {
                self.apply_owned_payload(
                    producer,
                    stamp,
                    PayloadOperation::End { id, time, status },
                    id,
                    payload,
                )?;
            }
            Operation::EndStage { id, time, payload } => {
                self.apply_owned_payload(
                    producer,
                    stamp,
                    PayloadOperation::EndStage { id, time },
                    id,
                    payload,
                )?;
            }
            Operation::Payload { id, payload } => {
                self.apply_owned_payload(
                    producer,
                    stamp,
                    PayloadOperation::Replace { id },
                    id,
                    Some(payload),
                )?;
            }
            Operation::Attribute { id, attribute } => {
                self.claim(Resource::Record(id), producer, stamp)?;
                self.attributes(std::slice::from_ref(&attribute))?;
                let owner = self
                    .owners
                    .get(id)
                    .ok_or_else(|| Error::invalid("owner is not active"))?;
                let name = self.declarations.string(attribute.key)?;
                if let TypeKind::Struct(fields) = &self
                    .declarations
                    .types
                    .get(
                        self.declarations
                            .generators
                            .get(owner.value.generator)?
                            .payload_type,
                    )?
                    .kind
                    && fields.iter().any(|field| field.name == name)
                {
                    return Err(Error::invalid("attribute duplicates payload field"));
                }
                if owner
                    .value
                    .attributes
                    .iter()
                    .any(|a| a.key == attribute.key)
                {
                    return Err(Error::invalid("duplicate record attribute"));
                }
                self.owners
                    .get_mut(id)
                    .unwrap()
                    .value
                    .attributes
                    .push(attribute);
            }
            Operation::SpanKind { id, kind } => {
                if kind > 5 {
                    return Err(Error::invalid("invalid span kind"));
                }
                self.claim(Resource::Record(id), producer, stamp)?;
                self.owners
                    .get_mut(id)
                    .ok_or_else(|| Error::invalid("owner is not active"))?
                    .value
                    .span_kind = kind;
            }
            Operation::EmbeddedEvent { owner, event } => {
                self.declarations.string(event.name)?;
                self.attributes(&event.attributes)?;
                self.created(event.id)?;
                self.claim(Resource::Record(owner), producer, stamp)?;
                self.owners
                    .get_mut(owner)
                    .ok_or_else(|| Error::invalid("owner is not active"))?
                    .value
                    .events
                    .push(event);
            }
            Operation::EmbeddedStage { owner, stage } => {
                self.declarations.string(stage.name)?;
                self.declarations.string(stage.lane)?;
                self.attributes(&stage.attributes)?;
                if stage.end.is_some_and(|end| end < stage.begin) {
                    return Err(Error::invalid("negative embedded stage duration"));
                }
                self.created(stage.id)?;
                self.claim(Resource::Record(owner), producer, stamp)?;
                self.owners
                    .get_mut(owner)
                    .ok_or_else(|| Error::invalid("owner is not active"))?
                    .value
                    .stages
                    .push(stage);
            }
            Operation::EndEmbedded { owner, stage, time } => {
                self.claim(Resource::Record(owner), producer, stamp)?;
                let stage = self
                    .owners
                    .get_mut(owner)
                    .ok_or_else(|| Error::invalid("owner is not active"))?
                    .value
                    .stages
                    .iter_mut()
                    .find(|item| item.id == stage)
                    .ok_or_else(|| Error::invalid("unknown embedded stage"))?;
                stage.end = Some(time.max(stage.begin));
            }
            Operation::Relate(relation) => {
                self.declarations.string(relation.kind)?;
                self.attributes(&relation.attributes)?;
                self.max_reference = self
                    .max_reference
                    .max(relation.source)
                    .max(relation.destination);
                self.tx_bytes += std::mem::size_of::<Relation>()
                    + super::transport::attributes_charge(&relation.attributes);
                self.peak_transaction_bytes = self.peak_transaction_bytes.max(self.tx_bytes);
                self.relations.push(relation);
                self.maybe_flush_transactions()?;
            }
            Operation::ClockRun {
                generator,
                id,
                first,
                period,
            } => {
                if period == 0 {
                    return Err(Error::invalid("zero clock period"));
                }
                self.claim(Resource::Clock(generator), producer, stamp)?;
                let clock = self
                    .clocks
                    .get(&generator)
                    .ok_or_else(|| Error::invalid("not a clock generator"))?;
                if clock.id != 0 {
                    if first <= clock.begin {
                        return Err(Error::invalid("clock stretches share an edge"));
                    }
                    self.stop_clock(generator, first - 1, 0)?;
                }
                let clock = self.clocks.get(&generator).unwrap();
                if clock.previous.is_some_and(|previous| first <= previous) {
                    return Err(Error::invalid("clock stretches overlap"));
                }
                self.created(id)?;
                let clock = self.clocks.get_mut(&generator).unwrap();
                clock.id = id;
                clock.begin = first;
                clock.period = period;
            }
            Operation::ClockStop { generator, time } => {
                self.claim(Resource::Clock(generator), producer, stamp)?;
                self.stop_clock(generator, time, 0)?;
            }
            Operation::Blackout { time, active } => {
                self.blackout(time, active)?;
            }
            Operation::Dump(enabled) => {
                if self.dump_active != enabled {
                    self.blackout(stamp, enabled)?;
                    self.dump_active = enabled;
                }
            }
        }
        Ok(())
    }
    #[inline]
    pub(super) fn advance(&mut self, time: u64) -> Result<()> {
        self.alive()?;
        self.captured_time = self.captured_time.max(time);
        self.waveform.advance(time)
    }
    #[inline]
    pub(super) fn emit_signal(
        &mut self,
        producer: ProducerId,
        stamp: u64,
        signal: SignalId,
        value: super::SignalInput,
    ) -> Result<()> {
        self.emit_signal_ref(producer, stamp, signal, value.borrow())
    }
    #[inline]
    pub(super) fn emit_signal_ref(
        &mut self,
        producer: ProducerId,
        stamp: u64,
        signal: SignalId,
        value: SignalRef<'_>,
    ) -> Result<()> {
        self.alive()?;
        self.captured_time = self.captured_time.max(stamp);
        self.emit(producer, stamp, signal, value)
    }
    fn emit(
        &mut self,
        producer: ProducerId,
        stamp: u64,
        signal: SignalId,
        value: SignalRef<'_>,
    ) -> Result<()> {
        self.claim(Resource::Signal(signal.0), producer, stamp)?;
        self.waveform.emit(
            signal.0,
            stamp,
            value,
            waveform::Options {
                group_size: self.metadata.group_size,
                deduplicate: self.encoding.waveform_deduplicate,
                enabled: self.dump_active,
            },
        )?;
        if self
            .waveform
            .should_flush(self.encoding.waveform_bytes, self.encoding.waveform_records)
        {
            self.flush_waves()?;
        }
        Ok(())
    }
    fn blackout(&mut self, time: u64, active: bool) -> Result<()> {
        if self.last_blackout.is_some_and(|last| last > time) {
            return Err(Error::invalid("blackout time moved backwards"));
        }
        self.blackout.push((time, active));
        self.last_blackout = Some(time);
        Ok(())
    }
    fn policy(&self) -> RecordPolicy {
        RecordPolicy {
            page_bytes: self.encoding.page_bytes,
            compression: self.encoding.compression,
            separate_logs: self.encoding.separate_logs,
        }
    }
    fn snapshot(&mut self) -> (Arc<Types>, Arc<Generators>, usize) {
        let types = self.declarations.types.len();
        let nodes = self.declarations.nodes.len();
        if self
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.types_count != types || snapshot.nodes_count != nodes)
        {
            let snapshot_types = Arc::new(self.declarations.types.clone());
            let generators = Arc::new(self.declarations.generators.clone());
            let charge = snapshot_types.memory_charge() + generators.memory_charge();
            self.snapshot = Some(SchemaSnapshot {
                types_count: types,
                nodes_count: nodes,
                types: snapshot_types,
                generators,
                charge,
            });
        }
        let snapshot = self.snapshot.as_ref().unwrap();
        (
            snapshot.types.clone(),
            snapshot.generators.clone(),
            snapshot.charge,
        )
    }
    fn block_number(&mut self) -> Result<u64> {
        let id = self.next_block;
        self.next_block = id
            .checked_add(1)
            .ok_or_else(|| Error::invalid("block number exhausted"))?;
        Ok(id)
    }
    fn submit_record(&mut self, record: LocatedRecord) -> Result<()> {
        self.record_bytes += std::mem::size_of::<LocatedRecord>() + record.payload.capacity();
        self.peak_record_bytes = self.peak_record_bytes.max(self.record_bytes);
        self.records.push(record);
        if self.record_bytes >= self.encoding.block_bytes
            || self.records.len() >= self.encoding.block_records
        {
            self.flush_records()?;
        }
        Ok(())
    }
    fn flush_records(&mut self) -> Result<()> {
        if self.records.is_empty() {
            return Ok(());
        }
        self.publish()?;
        let number = self.block_number()?;
        for record in &self.records {
            if self.declarations.generators.get(record.generator)?.kind != RecordKind::Log {
                let owner = self.owners.get_mut(record.parent).ok_or_else(|| {
                    Error::invalid("located owner was closed before publishing child locators")
                })?;
                if owner.value.child_blocks.last() != Some(&number) {
                    owner.value.child_blocks.push(number);
                }
            }
        }
        let begin = self.records.iter().map(|r| r.begin).min().unwrap();
        let end = self.records.iter().map(|r| r.end).max().unwrap();
        let bytes = self.records.capacity() * std::mem::size_of::<LocatedRecord>()
            + self
                .records
                .iter()
                .map(|r| r.payload.capacity())
                .sum::<usize>();
        let mut records = std::mem::take(&mut self.records);
        let (types, generators, schema_bytes) = self.snapshot();
        let policy = self.policy();
        let strings = self.declarations.strings.len() as u64;
        let recycle = self.payload_pool.return_handle();
        if self.encoding.workers == 0 {
            let result = self
                .output
                .encode_now(Section::new(9, begin, end, number), || {
                    encode_record_block(number, &types, &generators, &records, policy, strings)
                });
            recycle.recycle(
                records
                    .iter_mut()
                    .map(|record| std::mem::take(&mut record.payload)),
            );
            records.clear();
            self.records = records;
            result?;
            self.record_bytes = 0;
            return Ok(());
        }
        self.output.encode(
            Section::new(9, begin, end, number),
            bytes + schema_bytes,
            move || {
                let result =
                    encode_record_block(number, &types, &generators, &records, policy, strings);
                recycle.recycle(
                    records
                        .iter_mut()
                        .map(|record| std::mem::take(&mut record.payload)),
                );
                result
            },
        )?;
        self.record_bytes = 0;
        Ok(())
    }
    #[inline]
    fn submit_transaction(&mut self, transaction: Transaction) -> Result<()> {
        self.tx_bytes +=
            std::mem::size_of::<Transaction>() + Self::transaction_owned_bytes(&transaction);
        self.peak_transaction_bytes = self.peak_transaction_bytes.max(self.tx_bytes);
        self.transactions.push(transaction);
        self.maybe_flush_transactions()
    }
    fn maybe_flush_transactions(&mut self) -> Result<()> {
        if self.tx_bytes >= self.encoding.block_bytes
            || self.transactions.len() + self.relations.len() >= self.encoding.block_records
        {
            self.flush_transactions()?;
        }
        Ok(())
    }
    fn flush_transactions(&mut self) -> Result<()> {
        if self.transactions.is_empty() && self.relations.is_empty() {
            return Ok(());
        }
        self.publish()?;
        let number = self.block_number()?;
        let (begin, end) = if self.transactions.is_empty() {
            (0, 0)
        } else {
            (
                self.transactions.iter().map(|tx| tx.begin).min().unwrap(),
                self.transactions.iter().map(|tx| tx.end).max().unwrap(),
            )
        };
        let bytes = self.transactions.capacity() * std::mem::size_of::<Transaction>()
            + self
                .transactions
                .iter()
                .map(Self::transaction_owned_bytes)
                .sum::<usize>()
            + self.relations.capacity() * std::mem::size_of::<Relation>()
            + self
                .relations
                .iter()
                .map(|relation| super::transport::attributes_charge(&relation.attributes))
                .sum::<usize>();
        let mut transactions = std::mem::take(&mut self.transactions);
        let relations = std::mem::take(&mut self.relations);
        let (types, generators, schema_bytes) = self.snapshot();
        let policy = self.policy();
        let strings = self.declarations.strings.len() as u64;
        let recycle = self.payload_pool.return_handle();
        if self.encoding.workers == 0 {
            let result = self
                .output
                .encode_now(Section::new(5, begin, end, number), || {
                    encode_transaction_block(
                        number,
                        &types,
                        &generators,
                        &transactions,
                        &relations,
                        TransactionPolicy {
                            pages: policy,
                            string_count: strings,
                            shared_text: None,
                        },
                    )
                });
            recycle.recycle(
                transactions
                    .iter_mut()
                    .map(|record| std::mem::take(&mut record.payload)),
            );
            transactions.clear();
            self.transactions = transactions;
            let mut relations = relations;
            relations.clear();
            self.relations = relations;
            result?;
            self.tx_bytes = 0;
            return Ok(());
        }
        self.output.encode(
            Section::new(5, begin, end, number),
            bytes + schema_bytes,
            move || {
                let result = encode_transaction_block(
                    number,
                    &types,
                    &generators,
                    &transactions,
                    &relations,
                    TransactionPolicy {
                        pages: policy,
                        string_count: strings,
                        shared_text: None,
                    },
                );
                recycle.recycle(
                    transactions
                        .iter_mut()
                        .map(|record| std::mem::take(&mut record.payload)),
                );
                result
            },
        )?;
        self.tx_bytes = 0;
        Ok(())
    }
    fn end_stage(&mut self, id: u64, time: u64, payload: Option<Vec<u8>>) -> Result<()> {
        if let Some(payload) = payload {
            self.replace_payload(id, payload)?;
        }
        if !self
            .stages
            .get(&id)
            .ok_or_else(|| Error::invalid("stage is not active"))?
            .payload_ready
        {
            return Err(Error::invalid("missing mandatory stage payload"));
        }
        let mut stage = self
            .stages
            .remove(&id)
            .ok_or_else(|| Error::invalid("stage is not active"))?
            .value;
        stage.end = time.max(stage.begin);
        self.owners
            .get_mut(stage.parent)
            .ok_or_else(|| Error::invalid("stage owner is not active"))?
            .stages
            .retain(|stage| *stage != id);
        self.ownership.remove(&Resource::Record(id));
        self.submit_record(stage)
    }
    fn end_owner(
        &mut self,
        id: u64,
        time: u64,
        status: u8,
        payload: Option<Vec<u8>>,
    ) -> Result<()> {
        if status > 4 {
            return Err(Error::invalid("invalid transaction status"));
        }
        let owner = self
            .owners
            .get(id)
            .ok_or_else(|| Error::invalid("owner is not active"))?;
        if !owner.payload_ready && payload.is_none() {
            return Err(Error::invalid("missing mandatory transaction payload"));
        }
        if owner.stages.iter().any(|id| !self.stages[id].payload_ready) {
            return Err(Error::invalid("missing mandatory stage payload"));
        }
        let end = time.max(owner.value.begin);
        let stages = owner.stages.clone();
        if let Some(payload) = payload {
            self.replace_payload(id, payload)?;
        }
        for stage in stages {
            self.end_stage(stage, end, None)?;
        }
        self.flush_records()?;
        let owner = &mut self.owners.get_mut(id).unwrap().value;
        owner.end = end;
        owner.status = status;
        self.tx_bytes += std::mem::size_of::<Transaction>() + Self::transaction_owned_bytes(owner);
        self.peak_transaction_bytes = self.peak_transaction_bytes.max(self.tx_bytes);
        self.owners.finish_into(id, &mut self.transactions);
        self.ownership.remove(&Resource::Record(id));
        self.maybe_flush_transactions()
    }
    fn stop_clock(&mut self, generator: u32, time: u64, status: u8) -> Result<()> {
        let clock = self
            .clocks
            .get(&generator)
            .filter(|clock| clock.id != 0)
            .ok_or_else(|| Error::invalid("clock is not running"))?;
        let last =
            clock.begin + (time.max(clock.begin) - clock.begin) / clock.period * clock.period;
        let (id, begin, period) = (clock.id, clock.begin, clock.period);
        let attributes = if last == begin {
            vec![]
        } else {
            vec![Attribute {
                key: self.intern("vtr.period")?,
                value: Value::Time(period),
            }]
        };
        if self.encoding.separate_clocks {
            self.flush_transactions()?;
        }
        self.submit_transaction(Transaction {
            id,
            generator,
            begin,
            end: last,
            status,
            attributes,
            ..Transaction::default()
        })?;
        if self.encoding.separate_clocks {
            self.flush_transactions()?;
        }
        let clock = self.clocks.get_mut(&generator).unwrap();
        clock.previous = Some(last);
        clock.id = 0;
        Ok(())
    }
    fn flush_waves(&mut self) -> Result<()> {
        if let Some((begin, end, job)) = self.waveform.prepare(self.metadata.group_size)? {
            self.publish()?;
            let compression = self.encoding.compression;
            let bytes = job.bytes();
            if self.encoding.workers == 0 {
                let encoded = self.wave_encoder.encode(&job, compression);
                self.waveform.reclaim(job);
                self.output
                    .encode(Section::new(4, begin, end, 0), bytes, move || encoded)?;
            } else {
                self.output
                    .encode(Section::new(4, begin, end, 0), bytes, move || {
                        job.encode(compression)
                    })?;
            }
        }
        Ok(())
    }
    pub fn flush(&mut self) -> Result<()> {
        self.alive()?;
        self.flush_records()?;
        self.flush_transactions()?;
        self.flush_waves()?;
        self.publish()?;
        self.flush_blackouts()?;
        self.output.flush()
    }
    fn flush_blackouts(&mut self) -> Result<()> {
        if self.blackout.is_empty() {
            return Ok(());
        }
        let mut payload = Vec::new();
        crate::varint::put_u64(&mut payload, self.blackout.len() as u64);
        let mut previous = 0;
        for (time, active) in &self.blackout {
            payload.push(u8::from(*active));
            crate::varint::put_u64(&mut payload, time - previous);
            previous = *time;
        }
        self.output.section(Section::new(6, 0, 0, 0), payload)?;
        self.blackout.clear();
        Ok(())
    }
    pub fn finish_with(
        &mut self,
        incomplete: bool,
        ending: Option<(super::Ending, u64)>,
    ) -> Result<Fingerprint> {
        if let Some((ending, id)) = ending
            && !self.output.failed()
        {
            let scope = self.scope(
                SourceOptions::new(super::ending::SCOPE, None),
                ScopeType::Generic,
                "",
            )?;
            let marker = self.intern(super::ending::MARKER)?;
            let severity = self.intern("log.severity")?;
            let mut options = SourceOptions::new("run {}", Some(scope));
            options.attributes = vec![
                Attribute {
                    key: marker,
                    value: Value::Bool(true),
                },
                Attribute {
                    key: severity,
                    value: Value::U64(5),
                },
            ];
            let generator = self.source::<super::ending::RunEnding>(options, "LOG")?;
            let payload = crate::schema::capture(&super::ending::RunEnding::from(ending))?;
            self.apply(
                ProducerId(0),
                self.captured_time,
                Operation::Point {
                    id,
                    generator,
                    time: self.captured_time,
                    parent: 0,
                    payload,
                    log: true,
                },
            )?;
        }
        self.finish(incomplete)
    }
    pub fn finish(&mut self, incomplete: bool) -> Result<Fingerprint> {
        self.alive()?;
        if incomplete && self.output.failed() {
            let fingerprint = self.output.seal_failed()?;
            self.finished = true;
            return Ok(fingerprint);
        }
        if !incomplete && self.max_reference > self.max_created {
            return Err(Error::invalid(
                "unresolved relation endpoint at normal close",
            ));
        }
        if !incomplete
            && (self.owners.values().any(|owner| !owner.payload_ready)
                || self.stages.values().any(|stage| !stage.payload_ready))
        {
            return Err(Error::invalid("missing mandatory payload at normal close"));
        }
        if incomplete {
            self.discard_missing_payloads()?;
        }
        let mut owners: Vec<_> = self.owners.keys().copied().collect();
        owners.sort_unstable();
        for id in owners {
            self.end_owner(id, self.captured_time, 4, None)?;
        }
        let clocks: Vec<_> = self
            .clocks
            .iter()
            .filter(|(_, clock)| clock.id != 0)
            .map(|(id, _)| *id)
            .collect();
        for clock in clocks {
            self.stop_clock(clock, self.captured_time, 4)?;
        }
        self.flush()?;
        let fingerprint = self.output.finish(incomplete)?;
        self.finished = true;
        Ok(fingerprint)
    }
    fn discard_missing_payloads(&mut self) -> Result<()> {
        let mut stages: Vec<_> = self
            .stages
            .iter()
            .filter(|(_, stage)| !stage.payload_ready)
            .map(|(id, _)| *id)
            .collect();
        stages.sort_unstable();
        for id in stages {
            let stage = self.stages.remove(&id).unwrap();
            self.owners
                .get_mut(stage.value.parent)
                .unwrap()
                .stages
                .retain(|stage| *stage != id);
            self.ownership.remove(&Resource::Record(id));
        }
        let mut owners: Vec<_> = self
            .owners
            .iter()
            .filter(|(_, owner)| !owner.payload_ready)
            .map(|(id, _)| *id)
            .collect();
        owners.sort_unstable();
        for id in &owners {
            let stages = self.owners.get(*id).unwrap().stages.clone();
            for stage in stages {
                self.end_stage(stage, self.captured_time, None)?;
            }
        }
        // Surviving stages need their owners present while collecting child-block locators.
        self.flush_records()?;
        for id in owners {
            self.owners.remove(id);
            self.ownership.remove(&Resource::Record(id));
        }
        Ok(())
    }
    pub fn poll_output(&mut self) -> Result<()> {
        self.output.poll()
    }
    fn transaction_owned_bytes(value: &Transaction) -> usize {
        value.payload.capacity()
            + super::transport::attributes_charge(&value.attributes)
            + value.events.capacity() * std::mem::size_of::<crate::transactions::EmbeddedEvent>()
            + value
                .events
                .iter()
                .map(|event| super::transport::attributes_charge(&event.attributes))
                .sum::<usize>()
            + value.stages.capacity() * std::mem::size_of::<crate::transactions::EmbeddedStage>()
            + value
                .stages
                .iter()
                .map(|stage| super::transport::attributes_charge(&stage.attributes))
                .sum::<usize>()
            + value.child_blocks.capacity() * 8
    }
    pub fn stats(&self) -> super::WriterStats {
        super::WriterStats {
            live_owners: self.owners.len(),
            live_stages: self.stages.len(),
            max_created_id: self.max_created,
            output_bytes: self.output.position(),
            encoding: self.output.stats(),
            payload_pool_bytes: self.payload_pool.retained_bytes(),
            active_payload_bytes: self
                .owners
                .values()
                .map(|owner| Self::transaction_owned_bytes(&owner.value))
                .sum::<usize>()
                + self
                    .stages
                    .values()
                    .map(|stage| stage.value.payload.capacity())
                    .sum::<usize>(),
            peak_record_bytes: self.peak_record_bytes,
            peak_transaction_bytes: self.peak_transaction_bytes,
            peak_waveform_bytes: self.waveform.peak_bytes(),
            transaction_blocks: self.output.transaction_blocks,
            record_blocks: self.output.record_blocks,
            waveform_blocks: self.output.waveform_blocks,
            ..super::WriterStats::default()
        }
    }
}
