//! Source waveform block scans for exact activity sidecars.

use super::{Block, BlockScan, BuildOptions, Identity, Source, Summary};
use crate::block::{self, GroupView};
use crate::codec::{self, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::SignalKind;
use crate::trace::Reader;
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
                    bytes: reader.signal_block_bytes(i) + 24,
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

    fn cost(&self, i: usize) -> Result<u64> {
        scan_cost(self.reader, i)
    }

    fn worker(&self) -> Decompressor {
        Decompressor::new()
    }

    fn scan(&self, d: &mut Decompressor, i: usize, scan: &mut BlockScan) -> Result<()> {
        scan_block(self.reader, i, scan, d)
    }
}

fn scan_cost(reader: &Reader, i: usize) -> Result<u64> {
    let (p, h, kinds) = reader.signal_block(i)?;
    let (mut rows, mut run, mut points) = (0u64, 0u64, 0u64);
    for (g, clen, off) in block::dirty_groups(p, &h) {
        let view = GroupView::parse(
            block::group_container(p, &h, clen, off)?,
            reader.group_first(g),
            kinds,
        )?;
        rows += view.n_sigs as u64;
        points =
            points.saturating_add((view.aliases.len() as u64).saturating_mul(u64::from(h.n_times)));
        for r in &view.runs {
            let length = codec::raw_len(r.blob)? as u64;
            points = points.saturating_add(length);
            run = run.max(length.saturating_mul(2) + u64::from(r.count) * 16);
        }
    }
    let times =
        u64::from(h.n_times) * 8 + codec::raw_len(&p[block::HEADER_LEN..h.index_off()])? as u64;
    Ok(BlockScan::estimate(rows, points) + times + run)
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
    let group_size = reader.metadata().group_size.max(1);
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
