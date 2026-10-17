//! Random-access reader over a memory-mapped VTR file.

use crate::block::{self, BlockHeader, ColumnIter, GroupView, NO_BLOCK};
use crate::clock::{self, ClockId, ClockInfo, ClockTimeline};
use crate::codec::Decompressor;
use crate::container::{Container, DirEntry, SectionKind};
use crate::ending::{self, Ending};
use crate::error::{Error, Result};
use crate::hierarchy::{Hierarchy, NodeId, NodeKind, SignalId, SignalKind};
use crate::logblock::{
    self, LogArgType, LogBlockData, LogBlockHeader, LogRecord, LogSite, Severity,
};
use crate::logfmt::ParsedFmt;
use crate::sections::{self, Blackout, Meta};
use crate::signal::{self, OwnedSignalValue, SignalValue};
use crate::strings::{StrId, StringTable};
use crate::txblock::{self, Relation, Transaction, TxBlockData, TxBlockHeader, TxId};
use crate::value::{Value, packed_len};
use crate::varint;
use crate::xform::Xform;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    pub verify_crc: bool,
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

struct LogBlock {
    entry: DirEntry,
    header: LogBlockHeader,
    data: OnceLock<Arc<LogBlockData>>,
}

pub(crate) struct Piece {
    data: Vec<u8>,
    ranges: Vec<(u32, u32)>,
    xform: Xform,
    plain: Vec<OnceLock<Box<[u8]>>>,
    index: Mutex<Vec<Option<Arc<Vec<block::Checkpoint>>>>>,
}

const INDEX_STRIDE: usize = 256;

const EAGER_UNTRANSFORM_COLUMNS: usize = 1;

impl Piece {
    fn new(
        mut data: Vec<u8>,
        mut ranges: Vec<(u32, u32)>,
        mut xform: Xform,
        first_sig: u32,
        kinds: &[SignalKind],
    ) -> Result<Piece> {
        if xform != Xform::None && ranges.len() <= EAGER_UNTRANSFORM_COLUMNS {
            block::untransform_run(&mut data, &mut ranges, first_sig, kinds, xform)?;
            xform = Xform::None;
        }
        let plain = if xform == Xform::None {
            Vec::new()
        } else {
            (0..ranges.len()).map(|_| OnceLock::new()).collect()
        };
        Ok(Piece {
            data,
            ranges,
            xform,
            plain,
            index: Mutex::new(Vec::new()),
        })
    }

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
        let i = Arc::new(block::build_index(
            self.col(local, kind)?,
            kind,
            INDEX_STRIDE,
        )?);
        idx[local] = Some(i.clone());
        Ok(i)
    }
}

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
            let (i, _) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.1)
                .unwrap();
            self.entries.swap_remove(i);
        }
        self.entries.push(((blk, grp, piece), self.tick, v));
    }
}

pub struct Reader {
    data: Data,
    container: Container,
    meta: Meta,
    strings: StringTable,
    hier: Hierarchy,
    sig_blocks: Vec<SigBlock>,
    tx_blocks: Vec<TxBlock>,
    log_blocks: Vec<LogBlock>,
    log_order: Vec<u32>,
    tx_order: Vec<(bool, u32)>,
    log_sites: Vec<LogSite>,
    log_fmts: Vec<ParsedFmt>,
    site_index: Vec<u32>,
    blackout: Vec<Blackout>,
    clocks: Vec<ClockInfo>,
    clock_keys: [Option<StrId>; 2],
    clock_timelines: Vec<OnceLock<Arc<ClockTimeline>>>,
    verify_crc: bool,
    cache: Mutex<GroupCache>,
    global_times: OnceLock<Vec<u64>>,
}

fn find_clocks(hier: &Hierarchy, strings: &StringTable) -> (Vec<ClockInfo>, [Option<StrId>; 2]) {
    const NAMES: [&str; 3] = [clock::KEY_PERIOD, clock::KEY_CLOCK, clock::GENERATOR_KIND];
    let mut ids: [Option<StrId>; 3] = [None; 3];
    for i in 0..strings.len() {
        let s = strings.get(StrId(i as u32));
        if let Some(k) = NAMES.iter().position(|n| *n == s) {
            ids[k].get_or_insert(StrId(i as u32));
        }
    }
    let keys = [ids[0], ids[1]];
    let Some(kind) = ids[2] else {
        return (Vec::new(), keys);
    };
    let mut clocks = Vec::new();
    for generator in hier.nodes_of_kind(NodeKind::Generator) {
        if !matches!(hier.node(generator).data, crate::NodeData::Generator { kind: k } if k == kind)
        {
            continue;
        }
        let mut parts = Vec::new();
        let mut cur = Some(generator);
        while let Some(c) = cur {
            parts.push(strings.get(hier.name(c)));
            cur = hier.parent(c);
        }
        parts.reverse();
        clocks.push(ClockInfo {
            id: ClockId(clocks.len() as u32),
            generator,
            path: parts.join("."),
        });
    }
    (clocks, keys)
}

#[derive(Clone, Debug)]
pub struct SignalData {
    inner: Arc<SignalDataInner>,
}

#[derive(Debug)]
struct SignalDataInner {
    kind: SignalKind,
    initial: Vec<u8>,
    times: Vec<u64>,
    data: Vec<u8>,
    offsets: Vec<u32>,
}

impl SignalData {
    #[must_use]
    pub fn kind(&self) -> SignalKind {
        self.inner.kind
    }

    #[must_use]
    pub fn times(&self) -> &[u64] {
        &self.inner.times
    }

    fn entry_len(&self) -> usize {
        self.inner.kind.packed_len().unwrap_or(0)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.times.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.times.is_empty()
    }

    #[must_use]
    pub fn get(&self, i: usize) -> SignalValue<'_> {
        match self.inner.kind {
            SignalKind::VarLen => SignalValue::VarLen(
                &self.inner.data
                    [self.inner.offsets[i] as usize..self.inner.offsets[i + 1] as usize],
            ),
            k => {
                let l = self.entry_len();
                block::frame_value(k, &self.inner.data[i * l..(i + 1) * l])
            }
        }
    }

    #[must_use]
    pub fn initial(&self) -> SignalValue<'_> {
        block::frame_value(self.inner.kind, &self.inner.initial)
    }

    #[must_use]
    pub fn index_at(&self, t: u64) -> Option<usize> {
        let n = self.inner.times.partition_point(|&x| x <= t);
        if n == 0 { None } else { Some(n - 1) }
    }

    #[must_use]
    pub fn value_at(&self, t: u64) -> SignalValue<'_> {
        match self.index_at(t) {
            Some(i) => self.get(i),
            None => self.initial(),
        }
    }
}

struct SignalDataBuilder {
    kind: SignalKind,
    initial: Vec<u8>,
    times: Vec<u64>,
    data: Vec<u8>,
    offsets: Vec<u32>,
}

impl SignalDataBuilder {
    fn new(kind: SignalKind) -> Self {
        let mut initial = Vec::new();
        signal::default_value(kind, &mut initial);
        Self {
            kind,
            initial,
            times: Vec::new(),
            data: Vec::new(),
            offsets: vec![0],
        }
    }

    fn finish(self) -> SignalData {
        SignalData {
            inner: Arc::new(SignalDataInner {
                kind: self.kind,
                initial: self.initial,
                times: self.times,
                data: self.data,
                offsets: self.offsets,
            }),
        }
    }

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

#[derive(Clone, Debug, Default)]
pub struct TxQuery {
    pub generator: Option<NodeId>,
    pub scope: Option<NodeId>,
    pub window: Option<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub struct LogQuery {
    pub scope: Option<NodeId>,
    pub generator: Option<NodeId>,
    pub min_severity: Severity,
    pub window: Option<(u64, u64)>,
}

impl Default for LogQuery {
    fn default() -> Self {
        LogQuery {
            scope: None,
            generator: None,
            min_severity: Severity::Trace,
            window: None,
        }
    }
}

impl Reader {
    pub fn clear_cache(&mut self) {
        let cache = self.cache.get_mut().unwrap();
        cache.entries = Vec::new();
        cache.tick = 0;
        for block in &mut self.sig_blocks {
            block.times.take();
        }
        for block in &mut self.tx_blocks {
            block.data.take();
        }
        for block in &mut self.log_blocks {
            block.data.take();
        }
        for timeline in &mut self.clock_timelines {
            timeline.take();
        }
        self.global_times.take();
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Reader> {
        Self::open_with(path, ReadOptions::default())
    }

    pub fn open_with(path: impl AsRef<Path>, opts: ReadOptions) -> Result<Reader> {
        let file = std::fs::File::open(path)?;
        // SAFETY: Trace files must remain immutable while readers hold mappings; the mapping owns its lifetime.
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        #[cfg(unix)]
        drop(mmap.advise(memmap2::Advice::Random));
        Self::from_data(Data::Mmap(mmap), opts)
    }

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
        let mut log_blocks = Vec::new();
        let mut tx_order = Vec::new();
        let mut blackout = Vec::new();
        let mut dec = Decompressor::new();
        for e in &container.entries {
            if SectionKind::from_u32(e.kind) == Some(SectionKind::Strings) {
                let raw = dec.decompress(Container::payload(bytes, e, verify)?)?;
                strings.add_chunk(&raw)?;
            }
        }
        for e in &container.entries {
            let Some(kind) = SectionKind::from_u32(e.kind) else {
                if e.flags & crate::container::SECTION_FLAG_OPTIONAL != 0 {
                    continue;
                }
                return Err(Error::Corrupt("unknown required section kind"));
            };
            match kind {
                SectionKind::Strings | SectionKind::Directory => {}
                SectionKind::Meta => {
                    meta = Some(Meta::decode(Container::payload(bytes, e, verify)?)?);
                }
                SectionKind::Hierarchy => {
                    let raw = dec.decompress(Container::payload(bytes, e, verify)?)?;
                    hier.add_chunk(&raw)?;
                }
                SectionKind::SignalBlock => {
                    let p = Container::payload(bytes, e, verify)?;
                    let header = BlockHeader::parse(p)?;
                    sig_blocks.push(SigBlock {
                        entry: *e,
                        header,
                        times: OnceLock::new(),
                    });
                }
                SectionKind::TxBlock => {
                    let p = Container::payload(bytes, e, verify)?;
                    let header = TxBlockHeader::parse(p)?;
                    tx_order.push((false, tx_blocks.len() as u32));
                    tx_blocks.push(TxBlock {
                        entry: *e,
                        header,
                        data: OnceLock::new(),
                    });
                }
                SectionKind::LogBlock => {
                    let p = Container::payload(bytes, e, verify)?;
                    let header = LogBlockHeader::parse(p)?;
                    tx_order.push((true, log_blocks.len() as u32));
                    log_blocks.push(LogBlock {
                        entry: *e,
                        header,
                        data: OnceLock::new(),
                    });
                }
                SectionKind::Blackout => {
                    blackout = sections::decode_blackout(Container::payload(bytes, e, verify)?)?;
                }
            }
        }
        hier.build_index();
        const KEYS: [&str; 6] = [
            logblock::KEY_SEVERITY,
            logblock::KEY_ARGS,
            logblock::KEY_NAMES,
            logblock::KEY_FILE,
            logblock::KEY_LINE,
            logblock::KEY_FUNC,
        ];
        let mut keys: [Vec<StrId>; 6] = Default::default();
        for i in 0..strings.len() {
            let s = strings.get(StrId(i as u32));
            if s.starts_with("log.") {
                for (k, n) in KEYS.iter().enumerate() {
                    if s == *n {
                        keys[k].push(StrId(i as u32));
                    }
                }
            }
        }
        let mut log_sites = Vec::new();
        let mut site_index = vec![u32::MAX; hier.len()];
        if !keys[1].is_empty() {
            for n in hier.nodes_of_kind(NodeKind::Generator) {
                if !matches!(hier.node_data(n), crate::NodeData::Generator { kind } if strings.get(kind) == logblock::GENERATOR_KIND)
                {
                    continue;
                }
                let attrs = hier.attrs(n);
                let Some((_, Value::List(types))) = attrs.iter().find(|(k, _)| keys[1].contains(k))
                else {
                    continue;
                };
                let mut args = Vec::with_capacity(types.len());
                for t in types {
                    match t {
                        Value::U64(v) => args.push(LogArgType::from_u8(*v as u8)?),
                        _ => return Err(Error::Corrupt("log.args entry is not an integer")),
                    }
                }
                let get = |k: &[StrId]| attrs.iter().find(|(kk, _)| k.contains(kk)).map(|(_, v)| v);
                let severity = match get(&keys[0]) {
                    Some(Value::U64(v)) => Severity::from_code(*v as u8),
                    _ => Severity::Info,
                };
                let names = match get(&keys[2]) {
                    Some(Value::List(l)) => l
                        .iter()
                        .filter_map(|v| {
                            if let Value::Str(s) = v {
                                Some(*s)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let file = match get(&keys[3]) {
                    Some(Value::Str(s)) => Some(*s),
                    _ => None,
                };
                let line = match get(&keys[4]) {
                    Some(Value::U64(v)) => Some(*v as u32),
                    _ => None,
                };
                let func = match get(&keys[5]) {
                    Some(Value::Str(s)) => Some(*s),
                    _ => None,
                };
                let scope = hier.parent(n);
                site_index[n.0 as usize] = log_sites.len() as u32;
                log_sites.push(LogSite {
                    index: log_sites.len() as u32,
                    node: n,
                    scope,
                    severity,
                    fmt: hier.name(n),
                    file,
                    line,
                    func,
                    args,
                    names,
                });
            }
        }
        let log_fmts: Vec<ParsedFmt> = log_sites
            .iter()
            .map(|s| ParsedFmt::parse(strings.get(s.fmt)))
            .collect();
        let mut log_order: Vec<u32> = (0..log_blocks.len() as u32).collect();
        log_order.sort_by_key(|&i| (log_blocks[i as usize].header.t_min, i));
        let (clocks, clock_keys) = find_clocks(&hier, &strings);
        let clock_timelines = clocks.iter().map(|_| OnceLock::new()).collect();
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
            log_blocks,
            log_order,
            tx_order,
            log_sites,
            log_fmts,
            site_index,
            blackout,
            clocks,
            clock_keys,
            clock_timelines,
            verify_crc: verify,
            cache: Mutex::new(GroupCache {
                cap,
                tick: 0,
                entries: Vec::new(),
            }),
            global_times: OnceLock::new(),
        })
    }

    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    pub fn version(&self) -> (u16, u16) {
        self.container.version
    }

    pub fn recovered(&self) -> Option<u64> {
        self.container.recovered
    }

    pub fn ending(&self) -> Result<(Ending, Option<u64>)> {
        let mut scopes: Vec<NodeId> = self
            .log_sites
            .iter()
            .filter_map(|s| s.scope)
            .filter(|&n| self.hierarchy().parent(n).is_none() && self.name(n) == ending::SCOPE)
            .collect();
        scopes.dedup();
        if !scopes.is_empty() {
            for i in (0..self.log_blocks.len()).rev() {
                if self.log_blocks[i].header.n_rec == 0 {
                    continue;
                }
                let mut may = false;
                for &st in &scopes {
                    may |= self.log_block_may_match(i, None, Some(st), Severity::Trace)?;
                }
                if !may {
                    continue;
                }
                let d = self.log_block(i)?;
                for rec in d.recs.iter().rev() {
                    let Some(site) = self
                        .log_site(NodeId(rec.r#gen))
                        .filter(|s| s.scope.is_some_and(|scope| scopes.contains(&scope)))
                    else {
                        continue;
                    };
                    let args: Vec<_> = d.args(rec).collect();
                    let end = Ending::from_record(self.str(site.fmt), &args)
                        .ok_or(Error::Corrupt("malformed vtr.run ending record"))?;
                    return Ok((end, Some(rec.time)));
                }
            }
        }
        Ok((
            self.recovered()
                .map_or(Ending::Closed, |dropped| Ending::Recovered { dropped }),
            None,
        ))
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
        self.hier
            .signal_kind(s)
            .ok_or_else(|| Error::invalid(format!("unknown signal {}", s.0)))
    }

    pub fn signal_var_type(&self, s: SignalId) -> Result<crate::hierarchy::VarType> {
        self.hier
            .signal_var_type(s)
            .ok_or_else(|| Error::invalid(format!("unknown signal {}", s.0)))
    }

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

    pub fn name(&self, n: NodeId) -> &str {
        self.str(self.hier.name(n))
    }

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

    pub fn find_node(&self, path: &[&str]) -> Option<NodeId> {
        let mut level: Vec<NodeId> = self.hier.roots().collect();
        let mut found = None;
        for comp in path {
            found = level.iter().copied().find(|&n| self.name(n) == *comp);
            level = self.hier.children(found?).collect();
        }
        found
    }

    pub fn find_signal(&self, path: &str, sep: char) -> Option<SignalId> {
        let parts: Vec<&str> = path.split(sep).collect();
        let n = self.find_node(&parts)?;
        self.hier.signal_of(n)
    }

    pub fn generators(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.hier.nodes_of_kind(NodeKind::Generator)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.data.bytes()
    }

    pub(crate) fn last_record_id(&self) -> u64 {
        let tx = self
            .tx_blocks
            .iter()
            .filter(|b| b.header.n_tx > 0)
            .map(|b| b.header.max_id);
        tx.chain(
            self.log_blocks
                .iter()
                .filter(|b| b.header.n_rec > 0)
                .map(|b| b.header.max_id),
        )
        .max()
        .unwrap_or(0)
    }

    pub(crate) fn last_signal_time(&self) -> Option<u64> {
        self.sig_blocks
            .iter()
            .rev()
            .find(|b| b.header.n_times > 0)
            .map(|b| b.header.end_time)
    }

    pub fn block_count(&self) -> usize {
        self.sig_blocks.len()
    }

    pub fn block_range(&self, i: usize) -> (u64, u64) {
        let h = &self.sig_blocks[i].header;
        (h.start_time, h.end_time)
    }

    fn block_payload(&self, b: &SigBlock) -> Result<&[u8]> {
        Container::payload(self.bytes(), &b.entry, false)
    }

    pub(crate) fn signal_block(&self, i: usize) -> Result<(&[u8], BlockHeader, &[SignalKind])> {
        let b = &self.sig_blocks[i];
        Ok((self.block_payload(b)?, b.header, self.block_kinds(b)))
    }

    pub(crate) fn signal_block_bytes(&self, i: usize) -> u64 {
        self.sig_blocks[i].entry.len
    }

    pub fn block_times(&self, i: usize) -> Result<Arc<Vec<u64>>> {
        let b = &self.sig_blocks[i];
        if let Some(t) = b.times.get() {
            return Ok(t.clone());
        }
        let p = self.block_payload(b)?;
        let t = Arc::new(block::decode_time_table(
            p,
            &b.header,
            &mut Decompressor::new(),
        )?);
        drop(b.times.set(t));
        Ok(b.times.get().unwrap().clone())
    }

    pub fn time_table(&self) -> Result<&[u64]> {
        if let Some(t) = self.global_times.get() {
            return Ok(t);
        }
        let mut all = Vec::new();
        for i in 0..self.sig_blocks.len() {
            let t = self.block_times(i)?;
            let mut s = 0;
            if let (Some(&last), Some(&first)) = (all.last(), t.first())
                && first == last
            {
                s = 1;
            }
            all.extend_from_slice(&t[s..]);
        }
        drop(self.global_times.set(all));
        Ok(self.global_times.get().unwrap())
    }

    fn block_at(&self, t: u64) -> Option<usize> {
        let n = self
            .sig_blocks
            .partition_point(|b| b.header.start_time <= t);
        if n == 0 { None } else { Some(n - 1) }
    }

    fn group_of(&self, s: SignalId) -> u32 {
        s.0 / self.meta.group_size.max(1)
    }

    pub(crate) fn group_first(&self, g: u32) -> u32 {
        g * self.meta.group_size.max(1)
    }

    fn block_kinds(&self, b: &SigBlock) -> &[SignalKind] {
        let kinds = &self.hier.signals;
        &kinds[..(b.header.n_signals as usize).min(kinds.len())]
    }

    fn group_view(&self, bi: usize, g: u32) -> Result<Option<GroupView<'_>>> {
        let b = &self.sig_blocks[bi];
        let p = self.block_payload(b)?;
        let Some((clen, off)) = block::find_group(p, &b.header, g) else {
            return Ok(None);
        };
        let c = block::group_container(p, &b.header, clen, off)?;
        Ok(Some(GroupView::parse(
            c,
            self.group_first(g),
            self.block_kinds(b),
        )?))
    }

    fn piece(&self, bi: usize, g: u32, view: &GroupView<'_>, piece: u32) -> Result<Arc<Piece>> {
        if let Some(p) = self.cache.lock().unwrap().get(bi as u32, g, piece) {
            return Ok(p);
        }
        let mut d = Decompressor::new();
        let mut data = Vec::new();
        let (ranges, xform, first_sig) = if piece == 0 {
            (
                view.decode_frames(&self.hier.signals, &mut d, &mut data)?,
                Xform::None,
                view.first_sig,
            )
        } else {
            let run = view.runs[piece as usize - 1];
            (
                view.decode_run(piece as usize - 1, &mut d, &mut data)?,
                run.xform,
                view.first_sig + run.first_local,
            )
        };
        let p = Arc::new(Piece::new(
            data,
            ranges,
            xform,
            first_sig,
            &self.hier.signals,
        )?);
        self.cache
            .lock()
            .unwrap()
            .put(bi as u32, g, piece, p.clone());
        Ok(p)
    }

    fn column(
        &self,
        bi: usize,
        g: u32,
        view: &GroupView<'_>,
        sig: u32,
    ) -> Result<(Arc<Piece>, usize)> {
        if let Some(target) = view.alias_of(sig) {
            let tg = self.group_of(SignalId(target));
            if tg == g {
                if !view.holds(target) {
                    return Err(Error::Corrupt("alias target not in block"));
                }
                return self.column_direct(bi, g, view, target);
            }
            let tv = self
                .group_view(bi, tg)?
                .ok_or(Error::Corrupt("alias target group missing"))?;
            if !tv.holds(target) {
                return Err(Error::Corrupt("alias target not in block"));
            }
            return self.column_direct(bi, tg, &tv, target);
        }
        self.column_direct(bi, g, view, sig)
    }

    fn column_direct(
        &self,
        bi: usize,
        g: u32,
        view: &GroupView<'_>,
        sig: u32,
    ) -> Result<(Arc<Piece>, usize)> {
        let ri = view.run_of(sig);
        let p = self.piece(bi, g, view, ri as u32 + 1)?;
        let local = (sig - view.first_sig - view.runs[ri].first_local) as usize;
        Ok((p, local))
    }

    fn frame(
        &self,
        bi: usize,
        g: u32,
        view: &GroupView<'_>,
        sig: u32,
    ) -> Result<(Arc<Piece>, (u32, u32))> {
        let p = self.piece(bi, g, view, 0)?;
        let range = p.ranges[(sig - view.first_sig) as usize];
        Ok((p, range))
    }

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

    pub fn value_at(&self, sig: SignalId, t: u64) -> Result<OwnedSignalValue> {
        let kind = self.signal_kind(sig)?;
        let g = self.group_of(sig);
        let default = || {
            let mut v = Vec::new();
            signal::default_value(kind, &mut v);
            block::frame_value(kind, &v).to_owned()
        };
        let Some(bi) = self.block_at(t) else {
            return Ok(default());
        };
        let Some(dbi) = self.dirty_block_at_or_before(bi, g)? else {
            return Ok(default());
        };
        let view = self.group_view(dbi, g)?.unwrap();
        if !view.holds(sig.0) {
            return Ok(default());
        }
        let max_tidx = if dbi == bi {
            let times = self.block_times(bi)?;
            let n = times.partition_point(|&x| x <= t);
            if n == 0 {
                let (fp, fr) = self.frame(dbi, g, &view, sig.0)?;
                return Ok(
                    block::frame_value(kind, &fp.data[fr.0 as usize..fr.1 as usize]).to_owned(),
                );
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
        if let Some(c) = it.last_at_or_before(max_tidx)? {
            Ok(it.value(&c).to_owned())
        } else {
            let (fp, fr) = self.frame(dbi, g, &view, sig.0)?;
            Ok(block::frame_value(kind, &fp.data[fr.0 as usize..fr.1 as usize]).to_owned())
        }
    }

    pub fn changes(&self, sig: SignalId, t0: u64, t1: u64) -> Result<Vec<(u64, OwnedSignalValue)>> {
        let kind = self.signal_kind(sig)?;
        let g = self.group_of(sig);
        let mut out = Vec::new();
        let start = self.sig_blocks.partition_point(|b| b.header.end_time < t0);
        for bi in start..self.sig_blocks.len() {
            let h = &self.sig_blocks[bi].header;
            if h.start_time > t1 {
                break;
            }
            if h.end_time < t0 {
                continue;
            }
            let view = match self.group_view(bi, g)? {
                Some(v) if v.holds(sig.0) => v,
                _ => continue,
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

    pub(crate) fn changing(&self, sigs: &[SignalId], t0: u64, t1: u64) -> Result<Vec<bool>> {
        let mut hit = vec![false; sigs.len()];
        let mut left = sigs.len();
        let start = self.sig_blocks.partition_point(|b| b.header.end_time < t0);
        for bi in start..self.sig_blocks.len() {
            if left == 0 || self.sig_blocks[bi].header.start_time > t1 {
                break;
            }
            let times = self.block_times(bi)?;
            let (lo, hi) = (
                times.partition_point(|&t| t < t0) as u32,
                times.partition_point(|&t| t <= t1) as u32,
            );
            if lo == hi {
                continue;
            }
            let mut i = 0;
            while i < sigs.len() {
                let g = self.group_of(sigs[i]);
                let j = i + sigs[i..].partition_point(|&s| self.group_of(s) == g);
                let view = if hit[i..j].contains(&false) {
                    self.group_view(bi, g)?
                } else {
                    None
                };
                for k in i..j {
                    let Some(view) = view.as_ref().filter(|v| !hit[k] && v.holds(sigs[k].0)) else {
                        continue;
                    };
                    let kind = self.signal_kind(sigs[k])?;
                    let (cp, local) = self.column(bi, g, view, sigs[k].0)?;
                    let col = cp.col(local, kind)?;
                    let mut it = ColumnIter::new(col, kind);
                    if col.len() > 4096 {
                        it.seek(&cp.column_index(local, kind)?, lo);
                    }
                    while let Some(c) = it.next_raw()? {
                        if c.tidx >= lo {
                            if c.tidx < hi {
                                hit[k] = true;
                                left -= 1;
                            }
                            break;
                        }
                    }
                }
                i = j;
            }
        }
        Ok(hit)
    }

    pub fn run_stats(&self) -> Result<[(u64, u64); 5]> {
        let mut out = [(0u64, 0u64); 5];
        for b in &self.sig_blocks {
            let p = self.block_payload(b)?;
            for (g, clen, off) in block::dirty_groups(p, &b.header) {
                let c = block::group_container(p, &b.header, clen, off)?;
                let view = GroupView::parse(c, self.group_first(g), self.block_kinds(b))?;
                for r in &view.runs {
                    let e = &mut out[(r.xform as u8 as usize).min(4)];
                    e.0 += 1;
                    e.1 += r.blob.len() as u64;
                }
            }
        }
        Ok(out)
    }

    pub fn load_signal(&self, sig: SignalId) -> Result<SignalData> {
        Ok(self.load_signals(&[sig])?.pop().unwrap())
    }

    pub fn load_signals(&self, sigs: &[SignalId]) -> Result<Vec<SignalData>> {
        if sigs.is_empty() {
            return Ok(Vec::new());
        }
        let mut order: Vec<usize> = (0..sigs.len()).collect();
        order.sort_unstable_by_key(|&i| sigs[i].0);
        let mut unique = Vec::new();
        let mut out = Vec::new();
        let mut request_to_unique = vec![0; sigs.len()];
        for oi in order {
            let s = sigs[oi];
            if unique.last() != Some(&s) {
                out.push(SignalDataBuilder::new(self.signal_kind(s)?));
                unique.push(s);
            }
            request_to_unique[oi] = unique.len() - 1;
        }
        let sigs = unique.as_slice();
        let mut first_seen = vec![false; sigs.len()];
        for bi in 0..self.sig_blocks.len() {
            let mut times: Option<Arc<Vec<u64>>> = None;
            let mut i = 0;
            while i < sigs.len() {
                let g = self.group_of(sigs[i]);
                let mut j = i;
                while j < sigs.len() && self.group_of(sigs[j]) == g {
                    j += 1;
                }
                if let Some(view) = self.group_view(bi, g)? {
                    let mut frames: Option<Arc<Piece>> = None;
                    let mut run: Option<(usize, Arc<Piece>)> = None;
                    for oi in i..j {
                        let s = sigs[oi];
                        if !view.holds(s.0) {
                            continue;
                        }
                        if !first_seen[oi] {
                            first_seen[oi] = true;
                            if frames.is_none() {
                                frames = Some(self.piece(bi, g, &view, 0)?);
                            }
                            let fp = frames.as_ref().unwrap();
                            let fr = fp.ranges[(s.0 - view.first_sig) as usize];
                            out[oi].initial.clear();
                            out[oi]
                                .initial
                                .extend_from_slice(&fp.data[fr.0 as usize..fr.1 as usize]);
                        }
                        let (rp, local) = if view.alias_of(s.0).is_some() {
                            self.column(bi, g, &view, s.0)?
                        } else {
                            let ri = view.run_of(s.0);
                            if run.as_ref().map(|(r, _)| *r) != Some(ri) {
                                run = Some((ri, self.piece(bi, g, &view, ri as u32 + 1)?));
                            }
                            let rp = run.as_ref().unwrap().1.clone();
                            (
                                rp,
                                (s.0 - view.first_sig - view.runs[ri].first_local) as usize,
                            )
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
        let loaded: Vec<_> = out.into_iter().map(SignalDataBuilder::finish).collect();
        Ok(request_to_unique
            .into_iter()
            .map(|i| loaded[i].clone())
            .collect())
    }

    pub fn for_each_change(
        &self,
        t0: u64,
        t1: u64,
        mut f: impl FnMut(u64, SignalId, SignalValue<'_>),
    ) -> Result<()> {
        const NONE: u32 = u32::MAX;
        const MERGE_MAX_COLUMNS: usize = 1 << 15;
        const MIN_WINDOW_ENTRIES: usize = 1 << 18;
        const ENTRIES_PER_COLUMN: usize = 16;
        const MAX_WINDOW_ENTRIES: usize = 1 << 23;
        const MAX_WINDOW_STEPS: usize = 1 << 12;
        const TAG_SHIFT: u32 = 30;
        const MASK: u32 = (1 << TAG_SHIFT) - 1;
        const TAG_COMPACT: u32 = 0;
        const TAG_FULL: u32 = 1;
        const TAG_CODE: u32 = 2;
        const TAG_REAL: u32 = 3;
        #[derive(Clone, Copy, Default)]
        struct E {
            tidx: u32,
            sig: u32,
            a: u32,
        }
        struct Cursor<'a> {
            sig: u32,
            base: u32,
            it: ColumnIter<'a>,
            pending: block::RawChange,
            next: u32,
        }
        fn drain(
            cur: &mut Cursor<'_>,
            w1: u32,
            next: &mut u32,
            entries: &mut Vec<E>,
            mut tag: impl FnMut(&block::RawChange) -> u32,
        ) -> Result<()> {
            let (mut it, mut c, sig) = (cur.it, cur.pending, cur.sig);
            let r = loop {
                entries.push(E {
                    tidx: c.tidx,
                    sig,
                    a: tag(&c),
                });
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
        let start = self.sig_blocks.partition_point(|b| b.header.end_time < t0);
        let kinds = &self.hier.signals;
        let mut d = Decompressor::new();
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
                let view = GroupView::parse(
                    c,
                    self.group_first(g),
                    self.block_kinds(&self.sig_blocks[bi]),
                )?;
                aliases.extend_from_slice(&view.aliases);
                for ri in 0..view.runs.len() {
                    let mut data = Vec::new();
                    let mut ranges = view.decode_run(ri, &mut d, &mut data)?;
                    let first = view.first_sig + view.runs[ri].first_local;
                    block::untransform_run(
                        &mut data,
                        &mut ranges,
                        first,
                        kinds,
                        view.runs[ri].xform,
                    )?;
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
            fn column<'p>(
                pieces: &'p [RunPiece],
                sig_piece: &[u32],
                sig: u32,
            ) -> Option<(&'p [u8], u32)> {
                let pi = sig_piece[sig as usize];
                if pi == NONE {
                    return None;
                }
                let (first, data, ranges) = &pieces[pi as usize];
                let (a, b) = ranges[(sig - first) as usize];
                (a != b).then(|| (&data[a as usize..b as usize], a))
            }
            let own: Vec<(u32, (&[u8], u32))> = (0..h.n_signals)
                .filter_map(|sig| column(&pieces, &sig_piece, sig).map(|c| (sig, c)))
                .collect();
            let aliased: Vec<(u32, (&[u8], u32))> = aliases
                .iter()
                .filter_map(|&(sig, target)| column(&pieces, &sig_piece, target).map(|c| (sig, c)))
                .collect();
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
                    cursors.push(Cursor {
                        sig,
                        base,
                        it,
                        pending,
                        next: NONE,
                    });
                }
            }
            let n_times = h.n_times as usize;
            if cursors.len() <= MERGE_MAX_COLUMNS {
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
                                Some(pending) if pending.tidx as usize == ti => {
                                    cur.pending = pending;
                                }
                                Some(pending) => {
                                    if pending.tidx >= h.n_times || (pending.tidx as usize) < ti {
                                        return Err(Error::Corrupt(
                                            "time index out of order in column",
                                        ));
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
            let mut next_tidx: Vec<u32> = cursors.iter().map(|c| c.pending.tidx).collect();
            let window_entries =
                (ENTRIES_PER_COLUMN * cursors.len()).clamp(MIN_WINDOW_ENTRIES, MAX_WINDOW_ENTRIES);
            let mut density = (raw_bytes / 2 / n_times.max(1)).max(1);
            let mut w0 = 0usize;
            while w0 < n_times && times[w0] <= t1 {
                let steps = (window_entries / density)
                    .clamp(1, MAX_WINDOW_STEPS)
                    .min(n_times - w0);
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
                        SignalKind::Bits { width: 1, .. } => {
                            drain(cur, w1 as u32, next, &mut entries, |c| {
                                c.inline as u32 | (TAG_CODE << TAG_SHIFT)
                            })?;
                        }
                        SignalKind::Bits { states, .. } => {
                            drain(cur, w1 as u32, next, &mut entries, |c| {
                                let tag = if c.states == 2 && states != 2 {
                                    TAG_COMPACT
                                } else {
                                    TAG_FULL
                                };
                                (base + c.start) | (tag << TAG_SHIFT)
                            })?;
                        }
                        SignalKind::Real => drain(cur, w1 as u32, next, &mut entries, |c| {
                            reals.push(c.inline);
                            (reals.len() - 1) as u32 | (TAG_REAL << TAG_SHIFT)
                        })?,
                        SignalKind::VarLen => drain(cur, w1 as u32, next, &mut entries, |c| {
                            (base + c.inline as u32) | (TAG_FULL << TAG_SHIFT)
                        })?,
                    }
                }
                density = (entries.len() / steps).max(1);
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
                        TAG_CODE => SignalValue::Bits {
                            width: 1,
                            states: if a <= 1 { 2 } else { kind.states() },
                            data: &block::CODE_BYTES[a..=a],
                        },
                        TAG_REAL => SignalValue::Real(f64::from_bits(reals[a])),
                        _ => {
                            let data = &pieces[sig_piece[sig as usize] as usize].1;
                            if let SignalKind::Bits { width, states } = kind {
                                let (st, len) = if tag == TAG_COMPACT {
                                    (2, (width as usize).div_ceil(8))
                                } else {
                                    (states, packed_len(width, states))
                                };
                                SignalValue::Bits {
                                    width,
                                    states: st,
                                    data: &data[a..a + len],
                                }
                            } else {
                                let mut r = varint::Reader::new(data);
                                r.pos = a;
                                SignalValue::VarLen(r.blob()?)
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
        drop(b.data.set(d));
        Ok(b.data.get().unwrap().clone())
    }

    fn in_scope(&self, node: NodeId, scope: NodeId) -> bool {
        let mut parent = self.hier.parent(node);
        while let Some(id) = parent {
            if id == scope {
                return true;
            }
            parent = self.hier.parent(id);
        }
        false
    }

    pub fn visit_transactions(
        &self,
        q: &TxQuery,
        mut f: impl FnMut(&Transaction) -> bool,
    ) -> Result<()> {
        for &(is_log, idx) in &self.tx_order {
            let i = idx as usize;
            if is_log {
                if !self.visit_log_block_as_tx(i, q, &mut f)? {
                    return Ok(());
                }
                continue;
            }
            let h = &self.tx_blocks[i].header;
            if h.n_tx == 0 {
                continue;
            }
            if let Some((t0, t1)) = q.window
                && (h.t_max < t0 || h.t_min > t1)
            {
                continue;
            }
            if let Some(g) = q.generator {
                let p = Container::payload(self.bytes(), &self.tx_blocks[i].entry, false)?;
                if !txblock::block_generators(p, h).any(|x| x == g.0) {
                    continue;
                }
            }
            let d = self.tx_block(i)?;
            for tx in &d.transactions {
                if let Some(g) = q.generator
                    && tx.generator != g
                {
                    continue;
                }
                if let Some(s) = q.scope
                    && !self.in_scope(tx.generator, s)
                {
                    continue;
                }
                if let Some((t0, t1)) = q.window
                    && (tx.end < t0 || tx.begin > t1)
                {
                    continue;
                }
                if !f(tx) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    pub fn transactions(&self, q: &TxQuery) -> Result<Vec<Transaction>> {
        let mut v = Vec::new();
        self.visit_transactions(q, |t| {
            v.push(t.clone());
            true
        })?;
        Ok(v)
    }

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
        for i in 0..self.log_blocks.len() {
            let h = &self.log_blocks[i].header;
            if h.n_rec == 0 || id < h.min_id || id > h.max_id {
                continue;
            }
            let d = self.log_block(i)?;
            if let Some(rec) = d.recs.iter().find(|r| r.id == id)
                && let Some(site) = self.log_site(NodeId(rec.r#gen))
            {
                return Ok(Some(
                    LogRecord::new(site, &self.log_fmts[site.index as usize], rec, &d)
                        .to_transaction(),
                ));
            }
        }
        Ok(None)
    }

    pub fn transaction_generator(&self, id: TxId) -> Result<Option<NodeId>> {
        for i in 0..self.tx_blocks.len() {
            let h = &self.tx_blocks[i].header;
            if h.n_tx == 0 || id < h.min_id || id > h.max_id {
                continue;
            }
            let d = self.tx_block(i)?;
            if let Some(t) = d.transactions.iter().find(|t| t.id == id) {
                return Ok(Some(t.generator));
            }
        }
        for i in 0..self.log_blocks.len() {
            let h = &self.log_blocks[i].header;
            if h.n_rec == 0 || id < h.min_id || id > h.max_id {
                continue;
            }
            let d = self.log_block(i)?;
            if let Some(rec) = d.recs.iter().find(|r| r.id == id) {
                return Ok(Some(NodeId(rec.r#gen)));
            }
        }
        Ok(None)
    }

    pub fn transaction_generators(&self, ids: &[TxId]) -> Result<Vec<Option<NodeId>>> {
        let mut requested: Vec<_> = ids.to_vec();
        requested.sort_unstable();
        requested.dedup();
        let mut owners: std::collections::HashMap<_, _> =
            requested.iter().map(|&id| (id, None)).collect();
        let mut remaining = requested.len();
        let intersects = |min, max| {
            requested
                .get(requested.partition_point(|&id| id < min))
                .is_some_and(|&id| id <= max)
        };
        for i in 0..self.tx_blocks.len() {
            if remaining == 0 {
                break;
            }
            let h = &self.tx_blocks[i].header;
            if h.n_tx == 0 || !intersects(h.min_id, h.max_id) {
                continue;
            }
            for tx in &self.tx_block(i)?.transactions {
                if let Some(owner) = owners.get_mut(&tx.id).filter(|owner| owner.is_none()) {
                    *owner = Some(tx.generator);
                    remaining -= 1;
                }
            }
        }
        for i in 0..self.log_blocks.len() {
            if remaining == 0 {
                break;
            }
            let h = &self.log_blocks[i].header;
            if h.n_rec == 0 || !intersects(h.min_id, h.max_id) {
                continue;
            }
            for record in &self.log_block(i)?.recs {
                if let Some(owner) = owners.get_mut(&record.id).filter(|owner| owner.is_none()) {
                    *owner = Some(NodeId(record.r#gen));
                    remaining -= 1;
                }
            }
        }
        Ok(ids.iter().map(|id| owners[id]).collect())
    }

    pub fn clocks(&self) -> &[ClockInfo] {
        &self.clocks
    }

    pub fn find_clock(&self, path: &str) -> Option<ClockId> {
        self.clocks.iter().find(|c| c.path == path).map(|c| c.id)
    }

    pub fn generator_clock(&self, generator: NodeId) -> Option<ClockId> {
        if generator.0 as usize >= self.hier.len()
            || self.hier.kind(generator) != NodeKind::Generator
        {
            return None;
        }
        let key = self.clock_keys[1]?;
        self.hier
            .attrs(generator)
            .iter()
            .find_map(|(k, v)| match v {
                Value::Str(s) if *k == key => self.find_clock(self.str(*s)),
                Value::Text(s) if *k == key => self.find_clock(s),
                _ => None,
            })
    }

    pub fn clock(&self, id: ClockId) -> Result<Arc<ClockTimeline>> {
        let info = self
            .clocks
            .get(id.0 as usize)
            .ok_or_else(|| Error::invalid(format!("unknown clock {}", id.0)))?;
        let slot = &self.clock_timelines[id.0 as usize];
        if let Some(t) = slot.get() {
            return Ok(t.clone());
        }
        let key = self.clock_keys[0];
        let mut raw = Vec::new();
        let mut open = false;
        for (i, b) in self.tx_blocks.iter().enumerate() {
            if b.header.n_tx == 0 {
                continue;
            }
            let p = Container::payload(self.bytes(), &b.entry, false)?;
            if !txblock::block_generators(p, &b.header).any(|g| g == info.generator.0) {
                continue;
            }
            for tx in self
                .tx_block(i)?
                .transactions
                .iter()
                .filter(|t| t.generator == info.generator)
            {
                let period =
                    tx.attrs
                        .iter()
                        .find(|a| Some(a.key) == key)
                        .map_or(Ok(0), |a| match a.value {
                            Value::Time(p) | Value::U64(p) => Ok(p),
                            _ => Err(Error::Corrupt("vtr.period is not a time")),
                        })?;
                open |= tx.status == crate::TxStatus::Open;
                raw.push((tx.begin, tx.end, period));
            }
        }
        let timeline = Arc::new(ClockTimeline::new(raw, open)?);
        Ok(slot.get_or_init(|| timeline).clone())
    }

    pub fn log_sites(&self) -> &[LogSite] {
        &self.log_sites
    }

    pub fn log_site(&self, r#gen: NodeId) -> Option<&LogSite> {
        match self.site_index.get(r#gen.0 as usize) {
            Some(&i) if i != u32::MAX => Some(&self.log_sites[i as usize]),
            _ => None,
        }
    }

    pub fn log_block_count(&self) -> usize {
        self.log_blocks.len()
    }

    pub fn log_count(&self) -> u64 {
        self.log_blocks.iter().map(|b| b.header.n_rec).sum()
    }

    fn log_block(&self, i: usize) -> Result<Arc<LogBlockData>> {
        let b = &self.log_blocks[i];
        if let Some(d) = b.data.get() {
            return Ok(d.clone());
        }
        let p = Container::payload(self.bytes(), &b.entry, self.verify_crc)?;
        let d = Arc::new(logblock::decode_log_block(
            p,
            &mut Decompressor::new(),
            |g| self.log_site(NodeId(g)).map(|s| s.args.as_slice()),
        )?);
        drop(b.data.set(d));
        Ok(b.data.get().unwrap().clone())
    }

    fn log_block_may_match(
        &self,
        i: usize,
        generator: Option<NodeId>,
        scope: Option<NodeId>,
        min: Severity,
    ) -> Result<bool> {
        let b = &self.log_blocks[i];
        let p = Container::payload(self.bytes(), &b.entry, false)?;
        for g in logblock::block_generators(p, &b.header) {
            if generator.is_some_and(|r#gen| g != r#gen.0) {
                continue;
            }
            if let Some(s) = self.log_site(NodeId(g)) {
                if s.severity < min || scope.is_some_and(|st| !self.in_scope(s.node, st)) {
                    continue;
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn visit_log(&self, q: &LogQuery, mut f: impl FnMut(&LogRecord) -> bool) -> Result<()> {
        for &bi in &self.log_order {
            let i = bi as usize;
            let h = &self.log_blocks[i].header;
            if h.n_rec == 0 {
                continue;
            }
            if let Some((t0, t1)) = q.window
                && (h.t_max < t0 || h.t_min > t1)
            {
                continue;
            }
            if !self.log_block_may_match(i, q.generator, q.scope, q.min_severity)? {
                continue;
            }
            let d = self.log_block(i)?;
            for rec in &d.recs {
                if q.generator.is_some_and(|g| rec.r#gen != g.0) {
                    continue;
                }
                let Some(site) = self.log_site(NodeId(rec.r#gen)) else {
                    continue;
                };
                if site.severity < q.min_severity
                    || q.scope.is_some_and(|st| !self.in_scope(site.node, st))
                {
                    continue;
                }
                if let Some((t0, t1)) = q.window
                    && (rec.time < t0 || rec.time > t1)
                {
                    continue;
                }
                if !f(&LogRecord::new(
                    site,
                    &self.log_fmts[site.index as usize],
                    rec,
                    &d,
                )) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn visit_log_block_as_tx(
        &self,
        i: usize,
        q: &TxQuery,
        f: &mut impl FnMut(&Transaction) -> bool,
    ) -> Result<bool> {
        let h = &self.log_blocks[i].header;
        if h.n_rec == 0 {
            return Ok(true);
        }
        if let Some((t0, t1)) = q.window
            && (h.t_max < t0 || h.t_min > t1)
        {
            return Ok(true);
        }
        if !self.log_block_may_match(i, q.generator, q.scope, Severity::Trace)? {
            return Ok(true);
        }
        let d = self.log_block(i)?;
        for rec in &d.recs {
            if q.generator.is_some_and(|g| rec.r#gen != g.0) {
                continue;
            }
            let Some(site) = self.log_site(NodeId(rec.r#gen)) else {
                continue;
            };
            if q.scope.is_some_and(|st| !self.in_scope(site.node, st)) {
                continue;
            }
            if let Some((t0, t1)) = q.window
                && (rec.time < t0 || rec.time > t1)
            {
                continue;
            }
            if !f(
                &LogRecord::new(site, &self.log_fmts[site.index as usize], rec, &d)
                    .to_transaction(),
            ) {
                return Ok(false);
            }
        }
        Ok(true)
    }

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

    pub fn tx_counts(&self) -> (u64, u64) {
        let (a, b) = self
            .tx_blocks
            .iter()
            .fold((0, 0), |(a, b), t| (a + t.header.n_tx, b + t.header.n_rel));
        (a + self.log_count(), b)
    }

    pub fn sections(&self) -> &[DirEntry] {
        &self.container.entries
    }

    pub fn packed_len(&self, s: SignalId) -> Option<usize> {
        self.hier.signal_kind(s).and_then(|k| match k {
            SignalKind::Bits { width, states } => Some(packed_len(width, states)),
            SignalKind::Real => Some(8),
            SignalKind::VarLen => None,
        })
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::{
        Direction, LogArg, LogArgType, LogSiteSpec, Severity, TxStatus, VarType, Writer,
        WriterOptions,
    };

    #[test]
    fn eviction_releases_blocks_and_preserves_owned_results() {
        let path = std::env::temp_dir().join(format!("vtr-cache-{}.vtr", std::process::id()));
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                background: false,
                ..Default::default()
            },
        )
        .unwrap();
        let (_, signal) = writer
            .add_var(
                None,
                "counter",
                VarType::Wire,
                Direction::Implicit,
                SignalKind::Bits {
                    width: 8,
                    states: 2,
                },
            )
            .unwrap();
        let stream = writer
            .add_scope(None, "requests", crate::ScopeType::Generic, "")
            .unwrap();
        let generator = writer
            .add_generator(Some(stream), "request", "test")
            .unwrap();
        let site = writer
            .add_log_site(&LogSiteSpec::new(
                Some(stream),
                Severity::Info,
                "value={}",
                &[LogArgType::U64],
            ))
            .unwrap();
        writer.set_time(1).unwrap();
        writer.emit_u64(signal, 42).unwrap();
        let tx = writer.begin_tx(generator, 1).unwrap();
        writer.end_tx(tx, 2, TxStatus::Ok).unwrap();
        writer.log(site, 2, &[LogArg::U64(42)]).unwrap();
        writer.close().unwrap();
        let mut reader = Reader::open(&path).unwrap();
        for _ in 0..2 {
            let history = reader.load_signal(signal).unwrap();
            let times = reader.block_times(0).unwrap();
            reader.time_table().unwrap();
            reader.value_at(signal, 1).unwrap();
            let transactions = reader.transactions(&TxQuery::default()).unwrap();
            reader.visit_log(&LogQuery::default(), |_| true).unwrap();
            assert!(!reader.cache.lock().unwrap().entries.is_empty());
            assert!(reader.sig_blocks.iter().any(|b| b.times.get().is_some()));
            assert!(reader.tx_blocks.iter().any(|b| b.data.get().is_some()));
            assert!(reader.log_blocks.iter().any(|b| b.data.get().is_some()));
            assert!(reader.global_times.get().is_some());
            reader.clear_cache();
            assert!(reader.cache.lock().unwrap().entries.is_empty());
            assert!(reader.sig_blocks.iter().all(|b| b.times.get().is_none()));
            assert!(reader.tx_blocks.iter().all(|b| b.data.get().is_none()));
            assert!(reader.log_blocks.iter().all(|b| b.data.get().is_none()));
            assert!(reader.global_times.get().is_none());
            assert_eq!(history.value_at(1).as_u64(), Some(42));
            assert!(times.contains(&1));
            assert!(transactions.iter().any(|t| t.id == tx));
            assert!(reader.transaction(tx).unwrap().is_some());
        }
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }
}
