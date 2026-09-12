//! LEB128 variable-length integers and zig-zag mapping.
//!
//! Every variable-length integer in a VTR file is an unsigned LEB128 value
//! (7 payload bits per byte, little-endian order, high bit = continuation).
//! Signed quantities are zig-zag mapped first so small negatives stay short.

use crate::error::{Error, Result};

/// Appends `v` as unsigned LEB128.
#[inline]
pub fn put_u64(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Appends `v` zig-zag mapped as unsigned LEB128.
#[inline]
pub fn put_i64(out: &mut Vec<u8>, v: i64) {
    put_u64(out, zigzag(v));
}

#[inline]
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Number of bytes `put_u64` would emit.
#[inline]
pub fn len_u64(v: u64) -> usize {
    if v == 0 {
        1
    } else {
        (70 - v.leading_zeros() as usize) / 7
    }
}

/// Cursor over a byte slice for decoding.
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }

    #[inline]
    pub fn u64(&mut self) -> Result<u64> {
        let buf = self.buf;
        let mut pos = self.pos;
        // Fast path: single byte.
        if pos < buf.len() {
            let b = buf[pos];
            if b < 0x80 {
                self.pos = pos + 1;
                return Ok(b as u64);
            }
        }
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            if pos >= buf.len() {
                return Err(Error::Corrupt("truncated varint"));
            }
            let b = buf[pos];
            pos += 1;
            if shift >= 64 {
                return Err(Error::Corrupt("varint overflow"));
            }
            result |= ((b & 0x7f) as u64) << shift;
            if b < 0x80 {
                break;
            }
            shift += 7;
        }
        self.pos = pos;
        Ok(result)
    }

    #[inline]
    pub fn i64(&mut self) -> Result<i64> {
        Ok(unzigzag(self.u64()?))
    }

    #[inline]
    pub fn u32(&mut self) -> Result<u32> {
        let v = self.u64()?;
        u32::try_from(v).map_err(|_| Error::Corrupt("varint exceeds u32"))
    }

    #[inline]
    pub fn usize(&mut self) -> Result<usize> {
        let v = self.u64()?;
        usize::try_from(v).map_err(|_| Error::Corrupt("varint exceeds usize"))
    }

    #[inline]
    pub fn u8(&mut self) -> Result<u8> {
        if self.pos < self.buf.len() {
            let b = self.buf[self.pos];
            self.pos += 1;
            Ok(b)
        } else {
            Err(Error::Corrupt("truncated byte"))
        }
    }

    #[inline]
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n <= self.buf.len() {
            let s = &self.buf[self.pos..self.pos + n];
            self.pos += n;
            Ok(s)
        } else {
            Err(Error::Corrupt("truncated byte run"))
        }
    }

    /// Length-prefixed byte string.
    #[inline]
    pub fn blob(&mut self) -> Result<&'a [u8]> {
        let n = self.usize()?;
        self.bytes(n)
    }

    #[inline]
    pub fn f64(&mut self) -> Result<f64> {
        let b = self.bytes(8)?;
        Ok(f64::from_le_bytes(b.try_into().unwrap()))
    }

    #[inline]
    pub fn fixed_u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }

    #[inline]
    pub fn fixed_u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }
}

/// Appends a length-prefixed byte string.
#[inline]
pub fn put_blob(out: &mut Vec<u8>, b: &[u8]) {
    put_u64(out, b.len() as u64);
    out.extend_from_slice(b);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let vals = [0u64, 1, 127, 128, 300, 1 << 20, u32::MAX as u64, u64::MAX, (1 << 63) + 7];
        let mut out = Vec::new();
        for v in vals {
            let start = out.len();
            put_u64(&mut out, v);
            assert_eq!(out.len() - start, len_u64(v));
        }
        let mut r = Reader::new(&out);
        for v in vals {
            assert_eq!(r.u64().unwrap(), v);
        }
        assert!(r.is_empty());
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -1000, 1000] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
    }

    #[test]
    fn truncated() {
        let mut r = Reader::new(&[0x80, 0x80]);
        assert!(r.u64().is_err());
    }
}
