//! Transaction blocks: transactions, their attributes, events, stages and
//! relations, stored column-wise per block.
//!
//! The writer appends one row per finished transaction (and per relation)
//! to a row buffer; at flush time the rows are split into columns and the
//! column set is compressed as a single blob.

use crate::codec::{Compression, Compressor, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::NodeId;
use crate::strings::StrId;
use crate::value::{Value, ValueTag};
use crate::varint::{self, Reader};

/// Transaction id (assigned by the writer, unique in the file, starts at 1).
pub type TxId = u64;

/// Terminal status of a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum TxStatus {
    /// Ended normally without a status (FTR, Konata retire).
    #[default]
    Unset = 0,
    /// Explicit success (OpenTelemetry `Ok`).
    Ok = 1,
    /// Explicit failure (OpenTelemetry `Error`).
    Error = 2,
    /// Aborted / squashed / flushed (Konata flush).
    Aborted = 3,
    /// Never ended before the file was closed.
    Open = 4,
}

impl TxStatus {
    pub fn from_u8(v: u8) -> TxStatus {
        match v {
            1 => TxStatus::Ok,
            2 => TxStatus::Error,
            3 => TxStatus::Aborted,
            4 => TxStatus::Open,
            _ => TxStatus::Unset,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TxStatus::Unset => "unset",
            TxStatus::Ok => "ok",
            TxStatus::Error => "error",
            TxStatus::Aborted => "aborted",
            TxStatus::Open => "open",
        }
    }
}

/// Transaction kind (OpenTelemetry span kinds; 0 = unspecified).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum TxKind {
    #[default]
    Unspecified = 0,
    Internal = 1,
    Server = 2,
    Client = 3,
    Producer = 4,
    Consumer = 5,
}

impl TxKind {
    pub fn from_u8(v: u8) -> TxKind {
        match v {
            1 => TxKind::Internal,
            2 => TxKind::Server,
            3 => TxKind::Client,
            4 => TxKind::Producer,
            5 => TxKind::Consumer,
            _ => TxKind::Unspecified,
        }
    }
}

/// When an attribute was recorded relative to the transaction lifetime (FTR semantics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum AttrPhase {
    Begin = 0,
    #[default]
    Record = 1,
    End = 2,
}

impl AttrPhase {
    pub fn from_u8(v: u8) -> AttrPhase {
        match v {
            0 => AttrPhase::Begin,
            2 => AttrPhase::End,
            _ => AttrPhase::Record,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TxAttr {
    pub key: StrId,
    pub phase: AttrPhase,
    pub value: Value,
}

/// A timestamped point event on a transaction (OpenTelemetry span event).
#[derive(Clone, Debug, PartialEq)]
pub struct TxEvent {
    pub time: u64,
    pub name: StrId,
    pub attrs: Vec<(StrId, Value)>,
}

/// A named sub-interval of a transaction on a lane (Konata pipeline stage).
#[derive(Clone, Debug, PartialEq)]
pub struct TxStage {
    pub name: StrId,
    pub lane: StrId,
    pub begin: u64,
    /// `None` = still open when the transaction ended.
    pub end: Option<u64>,
    pub attrs: Vec<(StrId, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub id: TxId,
    pub generator: NodeId,
    pub begin: u64,
    pub end: u64,
    pub status: TxStatus,
    pub kind: TxKind,
    pub parent: Option<TxId>,
    pub attrs: Vec<TxAttr>,
    pub events: Vec<TxEvent>,
    pub stages: Vec<TxStage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    pub kind: StrId,
    pub from: TxId,
    pub to: TxId,
    pub attrs: Vec<(StrId, Value)>,
}

// ---------------------------------------------------------------------------
// Row format (writer scratch and block input)
// ---------------------------------------------------------------------------

/// Writes an attribute list (`count`, then key/phase/value triples) in row form.
pub fn row_attrs(out: &mut Vec<u8>, attrs: &[(StrId, Value)]) {
    varint::put_u64(out, attrs.len() as u64);
    for (k, v) in attrs {
        varint::put_u64(out, k.0 as u64);
        out.push(AttrPhase::Record as u8);
        v.encode(out);
    }
}

/// Encodes a whole transaction as a row.
pub fn encode_tx_row(tx: &Transaction, out: &mut Vec<u8>) {
    varint::put_u64(out, tx.id);
    varint::put_u64(out, tx.generator.0 as u64);
    varint::put_u64(out, tx.begin);
    varint::put_u64(out, tx.end.wrapping_sub(tx.begin));
    out.push(tx.status as u8);
    out.push(tx.kind as u8);
    varint::put_u64(out, tx.parent.map(|p| p + 1).unwrap_or(0));
    varint::put_u64(out, tx.attrs.len() as u64);
    for a in &tx.attrs {
        varint::put_u64(out, a.key.0 as u64);
        out.push(a.phase as u8);
        a.value.encode(out);
    }
    varint::put_u64(out, tx.events.len() as u64);
    for e in &tx.events {
        varint::put_u64(out, e.time);
        varint::put_u64(out, e.name.0 as u64);
        row_attrs(out, &e.attrs);
    }
    varint::put_u64(out, tx.stages.len() as u64);
    for s in &tx.stages {
        varint::put_u64(out, s.name.0 as u64);
        varint::put_u64(out, s.lane.0 as u64);
        varint::put_u64(out, s.begin);
        varint::put_u64(out, s.end.map(|e| e.wrapping_sub(s.begin) + 1).unwrap_or(0));
        row_attrs(out, &s.attrs);
    }
}

fn decode_row_attrs(r: &mut Reader) -> Result<Vec<TxAttr>> {
    let n = r.usize()?;
    if n > r.remaining() {
        return Err(Error::Corrupt("attribute count exceeds row"));
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let key = StrId(r.u32()?);
        let phase = AttrPhase::from_u8(r.u8()?);
        let value = Value::decode(r)?;
        v.push(TxAttr { key, phase, value });
    }
    Ok(v)
}

fn decode_row_kv(r: &mut Reader) -> Result<Vec<(StrId, Value)>> {
    Ok(decode_row_attrs(r)?.into_iter().map(|a| (a.key, a.value)).collect())
}

pub fn decode_tx_row(r: &mut Reader) -> Result<Transaction> {
    let id = r.u64()?;
    let generator = NodeId(r.u32()?);
    let begin = r.u64()?;
    let end = begin.wrapping_add(r.u64()?);
    let status = TxStatus::from_u8(r.u8()?);
    let kind = TxKind::from_u8(r.u8()?);
    let parent = match r.u64()? {
        0 => None,
        p => Some(p - 1),
    };
    let attrs = decode_row_attrs(r)?;
    let ne = r.usize()?;
    if ne > r.remaining() {
        return Err(Error::Corrupt("event count exceeds row"));
    }
    let mut events = Vec::with_capacity(ne);
    for _ in 0..ne {
        let time = r.u64()?;
        let name = StrId(r.u32()?);
        let attrs = decode_row_kv(r)?;
        events.push(TxEvent { time, name, attrs });
    }
    let ns = r.usize()?;
    if ns > r.remaining() {
        return Err(Error::Corrupt("stage count exceeds row"));
    }
    let mut stages = Vec::with_capacity(ns);
    for _ in 0..ns {
        let name = StrId(r.u32()?);
        let lane = StrId(r.u32()?);
        let begin = r.u64()?;
        let end = match r.u64()? {
            0 => None,
            d => Some(begin.wrapping_add(d - 1)),
        };
        let attrs = decode_row_kv(r)?;
        stages.push(TxStage { name, lane, begin, end, attrs });
    }
    Ok(Transaction { id, generator, begin, end, status, kind, parent, attrs, events, stages })
}

pub fn encode_rel_row(rel: &Relation, out: &mut Vec<u8>) {
    varint::put_u64(out, rel.kind.0 as u64);
    varint::put_u64(out, rel.from);
    varint::put_u64(out, rel.to);
    row_attrs(out, &rel.attrs);
}

pub fn decode_rel_row(r: &mut Reader) -> Result<Relation> {
    let kind = StrId(r.u32()?);
    let from = r.u64()?;
    let to = r.u64()?;
    let attrs = decode_row_kv(r)?;
    Ok(Relation { kind, from, to, attrs })
}

// ---------------------------------------------------------------------------
// Block encoding (columnar)
// ---------------------------------------------------------------------------

pub const TX_HEADER_LEN: usize = 88;

/// Fixed header of a transaction block.
#[derive(Clone, Copy, Debug, Default)]
pub struct TxBlockHeader {
    pub n_tx: u64,
    pub n_rel: u64,
    pub min_id: u64,
    pub max_id: u64,
    pub t_min: u64,
    pub t_max: u64,
    pub rel_min_from: u64,
    pub rel_max_from: u64,
    pub rel_min_to: u64,
    pub rel_max_to: u64,
    pub n_gens: u32,
    pub blob_len: u32,
}

impl TxBlockHeader {
    pub fn parse(p: &[u8]) -> Result<TxBlockHeader> {
        if p.len() < TX_HEADER_LEN {
            return Err(Error::Corrupt("tx block header truncated"));
        }
        let u64_at = |o: usize| u64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        let h = TxBlockHeader {
            n_tx: u64_at(0),
            n_rel: u64_at(8),
            min_id: u64_at(16),
            max_id: u64_at(24),
            t_min: u64_at(32),
            t_max: u64_at(40),
            rel_min_from: u64_at(48),
            rel_max_from: u64_at(56),
            rel_min_to: u64_at(64),
            rel_max_to: u64_at(72),
            n_gens: u32_at(80),
            blob_len: u32_at(84),
        };
        if h.blob_off() + h.blob_len as usize > p.len() {
            return Err(Error::Corrupt("tx block truncated"));
        }
        Ok(h)
    }
    pub fn gens_off(&self) -> usize {
        TX_HEADER_LEN
    }
    pub fn blob_off(&self) -> usize {
        TX_HEADER_LEN + self.n_gens as usize * 4
    }
    fn write(&self, out: &mut Vec<u8>) {
        for v in [
            self.n_tx,
            self.n_rel,
            self.min_id,
            self.max_id,
            self.t_min,
            self.t_max,
            self.rel_min_from,
            self.rel_max_from,
            self.rel_min_to,
            self.rel_max_to,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.n_gens.to_le_bytes());
        out.extend_from_slice(&self.blob_len.to_le_bytes());
    }
}

/// Generator ids present in a block (sorted).
pub fn block_generators<'a>(p: &'a [u8], h: &TxBlockHeader) -> impl Iterator<Item = u32> + 'a {
    let base = h.gens_off();
    (0..h.n_gens as usize).map(move |i| u32::from_le_bytes(p[base + i * 4..base + i * 4 + 4].try_into().unwrap()))
}

/// Column ids.
const C_ID: usize = 0;
const C_GEN: usize = 1;
const C_BEGIN: usize = 2;
const C_DUR: usize = 3;
const C_STATUS: usize = 4;
const C_PARENT: usize = 5;
const C_ACOUNT: usize = 6;
const C_AKEY: usize = 7;
const C_ATAG: usize = 8;
const C_ANUM: usize = 9;
const C_AF64: usize = 10;
const C_ASTR: usize = 11;
const C_AMISC: usize = 12;
const C_ECOUNT: usize = 13;
const C_ETIME: usize = 14;
const C_ENAME: usize = 15;
const C_SCOUNT: usize = 16;
const C_SNAME: usize = 17;
const C_SLANE: usize = 18;
const C_SBEGIN: usize = 19;
const C_SEND: usize = 20;
const C_RKIND: usize = 21;
const C_RFROM: usize = 22;
const C_RTO: usize = 23;
const N_COLS: usize = 24;

struct Columns {
    c: Vec<Vec<u8>>,
    prev_id: u64,
    prev_begin: u64,
    prev_from: u64,
}

impl Columns {
    fn new() -> Self {
        Columns { c: (0..N_COLS).map(|_| Vec::new()).collect(), prev_id: 0, prev_begin: 0, prev_from: 0 }
    }

    fn put_attr_value(&mut self, key: StrId, phase: u8, v: &Value) {
        varint::put_u64(&mut self.c[C_AKEY], key.0 as u64);
        self.c[C_ATAG].push((phase << 5) | v.tag() as u8);
        match v {
            Value::Null => {}
            Value::Bool(b) => self.c[C_ANUM].push(*b as u8),
            Value::I64(x) => varint::put_i64(&mut self.c[C_ANUM], *x),
            Value::U64(x) | Value::Time(x) | Value::Pointer(x) => varint::put_u64(&mut self.c[C_ANUM], *x),
            Value::F64(f) => self.c[C_AF64].extend_from_slice(&f.to_le_bytes()),
            Value::Str(s) => varint::put_u64(&mut self.c[C_ASTR], s.0 as u64),
            Value::Enum { value, name } => {
                varint::put_i64(&mut self.c[C_ANUM], *value);
                varint::put_u64(&mut self.c[C_ASTR], name.0 as u64);
            }
            Value::Fixed { raw, scale } => {
                varint::put_i64(&mut self.c[C_ANUM], *raw);
                varint::put_i64(&mut self.c[C_ANUM], *scale as i64);
            }
            Value::UFixed { raw, scale } => {
                varint::put_u64(&mut self.c[C_ANUM], *raw);
                varint::put_i64(&mut self.c[C_ANUM], *scale as i64);
            }
            other => other.encode_payload(&mut self.c[C_AMISC]),
        }
    }

    fn put_attrs(&mut self, attrs: &[TxAttr]) {
        varint::put_u64(&mut self.c[C_ACOUNT], attrs.len() as u64);
        for a in attrs {
            self.put_attr_value(a.key, a.phase as u8, &a.value);
        }
    }

    fn put_kv(&mut self, attrs: &[(StrId, Value)]) {
        varint::put_u64(&mut self.c[C_ACOUNT], attrs.len() as u64);
        for (k, v) in attrs {
            self.put_attr_value(*k, AttrPhase::Record as u8, v);
        }
    }

    /// Reads one attribute list in row form and appends it to the attribute columns.
    fn put_row_attrs(&mut self, r: &mut Reader) -> Result<()> {
        let n = r.usize()?;
        varint::put_u64(&mut self.c[C_ACOUNT], n as u64);
        for _ in 0..n {
            let key = StrId(r.u32()?);
            let phase = r.u8()?;
            let tag = ValueTag::from_u8(r.u8()?)?;
            varint::put_u64(&mut self.c[C_AKEY], key.0 as u64);
            self.c[C_ATAG].push((phase << 5) | tag as u8);
            match tag {
                ValueTag::Null => {}
                ValueTag::Bool => self.c[C_ANUM].push(r.u8()?),
                ValueTag::I64 => {
                    let v = r.i64()?;
                    varint::put_i64(&mut self.c[C_ANUM], v);
                }
                ValueTag::U64 | ValueTag::Time | ValueTag::Pointer => {
                    let v = r.u64()?;
                    varint::put_u64(&mut self.c[C_ANUM], v);
                }
                ValueTag::F64 => self.c[C_AF64].extend_from_slice(r.bytes(8)?),
                ValueTag::Str => {
                    let s = r.u64()?;
                    varint::put_u64(&mut self.c[C_ASTR], s);
                }
                ValueTag::Enum => {
                    let v = r.i64()?;
                    let s = r.u64()?;
                    varint::put_i64(&mut self.c[C_ANUM], v);
                    varint::put_u64(&mut self.c[C_ASTR], s);
                }
                ValueTag::Fixed => {
                    let a = r.i64()?;
                    let b = r.i64()?;
                    varint::put_i64(&mut self.c[C_ANUM], a);
                    varint::put_i64(&mut self.c[C_ANUM], b);
                }
                ValueTag::UFixed => {
                    let a = r.u64()?;
                    let b = r.i64()?;
                    varint::put_u64(&mut self.c[C_ANUM], a);
                    varint::put_i64(&mut self.c[C_ANUM], b);
                }
                other => {
                    // Nested / byte payloads: copy verbatim by re-decoding once.
                    let v = Value::decode_payload(other, r)?;
                    v.encode_payload(&mut self.c[C_AMISC]);
                }
            }
        }
        Ok(())
    }

    /// Transcodes one transaction row (see `encode_tx_row`) into the columns.
    /// Returns (id, generator, begin, end).
    fn put_tx_row(&mut self, r: &mut Reader) -> Result<(u64, u32, u64, u64)> {
        let id = r.u64()?;
        let gen = r.u32()?;
        let begin = r.u64()?;
        let end = begin.wrapping_add(r.u64()?);
        let status = r.u8()?;
        let kind = r.u8()?;
        let parent = r.u64()?;
        varint::put_i64(&mut self.c[C_ID], id.wrapping_sub(self.prev_id) as i64);
        self.prev_id = id;
        varint::put_u64(&mut self.c[C_GEN], gen as u64);
        varint::put_i64(&mut self.c[C_BEGIN], begin.wrapping_sub(self.prev_begin) as i64);
        self.prev_begin = begin;
        varint::put_u64(&mut self.c[C_DUR], end.wrapping_sub(begin));
        self.c[C_STATUS].push((kind << 4) | (status & 15));
        if parent == 0 {
            varint::put_u64(&mut self.c[C_PARENT], 0);
        } else {
            varint::put_u64(&mut self.c[C_PARENT], (varint::zigzag(id.wrapping_sub(parent - 1) as i64) << 1) | 1);
        }
        self.put_row_attrs(r)?;
        let ne = r.usize()?;
        varint::put_u64(&mut self.c[C_ECOUNT], ne as u64);
        for _ in 0..ne {
            let time = r.u64()?;
            let name = r.u64()?;
            varint::put_i64(&mut self.c[C_ETIME], time.wrapping_sub(begin) as i64);
            varint::put_u64(&mut self.c[C_ENAME], name);
            self.put_row_attrs(r)?;
        }
        let ns = r.usize()?;
        varint::put_u64(&mut self.c[C_SCOUNT], ns as u64);
        for _ in 0..ns {
            let name = r.u64()?;
            let lane = r.u64()?;
            let sbegin = r.u64()?;
            let send = r.u64()?; // 0 = open, else duration + 1
            varint::put_u64(&mut self.c[C_SNAME], name);
            varint::put_u64(&mut self.c[C_SLANE], lane);
            varint::put_i64(&mut self.c[C_SBEGIN], sbegin.wrapping_sub(begin) as i64);
            varint::put_u64(&mut self.c[C_SEND], send);
            self.put_row_attrs(r)?;
        }
        Ok((id, gen, begin, end))
    }

    /// Transcodes one relation row. Returns (from, to).
    fn put_rel_row(&mut self, r: &mut Reader) -> Result<(u64, u64)> {
        let kind = r.u64()?;
        let from = r.u64()?;
        let to = r.u64()?;
        varint::put_u64(&mut self.c[C_RKIND], kind);
        varint::put_i64(&mut self.c[C_RFROM], from.wrapping_sub(self.prev_from) as i64);
        self.prev_from = from;
        varint::put_i64(&mut self.c[C_RTO], to.wrapping_sub(from) as i64);
        self.put_row_attrs(r)?;
        Ok((from, to))
    }

    #[allow(dead_code)]
    fn put_tx(&mut self, tx: &Transaction) {
        varint::put_i64(&mut self.c[C_ID], tx.id.wrapping_sub(self.prev_id) as i64);
        self.prev_id = tx.id;
        varint::put_u64(&mut self.c[C_GEN], tx.generator.0 as u64);
        varint::put_i64(&mut self.c[C_BEGIN], tx.begin.wrapping_sub(self.prev_begin) as i64);
        self.prev_begin = tx.begin;
        varint::put_u64(&mut self.c[C_DUR], tx.end.wrapping_sub(tx.begin));
        self.c[C_STATUS].push((tx.kind as u8) << 4 | tx.status as u8);
        match tx.parent {
            None => varint::put_u64(&mut self.c[C_PARENT], 0),
            Some(p) => varint::put_u64(&mut self.c[C_PARENT], (varint::zigzag(tx.id.wrapping_sub(p) as i64) << 1) | 1),
        }
        self.put_attrs(&tx.attrs);
        varint::put_u64(&mut self.c[C_ECOUNT], tx.events.len() as u64);
        for e in &tx.events {
            varint::put_i64(&mut self.c[C_ETIME], e.time.wrapping_sub(tx.begin) as i64);
            varint::put_u64(&mut self.c[C_ENAME], e.name.0 as u64);
            self.put_kv(&e.attrs);
        }
        varint::put_u64(&mut self.c[C_SCOUNT], tx.stages.len() as u64);
        for s in &tx.stages {
            varint::put_u64(&mut self.c[C_SNAME], s.name.0 as u64);
            varint::put_u64(&mut self.c[C_SLANE], s.lane.0 as u64);
            varint::put_i64(&mut self.c[C_SBEGIN], s.begin.wrapping_sub(tx.begin) as i64);
            varint::put_u64(&mut self.c[C_SEND], s.end.map(|e| e.wrapping_sub(s.begin) + 1).unwrap_or(0));
            self.put_kv(&s.attrs);
        }
    }

    #[allow(dead_code)]
    fn put_rel(&mut self, r: &Relation) {
        varint::put_u64(&mut self.c[C_RKIND], r.kind.0 as u64);
        varint::put_i64(&mut self.c[C_RFROM], r.from.wrapping_sub(self.prev_from) as i64);
        self.prev_from = r.from;
        varint::put_i64(&mut self.c[C_RTO], r.to.wrapping_sub(r.from) as i64);
        self.put_kv(&r.attrs);
    }

    fn serialize(&self, out: &mut Vec<u8>) {
        varint::put_u64(out, N_COLS as u64);
        for c in &self.c {
            varint::put_u64(out, c.len() as u64);
        }
        for c in &self.c {
            out.extend_from_slice(c);
        }
    }
}

/// Input for one transaction block: rows in the writer's row format.
pub struct TxBlockInput {
    pub tx_rows: Vec<u8>,
    pub n_tx: u64,
    pub rel_rows: Vec<u8>,
    pub n_rel: u64,
}

/// Encodes a transaction block payload.
pub fn encode_tx_block(input: &TxBlockInput, comp: Compression, compressor: &mut Compressor, out: &mut Vec<u8>) -> Result<()> {
    let mut cols = Columns::new();
    let mut h = TxBlockHeader {
        n_tx: input.n_tx,
        n_rel: input.n_rel,
        min_id: u64::MAX,
        max_id: 0,
        t_min: u64::MAX,
        t_max: 0,
        rel_min_from: u64::MAX,
        rel_max_from: 0,
        rel_min_to: u64::MAX,
        rel_max_to: 0,
        n_gens: 0,
        blob_len: 0,
    };
    let mut gens: Vec<u32> = Vec::new();
    let mut last_gen = u32::MAX;
    let mut r = Reader::new(&input.tx_rows);
    for _ in 0..input.n_tx {
        let (id, gen, begin, end) = cols.put_tx_row(&mut r)?;
        h.min_id = h.min_id.min(id);
        h.max_id = h.max_id.max(id);
        h.t_min = h.t_min.min(begin);
        h.t_max = h.t_max.max(end);
        if gen != last_gen {
            last_gen = gen;
            if !gens.contains(&gen) {
                gens.push(gen);
            }
        }
    }
    let mut r = Reader::new(&input.rel_rows);
    for _ in 0..input.n_rel {
        let (from, to) = cols.put_rel_row(&mut r)?;
        h.rel_min_from = h.rel_min_from.min(from);
        h.rel_max_from = h.rel_max_from.max(from);
        h.rel_min_to = h.rel_min_to.min(to);
        h.rel_max_to = h.rel_max_to.max(to);
    }
    if input.n_tx == 0 {
        h.min_id = 0;
        h.t_min = 0;
    }
    if input.n_rel == 0 {
        h.rel_min_from = 0;
        h.rel_min_to = 0;
    }
    gens.sort_unstable();
    h.n_gens = gens.len() as u32;
    let mut raw = Vec::new();
    cols.serialize(&mut raw);
    let mut blob = Vec::new();
    compressor.compress_into(comp, &raw, &mut blob)?;
    h.blob_len = blob.len() as u32;
    h.write(out);
    for g in gens {
        out.extend_from_slice(&g.to_le_bytes());
    }
    out.extend_from_slice(&blob);
    Ok(())
}

/// Decoded contents of one transaction block.
#[derive(Debug, Default)]
pub struct TxBlockData {
    pub transactions: Vec<Transaction>,
    pub relations: Vec<Relation>,
}

struct ColReaders<'a> {
    r: Vec<Reader<'a>>,
}

impl<'a> ColReaders<'a> {
    fn attr_value(&mut self, tagbyte: u8) -> Result<(AttrPhase, Value)> {
        let phase = AttrPhase::from_u8(tagbyte >> 5);
        let tag = ValueTag::from_u8(tagbyte & 31)?;
        let v = match tag {
            ValueTag::Null => Value::Null,
            ValueTag::Bool => Value::Bool(self.r[C_ANUM].u8()? != 0),
            ValueTag::I64 => Value::I64(self.r[C_ANUM].i64()?),
            ValueTag::U64 => Value::U64(self.r[C_ANUM].u64()?),
            ValueTag::Time => Value::Time(self.r[C_ANUM].u64()?),
            ValueTag::Pointer => Value::Pointer(self.r[C_ANUM].u64()?),
            ValueTag::F64 => Value::F64(self.r[C_AF64].f64()?),
            ValueTag::Str => Value::Str(StrId(self.r[C_ASTR].u32()?)),
            ValueTag::Enum => {
                let value = self.r[C_ANUM].i64()?;
                let name = StrId(self.r[C_ASTR].u32()?);
                Value::Enum { value, name }
            }
            ValueTag::Fixed => {
                let raw = self.r[C_ANUM].i64()?;
                let scale = self.r[C_ANUM].i64()? as i32;
                Value::Fixed { raw, scale }
            }
            ValueTag::UFixed => {
                let raw = self.r[C_ANUM].u64()?;
                let scale = self.r[C_ANUM].i64()? as i32;
                Value::UFixed { raw, scale }
            }
            other => Value::decode_payload(other, &mut self.r[C_AMISC])?,
        };
        Ok((phase, v))
    }

    fn attrs(&mut self) -> Result<Vec<TxAttr>> {
        let n = self.r[C_ACOUNT].usize()?;
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let key = StrId(self.r[C_AKEY].u32()?);
            let tb = self.r[C_ATAG].u8()?;
            let (phase, value) = self.attr_value(tb)?;
            v.push(TxAttr { key, phase, value });
        }
        Ok(v)
    }

    fn kv(&mut self) -> Result<Vec<(StrId, Value)>> {
        Ok(self.attrs()?.into_iter().map(|a| (a.key, a.value)).collect())
    }
}

/// Decodes a transaction block payload.
pub fn decode_tx_block(p: &[u8], d: &mut Decompressor) -> Result<TxBlockData> {
    let h = TxBlockHeader::parse(p)?;
    let raw = d.decompress(&p[h.blob_off()..h.blob_off() + h.blob_len as usize])?;
    let mut r = Reader::new(&raw);
    let ncols = r.usize()?;
    if ncols < N_COLS {
        return Err(Error::Corrupt("tx block has too few columns"));
    }
    let mut lens = Vec::with_capacity(ncols);
    for _ in 0..ncols {
        lens.push(r.usize()?);
    }
    let mut pos = r.pos;
    let mut readers = Vec::with_capacity(ncols);
    for l in lens {
        if pos + l > raw.len() {
            return Err(Error::Corrupt("tx column exceeds blob"));
        }
        readers.push(Reader::new(&raw[pos..pos + l]));
        pos += l;
    }
    let mut c = ColReaders { r: readers };
    let mut out = TxBlockData::default();
    out.transactions.reserve(h.n_tx as usize);
    let (mut prev_id, mut prev_begin, mut prev_from) = (0u64, 0u64, 0u64);
    for _ in 0..h.n_tx {
        let id = prev_id.wrapping_add(c.r[C_ID].i64()? as u64);
        prev_id = id;
        let generator = NodeId(c.r[C_GEN].u32()?);
        let begin = prev_begin.wrapping_add(c.r[C_BEGIN].i64()? as u64);
        prev_begin = begin;
        let end = begin.wrapping_add(c.r[C_DUR].u64()?);
        let sb = c.r[C_STATUS].u8()?;
        let status = TxStatus::from_u8(sb & 15);
        let kind = TxKind::from_u8(sb >> 4);
        let parent = match c.r[C_PARENT].u64()? {
            0 => None,
            v => Some(id.wrapping_sub(varint::unzigzag(v >> 1) as u64)),
        };
        let attrs = c.attrs()?;
        let ne = c.r[C_ECOUNT].usize()?;
        let mut events = Vec::with_capacity(ne.min(1024));
        for _ in 0..ne {
            let time = begin.wrapping_add(c.r[C_ETIME].i64()? as u64);
            let name = StrId(c.r[C_ENAME].u32()?);
            let attrs = c.kv()?;
            events.push(TxEvent { time, name, attrs });
        }
        let ns = c.r[C_SCOUNT].usize()?;
        let mut stages = Vec::with_capacity(ns.min(1024));
        for _ in 0..ns {
            let name = StrId(c.r[C_SNAME].u32()?);
            let lane = StrId(c.r[C_SLANE].u32()?);
            let sbegin = begin.wrapping_add(c.r[C_SBEGIN].i64()? as u64);
            let send = match c.r[C_SEND].u64()? {
                0 => None,
                dd => Some(sbegin.wrapping_add(dd - 1)),
            };
            let attrs = c.kv()?;
            stages.push(TxStage { name, lane, begin: sbegin, end: send, attrs });
        }
        out.transactions.push(Transaction { id, generator, begin, end, status, kind, parent, attrs, events, stages });
    }
    out.relations.reserve(h.n_rel as usize);
    for _ in 0..h.n_rel {
        let kind = StrId(c.r[C_RKIND].u32()?);
        let from = prev_from.wrapping_add(c.r[C_RFROM].i64()? as u64);
        prev_from = from;
        let to = from.wrapping_add(c.r[C_RTO].i64()? as u64);
        let attrs = c.kv()?;
        out.relations.push(Relation { kind, from, to, attrs });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_block() {
        let txs = vec![
            Transaction {
                id: 5,
                generator: NodeId(3),
                begin: 100,
                end: 150,
                status: TxStatus::Ok,
                kind: TxKind::Client,
                parent: None,
                attrs: vec![
                    TxAttr { key: StrId(1), phase: AttrPhase::Begin, value: Value::U64(42) },
                    TxAttr { key: StrId(2), phase: AttrPhase::End, value: Value::F64(1.5) },
                    TxAttr { key: StrId(3), phase: AttrPhase::Record, value: Value::List(vec![Value::Bool(true)]) },
                ],
                events: vec![TxEvent { time: 120, name: StrId(7), attrs: vec![(StrId(8), Value::Str(StrId(9)))] }],
                stages: vec![
                    TxStage { name: StrId(10), lane: StrId(11), begin: 100, end: Some(110), attrs: vec![] },
                    TxStage { name: StrId(12), lane: StrId(11), begin: 110, end: None, attrs: vec![(StrId(1), Value::Null)] },
                ],
            },
            Transaction {
                id: 3,
                generator: NodeId(4),
                begin: 90,
                end: 200,
                status: TxStatus::Aborted,
                kind: TxKind::Unspecified,
                parent: Some(5),
                attrs: vec![],
                events: vec![],
                stages: vec![],
            },
        ];
        let rels = vec![Relation { kind: StrId(20), from: 5, to: 3, attrs: vec![(StrId(21), Value::I64(-1))] }];
        let mut input = TxBlockInput { tx_rows: vec![], n_tx: 2, rel_rows: vec![], n_rel: 1 };
        for t in &txs {
            encode_tx_row(t, &mut input.tx_rows);
        }
        encode_rel_row(&rels[0], &mut input.rel_rows);
        let mut out = Vec::new();
        encode_tx_block(&input, Compression::ZSTD_FAST, &mut Compressor::new(), &mut out).unwrap();
        let h = TxBlockHeader::parse(&out).unwrap();
        assert_eq!((h.min_id, h.max_id, h.t_min, h.t_max), (3, 5, 90, 200));
        assert_eq!(block_generators(&out, &h).collect::<Vec<_>>(), vec![3, 4]);
        let d = decode_tx_block(&out, &mut Decompressor::new()).unwrap();
        assert_eq!(d.transactions, txs);
        assert_eq!(d.relations, rels);
    }
}
