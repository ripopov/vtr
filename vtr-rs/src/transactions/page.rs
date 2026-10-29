use std::collections::HashMap;

use super::{
    EmbeddedEvent, EmbeddedStage, Relation, Transaction, TransactionBlockView, TransactionBounds,
    validate_transaction,
};
use crate::{
    Error, Result,
    arguments::{ArgumentColumns, ArgumentReader},
    attributes::{AttributeColumns, AttributeReader},
    records::Generator,
    schema::Types,
    varint::{len_u64, put_i64, put_u64, unzigzag},
    wire::{self, Cursor},
};

pub(super) struct Encoder<'a> {
    columns: [Vec<u8>; 33],
    attributes: AttributeColumns,
    arguments: ArgumentColumns<'a>,
    previous_id: u64,
    previous_event: u64,
    previous_stage: u64,
    previous_from: u64,
    previous_begin: u64,
    strings: u64,
}

pub(super) struct Checkpoint {
    columns: [usize; 33],
    attributes: [usize; 7],
    arguments: ([usize; 5], usize),
    previous: [u64; 5],
}

impl<'a> Encoder<'a> {
    pub(super) fn new(strings: u64, shared_text: Option<&'a HashMap<String, u32>>) -> Self {
        Self {
            columns: std::array::from_fn(|_| Vec::new()),
            attributes: AttributeColumns::default(),
            arguments: ArgumentColumns::new(strings, shared_text),
            previous_id: 0,
            previous_event: 0,
            previous_stage: 0,
            previous_from: 0,
            previous_begin: 0,
            strings,
        }
    }

    pub(super) fn transaction(
        &mut self,
        types: &Types,
        generator: &Generator,
        local: u32,
        tx: &Transaction,
    ) -> Result<()> {
        put_i64(
            &mut self.columns[0],
            tx.id.wrapping_sub(self.previous_id) as i64,
        );
        put_u64(&mut self.columns[1], u64::from(local));
        put_i64(
            &mut self.columns[2],
            tx.begin.wrapping_sub(self.previous_begin) as i64,
        );
        put_u64(&mut self.columns[3], tx.end - tx.begin);
        self.columns[4].push((tx.span_kind << 4) | tx.status);
        put_u64(
            &mut self.columns[5],
            if tx.parent == 0 { 0 } else { tx.id - tx.parent },
        );
        self.attributes.append(&tx.attributes, self.strings)?;
        put_u64(&mut self.columns[13], tx.events.len() as u64);
        for event in &tx.events {
            put_i64(
                &mut self.columns[14],
                event.time.wrapping_sub(tx.begin) as i64,
            );
            put_u64(&mut self.columns[15], u64::from(event.name));
            put_i64(
                &mut self.columns[31],
                event.id.wrapping_sub(self.previous_event) as i64,
            );
            self.previous_event = event.id;
            self.attributes.append(&event.attributes, self.strings)?;
        }
        put_u64(&mut self.columns[16], tx.stages.len() as u64);
        for stage in &tx.stages {
            put_u64(&mut self.columns[17], u64::from(stage.name));
            put_u64(&mut self.columns[18], u64::from(stage.lane));
            put_i64(
                &mut self.columns[19],
                stage.begin.wrapping_sub(tx.begin) as i64,
            );
            match stage.end {
                None => put_u64(&mut self.columns[20], 0),
                Some(u64::MAX) if stage.begin == 0 => {
                    self.columns[20].extend_from_slice(&super::time::FULL_DOMAIN_END);
                }
                Some(end) => put_u64(&mut self.columns[20], end - stage.begin + 1),
            }
            put_i64(
                &mut self.columns[32],
                stage.id.wrapping_sub(self.previous_stage) as i64,
            );
            self.previous_stage = stage.id;
            self.attributes.append(&stage.attributes, self.strings)?;
        }
        self.arguments
            .append(types, generator.payload_type, &tx.payload)?;
        put_u64(&mut self.columns[29], tx.child_blocks.len() as u64);
        let mut previous = 0;
        for block in &tx.child_blocks {
            put_u64(&mut self.columns[30], block - previous);
            previous = *block;
        }
        self.previous_id = tx.id;
        self.previous_begin = tx.begin;
        Ok(())
    }

    pub(super) fn relation(&mut self, relation: &Relation) -> Result<()> {
        put_u64(&mut self.columns[21], u64::from(relation.kind));
        put_i64(
            &mut self.columns[22],
            relation.source.wrapping_sub(self.previous_from) as i64,
        );
        put_i64(
            &mut self.columns[23],
            relation.destination.wrapping_sub(relation.source) as i64,
        );
        self.previous_from = relation.source;
        self.attributes.append(&relation.attributes, self.strings)
    }

    pub(super) fn size(&self) -> usize {
        let mut lengths = self.columns.each_ref().map(Vec::len);
        lengths[6..13].copy_from_slice(&self.attributes.columns.each_ref().map(Vec::len));
        lengths[24..29].copy_from_slice(&self.arguments.encoded_lengths());
        1 + lengths
            .into_iter()
            .map(|length| len_u64(length as u64) + length)
            .sum::<usize>()
    }

    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            columns: self.columns.each_ref().map(Vec::len),
            attributes: self.attributes.columns.each_ref().map(Vec::len),
            arguments: self.arguments.checkpoint(),
            previous: [
                self.previous_id,
                self.previous_event,
                self.previous_stage,
                self.previous_from,
                self.previous_begin,
            ],
        }
    }

    pub(super) fn restore(&mut self, checkpoint: Checkpoint) {
        for (column, length) in self.columns.iter_mut().zip(checkpoint.columns) {
            column.truncate(length);
        }
        for (column, length) in self
            .attributes
            .columns
            .iter_mut()
            .zip(checkpoint.attributes)
        {
            column.truncate(length);
        }
        self.arguments.restore(checkpoint.arguments);
        [
            self.previous_id,
            self.previous_event,
            self.previous_stage,
            self.previous_from,
            self.previous_begin,
        ] = checkpoint.previous;
    }

    pub(super) fn finish(mut self) -> Result<Vec<u8>> {
        for (i, column) in self.attributes.columns.into_iter().enumerate() {
            self.columns[i + 6] = column;
        }
        for (i, column) in self.arguments.finish().into_iter().enumerate() {
            self.columns[i + 24] = column;
        }
        wire::pack_columns(&self.columns)
    }
}

pub(super) fn decode(
    raw: &[u8],
    expected: TransactionBounds,
    view: &TransactionBlockView<'_>,
    visit_tx: &mut impl FnMut(Transaction) -> bool,
    visit_relation: &mut impl FnMut(Relation) -> bool,
) -> Result<bool> {
    let columns = wire::unpack_columns::<33>(raw)?;
    let mut metadata = columns.map(Cursor::new);
    let mut attributes = AttributeReader::new(
        std::array::from_fn(|i| columns[i + 6]),
        view.strings,
        view.max_payload_bytes,
    );
    let mut arguments = ArgumentReader::new(
        std::array::from_fn(|i| columns[i + 24]),
        view.strings,
        view.max_payload_bytes,
        view.shared_text,
    )?;
    for i in (6..13).chain(24..29) {
        metadata[i] = Cursor::new(&[]);
    }
    if expected.transactions > metadata[0].remaining() as u64
        || expected.relations > metadata[21].remaining() as u64
    {
        return Err(Error::Corrupt("invalid transaction page count"));
    }
    let mut previous_id = 0u64;
    let mut previous_event = 0u64;
    let mut previous_stage = 0u64;
    let mut previous_from = 0u64;
    let mut previous_begin = 0u64;
    let mut actual = TransactionBounds::default();
    for index in 0..expected.transactions {
        let id = previous_id.wrapping_add(unzigzag(metadata[0].var()?) as u64);
        let local = metadata[1].var()?;
        let generator = *view
            .ids
            .get(
                usize::try_from(local)
                    .map_err(|_| Error::Corrupt("invalid local generator index"))?,
            )
            .ok_or_else(|| Error::corrupt("invalid local generator index"))?;
        let begin = previous_begin.wrapping_add(unzigzag(metadata[2].var()?) as u64);
        let end = begin
            .checked_add(metadata[3].var()?)
            .ok_or_else(|| Error::corrupt("transaction end overflow"))?;
        let classification = metadata[4].byte()?;
        let distance = metadata[5].var()?;
        if distance != 0 && distance >= id {
            return Err(Error::Corrupt("invalid transaction parent distance"));
        }
        let mut tx = Transaction {
            id,
            parent: if distance == 0 { 0 } else { id - distance },
            generator,
            begin,
            end,
            status: classification & 15,
            span_kind: classification >> 4,
            attributes: attributes.read()?,
            ..Transaction::default()
        };
        let count = metadata[13].var()?;
        if count > metadata[15].remaining() as u64 || count > metadata[31].remaining() as u64 {
            return Err(Error::Corrupt("invalid embedded event count"));
        }
        for _ in 0..count {
            let time = begin.wrapping_add(unzigzag(metadata[14].var()?) as u64);
            let name = metadata[15].id()?;
            let event_id = previous_event.wrapping_add(unzigzag(metadata[31].var()?) as u64);
            previous_event = event_id;
            tx.events.push(EmbeddedEvent {
                id: event_id,
                time,
                name,
                attributes: attributes.read()?,
            });
        }
        let count = metadata[16].var()?;
        if count > metadata[17].remaining() as u64 || count > metadata[32].remaining() as u64 {
            return Err(Error::Corrupt("invalid embedded stage count"));
        }
        for _ in 0..count {
            let name = metadata[17].id()?;
            let lane = metadata[18].id()?;
            let start = begin.wrapping_add(unzigzag(metadata[19].var()?) as u64);
            let finish = super::time::stage_end(&mut metadata[20], start)?;
            let stage_id = previous_stage.wrapping_add(unzigzag(metadata[32].var()?) as u64);
            previous_stage = stage_id;
            tx.stages.push(EmbeddedStage {
                id: stage_id,
                name,
                lane,
                begin: start,
                end: finish,
                attributes: attributes.read()?,
            });
        }
        tx.payload = arguments.read(view.types, view.generators.get(generator)?.payload_type)?;
        let count = metadata[29].var()?;
        if count > metadata[30].remaining() as u64 {
            return Err(Error::Corrupt("invalid located block count"));
        }
        let mut block = 0u64;
        for _ in 0..count {
            let delta = metadata[30].var()?;
            if delta == 0 {
                return Err(Error::Corrupt("zero located block delta"));
            }
            block = block
                .checked_add(delta)
                .ok_or_else(|| Error::corrupt("located block delta overflow"))?;
            tx.child_blocks.push(block);
        }
        validate_transaction(&tx, view.generators.get(generator)?, view.strings)?;
        if index != 0 && previous_id >= id {
            return Err(Error::Corrupt("unordered transaction IDs"));
        }
        previous_id = id;
        previous_begin = begin;
        actual.add_transaction(&tx);
        if !visit_tx(tx) {
            return Ok(false);
        }
    }
    for _ in 0..expected.relations {
        let kind = metadata[21].id()?;
        if u64::from(kind) >= view.strings {
            return Err(Error::Corrupt("unknown relation kind"));
        }
        let source = previous_from.wrapping_add(unzigzag(metadata[22].var()?) as u64);
        let destination = source.wrapping_add(unzigzag(metadata[23].var()?) as u64);
        previous_from = source;
        let relation = Relation {
            kind,
            source,
            destination,
            attributes: attributes.read()?,
        };
        actual.add_relation(&relation);
        if !visit_relation(relation) {
            return Ok(false);
        }
    }
    for column in metadata {
        column.done()?;
    }
    attributes.done()?;
    arguments.done()?;
    if actual != expected {
        return Err(Error::Corrupt("decoded transaction page bounds disagree"));
    }
    Ok(true)
}
