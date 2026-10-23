use super::{Transaction, validate_transaction};
use crate::{
    Error, Result,
    records::{GeneratorId, Generators, RecordKind},
    schema::{TypeId, Types},
    wire::Cursor,
};

pub(super) fn eligible(
    types: &Types,
    generators: &Generators,
    rows: &[Transaction],
) -> Option<TypeId> {
    let datatype = generators.get(rows.first()?.generator).ok()?.payload_type;
    let words = types.get(datatype).ok()?.unsigned_words()?;
    if words > 256 {
        return None;
    }
    rows.iter()
        .all(|tx| {
            generators.get(tx.generator).is_ok_and(|generator| {
                generator.kind == RecordKind::Transaction && generator.payload_type == datatype
            }) && tx.attributes.is_empty()
                && tx.events.is_empty()
                && tx.stages.is_empty()
                && tx.child_blocks.is_empty()
        })
        .then_some(datatype)
}

pub(super) fn encode(
    types: &Types,
    datatype: TypeId,
    rows: &[&Transaction],
    ids: &[GeneratorId],
) -> Result<Vec<u8>> {
    let words = types
        .get(datatype)?
        .unsigned_words()
        .ok_or_else(|| Error::corrupt("invalid scalar page datatype"))?;
    let mut out = vec![2];
    out.extend_from_slice(&datatype.to_le_bytes());
    out.extend_from_slice(&super::checked_u32(rows.len())?.to_le_bytes());
    let mut values = Vec::with_capacity(rows.len());
    for column in 0..6 + words {
        let value = |tx: &Transaction| -> u64 {
            match column {
                0 => tx.id,
                1 => ids.binary_search(&tx.generator).unwrap() as u64,
                2 => tx.begin,
                3 => tx.end - tx.begin,
                4 => tx.parent,
                5 => u64::from((tx.span_kind << 4) | tx.status),
                _ => u64::from_le_bytes(
                    tx.payload[(column - 6) * 8..(column - 5) * 8]
                        .try_into()
                        .unwrap(),
                ),
            }
        };
        values.clear();
        values.extend(rows.iter().map(|row| value(row)));
        let used = values.iter().fold(0, |used, value| used | value);
        let width = (64 - used.leading_zeros()).div_ceil(8) as usize;
        out.push(width as u8);
        let start = out.len();
        out.resize(start + width * rows.len(), 0);
        for (byte, plane) in out[start..].chunks_mut(rows.len()).enumerate() {
            for (destination, value) in plane.iter_mut().zip(&values) {
                *destination = (value >> (byte * 8)) as u8;
            }
        }
    }
    Ok(out)
}

pub(super) fn decode(
    raw: &[u8],
    types: &Types,
    generators: &Generators,
    ids: &[GeneratorId],
    expected: u64,
    limit: usize,
    mut visit: impl FnMut(Transaction) -> bool,
) -> Result<bool> {
    let mut cursor = Cursor::new(raw);
    if cursor.byte()? != 2 {
        return Err(Error::Corrupt("invalid scalar page discriminator"));
    }
    let datatype = cursor.u32()?;
    let count = cursor.u32()? as usize;
    let words = types
        .get(datatype)?
        .unsigned_words()
        .ok_or_else(|| Error::corrupt("invalid scalar page datatype"))?;
    if count == 0
        || count as u64 != expected
        || words > 256
        || words * 8 > limit
        || count > raw.len()
    {
        return Err(Error::Corrupt("invalid scalar page schema/count"));
    }
    let mut planes = Vec::new();
    let mut widths = Vec::new();
    for _ in 0..6 + words {
        let width = cursor.byte()? as usize;
        if width > 8 {
            return Err(Error::Corrupt("invalid scalar column width"));
        }
        let data = cursor.take(
            width
                .checked_mul(count)
                .ok_or_else(|| Error::corrupt("scalar plane size overflow"))?,
        )?;
        if width != 0 && data[data.len() - count..].iter().all(|byte| *byte == 0) {
            return Err(Error::Corrupt("noncanonical scalar column width"));
        }
        planes.push(data);
        widths.push(width);
    }
    cursor.done()?;
    let declarations = ids
        .iter()
        .map(|&id| generators.get(id))
        .collect::<Result<Vec<_>>>()?;
    for row in 0..count {
        let value = |column: usize| -> u64 {
            (0..widths[column]).fold(0, |value, byte| {
                value | u64::from(planes[column][byte * count + row]) << (byte * 8)
            })
        };
        let declaration = *declarations
            .get(
                usize::try_from(value(1))
                    .map_err(|_| Error::Corrupt("invalid scalar generator index"))?,
            )
            .ok_or_else(|| Error::corrupt("invalid scalar generator index"))?;
        let generator = declaration.id;
        if declaration.kind != RecordKind::Transaction || declaration.payload_type != datatype {
            return Err(Error::Corrupt("scalar generator schema mismatch"));
        }
        let classification = value(5);
        if classification > 255 {
            return Err(Error::Corrupt("scalar classification overflows"));
        }
        let begin = value(2);
        let end = begin
            .checked_add(value(3))
            .ok_or_else(|| Error::corrupt("scalar interval overflows"))?;
        let mut tx = Transaction {
            id: value(0),
            parent: value(4),
            generator,
            begin,
            end,
            status: classification as u8 & 15,
            span_kind: classification as u8 >> 4,
            payload: Vec::with_capacity(words * 8),
            ..Transaction::default()
        };
        for column in 6..6 + words {
            tx.payload.extend_from_slice(&value(column).to_le_bytes());
        }
        validate_transaction(&tx, declaration, u64::MAX)?;
        if !visit(tx) {
            return Ok(false);
        }
    }
    Ok(true)
}
