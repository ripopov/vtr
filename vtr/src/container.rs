//! Physical file container: file header, sections, directory and trailer.

use crate::error::{Error, Result};
use std::io::Write;

pub const FILE_MAGIC: [u8; 8] = [0x89, b'V', b'T', b'R', 0x0D, 0x0A, 0x1A, 0x0A];
pub const END_MAGIC: [u8; 8] = *b"VTR_END\0";
pub const FILE_HEADER_LEN: usize = 32;
pub const SECTION_HEADER_LEN: usize = 24;
pub const TRAILER_LEN: usize = 24;
pub const DIR_ENTRY_LEN: usize = 40;

pub const VERSION_MAJOR: u16 = 2;
pub const VERSION_MINOR: u16 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum SectionKind {
    Meta = 1,
    Strings = 2,
    Hierarchy = 3,
    SignalBlock = 4,
    TxBlock = 5,
    Blackout = 6,
    Directory = 7,
    LogBlock = 8,
}

impl SectionKind {
    #[must_use]
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

pub const SECTION_FLAG_OPTIONAL: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub len: u64,
    pub aux0: u64,
    pub aux1: u64,
}

impl DirEntry {
    #[must_use]
    pub fn payload_offset(&self) -> u64 {
        self.offset + SECTION_HEADER_LEN as u64
    }
}

pub fn write_file_header(w: &mut impl Write) -> std::io::Result<()> {
    w.write_all(&encode_file_header(
        &FILE_MAGIC,
        VERSION_MAJOR,
        VERSION_MINOR,
    ))
}

#[must_use]
pub fn encode_file_header(magic: &[u8; 8], major: u16, minor: u16) -> [u8; FILE_HEADER_LEN] {
    let mut h = [0u8; FILE_HEADER_LEN];
    h[..8].copy_from_slice(magic);
    h[8..10].copy_from_slice(&major.to_le_bytes());
    h[10..12].copy_from_slice(&minor.to_le_bytes());
    h
}

#[must_use]
pub fn encode_section_header(
    kind: u32,
    flags: u32,
    len: u64,
    crc: u32,
) -> [u8; SECTION_HEADER_LEN] {
    let mut h = [0u8; SECTION_HEADER_LEN];
    h[0..4].copy_from_slice(&kind.to_le_bytes());
    h[4..8].copy_from_slice(&flags.to_le_bytes());
    h[8..16].copy_from_slice(&len.to_le_bytes());
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

#[must_use]
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

#[must_use]
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

#[derive(Debug)]
pub struct Container {
    pub version: (u16, u16),
    pub entries: Vec<DirEntry>,
    pub recovered: Option<u64>,
}

impl Container {
    pub fn parse(data: &[u8]) -> Result<Container> {
        if data.len() < FILE_HEADER_LEN || data[..8] != FILE_MAGIC {
            return Err(Error::Corrupt("not a VTR file (bad magic)"));
        }
        let major = u16::from_le_bytes([data[8], data[9]]);
        let minor = u16::from_le_bytes([data[10], data[11]]);
        if major != VERSION_MAJOR || minor != VERSION_MINOR {
            return Err(Error::UnsupportedVersion {
                major,
                minor,
                supported: VERSION_MAJOR,
            });
        }
        if let Some(entries) = Self::trailer_directory(data) {
            return Ok(Container {
                version: (major, minor),
                entries: entries?,
                recovered: None,
            });
        }
        let mut entries = Vec::new();
        let mut off = FILE_HEADER_LEN;
        while off + SECTION_HEADER_LEN <= data.len() {
            let h = &data[off..off + SECTION_HEADER_LEN];
            let kind = le_u32(&h[0..4]);
            let flags = le_u32(&h[4..8]);
            let len = le_u64(&h[8..16]);
            let crc = le_u32(&h[16..20]);
            let end = off as u64 + SECTION_HEADER_LEN as u64 + len;
            if kind == 0 || end > data.len() as u64 {
                break;
            }
            let payload = &data[off + SECTION_HEADER_LEN..end as usize];
            if crc != 0 && crc32fast::hash(payload) != crc {
                break;
            }
            if kind == SectionKind::Directory as u32 {
                off = end as usize;
                continue;
            }
            let (aux0, aux1) = crate::sections::recover_aux(kind, payload);
            entries.push(DirEntry {
                kind,
                flags,
                offset: off as u64,
                len,
                aux0,
                aux1,
            });
            off = end as usize;
        }
        Ok(Container {
            version: (major, minor),
            entries,
            recovered: Some((data.len() - off) as u64),
        })
    }

    #[must_use]
    pub fn directory_payload(data: &[u8]) -> Option<&[u8]> {
        if data.len() < FILE_HEADER_LEN + TRAILER_LEN {
            return None;
        }
        let t = &data[data.len() - TRAILER_LEN..];
        if t[16..24] != END_MAGIC {
            return None;
        }
        let dir_off = usize::try_from(le_u64(&t[0..8])).ok()?;
        let file_len = usize::try_from(le_u64(&t[8..16])).ok()?;
        if file_len != data.len() || dir_off.checked_add(SECTION_HEADER_LEN)? > data.len() {
            return None;
        }
        let h = &data[dir_off..dir_off + SECTION_HEADER_LEN];
        let len = usize::try_from(le_u64(&h[8..16])).ok()?;
        let end = (dir_off + SECTION_HEADER_LEN).checked_add(len)?;
        if le_u32(&h[0..4]) != SectionKind::Directory as u32 || end != data.len() - TRAILER_LEN {
            return None;
        }
        Some(&data[dir_off + SECTION_HEADER_LEN..end])
    }

    fn trailer_directory(data: &[u8]) -> Option<Result<Vec<DirEntry>>> {
        Self::directory_payload(data).map(Self::decode_directory)
    }

    pub fn parse_closed(data: &[u8], magic: &[u8; 8]) -> Result<((u16, u16), Vec<DirEntry>)> {
        if data.len() < FILE_HEADER_LEN || data[..8] != magic[..] {
            return Err(Error::Corrupt("bad magic"));
        }
        let version = (
            u16::from_le_bytes([data[8], data[9]]),
            u16::from_le_bytes([data[10], data[11]]),
        );
        let entries = Self::trailer_directory(data).ok_or(Error::Corrupt("no valid trailer"))??;
        for e in &entries {
            if e.offset
                .checked_add(SECTION_HEADER_LEN as u64)
                .and_then(|start| start.checked_add(e.len))
                .is_none_or(|end| end > data.len() as u64)
            {
                return Err(Error::Corrupt("section extends past end of file"));
            }
        }
        Ok((version, entries))
    }

    fn decode_directory(p: &[u8]) -> Result<Vec<DirEntry>> {
        if p.len() < 8 {
            return Err(Error::Corrupt("directory too short"));
        }
        let count = le_u64(&p[0..8]);
        if count > ((p.len() - 8) / DIR_ENTRY_LEN) as u64 {
            return Err(Error::Corrupt("directory truncated"));
        }
        let n = count as usize;
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
                return Err(Error::Checksum {
                    offset: entry.offset,
                });
            }
        }
        Ok(payload)
    }
}
