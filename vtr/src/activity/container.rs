//! Independent VTI framing; its directory has 40-byte entries.

use crate::{Error, Result, wire::Cursor};
use std::io::Write;

const HEADER: usize = 32;
const SECTION: usize = 24;
const TRAILER: usize = 24;
const END: &[u8; 8] = b"VTR_END\0";

#[derive(Clone, Copy, Debug)]
pub(super) struct Entry {
    pub kind: u32,
    pub offset: u64,
    pub len: u64,
    pub aux0: u64,
    pub aux1: u64,
}

fn header(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SECTION);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

pub(super) fn payload<'a>(bytes: &'a [u8], entry: &Entry) -> Result<&'a [u8]> {
    let start =
        usize::try_from(entry.offset).map_err(|_| Error::Corrupt("activity offset overflow"))?;
    let length =
        usize::try_from(entry.len).map_err(|_| Error::Corrupt("activity length overflow"))?;
    let data = bytes
        .get(start..)
        .ok_or(Error::Corrupt("activity offset outside file"))?;
    let mut cursor = Cursor::new(data);
    if cursor.u32()? != entry.kind || cursor.u32()? != 0 || cursor.u64()? != entry.len {
        return Err(Error::Corrupt("activity section disagrees with directory"));
    }
    let crc = cursor.u32()?;
    if cursor.u32()? != 0 {
        return Err(Error::Corrupt("activity section reserved flags"));
    }
    let body = cursor.take(length)?;
    if crc32fast::hash(body) != crc {
        return Err(Error::Checksum {
            offset: entry.offset,
        });
    }
    Ok(body)
}

pub(super) fn parse(bytes: &[u8], mut admit: impl FnMut(u64) -> Result<()>) -> Result<Vec<Entry>> {
    if bytes.len() < HEADER + SECTION + 8 + TRAILER || bytes[..8] != super::MAGIC {
        return Err(Error::Corrupt("invalid activity file header"));
    }
    let mut file_header = Cursor::new(&bytes[8..HEADER]);
    let major = u16::from_le_bytes(file_header.take(2)?.try_into().unwrap());
    let minor = u16::from_le_bytes(file_header.take(2)?.try_into().unwrap());
    if major != super::VERSION || minor != 0 {
        return Err(Error::UnsupportedVersion {
            format: "VTI",
            major,
            minor,
            supported_major: super::VERSION,
            supported_minor: 0,
        });
    }
    if file_header.take(20)?.iter().any(|&byte| byte != 0) {
        return Err(Error::Corrupt("activity file reserved flags"));
    }
    let trailer_start = bytes.len() - TRAILER;
    let mut trailer = Cursor::new(&bytes[trailer_start..]);
    let directory_offset = trailer.u64()?;
    if trailer.u64()? != bytes.len() as u64
        || trailer.take(8)? != END
        || directory_offset < HEADER as u64
        || directory_offset >= trailer_start as u64
    {
        return Err(Error::Corrupt("invalid activity trailer"));
    }
    let start = usize::try_from(directory_offset)
        .map_err(|_| Error::Corrupt("activity directory offset overflow"))?;
    let raw = &bytes[start..trailer_start];
    let mut cursor = Cursor::new(raw);
    if cursor.u32()? != 7 || cursor.u32()? != 0 {
        return Err(Error::Corrupt("invalid activity directory"));
    }
    let length = cursor.u64()?;
    let crc = cursor.u32()?;
    if cursor.u32()? != 0 || length != cursor.remaining() as u64 {
        return Err(Error::Corrupt("invalid activity directory bounds"));
    }
    let body = cursor.take(cursor.remaining())?;
    if crc32fast::hash(body) != crc {
        return Err(Error::Checksum {
            offset: directory_offset,
        });
    }
    let mut cursor = Cursor::new(body);
    let count = cursor.u64()?;
    if count != (cursor.remaining() / 40) as u64 || !cursor.remaining().is_multiple_of(40) {
        return Err(Error::Corrupt("invalid activity directory count"));
    }
    let count = count as usize;
    admit((count as u64).saturating_mul(std::mem::size_of::<Entry>() as u64))?;
    let mut entries = Vec::with_capacity(count);
    let mut offset = HEADER as u64;
    for _ in 0..count {
        let kind = cursor.u32()?;
        if cursor.u32()? != 0 {
            return Err(Error::Corrupt("activity directory reserved flags"));
        }
        let entry = Entry {
            kind,
            offset: cursor.u64()?,
            len: cursor.u64()?,
            aux0: cursor.u64()?,
            aux1: cursor.u64()?,
        };
        if entry.offset != offset {
            return Err(Error::Corrupt(
                "activity directory is not a section partition",
            ));
        }
        offset = offset
            .checked_add(SECTION as u64)
            .and_then(|v| v.checked_add(entry.len))
            .filter(|&v| v <= directory_offset)
            .ok_or(Error::Corrupt("activity section exceeds directory"))?;
        payload(bytes, &entry)?;
        entries.push(entry);
    }
    if offset != directory_offset {
        return Err(Error::Corrupt("activity directory leaves unindexed bytes"));
    }
    Ok(entries)
}

pub(super) struct FileWriter<W: Write> {
    output: W,
    position: u64,
    directory: Vec<Entry>,
}
impl<W: Write> FileWriter<W> {
    pub fn new(mut output: W) -> Result<Self> {
        let mut bytes = [0u8; HEADER];
        bytes[..8].copy_from_slice(&super::MAGIC);
        bytes[8..10].copy_from_slice(&super::VERSION.to_le_bytes());
        output.write_all(&bytes)?;
        Ok(Self {
            output,
            position: HEADER as u64,
            directory: Vec::new(),
        })
    }
    pub fn section(&mut self, kind: u32, parts: &[&[u8]], aux0: u64, aux1: u64) -> Result<()> {
        let length = parts
            .iter()
            .try_fold(0usize, |sum, part| sum.checked_add(part.len()))
            .ok_or(Error::invalid("activity section length overflow"))?;
        let mut crc = crc32fast::Hasher::new();
        for part in parts {
            crc.update(part);
        }
        let mut bytes = Vec::with_capacity(SECTION);
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(length as u64).to_le_bytes());
        bytes.extend_from_slice(&crc.finalize().to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        self.output.write_all(&bytes)?;
        for part in parts {
            self.output.write_all(part)?;
        }
        self.directory.push(Entry {
            kind,
            offset: self.position,
            len: length as u64,
            aux0,
            aux1,
        });
        self.position += SECTION as u64 + length as u64;
        Ok(())
    }
    pub fn finish(mut self) -> Result<u64> {
        let mut directory = Vec::new();
        directory.extend_from_slice(&(self.directory.len() as u64).to_le_bytes());
        for entry in self.directory {
            directory.extend_from_slice(&entry.kind.to_le_bytes());
            directory.extend_from_slice(&0u32.to_le_bytes());
            for number in [entry.offset, entry.len, entry.aux0, entry.aux1] {
                directory.extend_from_slice(&number.to_le_bytes());
            }
        }
        self.output.write_all(&header(7, &directory))?;
        self.output.write_all(&directory)?;
        let length = self.position + SECTION as u64 + directory.len() as u64 + TRAILER as u64;
        self.output.write_all(&self.position.to_le_bytes())?;
        self.output.write_all(&length.to_le_bytes())?;
        self.output.write_all(END)?;
        self.output.flush()?;
        Ok(length)
    }
}
