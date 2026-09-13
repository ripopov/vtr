//! Count-then-write Protobuf encoding over borrowed typed results. The sizing
//! pass allocates nothing; the writing pass allocates one admitted output buffer.
//! Generated Prost types are used by conformance tests, not as duplicate reply
//! trees on the server. Native in-process delivery never calls this module.
use crate::metadata::{Declaration, DeclarationData, Resolution, Text};
use crate::session::{Continuation, Delivery, Query, Reply, SessionInfo, SnapshotId};
use crate::wave::{Change, Kind, Limits, Sample, Value};
use crate::wire::{MAX_REPLY_BYTES, MAX_REQUEST_BYTES};
use crate::wire_request::{RequestBody, VERSION};
use crate::{Budget, Error, Grid, Interval, Reservation, Result, TimeBound};

#[derive(Debug)]
pub struct Encoded {
    bytes: Vec<u8>,
    _charge: Reservation,
}
impl Encoded {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
struct Writer {
    output: Option<Vec<u8>>,
    len: usize,
    limit: usize,
}
impl Writer {
    fn raw(&mut self, bytes: &[u8]) -> Result<()> {
        let end = self
            .len
            .checked_add(bytes.len())
            .ok_or(Error::ResourceLimit)?;
        if end > self.limit {
            return Err(Error::ResourceLimit);
        }
        if let Some(out) = &mut self.output {
            out.extend_from_slice(bytes);
        }
        self.len = end;
        Ok(())
    }
    fn varint(&mut self, mut value: u64) -> Result<()> {
        let mut data = [0; 10];
        let mut len = 0;
        loop {
            data[len] = (value & 127) as u8;
            value >>= 7;
            if value != 0 {
                data[len] |= 128;
            }
            len += 1;
            if value == 0 {
                break;
            }
        }
        self.raw(&data[..len])
    }
    fn uint(&mut self, tag: u32, value: u64) -> Result<()> {
        self.varint(u64::from(tag) << 3)?;
        self.varint(value)
    }
    fn fixed(&mut self, tag: u32, value: u64) -> Result<()> {
        self.varint((u64::from(tag) << 3) | 1)?;
        self.raw(&value.to_le_bytes())
    }
    fn bytes(&mut self, tag: u32, bytes: &[u8]) -> Result<()> {
        self.varint((u64::from(tag) << 3) | 2)?;
        self.varint(bytes.len() as u64)?;
        self.raw(bytes)
    }
    fn message(&mut self, tag: u32, body: impl Fn(&mut Self) -> Result<()>) -> Result<()> {
        let mut count = Self {
            output: None,
            len: 0,
            limit: self.limit,
        };
        body(&mut count)?;
        self.varint((u64::from(tag) << 3) | 2)?;
        self.varint(count.len as u64)?;
        if self.output.is_some() {
            body(self)
        } else {
            self.len = self
                .len
                .checked_add(count.len)
                .ok_or(Error::ResourceLimit)?;
            if self.len > self.limit {
                Err(Error::ResourceLimit)
            } else {
                Ok(())
            }
        }
    }
    fn empty(&mut self, tag: u32) -> Result<()> {
        self.bytes(tag, &[])
    }
}
fn encode(
    limit: usize,
    budget: &Budget,
    body: impl Fn(&mut Writer) -> Result<()>,
) -> Result<Encoded> {
    let mut count = Writer {
        output: None,
        len: 0,
        limit,
    };
    body(&mut count)?;
    let charge = budget.reserve(
        count
            .len
            .checked_add(std::mem::size_of::<Encoded>())
            .ok_or(Error::ResourceLimit)?,
    )?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(count.len)
        .map_err(|_| Error::ResourceLimit)?;
    if bytes.capacity() != count.len {
        return Err(Error::ResourceLimit);
    }
    let mut writer = Writer {
        output: Some(bytes),
        len: 0,
        limit: count.len,
    };
    body(&mut writer)?;
    if writer.len != count.len {
        return Err(Error::Invalid("encoding size changed between passes"));
    }
    Ok(Encoded {
        bytes: writer.output.unwrap(),
        _charge: charge,
    })
}
fn header(w: &mut Writer, request_id: u64, snapshot: Option<SnapshotId>) -> Result<()> {
    if request_id == 0 {
        return Err(Error::Invalid("zero request id"));
    }
    w.uint(1, u64::from(VERSION))?;
    w.uint(2, request_id)?;
    if let Some(snapshot) = snapshot {
        w.bytes(3, &snapshot.0)?;
    }
    Ok(())
}
fn cursor(w: &mut Writer, c: &Continuation) -> Result<()> {
    w.bytes(1, &c.snapshot.0)?;
    w.uint(2, c.operation)?;
    w.uint(3, c.step)
}
fn interval(w: &mut Writer, i: Interval) -> Result<()> {
    w.uint(1, i.start())?;
    w.message(2, |w| match i.end() {
        TimeBound::Tick(t) => w.uint(1, t),
        TimeBound::AfterMax => w.empty(2),
    })
}
fn grid(w: &mut Writer, g: Grid) -> Result<()> {
    w.uint(1, g.start())?;
    w.uint(2, u64::from(g.level()))?;
    w.uint(3, u64::from(g.count()))
}
fn kind(w: &mut Writer, k: Kind) -> Result<()> {
    match k {
        Kind::Bits { width, states } => w.message(1, |w| {
            w.uint(1, u64::from(width))?;
            w.uint(2, u64::from(states))
        }),
        Kind::Real => w.empty(2),
        Kind::Bytes => w.empty(3),
    }
}
fn value(w: &mut Writer, v: &Value) -> Result<()> {
    match v {
        Value::Bits {
            width,
            states,
            data,
        } => w.message(1, |w| {
            w.uint(1, u64::from(*width))?;
            w.uint(2, u64::from(*states))?;
            w.bytes(3, data.as_slice())
        }),
        Value::Real(bits) => w.fixed(2, *bits),
        Value::Bytes(bytes) => w.bytes(3, bytes.as_slice()),
    }
}
fn sample(w: &mut Writer, s: &Sample) -> Result<()> {
    match s {
        Sample::Known(v) => w.message(1, |w| value(w, v)),
        Sample::BackendDefault(k) => w.message(2, |w| kind(w, *k)),
        Sample::Event => w.empty(3),
    }
}
fn change(w: &mut Writer, c: &Change) -> Result<()> {
    w.uint(1, c.time)?;
    w.message(2, |w| value(w, &c.value))
}
fn text(w: &mut Writer, t: &Text) -> Result<()> {
    match t {
        Text::Inline(bytes) => {
            t.as_str()?;
            w.bytes(1, bytes.as_slice())
        }
        Text::Reference { id, bytes } => w.message(2, |w| {
            w.uint(1, u64::from(*id))?;
            w.uint(2, *bytes)
        }),
    }
}
fn declaration(w: &mut Writer, d: &Declaration) -> Result<()> {
    w.uint(1, u64::from(d.id))?;
    if let Some(parent) = d.parent {
        w.uint(2, u64::from(parent))?;
    }
    w.message(3, |w| text(w, &d.name))?;
    w.uint(4, d.children)?;
    w.uint(5, d.attributes)?;
    match &d.data {
        DeclarationData::Scope {
            type_code,
            component,
        } => w.message(10, |w| {
            w.uint(1, u64::from(*type_code))?;
            w.message(2, |w| text(w, component))
        }),
        DeclarationData::Variable {
            type_code,
            direction,
            signal,
            kind: k,
            alias,
        } => w.message(11, |w| {
            w.uint(1, u64::from(*type_code))?;
            w.uint(2, u64::from(*direction))?;
            w.uint(3, u64::from(*signal))?;
            w.message(4, |w| kind(w, *k))?;
            w.uint(5, u64::from(*alias))
        }),
        DeclarationData::Stream { kind } => w.message(12, |w| w.message(1, |w| text(w, kind))),
        DeclarationData::Generator => w.empty(13),
        DeclarationData::EnumTable { entries } => w.message(14, |w| w.uint(1, *entries)),
    }
}
fn bin(w: &mut Writer, bin: &crate::summary::WaveBin) -> Result<()> {
    w.message(1, |w| interval(w, bin.interval))?;
    w.message(2, |w| sample(w, &bin.entry))?;
    w.message(3, |w| sample(w, &bin.exit))?;
    w.uint(4, bin.changes)?;
    if let Some(first) = &bin.first {
        w.message(5, |w| change(w, first))?;
    }
    if let Some(last) = &bin.last {
        w.message(6, |w| change(w, last))?;
    }
    if let Some(real) = &bin.real {
        w.message(7, |w| {
            if let Some(v) = real.finite_min {
                w.fixed(1, v)?;
            }
            if let Some(v) = real.finite_max {
                w.fixed(2, v)?;
            }
            w.uint(3, real.nan_changes)?;
            w.uint(4, real.positive_infinite_changes)?;
            w.uint(5, real.negative_infinite_changes)
        })?;
    }
    Ok(())
}
fn reply(w: &mut Writer, reply: &Reply) -> Result<()> {
    match reply {
        Reply::Window(p) => w.message(10, |w| {
            w.message(1, |w| interval(w, p.interval))?;
            w.message(2, |w| sample(w, &p.predecessor.sample))?;
            for c in p.changes() {
                w.message(3, |w| change(w, c))?;
            }
            w.uint(4, u64::from(p.complete))
        }),
        Reply::Summary(p) => w.message(11, |w| {
            w.message(1, |w| grid(w, p.grid))?;
            w.uint(2, u64::from(p.offset))?;
            w.uint(3, u64::from(p.complete))?;
            for b in p.bins() {
                w.message(4, |w| bin(w, b))?;
            }
            Ok(())
        }),
        Reply::Children(p) => w.message(12, |w| {
            if let Some(parent) = p.parent {
                w.uint(1, u64::from(parent))?;
            }
            w.uint(2, p.offset)?;
            w.uint(3, u64::from(p.complete))?;
            for d in p.declarations() {
                w.message(4, |w| declaration(w, d))?;
            }
            Ok(())
        }),
        Reply::Search(p) => w.message(13, |w| {
            w.uint(1, u64::from(p.start_node))?;
            w.uint(2, u64::from(p.complete))?;
            for d in p.declarations() {
                w.message(3, |w| declaration(w, d))?;
            }
            Ok(())
        }),
        Reply::Resolve(p) => w.message(14, |w| {
            w.uint(1, p.offset as u64)?;
            w.uint(2, u64::from(p.complete))?;
            for result in p.results() {
                w.message(3, |w| match result {
                    Resolution::Found(id) => w.uint(1, u64::from(*id)),
                    Resolution::Missing => w.empty(2),
                    Resolution::Ambiguous => w.empty(3),
                })?;
            }
            Ok(())
        }),
        Reply::Text(p) => w.message(15, |w| {
            w.uint(1, u64::from(p.id))?;
            w.uint(2, p.offset)?;
            w.uint(3, p.total_bytes)?;
            w.bytes(4, p.bytes.as_slice())
        }),
    }
}
pub fn delivery(request_id: u64, d: &Delivery, budget: &Budget) -> Result<Encoded> {
    encode(MAX_REPLY_BYTES, budget, |w| {
        header(w, request_id, Some(d.request.snapshot))?;
        w.message(16, |w| {
            w.message(1, |w| cursor(w, &d.request))?;
            if let Some(next) = d.next {
                w.message(2, |w| cursor(w, &next))?;
            }
            reply(w, &d.reply)
        })
    })
}
pub fn opened(request_id: u64, info: &SessionInfo, budget: &Budget) -> Result<Encoded> {
    encode(MAX_REPLY_BYTES, budget, |w| {
        header(w, request_id, Some(info.snapshot))?;
        w.message(13, |w| {
            w.bytes(1, &info.snapshot.0)?;
            let scale = i32::from(info.timescale);
            w.uint(2, ((scale << 1) ^ (scale >> 31)) as u32 as u64)?;
            if let Some(range) = info.time_range {
                w.message(3, |w| interval(w, range))?;
            }
            w.uint(4, u64::from(info.signals))?;
            w.uint(5, info.declarations)?;
            for operation in 1..=6 {
                w.uint(6, operation)?;
            }
            Ok(())
        })
    })
}
pub fn welcome(request_id: u64, budget: &Budget) -> Result<Encoded> {
    encode(MAX_REPLY_BYTES, budget, |w| {
        header(w, request_id, None)?;
        w.message(11, |w| {
            w.uint(1, crate::wire::MAX_REQUEST_BYTES as u64)?;
            w.uint(2, crate::wire::MAX_REPLY_BYTES as u64)?;
            w.uint(3, crate::wire::MAX_DECODED_BYTES as u64)?;
            w.uint(4, crate::wire::MAX_RECORDS as u64)?;
            w.uint(5, crate::wire::MAX_OUTSTANDING as u64)
        })
    })
}
pub fn acknowledged(
    request_id: u64,
    snapshot: Option<SnapshotId>,
    budget: &Budget,
) -> Result<Encoded> {
    encode(MAX_REPLY_BYTES, budget, |w| {
        header(w, request_id, snapshot)?;
        w.empty(21)
    })
}
pub fn failure(
    request_id: u64,
    snapshot: Option<SnapshotId>,
    code: crate::wire::proto::failure::Code,
    message: &str,
    budget: &Budget,
) -> Result<Encoded> {
    if message.len() > 8192 {
        return Err(Error::ResourceLimit);
    }
    encode(MAX_REPLY_BYTES, budget, |w| {
        header(w, request_id, snapshot)?;
        w.message(20, |w| {
            w.uint(1, code as u64)?;
            w.bytes(2, message.as_bytes())
        })
    })
}
fn query(w: &mut Writer, q: &Query, limits: Limits) -> Result<()> {
    w.message(1, |w| {
        w.uint(1, limits.bytes as u64)?;
        w.uint(2, limits.records as u64)?;
        w.uint(3, limits.work as u64)
    })?;
    match q {
        Query::Window {
            signal,
            interval: i,
        } => w.message(10, |w| {
            w.uint(1, u64::from(*signal))?;
            w.message(2, |w| interval(w, *i))
        }),
        Query::Summary { signal, grid: g } => w.message(11, |w| {
            w.uint(1, u64::from(*signal))?;
            w.message(2, |w| grid(w, *g))
        }),
        Query::Children { parent } => w.message(12, |w| {
            if let Some(parent) = parent {
                w.uint(1, u64::from(*parent))?;
            }
            Ok(())
        }),
        Query::Search { scope, needle } => w.message(13, |w| {
            if let Some(scope) = scope {
                w.uint(1, u64::from(*scope))?;
            }
            w.bytes(2, needle.as_bytes())
        }),
        Query::Resolve { paths } => w.message(14, |w| {
            for p in paths {
                w.message(1, |w| {
                    for segment in &p.segments {
                        w.bytes(1, segment.as_bytes())?;
                    }
                    if let Some(n) = p.occurrence {
                        w.uint(2, u64::from(n))?;
                    }
                    if let Some(k) = p.kind {
                        w.uint(3, u64::from(k))?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        }),
        Query::Text { id, offset, length } => w.message(15, |w| {
            w.uint(1, u64::from(*id))?;
            w.uint(2, *offset)?;
            w.uint(3, *length as u64)
        }),
    }
}
pub fn request(
    request_id: u64,
    snapshot: Option<SnapshotId>,
    body: &RequestBody,
    budget: &Budget,
) -> Result<Encoded> {
    encode(MAX_REQUEST_BYTES, budget, |w| {
        header(w, request_id, snapshot)?;
        match body {
            RequestBody::Hello => w.empty(10),
            RequestBody::Open => w.empty(12),
            RequestBody::Close => w.empty(19),
            RequestBody::Query { query: q, limits } => w.message(14, |w| query(w, q, *limits)),
            RequestBody::Next(c) => w.message(15, |w| cursor(w, c)),
            RequestBody::Release(c) => w.message(18, |w| cursor(w, c)),
            RequestBody::Cancel(id) => w.message(17, |w| w.uint(1, *id)),
        }
    })
}
