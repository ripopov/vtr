//! Streaming writer.

use crate::block::{
    self, BlockInput, COMPACT_FLAG, ChunkEnc, ChunkInput, EncoderScratch, NO_BLOCK, Record,
};
use crate::clock::{self, ClockId};
use crate::codec::{Compression, Compressor};
use crate::container::{self, DirEntry, SECTION_FLAG_OPTIONAL, SectionKind};
use crate::ending::{self, Ending};
use crate::error::{Error, Result};
use crate::hierarchy::{
    self, Direction, Node, NodeData, NodeId, NodeKind, ScopeType, SignalId, SignalKind, VarType,
};
use crate::logblock::{self, LogArg, LogBlockInput, LogSiteEnc, LogSiteId, LogSiteSpec};
use crate::sections::{self, Blackout, FileType, Meta, Timescale};
use crate::signal::{self, Logic, LogicStates};
use crate::strings::{Interner, StrId};
use crate::txblock::{self, TxBlockInput, TxId, TxKind, TxStatus};
use crate::value::{Value, packed_len};
use crate::varint;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering, compiler_fence};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn ensure_unique_key(keys: impl IntoIterator<Item = StrId>, key: StrId) -> Result<()> {
    if keys.into_iter().any(|existing| existing == key) {
        return Err(Error::invalid(format!("duplicate attribute key {}", key.0)));
    }
    Ok(())
}

fn ensure_unique_attrs(attrs: &[(StrId, Value)]) -> Result<()> {
    for (i, (key, _)) in attrs.iter().enumerate() {
        ensure_unique_key(attrs[..i].iter().map(|(existing, _)| *existing), *key)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct WriterOptions {
    pub compression: Compression,
    pub group_size: u32,
    pub block_records: usize,
    pub chunk_records: usize,
    pub run_bytes: usize,
    pub tx_block_bytes: usize,
    pub background: bool,
    pub log_encoders: usize,
    pub dedup: bool,
    pub checksums: bool,
    pub commit_interval: Duration,
}

pub const CHUNK_RECORDS_PER_SIGNAL: usize = 16;

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
            log_encoders: 2,
            dedup: true,
            checksums: true,
            commit_interval: Duration::ZERO,
        }
    }
}

enum Msg {
    Section {
        kind: SectionKind,
        payload: Vec<u8>,
        aux0: u64,
        aux1: u64,
    },
    Hierarchy {
        payload: Vec<u8>,
        first: u32,
        count: u32,
        signals: u32,
    },
    Chunk(Box<ChunkInput>),
    Tx(Box<TxBlockInput>),
    Log(Box<LogBlockInput>),
    Close,
    Seal(Ending, SyncSender<Result<()>>),
}

struct LogPool {
    tx: Option<SyncSender<(u64, Box<LogBlockInput>)>>,
    rx: Receiver<(u64, Result<Vec<u8>>)>,
    handles: Vec<std::thread::JoinHandle<()>>,
    pending: usize,
    next_submit: u64,
    next_write: u64,
    done: std::collections::BTreeMap<u64, Result<Vec<u8>>>,
}

impl LogPool {
    fn start(n: usize, comp: Compression) -> Result<LogPool> {
        let (in_tx, in_rx) = sync_channel::<(u64, Box<LogBlockInput>)>(n * 2);
        let (out_tx, out_rx) = std::sync::mpsc::channel::<(u64, Result<Vec<u8>>)>();
        let in_rx = Arc::new(Mutex::new(in_rx));
        let mut handles = Vec::new();
        for _ in 0..n {
            let rx = in_rx.clone();
            let tx = out_tx.clone();
            handles.push(
                std::thread::Builder::new()
                    .name("vtr-logenc".into())
                    .spawn(move || {
                        let mut compressor = Compressor::new();
                        loop {
                            let Ok((seq, input)) = rx.lock().unwrap().recv() else {
                                return;
                            };
                            let mut out = Vec::new();
                            let r =
                                logblock::encode_log_block(&input, comp, &mut compressor, &mut out)
                                    .map(|()| out);
                            if tx.send((seq, r)).is_err() {
                                return;
                            }
                        }
                    })
                    .map_err(Error::Io)?,
            );
        }
        Ok(LogPool {
            tx: Some(in_tx),
            rx: out_rx,
            handles,
            pending: 0,
            next_submit: 0,
            next_write: 0,
            done: std::collections::BTreeMap::default(),
        })
    }
}

struct FileSink {
    log_pool: Option<LogPool>,
    file: BufWriter<Box<dyn Write + Send>>,
    offset: u64,
    entries: Vec<DirEntry>,
    comp: Compression,
    checksums: bool,
    compressor: Compressor,
    scratch: EncoderScratch,
    buf: Vec<u8>,
    recycle: Option<SyncSender<Box<ChunkInput>>>,
    chunks: Vec<ChunkEnc>,
    n_chunks: usize,
    chunk_pool: Vec<ChunkEnc>,
    group_size: u32,
    run_budget: usize,
    block: BlockInput,
    frame_parts: Vec<(u32, u32, u32, u32)>,
    frame: Vec<u8>,
    last_dirty_block: Vec<u32>,
    block_count: u32,
    meta_written: bool,
    known: std::collections::HashMap<&'static str, u32>,
    next_string: u32,
    next_node: u32,
    signals: u32,
    max_id: u64,
    signal_time: Option<u64>,
    max_time: u64,
}

impl FileSink {
    fn new(
        file: impl Write + Send + 'static,
        offset: u64,
        opts: &WriterOptions,
        group_size: u32,
    ) -> Result<FileSink> {
        let log_encoders = if opts.background {
            opts.log_encoders
        } else {
            0
        };
        Ok(FileSink {
            log_pool: if log_encoders > 0 {
                Some(LogPool::start(log_encoders, opts.compression)?)
            } else {
                None
            },
            file: BufWriter::with_capacity(1 << 20, Box::new(file)),
            offset,
            entries: Vec::new(),
            comp: opts.compression,
            checksums: opts.checksums,
            compressor: Compressor::new(),
            scratch: EncoderScratch::default(),
            buf: Vec::new(),
            recycle: None,
            chunks: Vec::new(),
            n_chunks: 0,
            chunk_pool: Vec::new(),
            group_size,
            run_budget: opts.run_bytes,
            block: BlockInput {
                block_index: 0,
                times: Vec::new(),
                kinds: Arc::new(Vec::new()),
                n_signals: 0,
                group_size,
                dirty_groups: Vec::new(),
                frame: Vec::new(),
                prev_dirty: Vec::new(),
                run_budget: opts.run_bytes,
            },
            frame_parts: Vec::new(),
            frame: Vec::new(),
            last_dirty_block: Vec::new(),
            block_count: 0,
            meta_written: false,
            known: std::collections::HashMap::default(),
            next_string: 0,
            next_node: 0,
            signals: 0,
            max_id: 0,
            signal_time: None,
            max_time: 0,
        })
    }

    fn write_log_payload(&mut self, payload: &[u8]) -> Result<()> {
        let h = logblock::LogBlockHeader::parse(payload)?;
        self.saw_ids(h.max_id, h.t_max);
        self.write_section_parts(
            SectionKind::LogBlock,
            SECTION_FLAG_OPTIONAL,
            &[payload],
            h.t_min,
            h.t_max,
        )
    }

    fn saw_ids(&mut self, max_id: u64, t_max: u64) {
        self.max_id = self.max_id.max(max_id);
        self.max_time = self.max_time.max(t_max);
    }

    fn drain_logs(&mut self, wait: bool) -> Result<()> {
        loop {
            loop {
                let r = match &mut self.log_pool {
                    Some(p) if p.pending > 0 => match p.done.remove(&p.next_write) {
                        Some(r) => {
                            p.next_write += 1;
                            p.pending -= 1;
                            r
                        }
                        None => break,
                    },
                    _ => return Ok(()),
                };
                let payload = r?;
                self.write_log_payload(&payload)?;
            }
            let pool = match &mut self.log_pool {
                Some(p) if p.pending > 0 => p,
                _ => return Ok(()),
            };
            let (seq, r) = if wait {
                match pool.rx.recv() {
                    Ok(p) => p,
                    Err(_) => return Err(Error::State("log encoder thread stopped")),
                }
            } else {
                match pool.rx.try_recv() {
                    Ok(p) => p,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                    Err(_) => return Err(Error::State("log encoder thread stopped")),
                }
            };
            pool.done.insert(seq, r);
        }
    }

    fn write_section(
        &mut self,
        kind: SectionKind,
        payload: &[u8],
        aux0: u64,
        aux1: u64,
    ) -> Result<()> {
        self.write_section_parts(kind, 0, &[payload], aux0, aux1)
    }

    fn write_section_parts(
        &mut self,
        kind: SectionKind,
        flags: u32,
        parts: &[&[u8]],
        aux0: u64,
        aux1: u64,
    ) -> Result<()> {
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
        let header = container::encode_section_header(kind as u32, flags, len as u64, crc);
        self.file.write_all(&header)?;
        for p in parts {
            self.file.write_all(p)?;
        }
        self.file.flush()?;
        self.entries.push(DirEntry {
            kind: kind as u32,
            flags,
            offset: self.offset,
            len: len as u64,
            aux0,
            aux1,
        });
        self.offset += (container::SECTION_HEADER_LEN + len) as u64;
        Ok(())
    }

    fn note_strings(&mut self, payload: &[u8]) -> Result<()> {
        let mut r = varint::Reader::new(payload);
        let first = r.u32()?;
        let count = r.u32()?;
        for i in 0..count {
            let s = r.blob()?;
            if let Some(c) = ending::strings().find(|c| c.as_bytes() == s) {
                self.known.entry(c).or_insert(first + i);
            }
        }
        Ok(())
    }

    fn write_blob_section(
        &mut self,
        kind: SectionKind,
        payload: &[u8],
        aux0: u64,
        aux1: u64,
    ) -> Result<()> {
        self.buf.clear();
        self.compressor
            .compress_into(self.comp, payload, &mut self.buf)?;
        let blob = std::mem::take(&mut self.buf);
        let r = self.write_section(kind, &blob, aux0, aux1);
        self.buf = blob;
        r
    }

    fn finish_signal_block(&mut self) -> Result<()> {
        let b = &mut self.block;
        let n_sig = b.kinds.len();
        let g = self.group_size as usize;
        let n_groups = n_sig.div_ceil(g);
        self.frame_parts.sort_unstable_by_key(|p| (p.0, p.1));
        b.frame.clear();
        b.dirty_groups.clear();
        for &(grp, _, off, len) in &self.frame_parts {
            b.frame
                .extend_from_slice(&self.frame[off as usize..(off + len) as usize]);
            if b.dirty_groups.last() != Some(&grp) {
                b.dirty_groups.push(grp);
            }
        }
        self.last_dirty_block.resize(n_groups, NO_BLOCK);
        b.prev_dirty.clear();
        b.prev_dirty.extend_from_slice(&self.last_dirty_block);
        for &grp in &b.dirty_groups {
            self.last_dirty_block[grp as usize] = self.block_count;
        }
        b.block_index = self.block_count;
        b.n_signals = n_sig as u32;
        b.group_size = self.group_size;
        b.run_budget = self.run_budget;
        if b.times.is_empty() {
            b.times.push(0);
        }
        if self.n_chunks == 0 {
            self.scratch.begin_block();
        }
        self.buf.clear();
        block::finish_block(
            &self.block,
            &self.chunks[..self.n_chunks],
            self.comp,
            &mut self.compressor,
            &mut self.scratch,
            &mut self.buf,
        )?;
        self.n_chunks = 0;
        let (a, z) = (self.block.times[0], *self.block.times.last().unwrap());
        self.signal_time = Some(z);
        let header = std::mem::take(&mut self.buf);
        let data = std::mem::take(self.scratch.data_mut());
        self.write_section_parts(SectionKind::SignalBlock, 0, &[&header, &data], a, z)?;
        *self.scratch.data_mut() = data;
        self.buf = header;
        self.block.times.clear();
        self.frame.clear();
        self.frame_parts.clear();
        self.block_count += 1;
        Ok(())
    }

    fn handle(&mut self, msg: Msg) -> Result<bool> {
        if !matches!(msg, Msg::Log(_) | Msg::Close | Msg::Seal(..)) {
            self.drain_logs(false)?;
        }
        match msg {
            Msg::Section {
                kind,
                payload,
                aux0,
                aux1,
            } => {
                if kind == SectionKind::Strings {
                    self.next_string = (aux0 + aux1) as u32;
                    self.note_strings(&payload)?;
                    self.write_blob_section(kind, &payload, aux0, aux1)?;
                } else {
                    self.meta_written |= kind == SectionKind::Meta;
                    self.write_section(kind, &payload, aux0, aux1)?;
                }
            }
            Msg::Hierarchy {
                payload,
                first,
                count,
                signals,
            } => {
                self.next_node = first + count;
                self.signals = signals;
                self.write_blob_section(
                    SectionKind::Hierarchy,
                    &payload,
                    u64::from(first),
                    u64::from(count),
                )?;
            }
            Msg::Chunk(mut input) => {
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
                self.block.times.append(&mut input.times);
                let mut off = 0usize;
                for &(grp, len) in &input.frame_parts {
                    let at = self.frame.len() as u32;
                    self.frame
                        .extend_from_slice(&input.frame[off..off + len as usize]);
                    self.frame_parts
                        .push((grp, self.frame_parts.len() as u32, at, len));
                    off += len as usize;
                }
                self.block.kinds = input.kinds.clone();
                let ends = input.ends_block;
                if let Some(r) = &self.recycle {
                    drop(r.try_send(input));
                }
                if ends {
                    self.finish_signal_block()?;
                }
            }
            Msg::Tx(input) => {
                self.buf.clear();
                txblock::encode_tx_block(&input, self.comp, &mut self.compressor, &mut self.buf)?;
                let h = txblock::TxBlockHeader::parse(&self.buf)?;
                self.saw_ids(h.max_id, h.t_max);
                let payload = std::mem::take(&mut self.buf);
                self.write_section(SectionKind::TxBlock, &payload, h.t_min, h.t_max)?;
                self.buf = payload;
            }
            Msg::Log(input) => {
                if let Some(pool) = &mut self.log_pool {
                    if let Some(tx) = &pool.tx {
                        pool.pending += 1;
                        let seq = pool.next_submit;
                        pool.next_submit += 1;
                        if tx.send((seq, input)).is_err() {
                            return Err(Error::State("log encoder thread stopped"));
                        }
                    }
                    self.drain_logs(false)?;
                } else {
                    self.buf.clear();
                    logblock::encode_log_block(
                        &input,
                        self.comp,
                        &mut self.compressor,
                        &mut self.buf,
                    )?;
                    let payload = std::mem::take(&mut self.buf);
                    self.write_log_payload(&payload)?;
                    self.buf = payload;
                }
            }
            Msg::Close => {
                self.finish()?;
                return Ok(true);
            }
            Msg::Seal(end, reply) => {
                let r = self.seal(end);
                let failed = r.is_err();
                drop(reply.send(r));
                return if failed {
                    Err(Error::State("sealing failed"))
                } else {
                    Ok(true)
                };
            }
        }
        Ok(false)
    }

    fn seal(&mut self, end: Ending) -> Result<()> {
        if self.n_chunks > 0 {
            self.finish_signal_block()?;
        }
        self.drain_logs(true)?;
        self.write_ending(end)?;
        self.finish()
    }

    fn write_ending(&mut self, end: Ending) -> Result<()> {
        let Some((site, args)) = end.record() else {
            return Ok(());
        };
        let (fmt, names, types) = ending::SITES[site];
        if !self.meta_written {
            let mut payload = Vec::new();
            Meta {
                writer: format!("vtr {}", env!("CARGO_PKG_VERSION")),
                group_size: self.group_size,
                ..Meta::default()
            }
            .encode(&mut payload);
            self.write_section(SectionKind::Meta, &payload, 0, 0)?;
            self.meta_written = true;
        }
        let first = self.next_string;
        let mut strings: Vec<&str> = if first == 0 { vec![""] } else { Vec::new() };
        let known = &self.known;
        let mut intern = |s: &'static str| {
            if let Some(&id) = known.get(s) {
                return StrId(id);
            }
            let i = strings.iter().position(|&x| x == s).unwrap_or_else(|| {
                strings.push(s);
                strings.len() - 1
            });
            StrId(first + i as u32)
        };
        let scope = NodeId(self.next_node);
        let scope_node = Node {
            parent: None,
            name: intern(ending::SCOPE),
            data: NodeData::Scope {
                scope_type: ScopeType::Generic,
                component: StrId(0),
            },
            attrs: Vec::new(),
        };
        let spec = LogSiteSpec {
            names,
            ..LogSiteSpec::new(Some(scope), logblock::Severity::Fatal, fmt, types)
        };
        let (name, attrs) = logblock::site_attrs(&spec, |s| {
            let s: &'static str = [
                ending::SCOPE,
                logblock::GENERATOR_KIND,
                fmt,
                logblock::KEY_SEVERITY,
                logblock::KEY_ARGS,
                logblock::KEY_NAMES,
            ]
            .into_iter()
            .chain(names.iter().copied())
            .find(|&c| c == s)
            .expect("constant");
            intern(s)
        })?;
        let kind = intern(logblock::GENERATOR_KIND);
        let site_node = Node {
            parent: Some(scope),
            name,
            data: NodeData::Generator { kind },
            attrs,
        };
        let mut payload = Vec::new();
        if !strings.is_empty() {
            varint::put_u64(&mut payload, u64::from(first));
            varint::put_u64(&mut payload, strings.len() as u64);
            for s in &strings {
                varint::put_blob(&mut payload, s.as_bytes());
            }
            self.next_string = first + strings.len() as u32;
            self.write_blob_section(
                SectionKind::Strings,
                &payload,
                u64::from(first),
                strings.len() as u64,
            )?;
            payload.clear();
        }
        varint::put_u64(&mut payload, u64::from(scope.0));
        varint::put_u64(&mut payload, 2);
        scope_node.encode(scope.0, self.signals, &mut payload);
        site_node.encode(scope.0 + 1, self.signals, &mut payload);
        self.next_node += 2;
        self.write_blob_section(SectionKind::Hierarchy, &payload, u64::from(scope.0), 2)?;
        let time = self.signal_time.unwrap_or(self.max_time);
        let mut rows = Vec::new();
        logblock::encode_row(&mut rows, 0, time, self.max_id + 1, 0, &args);
        let input = LogBlockInput {
            rows,
            n: 1,
            sites: Arc::new(vec![LogSiteEnc {
                node: scope.0 + 1,
                args: types.to_vec(),
            }]),
        };
        self.buf.clear();
        logblock::encode_log_block(&input, self.comp, &mut self.compressor, &mut self.buf)?;
        let payload = std::mem::take(&mut self.buf);
        self.write_log_payload(&payload)?;
        self.buf = payload;
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.drain_logs(true)?;
        if let Some(pool) = &mut self.log_pool {
            pool.tx = None;
            for h in pool.handles.drain(..) {
                drop(h.join());
            }
        }
        let dir = container::encode_directory(&self.entries);
        let dir_off = self.offset;
        let header =
            container::encode_section_header(SectionKind::Directory as u32, 0, dir.len() as u64, 0);
        self.file.write_all(&header)?;
        self.file.write_all(&dir)?;
        self.offset += (container::SECTION_HEADER_LEN + dir.len()) as u64;
        let file_len = self.offset + container::TRAILER_LEN as u64;
        self.file
            .write_all(&container::encode_trailer(dir_off, file_len))?;
        self.file.flush()?;
        Ok(())
    }
}

enum Sink {
    Inline(Box<FileSink>),
    Threaded {
        tx: Option<SyncSender<Msg>>,
        recycle: Receiver<Box<ChunkInput>>,
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
            Sink::Threaded {
                tx, failed, error, ..
            } => {
                if failed.load(Ordering::Relaxed) {
                    return Err(error
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap_or(Error::State("background writer failed")));
                }
                if let Some(tx) = tx
                    && tx.send(msg).is_err()
                {
                    return Err(error
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap_or(Error::State("background writer stopped")));
                }
                Ok(())
            }
        }
    }

    fn take_recycled(&mut self, chunk_pool: &mut Vec<ChunkInput>) {
        if let Sink::Threaded { recycle, .. } = self {
            while let Ok(c) = recycle.try_recv() {
                chunk_pool.push(*c);
            }
        }
    }

    fn join(&mut self) -> Result<()> {
        match self {
            Sink::Inline(_) => Ok(()),
            Sink::Threaded {
                tx, handle, error, ..
            } => {
                tx.take();
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

    fn close(&mut self) -> Result<()> {
        match self {
            Sink::Inline(s) => {
                s.handle(Msg::Close)?;
                Ok(())
            }
            Sink::Threaded { tx, .. } => {
                if let Some(t) = tx.take() {
                    drop(t.send(Msg::Close));
                }
                self.join()
            }
        }
    }
}

struct Shared {
    state: CrashState,
    tx: Option<SyncSender<Msg>>,
}

#[derive(Default)]
pub struct CrashState {
    busy: AtomicU32,
    poisoned: AtomicBool,
    watched: AtomicBool,
    park: AtomicUsize,
}

impl CrashState {
    #[inline(always)]
    pub fn enter(&self) -> u32 {
        let prev = self.busy.load(Ordering::Relaxed);
        self.busy.store(1, Ordering::Relaxed);
        compiler_fence(Ordering::SeqCst);
        prev
    }

    #[inline(always)]
    pub fn leave(&self, prev: u32) {
        compiler_fence(Ordering::SeqCst);
        self.busy.store(prev, Ordering::Relaxed);
        if prev == 0 {
            let park = self.park.load(Ordering::Relaxed);
            if park != 0 {
                // SAFETY: request_park stores only fn() pointers; zero is excluded above.
                let park: fn() = unsafe { std::mem::transmute::<usize, fn()>(park) };
                park();
            }
        }
    }

    pub fn request_park(&self, park: fn()) {
        self.park.store(park as usize, Ordering::Relaxed);
    }

    pub fn poison(&self) {
        self.poisoned.store(true, Ordering::Relaxed);
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed) != 0 || self.poisoned()
    }

    pub fn poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn watched(&self) -> bool {
        self.watched.load(Ordering::Relaxed)
    }

    pub fn set_watched(&self, watched: bool) {
        self.watched.store(watched, Ordering::Relaxed);
    }
}

struct Guarded {
    state: *const CrashState,
    prev: u32,
}

impl Drop for Guarded {
    fn drop(&mut self) {
        // SAFETY: Guarded is scoped inside guarded(), whose Writer retains the shared crash state.
        let state = unsafe { &*self.state };
        if std::thread::panicking() {
            state.poison();
        } else {
            state.leave(self.prev);
        }
    }
}

#[derive(Clone)]
pub struct Sealer {
    shared: Arc<Shared>,
}

impl Sealer {
    #[must_use]
    pub fn state(&self) -> &CrashState {
        &self.shared.state
    }

    pub fn seal(&self, end: Ending) -> Result<()> {
        let tx = self
            .shared
            .tx
            .as_ref()
            .ok_or(Error::State("an inline writer cannot be sealed"))?;
        let (reply, done) = sync_channel(1);
        tx.send(Msg::Seal(end, reply))
            .map_err(|_| Error::State("the writer is already closed"))?;
        done.recv()
            .map_err(|_| Error::State("the writer is already closed"))?
    }
}

#[derive(Default)]
struct StageRec {
    name: u32,
    lane: u32,
    begin: u64,
    end: u64,
    attrs: Vec<u8>,
    attr_keys: Vec<StrId>,
    n_attrs: u32,
}

#[derive(Default)]
struct OpenTx {
    id: u64,
    r#gen: u32,
    begin: u64,
    kind: u8,
    parent: u64,
    attrs: Vec<u8>,
    attr_keys: Vec<StrId>,
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
        self.attr_keys.clear();
        self.n_attrs = 0;
        self.events.clear();
        self.n_events = 0;
        self.stages.clear();
    }

    fn write_row(&self, end: u64, status: TxStatus, out: &mut Vec<u8>) {
        varint::put_u64(out, self.id);
        varint::put_u64(out, u64::from(self.r#gen));
        varint::put_u64(out, self.begin);
        varint::put_u64(out, end.wrapping_sub(self.begin));
        out.push(status as u8);
        out.push(self.kind);
        varint::put_u64(out, self.parent);
        varint::put_u64(out, u64::from(self.n_attrs));
        out.extend_from_slice(&self.attrs);
        varint::put_u64(out, u64::from(self.n_events));
        out.extend_from_slice(&self.events);
        varint::put_u64(out, self.stages.len() as u64);
        for s in &self.stages {
            varint::put_u64(out, u64::from(s.name));
            varint::put_u64(out, u64::from(s.lane));
            varint::put_u64(out, s.begin);
            let e = if s.end == u64::MAX {
                end.max(s.begin)
            } else {
                s.end
            };
            varint::put_u64(out, e.wrapping_sub(s.begin) + 1);
            varint::put_u64(out, u64::from(s.n_attrs));
            out.extend_from_slice(&s.attrs);
        }
    }
}

struct OpenTable {
    slots: Vec<OpenTx>,
    mask: usize,
    live: usize,
}

impl OpenTable {
    fn new() -> Self {
        let cap = 1024;
        OpenTable {
            slots: (0..cap).map(|_| OpenTx::default()).collect(),
            mask: cap - 1,
            live: 0,
        }
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

#[derive(Clone, Copy, Default)]
struct LastVal {
    payload: u64,
    epoch: u32,
    compact: bool,
}

#[derive(Clone, Copy)]
struct SigInfo {
    kind: u8,
    narrow: bool,
    var_type: VarType,
    width: u32,
}

struct ClockState {
    r#gen: u32,
    running: Option<(u64, u64, u64)>,
    last_edge: Option<u64>,
}

pub struct Writer {
    opts: WriterOptions,
    shared: Arc<Shared>,
    sink: Sink,
    meta: Meta,
    meta_written: bool,
    strings: Interner,
    pending_nodes: Vec<Node>,
    node_kinds: Vec<NodeKind>,
    kinds: Vec<SignalKind>,
    kinds_arc: Option<Arc<Vec<SignalKind>>>,
    info: Vec<SigInfo>,
    group_shift: u32,
    sig_counts: Vec<u32>,
    last: Vec<LastVal>,
    wide_last: Vec<u8>,
    wide_off: Vec<usize>,
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
    times_sent: u32,
    records: Vec<Record>,
    heap: Vec<u8>,
    dirty_bits: Vec<u64>,
    sent_bits: Vec<u64>,
    frame_sigs: usize,
    block_count: u32,
    block_pending: usize,
    chunk_limit: usize,
    chunk_pool: Vec<ChunkInput>,
    total_records: u64,
    open: OpenTable,
    next_tx_id: u64,
    tx_rows: Vec<u8>,
    n_tx_rows: u64,
    rel_rows: Vec<u8>,
    n_rel_rows: u64,
    total_tx: u64,
    log_sites: Vec<LogSiteEnc>,
    log_sites_arc: Option<Arc<Vec<LogSiteEnc>>>,
    log_rows: Vec<u8>,
    n_log_rows: u64,
    total_log: u64,
    clocks: Vec<ClockState>,
    clock_rows: Vec<u8>,
    n_clock_rows: u64,
    blackout: Vec<Blackout>,
    closed: bool,
    scratch: Vec<u8>,
    ticks: u32,
    block_at: Option<Instant>,
    tx_at: Option<Instant>,
    log_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct WriterBuilder {
    options: WriterOptions,
    timescale: Timescale,
    time_zero: i64,
    file_type: FileType,
    writer_name: Option<String>,
    date: String,
    comment: String,
    attrs: Vec<(String, Value)>,
}

impl WriterBuilder {
    #[must_use]
    pub fn options(mut self, options: WriterOptions) -> Self {
        self.options = options;
        self
    }
    #[must_use]
    pub fn timescale(mut self, timescale: Timescale) -> Self {
        self.timescale = timescale;
        self
    }
    #[must_use]
    pub fn time_zero(mut self, time_zero: i64) -> Self {
        self.time_zero = time_zero;
        self
    }
    #[must_use]
    pub fn file_type(mut self, file_type: FileType) -> Self {
        self.file_type = file_type;
        self
    }
    #[must_use]
    pub fn writer_name(mut self, name: impl Into<String>) -> Self {
        self.writer_name = Some(name.into());
        self
    }
    #[must_use]
    pub fn date(mut self, date: impl Into<String>) -> Self {
        self.date = date.into();
        self
    }
    #[must_use]
    pub fn comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = comment.into();
        self
    }
    #[must_use]
    pub fn file_attr(mut self, key: impl Into<String>, value: Value) -> Self {
        self.attrs.push((key.into(), value));
        self
    }
    pub fn create(self, path: impl AsRef<Path>) -> Result<Writer> {
        let mut writer = Writer::create_with(path, self.options)?;
        writer.meta.timescale = self.timescale.exponent();
        writer.meta.time_zero = self.time_zero;
        writer.meta.file_type = self.file_type;
        if let Some(name) = self.writer_name {
            writer.meta.writer = name;
        }
        writer.meta.date = self.date;
        writer.meta.comment = self.comment;
        for (key, value) in self.attrs {
            writer.set_file_attr(&key, value)?;
        }
        Ok(writer)
    }
}

impl Writer {
    #[must_use]
    pub fn builder() -> WriterBuilder {
        WriterBuilder::default()
    }

    pub fn create(path: impl AsRef<Path>) -> Result<Writer> {
        Self::create_with(path, WriterOptions::default())
    }

    pub fn create_with(path: impl AsRef<Path>, opts: WriterOptions) -> Result<Writer> {
        Self::from_writer(File::create(path)?, opts)
    }

    /// Writes a fresh VTR image to an owned output without seeking.
    /// Call `close` before using its contents; discard output after an error.
    /// Hosts without native threads must disable `WriterOptions::background`.
    pub fn from_writer(
        mut file: impl Write + Send + 'static,
        opts: WriterOptions,
    ) -> Result<Writer> {
        container::write_file_header(&mut file)?;
        let offset = container::FILE_HEADER_LEN as u64;
        let (recycle_tx, recycle_rx) = sync_channel(16);
        let group_size = opts.group_size.max(1).next_power_of_two();
        let mut fsink = FileSink::new(file, offset, &opts, group_size)?;
        let mut seal_tx = None;
        let sink = if opts.background {
            fsink.recycle = Some(recycle_tx);
            let (tx, rx) = sync_channel::<Msg>(4);
            seal_tx = Some(tx.clone());
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
            Sink::Threaded {
                tx: Some(tx),
                recycle: recycle_rx,
                handle: Some(handle),
                failed,
                error,
            }
        } else {
            Sink::Inline(Box::new(fsink))
        };
        let meta = Meta {
            group_size,
            writer: format!("vtr {}", env!("CARGO_PKG_VERSION")),
            ..Meta::default()
        };
        let mut opts = opts;
        opts.group_size = group_size;
        let record_cap = opts.chunk_records.min(opts.block_records).min(1 << 24);
        let chunk_limit = opts.chunk_records.min(opts.block_records).max(1);
        Ok(Writer {
            opts,
            shared: Arc::new(Shared {
                state: CrashState::default(),
                tx: seal_tx,
            }),
            sink,
            meta,
            meta_written: false,
            strings: Interner::new(),
            pending_nodes: Vec::new(),
            node_kinds: Vec::new(),
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
            times_sent: 0,
            records: Vec::with_capacity(record_cap),
            heap: Vec::new(),
            dirty_bits: Vec::new(),
            sent_bits: Vec::new(),
            frame_sigs: 0,
            block_count: 0,
            block_pending: 0,
            chunk_limit,
            chunk_pool: Vec::new(),
            total_records: 0,
            open: OpenTable::new(),
            next_tx_id: 1,
            tx_rows: Vec::new(),
            n_tx_rows: 0,
            rel_rows: Vec::new(),
            n_rel_rows: 0,
            total_tx: 0,
            log_sites: Vec::new(),
            log_sites_arc: None,
            log_rows: Vec::new(),
            n_log_rows: 0,
            total_log: 0,
            clocks: Vec::new(),
            clock_rows: Vec::new(),
            n_clock_rows: 0,
            blackout: Vec::new(),
            closed: false,
            scratch: Vec::new(),
            ticks: 0,
            block_at: None,
            tx_at: None,
            log_at: None,
        })
        .map(|mut w| {
            let now = w.now();
            (w.block_at, w.tx_at, w.log_at) = (now, now, now);
            w
        })
    }

    fn meta_mut(&mut self) -> Result<&mut Meta> {
        if self.meta_written {
            return Err(Error::State("metadata must be set before the first flush"));
        }
        Ok(&mut self.meta)
    }
    pub fn set_timescale(&mut self, timescale: Timescale) -> Result<()> {
        self.meta_mut()?.timescale = timescale.exponent();
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
    pub fn set_file_attr(&mut self, key: &str, value: Value) -> Result<()> {
        let k = self.strings.intern(key);
        self.meta_mut()?.attrs.push((k, value));
        Ok(())
    }

    pub fn intern(&mut self, s: &str) -> StrId {
        self.strings.intern(s)
    }

    #[must_use]
    pub fn options(&self) -> &WriterOptions {
        &self.opts
    }

    #[must_use]
    pub fn string(&self, id: StrId) -> &str {
        self.strings.get(id)
    }

    fn push_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.node_kinds.len() as u32);
        self.node_kinds.push(node.kind());
        self.pending_nodes.push(node);
        id
    }

    fn check_parent(&self, child: NodeKind, parent: Option<NodeId>) -> Result<()> {
        let parent_kind = match parent {
            None => None,
            Some(p) => Some(
                *self
                    .node_kinds
                    .get(p.0 as usize)
                    .ok_or_else(|| Error::invalid(format!("unknown parent node {}", p.0)))?,
            ),
        };
        if !hierarchy::parent_allowed(child, parent_kind) {
            let under = parent_kind.map_or("the root".to_string(), |k| format!("a {k:?}"));
            return Err(Error::invalid(format!(
                "a {child:?} cannot go under {under}"
            )));
        }
        Ok(())
    }

    pub fn add_scope(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        scope_type: ScopeType,
        component: &str,
    ) -> Result<NodeId> {
        if parent.is_none() && name == ending::SCOPE {
            return Err(Error::invalid(
                "the root scope vtr.run is reserved for the run's ending",
            ));
        }
        self.push_scope(parent, name, scope_type, component)
    }

    fn push_scope(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        scope_type: ScopeType,
        component: &str,
    ) -> Result<NodeId> {
        self.check_parent(NodeKind::Scope, parent)?;
        let name = self.strings.intern(name);
        let component = self.strings.intern(component);
        Ok(self.push_node(Node {
            parent,
            name,
            data: NodeData::Scope {
                scope_type,
                component,
            },
            attrs: Vec::new(),
        }))
    }

    pub fn add_module(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        component: &str,
    ) -> Result<NodeId> {
        self.add_scope(parent, name, ScopeType::Module, component)
    }

    pub fn add_wire(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        width: u32,
        direction: Direction,
    ) -> Result<SignalId> {
        self.add_var(
            parent,
            name,
            VarType::Wire,
            direction,
            SignalKind::bits(width, LogicStates::Four),
        )
        .map(|(_, signal)| signal)
    }

    pub fn add_reg(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        width: u32,
        direction: Direction,
    ) -> Result<SignalId> {
        self.add_var(
            parent,
            name,
            VarType::Reg,
            direction,
            SignalKind::bits(width, LogicStates::Four),
        )
        .map(|(_, signal)| signal)
    }

    pub fn add_var(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        var_type: VarType,
        direction: Direction,
        kind: SignalKind,
    ) -> Result<(NodeId, SignalId)> {
        self.check_parent(NodeKind::Var, parent)?;
        let name = self.strings.intern(name);
        let sig = self.new_signal(kind, var_type);
        let id = self.push_node(Node {
            parent,
            name,
            data: NodeData::Var {
                var_type,
                direction,
                signal: sig,
                declares: Some(kind),
            },
            attrs: Vec::new(),
        });
        Ok((id, sig))
    }

    pub fn add_alias(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        var_type: VarType,
        direction: Direction,
        signal: SignalId,
    ) -> Result<NodeId> {
        self.check_parent(NodeKind::Var, parent)?;
        if signal.0 as usize >= self.kinds.len() {
            return Err(Error::invalid(format!("unknown signal {}", signal.0)));
        }
        if (var_type == VarType::Event) != (self.info[signal.0 as usize].var_type == VarType::Event)
        {
            return Err(Error::invalid(
                "alias event type differs from signal declaration",
            ));
        }
        let name = self.strings.intern(name);
        Ok(self.push_node(Node {
            parent,
            name,
            data: NodeData::Var {
                var_type,
                direction,
                signal,
                declares: None,
            },
            attrs: Vec::new(),
        }))
    }

    pub fn add_enum_table(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        entries: &[(&str, &str)],
    ) -> Result<NodeId> {
        self.check_parent(NodeKind::EnumTable, parent)?;
        let name = self.strings.intern(name);
        let entries = entries
            .iter()
            .map(|(l, v)| (self.strings.intern(l), self.strings.intern(v)))
            .collect();
        Ok(self.push_node(Node {
            parent,
            name,
            data: NodeData::EnumTable { entries },
            attrs: Vec::new(),
        }))
    }

    pub fn add_generator(
        &mut self,
        parent: Option<NodeId>,
        name: &str,
        kind: &str,
    ) -> Result<NodeId> {
        self.check_parent(NodeKind::Generator, parent)?;
        let name = self.strings.intern(name);
        let kind = self.strings.intern(kind);
        Ok(self.push_node(Node {
            parent,
            name,
            data: NodeData::Generator { kind },
            attrs: Vec::new(),
        }))
    }

    pub fn node_attr(&mut self, node: NodeId, key: &str, value: Value) -> Result<()> {
        let n_nodes = self.node_kinds.len() as u32;
        let first_pending = n_nodes - self.pending_nodes.len() as u32;
        if node.0 < first_pending || node.0 >= n_nodes {
            return Err(Error::State(
                "node attributes must be added before the node is flushed",
            ));
        }
        let k = self.strings.intern(key);
        let attrs = &mut self.pending_nodes[(node.0 - first_pending) as usize].attrs;
        ensure_unique_key(attrs.iter().map(|(key, _)| *key), k)?;
        attrs.push((k, value));
        Ok(())
    }

    fn new_signal(&mut self, kind: SignalKind, var_type: VarType) -> SignalId {
        let id = SignalId(self.kinds.len() as u32);
        self.kinds.push(kind);
        self.kinds_arc = None;
        self.info.push(match kind {
            SignalKind::Bits { width, states } => SigInfo {
                kind: 0,
                var_type,
                narrow: packed_len(width, states) <= 8,
                width,
            },
            SignalKind::Real => SigInfo {
                kind: 1,
                var_type,
                narrow: true,
                width: 64,
            },
            SignalKind::VarLen => SigInfo {
                kind: 2,
                var_type,
                narrow: false,
                width: 0,
            },
        });
        self.sig_counts.push(0);
        let mut lv = LastVal::default();
        match kind {
            SignalKind::Bits { width, states } if packed_len(width, states) <= 8 => {
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
                self.wide_last.push(0);
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
        if g / 64 >= self.dirty_bits.len() {
            self.dirty_bits.resize(g / 64 + 1, 0);
            self.sent_bits.resize(g / 64 + 1, 0);
        }
        id
    }

    #[must_use]
    pub fn signal_count(&self) -> u32 {
        self.kinds.len() as u32
    }

    #[must_use]
    pub fn signal_kind(&self, s: SignalId) -> Option<SignalKind> {
        self.kinds.get(s.0 as usize).copied()
    }

    pub fn set_time(&mut self, t: u64) -> Result<()> {
        let started = self.block_started();
        if t < self.time && started {
            return Err(Error::invalid(format!(
                "time {t} is earlier than current time {}",
                self.time
            )));
        }
        if !started || t != self.time {
            self.times.push(t);
            self.cur_tidx = self.times_sent + self.times.len() as u32 - 1;
        }
        self.time = t;
        self.tick()
    }

    #[inline(always)]
    fn tick(&mut self) -> Result<()> {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(256) && !self.opts.commit_interval.is_zero() {
            return self.commit_due();
        }
        Ok(())
    }

    fn now(&self) -> Option<Instant> {
        (!self.opts.commit_interval.is_zero()).then(Instant::now)
    }

    #[cold]
    fn commit_due(&mut self) -> Result<()> {
        let (now, interval) = (Instant::now(), self.opts.commit_interval);
        let due = |at: Option<Instant>| {
            at.is_some_and(|at| now.saturating_duration_since(at) >= interval)
        };
        if due(self.block_at)
            && (!self.records.is_empty() || self.wide_bytes > 0 || self.block_pending > 0)
        {
            self.flush_signals()?;
        }
        if due(self.tx_at) && (self.n_tx_rows > 0 || self.n_rel_rows > 0 || self.n_clock_rows > 0) {
            self.flush_tx()?;
            self.flush_clocks()?;
        }
        if due(self.log_at) && self.n_log_rows > 0 {
            self.flush_log()?;
        }
        Ok(())
    }

    fn block_started(&self) -> bool {
        self.times_sent > 0 || !self.times.is_empty()
    }

    #[must_use]
    pub fn current_time(&self) -> u64 {
        self.time
    }

    pub fn dump_off(&mut self) {
        self.blackout.push(Blackout {
            time: self.time,
            active: false,
        });
    }

    pub fn dump_on(&mut self) {
        self.blackout.push(Blackout {
            time: self.time,
            active: true,
        });
    }

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

    #[inline(always)]
    fn emit_narrow(&mut self, s: usize, payload: u64, compact: bool) -> Result<()> {
        debug_assert!(s < self.last.len() && s < self.frame_cap.len() && s < self.sig_counts.len());
        // SAFETY: All callers validate the signal index; last, frame_cap, and sig_counts grow together.
        let lv = unsafe { *self.last.get_unchecked(s) };
        if lv.payload == payload
            && lv.compact == compact
            && self.opts.dedup
            && self.info[s].var_type != VarType::Event
        {
            return Ok(());
        }
        if lv.epoch != self.epoch {
            // SAFETY: The validated signal index addresses equally sized per-signal tables.
            unsafe {
                self.last.get_unchecked_mut(s).epoch = self.epoch;
                *self.frame_cap.get_unchecked_mut(s) = (lv.payload, lv.compact);
            }
            self.mark_first_change(s);
        }
        // SAFETY: The validated signal index addresses equally sized per-signal tables.
        unsafe {
            let l = self.last.get_unchecked_mut(s);
            l.payload = payload;
            l.compact = compact;
            *self.sig_counts.get_unchecked_mut(s) += 1;
        }
        let tidx = self.cur_tidx | if compact { COMPACT_FLAG } else { 0 };
        self.records.push(Record {
            sig: s as u32,
            tidx,
            payload,
        });
        if self.records.len() >= self.chunk_limit {
            self.flush_chunk(false)?;
        }
        Ok(())
    }

    fn emit_wide(&mut self, s: usize, data: &[u8], compact: bool) -> Result<()> {
        let kind = self.kinds[s];
        let SignalKind::Bits { width, states } = kind else {
            unreachable!()
        };
        let wi = self.last[s].payload as usize;
        let slot = self.wide_off[wi];
        let decl_len = packed_len(width, states);
        let len = if compact {
            (width as usize).div_ceil(8)
        } else {
            decl_len
        };
        let old_compact = self.wide_last[slot] != 0;
        let old_len = if old_compact {
            (width as usize).div_ceil(8)
        } else {
            decl_len
        };
        if self.opts.dedup
            && self.info[s].var_type != VarType::Event
            && old_compact == compact
            && self.wide_last[slot + 1..slot + 1 + len] == data[..len]
        {
            return Ok(());
        }
        if self.last[s].epoch != self.epoch {
            self.last[s].epoch = self.epoch;
            let off = self.frame_heap.len();
            self.frame_heap
                .extend_from_slice(&self.wide_last[slot + 1..slot + 1 + old_len]);
            self.frame_cap[s] = (off as u64, old_compact);
            self.mark_first_change(s);
            self.wide_last_tidx[wi] = 0;
        }
        if self.wide_val[wi].capacity() == 0
            && let Some(mut b) = self.wide_pool.pop()
        {
            b.clear();
            self.wide_val[wi] = b;
        }
        self.wide_last[slot] = u8::from(compact);
        self.wide_last[slot + 1..slot + 1 + len].copy_from_slice(&data[..len]);
        let dt = self.cur_tidx - self.wide_last_tidx[wi];
        self.wide_last_tidx[wi] = self.cur_tidx;
        let hdr = &mut self.wide_hdr[wi];
        let before = hdr.len();
        varint::put_u64(
            hdr,
            (u64::from(dt) << 1) | u64::from(compact || states == 2),
        );
        self.wide_val[wi].extend_from_slice(&data[..len]);
        self.wide_bytes += hdr.len() - before + len;
        self.wide_count[wi] += 1;
        self.wide_entries += 1;
        if self.wide_bytes / 16 + self.records.len() >= self.chunk_limit {
            self.flush_chunk(false)?;
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

    #[inline]
    pub fn emit_bit(&mut self, sig: SignalId, value: Logic) -> Result<()> {
        let code = value as u8;
        let s = self.check_sig(sig)?;
        // SAFETY: check_sig validated this index against the signal tables.
        let info = unsafe { *self.info.get_unchecked(s) };
        if info.kind == 0 && info.width == 1 && code <= 1 {
            return self.emit_narrow(s, u64::from(code), true);
        }
        match self.kinds[s] {
            SignalKind::Bits { width: 1, states } => {
                if signal::states_for_code(code) > states {
                    return Err(Error::invalid(
                        "logic code not representable in signal's states",
                    ));
                }
                self.emit_narrow(s, u64::from(code), code <= 1)
            }
            SignalKind::Bits { .. } => Err(Error::invalid("emit_bit requires a single-bit signal")),
            _ => Err(Error::invalid("emit_bit on a non-bit signal")),
        }
    }

    #[inline]
    pub fn emit_u64(&mut self, sig: SignalId, value: u64) -> Result<()> {
        let s = self.check_sig(sig)?;
        // SAFETY: check_sig validated this index against the signal tables.
        let info = unsafe { *self.info.get_unchecked(s) };
        if info.kind == 0 && info.narrow {
            let width = info.width;
            let v = if width >= 64 {
                value
            } else {
                value & ((1u64 << width) - 1)
            };
            return self.emit_narrow(s, v, true);
        }
        match self.kinds[s] {
            SignalKind::Bits { width, states } => {
                let v = if width >= 64 {
                    value
                } else {
                    value & ((1u64 << width) - 1)
                };
                if packed_len(width, states) <= 8 {
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

    pub fn emit_words(&mut self, sig: SignalId, words: &[u32]) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Bits { width, states } => {
                if width <= 64 {
                    let v = u64::from(words.first().copied().unwrap_or(0))
                        | u64::from(words.get(1).copied().unwrap_or(0)) << 32;
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

    pub fn emit_logic_str(&mut self, sig: SignalId, s_ascii: &[u8]) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Bits { width: 1, states } => {
                let c = s_ascii
                    .last()
                    .map_or(signal::LX, |&c| signal::code_from_ascii(c));
                if signal::states_for_code(c) > states {
                    return Err(Error::invalid(
                        "logic value not representable in signal's states",
                    ));
                }
                self.emit_narrow(s, u64::from(c), c <= 1)
            }
            SignalKind::Bits { width, states } => {
                let mut buf = std::mem::take(&mut self.scratch);
                buf.clear();
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
                let txt = std::str::from_utf8(s_ascii)
                    .map_err(|_| Error::invalid("real value is not UTF-8"))?;
                let v: f64 = txt
                    .trim()
                    .parse()
                    .map_err(|_| Error::invalid("cannot parse real value"))?;
                self.emit_real(sig, v)
            }
            SignalKind::VarLen => self.emit_varlen(sig, s_ascii),
        }
    }

    pub fn emit_packed(&mut self, sig: SignalId, states: u8, data: &[u8]) -> Result<()> {
        let s = self.check_sig(sig)?;
        match self.kinds[s] {
            SignalKind::Bits {
                width,
                states: decl,
            } => {
                if states != 2 && states != decl {
                    return Err(Error::invalid(
                        "packed value states must be 2 or the declared states",
                    ));
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
                        return self.emit_narrow(s, u64::from(b[0] & 15), compact);
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

    pub fn emit_varlen(&mut self, sig: SignalId, bytes: &[u8]) -> Result<()> {
        let s = self.check_sig(sig)?;
        if self.kinds[s] != SignalKind::VarLen {
            return Err(Error::invalid("emit_varlen on a fixed-width signal"));
        }
        let slot = self.last[s].payload as usize;
        if self.opts.dedup
            && self.info[s].var_type != VarType::Event
            && self.varlen_last[slot] == bytes
        {
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
        self.records.push(Record {
            sig: s as u32,
            tidx,
            payload: off,
        });
        self.sig_counts[s] += 1;
        if self.records.len() >= self.chunk_limit {
            self.flush_chunk(false)?;
        }
        Ok(())
    }

    fn flush_chunk(&mut self, end_block: bool) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if self.kinds_arc.is_none() {
            self.kinds_arc = Some(Arc::new(self.kinds.clone()));
        }
        self.sink.take_recycled(&mut self.chunk_pool);
        let mut chunk = self.chunk_pool.pop().unwrap_or_else(|| ChunkInput {
            records: Vec::new(),
            heap: Vec::new(),
            sig_counts: Vec::new(),
            kinds: Arc::new(Vec::new()),
            wide: Vec::new(),
            times: Vec::new(),
            frame: Vec::new(),
            frame_parts: Vec::new(),
            ends_block: false,
        });
        chunk.records.clear();
        std::mem::swap(&mut chunk.records, &mut self.records);
        chunk.heap.clear();
        std::mem::swap(&mut chunk.heap, &mut self.heap);
        chunk.sig_counts.clear();
        chunk.sig_counts.extend_from_slice(&self.sig_counts);
        self.sig_counts.fill(0);
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
                chunk
                    .wide
                    .push((self.wide_sig[wi], self.wide_count[wi], h, v));
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
        let ends = end_block || self.block_pending >= self.opts.block_records;
        if ends && !self.block_started() {
            self.times.push(self.time);
        }
        chunk.times.clear();
        self.times_sent += self.times.len() as u32;
        chunk.times.append(&mut self.times);
        self.frame_parts(&mut chunk);
        chunk.ends_block = ends;
        self.sink.send(Msg::Chunk(Box::new(chunk)))?;
        if ends {
            self.end_block();
        } else {
            self.chunk_limit = self
                .chunk_target()
                .min(self.opts.block_records - self.block_pending)
                .max(1);
        }
        Ok(())
    }

    fn frame_parts(&mut self, chunk: &mut ChunkInput) {
        chunk.frame.clear();
        chunk.frame_parts.clear();
        let g = self.meta.group_size as usize;
        let n_sig = self.kinds.len();
        let a = self.frame_sigs;
        if n_sig > a && !a.is_multiple_of(g) && self.sent_bits[a / g / 64] & 1 << (a / g % 64) != 0
        {
            let before = chunk.frame.len();
            for s in a..n_sig.min((a / g + 1) * g) {
                self.push_frame(s, &mut chunk.frame);
            }
            chunk
                .frame_parts
                .push(((a / g) as u32, (chunk.frame.len() - before) as u32));
        }
        self.frame_sigs = n_sig;
        for wi in 0..self.dirty_bits.len() {
            let mut w = self.dirty_bits[wi] & !self.sent_bits[wi];
            self.sent_bits[wi] |= w;
            while w != 0 {
                let grp = wi * 64 + w.trailing_zeros() as usize;
                w &= w - 1;
                let before = chunk.frame.len();
                for s in grp * g..((grp + 1) * g).min(n_sig) {
                    self.push_frame(s, &mut chunk.frame);
                }
                chunk
                    .frame_parts
                    .push((grp as u32, (chunk.frame.len() - before) as u32));
            }
        }
        self.frame_heap.clear();
    }

    fn push_frame(&self, s: usize, out: &mut Vec<u8>) {
        let lv = self.last[s];
        let captured = lv.epoch == self.epoch;
        match self.kinds[s] {
            SignalKind::Bits { width, states } => {
                let decl_len = packed_len(width, states);
                if decl_len <= 8 {
                    let (payload, compact) = if captured {
                        self.frame_cap[s]
                    } else {
                        (lv.payload, lv.compact)
                    };
                    let bytes = payload.to_le_bytes();
                    if compact && states != 2 {
                        signal::widen(&bytes, width, 2, states, out);
                    } else {
                        out.extend_from_slice(&bytes[..decl_len]);
                    }
                } else {
                    let (data, compact): (&[u8], bool) = if captured {
                        let (off, c) = self.frame_cap[s];
                        let l = if c {
                            (width as usize).div_ceil(8)
                        } else {
                            decl_len
                        };
                        (&self.frame_heap[off as usize..off as usize + l], c)
                    } else {
                        let slot = self.wide_off[lv.payload as usize];
                        let c = self.wide_last[slot] != 0;
                        let l = if c {
                            (width as usize).div_ceil(8)
                        } else {
                            decl_len
                        };
                        (&self.wide_last[slot + 1..slot + 1 + l], c)
                    };
                    if compact && states != 2 {
                        signal::widen(data, width, 2, states, out);
                    } else {
                        out.extend_from_slice(data);
                    }
                }
            }
            SignalKind::Real => {
                let payload = if captured {
                    self.frame_cap[s].0
                } else {
                    lv.payload
                };
                out.extend_from_slice(&payload.to_le_bytes());
            }
            SignalKind::VarLen => {
                if captured {
                    let mut r = varint::Reader::new(&self.frame_heap);
                    r.pos = self.frame_cap[s].0 as usize;
                    varint::put_blob(out, r.blob().unwrap());
                } else {
                    varint::put_blob(out, &self.varlen_last[lv.payload as usize]);
                }
            }
        }
    }

    fn end_block(&mut self) {
        self.block_at = self.now();
        self.block_pending = 0;
        self.chunk_limit = self.chunk_target().min(self.opts.block_records).max(1);
        self.dirty_bits.fill(0);
        self.sent_bits.fill(0);
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
            for lv in &mut self.last {
                lv.epoch = 0;
            }
        }
        self.block_count += 1;
        self.times.push(self.time);
        self.times_sent = 0;
        self.cur_tidx = 0;
    }

    fn chunk_target(&self) -> usize {
        self.opts
            .chunk_records
            .max(CHUNK_RECORDS_PER_SIGNAL * self.kinds.len())
    }

    fn flush_meta_and_hierarchy(&mut self) -> Result<()> {
        if self.strings.has_pending() {
            let (first, count, payload) = self.strings.take_chunk();
            self.sink.send(Msg::Section {
                kind: SectionKind::Strings,
                payload,
                aux0: u64::from(first),
                aux1: u64::from(count),
            })?;
        }
        if !self.meta_written {
            let mut payload = Vec::new();
            self.meta.encode(&mut payload);
            self.sink.send(Msg::Section {
                kind: SectionKind::Meta,
                payload,
                aux0: 0,
                aux1: 0,
            })?;
            self.meta_written = true;
        }
        if !self.pending_nodes.is_empty() {
            let first = (self.node_kinds.len() - self.pending_nodes.len()) as u32;
            let mut payload = Vec::new();
            varint::put_u64(&mut payload, u64::from(first));
            varint::put_u64(&mut payload, self.pending_nodes.len() as u64);
            let declared = self
                .pending_nodes
                .iter()
                .filter(|n| n.declares_signal())
                .count() as u32;
            let mut next_signal = self.kinds.len() as u32 - declared;
            for (i, n) in self.pending_nodes.iter().enumerate() {
                n.encode(first + i as u32, next_signal, &mut payload);
                next_signal += u32::from(n.declares_signal());
            }
            let count = self.pending_nodes.len() as u32;
            self.pending_nodes.clear();
            self.sink.send(Msg::Hierarchy {
                payload,
                first,
                count,
                signals: self.kinds.len() as u32,
            })?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if !self.records.is_empty() || self.wide_bytes > 0 || self.block_pending > 0 {
            self.flush_signals()?;
        }
        if self.n_tx_rows > 0 || self.n_rel_rows > 0 {
            self.flush_tx()?;
        }
        if self.n_log_rows > 0 {
            self.flush_log()?;
        }
        self.flush_clocks()
    }

    fn flush_signals(&mut self) -> Result<()> {
        self.flush_chunk(true)
    }

    fn check_gen(&self, r#gen: NodeId) -> Result<()> {
        if self.node_kinds.get(r#gen.0 as usize) != Some(&NodeKind::Generator) {
            return Err(Error::invalid(format!(
                "node {} is not a generator",
                r#gen.0
            )));
        }
        Ok(())
    }

    pub fn begin_tx(&mut self, r#gen: NodeId, time: u64) -> Result<TxId> {
        self.check_gen(r#gen)?;
        let id = self.next_tx_id;
        self.next_tx_id += 1;
        let t = self.open.insert(id);
        t.r#gen = r#gen.0;
        t.begin = time;
        Ok(id)
    }

    pub fn set_tx_parent(&mut self, tx: TxId, parent: TxId) -> Result<()> {
        self.open.get(tx)?.parent = parent + 1;
        Ok(())
    }

    pub fn set_tx_kind(&mut self, tx: TxId, kind: TxKind) -> Result<()> {
        self.open.get(tx)?.kind = kind as u8;
        Ok(())
    }

    #[inline]
    pub fn tx_attr(&mut self, tx: TxId, key: StrId, value: &Value) -> Result<()> {
        let t = self.open.get(tx)?;
        ensure_unique_key(t.attr_keys.iter().copied(), key)?;
        varint::put_u64(&mut t.attrs, u64::from(key.0));
        value.encode(&mut t.attrs);
        t.attr_keys.push(key);
        t.n_attrs += 1;
        Ok(())
    }

    pub fn tx_event(
        &mut self,
        tx: TxId,
        time: u64,
        name: StrId,
        attrs: &[(StrId, Value)],
    ) -> Result<()> {
        ensure_unique_attrs(attrs)?;
        let t = self.open.get(tx)?;
        varint::put_u64(&mut t.events, time);
        varint::put_u64(&mut t.events, u64::from(name.0));
        txblock::row_attrs(&mut t.events, attrs);
        t.n_events += 1;
        Ok(())
    }

    pub fn tx_stage_begin(&mut self, tx: TxId, name: StrId, lane: StrId, time: u64) -> Result<()> {
        let t = self.open.get(tx)?;
        for s in t.stages.iter_mut().rev() {
            if s.lane == lane.0 && s.end == u64::MAX {
                s.end = time.max(s.begin);
                break;
            }
        }
        t.stages.push(StageRec {
            name: name.0,
            lane: lane.0,
            begin: time,
            end: u64::MAX,
            attrs: Vec::new(),
            attr_keys: Vec::new(),
            n_attrs: 0,
        });
        Ok(())
    }

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

    pub fn tx_stage(
        &mut self,
        tx: TxId,
        name: StrId,
        lane: StrId,
        begin: u64,
        end: u64,
        attrs: &[(StrId, Value)],
    ) -> Result<()> {
        ensure_unique_attrs(attrs)?;
        let t = self.open.get(tx)?;
        let mut rec = StageRec {
            name: name.0,
            lane: lane.0,
            begin,
            end: end.max(begin),
            attrs: Vec::new(),
            attr_keys: attrs.iter().map(|(key, _)| *key).collect(),
            n_attrs: attrs.len() as u32,
        };
        for (k, v) in attrs {
            varint::put_u64(&mut rec.attrs, u64::from(k.0));
            v.encode(&mut rec.attrs);
        }
        t.stages.push(rec);
        Ok(())
    }

    pub fn tx_stage_attr(&mut self, tx: TxId, key: StrId, value: &Value) -> Result<()> {
        let t = self.open.get(tx)?;
        let s = t
            .stages
            .last_mut()
            .ok_or(Error::State("transaction has no stage"))?;
        ensure_unique_key(s.attr_keys.iter().copied(), key)?;
        varint::put_u64(&mut s.attrs, u64::from(key.0));
        value.encode(&mut s.attrs);
        s.attr_keys.push(key);
        s.n_attrs += 1;
        Ok(())
    }

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
        self.tick()
    }

    pub fn relate(
        &mut self,
        kind: StrId,
        from: TxId,
        to: TxId,
        attrs: &[(StrId, Value)],
    ) -> Result<()> {
        ensure_unique_attrs(attrs)?;
        varint::put_u64(&mut self.rel_rows, u64::from(kind.0));
        varint::put_u64(&mut self.rel_rows, from);
        varint::put_u64(&mut self.rel_rows, to);
        txblock::row_attrs(&mut self.rel_rows, attrs);
        self.n_rel_rows += 1;
        if self.tx_rows.len() + self.rel_rows.len() >= self.opts.tx_block_bytes {
            self.flush_tx()?;
        }
        Ok(())
    }

    #[must_use]
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
        self.tx_at = self.now();
        self.sink.send(Msg::Tx(input))
    }

    pub fn add_log_site(&mut self, spec: &LogSiteSpec) -> Result<LogSiteId> {
        self.check_parent(NodeKind::Generator, spec.scope)?;
        let strings = &mut self.strings;
        let (name, attrs) = logblock::site_attrs(spec, |s| strings.intern(s))?;
        let kind = self.strings.intern(logblock::GENERATOR_KIND);
        let node = self.push_node(Node {
            parent: spec.scope,
            name,
            data: NodeData::Generator { kind },
            attrs,
        });
        self.log_sites.push(LogSiteEnc {
            node: node.0,
            args: spec.args.to_vec(),
        });
        self.log_sites_arc = None;
        Ok(LogSiteId(self.log_sites.len() as u32 - 1))
    }

    #[must_use]
    pub fn log_site_node(&self, site: LogSiteId) -> Option<NodeId> {
        self.log_sites.get(site.0 as usize).map(|s| NodeId(s.node))
    }

    #[must_use]
    pub fn log_site_count(&self) -> u32 {
        self.log_sites.len() as u32
    }

    #[inline]
    pub fn log(&mut self, site: LogSiteId, time: u64, args: &[LogArg]) -> Result<TxId> {
        self.log_with_parent(site, time, None, args)
    }

    pub fn log_with_parent(
        &mut self,
        site: LogSiteId,
        time: u64,
        parent: Option<TxId>,
        args: &[LogArg],
    ) -> Result<TxId> {
        let Some(s) = self.log_sites.get(site.0 as usize) else {
            return Err(Error::invalid(format!("unknown log site {}", site.0)));
        };
        if args.len() != s.args.len() {
            return Err(Error::invalid(format!(
                "log site {} expects {} arguments, got {}",
                site.0,
                s.args.len(),
                args.len()
            )));
        }
        for (i, (a, t)) in args.iter().zip(&s.args).enumerate() {
            if a.arg_type() != *t {
                return Err(Error::invalid(format!(
                    "log site {} argument {i} is declared {} but a {} was passed",
                    site.0,
                    t.name(),
                    a.arg_type().name()
                )));
            }
        }
        let id = self.next_tx_id;
        self.next_tx_id += 1;
        logblock::encode_row(
            &mut self.log_rows,
            site.0,
            time,
            id,
            parent.map_or(0, |p| p + 1),
            args,
        );
        self.n_log_rows += 1;
        self.total_log += 1;
        if self.log_rows.len() >= self.opts.tx_block_bytes {
            self.flush_log()?;
        }
        self.tick()?;
        Ok(id)
    }

    #[inline]
    pub fn log_raw(
        &mut self,
        site: LogSiteId,
        time: u64,
        parent: Option<TxId>,
        args: &[u8],
    ) -> Result<TxId> {
        if site.0 as usize >= self.log_sites.len() {
            return Err(Error::invalid(format!("unknown log site {}", site.0)));
        }
        let id = self.next_tx_id;
        self.next_tx_id += 1;
        let out = &mut self.log_rows;
        varint::put_u64(out, u64::from(site.0));
        varint::put_u64(out, time);
        varint::put_u64(out, id);
        varint::put_u64(out, parent.map_or(0, |p| p + 1));
        out.extend_from_slice(args);
        self.n_log_rows += 1;
        self.total_log += 1;
        if self.log_rows.len() >= self.opts.tx_block_bytes {
            self.flush_log()?;
        }
        self.tick()?;
        Ok(id)
    }

    fn flush_log(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if self.n_log_rows == 0 {
            return Ok(());
        }
        if self.log_sites_arc.is_none() {
            self.log_sites_arc = Some(Arc::new(self.log_sites.clone()));
        }
        let rows = std::mem::replace(
            &mut self.log_rows,
            Vec::with_capacity(self.opts.tx_block_bytes.min(1 << 26) + 1024),
        );
        let input = Box::new(LogBlockInput {
            rows,
            n: self.n_log_rows,
            sites: self.log_sites_arc.clone().unwrap(),
        });
        self.n_log_rows = 0;
        self.log_at = self.now();
        self.sink.send(Msg::Log(input))
    }

    pub fn add_clock(&mut self, scope: Option<NodeId>, name: &str) -> Result<ClockId> {
        let r#gen = self.add_generator(scope, name, clock::GENERATOR_KIND)?;
        self.clocks.push(ClockState {
            r#gen: r#gen.0,
            running: None,
            last_edge: None,
        });
        Ok(ClockId(self.clocks.len() as u32 - 1))
    }

    #[must_use]
    pub fn clock_generator(&self, clock: ClockId) -> Option<NodeId> {
        self.clocks.get(clock.0 as usize).map(|c| NodeId(c.r#gen))
    }

    fn clock_state(&mut self, clock: ClockId) -> Result<&mut ClockState> {
        self.clocks
            .get_mut(clock.0 as usize)
            .ok_or_else(|| Error::invalid(format!("unknown clock {}", clock.0)))
    }

    pub fn clock_run(&mut self, clock: ClockId, first: u64, period: u64) -> Result<()> {
        if period == 0 {
            return Err(Error::invalid(
                "clock period must be at least one time unit",
            ));
        }
        let id = self.next_tx_id;
        let c = self.clock_state(clock)?;
        if c.running.is_some() {
            return Err(Error::State(
                "clock_run on a running clock; call clock_stop first",
            ));
        }
        if let Some(last) = c.last_edge.filter(|&last| first <= last) {
            return Err(Error::invalid(format!(
                "clock stretch starts at {first}, not after the previous last edge {last}"
            )));
        }
        c.running = Some((id, first, period));
        self.next_tx_id += 1;
        Ok(())
    }

    pub fn clock_stop(&mut self, clock: ClockId, t: u64) -> Result<()> {
        let c = self.clock_state(clock)?;
        let Some((_, first, _)) = c.running else {
            return Ok(());
        };
        if t < first {
            return Err(Error::invalid(format!(
                "clock_stop at {t} precedes the stretch's first edge {first}"
            )));
        }
        self.end_stretch(clock, t, TxStatus::Unset);
        Ok(())
    }

    fn end_stretch(&mut self, clock: ClockId, t: u64, status: TxStatus) {
        let key = self.strings.intern(clock::KEY_PERIOD);
        let c = &mut self.clocks[clock.0 as usize];
        let Some((id, first, period)) = c.running.take() else {
            return;
        };
        let end = first + (t.max(first) - first) / period * period;
        c.last_edge = Some(end);
        let out = &mut self.clock_rows;
        varint::put_u64(out, id);
        varint::put_u64(out, u64::from(c.r#gen));
        varint::put_u64(out, first);
        varint::put_u64(out, end - first);
        out.push(status as u8);
        out.push(TxKind::Unspecified as u8);
        varint::put_u64(out, 0);
        if end > first {
            varint::put_u64(out, 1);
            varint::put_u64(out, u64::from(key.0));
            Value::Time(period).encode(out);
        } else {
            varint::put_u64(out, 0);
        }
        varint::put_u64(out, 0);
        varint::put_u64(out, 0);
        self.n_clock_rows += 1;
    }

    fn flush_clocks(&mut self) -> Result<()> {
        self.flush_meta_and_hierarchy()?;
        if self.n_clock_rows == 0 {
            return Ok(());
        }
        let input = Box::new(TxBlockInput {
            tx_rows: std::mem::take(&mut self.clock_rows),
            n_tx: self.n_clock_rows,
            rel_rows: Vec::new(),
            n_rel: 0,
        });
        self.n_clock_rows = 0;
        self.sink.send(Msg::Tx(input))
    }

    pub fn close(&mut self) -> Result<()> {
        self.close_with(Ending::Closed)
    }

    pub fn close_with(&mut self, end: Ending) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        if self.shared.state.poisoned() {
            self.closed = true;
            let end = match end {
                Ending::Crashed {
                    signal,
                    code,
                    address,
                    thread,
                    ..
                } => Ending::Crashed {
                    signal,
                    code,
                    address,
                    thread,
                    sealed: true,
                },
                Ending::Closed => Ending::Poisoned,
                e => e,
            };
            let r = self.sealer().seal(end);
            let joined = self.sink.join();
            return r.and(joined);
        }
        if let Some((site, args)) = end.record() {
            let scope = self.push_scope(None, ending::SCOPE, ScopeType::Generic, "")?;
            let (fmt, names, types) = ending::SITES[site];
            let spec = LogSiteSpec {
                names,
                ..LogSiteSpec::new(Some(scope), logblock::Severity::Fatal, fmt, types)
            };
            let site = self.add_log_site(&spec)?;
            self.log(site, self.time, &args)?;
        }
        self.closed = true;
        let live: Vec<u64> = self
            .open
            .slots
            .iter()
            .filter(|s| s.id != 0)
            .map(|s| s.id)
            .collect();
        for id in live {
            let last = self.time;
            let t = self.open.get(id)?;
            let end = last.max(t.begin);
            t.write_row(end, TxStatus::Open, &mut self.tx_rows);
            self.open.remove(id);
            self.n_tx_rows += 1;
        }
        for i in 0..self.clocks.len() {
            self.end_stretch(ClockId(i as u32), self.time, TxStatus::Open);
        }
        self.flush_meta_and_hierarchy()?;
        if !self.records.is_empty()
            || self.wide_bytes > 0
            || self.block_pending > 0
            || self.block_count == 0 && self.block_started()
        {
            self.flush_signals()?;
        }
        self.flush_tx()?;
        self.flush_log()?;
        self.flush_clocks()?;
        if !self.blackout.is_empty() {
            let mut payload = Vec::new();
            sections::encode_blackout(&self.blackout, &mut payload);
            self.sink.send(Msg::Section {
                kind: SectionKind::Blackout,
                payload,
                aux0: 0,
                aux1: 0,
            })?;
        }
        self.sink.close()
    }

    pub fn guarded<R>(&mut self, f: impl FnOnce(&mut Writer) -> R) -> R {
        let state: *const CrashState = &raw const self.shared.state;
        let _mark = Guarded {
            state,
            // SAFETY: state points into the live shared allocation owned by this writer.
            prev: unsafe { (*state).enter() },
        };
        f(self)
    }

    #[must_use]
    pub fn state(&self) -> &CrashState {
        &self.shared.state
    }

    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.shared.state.poisoned()
    }

    #[must_use]
    pub fn sealer(&self) -> Sealer {
        Sealer {
            shared: self.shared.clone(),
        }
    }

    #[must_use]
    pub fn stats(&self) -> WriterStats {
        WriterStats {
            blocks: self.block_count,
            records: self.total_records + self.records.len() as u64 + self.wide_entries,
            transactions: self.total_tx,
            log_records: self.total_log,
            signals: self.kinds.len() as u32,
            nodes: self.node_kinds.len() as u32,
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.close());
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub blocks: u32,
    pub records: u64,
    pub transactions: u64,
    pub log_records: u64,
    pub signals: u32,
    pub nodes: u32,
}

pub fn recover(input: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<Option<u64>> {
    let r = crate::Reader::open(input.as_ref())?;
    let Some(dropped) = r.recovered() else {
        std::fs::copy(input.as_ref(), output.as_ref())?;
        return Ok(None);
    };
    let data = r.bytes();
    let mut file = File::create(output)?;
    file.write_all(&data[..container::FILE_HEADER_LEN])?;
    let opts = WriterOptions {
        background: false,
        ..WriterOptions::default()
    };
    let mut sink = FileSink::new(
        file,
        container::FILE_HEADER_LEN as u64,
        &opts,
        r.meta().group_size,
    )?;
    for e in r.sections() {
        let end = (e.payload_offset() + e.len) as usize;
        sink.file.write_all(&data[e.offset as usize..end])?;
        sink.entries.push(DirEntry {
            offset: sink.offset,
            ..*e
        });
        sink.offset += (end - e.offset as usize) as u64;
        sink.meta_written |= e.kind == SectionKind::Meta as u32;
    }
    sink.file.flush()?;
    sink.next_string = r.strings().len() as u32;
    for c in ending::strings() {
        if let Some(id) = r.strings().find(c) {
            sink.known.insert(c, id.0);
        }
    }
    sink.next_node = r.hierarchy().len() as u32;
    sink.signals = r.signal_count();
    sink.max_id = r.last_record_id();
    sink.signal_time = r.last_signal_time();
    sink.max_time = r.time_range().map_or(0, |(_, b)| b);
    sink.write_ending(Ending::Recovered { dropped })?;
    sink.finish()?;
    Ok(Some(dropped))
}
