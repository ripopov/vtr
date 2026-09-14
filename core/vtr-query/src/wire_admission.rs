//! Nonallocating schema and allocation preflight for both wire directions.
use crate::wire::{MAX_DECODED_BYTES, MAX_RECORDS};
use crate::{Error, Result};
#[derive(Clone, Copy)]
pub(crate) enum Schema {
    Envelope,
    ReplyEnvelope,
    Welcome,
    Info,
    Failure,
    Delivery,
    WindowPage,
    SummaryPage,
    DeclarationPage,
    SearchPage,
    ResolvePage,
    TextPart,
    ValuesAt,
    SignalTime,
    SampleAt,
    ValuesPage,
    FindChange,
    ChangeSearchPage,
    Change,
    Sample,
    Value,
    Kind,
    BitKind,
    Bits,
    Bin,
    RealSummary,
    Declaration,
    MetadataText,
    TextReference,
    Scope,
    Variable,
    Stream,
    EnumTable,
    Resolution,
    Empty,
    Query,
    Limits,
    Window,
    Summary,
    Children,
    Search,
    Resolve,
    Path,
    Text,
    Cursor,
    Cancel,
    Interval,
    Bound,
    Grid,
}
enum Field {
    U32,
    U64,
    Fixed64,
    Bool,
    Operations,
    Snapshot,
    Bytes,
    Text,
    Message(Schema),
}

fn field(schema: Schema, tag: u32) -> Result<Field> {
    use Field::*;
    use Schema as S;
    let field = match (schema, tag) {
        (S::Envelope | S::ReplyEnvelope, 1) => U32,
        (S::Envelope | S::ReplyEnvelope, 2) => U64,
        (S::Envelope | S::ReplyEnvelope, 3) => Snapshot,
        (S::Envelope, 10 | 12 | 19) => Message(S::Empty),
        (S::Envelope, 14) => Message(S::Query),
        (S::Envelope, 15 | 18) => Message(S::Cursor),
        (S::Envelope, 17) => Message(S::Cancel),
        (S::Query, 1) => Message(S::Limits),
        (S::Query, 10) => Message(S::Window),
        (S::Query, 11) => Message(S::Summary),
        (S::Query, 12) => Message(S::Children),
        (S::Query, 13) => Message(S::Search),
        (S::Query, 14) => Message(S::Resolve),
        (S::Query, 15) => Message(S::Text),
        (S::Query, 17) => Message(S::ValuesAt),
        (S::ValuesAt, 1) => Message(S::SignalTime),
        (S::SignalTime, 1) => U32,
        (S::SignalTime, 2) => U64,
        (S::Delivery, 17) => Message(S::ValuesPage),
        (S::ValuesPage, 1) => U32,
        (S::ValuesPage, 2) => Bool,
        (S::ValuesPage, 3) => Message(S::SampleAt),
        (S::SampleAt, 1) => Message(S::SignalTime),
        (S::SampleAt, 2) => Message(S::Sample),
        (S::Query, 16) => Message(S::FindChange),
        (S::FindChange, 1 | 3) => U32,
        (S::FindChange, 2) => U64,
        (S::Limits, 1 | 3) => U64,
        (S::Limits, 2) => U32,
        (S::Window | S::Summary, 1) => U32,
        (S::Window, 2) => Message(S::Interval),
        (S::Summary, 2) => Message(S::Grid),
        (S::Children | S::Search, 1) => U32,
        (S::Search, 2) => Text,
        (S::Resolve, 1) => Message(S::Path),
        (S::Path, 1) => Text,
        (S::Path, 2 | 3) => U32,
        (S::Text, 1) => U32,
        (S::Text, 2 | 3) => U64,
        (S::Cursor, 1) => Snapshot,
        (S::Cursor, 2 | 3) | (S::Cancel, 1) => U64,
        (S::Interval, 1) => U64,
        (S::Interval, 2) => Message(S::Bound),
        (S::Bound, 1) => U64,
        (S::Bound, 2) => Message(S::Empty),
        (S::Grid, 1) => U64,
        (S::Grid, 2 | 3) => U32,
        (S::ReplyEnvelope, 11) => Message(S::Welcome),
        (S::ReplyEnvelope, 13) => Message(S::Info),
        (S::ReplyEnvelope, 16) => Message(S::Delivery),
        (S::ReplyEnvelope, 20) => Message(S::Failure),
        (S::ReplyEnvelope, 21) => Message(S::Empty),
        (S::Welcome, 1..=5) => U32,
        (S::Info, 1) => Snapshot,
        (S::Info, 2 | 4) => U32,
        (S::Info, 3) => Message(S::Interval),
        (S::Info, 5) => U64,
        (S::Info, 6) => Operations,
        (S::Failure, 1) => U32,
        (S::Failure, 2) => Text,
        (S::Delivery, 1 | 2) => Message(S::Cursor),
        (S::Delivery, 10) => Message(S::WindowPage),
        (S::Delivery, 11) => Message(S::SummaryPage),
        (S::Delivery, 12) => Message(S::DeclarationPage),
        (S::Delivery, 13) => Message(S::SearchPage),
        (S::Delivery, 14) => Message(S::ResolvePage),
        (S::Delivery, 15) => Message(S::TextPart),
        (S::Delivery, 16) => Message(S::ChangeSearchPage),
        (S::ChangeSearchPage, 1 | 3) => Message(S::Empty),
        (S::ChangeSearchPage, 2) => U64,
        (S::WindowPage, 1) => Message(S::Interval),
        (S::WindowPage, 2) => Message(S::Sample),
        (S::WindowPage, 3) => Message(S::Change),
        (S::WindowPage, 4) => Bool,
        (S::SummaryPage, 1) => Message(S::Grid),
        (S::SummaryPage, 2) => U32,
        (S::SummaryPage, 3) => Bool,
        (S::SummaryPage, 4) => Message(S::Bin),
        (S::DeclarationPage, 1) => U32,
        (S::DeclarationPage, 2) => U64,
        (S::DeclarationPage, 3) => Bool,
        (S::DeclarationPage, 4) => Message(S::Declaration),
        (S::SearchPage, 1) => U32,
        (S::SearchPage, 2) => Bool,
        (S::SearchPage, 3) => Message(S::Declaration),
        (S::ResolvePage, 1) => U64,
        (S::ResolvePage, 2) => Bool,
        (S::ResolvePage, 3) => Message(S::Resolution),
        (S::Resolution, 1) => U32,
        (S::Resolution, 2 | 3) => Message(S::Empty),
        (S::TextPart, 1) => U32,
        (S::TextPart, 2 | 3) => U64,
        (S::TextPart, 4) => Bytes,
        (S::Change, 1) => U64,
        (S::Change, 2) => Message(S::Value),
        (S::Sample, 1) => Message(S::Value),
        (S::Sample, 2) => Message(S::Kind),
        (S::Sample, 3) => Message(S::Empty),
        (S::Value, 1) => Message(S::Bits),
        (S::Value, 2) => Fixed64,
        (S::Value, 3) => Bytes,
        (S::Kind, 1) => Message(S::BitKind),
        (S::Kind, 2 | 3) => Message(S::Empty),
        (S::BitKind | S::Bits, 1 | 2) => U32,
        (S::Bits, 3) => Bytes,
        (S::Bin, 1) => Message(S::Interval),
        (S::Bin, 2 | 3) => Message(S::Sample),
        (S::Bin, 4) => U64,
        (S::Bin, 5 | 6) => Message(S::Change),
        (S::Bin, 7) => Message(S::RealSummary),
        (S::RealSummary, 1 | 2) => Fixed64,
        (S::RealSummary, 3..=5) => U64,
        (S::Declaration, 1 | 2) => U32,
        (S::Declaration, 3) => Message(S::MetadataText),
        (S::Declaration, 4 | 5) => U64,
        (S::Declaration, 10) => Message(S::Scope),
        (S::Declaration, 11) => Message(S::Variable),
        (S::Declaration, 12) => Message(S::Stream),
        (S::Declaration, 13) => Message(S::Empty),
        (S::Declaration, 14) => Message(S::EnumTable),
        (S::MetadataText, 1) => Text,
        (S::MetadataText, 2) => Message(S::TextReference),
        (S::TextReference, 1) => U32,
        (S::TextReference, 2) => U64,
        (S::Scope, 1) => U32,
        (S::Scope, 2) | (S::Stream, 1) => Message(S::MetadataText),
        (S::Variable, 1..=3) => U32,
        (S::Variable, 4) => Message(S::Kind),
        (S::Variable, 5) => Bool,
        (S::EnumTable, 1) => U64,
        _ => return Err(Error::Frame("unknown or wrong-direction field")),
    };
    Ok(field)
}
pub(crate) fn varint(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for index in 0..10 {
        let (&byte, rest) = input
            .split_first()
            .ok_or(Error::Frame("truncated varint"))?;
        *input = rest;
        if index == 9 && byte > 1 {
            return Err(Error::Frame("overflowing varint"));
        }
        value |= u64::from(byte & 127) << (7 * index);
        if byte < 128 {
            return Ok(value);
        }
    }
    Err(Error::Frame("overflowing varint"))
}
struct Admission {
    bytes: usize,
    fields: usize,
    messages: usize,
}
impl Admission {
    fn add(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self.bytes.checked_add(bytes).ok_or(Error::ResourceLimit)?;
        if self.bytes > MAX_DECODED_BYTES {
            return Err(Error::ResourceLimit);
        }
        Ok(())
    }
    fn scan(&mut self, schema: Schema, mut input: &[u8], depth: usize) -> Result<()> {
        if depth > 12 || self.messages >= MAX_RECORDS {
            return Err(Error::ResourceLimit);
        }
        self.messages += 1;
        // Conservative upper bound for each nonrecursive generated wire
        // object and transient conversion storage, including vector spare capacity.
        // Schema conformance tests check generated and destination sizes.
        self.add(1024)?;
        let mut seen = 0u32;
        let mut oneof = false;
        while !input.is_empty() {
            self.fields += 1;
            if self.fields > 16384 {
                return Err(Error::ResourceLimit);
            }
            self.add(64)?;
            let key = varint(&mut input)?;
            let tag = u32::try_from(key >> 3).map_err(|_| Error::Frame("invalid field tag"))?;
            let kind = field(schema, tag)?;
            let repeated = matches!(
                (schema, tag),
                (Schema::Resolve | Schema::Path | Schema::ValuesAt, 1)
                    | (Schema::Info, 6)
                    | (
                        Schema::WindowPage
                            | Schema::SearchPage
                            | Schema::ResolvePage
                            | Schema::ValuesPage,
                        3
                    )
                    | (Schema::SummaryPage | Schema::DeclarationPage, 4)
            );
            if !repeated {
                let bit = 1u32
                    .checked_shl(tag)
                    .ok_or(Error::Frame("invalid field tag"))?;
                if seen & bit != 0 {
                    return Err(Error::Frame("duplicate singular field"));
                }
                seen |= bit;
            }
            let is_oneof = matches!(
                schema,
                Schema::Bound
                    | Schema::Sample
                    | Schema::Value
                    | Schema::Kind
                    | Schema::MetadataText
                    | Schema::Resolution
                    | Schema::ChangeSearchPage
            ) || matches!(
                schema,
                Schema::Envelope
                    | Schema::ReplyEnvelope
                    | Schema::Query
                    | Schema::Delivery
                    | Schema::Declaration
            ) && tag >= 10;
            if is_oneof {
                if oneof {
                    return Err(Error::Frame("multiple oneof alternatives"));
                }
                oneof = true;
            }
            match kind {
                Field::Fixed64 => {
                    if key & 7 != 1 || input.len() < 8 {
                        return Err(Error::Frame("invalid fixed64 field"));
                    }
                    input = &input[8..];
                }
                Field::Operations => {
                    if key & 7 == 0 {
                        let value = varint(&mut input)?;
                        if !(1..=8).contains(&value) {
                            return Err(Error::Frame("invalid operation"));
                        }
                    } else if key & 7 == 2 {
                        let len = usize::try_from(varint(&mut input)?)
                            .map_err(|_| Error::ResourceLimit)?;
                        let mut packed = input
                            .get(..len)
                            .ok_or(Error::Frame("truncated operations"))?;
                        input = &input[len..];
                        while !packed.is_empty() {
                            let value = varint(&mut packed)?;
                            if !(1..=8).contains(&value) {
                                return Err(Error::Frame("invalid operation"));
                            }
                            self.add(8)?;
                        }
                    } else {
                        return Err(Error::Frame("invalid operations wire type"));
                    }
                }
                Field::U32 | Field::U64 | Field::Bool => {
                    if key & 7 != 0 {
                        return Err(Error::Frame("invalid scalar wire type"));
                    }
                    let value = varint(&mut input)?;
                    if matches!(kind, Field::Bool) && value > 1 {
                        return Err(Error::Frame("invalid boolean"));
                    }
                    if matches!(kind, Field::U32) && value > u64::from(u32::MAX) {
                        return Err(Error::Frame("uint32 overflow"));
                    }
                }
                Field::Snapshot | Field::Bytes | Field::Text | Field::Message(_) => {
                    if key & 7 != 2 {
                        return Err(Error::Frame("invalid message wire type"));
                    }
                    let len =
                        usize::try_from(varint(&mut input)?).map_err(|_| Error::ResourceLimit)?;
                    let value = input
                        .get(..len)
                        .ok_or(Error::Frame("truncated length-delimited field"))?;
                    input = &input[len..];
                    match kind {
                        Field::Message(child) => self.scan(child, value, depth + 1)?,
                        Field::Snapshot => {
                            if len != 16 {
                                return Err(Error::Frame("snapshot must have 16 bytes"));
                            }
                            self.add(len * 2)?;
                        }
                        Field::Bytes => {
                            self.add(len.checked_mul(2).ok_or(Error::ResourceLimit)?)?
                        }
                        Field::Text => {
                            std::str::from_utf8(value)
                                .map_err(|_| Error::Frame("invalid UTF-8"))?;
                            self.add(len * 2)?;
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
        Ok(())
    }
}

/// Validate without allocating, also used before transmitting encoded replies.
pub(crate) fn decoded_bytes(schema: Schema, input: &[u8]) -> Result<usize> {
    let mut admission = Admission {
        bytes: 0,
        fields: 0,
        messages: 0,
    };
    admission.scan(schema, input, 0)?;
    Ok(admission.bytes)
}
pub(crate) fn admit(
    schema: Schema,
    input: &[u8],
    budget: &crate::Budget,
) -> Result<crate::Reservation> {
    budget.reserve(decoded_bytes(schema, input)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_status_preflight_rejects_conflicting_duplicate_and_unknown_fields() {
        let budget = crate::Budget::new(MAX_DECODED_BYTES);
        for bytes in [
            &[10, 0, 16, 0][..], // pending and found(0)
            &[16, 0, 16, 1][..], // repeated found
            &[26, 0, 10, 0][..], // exhausted and pending
            &[34, 0][..],        // unknown status
        ] {
            assert!(admit(Schema::ChangeSearchPage, bytes, &budget).is_err());
            assert_eq!(budget.used(), 0);
        }
    }
}
