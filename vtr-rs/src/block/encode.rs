//! Encode complete waveform columns into grouped, compressed blocks.

use super::{HEADER_LEN, INDEX_ENTRY_LEN, NO_BLOCK};
use crate::{
    codec::{Compression, Compressor},
    error::{Error, Result},
    hierarchy::SignalKind,
    signal::packed_len,
    varint::{self, Reader},
    xform::{self, Xform},
};
use std::sync::Arc;

pub(crate) struct EncodedColumn {
    pub signal: u32,
    pub count: u32,
    pub headers: Vec<u8>,
    pub values: Vec<u8>,
}

pub(crate) struct EncodedColumns(pub Vec<EncodedColumn>);
impl EncodedColumns {
    fn seek(&self, signal: u32, cursor: &mut usize) -> Option<&EncodedColumn> {
        while *cursor < self.0.len() {
            let column = &self.0[*cursor];
            if column.signal >= signal {
                return (column.signal == signal).then_some(column);
            }
            *cursor += 1;
        }
        None
    }

    fn find(&self, signal: u32) -> Option<&EncodedColumn> {
        self.0
            .binary_search_by_key(&signal, |column| column.signal)
            .ok()
            .map(|index| &self.0[index])
    }
}

pub(crate) struct BlockInput {
    pub times: Vec<u64>,
    pub kinds: Arc<Vec<SignalKind>>,
    pub group_size: u32,
    pub dirty_groups: Vec<u32>,
    pub frame: Vec<u8>,
    pub prev_dirty: Vec<u32>,
    pub run_budget: usize,
}

#[derive(Default)]
pub(crate) struct EncoderScratch {
    group_raw: Vec<u8>,
    data: Vec<u8>,
    tt: Vec<u8>,
    tt_c: Vec<u8>,
    frame_c: Vec<u8>,
    run_blobs: Vec<u8>,
    entry_counts: Vec<u32>,
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

#[derive(Clone, Copy, Default)]
struct ColInfo {
    vlen: u32,
    word: u64,
    w: u32,
    len: u32,
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

pub(crate) fn encode(
    input: &BlockInput,
    columns: &EncodedColumns,
    comp: Compression,
    compressor: &mut Compressor,
    scratch: &mut EncoderScratch,
    out: &mut Vec<u8>,
) -> Result<()> {
    let n_sig = input.kinds.len();
    let g = input.group_size as usize;
    let n_groups = n_sig.div_ceil(g.max(1));
    if input.prev_dirty.len() != n_groups {
        return Err(Error::State("prev_dirty table size mismatch"));
    }
    scratch.entry_counts.clear();
    scratch.entry_counts.resize(n_sig, 0);
    for column in &columns.0 {
        scratch.entry_counts[column.signal as usize] = column.count;
    }
    let mut alias_of = std::mem::take(&mut scratch.alias_of);
    alias_of.clear();
    alias_of.resize(n_sig, NO_BLOCK);
    scratch.dedup.clear();
    for column in &columns.0 {
        let signal = column.signal;
        let total = column.headers.len() + column.values.len();
        if total < 32 {
            continue;
        }
        let kind_tag = match input.kinds[signal as usize] {
            SignalKind::Bits { width, states } => u64::from(width) << 8 | u64::from(states),
            SignalKind::Real => 1 << 40,
            SignalKind::VarLen => 2 << 40,
        };
        let mut hash = kind_tag.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(column.count);
        for bytes in [&column.headers, &column.values] {
            hash = hash_bytes(hash, &bytes[..bytes.len().min(HASH_SAMPLE)]);
            hash = hash_bytes(hash, &bytes[bytes.len() - bytes.len().min(HASH_SAMPLE)..]);
        }
        let key = (hash, (kind_tag << 24) ^ total as u64);
        match scratch.dedup.get(&key) {
            Some(&target) => {
                let other = columns.find(target).expect("indexed waveform column");
                if column.headers == other.headers && column.values == other.values {
                    alias_of[signal as usize] = target;
                }
            }
            None => {
                scratch.dedup.insert(key, signal);
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
    let mut cursor = 0;
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
                    let b = fr
                        .blob()
                        .map_err(|_| Error::State("invalid waveform frame"))?;
                    varint::put_blob(raw, b);
                }
                k => {
                    let l = k.packed_len().unwrap();
                    raw.extend_from_slice(
                        fr.bytes(l)
                            .map_err(|_| Error::State("invalid waveform frame"))?,
                    );
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
        let group_start = cursor;
        for (s, &alias) in alias_of.iter().enumerate().take(last).skip(first) {
            let column = columns.seek(s as u32, &mut cursor);
            let hlen = column.map_or(0, |column| column.headers.len() as u64);
            let vlen = column.map_or(0, |column| column.values.len() as u64);
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
        cursor = group_start;
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
                let column = columns
                    .seek(s as u32, &mut cursor)
                    .expect("nonempty waveform column");
                raw.extend_from_slice(&column.headers);
                if !one_bit {
                    if info.w > 0 {
                        segs.push((raw.len() as u32, info.vlen, info.w));
                    }
                    raw.extend_from_slice(&column.values);
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
    out.extend_from_slice(data);
    scratch.alias_of = alias_of;
    Ok(())
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
