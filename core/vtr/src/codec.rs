//! Compression codecs used for section payloads and value-change groups.
//!
//! Every compressed blob in a VTR file is prefixed by the caller with its
//! codec id and uncompressed length, so a reader can always allocate the
//! exact output buffer.

use crate::error::{Error, Result};

/// Compression codec identifier as stored in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Codec {
    /// No compression.
    None = 0,
    /// LZ4 block format (raw block, no frame).
    Lz4 = 1,
    /// Zstandard frame.
    Zstd = 2,
}

impl Codec {
    pub fn from_u8(v: u8) -> Result<Codec> {
        Ok(match v {
            0 => Codec::None,
            1 => Codec::Lz4,
            2 => Codec::Zstd,
            _ => return Err(Error::Corrupt("unknown codec id")),
        })
    }
}

/// Compression settings chosen by the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compression {
    pub codec: Codec,
    /// Codec-specific level (zstd: 1..=22; ignored for lz4/none).
    pub level: i32,
}

impl Compression {
    pub const NONE: Compression = Compression { codec: Codec::None, level: 0 };
    pub const LZ4: Compression = Compression { codec: Codec::Lz4, level: 0 };
    pub const ZSTD_FAST: Compression = Compression { codec: Codec::Zstd, level: 1 };
    pub const ZSTD_DEFAULT: Compression = Compression { codec: Codec::Zstd, level: 3 };
    pub const ZSTD_HIGH: Compression = Compression { codec: Codec::Zstd, level: 9 };
}

impl Default for Compression {
    fn default() -> Self {
        Compression::ZSTD_FAST
    }
}

/// Reusable compressor state (zstd contexts are expensive to create): one context
/// for the configured level and one at level 1 for probes, trials and fast runs.
pub struct Compressor {
    zstd: Option<zstd::bulk::Compressor<'static>>,
    level: i32,
    fast: Option<zstd::bulk::Compressor<'static>>,
    trial_buf: Vec<u8>,
}

fn new_zstd(level: i32) -> Result<zstd::bulk::Compressor<'static>> {
    let mut c = zstd::bulk::Compressor::new(level).map_err(|e| Error::Codec(e.to_string()))?;
    // Content size is known from our own prefix; skip zstd's own
    // checksum since sections carry CRC32 already.
    let _ = c.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(false));
    let _ = c.include_contentsize(false);
    Ok(c)
}

impl Compressor {
    pub fn new() -> Self {
        Compressor { zstd: None, level: 0, fast: None, trial_buf: Vec::new() }
    }

    fn zstd_ctx(&mut self, level: i32) -> Result<&mut zstd::bulk::Compressor<'static>> {
        if level == 1 {
            return self.fast_ctx();
        }
        if self.zstd.is_none() || self.level != level {
            self.zstd = Some(new_zstd(level)?);
            self.level = level;
        }
        Ok(self.zstd.as_mut().unwrap())
    }

    fn fast_ctx(&mut self) -> Result<&mut zstd::bulk::Compressor<'static>> {
        if self.fast.is_none() {
            self.fast = Some(new_zstd(1)?);
        }
        Ok(self.fast.as_mut().unwrap())
    }

    /// Compressed size of `input` under fast zstd (level 1), for comparing encodings of a
    /// sample; nothing is emitted.
    pub fn trial_size(&mut self, input: &[u8]) -> Result<usize> {
        let mut buf = std::mem::take(&mut self.trial_buf);
        buf.resize(zstd::zstd_safe::compress_bound(input.len()), 0);
        let n = self.fast_ctx()?.compress_to_buffer(input, &mut buf).map_err(|e| Error::Codec(e.to_string()))?;
        self.trial_buf = buf;
        Ok(n)
    }

    /// Incompressibility probe: zstd level 1 on a 4 KiB sample from the middle of the
    /// input. Data that the fast level cannot shrink by 4% gains nothing from the
    /// expensive level either; such runs are handed to LZ4 (which bails out quickly).
    fn looks_incompressible(&mut self, input: &[u8]) -> bool {
        const SAMPLE: usize = 4096;
        if input.len() < 4 * SAMPLE {
            return false;
        }
        let start = (input.len() - SAMPLE) / 2;
        match self.trial_size(&input[start..start + SAMPLE]) {
            Ok(c) => Self::sample_incompressible(c, SAMPLE),
            Err(_) => false,
        }
    }

    /// The probe's verdict for a sample of `raw` bytes that fast zstd shrank to `compressed`.
    pub fn sample_incompressible(compressed: usize, raw: usize) -> bool {
        compressed * 100 >= raw * 96
    }

    /// Compresses `input` with `comp` and appends `[codec:u8][raw_len:varint][payload]` to `out`.
    /// Falls back to `Codec::None` when compression does not shrink the data. High-entropy
    /// inputs (detected by sampling) skip the expensive codec and are stored raw or LZ4-packed.
    pub fn compress_into(&mut self, comp: Compression, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let incompressible = comp.codec == Codec::Zstd && self.looks_incompressible(input);
        self.compress_into_probed(comp, input, out, incompressible)
    }

    /// [`compress_into`](Self::compress_into) for a caller that already sampled the input:
    /// `incompressible` routes zstd requests to LZ4.
    pub fn compress_into_probed(&mut self, comp: Compression, input: &[u8], out: &mut Vec<u8>, incompressible: bool) -> Result<()> {
        let comp = if comp.codec == Codec::Zstd && incompressible { Compression::LZ4 } else { comp };
        let header_pos = out.len();
        out.push(comp.codec as u8);
        crate::varint::put_u64(out, input.len() as u64);
        let payload_pos = out.len();
        match comp.codec {
            Codec::None => {
                out.extend_from_slice(input);
                return Ok(());
            }
            Codec::Lz4 => {
                let bound = lz4_flex::block::get_maximum_output_size(input.len());
                out.resize(payload_pos + bound, 0);
                let n = lz4_flex::block::compress_into(input, &mut out[payload_pos..])
                    .map_err(|e| Error::Codec(e.to_string()))?;
                out.truncate(payload_pos + n);
            }
            Codec::Zstd => {
                let bound = zstd::zstd_safe::compress_bound(input.len());
                out.resize(payload_pos + bound, 0);
                let ctx = self.zstd_ctx(comp.level)?;
                let n = ctx
                    .compress_to_buffer(input, &mut out[payload_pos..])
                    .map_err(|e| Error::Codec(e.to_string()))?;
                out.truncate(payload_pos + n);
            }
        }
        if out.len() - payload_pos >= input.len() {
            // Not worth it: store raw.
            out.truncate(header_pos);
            out.push(Codec::None as u8);
            crate::varint::put_u64(out, input.len() as u64);
            out.extend_from_slice(input);
        }
        Ok(())
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

/// Reusable decompressor state.
pub struct Decompressor {
    zstd: Option<zstd::bulk::Decompressor<'static>>,
}

impl Decompressor {
    pub fn new() -> Self {
        Decompressor { zstd: None }
    }

    /// Decodes a blob written by [`Compressor::compress_into`]. Returns the number
    /// of input bytes consumed. Output is appended to `out`.
    pub fn decompress_into(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<usize> {
        let mut r = crate::varint::Reader::new(input);
        let codec = Codec::from_u8(r.u8()?)?;
        let raw_len = r.usize()?;
        let payload = &input[r.pos..];
        let start = out.len();
        match codec {
            Codec::None => {
                if payload.len() < raw_len {
                    return Err(Error::Corrupt("truncated raw blob"));
                }
                out.extend_from_slice(&payload[..raw_len]);
                Ok(r.pos + raw_len)
            }
            Codec::Lz4 => {
                // LZ4 blocks do not self-delimit; the caller must pass exactly one blob.
                out.resize(start + raw_len, 0);
                let n = lz4_flex::block::decompress_into(payload, &mut out[start..])
                    .map_err(|e| Error::Codec(e.to_string()))?;
                if n != raw_len {
                    return Err(Error::Corrupt("lz4 length mismatch"));
                }
                Ok(input.len())
            }
            Codec::Zstd => {
                if self.zstd.is_none() {
                    let d = zstd::bulk::Decompressor::new().map_err(|e| Error::Codec(e.to_string()))?;
                    self.zstd = Some(d);
                }
                out.resize(start + raw_len, 0);
                let n = self
                    .zstd
                    .as_mut()
                    .unwrap()
                    .decompress_to_buffer(payload, &mut out[start..])
                    .map_err(|e| Error::Codec(e.to_string()))?;
                if n != raw_len {
                    return Err(Error::Corrupt("zstd length mismatch"));
                }
                Ok(input.len())
            }
        }
    }

    pub fn decompress(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.decompress_into(input, &mut out)?;
        Ok(out)
    }
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_codecs() {
        let data: Vec<u8> = (0..100_000u32).map(|i| ((i / 7) % 13) as u8).collect();
        let mut c = Compressor::new();
        let mut d = Decompressor::new();
        for comp in [Compression::NONE, Compression::LZ4, Compression::ZSTD_FAST, Compression::ZSTD_HIGH] {
            let mut out = Vec::new();
            c.compress_into(comp, &data, &mut out).unwrap();
            if comp.codec != Codec::None {
                assert!(out.len() < data.len() / 4, "{:?} did not compress", comp);
            }
            let back = d.decompress(&out).unwrap();
            assert_eq!(back, data);
        }
        // Incompressible data falls back to raw.
        let mut x = 0x9E3779B97F4A7C15u64;
        let noise: Vec<u8> = (0..4096u32)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let mut out = Vec::new();
        c.compress_into(Compression::ZSTD_FAST, &noise, &mut out).unwrap();
        assert_eq!(out[0], Codec::None as u8);
        assert_eq!(d.decompress(&out).unwrap(), noise);
        let mut out = Vec::new();
        c.compress_into(Compression::LZ4, &[], &mut out).unwrap();
        assert_eq!(d.decompress(&out).unwrap(), Vec::<u8>::new());
    }
}
