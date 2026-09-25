//! Paint the pipeline panel into a [`Scene`]: the label column, one row per
//! transaction with a cell per stage on the primary lane and a band per
//! overlay stage, plus the shared timeline header, cursor and markers.
//!
//! Cost per frame is O(painted rows × stages per row): the painter walks the
//! visible row range, skips transactions and stages outside the time window,
//! and below two pixels per row paints density steps of several rows.

use crate::color::Color;
use crate::data::transactions::TxStatus;
use crate::document::Document;
use crate::geometry::{CursorIcon, Point, Rect, point, size, snap};
use crate::icons::IconName;
use crate::scene::{FontRole, Scene, TextCache, TextMeasure};
use crate::theme::Theme;
use crate::wave::overlay::{self, TextPainter, TimeColumn};

use super::model::{Drag, Hit, PipelineModel, Rows};
use super::palette::StagePalette;

/// Rows at least this tall get a stroked cell edge and a gap between cells.
const EDGE_MIN_PX: f32 = 6.0;
/// Rows at least this tall show stage names in wide enough cells.
const TEXT_MIN_PX: f32 = 10.0;
/// Rows at least this tall show label text; below, only flush ticks.
const LABEL_MIN_PX: f32 = 7.0;
/// Dash length of an open transaction's trailing edge.
const DASH_PX: f32 = 3.0;

/// The two short segments of an arrow head at the `to` end of a segment.
fn arrow_head(from: Point, to: Point, size: f32) -> [[Point; 2]; 2] {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = dx.hypot(dy).max(f32::EPSILON);
    let (ux, uy) = (dx / length, dy / length);
    let base = point(to.x - ux * size, to.y - uy * size);
    let half = size * 0.45;
    [
        [to, point(base.x - uy * half, base.y + ux * half)],
        [to, point(base.x + uy * half, base.y - ux * half)],
    ]
}

/// Paint `model` using the layout from its last [`PipelineModel::layout`] call.
pub fn paint(
    model: &PipelineModel,
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
    let mut p = TextPainter {
        theme,
        text,
        measure,
        scene,
    };
    let viewport = model.nav.viewport(doc);
    let cursor = model.nav.cursor(doc);
    let base = doc.time_base();
    let cells = layout.cells;
    let labels = layout.labels;
    let header = layout.header;
    let column = TimeColumn {
        header: Rect::new(
            point(cells.left(), header.top()),
            size(cells.width(), header.height()),
        ),
        rulers: Rect::new(
            point(cells.left(), layout.rulers.top()),
            size(cells.width(), layout.rulers.height()),
        ),
        area: cells,
        viewport,
    };
    let clocks = &model.nav.clocks;

    // -- backgrounds and the tick grid ----------------------------------------
    p.scene.fill(bounds, t.editor.bg);
    p.scene.fill(labels, t.panel.bg);
    p.scene.fill(header, t.panel.bg);
    let (tick_list, unit) = column.main_ticks(base, t.zoom, clocks, &doc.clocks);
    overlay::grid(&mut p, &column, &tick_list);

    // -- rows --------------------------------------------------------------------
    let rows = model.rows(doc);
    let hover_row = match model.hover {
        Some(Hit::Row(row) | Hit::Cell { row, .. }) => Some(row),
        _ => None,
    };
    let selected_row = model.selected_row(doc);
    match &rows {
        Rows::Ready(set) if !set.is_empty() => {
            let row_px = layout.rows.row_px;
            let step = layout.row_step;
            let rh = row_px * step as f32;
            let pad = if row_px >= 8.0 {
                z(1.5)
            } else if row_px >= 4.0 {
                z(0.5)
            } else {
                0.0
            };
            let gap = if row_px >= EDGE_MIN_PX { z(1.0) } else { 0.0 };
            let border = row_px >= EDGE_MIN_PX;
            let show_text = row_px >= TEXT_MIN_PX;
            let cell_font = (row_px * 0.62).clamp(z(6.0), t.mono_size);
            let palette = model.palette();
            let primary = palette.primary_lane();
            let flush_tint = t.editor.error.with_alpha(0.16);
            let band = t.editor.text.with_alpha(0.28);
            let band_edge = t.editor.text.with_alpha(0.5);
            let wave_wf = column.width_f64();
            let x_of = |time: u64| cells.left() + viewport.x_of(time as f64, wave_wf) as f32;
            // Text is measured outside the clip closure; positions are exact.
            let mut cell_texts: Vec<(Point, f32, String, Color)> = Vec::new();
            let mut open_edges: Vec<[Point; 2]> = Vec::new();
            // A density step shows the flush of any of its rows.
            let flushed_in = |r: usize| {
                (r..(r + step).min(set.len())).any(|i| {
                    set.get(i)
                        .is_some_and(|(_, tx)| tx.status == TxStatus::Aborted)
                })
            };
            for r in layout.row_range.clone().step_by(step) {
                let Some((_, tx)) = set.get(r) else { continue };
                let y = layout.row_y(r);
                let row_rect = Rect::new(point(cells.left(), y), size(cells.width(), rh));
                if flushed_in(r) {
                    p.scene
                        .clipped(cells, |scene| scene.fill(row_rect, flush_tint));
                }
                if hover_row == Some(r) {
                    p.scene
                        .clipped(cells, |scene| scene.fill(row_rect, t.wave_row_hover));
                }
                if (tx.end as f64) < viewport.start || (tx.begin as f64) > viewport.end {
                    continue;
                }
                let mut last_right: Option<f32> = None;
                let mut painted_any = false;
                for stage in &tx.stages {
                    let sb = stage.begin;
                    let se = PipelineModel::stage_end(tx, stage);
                    if (se as f64) <= viewport.start || (sb as f64) >= viewport.end {
                        if se == sb && (sb as f64) >= viewport.start && (sb as f64) <= viewport.end
                        {
                            // A zero-length stage at the window edge still counts.
                        } else {
                            continue;
                        }
                    }
                    let mut xa = x_of(sb) + gap / 2.0;
                    let mut xb = x_of(se) - gap / 2.0;
                    if xb - xa < 1.0 {
                        xa = xa.min(xb);
                        xb = xa + 1.0;
                    }
                    painted_any = true;
                    if stage.lane != primary {
                        let ya = y + rh * 0.55;
                        let yb = y + rh - pad;
                        let rect = Rect::from_xywh(xa, ya, xb - xa, (yb - ya).max(1.0));
                        p.scene.clipped(cells, |scene| {
                            scene.fill(rect, band);
                            if border {
                                scene.fill(Rect::from_xywh(xa, ya, xb - xa, 1.0), band_edge);
                            }
                        });
                        continue;
                    }
                    let style = palette.style(&stage.name);
                    let rect = Rect::from_xywh(xa, y + pad, xb - xa, (rh - 2.0 * pad).max(1.0));
                    let (border_w, border_c) = if border && xb - xa >= 3.0 {
                        (1.0, style.edge)
                    } else {
                        (0.0, Color::TRANSPARENT)
                    };
                    p.scene.clipped(cells, |scene| {
                        scene.quad(rect, style.fill, 0.0, border_w, border_c)
                    });
                    last_right = Some(last_right.map_or(xb, |r: f32| r.max(xb)));
                    if show_text && xb - xa >= z(14.0) {
                        let w = p.width(&stage.name, FontRole::Mono, cell_font);
                        if xb - xa >= w + z(4.0) {
                            cell_texts.push((
                                point(snap((xa + xb - w) / 2.0), y),
                                rh,
                                stage.name.clone(),
                                style.text,
                            ));
                        }
                    }
                }
                if !painted_any && tx.stages.is_empty() {
                    // A transaction without stages is one cell over its lifetime.
                    let style = StagePalette::fallback();
                    let mut xa = x_of(tx.begin) + gap / 2.0;
                    let mut xb = x_of(tx.end) - gap / 2.0;
                    if xb - xa < 1.0 {
                        xa = xa.min(xb);
                        xb = xa + 1.0;
                    }
                    let rect = Rect::from_xywh(xa, y + pad, xb - xa, (rh - 2.0 * pad).max(1.0));
                    let (border_w, border_c) = if border && xb - xa >= 3.0 {
                        (1.0, style.edge)
                    } else {
                        (0.0, Color::TRANSPARENT)
                    };
                    p.scene.clipped(cells, |scene| {
                        scene.quad(rect, style.fill, 0.0, border_w, border_c)
                    });
                    last_right = Some(xb);
                }
                if tx.status == TxStatus::Open
                    && let Some(xr) = last_right
                {
                    let x = snap(xr) + 0.5;
                    let mut yy = y + pad;
                    let bottom = y + rh - pad;
                    while yy < bottom {
                        let end = (yy + z(DASH_PX)).min(bottom);
                        open_edges.push([point(x, yy), point(x, end)]);
                        yy = end + z(DASH_PX);
                    }
                }
            }
            let text_color_default = t.editor.text;
            p.scene.clipped(cells, |scene| {
                for (origin, height, text, color) in cell_texts {
                    scene.text(origin, height, text, FontRole::Mono, cell_font, color);
                }
                scene.lines(open_edges, text_color_default, 1.0);
            });

            // -- the selection: an outline, and its relations as arrows ----------
            if let Some(row) = selected_row {
                let y = layout.row_y(row);
                let rect = Rect::new(point(cells.left(), y), size(cells.width(), row_px));
                let focus = t.border_focused;
                let (into, out) = (t.tx_relation_in, t.tx_relation_out);
                let selection = doc.selection();
                p.scene.clipped(cells, |scene| {
                    scene.quad(rect, Color::TRANSPARENT, 0.0, 1.0, focus);
                    // Arrows for one row are a fact about that row; arrows for
                    // every row would be a wall of lines.
                    let Some(selection) = selection else { return };
                    let Some(generator) = doc.resident_generator(selection.track) else {
                        return;
                    };
                    let center = |r: usize| layout.row_y(r) + row_px * 0.5;
                    let anchor = |r: usize, time: u64| point(x_of(time), center(r));
                    let here = anchor(row, set.get(row).map_or(0, |(_, tx)| tx.begin));
                    let mut incoming = Vec::new();
                    let mut outgoing = Vec::new();
                    for edge in generator.relations_of(selection.id) {
                        let forward = edge.relation.from == selection.id
                            && edge.from_generator == selection.track;
                        let (other, other_generator) = if forward {
                            (edge.relation.to, edge.to_generator)
                        } else {
                            (edge.relation.from, edge.from_generator)
                        };
                        let Some(other_row) = set.row_of(other_generator, other) else {
                            continue;
                        };
                        let Some((_, tx)) = set.get(other_row) else {
                            continue;
                        };
                        let far = anchor(other_row, tx.begin);
                        let (from, to) = if forward { (here, far) } else { (far, here) };
                        let target = if forward {
                            &mut outgoing
                        } else {
                            &mut incoming
                        };
                        target.push([from, to]);
                        target.extend(arrow_head(from, to, z(5.0)));
                    }
                    scene.lines(incoming, into, 1.0);
                    scene.lines(outgoing, out, 1.0);
                });
            }

            // -- label column ---------------------------------------------------------
            let label_font = (row_px * 0.62).clamp(z(7.0), t.mono_size);
            let digits = set.len().max(1).to_string().len();
            let digit_w = p.width("0", FontRole::Mono, label_font);
            let index_right = labels.left() + z(8.0) + digit_w * digits as f32;
            let text_left = index_right + z(8.0);
            if row_px >= LABEL_MIN_PX {
                let mut texts: Vec<(Point, String, Color)> = Vec::new();
                let mut fills: Vec<(Rect, Color)> = Vec::new();
                for r in layout.row_range.clone() {
                    let Some((_, tx)) = set.get(r) else { continue };
                    let y = layout.row_y(r);
                    if hover_row == Some(r) {
                        fills.push((
                            Rect::new(point(labels.left(), y), size(labels.width(), row_px)),
                            t.hover.bg,
                        ));
                    }
                    if selected_row == Some(r) {
                        fills.push((
                            Rect::new(point(labels.left(), y), size(labels.width(), row_px)),
                            t.wave_row_selected,
                        ));
                    }
                    let index = r.to_string();
                    let w = p.width(&index, FontRole::Mono, label_font);
                    texts.push((
                        point(snap(index_right - w), y),
                        index,
                        t.panel.text_placeholder,
                    ));
                    let label = PipelineModel::label(tx);
                    let (label, color) = match tx.status {
                        TxStatus::Aborted => {
                            fills.push((
                                Rect::new(
                                    point(labels.left(), y + pad),
                                    size(z(2.0), row_px - 2.0 * pad),
                                ),
                                t.editor.error,
                            ));
                            (label, t.panel.text_muted)
                        }
                        TxStatus::Open => (label, t.panel.text_muted),
                        _ => (label, t.panel.text),
                    };
                    if !label.is_empty() {
                        texts.push((point(text_left, y), label, color));
                    }
                }
                p.scene.clipped(labels, |scene| {
                    for (rect, color) in fills {
                        scene.fill(rect, color);
                    }
                    for (origin, text, color) in texts {
                        scene.text(origin, row_px, text, FontRole::Mono, label_font, color);
                    }
                });
            } else {
                let mut ticks = Vec::new();
                for r in layout.row_range.clone().step_by(step) {
                    if flushed_in(r) {
                        ticks.push(Rect::new(
                            point(labels.left(), layout.row_y(r)),
                            size(z(2.0), rh.max(1.0)),
                        ));
                    }
                }
                let color = t.editor.error;
                p.scene.clipped(labels, |scene| {
                    for tick in ticks {
                        scene.fill(tick, color);
                    }
                });
            }
        }
        _ => {
            // -- empty states --------------------------------------------------------
            let area = Rect::new(
                point(bounds.left(), cells.top()),
                size(bounds.width(), cells.height()),
            );
            p.scene.fill(area, t.editor.bg);
            let path = model.track.path().join(".");
            let (title, hint, error): (String, String, bool) = match &rows {
                Rows::Loading => (format!("Loading {path}…"), String::new(), false),
                Rows::Failed(e) => (format!("Cannot load {path}"), (*e).to_owned(), true),
                Rows::Unresolved => (
                    "Not in this trace".into(),
                    format!("{path} was saved in the workspace but is not a track here"),
                    false,
                ),
                Rows::Unavailable => ("No trace open".into(), String::new(), false),
                Rows::Ready(_) => (
                    "No transactions recorded".into(),
                    format!("{path} has no records"),
                    false,
                ),
            };
            let cx_ = area.left() + area.width() / 2.0;
            let cy = area.top() + area.height() / 2.0;
            let icon = Rect::new(point(cx_ - z(16.0), cy - z(44.0)), size(z(32.0), z(32.0)));
            p.scene.icon(
                IconName::Workflow,
                icon,
                if error {
                    t.editor.error
                } else {
                    t.editor.text_placeholder
                },
            );
            let title_w = p.width(&title, FontRole::UiMedium, t.ui_size);
            p.scene.text(
                point(snap(cx_ - title_w / 2.0), cy - z(4.0)),
                z(20.0),
                title,
                FontRole::UiMedium,
                t.ui_size,
                t.editor.text_muted,
            );
            if !hint.is_empty() {
                let hint_w = p.width(&hint, FontRole::Ui, t.ui_size_small);
                p.scene.text(
                    point(snap(cx_ - hint_w / 2.0), cy + z(18.0)),
                    z(18.0),
                    hint,
                    FontRole::Ui,
                    t.ui_size_small,
                    if error {
                        t.editor.error
                    } else {
                        t.editor.text_placeholder
                    },
                );
            }
            if let Some(retry) = layout.retry {
                let hovered = model.hover == Some(Hit::Retry);
                let surface = if hovered { t.button_hover } else { t.button };
                p.scene
                    .quad(retry, surface.bg, z(4.0), 1.0, t.border_variant);
                let label = "Retry";
                let w = p.width(label, FontRole::UiMedium, t.ui_size_small);
                p.scene.text(
                    point(snap(retry.left() + (retry.width() - w) / 2.0), retry.top()),
                    retry.height(),
                    label,
                    FontRole::UiMedium,
                    t.ui_size_small,
                    surface.text,
                );
                p.scene.cursors.push((retry, CursorIcon::PointingHand));
            }
        }
    }

    if matches!(&rows, Rows::Ready(set) if !set.is_empty()) {
        let activity = model.activity(doc);
        if activity.visible == 0 {
            let message = if activity.above + activity.below == 0 {
                match (activity.earlier, activity.later) {
                    (true, true) => "No pipeline activity in this time window · activity ← and →",
                    (true, false) => "No pipeline activity in this time window · activity ←",
                    (false, true) => "No pipeline activity in this time window · activity →",
                    _ => "No pipeline activity in this time window",
                }
            } else {
                "No activity in these rows · reveal rows above/below or resume follow"
            };
            p.scene.clipped(cells, |scene| {
                scene.text(
                    point(cells.left() + z(12.0), cells.top() + cells.height() * 0.5),
                    z(22.0),
                    message,
                    FontRole::Ui,
                    t.ui_size_small,
                    t.editor.text_muted,
                );
            });
        }
    }

    for (_, rect, label) in &layout.activity_controls {
        p.scene
            .quad(*rect, t.button.bg, z(6.0), 1.0, t.border_variant);
        let width = p.width(label, FontRole::UiMedium, t.ui_size_small);
        p.scene.text(
            point(rect.left() + (rect.width() - width) * 0.5, rect.top()),
            rect.height(),
            label.clone(),
            FontRole::UiMedium,
            t.ui_size_small,
            t.button.text,
        );
        p.scene.cursors.push((*rect, CursorIcon::PointingHand));
    }

    // -- header: column titles, tick labels, unit ------------------------------
    p.scene.clipped(
        Rect::new(header.origin, size(labels.width(), header.height())),
        |scene| {
            scene.text(
                point(labels.left() + z(8.0), header.top()),
                header.height(),
                "#",
                FontRole::UiSemibold,
                t.ui_size_small,
                t.panel.text_muted,
            );
            scene.text(
                point(labels.left() + z(36.0), header.top()),
                header.height(),
                "LABEL",
                FontRole::UiSemibold,
                t.ui_size_small,
                t.panel.text_muted,
            );
        },
    );
    overlay::header_ticks(&mut p, &column, &tick_list, &unit);
    overlay::clock_rulers(
        &mut p,
        &column,
        Rect::new(
            point(bounds.left(), layout.rulers.top()),
            size(labels.width(), layout.rulers.height()),
        ),
        clocks,
        &doc.clocks,
        base,
    );

    // -- markers and cursor -----------------------------------------------------
    overlay::markers(&mut p, &column, doc, &layout.marker_chips, model.pointer);
    overlay::cursor(
        &mut p,
        &column,
        cursor,
        |c| overlay::cursor_label(c, base, clocks, &doc.clocks),
        focused,
        z(4.0),
    );

    // -- borders and pointer shapes ---------------------------------------------
    let near_split = model.hover == Some(Hit::LabelSplit);
    let split_border = if near_split || model.drag == Some(Drag::LabelSplit) {
        t.border_focused
    } else {
        t.border
    };
    p.scene.fill(
        Rect::new(
            point(labels.right() - 1.0, bounds.top()),
            size(1.0, bounds.height()),
        ),
        split_border,
    );
    p.scene.fill(
        Rect::new(
            point(bounds.left(), header.bottom() - 1.0),
            size(bounds.width(), 1.0),
        ),
        t.border_variant,
    );
    match model.drag {
        Some(Drag::LabelSplit) => p.scene.window_cursor = Some(CursorIcon::ResizeLeftRight),
        Some(Drag::Pan { moved: true, .. }) => p.scene.window_cursor = Some(CursorIcon::Grabbing),
        _ => p
            .scene
            .cursors
            .push((layout.label_split, CursorIcon::ResizeLeftRight)),
    }
}
