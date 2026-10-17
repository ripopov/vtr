//! The VTR front end: every block's change times from its entry headers, and the undecided
//! signals of a window from the reader.

use super::{Block, BlockScan, Budget, BuildOptions, Identity, Source, Summary};
use crate::block::{self, GroupView};
use crate::codec::{self, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::{SignalId, SignalKind};
use crate::reader::Reader;
use std::io::Write;

pub fn build(reader: &Reader, out: impl Write, opts: &BuildOptions) -> Result<Summary> {
    super::build_from(&VtrSource::new(reader)?, out, opts)
}

struct VtrSource<'a> {
    reader: &'a Reader,
    identity: Identity,
    blocks: Vec<Block>,
}

impl<'a> VtrSource<'a> {
    fn new(reader: &'a Reader) -> Result<Self> {
        let identity = Identity::of(reader)?;
        let blocks = (0..reader.block_count())
            .map(|i| {
                let (start, end) = reader.block_range(i);
                Block {
                    start,
                    end,
                    bytes: reader.signal_block_bytes(i),
                }
            })
            .collect();
        Ok(VtrSource {
            reader,
            identity,
            blocks,
        })
    }
}

impl Source for VtrSource<'_> {
    type Worker = Decompressor;

    fn identity(&self) -> Identity {
        self.identity
    }

    fn signal_count(&self) -> u32 {
        self.reader.signal_count()
    }

    fn t_min(&self) -> u64 {
        self.blocks.first().map_or(0, |b| b.start)
    }

    fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    fn cost(&self, i: usize, budget: Budget) -> Result<u64> {
        scan_cost(self.reader, i, self.blocks[i], budget)
    }

    fn worker(&self) -> Decompressor {
        Decompressor::new()
    }

    fn scan(&self, d: &mut Decompressor, i: usize, scan: &mut BlockScan) -> Result<()> {
        scan_block(self.reader, i, scan, d)
    }
}

pub fn resolve(reader: &Reader, signals: &[SignalId], t0: u64, t1: u64) -> Result<Vec<SignalId>> {
    let t_min = if reader.block_count() > 0 {
        reader.block_range(0).0
    } else {
        0
    };
    let t0 = t0.max(t_min.saturating_add(1));
    if t0 > t1 || signals.is_empty() {
        return Ok(Vec::new());
    }
    let mut sigs = signals.to_vec();
    sigs.sort_unstable();
    sigs.dedup();
    if let Some(s) = sigs.last().filter(|s| s.0 >= reader.signal_count()) {
        return Err(Error::invalid(format!("unknown signal {}", s.0)));
    }
    let hit = reader.changing(&sigs, t0, t1)?;
    Ok(sigs
        .into_iter()
        .zip(hit)
        .filter_map(|(s, h)| h.then_some(s))
        .collect())
}

fn scan_cost(reader: &Reader, i: usize, b: Block, budget: Budget) -> Result<u64> {
    let (p, h, kinds) = reader.signal_block(i)?;
    let (mut rows, mut run) = (0u64, 0u64);
    for (g, clen, off) in block::dirty_groups(p, &h) {
        let view = GroupView::parse(
            block::group_container(p, &h, clen, off)?,
            reader.group_first(g),
            kinds,
        )?;
        rows += view.n_sigs as u64;
        for r in &view.runs {
            run = run.max(codec::raw_len(r.blob)? as u64 + u64::from(r.count) * 8);
        }
    }
    let times = u64::from(h.n_times) * 8 + codec::raw_len(&p[h.tt_off()..h.index_off()])? as u64;
    Ok(BlockScan::estimate(rows, b, budget) + times + run)
}

fn scan_block(reader: &Reader, i: usize, scan: &mut BlockScan, d: &mut Decompressor) -> Result<()> {
    let (p, h, kinds) = reader.signal_block(i)?;
    let times = block::decode_time_table(p, &h, d)?;
    let mut run = Vec::new();
    let mut aliases = Vec::new();
    for (g, clen, off) in block::dirty_groups(p, &h) {
        let view = GroupView::parse(
            block::group_container(p, &h, clen, off)?,
            reader.group_first(g),
            kinds,
        )?;
        aliases.extend(view.aliases.iter().map(|&(alias, target)| (target, alias)));
        for ri in 0..view.runs.len() {
            let cols = view.decode_run(ri, d, &mut run)?;
            let first = view.first_sig + view.runs[ri].first_local;
            for (k, &(a, b)) in cols.iter().enumerate() {
                let sig = first + k as u32;
                feed(
                    scan,
                    sig,
                    &run[a as usize..b as usize],
                    kinds[sig as usize],
                    &times,
                )?;
            }
        }
    }
    aliases.sort_unstable();
    let group_size = reader.meta().group_size.max(1);
    let (mut decoded, mut cols) = (None, Vec::new());
    for (target, alias) in aliases {
        let g = target / group_size;
        let (clen, off) =
            block::find_group(p, &h, g).ok_or(Error::Corrupt("alias target group missing"))?;
        let view = GroupView::parse(
            block::group_container(p, &h, clen, off)?,
            reader.group_first(g),
            kinds,
        )?;
        if !view.holds(target) {
            return Err(Error::Corrupt("alias target not in block"));
        }
        let ri = view.run_of(target);
        if decoded != Some((g, ri)) {
            cols = view.decode_run(ri, d, &mut run)?;
            decoded = Some((g, ri));
        }
        let (a, b) = cols[(target - view.first_sig - view.runs[ri].first_local) as usize];
        feed(
            scan,
            alias,
            &run[a as usize..b as usize],
            kinds[target as usize],
            &times,
        )?;
    }
    Ok(())
}

fn feed(scan: &mut BlockScan, sig: u32, col: &[u8], kind: SignalKind, times: &[u64]) -> Result<()> {
    let mut outside = false;
    block::for_each_tidx(col, kind, |ti| match times.get(ti as usize) {
        Some(&t) => scan.push(sig, t),
        None => outside = true,
    })?;
    if outside {
        return Err(Error::Corrupt(
            "column entry outside its block's time table",
        ));
    }
    Ok(())
}
