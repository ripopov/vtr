//! Signal value representation and bit packing.
//!
//! Logic codes (one per bit): `0`=0, `1`=1, `2`=X, `3`=Z, `4`=U, `5`=W,
//! `6`=L, `7`=H, `8`=-. Vectors are packed LSB-first: bit `i` of the vector
//! (bit 0 = least significant = the last character of a VCD string) is
//! stored at byte `i/8` (2-state, 1 bit each), `i/4` (4-state, 2 bits each)
//! or `i/2` (9-state, 4 bits each), in the low-to-high bit positions.

use crate::hierarchy::SignalKind;
use crate::value::packed_len;

pub const L0: u8 = 0;
pub const L1: u8 = 1;
pub const LX: u8 = 2;
pub const LZ: u8 = 3;
pub const LU: u8 = 4;
pub const LW: u8 = 5;
pub const LL: u8 = 6;
pub const LH: u8 = 7;
pub const LDASH: u8 = 8;

/// ASCII spelling of each logic code.
pub const CODE_ASCII: [u8; 9] = *b"01xzuwlh-";

/// Maps a VCD/VHDL value character to a logic code (unknown characters map to X).
#[inline]
pub fn code_from_ascii(c: u8) -> u8 {
    match c {
        b'0' => L0,
        b'1' => L1,
        b'x' | b'X' => LX,
        b'z' | b'Z' => LZ,
        b'u' | b'U' => LU,
        b'w' | b'W' => LW,
        b'l' | b'L' => LL,
        b'h' | b'H' => LH,
        b'-' => LDASH,
        _ => LX,
    }
}

/// Minimum number of states needed to represent `code`.
#[inline]
pub fn states_for_code(code: u8) -> u8 {
    match code {
        0 | 1 => 2,
        2 | 3 => 4,
        _ => 9,
    }
}

/// Reads logic code `i` from a packed vector with `states` states per bit.
#[inline]
pub fn get_code(data: &[u8], states: u8, i: usize) -> u8 {
    match states {
        2 => (data[i >> 3] >> (i & 7)) & 1,
        4 => (data[i >> 2] >> ((i & 3) * 2)) & 3,
        _ => (data[i >> 1] >> ((i & 1) * 4)) & 15,
    }
}

/// Writes logic code `c` at bit `i` (the target bits must be zero).
#[inline]
pub fn set_code(data: &mut [u8], states: u8, i: usize, c: u8) {
    match states {
        2 => data[i >> 3] |= (c & 1) << (i & 7),
        4 => data[i >> 2] |= (c & 3) << ((i & 3) * 2),
        _ => data[i >> 1] |= (c & 15) << ((i & 1) * 4),
    }
}

/// Packs an ASCII vector (MSB first, like VCD) into `out` using `states`
/// states per bit. Returns `true` when every character was 0 or 1.
/// Strings shorter than `width` are zero-extended on the left (VCD rule:
/// left-extend with the MSB for x/z, with 0 otherwise); longer strings are
/// truncated to the low `width` bits.
pub fn pack_ascii(s: &[u8], width: u32, states: u8, out: &mut [u8]) -> bool {
    let w = width as usize;
    for b in out.iter_mut() {
        *b = 0;
    }
    let mut two_state = true;
    let n = s.len();
    // Character for bit i is s[n-1-i].
    let ext = if n > 0 {
        match s[0] {
            b'x' | b'X' => LX,
            b'z' | b'Z' => LZ,
            b'u' | b'U' => LU,
            b'w' | b'W' => LW,
            b'l' | b'L' => LL,
            b'h' | b'H' => LH,
            b'-' => LDASH,
            _ => L0,
        }
    } else {
        L0
    };
    for i in 0..w {
        let c = if i < n { code_from_ascii(s[n - 1 - i]) } else { ext };
        if c > 1 {
            two_state = false;
        }
        set_code(out, states, i, c & mask_for(states));
    }
    two_state
}

#[inline]
fn mask_for(states: u8) -> u8 {
    match states {
        2 => 1,
        4 => 3,
        _ => 15,
    }
}

/// Appends the ASCII spelling (MSB first) of a packed vector to `out`.
pub fn unpack_ascii(data: &[u8], width: u32, states: u8, out: &mut Vec<u8>) {
    let w = width as usize;
    out.reserve(w);
    for i in (0..w).rev() {
        let c = get_code(data, states, i);
        out.push(CODE_ASCII[(c as usize).min(8)]);
    }
}

/// Re-packs a vector from `from` states to `to` states (`to >= from`).
pub fn widen(data: &[u8], width: u32, from: u8, to: u8, out: &mut Vec<u8>) {
    let start = out.len();
    out.resize(start + packed_len(width, to), 0);
    if from == to {
        out[start..].copy_from_slice(&data[..packed_len(width, from)]);
        return;
    }
    let dst = &mut out[start..];
    if from == 2 && to == 4 {
        // Each source byte expands to two bytes (bit i -> bits 2i).
        for (i, &b) in data[..(width as usize).div_ceil(8)].iter().enumerate() {
            let e = SPREAD2[b as usize];
            dst[2 * i] = e as u8;
            if 2 * i + 1 < dst.len() {
                dst[2 * i + 1] = (e >> 8) as u8;
            }
        }
        return;
    }
    if from == 2 && to == 9 {
        for (i, &b) in data[..(width as usize).div_ceil(8)].iter().enumerate() {
            let e = SPREAD4[b as usize];
            for k in 0..4 {
                if 4 * i + k < dst.len() {
                    dst[4 * i + k] = (e >> (8 * k)) as u8;
                }
            }
        }
        return;
    }
    for i in 0..width as usize {
        set_code(dst, to, i, get_code(data, from, i));
    }
}

/// `SPREAD2[b]` places bit i of `b` at bit 2i.
static SPREAD2: [u16; 256] = {
    let mut t = [0u16; 256];
    let mut b = 0;
    while b < 256 {
        let mut v = 0u16;
        let mut i = 0;
        while i < 8 {
            if b & (1 << i) != 0 {
                v |= 1 << (2 * i);
            }
            i += 1;
        }
        t[b] = v;
        b += 1;
    }
    t
};

/// `SPREAD4[b]` places bit i of `b` at bit 4i.
static SPREAD4: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut b = 0;
    while b < 256 {
        let mut v = 0u32;
        let mut i = 0;
        while i < 8 {
            if b & (1 << i) != 0 {
                v |= 1 << (4 * i);
            }
            i += 1;
        }
        t[b] = v;
        b += 1;
    }
    t
};

/// Packs 2-state little-endian words into a 2-state vector. `words[0]` holds bits 0..32.
pub fn pack_words32(words: &[u32], width: u32, out: &mut [u8]) {
    let bytes = (width as usize).div_ceil(8);
    for (i, b) in out[..bytes].iter_mut().enumerate() {
        let w = words.get(i / 4).copied().unwrap_or(0);
        *b = (w >> ((i % 4) * 8)) as u8;
    }
    trim_top_bits(out, width);
}

/// Packs 2-state little-endian 64-bit words into a 2-state vector.
pub fn pack_words64(words: &[u64], width: u32, out: &mut [u8]) {
    let bytes = (width as usize).div_ceil(8);
    for (i, b) in out[..bytes].iter_mut().enumerate() {
        let w = words.get(i / 8).copied().unwrap_or(0);
        *b = (w >> ((i % 8) * 8)) as u8;
    }
    trim_top_bits(out, width);
}

/// Clears bits above `width` in the last byte of a 2-state vector.
#[inline]
pub fn trim_top_bits(out: &mut [u8], width: u32) {
    let rem = width % 8;
    if rem != 0 {
        let last = (width as usize).div_ceil(8) - 1;
        out[last] &= (1u8 << rem) - 1;
    }
}

/// Returns true if every code of a `states`-packed vector is 0 or 1.
pub fn is_two_state(data: &[u8], width: u32, states: u8) -> bool {
    match states {
        2 => true,
        4 => {
            // Any code >= 2 has its high bit (of the 2-bit group) set.
            let full = width as usize / 4;
            if data[..full].iter().any(|&b| b & 0xAA != 0) {
                return false;
            }
            for i in full * 4..width as usize {
                if get_code(data, 4, i) > 1 {
                    return false;
                }
            }
            true
        }
        _ => {
            let full = width as usize / 2;
            if data[..full].iter().any(|&b| b & 0xEE != 0) {
                return false;
            }
            for i in full * 2..width as usize {
                if get_code(data, 9, i) > 1 {
                    return false;
                }
            }
            true
        }
    }
}

/// Converts a packed vector (any states) to 2-state packing; only valid when `is_two_state`.
pub fn narrow_to_two_state(data: &[u8], width: u32, states: u8, out: &mut Vec<u8>) {
    let start = out.len();
    out.resize(start + packed_len(width, 2), 0);
    if states == 2 {
        out[start..].copy_from_slice(&data[..packed_len(width, 2)]);
        return;
    }
    let dst = &mut out[start..];
    for i in 0..width as usize {
        set_code(dst, 2, i, get_code(data, states, i) & 1);
    }
}

/// A borrowed signal value as stored in the file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignalValue<'a> {
    /// Packed bit vector. `states` is the packing actually used (2 when the
    /// value was stored compactly, otherwise the signal's declared states).
    Bits { width: u32, states: u8, data: &'a [u8] },
    Real(f64),
    VarLen(&'a [u8]),
}

impl<'a> SignalValue<'a> {
    /// ASCII spelling: bit string (MSB first), `%.17g`-like real, or raw bytes.
    pub fn to_ascii(&self) -> String {
        match *self {
            SignalValue::Bits { width, states, data } => {
                let mut v = Vec::with_capacity(width as usize);
                unpack_ascii(data, width, states, &mut v);
                String::from_utf8(v).unwrap()
            }
            SignalValue::Real(r) => format!("{}", r),
            SignalValue::VarLen(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }

    /// Value as an integer when it is a 2-state vector of at most 64 bits.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            SignalValue::Bits { width, states, data } if width <= 64 => {
                if states != 2 && !is_two_state(data, width, states) {
                    return None;
                }
                let mut v = 0u64;
                for i in 0..width as usize {
                    if get_code(data, states, i) & 1 != 0 {
                        v |= 1 << i;
                    }
                }
                Some(v)
            }
            _ => None,
        }
    }

    pub fn to_owned(&self) -> OwnedSignalValue {
        match *self {
            SignalValue::Bits { width, states, data } => {
                OwnedSignalValue::Bits { width, states, data: data.to_vec() }
            }
            SignalValue::Real(r) => OwnedSignalValue::Real(r),
            SignalValue::VarLen(b) => OwnedSignalValue::VarLen(b.to_vec()),
        }
    }
}

/// Owned variant of [`SignalValue`].
#[derive(Clone, Debug, PartialEq)]
pub enum OwnedSignalValue {
    Bits { width: u32, states: u8, data: Vec<u8> },
    Real(f64),
    VarLen(Vec<u8>),
}

impl OwnedSignalValue {
    pub fn borrow(&self) -> SignalValue<'_> {
        match self {
            OwnedSignalValue::Bits { width, states, data } => {
                SignalValue::Bits { width: *width, states: *states, data }
            }
            OwnedSignalValue::Real(r) => SignalValue::Real(*r),
            OwnedSignalValue::VarLen(b) => SignalValue::VarLen(b),
        }
    }
    pub fn to_ascii(&self) -> String {
        self.borrow().to_ascii()
    }
}

/// Default value of a signal before its first change.
pub fn default_value(kind: SignalKind, out: &mut Vec<u8>) {
    match kind {
        SignalKind::Bits { width, states } => {
            let n = packed_len(width, states);
            let start = out.len();
            out.resize(start + n, 0);
            if states != 2 {
                // All X.
                let dst = &mut out[start..];
                for i in 0..width as usize {
                    set_code(dst, states, i, LX);
                }
            }
        }
        SignalKind::Real => out.extend_from_slice(&0f64.to_le_bytes()),
        SignalKind::VarLen => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack() {
        let mut buf = [0u8; 2];
        assert!(pack_ascii(b"1011", 4, 2, &mut buf));
        assert_eq!(buf[0], 0b1011);
        let mut a = Vec::new();
        unpack_ascii(&buf, 4, 2, &mut a);
        assert_eq!(a, b"1011");

        let mut buf = [0u8; 2];
        assert!(!pack_ascii(b"x1z0", 4, 4, &mut buf));
        assert_eq!(get_code(&buf, 4, 3), LX);
        assert_eq!(get_code(&buf, 4, 2), L1);
        assert_eq!(get_code(&buf, 4, 1), LZ);
        assert_eq!(get_code(&buf, 4, 0), L0);
        let mut a = Vec::new();
        unpack_ascii(&buf, 4, 4, &mut a);
        assert_eq!(a, b"x1z0");
        assert!(!is_two_state(&buf, 4, 4));

        // Extension rule.
        let mut buf = [0u8; 1];
        pack_ascii(b"x", 4, 4, &mut buf);
        let mut a = Vec::new();
        unpack_ascii(&buf, 4, 4, &mut a);
        assert_eq!(a, b"xxxx");
        pack_ascii(b"1", 4, 4, &mut buf);
        a.clear();
        unpack_ascii(&buf, 4, 4, &mut a);
        assert_eq!(a, b"0001");

        let mut buf = [0u8; 3];
        assert!(!pack_ascii(b"UWLH-", 5, 9, &mut buf));
        let mut a = Vec::new();
        unpack_ascii(&buf, 5, 9, &mut a);
        assert_eq!(a, b"uwlh-");

        let mut w = Vec::new();
        widen(&[0b1011], 4, 2, 4, &mut w);
        let mut a = Vec::new();
        unpack_ascii(&w, 4, 4, &mut a);
        assert_eq!(a, b"1011");
        assert!(is_two_state(&w, 4, 4));
        let mut n = Vec::new();
        narrow_to_two_state(&w, 4, 4, &mut n);
        assert_eq!(n, vec![0b1011]);

        let mut buf = [0u8; 5];
        pack_words32(&[0xdeadbeef, 0x1], 33, &mut buf);
        assert_eq!(buf, [0xef, 0xbe, 0xad, 0xde, 0x01]);
        pack_words32(&[0xffffffff], 5, &mut buf);
        assert_eq!(buf[0], 0x1f);
        let v = SignalValue::Bits { width: 33, states: 2, data: &[0xef, 0xbe, 0xad, 0xde, 0x01] };
        assert_eq!(v.as_u64(), Some(0x1deadbeef));
    }
}
