//! Resolve VTR's interned names at the adapter boundary.
use super::transactions::*;
use super::vtr_source::LocalSession;
use vtr::{NodeData, Reader, Value};

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

impl TransactionQueries for LocalSession {
    fn tracks(&self) -> &[Track] {
        &self.tracks
    }
    fn visit_transactions(
        &self,
        query: &TransactionQuery,
        visitor: &mut dyn FnMut(&Transaction) -> bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            query.window.is_none_or(|(start, end)| start <= end),
            "transaction window is reversed"
        );
        if let Some(id) = query.generator {
            anyhow::ensure!(
                self.tracks
                    .iter()
                    .any(|t| t.id == id && matches!(t.kind, TrackKind::Generator { .. })),
                "unknown generator {}",
                id.0
            );
        }
        if let Some(id) = query.stream {
            anyhow::ensure!(
                self.tracks
                    .iter()
                    .any(|t| t.id == id && matches!(t.kind, TrackKind::Stream { .. })),
                "unknown stream {}",
                id.0
            );
        }
        self.reader.visit_transactions(
            &vtr::TxQuery {
                generator: query.generator.map(|id| vtr::NodeId(id.0)),
                stream: query.stream.map(|id| vtr::NodeId(id.0)),
                window: query.window,
            },
            |tx| visitor(&transaction(&self.reader, tx)),
        )?;
        Ok(())
    }
    fn transaction(&self, id: TransactionRef) -> anyhow::Result<Option<Transaction>> {
        Ok(self
            .reader
            .transaction(id.0)?
            .as_ref()
            .map(|tx| transaction(&self.reader, tx)))
    }
}

impl RelationQueries for LocalSession {
    fn relations_from(&self, id: TransactionRef) -> anyhow::Result<Vec<Relation>> {
        Ok(self
            .reader
            .relations_from(id.0)?
            .into_iter()
            .map(|r| relation(&self.reader, r))
            .collect())
    }
    fn relations_to(&self, id: TransactionRef) -> anyhow::Result<Vec<Relation>> {
        Ok(self
            .reader
            .relations_to(id.0)?
            .into_iter()
            .map(|r| relation(&self.reader, r))
            .collect())
    }
}
