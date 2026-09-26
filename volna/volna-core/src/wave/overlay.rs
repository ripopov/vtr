//! What every timed panel paints over its time column: the tick grid, the
//! header's tick labels and unit, the clock rulers under it, marker lines
//! with their chips, and the cursor line with its time or cycle chip. The
//! wave and pipeline painters call these with their own rectangles so both
//! agree pixel for pixel.

use crate::clock::{self, ClockView, Clocks};
use crate::color::Color;
use crate::document::{Document, Marker};
use crate::geometry::{CursorIcon, Point, Rect, point, size, snap};
use crate::scene::{FontRole, Scene, TextCache, TextMeasure};
use crate::theme::Theme;
use crate::wave::layout::SCROLLBAR_W;
use crate::wave::timeline::{Tick, TimeBase, format_time, ticks};
use crate::wave::viewport::Viewport;

/// Pixel constants are design sizes at zoom 1.0; painters multiply them by
/// the theme's zoom. Hairlines stay one pixel.
pub const TICK_SPACING_PX: f64 = 96.0;
/// Width of a marker chip at zoom 1.0.
pub const CHIP_W: f32 = 24.0;
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
/// (empty without rulers) and the rows area below them.
#[derive(Clone, Copy, Debug)]
pub struct TimeColumn {
    pub header: Rect,
    pub rulers: Rect,
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

/// Marker chip rectangle for a marker line at panel x.
pub fn marker_chip_bounds(header: Rect, x: f32, zoom: f32) -> Rect {
    Rect::new(
        point(x + 1.0, header.top() + 2.0 * zoom),
        size(CHIP_W * zoom, 14.0 * zoom),
    )
}

/// Chips of the markers inside the viewport, in marker order.
pub fn marker_chips(
    header: Rect,
    area_left: f32,
    width_px: f64,
    viewport: Viewport,
    markers: &[Marker],
    zoom: f32,
) -> Vec<(usize, Rect)> {
    let mut chips = Vec::new();
    for (ix, m) in markers.iter().enumerate() {
        let x = viewport.x_of(m.time as f64, width_px);
        if x < 0.0 || x > width_px {
            continue;
        }
        let xp = snap(area_left + x as f32);
        chips.push((ix, marker_chip_bounds(header, xp, zoom)));
    }
    chips
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

/// Marker lines down the rows area and their chips in the header.
pub fn markers(
    p: &mut TextPainter<'_>,
    column: &TimeColumn,
    doc: &Document,
    chips: &[(usize, Rect)],
    pointer: Option<Point>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let area = column.area;
    for (ix, chip) in chips {
        let m = &doc.markers[*ix];
        let x = column.x_of(m.time as f64);
        let marker = t.marker(m.id.saturating_sub(1) as usize);
        let color = marker.stroke;
        p.scene.clipped(area, |scene| {
            scene.fill(
                Rect::new(point(x, area.top()), size(1.0, area.height())),
                color,
            );
        });
        let hovered = pointer.is_some_and(|mp| chip.contains(mp));
        let bg = if hovered {
            marker.hover
        } else {
            marker.background
        };
        p.scene.quad(*chip, bg, z(3.0), 0.0, Color::TRANSPARENT);
        let label = format!("M{}", m.id);
        let w = p.width(&label, FontRole::UiSemibold, t.ui_size_small);
        p.scene.text(
            point(snap(chip.left() + (chip.width() - w) / 2.0), chip.top()),
            chip.height(),
            label,
            FontRole::UiSemibold,
            t.ui_size_small,
            if hovered {
                marker.hover_text
            } else {
                marker.text
            },
        );
        p.scene.cursors.push((*chip, CursorIcon::PointingHand));
    }
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
