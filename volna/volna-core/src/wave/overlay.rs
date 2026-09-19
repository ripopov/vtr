//! What every timed panel paints over its time column: the tick grid, the
//! header's tick labels and unit, marker lines with their chips, and the
//! cursor line with its time chip. The wave and pipeline painters call these
//! with their own rectangles so both agree pixel for pixel.

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

/// The time column of a panel: its header cell and the rows area below it.
#[derive(Clone, Copy, Debug)]
pub struct TimeColumn {
    pub header: Rect,
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
