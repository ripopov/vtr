//! Time formatting and tick placement for the timeline header.

use super::viewport::Viewport;

const TICK_STEPS: [f64; 4] = [1.0, 2.0, 2.5, 5.0];

/// SI units from femtoseconds to seconds, as (exponent, suffix).
const UNITS: [(i32, &str); 6] = [
    (-15, "fs"),
    (-12, "ps"),
    (-9, "ns"),
    (-6, "µs"),
    (-3, "ms"),
    (0, "s"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    pub time: f64,
    pub label: String,
}

/// Choose the display unit for a duration of `units` time units at `timescale`.
/// Returns the unit exponent and suffix such that the value is in `[1, 1000)`
/// where possible.
fn choose_unit(units: f64, timescale: i8) -> (i32, &'static str) {
    if units <= 0.0 || !units.is_finite() {
        return UNITS
            .iter()
            .copied()
            .find(|(e, _)| *e == timescale as i32)
            .unwrap_or((-9, "ns"));
    }
    let seconds_exp = units.log10() + timescale as f64;
    let mut best = UNITS[0];
    for u in UNITS {
        if seconds_exp >= u.0 as f64 {
            best = u;
        }
    }
    // Never display finer than the file's own unit.
    if best.0 < timescale as i32 {
        best = UNITS
            .iter()
            .copied()
            .find(|(e, _)| *e >= timescale as i32)
            .unwrap_or(best);
    }
    best
}

fn trim_float(v: f64, max_decimals: usize) -> String {
    let s = format!("{v:.*}", max_decimals);
    if s.contains('.') {
        let s = s.trim_end_matches('0');
        s.trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Format an absolute time with an automatically chosen unit, e.g. `1.25 µs`.
pub fn format_time(t: f64, timescale: i8) -> String {
    let (exp, suffix) = choose_unit(t.abs().max(1.0), timescale);
    let v = t * 10f64.powi(timescale as i32 - exp);
    let decimals = if v.abs() >= 100.0 {
        1
    } else if v.abs() >= 10.0 {
        2
    } else {
        3
    };
    format!("{} {suffix}", trim_float(v, decimals))
}

/// Format a time in a fixed unit (used for tick labels so all share a unit).
fn format_in_unit(t: f64, timescale: i8, exp: i32, decimals: usize) -> String {
    let v = t * 10f64.powi(timescale as i32 - exp);
    trim_float(v, decimals)
}

/// Tick step for a viewport: the smallest "nice" step that keeps labels at
/// least `min_spacing_px` apart.
fn tick_step(viewport: &Viewport, width_px: f64, min_spacing_px: f64) -> f64 {
    let max_ticks = (width_px / min_spacing_px).max(1.0);
    let raw = viewport.width() / max_ticks;
    let mag = 10f64.powf(raw.log10().floor());
    for s in TICK_STEPS {
        let step = s * mag;
        if step >= raw {
            return step;
        }
    }
    10.0 * mag
}

/// Ticks (time and label) for the visible window. Labels share one unit.
pub fn ticks(
    viewport: &Viewport,
    width_px: f64,
    timescale: i8,
    min_spacing_px: f64,
) -> (Vec<Tick>, &'static str) {
    if width_px <= 0.0 {
        return (Vec::new(), "");
    }
    let step = tick_step(viewport, width_px, min_spacing_px);
    let (exp, suffix) = choose_unit(step, timescale);
    // Decimals needed to distinguish consecutive labels in this unit.
    let step_in_unit = step * 10f64.powi(timescale as i32 - exp);
    let decimals = if step_in_unit >= 1.0 {
        0
    } else {
        (-step_in_unit.log10().floor()) as usize
    }
    .min(6);
    let first = (viewport.start / step).ceil() as i64;
    let last = (viewport.end / step).floor() as i64;
    let mut out = Vec::new();
    let mut k = first;
    while k <= last && out.len() < 1000 {
        let t = k as f64 * step;
        out.push(Tick {
            time: t,
            label: format_in_unit(t, timescale, exp, decimals),
        });
        k += 1;
    }
    (out, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(choose_unit(1.0, -9), (-9, "ns"));
        assert_eq!(choose_unit(1500.0, -9), (-6, "µs"));
        assert_eq!(choose_unit(2_000_000.0, -12), (-6, "µs"));
        assert_eq!(choose_unit(0.5, -9), (-9, "ns"));
    }

    #[test]
    fn formats() {
        assert_eq!(format_time(1250.0, -9), "1.25 µs");
        assert_eq!(format_time(958189.0, -9), "958.2 µs");
        assert_eq!(format_time(0.0, -9), "0 ns");
        assert_eq!(format_time(5.0, -12), "5 ps");
        assert_eq!(format_time(3_000_000.0, -9), "3 ms");
    }

    #[test]
    fn nice_steps() {
        let v = Viewport {
            start: 0.0,
            end: 1000.0,
        };
        let step = tick_step(&v, 1000.0, 100.0);
        assert_eq!(step, 100.0);
        let (t, unit) = ticks(&v, 1000.0, -9, 100.0);
        assert_eq!(unit, "ns");
        assert_eq!(t.len(), 11);
        assert_eq!(t[1].label, "100");
        let v = Viewport {
            start: 1000.0,
            end: 1250.0,
        };
        let (t, unit) = ticks(&v, 1000.0, -9, 100.0);
        assert_eq!(unit, "ns");
        assert_eq!(t[0].label, "1000");
        assert!(t.windows(2).all(|w| w[1].time > w[0].time));
    }

    #[test]
    fn sub_unit_decimals() {
        // 0..2.5 ns viewport at ps resolution shows fractional ns labels.
        let v = Viewport {
            start: 0.0,
            end: 2500.0,
        };
        let (t, unit) = ticks(&v, 1000.0, -12, 100.0);
        assert_eq!(unit, "ps");
        assert_eq!(t[1].label, "250");
    }
}
