//! Activity index: which signals change in a time window, answered for every signal at once
//! from a sidecar file beside the trace.

mod builder;
mod container;
mod control;
mod driver;
mod format;
mod index;
mod scan;
mod sidecar;
mod vtr_source;

pub use builder::Builder;
pub use control::{BuildControl, BuildProgress};
pub use driver::{Source, build_from};
pub use index::{Index, IndexBlock, Stretch};
pub use scan::BlockScan;
pub use sidecar::{Sidecar, default_cache_dir};
pub use vtr_source::build;

use crate::error::{Error, Result};
use crate::framing::RecordingId;
use crate::trace::Reader;

pub const MAGIC: [u8; 8] = [0x89, b'V', b'T', b'I', 0x0D, 0x0A, 0x1A, 0x0A];

pub const VERSION: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceFormat {
    Vtr,
}

impl SourceFormat {
    fn code(self) -> u8 {
        match self {
            SourceFormat::Vtr => 1,
        }
    }

    fn from_code(c: u8) -> Result<SourceFormat> {
        match c {
            1 => Ok(SourceFormat::Vtr),
            _ => Err(Error::Corrupt("unknown activity index source format")),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SourceFormat::Vtr => "vtr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Identity {
    pub format: SourceFormat,
    pub length: u64,
    pub toc_crc: u32,
    pub recording: RecordingId,
}

impl Identity {
    pub fn of(reader: &Reader) -> Result<Identity> {
        let fingerprint = reader
            .fingerprint()
            .ok_or_else(|| Error::invalid("activity indexes require a finalized trace"))?;
        Ok(Identity {
            format: SourceFormat::Vtr,
            length: fingerprint.length,
            toc_crc: fingerprint.directory_crc,
            recording: fingerprint.recording,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: u64,
    pub end: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub threads: usize,
    pub memory: u64,
    pub control: Option<std::sync::Arc<BuildControl>>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            threads: 0,
            memory: 512 << 20,
            control: None,
        }
    }
}

impl BuildOptions {
    pub(crate) fn worker_threads(&self) -> usize {
        match self.threads {
            0 => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            n => n,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub blocks: u32,
    pub signals: u32,
    pub changes: u64,
    pub stretches: u64,
    pub delta: Option<(u64, u64)>,
    pub bytes: u64,
    pub source_bytes: u64,
}
