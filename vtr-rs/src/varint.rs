//! LEB128 variable-length integers and zig-zag mapping.

use crate::error::{Error, Result};

#[inline]
pub fn put_u64(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

#[inline]
pub fn put_i64(out: &mut Vec<u8>, v: i64) {
    put_u64(out, zigzag(v));
}

#[inline]
#[must_use]
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
#[must_use]
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

#[inline]
#[must_use]
pub fn len_u64(v: u64) -> usize {
    if v == 0 {
        1
    } else {
        (70 - v.leading_zeros() as usize) / 7
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    #[inline]
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    #[inline]
    pub fn u64(&mut self) -> Result<u64> {
        if let Some(&byte) = self.buf.get(self.pos)
            && byte < 128
        {
            self.pos += 1;
            return Ok(u64::from(byte));
        }
        let mut value = 0;
        let mut pos = self.pos;
        for shift in (0..70).step_by(7) {
            let byte = *self
                .buf
                .get(pos)
                .ok_or_else(|| Error::corrupt("truncated varint"))?;
            pos += 1;
            if shift == 63 && byte > 1 {
                return Err(Error::Corrupt("varint overflow"));
            }
            value |= u64::from(byte & 127) << shift;
            if byte < 128 {
                if shift != 0 && byte == 0 {
                    return Err(Error::Corrupt("noncanonical varint"));
                }
                self.pos = pos;
                return Ok(value);
            }
        }
        Err(Error::Corrupt("varint overflow"))
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
        let bytes = self
            .buf
            .get(self.pos..)
            .and_then(|tail| tail.get(..n))
            .ok_or_else(|| Error::corrupt("truncated byte run"))?;
        self.pos += n;
        Ok(bytes)
    }

    #[inline]
    pub fn blob(&mut self) -> Result<&'a [u8]> {
        let n = self.usize()?;
        self.bytes(n)
    }
}

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
        let vals = [
            0u64,
            1,
            127,
            128,
            300,
            1 << 20,
            u64::from(u32::MAX),
            u64::MAX,
            (1 << 63) + 7,
        ];
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
        assert_eq!(r.pos, out.len());
        for v in [0i64, -1, 1, i64::MIN, i64::MAX, -1000, 1000] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
    }

    #[test]
    fn malformed_varints_and_lengths_fail_without_advancing() {
        for bytes in [
            vec![0x80, 0x80],
            vec![0x80, 0],
            vec![0x81, 0],
            [vec![0xff; 9], vec![2]].concat(),
            vec![0xff; 11],
        ] {
            let mut reader = Reader::new(&bytes);
            assert!(matches!(reader.u64(), Err(Error::Corrupt(_))));
            assert_eq!(reader.pos, 0);
        }
        let mut reader = Reader::new(&[1, 2]);
        assert_eq!(reader.u8().unwrap(), 1);
        assert!(matches!(reader.bytes(usize::MAX), Err(Error::Corrupt(_))));
        assert_eq!(reader.pos, 1);
        assert_eq!(reader.bytes(1).unwrap(), &[2]);
    }
}
