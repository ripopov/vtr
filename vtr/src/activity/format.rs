//! Sidecar sections (`vtr/docs/SPEC.md`, "Activity index sidecar").

pub(super) use super::container::FileWriter;
use super::{Identity, SourceFormat};
use crate::codec::{Compression, Compressor, Decompressor};
use crate::error::{Error, Result};
use crate::framing::RecordingId;
use crate::varint;

pub(super) const KIND_HEADER: u32 = 1;
pub(super) const KIND_BLOCK: u32 = 2;
pub(super) const KIND_TAIL: u32 = 3;

const HEADER_LEN: usize = 56;
pub(super) const BLOCK_HEAD_LEN: usize = 40;
pub(super) const TAIL_HEAD_LEN: usize = 8;

pub(super) const BLOCK_COLUMNS: usize = 6;
pub(super) const TAIL_COLUMNS: usize = 3;

pub(super) const COMPRESSION: Compression = Compression::ZSTD_DEFAULT;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Header {
    pub identity: Identity,
    pub signals: u32,
    pub t_min: u64,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..8].copy_from_slice(&self.identity.length.to_le_bytes());
        h[8..12].copy_from_slice(&self.identity.toc_crc.to_le_bytes());
        h[12] = self.identity.format.code();
        h[16..20].copy_from_slice(&self.signals.to_le_bytes());
        h[32..40].copy_from_slice(&self.t_min.to_le_bytes());
        h[40..56].copy_from_slice(&self.identity.recording.0);
        h
    }

    pub fn decode(p: &[u8]) -> Result<Header> {
        if p.len() != HEADER_LEN {
            return Err(Error::Corrupt("activity index header has the wrong length"));
        }
        if p[13..16].iter().chain(&p[20..32]).any(|&b| b != 0) {
            return Err(Error::Corrupt("activity header reserved fields"));
        }
        let format = SourceFormat::from_code(p[12])?;
        Ok(Header {
            identity: Identity {
                format,
                length: le64(p, 0),
                toc_crc: le32(p, 8),
                recording: RecordingId(p[40..56].try_into().unwrap()),
            },
            signals: le32(p, 16),
            t_min: le64(p, 32),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BlockHead {
    pub start: u64,
    pub end: u64,
    pub bytes: u64,
    pub rows: u32,
    pub closed: u32,
    pub exponent: u8,
}

impl BlockHead {
    pub fn encode(&self) -> [u8; BLOCK_HEAD_LEN] {
        let mut h = [0u8; BLOCK_HEAD_LEN];
        h[0..8].copy_from_slice(&self.start.to_le_bytes());
        h[8..16].copy_from_slice(&self.end.to_le_bytes());
        h[16..24].copy_from_slice(&self.bytes.to_le_bytes());
        h[24..28].copy_from_slice(&self.rows.to_le_bytes());
        h[28..32].copy_from_slice(&self.closed.to_le_bytes());
        h[32] = self.exponent;
        h
    }

    pub fn decode(p: &[u8]) -> Result<BlockHead> {
        if p.len() < BLOCK_HEAD_LEN {
            return Err(Error::Corrupt("activity index block truncated"));
        }
        let h = BlockHead {
            start: le64(p, 0),
            end: le64(p, 8),
            bytes: le64(p, 16),
            rows: le32(p, 24),
            closed: le32(p, 28),
            exponent: p[32],
        };
        if h.exponent != 0 || p[33..40].iter().any(|&b| b != 0) || h.end < h.start {
            return Err(Error::Corrupt("activity index block head out of range"));
        }
        Ok(h)
    }
}

fn le32(p: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(p[o..o + 4].try_into().unwrap())
}

fn le64(p: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(p[o..o + 8].try_into().unwrap())
}

pub(super) fn tail_rows(p: &[u8]) -> Result<u32> {
    if p.len() < TAIL_HEAD_LEN {
        return Err(Error::Corrupt("activity index tail truncated"));
    }
    if p[4..8].iter().any(|&b| b != 0) {
        return Err(Error::Corrupt("activity tail reserved fields"));
    }
    Ok(le32(p, 0))
}

pub(super) fn pack(
    cols: &[Vec<u8>],
    raw: &mut Vec<u8>,
    c: &mut Compressor,
    out: &mut Vec<u8>,
) -> Result<()> {
    raw.clear();
    for col in cols {
        varint::put_u64(raw, col.len() as u64);
    }
    for col in cols {
        raw.extend_from_slice(col);
    }
    out.clear();
    c.compress_into(COMPRESSION, raw, out)
}

pub(super) fn unpack<const N: usize>(
    blob: &[u8],
    d: &mut Decompressor,
    raw: &mut Vec<u8>,
) -> Result<[(usize, usize); N]> {
    raw.clear();
    d.decompress_into(blob, raw)?;
    let mut r = varint::Reader::new(raw);
    let mut lens = [0usize; N];
    for l in &mut lens {
        *l = r.usize()?;
    }
    let mut pos = r.pos;
    let mut out = [(0, 0); N];
    for (o, l) in out.iter_mut().zip(lens) {
        let end = pos
            .checked_add(l)
            .filter(|&e| e <= raw.len())
            .ok_or(Error::Corrupt("activity index column exceeds its section"))?;
        *o = (pos, end);
        pos = end;
    }
    if pos != raw.len() {
        return Err(Error::Corrupt("activity columns leave trailing bytes"));
    }
    Ok(out)
}
