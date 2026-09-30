//! The VTR front end: blocks scanned in parallel, stitched in time order.

use super::{Block, BlockScan, BuildOptions, Builder, Identity, Summary};
use crate::block::{self, GroupView};
use crate::codec::{self, Decompressor};
use crate::error::{Error, Result};
use crate::hierarchy::SignalKind;
use crate::reader::Reader;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{mpsc, Condvar, Mutex};

/// Builds the activity index of an open VTR file into `out`.
///
/// Worker threads ([`BuildOptions::threads`]) scan blocks ahead of the
/// stitch, at most two blocks per worker and [`BuildOptions::memory`] in
/// flight. A scan decompresses one column run at a time and decodes only
/// the entry headers, never values, so a 2,048-bit bus costs what a single
/// bit does. Peak memory is the builder's 24 bytes per signal plus the scans
/// in flight, independent of the length of the run. A file recovered without
/// its trailer cannot be indexed ([`Identity::of`]).
pub fn build(reader: &Reader, out: impl Write, opts: &BuildOptions) -> Result<Summary> {
    let id = Identity::of(reader)?;
    let n = reader.block_count();
    let blocks: Vec<Block> = (0..n)
        .map(|i| {
            let (start, end) = reader.block_range(i);
            Block { start, end, bytes: reader.signal_block_bytes(i) }
        })
        .collect();
    let t_min = blocks.first().map_or(0, |b| b.start);
    let budget = opts.budget;
    let mut cost = Vec::with_capacity(n);
    for (i, &b) in blocks.iter().enumerate() {
        let c = scan_cost(reader, i, b, opts)?;
        if c > opts.memory {
            return Err(Error::invalid(format!(
                "block {i} ({}..{}) needs about {} MiB to scan, more than the memory limit of {} MiB",
                b.start,
                b.end,
                c >> 20,
                opts.memory >> 20
            )));
        }
        cost.push(c);
    }
    let mut builder = Builder::new(out, id, reader.signal_count(), t_min, budget)?;
    let threads = opts.worker_threads().clamp(1, n.max(1));
    let depth = 2 * threads;

    struct Gate {
        next: usize,
        stitched: usize,
        /// Estimated bytes of the scans claimed and not yet stitched.
        held: u64,
        stop: bool,
    }
    let gate = Mutex::new(Gate { next: 0, stitched: 0, held: 0, stop: false });
    let moved = Condvar::new();
    std::thread::scope(|sc| {
        let (tx, rx) = mpsc::channel::<(usize, Result<BlockScan>)>();
        for _ in 0..threads {
            let (tx, gate, moved, blocks, cost) = (tx.clone(), &gate, &moved, &blocks, &cost);
            sc.spawn(move || {
                let mut d = Decompressor::new();
                loop {
                    let i = {
                        let mut g = gate.lock().unwrap();
                        while !g.stop && g.next < n && (g.next >= g.stitched + depth || (g.held > 0 && g.held + cost[g.next] > opts.memory)) {
                            g = moved.wait(g).unwrap();
                        }
                        if g.stop || g.next >= n {
                            return;
                        }
                        g.held += cost[g.next];
                        g.next += 1;
                        g.next - 1
                    };
                    let scan = BlockScan::new(blocks[i], i.checked_sub(1).map(|p| blocks[p].end), t_min, budget);
                    if tx.send((i, scan_block(reader, i, scan, &mut d))).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let mut pending = BTreeMap::new();
        let mut stitch = || -> Result<()> {
            for (i, scan) in rx.iter() {
                pending.insert(i, scan);
                while let Some(scan) = pending.remove(&builder.blocks_added()) {
                    builder.add(scan?)?;
                    let mut g = gate.lock().unwrap();
                    g.held -= cost[g.stitched];
                    g.stitched += 1;
                    moved.notify_all();
                }
            }
            Ok(())
        };
        let done = stitch();
        if done.is_err() {
            gate.lock().unwrap().stop = true;
            moved.notify_all();
        }
        done
    })?;
    if builder.blocks_added() != n {
        return Err(Error::State("an activity scan worker stopped early"));
    }
    builder.finish()
}

/// What scanning block `i` holds at most: its rows and candidates, its
/// time table, and its largest column run decompressed (64 KiB by default,
/// larger for a single long column), all read from headers.
fn scan_cost(reader: &Reader, i: usize, b: Block, opts: &BuildOptions) -> Result<u64> {
    let (p, h, kinds) = reader.signal_block(i)?;
    let (mut rows, mut run) = (0u64, 0u64);
    for (g, clen, off) in block::dirty_groups(p, &h) {
        let view = GroupView::parse(block::group_container(p, &h, clen, off)?, reader.group_first(g), kinds)?;
        rows += view.n_sigs as u64;
        for r in &view.runs {
            run = run.max(codec::raw_len(r.blob)? as u64 + r.count as u64 * 8);
        }
    }
    let times = h.n_times as u64 * 8 + codec::raw_len(&p[h.tt_off()..h.index_off()])? as u64;
    Ok(BlockScan::estimate(rows, b, opts.budget) + times + run)
}

/// Feeds every change time of block `i` to `scan`, one column at a time.
/// Its buffers are the block's own, freed with it, as [`scan_cost`] counts them.
fn scan_block(reader: &Reader, i: usize, mut scan: BlockScan, d: &mut Decompressor) -> Result<BlockScan> {
    let (p, h, kinds) = reader.signal_block(i)?;
    let times = block::decode_time_table(p, &h, d)?;
    let mut run = Vec::new();
    // Dynamic aliases (target, alias): the alias shares its target's column.
    let mut aliases = Vec::new();
    for (g, clen, off) in block::dirty_groups(p, &h) {
        let view = GroupView::parse(block::group_container(p, &h, clen, off)?, reader.group_first(g), kinds)?;
        aliases.extend(view.aliases.iter().map(|&(alias, target)| (target, alias)));
        for ri in 0..view.runs.len() {
            let cols = view.decode_run(ri, d, &mut run)?;
            let first = view.first_sig + view.runs[ri].first_local;
            for (k, &(a, b)) in cols.iter().enumerate() {
                let sig = first + k as u32;
                feed(&mut scan, sig, &run[a as usize..b as usize], kinds[sig as usize], &times)?;
            }
        }
    }
    aliases.sort_unstable();
    let group_size = reader.meta().group_size.max(1);
    // The (group, run) decoded into `run`, and its column ranges.
    let (mut decoded, mut cols) = (None, Vec::new());
    for (target, alias) in aliases {
        let g = target / group_size;
        let (clen, off) = block::find_group(p, &h, g).ok_or(Error::Corrupt("alias target group missing"))?;
        let view = GroupView::parse(block::group_container(p, &h, clen, off)?, reader.group_first(g), kinds)?;
        if !view.holds(target) {
            return Err(Error::Corrupt("alias target not in block"));
        }
        let ri = view.run_of(target);
        if decoded != Some((g, ri)) {
            cols = view.decode_run(ri, d, &mut run)?;
            decoded = Some((g, ri));
        }
        let (a, b) = cols[(target - view.first_sig - view.runs[ri].first_local) as usize];
        feed(&mut scan, alias, &run[a as usize..b as usize], kinds[target as usize], &times)?;
    }
    Ok(scan)
}

fn feed(scan: &mut BlockScan, sig: u32, col: &[u8], kind: SignalKind, times: &[u64]) -> Result<()> {
    let mut outside = false;
    block::for_each_tidx(col, kind, |ti| match times.get(ti as usize) {
        Some(&t) => scan.push(sig, t),
        None => outside = true,
    })?;
    if outside {
        return Err(Error::Corrupt("column entry outside its block's time table"));
    }
    Ok(())
}
