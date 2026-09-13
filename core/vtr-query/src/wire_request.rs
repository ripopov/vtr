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

pub(crate) fn missing() -> Error {
    Error::Frame("missing required protocol field")
}
pub(crate) fn snapshot(bytes: Vec<u8>) -> Result<SnapshotId> {
    Ok(SnapshotId(bytes.try_into().map_err(|_| {
        Error::Frame("snapshot must have 16 bytes")
    })?))
}
pub(crate) fn cursor(cursor: p::Continuation) -> Result<Continuation> {
    if cursor.operation == 0 {
        return Err(Error::Frame("zero operation id"));
    }
    Ok(Continuation {
        snapshot: snapshot(cursor.snapshot)?,
        operation: cursor.operation,
        step: cursor.step,
    })
}
pub(crate) fn interval(interval: p::Interval) -> Result<Interval> {
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
    let charge =
        crate::wire_admission::admit(crate::wire_admission::Schema::Envelope, input, budget)?;
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
