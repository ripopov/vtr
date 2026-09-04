//! Streaming writer.
//!
//! The writer is single-threaded from the caller's point of view. Encoding and
//! compression of finished blocks run on a background thread by default so the
//! simulator thread only pays for logging.

use crate::block::{self, BlockInput, ChunkEnc, ChunkInput, EncoderScratch, Record, COMPACT_FLAG, NO_BLOCK};
use crate::codec::{Compression, Compressor};
use crate::container::{self, DirEntry, SectionKind};
use crate::error::{Error, Result};
use crate::hierarchy::{Direction, Node, NodeData, NodeId, ScopeType, SignalId, SignalKind, VarType};
use crate::sections::{self, Blackout, FileType, Meta};
use crate::signal;
use crate::strings::{Interner, StrId};
use crate::txblock::{self, AttrPhase, TxBlockInput, TxId, TxKind, TxStatus};
use crate::value::{packed_len, Value};
use crate::varint;
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

/// Writer configuration.
#[derive(Clone, Debug)]
pub struct WriterOptions {
    /// Codec and level for all compressed payloads.
    pub compression: Compression,
    /// Signals per value-change group. Larger groups compress better; smaller
    /// groups make single-signal reads cheaper. Fixed for the file's lifetime.
    pub group_size: u32,
    /// Value changes per signal block (the compression unit).
    pub block_records: usize,
    /// Value changes handed to the background encoder at a time (the pipelining unit).
    pub chunk_records: usize,
    /// Raw bytes per independently compressed column run inside a group.
    /// Bounds how much must be decompressed to read one signal in one block.
    pub run_bytes: usize,
    /// Row bytes buffered before a transaction block is emitted.
    pub tx_block_bytes: usize,
    /// Encode and compress on a background thread.
    pub background: bool,
    /// Drop value changes that do not change the value.
    pub dedup: bool,
    /// Store a CRC32 for every section.
    pub checksums: bool,
}

impl Default for WriterOptions {
    fn default() -> Self {
        WriterOptions {
            compression: Compression::ZSTD_DEFAULT,
            group_size: 256,
            block_records: 1 << 24,
            chunk_records: 1 << 19,
            run_bytes: 64 << 10,
            tx_block_bytes: 4 << 20,
            background: true,
            dedup: true,
            checksums: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Sink: owns the output file, encodes and writes sections
// ---------------------------------------------------------------------------

enum Msg {
    Section { kind: SectionKind, payload: Vec<u8>, aux0: u64, aux1: u64 },
    Chunk(Box<ChunkInput>),
    Signal(Box<BlockInput>),
    Tx(Box<TxBlockInput>),
    Close,
}

/// Buffers returned by the sink for reuse.
enum Recycled {
    Chunk(Box<ChunkInput>),
    Block(Box<BlockInput>),
}

struct FileSink {
    file: BufWriter<File>,
    offset: u64,
    entries: Vec<DirEntry>,
    comp: Compression,
    checksums: bool,
    compressor: Compressor,
    scratch: EncoderScratch,
    buf: Vec<u8>,
    recycle: Option<SyncSender<Recycled>>,
    chunks: Vec<ChunkEnc>,
    n_chunks: usize,
    chunk_pool: Vec<ChunkEnc>,
}

impl FileSink {
    fn write_section(&mut self, kind: SectionKind, payload: &[u8], aux0: u64, aux1: u64) -> Result<()> {
        self.write_section_parts(kind, &[payload], aux0, aux1)
    }

    /// Writes a section whose payload is the concatenation of `parts`.
    fn write_section_parts(&mut self, kind: SectionKind, parts: &[&[u8]], aux0: u64, aux1: u64) -> Result<()> {
        let len: usize = parts.iter().map(|p| p.len()).sum();
        let crc = if self.checksums {
            let mut h = crc32fast::Hasher::new();
            for p in parts {
                h.update(p);
            }
            h.finalize()
        } else {
            0
        };
        let header = container::encode_section_header(kind as u32, 0, len as u64, crc);
        self.file.write_all(&header)?;
        for p in parts {
            self.file.write_all(p)?;
        }
        self.entries.push(DirEntry { kind: kind as u32, flags: 0, offset: self.offset, len: len as u64, aux0, aux1 });
        self.offset += (container::SECTION_HEADER_LEN + len) as u64;
        Ok(())
    }

    fn handle(&mut self, msg: Msg) -> Result<bool> {
        match msg {
            Msg::Section { kind, payload, aux0, aux1 } => {
                if matches!(kind, SectionKind::Strings | SectionKind::Hierarchy) {
                    // These sections are stored as one compressed blob.
                    self.buf.clear();
                    self.compressor.compress_into(self.comp, &payload, &mut self.buf)?;
                    let blob = std::mem::take(&mut self.buf);
                    self.write_section(kind, &blob, aux0, aux1)?;
                    self.buf = blob;
                } else {
                    self.write_section(kind, &payload, aux0, aux1)?;
                }
            }
            Msg::Chunk(input) => {
                if self.n_chunks == 0 {
                    self.scratch.begin_block();
                }
                let mut enc = self.chunk_pool.pop().unwrap_or_default();
                block::encode_chunk(&input, &input.kinds, &mut self.scratch, &mut enc)?;
                if self.n_chunks < self.chunks.len() {
                    let old = std::mem::replace(&mut self.chunks[self.n_chunks], enc);
                    self.chunk_pool.push(old);
                } else {
                    self.chunks.push(enc);
                }
                self.n_chunks += 1;
                if let Some(r) = &self.recycle {
                    let _ = r.try_send(Recycled::Chunk(input));
                }
            }
            Msg::Signal(input) => {
                if self.n_chunks == 0 {
                    self.scratch.begin_block();
                }
                self.buf.clear();
                block::finish_block(&input, &self.chunks[..self.n_chunks], self.comp, &mut self.compressor, &mut self.scratch, &mut self.buf)?;
                self.n_chunks = 0;
                let (a, b) = match (input.times.first(), input.times.last()) {
                    (Some(&a), Some(&b)) => (a, b),
                    _ => (0, 0),
                };
                let header = std::mem::take(&mut self.buf);
                let data = std::mem::take(self.scratch.data_mut());
                self.write_section_parts(SectionKind::SignalBlock, &[&header, &data], a, b)?;
                *self.scratch.data_mut() = data;
                self.buf = header;
                if let Some(r) = &self.recycle {
                    let _ = r.try_send(Recycled::Block(input));
                }
            }
            Msg::Tx(input) => {
                self.buf.clear();
                txblock::encode_tx_block(&input, self.comp, &mut self.compressor, &mut self.buf)?;
                let h = txblock::TxBlockHeader::parse(&self.buf)?;
                let payload = std::mem::take(&mut self.buf);
                self.write_section(SectionKind::TxBlock, &payload, h.t_min, h.t_max)?;
                self.buf = payload;
            }
            Msg::Close => {
                let dir = container::encode_directory(&self.entries);
                let dir_off = self.offset;
                let header = container::encode_section_header(SectionKind::Directory as u32, 0, dir.len() as u64, 0);
                self.file.write_all(&header)?;
                self.file.write_all(&dir)?;
                self.offset += (container::SECTION_HEADER_LEN + dir.len()) as u64;
                let file_len = self.offset + container::TRAILER_LEN as u64;
                self.file.write_all(&container::encode_trailer(dir_off, file_len))?;
                self.file.flush()?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

enum Sink {
    Inline(Box<FileSink>),
    Threaded {
        tx: Option<SyncSender<Msg>>,
        recycle: Receiver<Recycled>,
        handle: Option<std::thread::JoinHandle<Result<()>>>,
        failed: Arc<AtomicBool>,
        error: Arc<Mutex<Option<Error>>>,
    },
}

impl Sink {
    fn send(&mut self, msg: Msg) -> Result<()> {
        match self {
            Sink::Inline(s) => {
                s.handle(msg)?;
                Ok(())
            }
            Sink::Threaded { tx, failed, error, .. } => {
                if failed.load(Ordering::Relaxed) {
                    return Err(error.lock().unwrap().take().unwrap_or(Error::State("background writer failed")));
                }
                if let Some(tx) = tx {
                    if tx.send(msg).is_err() {
                        return Err(error.lock().unwrap().take().unwrap_or(Error::State("background writer stopped")));
                    }
                }
                Ok(())
            }
        }
    }

    fn take_recycled(&mut self, chunk_pool: &mut Vec<ChunkInput>, block_pool: &mut Vec<BlockInput>) {
        if let Sink::Threaded { recycle, .. } = self {
            while let Ok(r) = recycle.try_recv() {
                match r {
                    Recycled::Chunk(c) => chunk_pool.push(*c),
                    Recycled::Block(b) => block_pool.push(*b),
                }
            }
        }
    }

    fn close(&mut self) -> Result<()> {
        match self {
            Sink::Inline(s) => {
                s.handle(Msg::Close)?;
                Ok(())
            }
            Sink::Threaded { tx, handle, error, .. } => {
                if let Some(t) = tx.take() {
                    let _ = t.send(Msg::Close);
                }
                if let Some(h) = handle.take() {
                    match h.join() {
                        Ok(r) => r?,
                        Err(_) => return Err(Error::State("background writer panicked")),
                    }
                }
                if let Some(e) = error.lock().unwrap().take() {
                    return Err(e);
                }
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Open transactions
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StageRec {
    name: u32,
    lane: u32,
    begin: u64,
    end: u64, // u64::MAX = open
    attrs: Vec<u8>,
    n_attrs: u32,
}

#[derive(Default)]
struct OpenTx {
    id: u64, // 0 = free slot
    gen: u32,
    begin: u64,
    kind: u8,
    parent: u64,
    attrs: Vec<u8>,
    n_attrs: u32,
    events: Vec<u8>,
    n_events: u32,
    stages: Vec<StageRec>,
}

impl OpenTx {
    fn reset(&mut self) {
        self.id = 0;
        self.kind = 0;
        self.parent = 0;
        self.attrs.clear();
        self.n_attrs = 0;
        self.events.clear();
        self.n_events = 0;
        self.stages.clear();
    }

    fn write_row(&self, end: u64, status: TxStatus, out: &mut Vec<u8>) {
        varint::put_u64(out, self.id);
        varint::put_u64(out, self.gen as u64);
        varint::put_u64(out, self.begin);
        varint::put_u64(out, end.wrapping_sub(self.begin));
        out.push(status as u8);
        out.push(self.kind);
        varint::put_u64(out, self.parent);
        varint::put_u64(out, self.n_attrs as u64);
        out.extend_from_slice(&self.attrs);
        varint::put_u64(out, self.n_events as u64);
        out.extend_from_slice(&self.events);
        varint::put_u64(out, self.stages.len() as u64);
        for s in &self.stages {
            varint::put_u64(out, s.name as u64);
            varint::put_u64(out, s.lane as u64);
            varint::put_u64(out, s.begin);
            let e = if s.end == u64::MAX { end.max(s.begin) } else { s.end };
            varint::put_u64(out, e.wrapping_sub(s.begin) + 1);
            varint::put_u64(out, s.n_attrs as u64);
            out.extend_from_slice(&s.attrs);
        }
    }
}

/// Ring-addressed table of open transactions (ids are dense and increasing).
struct OpenTable {
    slots: Vec<OpenTx>,
    mask: usize,
    live: usize,
}

impl OpenTable {
    fn new() -> Self {
        let cap = 1024;
        OpenTable { slots: (0..cap).map(|_| OpenTx::default()).collect(), mask: cap - 1, live: 0 }
    }

    fn insert(&mut self, id: u64) -> &mut OpenTx {
        loop {
            let i = id as usize & self.mask;
            if self.slots[i].id == 0 {
                self.slots[i].id = id;
                self.live += 1;
                return &mut self.slots[i];
            }
            self.grow();
        }
    }

    fn grow(&mut self) {
        let new_cap = self.slots.len() * 2;
        let mut slots: Vec<OpenTx> = (0..new_cap).map(|_| OpenTx::default()).collect();
        let mask = new_cap - 1;
        for s in self.slots.drain(..) {
            if s.id != 0 {
                let i = s.id as usize & mask;
                slots[i] = s;
            }
        }
        self.slots = slots;
        self.mask = mask;
    }

    fn get(&mut self, id: u64) -> Result<&mut OpenTx> {
        let i = id as usize & self.mask;
        if id != 0 && self.slots[i].id == id {
            Ok(&mut self.slots[i])
        } else {
            Err(Error::Invalid(format!("transaction {id} is not open")))
        }
    }

    fn remove(&mut self, id: u64) {
        let i = id as usize & self.mask;
        if self.slots[i].id == id {
            self.slots[i].reset();
            self.live -= 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct LastVal {
    payload: u64,
    epoch: u32,
    compact: bool,
}

/// Hot per-signal facts (kept small and separate from `kinds`).
#[derive(Clone, Copy)]
struct SigInfo {
    /// 0 bits, 1 real, 2 varlen
    kind: u8,
    narrow: bool,
    width: u32,
}

/// Streaming VTR writer.
pub struct Writer {
    opts: WriterOptions,
    sink: Sink,
    meta: Meta,
    meta_written: bool,
    strings: Interner,
    pending_nodes: Vec<Node>,
    n_nodes: u32,
    scope_stack: Vec<NodeId>,
    // signals
    kinds: Vec<SignalKind>,
    kinds_arc: Option<Arc<Vec<SignalKind>>>,
    info: Vec<SigInfo>,
    group_shift: u32,
    sig_counts: Vec<u32>,
    last: Vec<LastVal>,
    wide_last: Vec<u8>,
    /// Per wide signal (index = `LastVal::payload`): byte offset of its slot in `wide_last`.
    wide_off: Vec<usize>,
    /// Per wide signal: signal id, column bytes for the current block, last time index, entry count.
    wide_sig: Vec<u32>,
    wide_hdr: Vec<Vec<u8>>,
    wide_val: Vec<Vec<u8>>,
    wide_last_tidx: Vec<u32>,
    wide_count: Vec<u32>,
    wide_pool: Vec<Vec<u8>>,
    wide_bytes: usize,
    wide_entries: u64,
    varlen_last: Vec<Vec<u8>>,
    frame_cap: Vec<(u64, bool)>,
    frame_heap: Vec<u8>,
    epoch: u32,
    time: u64,
    cur_tidx: u32,
    times: Vec<u64>,
    records: Vec<Record>,
    heap: Vec<u8>,
    dirty_bits: Vec<u64>,
    last_dirty_block: Vec<u32>,
    block_count: u32,
    /// Records of the current block already handed to the encoder as chunks.
    block_pending: usize,
    /// Records to buffer before the next chunk hand-off (bounded by the block budget).
    chunk_limit: usize,
    chunk_pool: Vec<ChunkInput>,
    block_pool: Vec<BlockInput>,
    total_records: u64,
    // transactions
    open: OpenTable,
    next_tx_id: u64,
    tx_rows: Vec<u8>,
    n_tx_rows: u64,
    rel_rows: Vec<u8>,
    n_rel_rows: u64,
    total_tx: u64,
    blackout: Vec<Blackout>,
    closed: bool,
    scratch: Vec<u8>,
}

impl Writer {
    /// Creates a new trace file with default options.
    pub fn create(path: impl AsRef<Path>) -> Result<Writer> {
        Self::create_with(path, WriterOptions::default())
    }

    pub fn create_with(path: impl AsRef<Path>, opts: WriterOptions) -> Result<Writer> {
        let mut file = File::create(path)?;
        container::write_file_header(&mut file)?;
        let offset = file.stream_position()?;
        let comp = opts.compression;
        let (recycle_tx, recycle_rx) = sync_channel(16);
        let mut fsink = FileSink {
            file: BufWriter::with_capacity(1 << 20, file),
            offset,
            entries: Vec::new(),
            comp,
            checksums: opts.checksums,
            compressor: Compressor::new(),
            scratch: EncoderScratch::default(),
            buf: Vec::new(),
            recycle: None,
            chunks: Vec::new(),
            n_chunks: 0,
            chunk_pool: Vec::new(),
        };
        let sink = if opts.background {
            fsink.recycle = Some(recycle_tx);
            let (tx, rx) = sync_channel::<Msg>(4);
            let failed = Arc::new(AtomicBool::new(false));
            let error = Arc::new(Mutex::new(None));
            let (f2, e2) = (failed.clone(), error.clone());
            let handle = std::thread::Builder::new()
                .name("vtr-writer".into())
                .spawn(move || {
                    let mut fsink = fsink;
                    while let Ok(msg) = rx.recv() {
                        match fsink.handle(msg) {
                            Ok(true) => return Ok(()),
                            Ok(false) => {}
                            Err(e) => {
                                *e2.lock().unwrap() = Some(e);
                                f2.store(true, Ordering::Relaxed);
                                return Err(Error::State("background writer failed"));
                            }
                        }
                    }
                    Ok(())
                })
                .map_err(Error::Io)?;
            Sink::Threaded { tx: Some(tx), recycle: recycle_rx, handle: Some(handle), failed, error }
        } else {
            Sink::Inline(Box::new(fsink))
        };
        let mut meta = Meta::default();
        // Group size is a power of two so group lookups are shifts.
        let group_size = opts.group_size.max(1).next_power_of_two();
        meta.group_size = group_size;
        let mut opts = opts;
        opts.group_size = group_size;
        let record_cap = opts.chunk_records.min(opts.block_records).min(1 << 24);
        let chunk_limit = opts.chunk_records.min(opts.block_records).max(1);
        meta.writer = format!("vtr {}", env!("CARGO_PKG_VERSION"));
        Ok(Writer {
            opts,
            sink,
            meta,
            meta_written: false,
            strings: Interner::new(),
            pending_nodes: Vec::new(),
            n_nodes: 0,
            scope_stack: Vec::new(),
            kinds: Vec::new(),
            kinds_arc: None,
            info: Vec::new(),
            group_shift: group_size.trailing_zeros(),
            sig_counts: Vec::new(),
            last: Vec::new(),
            wide_last: Vec::new(),
            wide_off: Vec::new(),
            wide_sig: Vec::new(),
            wide_hdr: Vec::new(),
            wide_val: Vec::new(),
            wide_last_tidx: Vec::new(),
            wide_count: Vec::new(),
            wide_pool: Vec::new(),
            wide_bytes: 0,
            wide_entries: 0,
            varlen_last: Vec::new(),
            frame_cap: Vec::new(),
            frame_heap: Vec::new(),
            epoch: 1,
            time: 0,
            cur_tidx: 0,
            times: Vec::new(),
            records: Vec::with_capacity(record_cap),
            heap: Vec::new(),
            dirty_bits: Vec::new(),
            last_dirty_block: Vec::new(),
            block_count: 0,
            block_pending: 0,
            chunk_limit,
            chunk_pool: Vec::new(),
            block_pool: Vec::new(),
            total_records: 0,
            open: OpenTable::new(),
            next_tx_id: 1,
            tx_rows: Vec::new(),
            n_tx_rows: 0,
            rel_rows: Vec::new(),
            n_rel_rows: 0,
            total_tx: 0,
            blackout: Vec::new(),
            closed: false,
            scratch: Vec::new(),
        })
    }

    // ----- metadata -----

    fn meta_mut(&mut self) -> Result<&mut Meta> {
        if self.meta_written {
            return Err(Error::State("metadata must be set before the first flush"));
        }
        Ok(&mut self.meta)
    }
    /// Time unit as a power of ten seconds (`-9` = ns). Default `-9`.
    pub fn set_timescale(&mut self, exp: i8) -> Result<()> {
        self.meta_mut()?.timescale = exp;
        Ok(())
    }
    pub fn set_time_zero(&mut self, t: i64) -> Result<()> {
        self.meta_mut()?.time_zero = t;
        Ok(())
    }
    pub fn set_file_type(&mut self, ft: FileType) -> Result<()> {
        self.meta_mut()?.file_type = ft;
        Ok(())
    }
    pub fn set_writer_name(&mut self, s: &str) -> Result<()> {
        self.meta_mut()?.writer = s.to_string();
        Ok(())
    }
    pub fn set_date(&mut self, s: &str) -> Result<()> {
        self.meta_mut()?.date = s.to_string();
        Ok(())
    }
    pub fn set_comment(&mut self, s: &str) -> Result<()> {
        self.meta_mut()?.comment = s.to_string();
        Ok(())
    }
    /// File-level attribute.
    pub fn set_file_attr(&mut self, key: &str, value: Value) -> Result<()> {
        let k = self.strings.intern(key);
        self.meta_mut()?.attrs.push((k, value));
        Ok(())
    }

    /// Interns a string (attribute keys, event names, ...). Cheap when already interned.
    pub fn intern(&mut self, s: &str) -> StrId {
        self.strings.intern(s)
    }

    pub fn options(&self) -> &WriterOptions {
        &self.opts
    }

    /// Looks up an interned string.
    pub fn string(&self, id: StrId) -> &str {
        self.strings.get(id)
    }

    // ----- hierarchy -----

    fn push_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.n_nodes);
        self.n_nodes += 1;
        self.pending_nodes.push(node);
        id
    }

    /// Opens a scope; subsequent vars/scopes are children until [`end_scope`](Self::end_scope).
    pub fn begin_scope(&mut self, name: &str, scope_type: ScopeType, component: &str) -> NodeId {
        let name = self.strings.intern(name);
        let component = self.strings.intern(component);
        let parent = self.scope_stack.last().copied();
        let id = self.push_node(Node { parent, name, data: NodeData::Scope { scope_type, component }, attrs: Vec::new() });
        self.scope_stack.push(id);
        id
    }

    pub fn end_scope(&mut self) -> Result<()> {
        self.scope_stack.pop().map(|_| ()).ok_or(Error::State("end_scope without begin_scope"))
    }

    /// Current innermost scope.
    pub fn current_scope(&self) -> Option<NodeId> {
        self.scope_stack.last().copied()
    }

    /// Declares a variable with a new signal in the current scope.
    pub fn add_var(&mut self, name: &str, var_type: VarType, direction: Direction, kind: SignalKind) -> (NodeId, SignalId) {
        let parent = self.scope_stack.last().copied();
        self.add_var_in(parent, name, var_type, direction, kind)
    }

    /// Declares a variable with a new signal under an explicit parent.
    pub fn add_var_in(&mut self, parent: Option<NodeId>, name: &str, var_type: VarType, direction: Direction, kind: SignalKind) -> (NodeId, SignalId) {
        let name = self.strings.intern(name);
        let sig = self.new_signal(kind);
        let id = self.push_node(Node { parent, name, data: NodeData::Var { var_type, direction, signal: sig, declares: Some(kind) }, attrs: Vec::new() });
        (id, sig)
    }

    /// Declares a variable that aliases an existing signal.
    pub fn add_alias(&mut self, name: &str, var_type: VarType, direction: Direction, signal: SignalId) -> Result<NodeId> {
        let parent = self.scope_stack.last().copied();
        self.add_alias_in(parent, name, var_type, direction, signal)
    }

    pub fn add_alias_in(&mut self, parent: Option<NodeId>, name: &str, var_type: VarType, direction: Direction, signal: SignalId) -> Result<NodeId> {
        if signal.0 as usize >= self.kinds.len() {
            return Err(Error::invalid(format!("unknown signal {}", signal.0)));
        }
        let name = self.strings.intern(name);
        Ok(self.push_node(Node { parent, name, data: NodeData::Var { var_type, direction, signal, declares: None }, attrs: Vec::new() }))
    }

    /// Declares an enumeration table (literal, value) in the current scope.
    pub fn add_enum_table(&mut self, name: &str, entries: &[(&str, &str)]) -> NodeId {
        let parent = self.scope_stack.last().copied();
        let name = self.strings.intern(name);
        let entries = entries.iter().map(|(l, v)| (self.strings.intern(l), self.strings.intern(v))).collect();
        self.push_node(Node { parent, name, data: NodeData::EnumTable { entries }, attrs: Vec::new() })
    }

    /// Declares a transaction stream. `parent` may be a scope or `None` for top level.
    pub fn add_stream(&mut self, parent: Option<NodeId>, name: &str, kind: &str) -> NodeId {
        let name = self.strings.intern(name);
        let kind = self.strings.intern(kind);
        self.push_node(Node { parent, name, data: NodeData::Stream { kind }, attrs: Vec::new() })
    }

    /// Declares a transaction generator (type) within a stream.
    pub fn add_generator(&mut self, stream: NodeId, name: &str) -> NodeId {
        let name = self.strings.intern(name);
        self.push_node(Node { parent: Some(stream), name, data: NodeData::Generator, attrs: Vec::new() })
    }

    /// Attaches an attribute to a node created since the last flush.
    pub fn node_attr(&mut self, node: NodeId, key: &str, value: Value) -> Result<()> {
        let first_pending = self.n_nodes - self.pending_nodes.len() as u32;
        if node.0 < first_pending || node.0 >= self.n_nodes {
            return Err(Error::State("node attributes must be added before the node is flushed"));
        }
        let k = self.strings.intern(key);
        self.pending_nodes[(node.0 - first_pending) as usize].attrs.push((k, value));
        Ok(())
    }

    fn new_signal(&mut self, kind: SignalKind) -> SignalId {
        let id = SignalId(self.kinds.len() as u32);
        self.kinds.push(kind);
        self.kinds_arc = None;
        self.info.push(match kind {
            SignalKind::Bits { width, states } => SigInfo { kind: 0, narrow: packed_len(width, states) <= 8, width },
            SignalKind::Real => SigInfo { kind: 1, narrow: true, width: 64 },
            SignalKind::VarLen => SigInfo { kind: 2, narrow: false, width: 0 },
        });
        self.sig_counts.push(0);
        let mut lv = LastVal::default();
        match kind {
            SignalKind::Bits { width, states } if packed_len(width, states) <= 8 => {
                // Default X for multi-state, 0 for two-state.
                if states != 2 {
                    let mut tmp = Vec::new();
                    signal::default_value(kind, &mut tmp);
                    let mut b = [0u8; 8];
                    b[..tmp.len()].copy_from_slice(&tmp);
                    lv.payload = u64::from_le_bytes(b);
                }
            }
            SignalKind::Bits { .. } => {
                let slot = self.wide_last.len();
                self.wide_last.push(0); // compact flag
                signal::default_value(kind, &mut self.wide_last);
                lv.payload = self.wide_off.len() as u64;
                self.wide_off.push(slot);
                self.wide_sig.push(id.0);
                self.wide_hdr.push(Vec::new());
                self.wide_val.push(Vec::new());
                self.wide_last_tidx.push(0);
                self.wide_count.push(0);
            }
            SignalKind::Real => {}
            SignalKind::VarLen => {
                lv.payload = self.varlen_last.len() as u64;
                self.varlen_last.push(Vec::new());
            }
        }
        self.last.push(lv);
        self.frame_cap.push((0, false));
        let g = (id.0 >> self.group_shift) as usize;
        if g >= self.last_dirty_block.len() {
            self.last_dirty_block.resize(g + 1, NO_BLOCK);
            self.dirty_bits.resize(g / 64 + 1, 0);
        }
        id
    }

    pub fn signal_count(&self) -> u32 {
        self.kinds.len() as u32
    }

    pub fn signal_kind(&self, s: SignalId) -> Option<SignalKind> {
        self.kinds.get(s.0 as usize).copied()
    }

    // ----- time -----

    /// Advances simulation time. Must be non-decreasing.
    pub fn set_time(&mut self, t: u64) -> Result<()> {
        if t < self.time && !self.times.is_empty() {
            return Err(Error::invalid(format!("time {t} is earlier than current time {}", self.time)));
        }
        if self.times.is_empty() || t != self.time {
            self.times.push(t);
            self.cur_tidx = (self.times.len() - 1) as u32;
        }
        self.time = t;
        Ok(())
    }

    pub fn current_time(&self) -> u64 {
        self.time
    }

    /// Stops recording values (VCD `$dumpoff`).
    pub fn dump_off(&mut self) {
        self.blackout.push(Blackout { time: self.time, active: false });
    }

    /// Resumes recording (VCD `$dumpon`).
    pub fn dump_on(&mut self) {
        self.blackout.push(Blackout { time: self.time, active: true });
    }

    /// Records a dump-on/off transition at an explicit time (for converters).
    pub fn blackout_at(&mut self, time: u64, active: bool) {
        self.blackout.push(Blackout { time, active });
    }

    #[inline(always)]
    fn tidx(&self) -> u32 {
        self.cur_tidx
    }

    #[inline]
    fn mark_first_change(&mut self, s: usize) {
        let g = s >> self.group_shift;
        self.dirty_bits[g / 64] |= 1 << (g % 64);
    }

    // ----- value changes -----

    /// Hot path. `s` has been validated by `check_sig`; all per-signal tables
    /// (`last`, `frame_cap`, `sig_counts`, `info`) grow together in `new_signal`.
    #[inline(always)]
    fn emit_narrow(&mut self, s: usize, payload: u64, compact: bool) -> Result<()> {
        debug_assert!(s < self.last.len() && s < self.frame_cap.len() && s < self.sig_counts.len());
        // Safety: see above; `s < kinds.len()` was checked by the caller.
        let lv = unsafe { *self.last.get_unchecked(s) };
        if lv.payload == payload && lv.compact == compact && self.opts.dedup {
            return Ok(());
        }
        if lv.epoch != self.epoch {
            unsafe {
                self.last.get_unchecked_mut(s).epoch = self.epoch;
                *self.frame_cap.get_unchecked_mut(s) = (lv.payload, lv.compact);
            }
            self.mark_first_change(s);
        }
        unsafe {
            let l = self.last.get_unchecked_mut(s);
            l.payload = payload;
            l.compact = compact;
            *self.sig_counts.get_unchecked_mut(s) += 1;
        }
        let tidx = self.cur_tidx | if compact { COMPACT_FLAG } else { 0 };
        self.records.push(Record { sig: s as u32, tidx, payload });
        if self.records.len() >= self.chunk_limit {
            self.flush_chunk()?;
        }
        Ok(())
    }

    /// Wide vectors bypass the record log: entries are appended, already in
    /// column format, to a per-signal buffer that the encoder uses verbatim.
    fn emit_wide(&mut self, s: usize, data: &[u8], compact: bool) -> Result<()> {
        let kind = self.kinds[s];
        let (width, states) = match kind {
            SignalKind::Bits { width, states } => (width, states),
            _ => unreachable!(),
        };
        let wi = self.last[s].payload as usize;
        let slot = self.wide_off[wi];
        let decl_len = packed_len(width, states);
        let len = if compact { (width as usize).div_ceil(8) } else { decl_len };
        let old_compact = self.wide_last[slot] != 0;
        let old_len = if old_compact { (width as usize).div_ceil(8) } else { decl_len };
        if self.opts.dedup && old_compact == compact && self.wide_last[slot + 1..slot + 1 + len] == data[..len] {
            return Ok(());
        }
        if self.last[s].epoch != self.epoch {
            self.last[s].epoch = self.epoch;
            let off = self.frame_heap.len();
            self.frame_heap.extend_from_slice(&self.wide_last[slot + 1..slot + 1 + old_len]);
            self.frame_cap[s] = (off as u64, old_compact);
            self.mark_first_change(s);
            self.wide_last_tidx[wi] = 0;
        }
        if self.wide_val[wi].capacity() == 0 {
            if let Some(mut b) = self.wide_pool.pop() {
                b.clear();
                self.wide_val[wi] = b;
            }
        }
        self.wide_last[slot] = compact as u8;
        self.wide_last[slot + 1..slot + 1 + len].copy_from_slice(&data[..len]);
        let dt = self.cur_tidx - self.wide_last_tidx[wi];
        self.wide_last_tidx[wi] = self.cur_tidx;
        let hdr = &mut self.wide_hdr[wi];
        let before = hdr.len();
        varint::put_u64(hdr, ((dt as u64) << 1) | (compact || states == 2) as u64);
        self.wide_val[wi].extend_from_slice(&data[..len]);
        self.wide_bytes += hdr.len() - before + len;
        self.wide_count[wi] += 1;
        self.wide_entries += 1;
        if self.wide_bytes / 16 + self.records.len() >= self.chunk_limit {
            self.flush_chunk()?;
        }
        Ok(())
    }

    fn check_sig(&self, sig: SignalId) -> Result<usize> {
        let s = sig.0 as usize;
        if s >= self.kinds.len() {
            return Err(Error::invalid(format!("unknown signal {}", sig.0)));
        }
        Ok(s)
    }

    /// Emits a single-bit value: logic code 0..=8 (see [`crate::signal`]).
    #[inline]
    pub fn emit_bit(&mut self, sig: SignalId, code: u8) -> Result<()> {
        let s = self.check_sig(sig)?;
        let info = unsafe { *self.info.get_unchecked(s) };
        if info.kind == 0 && info.width == 1 && code <= 1 {
            return self.emit_narrow(s, code as u64, true);
        }
        match self.kinds[s] {
            SignalKind::Bits { width: 1, states } => {
                let code = code.min(8);
                if signal::states_for_code(code) > states {
                    return Err(Error::invalid("logic code not representable in signal's states"));
                }
                self.emit_narrow(s, code as u64, code <= 1)
            }
            SignalKind::Bits { .. } => self.emit_u64(sig, (code & 1) as u64),
            _ => Err(Error::invalid("emit_bit on a non-bit signal")),
        }
    }

    /// Emits a 2-state integer value (zero-extended / truncated to the signal width).
    #[inline]
    pub fn emit_u64(&mut self, sig: SignalId, value: u64) -> Result<()> {
        let s = self.check_sig(sig)?;
        // Safety: check_sig validated `s < kinds.len()` and `info` grows with `kinds`.
        let info = unsafe { *self.info.get_unchecked(s) };
        if info.kind == 0 && info.narrow {
            let width = info.width;
            let v = if width >= 64 { value } else { value & ((1u64 << width) - 1) };
            return self.emit_narrow(s, v, true);
        }
        match self.kinds[s] {
            SignalKind::Bits { width, states } => {
                let v = if width >= 64 { value } else { value & ((1u64 << width) - 1) };
                if packed_len(width, states) <= 8 {
                    // Compact form when the declared packing is wider than 2-state.
                    self.emit_narrow(s, v, true)
                } else {
                    let bytes = v.to_le_bytes();
                    let n = (width as usize).div_ceil(8);
                    let mut buf = std::mem::take(&mut self.scratch);
                    buf.clear();
                    buf.resize(n, 0);
                    buf[..8.min(n)].copy_from_slice(&bytes[..8.min(n)]);
                    let r = self.emit_wide(s, &buf, true);
                    self.scratch = buf;
                    r
                }
            }
            SignalKind::Real => self.emit_real(sig, value as f64),
            SignalKind::VarLen => Err(Error::invalid("emit_u64 on a variable-length signal")),
        }
    }

    /// Emits a 2-state value given as little-endian 32-bit words (word 0 = bits 0..32).
    pub fn emit_words(&mut self, sig: SignalId, words: &[u32]) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Bits { width, states } => {
                if width <= 64 {
                    let v = words.first().copied().unwrap_or(0) as u64 | (words.get(1).copied().unwrap_or(0) as u64) << 32;
                    return self.emit_u64(sig, v);
                }
                let mut buf = std::mem::take(&mut self.scratch);
                buf.clear();
                buf.resize((width as usize).div_ceil(8), 0);
                signal::pack_words32(words, width, &mut buf);
                let _ = states;
                let r = self.emit_wide(s, &buf, true);
                self.scratch = buf;
                r
            }
            _ => Err(Error::invalid("emit_words on a non-bit signal")),
        }
    }

    /// Emits a value given as an ASCII bit string (MSB first, VCD style: `01xzXZuUwWlLhH-`).
    pub fn emit_logic_str(&mut self, sig: SignalId, s_ascii: &[u8]) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Bits { width: 1, states } => {
                let c = s_ascii.last().map(|&c| signal::code_from_ascii(c)).unwrap_or(signal::LX);
                if signal::states_for_code(c) > states {
                    return Err(Error::invalid("logic value not representable in signal's states"));
                }
                self.emit_narrow(s, c as u64, c <= 1)
            }
            SignalKind::Bits { width, states } => {
                let mut buf = std::mem::take(&mut self.scratch);
                buf.clear();
                // Try compact first.
                buf.resize((width as usize).div_ceil(8), 0);
                let two = signal::pack_ascii(s_ascii, width, 2, &mut buf);
                let mut compact = true;
                if !two {
                    if states == 2 {
                        self.scratch = buf;
                        return Err(Error::invalid("x/z value on a 2-state signal"));
                    }
                    buf.clear();
                    buf.resize(packed_len(width, states), 0);
                    signal::pack_ascii(s_ascii, width, states, &mut buf);
                    compact = false;
                    if states == 4 {
                        // Reject 9-state codes on 4-state signals.
                        for &c in s_ascii {
                            if signal::states_for_code(signal::code_from_ascii(c)) > 4 {
                                self.scratch = buf;
                                return Err(Error::invalid("9-state value on a 4-state signal"));
                            }
                        }
                    }
                }
                let r = if packed_len(width, states) <= 8 {
                    let mut b = [0u8; 8];
                    b[..buf.len()].copy_from_slice(&buf);
                    self.emit_narrow(s, u64::from_le_bytes(b), compact)
                } else {
                    self.emit_wide(s, &buf, compact)
                };
                self.scratch = buf;
                r
            }
            SignalKind::Real => {
                let txt = std::str::from_utf8(s_ascii).map_err(|_| Error::invalid("real value is not UTF-8"))?;
                let v: f64 = txt.trim().parse().map_err(|_| Error::invalid("cannot parse real value"))?;
                self.emit_real(sig, v)
            }
            SignalKind::VarLen => self.emit_varlen(sig, s_ascii),
        }
    }

    /// Emits a value already packed in VTR layout with `states` states per bit
    /// (2 for a compact value on a multi-state signal).
    pub fn emit_packed(&mut self, sig: SignalId, states: u8, data: &[u8]) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Bits { width, states: decl } => {
                if states != 2 && states != decl {
                    return Err(Error::invalid("packed value states must be 2 or the declared states"));
                }
                let len = packed_len(width, states);
                if data.len() < len {
                    return Err(Error::invalid("packed value too short"));
                }
                let compact = states == 2;
                if packed_len(width, decl) <= 8 {
                    let mut b = [0u8; 8];
                    b[..len].copy_from_slice(&data[..len]);
                    if width == 1 {
                        return self.emit_narrow(s, (b[0] & 15) as u64, compact);
                    }
                    self.emit_narrow(s, u64::from_le_bytes(b), compact)
                } else {
                    self.emit_wide(s, data, compact)
                }
            }
            _ => Err(Error::invalid("emit_packed on a non-bit signal")),
        }
    }

    pub fn emit_real(&mut self, sig: SignalId, value: f64) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Real => self.emit_narrow(s, value.to_bits(), false),
            _ => Err(Error::invalid("emit_real on a non-real signal")),
        }
    }

    /// Emits a variable-length value (strings, byte blobs).
    pub fn emit_varlen(&mut self, sig: SignalId, bytes: &[u8]) -> Result<()> {
        let s = self.check_sig(sig)?;
        if self.kinds[s] != SignalKind::VarLen {
            return Err(Error::invalid("emit_varlen on a fixed-width signal"));
        }
        let slot = self.last[s].payload as usize;
        if self.opts.dedup && self.varlen_last[slot] == bytes {
            return Ok(());
        }
        if self.last[s].epoch != self.epoch {
            self.last[s].epoch = self.epoch;
            let off = self.frame_heap.len();
            varint::put_blob(&mut self.frame_heap, &self.varlen_last[slot]);
            self.frame_cap[s] = (off as u64, false);
            self.mark_first_change(s);
        }
        self.varlen_last[slot].clear();
        self.varlen_last[slot].extend_from_slice(bytes);
        let off = self.heap.len() as u64;
        varint::put_blob(&mut self.heap, bytes);
        let tidx = self.tidx();
        self.records.push(Record { sig: s as u32, tidx, payload: off });
        self.sig_counts[s] += 1;
        if self.records.len() >= self.chunk_limit {
            self.flush_chunk()?;
        }
        Ok(())
    }

    // ----- flushing -----

    /// Hands the buffered records to the encoder as a chunk of the current block,
    /// finishing the block when it reached its size.
    fn flush_chunk(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if self.kinds_arc.is_none() {
            self.kinds_arc = Some(Arc::new(self.kinds.clone()));
        }
        self.sink.take_recycled(&mut self.chunk_pool, &mut self.block_pool);
        let mut chunk = self.chunk_pool.pop().unwrap_or_else(|| ChunkInput { records: Vec::new(), heap: Vec::new(), sig_counts: Vec::new(), kinds: Arc::new(Vec::new()), wide: Vec::new() });
        chunk.records.clear();
        std::mem::swap(&mut chunk.records, &mut self.records);
        chunk.heap.clear();
        std::mem::swap(&mut chunk.heap, &mut self.heap);
        chunk.sig_counts.clear();
        chunk.sig_counts.extend_from_slice(&self.sig_counts);
        for c in &mut self.sig_counts {
            *c = 0;
        }
        chunk.kinds = self.kinds_arc.clone().unwrap();
        for (_, _, mut h, mut v) in chunk.wide.drain(..) {
            h.clear();
            v.clear();
            self.wide_pool.push(h);
            self.wide_pool.push(v);
        }
        for wi in 0..self.wide_hdr.len() {
            if !self.wide_hdr[wi].is_empty() {
                let h = std::mem::take(&mut self.wide_hdr[wi]);
                let v = std::mem::take(&mut self.wide_val[wi]);
                chunk.wide.push((self.wide_sig[wi], self.wide_count[wi], h, v));
                self.wide_count[wi] = 0;
            }
        }
        self.block_pending += chunk.records.len() + self.wide_bytes / 16;
        self.total_records += chunk.records.len() as u64 + self.wide_entries;
        self.wide_entries = 0;
        self.wide_bytes = 0;
        if self.records.capacity() == 0 {
            self.records.reserve(self.opts.chunk_records.min(1 << 24));
        }
        self.sink.send(Msg::Chunk(Box::new(chunk)))?;
        if self.block_pending >= self.opts.block_records {
            self.flush_signals()?;
        } else {
            self.chunk_limit = self.opts.chunk_records.min(self.opts.block_records - self.block_pending).max(1);
        }
        Ok(())
    }

    fn flush_meta_and_hierarchy(&mut self) -> Result<()> {
        if self.strings.has_pending() {
            let (first, count, payload) = self.strings.take_chunk();
            self.sink.send(Msg::Section { kind: SectionKind::Strings, payload, aux0: first as u64, aux1: count as u64 })?;
        }
        if !self.meta_written {
            let mut payload = Vec::new();
            self.meta.encode(&mut payload);
            self.sink.send(Msg::Section { kind: SectionKind::Meta, payload, aux0: 0, aux1: 0 })?;
            self.meta_written = true;
        }
        if !self.pending_nodes.is_empty() {
            let first = self.n_nodes - self.pending_nodes.len() as u32;
            let mut payload = Vec::new();
            varint::put_u64(&mut payload, first as u64);
            varint::put_u64(&mut payload, self.pending_nodes.len() as u64);
            for n in &self.pending_nodes {
                n.encode(&mut payload);
            }
            let count = self.pending_nodes.len() as u64;
            self.pending_nodes.clear();
            self.sink.send(Msg::Section { kind: SectionKind::Hierarchy, payload, aux0: first as u64, aux1: count })?;
        }
        Ok(())
    }

    /// Emits the buffered value changes as a block (forces a block boundary).
    pub fn flush(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if !self.records.is_empty() || self.wide_bytes > 0 || self.block_pending > 0 {
            self.flush_signals()?;
        }
        if self.n_tx_rows > 0 || self.n_rel_rows > 0 {
            self.flush_tx()?;
        }
        Ok(())
    }

    fn flush_signals(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if !self.records.is_empty() || self.wide_bytes > 0 {
            // Send the tail of the block as a last chunk (without re-entering flush_signals).
            let save = self.opts.block_records;
            self.opts.block_records = usize::MAX;
            let r = self.flush_chunk();
            self.opts.block_records = save;
            r?;
        }
        let g = self.meta.group_size as usize;
        let n_sig = self.kinds.len();
        let n_groups = n_sig.div_ceil(g);
        // Dirty groups and frame.
        let mut dirty_groups = Vec::new();
        for (wi, &w) in self.dirty_bits.iter().enumerate() {
            let mut w = w;
            while w != 0 {
                let b = w.trailing_zeros() as usize;
                dirty_groups.push((wi * 64 + b) as u32);
                w &= w - 1;
            }
        }
        self.sink.take_recycled(&mut self.chunk_pool, &mut self.block_pool);
        let recycled = self.block_pool.pop();
        let mut input = recycled.unwrap_or_else(|| {
            BlockInput {
                block_index: 0,
                times: Vec::new(),
                kinds: Arc::new(Vec::new()),
                n_signals: 0,
                group_size: 0,
                dirty_groups: Vec::new(),
                frame: Vec::new(),
                prev_dirty: Vec::new(),
                run_budget: 0,
            }
        });
        input.run_budget = self.opts.run_bytes;
        input.frame.clear();
        for &grp in &dirty_groups {
            let first = grp as usize * g;
            let last = ((grp as usize + 1) * g).min(n_sig);
            for s in first..last {
                let kind = self.kinds[s];
                let lv = self.last[s];
                let captured = lv.epoch == self.epoch;
                match kind {
                    SignalKind::Bits { width, states } => {
                        let decl_len = packed_len(width, states);
                        if decl_len <= 8 {
                            let (payload, compact) = if captured { self.frame_cap[s] } else { (lv.payload, lv.compact) };
                            let bytes = payload.to_le_bytes();
                            if compact && states != 2 {
                                signal::widen(&bytes, width, 2, states, &mut input.frame);
                            } else {
                                input.frame.extend_from_slice(&bytes[..decl_len]);
                            }
                        } else {
                            let (data, compact): (&[u8], bool) = if captured {
                                let (off, c) = self.frame_cap[s];
                                let l = if c { (width as usize).div_ceil(8) } else { decl_len };
                                (&self.frame_heap[off as usize..off as usize + l], c)
                            } else {
                                let slot = self.wide_off[lv.payload as usize];
                                let c = self.wide_last[slot] != 0;
                                let l = if c { (width as usize).div_ceil(8) } else { decl_len };
                                (&self.wide_last[slot + 1..slot + 1 + l], c)
                            };
                            if compact && states != 2 {
                                signal::widen(data, width, 2, states, &mut input.frame);
                            } else {
                                input.frame.extend_from_slice(data);
                            }
                        }
                    }
                    SignalKind::Real => {
                        let payload = if captured { self.frame_cap[s].0 } else { lv.payload };
                        input.frame.extend_from_slice(&payload.to_le_bytes());
                    }
                    SignalKind::VarLen => {
                        if captured {
                            let off = self.frame_cap[s].0 as usize;
                            let mut r = varint::Reader::new(&self.frame_heap);
                            r.pos = off;
                            let b = r.blob().unwrap();
                            varint::put_blob(&mut input.frame, b);
                        } else {
                            varint::put_blob(&mut input.frame, &self.varlen_last[lv.payload as usize]);
                        }
                    }
                }
            }
        }
        input.prev_dirty.clear();
        input.prev_dirty.extend_from_slice(&self.last_dirty_block[..n_groups]);
        for &grp in &dirty_groups {
            self.last_dirty_block[grp as usize] = self.block_count;
        }
        if self.kinds_arc.is_none() {
            self.kinds_arc = Some(Arc::new(self.kinds.clone()));
        }
        input.block_index = self.block_count;
        input.kinds = self.kinds_arc.clone().unwrap();
        input.n_signals = n_sig as u32;
        input.group_size = g as u32;
        input.dirty_groups = dirty_groups;
        if self.times.is_empty() {
            self.times.push(self.time);
        }
        input.times.clear();
        input.times.append(&mut self.times);
        self.block_pending = 0;
        self.chunk_limit = self.opts.chunk_records.min(self.opts.block_records).max(1);
        self.sink.send(Msg::Signal(Box::new(input)))?;
        // Reset block state.
        self.frame_heap.clear();
        for w in &mut self.dirty_bits {
            *w = 0;
        }
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
            for lv in &mut self.last {
                lv.epoch = 0;
            }
        }
        self.block_count += 1;
        // The new block starts at the current time.
        self.times.push(self.time);
        self.cur_tidx = 0;
        Ok(())
    }

    // ----- transactions -----

    fn check_gen(&self, gen: NodeId) -> Result<()> {
        if gen.0 >= self.n_nodes {
            return Err(Error::invalid(format!("unknown generator node {}", gen.0)));
        }
        Ok(())
    }

    /// Begins a transaction of generator `gen` at `time`. Returns its id.
    pub fn begin_tx(&mut self, gen: NodeId, time: u64) -> Result<TxId> {
        self.check_gen(gen)?;
        let id = self.next_tx_id;
        self.next_tx_id += 1;
        let t = self.open.insert(id);
        t.gen = gen.0;
        t.begin = time;
        Ok(id)
    }

    /// Sets the parent transaction (structural nesting).
    pub fn set_tx_parent(&mut self, tx: TxId, parent: TxId) -> Result<()> {
        self.open.get(tx)?.parent = parent + 1;
        Ok(())
    }

    pub fn set_tx_kind(&mut self, tx: TxId, kind: TxKind) -> Result<()> {
        self.open.get(tx)?.kind = kind as u8;
        Ok(())
    }

    /// Records an attribute on an open transaction.
    #[inline]
    pub fn tx_attr(&mut self, tx: TxId, key: StrId, phase: AttrPhase, value: &Value) -> Result<()> {
        let t = self.open.get(tx)?;
        varint::put_u64(&mut t.attrs, key.0 as u64);
        t.attrs.push(phase as u8);
        value.encode(&mut t.attrs);
        t.n_attrs += 1;
        Ok(())
    }

    /// Records a timestamped event on an open transaction.
    pub fn tx_event(&mut self, tx: TxId, time: u64, name: StrId, attrs: &[(StrId, Value)]) -> Result<()> {
        let t = self.open.get(tx)?;
        varint::put_u64(&mut t.events, time);
        varint::put_u64(&mut t.events, name.0 as u64);
        txblock::row_attrs(&mut t.events, attrs);
        t.n_events += 1;
        Ok(())
    }

    /// Opens a stage on `lane`; an open stage on the same lane is closed at `time`.
    pub fn tx_stage_begin(&mut self, tx: TxId, name: StrId, lane: StrId, time: u64) -> Result<()> {
        let t = self.open.get(tx)?;
        for s in t.stages.iter_mut().rev() {
            if s.lane == lane.0 && s.end == u64::MAX {
                s.end = time.max(s.begin);
                break;
            }
        }
        t.stages.push(StageRec { name: name.0, lane: lane.0, begin: time, end: u64::MAX, attrs: Vec::new(), n_attrs: 0 });
        Ok(())
    }

    /// Closes the most recent open stage with `name` on `lane`. Returns false if none was open.
    pub fn tx_stage_end(&mut self, tx: TxId, name: StrId, lane: StrId, time: u64) -> Result<bool> {
        let t = self.open.get(tx)?;
        for s in t.stages.iter_mut().rev() {
            if s.lane == lane.0 && s.name == name.0 && s.end == u64::MAX {
                s.end = time.max(s.begin);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Records a complete stage.
    pub fn tx_stage(&mut self, tx: TxId, name: StrId, lane: StrId, begin: u64, end: u64, attrs: &[(StrId, Value)]) -> Result<()> {
        let t = self.open.get(tx)?;
        let mut rec = StageRec { name: name.0, lane: lane.0, begin, end: end.max(begin), attrs: Vec::new(), n_attrs: attrs.len() as u32 };
        for (k, v) in attrs {
            varint::put_u64(&mut rec.attrs, k.0 as u64);
            rec.attrs.push(AttrPhase::Record as u8);
            v.encode(&mut rec.attrs);
        }
        t.stages.push(rec);
        Ok(())
    }

    /// Attaches an attribute to the most recently begun stage.
    pub fn tx_stage_attr(&mut self, tx: TxId, key: StrId, value: &Value) -> Result<()> {
        let t = self.open.get(tx)?;
        let s = t.stages.last_mut().ok_or(Error::State("transaction has no stage"))?;
        varint::put_u64(&mut s.attrs, key.0 as u64);
        s.attrs.push(AttrPhase::Record as u8);
        value.encode(&mut s.attrs);
        s.n_attrs += 1;
        Ok(())
    }

    /// Ends a transaction. `time` must not precede its begin time.
    pub fn end_tx(&mut self, tx: TxId, time: u64, status: TxStatus) -> Result<()> {
        let t = self.open.get(tx)?;
        let end = time.max(t.begin);
        t.write_row(end, status, &mut self.tx_rows);
        self.open.remove(tx);
        self.n_tx_rows += 1;
        self.total_tx += 1;
        if self.tx_rows.len() + self.rel_rows.len() >= self.opts.tx_block_bytes {
            self.flush_tx()?;
        }
        Ok(())
    }

    /// Records a relation of kind `kind` from transaction `from` to `to`.
    pub fn relate(&mut self, kind: StrId, from: TxId, to: TxId, attrs: &[(StrId, Value)]) -> Result<()> {
        varint::put_u64(&mut self.rel_rows, kind.0 as u64);
        varint::put_u64(&mut self.rel_rows, from);
        varint::put_u64(&mut self.rel_rows, to);
        txblock::row_attrs(&mut self.rel_rows, attrs);
        self.n_rel_rows += 1;
        if self.tx_rows.len() + self.rel_rows.len() >= self.opts.tx_block_bytes {
            self.flush_tx()?;
        }
        Ok(())
    }

    pub fn open_tx_count(&self) -> usize {
        self.open.live
    }

    fn flush_tx(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if self.n_tx_rows == 0 && self.n_rel_rows == 0 {
            return Ok(());
        }
        let input = Box::new(TxBlockInput {
            tx_rows: std::mem::take(&mut self.tx_rows),
            n_tx: self.n_tx_rows,
            rel_rows: std::mem::take(&mut self.rel_rows),
            n_rel: self.n_rel_rows,
        });
        self.n_tx_rows = 0;
        self.n_rel_rows = 0;
        self.sink.send(Msg::Tx(input))
    }

    // ----- close -----

    /// Finishes the file. Also invoked by `Drop`, but errors are only reported here.
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // Open transactions are recorded with status Open.
        let live: Vec<u64> = self.open.slots.iter().filter(|s| s.id != 0).map(|s| s.id).collect();
        for id in live {
            let last = self.time;
            let t = self.open.get(id)?;
            let end = last.max(t.begin);
            t.write_row(end, TxStatus::Open, &mut self.tx_rows);
            self.open.remove(id);
            self.n_tx_rows += 1;
        }
        self.flush_meta_and_hierarchy()?;
        if !self.records.is_empty() || self.wide_bytes > 0 || self.block_pending > 0 || self.block_count == 0 && !self.times.is_empty() {
            self.flush_signals()?;
        }
        self.flush_tx()?;
        if !self.blackout.is_empty() {
            let mut payload = Vec::new();
            sections::encode_blackout(&self.blackout, &mut payload);
            self.sink.send(Msg::Section { kind: SectionKind::Blackout, payload, aux0: 0, aux1: 0 })?;
        }
        self.sink.close()
    }

    /// Number of value changes handed to the encoder so far.
    pub fn stats(&self) -> WriterStats {
        WriterStats {
            blocks: self.block_count,
            records: self.total_records + self.records.len() as u64 + self.wide_entries,
            transactions: self.total_tx,
            signals: self.kinds.len() as u32,
            nodes: self.n_nodes,
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Writer counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub blocks: u32,
    pub records: u64,
    pub transactions: u64,
    pub signals: u32,
    pub nodes: u32,
}

impl Writer {
    /// Convenience: declares a 2-state / 4-state bit-vector variable.
    pub fn add_bits(&mut self, name: &str, width: u32, states: u8) -> (NodeId, SignalId) {
        self.add_var(name, VarType::Wire, Direction::Implicit, SignalKind::Bits { width, states })
    }
}
