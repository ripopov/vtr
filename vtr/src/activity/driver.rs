//! The parallel build: a front end's blocks scanned by worker threads and stitched in time
//! order.

use super::{Block, BlockScan, Budget, BuildOptions, Builder, Identity, Summary};
use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Condvar, Mutex, mpsc};

pub trait Source: Sync {
    type Worker;

    fn identity(&self) -> Identity;

    fn signal_count(&self) -> u32;

    fn t_min(&self) -> u64;

    fn blocks(&self) -> &[Block];

    fn cost(&self, i: usize, budget: Budget) -> Result<u64>;

    fn worker(&self) -> Self::Worker;

    fn scan(&self, worker: &mut Self::Worker, i: usize, scan: &mut BlockScan) -> Result<()>;
}

pub fn build_from<S: Source>(source: &S, out: impl Write, opts: &BuildOptions) -> Result<Summary> {
    let blocks = source.blocks();
    let n = blocks.len();
    if let Some(control) = &opts.control {
        control.begin(n)?;
    }
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
        held: u64,
        stop: bool,
    }
    let gate = Mutex::new(Gate {
        next: 0,
        stitched: 0,
        held: 0,
        stop: false,
    });
    let moved = Condvar::new();
    std::thread::scope(|sc| {
        let (tx, rx) = mpsc::channel::<(usize, Result<BlockScan>)>();
        for _ in 0..threads {
            let (tx, gate, moved, cost) = (tx.clone(), &gate, &moved, &cost);
            sc.spawn(move || {
                let mut worker = source.worker();
                loop {
                    if opts.control.as_ref().is_some_and(|c| c.is_cancelled()) {
                        return;
                    }
                    let i = {
                        let mut g = gate.lock().unwrap();
                        while !g.stop
                            && g.next < n
                            && (g.next >= g.stitched + depth
                                || (g.held > 0 && g.held + cost[g.next] > opts.memory))
                        {
                            g = moved.wait(g).unwrap();
                        }
                        if g.stop
                            || g.next >= n
                            || opts.control.as_ref().is_some_and(|c| c.is_cancelled())
                        {
                            return;
                        }
                        g.held += cost[g.next];
                        g.next += 1;
                        g.next - 1
                    };
                    let mut scan = BlockScan::new(
                        blocks[i],
                        i.checked_sub(1).map(|p| blocks[p].end),
                        t_min,
                        budget,
                    );
                    let scanned = source.scan(&mut worker, i, &mut scan).map(|()| scan);
                    if tx.send((i, scanned)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let mut pending = BTreeMap::new();
        let mut stitch = || -> Result<()> {
            for (i, scan) in &rx {
                if let Some(control) = &opts.control {
                    control.check()?;
                }
                pending.insert(i, scan);
                while let Some(scan) = pending.remove(&builder.blocks_added()) {
                    builder.add(scan?)?;
                    let mut g = gate.lock().unwrap();
                    g.held -= cost[g.stitched];
                    g.stitched += 1;
                    if let Some(control) = &opts.control {
                        control.advance(g.stitched);
                    }
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
    if let Some(control) = &opts.control {
        control.check()?;
    }
    if builder.blocks_added() != n {
        return Err(Error::State("an activity scan worker stopped early"));
    }
    builder.finish()
}
