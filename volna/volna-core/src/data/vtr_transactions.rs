//! Resolve VTR's interned names at the adapter boundary.
use super::loaded_tracks::{LoadedGenerator, LoadedRelation, LoadedTrack, TransactionLocation};
use super::transactions::*;
use super::vtr_source::LocalSession;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use vtr::{NodeData, Reader, Value};

/// Load a whole raw track in one transaction scan and one relation scan.
/// Ordinals come directly from the reader's documented file-order traversal;
/// equal parallel edges therefore never collapse during loading.
pub(super) fn load_track(session: &LocalSession, track: TrackRef) -> anyhow::Result<LoadedTrack> {
    let selected = session
        .tracks
        .iter()
        .find(|t| t.id == track)
        .ok_or_else(|| anyhow::anyhow!("unknown transaction track {}", track.0))?;
    let generators: Vec<_> = match selected.kind {
        TrackKind::Generator { .. } => vec![track],
        TrackKind::Stream { .. } => session
            .tracks
            .iter()
            .filter_map(|t| match t.kind {
                TrackKind::Generator { stream } if stream == track => Some(t.id),
                _ => None,
            })
            .collect(),
    };
    let mut records: HashMap<_, Vec<Transaction>> =
        generators.iter().map(|&id| (id, vec![])).collect();
    let query = match selected.kind {
        TrackKind::Generator { .. } => vtr::TxQuery {
            generator: Some(vtr::NodeId(track.0)),
            ..Default::default()
        },
        TrackKind::Stream { .. } => vtr::TxQuery {
            stream: Some(vtr::NodeId(track.0)),
            ..Default::default()
        },
    };
    let mut owners = HashMap::new();
    session.reader.visit_transactions(&query, |tx| {
        let generator = TrackRef(tx.generator.0);
        owners.insert(TransactionRef(tx.id), generator);
        records
            .get_mut(&generator)
            .expect("selected generator in catalog")
            .push(transaction(&session.reader, tx));
        true
    })?;
    // Gather incident references first. Resolve external owners together;
    // asking the reader once per relation repeatedly scans the same blocks.
    let mut external = HashSet::new();
    for tx in records.values().flatten() {
        if let Some(parent) = tx.parent
            && !owners.contains_key(&parent)
        {
            external.insert(parent.0);
        }
    }
    let mut incident = Vec::new();
    let mut ordinal = 0u64;
    session.reader.visit_relations(|edge| {
        let id = ordinal;
        ordinal += 1;
        if owners.contains_key(&TransactionRef(edge.from))
            || owners.contains_key(&TransactionRef(edge.to))
        {
            for endpoint in [edge.from, edge.to] {
                if !owners.contains_key(&TransactionRef(endpoint)) {
                    external.insert(endpoint);
                }
            }
            incident.push((id, relation(&session.reader, edge.clone())));
        }
        true
    })?;
    let external: Vec<_> = external.into_iter().collect();
    for (id, generator) in external
        .iter()
        .zip(session.reader.transaction_generators(&external)?)
    {
        let generator =
            generator.ok_or_else(|| anyhow::anyhow!("missing referenced transaction {}", id))?;
        owners.insert(TransactionRef(*id), TrackRef(generator.0));
    }
    let mut edges: HashMap<_, Vec<LoadedRelation>> =
        generators.iter().map(|&id| (id, vec![])).collect();
    for (id, relation) in incident {
        let from_generator = owners[&relation.from];
        let to_generator = owners[&relation.to];
        let loaded = LoadedRelation {
            id,
            from_generator,
            to_generator,
            relation,
        };
        if let Some(out) = edges.get_mut(&from_generator) {
            out.push(loaded.clone());
        }
        if to_generator != from_generator
            && let Some(out) = edges.get_mut(&to_generator)
        {
            out.push(loaded);
        }
    }
    let mut loaded = Vec::with_capacity(generators.len());
    for generator in generators {
        let transactions = records.remove(&generator).expect("selected generator");
        let mut parents = HashMap::new();
        for tx in &transactions {
            if let Some(parent) = tx.parent {
                parents.insert(
                    tx.id,
                    TransactionLocation {
                        transaction: parent,
                        generator: owners[&parent],
                    },
                );
            }
        }
        loaded.push(Arc::new(LoadedGenerator::new(
            generator,
            transactions,
            parents,
            edges.remove(&generator).expect("selected generator"),
        )?));
    }
    Ok(LoadedTrack {
        track,
        generators: loaded,
    })
}

fn value(reader: &Reader, v: &Value) -> AttributeValue {
    match v {
        Value::Null => AttributeValue::Null,
        Value::Bool(x) => AttributeValue::Bool(*x),
        Value::I64(x) => AttributeValue::I64(*x),
        Value::U64(x) => AttributeValue::U64(*x),
        Value::F64(x) => AttributeValue::F64(*x),
        Value::Str(x) => AttributeValue::Text(reader.str(*x).into()),
        Value::Text(x) => AttributeValue::Text(x.clone()),
        Value::Bytes(x) => AttributeValue::Bytes(x.clone()),
        Value::Bits { width, data } => AttributeValue::Logic {
            width: *width,
            states: 2,
            data: data.clone(),
        },
        Value::Logic { width, data } => AttributeValue::Logic {
            width: *width,
            states: 4,
            data: data.clone(),
        },
        Value::Logic9 { width, data } => AttributeValue::Logic {
            width: *width,
            states: 9,
            data: data.clone(),
        },
        Value::Time(x) => AttributeValue::Time(*x),
        Value::Enum { value, name } => AttributeValue::Enum {
            value: *value,
            name: reader.str(*name).into(),
        },
        Value::Pointer(x) => AttributeValue::Pointer(*x),
        Value::Fixed { raw, scale } => AttributeValue::Fixed {
            raw: *raw,
            scale: *scale,
        },
        Value::UFixed { raw, scale } => AttributeValue::UFixed {
            raw: *raw,
            scale: *scale,
        },
        Value::List(xs) => AttributeValue::List(xs.iter().map(|v| value(reader, v)).collect()),
        Value::Map(xs) => AttributeValue::Map(attributes(reader, xs)),
    }
}

fn attributes(reader: &Reader, attrs: &[(vtr::StrId, Value)]) -> Attributes {
    attrs
        .iter()
        .map(|(key, v)| (reader.str(*key).into(), value(reader, v)))
        .collect()
}

pub(super) fn tracks(reader: &Reader) -> Vec<Track> {
    let h = reader.hierarchy();
    h.ids()
        .filter_map(|id| {
            let node = h.node(id);
            let kind = match node.data {
                NodeData::Stream { kind } => TrackKind::Stream {
                    kind: reader.str(kind).into(),
                },
                NodeData::Generator => TrackKind::Generator {
                    stream: TrackRef(node.parent?.0),
                },
                _ => return None,
            };
            let mut path = vec![reader.str(node.name).into()];
            let mut parent = node.parent;
            while let Some(id) = parent {
                let ancestor = h.node(id);
                path.push(reader.str(ancestor.name).into());
                parent = ancestor.parent;
            }
            path.reverse();
            Some(Track {
                id: TrackRef(id.0),
                path,
                kind,
                attributes: attributes(reader, &node.attrs),
            })
        })
        .collect()
}

fn transaction(reader: &Reader, tx: &vtr::Transaction) -> Transaction {
    Transaction {
        id: TransactionRef(tx.id),
        generator: TrackRef(tx.generator.0),
        begin: tx.begin,
        end: tx.end,
        status: tx.status,
        kind: tx.kind,
        parent: tx.parent.map(TransactionRef),
        attributes: tx
            .attrs
            .iter()
            .map(|a| TransactionAttribute {
                key: reader.str(a.key).into(),
                phase: a.phase,
                value: value(reader, &a.value),
            })
            .collect(),
        events: tx
            .events
            .iter()
            .map(|e| TransactionEvent {
                time: e.time,
                name: reader.str(e.name).into(),
                attributes: attributes(reader, &e.attrs),
            })
            .collect(),
        stages: tx
            .stages
            .iter()
            .map(|s| TransactionStage {
                name: reader.str(s.name).into(),
                lane: reader.str(s.lane).into(),
                begin: s.begin,
                end: s.end,
                attributes: attributes(reader, &s.attrs),
            })
            .collect(),
    }
}

fn relation(reader: &Reader, r: vtr::Relation) -> Relation {
    Relation {
        kind: reader.str(r.kind).into(),
        from: TransactionRef(r.from),
        to: TransactionRef(r.to),
        attributes: attributes(reader, &r.attrs),
    }
}
