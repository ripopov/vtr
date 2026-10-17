//! Activity index: which signals change in a time window, answered for every signal at once
//! from a sidecar file beside the trace.

mod builder;
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
pub use index::{Classification, Index, IndexBlock, Stretch};
pub use scan::BlockScan;
pub use sidecar::{Sidecar, default_cache_dir};
pub use vtr_source::{build, resolve};

use crate::container::Container;
use crate::error::{Error, Result};
use crate::reader::Reader;

pub const STRETCH_BYTES: u64 = 20;

pub const MAGIC: [u8; 8] = [0x89, b'V', b'T', b'I', 0x0D, 0x0A, 0x1A, 0x0A];

pub const VERSION: u16 = 2;

pub(crate) const BUCKETS: usize = 65;

#[inline]
pub(crate) fn bucket(len: u64) -> usize {
    debug_assert!(len >= 1);
    (64 - (len - 1).leading_zeros()) as usize
}

pub(crate) fn threshold(hist: &[u64; BUCKETS], cap: u64, j_max: u32) -> u32 {
    let mut above: u64 = hist[1..].iter().sum();
    let mut j = 0;
    while above > cap && j < j_max {
        j += 1;
        above -= hist[j as usize];
    }
    j
}

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
}

impl Identity {
    pub fn of(reader: &Reader) -> Result<Identity> {
        let bytes = reader.bytes();
        let toc = Container::directory_payload(bytes)
            .ok_or_else(|| Error::invalid("a trace without its directory (still being written, or recovered) cannot be indexed"))?;
        Ok(Identity {
            format: SourceFormat::Vtr,
            length: bytes.len() as u64,
            toc_crc: crc32fast::hash(toc),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub disk: f64,
    pub memory: f64,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            disk: 0.01,
            memory: 0.04,
        }
    }
}

impl Budget {
    fn ppm(share: f64) -> u32 {
        (share.clamp(0.0, 4000.0) * 1e6).round() as u32
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
    pub budget: Budget,
    pub memory: u64,
    pub control: Option<std::sync::Arc<BuildControl>>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            threads: 0,
            budget: Budget::default(),
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
