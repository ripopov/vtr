use super::*;

impl Table {
    pub(super) fn dock_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new("Columns").selected(self.dock == Some(Dock::Columns)))
                .clicked()
            {
                self.dock = Some(Dock::Columns);
            }
            if ui
                .add(egui::Button::new("Help & info").selected(self.dock == Some(Dock::Help)))
                .clicked()
            {
                self.dock = Some(Dock::Help);
            }
            if ui.button("Close").clicked() {
                self.dock = None;
            }
        });
        ui.separator();
        if self.dock == Some(Dock::Columns) {
            ui.label(
                RichText::new("Check to show · click a name to search")
                    .size(12.0)
                    .color(self.colors.muted),
            );

            ui.spacing_mut().item_spacing.y = 5.0;
            ui.spacing_mut().interact_size.y = 26.0;
            ui.label(RichText::new("Choose your columns").strong().size(16.0));
            ui.add(
                egui::TextEdit::singleline(&mut self.column_query)
                    .hint_text("Find a column…")
                    .desired_width(ui.available_width()),
            );
            ui.horizontal(|ui| {
                if ui.small_button("Reset columns").clicked() {
                    self.visible.fill(false);
                    self.visible[..self.options.initial_visible].fill(true);
                    self.scroll_x = 0.0;
                }
                if ui.small_button("Show all").clicked() {
                    self.visible.fill(true);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("column-list")
                .show(ui, |ui| {
                    let needle = self.column_query.to_lowercase();
                    for (group, range) in self.groups.clone() {
                        let columns: Vec<_> = range
                            .into_iter()
                            .filter(|&c| self.name(c).to_lowercase().contains(&needle))
                            .collect();
                        if columns.is_empty() {
                            continue;
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(group.to_uppercase())
                                .size(10.0)
                                .color(self.colors.muted),
                        );
                        for c in columns {
                            let name = self.name(c).to_owned();
                            let last =
                                self.visible[c] && self.visible.iter().filter(|&&v| v).count() == 1;
                            ui.horizontal(|ui| {
                                ui.add_enabled(
                                    !last,
                                    egui::Checkbox::without_text(&mut self.visible[c]),
                                );
                                // Keep the frame allocated in every state: selectable_label
                                // removes its border when idle, changing the row's height.
                                if ui
                                    .add(egui::Button::new(name).selected(self.search_column == c))
                                    .clicked()
                                {
                                    self.search_column = c;
                                    self.reveal_column(c);
                                    self.changed_search();
                                    self.focus_search = true;
                                }
                            });
                        }
                    }
                });
        } else if self.dock == Some(Dock::Help) {
            egui::ScrollArea::vertical().id_salt("viewer-help").show(ui, |ui| {
                ui.heading("Explore the table");
                ui.label("Click a header to select its column. Shift-click extends a range; Ctrl/Cmd-click toggles columns. Double-click a header to search it. All filters must match.");
                ui.label("Fuzzy matching ignores case and allows gaps: untd matches United Kingdom.");
                ui.separator();
                for (keys, action) in [
                    ("Shift+Up / Shift+Down", "Extend selection by whole rows"),
                    ("Ctrl/Cmd+C", "Copy selection with headers"),
                    ("Ctrl/Cmd+F", "Focus search"),
                    ("Up / Down", "Previous / next search match"),
                    ("Shift+Enter / Enter", "Previous / next search match"),
                    ("Tab / Shift+Tab", "Next / previous filter"),
                    ("Esc", "Clear the focused search or filter"),
                    ("Ctrl/Cmd+Shift+L", "Show / hide filter row"),
                    ("Home / End", "First / last row with table focused"),
                ] { ui.label(RichText::new(keys).strong()); ui.label(action); }
                ui.separator();
                ui.label("Left-drag across cells to select a rectangle. Drag at the edges to scroll. Shift-click extends the rectangle. Ctrl/Cmd+C copies selected cells with column headers as tab-separated text. Column selection includes every row in the filtered view. Copy runs in the background, with a 128 MiB text limit and Cancel. Changing filters clears selection and cancels copy. Drag header dividers to resize; double-click to reset.");
                ui.collapsing("Performance", |ui| {
                    self.diagnostics(ui);
                });
            });
        }
    }
    pub(super) fn diagnostics(&self, ui: &mut egui::Ui) {
        ui.label(format!(
            "UI build: {:.2} ms · {} visible cells",
            self.frame_ms, self.painted_cells
        ));

        ui.label(format!(
            "Decoded cache: {:.1} / 64 MiB",
            self.cache.bytes() as f64 / 1048576.0
        ));

        ui.label(format!(
            "{} rows · {} columns",
            self.view.len(),
            self.column_count()
        ));
        ui.label(format!(
            "Filter: {:.0} ms · mapping {:.2} MiB",
            self.filter_elapsed.as_secs_f64() * 1000.0,
            self.view.bytes() as f64 / 1048576.0
        ));
        ui.label(format!(
            "Search: {:.0} ms · {:.0}% scanned",
            self.search_elapsed.as_secs_f64() * 1000.0,
            self.search_progress * 100.0
        ));
    }
    pub(super) fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.options.title).size(25.0).strong());
            ui.label(
                RichText::new(format!("{} records", number(self.source.schema.rows)))
                    .color(self.colors.muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(format!(
                            "Columns  {} / {}",
                            self.visible.iter().filter(|&&v| v).count(),
                            self.column_count()
                        ))
                        .selected(self.dock == Some(Dock::Columns)),
                    )
                    .clicked()
                {
                    self.dock = if self.dock == Some(Dock::Columns) {
                        None
                    } else {
                        Some(Dock::Columns)
                    };
                }
                let count = self.filter_count();
                let label = if count == 0 {
                    "Filter row".into()
                } else {
                    format!("Filter row  {count}")
                };
                if ui
                    .add(egui::Button::new(label).selected(self.filters_visible))
                    .clicked()
                {
                    self.filters_visible = !self.filters_visible;
                }
                ui.label(
                    RichText::new("Read only")
                        .color(self.colors.muted)
                        .size(12.0),
                );
            });
        });
        ui.add_space(3.0);
        if !self.options.description.is_empty() {
            ui.label(RichText::new(&self.options.description).color(self.colors.muted));
        }
        ui.add_space(16.0);
        egui::Frame::new()
            .fill(self.colors.surface)
            .stroke(Stroke::new(1.0, self.colors.line))
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Find in").color(self.colors.muted));
                    if ui.button(self.name(self.search_column)).clicked() {
                        self.dock = if self.dock == Some(Dock::Columns) {
                            None
                        } else {
                            Some(Dock::Columns)
                        };
                    }
                    let ctx = ui.ctx().clone();
                    if ((ui.memory(|m| m.focused().is_none())
                        && ui.rect_contains_pointer(ui.max_rect()))
                        || ui.memory(|m| {
                            m.focused().is_some_and(|id| {
                                id == self.widget_id("table-body")
                                    || id == self.widget_id("find-text")
                                    || (0..self.column_count()).any(|c| {
                                        id == self.widget_id(("header", c))
                                            || id == self.widget_id(("column-filter", c))
                                    })
                            })
                        }))
                        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F))
                    {
                        self.focus_search = true;
                    }
                    let search_id = self.widget_id("find-text");
                    let search_focused = ui.memory(|m| {
                        m.has_focus(search_id) || m.has_focus(self.widget_id("table-body"))
                    });
                    if !self.search_text.is_empty() && search_focused {
                        if ctx.input_mut(|i| {
                            !i.modifiers.shift
                                && i.consume_key(egui::Modifiers::NONE, Key::ArrowDown)
                        }) {
                            self.next_match(false);
                        }
                        if ctx.input_mut(|i| {
                            !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, Key::ArrowUp)
                        }) {
                            self.next_match(true);
                        }
                        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Enter)) {
                            self.next_match(true);
                        } else if ctx
                            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter))
                        {
                            self.next_match(false);
                        }
                    }
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.search_text)
                            .id(search_id)
                            .desired_width((ui.available_width() - 300.0).clamp(140.0, 360.0))
                            .hint_text("Type to find…"),
                    );
                    if self.focus_search {
                        response.request_focus();
                        self.focus_search = false;
                    }
                    if response.changed() {
                        self.changed_search();
                    }
                    if response.has_focus()
                        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
                    {
                        self.search_text.clear();
                        self.changed_search();
                        response.surrender_focus();
                    }
                    if !self.search_text.is_empty() && ui.small_button("×").clicked() {
                        self.search_text.clear();
                        self.changed_search();
                    }
                    let n = self.matches.len();
                    if self.searching {
                        ui.add(egui::Spinner::new().size(16.0).color(self.colors.accent));
                    }
                    let text = if self.filtering && !self.search_text.is_empty() {
                        "Waiting for filters…".into()
                    } else if self.search_text.is_empty() {
                        "Up / Down to navigate".into()
                    } else if n == 0 && self.searching {
                        "Searching…".into()
                    } else if n == 0 {
                        "No matches".into()
                    } else {
                        format!(
                            "{} / {}{}",
                            number(self.match_index.unwrap_or(0) + 1),
                            number(n),
                            if self.searching { "+" } else { "" }
                        )
                    };
                    ui.label(RichText::new(text).size(12.0).color(self.colors.muted));
                    if arrow_button(ui, self.match_index.is_some_and(|i| i > 0), true).clicked() {
                        self.next_match(true);
                    }
                    if arrow_button(ui, n > 0 && self.match_index.unwrap_or(0) + 1 < n, false)
                        .clicked()
                    {
                        self.next_match(false);
                    }
                });
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            if self.filter_count() == 0 {
                ui.label(
                    RichText::new(if self.filters_visible {
                        "Filter below each heading · all rules must match"
                    } else {
                        "Show the filter row to narrow your view"
                    })
                    .size(12.0)
                    .color(self.colors.muted),
                );
            } else {
                ui.label(
                    RichText::new(format!("{} filters", self.filter_count()))
                        .size(12.0)
                        .color(self.colors.muted),
                );
                if ui.small_button("Clear all").clicked() {
                    self.clear_filters();
                }
                egui::ScrollArea::horizontal()
                    .id_salt("filter-summary")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_min_height(26.0);
                            for c in 0..self.column_count() {
                                if self.filters[c].trim().is_empty() {
                                    continue;
                                }
                                let value: String = self.filters[c].chars().take(24).collect();
                                let label = format!(
                                    "{}: {}{}",
                                    self.name(c),
                                    value,
                                    if self.visible[c] { "" } else { " (hidden)" }
                                );
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    if ui.small_button(label).clicked() {
                                        self.edit_filter(c);
                                    }
                                    if ui.small_button("×").clicked() {
                                        self.filters[c].clear();
                                        self.changed_filter();
                                    }
                                });
                            }
                        });
                    });
            }
        });
        ui.add_space(8.0);
    }
    pub(super) fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} rows", number(self.view.len())))
                    .size(12.0)
                    .strong(),
            );
            ui.label(RichText::new("·").color(self.colors.muted));
            if let Some(row) = self.selected {
                ui.label(
                    RichText::new(format!("Record {} selected", number(row + 1)))
                        .size(12.0)
                        .color(self.colors.muted),
                );
            } else {
                ui.label(
                    RichText::new("Select columns · Shift: range · Ctrl/Cmd: toggle")
                        .size(12.0)
                        .color(self.colors.muted),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Go").clicked() {
                    self.go_to_row();
                }
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.jump)
                        .hint_text("Row number")
                        .desired_width(100.0),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.go_to_row();
                }
                ui.label(RichText::new("Jump to").size(12.0).color(self.colors.muted));
                if ui
                    .add(egui::Button::new("Help & info").selected(self.dock == Some(Dock::Help)))
                    .clicked()
                {
                    self.dock = if self.dock == Some(Dock::Help) {
                        None
                    } else {
                        Some(Dock::Help)
                    };
                }
            });
        });
    }
}

impl Table {
    pub(super) fn selection_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(range) = self.selection.snapshot(self.view.len(), &self.visible) {
                ui.label(
                    RichText::new(format!(
                        "{} rows × {} cols",
                        number(range.last - range.first + 1),
                        range.columns.len()
                    ))
                    .size(12.0)
                    .strong(),
                );
                if ui
                    .add_enabled(
                        self.copy_job.is_none() && !self.filtering,
                        egui::Button::new("Copy with headers"),
                    )
                    .clicked()
                {
                    self.start_copy(ui.ctx());
                }
            }
            if let Some(job) = &self.copy_job {
                let done = job.progress.load(std::sync::atomic::Ordering::Relaxed);
                ui.spinner();
                ui.label(format!(
                    "Copying {:.0}%",
                    done as f64 / self.copy_rows.max(1) as f64 * 100.0
                ));
                if ui.small_button("Cancel").clicked() {
                    self.copy_job = None;
                    self.copy_status = "Copy cancelled".into();
                }
            } else if !self.copy_status.is_empty() {
                egui::ScrollArea::horizontal()
                    .id_salt("copy-status")
                    .show(ui, |ui| {
                        ui.label(&self.copy_status);
                    });
            } else if let Some((row, col, value)) = &self.cell_value {
                ui.label(
                    RichText::new(format!("Record {} · {}", number(row + 1), self.name(*col)))
                        .size(12.0)
                        .color(self.colors.muted),
                );
                egui::ScrollArea::horizontal()
                    .id_salt("cell-readout")
                    .show(ui, |ui| {
                        ui.label(value);
                    });
            } else {
                ui.label(
                    RichText::new("Click a cell to read its full value")
                        .size(12.0)
                        .color(self.colors.muted),
                );
            }
        });
    }
}
