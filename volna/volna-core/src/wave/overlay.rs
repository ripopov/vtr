//! What every timed panel paints over its time column: the tick grid, the
//! header's tick labels and unit, the clock rulers under it, the Markers
//! lane with its chips and marker lines, the Measure lane with the reference
//! and its live span to the cursor, and the cursor line with its time or
//! cycle chip. The wave and pipeline painters call these with their own
//! rectangles so both agree pixel for pixel.

use std::ops::Range;

use crate::clock::{self, Clock, ClockView, Clocks};
use crate::color::Color;
use crate::document::Document;
use crate::geometry::{CursorIcon, Point, Rect, point, size, snap};
use crate::marker::{LaneHit, Marker};
use crate::measure::{Measurement, measure};
use crate::scene::{FontRole, Scene, TextCache, TextMeasure};
use crate::theme::Theme;
use crate::wave::layout::SCROLLBAR_W;
use crate::wave::timeline::{Tick, TimeBase, format_time, ticks};
use crate::wave::viewport::Viewport;

/// Pixel constants are design sizes at zoom 1.0; painters multiply them by
/// the theme's zoom. Hairlines stay one pixel.
pub const TICK_SPACING_PX: f64 = 96.0;
/// Height of the Markers lane at zoom 1.0.
pub const LANE_H: f32 = 22.0;
/// Height of a chip on the Markers lane at zoom 1.0.
pub const CHIP_H: f32 = 16.0;
/// Horizontal padding inside a chip at zoom 1.0.
const CHIP_PAD: f32 = 5.0;
/// Chips closer than this at zoom 1.0 merge into a cluster.
const CHIP_GAP: f32 = 3.0;
/// Advance of a digit or `…` in a chip at zoom 1.0: the semibold UI glyphs
/// are about this wide. Names use [`chip_char_w`]; the layout needs no font.
const CHIP_CHAR_W: f32 = 7.0;
/// A shortened name keeps at least this many characters, or the chip shows
/// only the number.
const NAME_MIN_CHARS: usize = 3;
/// Width of a marker's name field at zoom 1.0, unless the lane ends sooner.
const NAME_FIELD_W: f32 = 180.0;
/// The grab area of a chip reaches this far past its left and right edges
/// at zoom 1.0, because a thin flag at the edge of a busy lane is a small
/// target.
const CHIP_GRAB: (f32, f32) = (3.0, 2.0);
/// A cluster's tooltip lists at most this many markers.
const CLUSTER_LIST: usize = 8;
/// Advance of a span label's character at zoom 1.0: the small monospace
/// face is 0.6 em wide.
const SPAN_CHAR_W: f32 = 6.6;
/// Clearance on each side of a span label at zoom 1.0.
const SPAN_LABEL_PAD: f32 = 5.0;
/// A span with less room than this at zoom 1.0 goes unlabelled.
const SPAN_LABEL_MIN: f32 = 24.0;
/// A span's line keeps this far from the chips it joins at zoom 1.0.
const SPAN_INSET: f32 = 2.0;
/// A span whose visible line is shorter than this at zoom 1.0 is not drawn.
const SPAN_MIN: f32 = 6.0;
/// Width of the reference's `R` tag on the Measure lane at zoom 1.0.
const TAG_W: f32 = 16.0;
/// Side of the Measure lane's `×` button at zoom 1.0.
const CLEAR_W: f32 = 16.0;
/// The live span's arrow head at the cursor end, at zoom 1.0.
const ARROW: f32 = 6.0;
/// Opacity of the tint over the measured interval.
const TINT_ALPHA: f32 = 0.07;
/// Height of one clock ruler row at zoom 1.0.
pub const RULER_H: f32 = 16.0;
/// Cycle labels on a clock ruler stay at least this far apart at zoom 1.0.
const RULER_LABEL_PX: f64 = 56.0;
/// Spacing of the hatching of a stopped clock at zoom 1.0.
const HATCH_PX: f32 = 6.0;

/// The painter's shared context: theme, text measurement and the scene.
pub struct TextPainter<'a> {
    pub theme: &'a Theme,
    pub text: &'a mut TextCache,
    pub measure: &'a mut dyn TextMeasure,
    pub scene: &'a mut Scene,
}

impl TextPainter<'_> {
    pub fn width(&mut self, text: &str, font: FontRole, size: f32) -> f32 {
        self.text.width(self.measure, text, font, size)
    }
}

/// The time column of a panel: its header cell, the clock rulers below it
/// (empty without rulers), the Markers lane with the Measure lane under it
/// while a reference exists, and the rows area below them.
#[derive(Clone, Copy, Debug)]
pub struct TimeColumn {
    pub header: Rect,
    pub rulers: Rect,
    pub lane: Rect,
    pub area: Rect,
    pub viewport: Viewport,
}

impl TimeColumn {
    pub fn width_f64(&self) -> f64 {
        f64::from(self.area.width()).max(1.0)
    }

    /// Snapped panel x of a time.
    pub fn x_of(&self, t: f64) -> f32 {
        snap(self.area.left() + self.viewport.x_of(t, self.width_f64()) as f32)
    }

    /// Tick placement for this column at the theme's zoom.
    pub fn ticks<'a>(&self, base: TimeBase<'a>, zoom: f32) -> (Vec<Tick>, &'a str) {
        ticks(
            &self.viewport,
            self.width_f64(),
            base,
            TICK_SPACING_PX * f64::from(zoom),
        )
    }
}

/// Clock ruler rows: each clock's name in `names` (the band's cells left of
/// the time column) and, in the time column, a tick at each rising edge,
/// cycle labels spaced per stretch, a flag at each change of speed,
/// hatching where the clock is stopped and a cursor chip with the cursor's
/// cycle in that clock. The selected clock's name is highlighted.
pub fn clock_rulers(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    names: Rect,
    view: &ClockView,
    clocks: &Clocks,
    base: TimeBase<'_>,
    cursor: Option<u64>,
) {
    let rows = view.rulers(clocks);
    if rows.is_empty() || column.rulers.height() <= 0.0 {
        return;
    }
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let row_h = column.rulers.height() / rows.len() as f32;
    let selected = view.selected(clocks).map(|c| c.track);
    let band_all = Rect::new(
        point(names.left(), column.rulers.top()),
        size(column.rulers.right() - names.left(), column.rulers.height()),
    );
    p.scene.fill(band_all, t.panel.bg);
    for (i, c) in rows.iter().enumerate() {
        let y = column.rulers.top() + row_h * i as f32;
        let band = Rect::new(
            point(column.rulers.left(), y),
            size(column.rulers.width(), row_h),
        );
        let cell = Rect::new(point(names.left(), y), size(names.width(), row_h));
        let is_selected = selected == Some(c.track);
        if is_selected {
            p.scene.fill(cell, t.selection.bg);
        }
        let name = c.name.clone();
        let color = if is_selected {
            t.selection.text
        } else {
            t.panel.text_muted
        };
        p.scene.clipped(cell, |scene| {
            scene.text(
                point(cell.left() + z(12.0), y),
                row_h,
                name,
                FontRole::Mono,
                t.ui_size_small,
                color,
            );
        });
        p.scene.fill(
            Rect::new(
                point(names.left(), band.bottom() - 1.0),
                size(band.right() - names.left(), 1.0),
            ),
            t.border_variant,
        );
        let Some(timeline) = c.timeline() else {
            let note = match &c.state {
                clock::ClockState::Failed(_) => "clock failed to load",
                _ => "loading…",
            };
            p.scene.clipped(band, |scene| {
                scene.text(
                    point(band.left() + z(8.0), y),
                    row_h,
                    note,
                    FontRole::Ui,
                    t.ui_size_small,
                    t.panel.text_placeholder,
                );
            });
            continue;
        };
        let marks = clock::ruler_marks(
            view,
            timeline,
            &column.viewport,
            column.width_f64(),
            RULER_LABEL_PX * f64::from(t.zoom),
            base,
        );
        let x_of = |time: f64| column.x_of(time);
        let mut hatch = Vec::new();
        for (a, b) in &marks.stopped {
            let (x0, x1) = (x_of(*a).max(band.left()), x_of(*b).min(band.right()));
            let mut x = x0 - row_h;
            while x < x1 {
                hatch.push([point(x, band.bottom()), point(x + row_h, band.top())]);
                x += z(HATCH_PX);
            }
        }
        let mut stubs = Vec::new();
        let mut labels = Vec::new();
        for tick in &marks.ticks {
            let x = x_of(tick.time as f64);
            let h = if tick.label.is_some() { z(6.0) } else { z(3.0) };
            stubs.push(Rect::new(point(x, band.bottom() - h - 1.0), size(1.0, h)));
            if let Some(label) = &tick.label {
                labels.push((x, label.clone()));
            }
        }
        let mut flags = Vec::new();
        for (time, text) in &marks.flags {
            let x = x_of(*time as f64);
            let w = p.width(text, FontRole::UiSemibold, t.ui_size_small);
            flags.push((x, text.clone(), w));
        }
        // The cursor's cycle in this clock, beside the cursor line like the time chip.
        let chip = cursor
            .filter(|&c| {
                let x = column.viewport.x_of(c as f64, column.width_f64());
                (-1.0..=column.width_f64() + 1.0).contains(&x)
            })
            .and_then(|c| Some((c, timeline.cycle_at(c)?)))
            .map(|(c, at)| {
                let text = clock::format_position(view, timeline, &at);
                let w = p.width(&text, FontRole::Mono, t.ui_size_small) + z(10.0);
                let x = x_of(c as f64);
                let left = if x + 1.0 + w > band.right() - z(SCROLLBAR_W) {
                    x - w
                } else {
                    x + 1.0
                };
                (
                    Rect::new(point(left, y + z(1.0)), size(w, row_h - z(2.0))),
                    text,
                )
            });
        p.scene.clipped(band, |scene| {
            if !hatch.is_empty() {
                scene.lines(hatch, t.wave_dense, 1.0);
            }
            for stub in stubs {
                scene.fill(stub, t.wave_tick_text);
            }
            for (x, label) in labels {
                scene.text(
                    point(x + z(3.0), y),
                    row_h - z(2.0),
                    label,
                    FontRole::Mono,
                    t.ui_size_small,
                    t.wave_tick_text,
                );
            }
            for (x, text, w) in flags {
                let chip = Rect::new(point(x + 1.0, y + z(1.0)), size(w + z(8.0), row_h - z(3.0)));
                scene.fill(Rect::new(point(x, y), size(1.0, row_h)), t.border_focused);
                scene.quad(chip, t.badge.bg, z(2.0), 0.0, Color::TRANSPARENT);
                scene.text(
                    point(chip.left() + z(4.0), chip.top()),
                    chip.height(),
                    text,
                    FontRole::UiSemibold,
                    t.ui_size_small,
                    t.badge.text,
                );
            }
            if let Some((rect, text)) = chip {
                scene.quad(rect, t.wave_cursor, z(3.0), 0.0, Color::TRANSPARENT);
                scene.text(
                    point(rect.left() + z(5.0), rect.top()),
                    rect.height(),
                    text,
                    FontRole::Mono,
                    t.ui_size_small,
                    t.wave_cursor_text,
                );
            }
        });
    }
}

/// Estimated advance of `c` in the chip font at zoom 1.0, by glyph class of
/// a proportional semibold UI face at 11 px. Estimates keep the layout
/// font-free; the painter trims a name whose measured text is wider.
fn chip_char_w(c: char) -> f32 {
    match c {
        '0'..='9' | '…' => CHIP_CHAR_W,
        'i' | 'l' | 'j' | '\'' | '.' | ',' | ':' | ';' | '|' | '!' => 3.3,
        'f' | 't' | 'r' | ' ' | 'I' | '(' | ')' | '[' | ']' | '-' | '/' => 4.4,
        'm' | 'w' | 'M' | 'W' => 9.9,
        'A'..='Z' => 7.7,
        c if c.is_ascii() => 6.4,
        _ => 11.0,
    }
}

/// Estimated width of a chip holding `text` at zoom 1.0, padding included.
fn chip_w(text: &str) -> f32 {
    text.chars().map(chip_char_w).sum::<f32>() + 2.0 * CHIP_PAD
}

/// The chip text for marker `id` named `name` in `room` design pixels: the
/// number and the whole name, else the number and the name cut to at least
/// [`NAME_MIN_CHARS`] characters and `…`, else `None` (the number alone).
fn fit_name(id: crate::marker::MarkerId, name: &str, room: f32) -> Option<String> {
    let full = format!("{id} {name}");
    if chip_w(&full) <= room {
        return Some(full);
    }
    let cuts: Vec<usize> = name.char_indices().map(|(i, _)| i).collect();
    (NAME_MIN_CHARS..cuts.len())
        .rev()
        .map(|n| format!("{id} {}…", name[..cuts[n]].trim_end()))
        .find(|text| chip_w(text) <= room)
}

/// One chip on the Markers lane: a single marker, or a cluster of markers
/// too close together to draw apart at this zoom.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerChip {
    /// Indices into the document's markers, consecutive in time.
    pub markers: Range<usize>,
    /// Its left edge is the first marker's time.
    pub rect: Rect,
    /// The marker's number and as much of its name as fits, or `…` and the
    /// count of a cluster.
    pub text: String,
}

impl MarkerChip {
    pub fn is_cluster(&self) -> bool {
        self.markers.len() > 1
    }
}

/// What labels the spans between markers: the panel's ruler clocks, the one
/// it has selected, and how the trace writes time.
#[derive(Clone, Copy, Debug)]
pub struct SpanClocks<'a> {
    pub rulers: &'a [&'a Clock],
    /// Index into `rulers`; the short label forms count in this clock.
    pub selected: Option<usize>,
    pub base: TimeBase<'a>,
}

impl<'a> SpanClocks<'a> {
    /// Spans labelled in time alone.
    pub fn time_only(base: TimeBase<'a>) -> Self {
        Self {
            rulers: &[],
            selected: None,
            base,
        }
    }

    /// The rulers of `view`, its selected clock among them (else the first).
    pub fn of(
        rulers: &'a [&'a Clock],
        view: &ClockView,
        clocks: &Clocks,
        base: TimeBase<'a>,
    ) -> Self {
        let selected = view.selected(clocks).map(|c| c.path.as_str());
        Self {
            rulers,
            selected: rulers
                .iter()
                .position(|c| Some(c.path.as_str()) == selected)
                .or((!rulers.is_empty()).then_some(0)),
            base,
        }
    }
}

/// The dimension line between two neighbouring markers on the Markers lane.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanMark {
    /// The earlier marker, an index into the document's markers; the span
    /// ends at the next one.
    pub first: usize,
    /// The line's ends, clear of the chips. A marker off the view puts its
    /// end at its time, past the lane's edge.
    pub x0: f32,
    pub x1: f32,
    pub measurement: Measurement,
    /// Label forms whose estimated width fits the visible line, longest
    /// first: the time and every ruler clock, the selected clock and the
    /// time, the selected clock, the time. Empty when none fits. The painter
    /// shows the first its font fits.
    pub labels: Vec<String>,
}

/// The label forms of a span, longest first.
fn span_forms(m: &Measurement, selected: Option<usize>, base: TimeBase<'_>) -> Vec<String> {
    let time = signed_time(m.dt(), base);
    let count = |c: &crate::measure::ClockCount| format!("{} {}", c.cycles, c.name);
    let mut forms = vec![
        std::iter::once(time.clone())
            .chain(m.clocks.iter().map(count))
            .collect::<Vec<_>>()
            .join(" · "),
    ];
    if let Some(c) = selected.and_then(|i| m.clocks.get(i)) {
        if m.clocks.len() > 1 {
            forms.push(format!("{} · {time}", count(c)));
        }
        forms.push(count(c));
    }
    forms.push(time);
    forms.dedup();
    forms
}

/// `−126 ns`: a signed interval.
pub fn signed_time(dt: i128, base: TimeBase<'_>) -> String {
    let sign = if dt < 0 { "−" } else { "" };
    format!("{sign}{}", format_time(dt.unsigned_abs() as f64, base))
}

/// The label forms of `m` whose estimated width fits a line `shown` pixels
/// long, longest first; none when the line is too short for any label.
fn fitting_labels(m: &Measurement, clocks: SpanClocks<'_>, shown: f32, zoom: f32) -> Vec<String> {
    let room = shown / zoom - 2.0 * SPAN_LABEL_PAD;
    if room < SPAN_LABEL_MIN {
        return Vec::new();
    }
    span_forms(m, clocks.selected, clocks.base)
        .into_iter()
        .filter(|f| span_text_w(f) <= room)
        .collect()
}

/// Estimated width of span label `text` at zoom 1.0, without padding.
fn span_text_w(text: &str) -> f32 {
    text.chars().count() as f32 * SPAN_CHAR_W
}

/// What a panel measures: from the document's reference to its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measuring {
    pub reference: u64,
    pub cursor: Option<u64>,
}

/// The Measure lane under the Markers lane, shown while a reference exists.
#[derive(Clone, Debug, PartialEq)]
pub struct MeasureLane {
    pub band: Rect,
    pub reference: u64,
    /// The `R` tag centred on the reference's time; it may lie off the lane.
    pub tag: Rect,
    /// The `×` button that clears the reference, at the right end of the
    /// lane's cells left of the time column.
    pub clear: Rect,
    /// The span from the reference to the cursor, unless there is no cursor
    /// or it sits on the reference.
    pub live: Option<LiveSpan>,
    /// The panel's selected clock, an index into the live measurement's
    /// clocks: the value cell counts in it.
    pub selected: Option<usize>,
}

/// The live span on the Measure lane.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveSpan {
    /// The line's ends left to right, clear of the tag. An end off the view
    /// lies past the lane's edge.
    pub x0: f32,
    pub x1: f32,
    /// From the reference to the cursor: negative when the cursor is earlier.
    pub measurement: Measurement,
    /// Label forms that fit the visible line, longest first, as on a
    /// [`SpanMark`].
    pub labels: Vec<String>,
}

/// The laid-out Markers lane of one panel, with its Measure lane. Pure:
/// computed from times and pixel geometry, then read by the painter and the
/// input handlers alike.
#[derive(Clone, Debug, Default)]
pub struct MarkerLane {
    /// The lane across the whole panel, below the clock rulers.
    pub band: Rect,
    /// Left edge of the time column; the cells left of it hold the title.
    pub time_left: f32,
    /// Markers inside the viewport, each in exactly one chip.
    pub visible: Range<usize>,
    /// Chips left to right; they never overlap.
    pub chips: Vec<MarkerChip>,
    /// Spans between neighbouring chips left to right, and to the nearest
    /// marker beyond each edge of the view.
    pub spans: Vec<SpanMark>,
    /// The Measure lane below, while a reference exists.
    pub measure: Option<MeasureLane>,
    pub zoom: f32,
}

impl MarkerLane {
    /// Index of the chip whose grab area holds `p`.
    pub fn chip_at(&self, p: Point) -> Option<usize> {
        if !self.band.contains(p) || p.x < self.time_left {
            return None;
        }
        let (left, right) = (CHIP_GRAB.0 * self.zoom, CHIP_GRAB.1 * self.zoom);
        self.chips
            .iter()
            .position(|c| p.x >= c.rect.left() - left && p.x <= c.rect.right() + right)
    }

    /// Where the name field of marker `ix` (an index into the markers)
    /// sits: over its chip, after the number, reaching [`NAME_FIELD_W`] or
    /// the chip's end, cut at the lane's end. `None` while the marker is
    /// off the view.
    pub fn name_field(&self, ix: usize, id: crate::marker::MarkerId) -> Option<Rect> {
        let chip = self.chips.iter().find(|c| c.markers.contains(&ix))?;
        let z = |v: f32| v * self.zoom;
        let left = chip.rect.left() + z(chip_w(&id.to_string()));
        let right = (left + z(NAME_FIELD_W))
            .max(chip.rect.right())
            .min(self.band.right());
        (right > left).then(|| {
            Rect::new(
                point(left, self.band.top() + 1.0),
                size(right - left, (self.band.height() - 2.0).max(0.0)),
            )
        })
    }

    /// The part of `span`'s line inside the time column.
    pub fn shown(&self, span: &SpanMark) -> (f32, f32) {
        (span.x0.max(self.time_left), span.x1.min(self.band.right()))
    }

    /// Index of the span whose line is under `p`, away from any chip.
    pub fn span_at(&self, p: Point) -> Option<usize> {
        if !self.band.contains(p) || p.x < self.time_left || self.chip_at(p).is_some() {
            return None;
        }
        self.spans.iter().position(|s| {
            let (x0, x1) = self.shown(s);
            p.x >= x0 && p.x <= x1
        })
    }

    /// Both lanes: the Markers lane and the Measure lane under it.
    pub fn strip(&self) -> Rect {
        match &self.measure {
            Some(m) => Rect::new(
                self.band.origin,
                size(self.band.width(), m.band.bottom() - self.band.top()),
            ),
            None => self.band,
        }
    }

    /// The part of the live span's line inside the time column.
    pub fn live_shown(&self, live: &LiveSpan) -> (f32, f32) {
        (live.x0.max(self.time_left), live.x1.min(self.band.right()))
    }

    /// Whether `p` is on the visible live span.
    pub fn live_at(&self, p: Point) -> bool {
        let Some(m) = &self.measure else {
            return false;
        };
        let Some(live) = &m.live else {
            return false;
        };
        let (x0, x1) = self.live_shown(live);
        m.band.contains(p) && x1 > x0 && p.x >= x0 && p.x <= x1
    }

    /// What `p` is over on the lanes.
    pub fn hit(&self, p: Point) -> Option<LaneHit> {
        if let Some(m) = &self.measure {
            if m.clear.contains(p) && m.clear.left() >= self.band.left() {
                return Some(LaneHit::ClearReference);
            }
            if m.band.contains(p) && p.x >= self.time_left && m.tag.contains(p) {
                return Some(LaneHit::Tag);
            }
        }
        if self.live_at(p) {
            return Some(LaneHit::Live);
        }
        let Some(ix) = self.chip_at(p) else {
            return self.span_at(p).map(|i| LaneHit::Span(self.spans[i].first));
        };
        let chip = &self.chips[ix];
        Some(if chip.is_cluster() {
            LaneHit::Cluster(chip.markers.clone())
        } else {
            LaneHit::Chip(chip.markers.start)
        })
    }
}

/// Lay out the Markers lane `band`, whose time column starts at `time_left`
/// and is `width_px` wide. One pass over the markers inside the viewport,
/// found by binary search in the time-sorted list: each gets a chip with its
/// number, and a chip that would touch the one before it joins it in a
/// cluster. Then each named marker's chip grows to its name, or a shortened
/// one, where the gap to the next chip or the lane's end allows. Last, each
/// gap between chips, and the way to the nearest marker beyond each edge of
/// the view, gets a span measured in `clocks` and labelled with the longest
/// form that fits. With `measuring`, the lower half of `band` is the Measure
/// lane ([`measure_lane`]).
#[allow(clippy::too_many_arguments)]
pub fn marker_lane(
    band: Rect,
    time_left: f32,
    width_px: f64,
    viewport: Viewport,
    markers: &[Marker],
    clocks: SpanClocks<'_>,
    measuring: Option<Measuring>,
    zoom: f32,
) -> MarkerLane {
    let z = |v: f32| v * zoom;
    let (band, measure_band) = match measuring {
        Some(_) => {
            let h = (band.height() / 2.0).floor();
            (
                Rect::new(band.origin, size(band.width(), h)),
                Some(Rect::new(
                    point(band.left(), band.top() + h),
                    size(band.width(), band.height() - h),
                )),
            )
        }
        None => (band, None),
    };
    let first = markers.partition_point(|m| (m.time as f64) < viewport.start);
    let last = first + markers[first..].partition_point(|m| (m.time as f64) <= viewport.end);
    let chip_h = z(CHIP_H);
    let top = snap(band.top() + (band.height() - chip_h) / 2.0);
    let width = |text: &str| z(chip_w(text));
    let mut chips: Vec<MarkerChip> = Vec::new();
    for (ix, m) in markers.iter().enumerate().take(last).skip(first) {
        let x = snap(time_left + viewport.x_of(m.time as f64, width_px) as f32);
        match chips.last_mut() {
            Some(chip) if x < chip.rect.right() + z(CHIP_GAP) => {
                chip.markers.end = ix + 1;
                chip.text = format!("…{}", chip.markers.len());
                let w = chip.rect.width().max(width(&chip.text));
                chip.rect = Rect::new(chip.rect.origin, size(w, chip_h));
            }
            _ => {
                let text = m.id.to_string();
                chips.push(MarkerChip {
                    markers: ix..ix + 1,
                    rect: Rect::new(point(x, top), size(width(&text), chip_h)),
                    text,
                });
            }
        }
    }
    for k in 0..chips.len() {
        let chip = &chips[k];
        if chip.is_cluster() {
            continue;
        }
        let m = &markers[chip.markers.start];
        let Some(name) = m.label.as_deref() else {
            continue;
        };
        let end = chips
            .get(k + 1)
            .map_or(band.right(), |next| next.rect.left() - z(CHIP_GAP));
        if let Some(text) = fit_name(m.id, name, (end - chip.rect.left()) / zoom) {
            let chip = &mut chips[k];
            chip.rect = Rect::new(chip.rect.origin, size(width(&text), chip_h));
            chip.text = text;
        }
    }
    let spans = lane_spans(
        band,
        time_left,
        width_px,
        viewport,
        markers,
        first..last,
        &chips,
        clocks,
        zoom,
    );
    let measure = measuring
        .zip(measure_band)
        .map(|(m, band)| measure_lane(band, time_left, width_px, viewport, m, clocks, zoom));
    MarkerLane {
        band,
        time_left,
        visible: first..last,
        chips,
        spans,
        measure,
        zoom,
    }
}

/// Lay out the Measure lane `band`: the `R` tag on the reference, and the
/// live span from the tag to the cursor measured in `clocks`, labelled like
/// the spans between markers.
pub fn measure_lane(
    band: Rect,
    time_left: f32,
    width_px: f64,
    viewport: Viewport,
    measuring: Measuring,
    clocks: SpanClocks<'_>,
    zoom: f32,
) -> MeasureLane {
    let z = |v: f32| v * zoom;
    let x_of = |t: u64| time_left + viewport.x_of(t as f64, width_px) as f32;
    let xr = snap(x_of(measuring.reference));
    let chip_h = z(CHIP_H);
    let tag = Rect::new(
        point(
            xr - (z(TAG_W) / 2.0).round(),
            snap(band.top() + (band.height() - chip_h) / 2.0),
        ),
        size(z(TAG_W), chip_h),
    );
    let live = measuring
        .cursor
        .filter(|&c| c != measuring.reference)
        .map(|c| {
            let xc = snap(x_of(c));
            let (x0, x1) = if c > measuring.reference {
                (tag.right() + z(SPAN_INSET), xc)
            } else {
                (xc, tag.left() - z(SPAN_INSET))
            };
            let measurement = measure(clocks.rulers, measuring.reference, c);
            let shown = x1.min(band.right()) - x0.max(time_left);
            let labels = fitting_labels(&measurement, clocks, shown, zoom);
            LiveSpan {
                x0,
                x1,
                measurement,
                labels,
            }
        });
    let clear = Rect::new(
        point(
            // Clear of the column divider's grab zone.
            time_left - z(6.0) - z(CLEAR_W),
            snap(band.top() + (band.height() - z(CLEAR_W)) / 2.0),
        ),
        size(z(CLEAR_W), z(CLEAR_W)),
    );
    MeasureLane {
        band,
        reference: measuring.reference,
        tag,
        clear,
        live,
        selected: clocks.selected,
    }
}

/// The spans of [`marker_lane`]: one per gap between chips, and one from
/// the first chip back to the marker before the view and from the last on
/// to the marker after it (or across the view when no chip is in it).
#[allow(clippy::too_many_arguments)]
fn lane_spans(
    band: Rect,
    time_left: f32,
    width_px: f64,
    viewport: Viewport,
    markers: &[Marker],
    visible: Range<usize>,
    chips: &[MarkerChip],
    clocks: SpanClocks<'_>,
    zoom: f32,
) -> Vec<SpanMark> {
    let z = |v: f32| v * zoom;
    let x_of = |ix: usize| time_left + viewport.x_of(markers[ix].time as f64, width_px) as f32;
    // (markers, left, right) of each end a span can join, left to right.
    let mut ends: Vec<(Range<usize>, f32, f32)> = Vec::with_capacity(chips.len() + 2);
    if visible.start > 0 {
        let ix = visible.start - 1;
        ends.push((ix..ix + 1, x_of(ix), x_of(ix)));
    }
    ends.extend(
        chips
            .iter()
            .map(|c| (c.markers.clone(), c.rect.left(), c.rect.right())),
    );
    if visible.end < markers.len() {
        let ix = visible.end;
        ends.push((ix..ix + 1, x_of(ix), x_of(ix)));
    }
    let right = band.right();
    ends.windows(2)
        .filter_map(|pair| {
            let first = pair[0].0.end - 1;
            let (from, to) = (markers[first].time, markers[first + 1].time);
            let (x0, x1) = (pair[0].2 + z(SPAN_INSET), pair[1].1 - z(SPAN_INSET));
            let shown = x1.min(right) - x0.max(time_left);
            if from == to || shown < z(SPAN_MIN) {
                return None;
            }
            let measurement = measure(clocks.rulers, from, to);
            let labels = fitting_labels(&measurement, clocks, shown, zoom);
            Some(SpanMark {
                first,
                x0,
                x1,
                measurement,
                labels,
            })
        })
        .collect()
}

/// One-pixel tick lines down the rows area.
pub fn grid(p: &mut TextPainter<'_>, column: &TimeColumn, tick_list: &[Tick]) {
    let area = column.area;
    let color = p.theme.wave_tick;
    let xs: Vec<f32> = tick_list
        .iter()
        .map(|tick| column.x_of(tick.time))
        .collect();
    p.scene.clipped(area, |scene| {
        for x in xs {
            scene.fill(
                Rect::new(point(x, area.top()), size(1.0, area.height())),
                color,
            );
        }
    });
}

/// Shade the time column's area before the trace starts and after it ends,
/// where panning and zooming reach but no data exists. Returns the area's
/// x range inside the trace (empty when none of it is).
pub fn outside_time(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    limits: (u64, u64),
) -> std::ops::Range<f32> {
    let area = column.area;
    let to_x = |t: u64| {
        let x = area.left() as f64 + column.viewport.x_of(t as f64, column.width_f64());
        x.clamp(f64::from(area.left()), f64::from(area.right())) as f32
    };
    let (start, end) = (to_x(limits.0), to_x(limits.1));
    let color = p.theme.wave_outside;
    if start > area.left() {
        p.scene.fill(
            Rect::new(area.origin, size(start - area.left(), area.height())),
            color,
        );
    }
    if end < area.right() {
        p.scene.fill(
            Rect::new(
                point(end, area.top()),
                size(area.right() - end, area.height()),
            ),
            color,
        );
    }
    start..end.max(start)
}

/// Tick stubs, labels and the unit in the header cell of the time column.
pub fn header_ticks(p: &mut TextPainter<'_>, column: &TimeColumn, tick_list: &[Tick], unit: &str) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let header = column.header;
    let unit_w = (!unit.is_empty()).then(|| p.width(unit, FontRole::Mono, t.ui_size_small));
    let unit_x = unit_w
        .map(|w| header.right() - w - z(SCROLLBAR_W + 4.0))
        .unwrap_or(header.right());
    let mut labels = Vec::new();
    for tick in tick_list {
        let x = column.x_of(tick.time);
        let w = p.width(&tick.label, FontRole::Mono, t.ui_size_small);
        labels.push((x, tick.label.clone(), w));
    }
    let panel_theme = t.panel;
    let unit = unit.to_owned();
    p.scene.clipped(header, |scene| {
        for (x, label, w) in labels {
            scene.fill(
                Rect::new(point(x, header.bottom() - z(7.0)), size(1.0, z(6.0))),
                panel_theme.text_placeholder,
            );
            if x + z(4.0) + w < unit_x - z(8.0) {
                scene.text(
                    point(x + z(4.0), header.top() + z(2.0)),
                    z(20.0),
                    label,
                    FontRole::Mono,
                    t.ui_size_small,
                    t.wave_tick_text,
                );
            }
        }
        if unit_w.is_some() {
            scene.text(
                point(unit_x, header.top() + z(2.0)),
                z(20.0),
                unit,
                FontRole::Mono,
                t.ui_size_small,
                panel_theme.text_placeholder,
            );
        }
    });
}

/// The Markers lane: its title in `title` (the band's cells left of the
/// time column), the marker count or the marker under the cursor in
/// `value` when the panel has a values column, the spans between markers,
/// the chips, and a line for every visible marker from its chip down
/// through the rows. Hovering a chip or cluster lists its markers, and
/// hovering a span gives its full measurement. The Measure lane follows
/// ([`measure_lane_paint`]). While a chip is dragged, `dragged_from` is
/// where its marker started, drawn as a dotted line.
#[allow(clippy::too_many_arguments)]
pub fn marker_lane_paint(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    lane: &MarkerLane,
    title: Rect,
    value: Option<Rect>,
    doc: &Document,
    cursor: Option<u64>,
    pointer: Option<Point>,
    dragged_from: Option<u64>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let band = lane.band;
    if band.height() <= 0.0 {
        return;
    }
    let markers = doc.markers();
    p.scene.fill(band, t.panel.bg);
    p.scene.fill(
        Rect::new(
            point(band.left(), band.bottom() - 1.0),
            size(band.width(), 1.0),
        ),
        t.border_variant,
    );
    let cell = |r: Rect| {
        Rect::new(
            point(r.left(), band.top()),
            size(r.width(), band.height() - 1.0),
        )
    };
    let title = cell(title);
    p.scene.clipped(title, |scene| {
        scene.text(
            point(title.left() + z(12.0), title.top()),
            title.height(),
            "Markers",
            FontRole::UiSemibold,
            t.ui_size_small,
            t.panel.text_muted,
        );
    });
    if let Some(value) = value.map(cell) {
        let here = cursor.and_then(|c| markers.iter().find(|m| m.time == c));
        let (text, color) = match (here, markers.len()) {
            (Some(m), _) => (format!("at {}", m.id), t.panel.text),
            (None, 0) => ("no markers".into(), t.panel.text_placeholder),
            (None, 1) => ("1 marker".into(), t.panel.text_muted),
            (None, n) => (format!("{n} markers"), t.panel.text_muted),
        };
        p.scene.clipped(value, |scene| {
            scene.text(
                point(value.left() + z(8.0), value.top()),
                value.height(),
                text,
                FontRole::Mono,
                t.ui_size_small,
                color,
            );
        });
    }

    if let Some(measure) = &lane.measure {
        measure_lane_paint(p, column, lane, measure, title, value, doc, pointer);
    }
    let hovered = pointer.and_then(|mp| lane.chip_at(mp));
    let hovered_span = pointer.and_then(|mp| lane.span_at(mp));
    let reference = match doc.reference() {
        Some(crate::marker::Reference::Marker(id)) => Some(id),
        _ => None,
    };
    let time_band = Rect::new(
        point(column.lane.left(), band.top()),
        size(column.lane.width(), band.height()),
    );
    let lines_clip = Rect::new(
        point(column.area.left(), band.top()),
        size(column.area.width(), column.area.bottom() - band.top()),
    );
    let line_top = lane.chips.first().map_or(band.top(), |c| c.rect.top());
    let lines: Vec<_> = markers[lane.visible.clone()]
        .iter()
        .map(|m| {
            (
                column.x_of(m.time as f64),
                t.marker(m.id.palette_index()).stroke,
            )
        })
        .collect();
    let ghost = dragged_from.map(|t| {
        let x = column.x_of(t as f64);
        let (dot, gap) = (z(2.0).round().max(1.0), z(3.0).round().max(1.0));
        let mut dots = Vec::new();
        let mut y = band.top();
        while y < column.area.bottom() {
            dots.push(Rect::new(point(x, y), size(1.0, dot)));
            y += dot + gap;
        }
        dots
    });
    let ghost_color = t.panel.text_muted;
    p.scene.clipped(lines_clip, |scene| {
        for dot in ghost.into_iter().flatten() {
            scene.fill(dot, ghost_color);
        }
        for (x, color) in lines {
            scene.fill(
                Rect::new(
                    point(x, line_top),
                    size(1.0, column.area.bottom() - line_top),
                ),
                color,
            );
        }
    });
    spans_paint(p, lane, time_band, hovered_span);
    let mut chips = Vec::new();
    for (i, chip) in lane.chips.iter().enumerate() {
        let hover = hovered == Some(i);
        // The reference's marker wears a ring in the cursor colour.
        let ring = !chip.is_cluster() && reference == Some(markers[chip.markers.start].id);
        let (bg, fg) = if chip.is_cluster() {
            let s = if hover { t.badge_hover } else { t.badge };
            (s.bg, s.text)
        } else {
            let c = t.marker(markers[chip.markers.start].id.palette_index());
            if hover {
                (c.hover, c.hover_text)
            } else {
                (c.background, c.text)
            }
        };
        let (text, w) = fit_chip_text(p, &chip.text, chip.rect.width() - z(2.0 * CHIP_PAD));
        chips.push((chip.rect, text, w, bg, fg, ring));
    }
    p.scene.clipped(time_band, |scene| {
        for (rect, text, w, bg, fg, ring) in chips {
            // One quad: marker backgrounds may be translucent, so parts must not overlap.
            let (border, border_color) = if ring {
                (1.0, t.wave_cursor)
            } else {
                (0.0, Color::TRANSPARENT)
            };
            scene.quad(rect, bg, z(3.0), border, border_color);
            scene.text(
                point(snap(rect.left() + (rect.width() - w) / 2.0), rect.top()),
                rect.height(),
                text,
                FontRole::UiSemibold,
                t.ui_size_small,
                fg,
            );
        }
    });
    for chip in &lane.chips {
        p.scene.cursors.push((chip.rect, CursorIcon::PointingHand));
    }
    let over_measure = pointer.and_then(|mp| {
        let hit = lane.hit(mp)?;
        let on_measure = matches!(hit, LaneHit::Live | LaneHit::Tag | LaneHit::ClearReference);
        on_measure.then_some((hit, lane.measure.as_ref()?, mp))
    });
    if let Some((hit, measure, mp)) = over_measure {
        measure_tooltip(p, column, hit, measure, doc, mp);
    } else if let (Some(i), Some(mp)) = (hovered, pointer) {
        let under = &markers[lane.chips[i].markers.clone()];
        let header = match under {
            [one] => format!("Marker {}", one.id),
            many => format!("{} markers · click to zoom in", many.len()),
        };
        marker_tooltip(p, column, header, under, doc, mp);
    } else if let (Some(i), Some(mp)) = (hovered_span, pointer) {
        span_tooltip(p, column, &lane.spans[i], markers, doc, mp);
    }
}

/// The Measure lane: its title and, with a values column, the live span's
/// cycles in the selected clock (else its time); the `R` tag; the live span
/// in the cursor colour with an arrow head at the cursor and the longest
/// label that fits; and, down through the rows, a light tint over the
/// measured interval and a dashed line at the reference.
#[allow(clippy::too_many_arguments)]
fn measure_lane_paint(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    lane: &MarkerLane,
    measure: &MeasureLane,
    title: Rect,
    value: Option<Rect>,
    doc: &Document,
    pointer: Option<Point>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let band = measure.band;
    let accent = t.wave_cursor;
    p.scene.fill(band, t.panel.bg);
    p.scene.fill(
        Rect::new(
            point(band.left(), band.bottom() - 1.0),
            size(band.width(), 1.0),
        ),
        t.border_variant,
    );
    let cell = |r: Rect| {
        Rect::new(
            point(r.left(), band.top()),
            size(r.width(), band.height() - 1.0),
        )
    };
    let title = cell(title);
    p.scene.clipped(title, |scene| {
        scene.text(
            point(title.left() + z(12.0), title.top()),
            title.height(),
            "R → cursor",
            FontRole::UiSemibold,
            t.ui_size_small,
            t.panel.text_muted,
        );
    });
    // The × that clears the reference, where the lane's cells meet the time column.
    let clear = measure.clear;
    let clear_hover = pointer.is_some_and(|mp| clear.contains(mp));
    if clear.left() >= band.left() {
        let (bg, fg) = if clear_hover {
            (t.badge_hover.bg, t.panel.text)
        } else {
            (Color::TRANSPARENT, t.panel.text_muted)
        };
        let c = point(
            clear.left() + clear.width() / 2.0,
            clear.top() + clear.height() / 2.0,
        );
        let r = z(3.5);
        p.scene.quad(clear, bg, z(3.0), 0.0, Color::TRANSPARENT);
        p.scene.lines(
            vec![
                [point(c.x - r, c.y - r), point(c.x + r, c.y + r)],
                [point(c.x - r, c.y + r), point(c.x + r, c.y - r)],
            ],
            fg,
            1.0,
        );
        p.scene.cursors.push((clear, CursorIcon::PointingHand));
    }
    let base = doc.time_base();
    let live = measure.live.as_ref();
    if let Some(value) = value.map(cell).map(|v| {
        Rect::new(
            v.origin,
            size((measure.clear.left() - v.left()).max(0.0), v.height()),
        )
    }) {
        let (text, color) = match live {
            Some(live) => (
                live_value(&live.measurement, measure.selected, base),
                accent,
            ),
            None => (
                format!("R at {}", format_time(measure.reference as f64, base)),
                t.panel.text_placeholder,
            ),
        };
        p.scene.clipped(value, |scene| {
            scene.text(
                point(value.left() + z(8.0), value.top()),
                value.height(),
                text,
                FontRole::Mono,
                t.ui_size_small,
                color,
            );
        });
    }

    // Down through the rows: the tint, then the reference's dashed line.
    let area = column.area;
    let rows_clip = Rect::new(
        point(area.left(), band.top()),
        size(area.width(), area.bottom() - band.top()),
    );
    let xr = column.x_of(measure.reference as f64);
    let tint = live.map(|l| {
        let xc = column.x_of(l.measurement.to as f64);
        let (a, b) = (xr.min(xc), xr.max(xc));
        Rect::new(
            point(a, band.bottom()),
            size(b - a, area.bottom() - band.bottom()),
        )
    });
    let (dash, gap) = (z(4.0).round().max(1.0), z(3.0).round().max(1.0));
    let mut dashes = Vec::new();
    let mut y = measure.tag.bottom();
    while y < area.bottom() {
        dashes.push(Rect::new(
            point(xr, y),
            size(1.0, dash.min(area.bottom() - y)),
        ));
        y += dash + gap;
    }
    p.scene.clipped(rows_clip, |scene| {
        if let Some(tint) = tint {
            scene.fill(tint, accent.with_alpha(TINT_ALPHA));
        }
        for d in dashes {
            scene.fill(d, accent);
        }
    });

    // The lane: the live span, then the tag over its end.
    let time_band = Rect::new(
        point(column.lane.left(), band.top()),
        size(column.lane.width(), band.height()),
    );
    let cy = snap(band.top() + (band.height() - 1.0) / 2.0);
    let hovered = pointer.is_some_and(|mp| lane.live_at(mp));
    let line = live.map(|l| {
        let (x0, x1) = lane.live_shown(l);
        let room = x1 - x0 - z(2.0 * SPAN_LABEL_PAD);
        let label = l.labels.iter().find_map(|text| {
            let w = p.width(text, FontRole::Mono, t.ui_size_small);
            (w <= room).then(|| (text.clone(), w))
        });
        let forward = l.measurement.dt() > 0;
        let tip = if forward { l.x1 } else { l.x0 };
        (x0, x1, tip, forward, label)
    });
    let bg = t.panel.bg;
    let tag = measure.tag;
    p.scene.clipped(time_band, |scene| {
        if let Some((x0, x1, tip, forward, label)) = line
            && x1 > x0
        {
            let (x0, x1) = (snap(x0), snap(x1));
            let h = if hovered { 2.0 } else { 1.0 };
            scene.fill(Rect::new(point(x0, cy), size(x1 - x0, h)), accent);
            if x1 - x0 > z(2.0 * ARROW) {
                let dir = if forward { 1.0 } else { -1.0 };
                let tip = snap(tip);
                let back = tip - dir * z(ARROW);
                let half = z(ARROW) / 2.0;
                scene.lines(
                    vec![
                        [point(tip, cy + 0.5), point(back, cy + 0.5 - half)],
                        [point(tip, cy + 0.5), point(back, cy + 0.5 + half)],
                    ],
                    accent,
                    1.0,
                );
            }
            if let Some((text, w)) = label {
                let left = snap((x0 + x1 - w) / 2.0);
                scene.fill(
                    Rect::new(point(left - z(4.0), cy - z(6.0)), size(w + z(8.0), z(13.0))),
                    bg,
                );
                scene.text(
                    point(left, band.top()),
                    band.height() - 1.0,
                    text,
                    FontRole::Mono,
                    t.ui_size_small,
                    accent,
                );
            }
        }
        scene.quad(tag, bg, z(3.0), 1.0, accent);
        scene.text(
            point(tag.left() + z(4.5), tag.top()),
            tag.height(),
            "R",
            FontRole::UiSemibold,
            t.ui_size_small,
            accent,
        );
    });
}

/// The live span's value cell: its cycles in the panel's selected clock,
/// else its time.
fn live_value(m: &Measurement, selected: Option<usize>, base: TimeBase<'_>) -> String {
    match selected.and_then(|i| m.clocks.get(i)) {
        Some(c) => format!("{} cyc", c.cycles),
        None => signed_time(m.dt(), base),
    }
}

/// A tooltip on the Measure lane: the live span's full measurement, as on
/// a span's tooltip; where the `R` tag is; or what the `×` does. Each names
/// `⇧R`, which clears the reference.
fn measure_tooltip(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    hit: LaneHit,
    measure: &MeasureLane,
    doc: &Document,
    pointer: Point,
) {
    let base = doc.time_base();
    let (header, lines) = match (hit, &measure.live) {
        (LaneHit::Live, Some(live)) => {
            let m = &live.measurement;
            let mut lines = vec![("Time".to_owned(), signed_time(m.dt(), base))];
            lines.extend(
                m.clocks
                    .iter()
                    .map(|c| (c.name.clone(), format!("{} cycles", c.cycles))),
            );
            if let Some(f) = clock::frequency(m.dt().unsigned_abs() as f64, base) {
                lines.push(("1/Δt".into(), f));
            }
            ("R → cursor · double-click to zoom · ⇧R clears", lines)
        }
        (LaneHit::Tag, _) => {
            let name = match doc.reference() {
                Some(crate::marker::Reference::Marker(id)) => {
                    match doc.markers().iter().find(|m| m.id == id) {
                        Some(Marker {
                            label: Some(label), ..
                        }) => format!("Marker {id} {label}"),
                        _ => format!("Marker {id}"),
                    }
                }
                _ => "Time".into(),
            };
            (
                "Reference · ⇧R clears",
                vec![(name, format_time(measure.reference as f64, base))],
            )
        }
        _ => ("Clear reference · ⇧R", Vec::new()),
    };
    tooltip(p, column, header.into(), lines, pointer);
}

/// Each span as a dimension line with end stops where its markers are in
/// view, and the longest of its labels the font fits centred on the
/// visible part, over a gap in the line.
fn spans_paint(p: &mut TextPainter<'_>, lane: &MarkerLane, clip: Rect, hovered: Option<usize>) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let cy = snap(lane.band.top() + (lane.band.height() - 1.0) / 2.0);
    let stop = z(3.0).round();
    let mut marks = Vec::new();
    for (i, span) in lane.spans.iter().enumerate() {
        let (x0, x1) = lane.shown(span);
        let color = if hovered == Some(i) {
            t.panel.text
        } else {
            t.panel.text_placeholder
        };
        let room = x1 - x0 - z(2.0 * SPAN_LABEL_PAD);
        let label = span.labels.iter().find_map(|text| {
            let w = p.width(text, FontRole::Mono, t.ui_size_small);
            (w <= room).then(|| (text.clone(), w))
        });
        marks.push((span, x0, x1, color, label));
    }
    let bg = t.panel.bg;
    let text_color = |c: Color| {
        if c == t.panel.text {
            c
        } else {
            t.panel.text_muted
        }
    };
    let line_h = lane.band.height() - 1.0;
    let band_top = lane.band.top();
    p.scene.clipped(clip, |scene| {
        for (span, x0, x1, color, label) in marks {
            let (x0, x1) = (snap(x0), snap(x1));
            scene.fill(Rect::new(point(x0, cy), size(x1 - x0, 1.0)), color);
            for x in [span.x0, span.x1 - 1.0] {
                let x = snap(x);
                if x >= x0 - 1.0 && x <= x1 {
                    scene.fill(
                        Rect::new(point(x, cy - stop), size(1.0, 2.0 * stop + 1.0)),
                        color,
                    );
                }
            }
            if let Some((text, w)) = label {
                let left = snap((x0 + x1 - w) / 2.0);
                scene.fill(
                    Rect::new(point(left - z(4.0), cy - z(6.0)), size(w + z(8.0), z(13.0))),
                    bg,
                );
                scene.text(
                    point(left, band_top),
                    line_h,
                    text,
                    FontRole::Mono,
                    t.ui_size_small,
                    text_color(color),
                );
            }
        }
    });
}

/// A hovered span's full measurement: the time, each ruler clock's cycles,
/// and 1/Δt, the frequency of an event with that period.
fn span_tooltip(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    span: &SpanMark,
    markers: &[Marker],
    doc: &Document,
    pointer: Point,
) {
    let base = doc.time_base();
    let m = &span.measurement;
    let name = |m: &Marker| match &m.label {
        Some(label) => format!("{} {label}", m.id),
        None => m.id.to_string(),
    };
    let (Some(a), Some(b)) = (markers.get(span.first), markers.get(span.first + 1)) else {
        return;
    };
    let header = format!("{} → {} · double-click to zoom", name(a), name(b));
    let mut lines = vec![("Time".to_owned(), format_time(m.dt() as f64, base))];
    lines.extend(
        m.clocks
            .iter()
            .map(|c| (c.name.clone(), format!("{} cycles", c.cycles))),
    );
    if let Some(f) = clock::frequency(m.dt() as f64, base) {
        lines.push(("1/Δt".into(), f));
    }
    tooltip(p, column, header, lines, pointer);
}

/// `text` as measured, or shortened with `…` until it fits `max` pixels:
/// the layout's width estimates can fall short of the real font.
fn fit_chip_text(p: &mut TextPainter<'_>, text: &str, max: f32) -> (String, f32) {
    let size = p.theme.ui_size_small;
    let w = p.width(text, FontRole::UiSemibold, size);
    if w <= max {
        return (text.to_owned(), w);
    }
    let base = text.strip_suffix('…').unwrap_or(text);
    let cuts: Vec<usize> = base.char_indices().map(|(i, _)| i).collect();
    for &cut in cuts.iter().rev() {
        let shorter = format!("{}…", base[..cut].trim_end());
        let w = p.width(&shorter, FontRole::UiSemibold, size);
        if w <= max {
            return (shorter, w);
        }
    }
    (text.to_owned(), w)
}

/// The markers under a hovered chip, each with its full name and time.
fn marker_tooltip(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    header: String,
    markers: &[Marker],
    doc: &Document,
    pointer: Point,
) {
    let base = doc.time_base();
    let name = |m: &Marker| match (&m.label, markers.len()) {
        (Some(label), 1) => label.clone(),
        (Some(label), _) => format!("{} {label}", m.id),
        (None, 1) => "No name".into(),
        (None, _) => format!("Marker {}", m.id),
    };
    let mut lines: Vec<(String, String)> = markers
        .iter()
        .take(CLUSTER_LIST)
        .map(|m| (name(m), format_time(m.time as f64, base)))
        .collect();
    if markers.len() > CLUSTER_LIST {
        lines.push((
            format!("+{} more", markers.len() - CLUSTER_LIST),
            String::new(),
        ));
    }
    tooltip(p, column, header, lines, pointer);
}

/// A tooltip below the Markers lane near `pointer`: a muted header, then
/// rows of a name and a monospace value.
fn tooltip(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    header: String,
    lines: Vec<(String, String)>,
    pointer: Point,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let line_h = z(18.0);
    let mut name_w: f32 = 0.0;
    let mut time_w: f32 = 0.0;
    for (name, time) in &lines {
        name_w = name_w.max(p.width(name, FontRole::Ui, t.ui_size_small));
        time_w = time_w.max(p.width(time, FontRole::Mono, t.ui_size_small));
    }
    let header_w = p.width(&header, FontRole::Ui, t.ui_size_small);
    let w = (name_w + time_w + z(16.0)).max(header_w) + z(14.0);
    let h = line_h * (lines.len() + 1) as f32 + z(8.0);
    let clip = Rect::new(
        point(column.area.left(), column.lane.top()),
        size(
            column.area.width(),
            column.area.bottom() - column.lane.top(),
        ),
    );
    let x = (pointer.x + z(14.0))
        .min(clip.right() - w - z(4.0))
        .max(clip.left());
    let top = (column.lane.bottom() + z(4.0))
        .min(clip.bottom() - h)
        .max(clip.top());
    let tip = Rect::from_xywh(snap(x), snap(top), w, h);
    let time_x = tip.left() + z(7.0) + name_w + z(16.0);
    p.scene.clipped(clip, |scene| {
        scene.quad(tip, t.tooltip.bg, z(4.0), 1.0, t.border);
        let left = tip.left() + z(7.0);
        let mut y = tip.top() + z(4.0);
        scene.text(
            point(left, y),
            line_h,
            header,
            FontRole::Ui,
            t.ui_size_small,
            t.editor.text_placeholder,
        );
        for (name, time) in lines {
            y += line_h;
            scene.text(
                point(left, y),
                line_h,
                name,
                FontRole::Ui,
                t.ui_size_small,
                t.tooltip.text,
            );
            scene.text(
                point(time_x, y),
                line_h,
                time,
                FontRole::Mono,
                t.ui_size_small,
                t.tooltip.text,
            );
        }
    });
}

/// The cursor line from just above the header's bottom edge to the bottom
/// of the column, and its time chip in the header. `right_inset` keeps the
/// chip clear of a scrollbar.
pub fn cursor(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    cursor: Option<u64>,
    base: TimeBase<'_>,
    focused: bool,
    right_inset: f32,
) {
    let Some(c) = cursor else { return };
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let header = column.header;
    let area = column.area;
    let wave_wf = column.width_f64();
    let xf = column.viewport.x_of(c as f64, wave_wf);
    if xf < -1.0 || xf > wave_wf + 1.0 {
        return;
    }
    let x = snap(area.left() + xf as f32);
    let label = format_time(c as f64, base);
    let label_w = p.width(&label, FontRole::Mono, t.ui_size_small);
    let chip_w = label_w + z(10.0);
    let mut cx0 = x + 1.0;
    if cx0 + chip_w > header.right() - right_inset {
        cx0 = x - chip_w;
    }
    let chip = Rect::new(point(cx0, header.bottom() - z(18.0)), size(chip_w, z(16.0)));
    let clip = Rect::new(
        point(area.left(), header.top()),
        size(area.width(), area.bottom() - header.top()),
    );
    p.scene.clipped(clip, |scene| {
        scene.fill(
            Rect::new(
                point(x, header.bottom() - z(8.0)),
                size(1.0, area.bottom() - header.bottom() + z(8.0)),
            ),
            if focused {
                t.wave_cursor
            } else {
                t.wave_cursor_inactive
            },
        );
        scene.quad(chip, t.wave_cursor, z(3.0), 0.0, Color::TRANSPARENT);
        scene.text(
            point(chip.left() + z(5.0), chip.top()),
            chip.height(),
            label,
            FontRole::Mono,
            t.ui_size_small,
            t.wave_cursor_text,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(markers: &[Marker], viewport: Viewport, zoom: f32) -> MarkerLane {
        let (core, bus) = clocks();
        lane_in(markers, viewport, zoom, &[&core, &bus], Some(0))
    }

    fn lane_in(
        markers: &[Marker],
        viewport: Viewport,
        zoom: f32,
        rulers: &[&Clock],
        selected: Option<usize>,
    ) -> MarkerLane {
        let band = Rect::from_xywh(0.0, 50.0, 900.0, LANE_H * zoom);
        let clocks = SpanClocks {
            rulers,
            selected,
            base: TimeBase::si(-9),
        };
        marker_lane(band, 200.0, 700.0, viewport, markers, clocks, None, zoom)
    }

    fn clock(name: &str, stretches: &[(u64, u64, u64)]) -> Clock {
        let timeline = vtr::ClockTimeline::new(stretches.to_vec(), false).unwrap();
        Clock {
            trace: crate::trace::TraceId::A,
            track: crate::data::transactions::TrackRef(0),
            path: format!("top.{name}"),
            name: name.into(),
            state: clock::ClockState::Ready(std::sync::Arc::new(timeline)),
        }
    }

    /// A 100 MHz core clock and a 40 MHz bus clock, in nanoseconds.
    fn clocks() -> (Clock, Clock) {
        (
            clock("core_clk", &[(0, 200_000, 10)]),
            clock("bus_clk", &[(5, 200_005, 25)]),
        )
    }

    fn at(times: &[u64]) -> Vec<Marker> {
        times
            .iter()
            .enumerate()
            .map(|(i, &time)| Marker {
                id: crate::marker::MarkerId::new(i as u32 + 1).unwrap(),
                time,
                label: None,
            })
            .collect()
    }

    #[test]
    fn every_visible_marker_is_in_exactly_one_chip_and_chips_never_overlap() {
        let mut seed = 7u64;
        let mut rnd = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for trial in 0..300 {
            let mut times: Vec<u64> = (0..1 + rnd(80)).map(|_| rnd(100_000)).collect();
            times.sort_unstable();
            times.dedup();
            let mut markers = at(&times);
            for m in &mut markers {
                m.label = match rnd(3) {
                    0 => None,
                    1 => Some("irq".into()),
                    _ => Some("the first beat of the read burst".into()),
                };
            }
            let start = rnd(80_000) as f64;
            let viewport = Viewport {
                start,
                end: start + 50.0 + rnd(100_000) as f64,
            };
            let zoom = [1.0, 1.5, 2.0][trial % 3];
            let l = lane(&markers, viewport, zoom);
            let visible: Vec<usize> = (0..markers.len())
                .filter(|&i| {
                    let t = markers[i].time as f64;
                    t >= viewport.start && t <= viewport.end
                })
                .collect();
            let covered: Vec<usize> = l.chips.iter().flat_map(|c| c.markers.clone()).collect();
            assert_eq!(covered, visible, "trial {trial}");
            assert_eq!(
                l.visible,
                visible
                    .first()
                    .map_or(l.visible.start..l.visible.start, |&a| a..visible
                        .last()
                        .unwrap()
                        + 1)
            );
            for pair in l.chips.windows(2) {
                assert!(
                    pair[0].rect.right() + CHIP_GAP * zoom <= pair[1].rect.left(),
                    "trial {trial}: {pair:?}"
                );
            }
            for c in &l.chips {
                assert!(l.band.contains(c.rect.origin) && c.rect.bottom() <= l.band.bottom());
                // Only a name must fit the lane; a number chip at its end may overhang.
                assert!(
                    c.rect.right() <= l.band.right() + 0.01 || !c.text.contains(' '),
                    "trial {trial}: {c:?}"
                );
                if c.is_cluster() {
                    assert_eq!(c.text, format!("…{}", c.markers.len()));
                } else {
                    let m = &markers[c.markers.start];
                    let number = m.id.to_string();
                    let name = c.text.strip_prefix(&number).unwrap();
                    assert!(name.is_empty() || m.label.is_some(), "trial {trial}");
                    if let Some(name) = name.strip_prefix(' ') {
                        let label = m.label.as_deref().unwrap();
                        let shown = name.strip_suffix('…').unwrap_or(name);
                        assert!(
                            label.starts_with(shown) && shown.chars().count() >= NAME_MIN_CHARS
                        );
                    }
                }
            }
            // Spans join neighbours, stay clear of every chip, and carry
            // label forms longest first whose estimate fits the line.
            for s in &l.spans {
                let (a, b) = (&markers[s.first], &markers[s.first + 1]);
                assert!(a.time < b.time, "trial {trial}");
                assert_eq!(s.measurement.dt(), i128::from(b.time - a.time));
                let (x0, x1) = l.shown(s);
                assert!(x1 - x0 >= SPAN_MIN * zoom - 0.01, "trial {trial}: {s:?}");
                for c in &l.chips {
                    assert!(
                        c.rect.right() <= x0 + 0.01 || c.rect.left() >= x1 - 0.01,
                        "trial {trial}: {c:?} {s:?}"
                    );
                }
                let forms = span_forms(&s.measurement, Some(0), TimeBase::si(-9));
                assert!(s.labels.iter().all(|f| forms.contains(f)), "trial {trial}");
                for f in &s.labels {
                    assert!(
                        span_text_w(f) + 2.0 * SPAN_LABEL_PAD <= (x1 - x0) / zoom + 0.01,
                        "trial {trial}: {f} in {}",
                        x1 - x0
                    );
                }
                assert!(s.labels.windows(2).all(|w| w[0].len() > w[1].len()));
            }
            assert!(l.spans.windows(2).all(|w| w[0].first < w[1].first));
            // Every gap wide enough for a line has its span, including the
            // way to a marker beyond either edge.
            let mut ends: Vec<(usize, f32, f32)> = l
                .chips
                .iter()
                .map(|c| (c.markers.end - 1, c.rect.left(), c.rect.right()))
                .collect();
            let x_of = |t: u64| 200.0 + viewport.x_of(t as f64, 700.0) as f32;
            if let Some(ix) = l.visible.start.checked_sub(1) {
                ends.insert(0, (ix, x_of(markers[ix].time), x_of(markers[ix].time)));
            }
            if let Some(m) = markers.get(l.visible.end) {
                ends.push((l.visible.end, x_of(m.time), x_of(m.time)));
            }
            for w in ends.windows(2) {
                let room = (w[1].1 - SPAN_INSET * zoom).min(900.0)
                    - (w[0].2 + SPAN_INSET * zoom).max(200.0);
                if room >= SPAN_MIN * zoom + 0.01 {
                    assert!(
                        l.spans.iter().any(|s| s.first == w[0].0),
                        "trial {trial}: no span after marker {}",
                        w[0].0
                    );
                }
            }
        }
    }

    #[test]
    fn spans_take_the_longest_label_that_fits_and_reach_markers_off_the_view() {
        let (core, bus) = clocks();
        // One marker left of the view, three in it, one right of it.
        let markers = at(&[10, 100, 400, 2000, 9000]);
        let view = Viewport {
            start: 40.0,
            end: 3000.0,
        };
        let l = lane_in(&markers, view, 1.0, &[&core, &bus], Some(0));
        let firsts: Vec<usize> = l.spans.iter().map(|s| s.first).collect();
        assert_eq!(firsts, [0, 1, 2, 3]);
        // The outer spans run off the lane toward their markers.
        assert!(l.spans[0].x0 < l.time_left);
        assert!(l.spans[3].x1 > l.band.right());
        let text = |s: &SpanMark| s.labels.first().cloned().unwrap_or_default();
        // Too short to label; the time alone; everything.
        assert_eq!(text(&l.spans[0]), "");
        assert_eq!(text(&l.spans[1]), "300 ns");
        assert_eq!(text(&l.spans[2]), "1.6 µs · 160 core_clk · 64.0 bus_clk");
        // Only the visible part of a span running off the lane holds its label.
        assert_eq!(text(&l.spans[3]), "700 core_clk · 7 µs");
        // Narrower, the long span drops the other clock, then the time.
        let narrow = |end: f64, selected| {
            let l = lane_in(
                &markers,
                Viewport { start: 40.0, end },
                1.0,
                &[&core, &bus],
                selected,
            );
            text(l.spans.iter().find(|s| s.first == 2).unwrap())
        };
        let forms: Vec<String> = [3000.0, 6000.0, 9000.0, 14_000.0]
            .iter()
            .map(|&end| narrow(end, Some(1)))
            .collect();
        assert_eq!(
            forms,
            [
                "1.6 µs · 160 core_clk · 64.0 bus_clk",
                "64.0 bus_clk · 1.6 µs",
                "64.0 bus_clk",
                "1.6 µs"
            ]
        );
        // Without clocks the label is the time.
        let l = lane_in(&markers, view, 1.0, &[], None);
        assert_eq!(text(&l.spans[2]), "1.6 µs");
    }

    #[test]
    fn a_span_is_hit_between_its_chips_and_a_chip_wins_over_it() {
        let markers = at(&[100, 400, 2000]);
        let l = lane(
            &markers,
            Viewport {
                start: 0.0,
                end: 3000.0,
            },
            1.0,
        );
        let y = l.band.top() + l.band.height() / 2.0;
        let (x0, x1) = l.shown(&l.spans[1]);
        assert_eq!(l.hit(point((x0 + x1) / 2.0, y)), Some(LaneHit::Span(1)));
        assert_eq!(l.hit(point(x1 + 3.0, y)), Some(LaneHit::Chip(2)));
        assert_eq!(l.hit(point((x0 + x1) / 2.0, l.band.bottom() + 1.0)), None);
        // Past the last marker there is no span.
        assert_eq!(l.hit(point(l.band.right() - 1.0, y)), None);
    }

    #[test]
    fn chips_grow_to_names_that_fit_cut_those_that_do_not_or_show_numbers() {
        let named = |names: &[(u64, &str)]| -> Vec<Marker> {
            let mut markers = at(&names.iter().map(|(t, _)| *t).collect::<Vec<_>>());
            for (m, (_, name)) in markers.iter_mut().zip(names) {
                m.label = (!name.is_empty()).then(|| name.to_string());
            }
            markers
        };
        let view = Viewport {
            start: 0.0,
            end: 7000.0,
        };
        // 100 time units are 10 px here.
        let texts = |markers: &[Marker]| -> Vec<String> {
            lane(markers, view, 1.0)
                .chips
                .into_iter()
                .map(|c| c.text)
                .collect()
        };
        let m = named(&[
            (0, "req A"),
            (2000, "response"),
            (2600, "irq"),
            (6800, "tail"),
        ]);
        // Marker 4's name would run past the lane's end.
        assert_eq!(texts(&m), ["1 req A", "2 resp…", "3 irq", "4"]);
        // Too little room for three letters: the number alone.
        let m = named(&[(0, "request"), (300, "")]);
        assert_eq!(texts(&m), ["1", "2"]);
        // A name never pushes a neighbour: the next chip keeps its place.
        let m = named(&[(0, "a very long name indeed"), (1000, "")]);
        let l = lane(&m, view, 1.0);
        assert!(l.chips[0].rect.right() + CHIP_GAP <= l.chips[1].rect.left());
        assert_eq!(l.chips[1].rect.left(), 200.0 + 100.0);
        // Names follow the interface zoom like numbers.
        let m = named(&[(0, "req A"), (3000, "")]);
        let big = lane(&m, view, 2.0);
        assert_eq!(big.chips[0].text, "1 req A");
        assert_eq!(big.chips[0].rect.width(), 2.0 * chip_w("1 req A"));
    }

    #[test]
    fn a_name_field_sits_after_the_number_and_stays_in_the_lane() {
        let mut markers = at(&[1000, 6700]);
        markers[0].label = Some("req A".into());
        let view = Viewport {
            start: 0.0,
            end: 7000.0,
        };
        let l = lane(&markers, view, 1.0);
        let first = l.name_field(0, markers[0].id).unwrap();
        assert_eq!(first.left(), l.chips[0].rect.left() + chip_w("1"));
        assert_eq!(first.width(), NAME_FIELD_W);
        assert!(first.top() > l.band.top() && first.bottom() < l.band.bottom());
        let last = l.name_field(1, markers[1].id).unwrap();
        assert_eq!(last.right(), l.band.right(), "cut at the lane's end");
        let away = Viewport {
            start: 2000.0,
            end: 3000.0,
        };
        assert_eq!(lane(&markers, away, 1.0).name_field(0, markers[0].id), None);
    }

    #[test]
    fn close_markers_cluster_when_zoomed_out_and_split_when_zoomed_in() {
        let markers = at(&[1000, 1010, 1020, 1030, 5000]);
        let out = lane(
            &markers,
            Viewport {
                start: 0.0,
                end: 10_000.0,
            },
            1.0,
        );
        assert_eq!(out.chips.len(), 2);
        assert_eq!(out.chips[0].markers, 0..4);
        assert_eq!(out.chips[0].text, "…4");
        assert_eq!(out.chips[1].text, "5");
        // 10 time units are 64 px apart here: room for 17 px chips, not 68 px ones.
        let near = Viewport {
            start: 990.0,
            end: 1100.0,
        };
        let texts: Vec<_> = lane(&markers, near, 1.0)
            .chips
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(texts, ["1", "2", "3", "4"]);
        // A larger interface zoom widens chips, so they merge sooner.
        let wide = lane(&markers, near, 4.0);
        assert!(wide.chips.len() < 4);
    }

    #[test]
    fn a_chip_is_grabbed_a_little_beyond_its_flag_and_hits_its_markers() {
        let markers = at(&[1000, 1010, 1020, 5000]);
        let l = lane(
            &markers,
            Viewport {
                start: 0.0,
                end: 10_000.0,
            },
            1.0,
        );
        let y = l.band.top() + LANE_H / 2.0;
        let one = &l.chips[1];
        assert_eq!(
            l.hit(point(one.rect.left() - 2.0, y)),
            Some(LaneHit::Chip(3))
        );
        assert_eq!(
            l.hit(point(one.rect.right() + 1.0, y)),
            Some(LaneHit::Chip(3))
        );
        assert_eq!(l.hit(point(one.rect.right() + 6.0, y)), None);
        assert_eq!(
            l.hit(point(l.chips[0].rect.left() + 1.0, y)),
            Some(LaneHit::Cluster(0..3))
        );
        assert_eq!(l.hit(point(one.rect.left(), l.band.bottom() + 1.0)), None);
        assert_eq!(
            l.hit(point(150.0, y)),
            None,
            "the title cells hold no chips"
        );
    }
}
