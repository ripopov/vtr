//! Waveform block framing, column transforms, and activity scans.

mod encode;
pub(crate) use encode::{BlockInput, EncodedColumn, EncodedColumns, EncoderScratch, encode};

use crate::{
    codec::Decompressor,
    error::{Error, Result},
    hierarchy::SignalKind,
    signal::{SignalValue, packed_len},
    varint::Reader,
    xform::{self, Xform},
};

pub const HEADER_LEN: usize = 40;
pub const INDEX_ENTRY_LEN: usize = 16;
pub const NO_BLOCK: u32 = u32::MAX;
pub const COMPACT_FLAG: u32 = 1 << 31;
pub static CODE_BYTES: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

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
    let blob = &p[HEADER_LEN..h.index_off()];
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
    pub first_sig: u32,
    pub n_sigs: usize,
    pub runs: Vec<Run<'a>>,
    pub aliases: Vec<(u32, u32)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Run<'a> {
    pub first_local: u32,
    pub count: u32,
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
        r.blob()?;
        let n_runs = r.usize()?;
        if n_runs > n {
            return Err(Error::Corrupt("too many runs"));
        }
        let mut sizes = Vec::with_capacity(n_runs);
        for _ in 0..n_runs {
            let rn = r.u32()?;
            Xform::from_u8(r.u8()?).ok_or_else(|| Error::corrupt("unknown value transform"))?;
            let clen = r.usize()?;
            sizes.push((rn, clen));
        }
        let mut runs = Vec::with_capacity(n_runs);
        let mut local = 0u32;
        for (rn, clen) in sizes {
            let blob = r.bytes(clen)?;
            runs.push(Run {
                first_local: local,
                count: rn,
                blob,
            });
            local += rn;
        }
        if local as usize != n {
            return Err(Error::Corrupt("runs do not cover the group"));
        }
        Ok(GroupView {
            first_sig,
            n_sigs: n,
            runs,
            aliases,
        })
    }

    #[must_use]
    pub fn holds(&self, sig: u32) -> bool {
        sig >= self.first_sig && ((sig - self.first_sig) as usize) < self.n_sigs
    }

    #[must_use]
    pub fn run_of(&self, sig: u32) -> usize {
        let local = sig - self.first_sig;
        let i = self.runs.partition_point(|r| r.first_local <= local);
        i - 1
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

pub fn untransform_column(
    col: &mut Vec<u8>,
    kind: SignalKind,
    x: Xform,
    tmp: &mut Vec<u8>,
) -> Result<()> {
    if x == Xform::None || col.is_empty() || matches!(kind, SignalKind::Bits { width: 1, .. }) {
        return Ok(());
    }
    let (_, val, width) = split_column(col, kind)?;
    let Some(width) = width else { return Ok(()) };
    let start = col.len() - val.len();
    if x == Xform::Dict {
        tmp.clear();
        xform::dict_decode(width, val, tmp)?;
        col.truncate(start);
        col.extend_from_slice(tmp);
    } else {
        if !val.len().is_multiple_of(width) {
            return Err(Error::Corrupt(
                "eligible column length is not a multiple of its entry width",
            ));
        }
        xform::inverse(x, width, &mut col[start..], tmp);
    }
    Ok(())
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
