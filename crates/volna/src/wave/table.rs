//! `WaveTable`: one custom element that paints the three aligned columns
//! (names, values, waves) plus the timeline header, cursor, markers and a
//! scrollbar, and handles all mouse interaction for the panel.
//!
//! Waveforms are sampled per pixel column: for every column the renderer asks
//! the history for the index of the last change at the column's right edge
//! (an exponential search from the previous column's index), so the cost of a
//! frame is O(columns x log(changes)) regardless of trace length.

use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity, Font,
    FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    PinchEvent, Pixels, ScrollWheelEvent, ShapedLine, SharedString, Style, TextAlign, TextRun,
    Window, fill, point, px, quad, size,
};
use web_time::Instant;

use super::timeline::{format_time, ticks};
use super::view::{Drag, WaveView};
use super::viewport::Viewport;
use crate::data::{Bit, SignalHistory, SignalShape, Translator, ValueKind, WaveValue};
use crate::theme::{Theme, theme};

const MIN_COLUMN: f32 = 72.0;
const SPLITTER_TOLERANCE: f32 = 4.0;
const SCROLLBAR_W: f32 = 10.0;
const TICK_SPACING_PX: f64 = 96.0;
const SNAP_PX: f64 = 6.0;
/// Vertical inset of the trace inside a row.
const TRACE_PAD: f32 = 5.0;

pub struct WaveTable {
    view: Entity<WaveView>,
}

impl WaveTable {
    pub fn new(view: Entity<WaveView>) -> Self {
        WaveTable { view }
    }
}

struct RowSnapshot {
    name: SharedString,
    shape: SignalShape,
    translator: std::sync::Arc<dyn Translator>,
    history: Option<std::sync::Arc<dyn SignalHistory>>,
    error: Option<SharedString>,
}

pub struct TableLayout {
    hitbox: Hitbox,
    /// Grab zones of the two column dividers; own the resize cursor.
    names_split: Hitbox,
    values_split: Hitbox,
    /// Format badges and marker chips; own the pointing-hand cursor.
    badge_hitboxes: Vec<Hitbox>,
    chip_hitboxes: Vec<Hitbox>,
    header: Bounds<Pixels>,
    names: Bounds<Pixels>,
    values: Bounds<Pixels>,
    waves: Bounds<Pixels>,
    rows: Range<usize>,
    row_h: Pixels,
    scroll_y: Pixels,
    max_scroll: Pixels,
    scrollbar: Option<(Bounds<Pixels>, Bounds<Pixels>)>,
    badges: Vec<(usize, Bounds<Pixels>)>,
    marker_chips: Vec<(usize, Bounds<Pixels>)>,
}

impl IntoElement for WaveTable {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn mono(t: &Theme) -> Font {
    Font {
        family: t.mono_font.clone(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

fn ui(t: &Theme, weight: FontWeight) -> Font {
    Font {
        family: t.ui_font.clone(),
        features: Default::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

fn shape(
    window: &Window,
    text: impl Into<SharedString>,
    font: Font,
    size: Pixels,
    color: Hsla,
) -> ShapedLine {
    let text: SharedString = text.into();
    let run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(text, size, &[run], None)
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

fn snap(p: Pixels) -> Pixels {
    px(f32::from(p).round())
}

fn changes_between(a: Option<usize>, b: Option<usize>) -> usize {
    match (a, b) {
        (None, None) | (Some(_), None) => 0,
        (None, Some(j)) => j + 1,
        (Some(i), Some(j)) => j.saturating_sub(i),
    }
}

/// Where a click on the waves at `x_px` lands after snapping to the nearest
/// transition of `history` within `SNAP_PX`.
fn snapped_time(
    vp: &Viewport,
    history: Option<&dyn SignalHistory>,
    x_px: f64,
    width_px: f64,
) -> u64 {
    let raw = vp.time_at(x_px, width_px).round().max(0.0);
    let Some(h) = history else { return raw as u64 };
    let tol = SNAP_PX / vp.px_per_unit(width_px);
    let t = raw as u64;
    let mut best: Option<(f64, u64)> = None;
    let mut consider = |cand: u64| {
        let d = (cand as f64 - raw).abs();
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, cand));
        }
    };
    match h.index_at(t) {
        Some(i) => {
            consider(h.time(i));
            if i + 1 < h.len() {
                consider(h.time(i + 1));
            }
        }
        None => {
            if !h.is_empty() {
                consider(h.time(0));
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(t)
}

fn marker_chip_bounds(header: Bounds<Pixels>, x: Pixels, chip_w: Pixels) -> Bounds<Pixels> {
    Bounds::new(
        point(x + px(1.0), header.origin.y + px(2.0)),
        size(chip_w, px(14.0)),
    )
}

/// Segments narrower than this are drawn as a dense band instead of a hexagon.
const MIN_SEGMENT_PX: usize = 5;

impl Element for WaveTable {
    type RequestLayoutState = ();
    type PrepaintState = TableLayout;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> TableLayout {
        let t = theme(cx).clone();
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let row_h = t.row_height;
        let header_h = t.timeline_height;

        let (names_w, values_w, item_count, scroll_y, markers, viewport, wave_w_for_markers) = {
            let v = self.view.read(cx);
            (
                v.names_width,
                v.values_width,
                v.items.len(),
                v.scroll_y,
                v.markers.clone(),
                v.viewport,
                v.wave_width,
            )
        };
        let total_w = f32::from(bounds.size.width);
        let names_w = px(f32::from(names_w).clamp(
            MIN_COLUMN,
            (total_w - 2.0 * MIN_COLUMN - 160.0).max(MIN_COLUMN),
        ));
        let values_w = px(f32::from(values_w).clamp(
            MIN_COLUMN,
            (total_w - f32::from(names_w) - 160.0).max(MIN_COLUMN),
        ));

        let header = Bounds::new(bounds.origin, size(bounds.size.width, header_h));
        let rows_top = bounds.origin.y + header_h;
        let rows_h = (bounds.size.height - header_h).max(px(0.0));
        let names = Bounds::new(point(bounds.origin.x, rows_top), size(names_w, rows_h));
        let values = Bounds::new(point(names.right(), rows_top), size(values_w, rows_h));
        let waves_w = (bounds.size.width - names_w - values_w).max(px(0.0));
        let waves = Bounds::new(point(values.right(), rows_top), size(waves_w, rows_h));

        let content_h = row_h * item_count as f32;
        let max_scroll = (content_h - rows_h).max(px(0.0));
        let scroll_y = px(f32::from(scroll_y).clamp(0.0, f32::from(max_scroll)));

        let first = (f32::from(scroll_y) / f32::from(row_h)).floor().max(0.0) as usize;
        let last =
            ((f32::from(scroll_y + rows_h) / f32::from(row_h)).ceil() as usize).min(item_count);
        let rows = first.min(last)..last;

        // Hover row from the pointer position.
        let mouse = window.mouse_position();
        let hover_row = if bounds.contains(&mouse) && mouse.y >= rows_top {
            let ix =
                ((f32::from(mouse.y - rows_top + scroll_y)) / f32::from(row_h)).floor() as usize;
            (ix < item_count).then_some(ix)
        } else {
            None
        };

        // Format badges: right-aligned inside the values column.
        let badge_w = px(36.0);
        let mut badges = Vec::new();
        for ix in rows.clone() {
            let y = rows_top + row_h * ix as f32 - scroll_y;
            let b = Bounds::new(
                point(values.right() - badge_w - px(6.0), y + px(4.0)),
                size(badge_w, row_h - px(8.0)),
            );
            badges.push((ix, b));
        }
        let badge_hover = badges
            .iter()
            .find(|(_, b)| b.contains(&mouse))
            .map(|(ix, _)| *ix);

        // Marker chips in the header.
        let wave_wf = f64::from(f32::from(waves_w)).max(1.0);
        let mut marker_chips = Vec::new();
        for (ix, m) in markers.iter().enumerate() {
            let x = viewport.x_of(m.time as f64, wave_wf);
            if x < 0.0 || x > wave_wf {
                continue;
            }
            let xp = snap(waves.origin.x + px(x as f32));
            marker_chips.push((ix, marker_chip_bounds(header, xp, px(24.0))));
        }
        let _ = wave_w_for_markers;

        let scrollbar = if max_scroll > px(0.0) && rows_h > px(0.0) {
            let track = Bounds::new(
                point(waves.right() - px(SCROLLBAR_W), rows_top),
                size(px(SCROLLBAR_W), rows_h),
            );
            let ratio = f32::from(rows_h) / f32::from(content_h);
            let thumb_h = (rows_h * ratio).max(px(24.0));
            let travel = rows_h - thumb_h;
            let top =
                rows_top + travel * (f32::from(scroll_y) / f32::from(max_scroll)).clamp(0.0, 1.0);
            let thumb = Bounds::new(
                point(track.origin.x + px(2.0), top),
                size(px(SCROLLBAR_W - 4.0), thumb_h),
            );
            Some((track, thumb))
        } else {
            None
        };

        let split_zone = |x: Pixels| {
            Bounds::new(
                point(x - px(SPLITTER_TOLERANCE), bounds.origin.y),
                size(px(2.0 * SPLITTER_TOLERANCE), bounds.size.height),
            )
        };
        let names_split = window.insert_hitbox(split_zone(names.right()), HitboxBehavior::Normal);
        let values_split = window.insert_hitbox(split_zone(values.right()), HitboxBehavior::Normal);
        let badge_hitboxes = badges
            .iter()
            .map(|(_, b)| window.insert_hitbox(*b, HitboxBehavior::Normal))
            .collect();
        let chip_hitboxes = marker_chips
            .iter()
            .map(|(_, b)| window.insert_hitbox(*b, HitboxBehavior::Normal))
            .collect();

        self.view.update(cx, |v, _| {
            v.scroll_y = scroll_y;
            v.wave_width = waves_w;
            v.hover_row = hover_row;
            v.badge_hover = badge_hover;
        });

        TableLayout {
            hitbox,
            names_split,
            values_split,
            badge_hitboxes,
            chip_hitboxes,
            header,
            names,
            values,
            waves,
            rows,
            row_h,
            scroll_y,
            max_scroll,
            scrollbar,
            badges,
            marker_chips,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        layout: &mut TableLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let t = theme(cx).clone();
        let view = self.view.clone();
        let mono_font = mono(&t);
        let char_w = shape(window, "0", mono_font.clone(), t.mono_size, t.text).width();

        // Snapshot the state we paint from.
        let (
            viewport,
            cursor,
            markers,
            selected,
            hover_row,
            badge_hover,
            drag,
            timescale,
            has_source,
        ) = {
            let v = view.read(cx);
            (
                v.viewport,
                v.cursor,
                v.markers.clone(),
                v.selected.clone(),
                v.hover_row,
                v.badge_hover,
                v.drag,
                v.timescale(),
                v.source.is_some(),
            )
        };
        let waves = layout.waves;
        let wave_wf = f64::from(f32::from(waves.size.width)).max(1.0);

        // -- backgrounds and chrome ------------------------------------------
        window.paint_quad(fill(bounds, t.bg_editor));
        window.paint_quad(fill(
            Bounds::new(
                layout.names.origin,
                size(
                    layout.names.size.width + layout.values.size.width,
                    layout.names.size.height,
                ),
            ),
            t.bg_panel,
        ));
        window.paint_quad(fill(layout.header, t.bg_panel));

        // -- tick grid in the waves area ---------------------------------------
        let (tick_list, unit) = ticks(&viewport, wave_wf, timescale, TICK_SPACING_PX);
        window.with_content_mask(Some(ContentMask { bounds: waves }), |window| {
            for tick in &tick_list {
                let x = snap(waves.origin.x + px(viewport.x_of(tick.time, wave_wf) as f32));
                window.paint_quad(fill(
                    Bounds::new(point(x, waves.origin.y), size(px(1.0), waves.size.height)),
                    t.wave_tick,
                ));
            }
        });

        // -- rows ----------------------------------------------------------------
        let row_h = layout.row_h;
        let mouse = window.mouse_position();
        // Snapshot the visible rows (Arc clones) so painting can borrow `cx` mutably.
        let visible: Vec<(usize, RowSnapshot)> = {
            let v = view.read(cx);
            layout
                .rows
                .clone()
                .filter_map(|ix| {
                    v.items.get(ix).map(|item| {
                        (
                            ix,
                            RowSnapshot {
                                name: item.name.clone(),
                                shape: item.shape,
                                translator: item.translator.clone(),
                                history: item.history.clone(),
                                error: item.error.clone(),
                            },
                        )
                    })
                })
                .collect()
        };
        {
            for (ix, item) in &visible {
                let ix = *ix;
                let y = layout.names.origin.y + row_h * ix as f32 - layout.scroll_y;
                let is_selected = selected.contains(&ix);
                let is_hover = hover_row == Some(ix);
                let left_row = Bounds::new(
                    point(bounds.origin.x, y),
                    size(layout.names.size.width + layout.values.size.width, row_h),
                );
                let wave_row = Bounds::new(point(waves.origin.x, y), size(waves.size.width, row_h));
                if is_selected {
                    window.paint_quad(fill(left_row, t.element_selected));
                    window.paint_quad(fill(wave_row, t.wave_row_selected));
                    if t.high_contrast {
                        window.paint_quad(quad(
                            left_row,
                            px(0.0),
                            gpui::transparent_black(),
                            px(1.0),
                            t.border_focused,
                            gpui::BorderStyle::default(),
                        ));
                        window.paint_quad(quad(
                            wave_row,
                            px(0.0),
                            gpui::transparent_black(),
                            px(1.0),
                            t.border_focused,
                            gpui::BorderStyle::default(),
                        ));
                    }
                } else if is_hover {
                    window.paint_quad(fill(left_row, t.element_hover));
                    window.paint_quad(fill(wave_row, t.wave_row_hover));
                }

                // Name column: leaf name, then a muted range badge for vectors.
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: layout.names,
                    }),
                    |window| {
                        let t = t.row(is_selected, is_hover);
                        let pad = px(12.0);
                        let avail = layout.names.size.width - pad * 2.0;
                        let dims = item.shape.dims();
                        let name_color = t.text;
                        let max_chars =
                            (f32::from(avail) / f32::from(char_w)).floor().max(0.0) as usize;
                        let dims_chars = if dims.is_empty() {
                            0
                        } else {
                            dims.chars().count() + 1
                        };
                        let name_budget = max_chars.saturating_sub(dims_chars.min(max_chars / 2));
                        if let Some(name) = truncate_chars(&item.name, name_budget) {
                            let line = shape(
                                window,
                                name.clone(),
                                mono_font.clone(),
                                t.mono_size,
                                name_color,
                            );
                            let origin = point(layout.names.origin.x + pad, y);
                            line.paint(origin, row_h, TextAlign::Left, None, window, cx)
                                .ok();
                            if !dims.is_empty() && max_chars > name.chars().count() + 1 {
                                let rest = max_chars - name.chars().count() - 1;
                                if let Some(d) = truncate_chars(&dims, rest) {
                                    let dl = shape(
                                        window,
                                        d,
                                        mono_font.clone(),
                                        t.mono_size,
                                        t.text_placeholder,
                                    );
                                    dl.paint(
                                        point(origin.x + line.width() + char_w, y),
                                        row_h,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    )
                                    .ok();
                                }
                            }
                        }
                    },
                );

                // Values column: value at cursor plus the format badge.
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: layout.values,
                    }),
                    |window| {
                        let t = t.row(is_selected, is_hover);
                        let pad = px(8.0);
                        let badge = layout
                            .badges
                            .iter()
                            .find(|(i, _)| *i == ix)
                            .map(|(_, b)| *b);
                        let text_right = badge
                            .map(|b| b.origin.x - pad)
                            .unwrap_or(layout.values.right() - pad);
                        let avail = text_right - layout.values.origin.x - pad;
                        let (text, color) = match (&item.history, cursor) {
                            (Some(h), Some(c)) => {
                                let value = h.value(h.index_at(c));
                                let tr = item.translator.translate(&value);
                                let color = if tr.kind == ValueKind::Normal {
                                    t.text
                                } else {
                                    t.value_color(tr.kind)
                                };
                                (tr.text, color)
                            }
                            (None, _) if item.error.is_none() => {
                                ("loading…".to_string(), t.text_placeholder)
                            }
                            _ => ("–".to_string(), t.text_placeholder),
                        };
                        let max_chars =
                            (f32::from(avail) / f32::from(char_w)).floor().max(0.0) as usize;
                        if let Some(text) = truncate_chars(&text, max_chars) {
                            let line = shape(window, text, mono_font.clone(), t.mono_size, color);
                            line.paint(
                                point(layout.values.origin.x + pad, y),
                                row_h,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .ok();
                        }
                        if let Some(b) = badge {
                            let hovered = badge_hover == Some(ix);
                            let show = hovered || is_selected || is_hover;
                            if show {
                                let bg = if hovered {
                                    t.element_active
                                } else {
                                    t.bg_editor
                                };
                                window.paint_quad(quad(
                                    b,
                                    px(3.0),
                                    bg,
                                    px(1.0),
                                    t.border_variant,
                                    gpui::BorderStyle::default(),
                                ));
                                let label = item.translator.badge();
                                let color = if hovered { t.text } else { t.text_muted };
                                let line = shape(
                                    window,
                                    label,
                                    ui(&t, FontWeight::MEDIUM),
                                    t.ui_size_small,
                                    color,
                                );
                                let x = b.origin.x + (b.size.width - line.width()) / 2.0;
                                line.paint(
                                    point(snap(x), b.origin.y),
                                    b.size.height,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .ok();
                            }
                        }
                    },
                );

                // Waves column.
                window.with_content_mask(Some(ContentMask { bounds: waves }), |window| {
                    match (&item.history, &item.error) {
                        (Some(h), _) => match item.shape {
                            SignalShape::Bit => {
                                paint_bit_row(h.as_ref(), &viewport, wave_row, &t, window)
                            }
                            _ => paint_bus_row(
                                h.as_ref(),
                                item.translator.as_ref(),
                                &viewport,
                                wave_row,
                                &t,
                                char_w,
                                &mono_font,
                                window,
                                cx,
                            ),
                        },
                        (None, Some(err)) => {
                            let line = shape(
                                window,
                                err.clone(),
                                ui(&t, FontWeight::NORMAL),
                                t.ui_size_small,
                                t.error,
                            );
                            line.paint(
                                point(waves.origin.x + px(8.0), y),
                                row_h,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .ok();
                        }
                        (None, None) => {
                            // Loading: a thin muted bar where the trace will be.
                            let bar = Bounds::new(
                                point(waves.origin.x + px(8.0), y + row_h / 2.0),
                                size(px(96.0), px(1.0)),
                            );
                            window.paint_quad(fill(bar, t.border));
                        }
                    }
                });
            }
        }

        // -- empty placeholder -------------------------------------------------
        if layout.rows.is_empty() && has_source {
            let area = Bounds::new(
                point(bounds.origin.x, layout.names.origin.y),
                size(bounds.size.width, layout.names.size.height),
            );
            let cx_ = area.origin.x + area.size.width / 2.0;
            let cy = area.origin.y + area.size.height / 2.0;
            let icon = Bounds::new(
                point(cx_ - px(16.0), cy - px(44.0)),
                size(px(32.0), px(32.0)),
            );
            window
                .paint_svg(
                    icon,
                    "icons/list-tree.svg".into(),
                    None,
                    gpui::TransformationMatrix::unit(),
                    t.text_placeholder,
                    cx,
                )
                .ok();
            let title = shape(
                window,
                "No signals displayed",
                ui(&t, FontWeight::MEDIUM),
                t.ui_size,
                t.text_muted,
            );
            title
                .paint(
                    point(snap(cx_ - title.width() / 2.0), cy - px(4.0)),
                    px(20.0),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            let hint = shape(
                window,
                "Double-click a variable in the sidebar, or select variables and press Enter",
                ui(&t, FontWeight::NORMAL),
                t.ui_size_small,
                t.text_placeholder,
            );
            hint.paint(
                point(snap(cx_ - hint.width() / 2.0), cy + px(18.0)),
                px(18.0),
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
        }

        // -- header: column titles, tick labels, unit -----------------------------
        let panel_theme = t.panel();
        let header = layout.header;
        let label_font = ui(&t, FontWeight::SEMIBOLD);
        let title = shape(
            window,
            "SIGNALS",
            label_font.clone(),
            t.ui_size_small,
            panel_theme.text_muted,
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: Bounds::new(
                    header.origin,
                    size(layout.names.size.width, header.size.height),
                ),
            }),
            |window| {
                title
                    .paint(
                        point(layout.names.origin.x + px(12.0), header.origin.y),
                        header.size.height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            },
        );
        let value_title = match cursor {
            Some(c) => format_time(c as f64, timescale),
            None => "VALUE".to_string(),
        };
        let (vfont, vcolor) = if cursor.is_some() {
            (mono_font.clone(), panel_theme.text)
        } else {
            (label_font.clone(), panel_theme.text_muted)
        };
        let vt = shape(
            window,
            value_title,
            vfont,
            if cursor.is_some() {
                t.mono_size
            } else {
                t.ui_size_small
            },
            vcolor,
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: Bounds::new(
                    point(layout.values.origin.x, header.origin.y),
                    size(layout.values.size.width, header.size.height),
                ),
            }),
            |window| {
                vt.paint(
                    point(layout.values.origin.x + px(8.0), header.origin.y),
                    header.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            },
        );
        let header_waves = Bounds::new(
            point(waves.origin.x, header.origin.y),
            size(waves.size.width, header.size.height),
        );
        window.with_content_mask(
            Some(ContentMask {
                bounds: header_waves,
            }),
            |window| {
                let unit_line = (!unit.is_empty()).then(|| {
                    shape(
                        window,
                        unit,
                        mono_font.clone(),
                        t.ui_size_small,
                        panel_theme.text_placeholder,
                    )
                });
                let unit_x = unit_line
                    .as_ref()
                    .map(|u| header_waves.right() - u.width() - px(SCROLLBAR_W + 4.0))
                    .unwrap_or(header_waves.right());
                for tick in &tick_list {
                    let x = snap(waves.origin.x + px(viewport.x_of(tick.time, wave_wf) as f32));
                    window.paint_quad(fill(
                        Bounds::new(point(x, header.bottom() - px(7.0)), size(px(1.0), px(6.0))),
                        panel_theme.text_placeholder,
                    ));
                    let line = shape(
                        window,
                        tick.label.clone(),
                        mono_font.clone(),
                        t.ui_size_small,
                        t.wave_tick_text,
                    );
                    if x + px(4.0) + line.width() < unit_x - px(8.0) {
                        line.paint(
                            point(x + px(4.0), header.origin.y + px(2.0)),
                            px(20.0),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                    }
                }
                if let Some(u) = unit_line {
                    u.paint(
                        point(unit_x, header.origin.y + px(2.0)),
                        px(20.0),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
                }
            },
        );

        // -- markers -----------------------------------------------------------------
        for (ix, chip) in &layout.marker_chips {
            let m = markers[*ix];
            let x = snap(waves.origin.x + px(viewport.x_of(m.time as f64, wave_wf) as f32));
            let color = t.marker_color(*ix);
            window.with_content_mask(Some(ContentMask { bounds: waves }), |window| {
                window.paint_quad(fill(
                    Bounds::new(point(x, waves.origin.y), size(px(1.0), waves.size.height)),
                    color,
                ));
            });
            let hovered = chip.contains(&mouse);
            let mut bg = color;
            if !hovered {
                bg.a = 0.85;
            }
            window.paint_quad(quad(
                *chip,
                px(3.0),
                bg,
                px(0.0),
                gpui::transparent_black(),
                gpui::BorderStyle::default(),
            ));
            let label = shape(
                window,
                format!("M{}", ix + 1),
                ui(&t, FontWeight::SEMIBOLD),
                t.ui_size_small,
                t.marker_text(bg),
            );
            label
                .paint(
                    point(
                        snap(chip.origin.x + (chip.size.width - label.width()) / 2.0),
                        chip.origin.y,
                    ),
                    chip.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }

        // -- cursor ------------------------------------------------------------------
        if let Some(c) = cursor {
            let xf = viewport.x_of(c as f64, wave_wf);
            if xf >= -1.0 && xf <= wave_wf + 1.0 {
                let x = snap(waves.origin.x + px(xf as f32));
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: Bounds::new(
                            point(waves.origin.x, header.origin.y),
                            size(waves.size.width, bounds.size.height),
                        ),
                    }),
                    |window| {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(x, header.bottom() - px(8.0)),
                                size(px(1.0), bounds.bottom() - header.bottom() + px(8.0)),
                            ),
                            t.wave_cursor,
                        ));
                        let label = shape(
                            window,
                            format_time(c as f64, timescale),
                            mono_font.clone(),
                            t.ui_size_small,
                            t.wave_cursor_text,
                        );
                        let chip_w = label.width() + px(10.0);
                        let mut cx0 = x + px(1.0);
                        if cx0 + chip_w > header_waves.right() - px(SCROLLBAR_W) {
                            cx0 = x - chip_w;
                        }
                        let chip = Bounds::new(
                            point(cx0, header.bottom() - px(18.0)),
                            size(chip_w, px(16.0)),
                        );
                        window.paint_quad(quad(
                            chip,
                            px(3.0),
                            t.wave_cursor,
                            px(0.0),
                            gpui::transparent_black(),
                            gpui::BorderStyle::default(),
                        ));
                        label
                            .paint(
                                point(chip.origin.x + px(5.0), chip.origin.y),
                                chip.size.height,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .ok();
                    },
                );
            }
        }

        // -- borders --------------------------------------------------------------
        let near_names = layout.names_split.is_hovered(window);
        let near_values = layout.values_split.is_hovered(window);
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
        window.paint_quad(fill(
            Bounds::new(
                point(layout.names.right() - px(1.0), bounds.origin.y),
                size(px(1.0), bounds.size.height),
            ),
            names_border,
        ));
        window.paint_quad(fill(
            Bounds::new(
                point(layout.values.right() - px(1.0), bounds.origin.y),
                size(px(1.0), bounds.size.height),
            ),
            values_border,
        ));
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x, header.bottom() - px(1.0)),
                size(bounds.size.width, px(1.0)),
            ),
            t.border_variant,
        ));
        // Cursor styles are attached to the small hitboxes so GPUI resolves them
        // per pointer position; only an active drag pins the resize cursor.
        if matches!(drag, Some(Drag::NamesSplit | Drag::ValuesSplit)) {
            window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
        } else {
            window.set_cursor_style(CursorStyle::ResizeLeftRight, &layout.names_split);
            window.set_cursor_style(CursorStyle::ResizeLeftRight, &layout.values_split);
            for h in layout
                .badge_hitboxes
                .iter()
                .chain(layout.chip_hitboxes.iter())
            {
                window.set_cursor_style(CursorStyle::PointingHand, h);
            }
        }

        // -- scrollbar -----------------------------------------------------------------
        if let Some((track, thumb)) = layout.scrollbar {
            let hovered = track.contains(&mouse) || matches!(drag, Some(Drag::Scroll { .. }));
            let color = if hovered {
                t.scrollbar_thumb_hover
            } else {
                t.scrollbar_thumb
            };
            window.paint_quad(quad(
                thumb,
                px(3.0),
                color,
                px(0.0),
                gpui::transparent_black(),
                gpui::BorderStyle::default(),
            ));
        }

        // -- input ------------------------------------------------------------------
        self.register_mouse_handlers(bounds, layout, window);

        let ms = started.elapsed().as_secs_f32() * 1000.0;
        view.update(cx, |v, _| v.record_frame(ms));
    }
}

impl WaveTable {
    fn register_mouse_handlers(
        &self,
        bounds: Bounds<Pixels>,
        layout: &TableLayout,
        window: &mut Window,
    ) {
        let view = self.view.clone();
        let hitbox = layout.hitbox.clone();
        let header = layout.header;
        let names = layout.names;
        let values = layout.values;
        let waves = layout.waves;
        let row_h = layout.row_h;
        let scroll_y = layout.scroll_y;
        let max_scroll = layout.max_scroll;
        let scrollbar = layout.scrollbar;
        let badges = layout.badges.clone();
        let marker_chips = layout.marker_chips.clone();
        let wave_wf = f64::from(f32::from(waves.size.width)).max(1.0);
        let row_at = move |y: Pixels| -> Option<usize> {
            if y < names.origin.y {
                return None;
            }
            let ix = (f32::from(y - names.origin.y + scroll_y) / f32::from(row_h)).floor();
            (ix >= 0.0).then_some(ix as usize)
        };

        // Mouse down: splitters, scrollbar, markers, cursor, selection, badges.
        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            let badges = badges.clone();
            let marker_chips = marker_chips.clone();
            move |ev: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let p = ev.position;
                view.update(cx, |v, cx| {
                    window.focus(&v.focus_handle, cx);
                    if v.menu.is_some() {
                        v.menu = None;
                    }
                    if ev.button == MouseButton::Left {
                        if (f32::from(p.x - names.right())).abs() <= SPLITTER_TOLERANCE {
                            v.drag = Some(Drag::NamesSplit);
                            cx.notify();
                            return;
                        }
                        if (f32::from(p.x - values.right())).abs() <= SPLITTER_TOLERANCE {
                            v.drag = Some(Drag::ValuesSplit);
                            cx.notify();
                            return;
                        }
                        if let Some((track, thumb)) = scrollbar {
                            if thumb.contains(&p) {
                                v.drag = Some(Drag::Scroll {
                                    grab: p.y - thumb.origin.y,
                                });
                                cx.notify();
                                return;
                            }
                            if track.contains(&p) {
                                let travel = track.size.height - thumb.size.height;
                                let frac =
                                    (f32::from(p.y - track.origin.y - thumb.size.height / 2.0)
                                        / f32::from(travel))
                                    .clamp(0.0, 1.0);
                                v.scroll_y = max_scroll * frac;
                                v.drag = Some(Drag::Scroll {
                                    grab: thumb.size.height / 2.0,
                                });
                                cx.notify();
                                return;
                            }
                        }
                        if let Some((ix, _)) = marker_chips.iter().find(|(_, b)| b.contains(&p)) {
                            if ev.modifiers.shift {
                                v.remove_marker(*ix, cx);
                            } else {
                                let t = v.markers[*ix].time;
                                v.set_cursor(Some(t), cx);
                            }
                            return;
                        }
                    }
                    let in_waves_x = p.x >= waves.origin.x && p.x < waves.right();
                    if header.contains(&p) {
                        if in_waves_x && ev.button == MouseButton::Left {
                            let x = f64::from(f32::from(p.x - waves.origin.x));
                            let t = snapped_time(&v.viewport, None, x, wave_wf);
                            v.set_cursor(Some(t), cx);
                            v.drag = Some(Drag::Cursor);
                        }
                        return;
                    }
                    let row = row_at(p.y).filter(|r| *r < v.items.len());
                    if in_waves_x {
                        match ev.button {
                            MouseButton::Left => {
                                let x = f64::from(f32::from(p.x - waves.origin.x));
                                let hist = row.and_then(|r| v.items[r].history.clone());
                                let t = snapped_time(&v.viewport, hist.as_deref(), x, wave_wf);
                                v.set_cursor(Some(t), cx);
                                v.drag = Some(Drag::Cursor);
                                if let Some(r) = row
                                    && (!v.selected.contains(&r)
                                        || ev.modifiers.shift
                                        || ev.modifiers.secondary())
                                {
                                    v.select_row(r, ev.modifiers, cx);
                                }
                            }
                            MouseButton::Middle | MouseButton::Right => {
                                v.drag = Some(Drag::Pan { last_x: p.x });
                            }
                            _ => {}
                        }
                        cx.notify();
                        return;
                    }
                    // Names / values columns.
                    if ev.button != MouseButton::Left {
                        return;
                    }
                    if let Some((ix, b)) = badges.iter().find(|(_, b)| b.contains(&p)) {
                        let pos = point(b.origin.x, b.bottom() + px(4.0));
                        if !v.selected.contains(ix) {
                            v.select_row(*ix, ev.modifiers, cx);
                        }
                        v.open_format_menu(*ix, pos, window, cx);
                        return;
                    }
                    match row {
                        Some(r) => v.select_row(r, ev.modifiers, cx),
                        None => {
                            v.selected.clear();
                            cx.notify();
                        }
                    }
                });
            }
        });

        // Mouse move: drags and hover feedback.
        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let p = ev.position;
                let drag = view.read(cx).drag;
                match drag {
                    Some(Drag::Cursor) => {
                        view.update(cx, |v, cx| {
                            let x = f64::from(f32::from(p.x - waves.origin.x)).clamp(0.0, wave_wf);
                            let row = row_at(p.y).filter(|r| *r < v.items.len());
                            let hist = row.and_then(|r| v.items[r].history.clone());
                            let t = snapped_time(&v.viewport, hist.as_deref(), x, wave_wf);
                            v.set_cursor(Some(t), cx);
                        });
                    }
                    Some(Drag::Pan { last_x }) => {
                        view.update(cx, |v, cx| {
                            v.pan_px(last_x - p.x, cx);
                            v.drag = Some(Drag::Pan { last_x: p.x });
                        });
                    }
                    Some(Drag::NamesSplit) => {
                        view.update(cx, |v, cx| {
                            v.names_width = px(f32::from(p.x - bounds.origin.x).max(MIN_COLUMN));
                            cx.notify();
                        });
                    }
                    Some(Drag::ValuesSplit) => {
                        view.update(cx, |v, cx| {
                            v.values_width = px(f32::from(p.x - names.right()).max(MIN_COLUMN));
                            cx.notify();
                        });
                    }
                    Some(Drag::Scroll { grab }) => {
                        if let Some((track, thumb)) = scrollbar {
                            view.update(cx, |v, cx| {
                                let travel = track.size.height - thumb.size.height;
                                if travel > px(0.0) {
                                    let frac = (f32::from(p.y - grab - track.origin.y)
                                        / f32::from(travel))
                                    .clamp(0.0, 1.0);
                                    v.scroll_y = max_scroll * frac;
                                    cx.notify();
                                }
                            });
                        }
                    }
                    None => {
                        // Hover feedback only needs a repaint when the hovered row or
                        // badge changes, when we enter/leave splitter zones, or when
                        // the pointer leaves the table while something was hovered.
                        let (prev_row, prev_badge, count) = {
                            let v = view.read(cx);
                            (v.hover_row, v.badge_hover, v.items.len())
                        };
                        if hitbox.is_hovered(window) || bounds.contains(&p) {
                            let row = row_at(p.y).filter(|r| *r < count);
                            let badge = badges
                                .iter()
                                .find(|(_, b)| b.contains(&p))
                                .map(|(ix, _)| *ix);
                            let near_split = (f32::from(p.x - names.right())).abs()
                                <= SPLITTER_TOLERANCE
                                || (f32::from(p.x - values.right())).abs() <= SPLITTER_TOLERANCE;
                            let prev_split = view.read(cx).split_hover;
                            let chip = marker_chips.iter().any(|(_, b)| b.contains(&p));
                            let prev_chip = view.read(cx).chip_hover;
                            if row != prev_row
                                || badge != prev_badge
                                || near_split != prev_split
                                || chip != prev_chip
                            {
                                view.update(cx, |v, _| {
                                    v.split_hover = near_split;
                                    v.chip_hover = chip;
                                });
                                window.refresh();
                            }
                        } else if prev_row.is_some()
                            || prev_badge.is_some()
                            || view.read(cx).split_hover
                            || view.read(cx).chip_hover
                        {
                            view.update(cx, |v, _| {
                                v.split_hover = false;
                                v.chip_hover = false;
                            });
                            window.refresh();
                        }
                    }
                }
            }
        });

        window.on_mouse_event({
            let view = view.clone();
            move |_: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                if view.read(cx).drag.is_some() {
                    view.update(cx, |v, cx| {
                        v.drag = None;
                        cx.notify();
                    });
                }
            }
        });

        // Trackpad pinch: zoom about the gesture centre.
        window.on_mouse_event({
            let view = view.clone();
            let hitbox = hitbox.clone();
            move |ev: &PinchEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let factor = f64::from(1.0 + ev.delta).clamp(0.2, 5.0);
                let x = (ev.position.x - waves.origin.x).max(px(0.0));
                view.update(cx, |v, cx| v.zoom_at(x, factor, cx));
            }
        });

        // Wheel: zoom with cmd/ctrl, pan horizontally, scroll rows vertically.
        window.on_mouse_event({
            let view = view.clone();
            move |ev: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let delta = ev.delta.pixel_delta(row_h);
                let dx = f32::from(delta.x);
                let dy = f32::from(delta.y);
                let p = ev.position;
                view.update(cx, |v, cx| {
                    if ev.modifiers.secondary() || ev.modifiers.control {
                        let factor = 2f64.powf(f64::from(dy) / 120.0);
                        let x = (p.x - waves.origin.x).max(px(0.0));
                        v.zoom_at(x, factor, cx);
                    } else if ev.modifiers.shift {
                        v.pan_px(px(-dy), cx);
                    } else {
                        if dx.abs() > 0.0 {
                            v.pan_px(px(-dx), cx);
                        }
                        if dy.abs() > 0.0 {
                            v.scroll_y =
                                px((f32::from(v.scroll_y) - dy).clamp(0.0, f32::from(max_scroll)));
                            cx.notify();
                        }
                    }
                });
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Waveform painting
// ---------------------------------------------------------------------------

/// Paint a 1-bit trace by sampling one pixel column at a time.
fn paint_bit_row(
    h: &dyn SignalHistory,
    vp: &Viewport,
    area: Bounds<Pixels>,
    t: &Theme,
    window: &mut Window,
) {
    let w_px = f32::from(area.size.width).floor().max(0.0) as usize;
    if w_px == 0 {
        return;
    }
    let wf = w_px as f64;
    let x0 = area.origin.x;
    let top = area.origin.y + px(TRACE_PAD);
    let bottom = area.origin.y + area.size.height - px(TRACE_PAD);
    let mid = snap(top + (bottom - top) / 2.0);
    // Top of the 1px line for a bit level.
    let y_of = |b: Bit| -> Pixels {
        match b {
            Bit::One => top,
            Bit::Zero => bottom - px(1.0),
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

    let emit_run = |window: &mut Window, xs: usize, xe: usize, b: Bit| {
        if xe <= xs {
            return;
        }
        let x = x0 + px(xs as f32);
        let w = px((xe - xs) as f32);
        let color = t.value_color(b.kind());
        window.paint_quad(fill(
            Bounds::new(point(x, y_of(b)), size(w, px(1.0))),
            color,
        ));
        if b == Bit::One {
            window.paint_quad(fill(
                Bounds::new(point(x, top + px(1.0)), size(w, bottom - top - px(1.0))),
                t.wave_high_fill,
            ));
        }
    };

    for x in 0..w_px {
        let idx1 = to_u(time_at((x + 1) as f64)).and_then(|tt| h.index_at_hint(tt, hint));
        let n = changes_between(idx, idx1);
        if n < 2 {
            // A dense region ends at the first column with fewer than two changes.
            if let Some(ds) = dense_start.take() {
                window.paint_quad(fill(
                    Bounds::new(
                        point(x0 + px(ds as f32), top),
                        size(px((x - ds) as f32), bottom - top),
                    ),
                    t.wave_dense,
                ));
            }
        }
        if n == 0 {
            continue;
        }
        let new_bit = h.bit(idx1);
        emit_run(window, run_start, x, bit);
        if n == 1 {
            let (ya, yb) = (y_of(bit), y_of(new_bit));
            let (lo, hi) = if ya <= yb { (ya, yb) } else { (yb, ya) };
            let color = t.value_color(if new_bit.kind() != ValueKind::Normal {
                new_bit.kind()
            } else {
                bit.kind()
            });
            window.paint_quad(fill(
                Bounds::new(
                    point(x0 + px(x as f32), lo),
                    size(px(1.0), hi - lo + px(1.0)),
                ),
                color,
            ));
        } else if dense_start.is_none() {
            dense_start = Some(x);
        }
        run_start = x + 1;
        bit = new_bit;
        idx = idx1;
        hint = idx1.unwrap_or(0);
    }
    if let Some(ds) = dense_start.take() {
        window.paint_quad(fill(
            Bounds::new(
                point(x0 + px(ds as f32), top),
                size(px((w_px - ds).max(1) as f32), bottom - top),
            ),
            t.wave_dense,
        ));
    }
    emit_run(window, run_start, w_px, bit);
}

struct Segment {
    x_start: usize,
    x_end: usize,
    idx: Option<usize>,
    dense: bool,
}

/// Paint a multi-bit trace as slanted "hexagon" segments with value text.
#[allow(clippy::too_many_arguments)]
fn paint_bus_row(
    h: &dyn SignalHistory,
    translator: &dyn Translator,
    vp: &Viewport,
    area: Bounds<Pixels>,
    t: &Theme,
    char_w: Pixels,
    mono_font: &Font,
    window: &mut Window,
    cx: &mut App,
) {
    let w_px = f32::from(area.size.width).floor().max(0.0) as usize;
    if w_px == 0 {
        return;
    }
    let wf = w_px as f64;
    let x0 = area.origin.x;
    let top = area.origin.y + px(TRACE_PAD);
    let bottom = area.origin.y + area.size.height - px(TRACE_PAD);
    let midf = f32::from(top + (bottom - top) / 2.0);
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

    // Paint. Horizontal edges are crisp quads; slants are stroked paths.
    let mut slants: Vec<(ValueKind, PathBuilder)> = Vec::new();
    let slant_w = 3.0f32;
    for seg in &segments {
        let xa = f32::from(x0) + seg.x_start as f32;
        let xb = f32::from(x0) + seg.x_end as f32;
        if seg.dense {
            window.paint_quad(fill(
                Bounds::new(point(px(xa), top), size(px(xb - xa), bottom - top)),
                t.wave_dense,
            ));
            continue;
        }
        let value = h.value(seg.idx);
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
            window.paint_quad(fill(
                Bounds::new(point(px(la), top), size(px(rb - la), px(1.0))),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(point(px(la), bottom - px(1.0)), size(px(rb - la), px(1.0))),
                color,
            ));
        }
        let builder = match slants.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, b)) => b,
            None => {
                slants.push((kind, PathBuilder::stroke(px(1.0))));
                &mut slants.last_mut().unwrap().1
            }
        };
        let topf = f32::from(top) + 0.5;
        let botf = f32::from(bottom) - 0.5;
        if !open_left {
            builder.move_to(point(px(xa), px(midf)));
            builder.line_to(point(px(xa + tw), px(topf)));
            builder.move_to(point(px(xa), px(midf)));
            builder.line_to(point(px(xa + tw), px(botf)));
        }
        if !open_right {
            builder.move_to(point(px(xb), px(midf)));
            builder.line_to(point(px(xb - tw), px(topf)));
            builder.move_to(point(px(xb), px(midf)));
            builder.line_to(point(px(xb - tw), px(botf)));
        }
        // Text.
        let avail = seg_w - 2.0 * tw - 8.0;
        let cw = f32::from(char_w);
        if avail >= cw {
            let max_chars = (avail / cw).floor() as usize;
            let tr = translator.translate(&value);
            if let Some(text) = truncate_chars(&tr.text, max_chars) {
                let color = if tr.kind == ValueKind::Normal {
                    t.wave_bus_text
                } else {
                    t.value_color(tr.kind)
                };
                let line = shape(window, text, mono_font.clone(), t.mono_size, color);
                let tx = snap(px(xa + tw + 4.0));
                line.paint(
                    point(tx, area.origin.y),
                    area.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
        }
    }
    for (kind, builder) in slants {
        if let Ok(path) = builder.build() {
            window.paint_path(path, t.value_color(kind));
        }
    }
}

fn value_kind(v: &WaveValue) -> ValueKind {
    v.kind()
}
