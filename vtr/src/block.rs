//! Signal value-change blocks: encoding (writer side) and decoding (reader side).

use crate::codec::{Compression, Compressor, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::SignalKind;
use crate::signal::SignalValue;
use crate::signal::packed_len;
use crate::varint::{self, Reader};
use crate::xform::{self, Xform};
use std::sync::Arc;

pub const HEADER_LEN: usize = 40;
pub const INDEX_ENTRY_LEN: usize = 16;
pub const NO_BLOCK: u32 = u32::MAX;
pub const COMPACT_FLAG: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Record {
    pub sig: u32,
    pub tidx: u32,
    pub payload: u64,
}

#[inline]
#[must_use]
pub fn is_narrow(kind: SignalKind) -> bool {
    match kind {
        SignalKind::Bits { width, states } => packed_len(width, states) <= 8,
        SignalKind::Real => true,
        SignalKind::VarLen => false,
    }
}

pub struct ChunkInput {
    pub records: Vec<Record>,
    pub heap: Vec<u8>,
    pub sig_counts: Vec<u32>,
    pub kinds: Arc<Vec<SignalKind>>,
    pub wide: Vec<(u32, u32, Vec<u8>, Vec<u8>)>,
    pub times: Vec<u64>,
    pub frame: Vec<u8>,
    pub frame_parts: Vec<(u32, u32)>,
    pub ends_block: bool,
}

#[derive(Default)]
pub struct ChunkEnc {
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

#[derive(Clone, Copy)]
pub(crate) struct ColumnFragment<'a> {
    signal: u32,
    headers: &'a [u8],
    values: &'a [u8],
}

pub(crate) trait ColumnFragments {
    fn len(&self) -> usize;
    fn fragment(&self, index: usize) -> ColumnFragment<'_>;

    fn seek(&self, signal: u32, cursor: &mut usize) -> Option<ColumnFragment<'_>> {
        while *cursor < self.len() {
            let fragment = self.fragment(*cursor);
            if fragment.signal >= signal {
                return (fragment.signal == signal).then_some(fragment);
            }
            *cursor += 1;
        }
        None
    }
    fn find(&self, signal: u32) -> Option<ColumnFragment<'_>> {
        let (mut first, mut last) = (0, self.len());
        while first < last {
            let middle = first + (last - first) / 2;
            let fragment = self.fragment(middle);
            match fragment.signal.cmp(&signal) {
                std::cmp::Ordering::Less => first = middle + 1,
                std::cmp::Ordering::Greater => last = middle,
                std::cmp::Ordering::Equal => return Some(fragment),
            }
        }
        None
    }
}
impl ColumnFragments for ChunkEnc {
    fn len(&self) -> usize {
        self.frags.len()
    }
    fn fragment(&self, index: usize) -> ColumnFragment<'_> {
        let f = self.frags[index];
        ColumnFragment {
            signal: f.sig,
            headers: &self.hdr[f.hoff as usize..(f.hoff + f.hlen) as usize],
            values: &self.val[f.voff as usize..(f.voff + f.vlen) as usize],
        }
    }
}

pub(crate) struct EncodedColumns(pub Vec<(u32, u32, Vec<u8>, Vec<u8>)>);
impl ColumnFragments for EncodedColumns {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn fragment(&self, index: usize) -> ColumnFragment<'_> {
        let (signal, _, headers, values) = &self.0[index];
        ColumnFragment {
            signal: *signal,
            headers,
            values,
        }
    }
}

pub struct BlockInput {
    pub block_index: u32,
    pub times: Vec<u64>,
    pub kinds: Arc<Vec<SignalKind>>,
    pub n_signals: u32,
    pub group_size: u32,
    pub dirty_groups: Vec<u32>,
    pub frame: Vec<u8>,
    pub prev_dirty: Vec<u32>,
    pub run_budget: usize,
}

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
    last: Vec<ColState>,
    entry_counts: Vec<u32>,
    cursors: Vec<usize>,
    dedup: std::collections::HashMap<(u64, u64), u32>,
    alias_of: Vec<u32>,
    cols: Vec<ColInfo>,
    segs: Vec<(u32, u32, u32)>,
    sample: Vec<u8>,
    sample_segs: Vec<(u32, u32, u32)>,
    sample_meta: Vec<(u32, u32, u32)>,
    trial_in: Vec<u8>,
    xf_tmp: Vec<u8>,
    dict_out: Vec<u8>,
    dict_ranges: Vec<(u32, u32)>,
    dict_table: xform::DictTable,
    dict_sample: Vec<u8>,
    raw2: Vec<u8>,
}

#[derive(Clone, Copy)]
struct ColState {
    tidx: u32,
    code: u8,
}

const NO_CODE: u8 = 0xFF;

impl Default for ColState {
    fn default() -> Self {
        ColState {
            tidx: 0,
            code: NO_CODE,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct ColInfo {
    vlen: u32,
    word: u64,
    w: u32,
    len: u32,
}

impl EncoderScratch {
    pub(crate) fn encoded_columns(&mut self, columns: &EncodedColumns, signals: usize) {
        self.entry_counts.clear();
        self.entry_counts.resize(signals, 0);
        for &(signal, count, _, _) in &columns.0 {
            self.entry_counts[signal as usize] = count;
        }
    }
    pub fn begin_block(&mut self) {
        for t in &mut self.last {
            *t = ColState::default();
        }
        self.entry_counts.fill(0);
    }
}

fn entry_width(kind: SignalKind, count: u32, vlen: u64) -> Option<(u32, bool)> {
    match kind {
        SignalKind::Bits { width, states } if width > 1 => {
            let compact = u64::from(width).div_ceil(8);
            let full = packed_len(width, states) as u64;
            if vlen == u64::from(count) * compact {
                Some((compact as u32, false))
            } else if vlen == u64::from(count) * full {
                Some((full as u32, true))
            } else {
                None
            }
        }
        SignalKind::Real => Some((8, false)),
        _ => None,
    }
}

pub fn encode_chunk(
    input: &ChunkInput,
    kinds: &[SignalKind],
    scratch: &mut EncoderScratch,
    out: &mut ChunkEnc,
) -> Result<()> {
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
            let signal = r.sig as usize;
            let p = fill
                .get_mut(signal)
                .ok_or(Error::State("record signal exceeds signal table"))?;
            if *p >= counts[signal + 1] {
                return Err(Error::State("record exceeds its signal count"));
            }
            sorted[*p as usize] = *r;
            *p += 1;
        }
    }
    out.frags.clear();
    out.hdr.clear();
    out.val.clear();
    let mut wide_pos = 0usize;
    for s in 0..n_sig {
        let recs = &sorted[counts[s] as usize..counts[s + 1] as usize];
        while wide_pos < input.wide.len() && (input.wide[wide_pos].0 as usize) < s {
            wide_pos += 1;
        }
        if wide_pos < input.wide.len() && input.wide[wide_pos].0 as usize == s {
            let (_, cnt, ref h, ref v) = input.wide[wide_pos];
            let f = Frag {
                sig: s as u32,
                hoff: out.hdr.len() as u32,
                hlen: h.len() as u32,
                voff: out.val.len() as u32,
                vlen: v.len() as u32,
            };
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
        encode_column(
            kinds[s],
            recs,
            &input.heap,
            &mut scratch.last[s],
            &mut out.hdr,
            &mut out.val,
        )?;
        scratch.entry_counts[s] += recs.len() as u32;
        out.frags.push(Frag {
            sig: s as u32,
            hoff: hoff as u32,
            hlen: (out.hdr.len() - hoff) as u32,
            voff: voff as u32,
            vlen: (out.val.len() - voff) as u32,
        });
    }
    Ok(())
}

pub fn finish_block(
    input: &BlockInput,
    chunks: &[ChunkEnc],
    comp: Compression,
    compressor: &mut Compressor,
    scratch: &mut EncoderScratch,
    out: &mut Vec<u8>,
) -> Result<usize> {
    finish_fragments(input, chunks, comp, compressor, scratch, out)
}

pub(crate) fn finish_fragments<C: ColumnFragments>(
    input: &BlockInput,
    chunks: &[C],
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
                    SignalKind::Bits { width, states } => u64::from(width) << 8 | u64::from(states),
                    SignalKind::Real => 1 << 40,
                    SignalKind::VarLen => 2 << 40,
                };
                let mut h = kind_tag.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    ^ u64::from(scratch.entry_counts[s]);
                let mut total = 0usize;
                let mut parts: [(usize, usize); 2] = [(0, 0); 2];
                let mut n_parts = 0usize;
                for (ci, ch) in chunks.iter().enumerate() {
                    let c = &mut cursors[ci];
                    if let Some(f) = ch.seek(s as u32, c) {
                        total += f.headers.len() + f.values.len();
                        if n_parts == 0 {
                            parts[0] = (ci, *c);
                        }
                        parts[1] = (ci, *c);
                        n_parts += 1;
                    }
                }
                if total < 32 {
                    continue;
                }
                for (ci, fi) in parts {
                    let f = chunks[ci].fragment(fi);
                    let (hd, vl) = (f.headers, f.values);
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
        let aliases = alias_of[first..last]
            .iter()
            .enumerate()
            .filter(|&(_, &a)| a != NO_BLOCK);
        varint::put_u64(data, aliases.clone().count() as u64);
        for (local, &target) in aliases {
            varint::put_u64(data, local as u64);
            varint::put_u64(data, u64::from(target));
        }
        let frame_c = &mut scratch.frame_c;
        frame_c.clear();
        compressor.compress_into(comp, raw, frame_c)?;
        varint::put_u64(data, frame_c.len() as u64);
        data.extend_from_slice(frame_c);
        let cols = &mut scratch.cols;
        cols.clear();
        let cur_start: Vec<usize> = cursors.clone();
        for (s, &alias) in alias_of.iter().enumerate().take(last).skip(first) {
            let (mut hlen, mut vlen) = (0u64, 0u64);
            for (ci, ch) in chunks.iter().enumerate() {
                let c = &mut cursors[ci];
                if let Some(f) = ch.seek(s as u32, c) {
                    hlen += f.headers.len() as u64;
                    vlen += f.values.len() as u64;
                }
            }
            let one_bit = matches!(input.kinds[s], SignalKind::Bits { width: 1, .. });
            let aliased = alias != NO_BLOCK;
            let mut info = ColInfo {
                vlen: vlen as u32,
                ..Default::default()
            };
            if hlen + vlen == 0 || aliased {
                info.len = 0;
            } else if one_bit {
                info.len = hlen as u32;
            } else {
                let (w, full) = entry_width(input.kinds[s], scratch.entry_counts[s], vlen)
                    .unwrap_or((0, false));
                info.w = w;
                info.word = (hlen << 2) | (u64::from(w > 0) << 1) | u64::from(full);
                info.len = (varint::len_u64(info.word) as u64 + hlen + vlen) as u32;
            }
            cols.push(info);
            total_entries += scratch.entry_counts[s] as usize;
        }
        let budget = input.run_budget.max(1);
        let mut runs: Vec<(usize, usize, bool)> = Vec::new();
        {
            let mut i = 0;
            while i < n {
                let mut bytes = cols[i].len as usize;
                let mut entries = if cols[i].len == 0 {
                    0
                } else {
                    scratch.entry_counts[first + i] as usize
                };
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
        cursors.copy_from_slice(&cur_start);
        for &(ri, rn, wide) in &runs {
            raw.clear();
            let segs = &mut scratch.segs;
            segs.clear();
            for c in &cols[ri..ri + rn] {
                varint::put_u64(raw, u64::from(c.len));
            }
            for s in first + ri..first + ri + rn {
                let info = cols[s - first];
                if info.len == 0 {
                    continue;
                }
                let one_bit = matches!(input.kinds[s], SignalKind::Bits { width: 1, .. });
                if !one_bit {
                    varint::put_u64(raw, info.word);
                }
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
                        if let Some(f) = ch.seek(s as u32, c) {
                            raw.extend_from_slice(if pass == 0 { f.headers } else { f.values });
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
                apply_dict(
                    raw,
                    &cols[ri..ri + rn],
                    segs,
                    &scratch.dict_out,
                    &scratch.dict_ranges,
                    &mut scratch.raw2,
                );
                &scratch.raw2
            } else {
                if x != Xform::None {
                    for &(off, len, w) in segs.iter() {
                        xform::forward(
                            x,
                            w as usize,
                            &mut raw[off as usize..(off + len) as usize],
                            &mut scratch.xf_tmp,
                        );
                    }
                }
                raw
            };
            let before = run_blobs.len();
            let c = if wide && comp.codec == crate::codec::Codec::Zstd && comp.level > 1 {
                Compression {
                    codec: comp.codec,
                    level: 1,
                }
            } else {
                comp
            };
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
    Ok(total_entries)
}

impl EncoderScratch {
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Vec<u8> {
        &mut self.data
    }
}

const MAX_RUN_SIGNALS: usize = 64;

const HASH_SAMPLE: usize = 64;

const TRIAL_BYTES: usize = 1024;
const TRIAL_SEG: usize = 32;
const TRIAL_MIN_GAIN: usize = 4;

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

fn choose_xform(
    raw: &[u8],
    segs: &[(u32, u32, u32)],
    compressor: &mut Compressor,
    t: TrialScratch<'_>,
) -> Result<(Xform, bool)> {
    let TrialScratch {
        sample,
        sample_segs,
        sample_meta,
        trial_in,
        tmp,
        dict_out,
        dict_ranges,
        dict_table,
        dict_sample,
    } = t;
    let total: u64 = segs.iter().map(|s| u64::from(s.1)).sum();
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
            xform::forward(
                x,
                w as usize,
                &mut trial_in[off as usize..(off + len) as usize],
                tmp,
            );
        }
        let size = compressor.trial_size(trial_in)?;
        if x == Xform::None {
            incompressible = Compressor::sample_incompressible(size, sample.len());
            best = (x, size * (100 - TRIAL_MIN_GAIN) / 100);
        } else if size < best.1 {
            best = (x, size);
        }
    }
    dict_out.clear();
    dict_ranges.clear();
    dict_ranges.resize(segs.len(), (0, 0));
    trial_in.clear();
    let mut est_dict = 0usize;
    let mut est_codes = 0.0f64;
    let mut plain_bytes = 0usize;
    let mut any = false;
    let mut k = 0usize;
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
        let coded = if w < 2 || m < 8 {
            None
        } else {
            xform::dict_encode(
                w,
                dict_sample,
                tmp,
                dict_table,
                (m / 2).min(xform::DICT_MAX),
            )
            .filter(|&d| d <= DICT_SAMPLE_MAX || d * count <= xform::DICT_MAX * m)
        };
        if let Some(d) = coded {
            trial_in.extend_from_slice(&tmp[tmp.len() - m..]);
            let mf = m as f64;
            est_codes += dict_table
                .counts()
                .iter()
                .map(|&c| f64::from(c) * (mf / f64::from(c)).log2())
                .sum::<f64>()
                / 8.0;
            let nd = (d * count).div_ceil(m).clamp(d, xform::DICT_MAX);
            est_dict += varint::len_u64(nd as u64) + nd * w;
            dict_ranges[si] = (1, 0);
            any = true;
        } else {
            trial_in.extend_from_slice(dict_sample);
            plain_bytes += dict_sample.len();
        }
    }
    let mut dict_bytes = 0usize;
    let est_dict = (est_dict as u64 * sample.len() as u64 / total) as usize;
    let est_h0 = est_codes as usize + est_dict + best.1 * plain_bytes / sample.len().max(1);
    let est_total = if any && est_h0 * 100 < best.1 * (100 - DICT_MIN_GAIN) {
        compressor.trial_size(trial_in)? + est_dict
    } else {
        usize::MAX
    };
    if est_total != usize::MAX && est_total * 100 < best.1 * (100 - DICT_MIN_GAIN) {
        for (si, &(off, len, w)) in segs.iter().enumerate() {
            if dict_ranges[si] != (1, 0) {
                continue;
            }
            dict_ranges[si] = (0, 0);
            let (off, len, w) = (off as usize, len as usize, w as usize);
            let start = dict_out.len();
            if let Some(nd) = xform::dict_encode(
                w,
                &raw[off..off + len],
                dict_out,
                dict_table,
                xform::DICT_MAX,
            ) {
                dict_ranges[si] = (start as u32, dict_out.len() as u32);
                dict_bytes += varint::len_u64(nd as u64) + nd * w;
            }
        }
    } else {
        for r in dict_ranges.iter_mut() {
            *r = (0, 0);
        }
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
                trial_in
                    .extend_from_slice(&coded[codes + i0 as usize..codes + (i0 + cnt) as usize]);
            }
        }
        let size = compressor.trial_size(trial_in)?
            + (dict_bytes as u64 * sample.len() as u64 / total) as usize;
        if size * 100 < best.1 * (100 - DICT_MIN_GAIN) {
            best = (Xform::Dict, size);
        }
    }
    Ok((best.0, incompressible && best.0 == Xform::None))
}

const DICT_MIN_GAIN: usize = 20;

const DICT_SAMPLE_MAX: usize = 64;

fn apply_dict(
    raw: &[u8],
    cols: &[ColInfo],
    segs: &[(u32, u32, u32)],
    dict_out: &[u8],
    dict_ranges: &[(u32, u32)],
    out: &mut Vec<u8>,
) {
    out.clear();
    let mut r = Reader::new(raw);
    for _ in cols {
        drop(r.u64());
    }
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
            let vals = if a == b {
                1 + slen as usize
            } else {
                (b - a) as usize
            };
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

#[inline]
fn hash_bytes(mut h: u64, bytes: &[u8]) -> u64 {
    let (chunks, rest) = bytes.as_chunks::<8>();
    for c in chunks {
        let w = u64::from_le_bytes(*c);
        h = (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    if !rest.is_empty() {
        let mut b = [0u8; 8];
        b[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(b) ^ (rest.len() as u64) << 56)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(29);
    }
    h
}

fn columns_equal<C: ColumnFragments>(chunks: &[C], a: u32, b: u32) -> bool {
    fn parts<C: ColumnFragments>(
        chunks: &[C],
        s: u32,
        pass: usize,
    ) -> impl Iterator<Item = &[u8]> + '_ {
        chunks.iter().filter_map(move |ch| {
            ch.find(s)
                .map(|f| if pass == 0 { f.headers } else { f.values })
        })
    }
    for pass in 0..2 {
        let (mut ia, mut ib) = (parts(chunks, a, pass), parts(chunks, b, pass));
        let (mut sa, mut sb): (&[u8], &[u8]) = (&[], &[]);
        loop {
            if sa.is_empty()
                && let Some(x) = ia.next()
            {
                sa = x;
                continue;
            }
            if sb.is_empty()
                && let Some(x) = ib.next()
            {
                sb = x;
                continue;
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

fn encode_column(
    kind: SignalKind,
    recs: &[Record],
    heap: &[u8],
    state: &mut ColState,
    hdr: &mut Vec<u8>,
    val: &mut Vec<u8>,
) -> Result<()> {
    let mut prev = state.tidx;
    match kind {
        SignalKind::Bits { width: 1, .. } => {
            let mut code = state.code;
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = u64::from(t.wrapping_sub(prev));
                prev = t;
                let c = (r.payload & 15) as u8;
                if code <= 1 && c == code ^ 1 {
                    varint::put_u64(hdr, dt << 1);
                } else {
                    varint::put_u64(hdr, (dt << 5) | (u64::from(c) << 1) | 1);
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
                let dt = u64::from(t.wrapping_sub(prev));
                prev = t;
                varint::put_u64(hdr, (dt << 1) | u64::from(compact));
                let len = if compact { compact_len } else { full_len };
                if narrow {
                    val.extend_from_slice(&r.payload.to_le_bytes()[..len]);
                } else {
                    let o = r.payload as usize;
                    let b = heap
                        .get(o..o + len)
                        .ok_or(Error::State("heap offset out of range"))?;
                    val.extend_from_slice(b);
                }
            }
        }
        SignalKind::Real => {
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = u64::from(t.wrapping_sub(prev));
                prev = t;
                varint::put_u64(hdr, dt);
                val.extend_from_slice(&r.payload.to_le_bytes());
            }
        }
        SignalKind::VarLen => {
            for r in recs {
                let t = r.tidx & !COMPACT_FLAG;
                let dt = u64::from(t.wrapping_sub(prev));
                prev = t;
                varint::put_u64(hdr, dt);
                let mut hr = Reader::new(heap);
                hr.pos = r.payload as usize;
                let b = hr
                    .blob()
                    .map_err(|_| Error::State("heap varlen out of range"))?;
                varint::put_blob(val, b);
            }
        }
    }
    state.tidx = prev;
    Ok(())
}

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
    #[must_use]
    pub fn tt_off(&self) -> usize {
        HEADER_LEN
    }
    #[must_use]
    pub fn index_off(&self) -> usize {
        HEADER_LEN + self.tt_len as usize
    }
    #[must_use]
    pub fn prev_off(&self) -> usize {
        self.index_off() + self.n_dirty as usize * INDEX_ENTRY_LEN
    }
    #[must_use]
    pub fn data_off(&self) -> usize {
        self.prev_off() + self.n_groups as usize * 4
    }
}

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

#[must_use]
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
        let mid = usize::midpoint(lo, hi);
        let (g, clen, off) = entry(mid);
        match g.cmp(&group) {
            std::cmp::Ordering::Equal => return Some((clen, off)),
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
        }
    }
    None
}

pub fn dirty_groups<'a>(
    p: &'a [u8],
    h: &BlockHeader,
) -> impl Iterator<Item = (u32, u32, u64)> + 'a + use<'a> {
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

#[must_use]
pub fn prev_dirty(p: &[u8], h: &BlockHeader, group: u32) -> u32 {
    if group >= h.n_groups {
        return NO_BLOCK;
    }
    let o = h.prev_off() + group as usize * 4;
    u32::from_le_bytes(p[o..o + 4].try_into().unwrap())
}

pub fn group_container<'a>(p: &'a [u8], h: &BlockHeader, clen: u32, off: u64) -> Result<&'a [u8]> {
    let s = h.data_off() + off as usize;
    let e = s + clen as usize;
    if e > p.len() {
        return Err(Error::Corrupt("group container out of range"));
    }
    Ok(&p[s..e])
}

pub struct GroupView<'a> {
    pub container: &'a [u8],
    pub first_sig: u32,
    pub n_sigs: usize,
    pub frame_blob: &'a [u8],
    pub runs: Vec<Run<'a>>,
    pub aliases: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Run<'a> {
    pub first_local: u32,
    pub count: u32,
    pub xform: Xform,
    pub blob: &'a [u8],
}

impl<'a> GroupView<'a> {
    pub fn parse(
        container: &'a [u8],
        first_sig: u32,
        kinds: &[SignalKind],
    ) -> Result<GroupView<'a>> {
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
            let x =
                Xform::from_u8(r.u8()?).ok_or_else(|| Error::corrupt("unknown value transform"))?;
            let clen = r.usize()?;
            sizes.push((rn, x, clen));
        }
        let mut runs = Vec::with_capacity(n_runs);
        let mut local = 0u32;
        for (rn, x, clen) in sizes {
            let blob = r.bytes(clen)?;
            runs.push(Run {
                first_local: local,
                count: rn,
                xform: x,
                blob,
            });
            local += rn;
        }
        if local as usize != n {
            return Err(Error::Corrupt("runs do not cover the group"));
        }
        Ok(GroupView {
            container,
            first_sig,
            n_sigs: n,
            frame_blob,
            runs,
            aliases,
        })
    }

    #[must_use]
    pub fn holds(&self, sig: u32) -> bool {
        sig >= self.first_sig && ((sig - self.first_sig) as usize) < self.n_sigs
    }

    #[must_use]
    pub fn alias_of(&self, sig: u32) -> Option<u32> {
        self.aliases
            .binary_search_by_key(&sig, |a| a.0)
            .ok()
            .map(|i| self.aliases[i].1)
    }

    #[must_use]
    pub fn run_of(&self, sig: u32) -> usize {
        let local = sig - self.first_sig;
        let i = self.runs.partition_point(|r| r.first_local <= local);
        i - 1
    }

    pub fn decode_frames(
        &self,
        kinds: &[SignalKind],
        d: &mut Decompressor,
        out: &mut Vec<u8>,
    ) -> Result<Vec<(u32, u32)>> {
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

    pub fn decode_run(
        &self,
        ri: usize,
        d: &mut Decompressor,
        out: &mut Vec<u8>,
    ) -> Result<Vec<(u32, u32)>> {
        let Run {
            count: rn, blob, ..
        } = self.runs[ri];
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
            SignalKind::Bits { width, states } => Some(if word & 1 != 0 {
                packed_len(width, states)
            } else {
                (width as usize).div_ceil(8)
            }),
            SignalKind::Real => Some(8),
            SignalKind::VarLen => {
                return Err(Error::Corrupt("variable-length column marked eligible"));
            }
        }
    };
    Ok((&col[r.pos..r.pos + hlen], &col[r.pos + hlen..], w))
}

pub fn untransform_column_vec(
    col: &mut Vec<u8>,
    kind: SignalKind,
    x: Xform,
    tmp: &mut Vec<u8>,
) -> Result<()> {
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

pub fn untransform_run(
    data: &mut Vec<u8>,
    ranges: &mut [(u32, u32)],
    first_sig: u32,
    kinds: &[SignalKind],
    x: Xform,
) -> Result<()> {
    if x == Xform::None {
        return Ok(());
    }
    let mut tmp = Vec::new();
    if x != Xform::Dict {
        for (k, &(a, b)) in ranges.iter().enumerate() {
            untransform_column(
                &mut data[a as usize..b as usize],
                kinds[first_sig as usize + k],
                x,
                &mut tmp,
            )?;
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

pub fn untransform_column(
    col: &mut [u8],
    kind: SignalKind,
    x: Xform,
    tmp: &mut Vec<u8>,
) -> Result<()> {
    debug_assert_ne!(x, Xform::Dict);
    if x == Xform::None || col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
        return Ok(());
    }
    let (hdr, val, w) = split_column(col, kind)?;
    if let Some(w) = w {
        if val.len() % w != 0 {
            return Err(Error::Corrupt(
                "eligible column length is not a multiple of its entry width",
            ));
        }
        let vstart = col.len() - val.len();
        let _ = hdr;
        xform::inverse(x, w, &mut col[vstart..], tmp);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct RawChange {
    pub tidx: u32,
    pub states: u8,
    pub start: u32,
    pub end: u32,
    pub inline: u64,
}

pub static CODE_BYTES: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

#[derive(Clone, Copy, Debug)]
pub struct Checkpoint {
    pub tidx: u32,
    pub tidx_before: u32,
    pub hpos: u32,
    pub vpos: u32,
    pub code_before: u8,
}

pub fn build_index(col: &[u8], kind: SignalKind, stride: usize) -> Result<Vec<Checkpoint>> {
    let mut it = ColumnIter::new(col, kind);
    let mut out = Vec::with_capacity(col.len() / (stride.max(1) * 2) + 1);
    let mut i = 0usize;
    loop {
        let (hpos, vpos) = (it.h.pos, it.v.pos);
        let (before, code_before) = (it.tidx, it.code);
        match it.next_raw()? {
            Some(c) => {
                if i.is_multiple_of(stride) {
                    out.push(Checkpoint {
                        tidx: c.tidx,
                        tidx_before: before,
                        hpos: hpos as u32,
                        vpos: vpos as u32,
                        code_before,
                    });
                }
                i += 1;
            }
            None => break,
        }
    }
    Ok(out)
}

#[derive(Clone, Copy)]
pub struct ColumnIter<'a> {
    h: Reader<'a>,
    v: Reader<'a>,
    kind: SignalKind,
    tidx: u32,
    code: u8,
}

impl<'a> ColumnIter<'a> {
    #[must_use]
    pub fn new(col: &'a [u8], kind: SignalKind) -> Self {
        if col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
            return ColumnIter {
                h: Reader::new(col),
                v: Reader::new(&[]),
                kind,
                tidx: 0,
                code: NO_CODE,
            };
        }
        let mut r = Reader::new(col);
        let hlen = (r.u64().unwrap_or(0) >> 2) as usize;
        let hstart = r.pos;
        let hend = (hstart + hlen).min(col.len());
        ColumnIter {
            h: Reader {
                buf: &col[..hend],
                pos: hstart,
            },
            v: Reader {
                buf: col,
                pos: hend,
            },
            kind,
            tidx: 0,
            code: NO_CODE,
        }
    }

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

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.h.is_empty()
    }

    #[inline]
    pub fn next_raw(&mut self) -> Result<Option<RawChange>> {
        if self.h.is_empty() {
            return Ok(None);
        }
        Ok(Some(match self.kind {
            SignalKind::Bits { width: 1, states } => {
                let x = self.h.u64()?;
                let (dt, code) = if x & 1 == 0 {
                    (x >> 1, u64::from(self.code ^ 1))
                } else {
                    (x >> 5, (x >> 1) & 15)
                };
                if code > 8 {
                    return Err(Error::Corrupt("bad logic code in 1-bit column"));
                }
                self.code = code as u8;
                self.tidx = self.tidx.wrapping_add(dt as u32);
                let st = if code <= 1 { 2 } else { states };
                RawChange {
                    tidx: self.tidx,
                    states: st,
                    start: 0,
                    end: 0,
                    inline: code,
                }
            }
            SignalKind::Bits { width, states } => {
                let x = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add((x >> 1) as u32);
                let compact = x & 1 != 0;
                let (len, st) = if compact {
                    ((width as usize).div_ceil(8), 2)
                } else {
                    (packed_len(width, states), states)
                };
                let s = self.v.pos;
                self.v.bytes(len)?;
                RawChange {
                    tidx: self.tidx,
                    states: st,
                    start: s as u32,
                    end: (s + len) as u32,
                    inline: 0,
                }
            }
            SignalKind::Real => {
                let dt = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add(dt as u32);
                let f = self.v.fixed_u64()?;
                RawChange {
                    tidx: self.tidx,
                    states: 0,
                    start: 0,
                    end: 0,
                    inline: f,
                }
            }
            SignalKind::VarLen => {
                let dt = self.h.u64()?;
                self.tidx = self.tidx.wrapping_add(dt as u32);
                let lpos = self.v.pos;
                let len = self.v.usize()?;
                let s = self.v.pos;
                self.v.bytes(len)?;
                RawChange {
                    tidx: self.tidx,
                    states: 255,
                    start: s as u32,
                    end: (s + len) as u32,
                    inline: lpos as u64,
                }
            }
        }))
    }

    pub fn for_each_bit(&mut self, mut f: impl FnMut(u32, u8)) -> Result<()> {
        debug_assert!(matches!(self.kind, SignalKind::Bits { width: 1, .. }));
        let buf = self.h.buf;
        let (mut pos, mut tidx, mut code) = (self.h.pos, self.tidx, self.code);
        while pos < buf.len() {
            let x = match buf[pos] {
                b if b < 0x80 => {
                    pos += 1;
                    u64::from(b)
                }
                _ => {
                    let mut r = Reader { buf, pos };
                    let x = r.u64()?;
                    pos = r.pos;
                    x
                }
            };
            let (dt, c) = if x & 1 == 0 {
                (x >> 1, code ^ 1)
            } else {
                (x >> 5, ((x >> 1) & 15) as u8)
            };
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

    #[inline]
    #[must_use]
    pub fn value_bytes(&self, c: &RawChange) -> &'a [u8] {
        &self.v.buf[c.start as usize..c.end as usize]
    }

    #[inline]
    #[must_use]
    pub fn value(&self, c: &RawChange) -> SignalValue<'a> {
        match self.kind {
            SignalKind::Bits { width: 1, .. } => {
                let code = (c.inline & 15) as usize;
                SignalValue::Bits {
                    width: 1,
                    states: c.states,
                    data: &CODE_BYTES[code..=code],
                }
            }
            SignalKind::Bits { width, .. } => SignalValue::Bits {
                width,
                states: c.states,
                data: &self.v.buf[c.start as usize..c.end as usize],
            },
            SignalKind::Real => SignalValue::Real(f64::from_bits(c.inline)),
            SignalKind::VarLen => {
                SignalValue::VarLen(&self.v.buf[c.start as usize..c.end as usize])
            }
        }
    }

    #[inline]
    pub fn next_change(&mut self) -> Result<Option<(u32, SignalValue<'a>)>> {
        Ok(self.next_raw()?.map(|c| (c.tidx, self.value(&c))))
    }

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

pub fn for_each_tidx(col: &[u8], kind: SignalKind, mut f: impl FnMut(u32)) -> Result<()> {
    if col.is_empty() {
        return Ok(());
    }
    let (hdr, one_bit, shift) = if let SignalKind::Bits { width: 1, .. } = kind {
        (col, true, 0)
    } else {
        let mut r = Reader::new(col);
        let hlen = (r.u64()? >> 2) as usize;
        if r.pos + hlen > col.len() {
            return Err(Error::Corrupt("column header stream exceeds column"));
        }
        (
            &col[r.pos..r.pos + hlen],
            false,
            i32::from(matches!(kind, SignalKind::Bits { .. })),
        )
    };
    let (mut pos, mut tidx) = (0usize, 0u32);
    while pos < hdr.len() {
        let x = match hdr[pos] {
            b if b < 0x80 => {
                pos += 1;
                u64::from(b)
            }
            _ => {
                let mut r = Reader { buf: hdr, pos };
                let x = r.u64()?;
                pos = r.pos;
                x
            }
        };
        let dt = if one_bit {
            if x & 1 == 0 { x >> 1 } else { x >> 5 }
        } else {
            x >> shift
        };
        tidx = tidx.wrapping_add(dt as u32);
        f(tidx);
    }
    Ok(())
}

#[must_use]
pub fn frame_value(kind: SignalKind, frame: &[u8]) -> SignalValue<'_> {
    match kind {
        SignalKind::Bits { width, states } => SignalValue::Bits {
            width,
            states,
            data: frame,
        },
        SignalKind::Real => SignalValue::Real(f64::from_le_bytes(frame[..8].try_into().unwrap())),
        SignalKind::VarLen => SignalValue::VarLen(frame),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_rejects_invalid_signal_counts_before_scattering() {
        let kinds = Arc::new(vec![SignalKind::Real, SignalKind::Real]);
        for (signal, counts) in [(2, vec![1, 0]), (0, vec![0, 1])] {
            let input = ChunkInput {
                records: vec![Record {
                    sig: signal,
                    tidx: 0,
                    payload: 0,
                }],
                heap: Vec::new(),
                sig_counts: counts,
                kinds: Arc::clone(&kinds),
                wide: Vec::new(),
                times: vec![0],
                frame: Vec::new(),
                frame_parts: Vec::new(),
                ends_block: false,
            };
            assert!(matches!(
                encode_chunk(
                    &input,
                    &kinds,
                    &mut EncoderScratch::default(),
                    &mut ChunkEnc::default()
                ),
                Err(Error::State(_))
            ));
        }
    }
}
