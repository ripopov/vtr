//! Sidecar sections (`docs/SPEC.md`, "Activity index sidecar").

use super::{Budget, Identity, SourceFormat};
use crate::codec::{Compression, Compressor, Decompressor};
use crate::container::{self, DirEntry, SectionKind};
use crate::error::{Error, Result};
use crate::varint;
use std::io::Write;

pub(super) const KIND_HEADER: u32 = 1;
pub(super) const KIND_BLOCK: u32 = 2;
pub(super) const KIND_TAIL: u32 = 3;

const HEADER_LEN: usize = 40;
pub(super) const BLOCK_HEAD_LEN: usize = 40;
pub(super) const TAIL_HEAD_LEN: usize = 8;

/// Columns of a BLOCK section body: signal-id deltas, row headers
/// (`closed << 1 | has_first`), first changes, then per closed stretch its
/// length, the silence after it and its largest gap.
pub(super) const BLOCK_COLUMNS: usize = 6;
/// Columns of the TAIL body: signal-id deltas, final stretch lengths, gaps.
pub(super) const TAIL_COLUMNS: usize = 3;

pub(super) const COMPRESSION: Compression = Compression::ZSTD_DEFAULT;

/// The HEADER section: the trace's identity, the budget and the time origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Header {
    pub identity: Identity,
    pub signals: u32,
    pub budget: Budget,
    pub t_min: u64,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..8].copy_from_slice(&self.identity.length.to_le_bytes());
        h[8..12].copy_from_slice(&self.identity.toc_crc.to_le_bytes());
        h[12] = self.identity.format.code();
        h[16..20].copy_from_slice(&self.signals.to_le_bytes());
        h[20..24].copy_from_slice(&Budget::ppm(self.budget.disk).to_le_bytes());
        h[24..28].copy_from_slice(&Budget::ppm(self.budget.memory).to_le_bytes());
        h[32..40].copy_from_slice(&self.t_min.to_le_bytes());
        h
    }

    pub fn decode(p: &[u8]) -> Result<Header> {
        if p.len() != HEADER_LEN {
            return Err(Error::Corrupt("activity index header has the wrong length"));
        }
        let format = SourceFormat::from_code(p[12])?;
        Ok(Header {
            identity: Identity { format, length: le64(p, 0), toc_crc: le32(p, 8) },
            signals: le32(p, 16),
            budget: Budget { disk: le32(p, 20) as f64 / 1e6, memory: le32(p, 24) as f64 / 1e6 },
            t_min: le64(p, 32),
        })
    }
}

/// The fixed head of a BLOCK section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BlockHead {
    pub start: u64,
    pub end: u64,
    pub bytes: u64,
    pub rows: u32,
    pub closed: u32,
    /// Δ = 2^exponent.
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
        let h = BlockHead { start: le64(p, 0), end: le64(p, 8), bytes: le64(p, 16), rows: le32(p, 24), closed: le32(p, 28), exponent: p[32] };
        if h.exponent > 63 || h.end < h.start {
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
    Ok(le32(p, 0))
}

/// Packs varint columns (each prefixed by its byte length) into one
/// compressed blob.
pub(super) fn pack(cols: &[Vec<u8>], raw: &mut Vec<u8>, c: &mut Compressor, out: &mut Vec<u8>) -> Result<()> {
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

/// Decompresses a blob written by [`pack`] into `raw` and returns the byte
/// ranges of its `N` columns.
pub(super) fn unpack<const N: usize>(blob: &[u8], d: &mut Decompressor, raw: &mut Vec<u8>) -> Result<[(usize, usize); N]> {
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
        let end = pos.checked_add(l).filter(|&e| e <= raw.len()).ok_or(Error::Corrupt("activity index column exceeds its section"))?;
        *o = (pos, end);
        pos = end;
    }
    Ok(out)
}

/// Writes the sidecar container: file header, sections with CRCs, then the
/// directory and trailer.
pub(super) struct FileWriter<W: Write> {
    w: W,
    pos: u64,
    dir: Vec<DirEntry>,
}

impl<W: Write> FileWriter<W> {
    pub fn new(mut w: W) -> Result<Self> {
        let h = container::encode_file_header(&super::MAGIC, super::VERSION, 0);
        w.write_all(&h)?;
        Ok(FileWriter { w, pos: h.len() as u64, dir: Vec::new() })
    }

    pub fn section(&mut self, kind: u32, parts: &[&[u8]], aux0: u64, aux1: u64) -> Result<()> {
        let len: u64 = parts.iter().map(|p| p.len() as u64).sum();
        let mut crc = crc32fast::Hasher::new();
        for p in parts {
            crc.update(p);
        }
        self.w.write_all(&container::encode_section_header(kind, 0, len, crc.finalize()))?;
        for p in parts {
            self.w.write_all(p)?;
        }
        self.dir.push(DirEntry { kind, flags: 0, offset: self.pos, len, aux0, aux1 });
        self.pos += container::SECTION_HEADER_LEN as u64 + len;
        Ok(())
    }

    /// Writes the directory and trailer; returns the file length.
    pub fn finish(mut self) -> Result<u64> {
        let dir = container::encode_directory(&self.dir);
        let at = self.pos;
        self.w.write_all(&container::encode_section_header(SectionKind::Directory as u32, 0, dir.len() as u64, crc32fast::hash(&dir)))?;
        self.w.write_all(&dir)?;
        let len = at + container::SECTION_HEADER_LEN as u64 + dir.len() as u64 + container::TRAILER_LEN as u64;
        self.w.write_all(&container::encode_trailer(at, len))?;
        self.w.flush()?;
        Ok(len)
    }
}
