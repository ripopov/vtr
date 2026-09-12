#![allow(non_camel_case_types)]
//! Stable C ABI over the VTR Rust implementation.
//!
//! Conventions:
//! * Every function that can fail returns `int`: 0 = `VTR_OK`, otherwise a
//!   `VTR_ERR_*` code. `vtr_last_error()` returns a thread-local message.
//! * Handles (`vtr_writer*`, `vtr_reader*`, ...) are opaque and owned by the
//!   caller until the matching `*_close`/`*_free` call. No global state.
//! * Strings passed in are NUL-terminated UTF-8 unless a length is given.
//! * Pointers returned by reader accessors are valid as long as the reader
//!   (or the buffer object they were read into) is alive.
//! * The header `include/vtr.h` documents every function.

#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;
use vtr::{
    AttrPhase, Direction, Error, LogArg, LogArgType, LogQuery, LogRecord, LogSiteId, LogSiteSpec, NodeData, NodeId, Reader, ScopeType, Severity, SignalId,
    SignalKind, StrId, Transaction, TxKind, TxQuery, TxStatus, Value, VarType, Writer, WriterOptions,
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

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

macro_rules! need {
    ($p:expr) => {
        match unsafe { $p.as_mut() } {
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
        match unsafe { $p.as_ref() } {
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
        match unsafe { cstr($p) } {
            Some(s) => s,
            None => {
                set_error("null or non-UTF-8 string");
                return VTR_ERR_NULL;
            }
        }
    };
}

/// Returns the last error message of this thread (never NULL).
#[no_mangle]
pub extern "C" fn vtr_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Library version string.
#[no_mangle]
pub extern "C" fn vtr_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// Tagged attribute value (mirrors `vtr::Value`; lists/maps are not exposed).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct vtr_value {
    /// One of the `VTR_VAL_*` tags.
    pub tag: u8,
    pub b: u8,
    pub width: u32,
    pub i: i64,
    pub u: u64,
    pub f: f64,
    pub str_id: u32,
    pub scale: i32,
    /// Bytes / packed bits (valid while the owning object is alive).
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
        vtr_value { tag: 0, b: 0, width: 0, i: 0, u: 0, f: 0.0, str_id: 0, scale: 0, data: ptr::null(), len: 0 }
    }
}

unsafe fn to_value(v: &vtr_value) -> Result<Value, Error> {
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
        VTR_VAL_BITS => Value::Bits { width: v.width, data: bytes(v) },
        VTR_VAL_LOGIC => Value::Logic { width: v.width, data: bytes(v) },
        VTR_VAL_LOGIC9 => Value::Logic9 { width: v.width, data: bytes(v) },
        VTR_VAL_TIME => Value::Time(v.u),
        VTR_VAL_ENUM => Value::Enum { value: v.i, name: StrId(v.str_id) },
        VTR_VAL_POINTER => Value::Pointer(v.u),
        VTR_VAL_FIXED => Value::Fixed { raw: v.i, scale: v.scale },
        VTR_VAL_UFIXED => Value::UFixed { raw: v.u, scale: v.scale },
        VTR_VAL_TEXT => Value::Text(String::from_utf8(bytes(v)).map_err(|_| Error::Invalid("text value is not UTF-8".into()))?),
        _ => return Err(Error::Invalid("unsupported value tag".into())),
    })
}

/// Borrowing conversion for log arguments (no allocation).
unsafe fn to_log_arg<'a>(v: &'a vtr_value) -> Result<LogArg<'a>, Error> {
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
        VTR_VAL_TEXT => LogArg::Text(std::str::from_utf8(slice(v)).map_err(|_| Error::Invalid("text argument is not UTF-8".into()))?),
        _ => return Err(Error::Invalid("unsupported log argument tag".into())),
    })
}

fn from_value(v: &Value) -> vtr_value {
    let mut o = vtr_value { tag: v.tag() as u8, ..Default::default() };
    match v {
        Value::Null => {}
        Value::Bool(b) => o.b = *b as u8,
        Value::I64(i) => o.i = *i,
        Value::U64(u) | Value::Time(u) | Value::Pointer(u) => o.u = *u,
        Value::F64(f) => o.f = *f,
        Value::Str(s) => o.str_id = s.0,
        Value::Bytes(b) => {
            o.data = b.as_ptr();
            o.len = b.len();
        }
        Value::Bits { width, data } | Value::Logic { width, data } | Value::Logic9 { width, data } => {
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

unsafe fn kv_list(n: usize, keys: *const u32, values: *const vtr_value) -> Result<Vec<(StrId, Value)>, Error> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let k = *keys.add(i);
        let v = to_value(&*values.add(i))?;
        out.push((StrId(k), v));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

pub struct vtr_writer(Writer);

#[repr(C)]
pub struct vtr_writer_options {
    /// 0 = none, 1 = lz4, 2 = zstd
    pub codec: u8,
    pub level: c_int,
    pub group_size: u32,
    pub block_records: u64,
    pub chunk_records: u64,
    pub tx_block_bytes: u64,
    pub background: c_int,
    pub dedup: c_int,
    pub checksums: c_int,
    /// Helper threads encoding log blocks (background mode); 0 = on the sink thread.
    pub log_encoders: u32,
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_options_default(o: *mut vtr_writer_options) {
    if let Some(o) = o.as_mut() {
        let d = WriterOptions::default();
        *o = vtr_writer_options {
            codec: d.compression.codec as u8,
            level: d.compression.level,
            group_size: d.group_size,
            block_records: d.block_records as u64,
            chunk_records: d.chunk_records as u64,
            tx_block_bytes: d.tx_block_bytes as u64,
            background: d.background as c_int,
            dedup: d.dedup as c_int,
            checksums: d.checksums as c_int,
            log_encoders: d.log_encoders as u32,
        };
    }
}

/// Creates a writer. `opts` may be NULL for defaults. Returns NULL on error.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_create(path: *const c_char, opts: *const vtr_writer_options) -> *mut vtr_writer {
    let path = match cstr(path) {
        Some(p) => p,
        None => {
            set_error("null path");
            return ptr::null_mut();
        }
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
}

/// Finishes the file and frees the writer. Returns the close status.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_close(w: *mut vtr_writer) -> c_int {
    if w.is_null() {
        return VTR_ERR_NULL;
    }
    let mut b = Box::from_raw(w);
    status(b.0.close())
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_timescale(w: *mut vtr_writer, exp: i8) -> c_int {
    status(need!(w).0.set_timescale(exp))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_time_zero(w: *mut vtr_writer, t: i64) -> c_int {
    status(need!(w).0.set_time_zero(t))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_file_type(w: *mut vtr_writer, ft: u8) -> c_int {
    status(need!(w).0.set_file_type(vtr::FileType::from_u8(ft)))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_writer_name(w: *mut vtr_writer, s: *const c_char) -> c_int {
    let s = need_str!(s);
    status(need!(w).0.set_writer_name(s))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_date(w: *mut vtr_writer, s: *const c_char) -> c_int {
    let s = need_str!(s);
    status(need!(w).0.set_date(s))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_comment(w: *mut vtr_writer, s: *const c_char) -> c_int {
    let s = need_str!(s);
    status(need!(w).0.set_comment(s))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_file_attr(w: *mut vtr_writer, key: *const c_char, v: *const vtr_value) -> c_int {
    let key = need_str!(key);
    let v = need_ref!(v);
    let val = match to_value(v) {
        Ok(v) => v,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.set_file_attr(key, val))
}

/// Interns a string and returns its id (0 for the empty string).
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_intern(w: *mut vtr_writer, s: *const c_char) -> u32 {
    match (w.as_mut(), cstr(s)) {
        (Some(w), Some(s)) => w.0.intern(s).0,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_begin_scope(w: *mut vtr_writer, name: *const c_char, scope_type: u16, component: *const c_char) -> u32 {
    match (w.as_mut(), cstr(name)) {
        (Some(w), Some(n)) => w.0.begin_scope(n, ScopeType::from_code(scope_type), cstr(component).unwrap_or("")).0,
        _ => VTR_NONE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_end_scope(w: *mut vtr_writer) -> c_int {
    status(need!(w).0.end_scope())
}

/// Declares a variable. `kind`: 0 = bits (width, states), 1 = real, 2 = variable-length.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_var(
    w: *mut vtr_writer,
    name: *const c_char,
    var_type: u16,
    direction: u8,
    kind: u8,
    width: u32,
    states: u8,
    node_out: *mut u32,
    signal_out: *mut u32,
) -> c_int {
    let name = need_str!(name);
    let w = need!(w);
    let k = match kind {
        0 => {
            if !matches!(states, 2 | 4 | 9) {
                set_error("states must be 2, 4 or 9");
                return VTR_ERR_INVALID;
            }
            SignalKind::Bits { width: width.max(1), states }
        }
        1 => SignalKind::Real,
        2 => SignalKind::VarLen,
        _ => {
            set_error("kind must be 0, 1 or 2");
            return VTR_ERR_INVALID;
        }
    };
    let (n, s) = w.0.add_var(name, VarType::from_code(var_type), Direction::from_u8(direction), k);
    if let Some(o) = node_out.as_mut() {
        *o = n.0;
    }
    if let Some(o) = signal_out.as_mut() {
        *o = s.0;
    }
    VTR_OK
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_alias(w: *mut vtr_writer, name: *const c_char, var_type: u16, direction: u8, signal: u32, node_out: *mut u32) -> c_int {
    let name = need_str!(name);
    match need!(w).0.add_alias(name, VarType::from_code(var_type), Direction::from_u8(direction), SignalId(signal)) {
        Ok(n) => {
            if let Some(o) = node_out.as_mut() {
                *o = n.0;
            }
            VTR_OK
        }
        Err(e) => status(Err(e)),
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_enum_table(w: *mut vtr_writer, name: *const c_char, n: usize, literals: *const *const c_char, values: *const *const c_char) -> u32 {
    let (w, name) = match (w.as_mut(), cstr(name)) {
        (Some(w), Some(n)) => (w, n),
        _ => return VTR_NONE,
    };
    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        let l = cstr(*literals.add(i)).unwrap_or("");
        let v = cstr(*values.add(i)).unwrap_or("");
        entries.push((l, v));
    }
    w.0.add_enum_table(name, &entries).0
}

/// `parent` may be VTR_NONE.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_stream(w: *mut vtr_writer, parent: u32, name: *const c_char, kind: *const c_char) -> u32 {
    match (w.as_mut(), cstr(name)) {
        (Some(w), Some(n)) => w.0.add_stream(if parent == VTR_NONE { None } else { Some(NodeId(parent)) }, n, cstr(kind).unwrap_or("")).0,
        _ => VTR_NONE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_generator(w: *mut vtr_writer, stream: u32, name: *const c_char) -> u32 {
    match (w.as_mut(), cstr(name)) {
        (Some(w), Some(n)) => w.0.add_generator(NodeId(stream), n).0,
        _ => VTR_NONE,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_node_attr(w: *mut vtr_writer, node: u32, key: *const c_char, v: *const vtr_value) -> c_int {
    let key = need_str!(key);
    let v = need_ref!(v);
    let val = match to_value(v) {
        Ok(v) => v,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.node_attr(NodeId(node), key, val))
}

#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_time(w: *mut vtr_writer, t: u64) -> c_int {
    status(need!(w).0.set_time(t))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_dump_off(w: *mut vtr_writer) -> c_int {
    need!(w).0.dump_off();
    VTR_OK
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_dump_on(w: *mut vtr_writer) -> c_int {
    need!(w).0.dump_on();
    VTR_OK
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_bit(w: *mut vtr_writer, sig: u32, code: u8) -> c_int {
    status(need!(w).0.emit_bit(SignalId(sig), code))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_u64(w: *mut vtr_writer, sig: u32, value: u64) -> c_int {
    status(need!(w).0.emit_u64(SignalId(sig), value))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_words(w: *mut vtr_writer, sig: u32, words: *const u32, n: usize) -> c_int {
    let ws = if words.is_null() { &[][..] } else { std::slice::from_raw_parts(words, n) };
    status(need!(w).0.emit_words(SignalId(sig), ws))
}
/// `len` may be SIZE_MAX for a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_logic_str(w: *mut vtr_writer, sig: u32, s: *const c_char, len: usize) -> c_int {
    if s.is_null() {
        set_error("null value");
        return VTR_ERR_NULL;
    }
    let bytes = if len == usize::MAX { CStr::from_ptr(s).to_bytes() } else { std::slice::from_raw_parts(s as *const u8, len) };
    status(need!(w).0.emit_logic_str(SignalId(sig), bytes))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_packed(w: *mut vtr_writer, sig: u32, states: u8, data: *const u8, len: usize) -> c_int {
    let d = if data.is_null() { &[][..] } else { std::slice::from_raw_parts(data, len) };
    status(need!(w).0.emit_packed(SignalId(sig), states, d))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_real(w: *mut vtr_writer, sig: u32, v: f64) -> c_int {
    status(need!(w).0.emit_real(SignalId(sig), v))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_emit_varlen(w: *mut vtr_writer, sig: u32, data: *const u8, len: usize) -> c_int {
    let d = if data.is_null() { &[][..] } else { std::slice::from_raw_parts(data, len) };
    status(need!(w).0.emit_varlen(SignalId(sig), d))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_flush(w: *mut vtr_writer) -> c_int {
    status(need!(w).0.flush())
}

// transactions
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_begin_tx(w: *mut vtr_writer, generator: u32, time: u64, tx_out: *mut u64) -> c_int {
    match need!(w).0.begin_tx(NodeId(generator), time) {
        Ok(id) => {
            if let Some(o) = tx_out.as_mut() {
                *o = id;
            }
            VTR_OK
        }
        Err(e) => status(Err(e)),
    }
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_tx_parent(w: *mut vtr_writer, tx: u64, parent: u64) -> c_int {
    status(need!(w).0.set_tx_parent(tx, parent))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_set_tx_kind(w: *mut vtr_writer, tx: u64, kind: u8) -> c_int {
    status(need!(w).0.set_tx_kind(tx, TxKind::from_u8(kind)))
}
/// `phase`: 0 = begin, 1 = record, 2 = end.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_tx_attr(w: *mut vtr_writer, tx: u64, key: u32, phase: u8, v: *const vtr_value) -> c_int {
    let v = need_ref!(v);
    let val = match to_value(v) {
        Ok(v) => v,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.tx_attr(tx, StrId(key), AttrPhase::from_u8(phase), &val))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_tx_event(w: *mut vtr_writer, tx: u64, time: u64, name: u32, n: usize, keys: *const u32, values: *const vtr_value) -> c_int {
    let attrs = match kv_list(n, keys, values) {
        Ok(a) => a,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.tx_event(tx, time, StrId(name), &attrs))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_tx_stage_begin(w: *mut vtr_writer, tx: u64, name: u32, lane: u32, time: u64) -> c_int {
    status(need!(w).0.tx_stage_begin(tx, StrId(name), StrId(lane), time))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_tx_stage_end(w: *mut vtr_writer, tx: u64, name: u32, lane: u32, time: u64) -> c_int {
    match need!(w).0.tx_stage_end(tx, StrId(name), StrId(lane), time) {
        Ok(true) => VTR_OK,
        Ok(false) => VTR_ERR_NOT_FOUND,
        Err(e) => status(Err(e)),
    }
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_tx_stage(w: *mut vtr_writer, tx: u64, name: u32, lane: u32, begin: u64, end: u64, n: usize, keys: *const u32, values: *const vtr_value) -> c_int {
    let attrs = match kv_list(n, keys, values) {
        Ok(a) => a,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.tx_stage(tx, StrId(name), StrId(lane), begin, end, &attrs))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_tx_stage_attr(w: *mut vtr_writer, tx: u64, key: u32, v: *const vtr_value) -> c_int {
    let v = need_ref!(v);
    let val = match to_value(v) {
        Ok(v) => v,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.tx_stage_attr(tx, StrId(key), &val))
}
/// `status_code`: 0 unset, 1 ok, 2 error, 3 aborted.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_end_tx(w: *mut vtr_writer, tx: u64, time: u64, status_code: u8) -> c_int {
    status(need!(w).0.end_tx(tx, time, TxStatus::from_u8(status_code)))
}
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_relate(w: *mut vtr_writer, kind: u32, from: u64, to: u64, n: usize, keys: *const u32, values: *const vtr_value) -> c_int {
    let attrs = match kv_list(n, keys, values) {
        Ok(a) => a,
        Err(e) => return status(Err(e)),
    };
    status(need!(w).0.relate(StrId(kind), from, to, &attrs))
}

// ----- logs -----

/// Declares a log stream (kind "LOG"); `parent` may be VTR_NONE.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_log_stream(w: *mut vtr_writer, parent: u32, name: *const c_char) -> u32 {
    let w = match w.as_mut() {
        Some(w) => w,
        None => return VTR_NONE,
    };
    let name = match cstr(name) {
        Some(s) => s,
        None => return VTR_NONE,
    };
    let parent = if parent == VTR_NONE { None } else { Some(NodeId(parent)) };
    w.0.add_log_stream(parent, name).0
}

/// Registers a log call site. `arg_types` are VTR_VAL_* tags (n_args of them;
/// allowed: BOOL I64 U64 F64 STR BYTES TIME POINTER TEXT); `file`, `func` and
/// `names` (n_args argument names) may be NULL. Returns the site id or VTR_NONE.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_add_log_site(
    w: *mut vtr_writer,
    stream: u32,
    severity: u8,
    fmt: *const c_char,
    file: *const c_char,
    line: u32,
    func: *const c_char,
    n_args: usize,
    arg_types: *const u8,
    names: *const *const c_char,
) -> u32 {
    let w = match w.as_mut() {
        Some(w) => w,
        None => {
            set_error("null handle");
            return VTR_NONE;
        }
    };
    let fmt = match cstr(fmt) {
        Some(s) => s,
        None => {
            set_error("null or non-UTF-8 format string");
            return VTR_NONE;
        }
    };
    if n_args > 0 && arg_types.is_null() {
        set_error("null arg_types");
        return VTR_NONE;
    }
    let mut types = Vec::with_capacity(n_args);
    for i in 0..n_args {
        match LogArgType::from_u8(*arg_types.add(i)) {
            Ok(t) => types.push(t),
            Err(_) => {
                set_error("unsupported log argument type");
                return VTR_NONE;
            }
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
    let mut spec = LogSiteSpec::new(NodeId(stream), Severity::from_code(severity), fmt, &types).names(&name_strs);
    spec.file = cstr(file).unwrap_or("");
    spec.line = line;
    spec.func = cstr(func).unwrap_or("");
    w.0.add_log_site(&spec).0
}

/// Generator node of a log site (VTR_NONE if unknown).
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_log_site_node(w: *const vtr_writer, site: u32) -> u32 {
    match w.as_ref() {
        Some(w) => w.0.log_site_node(LogSiteId(site)).map(|n| n.0).unwrap_or(VTR_NONE),
        None => VTR_NONE,
    }
}

/// Records a log message: `n` argument values matching the site's declared
/// types; `parent` is a transaction id or 0. `id_out` (nullable) receives the
/// record's transaction id.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_log(w: *mut vtr_writer, site: u32, time: u64, parent: u64, n: usize, args: *const vtr_value, id_out: *mut u64) -> c_int {
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
}

/// Records a log message from pre-encoded argument values (row encoding, see
/// `Writer::log_raw`); the caller guarantees they match the site's types.
#[no_mangle]
pub unsafe extern "C" fn vtr_writer_log_raw(w: *mut vtr_writer, site: u32, time: u64, parent: u64, args: *const u8, len: usize, id_out: *mut u64) -> c_int {
    let w = need!(w);
    let bytes: &[u8] = if len == 0 { &[] } else if args.is_null() {
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
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

pub struct vtr_reader(Reader);

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_open(path: *const c_char) -> *mut vtr_reader {
    let path = match cstr(path) {
        Some(p) => p,
        None => {
            set_error("null path");
            return ptr::null_mut();
        }
    };
    match Reader::open(path) {
        Ok(r) => Box::into_raw(Box::new(vtr_reader(r))),
        Err(e) => {
            set_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_close(r: *mut vtr_reader) {
    if !r.is_null() {
        drop(Box::from_raw(r));
    }
}

/// Requires exclusive access to the reader. Invalidates borrowed time tables.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_clear_cache(r: *mut vtr_reader) {
    if let Some(r) = r.as_mut() {
        r.0.clear_cache();
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
    /// Log records (also counted in `tx_count`).
    pub log_count: u64,
    pub log_site_count: u32,
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_meta(r: *const vtr_reader, out: *mut vtr_meta) -> c_int {
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
        has_time_range: tr.is_some() as c_int,
        time_start: tr.map(|x| x.0).unwrap_or(0),
        time_end: tr.map(|x| x.1).unwrap_or(0),
        version_major: r.0.version().0,
        version_minor: r.0.version().1,
        recovered: r.0.recovered() as c_int,
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
}

/// `which`: 0 writer, 1 date, 2 comment. Valid while the reader is alive.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_meta_string(r: *const vtr_reader, which: c_int, len_out: *mut usize) -> *const c_char {
    let r = match r.as_ref() {
        Some(r) => r,
        None => return ptr::null(),
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
    s.as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_file_attr(r: *const vtr_reader, i: u32, key_out: *mut u32, v_out: *mut vtr_value) -> c_int {
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
}

/// Interned string by id: pointer + length (not NUL-terminated). Valid while the reader is alive.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_str(r: *const vtr_reader, id: u32, len_out: *mut usize) -> *const c_char {
    let r = match r.as_ref() {
        Some(r) => r,
        None => return ptr::null(),
    };
    let s = r.0.str(StrId(id));
    if let Some(l) = len_out.as_mut() {
        *l = s.len();
    }
    s.as_ptr() as *const c_char
}

#[repr(C)]
pub struct vtr_node_info {
    /// 1 scope, 2 var, 3 stream, 4 generator, 5 enum table
    pub kind: u8,
    pub parent: u32,
    pub name: u32,
    /// Scope type or var type code.
    pub type_code: u16,
    pub direction: u8,
    /// Var: signal id. Others: VTR_NONE.
    pub signal: u32,
    pub is_alias: c_int,
    /// Scope: component string id. Stream: kind string id.
    pub aux_str: u32,
    pub attr_count: u32,
    /// Enum table: number of entries.
    pub entry_count: u32,
    pub child_count: u32,
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_node_count(r: *const vtr_reader) -> u32 {
    r.as_ref().map(|r| r.0.hierarchy().len() as u32).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_node(r: *const vtr_reader, id: u32, out: *mut vtr_node_info) -> c_int {
    let r = need_ref!(r);
    let out = need!(out);
    let h = r.0.hierarchy();
    if id as usize >= h.len() {
        return VTR_ERR_NOT_FOUND;
    }
    let n = h.node(NodeId(id));
    let mut info = vtr_node_info {
        kind: n.kind() as u8,
        parent: n.parent.map(|p| p.0).unwrap_or(VTR_NONE),
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
        NodeData::Scope { scope_type, component } => {
            info.type_code = scope_type.code();
            info.aux_str = component.0;
        }
        NodeData::Var { var_type, direction, signal, declares } => {
            info.type_code = var_type.code();
            info.direction = *direction as u8;
            info.signal = signal.0;
            info.is_alias = declares.is_none() as c_int;
        }
        NodeData::Stream { kind } => info.aux_str = kind.0,
        NodeData::Generator => {}
        NodeData::EnumTable { entries } => info.entry_count = entries.len() as u32,
    }
    *out = info;
    VTR_OK
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_node_attr(r: *const vtr_reader, id: u32, i: u32, key_out: *mut u32, v_out: *mut vtr_value) -> c_int {
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
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_enum_entry(r: *const vtr_reader, id: u32, i: u32, literal_out: *mut u32, value_out: *mut u32) -> c_int {
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
}

/// Copies up to `cap` child ids of `id` (or roots when `id == VTR_NONE`) and returns the total count.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_children(r: *const vtr_reader, id: u32, out: *mut u32, cap: usize) -> usize {
    let r = match r.as_ref() {
        Some(r) => r,
        None => return 0,
    };
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
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_signal_count(r: *const vtr_reader) -> u32 {
    r.as_ref().map(|r| r.0.signal_count()).unwrap_or(0)
}

/// `kind_out`: 0 bits, 1 real, 2 varlen.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_signal_kind(r: *const vtr_reader, sig: u32, kind_out: *mut u8, width_out: *mut u32, states_out: *mut u8) -> c_int {
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
}

/// Node that first declared `sig`.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_signal_var(r: *const vtr_reader, sig: u32) -> u32 {
    r.as_ref().and_then(|r| r.0.hierarchy().signal_var.get(sig as usize).map(|n| n.0)).unwrap_or(VTR_NONE)
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_find_signal(r: *const vtr_reader, path: *const c_char, sep: c_char, sig_out: *mut u32) -> c_int {
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
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_find_node(r: *const vtr_reader, path: *const c_char, sep: c_char, node_out: *mut u32) -> c_int {
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
}

/// Signal value view. `data` points into the owning object.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct vtr_signal_value {
    /// 0 bits, 1 real, 2 varlen
    pub kind: u8,
    /// Packing of `data` (2, 4 or 9) for bits.
    pub states: u8,
    pub width: u32,
    pub real: f64,
    pub data: *const u8,
    pub len: usize,
}

fn sv_from(v: vtr::SignalValue<'_>) -> vtr_signal_value {
    match v {
        vtr::SignalValue::Bits { width, states, data } => vtr_signal_value { kind: 0, states, width, real: 0.0, data: data.as_ptr(), len: data.len() },
        vtr::SignalValue::Real(f) => vtr_signal_value { kind: 1, states: 0, width: 64, real: f, data: ptr::null(), len: 0 },
        vtr::SignalValue::VarLen(b) => vtr_signal_value { kind: 2, states: 0, width: 0, real: 0.0, data: b.as_ptr(), len: b.len() },
    }
}

/// Owned value buffer used by point queries.
pub struct vtr_value_buf {
    v: vtr::OwnedSignalValue,
    ascii: Vec<u8>,
}

#[no_mangle]
pub extern "C" fn vtr_value_buf_new() -> *mut vtr_value_buf {
    Box::into_raw(Box::new(vtr_value_buf { v: vtr::OwnedSignalValue::Real(0.0), ascii: Vec::new() }))
}

#[no_mangle]
pub unsafe extern "C" fn vtr_value_buf_free(b: *mut vtr_value_buf) {
    if !b.is_null() {
        drop(Box::from_raw(b));
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_value_buf_get(b: *const vtr_value_buf, out: *mut vtr_signal_value) -> c_int {
    let b = need_ref!(b);
    let out = need!(out);
    *out = sv_from(b.v.borrow());
    VTR_OK
}

/// ASCII rendering (bit string MSB first / decimal real / raw bytes), NUL-terminated,
/// valid until the next call on the same buffer.
#[no_mangle]
pub unsafe extern "C" fn vtr_value_buf_ascii(b: *mut vtr_value_buf) -> *const c_char {
    let b = match b.as_mut() {
        Some(b) => b,
        None => return ptr::null(),
    };
    b.ascii.clear();
    b.ascii.extend_from_slice(b.v.to_ascii().as_bytes());
    b.ascii.push(0);
    b.ascii.as_ptr() as *const c_char
}

/// Value of `sig` at `time` into `buf`.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_value_at(r: *const vtr_reader, sig: u32, time: u64, buf: *mut vtr_value_buf) -> c_int {
    let r = need_ref!(r);
    let b = need!(buf);
    match r.0.value_at(SignalId(sig), time) {
        Ok(v) => {
            b.v = v;
            VTR_OK
        }
        Err(e) => status(Err(e)),
    }
}

pub type vtr_change_cb = Option<unsafe extern "C" fn(user: *mut std::ffi::c_void, time: u64, sig: u32, value: *const vtr_signal_value) -> c_int>;

/// Calls `cb` for every change of `sig` in `[t0, t1]`; a non-zero return stops iteration.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_changes(r: *const vtr_reader, sig: u32, t0: u64, t1: u64, cb: vtr_change_cb, user: *mut std::ffi::c_void) -> c_int {
    let r = need_ref!(r);
    let cb = match cb {
        Some(c) => c,
        None => return VTR_ERR_NULL,
    };
    match r.0.changes(SignalId(sig), t0, t1) {
        Ok(list) => {
            for (t, v) in &list {
                let sv = sv_from(v.borrow());
                if cb(user, *t, sig, &sv) != 0 {
                    break;
                }
            }
            VTR_OK
        }
        Err(e) => status(Err(e)),
    }
}

/// Calls `cb` for every change of every signal in `[t0, t1]` in time order.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_for_each_change(r: *const vtr_reader, t0: u64, t1: u64, cb: vtr_change_cb, user: *mut std::ffi::c_void) -> c_int {
    let r = need_ref!(r);
    let cb = match cb {
        Some(c) => c,
        None => return VTR_ERR_NULL,
    };
    let mut stop = false;
    let res = r.0.for_each_change(t0, t1, |t, s, v| {
        if stop {
            return;
        }
        let sv = sv_from(v);
        if cb(user, t, s.0, &sv) != 0 {
            stop = true;
        }
    });
    status(res)
}

pub struct vtr_signal_data(vtr::SignalData);

/// Loads all changes of a signal. Free with `vtr_signal_data_free`.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_load_signal(r: *const vtr_reader, sig: u32) -> *mut vtr_signal_data {
    let r = match r.as_ref() {
        Some(r) => r,
        None => return ptr::null_mut(),
    };
    match r.0.load_signal(SignalId(sig)) {
        Ok(d) => Box::into_raw(Box::new(vtr_signal_data(d))),
        Err(e) => {
            set_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

/// Loads `n` histories in request order into a caller-allocated array of handles.
/// Repeated signal IDs share immutable storage. On error, `out` is unchanged.
/// For `n == 0`, `sigs` and `out` may be NULL. Free each handle separately.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_load_signals(r: *const vtr_reader, sigs: *const u32, n: usize, out: *mut *mut vtr_signal_data) -> c_int {
    let r = need_ref!(r);
    if n == 0 {
        return VTR_OK;
    }
    if sigs.is_null() || out.is_null() {
        return VTR_ERR_NULL;
    }
    let sigs: Vec<_> = std::slice::from_raw_parts(sigs, n).iter().map(|&s| SignalId(s)).collect();
    match r.0.load_signals(&sigs) {
        Ok(loaded) => {
            for (i, d) in loaded.into_iter().enumerate() {
                out.add(i).write(Box::into_raw(Box::new(vtr_signal_data(d))));
            }
            VTR_OK
        }
        Err(e) => status(Err(e)),
    }
}

/// Clones a handle without copying its immutable waveform storage. NULL in, NULL out.
#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_clone(d: *const vtr_signal_data) -> *mut vtr_signal_data {
    d.as_ref().map(|d| Box::into_raw(Box::new(vtr_signal_data(d.0.clone())))).unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_free(d: *mut vtr_signal_data) {
    if !d.is_null() {
        drop(Box::from_raw(d));
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_len(d: *const vtr_signal_data) -> usize {
    d.as_ref().map(|d| d.0.len()).unwrap_or(0)
}

/// Pointer to the change-time array (`len` entries).
#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_times(d: *const vtr_signal_data) -> *const u64 {
    d.as_ref().map(|d| d.0.times().as_ptr()).unwrap_or(ptr::null())
}

#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_get(d: *const vtr_signal_data, i: usize, out: *mut vtr_signal_value) -> c_int {
    let d = need_ref!(d);
    let out = need!(out);
    if i >= d.0.len() {
        return VTR_ERR_NOT_FOUND;
    }
    *out = sv_from(d.0.get(i));
    VTR_OK
}

#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_initial(d: *const vtr_signal_data, out: *mut vtr_signal_value) -> c_int {
    let d = need_ref!(d);
    let out = need!(out);
    *out = sv_from(d.0.initial());
    VTR_OK
}

/// Index of the last change at or before `time`, or SIZE_MAX.
#[no_mangle]
pub unsafe extern "C" fn vtr_signal_data_index_at(d: *const vtr_signal_data, time: u64) -> usize {
    d.as_ref().and_then(|d| d.0.index_at(time)).unwrap_or(usize::MAX)
}

/// Global sorted time table; pointer valid while the reader is alive.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_time_table(r: *const vtr_reader, len_out: *mut usize) -> *const u64 {
    let r = match r.as_ref() {
        Some(r) => r,
        None => return ptr::null(),
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
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_blackout(r: *const vtr_reader, i: u32, time_out: *mut u64, active_out: *mut c_int) -> c_int {
    let r = need_ref!(r);
    match r.0.blackout().get(i as usize) {
        Some(b) => {
            if let Some(o) = time_out.as_mut() {
                *o = b.time;
            }
            if let Some(o) = active_out.as_mut() {
                *o = b.active as c_int;
            }
            VTR_OK
        }
        None => VTR_ERR_NOT_FOUND,
    }
}

// transactions

pub struct vtr_tx(Transaction);

#[repr(C)]
pub struct vtr_tx_info {
    pub id: u64,
    pub generator: u32,
    pub stream: u32,
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

#[no_mangle]
pub unsafe extern "C" fn vtr_tx_get(r: *const vtr_reader, tx: *const vtr_tx, out: *mut vtr_tx_info) -> c_int {
    let r = need_ref!(r);
    let t = &need_ref!(tx).0;
    let out = need!(out);
    *out = vtr_tx_info {
        id: t.id,
        generator: t.generator.0,
        stream: r.0.generator_stream(t.generator).map(|s| s.0).unwrap_or(VTR_NONE),
        begin: t.begin,
        end: t.end,
        status: t.status as u8,
        kind: t.kind as u8,
        has_parent: t.parent.is_some() as c_int,
        parent: t.parent.unwrap_or(0),
        attr_count: t.attrs.len() as u32,
        event_count: t.events.len() as u32,
        stage_count: t.stages.len() as u32,
    };
    VTR_OK
}

#[no_mangle]
pub unsafe extern "C" fn vtr_tx_attr(tx: *const vtr_tx, i: u32, key_out: *mut u32, phase_out: *mut u8, v_out: *mut vtr_value) -> c_int {
    let t = &need_ref!(tx).0;
    match t.attrs.get(i as usize) {
        Some(a) => {
            if let Some(o) = key_out.as_mut() {
                *o = a.key.0;
            }
            if let Some(o) = phase_out.as_mut() {
                *o = a.phase as u8;
            }
            if let Some(o) = v_out.as_mut() {
                *o = from_value(&a.value);
            }
            VTR_OK
        }
        None => VTR_ERR_NOT_FOUND,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_tx_event(tx: *const vtr_tx, i: u32, time_out: *mut u64, name_out: *mut u32, attr_count_out: *mut u32) -> c_int {
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
}

#[no_mangle]
pub unsafe extern "C" fn vtr_tx_event_attr(tx: *const vtr_tx, i: u32, j: u32, key_out: *mut u32, v_out: *mut vtr_value) -> c_int {
    let t = &need_ref!(tx).0;
    match t.events.get(i as usize).and_then(|e| e.attrs.get(j as usize)) {
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
}

#[no_mangle]
pub unsafe extern "C" fn vtr_tx_stage(tx: *const vtr_tx, i: u32, name_out: *mut u32, lane_out: *mut u32, begin_out: *mut u64, end_out: *mut u64, has_end_out: *mut c_int, attr_count_out: *mut u32) -> c_int {
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
                *o = s.end.is_some() as c_int;
            }
            if let Some(o) = attr_count_out.as_mut() {
                *o = s.attrs.len() as u32;
            }
            VTR_OK
        }
        None => VTR_ERR_NOT_FOUND,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_tx_stage_attr(tx: *const vtr_tx, i: u32, j: u32, key_out: *mut u32, v_out: *mut vtr_value) -> c_int {
    let t = &need_ref!(tx).0;
    match t.stages.get(i as usize).and_then(|s| s.attrs.get(j as usize)) {
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
}

pub type vtr_tx_cb = Option<unsafe extern "C" fn(user: *mut std::ffi::c_void, tx: *const vtr_tx) -> c_int>;

/// Visits transactions. `generator`/`stream` may be VTR_NONE; `t1 == 0` disables the window.
/// A non-zero callback return stops iteration.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_visit_transactions(r: *const vtr_reader, generator: u32, stream: u32, t0: u64, t1: u64, cb: vtr_tx_cb, user: *mut std::ffi::c_void) -> c_int {
    let r = need_ref!(r);
    let cb = match cb {
        Some(c) => c,
        None => return VTR_ERR_NULL,
    };
    let q = TxQuery {
        generator: if generator == VTR_NONE { None } else { Some(NodeId(generator)) },
        stream: if stream == VTR_NONE { None } else { Some(NodeId(stream)) },
        window: if t1 == 0 { None } else { Some((t0, t1)) },
    };
    let res = r.0.visit_transactions(&q, |t| {
        let h = vtr_tx(t.clone());
        cb(user, &h) == 0
    });
    status(res)
}

/// Looks up one transaction; the callback is invoked once if found.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_transaction(r: *const vtr_reader, id: u64, cb: vtr_tx_cb, user: *mut std::ffi::c_void) -> c_int {
    let r = need_ref!(r);
    let cb = match cb {
        Some(c) => c,
        None => return VTR_ERR_NULL,
    };
    match r.0.transaction(id) {
        Ok(Some(t)) => {
            let h = vtr_tx(t);
            cb(user, &h);
            VTR_OK
        }
        Ok(None) => VTR_ERR_NOT_FOUND,
        Err(e) => status(Err(e)),
    }
}

pub type vtr_relation_cb = Option<unsafe extern "C" fn(user: *mut std::ffi::c_void, kind: u32, from: u64, to: u64, n_attrs: u32, keys: *const u32, values: *const vtr_value) -> c_int>;

unsafe fn emit_relations(list: Vec<vtr::Relation>, cb: unsafe extern "C" fn(*mut std::ffi::c_void, u32, u64, u64, u32, *const u32, *const vtr_value) -> c_int, user: *mut std::ffi::c_void) {
    for rel in &list {
        let keys: Vec<u32> = rel.attrs.iter().map(|(k, _)| k.0).collect();
        let vals: Vec<vtr_value> = rel.attrs.iter().map(|(_, v)| from_value(v)).collect();
        if cb(user, rel.kind.0, rel.from, rel.to, keys.len() as u32, keys.as_ptr(), vals.as_ptr()) != 0 {
            break;
        }
    }
}

/// `direction`: 0 = relations from `id`, 1 = relations to `id`.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_relations(r: *const vtr_reader, id: u64, direction: c_int, cb: vtr_relation_cb, user: *mut std::ffi::c_void) -> c_int {
    let r = need_ref!(r);
    let cb = match cb {
        Some(c) => c,
        None => return VTR_ERR_NULL,
    };
    let res = if direction == 0 { r.0.relations_from(id) } else { r.0.relations_to(id) };
    match res {
        Ok(list) => {
            emit_relations(list, cb, user);
            VTR_OK
        }
        Err(e) => status(Err(e)),
    }
}


// ---------------------------------------------------------------------------
// Reader: logs
// ---------------------------------------------------------------------------

/// Static description of a log site.
#[repr(C)]
pub struct vtr_log_site_info {
    pub node: u32,
    pub stream: u32,
    pub severity: u8,
    /// String id of the format string.
    pub fmt: u32,
    /// String ids (VTR_NONE when absent).
    pub file: u32,
    pub func: u32,
    pub line: u32,
    pub arg_count: u32,
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_log_site_count(r: *const vtr_reader) -> u32 {
    match r.as_ref() {
        Some(r) => r.0.log_sites().len() as u32,
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_log_count(r: *const vtr_reader) -> u64 {
    match r.as_ref() {
        Some(r) => r.0.log_count(),
        None => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn vtr_reader_log_site(r: *const vtr_reader, i: u32, out: *mut vtr_log_site_info) -> c_int {
    let r = need_ref!(r);
    let out = need!(out);
    match r.0.log_sites().get(i as usize) {
        Some(s) => {
            *out = vtr_log_site_info {
                node: s.node.0,
                stream: s.stream.0,
                severity: s.severity.code(),
                fmt: s.fmt.0,
                file: s.file.map(|x| x.0).unwrap_or(VTR_NONE),
                func: s.func.map(|x| x.0).unwrap_or(VTR_NONE),
                line: s.line.unwrap_or(0),
                arg_count: s.args.len() as u32,
            };
            VTR_OK
        }
        None => VTR_ERR_NOT_FOUND,
    }
}

/// Type (VTR_VAL_*) and name (string id) of argument `j` of site `i`.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_log_site_arg(r: *const vtr_reader, i: u32, j: u32, type_out: *mut u8, name_out: *mut u32) -> c_int {
    let r = need_ref!(r);
    match r.0.log_sites().get(i as usize) {
        Some(s) if (j as usize) < s.args.len() => {
            if let Some(o) = type_out.as_mut() {
                *o = s.args[j as usize] as u8;
            }
            if let Some(o) = name_out.as_mut() {
                *o = s.names.get(j as usize).map(|n| n.0).unwrap_or(VTR_NONE);
            }
            VTR_OK
        }
        _ => VTR_ERR_NOT_FOUND,
    }
}

/// Site index of generator `gen`, or VTR_NONE when it is not a log site.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_log_site_of(r: *const vtr_reader, gen: u32) -> u32 {
    match r.as_ref() {
        Some(r) => r.0.log_site(NodeId(gen)).map(|s| s.index).unwrap_or(VTR_NONE),
        None => VTR_NONE,
    }
}

/// One log record, valid only inside the visit callback.
#[repr(C)]
pub struct vtr_log_rec {
    pub id: u64,
    pub time: u64,
    /// Parent transaction id, 0 = none.
    pub parent: u64,
    /// Site index (see `vtr_reader_log_site`).
    pub site: u32,
    pub generator: u32,
    pub stream: u32,
    pub severity: u8,
    pub arg_count: u32,
    inner: *const std::ffi::c_void,
}

pub type vtr_log_cb = Option<unsafe extern "C" fn(user: *mut std::ffi::c_void, rec: *const vtr_log_rec) -> c_int>;

/// Visits log records: `stream` / `generator` may be VTR_NONE, `min_severity`
/// 0 = all, `t1 == 0` = no time window. The callback returns non-zero to stop.
#[no_mangle]
pub unsafe extern "C" fn vtr_reader_visit_log(r: *const vtr_reader, stream: u32, generator: u32, min_severity: u8, t0: u64, t1: u64, cb: vtr_log_cb, user: *mut std::ffi::c_void) -> c_int {
    let r = need_ref!(r);
    let cb = match cb {
        Some(c) => c,
        None => return VTR_ERR_NULL,
    };
    let q = LogQuery {
        stream: if stream == VTR_NONE { None } else { Some(NodeId(stream)) },
        generator: if generator == VTR_NONE { None } else { Some(NodeId(generator)) },
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
            stream: rec.site.stream.0,
            severity: rec.site.severity.code(),
            arg_count: rec.arg_count() as u32,
            inner: rec as *const LogRecord as *const std::ffi::c_void,
        };
        cb(user, &h) == 0
    });
    status(res)
}

/// Argument `i` of a record. Text arguments come back as VTR_VAL_TEXT with
/// `data`/`len` pointing into the reader (valid while the reader is alive).
#[no_mangle]
pub unsafe extern "C" fn vtr_log_rec_arg(rec: *const vtr_log_rec, i: u32, v_out: *mut vtr_value) -> c_int {
    let rec = need_ref!(rec);
    let out = need!(v_out);
    let inner = &*(rec.inner as *const LogRecord);
    match inner.arg(i as usize) {
        Some(a) => {
            let mut o = vtr_value { tag: a.arg_type() as u8, ..Default::default() };
            match a {
                LogArg::Bool(b) => o.b = b as u8,
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
}

thread_local! {
    static FMT_BUF: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Renders the message of `rec` into `buf` (NUL-terminated when `cap > 0`,
/// truncated if it does not fit). Returns the full message length, like
/// `snprintf`. `buf` may be NULL with `cap == 0` to query the length.
#[no_mangle]
pub unsafe extern "C" fn vtr_log_rec_format(r: *const vtr_reader, rec: *const vtr_log_rec, buf: *mut c_char, cap: usize) -> usize {
    let (r, rec) = match (r.as_ref(), rec.as_ref()) {
        (Some(r), Some(rec)) => (r, rec),
        _ => return 0,
    };
    let inner = &*(rec.inner as *const LogRecord);
    FMT_BUF.with(|s| {
        let mut s = s.borrow_mut();
        s.clear();
        inner.format_into(r.0.strings(), &mut s);
        let n = s.len();
        if cap > 0 && !buf.is_null() {
            let m = n.min(cap - 1);
            ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, m);
            *buf.add(m) = 0;
        }
        n
    })
}
