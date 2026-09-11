use super::*;

impl Table {
    pub(super) fn filter_count(&self) -> usize {
        self.filters
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.trim().is_empty())
            .count()
    }
    pub(super) fn changed_filter(&mut self) {
        self.selection = Selection::Empty;
        self.cell_value = None;
        self.copy_job = None;
        self.copy_status.clear();

        self.filter_id = self.worker.cancel();

        self.filter_pending = Some(Instant::now());
        self.filtering = true;
        self.filter_progress = 0.0;
        self.search_pending = None;
        self.searching = false;
        self.matches = RowSet::All(0);
        self.match_index = None;
        self.selected = None;
    }
    pub(super) fn changed_search(&mut self) {
        self.matches = RowSet::All(0);
        self.match_index = None;
        self.search_progress = 0.0;
        // Filters own the shared worker until the new view is ready.
        if self.filtering {
            return;
        }

        self.search_id = self.worker.cancel();

        self.searching = !self.search_text.trim().is_empty();
        self.search_pending = self.searching.then(Instant::now);
    }
    pub(super) fn start_copy(&mut self, ctx: &egui::Context) {
        if self.filtering || self.copy_job.is_some() {
            return;
        }
        let Some(range) = self.selection.snapshot(self.view.len(), &self.visible) else {
            return;
        };
        let store = self.source.clone();
        self.copy_rows = range.last - range.first + 1;
        match CopyJob::start(store, self.view.clone(), range, ctx.clone()) {
            Ok(job) => {
                self.copy_job = Some(job);
                self.copy_status.clear();
            }
            Err(error) => self.copy_status = format!("Unable to start copy: {error}"),
        }
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.copy_job {
            match job.rx.try_recv() {
                Ok(result) => {
                    self.copy_job = None;
                    match result {
                        Ok(text) => {
                            ctx.copy_text(text);
                            self.copy_status = format!(
                                "Sent {} rows + headers to clipboard",
                                number(self.copy_rows)
                            );
                        }
                        Err(error) => self.copy_status = format!("Copy failed: {error:#}"),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.copy_job = None;
                    self.copy_status = "Copy worker stopped; nothing copied".into();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50))
                }
            }
        }
        if let Some(e) = self.cache.poll() {
            self.error = Some(e);
        }
        let events: Vec<_> = self.worker.rx.try_iter().collect();
        for event in events {
            match event {
                query::Event::Progress { id, fraction }
                    if id == self.filter_id && self.filtering =>
                {
                    self.filter_progress = fraction
                }
                query::Event::Filtered { id, view, elapsed }
                    if id == self.filter_id && self.filtering =>
                {
                    self.view = view;
                    self.filter_elapsed = elapsed;
                    self.filtering = false;
                    self.scroll_row = 0.0;
                    self.selected = None;
                    self.changed_search();
                }
                query::Event::Matches {
                    id,
                    matches,
                    done,
                    fraction,
                    elapsed,
                } if id == self.search_id && !self.filtering => {
                    self.matches = matches;
                    self.searching = !done;
                    self.search_progress = fraction;
                    self.search_elapsed = elapsed;
                    if self.match_index.is_none() && !self.matches.is_empty() {
                        self.match_index = Some(0);
                        self.jump_to_match();
                    }
                }
                query::Event::Error { id, message }
                    if id == self.filter_id || id == self.search_id || id == 0 =>
                {
                    self.error = Some(message);
                    self.searching = false;
                    self.filtering = false;
                }
                _ => {}
            }
        }
        if let Some(since) = self.filter_pending {
            if since.elapsed() >= self.options.filter_debounce {
                self.filter_pending = None;
                let predicates: Vec<_> = self
                    .filters
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| !s.trim().is_empty())
                    .map(|(column, text)| Predicate {
                        column,
                        text: text.trim().to_owned(),
                    })
                    .collect();

                self.worker.submit(Request::Filter {
                    id: self.filter_id,
                    predicates,
                });
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
        if let Some(since) = self.search_pending {
            if since.elapsed() >= self.options.search_debounce {
                self.search_pending = None;

                self.worker.submit(Request::Search {
                    id: self.search_id,
                    predicate: Predicate {
                        column: self.search_column,
                        text: self.search_text.trim().to_owned(),
                    },
                    view: self.view.clone(),
                });
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
    }
    pub(super) fn jump_to_match(&mut self) {
        if let Some(row) = self.match_index.and_then(|i| self.matches.select(i)) {
            self.selected = Some(row);
            self.scroll_row = self.view.rank_before(row).saturating_sub(3) as f64;
            self.reveal_column(self.search_column);
        }
    }
    pub fn next_match(&mut self, previous: bool) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let i = self.match_index.unwrap_or(0);
        self.match_index = Some(if previous {
            i.saturating_sub(1)
        } else {
            (i + 1).min(n - 1)
        });
        self.jump_to_match();
    }
    pub(super) fn reveal_column(&mut self, column: usize) {
        self.visible[column] = true;
        let left: f32 = (0..column)
            .filter(|&c| self.visible[c])
            .map(|c| self.widths[c])
            .sum();
        // Put the searched column first if it is to the left of the viewport or
        // far to the right. The grid clamps the final offset to its actual width.
        if left < self.scroll_x || left + self.widths[column] > self.scroll_x + 650.0 {
            self.scroll_x = left;
        }
    }
    pub(super) fn edit_filter(&mut self, column: usize) {
        self.filters_visible = true;
        self.reveal_column(column);
        self.focus_filter = Some(column);
    }
    pub fn clear_filters(&mut self) {
        self.filters.iter_mut().for_each(String::clear);
        self.changed_filter();
    }
    pub(super) fn go_to_row(&mut self) {
        if let Ok(n) = self.jump.replace(',', "").trim().parse::<u32>()
            && !self.view.is_empty()
        {
            let index = n.saturating_sub(1).min(self.view.len() - 1);
            self.scroll_row = index as f64;
            self.selected = self.view.select(index);
        }
    }
}

impl Table {
    pub(super) fn selection_shortcuts(&mut self, ui: &mut egui::Ui) {
        let grid_focused = ui.memory(|m| {
            m.has_focus(self.widget_id("table-body"))
                || (0..self.column_count()).any(|c| m.has_focus(self.widget_id(("header", c))))
        });
        if grid_focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.selection = Selection::Empty;
            self.copy_status.clear();
        }
        if grid_focused
            && ui.input_mut(|i| {
                let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                i.events.retain(|e| !matches!(e, egui::Event::Copy));
                i.consume_key(egui::Modifiers::COMMAND, Key::C) || copy
            })
        {
            self.start_copy(ui.ctx());
        }
    }
}
