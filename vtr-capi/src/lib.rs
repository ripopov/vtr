//! Stable C ABI over the VTR Rust implementation.
//!
//! # Pointer safety
//!
//! Non-null handles must come from the matching constructor, remain live throughout
//! a call, and be closed exactly once. A mutable handle requires exclusive access.
//! Input pointers must be aligned and readable for their declared lengths; strings
//! must be NUL-terminated. Output pointers must be aligned, writable, and disjoint
//! from borrowed inputs. Null pointers are accepted only where checked explicitly.
//! Nested pointers in values obey the same rules. Borrowed output data remains
//! valid only while its owning handle is live and unchanged.
//!
//! Registered callbacks and their user data must remain valid until unregistered,
//! including calls already in progress, and must permit invocation from the rescue
//! thread without unwinding. A watched writer must stay at a stable address and
//! remain on its registering thread until it is unwatched.

#![allow(
    non_camel_case_types,
    reason = "Rust names mirror the public C ABI types."
)]

#[cfg(feature = "private-heap")]
mod heap;

#[cfg(feature = "private-heap")]
#[global_allocator]
static HEAP: heap::VtrHeap = heap::VtrHeap;

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int};
use std::ptr;
use vtr::{
    ClockId, ClockTimeline, Direction, Ending, Error, LogArg, LogArgType, LogQuery, LogRecord,
    LogSiteId, LogSiteSpec, NodeData, NodeId, Reader, ScopeType, Severity, SignalId, SignalKind,
    StrId, Transaction, TxKind, TxQuery, TxStatus, Value, VarType, Writer, WriterOptions,
};

pub const VTR_OK: c_int = 0;
pub const VTR_ERR_INVALID: c_int = 1;
pub const VTR_ERR_STATE: c_int = 2;
pub const VTR_ERR_IO: c_int = 3;
pub const VTR_ERR_CORRUPT: c_int = 4;
pub const VTR_ERR_VERSION: c_int = 5;
pub const VTR_ERR_CODEC: c_int = 6;
pub const VTR_ERR_CHECKSUM: c_int = 7;
pub const VTR_ERR_NULL: c_int = 8;
pub const VTR_ERR_NOT_FOUND: c_int = 9;
pub const VTR_NONE: u32 = u32::MAX;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("").unwrap());
}

fn set_error(msg: &str) {
    LAST_ERROR.with(|e| *e.borrow_mut() = CString::new(msg.replace('\0', " ")).unwrap());
}

fn code(e: &Error) -> c_int {
    match e {
        Error::Corrupt(_) => VTR_ERR_CORRUPT,
        Error::UnsupportedVersion { .. } => VTR_ERR_VERSION,
        Error::Invalid(_) => VTR_ERR_INVALID,
        Error::State(_) => VTR_ERR_STATE,
        Error::Io(_) => VTR_ERR_IO,
        Error::Checksum { .. } => VTR_ERR_CHECKSUM,
        Error::Codec(_) => VTR_ERR_CODEC,
    }
}

fn status(r: vtr::Result<()>) -> c_int {
    match r {
        Ok(()) => VTR_OK,
        Err(e) => {
            set_error(&e.to_string());
            code(&e)
        }
    }
}

trait PanicDefault {
    fn panic_default() -> Self;
}
impl PanicDefault for c_int {
    fn panic_default() -> Self {
        VTR_ERR_STATE
    }
}
impl PanicDefault for u32 {
    fn panic_default() -> Self {
        VTR_NONE
    }
}
impl PanicDefault for u64 {
    fn panic_default() -> Self {
        0
    }
}
impl PanicDefault for usize {
    fn panic_default() -> Self {
        0
    }
}
impl PanicDefault for () {
    fn panic_default() -> Self {}
}
impl<T> PanicDefault for *mut T {
    fn panic_default() -> Self {
        ptr::null_mut()
    }
}
impl<T> PanicDefault for *const T {
    fn panic_default() -> Self {
        ptr::null()
    }
}

#[inline(always)]
fn ffi<R: PanicDefault>(f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| p.downcast_ref::<String>().map(std::string::String::as_str))
                .unwrap_or("unknown panic");
            set_error(&format!("internal error in libvtr: {msg}"));
            R::panic_default()
        }
    }
}

#[inline(always)]
unsafe fn ffi_w<R: PanicDefault>(w: *const vtr_writer, f: impl FnOnce() -> R) -> R {
    // SAFETY: The caller provides a live writer; its shared crash state outlives the callback.
    unsafe {
        let Some(state) = w
            .as_ref()
            .map(|w| std::ptr::from_ref::<vtr::CrashState>(w.0.state()))
        else {
            return ffi(f);
        };
        let state = &*state;
        if state.poisoned() {
            set_error("the writer is poisoned by an earlier panic; close it");
            return R::panic_default();
        }
        if !state.watched() {
            return match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                Ok(r) => r,
                Err(p) => {
                    state.poison();
                    let msg = p
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| p.downcast_ref::<String>().map(std::string::String::as_str))
                        .unwrap_or("unknown panic");
                    set_error(&format!("internal error in libvtr: {msg}"));
                    R::panic_default()
                }
            };
        }
        let prev = state.enter();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(r) => {
                state.leave(prev);
                r
            }
            Err(p) => {
                state.poison();
                let msg = p
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| p.downcast_ref::<String>().map(std::string::String::as_str))
                    .unwrap_or("unknown panic");
                set_error(&format!("internal error in libvtr: {msg}"));
                R::panic_default()
            }
        }
    }
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    // SAFETY: The caller keeps the non-null, NUL-terminated input alive for the returned borrow.
    unsafe {
        if p.is_null() {
            return None;
        }
        CStr::from_ptr(p).to_str().ok()
    }
}

macro_rules! need {
    ($p:expr) => {
        match $p.as_mut() {
            Some(x) => x,
            None => {
                set_error("null handle");
                return VTR_ERR_NULL;
            }
        }
    };
}

macro_rules! need_ref {
    ($p:expr) => {
        match $p.as_ref() {
            Some(x) => x,
            None => {
                set_error("null handle");
                return VTR_ERR_NULL;
            }
        }
    };
}

macro_rules! need_str {
    ($p:expr) => {
        match cstr($p) {
            Some(s) => s,
            None => {
                set_error("null or non-UTF-8 string");
                return VTR_ERR_NULL;
            }
        }
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0")
        .as_ptr()
        .cast::<c_char>()
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct vtr_value {
    pub tag: u8,
    pub b: u8,
    pub width: u32,
    pub i: i64,
    pub u: u64,
    pub f: f64,
    pub str_id: u32,
    pub scale: i32,
    pub data: *const u8,
    pub len: usize,
}

pub const VTR_VAL_NULL: u8 = 0;
pub const VTR_VAL_BOOL: u8 = 1;
pub const VTR_VAL_I64: u8 = 2;
pub const VTR_VAL_U64: u8 = 3;
pub const VTR_VAL_F64: u8 = 4;
pub const VTR_VAL_STR: u8 = 5;
pub const VTR_VAL_BYTES: u8 = 6;
pub const VTR_VAL_BITS: u8 = 7;
pub const VTR_VAL_LOGIC: u8 = 8;
pub const VTR_VAL_LOGIC9: u8 = 9;
pub const VTR_VAL_TIME: u8 = 10;
pub const VTR_VAL_ENUM: u8 = 11;
pub const VTR_VAL_POINTER: u8 = 12;
pub const VTR_VAL_FIXED: u8 = 13;
pub const VTR_VAL_UFIXED: u8 = 14;
pub const VTR_VAL_LIST: u8 = 15;
pub const VTR_VAL_MAP: u8 = 16;
pub const VTR_VAL_TEXT: u8 = 17;

impl Default for vtr_value {
    fn default() -> Self {
        vtr_value {
            tag: 0,
            b: 0,
            width: 0,
            i: 0,
            u: 0,
            f: 0.0,
            str_id: 0,
            scale: 0,
            data: ptr::null(),
            len: 0,
        }
    }
}

unsafe fn to_value(v: &vtr_value) -> Result<Value, Error> {
    // SAFETY: The caller guarantees that each non-null value buffer is readable for its declared length.
    unsafe {
        let bytes = |v: &vtr_value| -> Vec<u8> {
            if v.data.is_null() || v.len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(v.data, v.len).to_vec()
            }
        };
        Ok(match v.tag {
            VTR_VAL_NULL => Value::Null,
            VTR_VAL_BOOL => Value::Bool(v.b != 0),
            VTR_VAL_I64 => Value::I64(v.i),
            VTR_VAL_U64 => Value::U64(v.u),
            VTR_VAL_F64 => Value::F64(v.f),
            VTR_VAL_STR => Value::Str(StrId(v.str_id)),
            VTR_VAL_BYTES => Value::Bytes(bytes(v)),
            VTR_VAL_BITS => Value::Bits {
                width: v.width,
                data: bytes(v),
            },
            VTR_VAL_LOGIC => Value::Logic {
                width: v.width,
                data: bytes(v),
            },
            VTR_VAL_LOGIC9 => Value::Logic9 {
                width: v.width,
                data: bytes(v),
            },
            VTR_VAL_TIME => Value::Time(v.u),
            VTR_VAL_ENUM => Value::Enum {
                value: v.i,
                name: StrId(v.str_id),
            },
            VTR_VAL_POINTER => Value::Pointer(v.u),
            VTR_VAL_FIXED => Value::Fixed {
                raw: v.i,
                scale: v.scale,
            },
            VTR_VAL_UFIXED => Value::UFixed {
                raw: v.u,
                scale: v.scale,
            },
            VTR_VAL_TEXT => Value::Text(
                String::from_utf8(bytes(v))
                    .map_err(|_| Error::Invalid("text value is not UTF-8".into()))?,
            ),
            _ => return Err(Error::Invalid("unsupported value tag".into())),
        })
    }
}

unsafe fn to_log_arg<'a>(v: &'a vtr_value) -> Result<LogArg<'a>, Error> {
    // SAFETY: The caller keeps the value buffers readable and unchanged for the returned borrow.
    unsafe {
        let slice = |v: &'a vtr_value| -> &'a [u8] {
            if v.data.is_null() || v.len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(v.data, v.len)
            }
        };
        Ok(match v.tag {
            VTR_VAL_BOOL => LogArg::Bool(v.b != 0),
            VTR_VAL_I64 => LogArg::I64(v.i),
            VTR_VAL_U64 => LogArg::U64(v.u),
            VTR_VAL_F64 => LogArg::F64(v.f),
            VTR_VAL_STR => LogArg::Str(StrId(v.str_id)),
            VTR_VAL_BYTES => LogArg::Bytes(slice(v)),
            VTR_VAL_TIME => LogArg::Time(v.u),
            VTR_VAL_POINTER => LogArg::Pointer(v.u),
            VTR_VAL_TEXT => LogArg::Text(
                std::str::from_utf8(slice(v))
                    .map_err(|_| Error::Invalid("text argument is not UTF-8".into()))?,
            ),
            _ => return Err(Error::Invalid("unsupported log argument tag".into())),
        })
    }
}

fn from_value(v: &Value) -> vtr_value {
    let mut o = vtr_value {
        tag: v.tag() as u8,
        ..Default::default()
    };
    match v {
        Value::Null => {}
        Value::Bool(b) => o.b = u8::from(*b),
        Value::I64(i) => o.i = *i,
        Value::U64(u) | Value::Time(u) | Value::Pointer(u) => o.u = *u,
        Value::F64(f) => o.f = *f,
        Value::Str(s) => o.str_id = s.0,
        Value::Bytes(b) => {
            o.data = b.as_ptr();
            o.len = b.len();
        }
        Value::Bits { width, data }
        | Value::Logic { width, data }
        | Value::Logic9 { width, data } => {
            o.width = *width;
            o.data = data.as_ptr();
            o.len = data.len();
        }
        Value::Enum { value, name } => {
            o.i = *value;
            o.str_id = name.0;
        }
        Value::Fixed { raw, scale } => {
            o.i = *raw;
            o.scale = *scale;
        }
        Value::UFixed { raw, scale } => {
            o.u = *raw;
            o.scale = *scale;
        }
        Value::List(l) => o.len = l.len(),
        Value::Map(m) => o.len = m.len(),
        Value::Text(s) => {
            o.data = s.as_ptr();
            o.len = s.len();
        }
    }
    o
}

unsafe fn kv_list(
    n: usize,
    keys: *const u32,
    values: *const vtr_value,
) -> Result<Vec<(StrId, Value)>, Error> {
    // SAFETY: The caller provides n readable keys and values, including each nested value buffer.
    unsafe {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let k = *keys.add(i);
            let v = to_value(&*values.add(i))?;
            out.push((StrId(k), v));
        }
        Ok(out)
    }
}

pub struct vtr_writer(Writer);

#[repr(C)]
pub struct vtr_writer_options {
    pub codec: u8,
    pub level: c_int,
    pub group_size: u32,
    pub block_records: u64,
    pub chunk_records: u64,
    pub tx_block_bytes: u64,
    pub background: c_int,
    pub dedup: c_int,
    pub checksums: c_int,
    pub log_encoders: u32,
    pub commit_interval_ms: u32,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_options_default(o: *mut vtr_writer_options) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(o) = o.as_mut() {
                let d = WriterOptions::default();
                *o = vtr_writer_options {
                    codec: d.compression.codec as u8,
                    level: d.compression.level,
                    group_size: d.group_size,
                    block_records: d.block_records as u64,
                    chunk_records: d.chunk_records as u64,
                    tx_block_bytes: d.tx_block_bytes as u64,
                    background: c_int::from(d.background),
                    dedup: c_int::from(d.dedup),
                    checksums: c_int::from(d.checksums),
                    log_encoders: d.log_encoders as u32,
                    commit_interval_ms: d.commit_interval.as_millis() as u32,
                };
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_create(
    path: *const c_char,
    opts: *const vtr_writer_options,
) -> *mut vtr_writer {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(path) = cstr(path) else {
                set_error("null path");
                return ptr::null_mut();
            };
            let mut o = WriterOptions::default();
            if let Some(c) = opts.as_ref() {
                o.compression.codec = match vtr::Codec::from_u8(c.codec) {
                    Ok(k) => k,
                    Err(e) => {
                        set_error(&e.to_string());
                        return ptr::null_mut();
                    }
                };
                o.compression.level = c.level;
                o.group_size = c.group_size;
                o.block_records = c.block_records as usize;
                o.log_encoders = c.log_encoders as usize;
                o.commit_interval =
                    std::time::Duration::from_millis(u64::from(c.commit_interval_ms));
                o.chunk_records = c.chunk_records.max(1) as usize;
                o.tx_block_bytes = c.tx_block_bytes as usize;
                o.background = c.background != 0;
                o.dedup = c.dedup != 0;
                o.checksums = c.checksums != 0;
            }
            match Writer::create_with(path, o) {
                Ok(w) => Box::into_raw(Box::new(vtr_writer(w))),
                Err(e) => {
                    set_error(&e.to_string());
                    ptr::null_mut()
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_close(w: *mut vtr_writer) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| vtr_writer_close_ending(w, ptr::null())) }
}

pub const VTR_ENDING_CLOSED: u8 = 0;
pub const VTR_ENDING_STOPPED: u8 = 1;
pub const VTR_ENDING_EXITED: u8 = 2;
pub const VTR_ENDING_CRASHED: u8 = 3;
pub const VTR_ENDING_RECOVERED: u8 = 4;
pub const VTR_ENDING_POISONED: u8 = 5;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct vtr_ending {
    pub kind: u8,
    pub signal: i32,
    pub code: i32,
    pub status: i32,
    pub address: u64,
    pub thread: u64,
    pub dropped: u64,
    pub sealed: c_int,
    pub has_time: c_int,
    pub time: u64,
}

fn from_ending(e: &vtr_ending) -> Result<Ending, Error> {
    Ok(match e.kind {
        VTR_ENDING_CLOSED => Ending::Closed,
        VTR_ENDING_STOPPED => Ending::Stopped { signal: e.signal },
        VTR_ENDING_EXITED => Ending::Exited { status: e.status },
        VTR_ENDING_CRASHED => Ending::Crashed {
            signal: e.signal,
            code: e.code,
            address: e.address,
            thread: e.thread,
            sealed: e.sealed != 0,
        },
        VTR_ENDING_RECOVERED => Ending::Recovered { dropped: e.dropped },
        VTR_ENDING_POISONED => Ending::Poisoned,
        k => return Err(Error::Invalid(format!("unknown ending kind {k}"))),
    })
}

fn to_ending(e: Ending, time: Option<u64>) -> vtr_ending {
    let mut o = vtr_ending {
        has_time: c_int::from(time.is_some()),
        time: time.unwrap_or(0),
        ..Default::default()
    };
    match e {
        Ending::Closed => o.kind = VTR_ENDING_CLOSED,
        Ending::Stopped { signal } => (o.kind, o.signal) = (VTR_ENDING_STOPPED, signal),
        Ending::Exited { status } => (o.kind, o.status) = (VTR_ENDING_EXITED, status),
        Ending::Crashed {
            signal,
            code,
            address,
            thread,
            sealed,
        } => {
            (o.kind, o.signal, o.code, o.address, o.thread, o.sealed) = (
                VTR_ENDING_CRASHED,
                signal,
                code,
                address,
                thread,
                c_int::from(sealed),
            );
        }
        Ending::Recovered { dropped } => (o.kind, o.dropped) = (VTR_ENDING_RECOVERED, dropped),
        Ending::Poisoned => o.kind = VTR_ENDING_POISONED,
    }
    o
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_recover(
    in_path: *const c_char,
    out_path: *const c_char,
    dropped_out: *mut i64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let input = need_str!(in_path);
            let output = need_str!(out_path);
            match vtr::recover(input, output) {
                Ok(d) => {
                    if let Some(o) = dropped_out.as_mut() {
                        *o = d.map_or(-1, |d| d as i64);
                    }
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_enter(w: *mut vtr_writer) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { w.as_ref().map_or(0, |w| w.0.state().enter()) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_leave(w: *mut vtr_writer, prev: u32) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        if let Some(w) = w.as_ref() {
            w.0.state().leave(prev);
        }
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_close_ending(
    w: *mut vtr_writer,
    end: *const vtr_ending,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if w.is_null() {
                return VTR_ERR_NULL;
            }
            let mut b = Box::from_raw(w);
            let sealer = b.0.sealer();
            let prev = sealer.state().enter();
            let r = match end.as_ref().map(from_ending).transpose() {
                Ok(e) => b.0.close_with(e.unwrap_or(Ending::Closed)),
                Err(e) => {
                    drop(b.0.close());
                    Err(e)
                }
            };
            drop(b);
            sealer.state().leave(prev);
            status(r)
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_ending(r: *const vtr_reader, out: *mut vtr_ending) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out = need!(out);
            match r.0.ending() {
                Ok((e, t)) => {
                    *out = to_ending(e, t);
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_ending_format(
    end: *const vtr_ending,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let s = match end.as_ref().map(from_ending) {
                Some(Ok(e)) => e.to_string(),
                _ => return 0,
            };
            if cap > 0 && !buf.is_null() {
                let m = s.len().min(cap - 1);
                ptr::copy_nonoverlapping(s.as_ptr(), buf.cast::<u8>(), m);
                *buf.add(m) = 0;
            }
            s.len()
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_timescale(w: *mut vtr_writer, exp: i8) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            status(need!(w).0.set_timescale(vtr::Timescale::Exponent(exp)))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_time_zero(w: *mut vtr_writer, t: i64) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.set_time_zero(t))) }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_file_type(w: *mut vtr_writer, ft: u8) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            status(need!(w).0.set_file_type(vtr::FileType::from_u8(ft)))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_writer_name(w: *mut vtr_writer, s: *const c_char) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let s = need_str!(s);
            status(need!(w).0.set_writer_name(s))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_date(w: *mut vtr_writer, s: *const c_char) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let s = need_str!(s);
            status(need!(w).0.set_date(s))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_comment(w: *mut vtr_writer, s: *const c_char) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let s = need_str!(s);
            status(need!(w).0.set_comment(s))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_file_attr(
    w: *mut vtr_writer,
    key: *const c_char,
    v: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let key = need_str!(key);
            let v = need_ref!(v);
            let val = match to_value(v) {
                Ok(v) => v,
                Err(e) => return status(Err(e)),
            };
            status(need!(w).0.set_file_attr(key, val))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_intern(w: *mut vtr_writer, s: *const c_char) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || match (w.as_mut(), cstr(s)) {
            (Some(w), Some(s)) => w.0.intern(s).0,
            _ => 0,
        })
    }
}

fn node_or_none(r: vtr::Result<NodeId>) -> u32 {
    match r {
        Ok(n) => n.0,
        Err(e) => {
            set_error(&e.to_string());
            VTR_NONE
        }
    }
}

macro_rules! decl_args {
    ($w:expr, $name:expr) => {
        match ($w.as_mut(), cstr($name)) {
            (Some(w), Some(n)) => (w, n),
            (None, _) => {
                set_error("null handle");
                return VTR_NONE;
            }
            (_, None) => {
                set_error("null or non-UTF-8 string");
                return VTR_NONE;
            }
        }
    };
}

fn parent_id(parent: u32) -> Option<NodeId> {
    (parent != VTR_NONE).then_some(NodeId(parent))
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_scope(
    w: *mut vtr_writer,
    parent: u32,
    name: *const c_char,
    scope_type: u16,
    component: *const c_char,
) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let (w, name) = decl_args!(w, name);
            node_or_none(w.0.add_scope(
                parent_id(parent),
                name,
                ScopeType::from_code(scope_type),
                cstr(component).unwrap_or(""),
            ))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_var(
    w: *mut vtr_writer,
    parent: u32,
    name: *const c_char,
    var_type: u16,
    direction: u8,
    kind: u8,
    width: u32,
    states: u8,
    node_out: *mut u32,
    signal_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let name = need_str!(name);
            let w = need!(w);
            let k = match kind {
                0 => {
                    if !matches!(states, 2 | 4 | 9) {
                        set_error("states must be 2, 4 or 9");
                        return VTR_ERR_INVALID;
                    }
                    SignalKind::Bits {
                        width: width.max(1),
                        states,
                    }
                }
                1 => SignalKind::Real,
                2 => SignalKind::VarLen,
                _ => {
                    set_error("kind must be 0, 1 or 2");
                    return VTR_ERR_INVALID;
                }
            };
            let (n, s) = match w.0.add_var(
                parent_id(parent),
                name,
                VarType::from_code(var_type),
                Direction::from_u8(direction),
                k,
            ) {
                Ok(x) => x,
                Err(e) => return status(Err(e)),
            };
            if let Some(o) = node_out.as_mut() {
                *o = n.0;
            }
            if let Some(o) = signal_out.as_mut() {
                *o = s.0;
            }
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_alias(
    w: *mut vtr_writer,
    parent: u32,
    name: *const c_char,
    var_type: u16,
    direction: u8,
    signal: u32,
) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let (w, name) = decl_args!(w, name);
            node_or_none(w.0.add_alias(
                parent_id(parent),
                name,
                VarType::from_code(var_type),
                Direction::from_u8(direction),
                SignalId(signal),
            ))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_enum_table(
    w: *mut vtr_writer,
    parent: u32,
    name: *const c_char,
    n: usize,
    literals: *const *const c_char,
    values: *const *const c_char,
) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let (w, name) = decl_args!(w, name);
            let mut entries = Vec::with_capacity(n);
            for i in 0..n {
                let l = cstr(*literals.add(i)).unwrap_or("");
                let v = cstr(*values.add(i)).unwrap_or("");
                entries.push((l, v));
            }
            node_or_none(w.0.add_enum_table(parent_id(parent), name, &entries))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_generator(
    w: *mut vtr_writer,
    parent: u32,
    name: *const c_char,
    kind: *const c_char,
) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let (w, name) = decl_args!(w, name);
            node_or_none(w.0.add_generator(parent_id(parent), name, cstr(kind).unwrap_or("")))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_node_attr(
    w: *mut vtr_writer,
    node: u32,
    key: *const c_char,
    v: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let key = need_str!(key);
            let v = need_ref!(v);
            let val = match to_value(v) {
                Ok(v) => v,
                Err(e) => return status(Err(e)),
            };
            status(need!(w).0.node_attr(NodeId(node), key, val))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_time(w: *mut vtr_writer, t: u64) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.set_time(t))) }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_dump_off(w: *mut vtr_writer) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            need!(w).0.dump_off();
            VTR_OK
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_dump_on(w: *mut vtr_writer) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            need!(w).0.dump_on();
            VTR_OK
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_bit(w: *mut vtr_writer, sig: u32, code: u8) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let value = match code {
                0 => vtr::Logic::Zero,
                1 => vtr::Logic::One,
                2 => vtr::Logic::X,
                3 => vtr::Logic::Z,
                4 => vtr::Logic::U,
                5 => vtr::Logic::W,
                6 => vtr::Logic::L,
                7 => vtr::Logic::H,
                8 => vtr::Logic::DontCare,
                _ => return status(Err(Error::Invalid("unknown logic code".into()))),
            };
            status(need!(w).0.emit_bit(SignalId(sig), value))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_u64(w: *mut vtr_writer, sig: u32, value: u64) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.emit_u64(SignalId(sig), value))) }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_words(
    w: *mut vtr_writer,
    sig: u32,
    words: *const u32,
    n: usize,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let ws = if words.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(words, n)
            };
            status(need!(w).0.emit_words(SignalId(sig), ws))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_logic_str(
    w: *mut vtr_writer,
    sig: u32,
    s: *const c_char,
    len: usize,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            if s.is_null() {
                set_error("null value");
                return VTR_ERR_NULL;
            }
            let bytes = if len == usize::MAX {
                CStr::from_ptr(s).to_bytes()
            } else {
                std::slice::from_raw_parts(s.cast::<u8>(), len)
            };
            status(need!(w).0.emit_logic_str(SignalId(sig), bytes))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_packed(
    w: *mut vtr_writer,
    sig: u32,
    states: u8,
    data: *const u8,
    len: usize,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let d = if data.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(data, len)
            };
            status(need!(w).0.emit_packed(SignalId(sig), states, d))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_real(w: *mut vtr_writer, sig: u32, v: f64) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.emit_real(SignalId(sig), v))) }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_emit_varlen(
    w: *mut vtr_writer,
    sig: u32,
    data: *const u8,
    len: usize,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let d = if data.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(data, len)
            };
            status(need!(w).0.emit_varlen(SignalId(sig), d))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_flush(w: *mut vtr_writer) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.flush())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_begin_tx(
    w: *mut vtr_writer,
    generator: u32,
    time: u64,
    tx_out: *mut u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || match need!(w).0.begin_tx(NodeId(generator), time) {
            Ok(id) => {
                if let Some(o) = tx_out.as_mut() {
                    *o = id;
                }
                VTR_OK
            }
            Err(e) => status(Err(e)),
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_tx_parent(
    w: *mut vtr_writer,
    tx: u64,
    parent: u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.set_tx_parent(tx, parent))) }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_set_tx_kind(w: *mut vtr_writer, tx: u64, kind: u8) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            status(need!(w).0.set_tx_kind(tx, TxKind::from_u8(kind)))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_tx_attr(
    w: *mut vtr_writer,
    tx: u64,
    key: u32,
    v: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let v = need_ref!(v);
            let val = match to_value(v) {
                Ok(v) => v,
                Err(e) => return status(Err(e)),
            };
            status(need!(w).0.tx_attr(tx, StrId(key), &val))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_tx_event(
    w: *mut vtr_writer,
    tx: u64,
    time: u64,
    name: u32,
    n: usize,
    keys: *const u32,
    values: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let attrs = match kv_list(n, keys, values) {
                Ok(a) => a,
                Err(e) => return status(Err(e)),
            };
            status(need!(w).0.tx_event(tx, time, StrId(name), &attrs))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_tx_stage_begin(
    w: *mut vtr_writer,
    tx: u64,
    name: u32,
    lane: u32,
    time: u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            status(
                need!(w)
                    .0
                    .tx_stage_begin(tx, StrId(name), StrId(lane), time),
            )
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_tx_stage_end(
    w: *mut vtr_writer,
    tx: u64,
    name: u32,
    lane: u32,
    time: u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            match need!(w).0.tx_stage_end(tx, StrId(name), StrId(lane), time) {
                Ok(true) => VTR_OK,
                Ok(false) => VTR_ERR_NOT_FOUND,
                Err(e) => status(Err(e)),
            }
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_tx_stage(
    w: *mut vtr_writer,
    tx: u64,
    name: u32,
    lane: u32,
    begin: u64,
    end: u64,
    n: usize,
    keys: *const u32,
    values: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let attrs = match kv_list(n, keys, values) {
                Ok(a) => a,
                Err(e) => return status(Err(e)),
            };
            status(
                need!(w)
                    .0
                    .tx_stage(tx, StrId(name), StrId(lane), begin, end, &attrs),
            )
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_tx_stage_attr(
    w: *mut vtr_writer,
    tx: u64,
    key: u32,
    v: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let v = need_ref!(v);
            let val = match to_value(v) {
                Ok(v) => v,
                Err(e) => return status(Err(e)),
            };
            status(need!(w).0.tx_stage_attr(tx, StrId(key), &val))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_end_tx(
    w: *mut vtr_writer,
    tx: u64,
    time: u64,
    status_code: u8,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            status(need!(w).0.end_tx(tx, time, TxStatus::from_u8(status_code)))
        })
    }
}
/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_relate(
    w: *mut vtr_writer,
    kind: u32,
    from: u64,
    to: u64,
    n: usize,
    keys: *const u32,
    values: *const vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let attrs = match kv_list(n, keys, values) {
                Ok(a) => a,
                Err(e) => return status(Err(e)),
            };
            status(need!(w).0.relate(StrId(kind), from, to, &attrs))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_log_site(
    w: *mut vtr_writer,
    scope: u32,
    severity: u8,
    fmt: *const c_char,
    file: *const c_char,
    line: u32,
    func: *const c_char,
    n_args: usize,
    arg_types: *const u8,
    names: *const *const c_char,
) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let Some(w) = w.as_mut() else {
                set_error("null handle");
                return VTR_NONE;
            };
            let Some(fmt) = cstr(fmt) else {
                set_error("null or non-UTF-8 format string");
                return VTR_NONE;
            };
            if n_args > 0 && arg_types.is_null() {
                set_error("null arg_types");
                return VTR_NONE;
            }
            let mut types = Vec::with_capacity(n_args);
            for i in 0..n_args {
                if let Ok(t) = LogArgType::from_u8(*arg_types.add(i)) {
                    types.push(t);
                } else {
                    set_error("unsupported log argument type");
                    return VTR_NONE;
                }
            }
            let mut name_strs: Vec<&str> = Vec::new();
            if !names.is_null() {
                for i in 0..n_args {
                    match cstr(*names.add(i)) {
                        Some(s) => name_strs.push(s),
                        None => break,
                    }
                }
            }
            let mut spec =
                LogSiteSpec::new(parent_id(scope), Severity::from_code(severity), fmt, &types)
                    .names(&name_strs);
            spec.file = cstr(file).unwrap_or("");
            spec.line = line;
            spec.func = cstr(func).unwrap_or("");
            match w.0.add_log_site(&spec) {
                Ok(site) => site.0,
                Err(error) => {
                    set_error(&error.to_string());
                    VTR_NONE
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_log_site_node(w: *const vtr_writer, site: u32) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || match w.as_ref() {
            Some(w) => w.0.log_site_node(LogSiteId(site)).map_or(VTR_NONE, |n| n.0),
            None => VTR_NONE,
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_log(
    w: *mut vtr_writer,
    site: u32,
    time: u64,
    parent: u64,
    n: usize,
    args: *const vtr_value,
    id_out: *mut u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let w = need!(w);
            if n > 0 && args.is_null() {
                set_error("null args");
                return VTR_ERR_NULL;
            }
            let mut buf = [LogArg::Bool(false); 16];
            let mut heap: Vec<LogArg> = Vec::new();
            let list: &[LogArg] = if n <= 16 {
                for (i, slot) in buf.iter_mut().enumerate().take(n) {
                    *slot = match to_log_arg(&*args.add(i)) {
                        Ok(a) => a,
                        Err(e) => return status(Err(e)),
                    };
                }
                &buf[..n]
            } else {
                heap.reserve(n);
                for i in 0..n {
                    match to_log_arg(&*args.add(i)) {
                        Ok(a) => heap.push(a),
                        Err(e) => return status(Err(e)),
                    }
                }
                &heap
            };
            let parent = if parent == 0 { None } else { Some(parent) };
            match w.0.log_with_parent(LogSiteId(site), time, parent, list) {
                Ok(id) => {
                    if let Some(o) = id_out.as_mut() {
                        *o = id;
                    }
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_log_raw(
    w: *mut vtr_writer,
    site: u32,
    time: u64,
    parent: u64,
    args: *const u8,
    len: usize,
    id_out: *mut u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let w = need!(w);
            let bytes: &[u8] = if len == 0 {
                &[]
            } else if args.is_null() {
                set_error("null args");
                return VTR_ERR_NULL;
            } else {
                std::slice::from_raw_parts(args, len)
            };
            let parent = if parent == 0 { None } else { Some(parent) };
            match w.0.log_raw(LogSiteId(site), time, parent, bytes) {
                Ok(id) => {
                    if let Some(o) = id_out.as_mut() {
                        *o = id;
                    }
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_add_clock(
    w: *mut vtr_writer,
    scope: u32,
    name: *const c_char,
) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            let (w, name) = decl_args!(w, name);
            match w.0.add_clock(parent_id(scope), name) {
                Ok(c) => c.0,
                Err(e) => {
                    set_error(&e.to_string());
                    VTR_NONE
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_clock_generator(w: *const vtr_writer, clock: u32) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || match w.as_ref() {
            Some(w) => {
                w.0.clock_generator(ClockId(clock))
                    .map_or(VTR_NONE, |n| n.0)
            }
            None => VTR_NONE,
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_clock_run(
    w: *mut vtr_writer,
    clock: u32,
    first: u64,
    period: u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi_w(w, || {
            status(need!(w).0.clock_run(ClockId(clock), first, period))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_writer_clock_stop(w: *mut vtr_writer, clock: u32, t: u64) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi_w(w, || status(need!(w).0.clock_stop(ClockId(clock), t))) }
}

pub struct vtr_reader(Reader);

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_open(path: *const c_char) -> *mut vtr_reader {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(path) = cstr(path) else {
                set_error("null path");
                return ptr::null_mut();
            };
            match Reader::open(path) {
                Ok(r) => Box::into_raw(Box::new(vtr_reader(r))),
                Err(e) => {
                    set_error(&e.to_string());
                    ptr::null_mut()
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_close(r: *mut vtr_reader) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !r.is_null() {
                drop(Box::from_raw(r));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_clear_cache(r: *mut vtr_reader) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(r) = r.as_mut() {
                r.0.clear_cache();
            }
        });
    }
}

#[repr(C)]
pub struct vtr_meta {
    pub timescale: i8,
    pub time_zero: i64,
    pub file_type: u8,
    pub group_size: u32,
    pub has_time_range: c_int,
    pub time_start: u64,
    pub time_end: u64,
    pub version_major: u16,
    pub version_minor: u16,
    pub recovered: c_int,
    pub recovered_dropped: u64,
    pub signal_count: u32,
    pub node_count: u32,
    pub string_count: u32,
    pub signal_block_count: u32,
    pub tx_block_count: u32,
    pub tx_count: u64,
    pub relation_count: u64,
    pub blackout_count: u32,
    pub file_attr_count: u32,
    pub log_block_count: u32,
    pub log_count: u64,
    pub log_site_count: u32,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_meta(r: *const vtr_reader, out: *mut vtr_meta) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out = need!(out);
            let m = r.0.meta();
            let tr = r.0.time_range();
            let (ntx, nrel) = r.0.tx_counts();
            *out = vtr_meta {
                timescale: m.timescale,
                time_zero: m.time_zero,
                file_type: m.file_type as u8,
                group_size: m.group_size,
                has_time_range: c_int::from(tr.is_some()),
                time_start: tr.map_or(0, |x| x.0),
                time_end: tr.map_or(0, |x| x.1),
                version_major: r.0.version().0,
                version_minor: r.0.version().1,
                recovered: c_int::from(r.0.recovered().is_some()),
                recovered_dropped: r.0.recovered().unwrap_or(0),
                signal_count: r.0.signal_count(),
                node_count: r.0.hierarchy().len() as u32,
                string_count: r.0.strings().len() as u32,
                signal_block_count: r.0.block_count() as u32,
                tx_block_count: r.0.tx_block_count() as u32,
                tx_count: ntx,
                relation_count: nrel,
                blackout_count: r.0.blackout().len() as u32,
                file_attr_count: m.attrs.len() as u32,
                log_block_count: r.0.log_block_count() as u32,
                log_count: r.0.log_count(),
                log_site_count: r.0.log_sites().len() as u32,
            };
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_meta_string(
    r: *const vtr_reader,
    which: c_int,
    len_out: *mut usize,
) -> *const c_char {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                return ptr::null();
            };
            let m = r.0.meta();
            let s = match which {
                0 => &m.writer,
                1 => &m.date,
                _ => &m.comment,
            };
            if let Some(l) = len_out.as_mut() {
                *l = s.len();
            }
            s.as_ptr().cast::<c_char>()
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_file_attr(
    r: *const vtr_reader,
    i: u32,
    key_out: *mut u32,
    v_out: *mut vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            match r.0.meta().attrs.get(i as usize) {
                Some((k, v)) => {
                    if let Some(o) = key_out.as_mut() {
                        *o = k.0;
                    }
                    if let Some(o) = v_out.as_mut() {
                        *o = from_value(v);
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_str(
    r: *const vtr_reader,
    id: u32,
    len_out: *mut usize,
) -> *const c_char {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                return ptr::null();
            };
            let s = r.0.str(StrId(id));
            if let Some(l) = len_out.as_mut() {
                *l = s.len();
            }
            s.as_ptr().cast::<c_char>()
        })
    }
}

#[repr(C)]
pub struct vtr_node_info {
    pub kind: u8,
    pub parent: u32,
    pub name: u32,
    pub type_code: u16,
    pub direction: u8,
    pub signal: u32,
    pub is_alias: c_int,
    pub aux_str: u32,
    pub attr_count: u32,
    pub entry_count: u32,
    pub child_count: u32,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_node_count(r: *const vtr_reader) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| r.as_ref().map_or(0, |r| r.0.hierarchy().len() as u32)) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_node(
    r: *const vtr_reader,
    id: u32,
    out: *mut vtr_node_info,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out = need!(out);
            let h = r.0.hierarchy();
            if id as usize >= h.len() {
                return VTR_ERR_NOT_FOUND;
            }
            let n = h.node(NodeId(id));
            let mut info = vtr_node_info {
                kind: n.kind() as u8,
                parent: n.parent.map_or(VTR_NONE, |p| p.0),
                name: n.name.0,
                type_code: 0,
                direction: 0,
                signal: VTR_NONE,
                is_alias: 0,
                aux_str: 0,
                attr_count: n.attrs.len() as u32,
                entry_count: 0,
                child_count: h.children(NodeId(id)).count() as u32,
            };
            match &n.data {
                NodeData::Scope {
                    scope_type,
                    component,
                } => {
                    info.type_code = scope_type.code();
                    info.aux_str = component.0;
                }
                NodeData::Var {
                    var_type,
                    direction,
                    signal,
                    declares,
                } => {
                    info.type_code = var_type.code();
                    info.direction = *direction as u8;
                    info.signal = signal.0;
                    info.is_alias = c_int::from(declares.is_none());
                }
                NodeData::Generator { kind } => info.aux_str = kind.0,
                NodeData::EnumTable { entries } => info.entry_count = entries.len() as u32,
            }
            *out = info;
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_node_attr(
    r: *const vtr_reader,
    id: u32,
    i: u32,
    key_out: *mut u32,
    v_out: *mut vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let h = r.0.hierarchy();
            if id as usize >= h.len() {
                return VTR_ERR_NOT_FOUND;
            }
            match h.attrs(NodeId(id)).get(i as usize) {
                Some((k, v)) => {
                    if let Some(o) = key_out.as_mut() {
                        *o = k.0;
                    }
                    if let Some(o) = v_out.as_mut() {
                        *o = from_value(v);
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_enum_entry(
    r: *const vtr_reader,
    id: u32,
    i: u32,
    literal_out: *mut u32,
    value_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let h = r.0.hierarchy();
            if id as usize >= h.len() {
                return VTR_ERR_NOT_FOUND;
            }
            match h.enum_entries(NodeId(id)) {
                Some(entries) => match entries.get(i as usize) {
                    Some((l, v)) => {
                        if let Some(o) = literal_out.as_mut() {
                            *o = l.0;
                        }
                        if let Some(o) = value_out.as_mut() {
                            *o = v.0;
                        }
                        VTR_OK
                    }
                    None => VTR_ERR_NOT_FOUND,
                },
                None => VTR_ERR_INVALID,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_children(
    r: *const vtr_reader,
    id: u32,
    out: *mut u32,
    cap: usize,
) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else { return 0 };
            let h = r.0.hierarchy();
            let mut n = 0usize;
            let mut put = |c: NodeId| {
                if n < cap && !out.is_null() {
                    *out.add(n) = c.0;
                }
                n += 1;
            };
            if id == VTR_NONE {
                for c in h.roots() {
                    put(c);
                }
            } else if (id as usize) < h.len() {
                for c in h.children(NodeId(id)) {
                    put(c);
                }
            }
            n
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_signal_count(r: *const vtr_reader) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| r.as_ref().map_or(0, |r| r.0.signal_count())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_signal_kind(
    r: *const vtr_reader,
    sig: u32,
    kind_out: *mut u8,
    width_out: *mut u32,
    states_out: *mut u8,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            match r.0.hierarchy().signal_kind(SignalId(sig)) {
                Some(k) => {
                    let (kind, width, states) = match k {
                        SignalKind::Bits { width, states } => (0u8, width, states),
                        SignalKind::Real => (1, 64, 0),
                        SignalKind::VarLen => (2, 0, 0),
                    };
                    if let Some(o) = kind_out.as_mut() {
                        *o = kind;
                    }
                    if let Some(o) = width_out.as_mut() {
                        *o = width;
                    }
                    if let Some(o) = states_out.as_mut() {
                        *o = states;
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_signal_var_type(
    r: *const vtr_reader,
    sig: u32,
    type_out: *mut u16,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out = need!(type_out);
            match r.0.hierarchy().signal_var_type(SignalId(sig)) {
                Some(var_type) => {
                    *out = var_type.code();
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_signal_var(r: *const vtr_reader, sig: u32) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            r.as_ref()
                .and_then(|r| r.0.hierarchy().signal_var.get(sig as usize).map(|n| n.0))
                .unwrap_or(VTR_NONE)
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_find_signal(
    r: *const vtr_reader,
    path: *const c_char,
    sep: c_char,
    sig_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let path = need_str!(path);
            match r.0.find_signal(path, sep as u8 as char) {
                Some(s) => {
                    if let Some(o) = sig_out.as_mut() {
                        *o = s.0;
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_find_node(
    r: *const vtr_reader,
    path: *const c_char,
    sep: c_char,
    node_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let path = need_str!(path);
            let parts: Vec<&str> = path.split(sep as u8 as char).collect();
            match r.0.find_node(&parts) {
                Some(n) => {
                    if let Some(o) = node_out.as_mut() {
                        *o = n.0;
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct vtr_signal_value {
    pub kind: u8,
    pub states: u8,
    pub width: u32,
    pub real: f64,
    pub data: *const u8,
    pub len: usize,
}

fn sv_from(v: vtr::SignalValue<'_>) -> vtr_signal_value {
    match v {
        vtr::SignalValue::Bits {
            width,
            states,
            data,
        } => vtr_signal_value {
            kind: 0,
            states,
            width,
            real: 0.0,
            data: data.as_ptr(),
            len: data.len(),
        },
        vtr::SignalValue::Real(f) => vtr_signal_value {
            kind: 1,
            states: 0,
            width: 64,
            real: f,
            data: ptr::null(),
            len: 0,
        },
        vtr::SignalValue::VarLen(b) => vtr_signal_value {
            kind: 2,
            states: 0,
            width: 0,
            real: 0.0,
            data: b.as_ptr(),
            len: b.len(),
        },
    }
}

pub struct vtr_value_buf {
    v: vtr::OwnedSignalValue,
    ascii: Vec<u8>,
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_value_buf_new() -> *mut vtr_value_buf {
    ffi(|| {
        Box::into_raw(Box::new(vtr_value_buf {
            v: vtr::OwnedSignalValue::Real(0.0),
            ascii: Vec::new(),
        }))
    })
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_value_buf_free(b: *mut vtr_value_buf) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !b.is_null() {
                drop(Box::from_raw(b));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_value_buf_get(
    b: *const vtr_value_buf,
    out: *mut vtr_signal_value,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let b = need_ref!(b);
            let out = need!(out);
            *out = sv_from(b.v.borrow());
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_value_buf_ascii(b: *mut vtr_value_buf) -> *const c_char {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(b) = b.as_mut() else {
                return ptr::null();
            };
            b.ascii.clear();
            b.ascii.extend_from_slice(b.v.to_ascii().as_bytes());
            b.ascii.push(0);
            b.ascii.as_ptr().cast::<c_char>()
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_value_at(
    r: *const vtr_reader,
    sig: u32,
    time: u64,
    buf: *mut vtr_value_buf,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let b = need!(buf);
            match r.0.value_at(SignalId(sig), time) {
                Ok(v) => {
                    b.v = v;
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

pub type vtr_change_cb = Option<
    unsafe extern "C" fn(
        user: *mut std::ffi::c_void,
        time: u64,
        sig: u32,
        value: *const vtr_signal_value,
    ) -> c_int,
>;

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_changes(
    r: *const vtr_reader,
    sig: u32,
    t0: u64,
    t1: u64,
    cb: vtr_change_cb,
    user: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let Some(cb) = cb else { return VTR_ERR_NULL };
            match r.0.changes(SignalId(sig), t0, t1) {
                Ok(list) => {
                    for (t, v) in &list {
                        let sv = sv_from(v.borrow());
                        if cb(user, *t, sig, &raw const sv) != 0 {
                            break;
                        }
                    }
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_for_each_change(
    r: *const vtr_reader,
    t0: u64,
    t1: u64,
    cb: vtr_change_cb,
    user: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let Some(cb) = cb else { return VTR_ERR_NULL };
            let mut stop = false;
            let res = r.0.for_each_change(t0, t1, |t, s, v| {
                if stop {
                    return;
                }
                let sv = sv_from(v);
                if cb(user, t, s.0, &raw const sv) != 0 {
                    stop = true;
                }
            });
            status(res)
        })
    }
}

pub struct vtr_signal_data(vtr::SignalData);

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_load_signal(
    r: *const vtr_reader,
    sig: u32,
) -> *mut vtr_signal_data {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                return ptr::null_mut();
            };
            match r.0.load_signal(SignalId(sig)) {
                Ok(d) => Box::into_raw(Box::new(vtr_signal_data(d))),
                Err(e) => {
                    set_error(&e.to_string());
                    ptr::null_mut()
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_load_signals(
    r: *const vtr_reader,
    sigs: *const u32,
    n: usize,
    out: *mut *mut vtr_signal_data,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            if n == 0 {
                return VTR_OK;
            }
            if sigs.is_null() || out.is_null() {
                return VTR_ERR_NULL;
            }
            let sigs: Vec<_> = std::slice::from_raw_parts(sigs, n)
                .iter()
                .map(|&s| SignalId(s))
                .collect();
            match r.0.load_signals(&sigs) {
                Ok(loaded) => {
                    for (i, d) in loaded.into_iter().enumerate() {
                        out.add(i)
                            .write(Box::into_raw(Box::new(vtr_signal_data(d))));
                    }
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_clone(d: *const vtr_signal_data) -> *mut vtr_signal_data {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            d.as_ref().map_or(ptr::null_mut(), |d| {
                Box::into_raw(Box::new(vtr_signal_data(d.0.clone())))
            })
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_free(d: *mut vtr_signal_data) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !d.is_null() {
                drop(Box::from_raw(d));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_len(d: *const vtr_signal_data) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| d.as_ref().map_or(0, |d| d.0.len())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_times(d: *const vtr_signal_data) -> *const u64 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| d.as_ref().map_or(ptr::null(), |d| d.0.times().as_ptr())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_get(
    d: *const vtr_signal_data,
    i: usize,
    out: *mut vtr_signal_value,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let d = need_ref!(d);
            let out = need!(out);
            if i >= d.0.len() {
                return VTR_ERR_NOT_FOUND;
            }
            *out = sv_from(d.0.get(i));
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_initial(
    d: *const vtr_signal_data,
    out: *mut vtr_signal_value,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let d = need_ref!(d);
            let out = need!(out);
            *out = sv_from(d.0.initial());
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_signal_data_index_at(d: *const vtr_signal_data, time: u64) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            d.as_ref()
                .and_then(|d| d.0.index_at(time))
                .unwrap_or(usize::MAX)
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_time_table(
    r: *const vtr_reader,
    len_out: *mut usize,
) -> *const u64 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                return ptr::null();
            };
            match r.0.time_table() {
                Ok(t) => {
                    if let Some(l) = len_out.as_mut() {
                        *l = t.len();
                    }
                    t.as_ptr()
                }
                Err(e) => {
                    set_error(&e.to_string());
                    ptr::null()
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_blackout(
    r: *const vtr_reader,
    i: u32,
    time_out: *mut u64,
    active_out: *mut c_int,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            match r.0.blackout().get(i as usize) {
                Some(b) => {
                    if let Some(o) = time_out.as_mut() {
                        *o = b.time;
                    }
                    if let Some(o) = active_out.as_mut() {
                        *o = c_int::from(b.active);
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

pub struct vtr_tx(Transaction);

#[repr(C)]
pub struct vtr_tx_info {
    pub id: u64,
    pub generator: u32,
    pub scope: u32,
    pub begin: u64,
    pub end: u64,
    pub status: u8,
    pub kind: u8,
    pub has_parent: c_int,
    pub parent: u64,
    pub attr_count: u32,
    pub event_count: u32,
    pub stage_count: u32,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_tx_get(
    r: *const vtr_reader,
    tx: *const vtr_tx,
    out: *mut vtr_tx_info,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let t = &need_ref!(tx).0;
            let out = need!(out);
            *out = vtr_tx_info {
                id: t.id,
                generator: t.generator.0,
                scope: r
                    .0
                    .hierarchy()
                    .parent(t.generator)
                    .map_or(VTR_NONE, |s| s.0),
                begin: t.begin,
                end: t.end,
                status: t.status as u8,
                kind: t.kind as u8,
                has_parent: c_int::from(t.parent.is_some()),
                parent: t.parent.unwrap_or(0),
                attr_count: t.attrs.len() as u32,
                event_count: t.events.len() as u32,
                stage_count: t.stages.len() as u32,
            };
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_tx_attr(
    tx: *const vtr_tx,
    i: u32,
    key_out: *mut u32,
    v_out: *mut vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let t = &need_ref!(tx).0;
            match t.attrs.get(i as usize) {
                Some(a) => {
                    if let Some(o) = key_out.as_mut() {
                        *o = a.key.0;
                    }
                    if let Some(o) = v_out.as_mut() {
                        *o = from_value(&a.value);
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_tx_event(
    tx: *const vtr_tx,
    i: u32,
    time_out: *mut u64,
    name_out: *mut u32,
    attr_count_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let t = &need_ref!(tx).0;
            match t.events.get(i as usize) {
                Some(e) => {
                    if let Some(o) = time_out.as_mut() {
                        *o = e.time;
                    }
                    if let Some(o) = name_out.as_mut() {
                        *o = e.name.0;
                    }
                    if let Some(o) = attr_count_out.as_mut() {
                        *o = e.attrs.len() as u32;
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_tx_event_attr(
    tx: *const vtr_tx,
    i: u32,
    j: u32,
    key_out: *mut u32,
    v_out: *mut vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let t = &need_ref!(tx).0;
            match t
                .events
                .get(i as usize)
                .and_then(|e| e.attrs.get(j as usize))
            {
                Some((k, v)) => {
                    if let Some(o) = key_out.as_mut() {
                        *o = k.0;
                    }
                    if let Some(o) = v_out.as_mut() {
                        *o = from_value(v);
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_tx_stage(
    tx: *const vtr_tx,
    i: u32,
    name_out: *mut u32,
    lane_out: *mut u32,
    begin_out: *mut u64,
    end_out: *mut u64,
    has_end_out: *mut c_int,
    attr_count_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let t = &need_ref!(tx).0;
            match t.stages.get(i as usize) {
                Some(s) => {
                    if let Some(o) = name_out.as_mut() {
                        *o = s.name.0;
                    }
                    if let Some(o) = lane_out.as_mut() {
                        *o = s.lane.0;
                    }
                    if let Some(o) = begin_out.as_mut() {
                        *o = s.begin;
                    }
                    if let Some(o) = end_out.as_mut() {
                        *o = s.end.unwrap_or(t.end);
                    }
                    if let Some(o) = has_end_out.as_mut() {
                        *o = c_int::from(s.end.is_some());
                    }
                    if let Some(o) = attr_count_out.as_mut() {
                        *o = s.attrs.len() as u32;
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_tx_stage_attr(
    tx: *const vtr_tx,
    i: u32,
    j: u32,
    key_out: *mut u32,
    v_out: *mut vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let t = &need_ref!(tx).0;
            match t
                .stages
                .get(i as usize)
                .and_then(|s| s.attrs.get(j as usize))
            {
                Some((k, v)) => {
                    if let Some(o) = key_out.as_mut() {
                        *o = k.0;
                    }
                    if let Some(o) = v_out.as_mut() {
                        *o = from_value(v);
                    }
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

pub type vtr_tx_cb =
    Option<unsafe extern "C" fn(user: *mut std::ffi::c_void, tx: *const vtr_tx) -> c_int>;

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_visit_transactions(
    r: *const vtr_reader,
    generator: u32,
    scope: u32,
    t0: u64,
    t1: u64,
    cb: vtr_tx_cb,
    user: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let Some(cb) = cb else { return VTR_ERR_NULL };
            let q = TxQuery {
                generator: if generator == VTR_NONE {
                    None
                } else {
                    Some(NodeId(generator))
                },
                scope: if scope == VTR_NONE {
                    None
                } else {
                    Some(NodeId(scope))
                },
                window: if t1 == 0 { None } else { Some((t0, t1)) },
            };
            let res = r.0.visit_transactions(&q, |t| {
                let h = vtr_tx(t.clone());
                cb(user, &raw const h) == 0
            });
            status(res)
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_transaction(
    r: *const vtr_reader,
    id: u64,
    cb: vtr_tx_cb,
    user: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let Some(cb) = cb else { return VTR_ERR_NULL };
            match r.0.transaction(id) {
                Ok(Some(t)) => {
                    let h = vtr_tx(t);
                    cb(user, &raw const h);
                    VTR_OK
                }
                Ok(None) => VTR_ERR_NOT_FOUND,
                Err(e) => status(Err(e)),
            }
        })
    }
}

pub type vtr_relation_cb = Option<
    unsafe extern "C" fn(
        user: *mut std::ffi::c_void,
        kind: u32,
        from: u64,
        to: u64,
        n_attrs: u32,
        keys: *const u32,
        values: *const vtr_value,
    ) -> c_int,
>;

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_transaction_generator(
    r: *const vtr_reader,
    id: u64,
    out_generator: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            if out_generator.is_null() {
                return VTR_ERR_NULL;
            }
            match r.0.transaction_generator(id) {
                Ok(Some(generator)) => {
                    *out_generator = generator.0;
                    VTR_OK
                }
                Ok(None) => VTR_ERR_NOT_FOUND,
                Err(error) => status(Err(error)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_transaction_generators(
    r: *const vtr_reader,
    ids: *const u64,
    len: usize,
    out_generators: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            if len == 0 {
                return VTR_OK;
            }
            if ids.is_null() || out_generators.is_null() {
                return VTR_ERR_NULL;
            }
            match r
                .0
                .transaction_generators(std::slice::from_raw_parts(ids, len))
            {
                Ok(owners) => {
                    for (index, owner) in owners.into_iter().enumerate() {
                        *out_generators.add(index) = owner.map_or(VTR_NONE, |id| id.0);
                    }
                    VTR_OK
                }
                Err(error) => status(Err(error)),
            }
        })
    }
}

unsafe fn emit_relations(
    list: Vec<vtr::Relation>,
    cb: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        u32,
        u64,
        u64,
        u32,
        *const u32,
        *const vtr_value,
    ) -> c_int,
    user: *mut std::ffi::c_void,
) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        for rel in &list {
            let keys: Vec<u32> = rel.attrs.iter().map(|(k, _)| k.0).collect();
            let vals: Vec<vtr_value> = rel.attrs.iter().map(|(_, v)| from_value(v)).collect();
            if cb(
                user,
                rel.kind.0,
                rel.from,
                rel.to,
                keys.len() as u32,
                keys.as_ptr(),
                vals.as_ptr(),
            ) != 0
            {
                break;
            }
        }
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_relations(
    r: *const vtr_reader,
    id: u64,
    direction: c_int,
    cb: vtr_relation_cb,
    user: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let Some(cb) = cb else { return VTR_ERR_NULL };
            let res = if direction == 0 {
                r.0.relations_from(id)
            } else {
                r.0.relations_to(id)
            };
            match res {
                Ok(list) => {
                    emit_relations(list, cb, user);
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

#[repr(C)]
pub struct vtr_log_site_info {
    pub node: u32,
    pub scope: u32,
    pub severity: u8,
    pub fmt: u32,
    pub file: u32,
    pub func: u32,
    pub line: u32,
    pub arg_count: u32,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_log_site_count(r: *const vtr_reader) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| match r.as_ref() {
            Some(r) => r.0.log_sites().len() as u32,
            None => 0,
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_log_count(r: *const vtr_reader) -> u64 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| match r.as_ref() {
            Some(r) => r.0.log_count(),
            None => 0,
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_log_site(
    r: *const vtr_reader,
    i: u32,
    out: *mut vtr_log_site_info,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out = need!(out);
            match r.0.log_sites().get(i as usize) {
                Some(s) => {
                    *out = vtr_log_site_info {
                        node: s.node.0,
                        scope: s.scope.map_or(VTR_NONE, |n| n.0),
                        severity: s.severity.code(),
                        fmt: s.fmt.0,
                        file: s.file.map_or(VTR_NONE, |x| x.0),
                        func: s.func.map_or(VTR_NONE, |x| x.0),
                        line: s.line.unwrap_or(0),
                        arg_count: s.args.len() as u32,
                    };
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_log_site_arg(
    r: *const vtr_reader,
    i: u32,
    j: u32,
    type_out: *mut u8,
    name_out: *mut u32,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            match r.0.log_sites().get(i as usize) {
                Some(s) if (j as usize) < s.args.len() => {
                    if let Some(o) = type_out.as_mut() {
                        *o = s.args[j as usize] as u8;
                    }
                    if let Some(o) = name_out.as_mut() {
                        *o = s.names.get(j as usize).map_or(VTR_NONE, |n| n.0);
                    }
                    VTR_OK
                }
                _ => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_log_site_of(r: *const vtr_reader, r#gen: u32) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| match r.as_ref() {
            Some(r) => r.0.log_site(NodeId(r#gen)).map_or(VTR_NONE, |s| s.index),
            None => VTR_NONE,
        })
    }
}

#[repr(C)]
pub struct vtr_log_rec {
    pub id: u64,
    pub time: u64,
    pub parent: u64,
    pub site: u32,
    pub generator: u32,
    pub scope: u32,
    pub severity: u8,
    pub arg_count: u32,
    inner: *const std::ffi::c_void,
}

pub type vtr_log_cb =
    Option<unsafe extern "C" fn(user: *mut std::ffi::c_void, rec: *const vtr_log_rec) -> c_int>;

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_visit_log(
    r: *const vtr_reader,
    scope: u32,
    generator: u32,
    min_severity: u8,
    t0: u64,
    t1: u64,
    cb: vtr_log_cb,
    user: *mut std::ffi::c_void,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let Some(cb) = cb else { return VTR_ERR_NULL };
            let q = LogQuery {
                scope: if scope == VTR_NONE {
                    None
                } else {
                    Some(NodeId(scope))
                },
                generator: if generator == VTR_NONE {
                    None
                } else {
                    Some(NodeId(generator))
                },
                min_severity: Severity::from_code(min_severity),
                window: if t1 == 0 { None } else { Some((t0, t1)) },
            };
            let res = r.0.visit_log(&q, |rec| {
                let h = vtr_log_rec {
                    id: rec.id,
                    time: rec.time,
                    parent: rec.parent.unwrap_or(0),
                    site: rec.site.index,
                    generator: rec.site.node.0,
                    scope: rec.site.scope.map_or(VTR_NONE, |n| n.0),
                    severity: rec.site.severity.code(),
                    arg_count: rec.arg_count() as u32,
                    inner: std::ptr::from_ref::<LogRecord>(rec).cast::<std::ffi::c_void>(),
                };
                cb(user, &raw const h) == 0
            });
            status(res)
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_log_rec_arg(
    rec: *const vtr_log_rec,
    i: u32,
    v_out: *mut vtr_value,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let rec = need_ref!(rec);
            let out = need!(v_out);
            let inner = &*rec.inner.cast::<LogRecord>();
            match inner.arg(i as usize) {
                Some(a) => {
                    let mut o = vtr_value {
                        tag: a.arg_type() as u8,
                        ..Default::default()
                    };
                    match a {
                        LogArg::Bool(b) => o.b = u8::from(b),
                        LogArg::I64(x) => o.i = x,
                        LogArg::U64(x) | LogArg::Time(x) | LogArg::Pointer(x) => o.u = x,
                        LogArg::F64(f) => o.f = f,
                        LogArg::Str(s) => o.str_id = s.0,
                        LogArg::Bytes(b) => {
                            o.data = b.as_ptr();
                            o.len = b.len();
                        }
                        LogArg::Text(s) => {
                            o.data = s.as_ptr();
                            o.len = s.len();
                        }
                    }
                    *out = o;
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

thread_local! {
    static FMT_BUF: RefCell<String> = const { RefCell::new(String::new()) };
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_log_rec_format(
    r: *const vtr_reader,
    rec: *const vtr_log_rec,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let (Some(r), Some(rec)) = (r.as_ref(), rec.as_ref()) else {
                return 0;
            };
            let inner = &*rec.inner.cast::<LogRecord>();
            FMT_BUF.with(|s| {
                let mut s = s.borrow_mut();
                s.clear();
                inner.format_into(r.0.strings(), &mut s);
                let n = s.len();
                if cap > 0 && !buf.is_null() {
                    let m = n.min(cap - 1);
                    ptr::copy_nonoverlapping(s.as_ptr(), buf.cast::<u8>(), m);
                    *buf.add(m) = 0;
                }
                n
            })
        })
    }
}

#[repr(C)]
pub struct vtr_clock_info {
    pub generator: u32,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_clock_count(r: *const vtr_reader) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| match r.as_ref() {
            Some(r) => r.0.clocks().len() as u32,
            None => 0,
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_clock(
    r: *const vtr_reader,
    clock: u32,
    out: *mut vtr_clock_info,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out = need!(out);
            match r.0.clocks().get(clock as usize) {
                Some(c) => {
                    *out = vtr_clock_info {
                        generator: c.generator.0,
                    };
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_generator_clock(r: *const vtr_reader, scope: u32) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| match r.as_ref() {
            Some(r) if (scope as usize) < r.0.hierarchy().len() => {
                r.0.generator_clock(NodeId(scope)).map_or(VTR_NONE, |c| c.0)
            }
            _ => VTR_NONE,
        })
    }
}

pub struct vtr_census(vtr::Census);

pub struct vtr_scope_sizes {
    nodes: Vec<u32>,
    sizes: vtr::ScopeSizes,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct vtr_scope_size {
    pub node: u32,
    pub parent: u32,
    pub signals: u32,
    pub variables: u32,
    pub scopes: u32,
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_census_new() -> *mut vtr_census {
    ffi(|| Box::into_raw(Box::new(vtr_census(vtr::Census::new()))))
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_census_new_recording() -> *mut vtr_census {
    ffi(|| Box::into_raw(Box::new(vtr_census(vtr::Census::recording()))))
}

pub struct vtr_contributions(vtr::Contributions);

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_census_finish_recorded(
    c: *mut vtr_census,
    out: *mut *mut vtr_contributions,
) -> *mut vtr_scope_sizes {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if c.is_null() {
                return ptr::null_mut();
            }
            let (sizes, contributions) = Box::from_raw(c).0.finish_recorded();
            if let Some(out) = out.as_mut() {
                *out = Box::into_raw(Box::new(vtr_contributions(contributions)));
            }
            Box::into_raw(Box::new(vtr_scope_sizes {
                nodes: Vec::new(),
                sizes,
            }))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_contributions_count(
    x: *const vtr_contributions,
    signals: *const u32,
    n: usize,
    out: *mut u32,
    cap: usize,
) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(x) = x.as_ref() else { return 0 };
            let signals: &[u32] = if n == 0 || signals.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts(signals, n)
            };
            let mut counts = Vec::new();
            x.0.count(signals.iter().copied(), &mut counts);
            if !out.is_null() {
                std::slice::from_raw_parts_mut(out, cap.min(counts.len()))
                    .copy_from_slice(&counts[..cap.min(counts.len())]);
            }
            counts.len()
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_contributions_free(x: *mut vtr_contributions) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !x.is_null() {
                drop(Box::from_raw(x));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_census_enter(c: *mut vtr_census) -> u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(c) = c.as_mut() {
                c.0.enter()
            } else {
                set_error("null handle");
                VTR_NONE
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_census_var(c: *mut vtr_census, signal: u32) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let c = need!(c);
            if c.0.depth() == 0 {
                set_error("variable outside a scope");
                return VTR_ERR_INVALID;
            }
            c.0.var(signal);
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_census_leave(c: *mut vtr_census) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let c = need!(c);
            if c.0.depth() == 0 {
                set_error("no scope is open");
                return VTR_ERR_INVALID;
            }
            c.0.leave();
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_census_finish(c: *mut vtr_census) -> *mut vtr_scope_sizes {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if c.is_null() {
                return ptr::null_mut();
            }
            let sizes = Box::from_raw(c).0.finish();
            Box::into_raw(Box::new(vtr_scope_sizes {
                nodes: Vec::new(),
                sizes,
            }))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_census_free(c: *mut vtr_census) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !c.is_null() {
                drop(Box::from_raw(c));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_scope_sizes(r: *const vtr_reader) -> *mut vtr_scope_sizes {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                set_error("null handle");
                return ptr::null_mut();
            };
            let (nodes, sizes) = r.0.hierarchy().scope_sizes();
            Box::into_raw(Box::new(vtr_scope_sizes {
                nodes: nodes.into_iter().map(|n| n.0).collect(),
                sizes,
            }))
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_scope_sizes_len(s: *const vtr_scope_sizes) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| s.as_ref().map_or(0, |s| s.sizes.len())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_scope_sizes_get(
    s: *const vtr_scope_sizes,
    i: usize,
    out: *mut vtr_scope_size,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let s = need_ref!(s);
            let out = need!(out);
            if i >= s.sizes.len() {
                return VTR_ERR_NOT_FOUND;
            }
            let k = i as u32;
            *out = vtr_scope_size {
                node: s.nodes.get(i).copied().unwrap_or(VTR_NONE),
                parent: s.sizes.parent(k).unwrap_or(VTR_NONE),
                signals: s.sizes.signals(k),
                variables: s.sizes.variables(k),
                scopes: s.sizes.scopes(k),
            };
            VTR_OK
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_scope_sizes_free(s: *mut vtr_scope_sizes) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !s.is_null() {
                drop(Box::from_raw(s));
            }
        });
    }
}

pub struct vtr_activity_index(vtr::activity::Index);
pub struct vtr_activity_classes {
    active: Vec<u32>,
    undecided: Vec<u32>,
}

#[repr(C)]
pub struct vtr_activity_summary {
    pub blocks: u32,
    pub signals: u32,
    pub changes: u64,
    pub stretches: u64,
    pub delta_min: u64,
    pub delta_max: u64,
    pub bytes: u64,
    pub source_bytes: u64,
}

unsafe fn activity_cache(cache_dir: *const c_char) -> Option<std::path::PathBuf> {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        match cstr(cache_dir) {
            Some(d) => Some(d.into()),
            None => vtr::activity::default_cache_dir(),
        }
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_write(
    r: *const vtr_reader,
    trace_path: *const c_char,
    cache_dir: *const c_char,
    threads: u32,
    memory: u64,
    out: *mut vtr_activity_summary,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        vtr_activity_write_controlled(r, trace_path, cache_dir, threads, memory, ptr::null(), out)
    }
}

pub struct vtr_activity_control(std::sync::Arc<vtr::activity::BuildControl>);

#[unsafe(no_mangle)]
pub extern "C" fn vtr_activity_control_new() -> *mut vtr_activity_control {
    ffi(|| {
        Box::into_raw(Box::new(vtr_activity_control(std::sync::Arc::new(
            vtr::activity::BuildControl::default(),
        ))))
    })
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_control_free(c: *mut vtr_activity_control) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !c.is_null() {
                drop(Box::from_raw(c));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_control_cancel(c: *const vtr_activity_control) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| c_int::from(c.as_ref().is_some_and(|c| c.0.cancel()))) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_control_is_cancelled(
    c: *const vtr_activity_control,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| c_int::from(c.as_ref().is_some_and(|c| c.0.is_cancelled()))) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_control_progress(
    c: *const vtr_activity_control,
    completed: *mut usize,
    total: *mut usize,
) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let p = c
                .as_ref()
                .map_or(vtr::activity::BuildProgress::default(), |c| c.0.progress());
            if let Some(out) = completed.as_mut() {
                *out = p.completed;
            }
            if let Some(out) = total.as_mut() {
                *out = p.total;
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_write_controlled(
    r: *const vtr_reader,
    trace_path: *const c_char,
    cache_dir: *const c_char,
    threads: u32,
    memory: u64,
    control: *const vtr_activity_control,
    out: *mut vtr_activity_summary,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let path = need_str!(trace_path);
            let cache = activity_cache(cache_dir);
            let mut opts = vtr::activity::BuildOptions {
                threads: threads as usize,
                ..Default::default()
            };
            if memory != 0 {
                opts.memory = memory;
            }
            opts.control = control.as_ref().map(|c| std::sync::Arc::clone(&c.0));
            let written = vtr::activity::Identity::of(&r.0).and_then(|id| {
                let sidecar = vtr::activity::Sidecar::new(path.as_ref(), &id, cache.as_deref());
                let build = |w: &mut dyn std::io::Write| vtr::activity::build(&r.0, w, &opts);
                match &opts.control {
                    Some(c) => sidecar.write_with_control(c, build),
                    None => sidecar.write(build),
                }
            });
            match written {
                Ok((_, s)) => {
                    if let Some(out) = out.as_mut() {
                        let (delta_min, delta_max) = s.delta.unwrap_or((0, 0));
                        *out = vtr_activity_summary {
                            blocks: s.blocks,
                            signals: s.signals,
                            changes: s.changes,
                            stretches: s.stretches,
                            delta_min,
                            delta_max,
                            bytes: s.bytes,
                            source_bytes: s.source_bytes,
                        };
                    }
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_load(
    r: *const vtr_reader,
    trace_path: *const c_char,
    cache_dir: *const c_char,
) -> *mut vtr_activity_index {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let (Some(r), Some(path)) = (r.as_ref(), cstr(trace_path)) else {
                set_error("null handle or path");
                return ptr::null_mut();
            };
            let id = match vtr::activity::Identity::of(&r.0) {
                Ok(id) => id,
                Err(e) => {
                    set_error(&e.to_string());
                    return ptr::null_mut();
                }
            };
            let cache = activity_cache(cache_dir);
            if let Some((_, index)) =
                vtr::activity::Sidecar::new(path.as_ref(), &id, cache.as_deref()).load(&id)
            {
                Box::into_raw(Box::new(vtr_activity_index(index)))
            } else {
                set_error("no valid activity index for this trace");
                ptr::null_mut()
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_control_complete(c: *const vtr_activity_control) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(c) = c.as_ref() {
                match c.0.complete() {
                    Ok(()) => 1,
                    Err(error) => {
                        set_error(&error.to_string());
                        0
                    }
                }
            } else {
                set_error("null activity control");
                0
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_decode(
    r: *const vtr_reader,
    data: *const u8,
    len: usize,
    memory: u64,
    control: *const vtr_activity_control,
) -> *mut vtr_activity_index {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                set_error("null reader");
                return ptr::null_mut();
            };
            if (data.is_null() && len != 0) || len > isize::MAX as usize {
                set_error("invalid image pointer or length");
                return ptr::null_mut();
            }
            let bytes = if len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(data, len)
            };
            let id = match vtr::activity::Identity::of(&r.0) {
                Ok(id) => id,
                Err(error) => {
                    set_error(&error.to_string());
                    return ptr::null_mut();
                }
            };
            let mut decode = std::pin::pin!(vtr::activity::Index::decode_with(
                bytes,
                &id,
                |peak| if memory == 0 || peak <= memory {
                    Ok(())
                } else {
                    Err(Error::Invalid(
                        "activity decode exceeds memory limit".into(),
                    ))
                },
                || std::future::ready(if control.as_ref().is_some_and(|c| c.0.is_cancelled()) {
                    Err(Error::State("activity decode cancelled"))
                } else {
                    Ok(())
                })
            ));
            let waker = std::task::Waker::noop();
            match std::future::Future::poll(
                decode.as_mut(),
                &mut std::task::Context::from_waker(waker),
            ) {
                std::task::Poll::Ready(Ok(index)) => {
                    Box::into_raw(Box::new(vtr_activity_index(index)))
                }
                std::task::Poll::Ready(Err(error)) => {
                    set_error(&error.to_string());
                    ptr::null_mut()
                }
                std::task::Poll::Pending => unreachable!("blocking decode never yields"),
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_free(x: *mut vtr_activity_index) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !x.is_null() {
                drop(Box::from_raw(x));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_exact_width(
    x: *const vtr_activity_index,
    t0: u64,
    t1: u64,
) -> u64 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| x.as_ref().map_or(0, |x| x.0.exact_width(t0, t1))) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_classify(
    x: *const vtr_activity_index,
    t0: u64,
    t1: u64,
) -> *mut vtr_activity_classes {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(x) = x.as_ref() {
                let c = x.0.classify(t0, t1);
                let ids = |v: Vec<vtr::SignalId>| v.into_iter().map(|s| s.0).collect();
                Box::into_raw(Box::new(vtr_activity_classes {
                    active: ids(c.active),
                    undecided: ids(c.undecided),
                }))
            } else {
                set_error("null handle");
                ptr::null_mut()
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_classes_active(
    c: *const vtr_activity_classes,
    len: *mut usize,
) -> *const u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let v = c.as_ref().map_or(&[][..], |c| &c.active[..]);
            if let Some(len) = len.as_mut() {
                *len = v.len();
            }
            v.as_ptr()
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_classes_undecided(
    c: *const vtr_activity_classes,
    len: *mut usize,
) -> *const u32 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let v = c.as_ref().map_or(&[][..], |c| &c.undecided[..]);
            if let Some(len) = len.as_mut() {
                *len = v.len();
            }
            v.as_ptr()
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_classes_free(c: *mut vtr_activity_classes) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !c.is_null() {
                drop(Box::from_raw(c));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_activity_resolve(
    r: *const vtr_reader,
    signals: *const u32,
    n: usize,
    t0: u64,
    t1: u64,
    out: *mut u32,
    out_len: *mut usize,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let r = need_ref!(r);
            let out_len = need!(out_len);
            if n > 0 && (signals.is_null() || out.is_null()) {
                set_error("null signal array");
                return VTR_ERR_NULL;
            }
            let sigs: Vec<vtr::SignalId> = if n == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(signals, n)
                    .iter()
                    .map(|&s| vtr::SignalId(s))
                    .collect()
            };
            match vtr::activity::resolve(&r.0, &sigs, t0, t1) {
                Ok(v) => {
                    for (k, s) in v.iter().enumerate() {
                        *out.add(k) = s.0;
                    }
                    *out_len = v.len();
                    VTR_OK
                }
                Err(e) => status(Err(e)),
            }
        })
    }
}

pub struct vtr_clock_timeline(std::sync::Arc<ClockTimeline>);

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_reader_load_clock(
    r: *const vtr_reader,
    clock: u32,
) -> *mut vtr_clock_timeline {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let Some(r) = r.as_ref() else {
                set_error("null handle");
                return ptr::null_mut();
            };
            match r.0.clock(ClockId(clock)) {
                Ok(t) => Box::into_raw(Box::new(vtr_clock_timeline(t))),
                Err(e) => {
                    set_error(&e.to_string());
                    ptr::null_mut()
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_free(t: *mut vtr_clock_timeline) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if !t.is_null() {
                drop(Box::from_raw(t));
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_stretch_count(t: *const vtr_clock_timeline) -> usize {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| t.as_ref().map_or(0, |t| t.0.stretches().len())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_edge_count(t: *const vtr_clock_timeline) -> u64 {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| t.as_ref().map_or(0, |t| t.0.edge_count())) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_is_open(t: *const vtr_clock_timeline) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| t.as_ref().map_or(0, |t| c_int::from(t.0.is_open()))) }
}

#[repr(C)]
pub struct vtr_stretch {
    pub begin: u64,
    pub end: u64,
    pub period: u64,
    pub first_cycle: u64,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_stretch(
    t: *const vtr_clock_timeline,
    i: usize,
    out: *mut vtr_stretch,
) -> c_int {
    // SAFETY: The caller upholds the module pointer contract for every input and output.
    unsafe {
        ffi(|| {
            let t = need_ref!(t);
            let out = need!(out);
            match t.0.stretches().get(i) {
                Some(s) => {
                    *out = vtr_stretch {
                        begin: s.begin,
                        end: s.end,
                        period: s.period,
                        first_cycle: s.first_cycle,
                    };
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

#[repr(C)]
pub struct vtr_cycle_at {
    pub cycle: u64,
    pub edge: u64,
    pub has_next_edge: c_int,
    pub next_edge: u64,
    pub fraction: f64,
    pub stopped: c_int,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_cycle_at(
    t: *const vtr_clock_timeline,
    time: u64,
    out: *mut vtr_cycle_at,
) -> c_int {
    // SAFETY: The caller provides a live timeline and an exclusive writable output.
    unsafe {
        ffi(|| {
            let t = need_ref!(t);
            let out = need!(out);
            match t.0.cycle_at(time) {
                Some(c) => {
                    *out = vtr_cycle_at {
                        cycle: c.cycle,
                        edge: c.edge,
                        has_next_edge: c_int::from(c.next_edge.is_some()),
                        next_edge: c.next_edge.unwrap_or(0),
                        fraction: c.fraction,
                        stopped: c_int::from(c.stopped),
                    };
                    VTR_OK
                }
                None => VTR_ERR_NOT_FOUND,
            }
        })
    }
}

unsafe fn time_out(r: Option<u64>, out: *mut u64) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        match (r, out.as_mut()) {
            (_, None) => {
                set_error("null output");
                VTR_ERR_NULL
            }
            (Some(v), Some(o)) => {
                *o = v;
                VTR_OK
            }
            (None, Some(_)) => VTR_ERR_NOT_FOUND,
        }
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_edge(
    t: *const vtr_clock_timeline,
    cycle: u64,
    out: *mut u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| time_out(need_ref!(t).0.edge(cycle), out)) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_next_edge(
    t: *const vtr_clock_timeline,
    time: u64,
    out: *mut u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| time_out(need_ref!(t).0.next_edge(time), out)) }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_clock_timeline_prev_edge(
    t: *const vtr_clock_timeline,
    time: u64,
    out: *mut u64,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe { ffi(|| time_out(need_ref!(t).0.prev_edge(time), out)) }
}

#[repr(C)]
pub struct vtr_guard_options {
    pub deadline_ms: u32,
    pub crashes: c_int,
    pub stops: c_int,
    pub stop_grace_ms: u32,
    pub exit: c_int,
    pub park_signal: c_int,
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_guard_options_default(o: *mut vtr_guard_options) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(o) = o.as_mut() {
                let d = vtr_guard::Options::default();
                *o = vtr_guard_options {
                    deadline_ms: d.deadline.as_millis() as u32,
                    crashes: c_int::from(d.crashes),
                    stops: c_int::from(d.stops),
                    stop_grace_ms: d.stop_grace.as_millis() as u32,
                    exit: c_int::from(d.exit),
                    park_signal: d.park_signal,
                };
            }
        });
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_guard_install(
    opts: *const vtr_guard_options,
    active_out: *mut c_int,
) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let mut o = vtr_guard::Options::default();
            if let Some(c) = opts.as_ref() {
                o.deadline = std::time::Duration::from_millis(u64::from(c.deadline_ms));
                o.crashes = c.crashes != 0;
                o.stops = c.stops != 0;
                o.stop_grace = std::time::Duration::from_millis(u64::from(c.stop_grace_ms));
                o.exit = c.exit != 0;
                o.park_signal = c.park_signal;
            }
            match vtr_guard::install(&o) {
                Ok(active) => {
                    if let Some(a) = active_out.as_mut() {
                        *a = c_int::from(active);
                    }
                    VTR_OK
                }
                Err(e) => {
                    set_error(&e.to_string());
                    VTR_ERR_STATE
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_guard_watch(w: *mut vtr_writer) -> c_int {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            let w = need!(w);
            match vtr_guard::watch(&raw mut w.0) {
                Ok(()) => VTR_OK,
                Err(e) => {
                    set_error(&e.to_string());
                    VTR_ERR_STATE
                }
            }
        })
    }
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_guard_unwatch(w: *mut vtr_writer) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        ffi(|| {
            if let Some(w) = w.as_mut() {
                vtr_guard::unwatch(&raw mut w.0);
            }
        });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_guard_thread_init() {
    ffi(vtr_guard::thread_init);
}

struct StopCallback(unsafe extern "C" fn(*mut std::ffi::c_void, c_int), usize);

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_guard_add_stop_callback(
    cb: Option<unsafe extern "C" fn(*mut std::ffi::c_void, c_int)>,
    user: *mut std::ffi::c_void,
) -> u64 {
    ffi(|| match cb {
        Some(cb) => {
            let c = StopCallback(cb, user as usize);
            // SAFETY: Registration requires the callback and user data to remain valid on the rescue thread until removal.
            vtr_guard::add_stop_callback(Box::new(move |sig| unsafe {
                (c.0)(c.1 as *mut std::ffi::c_void, sig);
            }))
        }
        None => 0,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_guard_remove_stop_callback(id: u64) {
    ffi(|| vtr_guard::remove_stop_callback(id));
}

#[unsafe(no_mangle)]
pub extern "C" fn vtr_guard_stop_requested() -> c_int {
    vtr_guard::stop_requested()
}

/// # Safety
/// The caller must uphold the module's [pointer safety](self#pointer-safety) contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vtr_guard_ending(out: *mut vtr_ending) {
    // SAFETY: Inputs and outputs satisfy the caller’s module-level pointer safety contract.
    unsafe {
        if let Some(o) = out.as_mut() {
            *o = to_ending(vtr_guard::ending(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last_error() -> String {
        // SAFETY: The thread-local error CString remains live and unchanged until the next API call.
        unsafe { CStr::from_ptr(vtr_last_error()) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn a_panic_becomes_an_error_and_poisons_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = CString::new(dir.path().join("p.vtr").to_str().unwrap()).unwrap();
        // SAFETY: Handles come from these constructors; CStrings and outputs stay live until each call returns.
        unsafe {
            let w = vtr_writer_create(path.as_ptr(), ptr::null());
            let mut sig = 0u32;
            let name = CString::new("a").unwrap();
            vtr_writer_add_var(
                w,
                VTR_NONE,
                name.as_ptr(),
                0,
                0,
                0,
                8,
                2,
                ptr::null_mut(),
                &raw mut sig,
            );
            assert_eq!(vtr_writer_emit_u64(w, sig, 1), VTR_OK);
            let rc = ffi_w(w, || -> c_int { panic!("injected panic") });
            assert_eq!(rc, VTR_ERR_STATE);
            assert!(last_error().contains("injected panic"), "{}", last_error());
            assert_eq!(vtr_writer_emit_u64(w, sig, 2), VTR_ERR_STATE);
            assert!(last_error().contains("poisoned"), "{}", last_error());
            assert_eq!(
                vtr_writer_add_scope(w, VTR_NONE, name.as_ptr(), 0, ptr::null()),
                VTR_NONE
            );
            assert_eq!(vtr_writer_close(w), VTR_OK);
            let r = vtr_reader_open(path.as_ptr());
            assert!(!r.is_null());
            let mut end = vtr_ending::default();
            assert_eq!(vtr_reader_ending(r, &raw mut end), VTR_OK);
            assert_eq!(end.kind, VTR_ENDING_POISONED);
            vtr_reader_close(r);
        }
    }
}
