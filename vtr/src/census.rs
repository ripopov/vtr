//! Scope sizes: how many distinct signals, variables and scopes each scope holds, subscopes
//! included.

const NONE: u32 = u32::MAX;

#[derive(Debug, Default)]
pub struct Census {
    parent: Vec<u32>,
    uf: Vec<u32>,
    anc: Vec<u32>,
    signals: Vec<u32>,
    variables: Vec<u32>,
    last: Vec<u32>,
    open: Vec<u32>,
    record: Option<Vec<(u32, u32)>>,
}

const NEGATIVE: u32 = 1 << 31;

impl Census {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn recording() -> Self {
        Census {
            record: Some(Vec::new()),
            ..Self::default()
        }
    }

    pub fn enter(&mut self) -> u32 {
        let i = self.parent.len() as u32;
        self.parent.push(self.open.last().copied().unwrap_or(NONE));
        self.uf.push(i);
        self.anc.push(i);
        self.signals.push(0);
        self.variables.push(0);
        self.open.push(i);
        i
    }

    pub fn var(&mut self, signal: u32) {
        let i = *self.open.last().expect("Census::var outside a scope");
        self.variables[i as usize] += 1;
        let g = signal as usize;
        if g >= self.last.len() {
            self.last.resize(g + 1, NONE);
        }
        let prev = self.last[g];
        if prev == i {
            return;
        }
        self.signals[i as usize] = self.signals[i as usize].wrapping_add(1);
        if let Some(r) = &mut self.record {
            r.push((signal, i));
        }
        if prev != NONE {
            let root = self.find(prev);
            let l = self.anc[root as usize];
            if l != NONE {
                self.signals[l as usize] = self.signals[l as usize].wrapping_sub(1);
                if let Some(r) = &mut self.record {
                    r.push((signal, l | NEGATIVE));
                }
            }
        }
        self.last[g] = i;
    }

    pub fn leave(&mut self) {
        let i = self.open.pop().expect("Census::leave without a scope");
        let p = self.parent[i as usize];
        let b = self.find(i);
        if p == NONE {
            self.anc[b as usize] = NONE;
        } else {
            let a = self.find(p);
            self.uf[b as usize] = a;
            self.anc[a as usize] = p;
        }
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.open.len()
    }

    #[must_use]
    pub fn finish_recorded(mut self) -> (ScopeSizes, Contributions) {
        let record = self.record.take().unwrap_or_default();
        let signals = record
            .iter()
            .map(|&(s, _)| s as usize + 1)
            .max()
            .unwrap_or(0);
        let mut off = vec![0u32; signals + 1];
        for &(s, _) in &record {
            off[s as usize + 1] += 1;
        }
        for i in 0..signals {
            off[i + 1] += off[i];
        }
        let mut next = off.clone();
        let mut weights = vec![0u32; record.len()];
        for (s, w) in record {
            weights[next[s as usize] as usize] = w;
            next[s as usize] += 1;
        }
        let sizes = self.finish();
        let contributions = Contributions {
            parent: sizes.parent.clone(),
            off,
            weights,
        };
        (sizes, contributions)
    }

    #[must_use]
    pub fn finish(mut self) -> ScopeSizes {
        while !self.open.is_empty() {
            self.leave();
        }
        let n = self.parent.len();
        let mut scopes = vec![1u32; n];
        for i in (0..n).rev() {
            let p = self.parent[i];
            if p != NONE {
                let p = p as usize;
                self.signals[p] = self.signals[p].wrapping_add(self.signals[i]);
                self.variables[p] += self.variables[i];
                scopes[p] += scopes[i];
            }
        }
        ScopeSizes {
            parent: self.parent,
            signals: self.signals,
            variables: self.variables,
            scopes,
        }
    }

    fn find(&mut self, mut x: u32) -> u32 {
        let mut r = x;
        while self.uf[r as usize] != r {
            r = self.uf[r as usize];
        }
        while self.uf[x as usize] != r {
            let n = self.uf[x as usize];
            self.uf[x as usize] = r;
            x = n;
        }
        r
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Contributions {
    parent: Vec<u32>,
    off: Vec<u32>,
    weights: Vec<u32>,
}

impl Contributions {
    #[must_use]
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    #[must_use]
    pub fn memory_bytes(&self) -> u64 {
        ((self.parent.len() + self.off.len() + self.weights.len()) * 4) as u64
    }

    pub fn count(&self, signals: impl IntoIterator<Item = u32>, out: &mut Vec<u32>) {
        out.clear();
        out.resize(self.parent.len(), 0);
        let known = self.off.len().saturating_sub(1);
        for s in signals {
            let s = s as usize;
            if s >= known {
                continue;
            }
            for &w in &self.weights[self.off[s] as usize..self.off[s + 1] as usize] {
                let c = &mut out[(w & !NEGATIVE) as usize];
                *c = if w & NEGATIVE == 0 {
                    c.wrapping_add(1)
                } else {
                    c.wrapping_sub(1)
                };
            }
        }
        for i in (0..self.parent.len()).rev() {
            let p = self.parent[i];
            if p != NONE {
                out[p as usize] = out[p as usize].wrapping_add(out[i]);
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeSizes {
    parent: Vec<u32>,
    signals: Vec<u32>,
    variables: Vec<u32>,
    scopes: Vec<u32>,
}

impl ScopeSizes {
    #[must_use]
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    #[must_use]
    pub fn parent(&self, scope: u32) -> Option<u32> {
        Some(self.parent[scope as usize]).filter(|&p| p != NONE)
    }

    #[must_use]
    pub fn signals(&self, scope: u32) -> u32 {
        self.signals[scope as usize]
    }

    #[must_use]
    pub fn variables(&self, scope: u32) -> u32 {
        self.variables[scope as usize]
    }

    #[must_use]
    pub fn scopes(&self, scope: u32) -> u32 {
        self.scopes[scope as usize]
    }
}
