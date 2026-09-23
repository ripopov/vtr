//! Analog drawing of numeric signal rows: the row setting, its vertical
//! range, the block min/max summary of long histories, and the geometry the
//! painter strokes. A row's translator decides the numbers
//! ([`Translator::numeric_kind`]), so signed and unsigned readings of one bus
//! plot differently.
//!
//! Sparse views draw every change exactly. When changes outnumber a quarter
//! of the plot's pixel columns, each column draws from its minimum to its
//! maximum and joins its neighbours at its entry and exit values (M4), which
//! renders a line chart without pixel error, so single-sample glitches
//! survive any zoom level. An [`AnalogSummary`] answers those minima and
//! maxima in O(log n), so the cost of a frame is bounded by its pixels.

use std::sync::Arc;

use crate::data::value_view::ValueView;
use crate::data::{NumericKind, SignalHistory, SignalShape, Translator};
use crate::geometry::{Point, point};
use crate::wave::model::RowHeight;
use crate::wave::viewport::Viewport;

/// How consecutive values are joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalogDraw {
    /// Sample-and-hold: a value holds until the next change.
    Step,
    /// Straight lines between samples.
    Linear,
}

/// Which values the vertical range fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalogRange {
    /// Minimum and maximum over the whole history; stable while panning.
    Trace,
    /// Minimum and maximum of the visible window, eased as it changes.
    Window,
    /// The full range of the vector type in the row's format.
    Type,
}

impl AnalogDraw {
    /// Registers hold their value; real signals sample a continuous quantity.
    pub fn default_for(shape: SignalShape) -> Self {
        if shape == SignalShape::Real {
            Self::Linear
        } else {
            Self::Step
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Linear => "linear",
        }
    }
}

impl AnalogRange {
    pub fn label(self) -> &'static str {
        match self {
            Self::Trace => "whole trace",
            Self::Window => "visible window",
            Self::Type => "type limits",
        }
    }
}

/// Visible changes at or below this fraction of the plot width are drawn
/// exactly; above it, per-column min/max envelopes.
pub const EXACT_RATIO: f64 = 0.25;
/// Linear samples at least this many design pixels apart are marked.
pub const DOT_SPACING: f32 = 9.0;
/// Changes per leaf of an [`AnalogSummary`].
pub const SUMMARY_BLOCK: usize = 64;
/// Histories with fewer changes are scanned directly and get no summary.
pub const SUMMARY_MIN_CHANGES: usize = 1 << 14;
/// Time constant of the displayed range easing towards its target.
const RANGE_TAU_S: f64 = 0.065;

/// A signal row drawn as a plot.
#[derive(Clone, Debug)]
pub struct Analog {
    pub draw: AnalogDraw,
    pub range: AnalogRange,
    /// The height analog replaced; restored when analog is turned off while
    /// the row still has [`RowHeight::ANALOG`]. An explicit resize clears it.
    pub restore_height: Option<RowHeight>,
    /// The displayed range, easing towards `target` (view state, not saved).
    pub shown: Option<(f64, f64)>,
    pub target: Option<(f64, f64)>,
    /// Whole-history range of a short history, cached per history and
    /// reading; long histories read their summary's root instead.
    trace: Option<TraceRange>,
}

#[derive(Clone, Debug)]
struct TraceRange {
    history: usize,
    kind: NumericKind,
    range: Option<(f64, f64)>,
}

impl PartialEq for Analog {
    /// Settings equality; the displayed range and caches are view state.
    fn eq(&self, other: &Self) -> bool {
        self.draw == other.draw && self.range == other.range
    }
}

impl Analog {
    pub fn new(draw: AnalogDraw, range: AnalogRange) -> Self {
        Self {
            draw,
            range,
            restore_height: None,
            shown: None,
            target: None,
            trace: None,
        }
    }

    /// Recompute the target range for `vp`; the first target is shown at once.
    /// A long history's whole-trace range waits for its summary; meanwhile a
    /// zoomed-in view fits its visible window.
    pub fn update_target(
        &mut self,
        series: &Series<'_>,
        tr: &dyn Translator,
        shape: SignalShape,
        vp: &Viewport,
    ) {
        let range = match self.range {
            AnalogRange::Type => tr.limits(shape),
            AnalogRange::Window if series.waiting(visible_changes(series.history, vp)) => return,
            AnalogRange::Window => series.window_range(vp),
            AnalogRange::Trace => None,
        };
        let range = match range {
            Some(range) => Some(range),
            None => match self.trace_range(series) {
                Some(range) => range,
                // Until the summary arrives, a cheap window stands in.
                None if !series.waiting(visible_changes(series.history, vp)) => {
                    series.window_range(vp)
                }
                None => return,
            },
        };
        let target = padded(range.unwrap_or((0.0, 1.0)), shape);
        self.target = Some(target);
        if self.shown.is_none() {
            self.shown = Some(target);
        }
    }

    /// The whole-history range: the summary's root, or a cached scan of a
    /// short history. `None` while a long history's summary is building.
    fn trace_range(&mut self, series: &Series<'_>) -> Option<Option<(f64, f64)>> {
        if let Some(summary) = series.summary {
            return Some(summary.root().range());
        }
        if series.waiting(series.history.len()) {
            return None;
        }
        let history = series.identity;
        match &self.trace {
            Some(c) if c.history == history && c.kind == series.kind => Some(c.range),
            _ => {
                let range = series.range(None, series.history.len().checked_sub(1));
                self.trace = Some(TraceRange {
                    history,
                    kind: series.kind,
                    range,
                });
                Some(range)
            }
        }
    }

    /// Move the displayed range towards the target over `dt` seconds;
    /// returns whether it is still moving.
    pub fn ease(&mut self, dt: f64) -> bool {
        let (Some(shown), Some(target)) = (self.shown, self.target) else {
            return false;
        };
        let span = (target.1 - target.0).abs().max(f64::MIN_POSITIVE);
        let k = 1.0 - (-dt / RANGE_TAU_S).exp();
        let step = |from: f64, to: f64| {
            if (to - from).abs() <= span * 2e-3 {
                to
            } else {
                from + (to - from) * k
            }
        };
        let next = (step(shown.0, target.0), step(shown.1, target.1));
        self.shown = Some(next);
        next != target
    }

    pub fn is_settled(&self) -> bool {
        self.shown == self.target
    }
}

/// Whether a row of `shape` shown with `tr` can be drawn as a plot.
pub fn supports(shape: SignalShape, tr: &dyn Translator) -> bool {
    matches!(shape, SignalShape::Vector { .. } | SignalShape::Real) && tr.numeric_kind().is_some()
}

/// Identity of a resident history, for matching derived data to it.
pub fn history_identity(h: &Arc<dyn SignalHistory>) -> usize {
    Arc::as_ptr(h) as *const () as usize
}

/// A flat range gets room around it: ±0.5 for integers, ±5% for floats.
fn padded((lo, hi): (f64, f64), shape: SignalShape) -> (f64, f64) {
    if hi - lo > 1e-12 * lo.abs().max(hi.abs()).max(1.0) {
        return (lo, hi);
    }
    let d = if shape == SignalShape::Real {
        (lo.abs() * 0.05).max(0.05)
    } else {
        0.5
    };
    (lo - d, hi + d)
}

/// One value of a history as the painter sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sample {
    /// Nothing was recorded (before the first sample of some sources).
    Missing,
    /// X, Z or a non-finite value: drawn as an undefined span.
    Undefined,
    Value(f64),
}

pub fn sample(h: &dyn SignalHistory, kind: NumericKind, i: Option<usize>) -> Sample {
    let view = h.value_view(i);
    if matches!(view, ValueView::Unavailable) {
        return Sample::Missing;
    }
    kind.read(&view).map_or(Sample::Undefined, Sample::Value)
}

/// Minimum, maximum and undefined values of a run of changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub lo: f64,
    pub hi: f64,
    pub undefined: bool,
}

impl Extent {
    pub const EMPTY: Self = Self {
        lo: f64::INFINITY,
        hi: f64::NEG_INFINITY,
        undefined: false,
    };

    fn add(&mut self, s: Sample) {
        match s {
            Sample::Value(v) => {
                self.lo = self.lo.min(v);
                self.hi = self.hi.max(v);
            }
            Sample::Undefined => self.undefined = true,
            Sample::Missing => {}
        }
    }

    fn merge(&mut self, other: &Self) {
        self.lo = self.lo.min(other.lo);
        self.hi = self.hi.max(other.hi);
        self.undefined |= other.undefined;
    }

    pub fn range(&self) -> Option<(f64, f64)> {
        (self.lo <= self.hi).then_some((self.lo, self.hi))
    }
}

/// Minimum and maximum per block of [`SUMMARY_BLOCK`] changes of one history
/// read one way, with a binary tree of blocks above them. Built once on the
/// load worker when a long history first turns analog; the document shares
/// it between rows and releases it (and its memory reservation) with the
/// history.
pub struct AnalogSummary {
    kind: NumericKind,
    history: usize,
    len: usize,
    /// `levels[0]` holds one extent per block; each level above halves it.
    levels: Vec<Vec<Extent>>,
    reservation: Option<crate::remote::memory::Reservation>,
}

impl std::fmt::Debug for AnalogSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalogSummary")
            .field("kind", &self.kind)
            .field("len", &self.len)
            .field("bytes", &self.resident_bytes())
            .finish()
    }
}

impl AnalogSummary {
    pub fn build(h: &Arc<dyn SignalHistory>, kind: NumericKind) -> Self {
        let len = h.len();
        let mut leaves = vec![Extent::EMPTY; len.div_ceil(SUMMARY_BLOCK)];
        for (i, leaf) in (0..len).zip((0..len).map(|i| i / SUMMARY_BLOCK)) {
            leaves[leaf].add(sample(h.as_ref(), kind, Some(i)));
        }
        let mut levels = vec![leaves];
        while levels.last().is_some_and(|l| l.len() > 1) {
            let below = levels.last().unwrap();
            let above = below
                .chunks(2)
                .map(|pair| {
                    let mut e = pair[0];
                    if let Some(b) = pair.get(1) {
                        e.merge(b);
                    }
                    e
                })
                .collect();
            levels.push(above);
        }
        Self {
            kind,
            history: history_identity(h),
            len,
            levels,
            reservation: None,
        }
    }

    /// Charge the summary to a memory budget for as long as it lives.
    pub fn account(mut self, budget: &crate::remote::memory::MemoryBudget) -> anyhow::Result<Self> {
        self.reservation =
            Some(budget.reserve_object("the analog summary", self.resident_bytes())?);
        Ok(self)
    }

    pub fn resident_bytes(&self) -> u64 {
        let nodes: usize = self.levels.iter().map(Vec::len).sum();
        (nodes * std::mem::size_of::<Extent>()) as u64
    }

    pub fn kind(&self) -> NumericKind {
        self.kind
    }

    /// Identity of the summarized history.
    pub fn identity(&self) -> usize {
        self.history
    }

    /// Whether this summarizes `h` read as `kind`.
    pub fn matches(&self, h: &Arc<dyn SignalHistory>, kind: NumericKind) -> bool {
        self.kind == kind && self.history == history_identity(h) && self.len == h.len()
    }

    fn root(&self) -> Extent {
        self.levels
            .last()
            .and_then(|l| l.first())
            .copied()
            .unwrap_or(Extent::EMPTY)
    }

    /// The extent of changes `a..=b`: full blocks from the tree, the partial
    /// blocks at either end read from the history.
    pub fn query(&self, h: &dyn SignalHistory, a: usize, b: usize) -> Extent {
        let mut e = Extent::EMPTY;
        let b = b.min(self.len.saturating_sub(1));
        if a > b {
            return e;
        }
        let (first, end) = (a.div_ceil(SUMMARY_BLOCK), (b + 1) / SUMMARY_BLOCK);
        if first >= end {
            for i in a..=b {
                e.add(sample(h, self.kind, Some(i)));
            }
            return e;
        }
        for i in (a..first * SUMMARY_BLOCK).chain(end * SUMMARY_BLOCK..=b) {
            e.add(sample(h, self.kind, Some(i)));
        }
        let (mut l, mut r) = (first, end);
        for level in &self.levels {
            if l >= r {
                break;
            }
            if l & 1 == 1 {
                e.merge(&level[l]);
                l += 1;
            }
            if r & 1 == 1 {
                r -= 1;
                e.merge(&level[r]);
            }
            l >>= 1;
            r >>= 1;
        }
        e
    }
}

/// A history read as numbers, with its summary when one is resident.
#[derive(Clone, Copy)]
pub struct Series<'a> {
    pub history: &'a dyn SignalHistory,
    pub kind: NumericKind,
    pub summary: Option<&'a AnalogSummary>,
    /// A summary for this long history is still building.
    pub building: bool,
    identity: usize,
}

impl<'a> Series<'a> {
    /// `summary` is used only when it summarizes this history and reading.
    pub fn new(
        history: &'a Arc<dyn SignalHistory>,
        kind: NumericKind,
        summary: Option<&'a AnalogSummary>,
    ) -> Self {
        Self {
            history: history.as_ref(),
            kind,
            summary: summary.filter(|s| s.matches(history, kind)),
            building: false,
            identity: history_identity(history),
        }
    }

    /// The series a plot row draws: `history` read as `kind`, with the
    /// document's summary of `signal` when it matches.
    pub fn of(
        doc: &'a crate::document::Document,
        signal: Option<crate::data::SignalRef>,
        history: &'a Arc<dyn SignalHistory>,
        kind: NumericKind,
    ) -> Self {
        use crate::document::SummaryLoad;
        let load = signal.and_then(|s| doc.analog_summary(s, kind));
        let summary = match load {
            Some(SummaryLoad::Ready(s)) => Some(s.as_ref()),
            _ => None,
        };
        let mut series = Self::new(history, kind, summary);
        series.building = matches!(load, Some(SummaryLoad::Building { .. }));
        series
    }

    /// Whether reading `changes` changes should wait for the summary rather
    /// than scan them on the UI thread.
    pub fn waiting(&self, changes: usize) -> bool {
        self.building && self.summary.is_none() && changes >= SUMMARY_MIN_CHANGES
    }

    pub fn sample(&self, i: Option<usize>) -> Sample {
        sample(self.history, self.kind, i)
    }

    /// The extent of the value at `from` (the value before the first change
    /// when `None`) through change `to`.
    pub fn extent(&self, from: Option<usize>, to: Option<usize>) -> Extent {
        let mut e = Extent::EMPTY;
        let Some(to) = to else {
            e.add(self.sample(None));
            return e;
        };
        if from.is_none() {
            e.add(self.sample(None));
        }
        let a = from.unwrap_or(0);
        match self.summary {
            Some(s) if to - a >= 2 * SUMMARY_BLOCK => e.merge(&s.query(self.history, a, to)),
            _ => {
                for i in a..=to.min(self.history.len().saturating_sub(1)) {
                    e.add(self.sample(Some(i)));
                }
            }
        }
        e
    }

    pub fn range(&self, from: Option<usize>, to: Option<usize>) -> Option<(f64, f64)> {
        self.extent(from, to).range()
    }

    pub fn window_range(&self, vp: &Viewport) -> Option<(f64, f64)> {
        self.range(
            index_at(self.history, vp.start),
            index_at(self.history, vp.end),
        )
    }
}

/// The last change at or before `t`, `None` before the first (or before 0).
fn index_at(h: &dyn SignalHistory, t: f64) -> Option<usize> {
    if t < 0.0 {
        return None;
    }
    h.index_at(t.floor() as u64)
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

/// Changes inside the visible window.
pub fn visible_changes(h: &dyn SignalHistory, vp: &Viewport) -> usize {
    changes_between(index_at(h, vp.start), index_at(h, vp.end))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawMode {
    /// Every change at its own time.
    Exact,
    /// One min/max column per pixel.
    Envelope,
}

pub fn draw_mode(h: &dyn SignalHistory, vp: &Viewport, width_px: f32) -> DrawMode {
    if visible_changes(h, vp) as f64 <= f64::from(width_px) * EXACT_RATIO {
        DrawMode::Exact
    } else {
        DrawMode::Envelope
    }
}

/// Where a plot is drawn and which values map to its top and bottom.
#[derive(Clone, Copy, Debug)]
pub struct Plot {
    pub left: f32,
    pub width: f32,
    pub top: f32,
    pub bottom: f32,
    pub lo: f64,
    pub hi: f64,
}

impl Plot {
    pub fn y_of(&self, v: f64) -> f32 {
        let f = (v - self.lo) / (self.hi - self.lo);
        self.bottom - (f as f32) * (self.bottom - self.top)
    }

    fn x_of(&self, vp: &Viewport, t: f64) -> f32 {
        self.left + vp.x_of(t, f64::from(self.width)) as f32
    }

    /// Where fills start: the zero line when it is inside the range.
    pub fn baseline(&self) -> f32 {
        if self.lo <= 0.0 && self.hi >= 0.0 {
            self.y_of(0.0)
        } else {
            self.bottom
        }
    }
}

/// The drawable shape of a plot for one frame.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub envelope: bool,
    /// Too many changes to draw before the summary is ready.
    pub waiting: bool,
    /// Polylines, broken at undefined and missing values.
    pub runs: Vec<Vec<Point>>,
    /// Horizontal spans of undefined values.
    pub undefined: Vec<(f32, f32)>,
    /// Sample marks of linear plots zoomed in.
    pub dots: Vec<Point>,
}

impl Geometry {
    fn undefined(&mut self, a: f32, b: f32) {
        match self.undefined.last_mut() {
            Some(last) if a <= last.1 + 0.5 => last.1 = last.1.max(b),
            _ => self.undefined.push((a, b)),
        }
    }
}

/// Build the geometry of `series` in `plot` for viewport `vp`.
pub fn geometry(
    series: &Series<'_>,
    draw: AnalogDraw,
    vp: &Viewport,
    plot: &Plot,
    zoom: f32,
) -> Geometry {
    let mut g = Geometry::default();
    if plot.width <= 0.0 || vp.width() <= 0.0 || plot.hi <= plot.lo {
        return g;
    }
    match draw_mode(series.history, vp, plot.width) {
        DrawMode::Exact => exact(series, draw, vp, plot, zoom, &mut g),
        DrawMode::Envelope if series.waiting(visible_changes(series.history, vp)) => {
            g.envelope = true;
            g.waiting = true;
        }
        DrawMode::Envelope => envelope(series, vp, plot, &mut g),
    }
    g
}

fn exact(
    series: &Series<'_>,
    draw: AnalogDraw,
    vp: &Viewport,
    plot: &Plot,
    zoom: f32,
    g: &mut Geometry,
) {
    let h = series.history;
    let len = h.len();
    let first = index_at(h, vp.start);
    let right = plot.left + plot.width + 1.0;
    let last = match index_at(h, vp.end) {
        Some(i) => (i + 1).min(len - 1),
        None if len > 0 => 0,
        None => {
            let (a, b) = (plot.left - 1.0, right);
            match series.sample(None) {
                Sample::Value(v) => g
                    .runs
                    .push(vec![point(a, plot.y_of(v)), point(b, plot.y_of(v))]),
                Sample::Undefined => g.undefined(a, b),
                Sample::Missing => {}
            }
            return;
        }
    };
    let spacing = plot.width / (visible_changes(h, vp).max(1) as f32);
    let mark = draw == AnalogDraw::Linear && spacing >= DOT_SPACING * zoom;
    let indices = std::iter::once(first)
        .filter(|f| f.is_none())
        .chain((first.unwrap_or(0)..=last).map(Some));
    let mut run: Option<Vec<Point>> = None;
    for i in indices {
        let xa = match i {
            None => plot.left - 1.0,
            Some(i) => plot.x_of(vp, h.time(i) as f64).max(plot.left - 1.0),
        };
        let next = i.map_or(0, |i| i + 1);
        let xb = if next < len {
            plot.x_of(vp, h.time(next) as f64).min(right)
        } else {
            right
        };
        match series.sample(i) {
            Sample::Missing => g.runs.extend(run.take()),
            Sample::Undefined => {
                g.runs.extend(run.take());
                g.undefined(xa, xb);
            }
            Sample::Value(v) => {
                let y = plot.y_of(v);
                let points = run.get_or_insert_with(Vec::new);
                match draw {
                    AnalogDraw::Step => points.extend([point(xa, y), point(xb, y)]),
                    AnalogDraw::Linear => {
                        points.push(point(xa, y));
                        if mark && i.is_some() {
                            g.dots.push(point(xa, y));
                        }
                        // Hold flat before an undefined value and at the end.
                        let hold =
                            next >= len || !matches!(series.sample(Some(next)), Sample::Value(_));
                        if hold {
                            points.push(point(xb, y));
                        }
                    }
                }
            }
        }
        if xb >= right {
            break;
        }
    }
    g.runs.extend(run);
}

fn envelope(series: &Series<'_>, vp: &Viewport, plot: &Plot, g: &mut Geometry) {
    let h = series.history;
    let n = plot.width.floor().max(1.0) as usize;
    let w = f64::from(plot.width);
    let mut j = index_at(h, vp.start);
    let mut entry = series.sample(j);
    let mut run: Option<Vec<Point>> = None;
    for x in 0..n {
        let end = index_before(h, vp.time_at((x + 1) as f64, w), j).max(j);
        let changes = changes_between(j, end);
        let mut e = Extent::EMPTY;
        e.add(entry);
        let exit = if changes > 0 {
            e.merge(&series.extent(Some(j.map_or(0, |j| j + 1)), end));
            series.sample(end)
        } else {
            entry
        };
        j = end;
        let xc = plot.left + x as f32 + 0.5;
        if e.undefined {
            g.undefined(plot.left + x as f32, plot.left + x as f32 + 1.0);
        }
        match e.range() {
            None => g.runs.extend(run.take()),
            Some((lo, hi)) => {
                if !matches!(entry, Sample::Value(_)) {
                    g.runs.extend(run.take());
                }
                let points = run.get_or_insert_with(Vec::new);
                if let Sample::Value(v) = entry {
                    points.push(point(xc, plot.y_of(v)));
                }
                if changes > 0 {
                    points.extend([point(xc, plot.y_of(lo)), point(xc, plot.y_of(hi))]);
                }
                match exit {
                    Sample::Value(v) => points.push(point(xc, plot.y_of(v))),
                    _ => g.runs.extend(run.take()),
                }
            }
        }
        entry = exit;
    }
    g.envelope = true;
    g.runs.extend(run);
}

/// The curve's y at time `t`: the held value, or the line between two
/// samples of a linear plot drawn exactly.
pub fn y_at(
    series: &Series<'_>,
    draw: AnalogDraw,
    envelope: bool,
    plot: &Plot,
    t: f64,
) -> Option<f32> {
    let h = series.history;
    let i = index_at(h, t);
    let Sample::Value(mut v) = series.sample(i) else {
        return None;
    };
    let next = i.map_or(0, |i| i + 1);
    if draw == AnalogDraw::Linear
        && !envelope
        && i.is_some()
        && next < h.len()
        && let Sample::Value(v1) = series.sample(Some(next))
    {
        let (t0, t1) = (h.time(next - 1) as f64, h.time(next) as f64);
        if t1 > t0 {
            v += (v1 - v) * ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
        }
    }
    Some(plot.y_of(v))
}

/// What the pointer reads on a plot.
#[derive(Clone, Debug, PartialEq)]
pub enum Readout {
    /// One sample (linear: the nearest; step: the one held at the pointer).
    Sample {
        index: Option<usize>,
        time: f64,
        at: Point,
    },
    /// A dense pixel column: its range and number of changes.
    Column {
        lo: f64,
        hi: f64,
        changes: usize,
        x: f32,
    },
}

pub fn readout(
    series: &Series<'_>,
    draw: AnalogDraw,
    vp: &Viewport,
    plot: &Plot,
    x: f32,
) -> Option<Readout> {
    let h = series.history;
    let w = f64::from(plot.width);
    let dx = f64::from(x - plot.left);
    if dx < 0.0 || dx > w {
        return None;
    }
    if draw_mode(h, vp, plot.width) == DrawMode::Envelope {
        if series.waiting(visible_changes(h, vp)) {
            return None;
        }
        let col = dx.floor();
        let ia = index_at(h, vp.time_at(col, w));
        let ib = index_before(h, vp.time_at(col + 1.0, w), ia).max(ia);
        let (lo, hi) = series.range(ia, ib)?;
        return Some(Readout::Column {
            lo,
            hi,
            changes: changes_between(ia, ib),
            x: plot.left + col as f32 + 0.5,
        });
    }
    let t = vp.time_at(dx, w);
    let mut i = index_at(h, t);
    if draw == AnalogDraw::Linear {
        let next = i.map_or(0, |i| i + 1);
        let here = i.map(|i| (plot.x_of(vp, h.time(i) as f64) - x).abs());
        if next < h.len() {
            let there = (plot.x_of(vp, h.time(next) as f64) - x).abs();
            if here.is_none_or(|here| there < here) {
                i = Some(next);
            }
        }
    }
    let Sample::Value(v) = series.sample(i) else {
        return None;
    };
    let (time, px) = match (draw, i) {
        (AnalogDraw::Linear, Some(i)) => (h.time(i) as f64, plot.x_of(vp, h.time(i) as f64)),
        _ => (t, x),
    };
    Some(Readout::Sample {
        index: i,
        time,
        at: point(px, plot.y_of(v)),
    })
}

/// Pixel columns under the polylines, merged where their height agrees:
/// `(x, width, y)` quads that fill between the curve and the baseline. A
/// column takes the curve's height at its centre, or the last point of a
/// vertical (dense) stroke inside it.
pub fn fill_columns(runs: &[Vec<Point>], left: f32, width: f32) -> Vec<(f32, f32, f32)> {
    let n = width.max(0.0).ceil() as usize;
    let mut ys: Vec<Option<f32>> = vec![None; n];
    let col = |x: f32| {
        let c = (x - left).floor();
        (c >= 0.0 && (c as usize) < n).then_some(c as usize)
    };
    for run in runs {
        if let [only] = run.as_slice()
            && let Some(c) = col(only.x)
        {
            ys[c] = Some(only.y);
        }
        for pair in run.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b.x <= a.x {
                if let Some(c) = col(b.x) {
                    ys[c] = Some(b.y);
                }
                continue;
            }
            let c0 = (a.x - left - 0.5).ceil().max(0.0) as usize;
            let c1 = ((b.x - left - 0.5).floor() + 1.0).clamp(0.0, n as f32) as usize;
            for (c, y) in ys.iter_mut().enumerate().take(c1).skip(c0) {
                let xc = left + c as f32 + 0.5;
                *y = Some(a.y + (b.y - a.y) * (xc - a.x) / (b.x - a.x));
            }
        }
    }
    let mut out: Vec<(f32, f32, f32)> = Vec::new();
    for (c, y) in ys.into_iter().enumerate() {
        let Some(y) = y else { continue };
        let x = left + c as f32;
        match out.last_mut() {
            Some((x0, w, y0)) if (*x0 + *w - x).abs() < 1e-3 && (*y0 - y).abs() < 0.5 => *w += 1.0,
            _ => out.push((x, 1.0, y)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::WaveValue;
    use crate::data::history::VecHistory;

    fn history(values: &[Option<i64>]) -> Arc<dyn SignalHistory> {
        Arc::new(VecHistory {
            shape: SignalShape::Vector { width: 16 },
            times: (0..values.len() as u64).collect(),
            values: values
                .iter()
                .map(|v| match v {
                    Some(v) => WaveValue::Bits(format!("{:016b}", *v as u16)),
                    None => WaveValue::Bits("x".repeat(16)),
                })
                .collect(),
            initial: WaveValue::Unavailable,
        })
    }

    #[test]
    fn summary_queries_match_direct_scans_at_every_boundary() {
        let values: Vec<Option<i64>> = (0..1000i64)
            .map(|i| (i % 97 != 13).then_some((i * 7919) % 2001 - 1000))
            .collect();
        let h = history(&values);
        let s = AnalogSummary::build(&h, NumericKind::Signed);
        let direct = |a: usize, b: usize| {
            let mut e = Extent::EMPTY;
            for i in a..=b {
                e.add(sample(h.as_ref(), NumericKind::Signed, Some(i)));
            }
            e
        };
        for a in [0, 1, 63, 64, 65, 127, 128, 500, 999] {
            for b in [a, a + 1, 63, 64, 128, 200, 511, 512, 998, 999] {
                if b >= a && b < 1000 {
                    assert_eq!(s.query(h.as_ref(), a, b), direct(a, b), "{a}..={b}");
                }
            }
        }
        assert_eq!(s.root(), direct(0, 999));
        assert!(s.root().undefined);
        // 16 blocks, then 8, 4, 2 and the root.
        assert_eq!(
            s.resident_bytes(),
            31 * std::mem::size_of::<Extent>() as u64
        );
    }
}
