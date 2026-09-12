//! The change history of one signal.
//!
//! The renderer never iterates a history; it asks for the index of the change
//! at or before a time (a binary or exponential search) and reads single
//! changes by index. This is what keeps frame cost proportional to the number
//! of visible pixel columns rather than the number of transitions.

use super::value::{Bit, SignalShape, WaveValue};

pub trait SignalHistory: Send + Sync {
    fn shape(&self) -> SignalShape;

    /// Number of value changes.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Time of change `i` (`i < len()`). Non-decreasing in `i`.
    fn time(&self, i: usize) -> u64;

    /// Value after change `i`; `None` is the value before the first change.
    fn value(&self, i: Option<usize>) -> WaveValue;

    /// Fast path for 1-bit signals: the bit after change `i`.
    /// Vectors and reals return [`Bit::Other`].
    fn bit(&self, i: Option<usize>) -> Bit;

    /// Index of the last change with `time <= t`, `None` when `t` precedes the
    /// first change. O(log n).
    fn index_at(&self, t: u64) -> Option<usize> {
        let n = self.len();
        if n == 0 || self.time(0) > t {
            return None;
        }
        // Invariant: time(lo) <= t, and either hi == n or time(hi) > t.
        let (mut lo, mut hi) = (0usize, n);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.time(mid) <= t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(lo)
    }

    /// Like [`index_at`](Self::index_at) but starts an exponential search from
    /// `hint`, so a sweep across pixel columns costs O(log gap) per column.
    fn index_at_hint(&self, t: u64, hint: usize) -> Option<usize> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        let hint = hint.min(n - 1);
        if self.time(hint) <= t {
            // Gallop forward: find hi with time(hi) > t or hi == n.
            let mut lo = hint;
            let mut step = 1usize;
            let mut hi = hint + 1;
            while hi < n && self.time(hi) <= t {
                lo = hi;
                step <<= 1;
                hi = hint.saturating_add(step).min(n);
                if hi == n {
                    break;
                }
            }
            let mut hi = hi.min(n);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if self.time(mid) <= t {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            Some(lo)
        } else {
            // Gallop backward: find lo with time(lo) <= t, or conclude None.
            if self.time(0) > t {
                return None;
            }
            let mut hi = hint; // time(hi) > t
            let mut step = 1usize;
            let mut lo = hint.saturating_sub(step);
            while lo > 0 && self.time(lo) > t {
                hi = lo;
                step <<= 1;
                lo = hint.saturating_sub(step);
            }
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if self.time(mid) <= t {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            Some(lo)
        }
    }

    /// Time of the first change strictly after `t`, if any.
    fn next_change_after(&self, t: u64) -> Option<u64> {
        let n = self.len();
        let start = match self.index_at(t) {
            None => 0,
            Some(i) => i + 1,
        };
        (start..n).map(|i| self.time(i)).find(|&ti| ti > t)
    }

    /// Time of the last change strictly before `t`, if any.
    fn prev_change_before(&self, t: u64) -> Option<u64> {
        let mut i = self.index_at(t)?;
        loop {
            let ti = self.time(i);
            if ti < t {
                return Some(ti);
            }
            if i == 0 {
                return None;
            }
            i -= 1;
        }
    }
}

/// A history held entirely in memory as parallel `times`/`values` arrays.
/// Used by tests and by the synthetic source's in-memory mode.
pub struct VecHistory {
    pub shape: SignalShape,
    pub times: Vec<u64>,
    pub values: Vec<WaveValue>,
    pub initial: WaveValue,
}

impl SignalHistory for VecHistory {
    fn shape(&self) -> SignalShape {
        self.shape
    }
    fn len(&self) -> usize {
        self.times.len()
    }
    fn time(&self, i: usize) -> u64 {
        self.times[i]
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        match i {
            None => self.initial.clone(),
            Some(i) => self.values[i].clone(),
        }
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        match self.value(i) {
            WaveValue::Bits(s) => s.bytes().next().map(Bit::from_ascii).unwrap_or(Bit::Other),
            _ => Bit::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist(times: &[u64]) -> VecHistory {
        VecHistory {
            shape: SignalShape::Bit,
            times: times.to_vec(),
            values: times.iter().map(|_| WaveValue::Bits("1".into())).collect(),
            initial: WaveValue::Bits("0".into()),
        }
    }

    #[test]
    fn index_at_matches_linear_scan() {
        let h = hist(&[5, 5, 10, 20, 20, 20, 31]);
        for t in 0..40 {
            let expect = (0..h.len()).rev().find(|&i| h.time(i) <= t);
            assert_eq!(h.index_at(t), expect, "t={t}");
            for hint in 0..h.len() {
                assert_eq!(h.index_at_hint(t, hint), expect, "t={t} hint={hint}");
            }
        }
        assert_eq!(hist(&[]).index_at(3), None);
        assert_eq!(hist(&[]).index_at_hint(3, 0), None);
    }

    #[test]
    fn neighbours() {
        let h = hist(&[5, 10, 20]);
        assert_eq!(h.next_change_after(5), Some(10));
        assert_eq!(h.next_change_after(20), None);
        assert_eq!(h.prev_change_before(10), Some(5));
        assert_eq!(h.prev_change_before(5), None);
        assert_eq!(h.prev_change_before(7), Some(5));
    }
}
