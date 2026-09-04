//! Small sections: file metadata and blackout (dump on/off) intervals, plus
//! the aux recovery hook used when the directory is missing.

use crate::container::SectionKind;
use crate::error::Result;
use crate::strings::StrId;
use crate::value::{self, Value};
use crate::varint::{self, Reader};

/// Source language / producer class of the trace (FST file types 0..2 kept).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum FileType {
    #[default]
    Verilog = 0,
    Vhdl = 1,
    VerilogVhdl = 2,
    SystemC = 3,
    /// Pipeline / architectural simulator.
    Architectural = 4,
    /// Software tracing (OpenTelemetry etc.).
    Software = 5,
    Other = 255,
}

impl FileType {
    pub fn from_u8(v: u8) -> FileType {
        match v {
            0 => FileType::Verilog,
            1 => FileType::Vhdl,
            2 => FileType::VerilogVhdl,
            3 => FileType::SystemC,
            4 => FileType::Architectural,
            5 => FileType::Software,
            _ => FileType::Other,
        }
    }
}

/// File-level metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    /// Time unit as a power of ten seconds (`-9` = ns, `-12` = ps).
    pub timescale: i8,
    /// Offset added to every timestamp when displaying absolute time (FST "timezero").
    pub time_zero: i64,
    pub file_type: FileType,
    /// Producing tool and version.
    pub writer: String,
    /// Creation date, free form.
    pub date: String,
    pub comment: String,
    /// Signals per value-change group (constant for the whole file).
    pub group_size: u32,
    pub attrs: Vec<(StrId, Value)>,
}

impl Default for Meta {
    fn default() -> Self {
        Meta {
            timescale: -9,
            time_zero: 0,
            file_type: FileType::Verilog,
            writer: String::new(),
            date: String::new(),
            comment: String::new(),
            group_size: 256,
            attrs: Vec::new(),
        }
    }
}

impl Meta {
    pub fn encode(&self, out: &mut Vec<u8>) {
        varint::put_u64(out, 1); // layout version
        varint::put_i64(out, self.timescale as i64);
        varint::put_i64(out, self.time_zero);
        out.push(self.file_type as u8);
        varint::put_blob(out, self.writer.as_bytes());
        varint::put_blob(out, self.date.as_bytes());
        varint::put_blob(out, self.comment.as_bytes());
        varint::put_u64(out, self.group_size as u64);
        value::encode_attrs(&self.attrs, out);
    }

    pub fn decode(p: &[u8]) -> Result<Meta> {
        let mut r = Reader::new(p);
        let _ver = r.u64()?;
        let timescale = r.i64()? as i8;
        let time_zero = r.i64()?;
        let file_type = FileType::from_u8(r.u8()?);
        let writer = String::from_utf8_lossy(r.blob()?).into_owned();
        let date = String::from_utf8_lossy(r.blob()?).into_owned();
        let comment = String::from_utf8_lossy(r.blob()?).into_owned();
        let group_size = r.u32()?;
        let attrs = value::decode_attrs(&mut r)?;
        Ok(Meta { timescale, time_zero, file_type, writer, date, comment, group_size, attrs })
    }
}

/// One dump-on/off transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blackout {
    pub time: u64,
    /// `true` = dumping resumed at `time`, `false` = dumping stopped.
    pub active: bool,
}

pub fn encode_blackout(list: &[Blackout], out: &mut Vec<u8>) {
    varint::put_u64(out, list.len() as u64);
    let mut prev = 0u64;
    for b in list {
        out.push(b.active as u8);
        varint::put_u64(out, b.time.wrapping_sub(prev));
        prev = b.time;
    }
}

pub fn decode_blackout(p: &[u8]) -> Result<Vec<Blackout>> {
    let mut r = Reader::new(p);
    let n = r.usize()?;
    let mut out = Vec::with_capacity(n.min(1 << 20));
    let mut t = 0u64;
    for _ in 0..n {
        let active = r.u8()? != 0;
        t = t.wrapping_add(r.u64()?);
        out.push(Blackout { time: t, active });
    }
    Ok(out)
}

/// Recomputes directory aux fields from a section payload (used only when the
/// directory is missing, i.e. the writer crashed).
pub fn recover_aux(kind: u32, payload: &[u8]) -> (u64, u64) {
    match SectionKind::from_u32(kind) {
        Some(SectionKind::Strings) | Some(SectionKind::Hierarchy) => {
            let raw = match crate::codec::Decompressor::new().decompress(payload) {
                Ok(r) => r,
                Err(_) => return (0, 0),
            };
            let mut r = Reader::new(&raw);
            let first = r.u64().unwrap_or(0);
            let count = r.u64().unwrap_or(0);
            (first, count)
        }
        Some(SectionKind::SignalBlock) => match crate::block::BlockHeader::parse(payload) {
            Ok(h) => (h.start_time, h.end_time),
            Err(_) => (0, 0),
        },
        Some(SectionKind::TxBlock) => match crate::txblock::TxBlockHeader::parse(payload) {
            Ok(h) => (h.t_min, h.t_max),
            Err(_) => (0, 0),
        },
        _ => (0, 0),
    }
}
