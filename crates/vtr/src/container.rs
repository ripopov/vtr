//! Physical file container: file header, sections, directory and trailer.
//!
//! ```text
//! +-------------------+
//! | file header (32B) |
//! +-------------------+
//! | section ...       |   each: 24-byte header + payload
//! +-------------------+
//! | DIRECTORY section |   list of all sections with aux data (written at close)
//! +-------------------+
//! | trailer (24B)     |   offset of DIRECTORY + end magic
//! +-------------------+
//! ```
//!
//! A file whose writer crashed has no directory or trailer; the reader then
//! rebuilds the directory by walking section headers from the start.

use crate::error::{Error, Result};
use std::io::Write;

pub const FILE_MAGIC: [u8; 8] = [0x89, b'V', b'T', b'R', 0x0D, 0x0A, 0x1A, 0x0A];
pub const END_MAGIC: [u8; 8] = *b"VTR_END\0";
pub const FILE_HEADER_LEN: usize = 32;
pub const SECTION_HEADER_LEN: usize = 24;
pub const TRAILER_LEN: usize = 24;
pub const DIR_ENTRY_LEN: usize = 40;

/// Format version written by this library.
pub const VERSION_MAJOR: u16 = 1;
pub const VERSION_MINOR: u16 = 1;

/// Section kinds. Values below 0x1000 are reserved for the specification;
/// values 0x1000 and above are free for private extensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SectionKind {
    /// File-level metadata (timescale, strings, attributes).
    Meta = 1,
    /// String table chunk (append-only interned strings).
    Strings = 2,
    /// Hierarchy chunk (append-only nodes: scopes, vars, streams, ...).
    Hierarchy = 3,
    /// Signal value-change block.
    SignalBlock = 4,
    /// Transaction block.
    TxBlock = 5,
    /// Dump-on/off (blackout) intervals.
    Blackout = 6,
    /// Directory of all sections.
    Directory = 7,
    /// Log block (zero-duration transactions of log sites), optional section.
    LogBlock = 8,
}

impl SectionKind {
    pub fn from_u32(v: u32) -> Option<SectionKind> {
        Some(match v {
            1 => SectionKind::Meta,
            2 => SectionKind::Strings,
            3 => SectionKind::Hierarchy,
            4 => SectionKind::SignalBlock,
            5 => SectionKind::TxBlock,
            6 => SectionKind::Blackout,
            7 => SectionKind::Directory,
            8 => SectionKind::LogBlock,
            _ => return None,
        })
    }
}

/// Section flag: readers that do not understand `kind` may skip the section.
pub const SECTION_FLAG_OPTIONAL: u32 = 1;

/// One directory entry describing a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub kind: u32,
    pub flags: u32,
    /// Offset of the section header from the start of the file.
    pub offset: u64,
    /// Payload length (excluding the 24-byte header).
    pub len: u64,
    /// Kind-specific: first time / first id.
    pub aux0: u64,
    /// Kind-specific: last time / count.
    pub aux1: u64,
}

impl DirEntry {
    pub fn payload_offset(&self) -> u64 {
        self.offset + SECTION_HEADER_LEN as u64
    }
}

pub fn write_file_header(w: &mut impl Write) -> std::io::Result<()> {
    let mut h = [0u8; FILE_HEADER_LEN];
    h[..8].copy_from_slice(&FILE_MAGIC);
    h[8..10].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
    h[10..12].copy_from_slice(&VERSION_MINOR.to_le_bytes());
    w.write_all(&h)
}

pub fn encode_section_header(kind: u32, flags: u32, len: u64, crc: u32) -> [u8; SECTION_HEADER_LEN] {
    let mut h = [0u8; SECTION_HEADER_LEN];
    h[0..4].copy_from_slice(&kind.to_le_bytes());
    h[4..8].copy_from_slice(&flags.to_le_bytes());
    h[8..16].copy_from_slice(&len.to_le_bytes());
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

pub fn encode_directory(entries: &[DirEntry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + entries.len() * DIR_ENTRY_LEN);
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for e in entries {
        out.extend_from_slice(&e.kind.to_le_bytes());
        out.extend_from_slice(&e.flags.to_le_bytes());
        out.extend_from_slice(&e.offset.to_le_bytes());
        out.extend_from_slice(&e.len.to_le_bytes());
        out.extend_from_slice(&e.aux0.to_le_bytes());
        out.extend_from_slice(&e.aux1.to_le_bytes());
    }
    out
}

pub fn encode_trailer(directory_offset: u64, file_len: u64) -> [u8; TRAILER_LEN] {
    let mut t = [0u8; TRAILER_LEN];
    t[0..8].copy_from_slice(&directory_offset.to_le_bytes());
    t[8..16].copy_from_slice(&file_len.to_le_bytes());
    t[16..24].copy_from_slice(&END_MAGIC);
    t
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}
fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b[..8].try_into().unwrap())
}

/// Parsed container view over a complete file image.
#[derive(Debug)]
pub struct Container {
    pub version: (u16, u16),
    pub entries: Vec<DirEntry>,
    /// True when the directory was rebuilt by scanning (no trailer found).
    pub recovered: bool,
}

impl Container {
    /// Parses the header and directory of `data` (the whole file, e.g. an mmap).
    pub fn parse(data: &[u8]) -> Result<Container> {
        if data.len() < FILE_HEADER_LEN || data[..8] != FILE_MAGIC {
            return Err(Error::Corrupt("not a VTR file (bad magic)"));
        }
        let major = u16::from_le_bytes([data[8], data[9]]);
        let minor = u16::from_le_bytes([data[10], data[11]]);
        if major > VERSION_MAJOR {
            return Err(Error::UnsupportedVersion { major, minor, supported: VERSION_MAJOR });
        }
        // Try the trailer first.
        if data.len() >= FILE_HEADER_LEN + TRAILER_LEN {
            let t = &data[data.len() - TRAILER_LEN..];
            if t[16..24] == END_MAGIC {
                let dir_off = le_u64(&t[0..8]) as usize;
                let file_len = le_u64(&t[8..16]) as usize;
                if file_len == data.len() && dir_off + SECTION_HEADER_LEN <= data.len() {
                    let h = &data[dir_off..dir_off + SECTION_HEADER_LEN];
                    let kind = le_u32(&h[0..4]);
                    let len = le_u64(&h[8..16]) as usize;
                    if kind == SectionKind::Directory as u32
                        && dir_off + SECTION_HEADER_LEN + len == data.len() - TRAILER_LEN
                    {
                        let p = &data[dir_off + SECTION_HEADER_LEN..dir_off + SECTION_HEADER_LEN + len];
                        let entries = Self::decode_directory(p)?;
                        return Ok(Container { version: (major, minor), entries, recovered: false });
                    }
                }
            }
        }
        // Recovery: scan sections from the start.
        let mut entries = Vec::new();
        let mut off = FILE_HEADER_LEN;
        while off + SECTION_HEADER_LEN <= data.len() {
            let h = &data[off..off + SECTION_HEADER_LEN];
            let kind = le_u32(&h[0..4]);
            let flags = le_u32(&h[4..8]);
            let len = le_u64(&h[8..16]);
            let end = off as u64 + SECTION_HEADER_LEN as u64 + len;
            if kind == 0 || end > data.len() as u64 {
                break; // truncated tail
            }
            if kind == SectionKind::Directory as u32 {
                off = end as usize;
                continue;
            }
            let payload = &data[off + SECTION_HEADER_LEN..end as usize];
            let (aux0, aux1) = crate::sections::recover_aux(kind, payload);
            entries.push(DirEntry { kind, flags, offset: off as u64, len, aux0, aux1 });
            off = end as usize;
        }
        Ok(Container { version: (major, minor), entries, recovered: true })
    }

    fn decode_directory(p: &[u8]) -> Result<Vec<DirEntry>> {
        if p.len() < 8 {
            return Err(Error::Corrupt("directory too short"));
        }
        let n = le_u64(&p[0..8]) as usize;
        if p.len() < 8 + n * DIR_ENTRY_LEN {
            return Err(Error::Corrupt("directory truncated"));
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = &p[8 + i * DIR_ENTRY_LEN..8 + (i + 1) * DIR_ENTRY_LEN];
            entries.push(DirEntry {
                kind: le_u32(&e[0..4]),
                flags: le_u32(&e[4..8]),
                offset: le_u64(&e[8..16]),
                len: le_u64(&e[16..24]),
                aux0: le_u64(&e[24..32]),
                aux1: le_u64(&e[32..40]),
            });
        }
        Ok(entries)
    }

    /// Returns the payload bytes of `entry` within `data`, optionally verifying its CRC.
    pub fn payload<'a>(data: &'a [u8], entry: &DirEntry, verify_crc: bool) -> Result<&'a [u8]> {
        let start = entry.offset as usize;
        let end = start + SECTION_HEADER_LEN + entry.len as usize;
        if end > data.len() || start + SECTION_HEADER_LEN > data.len() {
            return Err(Error::Corrupt("section extends past end of file"));
        }
        let payload = &data[start + SECTION_HEADER_LEN..end];
        if verify_crc {
            let crc = le_u32(&data[start + 16..start + 20]);
            if crc != 0 && crc32fast::hash(payload) != crc {
                return Err(Error::Checksum { offset: entry.offset });
            }
        }
        Ok(payload)
    }
}
