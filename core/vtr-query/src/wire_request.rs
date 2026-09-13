//! Strict request decoding with a non-allocating schema pass before Prost.
use crate::wire::{proto as p, MAX_DECODED_BYTES, MAX_RECORDS, MAX_REQUEST_BYTES};
use crate::{
    metadata::Path,
    session::{Continuation, Query, SnapshotId},
    wave::Limits,
};
use crate::{Budget, Error, Grid, Interval, Reservation, Result, TimeBound};
use prost::Message;

pub const VERSION: u32 = 1;
#[derive(Debug)]
pub enum RequestBody {
    Hello,
    Open,
    Query { query: Query, limits: Limits },
    Next(Continuation),
    Cancel(u64),
    Release(Continuation),
    Close,
}
#[derive(Debug)]
pub struct Request {
    pub request_id: u64,
    pub snapshot: Option<SnapshotId>,
    pub body: RequestBody,
    _charge: Reservation,
}

#[derive(Clone, Copy)]
enum Schema {
    Envelope,
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
    Bytes,
    Text,
    Message(Schema),
}
fn field(schema: Schema, tag: u32) -> Result<Field> {
    use Field::*;
    use Schema as S;
    let field = match (schema, tag) {
        (S::Envelope, 1) => U32,
        (S::Envelope, 2) => U64,
        (S::Envelope, 3) => Bytes,
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
        (S::Cursor, 1) => Bytes,
        (S::Cursor, 2 | 3) | (S::Cancel, 1) => U64,
        (S::Interval, 1) => U64,
        (S::Interval, 2) => Message(S::Bound),
        (S::Bound, 1) => U64,
        (S::Bound, 2) => Message(S::Empty),
        (S::Grid, 1) => U64,
        (S::Grid, 2 | 3) => U32,
        _ => return Err(Error::Frame("unknown or wrong-direction request field")),
    };
    Ok(field)
}
fn varint(input: &mut &[u8]) -> Result<u64> {
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
        if depth > 8 || self.messages >= MAX_RECORDS {
            return Err(Error::ResourceLimit);
        }
        self.messages += 1;
        // Conservative upper bound for each nonrecursive generated request
        // object and transient conversion storage. Checked against schema types.
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
            let repeated = matches!((schema, tag), (Schema::Resolve | Schema::Path, 1));
            if !repeated {
                let bit = 1u32
                    .checked_shl(tag)
                    .ok_or(Error::Frame("invalid field tag"))?;
                if seen & bit != 0 {
                    return Err(Error::Frame("duplicate singular field"));
                }
                seen |= bit;
            }
            let is_oneof = matches!(schema, Schema::Bound)
                || matches!(schema, Schema::Envelope | Schema::Query) && tag >= 10;
            if is_oneof {
                if oneof {
                    return Err(Error::Frame("multiple oneof alternatives"));
                }
                oneof = true;
            }
            match kind {
                Field::U32 | Field::U64 => {
                    if key & 7 != 0 {
                        return Err(Error::Frame("invalid scalar wire type"));
                    }
                    let value = varint(&mut input)?;
                    if matches!(kind, Field::U32) && value > u64::from(u32::MAX) {
                        return Err(Error::Frame("uint32 overflow"));
                    }
                }
                Field::Bytes | Field::Text | Field::Message(_) => {
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
                        Field::Bytes => {
                            if len != 16 {
                                return Err(Error::Frame("snapshot must have 16 bytes"));
                            }
                            self.add(len * 2)?;
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
fn missing() -> Error {
    Error::Frame("missing required query field")
}
fn snapshot(bytes: Vec<u8>) -> Result<SnapshotId> {
    Ok(SnapshotId(bytes.try_into().map_err(|_| {
        Error::Frame("snapshot must have 16 bytes")
    })?))
}
fn cursor(cursor: p::Continuation) -> Result<Continuation> {
    if cursor.operation == 0 {
        return Err(Error::Frame("zero operation id"));
    }
    Ok(Continuation {
        snapshot: snapshot(cursor.snapshot)?,
        operation: cursor.operation,
        step: cursor.step,
    })
}
fn interval(interval: p::Interval) -> Result<Interval> {
    let end = match interval
        .end
        .ok_or_else(missing)?
        .value
        .ok_or_else(missing)?
    {
        p::bound::Value::Tick(time) => TimeBound::Tick(time),
        p::bound::Value::AfterMax(_) => TimeBound::AfterMax,
    };
    Interval::new(interval.start, end)
}
fn query(query: p::Query) -> Result<(Query, Limits)> {
    let limits = query.limits.ok_or_else(missing)?;
    let limits = Limits {
        bytes: usize::try_from(limits.bytes).map_err(|_| Error::ResourceLimit)?,
        records: limits.records as usize,
        work: usize::try_from(limits.work).map_err(|_| Error::ResourceLimit)?,
    };
    if limits.bytes == 0
        || limits.bytes > MAX_DECODED_BYTES
        || limits.records == 0
        || limits.records > MAX_RECORDS
        || limits.work == 0
        || limits.work > 65536
    {
        return Err(Error::ResourceLimit);
    }
    use p::query::Operation as O;
    let query = match query.operation.ok_or_else(missing)? {
        O::Window(q) => Query::Window {
            signal: q.signal,
            interval: interval(q.interval.ok_or_else(missing)?)?,
        },
        O::Summary(q) => {
            let g = q.grid.ok_or_else(missing)?;
            Query::Summary {
                signal: q.signal,
                grid: Grid::new(
                    g.start,
                    u8::try_from(g.level).map_err(|_| Error::Invalid("invalid grid level"))?,
                    g.count,
                )?,
            }
        }
        O::Children(q) => Query::Children { parent: q.parent },
        O::Search(q) => Query::Search {
            scope: q.scope,
            needle: q.needle,
        },
        O::Resolve(q) => Query::Resolve {
            paths: q
                .paths
                .into_iter()
                .map(|p| {
                    let kind = p
                        .kind
                        .map(|k| u8::try_from(k).map_err(|_| Error::Invalid("invalid node kind")))
                        .transpose()?;
                    if kind.is_some_and(|kind| !(1..=5).contains(&kind)) {
                        return Err(Error::Invalid("invalid node kind"));
                    }
                    Ok(Path {
                        segments: p.segments,
                        occurrence: p.occurrence,
                        kind,
                    })
                })
                .collect::<Result<_>>()?,
        },
        O::Text(q) => Query::Text {
            id: q.id,
            offset: q.offset,
            length: usize::try_from(q.length).map_err(|_| Error::ResourceLimit)?,
        },
    };
    Ok((query, limits))
}

pub fn decode(input: &[u8], budget: &Budget) -> Result<Request> {
    if input.is_empty() || input.len() > MAX_REQUEST_BYTES {
        return Err(Error::ResourceLimit);
    }
    let mut admission = Admission {
        bytes: 0,
        fields: 0,
        messages: 0,
    };
    admission.scan(Schema::Envelope, input, 0)?;
    let charge = budget.reserve(admission.bytes)?;
    let envelope =
        p::Envelope::decode(input).map_err(|_| Error::Frame("invalid Protobuf request"))?;
    if envelope.version != VERSION {
        return Err(Error::Frame("unsupported protocol version"));
    }
    if envelope.request_id == 0 {
        return Err(Error::Frame("zero request id"));
    }
    let snapshot = if envelope.snapshot.is_empty() {
        None
    } else {
        Some(snapshot(envelope.snapshot)?)
    };
    use p::envelope::Body as B;
    let body = match envelope.body.ok_or_else(missing)? {
        B::Hello(_) => RequestBody::Hello,
        B::Open(_) => RequestBody::Open,
        B::Close(_) => RequestBody::Close,
        B::Query(q) => {
            let (query, limits) = query(q)?;
            RequestBody::Query { query, limits }
        }
        B::Next(c) => RequestBody::Next(cursor(c)?),
        B::Release(c) => RequestBody::Release(cursor(c)?),
        B::Cancel(c) if c.request_id != 0 => RequestBody::Cancel(c.request_id),
        _ => return Err(Error::Frame("wrong-direction request body")),
    };
    if matches!(
        body,
        RequestBody::Query { .. }
            | RequestBody::Next(_)
            | RequestBody::Release(_)
            | RequestBody::Cancel(_)
    ) && snapshot.is_none()
    {
        return Err(Error::Frame("request needs a snapshot"));
    }
    if let RequestBody::Next(cursor) | RequestBody::Release(cursor) = &body {
        if Some(cursor.snapshot) != snapshot {
            return Err(Error::Frame("conflicting snapshot identities"));
        }
    }
    Ok(Request {
        request_id: envelope.request_id,
        snapshot,
        body,
        _charge: charge,
    })
}
