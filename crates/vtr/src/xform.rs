//! Value-stream transforms applied to a column run before compression
//! (spec section 6.4): byte shuffle (Blosc-style transposition) and delta
//! coding of fixed-width entries, alone or combined. They cost one pass over
//! the value bytes and turn counters, addresses and slowly changing high
//! bytes into long runs that the general-purpose compressor handles well.
//!
//! An entry stream is `n` consecutive entries of `w` bytes each; entries are
//! little-endian integers for delta purposes.

/// Transform applied to every eligible column of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Xform {
    #[default]
    None = 0,
    /// `out[b * n + i] = in[i * w + b]`: byte plane `b` of every entry stored together.
    Shuffle = 1,
    /// Entry `i` replaced by `entry[i] - entry[i - 1]` (little-endian, modulo `2^(8w)`).
    Delta = 2,
    /// Delta, then shuffle.
    DeltaShuffle = 3,
    /// Per-column dictionary: the value stream becomes `varint n_dict` (0: the plain
    /// values follow), `n_dict * w` dictionary bytes in first-occurrence order, then one
    /// code byte per entry. Unlike the others this changes the column length, so it is
    /// applied and undone at run level ([`dict_encode`], [`dict_decode`]).
    Dict = 4,
}

impl Xform {
    /// The in-place transforms ([`forward`] / [`inverse`]), in trial order.
    pub const IN_PLACE: [Xform; 4] = [Xform::None, Xform::Shuffle, Xform::Delta, Xform::DeltaShuffle];

    pub fn from_u8(v: u8) -> Option<Xform> {
        match v {
            0 => Some(Xform::None),
            1 => Some(Xform::Shuffle),
            2 => Some(Xform::Delta),
            3 => Some(Xform::DeltaShuffle),
            4 => Some(Xform::Dict),
            _ => None,
        }
    }
}

/// Most distinct entries a dictionary-coded column may have (codes are one byte).
pub const DICT_MAX: usize = 256;

/// Scratch state for [`dict_encode`]: an open-addressing table of `DICT_SLOTS` slots
/// holding `code + 1` (0 = empty), plus the dictionary and code buffers.
pub struct DictTable {
    slots: Vec<u16>,
    /// (key, code + 1) slots for entries of up to 8 bytes.
    narrow: Vec<(u64, u16)>,
    dict: Vec<u8>,
    codes: Vec<u8>,
    /// Occurrences of every code in the last encoded stream.
    counts: Vec<u32>,
}

impl DictTable {
    /// Occurrences of each dictionary code in the stream last passed to [`dict_encode`].
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }
}

#[inline]
fn load_le(e: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b[..e.len()].copy_from_slice(e);
    u64::from_le_bytes(b)
}

const DICT_SLOTS: usize = 1024;

impl Default for DictTable {
    fn default() -> Self {
        DictTable { slots: vec![0; DICT_SLOTS], narrow: vec![(0, 0); DICT_SLOTS], dict: Vec::new(), codes: Vec::new(), counts: Vec::new() }
    }
}

#[inline]
fn hash_entry(e: &[u8]) -> u64 {
    let mut h = 0x9E37_79B9_7F4A_7C15u64;
    let mut chunks = e.chunks_exact(8);
    for c in &mut chunks {
        h = (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0xD6E8_FEB8_6659_FD93).rotate_left(31);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut b = [0u8; 8];
        b[..rest.len()].copy_from_slice(rest);
        h = (h ^ u64::from_le_bytes(b)).wrapping_mul(0xD6E8_FEB8_6659_FD93).rotate_left(31);
    }
    h ^ (h >> 29)
}

/// Dictionary-codes `v` (a whole number of `w`-byte entries) and appends the coded
/// stream to `out`, returning the dictionary size. Returns `None` and leaves `out`
/// untouched when the entries have more than `max` distinct values (`max <= DICT_MAX`).
pub fn dict_encode(w: usize, v: &[u8], out: &mut Vec<u8>, t: &mut DictTable, max: usize) -> Option<usize> {
    debug_assert!(w > 0 && v.len() % w == 0 && max <= DICT_MAX);
    t.dict.clear();
    t.codes.clear();
    t.codes.reserve(v.len() / w);
    t.counts.clear();
    let mut nd = 0usize;
    if w <= 8 {
        // Narrow entries: the slot holds the key itself next to the code, so a hit costs
        // one load and one compare.
        t.narrow.fill((0, 0));
        for e in v.chunks_exact(w) {
            let key = load_le(e);
            let mut h = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - 10)) as usize;
            loop {
                let (k, c) = t.narrow[h];
                if c == 0 {
                    if nd == max {
                        return None;
                    }
                    t.dict.extend_from_slice(e);
                    nd += 1;
                    t.narrow[h] = (key, nd as u16);
                    t.codes.push((nd - 1) as u8);
                    t.counts.push(1);
                    break;
                }
                if k == key {
                    t.codes.push((c - 1) as u8);
                    t.counts[c as usize - 1] += 1;
                    break;
                }
                h = (h + 1) & (DICT_SLOTS - 1);
            }
        }
    } else {
        t.slots.fill(0);
        for e in v.chunks_exact(w) {
            let mut h = hash_entry(e) as usize & (DICT_SLOTS - 1);
            loop {
                let s = t.slots[h] as usize;
                if s == 0 {
                    if nd == max {
                        return None;
                    }
                    t.dict.extend_from_slice(e);
                    nd += 1;
                    t.slots[h] = nd as u16;
                    t.codes.push((nd - 1) as u8);
                    t.counts.push(1);
                    break;
                }
                if &t.dict[(s - 1) * w..s * w] == e {
                    t.codes.push((s - 1) as u8);
                    t.counts[s - 1] += 1;
                    break;
                }
                h = (h + 1) & (DICT_SLOTS - 1);
            }
        }
    }
    crate::varint::put_u64(out, nd as u64);
    out.extend_from_slice(&t.dict);
    out.extend_from_slice(&t.codes);
    Some(nd)
}

/// Undoes [`dict_encode`]: appends the plain `w`-byte entries of a coded stream to `out`.
pub fn dict_decode(w: usize, input: &[u8], out: &mut Vec<u8>) -> crate::error::Result<()> {
    use crate::error::Error;
    let mut r = crate::varint::Reader::new(input);
    let nd = r.u64()? as usize;
    let rest = &input[r.pos..];
    if nd == 0 {
        out.extend_from_slice(rest);
        return Ok(());
    }
    if nd > DICT_MAX || rest.len() < nd * w {
        return Err(Error::Corrupt("dictionary-coded column is truncated"));
    }
    let (dict, codes) = rest.split_at(nd * w);
    if codes.iter().any(|&c| c as usize >= nd) {
        return Err(Error::Corrupt("dictionary code out of range"));
    }
    out.reserve(codes.len() * w);
    match w {
        2 => dict_expand::<2>(dict, codes, out),
        4 => dict_expand::<4>(dict, codes, out),
        8 => dict_expand::<8>(dict, codes, out),
        _ => {
            for &c in codes {
                let c = c as usize;
                out.extend_from_slice(&dict[c * w..c * w + w]);
            }
        }
    }
    Ok(())
}

fn dict_expand<const W: usize>(dict: &[u8], codes: &[u8], out: &mut Vec<u8>) {
    for &c in codes {
        let c = c as usize;
        out.extend_from_slice(&dict[c * W..c * W + W]);
    }
}

/// Applies `x` in place to `v` (a whole number of `w`-byte entries); `tmp` is scratch space.
pub fn forward(x: Xform, w: usize, v: &mut [u8], tmp: &mut Vec<u8>) {
    debug_assert!(w > 0 && v.len() % w == 0);
    match x {
        Xform::None => {}
        Xform::Shuffle => shuffle(w, v, tmp),
        Xform::Delta => delta_encode(w, v),
        Xform::DeltaShuffle => {
            delta_encode(w, v);
            shuffle(w, v, tmp);
        }
        Xform::Dict => debug_assert!(false, "dictionary coding is applied at run level"),
    }
}

/// Undoes [`forward`].
pub fn inverse(x: Xform, w: usize, v: &mut [u8], tmp: &mut Vec<u8>) {
    debug_assert!(w > 0 && v.len() % w == 0);
    match x {
        Xform::None => {}
        Xform::Shuffle => unshuffle(w, v, tmp),
        Xform::Delta => delta_decode(w, v),
        Xform::DeltaShuffle => {
            unshuffle(w, v, tmp);
            delta_decode(w, v);
        }
        Xform::Dict => debug_assert!(false, "dictionary coding is undone at run level"),
    }
}

/// Entries per block of the plane-major loops (a block of `64 * w` bytes stays in L1).
const BLOCK: usize = 64;

/// Entry-major to plane-major. Plane-major loop: sequential writes, reads strided
/// within a block that stays in L1.
fn shuffle(w: usize, v: &mut [u8], tmp: &mut Vec<u8>) {
    if w < 2 {
        return;
    }
    let n = v.len() / w;
    tmp.clear();
    tmp.resize(v.len(), 0);
    let mut i0 = 0;
    while i0 < n {
        let i1 = (i0 + BLOCK).min(n);
        for b in 0..w {
            let plane = &mut tmp[b * n + i0..b * n + i1];
            for (i, p) in plane.iter_mut().enumerate() {
                *p = v[(i0 + i) * w + b];
            }
        }
        i0 = i1;
    }
    v.copy_from_slice(tmp);
}

/// Plane-major back to entry-major.
fn unshuffle(w: usize, v: &mut [u8], tmp: &mut Vec<u8>) {
    match w {
        0 | 1 => {}
        2 => unshuffle_w::<2>(v, tmp),
        4 => unshuffle_w::<4>(v, tmp),
        8 => unshuffle_w::<8>(v, tmp),
        16 => unshuffle_w::<16>(v, tmp),
        _ => unshuffle_any(w, v, tmp),
    }
}

/// Entry-major loop with a compile-time width: the gather of `W` bytes unrolls and every
/// entry is written sequentially (best for narrow entries).
fn unshuffle_w<const W: usize>(v: &mut [u8], tmp: &mut Vec<u8>) {
    let n = v.len() / W;
    tmp.clear();
    tmp.extend_from_slice(v);
    for (i, e) in v.chunks_exact_mut(W).enumerate() {
        for b in 0..W {
            e[b] = tmp[b * n + i];
        }
    }
}

/// Plane-major loop in blocks for wide entries (few streams touched at once).
fn unshuffle_any(w: usize, v: &mut [u8], tmp: &mut Vec<u8>) {
    let n = v.len() / w;
    tmp.clear();
    tmp.extend_from_slice(v);
    let mut i0 = 0;
    while i0 < n {
        let i1 = (i0 + BLOCK).min(n);
        for b in 0..w {
            let plane = &tmp[b * n + i0..b * n + i1];
            for (i, &p) in plane.iter().enumerate() {
                v[(i0 + i) * w + b] = p;
            }
        }
        i0 = i1;
    }
}

macro_rules! typed_delta {
    ($t:ty, $v:expr, $encode:expr) => {{
        const W: usize = std::mem::size_of::<$t>();
        let n = $v.len() / W;
        let at = |v: &[u8], i: usize| <$t>::from_le_bytes(v[i * W..i * W + W].try_into().unwrap());
        if $encode {
            for i in (1..n).rev() {
                let d = at($v, i).wrapping_sub(at($v, i - 1));
                $v[i * W..i * W + W].copy_from_slice(&d.to_le_bytes());
            }
        } else if n > 0 {
            // Keep the running value in a register rather than re-reading the store.
            let mut prev = at($v, 0);
            for i in 1..n {
                prev = at($v, i).wrapping_add(prev);
                $v[i * W..i * W + W].copy_from_slice(&prev.to_le_bytes());
            }
        }
    }};
}

fn delta_encode(w: usize, v: &mut [u8]) {
    match w {
        1 => typed_delta!(u8, v, true),
        2 => typed_delta!(u16, v, true),
        4 => typed_delta!(u32, v, true),
        8 => typed_delta!(u64, v, true),
        _ => {
            let n = v.len() / w;
            for i in (1..n).rev() {
                let (prev, cur) = v.split_at_mut(i * w);
                sub_le(&mut cur[..w], &prev[(i - 1) * w..]);
            }
        }
    }
}

fn delta_decode(w: usize, v: &mut [u8]) {
    match w {
        1 => typed_delta!(u8, v, false),
        2 => typed_delta!(u16, v, false),
        4 => typed_delta!(u32, v, false),
        8 => typed_delta!(u64, v, false),
        _ => {
            let n = v.len() / w;
            for i in 1..n {
                let (prev, cur) = v.split_at_mut(i * w);
                add_le(&mut cur[..w], &prev[(i - 1) * w..]);
            }
        }
    }
}

/// `dst -= src` on little-endian integers of `dst.len()` bytes.
fn sub_le(dst: &mut [u8], src: &[u8]) {
    let mut borrow = 0u64;
    let mut i = 0;
    while i < dst.len() {
        let k = (dst.len() - i).min(8);
        let (a, b) = (limb(&dst[i..i + k]), limb(&src[i..i + k]));
        let (d, o1) = a.overflowing_sub(b);
        let (d, o2) = d.overflowing_sub(borrow);
        borrow = (o1 | o2) as u64;
        store_limb(&mut dst[i..i + k], d);
        i += k;
    }
}

/// `dst += src` on little-endian integers of `dst.len()` bytes.
fn add_le(dst: &mut [u8], src: &[u8]) {
    let mut carry = 0u64;
    let mut i = 0;
    while i < dst.len() {
        let k = (dst.len() - i).min(8);
        let (a, b) = (limb(&dst[i..i + k]), limb(&src[i..i + k]));
        let (s, o1) = a.overflowing_add(b);
        let (s, o2) = s.overflowing_add(carry);
        // A partial limb (k < 8) carries out of its 8k bits; a full limb when the add wraps.
        carry = if k < 8 { s >> (8 * k) } else { (o1 | o2) as u64 };
        store_limb(&mut dst[i..i + k], s);
        i += k;
    }
}

#[inline]
fn store_limb(bytes: &mut [u8], x: u64) {
    if bytes.len() == 8 {
        bytes.copy_from_slice(&x.to_le_bytes());
    } else {
        bytes.copy_from_slice(&x.to_le_bytes()[..bytes.len()]);
    }
}

#[inline]
fn limb(bytes: &[u8]) -> u64 {
    if bytes.len() == 8 {
        return u64::from_le_bytes(bytes.try_into().unwrap());
    }
    let mut b = [0u8; 8];
    b[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> u8 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 56) as u8
    }

    #[test]
    fn roundtrip_all_widths() {
        let mut seed = 7u64;
        let mut tmp = Vec::new();
        for w in [1usize, 2, 3, 4, 5, 7, 8, 9, 12, 16, 33, 256] {
            for n in [0usize, 1, 2, 3, 63, 64, 65, 200] {
                let orig: Vec<u8> = (0..n * w).map(|_| lcg(&mut seed)).collect();
                for x in Xform::IN_PLACE {
                    let mut v = orig.clone();
                    forward(x, w, &mut v, &mut tmp);
                    if x != Xform::None && n > 1 && w > 1 {
                        assert_ne!(v, orig, "w={w} n={n} {x:?}");
                    }
                    inverse(x, w, &mut v, &mut tmp);
                    assert_eq!(v, orig, "w={w} n={n} {x:?}");
                }
            }
        }
    }

    #[test]
    fn delta_of_counter_is_constant() {
        let mut v = Vec::new();
        for i in 0u32..100 {
            v.extend_from_slice(&(i * 4).wrapping_add(0xFFFF_FF00).to_le_bytes());
        }
        let mut tmp = Vec::new();
        forward(Xform::Delta, 4, &mut v, &mut tmp);
        for i in 1..100 {
            assert_eq!(&v[i * 4..i * 4 + 4], &[4, 0, 0, 0]);
        }
        // Wide entries with borrows across limbs.
        let mut v = Vec::new();
        for i in 0u64..50 {
            let mut e = [0u8; 12];
            e[..8].copy_from_slice(&(u64::MAX - 3).wrapping_add(i * 5).to_le_bytes());
            e[8..].copy_from_slice(&(i as u32 / 3).to_le_bytes());
            v.extend_from_slice(&e);
        }
        let keep = v.clone();
        forward(Xform::DeltaShuffle, 12, &mut v, &mut tmp);
        inverse(Xform::DeltaShuffle, 12, &mut v, &mut tmp);
        assert_eq!(v, keep);
    }

    #[test]
    fn dict_roundtrip_and_limits() {
        let mut t = DictTable::default();
        let mut out = Vec::new();
        for w in [2usize, 3, 4, 8, 23] {
            // 300 entries cycling through 7 values, then a stream with 300 distinct values.
            let v: Vec<u8> = (0..300).flat_map(|i| (0..w).map(move |b| ((i % 7) * 37 + b) as u8)).collect();
            out.clear();
            assert_eq!(dict_encode(w, &v, &mut out, &mut t, DICT_MAX), Some(7));
            assert_eq!(out.len(), 1 + 7 * w + 300);
            let mut back = Vec::new();
            dict_decode(w, &out, &mut back).unwrap();
            assert_eq!(back, v);
            let v: Vec<u8> = (0..300u32).flat_map(|i| (0..w).map(move |b| if b == 0 { i as u8 } else { (i >> 8) as u8 })).collect();
            out.clear();
            assert_eq!(dict_encode(w, &v, &mut out, &mut t, DICT_MAX), None);
            assert!(out.is_empty());
            assert_eq!(dict_encode(w, &v[..256 * w], &mut out, &mut t, DICT_MAX), Some(256));
            let mut back = Vec::new();
            dict_decode(w, &out, &mut back).unwrap();
            assert_eq!(back, v[..256 * w]);
        }
        // A plain (count 0) stream passes through; a code past the dictionary is corrupt.
        let mut back = Vec::new();
        dict_decode(4, &[0, 1, 2, 3, 4], &mut back).unwrap();
        assert_eq!(back, [1, 2, 3, 4]);
        assert!(dict_decode(2, &[1, 9, 9, 0, 1], &mut Vec::new()).is_err());
    }

    #[test]
    fn shuffle_layout() {
        let mut v = vec![1, 2, 3, 4, 5, 6];
        let mut tmp = Vec::new();
        forward(Xform::Shuffle, 2, &mut v, &mut tmp);
        assert_eq!(v, [1, 3, 5, 2, 4, 6]);
    }
}
