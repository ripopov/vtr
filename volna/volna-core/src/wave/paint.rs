//! Paint the wave panel into a [`Scene`]: the three aligned columns (names,
//! values, waves) plus the timeline header, cursor, markers and a scrollbar.
//!
//! Waveforms are sampled per pixel column: for every column the painter asks
//! the history for the index of the last change at the column's right edge
//! (an exponential search from the previous column's index), so the cost of a
//! frame is O(columns x log(changes)) regardless of trace length.

use crate::color::Color;
use crate::data::transactions::TxStatus;
use crate::data::{Bit, SignalHistory, SignalShape, Translator, ValueKind, WaveValue};
use crate::document::Document;
use crate::geometry::CursorIcon;
use crate::geometry::{Point, Rect, point, size, snap};
use crate::icons::IconName;
use crate::pipeline::PipelineModel;
use crate::scene::{FontRole, Scene, TextCache, TextMeasure};
use crate::theme::Theme;
use crate::trace::TraceId;
use crate::wave::analog::{self, Analog, AnalogDraw, Plot, Readout, Series};
use crate::wave::group;
use crate::wave::lane::{self, LaneData, LaneGeometry, TxLane};
use crate::wave::layout::{CHEVRON_W, SCROLLBAR_W, WaveLayout, indent_x};
use crate::wave::marks;
use crate::wave::model::{
    DisplayedSignal, Drag, GroupRow, RowSource, WaveModel, WaveRow, ZOOM_RANGE_MIN_PX,
};
use crate::wave::overlay::{self, TextPainter, TimeColumn};
use crate::wave::timeline::format_time;
use crate::wave::tree;
use crate::wave::viewport::Viewport;

// Pixel constants are design sizes at zoom 1.0; the painter multiplies them
// by the theme's zoom. Hairlines (1 px strokes and borders) stay one pixel.
/// Vertical inset of the trace inside a row.
const TRACE_PAD: f32 = 5.0;
/// Segments narrower than this are drawn as a dense band instead of a hexagon.
const MIN_SEGMENT_PX: usize = 5;

/// A trace's letter, centred in `cell` on its badge colour, clipped to `clip`.
pub(crate) fn trace_letter(
    p: &mut TextPainter<'_>,
    t: &Theme,
    trace: TraceId,
    cell: Rect,
    clip: Rect,
) {
    let z = |v: f32| v * t.zoom;
    let w = z(14.0);
    let h = (cell.height() - z(6.0)).clamp(z(10.0), z(16.0));
    let badge = Rect::from_xywh(
        snap(cell.left() + (cell.width() - w) / 2.0),
        snap(cell.top() + (cell.height() - h) / 2.0),
        w,
        h,
    );
    let colors = t.trace(trace);
    let letter = trace.letter().to_string();
    let letter_w = p.width(&letter, FontRole::UiMedium, t.ui_size_small);
    p.scene.clipped(clip, |scene| {
        scene.quad(badge, colors.background, z(3.0), 0.0, Color::TRANSPARENT);
        scene.text(
            point(snap(badge.left() + (w - letter_w) / 2.0), badge.top()),
            badge.height(),
            letter,
            FontRole::UiMedium,
            t.ui_size_small,
            colors.text,
        );
    });
}

/// Truncate `text` to at most `max_chars` characters, appending an ellipsis.
fn truncate_chars(text: &str, max_chars: usize) -> Option<String> {
    let n = text.chars().count();
    if n <= max_chars {
        return Some(text.to_string());
    }
    if max_chars < 2 {
        return None;
    }
    let mut s: String = text.chars().take(max_chars - 1).collect();
    s.push('…');
    Some(s)
}

/// `text` cut to fit `max_w` in `font`, ending in an ellipsis when cut;
/// `None` when not even one character and the ellipsis fit.
fn fit_width(
    p: &mut TextPainter<'_>,
    text: &str,
    font: FontRole,
    size: f32,
    max_w: f32,
) -> Option<String> {
    if p.width(text, font, size) <= max_w {
        return (!text.is_empty()).then(|| text.to_owned());
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |n: usize| {
        chars[..n]
            .iter()
            .chain(std::iter::once(&'…'))
            .collect::<String>()
    };
    // The longest prefix that fits, by bisection over its length.
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if p.width(&cut(mid), font, size) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (lo > 0).then(|| cut(lo))
}

fn changes_between(a: Option<usize>, b: Option<usize>) -> usize {
    match (a, b) {
        (None, None) | (Some(_), None) => 0,
        (None, Some(j)) => j + 1,
        (Some(i), Some(j)) => j.saturating_sub(i),
    }
}

/// Paint `model` using the layout from its last [`WaveModel::layout`] call.
pub fn paint(
    model: &WaveModel,
    doc: &Document,
    theme: &Theme,
    text: &mut TextCache,
    measure: &mut dyn TextMeasure,
    scene: &mut Scene,
    focused: bool,
) {
    let layout = model.last_layout().clone();
    let bounds = layout.bounds;
    let t = theme;
    let z = |v: f32| v * t.zoom;
    let char_w = text.width(measure, "0", FontRole::Mono, t.mono_size);
    let mut p = TextPainter {
        theme,
        text,
        measure,
        scene,
    };
    let viewport = model.viewport(doc);
    let cursor = model.cursor(doc);
    let base = doc.time_base();
    let waves = layout.waves;
    let column = TimeColumn {
        header: Rect::new(
            point(waves.left(), layout.header.top()),
            size(waves.width(), layout.header.height()),
        ),
        rulers: Rect::new(
            point(waves.left(), layout.rulers.top()),
            size(waves.width(), layout.rulers.height()),
        ),
        lane: Rect::new(
            point(waves.left(), layout.marker_lane.strip().top()),
            size(waves.width(), layout.marker_lane.strip().height()),
        ),
        area: waves,
        viewport,
    };
    let clocks = model.nav.clocks();

    // -- backgrounds and chrome ------------------------------------------
    p.scene.fill(bounds, t.editor.bg);
    p.scene.fill(
        Rect::new(
            layout.names.origin,
            size(
                layout.names.width() + layout.values.width(),
                layout.names.height(),
            ),
        ),
        t.panel.bg,
    );
    p.scene.fill(layout.header, t.panel.bg);

    // -- tick grid in the waves area ---------------------------------------
    let (tick_list, unit) = column.ticks(base, t.zoom);
    overlay::grid(&mut p, &column, &tick_list);
    overlay::cycle_grid(&mut p, &column, clocks, &doc.clocks);
    if doc.is_loaded() {
        overlay::outside_time(&mut p, &column, doc.limits());
    }

    // -- rows ----------------------------------------------------------------
    // Text sits on a row's first line (`row_h`); tall rows give the waveform
    // their full height.
    let row_h = layout.row_h;
    let mut inks = crate::wave::tint::InkWalk::default();
    for pos in layout.rows.clone() {
        let Some(ix) = layout.entry(pos) else {
            continue;
        };
        let entry = &model.items()[ix];
        let row = &entry.row;
        // A coloured row paints with a theme whose signal ink is its colour.
        let ink = inks.ink(model.items(), ix);
        let inked;
        let t: &Theme = match ink {
            Some(_) => {
                inked = theme.inked(ink);
                &inked
            }
            None => theme,
        };
        let mut p = TextPainter {
            theme: t,
            text: &mut *p.text,
            measure: &mut *p.measure,
            scene: &mut *p.scene,
        };
        let y = layout.row_y(pos);
        let full_h = layout.row_height(pos);
        let name_x = indent_x(layout.name_left, entry.depth, t.zoom);
        let is_selected = model.selected.contains(&ix);
        let is_hover = model.hover_row == Some(ix);
        let left_row = Rect::new(
            point(bounds.left(), y),
            size(layout.names.width() + layout.values.width(), full_h),
        );
        let wave_row = Rect::new(point(waves.left(), y), size(waves.width(), full_h));
        if is_selected {
            p.scene.fill(left_row, t.selection.bg);
            p.scene.fill(wave_row, t.wave_row_selected);
            if t.appearance.is_high_contrast() {
                p.scene
                    .quad(left_row, Color::TRANSPARENT, 0.0, 1.0, t.border_focused);
                p.scene
                    .quad(wave_row, Color::TRANSPARENT, 0.0, 1.0, t.border_focused);
            }
        } else if is_hover {
            p.scene.fill(left_row, t.hover.bg);
            p.scene.fill(wave_row, t.wave_row_hover);
        }
        // One guide per enclosing group, down the rows it holds.
        for d in 0..entry.depth {
            let x = snap(indent_x(layout.name_left, d, t.zoom) + z(CHEVRON_W / 2.0));
            p.scene.clipped(layout.names, |scene| {
                scene.fill(Rect::from_xywh(x, y, 1.0, full_h), t.border_variant);
            });
        }
        if layout.name_left > layout.names.left()
            && let Some(trace) = model.row_trace(ix)
        {
            let gutter = Rect::new(
                point(layout.names.left(), y),
                size(layout.name_left - layout.names.left(), row_h),
            );
            trace_letter(&mut p, t, trace, gutter, layout.names);
        }
        // A stripe on the name marks a coloured row at a glance.
        if ink.is_some() {
            let stripe = Rect::from_xywh(layout.names.left(), y, z(3.0).max(2.0), full_h);
            p.scene
                .clipped(layout.names, |scene| scene.fill(stripe, t.wave_signal));
        }
        let colors = t.row(is_selected, is_hover);
        let cells = LaneCells {
            layout: &layout,
            pos,
            name_x,
            viewport,
            cursor,
            text: colors.text,
            muted: colors.text_placeholder,
        };
        let item = match row {
            WaveRow::Signal(item) => item,
            WaveRow::Lane(lane) => {
                paint_lane_row(lane, doc, &cells, &mut p);
                continue;
            }
            WaveRow::Clock(clock) => {
                paint_clock_row(clock, model.nav.clocks(), doc, &cells, &mut p);
                continue;
            }
            WaveRow::Group(g) => {
                // Groups are divided by a 1px border rather than zebra stripes.
                p.scene.fill(
                    Rect::from_xywh(bounds.left(), snap(y), bounds.width(), 1.0),
                    t.border_variant,
                );
                let members = model.group_histories(ix);
                let group = GroupCells {
                    row: g,
                    count: tree::leaves(model.items(), ix).count(),
                    members: &members,
                    renaming: model.rename == Some(ix),
                    reading: (model.hover_row == Some(ix)
                        && matches!(model.drag, None | Some(Drag::Cursor)))
                    .then_some(model.pointer)
                    .flatten(),
                };
                paint_group_row(&group, model, doc, &cells, char_w, &mut p);
                continue;
            }
        };

        // Name column: the leaf name in the interface face, then a muted
        // range badge in the mono face for vectors when both fit.
        {
            let pad = z(12.0);
            let avail = layout.names.right() - name_x - pad;
            let dims = item.shape.dims();
            let dims_w = if dims.is_empty() {
                0.0
            } else {
                p.width(&dims, FontRole::Mono, t.mono_size) + char_w
            };
            let name_color = if item.source.signal().is_some() {
                colors.text
            } else {
                colors.text_placeholder
            };
            let full_w = p.width(&item.name, FontRole::Ui, t.ui_size);
            let names_rect = layout.names;
            // The badge shows only beside the whole name.
            let show_dims = !dims.is_empty() && full_w + dims_w <= avail;
            if let Some(name) = fit_width(&mut p, &item.name, FontRole::Ui, t.ui_size, avail) {
                let name_w = p.width(&name, FontRole::Ui, t.ui_size);
                p.scene.clipped(names_rect, |scene| {
                    scene.text(
                        point(name_x, y),
                        row_h,
                        name,
                        FontRole::Ui,
                        t.ui_size,
                        name_color,
                    );
                    if show_dims {
                        scene.text(
                            point(name_x + name_w + char_w, y),
                            row_h,
                            dims,
                            FontRole::Mono,
                            t.mono_size,
                            colors.text_placeholder,
                        );
                    }
                });
            }
        }

        // A tall plot says how it is drawn under its name.
        if let Some(a) = &item.analog
            && full_h >= 2.0 * row_h - 0.5
        {
            let label = format!("{} · {}", a.draw.label(), a.range.label());
            p.scene.clipped(layout.names, |scene| {
                scene.text(
                    point(name_x, y + row_h - z(4.0)),
                    row_h,
                    label,
                    FontRole::Ui,
                    t.ui_size_small,
                    colors.text_placeholder,
                );
            });
        }

        // Values column: value at cursor plus the format badge.
        {
            let pad = z(8.0);
            let badge = layout
                .badges
                .iter()
                .find(|(i, _)| *i == ix)
                .map(|(_, b)| *b);
            let text_right = badge
                .map(|b| b.left() - pad)
                .unwrap_or(layout.values.right() - pad);
            let avail = text_right - layout.values.left() - pad;
            let (value_text, color) = match (&item.history, cursor) {
                (Some(h), Some(c)) => {
                    let index = h.index_at(c);
                    let value = if item.shape == SignalShape::Event {
                        WaveValue::Bits(
                            if index.is_some_and(|i| h.time(i) == c) {
                                "1"
                            } else {
                                "0"
                            }
                            .into(),
                        )
                    } else {
                        h.value(index)
                    };
                    let tr = item.translator.translate(&value);
                    let color = if tr.kind == ValueKind::Normal {
                        colors.text
                    } else {
                        colors.value_color(tr.kind)
                    };
                    (tr.text, color)
                }
                (None, _) if item.source.signal().is_none() => {
                    ("not in trace".to_string(), colors.text_placeholder)
                }
                (None, _) if item.error.is_none() => {
                    ("loading…".to_string(), colors.text_placeholder)
                }
                _ => ("–".to_string(), colors.text_placeholder),
            };
            let max_chars = (avail / char_w).floor().max(0.0) as usize;
            let values_rect = layout.values;
            // A number keeps its last digits here too, as a bus label does.
            let number = item.history.is_some() && item.translator.numeric_kind().is_some();
            if let Some(text) = fit_label(&value_text, max_chars, number) {
                p.scene.clipped(values_rect, |scene| {
                    scene.text(
                        point(values_rect.left() + pad, y),
                        row_h,
                        text,
                        FontRole::Mono,
                        t.mono_size,
                        color,
                    );
                });
            }
            if let Some(b) = badge {
                let hovered = model.badge_hover == Some(ix);
                let show = hovered || is_selected || is_hover;
                if show {
                    let bg = if hovered {
                        t.badge_hover.bg
                    } else {
                        t.badge.bg
                    };
                    let label = item.translator.badge();
                    let label_w = p.width(label, FontRole::UiMedium, t.ui_size_small);
                    let color = if hovered {
                        t.badge_hover.text
                    } else {
                        t.badge.text_muted
                    };
                    let x = b.left() + (b.width() - label_w) / 2.0;
                    p.scene.clipped(values_rect, |scene| {
                        scene.quad(b, bg, z(3.0), 1.0, t.border_variant);
                        scene.text(
                            point(snap(x), b.top()),
                            b.height(),
                            label,
                            FontRole::UiMedium,
                            t.ui_size_small,
                            color,
                        );
                    });
                }
            }
        }

        // Waves column.
        match (&item.history, &item.error) {
            (Some(h), _)
                if let Some(a) = &item.analog
                    && analog::supports(item.shape, item.translator.as_ref())
                    && let Some(kind) = item.translator.numeric_kind() =>
            {
                let series = Series::of(doc, item.source.signal(), h, kind);
                paint_analog_row(item, a, &series, &viewport, wave_row, waves, row_h, &mut p)
            }
            (Some(h), _) => match item.shape {
                SignalShape::Event => {
                    let mut counts = Vec::new();
                    p.scene.clipped(waves, |scene| {
                        counts = paint_event_row(h.as_ref(), &viewport, wave_row, t, scene)
                    });
                    paint_event_counts(&counts, wave_row, waves, &mut p);
                }
                SignalShape::Bit => p.scene.clipped(waves, |scene| {
                    paint_bit_row(h.as_ref(), &viewport, wave_row, t, scene)
                }),
                _ => paint_bus_row(
                    h.as_ref(),
                    item.translator.as_ref(),
                    &viewport,
                    wave_row,
                    waves,
                    char_w,
                    &mut p,
                ),
            },
            (None, Some(err)) => {
                let err = err.clone();
                p.scene.clipped(waves, |scene| {
                    scene.text(
                        point(waves.left() + z(8.0), y),
                        row_h,
                        err,
                        FontRole::Ui,
                        t.ui_size_small,
                        t.editor.error,
                    );
                });
            }
            (None, None) if item.source.signal().is_none() => {
                let label = match item.source {
                    RowSource::Unresolved {
                        ambiguous: true, ..
                    } => "ambiguous signal path",
                    _ => "not in trace",
                };
                p.scene.clipped(waves, |scene| {
                    scene.text(
                        point(waves.left() + z(8.0), y),
                        row_h,
                        label,
                        FontRole::Ui,
                        t.ui_size_small,
                        colors.text_placeholder,
                    )
                });
            }
            (None, None) => {
                // Loading: a thin muted bar where the trace will be.
                let bar = Rect::new(
                    point(waves.left() + z(8.0), y + full_h / 2.0),
                    size(z(96.0), 1.0),
                );
                p.scene.clipped(waves, |scene| scene.fill(bar, t.border));
            }
        }
    }

    // -- empty placeholder -------------------------------------------------
    if layout.rows.is_empty() && doc.is_loaded() {
        let area = Rect::new(
            point(bounds.left(), layout.names.top()),
            size(bounds.width(), layout.names.height()),
        );
        // The empty-state message spans the table, so give it one surface
        // instead of crossing differently themed name/value columns.
        p.scene.fill(area, t.editor.bg);
        let cx_ = area.left() + area.width() / 2.0;
        let cy = area.top() + area.height() / 2.0;
        let icon = Rect::new(point(cx_ - z(16.0), cy - z(44.0)), size(z(32.0), z(32.0)));
        p.scene
            .icon(IconName::ListTree, icon, t.editor.text_placeholder);
        let title = "No signals displayed";
        let title_w = p.width(title, FontRole::UiMedium, t.ui_size);
        p.scene.text(
            point(snap(cx_ - title_w / 2.0), cy - z(4.0)),
            z(20.0),
            title,
            FontRole::UiMedium,
            t.ui_size,
            t.editor.text_muted,
        );
        let hint = "Double-click a variable in the sidebar, or select variables and press Enter";
        let hint_w = p.width(hint, FontRole::Ui, t.ui_size_small);
        p.scene.text(
            point(snap(cx_ - hint_w / 2.0), cy + z(18.0)),
            z(18.0),
            hint,
            FontRole::Ui,
            t.ui_size_small,
            t.editor.text_placeholder,
        );
    }

    // Where dragged rows will land: a line from the level they take across
    // every column at the gap, or an outline of the folded group they join.
    if let Some(Drag::Rows {
        gap: Some(gap),
        depth,
        into,
        ..
    }) = model.drag
    {
        let rows_area = Rect::new(
            layout.names.origin,
            size(bounds.width(), layout.names.height()),
        );
        let w = z(2.0).max(2.0);
        if into.is_some() {
            let r = Rect::from_xywh(
                bounds.left() + 1.0,
                layout.row_y(gap) + 1.0,
                bounds.width() - 2.0,
                layout.row_height(gap) - 2.0,
            );
            p.scene.clipped(rows_area, |scene| {
                scene.quad(r, Color::TRANSPARENT, z(2.0), w, t.border_focused);
            });
        } else {
            let y = snap(layout.row_y(gap)).clamp(rows_area.top() + 1.0, rows_area.bottom() - 1.0);
            let x = indent_x(layout.name_left, depth, t.zoom) - z(8.0);
            p.scene.clipped(rows_area, |scene| {
                scene.fill(
                    Rect::from_xywh(x, y - w / 2.0, bounds.right() - x, w),
                    t.border_focused,
                );
                let knob = z(6.0);
                scene.quad(
                    Rect::from_xywh(x, y - knob / 2.0, knob, knob),
                    t.border_focused,
                    knob / 2.0,
                    0.0,
                    Color::TRANSPARENT,
                );
            });
        }
    }

    if let Some(Drag::ZoomRange { start, current }) = model.drag {
        let label = "Zoom to selected area · Esc to cancel";
        let label_width = p.width(label, FontRole::Ui, t.ui_size);
        let label_x = (current.x + z(8.0))
            .min(waves.right() - label_width - z(8.0))
            .max(waves.left() + z(8.0));
        p.scene.clipped(waves, |scene| {
            if (current.x - start.x).abs() >= z(ZOOM_RANGE_MIN_PX) {
                scene.fill(
                    Rect::from_xywh(
                        start.x.min(current.x),
                        waves.top(),
                        (current.x - start.x).abs(),
                        waves.height(),
                    ),
                    t.selection.bg.with_alpha(0.35),
                );
            }
            scene.lines(
                vec![
                    [point(start.x, waves.top()), point(start.x, waves.bottom())],
                    [
                        point(current.x, waves.top()),
                        point(current.x, waves.bottom()),
                    ],
                ],
                t.editor.text,
                1.0,
            );
            scene.text(
                point(label_x, (current.y + z(8.0)).min(waves.bottom() - z(20.0))),
                z(20.0),
                label,
                FontRole::Ui,
                t.ui_size,
                t.editor.text,
            );
        });
    }

    // -- header: column titles, tick labels, unit -----------------------------
    let panel_theme = t.panel;
    let header = layout.header;
    p.scene.clipped(
        Rect::new(header.origin, size(layout.names.width(), header.height())),
        |scene| {
            scene.text(
                point(layout.names.left() + z(12.0), header.top()),
                header.height(),
                "SIGNALS",
                FontRole::UiSemibold,
                t.ui_size_small,
                panel_theme.text_muted,
            );
        },
    );
    let (value_title, vfont, vsize, vcolor) = match cursor {
        Some(c) => (
            format_time(c as f64, base),
            FontRole::Mono,
            t.mono_size,
            panel_theme.text,
        ),
        None => (
            "VALUE".to_string(),
            FontRole::UiSemibold,
            t.ui_size_small,
            panel_theme.text_muted,
        ),
    };
    p.scene.clipped(
        Rect::new(
            point(layout.values.left(), header.top()),
            size(layout.values.width(), header.height()),
        ),
        |scene| {
            scene.text(
                point(layout.values.left() + z(8.0), header.top()),
                header.height(),
                value_title,
                vfont,
                vsize,
                vcolor,
            );
        },
    );
    overlay::header_ticks(&mut p, &column, &tick_list, unit);
    overlay::clock_rulers(
        &mut p,
        &column,
        Rect::new(
            point(bounds.left(), layout.rulers.top()),
            size(
                layout.names.width() + layout.values.width(),
                layout.rulers.height(),
            ),
        ),
        clocks,
        &doc.clocks,
        base,
        cursor,
    );

    // -- markers and cursor --------------------------------------------------------
    overlay::marker_lane_paint(
        &mut p,
        &column,
        &layout.marker_lane,
        layout.names,
        Some(layout.values),
        doc,
        cursor,
        model.pointer,
        match model.drag {
            Some(Drag::Marker(held)) if held.moved => Some(held.from),
            _ => None,
        },
    );
    overlay::cursor(&mut p, &column, cursor, base, focused, z(SCROLLBAR_W));
    paint_analog_overlays(model, doc, &layout, &viewport, cursor, &mut p);

    // -- borders --------------------------------------------------------------
    let drag = model.drag;
    let near_names = model
        .pointer
        .is_some_and(|mp| layout.names_split.contains(mp));
    let near_values = model
        .pointer
        .is_some_and(|mp| layout.values_split.contains(mp));
    let names_border = if near_names || drag == Some(Drag::NamesSplit) {
        t.border_focused
    } else {
        t.border
    };
    let values_border = if near_values || drag == Some(Drag::ValuesSplit) {
        t.border_focused
    } else {
        t.border
    };
    p.scene.fill(
        Rect::new(
            point(layout.names.right() - 1.0, bounds.top()),
            size(1.0, bounds.height()),
        ),
        names_border,
    );
    p.scene.fill(
        Rect::new(
            point(layout.values.right() - 1.0, bounds.top()),
            size(1.0, bounds.height()),
        ),
        values_border,
    );
    p.scene.fill(
        Rect::new(
            point(bounds.left(), header.bottom() - 1.0),
            size(bounds.width(), 1.0),
        ),
        t.border_variant,
    );
    // Pointer shapes: the dividers resize, badges and chips are clickable;
    // only an active drag pins the resize cursor.
    // Row bottom edges in the names column resize rows.
    let resizing = match drag {
        Some(Drag::RowHeight { row, .. }) => Some(row),
        _ => model.edge_hover,
    };
    for pos in layout
        .rows
        .clone()
        .filter(|pos| *pos < layout.visible.len())
    {
        let bottom = layout.row_y(pos) + layout.row_height(pos);
        if bottom <= layout.names.top() {
            continue;
        }
        let grab = z(crate::wave::model::ROW_EDGE_GRAB_PX);
        p.scene.cursors.push((
            Rect::from_xywh(
                layout.names.left(),
                bottom - grab,
                layout.names.width(),
                2.0 * grab,
            ),
            CursorIcon::ResizeUpDown,
        ));
        if resizing.is_some() && resizing == layout.entry(pos) {
            p.scene.fill(
                Rect::from_xywh(
                    layout.names.left(),
                    snap(bottom) - 1.0,
                    layout.names.width(),
                    z(2.0).max(2.0),
                ),
                t.border_focused,
            );
        }
    }
    if matches!(drag, Some(Drag::RowHeight { .. })) {
        p.scene.window_cursor = Some(CursorIcon::ResizeUpDown);
    } else if matches!(drag, Some(Drag::NamesSplit | Drag::ValuesSplit)) {
        p.scene.window_cursor = Some(CursorIcon::ResizeLeftRight);
    } else if matches!(drag, Some(Drag::Rows { started: true, .. })) {
        p.scene.window_cursor = Some(CursorIcon::Grabbing);
    } else {
        p.scene
            .cursors
            .push((layout.names_split, CursorIcon::ResizeLeftRight));
        p.scene
            .cursors
            .push((layout.values_split, CursorIcon::ResizeLeftRight));
        for (_, b) in &layout.badges {
            p.scene.cursors.push((*b, CursorIcon::PointingHand));
        }
    }

    // -- scrollbar -----------------------------------------------------------------
    if let Some((track, thumb)) = layout.scrollbar {
        let hovered = model.pointer.is_some_and(|mp| track.contains(mp))
            || matches!(drag, Some(Drag::Scroll { .. }));
        let color = if hovered {
            t.scrollbar_thumb_hover
        } else {
            t.scrollbar_thumb
        };
        p.scene.quad(thumb, color, z(3.0), 0.0, Color::TRANSPARENT);
    }
}

// ---------------------------------------------------------------------------
// Waveform painting
// ---------------------------------------------------------------------------

/// Where row `wave_row` plots its values, once its range is known.
fn analog_plot(a: &Analog, wave_row: Rect, zoom: f32) -> Option<Plot> {
    let (lo, hi) = a.shown.or(a.target)?;
    Some(Plot {
        left: wave_row.left(),
        width: wave_row.width().floor(),
        top: wave_row.top() + TRACE_PAD * zoom,
        bottom: wave_row.bottom() - TRACE_PAD * zoom,
        lo,
        hi,
    })
}

/// A number in the row's format: the translator formats the value that
/// reads back as `v`, or a plain number when it has none.
fn analog_label(item: &DisplayedSignal, v: f64) -> String {
    item.translator
        .numeric_kind()
        .and_then(|k| k.value_of(v, item.shape))
        .map(|value| item.translator.translate(&value).text)
        .unwrap_or_else(|| {
            if v.fract() == 0.0 && v.abs() < 1e15 {
                format!("{v:.0}")
            } else {
                format!("{v:.4}")
            }
        })
}

/// The overlap of two rectangles (empty when they are disjoint).
fn intersect(a: Rect, b: Rect) -> Rect {
    let (l, t) = (a.left().max(b.left()), a.top().max(b.top()));
    let (r, bt) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    Rect::from_xywh(l, t, (r - l).max(0.0), (bt - t).max(0.0))
}

/// Paint a row as a plot: guides, the zero line, undefined spans, the fill
/// under the curve, the curve, sample dots and, from 2×, range labels.
#[allow(clippy::too_many_arguments)]
fn paint_analog_row(
    item: &DisplayedSignal,
    a: &Analog,
    series: &Series<'_>,
    vp: &Viewport,
    wave_row: Rect,
    waves: Rect,
    row_h: f32,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let clip = intersect(wave_row, waves);
    let plot = analog_plot(a, wave_row, t.zoom);
    let g = plot.map(|plot| analog::geometry(series, a.draw, vp, &plot, t.zoom));
    // A long history waits for its summary rather than scanning every change.
    let (Some(plot), Some(g)) = (plot, g.filter(|g| !g.waiting)) else {
        if series.building {
            p.scene.clipped(clip, |scene| {
                scene.text(
                    point(wave_row.left() + z(8.0), wave_row.top()),
                    wave_row.height(),
                    "Summarizing…",
                    FontRole::Ui,
                    t.ui_size_small,
                    t.editor.text_placeholder,
                );
            });
        }
        return;
    };
    p.scene.clipped(clip, |scene| {
        scene.fill(
            Rect::from_xywh(plot.left, snap(plot.top), plot.width, 1.0),
            t.wave_tick,
        );
        scene.fill(
            Rect::from_xywh(plot.left, snap(plot.bottom), plot.width, 1.0),
            t.wave_tick,
        );
        if plot.lo < 0.0 && plot.hi > 0.0 {
            let y = snap(plot.y_of(0.0)) + 0.5;
            let (dash, gap) = (z(3.0), z(4.0));
            let mut segments = Vec::new();
            let mut x = plot.left;
            while x < plot.left + plot.width {
                segments.push([
                    point(x, y),
                    point((x + dash).min(plot.left + plot.width), y),
                ]);
                x += dash + gap;
            }
            scene.lines(segments, t.wave_tick_text.with_alpha(0.35), 1.0);
        }
        for &(xa, xb) in &g.undefined {
            let r = Rect::from_xywh(xa, plot.top, (xb - xa).max(1.0), plot.bottom - plot.top);
            scene.fill(r, t.wave_undef.with_alpha(0.14));
            scene.fill(
                Rect::from_xywh(xa, snap(plot.top), r.width(), 1.0),
                t.wave_undef,
            );
            scene.fill(
                Rect::from_xywh(xa, snap(plot.bottom) - 1.0, r.width(), 1.0),
                t.wave_undef,
            );
        }
        let base = plot.baseline();
        for (x, w, y) in analog::fill_columns(&g.runs, plot.left, plot.width) {
            let (y0, y1) = (y.min(base), y.max(base));
            if y1 - y0 >= 0.5 {
                scene.fill(Rect::from_xywh(x, y0, w, y1 - y0), t.wave_high_fill);
            }
        }
        let mut segments = Vec::new();
        for run in &g.runs {
            match run.as_slice() {
                [only] => segments.push([*only, point(only.x + 1.0, only.y)]),
                points => segments.extend(points.windows(2).map(|w| [w[0], w[1]])),
            }
        }
        let width = if g.envelope { 1.0 } else { z(1.25).max(1.0) };
        scene.lines(segments, t.wave_signal, width);
        let r = z(2.0);
        for d in &g.dots {
            scene.quad(
                Rect::from_xywh(d.x - r, d.y - r, 2.0 * r, 2.0 * r),
                t.wave_signal,
                r,
                0.0,
                Color::TRANSPARENT,
            );
        }
    });
    // Range labels at the top and bottom from 2× up.
    let Some((lo, hi)) = a.target else { return };
    if wave_row.height() < 2.0 * row_h - 0.5 {
        return;
    }
    let size = t.ui_size_small;
    let label_h = z(14.0);
    for (v, y, bound) in [
        (hi, plot.top + z(1.0), "max"),
        (lo, plot.bottom - label_h - z(1.0), "min"),
    ] {
        let text = format!("{bound} {}", analog_label(item, v));
        let w = p.width(&text, FontRole::Mono, size) + z(8.0);
        let chip = Rect::from_xywh(plot.left + z(4.0), y, w, label_h);
        p.scene.clipped(clip, |scene| {
            scene.quad(
                chip,
                t.editor.bg.with_alpha(0.82),
                z(3.0),
                0.0,
                Color::TRANSPARENT,
            );
            scene.text(
                point(chip.left() + z(4.0), chip.top()),
                label_h,
                text,
                FontRole::Mono,
                size,
                t.editor.text_placeholder,
            );
        });
    }
}

/// Over every visible plot: a dot where the cursor crosses the curve and,
/// under the pointer, a readout of the sample or dense column there.
fn paint_analog_overlays(
    model: &WaveModel,
    doc: &Document,
    layout: &WaveLayout,
    vp: &Viewport,
    cursor: Option<u64>,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let waves = layout.waves;
    let reading = matches!(model.drag, None | Some(Drag::Cursor));
    for pos in layout.rows.clone() {
        let Some(ix) = layout.entry(pos) else {
            continue;
        };
        let Some(item) = model.signal(ix) else {
            continue;
        };
        let (Some(a), Some(h)) = (&item.analog, &item.history) else {
            continue;
        };
        if !analog::supports(item.shape, item.translator.as_ref()) {
            continue;
        }
        let wave_row = Rect::from_xywh(
            waves.left(),
            layout.row_y(pos),
            waves.width(),
            layout.row_height(pos),
        );
        let Some(plot) = analog_plot(a, wave_row, t.zoom) else {
            continue;
        };
        let tr = item.translator.as_ref();
        let Some(kind) = tr.numeric_kind() else {
            continue;
        };
        let series = Series::of(doc, item.source.signal(), h, kind);
        let envelope = analog::draw_mode(h.as_ref(), vp, plot.width) == analog::DrawMode::Envelope;
        let clip = intersect(wave_row, waves);
        if let Some(c) = cursor
            && (c as f64) >= vp.start
            && (c as f64) <= vp.end
            && let Some(y) = analog::y_at(&series, a.draw, envelope, &plot, c as f64)
        {
            let x = snap(plot.left + vp.x_of(c as f64, f64::from(plot.width)) as f32) + 0.5;
            let r = z(3.25);
            p.scene.clipped(clip, |scene| {
                scene.quad(
                    Rect::from_xywh(x - r, y - r, 2.0 * r, 2.0 * r),
                    t.wave_signal,
                    r,
                    z(1.5),
                    t.editor.bg,
                );
            });
        }
        let Some(pointer) = model
            .pointer
            .filter(|p| reading && model.hover_row == Some(ix) && waves.contains(*p))
        else {
            continue;
        };
        let Some(read) = analog::readout(&series, a.draw, vp, &plot, pointer.x) else {
            continue;
        };
        let text = match &read {
            Readout::Sample { index, time, at } => {
                let r = z(3.5);
                p.scene.clipped(clip, |scene| {
                    scene.quad(
                        Rect::from_xywh(at.x - r, at.y - r, 2.0 * r, 2.0 * r),
                        t.editor.bg,
                        r,
                        z(1.5),
                        t.editor.text,
                    );
                });
                let value = tr.translate(&h.value(*index)).text;
                let prefix = if a.draw == AnalogDraw::Linear {
                    "sample at "
                } else {
                    "at "
                };
                format!("{value}  {prefix}{}", format_time(*time, doc.time_base()))
            }
            Readout::Column { lo, hi, changes, x } => {
                let (ya, yb) = (plot.y_of(*hi), plot.y_of(*lo));
                p.scene.clipped(clip, |scene| {
                    scene.fill(
                        Rect::from_xywh(x - 1.0, ya - 1.0, 2.0, yb - ya + 2.0),
                        t.editor.text,
                    );
                });
                format!(
                    "{} … {}  {changes} change{}",
                    analog_label(item, *lo),
                    analog_label(item, *hi),
                    if *changes == 1 { "" } else { "s" }
                )
            }
        };
        let h_chip = z(20.0);
        let w_chip = p.width(&text, FontRole::Mono, t.mono_size) + z(14.0);
        let x = (pointer.x + z(14.0))
            .min(waves.right() - w_chip - z(4.0))
            .max(waves.left());
        let y = if pointer.y - h_chip - z(10.0) >= waves.top() {
            pointer.y - h_chip - z(10.0)
        } else {
            pointer.y + z(16.0)
        };
        let chip = Rect::from_xywh(snap(x), snap(y), w_chip, h_chip);
        p.scene.clipped(waves, |scene| {
            scene.quad(chip, t.tooltip.bg, z(4.0), 1.0, t.border);
            scene.text(
                point(chip.left() + z(7.0), chip.top()),
                h_chip,
                text,
                FontRole::Mono,
                t.mono_size,
                t.tooltip.text,
            );
        });
    }
}

/// Paint occurrences, coalescing timestamps that occupy the same pixel.
/// Returns each coalesced arrow's x, its count and the room to its right
/// before the next arrow, for the caller to label with its count.
pub fn paint_event_row(
    h: &dyn SignalHistory,
    vp: &Viewport,
    area: Rect,
    t: &Theme,
    scene: &mut Scene,
) -> Vec<(f32, usize, f32)> {
    let mut counts: Vec<(f32, usize, f32)> = Vec::new();
    if area.width() <= 0.0 || vp.width() <= 0.0 {
        return counts;
    }
    // Search strictly before the viewport so duplicate timestamps at its
    // left edge are all included in the first marker's count.
    let mut i = if vp.start <= 0.0 {
        0
    } else {
        h.index_at((vp.start.ceil() as u64).saturating_sub(1))
            .map_or(0, |last| last + 1)
    };
    let pad = TRACE_PAD * t.zoom;
    while i < h.len() && h.time(i) as f64 <= vp.end {
        let x = vp.x_of(h.time(i) as f64, area.width() as f64).floor();
        let screen_x = area.left() + x as f32;
        let top = area.top() + pad;
        let bottom = (area.bottom() - pad).max(top);
        // Surfer's event glyph: a full-height stem with a filled upward
        // arrowhead, 5 px wide and one fifth of the trace height.
        let head_height = (bottom - top) * 0.2;
        let mut segments = vec![[point(screen_x, top), point(screen_x, bottom)]];
        if head_height > 0.0 {
            for row in 1..=head_height.ceil() as usize {
                let y = (row as f32).min(head_height);
                let half_width = 2.5 * t.zoom * y / head_height;
                segments.push([
                    point(screen_x - half_width, top + y),
                    point(screen_x + half_width, top + y),
                ]);
            }
        }
        // Count all occurrences in this pixel, but exclude those beyond the
        // inclusive viewport end. Binary search keeps dense rows bounded.
        let next = vp.time_at(x + 1.0, area.width() as f64).ceil();
        let last_time = ((next as u64).saturating_sub(1)).min(vp.end.floor() as u64);
        let end = h
            .index_at(last_time)
            .map_or(i + 1, |last| (last + 1).max(i + 1));
        let color = if end - i > 1 {
            t.wave_event_coalesced
        } else {
            t.wave_signal
        };
        scene.lines(segments, color, 1.0);
        if let Some(last) = counts.last_mut()
            && last.2.is_infinite()
        {
            last.2 = screen_x - last.0;
        }
        if end - i > 1 {
            counts.push((screen_x, end - i, f32::INFINITY));
        }
        i = end;
    }
    if let Some(last) = counts.last_mut()
        && last.2.is_infinite()
    {
        last.2 = area.right() - last.0;
    }
    counts
}

/// "×n" beside each coalesced event arrow that has room for it.
fn paint_event_counts(
    counts: &[(f32, usize, f32)],
    area: Rect,
    clip: Rect,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let gap = 3.0 * t.zoom;
    for &(x, n, room) in counts {
        let label = format!("×{n}");
        let w = p.width(&label, FontRole::Mono, t.ui_size_small);
        if w + 2.0 * gap <= room {
            p.scene.clipped(clip, |scene| {
                scene.text(
                    point(snap(x + gap), area.top()),
                    area.height(),
                    label,
                    FontRole::Mono,
                    t.ui_size_small,
                    t.wave_event_coalesced,
                )
            });
        }
    }
}

/// Paint a 1-bit trace by sampling one pixel column at a time.
pub fn paint_bit_row(
    h: &dyn SignalHistory,
    vp: &Viewport,
    area: Rect,
    t: &Theme,
    scene: &mut Scene,
) {
    let columns = 0..area.width().floor().max(0.0) as usize;
    paint_bits(h, vp, area, t, scene, true, columns);
}

/// A 1-bit trace: a level line per stretch, a 1px edge per change, and
/// aliased columns where a pixel holds two or more changes. X is a
/// mid-level line over its hatched tint, Z a bare line at mid level,
/// don't-care dotted and weak dashed; `high_fill` fills high stretches.
/// Only pixel `columns` of the area are drawn.
fn paint_bits(
    h: &dyn SignalHistory,
    vp: &Viewport,
    area: Rect,
    t: &Theme,
    scene: &mut Scene,
    high_fill: bool,
    columns: std::ops::Range<usize>,
) {
    let w_px = area.width().floor().max(0.0) as usize;
    let columns = columns.start.min(w_px)..columns.end.min(w_px);
    if columns.is_empty() {
        return;
    }
    let wf = w_px as f64;
    let x0 = area.left();
    let top = area.top() + TRACE_PAD * t.zoom;
    let bottom = area.bottom() - TRACE_PAD * t.zoom;
    let mid = snap(top + (bottom - top) / 2.0);
    let y_of = |b: Bit| -> f32 {
        match b {
            Bit::One => top,
            Bit::Zero => bottom - 1.0,
            _ => mid,
        }
    };
    let mut idx = index_at_x(h, vp, wf, columns.start as f64, None);
    let mut bit = h.bit(idx);
    let mut hint = idx.unwrap_or(0);
    let mut run_start = columns.start;
    let mut dense: Vec<marks::Aliased> = Vec::new();
    let mut kinds = marks::KindReader::new(h, true);
    // Dotted and dashed levels are collected per kind and stroked once.
    let mut dotted: Vec<[Point; 2]> = Vec::new();
    let mut dashed: Vec<[Point; 2]> = Vec::new();

    let mut emit_run = |scene: &mut Scene, xs: usize, xe: usize, b: Bit| {
        if xe <= xs || b == Bit::Unavailable {
            return;
        }
        let x = x0 + xs as f32;
        let w = (xe - xs) as f32;
        let kind = b.kind();
        let color = t.value_color(kind);
        match kind {
            ValueKind::Undef => {
                let band = Rect::new(point(x, top), size(w, bottom - top));
                scene.fill(band, t.wave_undef_fill);
                marks::hatch(scene, t, band, x0);
            }
            ValueKind::DontCare => {
                marks::dashes(&mut dotted, x, x + w, y_of(b), x0, marks::DOTTED);
                return;
            }
            ValueKind::Weak => {
                marks::dashes(&mut dashed, x, x + w, y_of(b), x0, scaled(marks::DASHED, t));
                return;
            }
            _ => {}
        }
        scene.fill(Rect::new(point(x, y_of(b)), size(w, 1.0)), color);
        if b == Bit::One && high_fill {
            scene.fill(
                Rect::new(point(x, top + 1.0), size(w, bottom - top - 1.0)),
                t.wave_high_fill,
            );
        }
    };

    for x in columns.clone() {
        let idx1 = index_at_x(h, vp, wf, (x + 1) as f64, Some(hint));
        let n = changes_between(idx, idx1);
        if n < 2 && !dense.is_empty() {
            // An aliased stretch ends at the first column with fewer than two changes.
            marks::paint_aliased(scene, t, &dense, x0, top, bottom);
            dense.clear();
        }
        if n == 0 {
            continue;
        }
        let new_bit = h.bit(idx1);
        emit_run(scene, run_start, x, bit);
        if n == 1 && bit != Bit::Unavailable && new_bit != Bit::Unavailable {
            let (ya, yb) = (y_of(bit), y_of(new_bit));
            let (lo, hi) = if ya <= yb { (ya, yb) } else { (yb, ya) };
            let color = t.value_color(if new_bit.kind() != ValueKind::Normal {
                new_bit.kind()
            } else {
                bit.kind()
            });
            scene.fill(
                Rect::new(point(x0 + x as f32, lo), size(1.0, hi - lo + 1.0)),
                color,
            );
        } else if n >= 2 {
            let last = idx1.expect("a column with changes ends on one");
            dense.push(marks::Aliased {
                x,
                changes: n,
                kinds: kinds.column(idx, last),
            });
        }
        run_start = x + 1;
        bit = new_bit;
        idx = idx1;
        hint = idx1.unwrap_or(0);
    }
    if !dense.is_empty() {
        marks::paint_aliased(scene, t, &dense, x0, top, bottom);
    }
    emit_run(scene, run_start, columns.end, bit);
    scene.lines(dotted, t.wave_dontcare, 1.0);
    scene.lines(dashed, t.wave_weak, 1.0);
}

/// A dash pattern at the theme's zoom.
fn scaled((on, off): (f32, f32), t: &Theme) -> (f32, f32) {
    (on * t.zoom, off * t.zoom)
}

/// Index of the last change at or before the time under pixel `x`.
fn index_at_x(
    h: &dyn SignalHistory,
    vp: &Viewport,
    width_px: f64,
    x: f64,
    hint: Option<usize>,
) -> Option<usize> {
    let t = vp.time_at(x, width_px);
    if t < 0.0 {
        return None;
    }
    match hint {
        Some(hint) => h.index_at_hint(t as u64, hint),
        None => h.index_at(t as u64),
    }
}

struct Segment {
    x_start: usize,
    x_end: usize,
    idx: Option<usize>,
    dense: bool,
}

/// Half-width of a bus hexagon's slant on each side of a change, design px.
const SLANT_PX: f32 = 4.0;
/// Gap between a slant and its label, design px.
const LABEL_GAP_PX: f32 = 4.0;

/// How a bus value is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BusLook {
    /// Every bit 0 on a numeric row: a low line, no label.
    Zero,
    /// Every bit Z: a bare line at mid level.
    Floating,
    /// Every bit unknown: the hatched X hexagon.
    Unknown,
    /// Any other value: an outlined hexagon with its label.
    Value,
}

fn bus_look(value: &WaveValue, numeric: bool) -> BusLook {
    let WaveValue::Bits(bits) = value else {
        return BusLook::Value;
    };
    let all = |f: fn(u8) -> bool| !bits.is_empty() && bits.bytes().all(f);
    if all(|c| matches!(c, b'x' | b'X' | b'u' | b'U' | b'w' | b'W')) {
        BusLook::Unknown
    } else if all(|c| matches!(c, b'z' | b'Z')) {
        BusLook::Floating
    } else if numeric && all(|c| c == b'0') {
        BusLook::Zero
    } else {
        BusLook::Value
    }
}

/// Fit a label into `max_chars` monospace cells. A number keeps its last
/// digits behind a leading ellipsis, where neighbouring values differ; text
/// keeps its beginning. A label that would show fewer than two characters
/// is dropped rather than shown as a lone ellipsis.
pub(crate) fn fit_label(text: &str, max_chars: usize, number: bool) -> Option<String> {
    let n = text.chars().count();
    if n <= max_chars {
        return (n > 0).then(|| text.to_owned());
    }
    if max_chars < 3 {
        return None;
    }
    let keep = max_chars - 1;
    Some(if number {
        std::iter::once('…')
            .chain(text.chars().skip(n - keep))
            .collect()
    } else {
        text.chars()
            .take(keep)
            .chain(std::iter::once('…'))
            .collect()
    })
}

/// The label of a partly unknown value in runs: unknown digits in the X
/// colour, floating ones in the Z colour, the rest in `text`.
fn label_runs(label: &str, t: &Theme, text: Color) -> Vec<(usize, String, Color)> {
    let mut runs: Vec<(usize, String, Color)> = Vec::new();
    for (i, c) in label.chars().enumerate() {
        let color = match c {
            'x' | 'X' | 'u' | 'U' => t.wave_undef,
            'z' | 'Z' => t.wave_highimp,
            _ => text,
        };
        match runs.last_mut() {
            Some((_, run, last)) if *last == color => run.push(c),
            _ => runs.push((i, c.to_string(), color)),
        }
    }
    runs
}

/// Fill a hexagon from `xa` to `xb` with slants `tw` wide (open ends have
/// none), column by column through the slants so the fill stays crisp.
#[allow(clippy::too_many_arguments)]
fn fill_hexagon(
    scene: &mut Scene,
    xa: f32,
    xb: f32,
    tw: f32,
    open: (bool, bool),
    top: f32,
    bottom: f32,
    color: Color,
) {
    let (la, rb) = (
        if open.0 { xa } else { xa + tw },
        if open.1 { xb } else { xb - tw },
    );
    if rb > la {
        scene.fill(Rect::from_xywh(la, top, rb - la, bottom - top), color);
    }
    let half = (bottom - top) / 2.0;
    let mid = top + half;
    let steps = tw.ceil().max(0.0) as usize;
    for k in 0..steps {
        let reach = ((k as f32 + 0.5) / tw).min(1.0) * half;
        for (edge, open, dir) in [(xa, open.0, 1.0), (xb, open.1, -1.0)] {
            if open {
                continue;
            }
            let x = if dir > 0.0 {
                edge + k as f32
            } else {
                edge - k as f32 - 1.0
            };
            scene.fill(Rect::from_xywh(x, mid - reach, 1.0, 2.0 * reach), color);
        }
    }
}

/// Paint a multi-bit trace as slanted "hexagon" segments with value text.
fn paint_bus_row(
    h: &dyn SignalHistory,
    translator: &dyn Translator,
    vp: &Viewport,
    area: Rect,
    clip: Rect,
    char_w: f32,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let w_px = area.width().floor().max(0.0) as usize;
    if w_px == 0 {
        return;
    }
    let wf = w_px as f64;
    let x0 = area.left();
    let top = area.top() + TRACE_PAD * t.zoom;
    let bottom = area.bottom() - TRACE_PAD * t.zoom;
    let midf = top + (bottom - top) / 2.0;
    let numeric = translator.numeric_kind().is_some();

    // Column sampling → segments, with each changing column's count and kinds.
    let mut segments: Vec<Segment> = Vec::new();
    let mut columns: Vec<(usize, marks::Kinds)> = vec![(0, marks::Kinds::NONE); w_px];
    let mut kinds = marks::KindReader::new(h, false);
    let mut idx = index_at_x(h, vp, wf, 0.0, None);
    let mut hint = idx.unwrap_or(0);
    let mut cur_start = 0usize;
    for (x, column) in columns.iter_mut().enumerate() {
        let idx1 = index_at_x(h, vp, wf, (x + 1) as f64, Some(hint));
        let n = changes_between(idx, idx1);
        if n == 0 {
            continue;
        }
        let last = idx1.expect("a column with changes ends on one");
        *column = (n, kinds.column(idx, last));
        if x > cur_start {
            segments.push(Segment {
                x_start: cur_start,
                x_end: x,
                idx,
                dense: false,
            });
        }
        if n == 1 {
            cur_start = x;
        } else {
            match segments.last_mut() {
                Some(last) if last.dense && last.x_end == x => last.x_end = x + 1,
                _ => segments.push(Segment {
                    x_start: x,
                    x_end: x + 1,
                    idx: idx1,
                    dense: true,
                }),
            }
            cur_start = x + 1;
        }
        idx = idx1;
        hint = idx1.unwrap_or(0);
    }
    if w_px > cur_start {
        segments.push(Segment {
            x_start: cur_start,
            x_end: w_px,
            idx,
            dense: false,
        });
    }
    // Segments too narrow to draw meaningfully become dense, then merge neighbours.
    for seg in segments.iter_mut() {
        if !seg.dense
            && seg.x_end - seg.x_start < MIN_SEGMENT_PX
            && seg.x_start != 0
            && seg.x_end != w_px
        {
            seg.dense = true;
        }
    }
    let mut merged: Vec<Segment> = Vec::with_capacity(segments.len());
    for seg in segments {
        match merged.last_mut() {
            Some(last) if last.dense && seg.dense && last.x_end == seg.x_start => {
                last.x_end = seg.x_end
            }
            _ => merged.push(seg),
        }
    }
    let segments = merged;

    // Paint. Horizontal edges are crisp quads; slants are stroked lines.
    let mut slants: Vec<(Color, Vec<[Point; 2]>)> = Vec::new();
    let slant_w = SLANT_PX * t.zoom;
    let mut texts: Vec<(f32, String, Color)> = Vec::new();
    p.scene.clipped(clip, |scene| {
        for seg in &segments {
            let xa = x0 + seg.x_start as f32;
            let xb = x0 + seg.x_end as f32;
            if seg.dense {
                // Every column of an aliased stretch counts as at least two changes.
                let entering = marks::Kinds::of(h.value_view(seg.idx).kind());
                let aliased: Vec<marks::Aliased> = (seg.x_start..seg.x_end)
                    .map(|x| marks::Aliased {
                        x,
                        changes: columns[x].0.max(2),
                        kinds: columns[x].1.union(entering),
                    })
                    .collect();
                marks::paint_aliased(scene, t, &aliased, x0, top, bottom);
                continue;
            }
            let value = h.value(seg.idx);
            if matches!(value, WaveValue::Unavailable) {
                continue;
            }
            let kind = value.kind();
            let look = bus_look(&value, numeric);
            let seg_w = xb - xa;
            let tw = slant_w.min(seg_w / 2.0);
            let open_left = seg.x_start == 0;
            let open_right = seg.x_end == w_px;
            let la = if open_left { xa } else { xa + tw };
            let rb = if open_right { xb } else { xb - tw };
            let (topf, botf) = (top + 0.5, bottom - 0.5);
            let color = match look {
                BusLook::Unknown => t.wave_undef,
                BusLook::Floating => t.wave_highimp,
                // A partly unknown or floating value keeps the signal outline.
                _ if matches!(kind, ValueKind::Undef | ValueKind::HighImp) => t.wave_signal,
                _ => t.value_color(kind),
            };
            let lines = match slants.iter_mut().find(|(c, _)| *c == color) {
                Some((_, b)) => b,
                None => {
                    slants.push((color, Vec::new()));
                    &mut slants.last_mut().unwrap().1
                }
            };
            match look {
                BusLook::Floating => {
                    scene.fill(Rect::from_xywh(xa, snap(midf), seg_w, 1.0), color);
                    continue;
                }
                BusLook::Zero => {
                    // Zero reads as idle: the slants run into a low line; hover still reads it.
                    if rb > la {
                        scene.fill(Rect::from_xywh(la, bottom - 1.0, rb - la, 1.0), color);
                    }
                    if !open_left {
                        lines.push([point(xa, midf), point(la, botf)]);
                    }
                    if !open_right {
                        lines.push([point(xb, midf), point(rb, botf)]);
                    }
                    continue;
                }
                BusLook::Unknown => {
                    fill_hexagon(
                        scene,
                        xa,
                        xb,
                        tw,
                        (open_left, open_right),
                        top,
                        bottom,
                        t.wave_undef_fill,
                    );
                    if rb > la {
                        marks::hatch(
                            scene,
                            t,
                            Rect::from_xywh(la, top, rb - la, bottom - top),
                            x0,
                        );
                    }
                }
                BusLook::Value if !numeric && kind == ValueKind::Normal => {
                    // States get a tint, so a recurring state is known before its label is read.
                    let tint = t.value_tint(&translator.translate(&value).text);
                    fill_hexagon(
                        scene,
                        xa,
                        xb,
                        tw,
                        (open_left, open_right),
                        top,
                        bottom,
                        tint,
                    );
                }
                BusLook::Value => {}
            }
            if rb > la {
                scene.fill(Rect::new(point(la, top), size(rb - la, 1.0)), color);
                scene.fill(
                    Rect::new(point(la, bottom - 1.0), size(rb - la, 1.0)),
                    color,
                );
            }
            if !open_left {
                lines.push([point(xa, midf), point(xa + tw, topf)]);
                lines.push([point(xa, midf), point(xa + tw, botf)]);
            }
            if !open_right {
                lines.push([point(xb, midf), point(xb - tw, topf)]);
                lines.push([point(xb, midf), point(xb - tw, botf)]);
            }
            // Text, one slant and a gap in from each end.
            let avail = seg_w - 2.0 * (tw + LABEL_GAP_PX * t.zoom);
            if avail >= char_w {
                let max_chars = (avail / char_w).floor() as usize;
                let tr = translator.translate(&value);
                if let Some(text) = fit_label(&tr.text, max_chars, numeric) {
                    let x = snap(xa + tw + LABEL_GAP_PX * t.zoom);
                    let plain = match look {
                        BusLook::Unknown => t.wave_undef,
                        _ if tr.kind == ValueKind::Normal => t.wave_bus_text,
                        _ if matches!(tr.kind, ValueKind::Undef | ValueKind::HighImp) => {
                            t.wave_bus_text
                        }
                        _ => t.value_color(tr.kind),
                    };
                    if look == BusLook::Value
                        && matches!(tr.kind, ValueKind::Undef | ValueKind::HighImp)
                    {
                        for (at, run, color) in label_runs(&text, t, plain) {
                            texts.push((x + at as f32 * char_w, run, color));
                        }
                    } else {
                        texts.push((x, text, plain));
                    }
                }
            }
        }
        for (color, segments) in slants {
            scene.lines(segments, color, 1.0);
        }
        for (tx, text, color) in texts {
            scene.text(
                point(tx, area.top()),
                area.height(),
                text,
                FontRole::Mono,
                t.mono_size,
                color,
            );
        }
    });
}

/// Where one lane, clock or group row is painted and what it reads.
struct LaneCells<'a> {
    layout: &'a WaveLayout,
    /// Visible position of the row.
    pos: usize,
    /// Where its name starts, after the indentation of its depth.
    name_x: f32,
    viewport: Viewport,
    cursor: Option<u64>,
    text: Color,
    muted: Color,
}

/// A clock row: its name, its cycle at the cursor in the values column, and
/// a square wave drawn from its stretches (a band where edges are denser
/// than pixels).
fn paint_clock_row(
    row: &crate::wave::model::ClockRow,
    view: &crate::clock::ClockView,
    doc: &Document,
    cells: &LaneCells<'_>,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let layout = cells.layout;
    let (names, values, waves) = (layout.names, layout.values, layout.waves);
    let row_h = layout.row_h;
    let y = layout.row_y(cells.pos);
    let full_h = layout.row_height(cells.pos);
    let timeline = row.timeline(doc);
    let name = row.name.clone();
    let name_color = if timeline.is_some() {
        cells.text
    } else {
        cells.muted
    };
    p.scene.clipped(names, |scene| {
        scene.text(
            point(cells.name_x, y),
            row_h,
            name,
            FontRole::Ui,
            t.ui_size,
            name_color,
        );
    });
    let value = match (timeline, doc.clocks.find(&row.key)) {
        (Some(tl), _) => cells
            .cursor
            .and_then(|c| tl.cycle_at(c))
            .map(|at| crate::clock::format_position(view, tl, &at)),
        (None, None) => Some("no such clock".into()),
        (None, Some(c)) => Some(match &c.state {
            crate::clock::ClockState::Failed(e) => e.clone(),
            _ => "loading…".into(),
        }),
    };
    if let Some(value) = value {
        p.scene.clipped(values, |scene| {
            scene.text(
                point(values.left() + z(8.0), y),
                row_h,
                value,
                FontRole::Mono,
                t.mono_size,
                cells.muted,
            );
        });
    }
    if let Some(tl) = timeline {
        let wave_row = Rect::new(point(waves.left(), y), size(waves.width(), full_h));
        paint_clock_wave(
            tl,
            &cells.viewport,
            wave_row,
            waves,
            doc.limits(),
            doc.time_base(),
            p,
        );
    }
}

/// A clock under 3px per half period is a band labelled with its frequency
/// and period; a gap between its stretches is a dashed low line marked
/// "gated".
const CLOCK_BAND_HALF_PX: f64 = 3.0;
/// Gaps at least this wide (design px) say "gated".
const GATED_LABEL_PX: f32 = 52.0;

/// A declared clock: where its edges are 3px apart or more, a square wave
/// without the high fill, whose stripes would only add noise; closer, a
/// band labelled "500.00 MHz · 2 ns"; between stretches, a dashed low line.
#[allow(clippy::too_many_arguments)]
fn paint_clock_wave(
    timeline: &std::sync::Arc<vtr::clock::ClockTimeline>,
    vp: &Viewport,
    area: Rect,
    clip: Rect,
    (first, last): (u64, u64),
    base: crate::wave::timeline::TimeBase<'_>,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let w_px = area.width().floor().max(0.0) as usize;
    if w_px == 0 {
        return;
    }
    let wf = w_px as f64;
    let ppu = vp.px_per_unit(wf);
    let top = area.top() + TRACE_PAD * t.zoom;
    let bottom = area.bottom() - TRACE_PAD * t.zoom;
    let col = |time: u64| vp.x_of(time as f64, wf).clamp(0.0, wf);
    let history = crate::clock::ClockHistory::new(timeline.clone());
    let stretches = timeline.stretches();
    // Stretches and the gaps between them, in time order.
    let mut gaps: Vec<(u64, u64)> = Vec::new();
    let mut from = first;
    for s in stretches {
        if s.begin > from {
            gaps.push((from, s.begin));
        }
        from = from.max(s.end);
    }
    if !timeline.is_open() && last > from {
        gaps.push((from, last));
    }
    let mut dashed: Vec<[Point; 2]> = Vec::new();
    // Each label with the span it must fit in: a band's inside it, "gated" in its gap.
    let mut labels: Vec<(f32, f32, String, Color)> = Vec::new();
    p.scene.clipped(clip, |scene| {
        for s in stretches {
            let (a, b) = (col(s.begin), col(s.end));
            if b <= 0.0 || a >= wf || (b - a) < 0.5 {
                continue;
            }
            if s.period as f64 / 2.0 * ppu >= CLOCK_BAND_HALF_PX {
                let columns = a.floor() as usize..(b.ceil() as usize).min(w_px);
                paint_bits(&history, vp, area, t, scene, false, columns);
                continue;
            }
            let (xa, xb) = (area.left() + a as f32, area.left() + b as f32);
            scene.fill(
                Rect::from_xywh(xa, top, xb - xa, bottom - top),
                t.wave_signal.with_alpha(0.16),
            );
            scene.fill(Rect::from_xywh(xa, top, xb - xa, 1.0), t.wave_signal);
            scene.fill(
                Rect::from_xywh(xa, bottom - 1.0, xb - xa, 1.0),
                t.wave_signal,
            );
            let period = format_time(s.period as f64, base);
            let label = match crate::clock::frequency(s.period as f64, base) {
                Some(f) => format!("{f} · {period}"),
                None => period,
            };
            labels.push((xa, xb, label, t.wave_bus_text));
        }
        for &(ga, gb) in &gaps {
            let (a, b) = (col(ga), col(gb));
            if b - a < 1.0 {
                continue;
            }
            let (xa, xb) = (area.left() + a as f32, area.left() + b as f32);
            marks::dashes(
                &mut dashed,
                xa,
                xb,
                bottom - 1.0,
                area.left(),
                scaled(marks::DASHED, t),
            );
            if xb - xa > z(GATED_LABEL_PX) {
                labels.push((xa, xb, "gated".to_owned(), t.editor.text_placeholder));
            }
        }
        scene.lines(std::mem::take(&mut dashed), t.wave_weak, 1.0);
    });
    for (xa, xb, label, color) in labels {
        let w = p.width(&label, FontRole::Mono, t.ui_size_small);
        let left = xa.max(clip.left()) + z(6.0);
        if left + w + z(6.0) <= xb.min(clip.right()) {
            p.scene.clipped(clip, |scene| {
                scene.text(
                    point(snap(left), area.top()),
                    area.height(),
                    label,
                    FontRole::Mono,
                    t.ui_size_small,
                    color,
                )
            });
        }
    }
}

/// What a group row shows besides its place.
struct GroupCells<'a> {
    row: &'a GroupRow,
    /// Rows below it that are not groups.
    count: usize,
    /// Loaded histories of the signals below it.
    members: &'a [std::sync::Arc<dyn SignalHistory>],
    /// Its name is being edited; the frontend draws the editor there.
    renaming: bool,
    /// The pointer, when it hovers the row and may read values.
    reading: Option<Point>,
}

/// A group: a chevron, its name and member count. Folded, it also reads its
/// members at the cursor ("2 of 9 changed · 1 X"), draws their merged
/// activity, and under the pointer lists their values.
fn paint_group_row(
    group: &GroupCells<'_>,
    model: &WaveModel,
    doc: &Document,
    cells: &LaneCells<'_>,
    char_w: f32,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let layout = cells.layout;
    let (names, values, waves) = (layout.names, layout.values, layout.waves);
    let row_h = layout.row_h;
    let y = layout.row_y(cells.pos);
    let full_h = layout.row_height(cells.pos);
    let g = group.row;

    // Chevron: › folded, ⌄ open.
    let cx = cells.name_x + z(CHEVRON_W / 2.0);
    let cy = y + row_h / 2.0;
    let a = z(3.5);
    let chevron = if g.collapsed {
        vec![
            [point(cx - a / 2.0, cy - a), point(cx + a / 2.0, cy)],
            [point(cx + a / 2.0, cy), point(cx - a / 2.0, cy + a)],
        ]
    } else {
        vec![
            [point(cx - a, cy - a / 2.0), point(cx, cy + a / 2.0)],
            [point(cx, cy + a / 2.0), point(cx + a, cy - a / 2.0)],
        ]
    };
    p.scene.clipped(names, |scene| {
        scene.lines(chevron, cells.muted, z(1.4).max(1.0));
    });
    if !group.renaming {
        let x = cells.name_x + z(CHEVRON_W);
        let name = g.name.clone();
        let name_w = p.width(&name, FontRole::UiSemibold, t.ui_size);
        let count = group.count.to_string();
        p.scene.clipped(names, |scene| {
            scene.text(
                point(x, y),
                row_h,
                name,
                FontRole::UiSemibold,
                t.ui_size,
                cells.text,
            );
            scene.text(
                point(x + name_w + z(6.0), y),
                row_h,
                count,
                FontRole::Ui,
                t.ui_size_small,
                cells.muted,
            );
        });
    }
    if !g.collapsed {
        return;
    }

    // Values column: how many members change at the cursor, and how many are X.
    if let Some(c) = cells.cursor {
        let reading = group::Reading::at(group.members, c);
        let text = reading.text();
        let color = if reading.changed > 0 {
            cells.text
        } else {
            cells.muted
        };
        let x = values.left() + z(8.0);
        let w = p.width(&text, FontRole::Ui, t.ui_size_small);
        let undefined = (reading.undefined > 0).then(|| format!(" · {} X", reading.undefined));
        p.scene.clipped(values, |scene| {
            scene.text(
                point(x, y),
                row_h,
                text,
                FontRole::Ui,
                t.ui_size_small,
                color,
            );
            if let Some(u) = undefined {
                scene.text(
                    point(x + w, y),
                    row_h,
                    u,
                    FontRole::Ui,
                    t.ui_size_small,
                    t.value_color(ValueKind::Undef),
                );
            }
        });
    }

    // Waves column: the members' merged activity in the bus shape.
    let wave_row = Rect::from_xywh(waves.left(), y, waves.width(), full_h);
    let summary = doc.group_summary(&group::key(group.members));
    paint_group_summary(group.members, summary, &cells.viewport, wave_row, waves, p);

    // Under the pointer: each member's value at that time.
    let Some(pointer) = group.reading.filter(|pt| waves.contains(*pt)) else {
        return;
    };
    let time = cells
        .viewport
        .time_at(f64::from(pointer.x - waves.left()), layout.wave_width_f64())
        .max(0.0) as u64;
    let Some(ix) = layout.entry(cells.pos) else {
        return;
    };
    const SHOWN: usize = 8;
    let leaves: Vec<usize> = tree::leaves(model.items(), ix).collect();
    let mut lines: Vec<(String, String, Color)> = leaves
        .iter()
        .take(SHOWN)
        .map(|&j| {
            let row = &model.items()[j];
            let (value, color) = match row.signal() {
                Some(s) => match &s.history {
                    Some(h) => {
                        let tr = s.translator.translate(&h.value(h.index_at(time)));
                        let color = if tr.kind == ValueKind::Normal {
                            t.tooltip.text
                        } else {
                            t.value_color(tr.kind)
                        };
                        (tr.text, color)
                    }
                    None => ("–".into(), t.editor.text_placeholder),
                },
                None => ("–".into(), t.editor.text_placeholder),
            };
            (row.name().to_owned(), value, color)
        })
        .collect();
    if leaves.len() > SHOWN {
        lines.push((
            format!("+{} more", leaves.len() - SHOWN),
            String::new(),
            t.editor.text_placeholder,
        ));
    }
    let header = format!("{} · {}", format_time(time as f64, doc.time_base()), g.name);
    let line_h = z(18.0);
    let name_w = lines
        .iter()
        .map(|(n, _, _)| n.chars().count())
        .max()
        .unwrap_or(0) as f32
        * char_w;
    let value_w = lines
        .iter()
        .map(|(_, v, _)| v.chars().count())
        .max()
        .unwrap_or(0) as f32
        * char_w;
    let header_w = p.width(&header, FontRole::Ui, t.ui_size_small);
    let w = (name_w + value_w + z(12.0)).max(header_w) + z(14.0);
    let h = line_h * (lines.len() + 1) as f32 + z(8.0);
    let x = (pointer.x + z(14.0))
        .min(waves.right() - w - z(4.0))
        .max(waves.left());
    let top = (pointer.y + z(14.0))
        .min(waves.bottom() - h - z(4.0))
        .max(waves.top());
    let chip = Rect::from_xywh(snap(x), snap(top), w, h);
    p.scene.clipped(waves, |scene| {
        scene.quad(chip, t.tooltip.bg, z(4.0), 1.0, t.border);
        let left = chip.left() + z(7.0);
        let mut ly = chip.top() + z(4.0);
        scene.text(
            point(left, ly),
            line_h,
            header,
            FontRole::Ui,
            t.ui_size_small,
            t.editor.text_placeholder,
        );
        for (name, value, color) in lines {
            ly += line_h;
            scene.text(
                point(left, ly),
                line_h,
                name,
                FontRole::Mono,
                t.mono_size,
                t.editor.text_placeholder,
            );
            scene.text(
                point(left + name_w + z(12.0), ly),
                line_h,
                value,
                FontRole::Mono,
                t.mono_size,
                color,
            );
        }
    });
}

/// A folded group's activity: a box between consecutive changes of any
/// member, a band where changes are denser than boxes can show, and the X
/// colour wherever a member is undefined.
fn paint_group_summary(
    members: &[std::sync::Arc<dyn SignalHistory>],
    summary: Option<&group::SummaryLoad>,
    vp: &Viewport,
    area: Rect,
    clip: Rect,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let w_px = area.width().floor().max(0.0) as usize;
    if w_px == 0 || members.is_empty() {
        return;
    }
    // Few visible changes are walked; more read the summary, and while it
    // builds the row says so rather than scanning on the UI thread.
    let columns = if group::visible_changes(members, vp) <= group::WALK_MAX {
        group::walk(members, vp, w_px)
    } else {
        match summary {
            Some(group::SummaryLoad::Ready(s)) => s
                .columns(vp, w_px)
                .unwrap_or_else(|| group::walk(members, vp, w_px)),
            Some(group::SummaryLoad::Building) => {
                p.scene.clipped(intersect(area, clip), |scene| {
                    scene.text(
                        point(area.left() + 8.0 * t.zoom, area.top()),
                        area.height(),
                        "Summarizing…",
                        FontRole::Ui,
                        t.ui_size_small,
                        t.editor.text_placeholder,
                    );
                });
                return;
            }
            _ => group::walk(members, vp, w_px),
        }
    };
    let x0 = area.left();
    let top = area.top() + TRACE_PAD * t.zoom;
    let bottom = area.bottom() - TRACE_PAD * t.zoom;
    let mid = top + (bottom - top) / 2.0;

    // Column counts → segments, as the bus painter does.
    let mut segments: Vec<(usize, usize, bool)> = Vec::new();
    let mut start = 0usize;
    for (x, c) in columns.iter().enumerate() {
        if c.changes == 0 {
            continue;
        }
        if x > start {
            segments.push((start, x, false));
        }
        if c.changes == 1 {
            start = x;
        } else {
            match segments.last_mut() {
                Some(last) if last.2 && last.1 == x => last.1 = x + 1,
                _ => segments.push((x, x + 1, true)),
            }
            start = x + 1;
        }
    }
    if w_px > start {
        segments.push((start, w_px, false));
    }
    let mut merged: Vec<(usize, usize, bool)> = Vec::with_capacity(segments.len());
    for (a, b, dense) in segments {
        let dense = dense || (b - a < MIN_SEGMENT_PX && a != 0 && b != w_px);
        match merged.last_mut() {
            Some(last) if last.2 && dense && last.1 == a => last.1 = b,
            _ => merged.push((a, b, dense)),
        }
    }

    let undef = t.value_color(ValueKind::Undef);
    let slant_w = 3.0 * t.zoom;
    p.scene.clipped(clip, |scene| {
        let mut slants: Vec<[Point; 2]> = Vec::new();
        let mut undef_slants: Vec<[Point; 2]> = Vec::new();
        for (a, b, dense) in merged {
            let (xa, xb) = (x0 + a as f32, x0 + b as f32);
            // A box's first column holds the change into it, and with it the
            // value before; only a band's own columns all count.
            let own = if dense || a == 0 { a } else { (a + 1).min(b) };
            let undefined = columns[own..b].iter().any(|c| c.undefined);
            if dense {
                let color = if undefined { undef } else { t.wave_dense };
                scene.fill(Rect::from_xywh(xa, top, xb - xa, bottom - top), color);
                continue;
            }
            // Undefined stretches inside a box are filled column by column.
            let mut x = own;
            while x < b {
                if !columns[x].undefined {
                    x += 1;
                    continue;
                }
                let run = columns[x..b].iter().take_while(|c| c.undefined).count();
                scene.fill(
                    Rect::from_xywh(x0 + x as f32, top, run as f32, bottom - top),
                    undef.with_alpha(0.14),
                );
                x += run;
            }
            let color = if undefined { undef } else { t.wave_signal };
            let tw = slant_w.min((xb - xa) / 2.0);
            let (open_left, open_right) = (a == 0, b == w_px);
            let la = if open_left { xa } else { xa + tw };
            let rb = if open_right { xb } else { xb - tw };
            if rb > la {
                scene.fill(Rect::from_xywh(la, top, rb - la, 1.0), color);
                scene.fill(Rect::from_xywh(la, bottom - 1.0, rb - la, 1.0), color);
            }
            let lines = if undefined {
                &mut undef_slants
            } else {
                &mut slants
            };
            let (topf, botf) = (top + 0.5, bottom - 0.5);
            if !open_left {
                lines.push([point(xa, mid), point(xa + tw, topf)]);
                lines.push([point(xa, mid), point(xa + tw, botf)]);
            }
            if !open_right {
                lines.push([point(xb, mid), point(xb - tw, topf)]);
                lines.push([point(xb, mid), point(xb - tw, botf)]);
            }
        }
        scene.lines(slants, t.wave_signal, 1.0);
        scene.lines(undef_slants, undef, 1.0);
    });
}

/// A bar prepared for painting: its lifetime, the solid stage spans inside
/// it, and a caption when one fits.
struct Bar {
    rect: Rect,
    color: Color,
    solid: Vec<Rect>,
    label: Option<(Point, String)>,
    selected: bool,
}

/// Paint a transaction lane: the generator name (with the number of folded
/// sub-rows), the records open at the cursor, and the bars or, zoomed out,
/// the density strip. One window visit per frame bounds the work by the
/// visible records.
fn paint_lane_row(lane: &TxLane, doc: &Document, cells: &LaneCells<'_>, p: &mut TextPainter<'_>) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let layout = cells.layout;
    let (names, values, waves) = (layout.names, layout.values, layout.waves);
    let row_h = layout.row_h;
    let y = layout.row_y(cells.pos);
    let full_h = layout.row_height(cells.pos);
    let data = lane.data(doc);
    let generator = match data {
        LaneData::Ready(g) => Some(g),
        _ => None,
    };
    let folded = generator.map_or(0, |g| lane::folded(g.depth(), lane.height));

    // Name column: the generator, a "+N" chip while sub-rows are folded,
    // and on taller rows the record count and the stacking.
    let pad = cells.name_x - names.left();
    let name_w = p.width(&lane.name, FontRole::Ui, t.ui_size);
    let name_color = if lane.track().is_some() {
        cells.text
    } else {
        cells.muted
    };
    let name = lane.name.clone();
    p.scene.clipped(names, |scene| {
        scene.text(
            point(names.left() + pad, y),
            row_h,
            name,
            FontRole::Ui,
            t.ui_size,
            name_color,
        );
    });
    if folded > 0 {
        let chip = format!("+{folded}");
        let chip_w = p.width(&chip, FontRole::UiMedium, t.ui_size_small) + z(8.0);
        let rect = Rect::from_xywh(
            names.left() + pad + name_w + z(6.0),
            y + z(5.0),
            chip_w,
            row_h - z(10.0),
        );
        p.scene.clipped(names, |scene| {
            scene.quad(rect, t.badge.bg, z(3.0), 1.0, t.border_variant);
            scene.text(
                point(rect.left() + z(4.0), rect.top()),
                rect.height(),
                chip,
                FontRole::UiMedium,
                t.ui_size_small,
                t.badge.text_muted,
            );
        });
    }
    if let Some(g) = generator {
        let mut lines = Vec::new();
        if full_h >= 2.0 * row_h {
            let n = g.transactions().len();
            lines.push(format!("{n} record{}", if n == 1 { "" } else { "s" }));
        }
        if full_h >= 3.0 * row_h {
            lines.push(if folded > 0 {
                format!(
                    "{folded} sub-row{} folded",
                    if folded == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "{} sub-row{}",
                    g.depth(),
                    if g.depth() == 1 { "" } else { "s" }
                )
            });
        }
        for (k, line) in lines.into_iter().enumerate() {
            let origin = point(names.left() + pad, y + row_h * (k + 1) as f32);
            p.scene.clipped(names, |scene| {
                scene.text(
                    origin,
                    row_h,
                    line,
                    FontRole::Ui,
                    t.ui_size_small,
                    cells.muted,
                );
            });
        }
    }

    // Values column: the records open at the cursor.
    let (value, value_color) = match data {
        LaneData::Ready(g) => match cells.cursor {
            Some(c) => {
                let (text, failed) = lane::value_text(g, c);
                (text, if failed { t.editor.error } else { cells.text })
            }
            None => (String::new(), cells.text),
        },
        LaneData::Loading => ("loading…".into(), cells.muted),
        LaneData::Unresolved => ("not in trace".into(), cells.muted),
        LaneData::Failed(_) | LaneData::Unavailable => ("–".into(), cells.muted),
    };
    let char_w = p.width("0", FontRole::Mono, t.mono_size).max(1.0);
    let max_chars = ((values.width() - z(16.0)) / char_w).floor().max(0.0) as usize;
    if let Some(text) = truncate_chars(&value, max_chars) {
        p.scene.clipped(values, |scene| {
            scene.text(
                point(values.left() + z(8.0), y),
                row_h,
                text,
                FontRole::Mono,
                t.mono_size,
                value_color,
            );
        });
    }

    // Waves column.
    let status = match data {
        LaneData::Ready(g) => {
            let width = f64::from(waves.width()).max(1.0);
            if lane::is_density(g, cells.viewport.px_per_unit(width)) {
                paint_lane_density(
                    g,
                    &cells.viewport,
                    Rect::from_xywh(waves.left(), y, waves.width(), full_h),
                    p,
                );
            } else {
                let geometry = LaneGeometry::new(y, full_h, row_h, lane.height);
                paint_lane_bars(lane, g, doc, &cells.viewport, waves, &geometry, p);
            }
            return;
        }
        LaneData::Loading => None,
        LaneData::Failed(error) => Some((format!("Failed to load: {error}"), t.editor.error)),
        LaneData::Unresolved => Some(("not in trace".to_owned(), cells.muted)),
        LaneData::Unavailable => Some(("records unavailable".to_owned(), cells.muted)),
    };
    match status {
        Some((label, color)) => p.scene.clipped(waves, |scene| {
            scene.text(
                point(waves.left() + z(8.0), y),
                row_h,
                label,
                FontRole::Ui,
                t.ui_size_small,
                color,
            )
        }),
        None => {
            let bar = Rect::new(
                point(waves.left() + z(8.0), y + full_h / 2.0),
                size(z(96.0), 1.0),
            );
            p.scene.clipped(waves, |scene| scene.fill(bar, t.border));
        }
    }
}

fn paint_lane_bars(
    lane: &TxLane,
    generator: &crate::data::loaded_tracks::LoadedGenerator,
    doc: &Document,
    viewport: &Viewport,
    waves: Rect,
    geometry: &LaneGeometry,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let width = f64::from(waves.width()).max(1.0);
    // Clamp far-off-screen coordinates so rectangles stay finite and small.
    let x_of = |time: u64| {
        (waves.left() + viewport.x_of(time as f64, width) as f32)
            .clamp(waves.left() - z(8.0), waves.right() + z(8.0))
    };
    let inset = z(2.0);
    let bar_h = (geometry.sub_h - 2.0 * inset).max(1.0);
    let font = (bar_h * 0.9).min(t.mono_size);
    let last = geometry.capacity - 1;
    let folds = generator.depth() > geometry.capacity;
    let selected = doc
        .selection()
        .filter(|s| Some(s.track) == lane.track())
        .map(|s| s.id);
    let (start, end) = lane::window(viewport);
    let mut raw = Vec::new();
    _ = generator.visit_window_ordinals(start, end, |ordinal, tx| {
        raw.push((ordinal, tx));
        true
    });
    let mut bars = Vec::with_capacity(raw.len());
    for (ordinal, tx) in raw {
        let sub = lane::shown_sub_row(generator.sub_row(ordinal), lane.height);
        let xa = x_of(tx.begin);
        let xb = x_of(tx.end).max(xa + z(2.0));
        let rect = Rect::from_xywh(xa, geometry.sub_top(sub) + inset, xb - xa, bar_h);
        let color = if tx.status == TxStatus::Error {
            t.editor.error
        } else {
            t.wave_signal
        };
        let solid = if tx.stages.is_empty() {
            vec![rect]
        } else {
            tx.stages
                .iter()
                .map(|stage| {
                    let a = x_of(stage.begin).max(xa);
                    let b = x_of(PipelineModel::stage_end(tx, stage)).min(xb);
                    Rect::from_xywh(a, rect.top(), (b - a).max(1.0), bar_h)
                })
                .collect()
        };
        let in_fold = folds && sub == last;
        let label = (!in_fold && bar_h >= z(8.0))
            .then(|| {
                let text = lane::label(tx);
                let lx = xa.max(waves.left()) + z(4.0);
                let w = p.width(&text, FontRole::Mono, font);
                (xb - lx >= w + z(4.0)).then(|| (point(lx, rect.top()), text))
            })
            .flatten();
        bars.push(Bar {
            rect,
            color,
            solid,
            label,
            selected: selected == Some(tx.id),
        });
    }
    let hatch = lane::fold_overlaps(generator, viewport, lane.height);
    let fold_top = geometry.sub_top(last) + inset;
    let text_color = t.editor.bg;
    p.scene.clipped(waves, |scene| {
        for bar in &bars {
            scene.fill(bar.rect, bar.color.with_alpha(0.4));
            for solid in &bar.solid {
                scene.fill(*solid, bar.color);
            }
            scene.quad(bar.rect, Color::TRANSPARENT, z(2.0), 1.0, bar.color);
            if let Some((origin, text)) = &bar.label {
                scene.text(
                    *origin,
                    bar_h,
                    text.clone(),
                    FontRole::Mono,
                    font,
                    text_color,
                );
            }
        }
        // Folded bars overlap where hatched.
        let step = z(4.0).max(3.0);
        for (a, b) in hatch {
            let (xa, xb) = (x_of(a), x_of(b));
            let area = Rect::from_xywh(xa, fold_top, (xb - xa).max(1.0), bar_h);
            let mut segments = Vec::new();
            let mut x = area.left() - bar_h;
            while x < area.right() {
                segments.push([point(x, area.bottom()), point(x + bar_h, area.top())]);
                x += step;
            }
            scene.clipped(area, |scene| {
                scene.lines(segments, t.editor.bg.with_alpha(0.7), 1.0);
            });
        }
        for bar in bars.iter().filter(|bar| bar.selected) {
            let r = bar.rect;
            scene.quad(
                Rect::from_xywh(
                    r.left() - 2.0,
                    r.top() - 2.0,
                    r.width() + 4.0,
                    r.height() + 4.0,
                ),
                Color::TRANSPARENT,
                z(3.0),
                2.0,
                t.border_focused,
            );
        }
    });
}

/// One column per pixel: its height is the share of the generator's depth
/// open there, and a red cap marks a failed record.
fn paint_lane_density(
    generator: &crate::data::loaded_tracks::LoadedGenerator,
    viewport: &Viewport,
    area: Rect,
    p: &mut TextPainter<'_>,
) {
    let t = p.theme;
    let z = |v: f32| v * t.zoom;
    let columns = lane::density(generator, viewport, area.width().max(0.0).round() as usize);
    let depth = f32::from(generator.depth().max(1));
    let base = area.bottom() - z(3.0);
    let full = (area.height() - z(6.0)).max(1.0);
    let px = area.width() / columns.len().max(1) as f32;
    p.scene.clipped(area, |scene| {
        scene.fill(
            Rect::from_xywh(area.left(), base, area.width(), 1.0),
            t.border_variant,
        );
        let mut x = 0;
        while x < columns.len() {
            let column = columns[x];
            let run = columns[x..].iter().take_while(|c| **c == column).count();
            if column.open > 0 {
                let share = (column.open as f32 / depth).min(1.0);
                let h = (full * share).max(2.0);
                let left = area.left() + x as f32 * px;
                let w = run as f32 * px;
                scene.fill(
                    Rect::from_xywh(left, base - h, w, h),
                    t.wave_signal.with_alpha(0.45 + 0.5 * share),
                );
                if column.failed {
                    scene.fill(
                        Rect::from_xywh(left, base - h - z(3.0), w.max(2.0), z(3.0)),
                        t.editor.error,
                    );
                }
            }
            x += run;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::history::VecHistory;
    use crate::scene::{MonoMeasure, Prim};

    #[test]
    fn labels_keep_a_numbers_last_digits_and_texts_beginning() {
        assert_eq!(fit_label("80000210", 8, true).as_deref(), Some("80000210"));
        assert_eq!(fit_label("80000210", 5, true).as_deref(), Some("…0210"));
        assert_eq!(
            fit_label("dot product loop", 6, false).as_deref(),
            Some("dot p…")
        );
        // Fewer than two characters would show: dropped, never a lone ellipsis.
        assert_eq!(fit_label("80000210", 2, true), None);
        assert_eq!(fit_label("5", 1, true).as_deref(), Some("5"));
        assert_eq!(fit_label("", 4, false), None);
    }

    #[test]
    fn a_bus_value_picks_its_shape() {
        let bits = |s: &str| WaveValue::Bits(s.into());
        assert_eq!(bus_look(&bits("0000"), true), BusLook::Zero);
        assert_eq!(bus_look(&bits("0000"), false), BusLook::Value);
        assert_eq!(bus_look(&bits("zzzz"), true), BusLook::Floating);
        assert_eq!(bus_look(&bits("xxXX"), true), BusLook::Unknown);
        assert_eq!(bus_look(&bits("x0z1"), true), BusLook::Value);
        assert_eq!(
            bus_look(&WaveValue::Text("idle".into()), false),
            BusLook::Value
        );
        let t = Theme::volna(true);
        let runs = label_runs("3x0z", &t, t.wave_bus_text);
        let colors: Vec<_> = runs.iter().map(|(at, _, c)| (*at, *c)).collect();
        assert_eq!(
            colors,
            [
                (0, t.wave_bus_text),
                (1, t.wave_undef),
                (2, t.wave_bus_text),
                (3, t.wave_highimp)
            ]
        );
    }

    fn bus(values: &[(u64, &str)]) -> VecHistory {
        VecHistory {
            shape: SignalShape::Vector { width: 8 },
            times: values.iter().map(|(t, _)| *t).collect(),
            values: values
                .iter()
                .map(|(_, v)| WaveValue::Bits((*v).into()))
                .collect(),
            initial: WaveValue::Bits("00000000".into()),
        }
    }

    fn paint_bus(h: &VecHistory, translator: &str, t: &Theme) -> Scene {
        let translators = crate::data::Translators::builtin();
        let translator = translators.get(translator).unwrap();
        let mut scene = Scene::default();
        let mut text = TextCache::default();
        let mut measure = MonoMeasure;
        let mut p = TextPainter {
            theme: t,
            text: &mut text,
            measure: &mut measure,
            scene: &mut scene,
        };
        let area = Rect::from_xywh(0.0, 0.0, 400.0, 24.0);
        let vp = Viewport {
            start: 0.0,
            end: 400.0,
        };
        let char_w = MonoMeasure.text_width("0", FontRole::Mono, t.mono_size);
        paint_bus_row(h, translator.as_ref(), &vp, area, area, char_w, &mut p);
        scene
    }

    #[test]
    fn buses_draw_zero_low_z_mid_and_unknowns_hatched() {
        let t = Theme::volna(true);
        // 0: zero, 100: a value, 200: all X, 300: all Z.
        let h = bus(&[(100, "10000000"), (200, "xxxxxxxx"), (300, "zzzzzzzz")]);
        let scene = paint_bus(&h, "hex", &t);
        let texts: Vec<_> = scene.texts().collect();
        // Zero is idle: no label. The value and the unknown are labelled.
        assert_eq!(texts, ["80", "xx"]);
        let quads: Vec<_> = scene.quads().collect();
        // The zero stretch is a low line from its left edge to its slant.
        assert!(quads.iter().any(|(r, c)| *c == t.wave_signal
            && r.left() == 0.0
            && r.top() == 18.0
            && r.height() == 1.0));
        // All X: tinted and hatched inside its hexagon.
        assert!(
            quads
                .iter()
                .any(|(r, c)| *c == t.wave_undef_fill && r.left() >= 200.0 && r.right() <= 300.0)
        );
        assert!(scene.prims.iter().any(|p| matches!(p, Prim::Lines { color, .. } if *color == t.wave_undef.with_alpha(marks::HATCH_ALPHA))));
        // All Z: one bare line at mid level, no hexagon.
        let z: Vec<_> = quads.iter().filter(|(_, c)| *c == t.wave_highimp).collect();
        assert_eq!(z.len(), 1);
        assert_eq!((z[0].0.left(), z[0].0.height()), (299.0, 1.0));
        // Slants run 4px either side of a change.
        let slants = scene.prims.iter().find_map(|p| match p {
            Prim::Lines {
                segments, color, ..
            } if *color == t.wave_signal => Some(segments.clone()),
            _ => None,
        });
        assert!(
            slants
                .unwrap()
                .iter()
                .any(|[a, b]| a.x == 99.0 && b.x == 103.0)
        );
    }

    #[test]
    fn partial_unknowns_keep_the_outline_and_colour_their_digits() {
        let t = Theme::volna(false);
        let h = bus(&[(100, "0011xxxx")]);
        let scene = paint_bus(&h, "hex", &t);
        let runs: Vec<_> = scene
            .prims
            .iter()
            .filter_map(|p| match p {
                Prim::Text { text, color, .. } => Some((text.as_str(), *color)),
                _ => None,
            })
            .collect();
        assert!(runs.contains(&("3", t.wave_bus_text)), "{runs:?}");
        assert!(
            runs.iter()
                .any(|(s, c)| s.contains('x') && *c == t.wave_undef),
            "{runs:?}"
        );
        assert!(
            scene.quads().all(|(_, c)| c != t.wave_undef_fill),
            "not the X band"
        );
    }

    #[test]
    fn text_values_get_a_stable_tint_and_numbers_stay_outlines() {
        let t = Theme::volna(true);
        let states = VecHistory {
            shape: SignalShape::Vector { width: 8 },
            times: vec![100, 200, 300],
            values: ["idle", "busy", "idle"]
                .map(|s| WaveValue::Text(s.into()))
                .to_vec(),
            initial: WaveValue::Text("busy".into()),
        };
        let scene = paint_bus(&states, "text", &t);
        let idle = t.value_tint("idle");
        let tinted = scene.quads().filter(|(_, c)| *c == idle).count();
        assert!(tinted >= 2, "both idle stretches are tinted");
        let numbers = paint_bus(&bus(&[(100, "00000011")]), "hex", &t);
        assert!(numbers.quads().all(|(_, c)| (c.a - 0.22).abs() > 1e-3));
    }

    #[test]
    fn x_and_dont_care_and_weak_bits_have_their_own_shapes() {
        let t = Theme::volna(true);
        let h = VecHistory {
            shape: SignalShape::Bit,
            times: vec![100, 200, 300],
            values: ["x", "-", "l"].map(|s| WaveValue::Bits(s.into())).to_vec(),
            initial: WaveValue::Bits("0".into()),
        };
        let mut scene = Scene::default();
        let area = Rect::from_xywh(0.0, 0.0, 400.0, 24.0);
        paint_bit_row(
            &h,
            &Viewport {
                start: 0.0,
                end: 400.0,
            },
            area,
            &t,
            &mut scene,
        );
        let lines = |color: Color| {
            scene.prims.iter().filter_map(move |p| match p {
                Prim::Lines {
                    segments, color: c, ..
                } if *c == color => Some(segments.len()),
                _ => None,
            })
        };
        // A change is drawn in the pixel column whose right edge reaches it.
        assert!(
            scene
                .quads()
                .any(|(r, c)| c == t.wave_undef_fill && r.left() == 100.0 && r.width() == 99.0)
        );
        assert!(
            lines(t.wave_undef.with_alpha(marks::HATCH_ALPHA)).sum::<usize>() >= 20,
            "hatched every 5px"
        );
        assert!(
            scene
                .quads()
                .any(|(r, c)| c == t.wave_undef && r.left() == 100.0 && r.height() == 1.0),
            "a mid-level line"
        );
        // Dotted: a dash every 3px; dashed: every 7px.
        let dotted: usize = lines(t.wave_dontcare).sum();
        let dashed: usize = lines(t.wave_weak).sum();
        assert!((32..=35).contains(&dotted), "{dotted}");
        assert!((14..=16).contains(&dashed), "{dashed}");
    }

    #[test]
    fn aliased_columns_show_their_strength_and_their_unknowns() {
        let t = Theme::volna(true);
        let bits = |s: &str| WaveValue::Bits(s.into());
        // Columns of 400 units at 1px per 100: 0..100 toggles 0/1 twice per
        // column, 100..200 toggles thirty times, 200..300 mixes in X, and
        // 300..400 changes only between unknowns.
        let mut times = Vec::new();
        let mut values = Vec::new();
        for c in 0..4u64 {
            let (n, pick): (u64, fn(u64) -> &'static str) = match c {
                0 => (2, |i| if i % 2 == 0 { "1" } else { "0" }),
                1 => (30, |i| if i % 2 == 0 { "1" } else { "0" }),
                2 => (4, |i| {
                    if i == 1 {
                        "x"
                    } else if i % 2 == 0 {
                        "1"
                    } else {
                        "0"
                    }
                }),
                _ => (4, |i| if i % 2 == 0 { "x" } else { "u" }),
            };
            for column in 0..100u64 {
                for i in 0..n {
                    times.push(c * 10_000 + column * 100 + 1 + i * (98 / n));
                    values.push(bits(pick(i)));
                }
            }
        }
        let h = VecHistory {
            shape: SignalShape::Bit,
            times,
            values,
            initial: bits("x"),
        };
        let mut scene = Scene::default();
        let area = Rect::from_xywh(0.0, 0.0, 400.0, 24.0);
        paint_bit_row(
            &h,
            &Viewport {
                start: 0.0,
                end: 40_000.0,
            },
            area,
            &t,
            &mut scene,
        );
        let fill_at = |x: f32| {
            scene
                .quads()
                .filter(|(r, c)| r.left() <= x && r.right() > x && r.height() > 2.0 && c.a > 0.1)
                .map(|(_, c)| c)
                .last()
                .unwrap()
        };
        assert!(
            fill_at(150.0).a > fill_at(50.0).a,
            "thirty changes read stronger than two"
        );
        assert_eq!(
            fill_at(350.0),
            t.wave_undef_fill,
            "only unknowns: the X band"
        );
        let rails = scene
            .quads()
            .filter(|(r, c)| {
                *c == t.wave_undef && r.height() == 1.0 && (200.0..300.0).contains(&r.left())
            })
            .count();
        assert_eq!(rails, 2, "X mixed into traffic: rails above and below");
    }

    #[test]
    fn clocks_show_periods_bands_and_gated_gaps() {
        let t = Theme::volna(true);
        // 2 ns from 0 to 400, gated until 600, then 4 ns to 1000.
        let timeline = std::sync::Arc::new(
            vtr::clock::ClockTimeline::new(vec![(0, 400, 2), (600, 1000, 4)], false).unwrap(),
        );
        let base = crate::wave::timeline::TimeBase {
            timescale: -9,
            unit: None,
        };
        let draw = |vp: Viewport| {
            let mut scene = Scene::default();
            let mut text = TextCache::default();
            let mut measure = MonoMeasure;
            let mut p = TextPainter {
                theme: &t,
                text: &mut text,
                measure: &mut measure,
                scene: &mut scene,
            };
            let area = Rect::from_xywh(0.0, 0.0, 500.0, 24.0);
            paint_clock_wave(&timeline, &vp, area, area, (0, 1000), base, &mut p);
            scene
        };
        let whole = draw(Viewport {
            start: 0.0,
            end: 1000.0,
        });
        let texts: Vec<_> = whole.texts().collect();
        assert_eq!(texts, ["500.00 MHz · 2 ns", "250.00 MHz · 4 ns", "gated"]);
        // No high fill on a clock: its band is the only wide translucent quad.
        assert!(whole.quads().all(|(_, c)| c != t.wave_high_fill));
        assert!(
            whole
                .prims
                .iter()
                .any(|p| matches!(p, Prim::Lines { color, .. } if *color == t.wave_weak))
        );
        // Closer, edges 3px apart or more are a square wave again, without labels.
        let close = draw(Viewport {
            start: 100.0,
            end: 200.0,
        });
        assert_eq!(close.texts().count(), 0);
        assert!(
            close
                .quads()
                .any(|(r, c)| c == t.wave_signal && r.width() == 1.0 && r.height() > 2.0)
        );
        assert!(close.quads().all(|(_, c)| c != t.wave_high_fill));
    }

    #[test]
    fn coalesced_events_count_themselves_where_there_is_room() {
        let t = Theme::volna(true);
        let h = VecHistory {
            shape: SignalShape::Event,
            // Three in one pixel, one alone, then two squeezed next to a neighbour.
            times: vec![100, 100, 100, 200, 390, 390, 392],
            values: vec![WaveValue::Bits("1".into()); 7],
            initial: WaveValue::Unavailable,
        };
        let mut scene = Scene::default();
        let mut text = TextCache::default();
        let mut measure = MonoMeasure;
        let area = Rect::from_xywh(0.0, 0.0, 400.0, 24.0);
        let vp = Viewport {
            start: 0.0,
            end: 400.0,
        };
        let counts = paint_event_row(&h, &vp, area, &t, &mut scene);
        assert_eq!(counts.iter().map(|c| c.1).collect::<Vec<_>>(), [3, 2]);
        let mut p = TextPainter {
            theme: &t,
            text: &mut text,
            measure: &mut measure,
            scene: &mut scene,
        };
        paint_event_counts(&counts, area, area, &mut p);
        assert_eq!(
            scene.texts().collect::<Vec<_>>(),
            ["×3"],
            "no room beside the second"
        );
    }
}
