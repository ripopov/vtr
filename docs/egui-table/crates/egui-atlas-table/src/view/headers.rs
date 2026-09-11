use super::*;

impl Table {
    pub(super) fn filter_keyboard(&mut self, ui: &mut egui::Ui) {
        if !self.filters_visible {
            return;
        }
        let focused = (0..self.column_count())
            .find(|&c| ui.memory(|m| m.has_focus(self.widget_id(("column-filter", c)))));
        let Some(column) = focused else {
            return;
        };
        let direction = ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::SHIFT, Key::Tab) {
                -1
            } else if i.consume_key(egui::Modifiers::NONE, Key::Tab) {
                1
            } else {
                0
            }
        });
        if direction != 0 {
            let visible: Vec<_> = (0..self.column_count())
                .filter(|&c| self.visible[c])
                .collect();
            if let Some(index) = visible.iter().position(|&c| c == column) {
                let next = (index as isize + direction).rem_euclid(visible.len() as isize) as usize;
                self.edit_filter(visible[next]);
                ui.ctx().request_repaint();
            }
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
            && !self.filters[column].is_empty()
        {
            self.filters[column].clear();
            self.changed_filter();
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
            && self.filter_pending.is_some()
        {
            self.filter_pending = Some(Instant::now() - Duration::from_secs(1));
            ui.ctx().request_repaint();
        }
    }
    pub(super) fn filter_inputs(
        &mut self,
        ui: &mut egui::Ui,
        header: Rect,
        columns: &[(usize, f32, f32)],
    ) {
        let band = Rect::from_min_max(
            header.left_bottom(),
            header.right_bottom() + vec2(0.0, 38.0),
        );
        ui.painter().rect_filled(band, 0.0, self.colors.header);
        ui.painter().text(
            pos2(band.min.x + GUTTER - 13.0, band.center().y),
            Align2::RIGHT_CENTER,
            "FILTER",
            FontId::proportional(10.0),
            self.colors.muted,
        );
        let clip = Rect::from_min_max(pos2(band.min.x + GUTTER, band.min.y), band.max);
        for &(c, left, width) in columns {
            let active = !self.filters[c].is_empty();
            let full =
                Rect::from_min_size(pos2(left + 9.0, band.min.y + 4.0), vec2(width - 18.0, 29.0));
            let edit = Rect::from_min_max(
                full.min,
                full.max - vec2(if active { 25.0 } else { 0.0 }, 0.0),
            )
            .intersect(clip);
            if edit.width() < 18.0 {
                continue;
            }
            let filter_id = self.widget_id(("column-filter", c));
            ui.scope_builder(egui::UiBuilder::new().max_rect(edit), |ui| {
                ui.set_clip_rect(edit);
                ui.spacing_mut().interact_size.y = 28.0;
                if active {
                    ui.visuals_mut().widgets.inactive.bg_stroke =
                        Stroke::new(1.0, self.colors.accent);
                }
                let response = ui.add_sized(
                    edit.size(),
                    egui::TextEdit::singleline(&mut self.filters[c])
                        .id(filter_id)
                        .font(FontId::proportional(12.0))
                        .margin(vec2(7.0, 5.0))
                        .hint_text("Filter…"),
                );
                if self.focus_filter == Some(c) {
                    response.request_focus();
                    self.focus_filter = None;
                }
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        self.widget_id(("column-filter", c)),
                        egui::EventFilter {
                            tab: true,
                            escape: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                        },
                    )
                });
                if response.changed() {
                    self.changed_filter();
                }
            });
            if active {
                let clear = Rect::from_min_size(
                    pos2(full.max.x - 23.0, full.min.y),
                    vec2(23.0, full.height()),
                )
                .intersect(clip);
                if clear.width() > 16.0
                    && ui.put(clear, egui::Button::new("×").frame(false)).clicked()
                {
                    self.filters[c].clear();
                    self.changed_filter();
                    self.focus_filter = Some(c);
                    ui.ctx().request_repaint();
                }
            }
        }
        ui.painter().line_segment(
            [band.left_bottom(), band.right_bottom()],
            Stroke::new(1.0, self.colors.line),
        );
    }
    pub(super) fn column_headers(
        &mut self,
        ui: &mut egui::Ui,
        header: Rect,
        columns: &[(usize, f32, f32)],
    ) {
        ui.painter().rect_filled(header, 0.0, self.colors.header);
        ui.painter().text(
            pos2(header.min.x + GUTTER - 13.0, header.center().y),
            Align2::RIGHT_CENTER,
            "RECORD",
            FontId::proportional(10.0),
            self.colors.muted,
        );
        let header_clip = Rect::from_min_max(pos2(header.min.x + GUTTER, header.min.y), header.max);
        for &(c, left, width) in columns {
            let cell = Rect::from_min_size(pos2(left, header.min.y), vec2(width, header.height()));
            let clip = cell.intersect(header_clip);
            let hp = ui.painter().with_clip_rect(clip);
            let name = self.name(c).to_owned();
            if matches!(self.selection, Selection::Columns { .. }) && self.selection.contains(0, c)
            {
                hp.rect_filled(cell, 0.0, self.colors.selected);
            }
            hp.text(
                pos2(left + 13.0, header.center().y),
                Align2::LEFT_CENTER,
                &name,
                FontId::proportional(12.0),
                if c == self.search_column {
                    self.colors.accent
                } else {
                    self.colors.ink
                },
            );
            if c == self.search_column {
                hp.line_segment(
                    [
                        pos2(left + 12.0, header.max.y - 1.0),
                        pos2(left + width - 12.0, header.max.y - 1.0),
                    ],
                    Stroke::new(2.0, self.colors.accent),
                );
            }
            let title = Rect::from_min_max(cell.min, pos2(cell.max.x - 4.0, cell.max.y))
                .intersect(header_clip);
            let title_response = ui.interact(title, self.widget_id(("header", c)), Sense::click());
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    self.widget_id(("header", c)),
                    egui::EventFilter {
                        escape: true,
                        ..Default::default()
                    },
                )
            });
            if title_response.clicked() {
                let modifiers = ui.input(|i| i.modifiers);
                self.selection.column(
                    c,
                    modifiers.shift,
                    modifiers.command || modifiers.ctrl,
                    &self.visible,
                );
                self.copy_status.clear();
                title_response.request_focus();
            }
            if title_response.double_clicked()
                && ui.input(|i| !i.modifiers.shift && !i.modifiers.command && !i.modifiers.ctrl)
            {
                self.search_column = c;
                self.changed_search();
                self.focus_search = true;
            }
            let resize = Rect::from_min_max(
                pos2(cell.max.x - 3.0, cell.min.y),
                pos2(cell.max.x + 3.0, cell.max.y),
            )
            .intersect(header_clip);
            let response = ui
                .interact(resize, self.widget_id(("resize", c)), Sense::drag())
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            if response.dragged() {
                self.widths[c] =
                    (self.widths[c] + ui.input(|i| i.pointer.delta().x)).clamp(100.0, 600.0);
            }
            if response.double_clicked() {
                self.widths[c] = 180.0;
            }
        }
        // Paint separators above all cell backgrounds and clip to the header.
        // A cell clip ends at the stroke's center and can cut off the entire
        // line as pixel rounding changes during a resize.
        let separators = ui.painter().with_clip_rect(header_clip);
        for &(_, left, width) in columns {
            separators.line_segment(
                [
                    pos2(left + width, header.min.y + 11.0),
                    pos2(left + width, header.max.y - 11.0),
                ],
                Stroke::new(1.0, self.colors.line),
            );
        }
        ui.painter().line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, self.colors.line),
        );
        if self.filters_visible {
            self.filter_inputs(ui, header, columns);
        }
    }
}
