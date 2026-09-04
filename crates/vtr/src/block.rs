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
//! varint n_alias | n_alias x { varint local_sig, varint target_sig }
//!                                     signals whose column in this block is byte-identical to
//!                                     `target_sig`'s (same kind); their own column is empty
//! varint frame_clen | frame blob      raw = value of every signal of the group at start_time
//!                                     in declared packing (VarLen: varint len + bytes)
//! varint n_runs
//! n_runs x { varint n_in_run, varint xform, varint clen }   runs cover the group's signals in order
//! run blobs...                        raw = varint column_len[n_in_run] then the columns
//! ```
//!
//! A run holds consecutive signals whose raw columns fit in `run_budget`
//! bytes (a single larger column forms a run of its own), so reading one
//! signal decompresses only a bounded amount of data. `xform` ([`Xform`])
//! names the value transform applied to every eligible column of the run
//! before compression; the writer picks it per run by trial-compressing a
//! small sample.
//!
//! Columns (`dt` = time-index delta from the signal's previous change in the block):
//! * 1-bit signals: a sequence of entries. `varint(dt << 1)` means the value toggled
//!   (`code = previous code ^ 1`, previous code 0 or 1); `varint((dt << 5) | (code << 1) | 1)`
//!   gives the logic code explicitly and is used for the first entry of a column and
//!   whenever the value did not toggle;
//! * every other kind: `varint((header_len << 2) | (eligible << 1) | full)`, then
//!   `header_len` bytes of entry headers, then the values concatenated in the same order
//!   (headers and values are separate streams so that the compressor sees homogeneous data).
//!   `eligible` marks a column whose entries all have the same length (`full` says which of
//!   the two packings) and that therefore carries the run's value transform:
//!   - vectors: header `varint((dt << 1) | compact)`, value `ceil(w/8)` bytes (compact,
//!     2-state packing) or `packed_len(w, states)` bytes (declared packing);
//!   - reals: header `varint(dt)`, value 8 bytes little-endian IEEE double;
//!   - variable length: header `varint(dt)`, value `varint(len)` + bytes (never eligible).

use crate::codec::{Compression, Compressor, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::SignalKind;
use crate::signal::SignalValue;
use crate::value::packed_len;
use crate::varint::{self, Reader};
use crate::xform::{self, Xform};
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
    data: Vec<u8>,
    tt: Vec<u8>,
    tt_c: Vec<u8>,
    frame_c: Vec<u8>,
    run_blobs: Vec<u8>,
    /// Per-signal column state within the current block (delta continuity across chunks).
    last: Vec<ColState>,
    /// Entries per signal within the current block.
    entry_counts: Vec<u32>,
    cursors: Vec<usize>,
    /// Column hash -> first signal with that column (dynamic aliasing), per block.
    dedup: std::collections::HashMap<(u64, u64), u32>,
    alias_of: Vec<u32>,
    cols: Vec<ColInfo>,
    /// Eligible value slices of the run being assembled: (offset in raw, len, entry width).
    segs: Vec<(u32, u32, u32)>,
    sample: Vec<u8>,
    sample_segs: Vec<(u32, u32, u32)>,
    /// Per sample segment: (index into `segs`, first entry index, entry count).
    sample_meta: Vec<(u32, u32, u32)>,
    trial_in: Vec<u8>,
    xf_tmp: Vec<u8>,
    /// Dictionary-coded value streams of the run being decided, one range per `segs` entry
    /// ((0, 0): not coded).
    dict_out: Vec<u8>,
    dict_ranges: Vec<(u32, u32)>,
    dict_table: xform::DictTable,
    dict_sample: Vec<u8>,
    /// The run rebuilt with dictionary-coded columns (`Xform::Dict`).
    raw2: Vec<u8>,
}

/// Delta-coding state of one column at a chunk boundary.
#[derive(Clone, Copy)]
struct ColState {
    tidx: u32,
    /// Previous logic code of a 1-bit column; `NO_CODE` before its first entry.
    code: u8,
}

const NO_CODE: u8 = 0xFF;

impl Default for ColState {
    fn default() -> Self {
        ColState { tidx: 0, code: NO_CODE }
    }
}

/// Layout of one column inside a run being assembled.
#[derive(Clone, Copy, Default)]
struct ColInfo {
    vlen: u32,
    /// Header word `(hlen << 2) | (eligible << 1) | full` (unused for 1-bit columns).
    word: u64,
    /// Entry width in bytes when the column is eligible for value transforms, else 0.
    w: u32,
    /// Total column bytes in the run (0: no changes, or aliased).
    len: u32,
}

impl EncoderScratch {
    /// Resets per-block state (call before the first chunk of a block).
    pub fn begin_block(&mut self) {
        for t in &mut self.last {
            *t = ColState::default();
        }
        for c in &mut self.entry_counts {
            *c = 0;
        }
    }
}

/// Entry width of a fixed-width column (`None` for mixed packings, 1-bit and varlen columns).
fn entry_width(kind: SignalKind, count: u32, vlen: u64) -> Option<(u32, bool)> {
    match kind {
        SignalKind::Bits { width, states } if width > 1 => {
            let compact = (width as u64).div_ceil(8);
            let full = packed_len(width, states) as u64;
            if vlen == count as u64 * compact {
                Some((compact as u32, false))
            } else if vlen == count as u64 * full {
                Some((full as u32, true))
            } else {
                None
            }
        }
        SignalKind::Real => Some((8, false)),
        _ => None,
    }
}

/// Sorts a chunk by signal and encodes its records into per-signal column fragments.
pub fn encode_chunk(input: &ChunkInput, kinds: &[SignalKind], scratch: &mut EncoderScratch, out: &mut ChunkEnc) -> Result<()> {
    let n_sig = input.sig_counts.len();
    if n_sig > kinds.len() {
        return Err(Error::State("sig_counts exceed signal table"));
    }
    if scratch.last.len() < n_sig {
        scratch.last.resize(n_sig, ColState::default());
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
        encode_column(kinds[s], recs, &input.heap, &mut scratch.last[s], &mut out.hdr, &mut out.val)?;
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
    // --- dynamic aliasing: signals whose column bytes equal an earlier signal's ---
    let mut alias_of = std::mem::take(&mut scratch.alias_of);
    alias_of.clear();
    alias_of.resize(n_sig, NO_BLOCK);
    scratch.dedup.clear();
    {
        let cursors = &mut scratch.cursors;
        cursors.clear();
        cursors.resize(chunks.len(), 0);
        for &grp in &input.dirty_groups {
            let first = grp as usize * g;
            let last = ((grp as usize + 1) * g).min(n_sig);
            for (s, alias) in alias_of.iter_mut().enumerate().take(last).skip(first) {
                if scratch.entry_counts[s] == 0 {
                    continue;
                }
                let kind_tag = match input.kinds[s] {
                    SignalKind::Bits { width, states } => (width as u64) << 8 | states as u64,
                    SignalKind::Real => 1 << 40,
                    SignalKind::VarLen => 2 << 40,
                };
                // Sampled hash: lengths, entry count and the head and tail of both streams.
                // `columns_equal` verifies candidates byte for byte, so collisions only cost time.
                let mut h = kind_tag.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ scratch.entry_counts[s] as u64;
                let mut total = 0usize;
                let mut parts: [(usize, usize); 2] = [(0, 0); 2]; // (chunk, frag) of the first and last fragment
                let mut n_parts = 0usize;
                for (ci, ch) in chunks.iter().enumerate() {
                    let c = &mut cursors[ci];
                    while *c < ch.frags.len() && (ch.frags[*c].sig as usize) < s {
                        *c += 1;
                    }
                    if *c < ch.frags.len() && ch.frags[*c].sig as usize == s {
                        let f = ch.frags[*c];
                        total += (f.hlen + f.vlen) as usize;
                        if n_parts == 0 {
                            parts[0] = (ci, *c);
                        }
                        parts[1] = (ci, *c);
                        n_parts += 1;
                    }
                }
                if total < 32 {
                    continue; // not worth an alias entry
                }
                for (ci, fi) in parts {
                    let (ch, f) = (&chunks[ci], chunks[ci].frags[fi]);
                    let hd = &ch.hdr[f.hoff as usize..(f.hoff + f.hlen) as usize];
                    let vl = &ch.val[f.voff as usize..(f.voff + f.vlen) as usize];
                    h = hash_bytes(h, &hd[..hd.len().min(HASH_SAMPLE)]);
                    h = hash_bytes(h, &hd[hd.len() - hd.len().min(HASH_SAMPLE)..]);
                    h = hash_bytes(h, &vl[..vl.len().min(HASH_SAMPLE)]);
                    h = hash_bytes(h, &vl[vl.len() - vl.len().min(HASH_SAMPLE)..]);
                }
                let key = (h, (kind_tag << 24) ^ total as u64);
                match scratch.dedup.get(&key) {
                    Some(&t) if columns_equal(chunks, s as u32, t) => *alias = t,
                    Some(_) => {}
                    None => {
                        scratch.dedup.insert(key, s as u32);
                    }
                }
            }
        }
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
        let aliases = alias_of[first..last].iter().enumerate().filter(|(_, &a)| a != NO_BLOCK);
        varint::put_u64(data, aliases.clone().count() as u64);
        for (local, &target) in aliases {
            varint::put_u64(data, local as u64);
            varint::put_u64(data, target as u64);
        }
        let frame_c = &mut scratch.frame_c;
        frame_c.clear();
        compressor.compress_into(comp, raw, frame_c)?;
        varint::put_u64(data, frame_c.len() as u64);
        data.extend_from_slice(frame_c);
        // Column layouts are known from the fragments; decide runs before copying anything.
        let cols = &mut scratch.cols;
        cols.clear();
        let cur_start: Vec<usize> = cursors.clone();
        for (s, &alias) in alias_of.iter().enumerate().take(last).skip(first) {
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
            let aliased = alias != NO_BLOCK;
            let mut info = ColInfo { vlen: vlen as u32, ..Default::default() };
            if hlen + vlen == 0 || aliased {
                info.len = 0;
            } else if one_bit {
                info.len = hlen as u32;
            } else {
                let (w, full) = entry_width(input.kinds[s], scratch.entry_counts[s], vlen).unwrap_or((0, false));
                info.w = w;
                info.word = (hlen << 2) | (((w > 0) as u64) << 1) | full as u64;
                info.len = (varint::len_u64(info.word) as u64 + hlen + vlen) as u32;
            }
            cols.push(info);
            total_entries += scratch.entry_counts[s] as usize;
        }
        let budget = input.run_budget.max(1);
        let mut runs: Vec<(usize, usize, bool)> = Vec::new(); // (first local, count, wide-dominated)
        {
            let mut i = 0;
            while i < n {
                let mut bytes = cols[i].len as usize;
                let mut entries = if cols[i].len == 0 { 0 } else { scratch.entry_counts[first + i] as usize };
                let mut j = i + 1;
                while j < n && j - i < MAX_RUN_SIGNALS && bytes + cols[j].len as usize <= budget {
                    bytes += cols[j].len as usize;
                    if cols[j].len != 0 {
                        entries += scratch.entry_counts[first + j] as usize;
                    }
                    j += 1;
                }
                runs.push((i, j - i, bytes >= 16 * entries.max(1)));
                i = j;
            }
        }
        varint::put_u64(data, runs.len() as u64);
        let run_blobs = &mut scratch.run_blobs;
        run_blobs.clear();
        let mut run_table: Vec<(usize, Xform, usize)> = Vec::with_capacity(runs.len());
        // Rewind the chunk cursors and build each run's raw bytes directly.
        cursors.copy_from_slice(&cur_start);
        for &(ri, rn, wide) in &runs {
            raw.clear();
            let segs = &mut scratch.segs;
            segs.clear();
            for c in &cols[ri..ri + rn] {
                varint::put_u64(raw, c.len as u64);
            }
            for s in first + ri..first + ri + rn {
                let info = cols[s - first];
                if info.len == 0 {
                    continue; // no changes, or aliased
                }
                let one_bit = matches!(input.kinds[s], SignalKind::Bits { width: 1, .. });
                if !one_bit {
                    varint::put_u64(raw, info.word);
                }
                // Headers of all chunks, then values of all chunks.
                let save: Vec<usize> = cursors.clone();
                for pass in 0..if one_bit { 1 } else { 2 } {
                    if pass == 1 {
                        cursors.copy_from_slice(&save);
                        if info.w > 0 {
                            segs.push((raw.len() as u32, info.vlen, info.w));
                        }
                    }
                    for (ci, ch) in chunks.iter().enumerate() {
                        let c = &mut cursors[ci];
                        while *c < ch.frags.len() && (ch.frags[*c].sig as usize) < s {
                            *c += 1;
                        }
                        if *c < ch.frags.len() && ch.frags[*c].sig as usize == s {
                            let f = ch.frags[*c];
                            if pass == 0 {
                                raw.extend_from_slice(&ch.hdr[f.hoff as usize..(f.hoff + f.hlen) as usize]);
                            } else {
                                raw.extend_from_slice(&ch.val[f.voff as usize..(f.voff + f.vlen) as usize]);
                            }
                        }
                    }
                }
            }
            let (x, incompressible) = if comp.codec == crate::codec::Codec::None {
                (Xform::None, false)
            } else {
                let t = TrialScratch {
                    sample: &mut scratch.sample,
                    sample_segs: &mut scratch.sample_segs,
                    sample_meta: &mut scratch.sample_meta,
                    trial_in: &mut scratch.trial_in,
                    tmp: &mut scratch.xf_tmp,
                    dict_out: &mut scratch.dict_out,
                    dict_ranges: &mut scratch.dict_ranges,
                    dict_table: &mut scratch.dict_table,
                    dict_sample: &mut scratch.dict_sample,
                };
                choose_xform(raw, segs, compressor, t)?
            };
            let run_input: &[u8] = if x == Xform::Dict {
                apply_dict(raw, &cols[ri..ri + rn], segs, &scratch.dict_out, &scratch.dict_ranges, &mut scratch.raw2);
                &scratch.raw2
            } else {
                if x != Xform::None {
                    for &(off, len, w) in segs.iter() {
                        xform::forward(x, w as usize, &mut raw[off as usize..(off + len) as usize], &mut scratch.xf_tmp);
                    }
                }
                raw
            };
            let before = run_blobs.len();
            // Runs dominated by wide value bytes gain nothing from higher zstd levels.
            let c = if wide && comp.codec == crate::codec::Codec::Zstd && comp.level > 1 { Compression { codec: comp.codec, level: 1 } } else { comp };
            compressor.compress_into_probed(c, run_input, run_blobs, incompressible)?;
            run_table.push((rn, x, run_blobs.len() - before));
        }
        for &(rn, x, clen) in &run_table {
            varint::put_u64(data, rn as u64);
            varint::put_u64(data, x as u64);
            varint::put_u64(data, clen as u64);
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
    scratch.alias_of = alias_of;
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

/// Signals per column run at most: bounds the decompression a single-signal read pays in
/// designs whose columns are tiny (many signals, few changes each).
const MAX_RUN_SIGNALS: usize = 64;

/// Bytes hashed at each end of a column stream when looking for dynamic aliases.
const HASH_SAMPLE: usize = 64;

/// Sample bytes per candidate in the transform trial.
const TRIAL_BYTES: usize = 1024;
/// Consecutive entries per sampled segment (delta and match structure need neighbours).
const TRIAL_SEG: usize = 32;
/// A transform is used only when its sample is at least this much smaller than the plain
/// sample (percent): marginal gains do not pay for the extra pass when reading.
const TRIAL_MIN_GAIN: usize = 4;

/// Scratch buffers of the transform trial (fields of [`EncoderScratch`]).
struct TrialScratch<'a> {
    sample: &'a mut Vec<u8>,
    sample_segs: &'a mut Vec<(u32, u32, u32)>,
    sample_meta: &'a mut Vec<(u32, u32, u32)>,
    trial_in: &'a mut Vec<u8>,
    tmp: &'a mut Vec<u8>,
    dict_out: &'a mut Vec<u8>,
    dict_ranges: &'a mut Vec<(u32, u32)>,
    dict_table: &'a mut xform::DictTable,
    dict_sample: &'a mut Vec<u8>,
}

/// Picks the value transform for a run by compressing a sample of its eligible value
/// slices (`segs`: offset, len, entry width) with fast zstd under each candidate. Also
/// reports whether the plain sample looked incompressible (the codec probe's verdict).
fn choose_xform(raw: &[u8], segs: &[(u32, u32, u32)], compressor: &mut Compressor, t: TrialScratch<'_>) -> Result<(Xform, bool)> {
    let TrialScratch { sample, sample_segs, sample_meta, trial_in, tmp, dict_out, dict_ranges, dict_table, dict_sample } = t;
    let total: u64 = segs.iter().map(|s| s.1 as u64).sum();
    if total < 64 {
        return Ok((Xform::None, false));
    }
    sample.clear();
    sample_segs.clear();
    sample_meta.clear();
    for (si, &(off, len, w)) in segs.iter().enumerate() {
        let (off, len, w) = (off as usize, len as usize, w as usize);
        let count = len / w;
        let want = ((len as u64 * TRIAL_BYTES as u64 / total) as usize).max(w);
        let m = (want / w).clamp(1, count);
        let seg = m.min(TRIAL_SEG);
        let k = m.div_ceil(seg);
        let step = count / k;
        for j in 0..k {
            let i0 = (j * step).min(count - seg);
            sample_segs.push((sample.len() as u32, (seg * w) as u32, w as u32));
            sample_meta.push((si as u32, i0 as u32, seg as u32));
            sample.extend_from_slice(&raw[off + i0 * w..off + (i0 + seg) * w]);
        }
    }
    let mut best = (Xform::None, usize::MAX);
    let mut incompressible = false;
    for x in Xform::IN_PLACE {
        trial_in.clear();
        trial_in.extend_from_slice(sample);
        for &(off, len, w) in sample_segs.iter() {
            xform::forward(x, w as usize, &mut trial_in[off as usize..(off + len) as usize], tmp);
        }
        let size = compressor.trial_size(trial_in)?;
        if x == Xform::None {
            incompressible = Compressor::sample_incompressible(size, sample.len());
            best = (x, size * (100 - TRIAL_MIN_GAIN) / 100);
        } else if size < best.1 {
            best = (x, size);
        }
    }
    // Dictionary candidate. A sample cannot tell how many distinct values a column has,
    // so it is used twice: first, dictionaries built from the sample alone estimate
    // whether codes beat the best in-place transform at all (columns whose sample
    // already shows too many distinct entries stay plain); only then are the promising
    // columns coded whole, and the trial is repeated with their real codes and the
    // dictionaries' share of the bytes.
    dict_out.clear();
    dict_ranges.clear();
    dict_ranges.resize(segs.len(), (0, 0));
    trial_in.clear();
    let mut est_dict = 0usize;
    let mut est_codes = 0.0f64; // order-0 entropy of the sampled codes, in bytes
    let mut plain_bytes = 0usize; // sample bytes of the columns that stay plain
    let mut any = false;
    let mut k = 0usize; // sample segments come in `segs` order
    for (si, &(_, len, w)) in segs.iter().enumerate() {
        let (len, w) = (len as usize, w as usize);
        dict_sample.clear();
        let mut m = 0usize;
        while k < sample_segs.len() && sample_meta[k].0 as usize == si {
            let (soff, slen, _) = sample_segs[k];
            dict_sample.extend_from_slice(&sample[soff as usize..(soff + slen) as usize]);
            m += sample_meta[k].2 as usize;
            k += 1;
        }
        let count = len / w;
        tmp.clear();
        // Worth a look only when the sampled values recur (a column with 256 distinct
        // values spread evenly shows about 70% distinct entries in a 200-entry sample).
        let coded = if w < 2 || m < 8 {
            None
        } else {
            xform::dict_encode(w, dict_sample, tmp, dict_table, (m / 2).min(xform::DICT_MAX)).filter(|&d| d <= DICT_SAMPLE_MAX || d * count <= xform::DICT_MAX * m)
        };
        match coded {
            Some(d) => {
                trial_in.extend_from_slice(&tmp[tmp.len() - m..]);
                let mf = m as f64;
                est_codes += dict_table.counts().iter().map(|&c| c as f64 * (mf / c as f64).log2()).sum::<f64>() / 8.0;
                let nd = (d * count).div_ceil(m).clamp(d, xform::DICT_MAX);
                est_dict += varint::len_u64(nd as u64) + nd * w;
                dict_ranges[si] = (1, 0); // marked: worth coding whole
                any = true;
            }
            None => {
                trial_in.extend_from_slice(dict_sample);
                plain_bytes += dict_sample.len();
            }
        }
    }
    let mut dict_bytes = 0usize;
    let est_dict = (est_dict as u64 * sample.len() as u64 / total) as usize;
    // Order-0 coding of the codes is a pessimistic stand-in for zstd (which also finds
    // repeats); the plain columns are assumed to compress as in the best candidate.
    // Only a run that passes this free estimate pays for a trial compression.
    let est_h0 = est_codes as usize + est_dict + best.1 * plain_bytes / sample.len().max(1);
    let est_total = if any && est_h0 * 100 < best.1 * (100 - DICT_MIN_GAIN) { compressor.trial_size(trial_in)? + est_dict } else { usize::MAX };
    if est_total != usize::MAX && est_total * 100 < best.1 * (100 - DICT_MIN_GAIN) {
        for (si, &(off, len, w)) in segs.iter().enumerate() {
            if dict_ranges[si] != (1, 0) {
                continue;
            }
            dict_ranges[si] = (0, 0);
            let (off, len, w) = (off as usize, len as usize, w as usize);
            let start = dict_out.len();
            if let Some(nd) = xform::dict_encode(w, &raw[off..off + len], dict_out, dict_table, xform::DICT_MAX) {
                dict_ranges[si] = (start as u32, dict_out.len() as u32);
                dict_bytes += varint::len_u64(nd as u64) + nd * w;
            }
        }
    } else {
        dict_ranges.iter_mut().for_each(|r| *r = (0, 0));
    }
    if dict_bytes > 0 {
        trial_in.clear();
        for (k, &(soff, slen, w)) in sample_segs.iter().enumerate() {
            let (si, i0, cnt) = sample_meta[k];
            let (a, b) = dict_ranges[si as usize];
            if a == b {
                trial_in.extend_from_slice(&sample[soff as usize..(soff + slen) as usize]);
            } else {
                let coded = &dict_out[a as usize..b as usize];
                let mut r = Reader::new(coded);
                let nd = r.u64()? as usize;
                let codes = r.pos + nd * w as usize;
                trial_in.extend_from_slice(&coded[codes + i0 as usize..codes + (i0 + cnt) as usize]);
            }
        }
        let size = compressor.trial_size(trial_in)? + (dict_bytes as u64 * sample.len() as u64 / total) as usize;
        if size * 100 < best.1 * (100 - DICT_MIN_GAIN) {
            best = (Xform::Dict, size);
        }
    }
    Ok((best.0, incompressible && best.0 == Xform::None))
}

/// The dictionary candidate must beat the best in-place candidate by this much (percent)
/// on the sample. Its sample estimate is optimistic (32-entry segments favour codes over
/// the long-range matches zstd finds in plain or shuffled values): at 0% it lost on
/// three of four dictionary runs of the scr1_x8 workload (+0.9% file), at 20% the file
/// shrinks on every workload measured (scr1_x8 -0.2%, scr1_axi -0.6%, C910 -2.6%).
const DICT_MIN_GAIN: usize = 20;

/// A column whose sample shows more distinct entries than this is dictionary-coded only
/// when the sample's rate of distinct values, scaled to the column, still fits `DICT_MAX`.
const DICT_SAMPLE_MAX: usize = 64;

/// Rewrites a run's raw bytes with dictionary-coded value streams (`Xform::Dict`): the
/// value stream of every eligible column becomes its entry of `dict_out` when it was
/// coded, else a zero count followed by the plain values; column lengths follow.
fn apply_dict(raw: &[u8], cols: &[ColInfo], segs: &[(u32, u32, u32)], dict_out: &[u8], dict_ranges: &[(u32, u32)], out: &mut Vec<u8>) {
    out.clear();
    let mut r = Reader::new(raw);
    for _ in cols {
        let _ = r.u64();
    }
    // (column start in raw, new length, seg index or NONE)
    let mut plan: Vec<(usize, usize, usize)> = Vec::with_capacity(cols.len());
    let mut pos = r.pos;
    let mut si = 0usize;
    for c in cols {
        let len = c.len as usize;
        let mut new_len = len;
        let mut seg = usize::MAX;
        if len > 0 && c.w > 0 {
            let (off, slen, _) = segs[si];
            let (a, b) = dict_ranges[si];
            let vals = if a == b { 1 + slen as usize } else { (b - a) as usize };
            new_len = (off as usize - pos) + vals;
            seg = si;
            si += 1;
        }
        plan.push((pos, new_len, seg));
        pos += len;
    }
    for &(_, new_len, _) in &plan {
        varint::put_u64(out, new_len as u64);
    }
    for (k, &(start, _, seg)) in plan.iter().enumerate() {
        let len = cols[k].len as usize;
        if seg == usize::MAX {
            out.extend_from_slice(&raw[start..start + len]);
            continue;
        }
        let (off, slen, _) = segs[seg];
        out.extend_from_slice(&raw[start..off as usize]);
        let (a, b) = dict_ranges[seg];
        if a == b {
            out.push(0);
            out.extend_from_slice(&raw[off as usize..(off + slen) as usize]);
        } else {
            out.extend_from_slice(&dict_out[a as usize..b as usize]);
        }
    }
}

/// Fast 64-bit hash over `bytes`, continuing from `h` (8 bytes per step, multiply-rotate mix).
#[inline]
fn hash_bytes(mut h: u64, bytes: &[u8]) -> u64 {
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().unwrap());
        h = (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut b = [0u8; 8];
        b[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(b) ^ (rest.len() as u64) << 56).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    h
}

/// Byte-exact comparison of two signals' column streams across the chunks (headers, then values).
fn columns_equal(chunks: &[ChunkEnc], a: u32, b: u32) -> bool {
    fn parts<'a>(chunks: &'a [ChunkEnc], s: u32, pass: usize) -> impl Iterator<Item = &'a [u8]> + 'a {
        chunks.iter().filter_map(move |ch| {
            ch.frags.binary_search_by_key(&s, |f| f.sig).ok().map(|i| {
                let f = ch.frags[i];
                if pass == 0 {
                    &ch.hdr[f.hoff as usize..(f.hoff + f.hlen) as usize]
                } else {
                    &ch.val[f.voff as usize..(f.voff + f.vlen) as usize]
                }
            })
        })
    }
    for pass in 0..2 {
        let (mut ia, mut ib) = (parts(chunks, a, pass), parts(chunks, b, pass));
        let (mut sa, mut sb): (&[u8], &[u8]) = (&[], &[]);
        loop {
            if sa.is_empty() {
                if let Some(x) = ia.next() {
                    sa = x;
                    continue;
                }
            }
            if sb.is_empty() {
                if let Some(x) = ib.next() {
                    sb = x;
                    continue;
                }
            }
            if sa.is_empty() || sb.is_empty() {
                if sa.is_empty() != sb.is_empty() {
                    return false;
                }
                break;
            }
            let n = sa.len().min(sb.len());
            if sa[..n] != sb[..n] {
                return false;
            }
            sa = &sa[n..];
            sb = &sb[n..];
        }
    }
    true
}

fn encode_column(kind: SignalKind, recs: &[Record], heap: &[u8], state: &mut ColState, hdr: &mut Vec<u8>, val: &mut Vec<u8>) -> Result<()> {
    let mut prev = state.tidx;
    match kind {
        SignalKind::Bits { width: 1, .. } => {
            let mut code = state.code;
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = t.wrapping_sub(prev) as u64;
                prev = t;
                let c = (r.payload & 15) as u8;
                if code <= 1 && c == code ^ 1 {
                    varint::put_u64(hdr, dt << 1);
                } else {
                    varint::put_u64(hdr, (dt << 5) | ((c as u64) << 1) | 1);
                }
                code = c;
            }
            state.code = code;
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
    state.tidx = prev;
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
    pub runs: Vec<Run<'a>>,
    /// Dynamic aliases: (signal, target signal) ascending by signal.
    pub aliases: Vec<(u32, u32)>,
}

/// One column run of a group.
#[derive(Clone, Copy, Debug)]
pub struct Run<'a> {
    /// First signal of the run, relative to the group.
    pub first_local: u32,
    pub count: u32,
    /// Value transform applied to the run's eligible columns.
    pub xform: Xform,
    pub blob: &'a [u8],
}

impl<'a> GroupView<'a> {
    pub fn parse(container: &'a [u8], first_sig: u32, kinds: &[SignalKind]) -> Result<GroupView<'a>> {
        let mut r = Reader::new(container);
        let n = r.usize()?;
        if first_sig as usize + n > kinds.len() {
            return Err(Error::Corrupt("group exceeds signal table"));
        }
        let n_alias = r.usize()?;
        if n_alias > n {
            return Err(Error::Corrupt("too many aliases"));
        }
        let mut aliases = Vec::with_capacity(n_alias);
        for _ in 0..n_alias {
            let local = r.u32()?;
            let target = r.u32()?;
            if local as usize >= n || target as usize >= kinds.len() {
                return Err(Error::Corrupt("alias out of range"));
            }
            aliases.push((first_sig + local, target));
        }
        let frame_blob = r.blob()?;
        let n_runs = r.usize()?;
        if n_runs > n {
            return Err(Error::Corrupt("too many runs"));
        }
        let mut sizes = Vec::with_capacity(n_runs);
        for _ in 0..n_runs {
            let rn = r.u32()?;
            let x = Xform::from_u8(r.u8()?).ok_or(Error::Corrupt("unknown value transform"))?;
            let clen = r.usize()?;
            sizes.push((rn, x, clen));
        }
        let mut runs = Vec::with_capacity(n_runs);
        let mut local = 0u32;
        for (rn, x, clen) in sizes {
            let blob = r.bytes(clen)?;
            runs.push(Run { first_local: local, count: rn, xform: x, blob });
            local += rn;
        }
        if local as usize != n {
            return Err(Error::Corrupt("runs do not cover the group"));
        }
        Ok(GroupView { container, first_sig, n_sigs: n, frame_blob, runs, aliases })
    }

    /// Target signal when `sig`'s column in this block is an alias.
    pub fn alias_of(&self, sig: u32) -> Option<u32> {
        self.aliases.binary_search_by_key(&sig, |a| a.0).ok().map(|i| self.aliases[i].1)
    }

    /// Index of the run holding `sig`.
    pub fn run_of(&self, sig: u32) -> usize {
        let local = sig - self.first_sig;
        // Runs are few (<= n_sigs); binary search on first-local.
        let i = self.runs.partition_point(|r| r.first_local <= local);
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

    /// Decompresses run `ri`: returns per-signal column slices (into `out`), indexed by local
    /// signal within the run. Columns are still in the run's transformed form (see
    /// [`untransform_column`]).
    pub fn decode_run(&self, ri: usize, d: &mut Decompressor, out: &mut Vec<u8>) -> Result<Vec<(u32, u32)>> {
        let Run { count: rn, blob, .. } = self.runs[ri];
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

/// Splits a non-1-bit column into `(header stream, value stream, entry width if eligible)`.
fn split_column(col: &[u8], kind: SignalKind) -> Result<(&[u8], &[u8], Option<usize>)> {
    let mut r = Reader::new(col);
    let word = r.u64()?;
    let hlen = (word >> 2) as usize;
    if r.pos + hlen > col.len() {
        return Err(Error::Corrupt("column header stream exceeds column"));
    }
    let w = if word & 2 == 0 {
        None
    } else {
        match kind {
            SignalKind::Bits { width, states } => Some(if word & 1 != 0 { packed_len(width, states) } else { (width as usize).div_ceil(8) }),
            SignalKind::Real => Some(8),
            SignalKind::VarLen => return Err(Error::Corrupt("variable-length column marked eligible")),
        }
    };
    Ok((&col[r.pos..r.pos + hlen], &col[r.pos + hlen..], w))
}

/// Undoes the run's value transform on one column, which may change its length
/// (`Xform::Dict`); no-op for `Xform::None`, 1-bit columns and ineligible columns.
pub fn untransform_column_vec(col: &mut Vec<u8>, kind: SignalKind, x: Xform, tmp: &mut Vec<u8>) -> Result<()> {
    if x != Xform::Dict {
        return untransform_column(col, kind, x, tmp);
    }
    if col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
        return Ok(());
    }
    let (_, val, w) = split_column(col, kind)?;
    let Some(w) = w else { return Ok(()) };
    let vstart = col.len() - val.len();
    tmp.clear();
    xform::dict_decode(w, val, tmp)?;
    col.truncate(vstart);
    col.extend_from_slice(tmp);
    Ok(())
}

/// Undoes the run's value transform on every column of a decoded run (`data`, with the
/// columns' byte ranges in `ranges`); rewrites both for `Xform::Dict`.
pub fn untransform_run(data: &mut Vec<u8>, ranges: &mut [(u32, u32)], first_sig: u32, kinds: &[SignalKind], x: Xform) -> Result<()> {
    if x == Xform::None {
        return Ok(());
    }
    let mut tmp = Vec::new();
    if x != Xform::Dict {
        for (k, &(a, b)) in ranges.iter().enumerate() {
            untransform_column(&mut data[a as usize..b as usize], kinds[first_sig as usize + k], x, &mut tmp)?;
        }
        return Ok(());
    }
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut col = Vec::new();
    for (k, r) in ranges.iter_mut().enumerate() {
        col.clear();
        col.extend_from_slice(&data[r.0 as usize..r.1 as usize]);
        untransform_column_vec(&mut col, kinds[first_sig as usize + k], x, &mut tmp)?;
        let a = out.len();
        out.extend_from_slice(&col);
        *r = (a as u32, out.len() as u32);
    }
    *data = out;
    Ok(())
}

/// Undoes an in-place value transform on one column (no-op for `Xform::None`,
/// 1-bit columns and columns that were not eligible).
pub fn untransform_column(col: &mut [u8], kind: SignalKind, x: Xform, tmp: &mut Vec<u8>) -> Result<()> {
    debug_assert!(x != Xform::Dict);
    if x == Xform::None || col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
        return Ok(());
    }
    let (hdr, val, w) = split_column(col, kind)?;
    if let Some(w) = w {
        if val.len() % w != 0 {
            return Err(Error::Corrupt("eligible column length is not a multiple of its entry width"));
        }
        let vstart = col.len() - val.len();
        let _ = hdr;
        xform::inverse(x, w, &mut col[vstart..], tmp);
    }
    Ok(())
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
    /// Inline value: logic code for 1-bit signals, IEEE bits for reals, position of the
    /// length prefix for variable-length values.
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
    /// Logic code of the preceding entry (1-bit columns).
    pub code_before: u8,
}

/// Builds a skip index with one checkpoint every `stride` entries.
pub fn build_index(col: &[u8], kind: SignalKind, stride: usize) -> Result<Vec<Checkpoint>> {
    let mut it = ColumnIter::new(col, kind);
    let mut out = Vec::with_capacity(col.len() / (stride.max(1) * 2) + 1);
    let mut i = 0usize;
    loop {
        let (hpos, vpos) = (it.h.pos, it.v.pos);
        let (before, code_before) = (it.tidx, it.code);
        match it.next_raw()? {
            Some(c) => {
                if i % stride == 0 {
                    out.push(Checkpoint { tidx: c.tidx, tidx_before: before, hpos: hpos as u32, vpos: vpos as u32, code_before });
                }
                i += 1;
            }
            None => break,
        }
    }
    Ok(out)
}

/// Iterator over the changes in one column.
#[derive(Clone, Copy)]
pub struct ColumnIter<'a> {
    /// Header cursor (entry headers); for 1-bit signals the whole column.
    h: Reader<'a>,
    /// Value cursor (value stream); unused for 1-bit signals.
    v: Reader<'a>,
    kind: SignalKind,
    tidx: u32,
    /// Previous logic code (1-bit columns); `NO_CODE` before the first entry.
    code: u8,
}

impl<'a> ColumnIter<'a> {
    /// Positions the iterator at the first entry of `col` (a column in plain, untransformed form).
    pub fn new(col: &'a [u8], kind: SignalKind) -> Self {
        if col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
            return ColumnIter { h: Reader::new(col), v: Reader::new(&[]), kind, tidx: 0, code: NO_CODE };
        }
        let mut r = Reader::new(col);
        let hlen = (r.u64().unwrap_or(0) >> 2) as usize;
        let hstart = r.pos;
        let hend = (hstart + hlen).min(col.len());
        ColumnIter { h: Reader { buf: &col[..hend], pos: hstart }, v: Reader { buf: col, pos: hend }, kind, tidx: 0, code: NO_CODE }
    }

    /// Jumps to the last checkpoint whose entry has time index <= `max_tidx`.
    pub fn seek(&mut self, index: &[Checkpoint], max_tidx: u32) {
        let n = index.partition_point(|c| c.tidx <= max_tidx);
        if n > 0 {
            let c = index[n - 1];
            self.tidx = c.tidx_before;
            self.code = c.code_before;
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
                let (dt, code) = if x & 1 == 0 { (x >> 1, (self.code ^ 1) as u64) } else { (x >> 5, (x >> 1) & 15) };
                if code > 8 {
                    return Err(Error::Corrupt("bad logic code in 1-bit column"));
                }
                self.code = code as u8;
                self.tidx = self.tidx.wrapping_add(dt as u32);
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
                let lpos = self.v.pos;
                let len = self.v.usize()?;
                let s = self.v.pos;
                self.v.bytes(len)?;
                RawChange { tidx: self.tidx, states: 255, start: s as u32, end: (s + len) as u32, inline: lpos as u64 }
            }
        }))
    }

    /// Fast path for 1-bit columns: calls `f(time index, logic code)` for every remaining
    /// entry. Same decoding as [`next_raw`](Self::next_raw) with the state kept in registers.
    pub fn for_each_bit(&mut self, mut f: impl FnMut(u32, u8)) -> Result<()> {
        debug_assert!(matches!(self.kind, SignalKind::Bits { width: 1, .. }));
        let buf = self.h.buf;
        let (mut pos, mut tidx, mut code) = (self.h.pos, self.tidx, self.code);
        while pos < buf.len() {
            let x = match buf[pos] {
                b if b < 0x80 => {
                    pos += 1;
                    b as u64
                }
                _ => {
                    let mut r = Reader { buf, pos };
                    let x = r.u64()?;
                    pos = r.pos;
                    x
                }
            };
            let (dt, c) = if x & 1 == 0 { (x >> 1, code ^ 1) } else { (x >> 5, ((x >> 1) & 15) as u8) };
            if c > 8 {
                return Err(Error::Corrupt("bad logic code in 1-bit column"));
            }
            code = c;
            tidx = tidx.wrapping_add(dt as u32);
            f(tidx, code);
        }
        self.h.pos = pos;
        self.tidx = tidx;
        self.code = code;
        Ok(())
    }

    /// Value bytes of an entry (vectors and variable-length values).
    #[inline]
    pub fn value_bytes(&self, c: &RawChange) -> &'a [u8] {
        &self.v.buf[c.start as usize..c.end as usize]
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
            let save = (self.h.pos, self.v.pos, self.tidx, self.code);
            let c = self.next_raw()?.unwrap();
            if c.tidx > max_tidx {
                self.h.pos = save.0;
                self.v.pos = save.1;
                self.tidx = save.2;
                self.code = save.3;
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
