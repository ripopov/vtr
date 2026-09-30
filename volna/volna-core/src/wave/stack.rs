//! Stacked areas: a group drawn as one area whose layers are its signals
//! (`docs/stacked-areas.html`).
//!
//! Every signal below a stacked group that reads as a number is a layer: a
//! bit counts 0 or 1, a vector or real the number its row's format reads
//! ([`Translator::numeric_kind`]), the same number its plot would draw.
//! List order is stack order, so the last layer sits on the baseline.
//! Positive parts stack up from zero and negative parts down from it.
//! Where a layer is undefined (X, Z, or no value yet), the layers below it
//! are drawn and the sum reads X; a missing value is never read as zero.
//!
//! Members change at different times, so a stack is built on the union of
//! their change times, never by sample index. Views with few changes draw
//! every step of that union exactly; busier views draw one pixel column
//! each, with every layer's time-weighted mean over the column ([`frame`]).
//! Means over one span add up, so the stack's top is the column's mean
//! total. The whole-trace scale comes from one merged walk over the layers
//! ([`TotalSummary`]), on the load worker when they are long.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use crate::data::value_view::ValueView;
use crate::data::{Bit, NumericKind, SignalHistory, SignalShape, Translator};
use crate::wave::analog::{self, EXACT_RATIO, Sample};
use crate::wave::viewport::Viewport;

/// Layers that change at most this often in total get their whole-trace
/// scale from a walk on the UI thread; longer ones build it on the load
/// worker.
pub const WALK_MAX: usize = super::group::WALK_MAX;
/// The merged walk sums changes incrementally and recomposes every this
/// many instants, so rounding cannot drift.
const RECOMPOSE_EVERY: u32 = 256;

/// How a member counts in a stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reading {
    /// A one-bit signal: 0 or 1, whatever its format.
    Bit,
    /// A vector or real, as its row's format reads it.
    Number(NumericKind),
}

impl Reading {
    /// How a row of `shape` shown with `tr` counts, or `None` when it has no
    /// number (text, strings, events): such a row is left out.
    pub fn of(shape: SignalShape, tr: &dyn Translator) -> Option<Self> {
        match shape {
            SignalShape::Bit => Some(Self::Bit),
            SignalShape::Vector { .. } | SignalShape::Real => tr.numeric_kind().map(Self::Number),
            SignalShape::Event | SignalShape::Text => None,
        }
    }

    /// Whether every number it reads is a whole number.
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::Bit | Self::Number(NumericKind::Unsigned | NumericKind::Signed)
        )
    }

    /// Value `i` of `h` (the value before the first change when `None`).
    pub fn sample(self, h: &dyn SignalHistory, i: Option<usize>) -> Sample {
        let Self::Number(kind) = self else {
            return match h.bit(i) {
                Bit::Zero => Sample::Value(0.0),
                Bit::One => Sample::Value(1.0),
                Bit::Unavailable => Sample::Missing,
                // A weak drive still has a level.
                Bit::Other => match h.value_view(i) {
                    ValueView::Logic(l) if l.width == 1 => match l.bit(0) {
                        b'h' | b'H' => Sample::Value(1.0),
                        b'l' | b'L' => Sample::Value(0.0),
                        _ => Sample::Undefined,
                    },
                    _ => Sample::Undefined,
                },
                Bit::X | Bit::Z | Bit::DontCare => Sample::Undefined,
            };
        };
        analog::sample(h, kind, i)
    }
}

/// A member of a stacked group, read as a number.
#[derive(Clone)]
pub struct Layer {
    /// The member's entry in its panel.
    pub entry: usize,
    pub history: Arc<dyn SignalHistory>,
    pub reading: Reading,
}

impl Layer {
    pub fn sample(&self, i: Option<usize>) -> Sample {
        self.reading.sample(self.history.as_ref(), i)
    }

    /// The value held at `t`.
    pub fn at(&self, t: u64) -> Sample {
        self.sample(self.history.index_at(t))
    }
}

/// What a [`TotalSummary`] summarizes: each layer's history and reading, in
/// list order.
pub type Key = Vec<(usize, Reading)>;

pub fn key(layers: &[Layer]) -> Key {
    layers
        .iter()
        .map(|l| (analog::history_identity(&l.history), l.reading))
        .collect()
}

/// Where one instant (or one column's means) of a stack reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Composed {
    /// Top of the positive parts drawn; zero when there are none.
    pub top: f64,
    /// Bottom of the negative parts drawn; zero when there are none.
    pub bottom: f64,
    /// Some layer is undefined: it and the layers above it are not drawn.
    pub undefined: bool,
}

/// Stack `values` (in list order) from the baseline up, calling
/// `band(layer, low, high)` for every part drawn: positive parts go up
/// from the top so far, negative ones down from the bottom so far, until
/// the first undefined layer.
pub fn compose(values: &[Sample], mut band: impl FnMut(usize, f64, f64)) -> Composed {
    let mut c = Composed::default();
    for (k, v) in values.iter().enumerate().rev() {
        match *v {
            Sample::Value(v) if v >= 0.0 => {
                band(k, c.top, c.top + v);
                c.top += v;
            }
            Sample::Value(v) => {
                band(k, c.bottom + v, c.bottom);
                c.bottom += v;
            }
            Sample::Undefined | Sample::Missing => {
                c.undefined = true;
                break;
            }
        }
    }
    c
}

/// The sum of `values`, or `None` when one is undefined: the sum reads X.
pub fn total(values: &[Sample]) -> Option<f64> {
    values.iter().try_fold(0.0, |sum, v| match v {
        Sample::Value(v) => Some(sum + v),
        _ => None,
    })
}

/// Each layer's value at `t`, in list order.
pub fn values_at(layers: &[Layer], t: u64) -> Vec<Sample> {
    layers.iter().map(|l| l.at(t)).collect()
}

/// The last change at or before `t`, `None` before the first (or before 0).
fn index_at(h: &dyn SignalHistory, t: f64) -> Option<usize> {
    if t < 0.0 {
        return None;
    }
    h.index_at(t.floor() as u64)
}

/// Every layer's changes inside `vp`.
pub fn visible_changes(layers: &[Layer], vp: &Viewport) -> usize {
    layers
        .iter()
        .map(|l| analog::visible_changes(l.history.as_ref(), vp))
        .sum()
}

/// What a stacked row draws in one frame of `width` pixel columns.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// Every step exactly (`true`), or one pixel column each.
    pub exact: bool,
    /// Each span's left and right edge in pixels from the plot's left: a
    /// step between two changes, or a column `x..x + 1`.
    pub spans: Vec<(f64, f64)>,
    /// Layer values per span, in list order: the values held during a
    /// step, or each layer's mean over a column.
    values: Vec<Sample>,
    layers: usize,
    /// Per column, the layers' changes inside it after its first instant:
    /// its values are means only where there are some. Empty when exact.
    changes: Vec<u32>,
}

impl Frame {
    /// The layer values of span `span`.
    pub fn values(&self, span: usize) -> &[Sample] {
        &self.values[span * self.layers..(span + 1) * self.layers]
    }

    /// The layers' changes inside column `span` after its first instant;
    /// none for a step.
    pub fn changes(&self, span: usize) -> u32 {
        self.changes.get(span).copied().unwrap_or(0)
    }

    /// The span under `x` pixels from the plot's left.
    pub fn span_at(&self, x: f64) -> Option<usize> {
        let i = self.spans.partition_point(|s| s.1 <= x);
        (i < self.spans.len() && self.spans[i].0 <= x).then_some(i)
    }

    /// The lowest and highest point the frame draws, with the baseline.
    pub fn extent(&self) -> (f64, f64) {
        (0..self.spans.len()).fold((0.0, 0.0), |(lo, hi), s| {
            let c = compose(self.values(s), |_, _, _| {});
            (lo.min(c.bottom), hi.max(c.top))
        })
    }
}

/// The frame of `layers` over `vp` in `width` columns: every step when the
/// layers change at most a quarter as often as there are columns, else
/// one time-weighted mean per layer and column. Its allocations are
/// bounded by the width.
pub fn frame(layers: &[Layer], vp: &Viewport, width: usize) -> Frame {
    if width == 0 || vp.width() <= 0.0 || layers.is_empty() {
        return Frame {
            layers: layers.len(),
            ..Frame::default()
        };
    }
    if visible_changes(layers, vp) as f64 <= width as f64 * EXACT_RATIO {
        steps(layers, vp, width)
    } else {
        columns(layers, vp, width)
    }
}

/// Every step of the layers' merged changes inside `vp`.
pub fn steps(layers: &[Layer], vp: &Viewport, width: usize) -> Frame {
    let w = width as f64;
    let mut times: Vec<u64> = Vec::new();
    for l in layers {
        let h = l.history.as_ref();
        let a = index_at(h, vp.start).map_or(0, |i| i + 1);
        let b = index_at(h, vp.end).map_or(0, |i| i + 1);
        times.extend((a..b).map(|i| h.time(i)));
    }
    times.sort_unstable();
    times.dedup();
    let bounds: Vec<f64> = std::iter::once(vp.start)
        .chain(times.iter().map(|&t| t as f64))
        .chain(std::iter::once(vp.end))
        .collect();
    let mut at: Vec<Option<usize>> = layers
        .iter()
        .map(|l| index_at(l.history.as_ref(), vp.start))
        .collect();
    let mut frame = Frame {
        exact: true,
        layers: layers.len(),
        ..Frame::default()
    };
    for pair in bounds.windows(2) {
        let (ta, tb) = (pair[0], pair[1]);
        if tb <= ta {
            continue;
        }
        for (l, i) in layers.iter().zip(&mut at) {
            let h = l.history.as_ref();
            // Up to the last change at `ta`, of several at one time.
            while let Some(next) =
                Some(i.map_or(0, |i| i + 1)).filter(|&n| n < h.len() && h.time(n) as f64 <= ta)
            {
                *i = Some(next);
            }
            frame.values.push(l.sample(*i));
        }
        frame.spans.push((vp.x_of(ta, w), vp.x_of(tb, w)));
    }
    frame
}

/// Each layer's time-weighted mean over each pixel column of `vp`, walking
/// its changes inside the view once: O(visible changes + layers × width).
/// A layer undefined for any part of a column is undefined there, so a
/// short X survives any zoom.
pub fn columns(layers: &[Layer], vp: &Viewport, width: usize) -> Frame {
    let n = layers.len();
    let w = width as f64;
    let mut values = vec![Sample::Missing; width * n];
    let mut changes = vec![0u32; width];
    for (k, l) in layers.iter().enumerate() {
        let h = l.history.as_ref();
        let next_time = |i: Option<usize>| {
            let next = i.map_or(0, |i| i + 1);
            if next < h.len() {
                h.time(next) as f64
            } else {
                f64::INFINITY
            }
        };
        let mut i = index_at(h, vp.time_at(0.0, w));
        let mut value = l.sample(i);
        let mut next = next_time(i);
        for c in 0..width {
            let (ta, tb) = (vp.time_at(c as f64, w), vp.time_at((c + 1) as f64, w));
            let (mut sum, mut defined, mut t) = (0.0, true, ta);
            loop {
                let until = next.min(tb);
                if until > t {
                    match value {
                        Sample::Value(v) => sum += v * (until - t),
                        _ => defined = false,
                    }
                }
                if next >= tb {
                    break;
                }
                if next > ta {
                    changes[c] += 1;
                }
                t = next;
                i = Some(i.map_or(0, |i| i + 1));
                value = l.sample(i);
                next = next_time(i);
            }
            values[c * n + k] = if defined {
                Sample::Value(sum / (tb - ta))
            } else {
                Sample::Undefined
            };
        }
    }
    Frame {
        exact: false,
        spans: (0..width).map(|c| (c as f64, c as f64 + 1.0)).collect(),
        values,
        layers: n,
        changes,
    }
}

/// What the pointer reads over a stacked row.
#[derive(Clone, Debug, PartialEq)]
pub struct Readout {
    /// The span read: a step, or a column whose values are means.
    pub span: usize,
    /// Each layer's value (or mean), in list order.
    pub values: Vec<Sample>,
    /// Their sum, `None` when a layer is undefined.
    pub total: Option<f64>,
    /// The layer whose part is under the pointer.
    pub hot: Option<usize>,
}

impl Readout {
    /// A layer's share of the stack: its size over the sum of every
    /// layer's size, so negative parts count too.
    pub fn share(&self, layer: usize) -> Option<f64> {
        let Sample::Value(v) = self.values.get(layer)? else {
            return None;
        };
        let whole: f64 = self
            .values
            .iter()
            .map(|v| match v {
                Sample::Value(v) => v.abs(),
                _ => 0.0,
            })
            .sum();
        (whole > 0.0).then(|| v.abs() / whole)
    }
}

/// What the pointer at `x` pixels from the plot's left and at value
/// `value` (on the plot's scale) reads from `frame`.
pub fn readout(frame: &Frame, x: f64, value: f64) -> Option<Readout> {
    let span = frame.span_at(x)?;
    let values = frame.values(span).to_vec();
    let mut hot = None;
    compose(&values, |k, lo, hi| {
        if value >= lo && value < hi {
            hot = Some(k);
        }
    });
    Some(Readout {
        span,
        total: total(&values),
        values,
        hot,
    })
}

/// The next time after `from` (or before it, with `forward == false`)
/// where the stack's sum changes, stepping over changes that cancel out.
/// Gives up after `limit` candidate times.
pub fn total_edge(layers: &[Layer], from: u64, forward: bool, limit: usize) -> Option<u64> {
    let sum_at = |t: u64| {
        let values = values_at(layers, t);
        total(&values).map(f64::to_bits)
    };
    let mut at = from;
    for _ in 0..limit {
        let t = if forward {
            layers
                .iter()
                .filter_map(|l| l.history.next_change_after(at))
                .min()?
        } else {
            layers
                .iter()
                .filter_map(|l| l.history.prev_change_before(at))
                .max()?
        };
        if sum_at(t) != t.checked_sub(1).map_or(Some(f64::NAN.to_bits()), sum_at) {
            return Some(t);
        }
        at = t;
    }
    None
}

/// A scale from `lo` to `hi` rounded out to clean numbers, always holding
/// zero: 1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6 or 8 times a power of ten.
pub fn clean_range((lo, hi): (f64, f64)) -> (f64, f64) {
    fn up(x: f64) -> f64 {
        if x <= 0.0 || !x.is_finite() {
            return 0.0;
        }
        let e = 10f64.powf(x.log10().floor());
        let f = x / e;
        let step = [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0]
            .into_iter()
            .find(|&s| s >= f * (1.0 - 1e-12))
            .unwrap_or(10.0);
        step * e
    }
    let lo = if lo < 0.0 { -up(-lo) } else { 0.0 };
    let hi = up(hi);
    if hi > lo { (lo, hi) } else { (0.0, 1.0) }
}

/// Every change of the layers' histories: a stack of at most [`WALK_MAX`]
/// is walked on the UI thread.
pub fn changes(layers: &[Layer]) -> usize {
    layers.iter().map(|l| l.history.len()).sum()
}

/// What one merged walk over a stack's layers gives: the lowest bottom and
/// highest top it reaches over the whole trace, which set its scale. Built
/// on the load worker for long layers; the document shares it between the
/// panels that stack the same signals the same way and charges it to the
/// memory budget.
pub struct TotalSummary {
    key: Key,
    range: (f64, f64),
    reservation: Option<crate::remote::memory::Reservation>,
}

impl TotalSummary {
    /// Walk every change of `layers` in time order, composing the stack at
    /// each distinct change time: O(changes × log layers).
    pub fn build(layers: &[Layer]) -> Self {
        let mut at: Vec<Option<usize>> = vec![None; layers.len()];
        let mut values: Vec<Sample> = layers.iter().map(|l| l.sample(None)).collect();
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = layers
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.history.is_empty())
            .map(|(k, l)| Reverse((l.history.time(0), k)))
            .collect();
        // Sums of the defined parts, kept incrementally while no layer is
        // undefined; a stack with an undefined layer is composed in full.
        let sums = |values: &[Sample]| {
            values
                .iter()
                .fold((0.0, 0.0, 0u32), |(p, n, u), v| match *v {
                    Sample::Value(v) if v >= 0.0 => (p + v, n, u),
                    Sample::Value(v) => (p, n + v, u),
                    _ => (p, n, u + 1),
                })
        };
        let (mut pos, mut neg, mut undefined) = sums(&values);
        let mut range = (0.0f64, 0.0f64);
        let mut account = |values: &[Sample], pos: f64, neg: f64, undefined: u32| {
            let (top, bottom) = if undefined == 0 {
                (pos, neg)
            } else {
                let c = compose(values, |_, _, _| {});
                (c.top, c.bottom)
            };
            range = (range.0.min(bottom), range.1.max(top));
        };
        account(&values, pos, neg, undefined);
        let mut instants = 0u32;
        while let Some(&Reverse((t, _))) = heap.peek() {
            while let Some(&Reverse((tk, k))) = heap.peek() {
                if tk != t {
                    break;
                }
                heap.pop();
                let h = layers[k].history.as_ref();
                let mut i = at[k].map_or(0, |i| i + 1);
                while i + 1 < h.len() && h.time(i + 1) == t {
                    i += 1;
                }
                at[k] = Some(i);
                let new = layers[k].sample(Some(i));
                for (v, sign) in [(values[k], -1.0), (new, 1.0)] {
                    match v {
                        Sample::Value(v) if v >= 0.0 => pos += sign * v,
                        Sample::Value(v) => neg += sign * v,
                        _ if sign > 0.0 => undefined += 1,
                        _ => undefined -= 1,
                    }
                }
                values[k] = new;
                if i + 1 < h.len() {
                    heap.push(Reverse((h.time(i + 1), k)));
                }
            }
            instants += 1;
            if instants.is_multiple_of(RECOMPOSE_EVERY) {
                (pos, neg, undefined) = sums(&values);
            }
            account(&values, pos, neg, undefined);
        }
        Self {
            key: key(layers),
            range,
            reservation: None,
        }
    }

    /// Charge the summary to a memory budget for as long as it lives.
    pub fn account(mut self, budget: &crate::remote::memory::MemoryBudget) -> anyhow::Result<Self> {
        self.reservation = Some(budget.reserve_object("the stack total", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        (std::mem::size_of::<Self>() + self.key.len() * std::mem::size_of::<(usize, Reading)>())
            as u64
    }

    /// The layers it summarizes (see [`key`]).
    pub fn key(&self) -> &Key {
        &self.key
    }

    /// The lowest bottom and highest top over the whole trace, with zero.
    pub fn range(&self) -> (f64, f64) {
        self.range
    }
}

/// A stack's whole-trace summary, as the document holds it.
pub enum TotalLoad {
    Building,
    Ready(Arc<TotalSummary>),
    /// Refused (the memory budget); the scale follows the visible window.
    Failed,
}

/// A number as a stack's cells and labels show it: whole numbers plainly,
/// others with three or four significant digits.
pub fn number(v: f64) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{v:.0}");
    }
    let digits = match v.abs() {
        a if a >= 1000.0 => 0,
        a if a >= 100.0 => 1,
        a if a >= 10.0 => 2,
        _ => 3,
    };
    let text = format!("{v:.digits$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    };
    if text == "-0" { "0".into() } else { text }
}
