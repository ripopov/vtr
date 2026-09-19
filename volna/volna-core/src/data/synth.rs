//! Synthetic trace source for stress testing.
//!
//! Histories are procedural: change `i` happens at `i * period + jitter(i)`
//! and its value is a hash of `i`, so a 100-million-transition signal costs
//! no memory and the renderer's search path is exercised exactly as it would
//! be on a file-backed history.

use std::sync::Arc;

use super::history::SignalHistory;
use super::source::{Direction, Hierarchy, SignalRef, TraceInfo, Variable};
use super::value::{Bit, SignalShape, WaveValue};
use crate::session::Session;

pub struct SynthSource {
    info: TraceInfo,
    hierarchy: Hierarchy,
    signals: Vec<Arc<dyn SignalHistory>>,
}

fn mix(mut x: u64) -> u64 {
    // splitmix64 finaliser
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

#[derive(Clone, Copy)]
enum Pattern {
    /// Toggles every change.
    Clock,
    /// Pseudo-random bit, mostly stable.
    RandomBit,
    /// Increments by one per change.
    Counter,
    /// Pseudo-random vector.
    RandomVector,
    /// Occasional `x`/`z` values.
    Glitchy,
}

struct ProceduralHistory {
    shape: SignalShape,
    count: usize,
    period: u64,
    jitter: u64,
    seed: u64,
    pattern: Pattern,
}

impl ProceduralHistory {
    fn value_bits(&self, i: usize) -> String {
        let width = self.shape.width() as usize;
        let h = mix(i as u64 ^ self.seed);
        match self.pattern {
            Pattern::Clock => {
                if i.is_multiple_of(2) {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Pattern::RandomBit => {
                if h & 0b111 < 3 {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Pattern::Counter => {
                let mut s = String::with_capacity(width);
                for b in (0..width).rev() {
                    s.push(if (i >> b) & 1 == 1 { '1' } else { '0' });
                }
                s
            }
            Pattern::RandomVector => {
                let mut s = String::with_capacity(width);
                let mut v = h;
                for b in 0..width {
                    if b % 64 == 0 {
                        v = mix(h ^ (b as u64));
                    }
                    s.push(if (v >> (b % 64)) & 1 == 1 { '1' } else { '0' });
                }
                s
            }
            Pattern::Glitchy => match h % 13 {
                0 => "x".repeat(width),
                1 => "z".repeat(width),
                _ => {
                    let mut s = String::with_capacity(width);
                    for b in 0..width {
                        s.push(if (h >> (b % 64)) & 1 == 1 { '1' } else { '0' });
                    }
                    s
                }
            },
        }
    }
}

impl SignalHistory for ProceduralHistory {
    fn shape(&self) -> SignalShape {
        self.shape
    }
    fn len(&self) -> usize {
        self.count
    }
    fn time(&self, i: usize) -> u64 {
        let base = i as u64 * self.period;
        if self.jitter == 0 {
            base
        } else {
            base + mix(i as u64 ^ 0x5151) % self.jitter
        }
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        match i {
            None => WaveValue::Bits("x".repeat(self.shape.width() as usize)),
            Some(i) => WaveValue::Bits(self.value_bits(i)),
        }
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        match i {
            None => Bit::X,
            Some(i) => match self.pattern {
                Pattern::Clock => {
                    if i % 2 == 0 {
                        Bit::One
                    } else {
                        Bit::Zero
                    }
                }
                Pattern::RandomBit => {
                    if mix(i as u64 ^ self.seed) & 0b111 < 3 {
                        Bit::One
                    } else {
                        Bit::Zero
                    }
                }
                _ => Bit::Other,
            },
        }
    }
}

impl SynthSource {
    /// A synthetic trace whose busiest signal has `transitions` changes.
    pub fn new(transitions: usize) -> Self {
        let transitions = transitions.max(2);
        let period = 10u64; // time units per clock half-period
        let mut hierarchy = Hierarchy::default();
        let top = hierarchy.push_scope("synth".into(), "module".into(), None);
        hierarchy.push_scope("core".into(), "module".into(), Some(top));

        let mut signals: Vec<Arc<dyn SignalHistory>> = Vec::new();
        let mut add = |name: &str,
                       scope: usize,
                       shape: SignalShape,
                       hist: ProceduralHistory,
                       dir: Direction,
                       h: &mut Hierarchy| {
            let id = signals.len();
            signals.push(Arc::new(hist));
            let vid = h.vars.len();
            h.vars.push(Variable {
                name: name.into(),
                scope,
                shape,
                var_type: if shape == SignalShape::Bit {
                    "wire".into()
                } else {
                    "logic".into()
                },
                direction: dir,
                signal: SignalRef(id as u32),
                enum_table: None,
            });
            h.scopes[scope].vars.push(vid);
        };
        let bit = SignalShape::Bit;
        let vec = |w: u32| SignalShape::Vector { width: w };
        add(
            "clk",
            0,
            bit,
            ProceduralHistory {
                shape: bit,
                count: transitions,
                period,
                jitter: 0,
                seed: 1,
                pattern: Pattern::Clock,
            },
            Direction::Input,
            &mut hierarchy,
        );
        add(
            "rst_n",
            0,
            bit,
            ProceduralHistory {
                shape: bit,
                count: 2,
                period: period * 8,
                jitter: 0,
                seed: 2,
                pattern: Pattern::Clock,
            },
            Direction::Input,
            &mut hierarchy,
        );
        add(
            "valid",
            1,
            bit,
            ProceduralHistory {
                shape: bit,
                count: transitions / 4,
                period: period * 8,
                jitter: period * 4,
                seed: 3,
                pattern: Pattern::RandomBit,
            },
            Direction::Output,
            &mut hierarchy,
        );
        add(
            "ready",
            1,
            bit,
            ProceduralHistory {
                shape: bit,
                count: transitions / 6,
                period: period * 12,
                jitter: period * 6,
                seed: 4,
                pattern: Pattern::RandomBit,
            },
            Direction::Input,
            &mut hierarchy,
        );
        add(
            "counter",
            1,
            vec(8),
            ProceduralHistory {
                shape: vec(8),
                count: transitions / 2,
                period: period * 2,
                jitter: 0,
                seed: 5,
                pattern: Pattern::Counter,
            },
            Direction::None,
            &mut hierarchy,
        );
        add(
            "addr",
            1,
            vec(16),
            ProceduralHistory {
                shape: vec(16),
                count: transitions / 8,
                period: period * 16,
                jitter: period * 8,
                seed: 6,
                pattern: Pattern::RandomVector,
            },
            Direction::Output,
            &mut hierarchy,
        );
        add(
            "data",
            1,
            vec(32),
            ProceduralHistory {
                shape: vec(32),
                count: transitions / 8,
                period: period * 16,
                jitter: period * 8,
                seed: 7,
                pattern: Pattern::RandomVector,
            },
            Direction::InOut,
            &mut hierarchy,
        );
        add(
            "fp_result",
            1,
            vec(32),
            ProceduralHistory {
                shape: vec(32),
                count: transitions / 16,
                period: period * 32,
                jitter: period * 16,
                seed: 8,
                pattern: Pattern::RandomVector,
            },
            Direction::Output,
            &mut hierarchy,
        );
        add(
            "wide_bus",
            1,
            vec(128),
            ProceduralHistory {
                shape: vec(128),
                count: transitions / 32,
                period: period * 64,
                jitter: period * 32,
                seed: 9,
                pattern: Pattern::RandomVector,
            },
            Direction::None,
            &mut hierarchy,
        );
        add(
            "status",
            1,
            vec(4),
            ProceduralHistory {
                shape: vec(4),
                count: transitions / 10,
                period: period * 20,
                jitter: period * 10,
                seed: 10,
                pattern: Pattern::Glitchy,
            },
            Direction::None,
            &mut hierarchy,
        );

        let end = (transitions as u64) * period;
        let info = TraceInfo {
            design_id: None,
            name: format!("synthetic ({} transitions)", human(transitions)),
            timescale: -9,
            time_range: (0, end),
            signal_count: signals.len(),
            change_count: Some(signals.iter().map(|s| s.len() as u64).sum()),
            time_unit: None,
        };
        SynthSource {
            info,
            hierarchy,
            signals,
        }
    }
}

fn human(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}

impl Session for SynthSource {
    fn info(&self) -> &TraceInfo {
        &self.info
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        self.signals
            .get(signal.0 as usize)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown signal {}", signal.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_times_are_monotonic_and_searchable() {
        let src = SynthSource::new(10_000);
        for var in &src.hierarchy().vars {
            let h = src.load_signal(var.signal).unwrap();
            for i in 1..h.len().min(2000) {
                assert!(h.time(i) >= h.time(i - 1), "{}: {i}", var.name);
            }
            let t = h.time(h.len() / 2);
            let i = h.index_at(t).unwrap();
            assert!(h.time(i) <= t && (i + 1 == h.len() || h.time(i + 1) > t));
            assert_eq!(h.index_at_hint(t, 0), Some(i));
            assert_eq!(h.index_at_hint(t, h.len() - 1), Some(i));
        }
    }
}
