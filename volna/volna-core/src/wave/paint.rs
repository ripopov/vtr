//! Paint the wave panel into a [`Scene`]: the three aligned columns (names,
//! values, waves) plus the timeline header, cursor, markers and a scrollbar.
//!
//! Waveforms are sampled per pixel column: for every column the painter asks
//! the history for the index of the last change at the column's right edge
//! (an exponential search from the previous column's index), so the cost of a
//! frame is O(columns x log(changes)) regardless of trace length.

use crate::data::{Bit, SignalHistory, SignalShape, Translator, ValueKind, WaveValue};
use crate::document::Document;
use crate::geometry::CursorIcon;
use crate::geometry::{Rect, point, size, snap};
use crate::icons::IconName;
use crate::scene::{FontRole, Scene, TextCache, TextMeasure};
use crate::theme::Theme;
use crate::wave::layout::{SCROLLBAR_W, WaveLayout};
use crate::wave::model::{Drag, RowSource, WaveModel};
use crate::wave::timeline::{format_time, ticks};
use crate::wave::viewport::Viewport;

const TICK_SPACING_PX: f64 = 96.0;
/// Vertical inset of the trace inside a row.
const TRACE_PAD: f32 = 5.0;
/// Segments narrower than this are drawn as a dense band instead of a hexagon.
const MIN_SEGMENT_PX: usize = 5;

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

fn changes_between(a: Option<usize>, b: Option<usize>) -> usize {
    match (a, b) {
        (None, None) | (Some(_), None) => 0,
        (None, Some(j)) => j + 1,
        (Some(i), Some(j)) => j.saturating_sub(i),
    }
}

struct Painter<'a> {
    theme: &'a Theme,
    text: &'a mut TextCache,
    measure: &'a mut dyn TextMeasure,
    scene: &'a mut Scene,
    char_w: f32,
}

impl Painter<'_> {
    fn width(&mut self, text: &str, font: FontRole, size: f32) -> f32 {
        self.text.width(self.measure, text, font, size)
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
    let char_w = text.width(measure, "0", FontRole::Mono, t.mono_size);
    let mut p = Painter {
        theme,
        text,
        measure,
        scene,
        char_w,
    };
    let viewport = model.viewport(doc);
    let cursor = model.cursor(doc);
    let timescale = doc.timescale();
    let has_source = doc.is_loaded();
    let waves = layout.waves;
    let wave_wf = layout.wave_width_f64();

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
    let (tick_list, unit) = ticks(&viewport, wave_wf, timescale, TICK_SPACING_PX);
    p.scene.clipped(waves, |scene| {
        for tick in &tick_list {
            let x = snap(waves.left() + viewport.x_of(tick.time, wave_wf) as f32);
            scene.fill(
                Rect::new(point(x, waves.top()), size(1.0, waves.height())),
                t.wave_tick,
            );
        }
    });

    // -- rows ----------------------------------------------------------------
    let row_h = layout.row_h;
    for ix in layout.rows.clone() {
        let Some(item) = model.items.get(ix) else {
            continue;
        };
        let y = layout.row_y(ix);
        let is_selected = model.selected.contains(&ix);
        let is_hover = model.hover_row == Some(ix);
        let left_row = Rect::new(
            point(bounds.left(), y),
            size(layout.names.width() + layout.values.width(), row_h),
        );
        let wave_row = Rect::new(point(waves.left(), y), size(waves.width(), row_h));
        if is_selected {
            p.scene.fill(left_row, t.selection.bg);
            p.scene.fill(wave_row, t.wave_row_selected);
            if t.appearance.is_high_contrast() {
                p.scene.quad(
                    left_row,
                    crate::color::Color::TRANSPARENT,
                    0.0,
                    1.0,
                    t.border_focused,
                );
                p.scene.quad(
                    wave_row,
                    crate::color::Color::TRANSPARENT,
                    0.0,
                    1.0,
                    t.border_focused,
                );
            }
        } else if is_hover {
            p.scene.fill(left_row, t.hover.bg);
            p.scene.fill(wave_row, t.wave_row_hover);
        }
        let colors = t.row(is_selected, is_hover);

        // Name column: leaf name, then a muted range badge for vectors.
        {
            let pad = 12.0;
            let avail = layout.names.width() - pad * 2.0;
            let dims = item.shape.dims();
            let max_chars = (avail / char_w).floor().max(0.0) as usize;
            let dims_chars = if dims.is_empty() {
                0
            } else {
                dims.chars().count() + 1
            };
            let name_budget = max_chars.saturating_sub(dims_chars.min(max_chars / 2));
            if let Some(name) = truncate_chars(&item.name, name_budget) {
                let name_w = p.width(&name, FontRole::Mono, t.mono_size);
                let origin = point(layout.names.left() + pad, y);
                let names_rect = layout.names;
                let name_len = name.chars().count();
                p.scene.clipped(names_rect, |scene| {
                    scene.text(
                        origin,
                        row_h,
                        name,
                        FontRole::Mono,
                        t.mono_size,
                        if item.source.signal().is_some() {
                            colors.text
                        } else {
                            colors.text_placeholder
                        },
                    );
                });
                if !dims.is_empty() && max_chars > name_len + 1 {
                    let rest = max_chars - name_len - 1;
                    if let Some(d) = truncate_chars(&dims, rest) {
                        p.scene.clipped(names_rect, |scene| {
                            scene.text(
                                point(origin.x + name_w + char_w, y),
                                row_h,
                                d,
                                FontRole::Mono,
                                t.mono_size,
                                colors.text_placeholder,
                            );
                        });
                    }
                }
            }
        }

        // Values column: value at cursor plus the format badge.
        {
            let pad = 8.0;
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
            if let Some(text) = truncate_chars(&value_text, max_chars) {
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
                        scene.quad(b, bg, 3.0, 1.0, t.border_variant);
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
            (Some(h), _) => match item.shape {
                SignalShape::Event => p.scene.clipped(waves, |scene| {
                    paint_event_row(h.as_ref(), &viewport, wave_row, t, scene)
                }),
                SignalShape::Bit => p.scene.clipped(waves, |scene| {
                    paint_bit_row(h.as_ref(), &viewport, wave_row, t, scene)
                }),
                _ => paint_bus_row(
                    h.as_ref(),
                    item.translator.as_ref(),
                    &viewport,
                    wave_row,
                    waves,
                    &mut p,
                ),
            },
            (None, Some(err)) => {
                let err = err.clone();
                p.scene.clipped(waves, |scene| {
                    scene.text(
                        point(waves.left() + 8.0, y),
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
                        point(waves.left() + 8.0, y),
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
                let bar = Rect::new(point(waves.left() + 8.0, y + row_h / 2.0), size(96.0, 1.0));
                p.scene.clipped(waves, |scene| scene.fill(bar, t.border));
            }
        }
    }

    // -- empty placeholder -------------------------------------------------
    if layout.rows.is_empty() && has_source {
        let area = Rect::new(
            point(bounds.left(), layout.names.top()),
            size(bounds.width(), layout.names.height()),
        );
        // The empty-state message spans the table, so give it one surface
        // instead of crossing differently themed name/value columns.
        p.scene.fill(area, t.editor.bg);
        let cx_ = area.left() + area.width() / 2.0;
        let cy = area.top() + area.height() / 2.0;
        let icon = Rect::new(point(cx_ - 16.0, cy - 44.0), size(32.0, 32.0));
        p.scene
            .icon(IconName::ListTree, icon, t.editor.text_placeholder);
        let title = "No signals displayed";
        let title_w = p.width(title, FontRole::UiMedium, t.ui_size);
        p.scene.text(
            point(snap(cx_ - title_w / 2.0), cy - 4.0),
            20.0,
            title,
            FontRole::UiMedium,
            t.ui_size,
            t.editor.text_muted,
        );
        let hint = "Double-click a variable in the sidebar, or select variables and press Enter";
        let hint_w = p.width(hint, FontRole::Ui, t.ui_size_small);
        p.scene.text(
            point(snap(cx_ - hint_w / 2.0), cy + 18.0),
            18.0,
            hint,
            FontRole::Ui,
            t.ui_size_small,
            t.editor.text_placeholder,
        );
    }

    // -- header: column titles, tick labels, unit -----------------------------
    let panel_theme = t.panel;
    let header = layout.header;
    p.scene.clipped(
        Rect::new(header.origin, size(layout.names.width(), header.height())),
        |scene| {
            scene.text(
                point(layout.names.left() + 12.0, header.top()),
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
            format_time(c as f64, timescale),
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
                point(layout.values.left() + 8.0, header.top()),
                header.height(),
                value_title,
                vfont,
                vsize,
                vcolor,
            );
        },
    );
    let header_waves = Rect::new(
        point(waves.left(), header.top()),
        size(waves.width(), header.height()),
    );
    {
        let unit_w = (!unit.is_empty()).then(|| p.width(unit, FontRole::Mono, t.ui_size_small));
        let unit_x = unit_w
            .map(|w| header_waves.right() - w - (SCROLLBAR_W + 4.0))
            .unwrap_or(header_waves.right());
        let mut labels = Vec::new();
        for tick in &tick_list {
            let x = snap(waves.left() + viewport.x_of(tick.time, wave_wf) as f32);
            let w = p.width(&tick.label, FontRole::Mono, t.ui_size_small);
            labels.push((x, tick.label.clone(), w));
        }
        p.scene.clipped(header_waves, |scene| {
            for (x, label, w) in labels {
                scene.fill(
                    Rect::new(point(x, header.bottom() - 7.0), size(1.0, 6.0)),
                    panel_theme.text_placeholder,
                );
                if x + 4.0 + w < unit_x - 8.0 {
                    scene.text(
                        point(x + 4.0, header.top() + 2.0),
                        20.0,
                        label,
                        FontRole::Mono,
                        t.ui_size_small,
                        t.wave_tick_text,
                    );
                }
            }
            if unit_w.is_some() {
                scene.text(
                    point(unit_x, header.top() + 2.0),
                    20.0,
                    unit,
                    FontRole::Mono,
                    t.ui_size_small,
                    panel_theme.text_placeholder,
                );
            }
        });
    }

    // -- markers -----------------------------------------------------------------
    for (ix, chip) in &layout.marker_chips {
        let m = &doc.markers[*ix];
        let x = snap(waves.left() + viewport.x_of(m.time as f64, wave_wf) as f32);
        let marker = t.marker(m.id.saturating_sub(1) as usize);
        let color = marker.stroke;
        p.scene.clipped(waves, |scene| {
            scene.fill(
                Rect::new(point(x, waves.top()), size(1.0, waves.height())),
                color,
            );
        });
        let hovered = model.pointer.is_some_and(|mp| chip.contains(mp));
        let bg = if hovered {
            marker.hover
        } else {
            marker.background
        };
        p.scene
            .quad(*chip, bg, 3.0, 0.0, crate::color::Color::TRANSPARENT);
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

    // -- cursor ------------------------------------------------------------------
    if let Some(c) = cursor {
        let xf = viewport.x_of(c as f64, wave_wf);
        if xf >= -1.0 && xf <= wave_wf + 1.0 {
            let x = snap(waves.left() + xf as f32);
            let label = format_time(c as f64, timescale);
            let label_w = p.width(&label, FontRole::Mono, t.ui_size_small);
            let chip_w = label_w + 10.0;
            let mut cx0 = x + 1.0;
            if cx0 + chip_w > header_waves.right() - SCROLLBAR_W {
                cx0 = x - chip_w;
            }
            let chip = Rect::new(point(cx0, header.bottom() - 18.0), size(chip_w, 16.0));
            p.scene.clipped(
                Rect::new(
                    point(waves.left(), header.top()),
                    size(waves.width(), bounds.height()),
                ),
                |scene| {
                    scene.fill(
                        Rect::new(
                            point(x, header.bottom() - 8.0),
                            size(1.0, bounds.bottom() - header.bottom() + 8.0),
                        ),
                        if focused {
                            t.wave_cursor
                        } else {
                            t.wave_cursor_inactive
                        },
                    );
                    scene.quad(
                        chip,
                        t.wave_cursor,
                        3.0,
                        0.0,
                        crate::color::Color::TRANSPARENT,
                    );
                    scene.text(
                        point(chip.left() + 5.0, chip.top()),
                        chip.height(),
                        label,
                        FontRole::Mono,
                        t.ui_size_small,
                        t.wave_cursor_text,
                    );
                },
            );
        }
    }

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
    if matches!(drag, Some(Drag::NamesSplit | Drag::ValuesSplit)) {
        p.scene.window_cursor = Some(CursorIcon::ResizeLeftRight);
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
        p.scene
            .quad(thumb, color, 3.0, 0.0, crate::color::Color::TRANSPARENT);
    }
}

// ---------------------------------------------------------------------------
// Waveform painting
// ---------------------------------------------------------------------------

/// Paint occurrences, coalescing timestamps that occupy the same pixel.
pub fn paint_event_row(
    h: &dyn SignalHistory,
    vp: &Viewport,
    area: Rect,
    t: &Theme,
    scene: &mut Scene,
) {
    if area.width() <= 0.0 || vp.width() <= 0.0 {
        return;
    }
    // Search strictly before the viewport so duplicate timestamps at its
    // left edge are all included in the first marker's count.
    let mut i = if vp.start <= 0.0 {
        0
    } else {
        h.index_at((vp.start.ceil() as u64).saturating_sub(1))
            .map_or(0, |last| last + 1)
    };
    while i < h.len() && h.time(i) as f64 <= vp.end {
        let x = vp.x_of(h.time(i) as f64, area.width() as f64).floor();
        let screen_x = area.left() + x as f32;
        let top = area.top() + TRACE_PAD;
        let bottom = (area.bottom() - TRACE_PAD).max(top);
        // Surfer's event glyph: a full-height stem with a filled upward
        // arrowhead, 5 px wide and one fifth of the trace height.
        let head_height = (bottom - top) * 0.2;
        let mut segments = vec![[point(screen_x, top), point(screen_x, bottom)]];
        if head_height > 0.0 {
            for row in 1..=head_height.ceil() as usize {
                let y = (row as f32).min(head_height);
                let half_width = 2.5 * y / head_height;
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
        i = end;
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
    let w_px = area.width().floor().max(0.0) as usize;
    if w_px == 0 {
        return;
    }
    let wf = w_px as f64;
    let x0 = area.left();
    let top = area.top() + TRACE_PAD;
    let bottom = area.bottom() - TRACE_PAD;
    let mid = snap(top + (bottom - top) / 2.0);
    // Top of the 1px line for a bit level.
    let y_of = |b: Bit| -> f32 {
        match b {
            Bit::One => top,
            Bit::Zero => bottom - 1.0,
            _ => mid,
        }
    };
    let to_u = |tt: f64| -> Option<u64> { (tt >= 0.0).then_some(tt as u64) };
    let time_at = |x: f64| vp.time_at(x, wf);

    let mut idx = to_u(time_at(0.0)).and_then(|tt| h.index_at(tt));
    let mut bit = h.bit(idx);
    let mut hint = idx.unwrap_or(0);
    let mut run_start = 0usize;
    let mut dense_start: Option<usize> = None;

    let emit_run = |scene: &mut Scene, xs: usize, xe: usize, b: Bit| {
        if xe <= xs || b == Bit::Unavailable {
            return;
        }
        let x = x0 + xs as f32;
        let w = (xe - xs) as f32;
        let color = t.value_color(b.kind());
        scene.fill(Rect::new(point(x, y_of(b)), size(w, 1.0)), color);
        if b == Bit::One {
            scene.fill(
                Rect::new(point(x, top + 1.0), size(w, bottom - top - 1.0)),
                t.wave_high_fill,
            );
        }
    };

    for x in 0..w_px {
        let idx1 = to_u(time_at((x + 1) as f64)).and_then(|tt| h.index_at_hint(tt, hint));
        let n = changes_between(idx, idx1);
        if n < 2 {
            // A dense region ends at the first column with fewer than two changes.
            if let Some(ds) = dense_start.take() {
                scene.fill(
                    Rect::new(
                        point(x0 + ds as f32, top),
                        size((x - ds) as f32, bottom - top),
                    ),
                    t.wave_dense,
                );
            }
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
        } else if n >= 2 && dense_start.is_none() {
            dense_start = Some(x);
        }
        run_start = x + 1;
        bit = new_bit;
        idx = idx1;
        hint = idx1.unwrap_or(0);
    }
    if let Some(ds) = dense_start.take() {
        scene.fill(
            Rect::new(
                point(x0 + ds as f32, top),
                size((w_px - ds).max(1) as f32, bottom - top),
            ),
            t.wave_dense,
        );
    }
    emit_run(scene, run_start, w_px, bit);
}

struct Segment {
    x_start: usize,
    x_end: usize,
    idx: Option<usize>,
    dense: bool,
}

/// Paint a multi-bit trace as slanted "hexagon" segments with value text.
fn paint_bus_row(
    h: &dyn SignalHistory,
    translator: &dyn Translator,
    vp: &Viewport,
    area: Rect,
    clip: Rect,
    p: &mut Painter<'_>,
) {
    let t = p.theme;
    let w_px = area.width().floor().max(0.0) as usize;
    if w_px == 0 {
        return;
    }
    let wf = w_px as f64;
    let x0 = area.left();
    let top = area.top() + TRACE_PAD;
    let bottom = area.bottom() - TRACE_PAD;
    let midf = top + (bottom - top) / 2.0;
    let to_u = |tt: f64| -> Option<u64> { (tt >= 0.0).then_some(tt as u64) };
    let time_at = |x: f64| vp.time_at(x, wf);

    // Column sampling → segments.
    let mut segments: Vec<Segment> = Vec::new();
    let mut idx = to_u(time_at(0.0)).and_then(|tt| h.index_at(tt));
    let mut hint = idx.unwrap_or(0);
    let mut cur_start = 0usize;
    for x in 0..w_px {
        let idx1 = to_u(time_at((x + 1) as f64)).and_then(|tt| h.index_at_hint(tt, hint));
        let n = changes_between(idx, idx1);
        if n == 0 {
            continue;
        }
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
    let mut slants: Vec<(ValueKind, Vec<[crate::geometry::Point; 2]>)> = Vec::new();
    let slant_w = 3.0f32;
    let char_w = p.char_w;
    let mut texts = Vec::new();
    p.scene.clipped(clip, |scene| {
        for seg in &segments {
            let xa = x0 + seg.x_start as f32;
            let xb = x0 + seg.x_end as f32;
            if seg.dense {
                scene.fill(
                    Rect::new(point(xa, top), size(xb - xa, bottom - top)),
                    t.wave_dense,
                );
                continue;
            }
            let value = h.value(seg.idx);
            if matches!(value, WaveValue::Unavailable) {
                continue;
            }
            let kind = value_kind(&value);
            let color = t.value_color(kind);
            let seg_w = xb - xa;
            let tw = slant_w.min(seg_w / 2.0);
            let open_left = seg.x_start == 0;
            let open_right = seg.x_end == w_px;
            let la = if open_left { xa } else { xa + tw };
            let rb = if open_right { xb } else { xb - tw };
            // Top and bottom lines.
            if rb > la {
                scene.fill(Rect::new(point(la, top), size(rb - la, 1.0)), color);
                scene.fill(
                    Rect::new(point(la, bottom - 1.0), size(rb - la, 1.0)),
                    color,
                );
            }
            let lines = match slants.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, b)) => b,
                None => {
                    slants.push((kind, Vec::new()));
                    &mut slants.last_mut().unwrap().1
                }
            };
            let topf = top + 0.5;
            let botf = bottom - 0.5;
            if !open_left {
                lines.push([point(xa, midf), point(xa + tw, topf)]);
                lines.push([point(xa, midf), point(xa + tw, botf)]);
            }
            if !open_right {
                lines.push([point(xb, midf), point(xb - tw, topf)]);
                lines.push([point(xb, midf), point(xb - tw, botf)]);
            }
            // Text.
            let avail = seg_w - 2.0 * tw - 8.0;
            if avail >= char_w {
                let max_chars = (avail / char_w).floor() as usize;
                let tr = translator.translate(&value);
                if let Some(text) = truncate_chars(&tr.text, max_chars) {
                    let color = if tr.kind == ValueKind::Normal {
                        t.wave_bus_text
                    } else {
                        t.value_color(tr.kind)
                    };
                    texts.push((snap(xa + tw + 4.0), text, color));
                }
            }
        }
        for (kind, segments) in slants {
            scene.lines(segments, t.value_color(kind), 1.0);
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

fn value_kind(v: &WaveValue) -> ValueKind {
    v.kind()
}

#[allow(dead_code)]
fn _layout_is_used(_: &WaveLayout) {}
