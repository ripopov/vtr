//! Signal value-change blocks: encoding (writer side) and decoding (reader side).
//!
//! Block payload layout (all fixed fields little-endian):
//!
//! ```text
//! u64 start_time | u64 end_time | u32 n_times | u32 n_signals | u32 group_size
//! u32 n_dirty | u32 n_groups | u32 tt_len
//! time table blob            (tt_len bytes; compressed varint deltas)
//! dirty index                (n_dirty x {u32 group, u32 clen, u64 offset}, ascending group)
//! prev-dirty table           (n_groups x u32: index of the previous block in which the group
//!                             had changes, or 0xFFFFFFFF)
//! group blobs                (compressed; offsets relative to the start of this area)
//! ```
//!
//! Group container (stored uncompressed; its pieces are compressed separately):
//!
//! ```text
//! varint n_sigs
//! varint frame_clen | frame blob      raw = value of every signal of the group at start_time
//!                                     in declared packing (VarLen: varint len + bytes)
//! varint n_runs
//! n_runs x { varint n_in_run, varint clen }   runs cover the group's signals in order
//! run blobs...                        raw = varint column_len[n_in_run] then the columns
//! ```
//!
//! A run holds consecutive signals whose raw columns fit in `run_budget`
//! bytes (a single larger column forms a run of its own), so reading one
//! signal decompresses only a bounded amount of data.
//!
//! Columns (`dt` = time-index delta from the signal's previous change in the block):
//! * 1-bit signals: a sequence of `varint((dt << 4) | code)` entries;
//! * every other kind: `varint header_len`, then `header_len` bytes of entry headers, then
//!   the values concatenated in the same order (headers and values are separate streams so
//!   that the compressor sees homogeneous data):
//!   - vectors: header `varint((dt << 1) | compact)`, value `ceil(w/8)` bytes (compact,
//!     2-state packing) or `packed_len(w, states)` bytes (declared packing);
//!   - reals: header `varint(dt)`, value 8 bytes little-endian IEEE double;
//!   - variable length: header `varint(dt)`, value `varint(len)` + bytes.

use crate::codec::{Compression, Compressor, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::SignalKind;
use crate::signal::SignalValue;
use crate::value::packed_len;
use crate::varint::{self, Reader};
use std::sync::Arc;

pub const HEADER_LEN: usize = 40;
pub const INDEX_ENTRY_LEN: usize = 16;
pub const NO_BLOCK: u32 = u32::MAX;
pub const COMPACT_FLAG: u32 = 1 << 31;

/// One value change as logged by the writer.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Record {
    pub sig: u32,
    /// Time-table index; bit 31 marks a compact (2-state) payload for multi-state signals.
    pub tidx: u32,
    /// Inline packed value (narrow signals / reals) or heap offset (wide / varlen).
    pub payload: u64,
}

/// True when values of `kind` fit into the 8-byte inline payload.
#[inline]
pub fn is_narrow(kind: SignalKind) -> bool {
    match kind {
        SignalKind::Bits { width, states } => packed_len(width, states) <= 8,
        SignalKind::Real => true,
        SignalKind::VarLen => false,
    }
}

/// A slice of the value-change log handed to the encoder before the block is complete.
pub struct ChunkInput {
    pub records: Vec<Record>,
    pub heap: Vec<u8>,
    /// Records per signal in this chunk.
    pub sig_counts: Vec<u32>,
    /// Signal table snapshot covering every signal in the chunk.
    pub kinds: Arc<Vec<SignalKind>>,
    /// Pre-encoded column fragments of wide signals: (signal, entry count, headers, values), ascending by signal.
    pub wide: Vec<(u32, u32, Vec<u8>, Vec<u8>)>,
}

/// Per-signal column fragments produced from one chunk.
#[derive(Default)]
pub struct ChunkEnc {
    /// Per fragment: signal, header range in `hdr`, value range in `val`; ascending by signal.
    pub frags: Vec<Frag>,
    pub hdr: Vec<u8>,
    pub val: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Frag {
    pub sig: u32,
    pub hoff: u32,
    pub hlen: u32,
    pub voff: u32,
    pub vlen: u32,
}

/// Everything the encoder needs to finish one block.
pub struct BlockInput {
    pub block_index: u32,
    pub times: Vec<u64>,
    pub kinds: Arc<Vec<SignalKind>>,
    pub n_signals: u32,
    pub group_size: u32,
    /// Ascending group ids that have at least one change.
    pub dirty_groups: Vec<u32>,
    /// Frame entries of all signals of all dirty groups, concatenated in order.
    pub frame: Vec<u8>,
    /// Per group (all groups): previous block index where the group was dirty.
    pub prev_dirty: Vec<u32>,
    /// Raw bytes per compressed column run.
    pub run_budget: usize,
}

/// Scratch buffers reused across chunks and blocks by the encoder.
#[derive(Default)]
pub struct EncoderScratch {
    counts: Vec<u32>,
    fill: Vec<u32>,
    sorted: Vec<Record>,
    group_raw: Vec<u8>,
    col_lens: Vec<u32>,
    col_counts: Vec<u32>,
    data: Vec<u8>,
    tt: Vec<u8>,
    tt_c: Vec<u8>,
    frame_c: Vec<u8>,
    run_blobs: Vec<u8>,
    /// Last time index per signal within the current block (delta continuity across chunks).
    last_tidx: Vec<u32>,
    /// Entries per signal within the current block.
    entry_counts: Vec<u32>,
    cursors: Vec<usize>,
}

impl EncoderScratch {
    /// Resets per-block state (call before the first chunk of a block).
    pub fn begin_block(&mut self) {
        for t in &mut self.last_tidx {
            *t = 0;
        }
        for c in &mut self.entry_counts {
            *c = 0;
        }
    }
}

/// Sorts a chunk by signal and encodes its records into per-signal column fragments.
pub fn encode_chunk(input: &ChunkInput, kinds: &[SignalKind], scratch: &mut EncoderScratch, out: &mut ChunkEnc) -> Result<()> {
    let n_sig = input.sig_counts.len();
    if n_sig > kinds.len() {
        return Err(Error::State("sig_counts exceed signal table"));
    }
    if scratch.last_tidx.len() < n_sig {
        scratch.last_tidx.resize(n_sig, 0);
        scratch.entry_counts.resize(n_sig, 0);
    }
    let counts = &mut scratch.counts;
    counts.clear();
    counts.resize(n_sig + 1, 0);
    for (i, &c) in input.sig_counts.iter().enumerate() {
        counts[i + 1] = counts[i] + c;
    }
    if counts[n_sig] as usize != input.records.len() {
        return Err(Error::State("sig_counts do not match record count"));
    }
    let sorted = &mut scratch.sorted;
    sorted.clear();
    sorted.resize(input.records.len(), Record::default());
    {
        let fill = &mut scratch.fill;
        fill.clear();
        fill.extend_from_slice(&counts[..n_sig]);
        for r in &input.records {
            let p = unsafe { fill.get_unchecked_mut(r.sig as usize) };
            unsafe { *sorted.get_unchecked_mut(*p as usize) = *r };
            *p += 1;
        }
    }
    out.frags.clear();
    out.hdr.clear();
    out.val.clear();
    let mut wide_pos = 0usize;
    for s in 0..n_sig {
        let recs = &sorted[counts[s] as usize..counts[s + 1] as usize];
        // Wide fragments arrive pre-encoded from the writer.
        while wide_pos < input.wide.len() && (input.wide[wide_pos].0 as usize) < s {
            wide_pos += 1;
        }
        if wide_pos < input.wide.len() && input.wide[wide_pos].0 as usize == s {
            let (_, cnt, ref h, ref v) = input.wide[wide_pos];
            let f = Frag { sig: s as u32, hoff: out.hdr.len() as u32, hlen: h.len() as u32, voff: out.val.len() as u32, vlen: v.len() as u32 };
            out.hdr.extend_from_slice(h);
            out.val.extend_from_slice(v);
            scratch.entry_counts[s] += cnt;
            out.frags.push(f);
            continue;
        }
        if recs.is_empty() {
            continue;
        }
        let (hoff, voff) = (out.hdr.len(), out.val.len());
        let prev = scratch.last_tidx[s];
        encode_column(kinds[s], recs, &input.heap, prev, &mut out.hdr, &mut out.val)?;
        scratch.last_tidx[s] = recs.last().unwrap().tidx & !COMPACT_FLAG;
        scratch.entry_counts[s] += recs.len() as u32;
        out.frags.push(Frag { sig: s as u32, hoff: hoff as u32, hlen: (out.hdr.len() - hoff) as u32, voff: voff as u32, vlen: (out.val.len() - voff) as u32 });
    }
    Ok(())
}

/// Finishes a block from its encoded chunks. Returns the number of column entries.
pub fn finish_block(
    input: &BlockInput,
    chunks: &[ChunkEnc],
    comp: Compression,
    compressor: &mut Compressor,
    scratch: &mut EncoderScratch,
    out: &mut Vec<u8>,
) -> Result<usize> {
    let n_sig = input.n_signals as usize;
    let g = input.group_size as usize;
    let n_groups = n_sig.div_ceil(g.max(1));
    if input.prev_dirty.len() != n_groups {
        return Err(Error::State("prev_dirty table size mismatch"));
    }
    if scratch.entry_counts.len() < n_sig {
        scratch.entry_counts.resize(n_sig, 0);
    }
    // --- time table ---
    let tt = &mut scratch.tt;
    tt.clear();
    let mut prev = 0u64;
    for &t in &input.times {
        varint::put_u64(tt, t.wrapping_sub(prev));
        prev = t;
    }
    let tt_c = &mut scratch.tt_c;
    tt_c.clear();
    compressor.compress_into(comp, tt, tt_c)?;
    // --- groups ---
    let data = &mut scratch.data;
    data.clear();
    let mut index: Vec<(u32, u32, u64)> = Vec::with_capacity(input.dirty_groups.len());
    let mut frame_pos = 0usize;
    let cursors = &mut scratch.cursors;
    cursors.clear();
    cursors.resize(chunks.len(), 0);
    let mut total_entries = 0usize;
    for &grp in &input.dirty_groups {
        let first = grp as usize * g;
        let last = ((grp as usize + 1) * g).min(n_sig);
        let n = last - first;
        let raw = &mut scratch.group_raw;
        raw.clear();
        // Frame piece.
        let mut fr = Reader::new(&input.frame[frame_pos..]);
        for s in first..last {
            match input.kinds[s] {
                SignalKind::VarLen => {
                    let b = fr.blob()?;
                    varint::put_blob(raw, b);
                }
                k => {
                    let l = k.packed_len().unwrap();
                    raw.extend_from_slice(fr.bytes(l)?);
                }
            }
        }
        frame_pos += fr.pos;
        let off = data.len() as u64;
        varint::put_u64(data, n as u64);
        let frame_c = &mut scratch.frame_c;
        frame_c.clear();
        compressor.compress_into(comp, raw, frame_c)?;
        varint::put_u64(data, frame_c.len() as u64);
        data.extend_from_slice(frame_c);
        // Column lengths are known from the fragments; decide runs before copying anything.
        let col_lens = &mut scratch.col_lens;
        col_lens.clear();
        let col_counts = &mut scratch.col_counts;
        col_counts.clear();
        let cur_start: Vec<usize> = cursors.clone();
        for s in first..last {
            let (mut hlen, mut vlen) = (0u64, 0u64);
            for (ci, ch) in chunks.iter().enumerate() {
                let c = &mut cursors[ci];
                while *c < ch.frags.len() && (ch.frags[*c].sig as usize) < s {
                    *c += 1;
                }
                if *c < ch.frags.len() && ch.frags[*c].sig as usize == s {
                    hlen += ch.frags[*c].hlen as u64;
                    vlen += ch.frags[*c].vlen as u64;
                }
            }
            let one_bit = matches!(input.kinds[s], SignalKind::Bits { width: 1, .. });
            let len = if hlen + vlen == 0 { 0 } else if one_bit { hlen } else { varint::len_u64(hlen) as u64 + hlen + vlen };
            col_lens.push(len as u32);
            col_counts.push(scratch.entry_counts[s]);
            total_entries += scratch.entry_counts[s] as usize;
        }
        let budget = input.run_budget.max(1);
        let mut runs: Vec<(usize, usize, bool)> = Vec::new(); // (first local, count, wide-dominated)
        {
            let mut i = 0;
            while i < n {
                let mut bytes = col_lens[i] as usize;
                let mut entries = col_counts[i] as usize;
                let mut j = i + 1;
                while j < n && bytes + col_lens[j] as usize <= budget {
                    bytes += col_lens[j] as usize;
                    entries += col_counts[j] as usize;
                    j += 1;
                }
                runs.push((i, j - i, bytes >= 16 * entries.max(1)));
                i = j;
            }
        }
        varint::put_u64(data, runs.len() as u64);
        let run_blobs = &mut scratch.run_blobs;
        run_blobs.clear();
        let mut run_sizes: Vec<usize> = Vec::with_capacity(runs.len());
        // Rewind the chunk cursors and build each run's raw bytes directly.
        cursors.copy_from_slice(&cur_start);
        for &(ri, rn, wide) in &runs {
            raw.clear();
            for &l in &col_lens[ri..ri + rn] {
                varint::put_u64(raw, l as u64);
            }
            for s in first + ri..first + ri + rn {
                if col_lens[s - first] == 0 {
                    continue;
                }
                let one_bit = matches!(input.kinds[s], SignalKind::Bits { width: 1, .. });
                // Headers of all chunks, then values of all chunks.
                let save: Vec<usize> = cursors.clone();
                let mut hlen = 0u64;
                for (ci, ch) in chunks.iter().enumerate() {
                    let c = &mut cursors[ci];
                    while *c < ch.frags.len() && (ch.frags[*c].sig as usize) < s {
                        *c += 1;
                    }
                    if *c < ch.frags.len() && ch.frags[*c].sig as usize == s {
                        hlen += ch.frags[*c].hlen as u64;
                    }
                }
                if !one_bit {
                    varint::put_u64(raw, hlen);
                }
                cursors.copy_from_slice(&save);
                for (ci, ch) in chunks.iter().enumerate() {
                    let c = &mut cursors[ci];
                    while *c < ch.frags.len() && (ch.frags[*c].sig as usize) < s {
                        *c += 1;
                    }
                    if *c < ch.frags.len() && ch.frags[*c].sig as usize == s {
                        let f = ch.frags[*c];
                        raw.extend_from_slice(&ch.hdr[f.hoff as usize..(f.hoff + f.hlen) as usize]);
                    }
                }
                if !one_bit {
                    cursors.copy_from_slice(&save);
                    for (ci, ch) in chunks.iter().enumerate() {
                        let c = &mut cursors[ci];
                        while *c < ch.frags.len() && (ch.frags[*c].sig as usize) < s {
                            *c += 1;
                        }
                        if *c < ch.frags.len() && ch.frags[*c].sig as usize == s {
                            let f = ch.frags[*c];
                            raw.extend_from_slice(&ch.val[f.voff as usize..(f.voff + f.vlen) as usize]);
                        }
                    }
                }
            }
            let before = run_blobs.len();
            // Runs dominated by wide value bytes gain nothing from higher zstd levels.
            let c = if wide && comp.codec == crate::codec::Codec::Zstd && comp.level > 1 { Compression { codec: comp.codec, level: 1 } } else { comp };
            compressor.compress_into(c, raw, run_blobs)?;
            run_sizes.push(run_blobs.len() - before);
        }
        for (k, &(_, rn, _)) in runs.iter().enumerate() {
            varint::put_u64(data, rn as u64);
            varint::put_u64(data, run_sizes[k] as u64);
        }
        data.extend_from_slice(run_blobs);
        index.push((grp, (data.len() as u64 - off) as u32, off));
    }
    // --- assemble ---
    let (start, end) = match (input.times.first(), input.times.last()) {
        (Some(&a), Some(&b)) => (a, b),
        _ => (0, 0),
    };
    out.reserve(HEADER_LEN + tt_c.len() + index.len() * INDEX_ENTRY_LEN + n_groups * 4);
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&end.to_le_bytes());
    out.extend_from_slice(&(input.times.len() as u32).to_le_bytes());
    out.extend_from_slice(&(n_sig as u32).to_le_bytes());
    out.extend_from_slice(&(g as u32).to_le_bytes());
    out.extend_from_slice(&(index.len() as u32).to_le_bytes());
    out.extend_from_slice(&(n_groups as u32).to_le_bytes());
    out.extend_from_slice(&(tt_c.len() as u32).to_le_bytes());
    out.extend_from_slice(tt_c);
    for (grp, clen, off) in &index {
        out.extend_from_slice(&grp.to_le_bytes());
        out.extend_from_slice(&clen.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
    }
    for &p in &input.prev_dirty {
        out.extend_from_slice(&p.to_le_bytes());
    }
    // The group data stays in `scratch.data()`; the caller writes it after `out`.
    Ok(total_entries)
}

impl EncoderScratch {
    /// Group data area produced by the last [`finish_block`] call.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Mutable access to the group data buffer (to hand it to an I/O call without copying).
    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.data
    }
}

fn encode_column(kind: SignalKind, recs: &[Record], heap: &[u8], prev0: u32, hdr: &mut Vec<u8>, val: &mut Vec<u8>) -> Result<()> {
    let mut prev = prev0;
    match kind {
        SignalKind::Bits { width: 1, .. } => {
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = t.wrapping_sub(prev) as u64;
                prev = t;
                varint::put_u64(hdr, (dt << 4) | (r.payload & 15));
            }
        }
        SignalKind::Bits { width, states } => {
            let narrow = packed_len(width, states) <= 8;
            let compact_len = (width as usize).div_ceil(8);
            let full_len = packed_len(width, states);
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let compact = r.tidx & COMPACT_FLAG != 0 || states == 2;
                let dt = t.wrapping_sub(prev) as u64;
                prev = t;
                varint::put_u64(hdr, (dt << 1) | compact as u64);
                let len = if compact { compact_len } else { full_len };
                if narrow {
                    val.extend_from_slice(&r.payload.to_le_bytes()[..len]);
                } else {
                    let o = r.payload as usize;
                    let b = heap.get(o..o + len).ok_or(Error::State("heap offset out of range"))?;
                    val.extend_from_slice(b);
                }
            }
        }
        SignalKind::Real => {
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = t.wrapping_sub(prev) as u64;
                prev = t;
                varint::put_u64(hdr, dt);
                val.extend_from_slice(&r.payload.to_le_bytes());
            }
        }
        SignalKind::VarLen => {
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = t.wrapping_sub(prev) as u64;
                prev = t;
                varint::put_u64(hdr, dt);
                let mut hr = Reader::new(heap);
                hr.pos = r.payload as usize;
                let b = hr.blob().map_err(|_| Error::State("heap varlen out of range"))?;
                varint::put_blob(val, b);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Parsed fixed header of a block.
#[derive(Clone, Copy, Debug)]
pub struct BlockHeader {
    pub start_time: u64,
    pub end_time: u64,
    pub n_times: u32,
    pub n_signals: u32,
    pub group_size: u32,
    pub n_dirty: u32,
    pub n_groups: u32,
    pub tt_len: u32,
}

impl BlockHeader {
    pub fn parse(p: &[u8]) -> Result<BlockHeader> {
        if p.len() < HEADER_LEN {
            return Err(Error::Corrupt("signal block header truncated"));
        }
        let u64_at = |o: usize| u64::from_le_bytes(p[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        let h = BlockHeader {
            start_time: u64_at(0),
            end_time: u64_at(8),
            n_times: u32_at(16),
            n_signals: u32_at(20),
            group_size: u32_at(24),
            n_dirty: u32_at(28),
            n_groups: u32_at(32),
            tt_len: u32_at(36),
        };
        if h.data_off() > p.len() {
            return Err(Error::Corrupt("signal block index truncated"));
        }
        Ok(h)
    }
    pub fn tt_off(&self) -> usize {
        HEADER_LEN
    }
    pub fn index_off(&self) -> usize {
        HEADER_LEN + self.tt_len as usize
    }
    pub fn prev_off(&self) -> usize {
        self.index_off() + self.n_dirty as usize * INDEX_ENTRY_LEN
    }
    pub fn data_off(&self) -> usize {
        self.prev_off() + self.n_groups as usize * 4
    }
}

/// Decodes the block time table.
pub fn decode_time_table(p: &[u8], h: &BlockHeader, d: &mut Decompressor) -> Result<Vec<u64>> {
    let blob = &p[h.tt_off()..h.index_off()];
    let raw = d.decompress(blob)?;
    let mut r = Reader::new(&raw);
    let mut out = Vec::with_capacity(h.n_times as usize);
    let mut t = 0u64;
    for _ in 0..h.n_times {
        t = t.wrapping_add(r.u64()?);
        out.push(t);
    }
    Ok(out)
}

/// Finds the dirty-index entry of `group`: `(clen, offset)`.
pub fn find_group(p: &[u8], h: &BlockHeader, group: u32) -> Option<(u32, u64)> {
    let base = h.index_off();
    let n = h.n_dirty as usize;
    let entry = |i: usize| {
        let e = &p[base + i * INDEX_ENTRY_LEN..base + (i + 1) * INDEX_ENTRY_LEN];
        (
            u32::from_le_bytes(e[0..4].try_into().unwrap()),
            u32::from_le_bytes(e[4..8].try_into().unwrap()),
            u64::from_le_bytes(e[8..16].try_into().unwrap()),
        )
    };
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (g, clen, off) = entry(mid);
        if g == group {
            return Some((clen, off));
        } else if g < group {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    None
}

/// Iterates all dirty groups of a block: `(group, clen, offset)`.
pub fn dirty_groups<'a>(p: &'a [u8], h: &BlockHeader) -> impl Iterator<Item = (u32, u32, u64)> + 'a {
    let base = h.index_off();
    (0..h.n_dirty as usize).map(move |i| {
        let e = &p[base + i * INDEX_ENTRY_LEN..base + (i + 1) * INDEX_ENTRY_LEN];
        (
            u32::from_le_bytes(e[0..4].try_into().unwrap()),
            u32::from_le_bytes(e[4..8].try_into().unwrap()),
            u64::from_le_bytes(e[8..16].try_into().unwrap()),
        )
    })
}

/// Previous block in which `group` was dirty (`NO_BLOCK` if none).
pub fn prev_dirty(p: &[u8], h: &BlockHeader, group: u32) -> u32 {
    if group >= h.n_groups {
        return NO_BLOCK;
    }
    let o = h.prev_off() + group as usize * 4;
    u32::from_le_bytes(p[o..o + 4].try_into().unwrap())
}

/// Returns the (uncompressed) group container of a dirty group.
pub fn group_container<'a>(p: &'a [u8], h: &BlockHeader, clen: u32, off: u64) -> Result<&'a [u8]> {
    let s = h.data_off() + off as usize;
    let e = s + clen as usize;
    if e > p.len() {
        return Err(Error::Corrupt("group container out of range"));
    }
    Ok(&p[s..e])
}

/// Parsed structure of a group container (no decompression performed).
pub struct GroupView<'a> {
    pub container: &'a [u8],
    pub first_sig: u32,
    pub n_sigs: usize,
    /// Compressed frame blob.
    pub frame_blob: &'a [u8],
    /// Runs: (first local signal, count, compressed blob).
    pub runs: Vec<(u32, u32, &'a [u8])>,
}

impl<'a> GroupView<'a> {
    pub fn parse(container: &'a [u8], first_sig: u32, kinds: &[SignalKind]) -> Result<GroupView<'a>> {
        let mut r = Reader::new(container);
        let n = r.usize()?;
        if first_sig as usize + n > kinds.len() {
            return Err(Error::Corrupt("group exceeds signal table"));
        }
        let frame_blob = r.blob()?;
        let n_runs = r.usize()?;
        if n_runs > n {
            return Err(Error::Corrupt("too many runs"));
        }
        let mut sizes = Vec::with_capacity(n_runs);
        for _ in 0..n_runs {
            let rn = r.u32()?;
            let clen = r.usize()?;
            sizes.push((rn, clen));
        }
        let mut runs = Vec::with_capacity(n_runs);
        let mut local = 0u32;
        for (rn, clen) in sizes {
            let blob = r.bytes(clen)?;
            runs.push((local, rn, blob));
            local += rn;
        }
        if local as usize != n {
            return Err(Error::Corrupt("runs do not cover the group"));
        }
        Ok(GroupView { container, first_sig, n_sigs: n, frame_blob, runs })
    }

    /// Index of the run holding `sig`.
    pub fn run_of(&self, sig: u32) -> usize {
        let local = sig - self.first_sig;
        // Runs are few (<= n_sigs); binary search on first-local.
        let i = self.runs.partition_point(|&(f, _, _)| f <= local);
        i - 1
    }

    /// Decompresses the frame piece: returns per-signal frame slices (into `out`).
    pub fn decode_frames(&self, kinds: &[SignalKind], d: &mut Decompressor, out: &mut Vec<u8>) -> Result<Vec<(u32, u32)>> {
        out.clear();
        d.decompress_into(self.frame_blob, out)?;
        let mut r = Reader::new(out);
        let mut frames = Vec::with_capacity(self.n_sigs);
        for i in 0..self.n_sigs {
            let k = kinds[self.first_sig as usize + i];
            match k {
                SignalKind::VarLen => {
                    let len = r.usize()?;
                    let s = r.pos;
                    r.bytes(len)?;
                    frames.push((s as u32, (s + len) as u32));
                }
                k => {
                    let l = k.packed_len().unwrap();
                    let s = r.pos;
                    r.bytes(l)?;
                    frames.push((s as u32, (s + l) as u32));
                }
            }
        }
        Ok(frames)
    }

    /// Decompresses run `ri`: returns per-signal column slices (into `out`), indexed by local signal within the run.
    pub fn decode_run(&self, ri: usize, d: &mut Decompressor, out: &mut Vec<u8>) -> Result<Vec<(u32, u32)>> {
        let (_, rn, blob) = self.runs[ri];
        out.clear();
        d.decompress_into(blob, out)?;
        let mut r = Reader::new(out);
        let mut lens = Vec::with_capacity(rn as usize);
        for _ in 0..rn {
            lens.push(r.usize()?);
        }
        let mut pos = r.pos;
        let mut cols = Vec::with_capacity(rn as usize);
        for l in lens {
            if pos + l > out.len() {
                return Err(Error::Corrupt("column exceeds run"));
            }
            cols.push((pos as u32, (pos + l) as u32));
            pos += l;
        }
        Ok(cols)
    }
}

/// One decoded column entry, referring into the column bytes.
#[derive(Clone, Copy, Debug)]
pub struct RawChange {
    pub tidx: u32,
    /// Packing states of the stored value (2 for compact), 0 for reals, 255 for varlen.
    pub states: u8,
    /// Byte range of the value inside the column (vectors and variable-length values).
    pub start: u32,
    pub end: u32,
    /// Inline value: logic code for 1-bit signals, IEEE bits for reals.
    pub inline: u64,
}

pub static CODE_BYTES: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// A sparse in-memory skip index over one column (built by [`build_index`]).
#[derive(Clone, Copy, Debug)]
pub struct Checkpoint {
    /// Time index of the entry at the checkpoint.
    pub tidx: u32,
    /// Time index of the entry preceding it (needed to resume delta decoding).
    pub tidx_before: u32,
    /// Header and value cursor positions of the entry.
    pub hpos: u32,
    pub vpos: u32,
}

/// Builds a skip index with one checkpoint every `stride` entries.
pub fn build_index(col: &[u8], kind: SignalKind, stride: usize) -> Result<Vec<Checkpoint>> {
    let mut it = ColumnIter::new(col, kind);
    let mut out = Vec::with_capacity(col.len() / (stride.max(1) * 2) + 1);
    let mut i = 0usize;
    loop {
        let (hpos, vpos) = (it.h.pos, it.v.pos);
        let before = it.tidx;
        match it.next_raw()? {
            Some(c) => {
                if i % stride == 0 {
                    out.push(Checkpoint { tidx: c.tidx, tidx_before: before, hpos: hpos as u32, vpos: vpos as u32 });
                }
                i += 1;
            }
            None => break,
        }
    }
    Ok(out)
}

/// Iterator over the changes in one column.
pub struct ColumnIter<'a> {
    /// Header cursor (entry headers); for 1-bit signals the whole column.
    h: Reader<'a>,
    /// Value cursor (value stream); unused for 1-bit signals.
    v: Reader<'a>,
    kind: SignalKind,
    tidx: u32,
}

impl<'a> ColumnIter<'a> {
    /// Positions the iterator at the first entry of `col`.
    pub fn new(col: &'a [u8], kind: SignalKind) -> Self {
        if col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
            return ColumnIter { h: Reader::new(col), v: Reader::new(&[]), kind, tidx: 0 };
        }
        let mut r = Reader::new(col);
        let hlen = r.u64().unwrap_or(0) as usize;
        let hstart = r.pos;
        let hend = (hstart + hlen).min(col.len());
        ColumnIter { h: Reader { buf: &col[..hend], pos: hstart }, v: Reader { buf: col, pos: hend }, kind, tidx: 0 }
    }

    /// Jumps to the last checkpoint whose entry has time index <= `max_tidx`.
    pub fn seek(&mut self, index: &[Checkpoint], max_tidx: u32) {
        let n = index.partition_point(|c| c.tidx <= max_tidx);
        if n > 0 {
            let c = index[n - 1];
            self.tidx = c.tidx_before;
            self.h.pos = c.hpos as usize;
            self.v.pos = c.vpos as usize;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.h.is_empty()
    }

    /// Decodes the next entry; `None` at the end of the column.
    #[inline]
    pub fn next_raw(&mut self) -> Result<Option<RawChange>> {
        if self.h.is_empty() {
            return Ok(None);
        }
        Ok(Some(match self.kind {
            SignalKind::Bits { width: 1, states } => {
                let x = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add((x >> 4) as u32);
                let code = x & 15;
                let st = if code <= 1 { 2 } else { states };
                RawChange { tidx: self.tidx, states: st, start: 0, end: 0, inline: code }
            }
            SignalKind::Bits { width, states } => {
                let x = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add((x >> 1) as u32);
                let compact = x & 1 != 0;
                let (len, st) = if compact { ((width as usize).div_ceil(8), 2) } else { (packed_len(width, states), states) };
                let s = self.v.pos;
                self.v.bytes(len)?;
                RawChange { tidx: self.tidx, states: st, start: s as u32, end: (s + len) as u32, inline: 0 }
            }
            SignalKind::Real => {
                let dt = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add(dt as u32);
                let f = self.v.fixed_u64()?;
                RawChange { tidx: self.tidx, states: 0, start: 0, end: 0, inline: f }
            }
            SignalKind::VarLen => {
                let dt = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add(dt as u32);
                let len = self.v.usize()?;
                let s = self.v.pos;
                self.v.bytes(len)?;
                RawChange { tidx: self.tidx, states: 255, start: s as u32, end: (s + len) as u32, inline: 0 }
            }
        }))
    }

    /// Materialises a value returned by [`next_raw`](Self::next_raw).
    #[inline]
    pub fn value(&self, c: &RawChange) -> SignalValue<'a> {
        match self.kind {
            SignalKind::Bits { width: 1, .. } => {
                let code = (c.inline & 15) as usize;
                SignalValue::Bits { width: 1, states: c.states, data: &CODE_BYTES[code..code + 1] }
            }
            SignalKind::Bits { width, .. } => {
                SignalValue::Bits { width, states: c.states, data: &self.v.buf[c.start as usize..c.end as usize] }
            }
            SignalKind::Real => SignalValue::Real(f64::from_bits(c.inline)),
            SignalKind::VarLen => SignalValue::VarLen(&self.v.buf[c.start as usize..c.end as usize]),
        }
    }

    /// Convenience: next `(time index, value)`.
    #[inline]
    pub fn next_change(&mut self) -> Result<Option<(u32, SignalValue<'a>)>> {
        Ok(self.next_raw()?.map(|c| (c.tidx, self.value(&c))))
    }

    /// Consumes entries while their time index is <= `max_tidx`; returns the last one.
    pub fn last_at_or_before(&mut self, max_tidx: u32) -> Result<Option<RawChange>> {
        let mut found = None;
        while !self.h.is_empty() {
            let save = (self.h.pos, self.v.pos, self.tidx);
            let c = self.next_raw()?.unwrap();
            if c.tidx > max_tidx {
                self.h.pos = save.0;
                self.v.pos = save.1;
                self.tidx = save.2;
                break;
            }
            found = Some(c);
        }
        Ok(found)
    }
}

/// Frame value of a signal as a `SignalValue`.
pub fn frame_value(kind: SignalKind, frame: &[u8]) -> SignalValue<'_> {
    match kind {
        SignalKind::Bits { width, states } => SignalValue::Bits { width, states, data: frame },
        SignalKind::Real => SignalValue::Real(f64::from_le_bytes(frame[..8].try_into().unwrap())),
        SignalKind::VarLen => SignalValue::VarLen(frame),
    }
}
