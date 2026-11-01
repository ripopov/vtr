//! The parallel build: a front end's blocks scanned by worker threads and stitched in time
//! order.

use super::{Block, BlockScan, BuildControl, BuildOptions, Builder, Identity, Summary, memory};
use crate::error::{Error, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::{Arc, Condvar, Mutex, mpsc};

pub trait Source: Sync {
    type Worker;

    fn identity(&self) -> Identity;

    fn signal_count(&self) -> u32;

    fn t_min(&self) -> u64;

    fn blocks(&self) -> &[Block];

    /// Scheduling reservation; an oversized estimate runs alone within the build budget.
    fn cost(&self, i: usize) -> Result<u64>;

    /// Visits exact closed activity intervals without retaining record payloads.
    /// Calls `admit` before allocating page workspace; `visit` may cancel the scan.
    fn generator_activity(
        &self,
        _admit: &mut dyn FnMut(usize) -> Result<()>,
        _visit: &mut dyn FnMut(u32, u64, u64) -> Result<()>,
    ) -> Result<()> {
        Ok(())
    }

    fn worker(&self) -> Self::Worker;

    /// Admits decoding buffers with `scan.workspace` before allocating them,
    /// and releases those temporary buffers before returning.
    fn scan(&self, worker: &mut Self::Worker, i: usize, scan: &mut BlockScan) -> Result<()>;
}

struct State {
    next: usize,
    stitched: usize,
    running: usize,
    scans: u64,
    encoding: u64,
    encoding_waiting: bool,
    stopped: bool,
}

struct Workspace {
    state: Mutex<State>,
    changed: Condvar,
    limit: u64,
    depth: usize,
    control: Option<Arc<BuildControl>>,
}

impl Workspace {
    fn available(&self, state: &State) -> u64 {
        self.limit
            .saturating_sub(state.scans)
            .saturating_sub(state.encoding)
    }

    fn admit(self: &Arc<Self>, costs: &[u64]) -> Option<(usize, ScanPermit)> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.stopped
                || state.next == costs.len()
                || self
                    .control
                    .as_ref()
                    .is_some_and(|control| control.is_cancelled())
            {
                return None;
            }
            if !state.encoding_waiting
                && state.next < state.stitched + self.depth
                && costs[state.next] <= self.available(&state)
            {
                let index = state.next;
                state.next += 1;
                state.running += 1;
                state.scans += costs[index];
                return Some((
                    index,
                    ScanPermit {
                        workspace: self.clone(),
                        bytes: costs[index],
                        running: true,
                    },
                ));
            }
            state = self.changed.wait(state).unwrap();
        }
    }

    fn stitched(&self, costs: &[u64]) -> bool {
        let mut state = self.state.lock().unwrap();
        state.stitched += 1;
        let trim = state.scans == 0
            && state.next < costs.len()
            && costs[state.next] > self.available(&state);
        self.changed.notify_all();
        trim
    }

    fn stop(&self) {
        self.state.lock().unwrap().stopped = true;
        self.changed.notify_all();
    }
}

impl memory::Budget for Workspace {
    fn acquire(&self, minimum: u64, preferred: u64) -> Result<Option<u64>> {
        let mut state = self.state.lock().unwrap();
        while minimum > self.available(&state)
            && state.running != 0
            && !state.stopped
            && !self
                .control
                .as_ref()
                .is_some_and(|control| control.is_cancelled())
        {
            // Running scans already own their decoding space and can finish without admission.
            state.encoding_waiting = true;
            state = self.changed.wait(state).unwrap();
        }
        state.encoding_waiting = false;
        self.changed.notify_all();
        if let Some(control) = &self.control {
            control.check()?;
        }
        if state.stopped {
            return Err(Error::State("activity scan worker stopped early"));
        }
        if minimum > self.available(&state) {
            return Ok(None);
        }
        let granted = preferred.min(self.available(&state));
        state.encoding += granted;
        Ok(Some(granted))
    }

    fn release(&self, bytes: u64) {
        if bytes != 0 {
            self.state.lock().unwrap().encoding -= bytes;
            self.changed.notify_all();
        }
    }
}

struct ScanPermit {
    workspace: Arc<Workspace>,
    bytes: u64,
    running: bool,
}

impl ScanPermit {
    fn complete(&mut self, retained: u64) {
        debug_assert!(retained <= self.bytes);
        let mut state = self.workspace.state.lock().unwrap();
        state.scans -= self.bytes;
        state.scans += retained;
        state.running -= 1;
        self.bytes = retained;
        self.running = false;
        self.workspace.changed.notify_all();
    }
}

impl Drop for ScanPermit {
    fn drop(&mut self) {
        let mut state = self.workspace.state.lock().unwrap();
        state.scans -= self.bytes;
        if self.running {
            state.running -= 1;
            state.stopped = true;
        }
        self.workspace.changed.notify_all();
    }
}

struct Scanned {
    // Queued buffers must drop before their permit makes the memory available again.
    scan: Result<BlockScan>,
    permit: ScanPermit,
}

pub fn build_from<S: Source>(source: &S, out: impl Write, opts: &BuildOptions) -> Result<Summary> {
    let blocks = source.blocks();
    let n = blocks.len();
    if let Some(control) = &opts.control {
        control.begin(n)?;
    }
    let t_min = source.t_min();
    // Includes directory capacity growth and serialization alongside queued block results.
    let per_block = (std::mem::size_of::<Block>()
        + std::mem::size_of::<u64>()
        + 4 * std::mem::size_of::<super::container::Entry>()) as u64;
    let base = Builder::<std::io::Sink>::base_bytes(source.signal_count())
        .checked_add((n as u64).saturating_add(2).saturating_mul(per_block))
        .ok_or(Error::invalid("activity base memory overflow"))?;
    let memory = opts.memory.checked_sub(base).ok_or(Error::invalid(
        "activity signal tables exceed the memory limit",
    ))?;
    let cost = (0..n)
        .map(|i| source.cost(i).map(|cost| cost.min(memory)))
        .collect::<Result<Vec<_>>>()?;
    let mut builder = Builder::new(out, source.identity(), source.signal_count(), t_min)?;
    let threads = opts.worker_threads().clamp(1, n.max(1));
    let workspace = Arc::new(Workspace {
        state: Mutex::new(State {
            next: 0,
            stitched: 0,
            running: 0,
            scans: 0,
            encoding: 0,
            encoding_waiting: false,
            stopped: false,
        }),
        changed: Condvar::new(),
        limit: memory,
        depth: 2 * threads,
        control: opts.control.clone(),
    });
    builder.set_memory(Some(memory::Reservation::new(workspace.clone())));
    std::thread::scope(|sc| {
        let (tx, rx) = mpsc::channel::<(usize, Scanned)>();
        for _ in 0..threads {
            let (tx, workspace, cost) = (tx.clone(), workspace.clone(), &cost);
            sc.spawn(move || {
                let mut worker = source.worker();
                while let Some((i, mut permit)) = workspace.admit(cost) {
                    let mut scan =
                        BlockScan::new(blocks[i], i.checked_sub(1).map(|p| blocks[p].end), t_min);
                    scan.set_limit(cost[i]);
                    let scan = source
                        .scan(&mut worker, i, &mut scan)
                        .map(|()| scan)
                        .map_err(|error| match error {
                            Error::Invalid(message) => Error::invalid(format!(
                                "activity block {i} ({}..{}): {message}",
                                blocks[i].start, blocks[i].end
                            )),
                            error => error,
                        });
                    permit.complete(scan.as_ref().map_or(0, BlockScan::heap_bytes));
                    if tx.send((i, Scanned { scan, permit })).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let mut pending = BTreeMap::new();
        let mut stitch = || -> Result<()> {
            for (i, scanned) in &rx {
                if let Some(control) = &opts.control {
                    control.check()?;
                }
                pending.insert(i, scanned);
                while let Some(Scanned { scan, permit }) = pending.remove(&builder.blocks_added()) {
                    builder.add(scan?)?;
                    drop(permit);
                    if workspace.stitched(&cost) {
                        builder.release_workspace();
                    }
                    if let Some(control) = &opts.control {
                        control.advance(builder.blocks_added());
                    }
                }
            }
            Ok(())
        };
        let done = stitch();
        if done.is_err() {
            workspace.stop();
        }
        done
    })?;
    if let Some(control) = &opts.control {
        control.check()?;
    }
    if builder.blocks_added() != n {
        return Err(Error::State("an activity scan worker stopped early"));
    }
    builder.set_memory(None);
    let generators = super::generators::collect(source, opts, base)?;
    builder.finish_with_generators(generators, opts.control.as_deref())
}
