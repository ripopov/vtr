//! Random-access reader over a memory-mapped VTR file.
//!
//! Opening a file reads only the directory, metadata, string table and
//! hierarchy. Value changes and transactions are decoded on demand, one
//! compressed group (a few hundred signals of one block) at a time.

use crate::block::{self, BlockHeader, ColumnIter, GroupView, NO_BLOCK};
use crate::codec::Decompressor;
use crate::container::{Container, DirEntry, SectionKind};
use crate::error::{Error, Result};
use crate::hierarchy::{Hierarchy, NodeId, NodeKind, SignalId, SignalKind};
use crate::sections::{self, Blackout, Meta};
use crate::signal::{self, OwnedSignalValue, SignalValue};
use crate::strings::{StrId, StringTable};
use crate::txblock::{self, Relation, Transaction, TxBlockData, TxBlockHeader, TxId};
use crate::value::packed_len;
use crate::varint;
use crate::xform::Xform;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

/// Reader configuration.
#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    /// Verify section checksums when a section is first accessed.
    pub verify_crc: bool,
    /// Number of decompressed group pieces kept in the cache (default 256).
    pub group_cache: Option<usize>,
}

enum Data {
    Mmap(memmap2::Mmap),
    Vec(Vec<u8>),
}

impl Data {
    fn bytes(&self) -> &[u8] {
        match self {
            Data::Mmap(m) => m,
            Data::Vec(v) => v,
        }
    }
}

struct SigBlock {
    entry: DirEntry,
    header: BlockHeader,
    times: OnceLock<Arc<Vec<u64>>>,
}

struct TxBlock {
    entry: DirEntry,
    header: TxBlockHeader,
    data: OnceLock<Arc<TxBlockData>>,
}

/// A decompressed group piece (frames or one column run) with per-signal ranges,
/// lazily untransformed columns and lazily built skip indexes for point queries.
pub(crate) struct Piece {
    data: Vec<u8>,
    ranges: Vec<(u32, u32)>,
    /// Value transform of the run; columns are undone on first access.
    xform: Xform,
    plain: Vec<OnceLock<Box<[u8]>>>,
    index: Mutex<Vec<Option<Arc<Vec<block::Checkpoint>>>>>,
}

/// Entries between skip-index checkpoints.
const INDEX_STRIDE: usize = 256;

/// Runs with at most this many columns are untransformed as a whole when decoded;
/// larger runs undo the transform per column on first access (a random load of a few
/// signals then pays only for the columns it touches).
const EAGER_UNTRANSFORM_COLUMNS: usize = 1;

impl Piece {
    fn new(mut data: Vec<u8>, mut ranges: Vec<(u32, u32)>, mut xform: Xform, first_sig: u32, kinds: &[SignalKind]) -> Result<Piece> {
        if xform != Xform::None && ranges.len() <= EAGER_UNTRANSFORM_COLUMNS {
            block::untransform_run(&mut data, &mut ranges, first_sig, kinds, xform)?;
            xform = Xform::None;
        }
        let plain = if xform == Xform::None { Vec::new() } else { (0..ranges.len()).map(|_| OnceLock::new()).collect() };
        Ok(Piece { data, ranges, xform, plain, index: Mutex::new(Vec::new()) })
    }

    /// Column `local` in plain (untransformed) form.
    fn col(&self, local: usize, kind: SignalKind) -> Result<&[u8]> {
        let (a, b) = self.ranges[local];
        if self.xform == Xform::None {
            return Ok(&self.data[a as usize..b as usize]);
        }
        if let Some(c) = self.plain[local].get() {
            return Ok(c);
        }
        let mut v = self.data[a as usize..b as usize].to_vec();
        block::untransform_column_vec(&mut v, kind, self.xform, &mut Vec::new())?;
        Ok(self.plain[local].get_or_init(|| v.into_boxed_slice()))
    }

    fn column_index(&self, local: usize, kind: SignalKind) -> Result<Arc<Vec<block::Checkpoint>>> {
        let mut idx = self.index.lock().unwrap();
        if idx.len() < self.ranges.len() {
            idx.resize(self.ranges.len(), None);
        }
        if let Some(i) = &idx[local] {
            return Ok(i.clone());
        }
        let i = Arc::new(block::build_index(self.col(local, kind)?, kind, INDEX_STRIDE)?);
        idx[local] = Some(i.clone());
        Ok(i)
    }
}

/// (block, group, piece) -> (last use tick, decompressed piece)
type CacheEntry = ((u32, u32, u32), u64, Arc<Piece>);

struct GroupCache {
    cap: usize,
    tick: u64,
    entries: Vec<CacheEntry>,
}

impl GroupCache {
    fn get(&mut self, blk: u32, grp: u32, piece: u32) -> Option<Arc<Piece>> {
        self.tick += 1;
        for e in &mut self.entries {
            if e.0 == (blk, grp, piece) {
                e.1 = self.tick;
                return Some(e.2.clone());
            }
        }
        None
    }
    fn put(&mut self, blk: u32, grp: u32, piece: u32, v: Arc<Piece>) {
        if self.cap == 0 {
            return;
        }
        self.tick += 1;
        if self.entries.len() >= self.cap {
            let (i, _) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.1).unwrap();
            self.entries.swap_remove(i);
        }
        self.entries.push(((blk, grp, piece), self.tick, v));
    }
}

/// Random-access VTR reader. Safe to share between threads.
pub struct Reader {
    data: Data,
    container: Container,
    meta: Meta,
    strings: StringTable,
    hier: Hierarchy,
    sig_blocks: Vec<SigBlock>,
    tx_blocks: Vec<TxBlock>,
    blackout: Vec<Blackout>,
    verify_crc: bool,
    cache: Mutex<GroupCache>,
    global_times: OnceLock<Vec<u64>>,
}

/// All value changes of one signal, in time order.
#[derive(Clone, Debug)]
pub struct SignalData {
    pub kind: SignalKind,
    /// Value before the first change (declared packing).
    pub initial: Vec<u8>,
    /// Change times (non-decreasing; equal times = same-time updates in emission order).
    pub times: Vec<u64>,
    /// For fixed-size kinds: `entry_len` bytes per change in declared packing.
    pub data: Vec<u8>,
    /// For `VarLen`: `offsets[i]..offsets[i+1]` slices `data`.
    pub offsets: Vec<u32>,
}

impl SignalData {
    fn entry_len(&self) -> usize {
        self.kind.packed_len().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// Value of change `i`.
    pub fn get(&self, i: usize) -> SignalValue<'_> {
        match self.kind {
            SignalKind::VarLen => SignalValue::VarLen(&self.data[self.offsets[i] as usize..self.offsets[i + 1] as usize]),
            k => {
                let l = self.entry_len();
                block::frame_value(k, &self.data[i * l..(i + 1) * l])
            }
        }
    }

    /// Initial value.
    pub fn initial(&self) -> SignalValue<'_> {
        block::frame_value(self.kind, &self.initial)
    }

    /// Index of the last change at or before `t`, if any.
    pub fn index_at(&self, t: u64) -> Option<usize> {
        let n = self.times.partition_point(|&x| x <= t);
        if n == 0 {
            None
        } else {
            Some(n - 1)
        }
    }

    /// Value at time `t` (initial value before the first change).
    pub fn value_at(&self, t: u64) -> SignalValue<'_> {
        match self.index_at(t) {
            Some(i) => self.get(i),
            None => self.initial(),
        }
    }

    /// Appends every entry of one (plain) column, `times` being its block's time table.
    fn append_column(&mut self, col: &[u8], times: &[u64]) -> Result<()> {
        let mut it = ColumnIter::new(col, self.kind);
        match self.kind {
            SignalKind::Bits { width: 1, .. } => {
                let (ts, data) = (&mut self.times, &mut self.data);
                it.for_each_bit(|tidx, code| {
                    ts.push(times[tidx as usize]);
                    data.push(code);
                })?;
            }
            SignalKind::Bits { width, states } => {
                while let Some(c) = it.next_raw()? {
                    self.times.push(times[c.tidx as usize]);
                    let v = it.value_bytes(&c);
                    if c.states == states {
                        self.data.extend_from_slice(v);
                    } else {
                        signal::widen(v, width, c.states, states, &mut self.data);
                    }
                }
            }
            SignalKind::Real => {
                while let Some(c) = it.next_raw()? {
                    self.times.push(times[c.tidx as usize]);
                    self.data.extend_from_slice(&c.inline.to_le_bytes());
                }
            }
            SignalKind::VarLen => {
                while let Some(c) = it.next_raw()? {
                    self.times.push(times[c.tidx as usize]);
                    self.data.extend_from_slice(it.value_bytes(&c));
                    self.offsets.push(self.data.len() as u32);
                }
            }
        }
        Ok(())
    }
}

/// Transaction query filter.
#[derive(Clone, Debug, Default)]
pub struct TxQuery {
    /// Only transactions of this generator.
    pub generator: Option<NodeId>,
    /// Only transactions of generators belonging to this stream.
    pub stream: Option<NodeId>,
    /// Only transactions overlapping `[t0, t1]`.
    pub window: Option<(u64, u64)>,
}

impl Reader {
    /// Opens a file with default options.
    pub fn open(path: impl AsRef<Path>) -> Result<Reader> {
        Self::open_with(path, ReadOptions::default())
    }

    pub fn open_with(path: impl AsRef<Path>, opts: ReadOptions) -> Result<Reader> {
        let file = std::fs::File::open(path)?;
        // Safety: the file is only read; concurrent modification is a documented caller error.
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        #[cfg(unix)]
        let _ = mmap.advise(memmap2::Advice::Random);
        Self::from_data(Data::Mmap(mmap), opts)
    }

    /// Opens an in-memory image.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Reader> {
        Self::from_data(Data::Vec(bytes), ReadOptions::default())
    }

    fn from_data(data: Data, opts: ReadOptions) -> Result<Reader> {
        let container = Container::parse(data.bytes())?;
        let verify = opts.verify_crc;
        let bytes = data.bytes();
        let mut strings = StringTable::new();
        let mut meta = None;
        let mut hier = Hierarchy::new();
        let mut sig_blocks = Vec::new();
        let mut tx_blocks = Vec::new();
        let mut blackout = Vec::new();
        let mut dec = Decompressor::new();
        for e in &container.entries {
            if SectionKind::from_u32(e.kind) == Some(SectionKind::Strings) {
                let raw = dec.decompress(Container::payload(bytes, e, verify)?)?;
                strings.add_chunk(&raw)?;
            }
        }
        for e in &container.entries {
            let kind = match SectionKind::from_u32(e.kind) {
                Some(k) => k,
                None => {
                    if e.flags & crate::container::SECTION_FLAG_OPTIONAL != 0 {
                        continue;
                    }
                    return Err(Error::Corrupt("unknown required section kind"));
                }
            };
            match kind {
                SectionKind::Strings | SectionKind::Directory => {}
                SectionKind::Meta => meta = Some(Meta::decode(Container::payload(bytes, e, verify)?)?),
                SectionKind::Hierarchy => {
                    let raw = dec.decompress(Container::payload(bytes, e, verify)?)?;
                    hier.add_chunk(&raw)?;
                }
                SectionKind::SignalBlock => {
                    let p = Container::payload(bytes, e, verify)?;
                    let header = BlockHeader::parse(p)?;
                    sig_blocks.push(SigBlock { entry: *e, header, times: OnceLock::new() });
                }
                SectionKind::TxBlock => {
                    let p = Container::payload(bytes, e, verify)?;
                    let header = TxBlockHeader::parse(p)?;
                    tx_blocks.push(TxBlock { entry: *e, header, data: OnceLock::new() });
                }
                SectionKind::Blackout => blackout = sections::decode_blackout(Container::payload(bytes, e, verify)?)?,
            }
        }
        hier.build_index();
        let meta = meta.unwrap_or_default();
        let cap = opts.group_cache.unwrap_or(256);
        Ok(Reader {
            data,
            container,
            meta,
            strings,
            hier,
            sig_blocks,
            tx_blocks,
            blackout,
            verify_crc: verify,
            cache: Mutex::new(GroupCache { cap, tick: 0, entries: Vec::new() }),
            global_times: OnceLock::new(),
        })
    }

    // ----- metadata -----

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// Format version of the file.
    pub fn version(&self) -> (u16, u16) {
        self.container.version
    }

    /// True when the file had no directory (writer crashed) and was recovered by scanning.
    pub fn recovered(&self) -> bool {
        self.container.recovered
    }

    pub fn strings(&self) -> &StringTable {
        &self.strings
    }

    pub fn str(&self, id: StrId) -> &str {
        self.strings.get(id)
    }

    pub fn hierarchy(&self) -> &Hierarchy {
        &self.hier
    }

    pub fn blackout(&self) -> &[Blackout] {
        &self.blackout
    }

    pub fn signal_count(&self) -> u32 {
        self.hier.signals.len() as u32
    }

    pub fn signal_kind(&self, s: SignalId) -> Result<SignalKind> {
        self.hier.signal_kind(s).ok_or_else(|| Error::invalid(format!("unknown signal {}", s.0)))
    }

    /// Time span covered by signal and transaction data.
    pub fn time_range(&self) -> Option<(u64, u64)> {
        let mut r: Option<(u64, u64)> = None;
        let mut add = |a: u64, b: u64| {
            r = Some(match r {
                None => (a, b),
                Some((x, y)) => (x.min(a), y.max(b)),
            });
        };
        for b in &self.sig_blocks {
            if b.header.n_times > 0 {
                add(b.header.start_time, b.header.end_time);
            }
        }
        for b in &self.tx_blocks {
            if b.header.n_tx > 0 {
                add(b.header.t_min, b.header.t_max);
            }
        }
        r
    }

    /// Name of a node.
    pub fn name(&self, n: NodeId) -> &str {
        self.str(self.hier.name(n))
    }

    /// Full hierarchical path of a node joined with `sep`.
    pub fn full_path(&self, n: NodeId, sep: &str) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(n);
        while let Some(c) = cur {
            parts.push(self.str(self.hier.name(c)));
            cur = self.hier.parent(c);
        }
        parts.reverse();
        parts.join(sep)
    }

    /// Finds a node by path components (linear scan per level).
    pub fn find_node(&self, path: &[&str]) -> Option<NodeId> {
        let mut level: Vec<NodeId> = self.hier.roots().collect();
        let mut found = None;
        for comp in path {
            found = level.iter().copied().find(|&n| self.name(n) == *comp);
            match found {
                Some(n) => level = self.hier.children(n).collect(),
                None => return None,
            }
        }
        found
    }

    /// Finds a variable by dotted path and returns its signal.
    pub fn find_signal(&self, path: &str, sep: char) -> Option<SignalId> {
        let parts: Vec<&str> = path.split(sep).collect();
        let n = self.find_node(&parts)?;
        self.hier.signal_of(n)
    }

    /// Stream nodes.
    pub fn streams(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.hier.nodes_of_kind(NodeKind::Stream)
    }

    /// Generator nodes.
    pub fn generators(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.hier.nodes_of_kind(NodeKind::Generator)
    }

    // ----- blocks and time -----

    fn bytes(&self) -> &[u8] {
        self.data.bytes()
    }

    pub fn block_count(&self) -> usize {
        self.sig_blocks.len()
    }

    /// Time range of a signal block.
    pub fn block_range(&self, i: usize) -> (u64, u64) {
        let h = &self.sig_blocks[i].header;
        (h.start_time, h.end_time)
    }

    fn block_payload(&self, b: &SigBlock) -> Result<&[u8]> {
        Container::payload(self.bytes(), &b.entry, false)
    }

    /// Time table of block `i` (cached).
    pub fn block_times(&self, i: usize) -> Result<Arc<Vec<u64>>> {
        let b = &self.sig_blocks[i];
        if let Some(t) = b.times.get() {
            return Ok(t.clone());
        }
        let p = self.block_payload(b)?;
        let t = Arc::new(block::decode_time_table(p, &b.header, &mut Decompressor::new())?);
        let _ = b.times.set(t.clone());
        Ok(b.times.get().unwrap().clone())
    }

    /// Global sorted table of all distinct time steps.
    pub fn time_table(&self) -> Result<&[u64]> {
        if let Some(t) = self.global_times.get() {
            return Ok(t);
        }
        let mut all = Vec::new();
        for i in 0..self.sig_blocks.len() {
            let t = self.block_times(i)?;
            let mut s = 0;
            if let (Some(&last), Some(&first)) = (all.last(), t.first()) {
                if first == last {
                    s = 1;
                }
            }
            all.extend_from_slice(&t[s..]);
        }
        let _ = self.global_times.set(all);
        Ok(self.global_times.get().unwrap())
    }

    /// Index of the last block whose start time is <= `t` (None if `t` precedes all data).
    fn block_at(&self, t: u64) -> Option<usize> {
        let n = self.sig_blocks.partition_point(|b| b.header.start_time <= t);
        if n == 0 {
            None
        } else {
            Some(n - 1)
        }
    }

    fn group_of(&self, s: SignalId) -> u32 {
        s.0 / self.meta.group_size.max(1)
    }

    fn group_first(&self, g: u32) -> u32 {
        g * self.meta.group_size.max(1)
    }

    /// Parses the container of group `g` in block `bi` (no decompression).
    fn group_view(&self, bi: usize, g: u32) -> Result<Option<GroupView<'_>>> {
        let b = &self.sig_blocks[bi];
        let p = self.block_payload(b)?;
        let (clen, off) = match block::find_group(p, &b.header, g) {
            Some(x) => x,
            None => return Ok(None),
        };
        let c = block::group_container(p, &b.header, clen, off)?;
        Ok(Some(GroupView::parse(c, self.group_first(g), &self.hier.signals)?))
    }

    /// Decompressed piece of a group: piece 0 = frames, piece k+1 = run k.
    fn piece(&self, bi: usize, g: u32, view: &GroupView<'_>, piece: u32) -> Result<Arc<Piece>> {
        if let Some(p) = self.cache.lock().unwrap().get(bi as u32, g, piece) {
            return Ok(p);
        }
        let mut d = Decompressor::new();
        let mut data = Vec::new();
        let (ranges, xform, first_sig) = if piece == 0 {
            (view.decode_frames(&self.hier.signals, &mut d, &mut data)?, Xform::None, view.first_sig)
        } else {
            let run = view.runs[piece as usize - 1];
            (view.decode_run(piece as usize - 1, &mut d, &mut data)?, run.xform, view.first_sig + run.first_local)
        };
        let p = Arc::new(Piece::new(data, ranges, xform, first_sig, &self.hier.signals)?);
        self.cache.lock().unwrap().put(bi as u32, g, piece, p.clone());
        Ok(p)
    }

    /// Run piece holding `sig`'s column in block `bi` and the column's index within it,
    /// following a dynamic alias when present.
    fn column(&self, bi: usize, g: u32, view: &GroupView<'_>, sig: u32) -> Result<(Arc<Piece>, usize)> {
        if let Some(target) = view.alias_of(sig) {
            let tg = self.group_of(SignalId(target));
            if tg == g {
                return self.column_direct(bi, g, view, target);
            }
            let tv = self.group_view(bi, tg)?.ok_or(Error::Corrupt("alias target group missing"))?;
            return self.column_direct(bi, tg, &tv, target);
        }
        self.column_direct(bi, g, view, sig)
    }

    fn column_direct(&self, bi: usize, g: u32, view: &GroupView<'_>, sig: u32) -> Result<(Arc<Piece>, usize)> {
        let ri = view.run_of(sig);
        let p = self.piece(bi, g, view, ri as u32 + 1)?;
        let local = (sig - view.first_sig - view.runs[ri].first_local) as usize;
        Ok((p, local))
    }

    fn frame(&self, bi: usize, g: u32, view: &GroupView<'_>, sig: u32) -> Result<(Arc<Piece>, (u32, u32))> {
        let p = self.piece(bi, g, view, 0)?;
        let range = p.ranges[(sig - view.first_sig) as usize];
        Ok((p, range))
    }

    /// Finds the latest block index <= `bi` where group `g` is dirty.
    fn dirty_block_at_or_before(&self, bi: usize, g: u32) -> Result<Option<usize>> {
        let b = &self.sig_blocks[bi];
        let p = self.block_payload(b)?;
        if block::find_group(p, &b.header, g).is_some() {
            return Ok(Some(bi));
        }
        let prev = block::prev_dirty(p, &b.header, g);
        if prev == NO_BLOCK || prev as usize >= bi {
            Ok(None)
        } else {
            Ok(Some(prev as usize))
        }
    }

    // ----- signal queries -----

    /// Value of `sig` at time `t`: the last change at or before `t`, or the
    /// initial value if the signal has not changed yet.
    pub fn value_at(&self, sig: SignalId, t: u64) -> Result<OwnedSignalValue> {
        let kind = self.signal_kind(sig)?;
        let g = self.group_of(sig);
        let default = || {
            let mut v = Vec::new();
            signal::default_value(kind, &mut v);
            block::frame_value(kind, &v).to_owned()
        };
        let bi = match self.block_at(t) {
            Some(bi) => bi,
            None => return Ok(default()),
        };
        let dbi = match self.dirty_block_at_or_before(bi, g)? {
            Some(x) => x,
            None => return Ok(default()),
        };
        let view = self.group_view(dbi, g)?.unwrap();
        let max_tidx = if dbi == bi {
            let times = self.block_times(bi)?;
            let n = times.partition_point(|&x| x <= t);
            if n == 0 {
                let (fp, fr) = self.frame(dbi, g, &view, sig.0)?;
                return Ok(block::frame_value(kind, &fp.data[fr.0 as usize..fr.1 as usize]).to_owned());
            }
            (n - 1) as u32
        } else {
            u32::MAX
        };
        let (cp, local) = self.column(dbi, g, &view, sig.0)?;
        let col = cp.col(local, kind)?;
        let mut it = ColumnIter::new(col, kind);
        if col.len() > 4096 {
            let index = cp.column_index(local, kind)?;
            it.seek(&index, max_tidx);
        }
        match it.last_at_or_before(max_tidx)? {
            Some(c) => Ok(it.value(&c).to_owned()),
            None => {
                let (fp, fr) = self.frame(dbi, g, &view, sig.0)?;
                Ok(block::frame_value(kind, &fp.data[fr.0 as usize..fr.1 as usize]).to_owned())
            }
        }
    }

    /// All changes of `sig` with time in `[t0, t1]`.
    pub fn changes(&self, sig: SignalId, t0: u64, t1: u64) -> Result<Vec<(u64, OwnedSignalValue)>> {
        let kind = self.signal_kind(sig)?;
        let g = self.group_of(sig);
        let mut out = Vec::new();
        let start = self.block_at(t0).unwrap_or(0);
        for bi in start..self.sig_blocks.len() {
            let h = &self.sig_blocks[bi].header;
            if h.start_time > t1 {
                break;
            }
            if h.end_time < t0 {
                continue;
            }
            let view = match self.group_view(bi, g)? {
                Some(v) => v,
                None => continue,
            };
            let (cp, local) = self.column(bi, g, &view, sig.0)?;
            let col = cp.col(local, kind)?;
            if col.is_empty() {
                continue;
            }
            let times = self.block_times(bi)?;
            let mut it = ColumnIter::new(col, kind);
            while let Some(c) = it.next_raw()? {
                let t = times[c.tidx as usize];
                if t < t0 {
                    continue;
                }
                if t > t1 {
                    break;
                }
                out.push((t, it.value(&c).to_owned()));
            }
        }
        Ok(out)
    }

    /// Column runs per value transform: `(runs, compressed bytes)` indexed by the
    /// transform code (0 none, 1 shuffle, 2 delta, 3 delta+shuffle, 4 dictionary).
    /// Reads only the group headers, not the runs.
    pub fn run_stats(&self) -> Result<[(u64, u64); 5]> {
        let mut out = [(0u64, 0u64); 5];
        let kinds = &self.hier.signals;
        for b in &self.sig_blocks {
            let p = self.block_payload(b)?;
            for (g, clen, off) in block::dirty_groups(p, &b.header) {
                let c = block::group_container(p, &b.header, clen, off)?;
                let view = GroupView::parse(c, self.group_first(g), kinds)?;
                for r in &view.runs {
                    let e = &mut out[(r.xform as u8 as usize).min(4)];
                    e.0 += 1;
                    e.1 += r.blob.len() as u64;
                }
            }
        }
        Ok(out)
    }

    /// Loads all changes of one signal.
    pub fn load_signal(&self, sig: SignalId) -> Result<SignalData> {
        Ok(self.load_signals(&[sig])?.pop().unwrap())
    }

    /// Loads several signals, decompressing each run of each block only once.
    pub fn load_signals(&self, sigs: &[SignalId]) -> Result<Vec<SignalData>> {
        let mut out = Vec::with_capacity(sigs.len());
        for &s in sigs {
            let kind = self.signal_kind(s)?;
            let mut initial = Vec::new();
            signal::default_value(kind, &mut initial);
            out.push(SignalData { kind, initial, times: Vec::new(), data: Vec::new(), offsets: vec![0] });
        }
        let mut order: Vec<usize> = (0..sigs.len()).collect();
        order.sort_by_key(|&i| (self.group_of(sigs[i]), sigs[i].0));
        let mut first_seen = vec![false; sigs.len()];
        for bi in 0..self.sig_blocks.len() {
            let mut times: Option<Arc<Vec<u64>>> = None;
            let mut i = 0;
            while i < order.len() {
                let g = self.group_of(sigs[order[i]]);
                let mut j = i;
                while j < order.len() && self.group_of(sigs[order[j]]) == g {
                    j += 1;
                }
                if let Some(view) = self.group_view(bi, g)? {
                    let mut frames: Option<Arc<Piece>> = None;
                    let mut run: Option<(usize, Arc<Piece>)> = None;
                    for &oi in &order[i..j] {
                        let s = sigs[oi];
                        if !first_seen[oi] {
                            first_seen[oi] = true;
                            if frames.is_none() {
                                frames = Some(self.piece(bi, g, &view, 0)?);
                            }
                            let fp = frames.as_ref().unwrap();
                            let fr = fp.ranges[(s.0 - view.first_sig) as usize];
                            out[oi].initial.clear();
                            out[oi].initial.extend_from_slice(&fp.data[fr.0 as usize..fr.1 as usize]);
                        }
                        let (rp, local) = if view.alias_of(s.0).is_some() {
                            self.column(bi, g, &view, s.0)?
                        } else {
                            let ri = view.run_of(s.0);
                            if run.as_ref().map(|(r, _)| *r) != Some(ri) {
                                run = Some((ri, self.piece(bi, g, &view, ri as u32 + 1)?));
                            }
                            let rp = run.as_ref().unwrap().1.clone();
                            (rp, (s.0 - view.first_sig - view.runs[ri].first_local) as usize)
                        };
                        let col = rp.col(local, out[oi].kind)?;
                        if col.is_empty() {
                            continue;
                        }
                        if times.is_none() {
                            times = Some(self.block_times(bi)?);
                        }
                        out[oi].append_column(col, times.as_ref().unwrap())?;
                    }
                }
                i = j;
            }
        }
        Ok(out)
    }

    /// Streams every change of every signal in time order within `[t0, t1]`
    /// (VCD-style dump). The callback receives `(time, signal, value)`.
    /// Within one time step the order of signals is deterministic but unspecified.
    ///
    /// Per block every run is decompressed once and a cursor is opened on every
    /// non-empty column. Blocks with few active columns are merged through a
    /// linked list per time step (GTKWave's block iterator scheme: no sorting,
    /// no per-change memory). Blocks with many active columns are walked in
    /// windows of time steps instead, so that every column visit yields many
    /// entries: each window's entries are counting-sorted by time index and
    /// delivered. Memory is proportional to the number of columns plus one
    /// window of entries, never to the length of the trace.
    pub fn for_each_change(&self, t0: u64, t1: u64, mut f: impl FnMut(u64, SignalId, SignalValue<'_>)) -> Result<()> {
        const NONE: u32 = u32::MAX;
        /// Up to this many active columns the cursors stay cache resident and the
        /// linked-list merge wins; beyond it, windows amortise the cost of visiting a column.
        const MERGE_MAX_COLUMNS: usize = 1 << 15;
        /// Window sort buffer: at least this many entries (12 bytes each) ...
        const MIN_WINDOW_ENTRIES: usize = 1 << 18;
        /// ... and at least this many per column, so that a visit to a column pays off.
        const ENTRIES_PER_COLUMN: usize = 16;
        const MAX_WINDOW_ENTRIES: usize = 1 << 23;
        const MAX_WINDOW_STEPS: usize = 1 << 12;
        // Window entry tags (top two bits of `a`).
        const TAG_SHIFT: u32 = 30;
        const MASK: u32 = (1 << TAG_SHIFT) - 1;
        const TAG_COMPACT: u32 = 0; // bits, 2-state packing; `a` = value offset in the piece
        const TAG_FULL: u32 = 1; // bits, declared packing, or varlen (`a` = offset of its length)
        const TAG_CODE: u32 = 2; // 1-bit logic code in `a`
        const TAG_REAL: u32 = 3; // `a` indexes `reals`
        #[derive(Clone, Copy, Default)]
        struct E {
            tidx: u32,
            sig: u32,
            a: u32,
        }
        /// A column being walked: its iterator, the entry not yet delivered and, for the
        /// linked-list merge, the next cursor due at the same time step.
        struct Cursor<'a> {
            sig: u32,
            base: u32,
            it: ColumnIter<'a>,
            pending: block::RawChange,
            next: u32,
        }
        /// Appends the cursor's entries with time index below `w1`; `next` receives the
        /// time index of the first entry left (or `NONE`).
        fn drain(cur: &mut Cursor<'_>, w1: u32, next: &mut u32, entries: &mut Vec<E>, mut tag: impl FnMut(&block::RawChange) -> u32) -> Result<()> {
            let (mut it, mut c, sig) = (cur.it, cur.pending, cur.sig);
            let r = loop {
                entries.push(E { tidx: c.tidx, sig, a: tag(&c) });
                match it.next_raw()? {
                    Some(n) if n.tidx < w1 => c = n,
                    Some(n) => {
                        c = n;
                        break n.tidx;
                    }
                    None => break NONE,
                }
            };
            cur.it = it;
            cur.pending = c;
            *next = r;
            Ok(())
        }
        let start = self.block_at(t0).unwrap_or(0);
        let kinds = &self.hier.signals;
        let mut d = Decompressor::new();
        // (first signal of the run, decompressed run, per-signal column ranges)
        type RunPiece = (u32, Vec<u8>, Vec<(u32, u32)>);
        let mut pieces: Vec<RunPiece> = Vec::new();
        let mut sig_piece: Vec<u32> = Vec::new();
        let mut heads: Vec<u32> = Vec::new();
        let mut entries: Vec<E> = Vec::new();
        let mut sorted: Vec<E> = Vec::new();
        let mut reals: Vec<u64> = Vec::new();
        let mut counts: Vec<u32> = Vec::new();
        for bi in start..self.sig_blocks.len() {
            let h = self.sig_blocks[bi].header;
            if h.start_time > t1 {
                break;
            }
            if h.end_time < t0 {
                continue;
            }
            let p = self.block_payload(&self.sig_blocks[bi])?;
            let times = self.block_times(bi)?;
            pieces.clear();
            sig_piece.clear();
            sig_piece.resize(h.n_signals as usize, NONE);
            let mut aliases: Vec<(u32, u32)> = Vec::new();
            let mut raw_bytes = 0usize;
            for (g, clen, off) in block::dirty_groups(p, &h) {
                let c = block::group_container(p, &h, clen, off)?;
                let view = GroupView::parse(c, self.group_first(g), kinds)?;
                aliases.extend_from_slice(&view.aliases);
                for ri in 0..view.runs.len() {
                    let mut data = Vec::new();
                    let mut ranges = view.decode_run(ri, &mut d, &mut data)?;
                    let first = view.first_sig + view.runs[ri].first_local;
                    block::untransform_run(&mut data, &mut ranges, first, kinds, view.runs[ri].xform)?;
                    if data.len() > MASK as usize {
                        return Err(Error::Corrupt("column run too large"));
                    }
                    for k in 0..ranges.len() {
                        sig_piece[first as usize + k] = pieces.len() as u32;
                    }
                    raw_bytes += data.len();
                    pieces.push((first, data, ranges));
                }
            }
            // One cursor per non-empty column, in signal order; aliased signals (whose own
            // column is empty) walk their target's column and then share its piece.
            fn column<'p>(pieces: &'p [RunPiece], sig_piece: &[u32], sig: u32) -> Option<(&'p [u8], u32)> {
                let pi = sig_piece[sig as usize];
                if pi == NONE {
                    return None;
                }
                let (first, data, ranges) = &pieces[pi as usize];
                let (a, b) = ranges[(sig - first) as usize];
                (a != b).then(|| (&data[a as usize..b as usize], a))
            }
            let own: Vec<(u32, (&[u8], u32))> = (0..h.n_signals).filter_map(|sig| column(&pieces, &sig_piece, sig).map(|c| (sig, c))).collect();
            let aliased: Vec<(u32, (&[u8], u32))> = aliases.iter().filter_map(|&(sig, target)| column(&pieces, &sig_piece, target).map(|c| (sig, c))).collect();
            for &(sig, target) in &aliases {
                sig_piece[sig as usize] = sig_piece[target as usize];
            }
            let mut cursors: Vec<Cursor<'_>> = Vec::new();
            for (sig, (col, base)) in own.into_iter().chain(aliased) {
                let mut it = ColumnIter::new(col, kinds[sig as usize]);
                if let Some(pending) = it.next_raw()? {
                    if pending.tidx >= h.n_times {
                        return Err(Error::Corrupt("time index beyond block time table"));
                    }
                    cursors.push(Cursor { sig, base, it, pending, next: NONE });
                }
            }
            let n_times = h.n_times as usize;
            if cursors.len() <= MERGE_MAX_COLUMNS {
                // Linked-list merge: `heads[t]` chains the cursors whose pending entry is at step `t`.
                heads.clear();
                heads.resize(n_times, NONE);
                for (ci, cur) in cursors.iter_mut().enumerate() {
                    let slot = &mut heads[cur.pending.tidx as usize];
                    cur.next = *slot;
                    *slot = ci as u32;
                }
                for (ti, &t) in times.iter().enumerate() {
                    if t > t1 {
                        break;
                    }
                    let mut ci = heads[ti];
                    while ci != NONE {
                        let cur = &mut cursors[ci as usize];
                        let next_in_step = cur.next;
                        loop {
                            if t >= t0 {
                                f(t, SignalId(cur.sig), cur.it.value(&cur.pending));
                            }
                            match cur.it.next_raw()? {
                                // Further changes of this signal at the same step follow right away.
                                Some(pending) if pending.tidx as usize == ti => cur.pending = pending,
                                Some(pending) => {
                                    if pending.tidx >= h.n_times || (pending.tidx as usize) < ti {
                                        return Err(Error::Corrupt("time index out of order in column"));
                                    }
                                    cur.pending = pending;
                                    let slot = &mut heads[pending.tidx as usize];
                                    cur.next = *slot;
                                    *slot = ci;
                                    break;
                                }
                                None => break,
                            }
                        }
                        ci = next_in_step;
                    }
                }
                continue;
            }
            // Windows of time steps, sized from the observed change density.
            let mut next_tidx: Vec<u32> = cursors.iter().map(|c| c.pending.tidx).collect();
            let window_entries = (ENTRIES_PER_COLUMN * cursors.len()).clamp(MIN_WINDOW_ENTRIES, MAX_WINDOW_ENTRIES);
            let mut density = (raw_bytes / 2 / n_times.max(1)).max(1); // entries per step, initial guess
            let mut w0 = 0usize;
            while w0 < n_times && times[w0] <= t1 {
                let steps = (window_entries / density).clamp(1, MAX_WINDOW_STEPS).min(n_times - w0);
                let w1 = w0 + steps;
                entries.clear();
                reals.clear();
                for (ci, cur) in cursors.iter_mut().enumerate() {
                    if next_tidx[ci] as usize >= w1 {
                        continue;
                    }
                    let base = cur.base;
                    let next = &mut next_tidx[ci];
                    match kinds[cur.sig as usize] {
                        SignalKind::Bits { width: 1, .. } => drain(cur, w1 as u32, next, &mut entries, |c| c.inline as u32 | (TAG_CODE << TAG_SHIFT))?,
                        SignalKind::Bits { states, .. } => drain(cur, w1 as u32, next, &mut entries, |c| {
                            let tag = if c.states == 2 && states != 2 { TAG_COMPACT } else { TAG_FULL };
                            (base + c.start) | (tag << TAG_SHIFT)
                        })?,
                        SignalKind::Real => drain(cur, w1 as u32, next, &mut entries, |c| {
                            reals.push(c.inline);
                            (reals.len() - 1) as u32 | (TAG_REAL << TAG_SHIFT)
                        })?,
                        SignalKind::VarLen => drain(cur, w1 as u32, next, &mut entries, |c| (base + c.inline as u32) | (TAG_FULL << TAG_SHIFT))?,
                    }
                }
                density = (entries.len() / steps).max(1);
                // Stable counting sort of the window by time index, then delivery.
                counts.clear();
                counts.resize(steps + 1, 0);
                for e in &entries {
                    if (e.tidx as usize) < w0 || e.tidx as usize >= w1 {
                        return Err(Error::Corrupt("time index out of order in column"));
                    }
                    counts[e.tidx as usize - w0 + 1] += 1;
                }
                for i in 0..steps {
                    counts[i + 1] += counts[i];
                }
                if sorted.len() < entries.len() {
                    sorted.resize(entries.len(), E::default());
                }
                for e in &entries {
                    let slot = &mut counts[e.tidx as usize - w0];
                    sorted[*slot as usize] = *e;
                    *slot += 1;
                }
                for en in &sorted[..entries.len()] {
                    let t = times[en.tidx as usize];
                    if t < t0 || t > t1 {
                        continue;
                    }
                    let sig = en.sig;
                    let kind = kinds[sig as usize];
                    let (tag, a) = (en.a >> TAG_SHIFT, (en.a & MASK) as usize);
                    let v = match tag {
                        TAG_CODE => SignalValue::Bits { width: 1, states: if a <= 1 { 2 } else { kind.states() }, data: &block::CODE_BYTES[a..a + 1] },
                        TAG_REAL => SignalValue::Real(f64::from_bits(reals[a])),
                        _ => {
                            let data = &pieces[sig_piece[sig as usize] as usize].1;
                            match kind {
                                SignalKind::Bits { width, states } => {
                                    let (st, len) = if tag == TAG_COMPACT { (2, (width as usize).div_ceil(8)) } else { (states, packed_len(width, states)) };
                                    SignalValue::Bits { width, states: st, data: &data[a..a + len] }
                                }
                                _ => {
                                    let mut r = varint::Reader::new(data);
                                    r.pos = a;
                                    SignalValue::VarLen(r.blob()?)
                                }
                            }
                        }
                    };
                    f(t, SignalId(sig), v);
                }
                w0 = w1;
            }
        }
        Ok(())
    }

    // ----- transactions -----

    pub fn tx_block_count(&self) -> usize {
        self.tx_blocks.len()
    }

    fn tx_block(&self, i: usize) -> Result<Arc<TxBlockData>> {
        let b = &self.tx_blocks[i];
        if let Some(d) = b.data.get() {
            return Ok(d.clone());
        }
        let p = Container::payload(self.bytes(), &b.entry, self.verify_crc)?;
        let d = Arc::new(txblock::decode_tx_block(p, &mut Decompressor::new())?);
        let _ = b.data.set(d);
        Ok(b.data.get().unwrap().clone())
    }

    /// Parent stream of a generator node.
    pub fn generator_stream(&self, gen: NodeId) -> Option<NodeId> {
        self.hier.parent(gen)
    }

    /// Visits transactions matching `q` in file order; stop by returning `false`.
    pub fn visit_transactions(&self, q: &TxQuery, mut f: impl FnMut(&Transaction) -> bool) -> Result<()> {
        for i in 0..self.tx_blocks.len() {
            let h = &self.tx_blocks[i].header;
            if h.n_tx == 0 {
                continue;
            }
            if let Some((t0, t1)) = q.window {
                if h.t_max < t0 || h.t_min > t1 {
                    continue;
                }
            }
            if let Some(g) = q.generator {
                let p = Container::payload(self.bytes(), &self.tx_blocks[i].entry, false)?;
                if !txblock::block_generators(p, h).any(|x| x == g.0) {
                    continue;
                }
            }
            let d = self.tx_block(i)?;
            for tx in &d.transactions {
                if let Some(g) = q.generator {
                    if tx.generator != g {
                        continue;
                    }
                }
                if let Some(s) = q.stream {
                    if self.generator_stream(tx.generator) != Some(s) {
                        continue;
                    }
                }
                if let Some((t0, t1)) = q.window {
                    if tx.end < t0 || tx.begin > t1 {
                        continue;
                    }
                }
                if !f(tx) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Collects transactions matching `q`.
    pub fn transactions(&self, q: &TxQuery) -> Result<Vec<Transaction>> {
        let mut v = Vec::new();
        self.visit_transactions(q, |t| {
            v.push(t.clone());
            true
        })?;
        Ok(v)
    }

    /// Looks up one transaction by id.
    pub fn transaction(&self, id: TxId) -> Result<Option<Transaction>> {
        for i in 0..self.tx_blocks.len() {
            let h = &self.tx_blocks[i].header;
            if h.n_tx == 0 || id < h.min_id || id > h.max_id {
                continue;
            }
            let d = self.tx_block(i)?;
            if let Some(t) = d.transactions.iter().find(|t| t.id == id) {
                return Ok(Some(t.clone()));
            }
        }
        Ok(None)
    }

    /// Relations whose source is `id`.
    pub fn relations_from(&self, id: TxId) -> Result<Vec<Relation>> {
        let mut v = Vec::new();
        for i in 0..self.tx_blocks.len() {
            let h = &self.tx_blocks[i].header;
            if h.n_rel == 0 || id < h.rel_min_from || id > h.rel_max_from {
                continue;
            }
            let d = self.tx_block(i)?;
            v.extend(d.relations.iter().filter(|r| r.from == id).cloned());
        }
        Ok(v)
    }

    /// Relations whose target is `id`.
    pub fn relations_to(&self, id: TxId) -> Result<Vec<Relation>> {
        let mut v = Vec::new();
        for i in 0..self.tx_blocks.len() {
            let h = &self.tx_blocks[i].header;
            if h.n_rel == 0 || id < h.rel_min_to || id > h.rel_max_to {
                continue;
            }
            let d = self.tx_block(i)?;
            v.extend(d.relations.iter().filter(|r| r.to == id).cloned());
        }
        Ok(v)
    }

    /// Visits every relation in file order.
    pub fn visit_relations(&self, mut f: impl FnMut(&Relation) -> bool) -> Result<()> {
        for i in 0..self.tx_blocks.len() {
            if self.tx_blocks[i].header.n_rel == 0 {
                continue;
            }
            let d = self.tx_block(i)?;
            for r in &d.relations {
                if !f(r) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Total number of transactions and relations in the file (from block headers).
    pub fn tx_counts(&self) -> (u64, u64) {
        self.tx_blocks.iter().fold((0, 0), |(a, b), t| (a + t.header.n_tx, b + t.header.n_rel))
    }

    /// Directory entries (for tools).
    pub fn sections(&self) -> &[DirEntry] {
        &self.container.entries
    }

    /// Size of the packed representation of one value of a signal, if fixed.
    pub fn packed_len(&self, s: SignalId) -> Option<usize> {
        self.hier.signal_kind(s).and_then(|k| match k {
            SignalKind::Bits { width, states } => Some(packed_len(width, states)),
            SignalKind::Real => Some(8),
            SignalKind::VarLen => None,
        })
    }
}
