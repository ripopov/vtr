//! Measuring between two times in time and in each clock's cycles. Cycles
//! are counted, not divided: a clock's position at `t` is the number of its
//! recorded rising edges at or before `t` plus the fraction of the running
//! cycle, and the cycles between two times are the difference of their
//! positions. That stays right across frequency changes (each cycle keeps
//! its own length) and gating (a stopped clock adds nothing).

use std::fmt;

use vtr::ClockTimeline;

use crate::clock::Clock;

/// Cycles of one clock between two times, `to` minus `from`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cycles {
    /// Both ends lie on edges, before the first edge or where the clock is
    /// stopped: exactly the edges in the interval.
    Whole(i64),
    /// An end lies mid-cycle.
    Fraction(f64),
    /// The clock's edges are still loading.
    Loading,
}

impl fmt::Display for Cycles {
    /// `80`, `−26.4`, `…`: one decimal when an end lies mid-cycle.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = |negative: bool| if negative { "−" } else { "" };
        match *self {
            Self::Whole(n) => write!(f, "{}{}", sign(n < 0), n.unsigned_abs()),
            Self::Fraction(x) => {
                let x = (x * 10.0).round() / 10.0;
                write!(f, "{}{:.1}", sign(x < 0.0), x.abs())
            }
            Self::Loading => f.write_str("…"),
        }
    }
}

/// A clock's position at a time: whole edges at or before it, and how far
/// into the running cycle it lies (0 before the first edge, on an edge and
/// while the clock is stopped).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub edges: u64,
    pub fraction: f64,
}

pub fn position(timeline: &ClockTimeline, t: u64) -> Position {
    match timeline.cycle_at(t) {
        None => Position {
            edges: 0,
            fraction: 0.0,
        },
        Some(at) => Position {
            edges: at.cycle + 1,
            fraction: if at.stopped { 0.0 } else { at.fraction },
        },
    }
}

/// Cycles of `timeline` from `from` to `to`, negative when `to` is earlier.
pub fn cycles(timeline: &ClockTimeline, from: u64, to: u64) -> Cycles {
    let (a, b) = (position(timeline, from), position(timeline, to));
    let whole = b.edges as i64 - a.edges as i64;
    if a.fraction == 0.0 && b.fraction == 0.0 {
        Cycles::Whole(whole)
    } else {
        Cycles::Fraction(whole as f64 + (b.fraction - a.fraction))
    }
}

/// One clock's count in a [`Measurement`].
#[derive(Clone, Debug, PartialEq)]
pub struct ClockCount {
    pub name: String,
    pub cycles: Cycles,
}

/// The span from one time to another, in time and in each clock's cycles.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub from: u64,
    pub to: u64,
    pub clocks: Vec<ClockCount>,
}

impl Measurement {
    /// `to` minus `from` in file time units.
    pub fn dt(&self) -> i128 {
        i128::from(self.to) - i128::from(self.from)
    }
}

/// Measure from `from` to `to` in each of `clocks`.
pub fn measure(clocks: &[&Clock], from: u64, to: u64) -> Measurement {
    Measurement {
        from,
        to,
        clocks: clocks
            .iter()
            .map(|clock| ClockCount {
                name: clock.name.clone(),
                cycles: clock
                    .timeline()
                    .map_or(Cycles::Loading, |t| cycles(t, from, to)),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(stretches: &[(u64, u64, u64)], open: bool) -> ClockTimeline {
        ClockTimeline::new(stretches.to_vec(), open).unwrap()
    }

    /// Every recorded edge, in order.
    fn edges(t: &ClockTimeline) -> Vec<u64> {
        t.stretches()
            .iter()
            .flat_map(|s| {
                let step = s.period.max(1);
                (s.begin..=s.end).step_by(step as usize)
            })
            .collect()
    }

    #[test]
    fn counting_follows_a_frequency_change() {
        // 10 units per cycle, then 4 from t = 100.
        let t = timeline(&[(0, 90, 10), (100, 200, 4)], false);
        assert_eq!(cycles(&t, 30, 104), Cycles::Whole(8));
        assert_eq!(cycles(&t, 35, 106), Cycles::Fraction(8.0));
        assert_eq!(cycles(&t, 35, 108), Cycles::Fraction(8.5));
        assert_eq!(cycles(&t, 108, 35), Cycles::Fraction(-8.5));
        // Dividing by either period would be wrong here.
        assert_ne!(73.0 / 10.0, 8.5);
        assert_ne!(73.0 / 4.0, 8.5);
    }

    #[test]
    fn a_stopped_clock_adds_nothing_and_before_the_first_edge_counts_zero() {
        // Stopped from 50 to 1000: the gap is far longer than two periods.
        let t = timeline(&[(10, 50, 10), (1000, 1040, 10)], false);
        assert!(t.cycle_at(500).unwrap().stopped);
        assert_eq!(cycles(&t, 50, 1000), Cycles::Whole(1));
        assert_eq!(cycles(&t, 45, 500), Cycles::Fraction(0.5));
        assert_eq!(cycles(&t, 500, 700), Cycles::Whole(0));
        assert_eq!(cycles(&t, 0, 5), Cycles::Whole(0));
        assert_eq!(cycles(&t, 0, 30), Cycles::Whole(3));
        // A single-edge clock.
        let one = timeline(&[(100, 100, 0)], false);
        assert_eq!(cycles(&one, 0, 200), Cycles::Whole(1));
        assert_eq!(cycles(&one, 100, 100), Cycles::Whole(0));
    }

    #[test]
    fn counts_equal_brute_force_edge_counting_on_random_timelines() {
        let mut seed = 11u64;
        let mut rnd = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n.max(1)
        };
        for trial in 0..500 {
            // Stretches of changing period, some gaps long enough to stop,
            // some single edges.
            let mut raw = Vec::new();
            let mut at = rnd(50);
            for _ in 0..1 + rnd(5) {
                let period = [0, 1 + rnd(20)][(rnd(5) != 0) as usize];
                let n = if period == 0 { 0 } else { rnd(30) };
                raw.push((at, at + n * period, period));
                at += n * period + 1 + [rnd(5), 50 + rnd(500)][rnd(2) as usize];
            }
            let t = timeline(&raw, rnd(2) == 0);
            let all = edges(&t);
            let end = at + 50;
            for _ in 0..40 {
                let (a, b) = (rnd(end), rnd(end));
                let (lo, hi) = (a.min(b), a.max(b));
                let between = all.iter().filter(|&&e| e > lo && e <= hi).count() as i64;
                let signed = if b >= a { between } else { -between };
                let on_edge = |x: u64| all.contains(&x) || position(&t, x).fraction == 0.0;
                match cycles(&t, a, b) {
                    Cycles::Whole(n) => {
                        assert_eq!(n, signed, "trial {trial}: {a}..{b} {raw:?}");
                    }
                    Cycles::Fraction(x) => {
                        assert!(!(on_edge(a) && on_edge(b)), "trial {trial}");
                        assert!(
                            (x - signed as f64).abs() < 1.0,
                            "trial {trial}: {a}..{b} {x} vs {signed} {raw:?}"
                        );
                    }
                    Cycles::Loading => unreachable!(),
                }
                // Whole edges are exact whatever the fractions.
                let (pa, pb) = (position(&t, lo), position(&t, hi));
                assert_eq!((pb.edges - pa.edges) as i64, between, "trial {trial}");
            }
        }
    }

    #[test]
    fn counts_print_signed_with_one_decimal_mid_cycle() {
        assert_eq!(Cycles::Whole(80).to_string(), "80");
        assert_eq!(Cycles::Whole(-3).to_string(), "−3");
        assert_eq!(Cycles::Fraction(26.44).to_string(), "26.4");
        assert_eq!(Cycles::Fraction(-0.46).to_string(), "−0.5");
        assert_eq!(Cycles::Fraction(-0.04).to_string(), "0.0");
        assert_eq!(Cycles::Loading.to_string(), "…");
    }
}
