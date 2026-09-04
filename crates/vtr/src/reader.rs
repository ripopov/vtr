//! Random-access reader over a memory-mapped VTR file.
//!
//! Opening a file reads only the directory, metadata, string table and
//! hierarchy. Value changes and transactions are decoded on demand, one
//! compressed group (a few hundred signals of one block) at a time.

use crate::block::{self, BlockHeader, ColumnIter, GroupView, NO_BLOCK};
use crate::codec::Decompressor;
use crate::container::{Container, DirEntry, SectionKind};
use crate::error::{Error, Result};
use crate::hierarchy::{Hierarchy, NodeData, NodeId, NodeKind, SignalId, SignalKind};
use crate::sections::{self, Blackout, Meta};
use crate::signal::{self, OwnedSignalValue, SignalValue};
use crate::strings::{StrId, StringTable};
use crate::txblock::{self, Relation, Transaction, TxBlockData, TxBlockHeader, TxId};
use crate::value::packed_len;
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

/// A decompressed group piece (frames or one column run) with per-signal ranges
/// and lazily built skip indexes for point queries.
pub(crate) struct Piece {
    data: Vec<u8>,
    ranges: Vec<(u32, u32)>,
    index: Mutex<Vec<Option<Arc<Vec<block::Checkpoint>>>>>,
}

/// Entries between skip-index checkpoints.
const INDEX_STRIDE: usize = 256;

impl Piece {
    fn column_index(&self, local: usize, kind: SignalKind) -> Result<Arc<Vec<block::Checkpoint>>> {
        let mut idx = self.index.lock().unwrap();
        if idx.len() < self.ranges.len() {
            idx.resize(self.ranges.len(), None);
        }
        if let Some(i) = &idx[local] {
            return Ok(i.clone());
        }
        let (a, b) = self.ranges[local];
        let i = Arc::new(block::build_index(&self.data[a as usize..b as usize], kind, INDEX_STRIDE)?);
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

    fn push(&mut self, t: u64, v: SignalValue) {
        self.times.push(t);
        match (self.kind, v) {
            (SignalKind::Bits { width, states }, SignalValue::Bits { states: st, data, .. }) => {
                signal::widen(data, width, st, states, &mut self.data);
            }
            (SignalKind::Real, SignalValue::Real(r)) => self.data.extend_from_slice(&r.to_le_bytes()),
            (SignalKind::VarLen, SignalValue::VarLen(b)) => {
                self.data.extend_from_slice(b);
                self.offsets.push(self.data.len() as u32);
            }
            _ => {}
        }
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
        self.str(self.hier.node(n).name)
    }

    /// Full hierarchical path of a node joined with `sep`.
    pub fn full_path(&self, n: NodeId, sep: &str) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(n);
        while let Some(c) = cur {
            let node = self.hier.node(c);
            parts.push(self.str(node.name));
            cur = node.parent;
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
        match self.hier.node(n).data {
            NodeData::Var { signal, .. } => Some(signal),
            _ => None,
        }
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
        let ranges = if piece == 0 {
            view.decode_frames(&self.hier.signals, &mut d, &mut data)?
        } else {
            view.decode_run(piece as usize - 1, &mut d, &mut data)?
        };
        let p = Arc::new(Piece { data, ranges, index: Mutex::new(Vec::new()) });
        self.cache.lock().unwrap().put(bi as u32, g, piece, p.clone());
        Ok(p)
    }

    /// Column bytes of `sig` in block `bi`, following a dynamic alias when present.
    fn column(&self, bi: usize, g: u32, view: &GroupView<'_>, sig: u32) -> Result<(Arc<Piece>, (u32, u32))> {
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

    fn column_direct(&self, bi: usize, g: u32, view: &GroupView<'_>, sig: u32) -> Result<(Arc<Piece>, (u32, u32))> {
        let ri = view.run_of(sig);
        let p = self.piece(bi, g, view, ri as u32 + 1)?;
        let local = (sig - view.first_sig - view.runs[ri].0) as usize;
        let range = p.ranges[local];
        Ok((p, range))
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
        let (cp, cr) = self.column(dbi, g, &view, sig.0)?;
        let col = &cp.data[cr.0 as usize..cr.1 as usize];
        let mut it = ColumnIter::new(col, kind);
        if col.len() > 4096 {
            let local = cp.ranges.iter().position(|r| *r == cr).unwrap_or(0);
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
            let (cp, cr) = self.column(bi, g, &view, sig.0)?;
            let col = &cp.data[cr.0 as usize..cr.1 as usize];
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
                        let (rp, cr) = if view.alias_of(s.0).is_some() {
                            self.column(bi, g, &view, s.0)?
                        } else {
                            let ri = view.run_of(s.0);
                            if run.as_ref().map(|(r, _)| *r) != Some(ri) {
                                run = Some((ri, self.piece(bi, g, &view, ri as u32 + 1)?));
                            }
                            let rp = run.as_ref().unwrap().1.clone();
                            let local = (s.0 - view.first_sig - view.runs[ri].0) as usize;
                            let cr = rp.ranges[local];
                            (rp, cr)
                        };
                        let col = &rp.data[cr.0 as usize..cr.1 as usize];
                        if col.is_empty() {
                            continue;
                        }
                        if times.is_none() {
                            times = Some(self.block_times(bi)?);
                        }
                        let tt = times.as_ref().unwrap();
                        let mut it = ColumnIter::new(col, out[oi].kind);
                        while let Some(c) = it.next_raw()? {
                            let v = it.value(&c);
                            out[oi].push(tt[c.tidx as usize], v);
                        }
                    }
                }
                i = j;
            }
        }
        Ok(out)
    }

    /// Streams every change of every signal in time order within `[t0, t1]`
    /// (VCD-style dump). The callback receives `(time, signal, value)`.
    /// Within one time step, changes are ordered by signal id.
    pub fn for_each_change(&self, t0: u64, t1: u64, mut f: impl FnMut(u64, SignalId, SignalValue<'_>)) -> Result<()> {
        // Entry tags (top two bits of `b`).
        const TAG_SHIFT: u32 = 30;
        const MASK: u32 = (1 << TAG_SHIFT) - 1;
        const TAG_COMPACT: u32 = 0; // bits, 2-state packing, data = col[a..b]
        const TAG_FULL: u32 = 1; // bits, declared packing
        const TAG_CODE: u32 = 2; // 1-bit code in `a`
        const TAG_REAL: u32 = 3; // IEEE bits split over `a` (low) and `b` (high 30 bits + tag)... see below
        #[derive(Clone, Copy, Default)]
        struct E {
            tidx: u32,
            sig: u32,
            a: u32,
            b: u32,
        }
        let start = self.block_at(t0).unwrap_or(0);
        let kinds = &self.hier.signals;
        let mut d = Decompressor::new();
        // (first signal of the run, decompressed run, per-signal column ranges)
        type RunPiece = (u32, Vec<u8>, Vec<(u32, u32)>);
        let mut pieces: Vec<RunPiece> = Vec::new();
        let mut sig_piece: Vec<u32> = Vec::new();
        let mut buckets: Vec<Vec<E>> = Vec::new();
        let mut local: Vec<E> = Vec::new();
        let mut reals: Vec<u64> = Vec::new();
        const COARSE_BITS: u32 = 10;
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
            sig_piece.resize(h.n_signals as usize, u32::MAX);
            let mut aliases: Vec<(u32, u32)> = Vec::new();
            for (g, clen, off) in block::dirty_groups(p, &h) {
                let c = block::group_container(p, &h, clen, off)?;
                let view = GroupView::parse(c, self.group_first(g), kinds)?;
                aliases.extend_from_slice(&view.aliases);
                for ri in 0..view.runs.len() {
                    let mut data = Vec::new();
                    let ranges = view.decode_run(ri, &mut d, &mut data)?;
                    if data.len() > MASK as usize {
                        return Err(Error::Corrupt("column run too large"));
                    }
                    let first = view.first_sig + view.runs[ri].0;
                    for k in 0..ranges.len() {
                        sig_piece[(first + k as u32) as usize] = pieces.len() as u32;
                    }
                    pieces.push((first, data, ranges));
                }
            }
            // Single decode pass producing self-contained entries, bucketed by coarse time.
            let n_coarse = (h.n_times as usize >> COARSE_BITS) + 1;
            if buckets.len() < n_coarse {
                buckets.resize_with(n_coarse, Vec::new);
            }
            for b in buckets.iter_mut() {
                b.clear();
            }
            reals.clear();
            macro_rules! push {
                ($e:expr) => {{
                    let e: E = $e;
                    buckets[(e.tidx >> COARSE_BITS) as usize].push(e);
                }};
            }
            // Aliased signals decode their target's column under their own id.
            let alias_jobs: Vec<(u32, usize, u32, u32)> = aliases
                .iter()
                .filter_map(|&(sig, target)| {
                    let pi = sig_piece[target as usize];
                    if pi == u32::MAX {
                        return None;
                    }
                    let (first, _, ranges) = &pieces[pi as usize];
                    let (a0, b0) = ranges[(target - first) as usize];
                    Some((sig, pi as usize, a0, b0))
                })
                .collect();
            for &(sig, target) in &aliases {
                sig_piece[sig as usize] = sig_piece[target as usize];
            }
            let own_jobs = pieces.iter().enumerate().flat_map(|(pi, (first, _, ranges))| {
                ranges.iter().enumerate().map(move |(k, &(a0, b0))| (*first + k as u32, pi, a0, b0))
            });
            for (sig, pi, a0, b0) in own_jobs.chain(alias_jobs.into_iter()) {
                {
                    if a0 == b0 {
                        continue;
                    }
                    let data = &pieces[pi].1;
                    let kind = kinds[sig as usize];
                    let col = &data[a0 as usize..b0 as usize];
                    let mut it = ColumnIter::new(col, kind);
                    match kind {
                        SignalKind::Bits { width: 1, .. } => {
                            while let Some(c) = it.next_raw()? {
                                push!(E { tidx: c.tidx, sig, a: c.inline as u32, b: TAG_CODE << TAG_SHIFT });
                            }
                        }
                        SignalKind::Bits { states, .. } => {
                            while let Some(c) = it.next_raw()? {
                                let tag = if c.states == 2 && states != 2 { TAG_COMPACT } else { TAG_FULL };
                                push!(E { tidx: c.tidx, sig, a: a0 + c.start, b: (a0 + c.end) | (tag << TAG_SHIFT) });
                            }
                        }
                        SignalKind::Real => {
                            // Reals: store the 64 IEEE bits across `a` and the value area position.
                            while let Some(c) = it.next_raw()? {
                                reals.push(c.inline);
                                push!(E { tidx: c.tidx, sig, a: (reals.len() - 1) as u32, b: TAG_REAL << TAG_SHIFT });
                            }
                        }
                        SignalKind::VarLen => {
                            while let Some(c) = it.next_raw()? {
                                push!(E { tidx: c.tidx, sig, a: a0 + c.start, b: (a0 + c.end) | (TAG_FULL << TAG_SHIFT) });
                            }
                        }
                    }
                }
            }
            let mut lcount = vec![0u32; (1usize << COARSE_BITS) + 1];
            for (cb, bucket) in buckets.iter().enumerate().take(n_coarse) {
                if bucket.is_empty() {
                    continue;
                }
                let base_t = (cb as u32) << COARSE_BITS;
                let lo_mask = (1u32 << COARSE_BITS) - 1;
                for c in lcount.iter_mut() {
                    *c = 0;
                }
                for e in bucket {
                    lcount[(e.tidx & lo_mask) as usize + 1] += 1;
                }
                for i in 0..(1usize << COARSE_BITS) {
                    lcount[i + 1] += lcount[i];
                }
                local.clear();
                local.resize(bucket.len(), E::default());
                {
                    let mut fill = lcount.clone();
                    for e in bucket {
                        let s = &mut fill[(e.tidx & lo_mask) as usize];
                        local[*s as usize] = *e;
                        *s += 1;
                    }
                }
                for li in 0..(1usize << COARSE_BITS) {
                    let (s, e) = (lcount[li] as usize, lcount[li + 1] as usize);
                    if s == e {
                        continue;
                    }
                    let ti = (base_t as usize) + li;
                    let t = times[ti];
                    if t < t0 || t > t1 {
                        continue;
                    }
                    for en in &local[s..e] {
                        let sig = en.sig;
                        let kind = kinds[sig as usize];
                        let (_, data, _) = &pieces[sig_piece[sig as usize] as usize];
                        let tag = en.b >> TAG_SHIFT;
                        let v = match tag {
                            TAG_CODE => {
                                let code = (en.a & 15) as usize;
                                SignalValue::Bits { width: 1, states: if code <= 1 { 2 } else { kind.states() }, data: &block::CODE_BYTES[code..code + 1] }
                            }
                            TAG_REAL => SignalValue::Real(f64::from_bits(reals[en.a as usize])),
                            _ => match kind {
                                SignalKind::Bits { width, states } => {
                                    SignalValue::Bits { width, states: if tag == TAG_COMPACT { 2 } else { states }, data: &data[en.a as usize..(en.b & MASK) as usize] }
                                }
                                _ => SignalValue::VarLen(&data[en.a as usize..(en.b & MASK) as usize]),
                            },
                        };
                        f(t, SignalId(sig), v);
                    }
                }
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
        self.hier.node(gen).parent
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
