//! Signal value representation and bit packing.

use crate::hierarchy::SignalKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Logic {
    Zero = 0,
    One = 1,
    X = 2,
    Z = 3,
    U = 4,
    W = 5,
    L = 6,
    H = 7,
    DontCare = 8,
}

impl From<bool> for Logic {
    fn from(value: bool) -> Self {
        if value { Self::One } else { Self::Zero }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LogicStates {
    Two = 2,
    Four = 4,
    Nine = 9,
}

pub const L0: u8 = 0;
pub const L1: u8 = 1;
pub const LX: u8 = 2;
pub const LZ: u8 = 3;
pub const LU: u8 = 4;
pub const LW: u8 = 5;
pub const LL: u8 = 6;
pub const LH: u8 = 7;
pub const LDASH: u8 = 8;

pub const CODE_ASCII: [u8; 9] = *b"01xzuwlh-";

#[inline]
#[must_use]
pub fn code_from_ascii(c: u8) -> u8 {
    match c {
        b'0' => L0,
        b'1' => L1,
        b'z' | b'Z' => LZ,
        b'u' | b'U' => LU,
        b'w' | b'W' => LW,
        b'l' | b'L' => LL,
        b'h' | b'H' => LH,
        b'-' => LDASH,
        _ => LX,
    }
}

#[inline]
#[must_use]
pub fn get_code(data: &[u8], states: u8, i: usize) -> u8 {
    match states {
        2 => (data[i >> 3] >> (i & 7)) & 1,
        4 => (data[i >> 2] >> ((i & 3) * 2)) & 3,
        _ => (data[i >> 1] >> ((i & 1) * 4)) & 15,
    }
}

#[inline]
pub fn set_code(data: &mut [u8], states: u8, i: usize, c: u8) {
    match states {
        2 => data[i >> 3] |= (c & 1) << (i & 7),
        4 => data[i >> 2] |= (c & 3) << ((i & 3) * 2),
        _ => data[i >> 1] |= (c & 15) << ((i & 1) * 4),
    }
}

pub fn pack_ascii(s: &[u8], width: u32, states: u8, out: &mut [u8]) -> bool {
    let w = width as usize;
    for b in out.iter_mut() {
        *b = 0;
    }
    let mut two_state = true;
    let n = s.len();
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
        let c = if i < n {
            code_from_ascii(s[n - 1 - i])
        } else {
            ext
        };
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

pub fn unpack_ascii(data: &[u8], width: u32, states: u8, out: &mut Vec<u8>) {
    let w = width as usize;
    out.reserve(w);
    for i in (0..w).rev() {
        let c = get_code(data, states, i);
        out.push(CODE_ASCII[(c as usize).min(8)]);
    }
}

pub fn widen(data: &[u8], width: u32, from: u8, to: u8, out: &mut Vec<u8>) {
    let start = out.len();
    out.resize(start + packed_len(width, to), 0);
    if from == to {
        out[start..].copy_from_slice(&data[..packed_len(width, from)]);
        return;
    }
    let dst = &mut out[start..];
    if from == 2 && to == 4 {
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

pub fn pack_words32(words: &[u32], width: u32, out: &mut [u8]) {
    let bytes = (width as usize).div_ceil(8);
    for (i, b) in out[..bytes].iter_mut().enumerate() {
        let w = words.get(i / 4).copied().unwrap_or(0);
        *b = (w >> ((i % 4) * 8)) as u8;
    }
    trim_top_bits(out, width);
}

#[inline]
pub fn trim_top_bits(out: &mut [u8], width: u32) {
    let rem = width % 8;
    if rem != 0 {
        let last = (width as usize).div_ceil(8) - 1;
        out[last] &= (1u8 << rem) - 1;
    }
}

#[must_use]
pub fn is_two_state(data: &[u8], width: u32, states: u8) -> bool {
    match states {
        2 => true,
        4 => {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SignalValue<'a> {
    Bits {
        width: u32,
        states: u8,
        data: &'a [u8],
    },
    Real(f64),
    VarLen(&'a [u8]),
}

impl SignalValue<'_> {
    #[must_use]
    pub fn to_ascii(&self) -> String {
        match *self {
            SignalValue::Bits {
                width,
                states,
                data,
            } => {
                let mut v = Vec::with_capacity(width as usize);
                unpack_ascii(data, width, states, &mut v);
                String::from_utf8(v).unwrap()
            }
            SignalValue::Real(r) => format!("{r}"),
            SignalValue::VarLen(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }

    #[must_use]
    #[inline]
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            SignalValue::Bits {
                width,
                states,
                data,
            } if width <= 64 => {
                if states == 2 {
                    let length = (width as usize).div_ceil(8);
                    let bytes = data.get(..length)?;
                    let value = match length {
                        0 => 0,
                        1 => u64::from(bytes[0]),
                        2 => u64::from(u16::from_le_bytes(bytes.try_into().unwrap())),
                        3 => {
                            u64::from(u16::from_le_bytes(bytes[..2].try_into().unwrap()))
                                | (u64::from(bytes[2]) << 16)
                        }
                        4 => u64::from(u32::from_le_bytes(bytes.try_into().unwrap())),
                        8 => u64::from_le_bytes(bytes.try_into().unwrap()),
                        _ => {
                            let low = u64::from(u32::from_le_bytes(bytes[..4].try_into().unwrap()));
                            bytes[4..].iter().enumerate().fold(low, |word, (i, &byte)| {
                                word | (u64::from(byte) << (32 + i * 8))
                            })
                        }
                    };
                    let mask = u64::MAX.checked_shr(64 - width).unwrap_or(0);
                    return Some(value & mask);
                }
                if !is_two_state(data, width, states) {
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

    #[must_use]
    pub fn to_owned(&self) -> OwnedSignalValue {
        match *self {
            SignalValue::Bits {
                width,
                states,
                data,
            } => OwnedSignalValue::Bits {
                width,
                states,
                data: data.to_vec(),
            },
            SignalValue::Real(r) => OwnedSignalValue::Real(r),
            SignalValue::VarLen(b) => OwnedSignalValue::VarLen(b.to_vec()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum OwnedSignalValue {
    Bits {
        width: u32,
        states: u8,
        data: Vec<u8>,
    },
    Real(f64),
    VarLen(Vec<u8>),
}

impl OwnedSignalValue {
    pub(crate) fn copy_from(&mut self, value: SignalValue<'_>) {
        match (self, value) {
            (
                Self::Bits {
                    width,
                    states,
                    data,
                },
                SignalValue::Bits {
                    width: w,
                    states: s,
                    data: bytes,
                },
            ) => {
                *width = w;
                *states = s;
                data.clear();
                data.extend_from_slice(bytes);
            }
            (Self::Real(value), SignalValue::Real(next)) => *value = next,
            (Self::VarLen(value), SignalValue::VarLen(next)) => {
                value.clear();
                value.extend_from_slice(next);
            }
            (target, value) => *target = value.to_owned(),
        }
    }

    #[must_use]
    pub fn borrow(&self) -> SignalValue<'_> {
        match self {
            OwnedSignalValue::Bits {
                width,
                states,
                data,
            } => SignalValue::Bits {
                width: *width,
                states: *states,
                data,
            },
            OwnedSignalValue::Real(r) => SignalValue::Real(*r),
            OwnedSignalValue::VarLen(b) => SignalValue::VarLen(b),
        }
    }
    #[must_use]
    pub fn to_ascii(&self) -> String {
        self.borrow().to_ascii()
    }
}

pub fn default_value(kind: SignalKind, out: &mut Vec<u8>) {
    match kind {
        SignalKind::Bits { width, states } => {
            let n = packed_len(width, states);
            let start = out.len();
            out.resize(start + n, 0);
            if states != 2 {
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

#[inline]
#[must_use]
pub fn packed_len(width: u32, states: u8) -> usize {
    let width = width as usize;
    match states {
        2 => width.div_ceil(8),
        4 => width.div_ceil(4),
        _ => width.div_ceil(2),
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
        pack_words32(&[0xdead_beef, 0x1], 33, &mut buf);
        assert_eq!(buf, [0xef, 0xbe, 0xad, 0xde, 0x01]);
        pack_words32(&[0xffff_ffff], 5, &mut buf);
        assert_eq!(buf[0], 0x1f);
        let v = SignalValue::Bits {
            width: 33,
            states: 2,
            data: &[0xef, 0xbe, 0xad, 0xde, 0x01],
        };
        assert_eq!(v.as_u64(), Some(0x0001_dead_beef));
    }
}
