//! Activity index: which signals change in a time window, answered for every
//! signal at once from a sidecar file beside the trace
//! (`docs/hierarchy-activity.html`).
//!
//! A signal is quiet in `[t0, t1]` exactly when the window lies inside one of
//! its *silences*, the time between two consecutive changes. The index keeps
//! each signal's long silences. Between them the signal is busy, and each busy
//! *stretch* keeps its first change, its last change and its largest inner
//! gap. How long counts as long is decided per source block from its own
//! bytes, never from a time constant: a block's threshold Δ is the smallest
//! power of two for which the silences ending in the block fit
//! [`Budget::memory`] of the block's compressed bytes when loaded
//! ([`STRETCH_BYTES`] each) and its index section fits [`Budget::disk`] of
//! them, but at most a quarter of the block's span. A silence is kept when it
//! is longer than the smallest Δ of the blocks its interior touches, so every
//! window at least that wide is answered exactly and a narrower one touches at
//! most two blocks.
//!
//! A *change* is a recorded value after the trace's first time step, as a
//! waveform row draws it: initial values are not changes, and several values
//! recorded at one time step are one change.
//!
//! * [`BlockScan`] reduces one source block's change times, and
//!   [`Builder`] stitches scans, in time order, into a sidecar written
//!   section by section. Memory is 24 bytes per signal plus the scans in
//!   flight, whatever the length of the run.
//! * [`build`] is the VTR front end: it scans blocks in parallel and decodes
//!   only entry headers, never values.
//! * [`Index`] is a loaded sidecar, valid for the trace whose [`Identity`]
//!   it records.
//! * [`Sidecar`] names where a trace's index lives (`<trace>.index`, else a
//!   user cache directory) and writes one through a temporary file.
//!
//! The sidecar format is specified in `docs/SPEC.md` ("Activity index
//! sidecar"). It is derived data: rebuilding it gives the same bytes.
//!
//! ```
//! use vtr::activity::{self, BuildOptions, Index, Identity};
//! # fn main() -> vtr::Result<()> {
//! # let dir = std::env::temp_dir().join(format!("vtr-doc-activity-{}", std::process::id()));
//! # std::fs::create_dir_all(&dir)?;
//! # let path = dir.join("run.vtr");
//! # {
//! #     use vtr::{Direction, ScopeType, SignalKind, VarType, Writer};
//! #     let mut w = Writer::create(&path)?;
//! #     let top = Some(w.add_scope(None, "top", ScopeType::Module, "")?);
//! #     let (_, clk) = w.add_var(top, "clk", VarType::Wire, Direction::Input, SignalKind::Bits { width: 1, states: 2 })?;
//! #     for t in 0..100 { w.set_time(t * 5)?; w.emit_bit(clk, (t & 1) as u8)?; }
//! #     w.close()?;
//! # }
//! let reader = vtr::Reader::open(&path)?;
//! let id = Identity::of(&reader)?;
//! let mut image = Vec::new();
//! let summary = activity::build(&reader, &mut image, &BuildOptions::default())?;
//! let index = Index::decode(&image, &id)?;
//! assert_eq!(index.stretch_count(), summary.stretches);
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok(())
//! # }
//! ```

mod builder;
mod format;
mod index;
mod scan;
mod sidecar;
mod source;

pub use builder::Builder;
pub use index::{Classification, Index, IndexBlock, Stretch};
pub use scan::BlockScan;
pub use sidecar::{default_cache_dir, Sidecar};
pub use source::{build, resolve};

use crate::container::Container;
use crate::error::{Error, Result};
use crate::reader::Reader;

/// Bytes one stretch occupies in a loaded [`Index`]: its first and last
/// change and its largest inner gap. [`Budget::memory`] is counted in them.
pub const STRETCH_BYTES: u64 = 20;

/// Sidecar file magic: `\x89VTI\r\n\x1a\n`.
pub const MAGIC: [u8; 8] = [0x89, b'V', b'T', b'I', 0x0D, 0x0A, 0x1A, 0x0A];

/// Sidecar format version written and read by this library. Other versions
/// fail with [`Error::UnsupportedVersion`] and are rebuilt.
pub const VERSION: u16 = 1;

/// Silence-length buckets: bucket `k` holds lengths in `(2^(k-1), 2^k]`,
/// bucket 0 the length 1.
pub(crate) const BUCKETS: usize = 65;

#[inline]
pub(crate) fn bucket(len: u64) -> usize {
    debug_assert!(len >= 1);
    (64 - (len - 1).leading_zeros()) as usize
}

/// The smallest threshold exponent `j` (Δ = 2^j) at which the silences of
/// `hist` longer than Δ number at most `cap`, or `j_max`.
pub(crate) fn threshold(hist: &[u64; BUCKETS], cap: u64, j_max: u32) -> u32 {
    let mut above: u64 = hist[1..].iter().sum();
    let mut j = 0;
    while above > cap && j < j_max {
        j += 1;
        above -= hist[j as usize];
    }
    j
}

/// Format of the trace an index was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceFormat {
    Vtr,
    Fst,
}

impl SourceFormat {
    fn code(self) -> u8 {
        match self {
            SourceFormat::Vtr => 1,
            SourceFormat::Fst => 2,
        }
    }

    fn from_code(c: u8) -> Result<SourceFormat> {
        match c {
            1 => Ok(SourceFormat::Vtr),
            2 => Ok(SourceFormat::Fst),
            _ => Err(Error::Corrupt("unknown activity index source format")),
        }
    }

    /// Lower-case name, as in cache file names.
    pub fn name(self) -> &'static str {
        match self {
            SourceFormat::Vtr => "vtr",
            SourceFormat::Fst => "fst",
        }
    }
}

/// What binds a sidecar to one trace: the trace's format, its length and a
/// CRC-32 of its table of contents. An index is used only for the trace
/// whose identity it records, so one left beside a re-simulated trace is
/// ignored and rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Identity {
    pub format: SourceFormat,
    /// Length of the trace file in bytes.
    pub length: u64,
    /// CRC-32 of the table of contents: a VTR file's DIRECTORY payload.
    pub toc_crc: u32,
}

impl Identity {
    /// Identity of an open VTR file. A file recovered without its trailer
    /// has no stable identity and cannot be indexed ([`Error::Invalid`]).
    pub fn of(reader: &Reader) -> Result<Identity> {
        let bytes = reader.bytes();
        let toc = Container::directory_payload(bytes)
            .ok_or_else(|| Error::invalid("a trace without its directory (still being written, or recovered) cannot be indexed"))?;
        Ok(Identity { format: SourceFormat::Vtr, length: bytes.len() as u64, toc_crc: crc32fast::hash(toc) })
    }
}

/// The index's share of each source block's compressed bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    /// Share on disk: the compressed index section of a block. Default 1%.
    pub disk: f64,
    /// Share in memory once loaded, at [`STRETCH_BYTES`] per stretch.
    /// Default 4%.
    pub memory: f64,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { disk: 0.01, memory: 0.04 }
    }
}

impl Budget {
    fn ppm(share: f64) -> u32 {
        (share.clamp(0.0, 4000.0) * 1e6).round() as u32
    }
}

/// A block of the source trace: the time range of its time steps and its
/// compressed size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// First time step of the block.
    pub start: u64,
    /// Last time step of the block. A block's *cell* is the time after the
    /// previous block's end up to this end; cells partition the run.
    pub end: u64,
    /// Compressed bytes of the block in the trace.
    pub bytes: u64,
}

/// Settings of a build.
#[derive(Clone, Copy, Debug)]
pub struct BuildOptions {
    /// Worker threads scanning blocks; 0 = available parallelism.
    pub threads: usize,
    pub budget: Budget,
    /// Bytes the block scans in flight may hold, beside the builder's 24
    /// bytes per signal. Workers wait rather than exceed it, and a block
    /// whose scan alone would need more is refused with an error naming it.
    /// Default 512 MiB.
    pub memory: u64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { threads: 0, budget: Budget::default(), memory: 512 << 20 }
    }
}

impl BuildOptions {
    pub(crate) fn worker_threads(&self) -> usize {
        match self.threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            n => n,
        }
    }
}

/// What a build made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub blocks: u32,
    pub signals: u32,
    /// Changes after the first time step, one per signal and time step.
    pub changes: u64,
    /// Busy stretches kept, over all signals.
    pub stretches: u64,
    /// Smallest and largest block threshold Δ in time units; `None` without blocks.
    pub delta: Option<(u64, u64)>,
    /// Bytes of the sidecar.
    pub bytes: u64,
    /// Bytes of the source trace.
    pub source_bytes: u64,
}
