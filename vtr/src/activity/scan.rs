//! One source block reduced to what the index needs.

use super::{BUCKETS, Block, Budget, STRETCH_BYTES, bucket};

#[derive(Clone, Copy, Debug)]
pub(super) struct Cand {
    pub a: u64,
    pub b: u64,
    pub pre: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Row {
    pub signal: u32,
    pub cand0: u32,
    pub ncand: u32,
    pub first: u64,
    pub last: u64,
    pub tail: u64,
}

#[derive(Clone, Debug)]
pub struct BlockScan {
    pub(super) block: Block,
    pub(super) prev_end: Option<u64>,
    pub(super) t_min: u64,
    pub(super) j_max: u32,
    pub(super) cap: u64,
    pub(super) hist: [u64; BUCKETS],
    lb: u32,
    above: u64,
    pub(super) rows: Vec<Row>,
    pub(super) cands: Vec<Cand>,
    pub(super) changes: u64,
    open: bool,
    sig: u32,
    first: u64,
    prev: u64,
    run: u64,
    cand0: u32,
    compact_at: usize,
}

const COMPACT_MIN: usize = 4096;

impl BlockScan {
    #[must_use]
    pub fn new(block: Block, prev_end: Option<u64>, t_min: u64, budget: Budget) -> BlockScan {
        let span = match prev_end {
            Some(p) => block.end.saturating_sub(p),
            None => block.end.saturating_sub(block.start).saturating_add(1),
        };
        let j_max = (span / 4).max(1).ilog2();
        let cap = (budget.memory.max(0.0) * block.bytes as f64 / STRETCH_BYTES as f64) as u64;
        BlockScan {
            block,
            prev_end,
            t_min,
            j_max,
            cap,
            hist: [0; BUCKETS],
            lb: 0,
            above: 0,
            rows: Vec::new(),
            cands: Vec::new(),
            changes: 0,
            open: false,
            sig: 0,
            first: 0,
            prev: 0,
            run: 0,
            cand0: 0,
            compact_at: (2 * cap as usize).max(COMPACT_MIN),
        }
    }

    #[must_use]
    pub fn estimate(rows: u64, block: Block, budget: Budget) -> u64 {
        let cap = (budget.memory.max(0.0) * block.bytes as f64 / STRETCH_BYTES as f64) as u64;
        rows * std::mem::size_of::<Row>() as u64
            + 2 * cap.max(COMPACT_MIN as u64) * std::mem::size_of::<Cand>() as u64
    }

    #[inline]
    pub fn push(&mut self, signal: u32, t: u64) {
        if t <= self.t_min {
            return;
        }
        if !self.open || signal != self.sig {
            self.close();
            self.open = true;
            self.sig = signal;
            self.first = t;
            self.prev = t;
            self.run = 0;
            self.cand0 = self.cands.len() as u32;
            self.changes += 1;
            return;
        }
        if t <= self.prev {
            return;
        }
        self.changes += 1;
        let len = t - self.prev;
        let k = bucket(len) as u32;
        self.hist[k as usize] += 1;
        if k > self.lb {
            self.above += 1;
            while self.above > self.cap && self.lb < self.j_max {
                self.lb += 1;
                self.above -= self.hist[self.lb as usize];
            }
        }
        if k > self.lb {
            self.cands.push(Cand {
                a: self.prev,
                b: t,
                pre: self.run,
            });
            self.run = 0;
            if self.cands.len() >= self.compact_at {
                self.compact();
            }
        } else {
            self.run = self.run.max(len);
        }
        self.prev = t;
    }

    fn compact(&mut self) {
        let delta = 1u64 << self.lb;
        let mut w = 0usize;
        for row in &mut self.rows {
            let r0 = row.cand0 as usize;
            row.cand0 = w as u32;
            let carry = keep_longer(&mut self.cands, r0..r0 + row.ncand as usize, delta, &mut w);
            row.ncand = w as u32 - row.cand0;
            row.tail = row.tail.max(carry);
        }
        if self.open {
            let r0 = self.cand0 as usize;
            self.cand0 = w as u32;
            let n = self.cands.len();
            let carry = keep_longer(&mut self.cands, r0..n, delta, &mut w);
            self.run = self.run.max(carry);
        }
        self.cands.truncate(w);
        self.compact_at = (2 * self.cands.len())
            .max(2 * self.cap as usize)
            .max(COMPACT_MIN);
    }

    pub(super) fn close(&mut self) {
        if self.open {
            self.open = false;
            let ncand = self.cands.len() as u32 - self.cand0;
            self.rows.push(Row {
                signal: self.sig,
                cand0: self.cand0,
                ncand,
                first: self.first,
                last: self.prev,
                tail: self.run,
            });
        }
    }
}

fn keep_longer(
    cands: &mut [Cand],
    range: std::ops::Range<usize>,
    delta: u64,
    w: &mut usize,
) -> u64 {
    let mut carry = 0u64;
    for i in range {
        let c = cands[i];
        if c.b - c.a > delta {
            cands[*w] = Cand {
                pre: c.pre.max(carry),
                ..c
            };
            *w += 1;
            carry = 0;
        } else {
            carry = carry.max(c.pre).max(c.b - c.a);
        }
    }
    carry
}

#[cfg(test)]
mod tests {
    use super::super::{Builder, Identity, SourceFormat};
    use super::*;

    #[test]
    fn compaction_does_not_change_the_index() {
        let id = Identity {
            format: SourceFormat::Vtr,
            length: 1,
            toc_crc: 0,
        };
        let budget = Budget {
            disk: 1.0,
            memory: 0.04,
        };
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut rand = move |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let signals: Vec<Vec<u64>> = (0..40)
            .map(|_| {
                let mut t = 1 + rand(50);
                let mut out = Vec::new();
                for _ in 0..20 + rand(40) {
                    for _ in 0..20 + rand(200) {
                        t += 1 + rand(8);
                        out.push(t);
                    }
                    t += if rand(10) == 0 {
                        1_000_000 + rand(9_000_000)
                    } else {
                        500 + rand(4_500)
                    };
                }
                out
            })
            .collect();
        let end = signals
            .iter()
            .filter_map(|s| s.last())
            .max()
            .copied()
            .unwrap()
            + 1;
        let block = Block {
            start: 1,
            end,
            bytes: 40_000,
        };
        let kept = std::cell::Cell::new(0);
        let build = |every: bool| {
            let mut scan = BlockScan::new(block, None, 0, budget);
            scan.compact_at = usize::MAX;
            for (s, times) in signals.iter().enumerate() {
                for &t in times {
                    scan.push(s as u32, t);
                    if every {
                        scan.compact();
                        scan.compact_at = usize::MAX;
                    }
                }
            }
            let mut image = Vec::new();
            let mut b = Builder::new(&mut image, id, signals.len() as u32, 0, budget).unwrap();
            b.add(scan).unwrap();
            kept.set(b.finish().unwrap().stretches);
            image
        };
        assert_eq!(build(true), build(false));
        assert!(
            kept.get() > 100,
            "the test keeps silences ({} stretches)",
            kept.get()
        );
    }
}
