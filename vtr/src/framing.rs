//! Recording identities, checked sections, final directories, and prefix recovery.

use std::{
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

use crate::{Error, Result, wire::Cursor};

pub const MAGIC: [u8; 8] = [0x89, b'V', b'T', b'R', 13, 10, 26, 10];
pub const TRAILER_MAGIC: [u8; 8] = *b"VTR_END\0";
pub const VERSION: (u16, u16) = (3, 4);
pub const HEADER_BYTES: usize = 32;
pub const SECTION_BYTES: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecordingId(pub [u8; 16]);

impl std::fmt::Display for RecordingId {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(output, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl RecordingId {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0; 16];
        getrandom::getrandom(&mut bytes)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if bytes == [0; 16] {
            return Err(Error::State("random source returned a zero identity"));
        }
        Ok(Self(bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    Complete,
    Incomplete,
    Unfinished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub recording: RecordingId,
    pub length: u64,
    pub directory_crc: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub length: u64,
    pub aux0: u64,
    pub aux1: u64,
    pub block_number: u64,
}

impl Section {
    #[must_use]
    pub fn payload_offset(self) -> u64 {
        self.offset + SECTION_BYTES as u64
    }
}

pub struct FramedWriter<W: Write> {
    output: W,
    identity: RecordingId,
    position: u64,
    sections: Vec<Section>,
    finished: bool,
}

impl<W: Write> FramedWriter<W> {
    pub fn new(output: W) -> Result<Self> {
        Self::with_identity(output, RecordingId::generate()?)
    }

    pub fn with_identity(mut output: W, identity: RecordingId) -> Result<Self> {
        if identity.0 == [0; 16] {
            return Err(Error::invalid("zero recording identity"));
        }
        output.write_all(&header(identity))?;
        Ok(Self {
            output,
            identity,
            position: HEADER_BYTES as u64,
            sections: Vec::new(),
            finished: false,
        })
    }

    pub fn identity(&self) -> RecordingId {
        self.identity
    }
    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn section(
        &mut self,
        kind: u32,
        flags: u32,
        payload: &[u8],
        aux0: u64,
        aux1: u64,
        block_number: u64,
    ) -> Result<()> {
        if self.finished {
            return Err(Error::State("container is finalized"));
        }
        if kind == 0 || kind == 7 || (matches!(kind, 5 | 9) != (block_number != 0)) {
            return Err(Error::invalid("invalid section kind or block number"));
        }
        let next = self
            .position
            .checked_add(SECTION_BYTES as u64)
            .and_then(|n| n.checked_add(payload.len() as u64))
            .ok_or_else(|| Error::invalid("container length overflow"))?;
        self.output.write_all(&section_header(
            kind,
            flags,
            payload.len() as u64,
            crc32fast::hash(payload),
        ))?;
        self.output.write_all(payload)?;
        self.sections.push(Section {
            kind,
            flags,
            offset: self.position,
            length: payload.len() as u64,
            aux0,
            aux1,
            block_number,
        });
        self.position = next;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.output.flush()?;
        Ok(())
    }

    pub fn finish(&mut self, incomplete: bool, dropped: u64) -> Result<Fingerprint> {
        if self.finished {
            return Err(Error::State("container is finalized"));
        }
        if !incomplete && dropped != 0 {
            return Err(Error::invalid(
                "complete output cannot report dropped bytes",
            ));
        }
        let mut directory = (self.sections.len() as u64).to_le_bytes().to_vec();
        directory.extend_from_slice(&dropped.to_le_bytes());
        for section in &self.sections {
            directory.extend_from_slice(&section.kind.to_le_bytes());
            directory.extend_from_slice(&section.flags.to_le_bytes());
            for value in [
                section.offset,
                section.length,
                section.aux0,
                section.aux1,
                section.block_number,
            ] {
                directory.extend_from_slice(&value.to_le_bytes());
            }
        }
        let directory_crc = crc32fast::hash(&directory);
        let length = self
            .position
            .checked_add(directory.len() as u64)
            .and_then(|n| n.checked_add(48))
            .ok_or_else(|| Error::invalid("container length overflow"))?;
        self.output.write_all(&section_header(
            7,
            if incomplete { 2 } else { 0 },
            directory.len() as u64,
            directory_crc,
        ))?;
        self.output.write_all(&directory)?;
        self.output.write_all(&self.position.to_le_bytes())?;
        self.output.write_all(&length.to_le_bytes())?;
        self.output.write_all(&TRAILER_MAGIC)?;
        self.output.flush()?;
        self.position = length;
        self.finished = true;
        Ok(Fingerprint {
            recording: self.identity,
            length,
            directory_crc,
        })
    }

    pub fn into_inner(self) -> W {
        self.output
    }
}

fn header(identity: RecordingId) -> [u8; HEADER_BYTES] {
    let mut out = [0; HEADER_BYTES];
    out[..8].copy_from_slice(&MAGIC);
    out[8..10].copy_from_slice(&VERSION.0.to_le_bytes());
    out[10..12].copy_from_slice(&VERSION.1.to_le_bytes());
    out[16..].copy_from_slice(&identity.0);
    out
}

fn section_header(kind: u32, flags: u32, length: u64, crc: u32) -> [u8; SECTION_BYTES] {
    let mut out = [0; SECTION_BYTES];
    out[..4].copy_from_slice(&kind.to_le_bytes());
    out[4..8].copy_from_slice(&flags.to_le_bytes());
    out[8..16].copy_from_slice(&length.to_le_bytes());
    out[16..20].copy_from_slice(&crc.to_le_bytes());
    out
}

enum Data {
    Mapped(memmap2::Mmap),
    Owned(Arc<[u8]>),
}

impl Data {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Mapped(data) => data,
            Self::Owned(data) => data,
        }
    }
}

pub struct Container {
    data: Data,
    pub identity: RecordingId,
    pub completion: Completion,
    pub dropped: u64,
    pub fingerprint: Option<Fingerprint>,
    sections: Vec<Section>,
    verify_crc: bool,
}

impl Container {
    pub fn open(path: impl AsRef<Path>, verify_crc: bool) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        // SAFETY: The mapping is read-only and owned until the container is dropped.
        // Callers must keep recording files immutable while readers are open.
        let mapped = unsafe { memmap2::MmapOptions::new().map(&file)? };
        Self::parse(Data::Mapped(mapped), verify_crc, true)
    }

    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>, verify_crc: bool) -> Result<Self> {
        Self::parse(Data::Owned(bytes.into()), verify_crc, true)
    }

    fn parse(data: Data, verify_crc: bool, use_directory: bool) -> Result<Self> {
        let bytes = data.bytes();
        let mut input = Cursor::new(bytes);
        if input.take(8)? != MAGIC {
            return Err(Error::Corrupt("invalid VTR magic"));
        }
        let major = u16::from_le_bytes(input.fixed()?);
        let minor = u16::from_le_bytes(input.fixed()?);
        if (major, minor) != VERSION {
            return Err(Error::UnsupportedVersion {
                format: "VTR",
                major,
                minor,
                supported_major: VERSION.0,
                supported_minor: VERSION.1,
            });
        }
        if input.u32()? != 0 {
            return Err(Error::Corrupt("unsupported VTR header flags"));
        }
        let identity = RecordingId(input.fixed()?);
        if identity.0 == [0; 16] {
            return Err(Error::Corrupt("missing recording identity"));
        }
        let mut container = Self {
            data,
            identity,
            completion: Completion::Unfinished,
            dropped: 0,
            fingerprint: None,
            sections: Vec::new(),
            verify_crc,
        };
        let bytes = container.data.bytes();
        if use_directory && bytes.len() >= 56 && bytes.ends_with(&TRAILER_MAGIC) {
            let mut trailer = Cursor::new(&bytes[bytes.len() - 24..]);
            let offset = trailer.u64()?;
            let length = trailer.u64()?;
            if length == bytes.len() as u64 && offset >= 32 && offset <= length - 48 {
                let directory = container.section_at(offset)?;
                if directory.kind != 7
                    || directory.length != length - offset - 48
                    || directory.flags & !2 != 0
                {
                    return Err(Error::Corrupt("invalid directory bounds or flags"));
                }
                let payload = container.payload(&directory, true)?;
                let mut cursor = Cursor::new(payload);
                let count = cursor.u64()?;
                let dropped = cursor.u64()?;
                if (directory.flags == 0 && dropped != 0) || count > cursor.remaining() as u64 / 48
                {
                    return Err(Error::Corrupt(
                        "invalid directory recovery state or entry count",
                    ));
                }
                let mut previous = HEADER_BYTES as u64;
                let mut sections = Vec::new();
                let mut blocks = HashSet::new();
                for _ in 0..count {
                    let section = Section {
                        kind: cursor.u32()?,
                        flags: cursor.u32()?,
                        offset: cursor.u64()?,
                        length: cursor.u64()?,
                        aux0: cursor.u64()?,
                        aux1: cursor.u64()?,
                        block_number: cursor.u64()?,
                    };
                    if section.offset != previous
                        || section.offset >= offset
                        || offset - section.offset < 24
                        || section.length > offset - section.offset - 24
                        || section.kind == 0
                        || section.kind == 7
                        || (matches!(section.kind, 5 | 9) != (section.block_number != 0))
                        || (section.block_number != 0 && !blocks.insert(section.block_number))
                    {
                        return Err(Error::Corrupt("invalid directory entry"));
                    }
                    let actual = container.section_at(section.offset)?;
                    if actual.kind != section.kind
                        || actual.flags != section.flags
                        || actual.length != section.length
                    {
                        return Err(Error::Corrupt("directory/header mismatch"));
                    }
                    previous = section.offset + 24 + section.length;
                    sections.push(section);
                }
                cursor.done()?;
                if previous != offset {
                    return Err(Error::Corrupt("directory omits sections"));
                }
                container.fingerprint = Some(Fingerprint {
                    recording: identity,
                    length,
                    directory_crc: crc32fast::hash(payload),
                });
                container.completion = if directory.flags == 0 {
                    Completion::Complete
                } else {
                    Completion::Incomplete
                };
                container.dropped = dropped;
                container.sections = sections;
                return Ok(container);
            }
        }
        let mut offset = HEADER_BYTES as u64;
        while bytes.len() as u64 - offset >= SECTION_BYTES as u64 {
            let parsed = container.section_at(offset).and_then(|mut section| {
                if section.kind == 0 || section.kind == 7 {
                    return Err(Error::Corrupt("end of recoverable prefix"));
                }
                let payload = container.payload(&section, true)?;
                let mut bounds = Cursor::new(payload);
                match section.kind {
                    4 => {
                        section.aux0 = bounds.u64()?;
                        section.aux1 = bounds.u64()?;
                    }
                    5 | 9 => {
                        section.block_number = bounds.u64()?;
                        bounds.take(if section.kind == 5 { 32 } else { 24 })?;
                        section.aux0 = bounds.u64()?;
                        section.aux1 = bounds.u64()?;
                    }
                    10 => {
                        section.aux0 = bounds.var()?;
                        section.aux1 = bounds.var()?;
                    }
                    2 | 3 => {
                        let mut compressed = Cursor::new(payload);
                        compressed.byte()?;
                        let length = usize::try_from(compressed.var()?)
                            .map_err(|_| Error::Corrupt("declaration size exceeds usize"))?;
                        let raw = crate::wire::decompress(payload, length)?;
                        let mut declarations = Cursor::new(&raw);
                        section.aux0 = declarations.var()?;
                        section.aux1 = declarations.var()?;
                    }
                    _ => (),
                }
                Ok(section)
            });
            let Ok(section) = parsed else {
                break;
            };
            offset += SECTION_BYTES as u64 + section.length;
            container.sections.push(section);
        }
        container.dropped = bytes.len() as u64 - offset;
        Ok(container)
    }

    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.data.bytes()
    }

    fn slice(&self, offset: u64, size: u64) -> Result<&[u8]> {
        let length = self.data.bytes().len() as u64;
        if offset > length || size > length - offset {
            return Err(Error::Corrupt("section exceeds file"));
        }
        Ok(&self.data.bytes()[offset as usize..(offset + size) as usize])
    }

    fn section_at(&self, offset: u64) -> Result<Section> {
        let mut cursor = Cursor::new(self.slice(offset, SECTION_BYTES as u64)?);
        let kind = cursor.u32()?;
        let flags = cursor.u32()?;
        let length = cursor.u64()?;
        cursor.u32()?;
        if cursor.u32()? != 0 {
            return Err(Error::Corrupt("nonzero section reserved field"));
        }
        self.slice(offset + SECTION_BYTES as u64, length)?;
        Ok(Section {
            kind,
            flags,
            offset,
            length,
            aux0: 0,
            aux1: 0,
            block_number: 0,
        })
    }

    pub fn payload(&self, section: &Section, force_crc: bool) -> Result<&[u8]> {
        let payload = self.slice(section.payload_offset(), section.length)?;
        if self.verify_crc || force_crc {
            let expected =
                u32::from_le_bytes(self.slice(section.offset + 16, 4)?.try_into().unwrap());
            if expected != 0 && expected != crc32fast::hash(payload) {
                return Err(Error::Checksum {
                    offset: section.offset,
                });
            }
        }
        Ok(payload)
    }
}

use std::collections::HashSet;

pub fn recover(input: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<()> {
    let input = input.as_ref();
    let output = output.as_ref();
    if input == output
        || output
            .canonicalize()
            .is_ok_and(|path| input.canonicalize().is_ok_and(|source| source == path))
    {
        return Err(Error::invalid("recovery paths refer to the same file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(source), Ok(destination)) = (input.metadata(), output.metadata())
            && source.dev() == destination.dev()
            && source.ino() == destination.ino()
        {
            return Err(Error::invalid("recovery paths refer to the same file"));
        }
    }
    #[cfg(windows)]
    if output.try_exists()? && same_file::is_same_file(input, output)? {
        return Err(Error::invalid("recovery paths refer to the same file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(input)?.read_to_end(&mut bytes)?;
    let parsed = Container::parse(Data::Owned(bytes.into()), true, false)?;
    if !parsed.sections.iter().any(|section| section.kind == 1) {
        return Err(Error::Corrupt("recovery prefix has no metadata"));
    }
    let mut writer = FramedWriter::with_identity(
        std::io::BufWriter::new(std::fs::File::create(output)?),
        parsed.identity,
    )?;
    for section in &parsed.sections {
        writer.section(
            section.kind,
            section.flags,
            parsed.payload(section, true)?,
            section.aux0,
            section.aux1,
            section.block_number,
        )?;
    }
    writer.finish(true, parsed.dropped)?;
    Ok(())
}
