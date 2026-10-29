use std::collections::{BTreeMap, BTreeSet};

use super::Reader;
use crate::{
    Error, Result,
    attributes::{Attribute, Value},
    declarations::NodeData,
    framing::Completion,
    records::{RecordKind, RecordQuery},
    schema::TypeKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingKind {
    Parent,
    RelationSource,
    RelationDestination,
    ChildBlock,
    ClockGenerator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingReference {
    pub kind: MissingKind,
    pub source: u64,
    pub target: u64,
    pub block: u64,
}

#[derive(Clone, Debug)]
pub struct ValidationReport {
    pub incomplete: bool,
    pub records: u64,
    pub relations: u64,
    pub missing: Vec<MissingReference>,
}

impl ValidationReport {
    fn missing(&mut self, kind: MissingKind, source: u64, target: u64, block: u64) -> Result<()> {
        if !self.incomplete {
            return Err(Error::Corrupt(
                "unresolved reference in a complete recording",
            ));
        }
        self.missing.push(MissingReference {
            kind,
            source,
            target,
            block,
        });
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Identity {
    kind: RecordKind,
    parent: u64,
    block: u64,
}

impl Reader {
    fn validate_labels(&self, attributes: &[Attribute]) -> Result<()> {
        for attribute in attributes {
            if self.string(attribute.key)? == "vtr.label"
                && !matches!(attribute.value, Value::Str(_) | Value::Text(_))
            {
                return Err(Error::Corrupt("label must be a string or text"));
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<ValidationReport> {
        let mut report = ValidationReport {
            incomplete: self.completion() != Completion::Complete,
            records: 0,
            relations: 0,
            missing: Vec::new(),
        };
        self.validate_labels(&self.metadata.attributes)?;
        for id in crate::schema::FIRST_DECLARED_TYPE..self.types.len() as u32 {
            self.validate_labels(&self.types.get(id)?.attributes)?;
        }
        for (index, node) in self.declarations.nodes.iter().enumerate() {
            self.validate_labels(&node.attributes)?;
            let NodeData::Generator { payload_type, .. } = node.data else {
                continue;
            };
            if let TypeKind::Struct(fields) = &self.types.get(payload_type)?.kind
                && fields
                    .iter()
                    .any(|f| f.name == "vtr.label" && f.datatype != 12 && f.datatype != 13)
            {
                return Err(Error::Corrupt("invalid payload label datatype"));
            }
            for attribute in &node.attributes {
                let name = self.string(attribute.key)?;
                if name == "vtr.clock" {
                    let Value::U32(target) = attribute.value else {
                        return Err(Error::Corrupt("clock association must be u32"));
                    };
                    if target as usize >= self.declarations.nodes.len() {
                        report.missing(
                            MissingKind::ClockGenerator,
                            index as u64,
                            u64::from(target),
                            0,
                        )?;
                    } else if self.declarations.generators.get(target)?.kind != RecordKind::Clock {
                        return Err(Error::Corrupt("associated generator is not a clock"));
                    }
                }
            }
        }
        let mut records = BTreeMap::new();
        let mut declared = BTreeMap::new();
        let mut actual: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        let mut edges = Vec::new();
        let mut clocks: BTreeMap<u32, Vec<(u64, u64)>> = BTreeMap::new();
        let mut add = |id, kind, parent, block| -> Result<()> {
            if id == 0
                || records
                    .insert(
                        id,
                        Identity {
                            kind,
                            parent,
                            block,
                        },
                    )
                    .is_some()
            {
                return Err(Error::Corrupt("duplicate global record identifier"));
            }
            report.records += 1;
            Ok(())
        };
        for section in self.sections() {
            self.container.payload(section, true)?;
            if section.kind == 9 {
                let mut rows = Vec::new();
                self.record_block(section)?
                    .query(RecordQuery::default(), |row| {
                        rows.push(row);
                        true
                    })?;
                for row in rows {
                    let kind = self.declarations.generators.get(row.generator)?.kind;
                    add(row.id, kind, row.parent, section.block_number)?;
                    if matches!(kind, RecordKind::Stage | RecordKind::Event) {
                        actual
                            .entry(row.parent)
                            .or_default()
                            .insert(section.block_number);
                    }
                }
            }
            if section.kind == 5 {
                let mut rows = Vec::new();
                let mut relations = Vec::new();
                self.transaction_block(section)?.scan(
                    |row| {
                        rows.push(row);
                        true
                    },
                    |relation| {
                        relations.push(relation);
                        true
                    },
                )?;
                for row in rows {
                    let generator = self.declarations.generators.get(row.generator)?;
                    add(row.id, generator.kind, row.parent, section.block_number)?;
                    self.validate_labels(&row.attributes)?;
                    if let TypeKind::Struct(fields) = &self.types.get(generator.payload_type)?.kind
                    {
                        for attribute in &row.attributes {
                            let name = self.string(attribute.key)?;
                            if fields.iter().any(|field| field.name == name) {
                                return Err(Error::Corrupt("attribute duplicates a payload field"));
                            }
                        }
                    }
                    for event in &row.events {
                        add(
                            event.id,
                            RecordKind::EmbeddedEvent,
                            row.id,
                            section.block_number,
                        )?;
                        self.validate_labels(&event.attributes)?;
                    }
                    for stage in &row.stages {
                        add(
                            stage.id,
                            RecordKind::EmbeddedStage,
                            row.id,
                            section.block_number,
                        )?;
                        self.validate_labels(&stage.attributes)?;
                    }
                    if generator.kind == RecordKind::Clock {
                        self.clock_period(&row)?;
                        clocks
                            .entry(row.generator)
                            .or_default()
                            .push((row.begin, row.end));
                    }
                    declared.insert(row.id, row.child_blocks);
                }
                for relation in relations {
                    self.validate_labels(&relation.attributes)?;
                    edges.push((relation, section.block_number));
                }
            }
        }
        self.validate_waveforms()?;
        report.relations = edges.len() as u64;
        for (&id, identity) in &records {
            if identity.parent == 0 {
                continue;
            }
            if let Some(parent) = records.get(&identity.parent) {
                if parent.kind != RecordKind::Transaction || identity.parent >= id {
                    return Err(Error::Corrupt("invalid ownership kind or order"));
                }
            } else {
                report.missing(MissingKind::Parent, id, identity.parent, identity.block)?;
            }
        }
        for (edge, block) in edges {
            if edge.source != 0 && !records.contains_key(&edge.source) {
                report.missing(MissingKind::RelationSource, 0, edge.source, block)?;
            }
            if edge.destination != 0 && !records.contains_key(&edge.destination) {
                report.missing(MissingKind::RelationDestination, 0, edge.destination, block)?;
            }
        }
        for (owner, blocks) in &actual {
            if let Some(listed) = declared.get(owner)
                && blocks
                    .iter()
                    .any(|number| listed.binary_search(number).is_err())
            {
                return Err(Error::Corrupt("owner locator omits a located child block"));
            }
        }
        for (owner, blocks) in declared {
            for number in blocks {
                if let Some(index) = self.blocks.get(&number) {
                    if self.sections()[*index].kind != 9 {
                        return Err(Error::Corrupt("child locator references wrong block kind"));
                    }
                    if !actual
                        .get(&owner)
                        .is_some_and(|blocks| blocks.contains(&number))
                    {
                        return Err(Error::Corrupt("child locator has no matching child"));
                    }
                } else {
                    report.missing(
                        MissingKind::ChildBlock,
                        owner,
                        number,
                        records[&owner].block,
                    )?;
                }
            }
        }
        for stretches in clocks.values_mut() {
            stretches.sort_unstable();
            if stretches.windows(2).any(|pair| pair[0].1 >= pair[1].0) {
                return Err(Error::Corrupt("clock stretches share an edge"));
            }
        }
        Ok(report)
    }
}
