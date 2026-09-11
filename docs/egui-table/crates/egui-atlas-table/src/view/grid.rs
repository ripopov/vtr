use super::*;

impl Table {
    pub(super) fn indicator(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        title: &str,
        detail: &str,
        progress: Option<f32>,
    ) {
        ui.painter().rect_filled(rect, 8.0, self.colors.surface);
        let inner = Rect::from_center_size(rect.center(), vec2(390.0, 140.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(26.0).color(self.colors.accent));
                ui.add_space(10.0);
                ui.label(RichText::new(title).size(20.0).strong());
                ui.label(RichText::new(detail).color(self.colors.muted));
                if let Some(p) = progress {
                    ui.add_space(9.0);
                    let (track, _) = ui.allocate_exact_size(vec2(280.0, 6.0), Sense::hover());
                    ui.painter().rect_filled(track, 3.0, self.colors.line);
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            track.min,
                            vec2((track.width() * p).max(3.0), track.height()),
                        ),
                        3.0,
                        self.colors.accent,
                    );
                    ui.label(
                        RichText::new(format!("{:.0}%", p * 100.0))
                            .size(12.0)
                            .color(self.colors.muted),
                    );
                }
            });
        });
    }
    pub(super) fn table(&mut self, ui: &mut egui::Ui, rect: Rect) {
        if rect.width() < GUTTER + 32.0 || rect.height() < 110.0 {
            self.painted_cells = 0;
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "More space needed",
                FontId::proportional(12.0),
                self.colors.muted,
            );
            return;
        }
        self.filter_keyboard(ui);
        ui.painter().rect_filled(rect, 8.0, self.colors.surface);
        let header = Rect::from_min_max(rect.min, pos2(rect.max.x - 14.0, rect.min.y + 42.0));
        let body = Rect::from_min_max(
            pos2(
                rect.min.x,
                header.max.y + if self.filters_visible { 38.0 } else { 0.0 },
            ),
            pos2(rect.max.x - 14.0, rect.max.y - 14.0),
        );
        let page_rows = (body.height() as f64 / ROW).max(1.0);
        let max_scroll = (self.view.len() as f64 - page_rows).max(0.0);
        let body_response = ui.interact(
            body,
            self.widget_id("table-body"),
            if self.filtering {
                Sense::hover()
            } else {
                Sense::click_and_drag()
            },
        );
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                self.widget_id("table-body"),
                egui::EventFilter {
                    escape: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            )
        });
        if body_response.clicked() || body_response.drag_started() {
            body_response.request_focus();
        }
        if body_response.hovered() && !self.filtering {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            self.scroll_row -= delta.y as f64 / ROW;
            self.scroll_x -= delta.x;
        }
        if body_response.dragged_by(egui::PointerButton::Primary)
            && !body_response.drag_started()
            && let Some(p) = body_response.interact_pointer_pos()
        {
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            let dy = if p.y < body.min.y + 20.0 {
                -1.0
            } else if p.y > body.max.y - 20.0 {
                1.0
            } else {
                0.0
            };
            let dx = if p.x < body.min.x + GUTTER + 15.0 {
                -1.0
            } else if p.x > body.max.x - 15.0 {
                1.0
            } else {
                0.0
            };
            self.scroll_row += dy * dt as f64 * 24.0;
            self.scroll_x += dx * dt * 500.0;
            if dx != 0.0 || dy != 0.0 {
                ui.ctx().request_repaint();
            }
        }
        let total_width: f32 = (0..self.column_count())
            .filter(|&c| self.visible[c])
            .map(|c| self.widths[c])
            .sum();
        let content_width = (body.width() - GUTTER).max(1.0);
        let max_x = (total_width - content_width).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, max_x);
        if body_response.has_focus() && !self.filtering {
            let mut index = self
                .selected
                .map(|r| self.view.rank_before(r))
                .unwrap_or(self.scroll_row as u32);
            let current = index;
            let mut moved = false;
            let mut extend_rows = false;
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::SHIFT, Key::ArrowDown) {
                    index = index.saturating_add(1);
                    moved = true;
                    extend_rows = true;
                } else if i.consume_key(egui::Modifiers::SHIFT, Key::ArrowUp) {
                    index = index.saturating_sub(1);
                    moved = true;
                    extend_rows = true;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::Home)
                    || i.consume_key(egui::Modifiers::COMMAND, Key::Home)
                {
                    index = 0;
                    moved = true;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::End)
                    || i.consume_key(egui::Modifiers::COMMAND, Key::End)
                {
                    index = self.view.len().saturating_sub(1);
                    moved = true;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::PageDown) {
                    index = index.saturating_add(page_rows as u32);
                    moved = true;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::PageUp) {
                    index = index.saturating_sub(page_rows as u32);
                    moved = true;
                }
                if self.search_text.is_empty() {
                    if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                        index = index.saturating_add(1);
                        moved = true;
                    }
                    if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                        index = index.saturating_sub(1);
                        moved = true;
                    }
                }
            });
            if moved && !self.view.is_empty() {
                index = index.min(self.view.len() - 1);
                if extend_rows {
                    self.selection.extend_row(current, index);
                    self.copy_status.clear();
                } else if matches!(self.selection, Selection::Rows { .. }) {
                    self.selection = Selection::Empty;
                }
                self.selected = self.view.select(index);
                if (index as f64) < self.scroll_row {
                    self.scroll_row = index as f64;
                }
                if index as f64 >= self.scroll_row + page_rows - 1.0 {
                    self.scroll_row = index as f64 - page_rows + 1.0;
                }
            }
        }
        self.scroll_row = self.scroll_row.clamp(0.0, max_scroll);
        self.scrollbar(
            ui,
            Rect::from_min_max(
                pos2(body.max.x + 2.0, body.min.y),
                pos2(rect.max.x - 2.0, body.max.y),
            ),
            max_scroll,
            page_rows,
        );
        if max_x > 0.0 {
            let track = Rect::from_min_max(
                pos2(body.min.x + GUTTER, body.max.y + 3.0),
                pos2(body.max.x, rect.max.y - 3.0),
            );
            let size = (track.width() * content_width / total_width)
                .max(36.0)
                .min(track.width());
            let travel = (track.width() - size).max(1.0);
            let thumb = Rect::from_min_size(
                pos2(track.min.x + self.scroll_x / max_x * travel, track.min.y),
                vec2(size, track.height()),
            );
            let r = ui.interact(track, self.widget_id("hscroll"), Sense::click_and_drag());
            if (r.dragged() || r.clicked())
                && let Some(p) = r.interact_pointer_pos()
            {
                self.scroll_x =
                    ((p.x - track.min.x - size / 2.0) / travel * max_x).clamp(0.0, max_x);
            }
            ui.painter().rect_filled(
                thumb,
                5.0,
                if r.hovered() {
                    self.colors.muted
                } else {
                    self.colors.line
                },
            );
        }
        let mut columns = Vec::new();
        let width_scale = (content_width / total_width).max(1.0);
        let mut x = body.min.x + GUTTER - self.scroll_x;
        for c in 0..self.column_count() {
            if !self.visible[c] {
                continue;
            }
            let w = self.widths[c] * width_scale;
            if x + w > body.min.x + GUTTER && x < body.max.x {
                columns.push((c, x, w));
            }
            x += w;
        }
        self.column_headers(ui, header, &columns);
        if self.filtering {
            self.painted_cells = 0;
            self.indicator(
                ui,
                body,
                "Updating your view",
                "Keep typing to refine your filters.",
                Some(self.filter_progress),
            );
            return;
        }
        let hit = |p: egui::Pos2| -> Option<(u32, usize)> {
            if self.view.is_empty() {
                return None;
            }
            let px = p.x.clamp(body.min.x + GUTTER + 0.1, body.max.x - 0.1);
            let column = columns.iter().find(|(_, x, w)| px >= *x && px < x + w)?.0;
            let row = (self.scroll_row
                + ((p.y.clamp(body.min.y, body.max.y - 0.1) - body.min.y) as f64 / ROW))
                .floor() as u32;
            Some((row.min(self.view.len() - 1), column))
        };
        if body_response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(point) = ui.input(|i| i.pointer.press_origin()).and_then(hit) {
                self.selection.cell(point, ui.input(|i| i.modifiers.shift));
                self.copy_status.clear();
            }
        } else if body_response.clicked_by(egui::PointerButton::Primary)
            && let Some(point) = body_response.interact_pointer_pos().and_then(hit)
        {
            self.selection.cell(point, ui.input(|i| i.modifiers.shift));
            self.copy_status.clear();
        }
        if (body_response.dragged_by(egui::PointerButton::Primary)
            || body_response.drag_stopped_by(egui::PointerButton::Primary))
            && let Some(point) = body_response.interact_pointer_pos().and_then(hit)
        {
            self.selection.cell(point, true);
            self.selected = self.view.select(point.0);
        }
        let first = self.scroll_row.floor() as u32;
        let end = ((self.scroll_row + page_rows).ceil() as u32)
            .saturating_add(1)
            .min(self.view.len());
        let rows: Vec<(u32, u32)> = (first..end)
            .filter_map(|i| self.view.select(i).map(|r| (i, r)))
            .collect();

        let mut keys: Vec<_> = rows
            .iter()
            .flat_map(|(_, r)| columns.iter().map(move |(c, _, _)| (*r, *c)))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        self.cache.request(&keys);
        let loading = keys.iter().any(|k| self.cache.value(k.0, k.1).is_none());
        if loading {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        // All painter positions are relative to the viewport. Never multiply a
        // ten-million-row index by a row height in f32 screen coordinates.
        let painter = ui.painter().with_clip_rect(body);
        let cell_clip = Rect::from_min_max(pos2(body.min.x + GUTTER, body.min.y), body.max);
        self.painted_cells = 0;
        for (index, source) in rows {
            let y = body.min.y + ((index as f64 - self.scroll_row) * ROW) as f32;
            let row_rect = Rect::from_min_size(pos2(body.min.x, y), vec2(body.width(), ROW as f32));
            let selected = self.selected == Some(source);
            let hovered = body_response.hovered()
                && ui.input(|i| i.pointer.hover_pos().is_some_and(|p| row_rect.contains(p)));
            let fill = if selected || hovered {
                self.colors.hover
            } else if index % 2 == 1 {
                self.colors.row_alt
            } else {
                self.colors.surface
            };
            painter.rect_filled(row_rect, 0.0, fill);
            if selected {
                painter.rect_filled(
                    Rect::from_min_size(row_rect.min, vec2(3.0, ROW as f32)),
                    0.0,
                    self.colors.accent,
                );
            }
            if hovered && body_response.clicked() {
                self.selected = Some(source);
            }
            painter.text(
                pos2(body.min.x + GUTTER - 13.0, y + ROW as f32 / 2.0),
                Align2::RIGHT_CENTER,
                number(source + 1),
                FontId::monospace(11.0),
                self.colors.muted,
            );
            for &(c, left, width) in &columns {
                let full_cell = Rect::from_min_size(pos2(left, y), vec2(width, ROW as f32));
                let cell = full_cell.intersect(cell_clip);
                let cp = painter.with_clip_rect(cell.shrink2(vec2(1.0, 0.0)));
                if self.selection.contains(index, c) {
                    cp.rect_filled(cell, 0.0, self.colors.selected);
                    cp.rect_stroke(
                        cell,
                        0.0,
                        Stroke::new(1.0, self.colors.accent),
                        egui::StrokeKind::Inside,
                    );
                }
                if let Some(value) = self.cache.value(source, c) {
                    let text_pos = pos2(left + 13.0, y + ROW as f32 / 2.0);
                    if c == self.search_column
                        && !self.search_text.is_empty()
                        && self.matches.contains(source)
                    {
                        cp.rect_filled(
                            Rect::from_min_size(
                                pos2(left + 7.0, y + 5.0),
                                vec2(width - 14.0, ROW as f32 - 10.0),
                            ),
                            4.0,
                            self.colors.match_fill,
                        );
                    }
                    let style = &self.options.columns[c].style;
                    if let Some(renderer) = &self.options.cell_painter {
                        renderer.paint(
                            &cp,
                            full_cell,
                            source,
                            c,
                            value,
                            self.selection.contains(index, c),
                        );
                    } else {
                        let (align, p) = if style.right_aligned {
                            (Align2::RIGHT_CENTER, pos2(left + width - 14.0, text_pos.y))
                        } else {
                            (Align2::LEFT_CENTER, text_pos)
                        };
                        cp.text(
                            p,
                            align,
                            value,
                            if style.monospace {
                                FontId::monospace(12.0)
                            } else {
                                FontId::proportional(13.0)
                            },
                            if style.muted {
                                self.colors.muted
                            } else {
                                self.colors.ink
                            },
                        );
                    }
                    if hovered
                        && ui.input(|i| i.pointer.hover_pos().is_some_and(|p| cell.contains(p)))
                        && body_response.clicked()
                    {
                        self.cell_value = Some((source, c, value.to_owned()));
                    }
                } else {
                    cp.rect_filled(
                        Rect::from_min_size(
                            pos2(left + 13.0, y + 13.0),
                            vec2((width * 0.55).min(110.0), 8.0),
                        ),
                        4.0,
                        self.colors.line,
                    );
                }
                self.painted_cells += 1;
            }
        }
        painter.line_segment(
            [
                pos2(body.min.x + GUTTER, body.min.y),
                pos2(body.min.x + GUTTER, body.max.y),
            ],
            Stroke::new(1.0, self.colors.line),
        );
        if self.view.is_empty() {
            painter.text(
                body.center() - vec2(0.0, 16.0),
                Align2::CENTER_CENTER,
                "No records match these filters",
                FontId::proportional(20.0),
                self.colors.ink,
            );
            painter.text(
                body.center() + vec2(0.0, 16.0),
                Align2::CENTER_CENTER,
                "Try fewer letters, or remove a filter above.",
                FontId::proportional(14.0),
                self.colors.muted,
            );
        }
        ui.painter().rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0, self.colors.line),
            egui::StrokeKind::Inside,
        );
    }
    pub(super) fn scrollbar(&mut self, ui: &mut egui::Ui, track: Rect, max_scroll: f64, page: f64) {
        if max_scroll <= 0.0 {
            return;
        }
        let size = (track.height() * (page / (max_scroll + page)) as f32)
            .max(38.0)
            .min(track.height());
        let travel = (track.height() - size).max(1.0);
        let thumb = Rect::from_min_size(
            pos2(
                track.min.x,
                track.min.y + (self.scroll_row / max_scroll) as f32 * travel,
            ),
            vec2(track.width(), size),
        );
        let r = ui.interact(track, self.widget_id("vscroll"), Sense::click_and_drag());
        let grab_id = self.widget_id("vscroll-grab");
        if (r.drag_started() || r.clicked())
            && let Some(p) = r.interact_pointer_pos()
        {
            let grab = if thumb.contains(p) {
                p.y - thumb.min.y
            } else {
                size / 2.0
            };
            ui.data_mut(|d| d.insert_temp(grab_id, grab));
        }
        if (r.dragged() || r.clicked())
            && let Some(p) = r.interact_pointer_pos()
        {
            let grab = ui
                .data(|d| d.get_temp::<f32>(grab_id))
                .unwrap_or(size / 2.0);
            self.scroll_row =
                (((p.y - track.min.y - grab) / travel) as f64 * max_scroll).clamp(0.0, max_scroll);
        }
        let thumb = Rect::from_min_size(
            pos2(
                track.min.x,
                track.min.y + (self.scroll_row / max_scroll) as f32 * travel,
            ),
            vec2(track.width(), size),
        );
        ui.painter().rect_filled(
            thumb,
            5.0,
            if r.hovered() || r.dragged() {
                self.colors.muted
            } else {
                self.colors.line
            },
        );
    }
}
