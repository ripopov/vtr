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
//! total. Long layers read those means from an [`IntegralSummary`], so a
//! frame costs O(layers × width) instead of its visible changes. The
//! whole-trace scale comes from one merged walk over the layers
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
/// Changes per block of an [`IntegralSummary`].
pub const INTEGRAL_BLOCK: usize = 64;
/// A stack whose layers change more often than [`WALK_MAX`] in all gets an
/// [`IntegralSummary`] for each layer with at least this many changes;
/// shorter layers are cheap to walk.
pub const INTEGRAL_MIN_CHANGES: usize = analog::SUMMARY_MIN_CHANGES;
/// A layer with more visible changes than this many per pixel column reads
/// its column means from its summary instead of walking them.
const SUMMARY_PER_COLUMN: usize = 16;

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
    /// Its integral summary, when one matching the history and reading is
    /// resident.
    pub summary: Option<Arc<IntegralSummary>>,
    /// A summary for this long history is still building.
    pub building: bool,
}

impl Layer {
    pub fn new(entry: usize, history: Arc<dyn SignalHistory>, reading: Reading) -> Self {
        Self {
            entry,
            history,
            reading,
            summary: None,
            building: false,
        }
    }

    /// With `summary` when it summarizes this history read this way.
    pub fn with_summary(mut self, summary: Option<Arc<IntegralSummary>>) -> Self {
        self.summary = summary.filter(|s| s.matches(&self.history, self.reading));
        self
    }

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
    waiting: bool,
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

    /// Too many changes to walk before the layers' summaries are ready.
    pub fn waiting(&self) -> bool {
        self.waiting
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
    let visible = visible_changes(layers, vp);
    if visible as f64 <= width as f64 * EXACT_RATIO {
        return steps(layers, vp, width);
    }
    // A long layer whose summary builds waits rather than walk on the UI thread.
    let waiting = layers.iter().any(|l| {
        l.building
            && l.summary.is_none()
            && analog::visible_changes(l.history.as_ref(), vp) >= INTEGRAL_MIN_CHANGES
    });
    if waiting && visible > WALK_MAX {
        return Frame {
            waiting: true,
            layers: layers.len(),
            ..Frame::default()
        };
    }
    columns(layers, vp, width)
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

/// Each layer's time-weighted mean over each pixel column of `vp`: from
/// its [`IntegralSummary`] when it has one and changes more than 16 times
/// per column in view, O(width × (log n + block)), else by walking its
/// changes inside the view once, O(visible changes + width). A layer
/// undefined for any part of a column is undefined there, so a short X
/// survives any zoom.
pub fn columns(layers: &[Layer], vp: &Viewport, width: usize) -> Frame {
    let n = layers.len();
    let w = width as f64;
    let mut values = vec![Sample::Missing; width * n];
    let mut changes = vec![0u32; width];
    for (k, l) in layers.iter().enumerate() {
        let h = l.history.as_ref();
        if let Some(summary) = &l.summary
            && analog::visible_changes(h, vp) > SUMMARY_PER_COLUMN * width
        {
            let mut prefixes = Prefixes::new(summary, h);
            let mut ia = index_at(h, vp.time_at(0.0, w));
            let mut density = 0;
            for c in 0..width {
                let (ta, tb) = (vp.time_at(c as f64, w), vp.time_at((c + 1) as f64, w));
                // Columns change about as often as their neighbours: start
                // the search where the last column's density says.
                let guess = ia.map(|i| i + density);
                let ib = index_before(h, tb, guess).max(ia);
                density = changes_between(ia, ib);
                values[c * n + k] = prefixes.mean(ia, ib, ta, tb);
                changes[c] += (changes_between(ia, ib)) as u32;
                // The next column starts where this one ends.
                ia = match ib {
                    Some(ib) if tb >= 0.0 => h.index_at_hint(tb.floor() as u64, ib),
                    _ => index_at(h, tb),
                }
                .max(ib);
            }
            continue;
        }
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
        waiting: false,
    }
}

/// The last change strictly before `t`, searching from `hint`.
fn index_before(h: &dyn SignalHistory, t: f64, hint: Option<usize>) -> Option<usize> {
    if t <= 0.0 {
        return None;
    }
    let t = (t.ceil() as u64).saturating_sub(1);
    match hint {
        Some(hint) => h.index_at_hint(t, hint),
        None => h.index_at(t),
    }
}

fn changes_between(a: Option<usize>, b: Option<usize>) -> usize {
    match (a, b) {
        (_, None) => 0,
        (None, Some(b)) => b + 1,
        (Some(a), Some(b)) => b.saturating_sub(a),
    }
}

/// A sum kept with Kahan's compensation, so long prefix sums stay exact to
/// the last bits.
#[derive(Clone, Copy, Default)]
struct Kahan {
    sum: f64,
    carry: f64,
}

impl Kahan {
    fn add(&mut self, v: f64) {
        let y = v - self.carry;
        let t = self.sum + y;
        self.carry = (t - self.sum) - y;
        self.sum = t;
    }
}

/// Per block of [`INTEGRAL_BLOCK`] changes of one history read one way, the
/// integral of its values over their spans since its first change, and how
/// many of those spans are undefined for a while: about 0.19 bytes per
/// change. A column's mean is then the difference of two prefixes, each a
/// block entry and at most half a block read from the history. Built on the
/// load worker for the long layers of a stack; the document shares it
/// between panels and releases it (and its memory reservation) with the
/// history. One-bit members get one too, since they have no analog summary.
pub struct IntegralSummary {
    reading: Reading,
    history: usize,
    len: usize,
    /// `integral[m]`: the sum of every span of changes `0..64 m`.
    integral: Vec<f64>,
    /// `undefined[m]`: how many of those spans are undefined and last.
    undefined: Vec<u32>,
    reservation: Option<crate::remote::memory::Reservation>,
}

impl std::fmt::Debug for IntegralSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntegralSummary")
            .field("reading", &self.reading)
            .field("len", &self.len)
            .field("bytes", &self.resident_bytes())
            .finish()
    }
}

impl IntegralSummary {
    /// One pass over `h` read as `reading`. The span of change `j` is its
    /// value from its time to the next change's; the last change's span is
    /// open and never summed.
    pub fn build(h: &Arc<dyn SignalHistory>, reading: Reading) -> Self {
        let len = h.len();
        let blocks = len.saturating_sub(1) / INTEGRAL_BLOCK + 1;
        let mut integral = Vec::with_capacity(blocks);
        let mut undefined = Vec::with_capacity(blocks);
        let (mut sum, mut count) = (Kahan::default(), 0u32);
        for j in 0..len.saturating_sub(1) {
            if j % INTEGRAL_BLOCK == 0 {
                integral.push(sum.sum);
                undefined.push(count);
            }
            let dur = (h.time(j + 1) - h.time(j)) as f64;
            match reading.sample(h.as_ref(), Some(j)) {
                Sample::Value(v) => sum.add(v * dur),
                _ if dur > 0.0 => count += 1,
                _ => {}
            }
        }
        if integral.is_empty() {
            integral.push(0.0);
            undefined.push(0);
        }
        Self {
            reading,
            history: analog::history_identity(h),
            len,
            integral,
            undefined,
            reservation: None,
        }
    }

    /// Charge the summary to a memory budget for as long as it lives.
    pub fn account(mut self, budget: &crate::remote::memory::MemoryBudget) -> anyhow::Result<Self> {
        self.reservation =
            Some(budget.reserve_object("the integral summary", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        (self.integral.len() * std::mem::size_of::<f64>()
            + self.undefined.len() * std::mem::size_of::<u32>()) as u64
    }

    pub fn reading(&self) -> Reading {
        self.reading
    }

    /// Identity of the summarized history.
    pub fn identity(&self) -> usize {
        self.history
    }

    /// Whether this summarizes `h` read as `reading`.
    pub fn matches(&self, h: &Arc<dyn SignalHistory>, reading: Reading) -> bool {
        self.reading == reading
            && self.history == analog::history_identity(h)
            && self.len == h.len()
    }

    /// The sum of the spans of changes `0..k` (`k < len`) and how many of
    /// them are undefined: the nearer block entry, corrected by at most half
    /// a block of spans read from `h`.
    pub fn prefix(&self, h: &dyn SignalHistory, k: usize) -> (f64, u32) {
        let span = |j: usize| {
            let dur = (h.time(j + 1) - h.time(j)) as f64;
            match self.reading.sample(h, Some(j)) {
                Sample::Value(v) => (v * dur, 0),
                _ if dur > 0.0 => (0.0, 1),
                _ => (0.0, 0),
            }
        };
        let m = (k / INTEGRAL_BLOCK).min(self.integral.len() - 1);
        let up = m + 1;
        if k % INTEGRAL_BLOCK > INTEGRAL_BLOCK / 2 && up < self.integral.len() {
            // Nearer the next block: subtract the spans up to it.
            let (mut sum, mut count) = (self.integral[up], self.undefined[up]);
            for j in k..up * INTEGRAL_BLOCK {
                let (v, u) = span(j);
                sum -= v;
                count -= u;
            }
            return (sum, count);
        }
        let (mut sum, mut count) = (self.integral[m], self.undefined[m]);
        for j in m * INTEGRAL_BLOCK..k {
            let (v, u) = span(j);
            sum += v;
            count += u;
        }
        (sum, count)
    }

    /// The mean over `[ta, tb)` of `h`, where `ia` holds at `ta` and `ib` is
    /// the last change before `tb`; undefined when any part of it is.
    pub fn mean(
        &self,
        h: &dyn SignalHistory,
        ia: Option<usize>,
        ib: Option<usize>,
        ta: f64,
        tb: f64,
    ) -> Sample {
        Prefixes::new(self, h).mean(ia, ib, ta, tb)
    }
}

/// Prefixes of one summarized history read left to right: a prefix just
/// after the last one continues it instead of reading a block entry, so a
/// column that starts where the previous one ended reads almost nothing.
struct Prefixes<'a> {
    summary: &'a IntegralSummary,
    h: &'a dyn SignalHistory,
    last: Option<(usize, f64, u32)>,
}

impl<'a> Prefixes<'a> {
    fn new(summary: &'a IntegralSummary, h: &'a dyn SignalHistory) -> Self {
        Self {
            summary,
            h,
            last: None,
        }
    }

    fn at(&mut self, k: usize) -> (f64, u32) {
        let block = k % INTEGRAL_BLOCK;
        let to_block = block.min(INTEGRAL_BLOCK - block);
        let (sum, count) = match self.last {
            Some((last, sum, count)) if k >= last && k - last <= to_block => {
                let (mut sum, mut count) = (sum, count);
                for j in last..k {
                    let dur = (self.h.time(j + 1) - self.h.time(j)) as f64;
                    match self.summary.reading.sample(self.h, Some(j)) {
                        Sample::Value(v) => sum += v * dur,
                        _ if dur > 0.0 => count += 1,
                        _ => {}
                    }
                }
                (sum, count)
            }
            _ => self.summary.prefix(self.h, k),
        };
        self.last = Some((k, sum, count));
        (sum, count)
    }

    fn mean(&mut self, ia: Option<usize>, ib: Option<usize>, ta: f64, tb: f64) -> Sample {
        let h = self.h;
        let value = |i: Option<usize>| self.summary.reading.sample(h, i);
        let Some(ib) = ib.filter(|&ib| Some(ib) != ia) else {
            return match value(ia) {
                Sample::Value(v) => Sample::Value(v),
                _ => Sample::Undefined,
            };
        };
        let first = ia.map_or(0, |i| i + 1);
        let (Sample::Value(va), Sample::Value(vb)) = (value(ia), value(Some(ib))) else {
            return Sample::Undefined;
        };
        let (from, from_undefined) = self.at(first);
        let (to, to_undefined) = self.at(ib);
        if to_undefined > from_undefined {
            return Sample::Undefined;
        }
        let head = va * (h.time(first) as f64 - ta);
        let tail = vb * (tb - h.time(ib) as f64);
        Sample::Value((head + (to - from) + tail) / (tb - ta))
    }
}

/// A layer's integral summary, as the document holds it.
pub enum IntegralLoad {
    Building {
        history: usize,
    },
    Ready(Arc<IntegralSummary>),
    /// Refused (the memory budget); the painter walks the changes instead.
    Failed {
        history: usize,
    },
}

impl IntegralLoad {
    pub fn history(&self) -> usize {
        match self {
            Self::Building { history } | Self::Failed { history } => *history,
            Self::Ready(s) => s.identity(),
        }
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

/// The layers' changes in time order, one instant at a time, with the
/// values held after each: what the whole-trace walk and the peaks of a
/// view share. Sums of the defined values are kept incrementally and
/// recomputed every [`RECOMPOSE_EVERY`] instants, so rounding cannot drift.
struct Merge<'a> {
    layers: &'a [Layer],
    at: Vec<Option<usize>>,
    values: Vec<Sample>,
    heap: BinaryHeap<Reverse<(u64, usize)>>,
    pos: f64,
    neg: f64,
    undefined: u32,
    instants: u32,
}

impl<'a> Merge<'a> {
    /// The values held at `at`, each layer's last change at or before it
    /// (`None` before a layer's first).
    fn new(layers: &'a [Layer], at: Vec<Option<usize>>) -> Self {
        let values: Vec<Sample> = layers.iter().zip(&at).map(|(l, &i)| l.sample(i)).collect();
        let heap = layers
            .iter()
            .zip(&at)
            .enumerate()
            .filter_map(|(k, (l, i))| {
                let next = i.map_or(0, |i| i + 1);
                (next < l.history.len()).then(|| Reverse((l.history.time(next), k)))
            })
            .collect();
        let mut merge = Self {
            layers,
            at,
            values,
            heap,
            pos: 0.0,
            neg: 0.0,
            undefined: 0,
            instants: 0,
        };
        merge.resum();
        merge
    }

    fn resum(&mut self) {
        (self.pos, self.neg, self.undefined) =
            self.values
                .iter()
                .fold((0.0, 0.0, 0u32), |(p, n, u), v| match *v {
                    Sample::Value(v) if v >= 0.0 => (p + v, n, u),
                    Sample::Value(v) => (p, n + v, u),
                    _ => (p, n, u + 1),
                });
    }

    /// The time of the next change, if any.
    fn next(&self) -> Option<u64> {
        self.heap.peek().map(|Reverse((t, _))| *t)
    }

    /// Apply every change at the next change time and return that time.
    fn step(&mut self) -> Option<u64> {
        let t = self.next()?;
        while let Some(&Reverse((tk, k))) = self.heap.peek() {
            if tk != t {
                break;
            }
            self.heap.pop();
            let h = self.layers[k].history.as_ref();
            let mut i = self.at[k].map_or(0, |i| i + 1);
            while i + 1 < h.len() && h.time(i + 1) == t {
                i += 1;
            }
            self.at[k] = Some(i);
            let new = self.layers[k].sample(Some(i));
            for (v, sign) in [(self.values[k], -1.0), (new, 1.0)] {
                match v {
                    Sample::Value(v) if v >= 0.0 => self.pos += sign * v,
                    Sample::Value(v) => self.neg += sign * v,
                    _ if sign > 0.0 => self.undefined += 1,
                    _ => self.undefined -= 1,
                }
            }
            self.values[k] = new;
            if i + 1 < h.len() {
                self.heap.push(Reverse((h.time(i + 1), k)));
            }
        }
        self.instants += 1;
        if self.instants.is_multiple_of(RECOMPOSE_EVERY) {
            self.resum();
        }
        Some(t)
    }

    /// The sum of the values held, `None` when one is undefined.
    fn total(&self) -> Option<f64> {
        (self.undefined == 0).then_some(self.pos + self.neg)
    }

    /// The top and bottom of the stack held.
    fn reach(&self) -> (f64, f64) {
        if self.undefined == 0 {
            (self.pos, self.neg)
        } else {
            let c = compose(&self.values, |_, _, _| {});
            (c.top, c.bottom)
        }
    }
}

/// The lowest and highest value of a column's total, `None` where it is
/// undefined throughout.
pub type Peak = Option<(f64, f64)>;

fn widen(peak: &mut Peak, v: f64) {
    *peak = Some(match *peak {
        Some((lo, hi)) => (lo.min(v), hi.max(v)),
        None => (v, v),
    });
}

/// The total's lowest and highest value in each of `width` pixel columns of
/// `vp`, from a merged walk over the layers' changes in view:
/// O(visible changes × log layers + width). Exact; the peak band of a view
/// with few changes.
pub fn peaks(layers: &[Layer], vp: &Viewport, width: usize) -> Vec<Peak> {
    let w = width as f64;
    let t0 = vp.time_at(0.0, w);
    let at = layers
        .iter()
        .map(|l| index_at(l.history.as_ref(), t0))
        .collect();
    let mut merge = Merge::new(layers, at);
    let mut out = vec![None; width];
    for (c, peak) in out.iter_mut().enumerate() {
        let (ta, tb) = (vp.time_at(c as f64, w), vp.time_at((c + 1) as f64, w));
        // A change at the column's first instant replaces what held before.
        while merge.next().is_some_and(|t| (t as f64) <= ta) {
            merge.step();
        }
        if let Some(v) = merge.total() {
            widen(peak, v);
        }
        while merge.next().is_some_and(|t| (t as f64) < tb) {
            merge.step();
            if let Some(v) = merge.total() {
                widen(peak, v);
            }
        }
    }
    out
}

/// Time blocks of a [`TotalSummary`]'s finest level, at most.
const TOTAL_BLOCKS: u64 = 1 << 16;
/// Each coarser level of a [`TotalSummary`] merges this many blocks.
const TOTAL_FANOUT: usize = 16;

/// What one merged walk over a stack's layers gives: the lowest bottom and
/// highest top it reaches over the whole trace, which set its scale, and
/// the total's lowest and highest value per block of time, with coarser
/// levels of 16 blocks above, for the peak band of busy views. No summary
/// of the members can give those, because the maximum of a sum is not the
/// sum of the maxima. Built on the load worker for long layers; the
/// document shares it between the panels that stack the same signals the
/// same way and charges it to the memory budget.
pub struct TotalSummary {
    key: Key,
    range: (f64, f64),
    start: u64,
    shift: u32,
    /// `levels[0]` has one extent per block of `1 << shift` ticks from
    /// `start`; each level above merges [`TOTAL_FANOUT`] of the one below.
    levels: Vec<Vec<Peak>>,
    reservation: Option<crate::remote::memory::Reservation>,
}

impl TotalSummary {
    /// Walk every change of `layers` in time order over the trace's time
    /// range `span`: O(changes × log layers + blocks).
    pub fn build(layers: &[Layer], span: (u64, u64)) -> Self {
        let (start, end) = (span.0, span.1.max(span.0));
        let shift = (0..64)
            .find(|&s| ((end - start) >> s) < TOTAL_BLOCKS)
            .unwrap_or(63);
        let blocks = ((end - start) >> shift) as usize + 1;
        let block = |t: u64| ((t.clamp(start, end) - start) >> shift) as usize;
        let mut level: Vec<Peak> = vec![None; blocks];
        // The total held from `since` up to (not including) `until`.
        let hold = |level: &mut [Peak], total: Option<f64>, since: u64, until: u64| {
            if let Some(v) = total
                && until > since
            {
                for peak in &mut level[block(since)..=block(until - 1)] {
                    widen(peak, v);
                }
            }
        };
        let mut merge = Merge::new(layers, vec![None; layers.len()]);
        let mut range = (0.0f64, 0.0f64);
        let mut account = |merge: &Merge<'_>| {
            let (top, bottom) = merge.reach();
            range = (range.0.min(bottom), range.1.max(top));
        };
        account(&merge);
        let mut since = start;
        while let Some(t) = merge.next() {
            hold(&mut level, merge.total(), since, t);
            merge.step();
            since = since.max(t);
            account(&merge);
        }
        hold(&mut level, merge.total(), since, end + 1);
        let mut levels = vec![level];
        while levels.last().is_some_and(|l| l.len() > TOTAL_FANOUT) {
            let above = levels
                .last()
                .unwrap()
                .chunks(TOTAL_FANOUT)
                .map(|chunk| {
                    chunk.iter().flatten().fold(None, |mut peak, &(lo, hi)| {
                        widen(&mut peak, lo);
                        widen(&mut peak, hi);
                        peak
                    })
                })
                .collect();
            levels.push(above);
        }
        Self {
            key: key(layers),
            range,
            start,
            shift,
            levels,
            reservation: None,
        }
    }

    fn block_ticks(&self, level: usize) -> f64 {
        (1u64 << self.shift) as f64 * (TOTAL_FANOUT as f64).powi(level as i32)
    }

    /// The total's lowest and highest value in each of `width` columns of
    /// `vp`, from the coarsest level whose blocks fit a column (the finest
    /// when none does): a column reads the blocks it touches, so its band
    /// may reach up to one block past it on either side, never below its
    /// own peak.
    pub fn peaks(&self, vp: &Viewport, width: usize) -> Vec<Peak> {
        let mut out = vec![None; width];
        if width == 0 || vp.width() <= 0.0 {
            return out;
        }
        let per_column = vp.width() / width as f64;
        let k = (0..self.levels.len())
            .rev()
            .find(|&k| self.block_ticks(k) <= per_column)
            .unwrap_or(0);
        let (level, ticks) = (&self.levels[k], self.block_ticks(k));
        let w = width as f64;
        let block_at = |t: f64| ((t - self.start as f64) / ticks).floor();
        for (c, peak) in out.iter_mut().enumerate() {
            let a = block_at(vp.time_at(c as f64, w)).max(0.0);
            // The block holding the column's last instant.
            let b = block_at(vp.time_at((c + 1) as f64, w) - 1e-9 * ticks);
            if b < 0.0 || a >= level.len() as f64 {
                continue;
            }
            let b = (b as usize).min(level.len() - 1);
            for &(lo, hi) in level[a as usize..=b].iter().flatten() {
                widen(peak, lo);
                widen(peak, hi);
            }
        }
        out
    }

    /// Charge the summary to a memory budget for as long as it lives.
    pub fn account(mut self, budget: &crate::remote::memory::MemoryBudget) -> anyhow::Result<Self> {
        self.reservation = Some(budget.reserve_object("the stack total", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        let blocks: usize = self.levels.iter().map(Vec::len).sum();
        (std::mem::size_of::<Self>()
            + self.key.len() * std::mem::size_of::<(usize, Reading)>()
            + blocks * std::mem::size_of::<Peak>()) as u64
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
