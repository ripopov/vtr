//! The two-axis zoom of a pipeline panel. One gesture walks a path through
//! (time scale, row height) space instead of scaling both axes blindly: both
//! axes move together along the aspect the user sees, an axis that reaches
//! its limit hands the whole step to the other one, and the aspect from
//! before an axis stopped is remembered, so zooming out and back in returns
//! the same view.
//!
//! Coordinates are natural logarithms: `time` of pixels per time unit, `rows`
//! of row height in pixels. On a log scale "both axes × f" is a 45° line, the
//! path is that line clipped into the zoom box, and a step of `ln f` moves
//! every axis that moves by exactly `f`.
//!
//! ```text
//!  rows ▲
//!    hi ┼ ─ ─ ─ ─ ─ ─ ─ ─ ─ ●━━━━━━━━━━● zoom-in end
//!       │                 ╱  rows at their cap: time only
//!       │               ╱
//!       │             ╱  both axes, same factor
//!       │           ● ← the aspect remembered here
//!       │           ┃  time at fit: rows only
//!    lo ┼ ─ ─ ─ ─ ─ ● zoom-out end: the whole trace
//!       └───────────┼──────────────────────┼──▶ time
//!                   lo                     hi
//! ```

/// Row height the two-axis zoom stops growing rows at, in design pixels:
/// the wave panel's row height. A taller row set by a rows-only zoom
/// raises a panel's own cap (up to [`super::rows::ROW_PX_MAX`]).
pub const ROW_PX_CAP: f32 = 24.0;
/// Cycle width the two-axis zoom stops zooming time at, in design pixels.
/// Wide enough for a long stage name; a linked wave panel may zoom deeper.
pub const CYCLE_PX_MAX: f64 = 120.0;

/// Below this (in log space) two positions are the same.
const EPS: f64 = 1e-9;
/// An axis this close to a limit (in log space) is pinned there.
const PINNED: f64 = 1e-6;

/// A zoom state: `ln` of pixels per time unit and of row height in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomPoint {
    pub time: f64,
    pub rows: f64,
}

impl ZoomPoint {
    /// `rows - time`: the aspect a point shows.
    pub fn aspect(self) -> f64 {
        self.rows - self.time
    }
}

/// The range each axis may zoom in, `(lo, hi)` in log space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomBox {
    pub time: (f64, f64),
    pub rows: (f64, f64),
}

impl ZoomBox {
    /// Whether `p` is inside the box with neither axis at a limit.
    pub fn is_interior(&self, p: ZoomPoint) -> bool {
        let inside = |v: f64, (lo, hi): (f64, f64)| v > lo + PINNED && v < hi - PINNED;
        inside(p.time, self.time) && inside(p.rows, self.rows)
    }

    /// Whether an axis at `v` is at (or beyond) the low end of `range`.
    pub fn at_lo(v: f64, range: (f64, f64)) -> bool {
        v <= range.0 + PINNED
    }
}

/// Room left for `v` to move in direction `dir` within `range`. A value
/// outside the range never moves further out but may move back in.
fn room(v: f64, (lo, hi): (f64, f64), dir: f64) -> f64 {
    if dir > 0.0 {
        (hi.max(v) - v).max(0.0)
    } else {
        (v - lo.min(v)).max(0.0)
    }
}

/// Walk `step` (`ln` of the zoom factor; positive zooms in) from `from`
/// along the line of `aspect` clipped into `bounds`.
///
/// On the line both axes move by the step until one reaches a limit, then
/// the other takes the rest. Off the line (only possible along an edge)
/// zooming in moves the axis that lags behind the aspect and zooming out the
/// one ahead of it, until the point is back on the line; an axis that cannot
/// move in that direction hands the step to the other.
pub fn walk(from: ZoomPoint, bounds: ZoomBox, aspect: f64, step: f64) -> ZoomPoint {
    if !step.is_finite() {
        return from;
    }
    let dir = step.signum();
    let mut left = step.abs();
    let mut p = from;
    // Each pass ends on the line, at a limit or with the step spent, so a
    // few passes cover every corner of the path.
    for _ in 0..8 {
        if left <= EPS {
            break;
        }
        let room_t = room(p.time, bounds.time, dir);
        let room_r = room(p.rows, bounds.rows, dir);
        if room_t <= EPS && room_r <= EPS {
            break;
        }
        // Positive above the line (rows ahead of time), negative below.
        let dev = p.aspect() - aspect;
        if dev.abs() <= EPS && room_t > EPS && room_r > EPS {
            let d = left.min(room_t).min(room_r);
            p.time += dir * d;
            p.rows += dir * d;
            left -= d;
            continue;
        }
        let prefer_rows = if dev.abs() <= EPS {
            room_r > EPS
        } else if dir > 0.0 {
            dev < 0.0
        } else {
            dev > 0.0
        };
        let move_rows = if prefer_rows {
            room_r > EPS
        } else {
            room_t <= EPS
        };
        if move_rows {
            let mut d = left.min(room_r);
            // Moving rows changes the deviation by `dir * d`.
            if dev.abs() > EPS && dev * dir < 0.0 {
                d = d.min(dev.abs());
            }
            p.rows += dir * d;
            left -= d;
        } else {
            let mut d = left.min(room_t);
            // Moving time changes the deviation by `-dir * d`.
            if dev.abs() > EPS && dev * dir > 0.0 {
                d = d.min(dev.abs());
            }
            p.time += dir * d;
            left -= d;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    const LN2: f64 = std::f64::consts::LN_2;

    fn bounds() -> ZoomBox {
        ZoomBox {
            time: (0.0, 10.0 * LN2),
            rows: (0.0, 6.0 * LN2),
        }
    }

    fn close(a: ZoomPoint, b: ZoomPoint) -> bool {
        (a.time - b.time).abs() < 1e-9 && (a.rows - b.rows).abs() < 1e-9
    }

    #[test]
    fn on_the_line_both_axes_move_by_the_step() {
        let from = ZoomPoint {
            time: 3.0 * LN2,
            rows: 2.0 * LN2,
        };
        let to = walk(from, bounds(), from.aspect(), LN2);
        assert!(close(
            to,
            ZoomPoint {
                time: 4.0 * LN2,
                rows: 3.0 * LN2
            }
        ));
    }

    #[test]
    fn a_stopped_axis_hands_the_whole_step_to_the_other() {
        // Rows reach their cap half-way through the step; time takes the rest.
        let from = ZoomPoint {
            time: 3.0 * LN2,
            rows: 5.5 * LN2,
        };
        let to = walk(from, bounds(), from.aspect(), LN2);
        assert!(close(
            to,
            ZoomPoint {
                time: 4.0 * LN2,
                rows: 6.0 * LN2
            }
        ));
        // Time at its fit: zooming out moves rows only, at full speed.
        let from = ZoomPoint {
            time: 0.0,
            rows: 3.0 * LN2,
        };
        let to = walk(from, bounds(), 3.0 * LN2, -LN2);
        assert!(close(
            to,
            ZoomPoint {
                time: 0.0,
                rows: 2.0 * LN2
            }
        ));
    }

    #[test]
    fn every_step_moves_until_a_corner_and_out_then_in_is_reversible() {
        let start = ZoomPoint {
            time: 2.0 * LN2,
            rows: 4.0 * LN2,
        };
        let aspect = start.aspect();
        let b = bounds();
        let mut p = start;
        let mut path = vec![p];
        loop {
            let next = walk(p, b, aspect, -0.5 * LN2);
            if close(next, p) {
                break;
            }
            // One notch moves at least one axis by the whole factor.
            let moved = (next.time - p.time).abs().max((next.rows - p.rows).abs());
            assert!((moved - 0.5 * LN2).abs() < 1e-9);
            p = next;
            path.push(p);
        }
        assert!(
            close(
                p,
                ZoomPoint {
                    time: 0.0,
                    rows: 0.0
                }
            ),
            "{p:?}"
        );
        for expected in path.iter().rev().skip(1) {
            p = walk(p, b, aspect, 0.5 * LN2);
            assert!(close(p, *expected), "{p:?} != {expected:?}");
        }
        loop {
            let next = walk(p, b, aspect, 0.5 * LN2);
            if close(next, p) {
                break;
            }
            p = next;
        }
        assert!(close(
            p,
            ZoomPoint {
                time: 10.0 * LN2,
                rows: 6.0 * LN2
            }
        ));
    }

    #[test]
    fn off_the_line_the_lagging_axis_catches_up_first() {
        // On the bottom edge left of the line: zooming in moves time only
        // until the aspect is restored, then both.
        let b = bounds();
        let from = ZoomPoint {
            time: 1.0 * LN2,
            rows: 0.0,
        };
        let aspect = -3.0 * LN2;
        let to = walk(from, b, aspect, 3.0 * LN2);
        assert!(close(
            to,
            ZoomPoint {
                time: 4.0 * LN2,
                rows: 1.0 * LN2
            }
        ));
    }

    #[test]
    fn a_value_outside_its_range_moves_back_but_never_further_out() {
        let b = bounds();
        // Rows taller than the cap (a rows-only zoom, a saved view).
        let from = ZoomPoint {
            time: 5.0 * LN2,
            rows: 7.0 * LN2,
        };
        let to = walk(from, b, from.aspect(), LN2);
        assert_eq!(to.rows, from.rows);
        assert!((to.time - 6.0 * LN2).abs() < 1e-9);
        let to = walk(from, b, from.aspect(), -LN2);
        assert!(close(
            to,
            ZoomPoint {
                time: 4.0 * LN2,
                rows: 6.0 * LN2
            }
        ));
        // A degenerate range (every row fits at the cap) holds that axis.
        let flat = ZoomBox {
            rows: (3.0, 3.0),
            ..b
        };
        let from = ZoomPoint {
            time: 5.0 * LN2,
            rows: 3.0,
        };
        let to = walk(from, flat, from.aspect(), -LN2);
        assert!(close(
            to,
            ZoomPoint {
                time: 4.0 * LN2,
                rows: 3.0
            }
        ));
    }

    #[test]
    fn interior_and_pinned_points() {
        let b = bounds();
        assert!(b.is_interior(ZoomPoint {
            time: 1.0,
            rows: 1.0
        }));
        assert!(!b.is_interior(ZoomPoint {
            time: 0.0,
            rows: 1.0
        }));
        assert!(!b.is_interior(ZoomPoint {
            time: 1.0,
            rows: 6.0 * LN2
        }));
        assert!(ZoomBox::at_lo(-1.0, b.time));
    }
}
