//! Declared clocks (`docs/vtr_clocks.html`): the catalog of a trace's `CLOCK`
//! streams, their timelines built from loaded stretches, each panel's clock
//! choices, and the ruler and readout arithmetic every timed panel
//! shares.
//!
//! A clock is an ordinary transaction stream, so its stretches load through
//! the same track requests as any other stream, locally and remotely. The
//! file stores only stretches; cycle numbers come from [`ClockTimeline`].

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
pub use vtr::{ClockTimeline, CycleAt};

use crate::data::loaded_tracks::LoadedGenerator;
use crate::data::transactions::{AttributeValue, Track, TrackKind, TrackRef, TxStatus};
use crate::wave::timeline::TimeBase;
use crate::wave::viewport::Viewport;

/// Stream kind of a clock.
pub const STREAM_KIND: &str = vtr::CLOCK_STREAM_KIND;
/// Stream attribute naming, by path, the clock its stages are counted in.
pub const LINK_ATTRIBUTE: &str = vtr::clock::KEY_CLOCK;
/// Stretch attribute: the spacing of its edges.
pub const PERIOD_ATTRIBUTE: &str = vtr::clock::KEY_PERIOD;

/// Whether a clock's stretches are resident.
#[derive(Clone, Debug)]
pub enum ClockState {
    Loading,
    Ready(Arc<ClockTimeline>),
    Failed(String),
}

/// One declared clock of the open trace.
#[derive(Clone, Debug)]
pub struct Clock {
    /// Its stream, retained by the document for as long as the trace is open.
    pub track: TrackRef,
    /// The stream's path joined with '.': the clock's identity in `vtr.clock`
    /// links and in workspaces.
    pub path: String,
    pub name: String,
    pub state: ClockState,
}

impl Clock {
    pub fn timeline(&self) -> Option<&Arc<ClockTimeline>> {
        match &self.state {
            ClockState::Ready(t) => Some(t),
            _ => None,
        }
    }
}

/// Every clock of the open trace and which stream counts in which clock.
#[derive(Clone, Debug, Default)]
pub struct Clocks {
    clocks: Vec<Clock>,
    /// Stream or generator track -> index of the clock its stream names.
    links: HashMap<TrackRef, usize>,
    /// Paths of the clocks the workspace's pipelines count in: the rulers a
    /// panel shows until it chooses its own. The app refreshes it each frame.
    pub defaults: Vec<String>,
}

impl Clocks {
    /// The clocks declared in a track catalog, in catalog order.
    pub fn from_tracks(tracks: &[Track]) -> Self {
        let clocks: Vec<Clock> = tracks
            .iter()
            .filter(|t| matches!(&t.kind, TrackKind::Stream { kind } if kind == STREAM_KIND))
            .map(|t| Clock {
                track: t.id,
                path: t.path.join("."),
                name: t.path.last().cloned().unwrap_or_default(),
                state: ClockState::Loading,
            })
            .collect();
        let mut links = HashMap::new();
        let mut stream_clock = HashMap::new();
        for t in tracks {
            if let TrackKind::Stream { .. } = t.kind
                && let Some(path) = t.attributes.iter().find_map(|(k, v)| match v {
                    AttributeValue::Text(p) if k == LINK_ATTRIBUTE => Some(p),
                    _ => None,
                })
                && let Some(ix) = clocks.iter().position(|c| &c.path == path)
            {
                links.insert(t.id, ix);
                stream_clock.insert(t.id, ix);
            }
        }
        for t in tracks {
            if let TrackKind::Generator { stream } = t.kind
                && let Some(&ix) = stream_clock.get(&stream)
            {
                links.insert(t.id, ix);
            }
        }
        Self {
            clocks,
            links,
            defaults: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.clocks.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Clock> {
        self.clocks.iter()
    }

    pub fn get(&self, ix: usize) -> Option<&Clock> {
        self.clocks.get(ix)
    }

    /// The clock with this path.
    pub fn find(&self, path: &str) -> Option<&Clock> {
        self.clocks.iter().find(|c| c.path == path)
    }

    /// The clock of a clock stream or of its generator.
    pub fn of_track(&self, track: TrackRef, tracks: &[Track]) -> Option<&Clock> {
        let stream = match tracks.iter().find(|t| t.id == track)?.kind {
            TrackKind::Generator { stream } => stream,
            TrackKind::Stream { .. } => track,
        };
        self.clocks.iter().find(|c| c.track == stream)
    }

    /// The clock a stream (or generator of a stream) counts in, by its `vtr.clock`.
    pub fn linked(&self, track: TrackRef) -> Option<&Clock> {
        self.links.get(&track).map(|&ix| &self.clocks[ix])
    }

    pub fn is_clock_track(&self, track: TrackRef) -> bool {
        self.clocks.iter().any(|c| c.track == track)
    }

    /// Stretches of `track` arrived or failed.
    pub(crate) fn deliver(
        &mut self,
        track: TrackRef,
        result: Result<&[Arc<LoadedGenerator>], &str>,
    ) {
        let Some(clock) = self.clocks.iter_mut().find(|c| c.track == track) else {
            return;
        };
        clock.state = match result
            .map_err(str::to_owned)
            .and_then(|g| timeline_of(g).map_err(|e| format!("{e:#}")))
        {
            Ok(t) => ClockState::Ready(Arc::new(t)),
            Err(e) => ClockState::Failed(e),
        };
    }
}

/// The timeline of a clock stream's loaded stretches.
pub fn timeline_of(generators: &[Arc<LoadedGenerator>]) -> anyhow::Result<ClockTimeline> {
    let mut raw = Vec::new();
    let mut open = false;
    for tx in generators.iter().flat_map(|g| g.transactions()) {
        let period = tx
            .attributes
            .iter()
            .find(|a| a.key == PERIOD_ATTRIBUTE)
            .map_or(Ok(0), |a| match a.value {
                AttributeValue::Time(p) | AttributeValue::U64(p) => Ok(p),
                _ => Err(anyhow::anyhow!("{PERIOD_ATTRIBUTE} is not a time")),
            })?;
        open |= tx.status == TxStatus::Open;
        raw.push((tx.begin, tx.end, period));
    }
    Ok(ClockTimeline::new(raw, open)?)
}

// -- a panel's clock choices ------------------------------------------------------

/// What a timed panel shows of the clocks. Saved in the workspace, never in
/// the trace; clocks are named by path so a workspace survives a new run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockView {
    /// Ruler rows under the time ruler, top to bottom. `None` shows the
    /// clocks the workspace's pipelines count in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rulers: Option<Vec<String>>,
    /// The clock clicks snap to, `[` / `]` step through and go-to counts in;
    /// defaults to the first ruler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// Time whose cycle every clock of the panel numbers 0; `None` numbers
    /// from each clock's first recorded edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<u64>,
}

impl ClockView {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Paths of the ruler rows, whether or not the clocks exist in this trace.
    pub fn ruler_paths<'a>(&'a self, clocks: &'a Clocks) -> &'a [String] {
        self.rulers.as_deref().unwrap_or(&clocks.defaults)
    }

    /// The ruler rows of clocks this trace declares, top to bottom.
    pub fn rulers<'a>(&self, clocks: &'a Clocks) -> Vec<&'a Clock> {
        self.ruler_paths(clocks)
            .iter()
            .filter_map(|p| clocks.find(p))
            .collect()
    }

    /// The clock clicks snap to and cycle steps follow.
    pub fn selected<'a>(&self, clocks: &'a Clocks) -> Option<&'a Clock> {
        self.selected
            .as_deref()
            .and_then(|p| clocks.find(p))
            .or_else(|| self.rulers(clocks).into_iter().next())
    }

    /// Show a ruler row (starting from the default set) unless it is shown.
    pub fn show_ruler(&mut self, clocks: &Clocks, path: &str) {
        let mut rulers = self.ruler_paths(clocks).to_vec();
        if !rulers.iter().any(|p| p == path) {
            rulers.push(path.to_owned());
        }
        self.rulers = Some(rulers);
    }

    /// Hide a ruler row (starting from the default set) if it is shown.
    pub fn hide_ruler(&mut self, clocks: &Clocks, path: &str) {
        if self.ruler_paths(clocks).iter().any(|p| p == path) {
            self.toggle_ruler(clocks, path);
        }
    }

    /// Show or hide a ruler row, starting from the default set.
    pub fn toggle_ruler(&mut self, clocks: &Clocks, path: &str) {
        let mut rulers = self.ruler_paths(clocks).to_vec();
        match rulers.iter().position(|p| p == path) {
            Some(ix) => {
                rulers.remove(ix);
                if self.selected.as_deref() == Some(path) {
                    self.selected = None;
                }
            }
            None => rulers.push(path.to_owned()),
        }
        self.rulers = Some(rulers);
    }

    /// The cycle a clock numbers 0 in this panel.
    pub fn origin_cycle(&self, timeline: &ClockTimeline) -> u64 {
        self.origin
            .and_then(|t| timeline.cycle_at(t))
            .map_or(0, |c| c.cycle)
    }

    /// A clock's cycle as this panel numbers it.
    pub fn display_cycle(&self, timeline: &ClockTimeline, cycle: u64) -> i64 {
        cycle as i64 - self.origin_cycle(timeline) as i64
    }

    /// The absolute cycle of a displayed cycle number.
    pub fn absolute_cycle(&self, timeline: &ClockTimeline, shown: i64) -> Option<u64> {
        u64::try_from(shown + self.origin_cycle(timeline) as i64).ok()
    }
}

/// The edge of `timeline` nearest to `t` within `tolerance` time units.
pub fn nearest_edge(timeline: &ClockTimeline, t: f64, tolerance: f64) -> Option<u64> {
    let at = t.max(0.0).floor() as u64;
    let before = timeline.cycle_at(at).map(|c| c.edge);
    let after = timeline.next_edge(at);
    [before, after]
        .into_iter()
        .flatten()
        .map(|e| ((e as f64 - t).abs(), e))
        .filter(|(d, _)| *d <= tolerance)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

// -- a clock drawn as a waveform --------------------------------------------------

/// A clock's stretches as a one-bit waveform, computed on demand: it rises
/// at every recorded edge and falls half a period later (the file records no
/// falling edges, so the drawn duty cycle is 50%). Nothing is materialized,
/// so a clock of a billion edges costs its stretches only.
pub struct ClockHistory {
    timeline: Arc<ClockTimeline>,
}

impl ClockHistory {
    pub fn new(timeline: Arc<ClockTimeline>) -> Self {
        Self { timeline }
    }

    /// Period of the stretch holding cycle `cycle`.
    fn period(&self, cycle: u64) -> u64 {
        let s = self.timeline.stretches();
        let i = s
            .partition_point(|s| s.first_cycle <= cycle)
            .saturating_sub(1);
        s.get(i).map_or(0, |s| s.period)
    }
}

impl crate::data::SignalHistory for ClockHistory {
    fn shape(&self) -> crate::data::SignalShape {
        crate::data::SignalShape::Bit
    }

    fn len(&self) -> usize {
        (self.timeline.edge_count() * 2) as usize
    }

    fn time(&self, i: usize) -> u64 {
        let cycle = (i / 2) as u64;
        let edge = self.timeline.edge(cycle).unwrap_or(0);
        if i.is_multiple_of(2) {
            return edge;
        }
        let fall = edge + (self.period(cycle) / 2).max(1);
        self.timeline
            .edge(cycle + 1)
            .map_or(fall, |next| fall.min(next))
    }

    fn value(&self, i: Option<usize>) -> crate::data::WaveValue {
        match i {
            None => crate::data::WaveValue::Unavailable,
            Some(i) => crate::data::WaveValue::Bits(if i % 2 == 0 { "1" } else { "0" }.into()),
        }
    }

    fn bit(&self, i: Option<usize>) -> crate::data::Bit {
        match i {
            None => crate::data::Bit::Unavailable,
            Some(i) if i % 2 == 0 => crate::data::Bit::One,
            Some(_) => crate::data::Bit::Zero,
        }
    }
}

// -- readouts ----------------------------------------------------------------------

/// `150231 + 0.42`: a cycle number and the position in it, as the cursor
/// chip and the status bar show it. A stopped cycle says so.
pub fn format_position(view: &ClockView, timeline: &ClockTimeline, at: &CycleAt) -> String {
    let cycle = view.display_cycle(timeline, at.cycle);
    if at.stopped {
        format!("{cycle} (stopped)")
    } else if at.fraction > 0.0 {
        format!("{cycle} + {:.2}", at.fraction)
    } else {
        cycle.to_string()
    }
}

/// The cursor's position in `clock`, or a dash before its first edge.
pub fn position_at(view: &ClockView, clock: &Clock, time: u64) -> String {
    match clock.timeline().and_then(|t| Some((t, t.cycle_at(time)?))) {
        Some((t, at)) => format!("{} {}", clock.name, format_position(view, t, &at)),
        None => format!("{} –", clock.name),
    }
}

/// Whole cycles of `clock` between two times (signed, `b - a`).
pub fn cycles_between(timeline: &ClockTimeline, a: u64, b: u64) -> Option<i64> {
    let at = |t: u64| timeline.cycle_at(t).map(|c| c.cycle as i64);
    Some(at(b)? - at(a)?)
}

/// `→ 2.00 GHz` for a stretch of `period` file units; a trace with a named
/// time unit (Kanata cycles) states the period instead.
pub fn speed_label(period: u64, base: TimeBase<'_>) -> String {
    if let Some(unit) = base.unit {
        return format!("→ {period} {unit}");
    }
    let hz = 1.0 / (period as f64 * 10f64.powi(base.timescale as i32));
    let (value, suffix) = [(1e9, "GHz"), (1e6, "MHz"), (1e3, "kHz"), (1.0, "Hz")]
        .into_iter()
        .find(|(scale, _)| hz * (1.0 + 1e-9) >= *scale)
        .map_or((hz, "Hz"), |(scale, s)| (hz / scale, s));
    format!("→ {value:.2} {suffix}")
}

// -- ruler marks -------------------------------------------------------------------

/// One rising edge on a ruler; labelled edges carry their displayed cycle.
#[derive(Clone, Debug, PartialEq)]
pub struct RulerTick {
    pub time: u64,
    pub label: Option<String>,
}

/// What one clock ruler draws in the visible window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RulerMarks {
    pub ticks: Vec<RulerTick>,
    /// Stretch starts after a change of speed, with their new speed.
    pub flags: Vec<(u64, String)>,
    /// Intervals in which the clock is stopped, including after its end.
    pub stopped: Vec<(f64, f64)>,
}

/// Edges closer than this many pixels are drawn only where labelled.
pub const MIN_TICK_PX: f64 = 4.0;
/// Most ticks one ruler draws in a frame.
const MAX_TICKS: usize = 4000;
const NICE: [u64; 3] = [1, 2, 5];

/// The smallest step of the 1-2-5 series whose `cycle_px` multiple is at
/// least `min_px` wide.
fn label_step(cycle_px: f64, min_px: f64) -> u64 {
    let mut magnitude = 1u64;
    loop {
        for n in NICE {
            let step = n * magnitude;
            if step as f64 * cycle_px >= min_px || magnitude > u64::MAX / 100 {
                return step;
            }
        }
        magnitude *= 10;
    }
}

/// Ticks, flags and stopped intervals of a clock in `viewport`, labelling
/// every `step`th displayed cycle, where each stretch picks its own step so
/// labels stay at least `min_label_px` apart. Edges closer than
/// [`MIN_TICK_PX`] are ticked only where labelled.
pub fn ruler_marks(
    view: &ClockView,
    timeline: &ClockTimeline,
    viewport: &Viewport,
    width_px: f64,
    min_label_px: f64,
    base: TimeBase<'_>,
) -> RulerMarks {
    let mut marks = RulerMarks::default();
    let stretches = timeline.stretches();
    if stretches.is_empty() || width_px <= 0.0 {
        return marks;
    }
    let ppu = viewport.px_per_unit(width_px);
    let origin = view.origin_cycle(timeline) as i64;
    let (lo, hi) = (viewport.start.max(0.0), viewport.end.max(0.0));
    for (i, s) in stretches.iter().enumerate() {
        let next_begin = stretches.get(i + 1).map(|n| n.begin);
        // Stopped: a long gap to the next stretch, or the end of a clock that stopped for good.
        let gap_end = match next_begin {
            Some(b) => {
                let at = timeline.cycle_at(s.end);
                at.filter(|a| a.stopped).map(|_| b as f64)
            }
            None => (!timeline.is_open()).then_some(f64::INFINITY),
        };
        if let Some(end) = gap_end
            && (s.end as f64) < hi
            && end > lo
        {
            marks.stopped.push((s.end as f64, end.min(viewport.end)));
        }
        if i > 0
            && s.period != 0
            && s.period != stretches[i - 1].period
            && (lo..=hi).contains(&(s.begin as f64))
        {
            marks.flags.push((s.begin, speed_label(s.period, base)));
        }
        if (s.end as f64) < lo || (s.begin as f64) > hi || marks.ticks.len() >= MAX_TICKS {
            continue;
        }
        let edges = s.edges();
        let cycle_px = if s.period == 0 {
            f64::INFINITY
        } else {
            s.period as f64 * ppu
        };
        let step = label_step(cycle_px, min_label_px);
        // Edge indices of this stretch inside the window.
        let k0 = if s.period == 0 || lo <= s.begin as f64 {
            0
        } else {
            ((lo - s.begin as f64) / s.period as f64).floor() as u64
        };
        let k1 = if s.period == 0 {
            0
        } else {
            (((hi - s.begin as f64) / s.period as f64).floor() as u64).min(edges - 1)
        };
        let shown = |k: u64| (s.first_cycle + k) as i64 - origin;
        let label = |k: u64| (shown(k).rem_euclid(step as i64) == 0).then(|| shown(k).to_string());
        if cycle_px >= MIN_TICK_PX {
            for k in k0..=k1 {
                if marks.ticks.len() >= MAX_TICKS {
                    break;
                }
                marks.ticks.push(RulerTick {
                    time: s.begin + k * s.period,
                    label: label(k),
                });
            }
        } else {
            // Only the labelled edges: the first multiple of `step` at or after k0.
            let first = shown(k0);
            let mut k = k0
                + (first.rem_euclid(step as i64) != 0) as u64
                    * (step as i64 - first.rem_euclid(step as i64)) as u64;
            while k <= k1 && marks.ticks.len() < MAX_TICKS {
                marks.ticks.push(RulerTick {
                    time: s.begin + k * s.period,
                    label: label(k),
                });
                k += step;
            }
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> ClockTimeline {
        // The page's core_clk: three stretches, 334, 500 and 1000 ps.
        ClockTimeline::new(
            vec![(400, 19772, 334), (20272, 49772, 500), (50772, 99772, 1000)],
            true,
        )
        .unwrap()
    }

    #[test]
    fn steps_follow_the_1_2_5_series() {
        assert_eq!(label_step(100.0, 64.0), 1);
        assert_eq!(label_step(40.0, 64.0), 2);
        assert_eq!(label_step(20.0, 64.0), 5);
        assert_eq!(label_step(1.0, 64.0), 100);
    }

    #[test]
    fn ruler_ticks_every_edge_and_labels_nice_cycles() {
        let t = demo();
        let view = ClockView::default();
        let vp = Viewport {
            start: 0.0,
            end: 5000.0,
        };
        let m = ruler_marks(&view, &t, &vp, 1000.0, 64.0, TimeBase::si(-12));
        // 334 ps is 66.8 px: every edge is ticked and labelled.
        let times: Vec<u64> = m.ticks.iter().map(|t| t.time).collect();
        assert_eq!(times, (0..14).map(|k| 400 + k * 334).collect::<Vec<_>>());
        assert!(m.ticks.iter().all(|t| t.label.is_some()));
        assert_eq!(m.ticks[3].label.as_deref(), Some("3"));
        assert!(m.flags.is_empty() && m.stopped.is_empty());
        // Zoomed out, only labelled edges remain, at multiples of the step.
        let vp = Viewport {
            start: 0.0,
            end: 100_000.0,
        };
        let m = ruler_marks(&view, &t, &vp, 1000.0, 64.0, TimeBase::si(-12));
        // 3.34 px per cycle in the first stretch: only every 20th edge, labelled.
        let first: Vec<_> = m.ticks.iter().filter(|t| t.time < 20000).collect();
        assert!(first.iter().all(|t| {
            t.label
                .as_deref()
                .is_some_and(|l| l.parse::<i64>().unwrap() % 20 == 0)
        }));
        // 10 px per cycle in the last: every edge, every 10th labelled.
        let last: Vec<_> = m.ticks.iter().filter(|t| t.time >= 50772).collect();
        assert_eq!(last.len(), 50);
        assert!(last.iter().all(|t| {
            t.label
                .as_ref()
                .is_none_or(|l| l.parse::<i64>().unwrap() % 10 == 0)
        }));
        assert_eq!(last.iter().filter(|t| t.label.is_some()).count(), 5);
        assert_eq!(
            m.flags,
            vec![(20272, "→ 2.00 GHz".into()), (50772, "→ 1.00 GHz".into())]
        );
    }

    #[test]
    fn origin_renumbers_and_stopped_is_hatched() {
        let t = ClockTimeline::new(vec![(0, 100, 10), (1000, 1100, 10)], false).unwrap();
        let view = ClockView {
            origin: Some(55),
            ..Default::default()
        };
        assert_eq!(view.display_cycle(&t, 5), 0);
        assert_eq!(view.absolute_cycle(&t, -5), Some(0));
        let at = t.cycle_at(57).unwrap();
        assert_eq!(format_position(&view, &t, &at), "0 + 0.70");
        let vp = Viewport {
            start: 0.0,
            end: 2000.0,
        };
        let m = ruler_marks(&view, &t, &vp, 2000.0, 20.0, TimeBase::si(-9));
        assert_eq!(m.stopped, vec![(100.0, 1000.0), (1100.0, 2000.0)]);
        // 10 px per cycle: every second displayed cycle is labelled.
        assert_eq!(m.ticks[0].label, None);
        assert_eq!(m.ticks[1].label.as_deref(), Some("-4"));
    }

    #[test]
    fn speeds_and_deltas() {
        assert_eq!(speed_label(500, TimeBase::si(-12)), "→ 2.00 GHz");
        assert_eq!(speed_label(3, TimeBase::si(-9)), "→ 333.33 MHz");
        let cycles = TimeBase {
            timescale: 0,
            unit: Some("cycle"),
        };
        assert_eq!(speed_label(2, cycles), "→ 2 cycle");
        // The first stretch holds 59 edges, so 20272 is cycle 59.
        assert_eq!(cycles_between(&demo(), 400, 20272 + 500), Some(60));
    }
}
