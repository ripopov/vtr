//! Compression codecs used for section payloads and value-change groups.

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Codec {
    None = 0,
    Lz4 = 1,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compression {
    pub codec: Codec,
    pub level: i32,
}

impl Compression {
    pub const NONE: Compression = Compression {
        codec: Codec::None,
        level: 0,
    };
    pub const LZ4: Compression = Compression {
        codec: Codec::Lz4,
        level: 0,
    };
    pub const ZSTD_FAST: Compression = Compression {
        codec: Codec::Zstd,
        level: 1,
    };
    pub const ZSTD_DEFAULT: Compression = Compression {
        codec: Codec::Zstd,
        level: 3,
    };
    pub const ZSTD_HIGH: Compression = Compression {
        codec: Codec::Zstd,
        level: 9,
    };
}

impl Default for Compression {
    fn default() -> Self {
        Compression::ZSTD_FAST
    }
}

#[cfg(not(target_family = "wasm"))]
fn lz4_bound(length: usize) -> Result<usize> {
    let length = i32::try_from(length).map_err(|_| Error::Corrupt("LZ4 input exceeds i32"))?;
    // SAFETY: LZ4_compressBound accepts any integer and reports unsupported lengths with zero.
    let bound = unsafe { lz4_sys::LZ4_compressBound(length) };
    if bound <= 0 {
        return Err(Error::Corrupt("LZ4 input exceeds codec limit"));
    }
    Ok(bound as usize)
}
#[cfg(target_family = "wasm")]
fn lz4_bound(length: usize) -> Result<usize> {
    Ok(lz4_flex::block::get_maximum_output_size(length))
}

#[cfg(not(target_family = "wasm"))]
fn lz4_compress(input: &[u8], output: &mut [u8]) -> Result<usize> {
    let length = i32::try_from(input.len()).map_err(|_| Error::Corrupt("LZ4 input exceeds i32"))?;
    let capacity =
        i32::try_from(output.len()).map_err(|_| Error::Corrupt("LZ4 output exceeds i32"))?;
    // SAFETY: Both slices are live and disjoint, with lengths passed to the bounded C encoder.
    let size = unsafe {
        lz4_sys::LZ4_compress_default(
            input.as_ptr().cast(),
            output.as_mut_ptr().cast(),
            length,
            capacity,
        )
    };
    if size <= 0 {
        return Err(Error::Corrupt("LZ4 compression failed"));
    }
    Ok(size as usize)
}
#[cfg(target_family = "wasm")]
fn lz4_compress(input: &[u8], output: &mut [u8]) -> Result<usize> {
    lz4_flex::block::compress_into(input, output).map_err(|e| Error::Codec(e.to_string()))
}

#[cfg(not(target_family = "wasm"))]
fn lz4_decompress(input: &[u8], output: &mut [u8]) -> Result<usize> {
    let length = i32::try_from(input.len()).map_err(|_| Error::Corrupt("LZ4 input exceeds i32"))?;
    let capacity =
        i32::try_from(output.len()).map_err(|_| Error::Corrupt("LZ4 output exceeds i32"))?;
    // SAFETY: The decoder receives exact readable and writable slice bounds and rejects malformed input.
    let size = unsafe {
        lz4_sys::LZ4_decompress_safe(
            input.as_ptr().cast(),
            output.as_mut_ptr().cast(),
            length,
            capacity,
        )
    };
    if size < 0 {
        return Err(Error::Corrupt("invalid LZ4 block"));
    }
    Ok(size as usize)
}
#[cfg(target_family = "wasm")]
fn lz4_decompress(input: &[u8], output: &mut [u8]) -> Result<usize> {
    lz4_flex::block::decompress_into(input, output).map_err(|e| Error::Codec(e.to_string()))
}

pub struct Compressor {
    zstd: Option<ZstdContext>,
    level: i32,
    fast: Option<ZstdContext>,
    trial_buf: Vec<u8>,
}

struct ZstdContext(std::ptr::NonNull<zstd_sys::ZSTD_CCtx>);

// SAFETY: The context has a single owner and zstd permits moving it between threads.
unsafe impl Send for ZstdContext {}

const ZSTD_HEADER: usize = 16;

unsafe extern "C" fn zstd_alloc(_: *mut std::ffi::c_void, size: usize) -> *mut std::ffi::c_void {
    // SAFETY: The layout reserves an aligned header before the returned allocation.
    unsafe {
        let Some(total) = size.checked_add(ZSTD_HEADER) else {
            return std::ptr::null_mut();
        };
        let Ok(layout) = std::alloc::Layout::from_size_align(total, ZSTD_HEADER) else {
            return std::ptr::null_mut();
        };
        let p = std::alloc::alloc(layout);
        if p.is_null() {
            return p.cast();
        }
        p.cast::<usize>().write_unaligned(size);
        p.add(ZSTD_HEADER).cast()
    }
}

unsafe extern "C" fn zstd_free(_: *mut std::ffi::c_void, p: *mut std::ffi::c_void) {
    // SAFETY: Zstd returns only pointers allocated by zstd_alloc, exactly once.
    unsafe {
        if p.is_null() {
            return;
        }
        let base = p.cast::<u8>().sub(ZSTD_HEADER);
        let size = base.cast::<usize>().read_unaligned();
        std::alloc::dealloc(
            base,
            std::alloc::Layout::from_size_align_unchecked(size + ZSTD_HEADER, ZSTD_HEADER),
        );
    }
}

impl ZstdContext {
    fn new(level: i32) -> Result<ZstdContext> {
        let mem = zstd_sys::ZSTD_customMem {
            customAlloc: Some(zstd_alloc),
            customFree: Some(zstd_free),
            opaque: std::ptr::null_mut(),
        };
        // SAFETY: Both callbacks implement zstd allocation semantics and require no opaque state.
        let ctx = std::ptr::NonNull::new(unsafe { zstd_sys::ZSTD_createCCtx_advanced(mem) })
            .ok_or_else(|| Error::Codec("cannot create a zstd context".into()))?;
        let c = ZstdContext(ctx);
        c.set(zstd_sys::ZSTD_cParameter::ZSTD_c_compressionLevel, level)?;
        c.set(zstd_sys::ZSTD_cParameter::ZSTD_c_checksumFlag, 0)?;
        c.set(zstd_sys::ZSTD_cParameter::ZSTD_c_contentSizeFlag, 0)?;
        Ok(c)
    }

    fn set(&self, p: zstd_sys::ZSTD_cParameter, v: i32) -> Result<()> {
        // SAFETY: The owned context is live and this private method does not overlap compression.
        Self::check(unsafe { zstd_sys::ZSTD_CCtx_setParameter(self.0.as_ptr(), p, v) })?;
        Ok(())
    }

    fn check(code: usize) -> Result<usize> {
        // SAFETY: Zstd accepts any size_t result code here.
        if unsafe { zstd_sys::ZSTD_isError(code) } != 0 {
            // SAFETY: Zstd returns a static, NUL-terminated error string.
            let name = unsafe { std::ffi::CStr::from_ptr(zstd_sys::ZSTD_getErrorName(code)) };
            return Err(Error::Codec(name.to_string_lossy().into_owned()));
        }
        Ok(code)
    }

    fn compress_to_buffer(&mut self, src: &[u8], dst: &mut [u8]) -> Result<usize> {
        // SAFETY: The context is exclusively borrowed; source and destination are disjoint live slices.
        Self::check(unsafe {
            zstd_sys::ZSTD_compress2(
                self.0.as_ptr(),
                dst.as_mut_ptr().cast(),
                dst.len(),
                src.as_ptr().cast(),
                src.len(),
            )
        })
    }
}

impl Drop for ZstdContext {
    fn drop(&mut self) {
        // SAFETY: This is the sole owner of the context and no operation can outlive it.
        unsafe { zstd_sys::ZSTD_freeCCtx(self.0.as_ptr()) };
    }
}

impl Compressor {
    #[must_use]
    pub fn new() -> Self {
        Compressor {
            zstd: None,
            level: 0,
            fast: None,
            trial_buf: Vec::new(),
        }
    }

    fn zstd_ctx(&mut self, level: i32) -> Result<&mut ZstdContext> {
        if level == 1 {
            return self.fast_ctx();
        }
        if self.zstd.is_none() || self.level != level {
            self.zstd = Some(ZstdContext::new(level)?);
            self.level = level;
        }
        Ok(self.zstd.as_mut().unwrap())
    }

    fn fast_ctx(&mut self) -> Result<&mut ZstdContext> {
        if self.fast.is_none() {
            self.fast = Some(ZstdContext::new(1)?);
        }
        Ok(self.fast.as_mut().unwrap())
    }

    pub fn trial_size(&mut self, input: &[u8]) -> Result<usize> {
        let mut buf = std::mem::take(&mut self.trial_buf);
        buf.resize(zstd::zstd_safe::compress_bound(input.len()), 0);
        let n = self.fast_ctx()?.compress_to_buffer(input, &mut buf)?;
        self.trial_buf = buf;
        Ok(n)
    }

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

    #[must_use]
    pub fn sample_incompressible(compressed: usize, raw: usize) -> bool {
        compressed * 100 >= raw * 96
    }

    pub fn compress_into(
        &mut self,
        comp: Compression,
        input: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let incompressible = comp.codec == Codec::Zstd && self.looks_incompressible(input);
        self.compress_into_probed(comp, input, out, incompressible)
    }

    pub fn compress_into_probed(
        &mut self,
        comp: Compression,
        input: &[u8],
        out: &mut Vec<u8>,
        incompressible: bool,
    ) -> Result<()> {
        let comp = if comp.codec == Codec::Zstd && incompressible {
            Compression::LZ4
        } else {
            comp
        };
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
                let bound = lz4_bound(input.len())?;
                out.resize(payload_pos + bound, 0);
                let n = lz4_compress(input, &mut out[payload_pos..])?;
                out.truncate(payload_pos + n);
            }
            Codec::Zstd => {
                let bound = zstd::zstd_safe::compress_bound(input.len());
                out.resize(payload_pos + bound, 0);
                let ctx = self.zstd_ctx(comp.level)?;
                let n = ctx.compress_to_buffer(input, &mut out[payload_pos..])?;
                out.truncate(payload_pos + n);
            }
        }
        if out.len() - payload_pos >= input.len() {
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

pub fn raw_len(blob: &[u8]) -> Result<usize> {
    let mut r = crate::varint::Reader::new(blob);
    Codec::from_u8(r.u8()?)?;
    r.usize()
}

pub struct Decompressor {
    zstd: Option<zstd::bulk::Decompressor<'static>>,
}

impl Decompressor {
    #[must_use]
    pub fn new() -> Self {
        Decompressor { zstd: None }
    }

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
                out.resize(start + raw_len, 0);
                let n = lz4_decompress(payload, &mut out[start..])?;
                if n != raw_len {
                    return Err(Error::Corrupt("lz4 length mismatch"));
                }
                Ok(input.len())
            }
            Codec::Zstd => {
                if self.zstd.is_none() {
                    let d =
                        zstd::bulk::Decompressor::new().map_err(|e| Error::Codec(e.to_string()))?;
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
    fn allocator_preserves_payload_and_frees_matching_layouts() {
        for size in [0, 1, 15, 16, 17, 4096] {
            // SAFETY: Each pointer comes from zstd_alloc, stays within size bytes, and is freed once.
            unsafe {
                let p = zstd_alloc(std::ptr::null_mut(), size).cast::<u8>();
                assert!(!p.is_null());
                assert_eq!(p.addr() % ZSTD_HEADER, 0);
                for i in 0..size {
                    p.add(i).write((i % 251) as u8);
                }
                for i in 0..size {
                    assert_eq!(p.add(i).read(), (i % 251) as u8);
                }
                zstd_free(std::ptr::null_mut(), p.cast());
            }
        }
    }

    #[test]
    fn allocator_rejects_overflow_and_accepts_null_free() {
        // SAFETY: Rejected sizes return null; zstd_free explicitly accepts null.
        unsafe {
            assert!(zstd_alloc(std::ptr::null_mut(), usize::MAX).is_null());
            assert!(zstd_alloc(std::ptr::null_mut(), isize::MAX as usize).is_null());
            zstd_free(std::ptr::null_mut(), std::ptr::null_mut());
        }
    }

    #[test]
    fn roundtrip_all_codecs() {
        let data: Vec<u8> = (0..100_000u32).map(|i| ((i / 7) % 13) as u8).collect();
        let mut c = Compressor::new();
        let mut d = Decompressor::new();
        for comp in [
            Compression::NONE,
            Compression::LZ4,
            Compression::ZSTD_FAST,
            Compression::ZSTD_HIGH,
        ] {
            let mut out = Vec::new();
            c.compress_into(comp, &data, &mut out).unwrap();
            if comp.codec != Codec::None {
                assert!(out.len() < data.len() / 4, "{comp:?} did not compress");
            }
            let back = d.decompress(&out).unwrap();
            assert_eq!(back, data);
        }
        let mut x = 0x9E37_79B9_7F4A_7C15_u64;
        let noise: Vec<u8> = (0..4096u32)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let mut out = Vec::new();
        c.compress_into(Compression::ZSTD_FAST, &noise, &mut out)
            .unwrap();
        assert_eq!(out[0], Codec::None as u8);
        assert_eq!(d.decompress(&out).unwrap(), noise);
        let mut out = Vec::new();
        c.compress_into(Compression::LZ4, &[], &mut out).unwrap();
        assert_eq!(d.decompress(&out).unwrap(), Vec::<u8>::new());
    }
}
