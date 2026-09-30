//! The parallel build: a front end's blocks scanned by worker threads and
//! stitched in time order.

use super::{Block, BlockScan, Budget, BuildOptions, Builder, Identity, Summary};
use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{mpsc, Condvar, Mutex};

/// A trace format's side of a build: its blocks and how to scan one.
///
/// [`build_from`] asks every block's [`cost`](Self::cost) first, then scans
/// blocks on worker threads, each with its own [`Worker`](Self::Worker)
/// created on that thread, and stitches the scans in time order.
pub trait Source: Sync {
    /// Per-thread decoding state.
    type Worker;

    fn identity(&self) -> Identity;

    fn signal_count(&self) -> u32;

    /// The trace's first time step: values recorded at it are not changes.
    fn t_min(&self) -> u64;

    /// The trace's blocks in time order.
    fn blocks(&self) -> &[Block];

    /// Bytes that scanning block `i` holds at most, read from headers.
    fn cost(&self, i: usize, budget: Budget) -> Result<u64>;

    fn worker(&self) -> Self::Worker;

    /// Feeds every change of block `i` to `scan` with
    /// [`BlockScan::push`], one signal after another.
    fn scan(&self, worker: &mut Self::Worker, i: usize, scan: &mut BlockScan) -> Result<()>;
}

/// Builds the activity index of `source` into `out`.
///
/// Worker threads ([`BuildOptions::threads`]) scan blocks ahead of the
/// stitch, at most two blocks per worker and [`BuildOptions::memory`] in
/// flight, and a block whose scan alone would need more is refused with an
/// error naming it. Peak memory is the builder's 24 bytes per signal plus the
/// scans in flight, independent of the length of the run. The output does not
/// depend on the number of threads.
pub fn build_from<S: Source>(source: &S, out: impl Write, opts: &BuildOptions) -> Result<Summary> {
    let blocks = source.blocks();
    let n = blocks.len();
    let (t_min, budget) = (source.t_min(), opts.budget);
    let mut cost = Vec::with_capacity(n);
    for (i, b) in blocks.iter().enumerate() {
        let c = source.cost(i, budget)?;
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
    let mut builder = Builder::new(out, source.identity(), source.signal_count(), t_min, budget)?;
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
            let (tx, gate, moved, cost) = (tx.clone(), &gate, &moved, &cost);
            sc.spawn(move || {
                let mut worker = source.worker();
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
                    let mut scan = BlockScan::new(blocks[i], i.checked_sub(1).map(|p| blocks[p].end), t_min, budget);
                    let scanned = source.scan(&mut worker, i, &mut scan).map(|_| scan);
                    if tx.send((i, scanned)).is_err() {
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
