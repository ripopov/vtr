//! The FST front end of the activity index (`docs/hierarchy-activity.html`).
//!
//! [`FstTrace`] maps an FST file and walks its blocks. Each value-change
//! block is one index block. A scan reads the block's time table and position
//! table, then each signal's value chain on its own, and walks the entry
//! headers for their time indexes without decoding a value. A signal is an
//! FST handle counted from 0 (handle 1 is signal 0). A change is a value
//! recorded after the header's start time; a block's frame, the values at
//! its start, is never a change.
//!
//! The writer chose the blocks' sizes, so every block's memory need is read
//! from its section header first, and [`vtr::activity::build_from`] refuses
//! a block over the limit. A gzip-wrapped file is unwrapped by streaming it to
//! a temporary file, never into memory.

use memmap2::Mmap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use vtr::activity::{Block, BlockScan, Budget, BuildOptions, Identity, Source, SourceFormat, Summary};
use vtr::{Error, Result, SignalId};

const BL_HEADER: u8 = 0;
const BL_VCDATA: u8 = 1;
const BL_GEOMETRY: u8 = 3;
const BL_HIERARCHY: u8 = 4;
const BL_VCDATA_DYN_ALIAS: u8 = 5;
const BL_HIERARCHY_LZ4: u8 = 6;
const BL_HIERARCHY_LZ4DUO: u8 = 7;
const BL_VCDATA_DYN_ALIAS2: u8 = 8;
const BL_GZIP_WRAPPER: u8 = 254;
const BL_SKIP: u8 = 255;

/// Geometry of a real signal (8-byte values) and of a variable-length one.
const GEOM_REAL: u32 = 0;
const GEOM_VARLEN: u32 = u32::MAX;

/// A value-change block: its section (from the length field) and layout.
#[derive(Clone, Copy, Debug)]
struct Section {
    at: usize,
    len: usize,
    alias2: bool,
    /// Bytes the writer says all the block's value chains take decompressed.
    mem: u64,
}

/// An FST file opened for the activity index: its blocks, signal geometry
/// and identity over a memory map. `Sync`, so blocks scan in parallel.
pub struct FstTrace {
    map: Mmap,
    identity: Identity,
    t_min: u64,
    timescale: i8,
    geometry: Vec<u32>,
    blocks: Vec<Block>,
    sections: Vec<Section>,
    /// The unwrapped copy of a gzip-wrapped file, removed on drop.
    unwrapped: Option<PathBuf>,
}

impl Drop for FstTrace {
    fn drop(&mut self) {
        if let Some(p) = &self.unwrapped {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn corrupt<T>(msg: &'static str) -> Result<T> {
    Err(Error::Corrupt(msg))
}

fn be64(b: &[u8], at: usize) -> Result<u64> {
    b.get(at..at + 8).map(|x| u64::from_be_bytes(x.try_into().unwrap())).ok_or(Error::Corrupt("FST section truncated"))
}

/// An FST varint (LEB128) at `*p`.
#[inline]
fn varint(b: &[u8], p: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*p).ok_or(Error::Corrupt("FST varint truncated"))?;
        *p += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    corrupt("FST varint too long")
}

/// A signed FST varint, sign-extended from its last group.
fn svarint(b: &[u8], p: &mut usize) -> Result<i64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*p).ok_or(Error::Corrupt("FST varint truncated"))?;
        *p += 1;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            if shift + 7 < 64 && byte & 0x40 != 0 {
                v |= u64::MAX << (shift + 7);
            }
            return Ok(v as i64);
        }
    }
    corrupt("FST varint too long")
}

fn inflate(input: &[u8], len: usize, out: &mut Vec<u8>) -> Result<()> {
    out.clear();
    out.reserve(len);
    let mut d = flate2::Decompress::new(true);
    d.decompress_vec(input, out, flate2::FlushDecompress::Finish).map_err(|e| Error::Codec(e.to_string()))?;
    if out.len() != len {
        return corrupt("FST zlib data has the wrong length");
    }
    Ok(())
}

impl FstTrace {
    /// Maps the FST file at `path` and reads its block list, header and
    /// geometry. A gzip-wrapped file is unwrapped to a temporary file first.
    /// A file without its geometry and hierarchy blocks (still being written,
    /// or cut short) has no stable identity and fails.
    pub fn open(path: impl AsRef<Path>) -> Result<FstTrace> {
        let file = std::fs::File::open(path.as_ref())?;
        // Safety: the file is only read; concurrent modification is a documented caller error.
        let map = unsafe { Mmap::map(&file)? };
        if map.first() == Some(&BL_GZIP_WRAPPER) {
            let (unwrapped, crc) = unwrap_gzip(&map)?;
            let length = map.len() as u64;
            let copy = std::fs::File::open(&unwrapped)?;
            // Safety: the temporary file is private to this trace.
            let inner = unsafe { Mmap::map(&copy)? };
            let mut t = FstTrace::parse(inner, Some(unwrapped))?;
            // A wrapped file is bound by its own bytes.
            t.identity = Identity { format: SourceFormat::Fst, length, toc_crc: crc };
            return Ok(t);
        }
        FstTrace::parse(map, None)
    }

    fn parse(map: Mmap, unwrapped: Option<PathBuf>) -> Result<FstTrace> {
        let b: &[u8] = &map;
        let (mut t_min, mut timescale, mut geometry, mut toc) = (None, 0i8, None, crc32fast::Hasher::new());
        let (mut have_hierarchy, mut blocks, mut sections) = (false, Vec::new(), Vec::new());
        let mut pos = 0usize;
        while pos + 9 <= b.len() {
            let tpe = b[pos];
            let len = be64(b, pos + 1)?;
            if tpe == BL_SKIP && len == 0 {
                break;
            }
            let end = (pos as u64 + 1).checked_add(len).filter(|&e| len >= 8 && e <= b.len() as u64).ok_or(Error::Corrupt("FST block extends past the end of the file"))? as usize;
            let sec = &b[pos + 1..end];
            match tpe {
                BL_HEADER => {
                    t_min = Some(be64(sec, 8)?);
                    timescale = *sec.get(72).ok_or(Error::Corrupt("FST header truncated"))? as i8;
                    toc.update(&b[pos..end]);
                }
                BL_GEOMETRY => {
                    geometry = Some(read_geometry(sec)?);
                    toc.update(&b[pos..end]);
                }
                BL_HIERARCHY | BL_HIERARCHY_LZ4 | BL_HIERARCHY_LZ4DUO => {
                    have_hierarchy = true;
                    toc.update(&b[pos..end]);
                }
                BL_VCDATA | BL_VCDATA_DYN_ALIAS | BL_VCDATA_DYN_ALIAS2 => {
                    let (start, stop, mem) = (be64(sec, 8)?, be64(sec, 16)?, be64(sec, 24)?);
                    if stop < start || blocks.last().is_some_and(|p: &Block| start < p.end) {
                        return Err(Error::Invalid(format!("FST value-change block at offset {pos} is out of time order")));
                    }
                    blocks.push(Block { start, end: stop, bytes: len });
                    sections.push(Section { at: pos + 1, len: len as usize, alias2: tpe == BL_VCDATA_DYN_ALIAS2, mem });
                }
                _ => {}
            }
            pos = end;
        }
        let (Some(t_min), Some(geometry), true) = (t_min, geometry, have_hierarchy) else {
            return Err(Error::Invalid("an FST file without its header, geometry and hierarchy blocks (still being written) cannot be indexed".into()));
        };
        let identity = Identity { format: SourceFormat::Fst, length: b.len() as u64, toc_crc: toc.finalize() };
        Ok(FstTrace { map, identity, t_min, timescale, geometry, blocks, sections, unwrapped })
    }

    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// Exponent of the time unit: one unit is `10^timescale` seconds.
    pub fn timescale(&self) -> i8 {
        self.timescale
    }

    /// Number of signals: the highest handle.
    pub fn signal_count(&self) -> u32 {
        self.geometry.len() as u32
    }

    /// Builds the activity index into `out` (see [`vtr::activity::build_from`]).
    pub fn build(&self, out: impl Write, opts: &BuildOptions) -> Result<Summary> {
        vtr::activity::build_from(self, out, opts)
    }

    /// The signals among `signals` that change in `[t0, t1]`, ascending: the
    /// exact answer for the ones the index leaves undecided. It reads the
    /// blocks overlapping the window and, in them, only these signals' value
    /// chains, each walked to its first entry in the window.
    pub fn resolve(&self, signals: &[SignalId], t0: u64, t1: u64) -> Result<Vec<SignalId>> {
        let t0 = t0.max(self.t_min.saturating_add(1));
        let mut sigs = signals.to_vec();
        sigs.sort_unstable();
        sigs.dedup();
        if let Some(s) = sigs.last().filter(|s| s.0 >= self.signal_count()) {
            return Err(Error::Invalid(format!("unknown signal {}", s.0)));
        }
        let mut hit = vec![false; sigs.len()];
        let mut w = Scratch::default();
        for (k, blk) in self.blocks.iter().enumerate() {
            if t0 > t1 || hit.iter().all(|&h| h) || blk.start > t1 {
                break;
            }
            if blk.end < t0 {
                continue;
            }
            let v = self.view(k, &mut w)?;
            let (lo, hi) = (w.times.partition_point(|&t| t < t0), w.times.partition_point(|&t| t <= t1));
            if lo == hi {
                continue;
            }
            for (i, s) in sigs.iter().enumerate() {
                if hit[i] {
                    continue;
                }
                let Some((off, len)) = v.chain_of(&w, s.0) else { continue };
                let chain = v.decode(off, len, &mut w.chain)?;
                let mut first = None;
                walk(chain, self.geometry[s.0 as usize], |ti| {
                    if first.is_none() && ti >= lo {
                        first = Some(ti);
                    }
                    first.is_none()
                })?;
                hit[i] = first.is_some_and(|ti| ti < hi);
            }
        }
        Ok(sigs.into_iter().zip(hit).filter_map(|(s, h)| h.then_some(s)).collect())
    }

    /// Reads block `k`'s time table and position table into `w`.
    fn view(&self, k: usize, w: &mut Scratch) -> Result<View<'_>> {
        let sec = self.sections[k];
        let s = &self.map[sec.at..sec.at + sec.len];
        // The time table closes the section: its data, then three lengths.
        let (tu, tc, count) = (be64(s, sec.len.saturating_sub(24))?, be64(s, sec.len.saturating_sub(16))?, be64(s, sec.len.saturating_sub(8))?);
        let tdata_at = (sec.len as u64).checked_sub(24 + tc).filter(|&a| a >= 32).ok_or(Error::Corrupt("FST time table out of its section"))? as usize;
        let tdata = &s[tdata_at..sec.len - 24];
        let raw: &[u8] = if tc == tu {
            tdata
        } else {
            inflate(tdata, tu as usize, &mut w.chain)?;
            &w.chain
        };
        w.times.clear();
        let (mut p, mut t) = (0usize, 0u64);
        for _ in 0..count {
            t = t.wrapping_add(varint(raw, &mut p)?);
            w.times.push(t);
        }
        // The frame, then the value chains from `vc`, the pack type's byte.
        let mut p = 32usize;
        let (_, fclen, _) = (varint(s, &mut p)?, varint(s, &mut p)?, varint(s, &mut p)?);
        p = p.checked_add(fclen as usize).filter(|&q| q <= tdata_at).ok_or(Error::Corrupt("FST frame out of its section"))?;
        let max_handle = varint(s, &mut p)?;
        let vc = p;
        let pack = *s.get(vc).ok_or(Error::Corrupt("FST value changes truncated"))?;
        // The position table ends right before the time table's length field.
        let chain_len_at = tdata_at.checked_sub(8).ok_or(Error::Corrupt("FST position table missing"))?;
        let chain_len = be64(s, chain_len_at)? as usize;
        let chain_at = chain_len_at.checked_sub(chain_len).filter(|&a| a > vc).ok_or(Error::Corrupt("FST position table out of its section"))?;
        read_positions(&s[chain_at..chain_len_at], sec.alias2, max_handle.min(self.geometry.len() as u64) as u32, (chain_at - vc) as u32, w)?;
        Ok(View { s, vc, pack })
    }
}

/// The geometry: per signal, its bit length, or real, or variable length.
fn read_geometry(sec: &[u8]) -> Result<Vec<u32>> {
    let (len, ulen, count) = (be64(sec, 0)?, be64(sec, 8)?, be64(sec, 16)?);
    let data = sec.get(24..len as usize).ok_or(Error::Corrupt("FST geometry truncated"))?;
    let mut buf = Vec::new();
    let raw: &[u8] = if data.len() as u64 == ulen {
        data
    } else {
        inflate(data, ulen as usize, &mut buf)?;
        &buf
    };
    let mut p = 0;
    (0..count).map(|_| Ok(varint(raw, &mut p)? as u32)).collect()
}

/// Streams a gzip-wrapped FST into a temporary file; returns its path and
/// the CRC-32 of the wrapped file, which identifies it.
fn unwrap_gzip(b: &[u8]) -> Result<(PathBuf, u32)> {
    let len = be64(b, 1)?;
    if len == 0 {
        return Err(Error::Invalid("a gzip-wrapped FST that its writer did not finish cannot be indexed".into()));
    }
    let body = b.get(17..(1 + len) as usize).ok_or(Error::Corrupt("FST gzip wrapper truncated"))?;
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!("vtr-fst-{}-{}.fst", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let copy = (|| -> Result<()> {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&path)?);
        let mut gz = flate2::read::GzDecoder::new(body);
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = gz.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
        }
        out.flush()?;
        Ok(())
    })();
    if let Err(e) = copy {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok((path, crc32fast::hash(b)))
}

/// Decoding buffers: a block's time table and position table, and one
/// decompressed value chain.
#[derive(Default)]
struct Scratch {
    times: Vec<u64>,
    /// (signal, offset from the pack byte, length) of every chain, by signal.
    chains: Vec<(u32, u32, u32)>,
    /// (signal, signal whose chain it shares).
    aliases: Vec<(u32, u32)>,
    chain: Vec<u8>,
}

/// Fills `w.chains` and `w.aliases` from a block's position table, `end`
/// being where the last chain ends (relative to the pack byte).
fn read_positions(t: &[u8], alias2: bool, max_handle: u32, end: u32, w: &mut Scratch) -> Result<()> {
    w.chains.clear();
    w.aliases.clear();
    let (mut p, mut idx, mut offset, mut last_alias) = (0usize, 0u32, 0i64, 0u32);
    let bad = || Error::Corrupt("FST position table out of range");
    while p < t.len() {
        if alias2 {
            if t[p] & 1 == 1 {
                let v = svarint(t, &mut p)? >> 1;
                if v > 0 {
                    offset += v;
                    w.chains.push((idx, u32::try_from(offset).map_err(|_| bad())?, 0));
                } else {
                    if v < 0 {
                        last_alias = u32::try_from(-v - 1).map_err(|_| bad())?;
                    }
                    w.aliases.push((idx, last_alias));
                }
                idx += 1;
            } else {
                idx += (varint(t, &mut p)? >> 1) as u32;
            }
        } else {
            let v = varint(t, &mut p)?;
            if v == 0 {
                let target = varint(t, &mut p)?.checked_sub(1).ok_or_else(bad)?;
                w.aliases.push((idx, target as u32));
                idx += 1;
            } else if v & 1 == 1 {
                offset += (v >> 1) as i64;
                w.chains.push((idx, u32::try_from(offset).map_err(|_| bad())?, 0));
                idx += 1;
            } else {
                idx += (v >> 1) as u32;
            }
        }
        if idx > max_handle {
            return Err(bad());
        }
    }
    // Each chain runs to the next one's start, the last to the table.
    for i in 0..w.chains.len() {
        let next = w.chains.get(i + 1).map_or(end, |c| c.1);
        w.chains[i].2 = next.checked_sub(w.chains[i].1).filter(|&l| l > 0).ok_or_else(bad)?;
    }
    Ok(())
}

/// A block's bytes and where its value chains start.
struct View<'a> {
    s: &'a [u8],
    vc: usize,
    pack: u8,
}

impl View<'_> {
    /// Offset and length of `signal`'s chain, following an alias.
    fn chain_of(&self, w: &Scratch, signal: u32) -> Option<(u32, u32)> {
        let find = |s: u32| w.chains.binary_search_by_key(&s, |c| c.0).ok().map(|i| (w.chains[i].1, w.chains[i].2));
        find(signal).or_else(|| w.aliases.binary_search_by_key(&signal, |a| a.0).ok().and_then(|i| find(w.aliases[i].1)))
    }

    /// The decompressed chain at `off` (from the pack byte) of `len` bytes.
    fn decode<'b>(&'b self, off: u32, len: u32, buf: &'b mut Vec<u8>) -> Result<&'b [u8]> {
        let at = self.vc + off as usize;
        let c = self.s.get(at..at + len as usize).ok_or(Error::Corrupt("FST value chain out of its section"))?;
        let mut p = 0;
        let ulen = varint(c, &mut p)? as usize;
        let data = &c[p..];
        if ulen == 0 {
            return Ok(data);
        }
        match self.pack {
            b'4' => {
                buf.clear();
                buf.resize(ulen, 0);
                let n = lz4_flex::block::decompress_into(data, buf).map_err(|e| Error::Codec(e.to_string()))?;
                if n != ulen {
                    return corrupt("FST lz4 chain has the wrong length");
                }
            }
            b'F' => fastlz(data, ulen, buf)?,
            _ => inflate(data, ulen, buf)?,
        }
        Ok(buf)
    }
}

/// Calls `f(time index)` for every entry of a decompressed value chain,
/// skipping values, while `f` returns true.
fn walk(chain: &[u8], geometry: u32, mut f: impl FnMut(usize) -> bool) -> Result<()> {
    let (mut p, mut ti) = (0usize, 0usize);
    let bytes = |n: usize| n.div_ceil(8);
    while p < chain.len() {
        let v = varint(chain, &mut p)?;
        let skip = match geometry {
            1 => {
                ti += (v >> (2 << (v & 1))) as usize;
                0
            }
            GEOM_VARLEN => {
                ti += (v >> 1) as usize;
                varint(chain, &mut p)? as usize
            }
            GEOM_REAL if v & 1 == 1 => {
                ti += (v >> 1) as usize;
                8
            }
            GEOM_REAL => return corrupt("packed FST real values are not supported"),
            n => {
                ti += (v >> 1) as usize;
                if v & 1 == 0 { bytes(n as usize) } else { n as usize }
            }
        };
        p = p.checked_add(skip).filter(|&q| q <= chain.len()).ok_or(Error::Corrupt("FST value runs past its chain"))?;
        if !f(ti) {
            break;
        }
    }
    Ok(())
}

/// FastLZ (levels 1 and 2) decompression of `input` into `out` (`len` bytes).
fn fastlz(input: &[u8], len: usize, out: &mut Vec<u8>) -> Result<()> {
    const MAX_L2_DISTANCE: usize = 8191;
    let bad = || Error::Corrupt("FST fastlz data out of range");
    out.clear();
    out.reserve(len);
    let level2 = input.first().is_some_and(|b| b >> 5 == 1);
    let mut ip = 0usize;
    let next = |ip: &mut usize| -> Result<u8> {
        let b = *input.get(*ip).ok_or_else(bad)?;
        *ip += 1;
        Ok(b)
    };
    let mut ctrl = next(&mut ip)? & 31;
    loop {
        if ctrl >= 32 {
            let mut n = (ctrl >> 5) as usize - 1;
            let high = ((ctrl & 31) as usize) << 8;
            if n == 6 {
                if level2 {
                    loop {
                        let c = next(&mut ip)?;
                        n += c as usize;
                        if c != 255 {
                            break;
                        }
                    }
                } else {
                    n += next(&mut ip)? as usize;
                }
            }
            let code = next(&mut ip)? as usize;
            let distance = if level2 && code == 255 && high == 31 << 8 {
                let far = ((next(&mut ip)? as usize) << 8) | next(&mut ip)? as usize;
                far + MAX_L2_DISTANCE + 1
            } else {
                high + code + 1
            };
            let from = out.len().checked_sub(distance).ok_or_else(bad)?;
            for k in 0..n + 3 {
                let b = out[from + k];
                out.push(b);
            }
        } else {
            let run = ctrl as usize + 1;
            out.extend_from_slice(input.get(ip..ip + run).ok_or_else(bad)?);
            ip += run;
        }
        if ip >= input.len() {
            break;
        }
        ctrl = next(&mut ip)?;
    }
    if out.len() != len {
        return corrupt("FST fastlz data has the wrong length");
    }
    Ok(())
}

impl Source for FstTrace {
    /// Nothing: a scan's buffers are its block's own, freed with it, as
    /// [`cost`](Self::cost) counts them.
    type Worker = ();

    fn identity(&self) -> Identity {
        self.identity
    }

    fn signal_count(&self) -> u32 {
        self.geometry.len() as u32
    }

    fn t_min(&self) -> u64 {
        self.t_min
    }

    fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The block's decompressed chains, as its writer states them, its time
    /// table and position table, and the scan's rows and candidates.
    fn cost(&self, i: usize, budget: Budget) -> Result<u64> {
        let sec = self.sections[i];
        let s = &self.map[sec.at..sec.at + sec.len];
        let (tu, count) = (be64(s, sec.len.saturating_sub(24))?, be64(s, sec.len.saturating_sub(8))?);
        let n = self.geometry.len() as u64;
        Ok(BlockScan::estimate(n, self.blocks[i], budget) + sec.mem + count * 8 + tu + n * 12)
    }

    fn worker(&self) {}

    fn scan(&self, _: &mut (), i: usize, scan: &mut BlockScan) -> Result<()> {
        let mut w = Scratch::default();
        let v = self.view(i, &mut w)?;
        let (times, chains, aliases, buf) = (&w.times, &w.chains, &w.aliases, &mut w.chain);
        let mut outside = false;
        let mut feed = |signal: u32, chain: &[u8]| {
            walk(chain, self.geometry[signal as usize], |ti| {
                match times.get(ti) {
                    Some(&t) => scan.push(signal, t),
                    None => outside = true,
                }
                true
            })
        };
        for &(signal, off, len) in chains {
            if signal as usize >= self.geometry.len() {
                return corrupt("FST chain of an unknown signal");
            }
            feed(signal, v.decode(off, len, buf)?)?;
        }
        for &(signal, target) in aliases {
            let (Ok(c), true) = (chains.binary_search_by_key(&target, |c| c.0), (signal as usize) < self.geometry.len()) else {
                return corrupt("FST alias of a signal without a chain");
            };
            feed(signal, v.decode(chains[c].1, chains[c].2, buf)?)?;
        }
        if outside {
            return corrupt("FST value change outside its block's time table");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both position-table formats: signal 0 at offset 3, two signals
    /// without data, signal 3 sharing signal 0's chain, signal 4 at 8.
    #[test]
    fn position_tables() {
        let old = [7u8, 4, 0, 1, 11];
        let alias2 = [7u8, 4, 0x7f, 11];
        for (table, is_alias2) in [(&old[..], false), (&alias2[..], true)] {
            let mut w = Scratch::default();
            read_positions(table, is_alias2, 5, 20, &mut w).unwrap();
            assert_eq!(w.chains, [(0, 3, 5), (4, 8, 12)], "alias2 {is_alias2}");
            assert_eq!(w.aliases, [(3, 0)], "alias2 {is_alias2}");
        }
        // A table naming more signals than the block has is refused.
        assert!(read_positions(&old, false, 4, 20, &mut Scratch::default()).is_err());
    }

    /// FastLZ level 1: a literal run, then a match copying it twice over.
    #[test]
    fn fastlz_level1() {
        let mut out = Vec::new();
        // Literal "abc" (ctrl 2), then a match of length 6 at distance 3:
        // ctrl = (len - 2) << 5 | high distance = 4 << 5, then the low distance 2.
        fastlz(&[2, b'a', b'b', b'c', 4 << 5, 2], 9, &mut out).unwrap();
        assert_eq!(out, b"abcabcabc");
        assert!(fastlz(&[2, b'a', b'b', b'c', 4 << 5, 9], 9, &mut Vec::new()).is_err(), "a match before the start");
    }
}
