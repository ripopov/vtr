//! Admitted wire replies converted to the same immutable pages as native queries.
//! Cross-page query identity and demand matching remain the RPC driver's job.
use crate::metadata::{
    Declaration, DeclarationData, DeclarationPage, Resolution, ResolvePage, SearchPage, Text,
    TextPart,
};
use crate::session::{Delivery, Reply, SessionInfo, SnapshotId};
use crate::summary::{RealSummary, SummaryPage, WaveBin};
use crate::wave::{Bytes, Change, Kind, Predecessor, Sample, Value, WindowPage};
use crate::wire::{
    proto as p, MAX_DECODED_BYTES, MAX_OUTSTANDING, MAX_RECORDS, MAX_REPLY_BYTES, MAX_REQUEST_BYTES,
};
use crate::wire_request::{cursor, interval, missing, snapshot, VERSION};
use crate::{Budget, Error, Grid, Interval, Reservation, Result};
use prost::Message;
use std::sync::Arc;

#[derive(Debug)]
pub enum Body {
    Welcome(p::Welcome),
    Opened {
        info: SessionInfo,
        operations: Vec<p::Operation>,
    },
    Delivery(Arc<Delivery>),
    Failure {
        code: p::failure::Code,
        message: String,
    },
    Acknowledged,
}
#[derive(Debug)]
pub struct Response {
    pub request_id: u64,
    pub snapshot: Option<SnapshotId>,
    body: Body,
    _charge: Reservation,
}
impl Response {
    pub fn body(&self) -> &Body {
        &self.body
    }
}
fn invalid() -> Error {
    Error::Frame("invalid reply semantics")
}
fn required<T>(value: Option<T>) -> Result<T> {
    value.ok_or_else(missing)
}
fn bit_kind(width: u32, states: u32) -> Result<Kind> {
    if !matches!(states, 2 | 4 | 9) {
        return Err(invalid());
    }
    Ok(Kind::Bits {
        width,
        states: states as u8,
    })
}
fn kind(k: p::Kind) -> Result<Kind> {
    Ok(match required(k.value)? {
        p::kind::Value::Bits(k) => bit_kind(k.width, k.states)?,
        p::kind::Value::Real(_) => Kind::Real,
        p::kind::Value::Bytes(_) => Kind::Bytes,
    })
}
fn value(v: p::Value, charge: &Reservation) -> Result<Value> {
    Ok(match required(v.value)? {
        p::value::Value::Bits(b) => {
            bit_kind(b.width, b.states)?;
            let bits = match b.states {
                2 => 1u64,
                4 => 2,
                _ => 4,
            };
            let count = u64::from(b.width) * bits;
            if b.data.len() as u64 != count.div_ceil(8) {
                return Err(invalid());
            }
            // Unused packing bits are raw backend bytes, not logic codes.
            if b.states == 9
                && (0..u64::from(b.width))
                    .any(|i| (b.data[(i / 2) as usize] >> ((i % 2) * 4)) & 15 > 8)
            {
                return Err(invalid());
            }
            Value::Bits {
                width: b.width,
                states: b.states as u8,
                data: Bytes::from_admitted(b.data, charge),
            }
        }
        p::value::Value::Real(bits) => Value::Real(bits),
        p::value::Value::Bytes(bytes) => Value::Bytes(Bytes::from_admitted(bytes, charge)),
    })
}
fn sample(s: p::Sample, charge: &Reservation) -> Result<Sample> {
    Ok(match required(s.value)? {
        p::sample::Value::Known(v) => Sample::Known(value(v, charge)?),
        p::sample::Value::BackendDefault(k) => Sample::BackendDefault(kind(k)?),
        p::sample::Value::Event(_) => Sample::Event,
    })
}
fn value_kind(v: &Value) -> Kind {
    match v {
        Value::Bits { width, states, .. } => Kind::Bits {
            width: *width,
            states: *states,
        },
        Value::Real(_) => Kind::Real,
        Value::Bytes(_) => Kind::Bytes,
    }
}
fn sample_kind(s: &Sample) -> Option<Kind> {
    match s {
        Sample::Known(v) => Some(value_kind(v)),
        Sample::BackendDefault(k) => Some(*k),
        Sample::Event => None,
    }
}
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Real(a), Value::Real(b)) => a == b,
        (Value::Bytes(a), Value::Bytes(b)) => a.as_slice() == b.as_slice(),
        (
            Value::Bits {
                width: a,
                states: sa,
                data: da,
            },
            Value::Bits {
                width: b,
                states: sb,
                data: db,
            },
        ) => a == b && sa == sb && da.as_slice() == db.as_slice(),
        _ => false,
    }
}
pub(crate) fn same_sample(a: &Sample, b: &Sample) -> bool {
    match (a, b) {
        (Sample::Known(a), Sample::Known(b)) => same_value(a, b),
        (Sample::BackendDefault(a), Sample::BackendDefault(b)) => a == b,
        (Sample::Event, Sample::Event) => true,
        _ => false,
    }
}
fn change(
    c: p::Change,
    range: Interval,
    domain: Option<Kind>,
    charge: &Reservation,
) -> Result<Change> {
    let value = value(required(c.value)?, charge)?;
    if !range.contains(c.time) || domain.is_some_and(|k| k != value_kind(&value)) {
        return Err(invalid());
    }
    Ok(Change {
        time: c.time,
        value,
    })
}
fn text(t: p::Text, charge: &Reservation) -> Result<Text> {
    Ok(match required(t.value)? {
        p::text::Value::Inline(s) => Text::Inline(Bytes::from_admitted(s.into_bytes(), charge)),
        p::text::Value::Reference(r) => Text::Reference {
            id: r.id,
            bytes: r.bytes,
        },
    })
}
fn declaration(d: p::Declaration, charge: &Reservation) -> Result<Declaration> {
    use p::declaration::Data as D;
    let data = match required(d.data)? {
        D::Scope(s) => DeclarationData::Scope {
            type_code: u16::try_from(s.type_code).map_err(|_| invalid())?,
            component: text(required(s.component)?, charge)?,
        },
        D::Variable(v) => DeclarationData::Variable {
            type_code: u16::try_from(v.type_code).map_err(|_| invalid())?,
            direction: u8::try_from(v.direction).map_err(|_| invalid())?,
            signal: v.signal,
            kind: kind(required(v.kind)?)?,
            alias: v.alias,
        },
        D::Stream(s) => DeclarationData::Stream {
            kind: text(required(s.kind)?, charge)?,
        },
        D::Generator(_) => DeclarationData::Generator,
        D::EnumTable(e) => DeclarationData::EnumTable { entries: e.entries },
    };
    if d.parent == Some(d.id) {
        return Err(invalid());
    }
    Ok(Declaration {
        id: d.id,
        parent: d.parent,
        name: text(required(d.name)?, charge)?,
        data,
        children: d.children,
        attributes: d.attributes,
    })
}
fn declarations(ds: Vec<p::Declaration>, charge: &Reservation) -> Result<Vec<Declaration>> {
    let mut last = None;
    ds.into_iter()
        .map(|d| {
            if last.is_some_and(|last| last >= d.id) {
                return Err(invalid());
            }
            last = Some(d.id);
            declaration(d, charge)
        })
        .collect()
}
fn bin(b: p::WaveBin, expected: Interval, charge: &Reservation) -> Result<Arc<WaveBin>> {
    let range = interval(required(b.interval)?)?;
    if range != expected {
        return Err(invalid());
    }
    let entry = sample(required(b.entry)?, charge)?;
    let exit = sample(required(b.exit)?, charge)?;
    let domain = sample_kind(&entry);
    if sample_kind(&exit) != domain {
        return Err(invalid());
    }
    let first = b
        .first
        .map(|c| change(c, range, domain, charge))
        .transpose()?;
    let last = b
        .last
        .map(|c| change(c, range, domain, charge))
        .transpose()?;
    if (b.changes == 0) != first.is_none()
        || first.is_some() != last.is_some()
        || first
            .as_ref()
            .zip(last.as_ref())
            .is_some_and(|(f, l)| f.time > l.time)
    {
        return Err(invalid());
    }
    if b.changes == 0 && !same_sample(&entry, &exit) {
        return Err(invalid());
    }
    if let Some((first, last)) = first.as_ref().zip(last.as_ref()) {
        if b.changes == 1 && (first.time != last.time || !same_value(&first.value, &last.value)) {
            return Err(invalid());
        }
        if domain.is_some() && !matches!(&exit, Sample::Known(v) if same_value(v,&last.value)) {
            return Err(invalid());
        }
    }
    if b.real.is_some() != (domain == Some(Kind::Real)) {
        return Err(invalid());
    }
    let real = b
        .real
        .map(|r| {
            match (r.finite_min, r.finite_max) {
                (None, None) => (),
                (Some(min), Some(max))
                    if f64::from_bits(min).is_finite()
                        && f64::from_bits(max).is_finite()
                        && !f64::from_bits(min).total_cmp(&f64::from_bits(max)).is_gt() => {}
                _ => return Err(invalid()),
            }
            let nonfinite = r
                .nan_changes
                .checked_add(r.positive_infinite_changes)
                .and_then(|n| n.checked_add(r.negative_infinite_changes))
                .ok_or_else(invalid)?;
            if nonfinite > b.changes {
                return Err(invalid());
            }
            Ok(RealSummary {
                finite_min: r.finite_min,
                finite_max: r.finite_max,
                nan_changes: r.nan_changes,
                positive_infinite_changes: r.positive_infinite_changes,
                negative_infinite_changes: r.negative_infinite_changes,
            })
        })
        .transpose()?;
    Ok(Arc::new(WaveBin {
        interval: range,
        entry,
        exit,
        changes: b.changes,
        first,
        last,
        real,
        _charge: charge.clone(),
    }))
}
fn delivery(d: p::Delivery, identity: SnapshotId, charge: &Reservation) -> Result<Arc<Delivery>> {
    let request = cursor(required(d.request)?)?;
    let next = d.next.map(cursor).transpose()?;
    if request.snapshot != identity
        || next.is_some_and(|n| {
            n.snapshot != identity
                || n.operation != request.operation
                || request.step.checked_add(1) != Some(n.step)
        })
    {
        return Err(invalid());
    }
    use p::delivery::Page as P;
    let reply = match required(d.page)? {
        P::Window(p) => {
            let range = interval(required(p.interval)?)?;
            let predecessor = sample(required(p.predecessor)?, charge)?;
            let domain = sample_kind(&predecessor);
            let mut last = None;
            let changes = p
                .changes
                .into_iter()
                .map(|c| {
                    if last.is_some_and(|last| last > c.time) {
                        return Err(invalid());
                    }
                    last = Some(c.time);
                    change(c, range, domain, charge)
                })
                .collect::<Result<_>>()?;
            Reply::Window(Arc::new(WindowPage {
                interval: range,
                predecessor: Arc::new(Predecessor {
                    sample: predecessor,
                    _charge: charge.clone(),
                }),
                changes,
                complete: p.complete,
                _charge: charge.clone(),
            }))
        }
        P::Summary(p) => {
            let g = required(p.grid)?;
            let grid = Grid::new(
                g.start,
                u8::try_from(g.level).map_err(|_| invalid())?,
                g.count,
            )?;
            let end = p
                .offset
                .checked_add(u32::try_from(p.bins.len()).map_err(|_| invalid())?)
                .ok_or_else(invalid)?;
            if end > grid.count() || p.complete != (end == grid.count()) {
                return Err(invalid());
            }
            let bins = p
                .bins
                .into_iter()
                .enumerate()
                .map(|(i, b)| bin(b, grid.bin(p.offset + i as u32)?, charge))
                .collect::<Result<_>>()?;
            Reply::Summary(Arc::new(SummaryPage {
                grid,
                offset: p.offset,
                complete: p.complete,
                bins,
                _charge: charge.clone(),
            }))
        }
        P::Children(p) => {
            let declarations = declarations(p.declarations, charge)?;
            if declarations.iter().any(|d| d.parent != p.parent) {
                return Err(invalid());
            }
            p.offset
                .checked_add(declarations.len() as u64)
                .ok_or_else(invalid)?;
            Reply::Children(Arc::new(DeclarationPage {
                parent: p.parent,
                offset: p.offset,
                complete: p.complete,
                declarations,
                _charge: charge.clone(),
            }))
        }
        P::Search(p) => {
            let declarations = declarations(p.declarations, charge)?;
            // start_node is diagnostic progress; a partially matched name may
            // have started in an earlier work slice.
            Reply::Search(Arc::new(SearchPage {
                start_node: p.start_node,
                complete: p.complete,
                declarations,
                _charge: charge.clone(),
            }))
        }
        P::Resolve(p) => {
            let offset = usize::try_from(p.offset).map_err(|_| invalid())?;
            offset.checked_add(p.results.len()).ok_or_else(invalid)?;
            let results = p
                .results
                .into_iter()
                .map(|r| {
                    Ok(match required(r.result)? {
                        p::resolution::Result::Found(id) => Resolution::Found(id),
                        p::resolution::Result::Missing(_) => Resolution::Missing,
                        p::resolution::Result::Ambiguous(_) => Resolution::Ambiguous,
                    })
                })
                .collect::<Result<_>>()?;
            Reply::Resolve(Arc::new(ResolvePage {
                offset,
                complete: p.complete,
                results,
                _charge: charge.clone(),
            }))
        }
        P::Text(p) => {
            if p.offset
                .checked_add(p.bytes.len() as u64)
                .is_none_or(|end| end > p.total_bytes)
            {
                return Err(invalid());
            }
            Reply::Text(Arc::new(TextPart {
                id: p.id,
                offset: p.offset,
                total_bytes: p.total_bytes,
                bytes: Bytes::from_admitted(p.bytes, charge),
                _charge: charge.clone(),
            }))
        }
    };
    if reply.complete() != next.is_none() {
        return Err(invalid());
    }
    Ok(Arc::new(Delivery {
        request,
        next,
        reply,
        _charge: charge.clone(),
    }))
}

pub fn decode(input: &[u8], budget: &Budget) -> Result<Response> {
    if input.is_empty() || input.len() > MAX_REPLY_BYTES {
        return Err(Error::ResourceLimit);
    }
    let charge =
        crate::wire_admission::admit(crate::wire_admission::Schema::ReplyEnvelope, input, budget)?;
    decode_admitted(input, charge)
}

pub(crate) fn decode_reserved(
    input: &[u8],
    charge: Reservation,
    decoded_limit: usize,
) -> Result<Response> {
    if input.is_empty() || input.len() > MAX_REPLY_BYTES {
        return Err(Error::ResourceLimit);
    }
    let decoded =
        crate::wire_admission::decoded_bytes(crate::wire_admission::Schema::ReplyEnvelope, input)?;
    if decoded > decoded_limit {
        return Err(Error::ResourceLimit);
    }
    let retained = decoded
        .checked_add(input.len())
        .ok_or(Error::ResourceLimit)?;
    if retained > charge.bytes() {
        return Err(Error::ResourceLimit);
    }
    let response = decode_admitted(input, charge.clone())?;
    charge.shrink(retained)?;
    Ok(response)
}

/// Read only the outer request ID to select prepaid capacity. Full schema and
/// semantic validation must still run before accepting the response.
pub(crate) fn request_id(mut input: &[u8]) -> Result<u64> {
    use crate::wire_admission::varint;
    if input.is_empty() || input.len() > MAX_REPLY_BYTES {
        return Err(Error::ResourceLimit);
    }
    let mut id = None;
    while !input.is_empty() {
        let key = varint(&mut input)?;
        match key {
            8 => {
                varint(&mut input)?;
            }
            16 => {
                if id.is_some() {
                    return Err(invalid());
                }
                id = Some(varint(&mut input)?);
            }
            26 | 90 | 106 | 130 | 162 | 170 => {
                let length = usize::try_from(varint(&mut input)?).map_err(|_| invalid())?;
                input = input.get(length..).ok_or_else(invalid)?;
            }
            _ => return Err(invalid()),
        }
    }
    id.filter(|id| *id != 0).ok_or_else(invalid)
}

fn decode_admitted(input: &[u8], charge: Reservation) -> Result<Response> {
    let e = p::Envelope::decode(input).map_err(|_| Error::Frame("invalid Protobuf reply"))?;
    if e.version != VERSION || e.request_id == 0 {
        return Err(invalid());
    }
    let identity = if e.snapshot.is_empty() {
        None
    } else {
        Some(snapshot(e.snapshot)?)
    };
    use p::envelope::Body as B;
    let body = match required(e.body)? {
        B::Welcome(w) => {
            let pairs = [
                (w.max_request_bytes, MAX_REQUEST_BYTES),
                (w.max_reply_bytes, MAX_REPLY_BYTES),
                (w.max_decoded_bytes, MAX_DECODED_BYTES),
                (w.max_records, MAX_RECORDS),
                (w.max_outstanding, MAX_OUTSTANDING),
            ];
            if identity.is_some()
                || pairs
                    .iter()
                    .any(|(value, max)| *value == 0 || *value as usize > *max)
            {
                return Err(invalid());
            }
            Body::Welcome(w)
        }
        B::Opened(i) => {
            let snapshot = snapshot(i.snapshot)?;
            if identity != Some(snapshot) || i.operations.len() > 6 {
                return Err(invalid());
            }
            let mut seen = 0u32;
            let operations = i
                .operations
                .into_iter()
                .map(|o| {
                    if !(1..=6).contains(&o) || seen & (1 << o) != 0 {
                        return Err(invalid());
                    }
                    seen |= 1 << o;
                    p::Operation::try_from(o).map_err(|_| invalid())
                })
                .collect::<Result<_>>()?;
            Body::Opened {
                info: SessionInfo {
                    snapshot,
                    timescale: i8::try_from(i.timescale).map_err(|_| invalid())?,
                    time_range: i.time_range.map(interval).transpose()?,
                    signals: i.signals,
                    declarations: i.declarations,
                },
                operations,
            }
        }
        B::Reply(d) => Body::Delivery(delivery(d, identity.ok_or_else(invalid)?, &charge)?),
        B::Failure(f) => {
            let code = p::failure::Code::try_from(f.code).map_err(|_| invalid())?;
            if code == p::failure::Code::Unspecified || f.message.len() > 8192 {
                return Err(invalid());
            }
            Body::Failure {
                code,
                message: f.message,
            }
        }
        B::Acknowledged(_) => Body::Acknowledged,
        _ => return Err(invalid()),
    };
    Ok(Response {
        request_id: e.request_id,
        snapshot: identity,
        body,
        _charge: charge,
    })
}
