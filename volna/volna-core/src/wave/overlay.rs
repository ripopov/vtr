//! What every timed panel paints over its time column: the tick grid, the
//! header's tick labels and unit, the clock rulers under it, the Markers
//! lane with its chips and marker lines, and the cursor line with its time
//! or cycle chip. The wave and pipeline painters call these with their own
//! rectangles so both agree pixel for pixel.

use std::ops::Range;

use crate::clock::{self, ClockView, Clocks};
use crate::color::Color;
use crate::document::Document;
use crate::geometry::{CursorIcon, Point, Rect, point, size, snap};
use crate::marker::{LaneHit, Marker};
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
/// (empty without rulers), the Markers lane and the rows area below them.
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

/// The laid-out Markers lane of one panel. Pure: computed from times and
/// pixel geometry, then read by the painter and the input handlers alike.
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

    /// What `p` is over on the lane.
    pub fn hit(&self, p: Point) -> Option<LaneHit> {
        let chip = &self.chips[self.chip_at(p)?];
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
/// one, where the gap to the next chip or the lane's end allows.
pub fn marker_lane(
    band: Rect,
    time_left: f32,
    width_px: f64,
    viewport: Viewport,
    markers: &[Marker],
    zoom: f32,
) -> MarkerLane {
    let z = |v: f32| v * zoom;
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
    MarkerLane {
        band,
        time_left,
        visible: first..last,
        chips,
        zoom,
    }
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
/// `value` when the panel has a values column, the chips, and a line for
/// every visible marker from its chip down through the rows. Hovering a
/// cluster lists its markers.
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

    let hovered = pointer.and_then(|mp| lane.chip_at(mp));
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
    p.scene.clipped(lines_clip, |scene| {
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
    let mut chips = Vec::new();
    for (i, chip) in lane.chips.iter().enumerate() {
        let hover = hovered == Some(i);
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
        chips.push((chip.rect, text, w, bg, fg));
    }
    p.scene.clipped(time_band, |scene| {
        for (rect, text, w, bg, fg) in chips {
            // One quad: marker backgrounds may be translucent, so parts must not overlap.
            scene.quad(rect, bg, z(3.0), 0.0, Color::TRANSPARENT);
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
    if let (Some(i), Some(mp)) = (hovered, pointer) {
        let under = &markers[lane.chips[i].markers.clone()];
        let header = match under {
            [one] => format!("Marker {}", one.id),
            many => format!("{} markers · click to zoom in", many.len()),
        };
        marker_tooltip(p, column, header, under, doc, mp);
    }
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
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
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
        let band = Rect::from_xywh(0.0, 50.0, 900.0, LANE_H * zoom);
        marker_lane(band, 200.0, 700.0, viewport, markers, zoom)
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
        }
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
