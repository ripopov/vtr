//! Log blocks: timestamped log records stored column-wise per block.
//!
//! A log record is a zero-duration transaction of a generator that describes
//! a *log site*: the format string, severity, source location and the
//! argument types of one logging call site (`LOG_INFO("addr={:#x}", a)`).
//! Because the argument types are declared once on the generator, a record
//! stores only the argument values: no attribute keys, no tags. Text
//! arguments are deduplicated in a per-block dictionary at encode time (off
//! the caller's thread), so the hot path is a plain copy of the values.
//!
//! The design borrows from NanoLog / binlog (static call-site metadata
//! written once, dynamic arguments copied raw) and from CLP (a dictionary
//! for repeated string variables, integers kept as integers), see
//! `docs/RATIONALE.md`.

use crate::codec::{Compression, Compressor, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::NodeId;
use crate::strings::StrId;
use crate::txblock::{AttrPhase, Transaction, TxAttr, TxId, TxKind, TxStatus};
use crate::value::{Value, ValueTag};
use crate::varint::{self, Reader};
use crate::logfmt::ParsedFmt;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

/// Multiply-xorshift hasher for the per-block text dictionary (short byte
/// strings, no adversarial input): several times faster than SipHash.
#[derive(Default)]
struct FastHasher(u64);

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut h = self.0 ^ (bytes.len() as u64).wrapping_mul(K);
        let mut b = bytes;
        while b.len() >= 8 {
            let w = u64::from_le_bytes(b[..8].try_into().unwrap());
            h = (h ^ w).wrapping_mul(K).rotate_left(29);
            b = &b[8..];
        }
        if !b.is_empty() {
            let mut tail = [0u8; 8];
            tail[..b.len()].copy_from_slice(b);
            h = (h ^ u64::from_le_bytes(tail)).wrapping_mul(K).rotate_left(29);
        }
        self.0 = h;
    }
    #[inline]
    fn finish(&self) -> u64 {
        let h = self.0;
        (h ^ (h >> 32)).wrapping_mul(0xD6E8_FEB8_6659_FD93)
    }
}

type FastMap<'a> = HashMap<&'a [u8], u32, BuildHasherDefault<FastHasher>>;

// ---------------------------------------------------------------------------
// Static site description
// ---------------------------------------------------------------------------

/// Severity of a log site (generator attribute `log.severity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    /// Producer-specific level; ranks by its code.
    Other(u8),
}

impl Severity {
    pub fn code(self) -> u8 {
        match self {
            Severity::Trace => 0,
            Severity::Debug => 1,
            Severity::Info => 2,
            Severity::Warn => 3,
            Severity::Error => 4,
            Severity::Fatal => 5,
            Severity::Other(c) => c,
        }
    }
    pub fn from_code(c: u8) -> Severity {
        match c {
            0 => Severity::Trace,
            1 => Severity::Debug,
            2 => Severity::Info,
            3 => Severity::Warn,
            4 => Severity::Error,
            5 => Severity::Fatal,
            c => Severity::Other(c),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Severity::Trace => "trace",
            Severity::Debug => "debug",
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
            Severity::Other(_) => "other",
        }
    }
    /// Parses `trace|debug|info|warn|warning|error|fatal` (case-insensitive) or a number.
    pub fn from_name(s: &str) -> Option<Severity> {
        Some(match s.to_ascii_lowercase().as_str() {
            "trace" => Severity::Trace,
            "debug" => Severity::Debug,
            "info" => Severity::Info,
            "warn" | "warning" => Severity::Warn,
            "error" => Severity::Error,
            "fatal" => Severity::Fatal,
            other => Severity::from_code(other.parse::<u8>().ok()?),
        })
    }
}

impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.code().cmp(&other.code())
    }
}

/// Argument types a log site declares (a subset of the value tags).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LogArgType {
    Bool = ValueTag::Bool as u8,
    I64 = ValueTag::I64 as u8,
    U64 = ValueTag::U64 as u8,
    F64 = ValueTag::F64 as u8,
    /// Interned string id (the producer interned it; stable across the file).
    Str = ValueTag::Str as u8,
    /// Opaque bytes, stored verbatim.
    Bytes = ValueTag::Bytes as u8,
    Time = ValueTag::Time as u8,
    Pointer = ValueTag::Pointer as u8,
    /// UTF-8 text, deduplicated per block (the common case for names, states, ids).
    Text = ValueTag::Text as u8,
}

impl LogArgType {
    pub fn from_u8(v: u8) -> Result<LogArgType> {
        Ok(match v {
            1 => LogArgType::Bool,
            2 => LogArgType::I64,
            3 => LogArgType::U64,
            4 => LogArgType::F64,
            5 => LogArgType::Str,
            6 => LogArgType::Bytes,
            10 => LogArgType::Time,
            12 => LogArgType::Pointer,
            17 => LogArgType::Text,
            _ => return Err(Error::Corrupt("unknown log argument type")),
        })
    }
    pub fn tag(self) -> ValueTag {
        ValueTag::from_u8(self as u8).unwrap()
    }
    pub fn name(self) -> &'static str {
        match self {
            LogArgType::Bool => "bool",
            LogArgType::I64 => "i64",
            LogArgType::U64 => "u64",
            LogArgType::F64 => "f64",
            LogArgType::Str => "str",
            LogArgType::Bytes => "bytes",
            LogArgType::Time => "time",
            LogArgType::Pointer => "pointer",
            LogArgType::Text => "text",
        }
    }
}

/// One argument of a log record. Borrowed: logging allocates nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogArg<'a> {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(StrId),
    Bytes(&'a [u8]),
    Time(u64),
    Pointer(u64),
    Text(&'a str),
}

impl<'a> LogArg<'a> {
    pub fn arg_type(&self) -> LogArgType {
        match self {
            LogArg::Bool(_) => LogArgType::Bool,
            LogArg::I64(_) => LogArgType::I64,
            LogArg::U64(_) => LogArgType::U64,
            LogArg::F64(_) => LogArgType::F64,
            LogArg::Str(_) => LogArgType::Str,
            LogArg::Bytes(_) => LogArgType::Bytes,
            LogArg::Time(_) => LogArgType::Time,
            LogArg::Pointer(_) => LogArgType::Pointer,
            LogArg::Text(_) => LogArgType::Text,
        }
    }
    /// Owned attribute value (used when a record is viewed as a transaction).
    pub fn to_value(&self) -> Value {
        match *self {
            LogArg::Bool(b) => Value::Bool(b),
            LogArg::I64(v) => Value::I64(v),
            LogArg::U64(v) => Value::U64(v),
            LogArg::F64(v) => Value::F64(v),
            LogArg::Str(s) => Value::Str(s),
            LogArg::Bytes(b) => Value::Bytes(b.to_vec()),
            LogArg::Time(v) => Value::Time(v),
            LogArg::Pointer(v) => Value::Pointer(v),
            LogArg::Text(s) => Value::Text(s.to_string()),
        }
    }
}

macro_rules! arg_from {
    ($($t:ty => $variant:ident as $cast:ty),* $(,)?) => {
        $(impl<'a> From<$t> for LogArg<'a> {
            #[inline]
            fn from(v: $t) -> Self { LogArg::$variant(v as $cast) }
        })*
    };
}
arg_from!(i8 => I64 as i64, i16 => I64 as i64, i32 => I64 as i64, i64 => I64 as i64, isize => I64 as i64,
          u8 => U64 as u64, u16 => U64 as u64, u32 => U64 as u64, u64 => U64 as u64, usize => U64 as u64,
          f32 => F64 as f64, f64 => F64 as f64);
impl<'a> From<bool> for LogArg<'a> {
    fn from(v: bool) -> Self {
        LogArg::Bool(v)
    }
}
impl<'a> From<&'a str> for LogArg<'a> {
    fn from(v: &'a str) -> Self {
        LogArg::Text(v)
    }
}
impl<'a> From<&'a String> for LogArg<'a> {
    fn from(v: &'a String) -> Self {
        LogArg::Text(v.as_str())
    }
}
impl<'a> From<&'a [u8]> for LogArg<'a> {
    fn from(v: &'a [u8]) -> Self {
        LogArg::Bytes(v)
    }
}
impl<'a> From<StrId> for LogArg<'a> {
    fn from(v: StrId) -> Self {
        LogArg::Str(v)
    }
}

/// Writer-side handle of a registered log site (dense index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LogSiteId(pub u32);

/// What a producer declares about a call site (see `Writer::add_log_site`).
#[derive(Clone, Debug)]
pub struct LogSiteSpec<'a> {
    /// The `LOG` stream the site belongs to.
    pub stream: NodeId,
    pub severity: Severity,
    /// Format string with `{}` placeholders (`{:x}`, `{:#010x}`, `{:.3}`, `{1}` ...).
    pub fmt: &'a str,
    /// Argument types in placeholder order.
    pub args: &'a [LogArgType],
    /// Optional argument names (attribute keys when the record is read as a
    /// transaction); `"0"`, `"1"`, ... when empty.
    pub names: &'a [&'a str],
    /// Source location; empty / 0 when unknown.
    pub file: &'a str,
    pub line: u32,
    pub func: &'a str,
}

impl<'a> LogSiteSpec<'a> {
    pub fn new(stream: NodeId, severity: Severity, fmt: &'a str, args: &'a [LogArgType]) -> Self {
        LogSiteSpec { stream, severity, fmt, args, names: &[], file: "", line: 0, func: "" }
    }
    pub fn names(mut self, names: &'a [&'a str]) -> Self {
        self.names = names;
        self
    }
    pub fn location(mut self, file: &'a str, line: u32) -> Self {
        self.file = file;
        self.line = line;
        self
    }
    pub fn func(mut self, func: &'a str) -> Self {
        self.func = func;
        self
    }
}

/// Reader-side description of a log site (decoded from the generator's attributes).
#[derive(Clone, Debug, PartialEq)]
pub struct LogSite {
    /// Position in `Reader::log_sites()`.
    pub index: u32,
    pub node: NodeId,
    pub stream: NodeId,
    pub severity: Severity,
    /// Format string (the generator's name).
    pub fmt: StrId,
    pub file: Option<StrId>,
    pub line: Option<u32>,
    pub func: Option<StrId>,
    pub args: Vec<LogArgType>,
    /// One name per argument (always present in files written by this crate).
    pub names: Vec<StrId>,
}

/// Attribute keys used on log-site generators.
pub const KEY_SEVERITY: &str = "log.severity";
pub const KEY_ARGS: &str = "log.args";
pub const KEY_NAMES: &str = "log.names";
pub const KEY_FILE: &str = "log.file";
pub const KEY_LINE: &str = "log.line";
pub const KEY_FUNC: &str = "log.func";
/// Stream kind of log streams.
pub const STREAM_KIND: &str = "LOG";

// ---------------------------------------------------------------------------
// Row format (writer scratch and block input)
// ---------------------------------------------------------------------------

/// Appends one record in row form: `site, time, id, parent(0/id+1), values...`.
#[inline]
pub fn encode_row(out: &mut Vec<u8>, site: u32, time: u64, id: u64, parent: u64, args: &[LogArg]) {
    varint::put_u64(out, site as u64);
    varint::put_u64(out, time);
    varint::put_u64(out, id);
    varint::put_u64(out, parent);
    for a in args {
        match *a {
            LogArg::Bool(b) => out.push(b as u8),
            LogArg::I64(v) => varint::put_i64(out, v),
            LogArg::U64(v) | LogArg::Time(v) | LogArg::Pointer(v) => varint::put_u64(out, v),
            LogArg::F64(v) => out.extend_from_slice(&v.to_le_bytes()),
            LogArg::Str(s) => varint::put_u64(out, s.0 as u64),
            LogArg::Bytes(b) => varint::put_blob(out, b),
            LogArg::Text(s) => varint::put_blob(out, s.as_bytes()),
        }
    }
}

/// Per-site facts the block encoder needs.
#[derive(Clone, Debug)]
pub struct LogSiteEnc {
    pub node: u32,
    pub args: Vec<LogArgType>,
}

/// Input for one log block: rows plus a snapshot of the site table.
pub struct LogBlockInput {
    pub rows: Vec<u8>,
    pub n: u64,
    pub sites: Arc<Vec<LogSiteEnc>>,
}

// ---------------------------------------------------------------------------
// Block encoding (columnar)
// ---------------------------------------------------------------------------

pub const LOG_HEADER_LEN: usize = 48;

/// Fixed header of a log block.
#[derive(Clone, Copy, Debug, Default)]
pub struct LogBlockHeader {
    pub n_rec: u64,
    pub min_id: u64,
    pub max_id: u64,
    pub t_min: u64,
    pub t_max: u64,
    pub n_gens: u32,
    pub blob_len: u32,
}

impl LogBlockHeader {
    pub fn parse(p: &[u8]) -> Result<LogBlockHeader> {
        if p.len() < LOG_HEADER_LEN {
            return Err(Error::Corrupt("log block header truncated"));
        }
        let u64_at = |o: usize| u64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        let h = LogBlockHeader {
            n_rec: u64_at(0),
            min_id: u64_at(8),
            max_id: u64_at(16),
            t_min: u64_at(24),
            t_max: u64_at(32),
            n_gens: u32_at(40),
            blob_len: u32_at(44),
        };
        if h.blob_off() + h.blob_len as usize > p.len() {
            return Err(Error::Corrupt("log block truncated"));
        }
        Ok(h)
    }
    pub fn gens_off(&self) -> usize {
        LOG_HEADER_LEN
    }
    pub fn blob_off(&self) -> usize {
        LOG_HEADER_LEN + self.n_gens as usize * 4
    }
    fn write(&self, out: &mut Vec<u8>) {
        for v in [self.n_rec, self.min_id, self.max_id, self.t_min, self.t_max] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.n_gens.to_le_bytes());
        out.extend_from_slice(&self.blob_len.to_le_bytes());
    }
}

/// Generator ids present in a block, in order of first occurrence.
pub fn block_generators<'a>(p: &'a [u8], h: &LogBlockHeader) -> impl Iterator<Item = u32> + 'a {
    let base = h.gens_off();
    (0..h.n_gens as usize).map(move |i| u32::from_le_bytes(p[base + i * 4..base + i * 4 + 4].try_into().unwrap()))
}

const C_GEN: usize = 0;
const C_TIME: usize = 1;
const C_ID: usize = 2;
const C_PARENT: usize = 3;
const C_NUM: usize = 4;
const C_F64: usize = 5;
const C_TEXT: usize = 6;
const C_DICT: usize = 7;
const C_MISC: usize = 8;
const N_COLS: usize = 9;

/// Encodes a log block payload.
pub fn encode_log_block(input: &LogBlockInput, comp: Compression, compressor: &mut Compressor, out: &mut Vec<u8>) -> Result<()> {
    let n = input.n as usize;
    // Columns as separate vectors (no per-push indexing), pre-sized from the row volume.
    let mut c_gen: Vec<u8> = Vec::with_capacity(n + 16);
    let mut c_time: Vec<u8> = Vec::with_capacity(2 * n + 16);
    let mut c_id: Vec<u8> = Vec::with_capacity(n + 16);
    let mut c_parent: Vec<u8> = Vec::with_capacity(n + 16);
    let mut c_num: Vec<u8> = Vec::with_capacity(input.rows.len());
    let mut c_f64: Vec<u8> = Vec::new();
    let mut c_text: Vec<u8> = Vec::with_capacity(n + 16);
    let mut c_misc: Vec<u8> = Vec::new();
    let mut h = LogBlockHeader { n_rec: input.n, min_id: u64::MAX, max_id: 0, t_min: u64::MAX, t_max: 0, n_gens: 0, blob_len: 0 };
    let mut gens: Vec<u32> = Vec::new();
    // site index -> local generator index (u32::MAX = not seen yet)
    let mut local: Vec<u32> = vec![u32::MAX; input.sites.len()];
    let mut dict: FastMap = FastMap::default();
    let mut n_dict = 0u32;
    let mut dict_col: Vec<u8> = Vec::new();
    let (mut prev_t, mut prev_id) = (0u64, 0u64);
    let mut r = Reader::new(&input.rows);
    let sites: &[LogSiteEnc] = &input.sites;
    for _ in 0..n {
        let site = r.usize()?;
        let time = r.u64()?;
        let id = r.u64()?;
        let parent = r.u64()?;
        let s = sites.get(site).ok_or(Error::Corrupt("log row references an unknown site"))?;
        let li = local[site];
        let li = if li == u32::MAX {
            let l = gens.len() as u32;
            local[site] = l;
            gens.push(s.node);
            l
        } else {
            li
        };
        varint::put_u64(&mut c_gen, li as u64);
        varint::put_i64(&mut c_time, time.wrapping_sub(prev_t) as i64);
        prev_t = time;
        varint::put_i64(&mut c_id, id.wrapping_sub(prev_id) as i64);
        prev_id = id;
        if parent == 0 {
            c_parent.push(0);
        } else {
            varint::put_u64(&mut c_parent, (varint::zigzag(id.wrapping_sub(parent - 1) as i64) << 1) | 1);
        }
        h.min_id = h.min_id.min(id);
        h.max_id = h.max_id.max(id);
        h.t_min = h.t_min.min(time);
        h.t_max = h.t_max.max(time);
        for t in &s.args {
            match t {
                LogArgType::Bool => c_num.push(r.u8()?),
                LogArgType::I64 | LogArgType::U64 | LogArgType::Time | LogArgType::Pointer => {
                    // Already LEB128 in the row (zig-zag for I64): copy the encoded bytes.
                    let start = r.pos;
                    let _ = r.u64()?;
                    c_num.extend_from_slice(&r.buf[start..r.pos]);
                }
                LogArgType::F64 => c_f64.extend_from_slice(r.bytes(8)?),
                LogArgType::Str => {
                    let start = r.pos;
                    let _ = r.u64()?;
                    c_misc.extend_from_slice(&r.buf[start..r.pos]);
                }
                LogArgType::Bytes => {
                    let b = r.blob()?;
                    varint::put_blob(&mut c_misc, b);
                }
                LogArgType::Text => {
                    let b = r.blob()?;
                    let idx = *dict.entry(b).or_insert_with(|| {
                        varint::put_blob(&mut dict_col, b);
                        n_dict += 1;
                        n_dict - 1
                    });
                    varint::put_u64(&mut c_text, idx as u64);
                }
            }
        }
    }
    if input.n == 0 {
        h.min_id = 0;
        h.t_min = 0;
    }
    let mut c_dict: Vec<u8> = Vec::with_capacity(dict_col.len() + 8);
    varint::put_u64(&mut c_dict, n_dict as u64);
    c_dict.extend_from_slice(&dict_col);
    h.n_gens = gens.len() as u32;
    let cols: [&[u8]; N_COLS] = [&c_gen, &c_time, &c_id, &c_parent, &c_num, &c_f64, &c_text, &c_dict, &c_misc];
    let total: usize = cols.iter().map(|c| c.len()).sum();
    let mut raw = Vec::with_capacity(total + 64);
    varint::put_u64(&mut raw, N_COLS as u64);
    for col in &cols {
        varint::put_u64(&mut raw, col.len() as u64);
    }
    for col in &cols {
        raw.extend_from_slice(col);
    }
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

// ---------------------------------------------------------------------------
// Decoded block
// ---------------------------------------------------------------------------

/// One decoded record (argument values live in the block's cell array).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogRec {
    pub id: u64,
    pub time: u64,
    /// Generator node id of the site.
    pub gen: u32,
    /// 0 = none, else parent id + 1.
    pub parent: u64,
    first: u32,
    n_args: u32,
}

#[derive(Clone, Copy, Debug)]
enum Cell {
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(u32),
    Time(u64),
    Pointer(u64),
    Text(u32),
    Bytes(u32, u32),
}

/// Decoded contents of one log block.
#[derive(Debug, Default)]
pub struct LogBlockData {
    pub recs: Vec<LogRec>,
    cells: Vec<Cell>,
    dict: Vec<u8>,
    /// `dict_off[i]..dict_off[i+1]` is dictionary string i.
    dict_off: Vec<u32>,
    bytes: Vec<u8>,
}

impl LogBlockData {
    /// Number of arguments of `rec`.
    pub fn arg_count(&self, rec: &LogRec) -> usize {
        rec.n_args as usize
    }
    /// Argument `i` of `rec`.
    pub fn arg(&self, rec: &LogRec, i: usize) -> Option<LogArg<'_>> {
        if i >= rec.n_args as usize {
            return None;
        }
        Some(match self.cells[rec.first as usize + i] {
            Cell::Bool(b) => LogArg::Bool(b),
            Cell::I64(v) => LogArg::I64(v),
            Cell::U64(v) => LogArg::U64(v),
            Cell::F64(v) => LogArg::F64(v),
            Cell::Str(s) => LogArg::Str(StrId(s)),
            Cell::Time(v) => LogArg::Time(v),
            Cell::Pointer(v) => LogArg::Pointer(v),
            Cell::Text(d) => {
                let (a, b) = (self.dict_off[d as usize] as usize, self.dict_off[d as usize + 1] as usize);
                // Validated UTF-8 in decode.
                LogArg::Text(unsafe { std::str::from_utf8_unchecked(&self.dict[a..b]) })
            }
            Cell::Bytes(off, len) => LogArg::Bytes(&self.bytes[off as usize..(off + len) as usize]),
        })
    }
    pub fn args<'a>(&'a self, rec: &'a LogRec) -> impl Iterator<Item = LogArg<'a>> + 'a {
        (0..rec.n_args as usize).map(move |i| self.arg(rec, i).unwrap())
    }
}

/// Decodes a log block. `types(gen)` returns the declared argument types of a
/// generator (from the hierarchy); unknown generators are an error.
pub fn decode_log_block<'t>(p: &[u8], d: &mut Decompressor, types: impl Fn(u32) -> Option<&'t [LogArgType]>) -> Result<LogBlockData> {
    let h = LogBlockHeader::parse(p)?;
    let gens: Vec<u32> = block_generators(p, &h).collect();
    let gen_types: Vec<&[LogArgType]> = gens
        .iter()
        .map(|&g| types(g).ok_or(Error::Corrupt("log block references a generator without log.args")))
        .collect::<Result<_>>()?;
    let raw = d.decompress(&p[h.blob_off()..h.blob_off() + h.blob_len as usize])?;
    let mut r = Reader::new(&raw);
    let ncols = r.usize()?;
    if ncols < N_COLS {
        return Err(Error::Corrupt("log block has too few columns"));
    }
    let mut lens = Vec::with_capacity(ncols);
    for _ in 0..ncols {
        lens.push(r.usize()?);
    }
    let mut pos = r.pos;
    let mut cols = Vec::with_capacity(ncols);
    for l in lens {
        if pos + l > raw.len() {
            return Err(Error::Corrupt("log column exceeds blob"));
        }
        cols.push(Reader::new(&raw[pos..pos + l]));
        pos += l;
    }
    let mut out = LogBlockData::default();
    // Dictionary.
    let n_dict = cols[C_DICT].usize()?;
    if n_dict > cols[C_DICT].remaining() {
        return Err(Error::Corrupt("log dictionary count exceeds data"));
    }
    out.dict_off.reserve(n_dict + 1);
    out.dict_off.push(0);
    for _ in 0..n_dict {
        let s = cols[C_DICT].blob()?;
        std::str::from_utf8(s).map_err(|_| Error::Corrupt("log text is not UTF-8"))?;
        out.dict.extend_from_slice(s);
        out.dict_off.push(out.dict.len() as u32);
    }
    out.recs.reserve(h.n_rec as usize);
    let (mut prev_t, mut prev_id) = (0u64, 0u64);
    for _ in 0..h.n_rec {
        let li = cols[C_GEN].usize()?;
        let gen = *gens.get(li).ok_or(Error::Corrupt("log generator index out of range"))?;
        let time = prev_t.wrapping_add(cols[C_TIME].i64()? as u64);
        prev_t = time;
        let id = prev_id.wrapping_add(cols[C_ID].i64()? as u64);
        prev_id = id;
        let parent = match cols[C_PARENT].u64()? {
            0 => 0,
            v => id.wrapping_sub(varint::unzigzag(v >> 1) as u64) + 1,
        };
        let first = out.cells.len() as u32;
        let ts = gen_types[li];
        for t in ts {
            out.cells.push(match t {
                LogArgType::Bool => Cell::Bool(cols[C_NUM].u8()? != 0),
                LogArgType::I64 => Cell::I64(cols[C_NUM].i64()?),
                LogArgType::U64 => Cell::U64(cols[C_NUM].u64()?),
                LogArgType::Time => Cell::Time(cols[C_NUM].u64()?),
                LogArgType::Pointer => Cell::Pointer(cols[C_NUM].u64()?),
                LogArgType::F64 => Cell::F64(cols[C_F64].f64()?),
                LogArgType::Str => Cell::Str(cols[C_MISC].u32()?),
                LogArgType::Bytes => {
                    let b = cols[C_MISC].blob()?;
                    let off = out.bytes.len() as u32;
                    out.bytes.extend_from_slice(b);
                    Cell::Bytes(off, b.len() as u32)
                }
                LogArgType::Text => {
                    let d = cols[C_TEXT].u32()?;
                    if d as usize >= n_dict {
                        return Err(Error::Corrupt("log text index out of range"));
                    }
                    Cell::Text(d)
                }
            });
        }
        out.recs.push(LogRec { id, time, gen, parent, first, n_args: ts.len() as u32 });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reader-facing record view
// ---------------------------------------------------------------------------

/// One log record as returned by the reader (borrows the decoded block).
#[derive(Clone, Copy)]
pub struct LogRecord<'a> {
    pub id: TxId,
    pub time: u64,
    pub parent: Option<TxId>,
    pub site: &'a LogSite,
    pub(crate) fmt: &'a ParsedFmt,
    rec: &'a LogRec,
    block: &'a LogBlockData,
}

impl<'a> LogRecord<'a> {
    pub(crate) fn new(site: &'a LogSite, fmt: &'a ParsedFmt, rec: &'a LogRec, block: &'a LogBlockData) -> Self {
        LogRecord { id: rec.id, time: rec.time, parent: if rec.parent == 0 { None } else { Some(rec.parent - 1) }, site, fmt, rec, block }
    }
    pub fn severity(&self) -> Severity {
        self.site.severity
    }
    pub fn arg_count(&self) -> usize {
        self.rec.n_args as usize
    }
    pub fn arg(&self, i: usize) -> Option<LogArg<'a>> {
        self.block.arg(self.rec, i)
    }
    pub fn args(&self) -> impl Iterator<Item = LogArg<'a>> + 'a {
        self.block.args(self.rec)
    }
    /// The record viewed as a zero-duration transaction (attribute keys = argument names).
    pub fn to_transaction(&self) -> Transaction {
        let attrs = self
            .args()
            .enumerate()
            .map(|(i, a)| TxAttr { key: self.site.names.get(i).copied().unwrap_or(StrId(0)), phase: AttrPhase::Record, value: a.to_value() })
            .collect();
        Transaction {
            id: self.id,
            generator: self.site.node,
            begin: self.time,
            end: self.time,
            status: TxStatus::Unset,
            kind: TxKind::Unspecified,
            parent: self.parent,
            attrs,
            events: Vec::new(),
            stages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_block() {
        let sites = Arc::new(vec![
            LogSiteEnc { node: 7, args: vec![LogArgType::U64, LogArgType::Text, LogArgType::I64] },
            LogSiteEnc { node: 3, args: vec![] },
            LogSiteEnc { node: 9, args: vec![LogArgType::F64, LogArgType::Bool, LogArgType::Str, LogArgType::Bytes, LogArgType::Time, LogArgType::Pointer] },
        ]);
        let mut rows = Vec::new();
        encode_row(&mut rows, 0, 100, 1, 0, &[LogArg::U64(0xdead), LogArg::Text("axi0"), LogArg::I64(-5)]);
        encode_row(&mut rows, 1, 100, 2, 2, &[]);
        encode_row(&mut rows, 0, 90, 3, 0, &[LogArg::U64(1), LogArg::Text("axi1"), LogArg::I64(7)]);
        encode_row(&mut rows, 2, 200, 4, 0, &[LogArg::F64(1.5), LogArg::Bool(true), LogArg::Str(StrId(4)), LogArg::Bytes(&[1, 2, 3]), LogArg::Time(9), LogArg::Pointer(0x10)]);
        encode_row(&mut rows, 0, 200, 10, 0, &[LogArg::U64(2), LogArg::Text("axi0"), LogArg::I64(0)]);
        let input = LogBlockInput { rows, n: 5, sites: sites.clone() };
        let mut out = Vec::new();
        encode_log_block(&input, Compression::ZSTD_FAST, &mut Compressor::new(), &mut out).unwrap();
        let h = LogBlockHeader::parse(&out).unwrap();
        assert_eq!((h.n_rec, h.min_id, h.max_id, h.t_min, h.t_max), (5, 1, 10, 90, 200));
        assert_eq!(block_generators(&out, &h).collect::<Vec<_>>(), vec![7, 3, 9]);
        let d = decode_log_block(&out, &mut Decompressor::new(), |g| sites.iter().find(|s| s.node == g).map(|s| s.args.as_slice())).unwrap();
        assert_eq!(d.recs.len(), 5);
        assert_eq!((d.recs[0].id, d.recs[0].time, d.recs[0].gen, d.recs[0].parent), (1, 100, 7, 0));
        assert_eq!((d.recs[1].id, d.recs[1].gen, d.recs[1].parent), (2, 3, 2));
        assert_eq!(d.args(&d.recs[0]).collect::<Vec<_>>(), vec![LogArg::U64(0xdead), LogArg::Text("axi0"), LogArg::I64(-5)]);
        assert_eq!(d.args(&d.recs[2]).collect::<Vec<_>>(), vec![LogArg::U64(1), LogArg::Text("axi1"), LogArg::I64(7)]);
        assert_eq!(
            d.args(&d.recs[3]).collect::<Vec<_>>(),
            vec![LogArg::F64(1.5), LogArg::Bool(true), LogArg::Str(StrId(4)), LogArg::Bytes(&[1, 2, 3]), LogArg::Time(9), LogArg::Pointer(0x10)]
        );
        assert_eq!(d.arg(&d.recs[4], 1), Some(LogArg::Text("axi0")));
        assert_eq!(d.dict_off.len(), 3); // two distinct strings
    }

    #[test]
    fn severity_order_and_names() {
        assert!(Severity::Warn > Severity::Info);
        assert!(Severity::Other(9) > Severity::Fatal);
        assert_eq!(Severity::from_name("WARNING"), Some(Severity::Warn));
        assert_eq!(Severity::from_name("7"), Some(Severity::Other(7)));
        assert_eq!(Severity::from_code(4), Severity::Error);
    }
}
