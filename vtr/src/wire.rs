//! Bounded, canonical decoding for independently framed VTR data.

use crate::{Error, Result};

pub(crate) const MAX_BLOCK_BYTES: usize = 1 << 30;

pub(crate) fn pack_columns<const N: usize>(columns: &[Vec<u8>; N]) -> Result<Vec<u8>> {
    use crate::varint::put_u64;
    let size = columns
        .iter()
        .try_fold(N * 10 + 10, |size, column| {
            size.checked_add(column.len())
                .filter(|size| *size < MAX_BLOCK_BYTES)
        })
        .ok_or_else(|| Error::corrupt("column block exceeds format limit"))?;
    let mut out = Vec::with_capacity(size);
    put_u64(&mut out, N as u64);
    for column in columns {
        put_u64(&mut out, column.len() as u64);
    }
    for column in columns {
        out.extend_from_slice(column);
    }
    Ok(out)
}

pub(crate) fn unpack_columns<const N: usize>(data: &[u8]) -> Result<[&[u8]; N]> {
    let mut cursor = Cursor::new(data);
    if cursor.var()? != N as u64 {
        return Err(Error::Corrupt("invalid column count"));
    }
    let mut lengths = [0; N];
    for length in &mut lengths {
        *length = usize::try_from(cursor.var()?)
            .map_err(|_| Error::Corrupt("column size exceeds usize"))?;
    }
    let mut columns = [&[][..]; N];
    for (column, length) in columns.iter_mut().zip(lengths) {
        *column = cursor.take(length)?;
    }
    cursor.done()?;
    Ok(columns)
}

pub(crate) fn compress(data: &[u8], compression: crate::Compression) -> Result<Vec<u8>> {
    if data.len() >= MAX_BLOCK_BYTES {
        return Err(Error::invalid("compression block exceeds format limit"));
    }
    let mut out = Vec::new();
    crate::codec::Compressor::new().compress_into(compression, data, &mut out)?;
    Ok(out)
}

pub(crate) fn decompress(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(data);
    let codec = crate::Codec::from_u8(cursor.byte()?)?;
    let size = cursor.var()?;
    if size >= MAX_BLOCK_BYTES as u64 || size != expected as u64 {
        return Err(Error::Corrupt("decompressed length exceeds bounds"));
    }
    if codec == crate::Codec::None && cursor.remaining() != expected {
        return Err(Error::Corrupt("stored blob size mismatch"));
    }
    let mut out = Vec::new();
    let consumed = crate::codec::Decompressor::new().decompress_into(data, &mut out)?;
    if consumed != data.len() || out.len() != expected {
        return Err(Error::Corrupt("compressed blob size mismatch"));
    }
    Ok(out)
}

pub(crate) fn decompress_any(data: &[u8]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(data);
    cursor.byte()?;
    let expected =
        usize::try_from(cursor.var()?).map_err(|_| Error::Corrupt("blob length exceeds usize"))?;
    decompress(data, expected)
}

#[derive(Clone, Copy)]
pub(crate) struct Cursor<'a> {
    data: &'a [u8],
}

impl<'a> Cursor<'a> {
    #[inline]
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    #[inline]
    pub(crate) fn remaining(&self) -> usize {
        self.data.len()
    }

    #[inline]
    pub(crate) fn starts_with(&self, prefix: &[u8]) -> bool {
        self.data.starts_with(prefix)
    }

    #[inline]
    pub(crate) fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let (head, tail) = self
            .data
            .split_at_checked(size)
            .ok_or_else(|| Error::corrupt("truncated VTR data"))?;
        self.data = tail;
        Ok(head)
    }

    #[inline]
    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    #[inline]
    pub(crate) fn fixed<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    #[inline]
    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }

    #[inline]
    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    #[inline]
    pub(crate) fn var(&mut self) -> Result<u64> {
        if let Some((&byte, tail)) = self.data.split_first()
            && byte < 128
        {
            self.data = tail;
            return Ok(u64::from(byte));
        }
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.byte()?;
            if shift == 63 && byte > 1 {
                return Err(Error::Corrupt("varint overflow"));
            }
            value |= u64::from(byte & 127) << shift;
            if byte < 128 {
                if shift != 0 && byte == 0 {
                    return Err(Error::Corrupt("noncanonical varint"));
                }
                return Ok(value);
            }
        }
        Err(Error::Corrupt("varint overflow"))
    }

    #[inline]
    pub(crate) fn id(&mut self) -> Result<u32> {
        u32::try_from(self.var()?).map_err(|_| Error::Corrupt("identifier exceeds u32"))
    }

    #[inline]
    pub(crate) fn blob(&mut self) -> Result<&'a [u8]> {
        let size =
            usize::try_from(self.var()?).map_err(|_| Error::Corrupt("blob size exceeds usize"))?;
        self.take(size)
    }

    #[inline]
    pub(crate) fn text(&mut self) -> Result<&'a str> {
        std::str::from_utf8(self.blob()?).map_err(|_| Error::Corrupt("invalid UTF-8"))
    }

    #[inline]
    pub(crate) fn done(self) -> Result<()> {
        if self.data.is_empty() {
            Ok(())
        } else {
            Err(Error::Corrupt("trailing VTR data"))
        }
    }
}
