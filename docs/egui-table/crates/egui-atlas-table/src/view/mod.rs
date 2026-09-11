use crate::clipboard::CopyJob;
use crate::pages::PageCache;
use crate::selection::Selection;
use crate::{
    query::{self, Predicate, Request},
    rowset::RowSet,
    source::Source,
};
use egui::{Align2, Color32, FontId, Id, Key, Rect, RichText, Sense, Stroke, pos2, vec2};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

const ROW: f64 = 34.0;
const GUTTER: f32 = 86.0;
pub use options::{
    CellPainter, CellStyle, ColumnOptions, TableColors, TableOptions, TableResponse,
};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dock {
    Columns,
    Help,
}

pub struct Table {
    id: Id,
    colors: TableColors,
    options: TableOptions,
    groups: Vec<(String, Vec<usize>)>,
    source: Arc<Source>,
    cache: PageCache,
    worker: query::Worker,
    view: RowSet,
    visible: Vec<bool>,
    widths: Vec<f32>,
    column_query: String,
    dock: Option<Dock>,
    cell_value: Option<(u32, usize, String)>,
    selection: Selection,
    copy_job: Option<CopyJob>,
    copy_status: String,
    copy_rows: u32,
    filters_visible: bool,
    filters: Vec<String>,
    focus_filter: Option<usize>,
    filter_pending: Option<Instant>,
    filtering: bool,
    filter_progress: f32,
    filter_elapsed: Duration,
    filter_id: u64,
    search_column: usize,
    search_text: String,
    search_pending: Option<Instant>,
    search_id: u64,
    searching: bool,
    search_progress: f32,
    search_elapsed: Duration,
    matches: RowSet,
    match_index: Option<u32>,
    focus_search: bool,
    scroll_row: f64,
    scroll_x: f32,
    selected: Option<u32>,
    jump: String,
    error: Option<String>,
    frame_ms: f64,
    painted_cells: usize,
}
impl Table {
    /// Creates a table without reading any data blocks. The source must remain
    /// immutable; create a new Table when its schema or data revision changes.
    pub fn new(
        id: impl std::hash::Hash + std::fmt::Debug,
        source: Arc<dyn crate::DataSource>,
        ctx: egui::Context,
        mut options: TableOptions,
    ) -> anyhow::Result<Self> {
        let store = Arc::new(Source::new(source)?);
        let count = store.schema.columns.len();
        options.columns.resize_with(count, ColumnOptions::default);
        options.columns.truncate(count);
        options.initial_visible = options.initial_visible.clamp(1, count);
        options.initial_search_column = options.initial_search_column.min(count - 1);
        let mut visible = vec![false; count];
        visible[..options.initial_visible].fill(true);
        let widths = options
            .columns
            .iter()
            .map(|c| c.width.clamp(100.0, 600.0))
            .collect();
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (c, column) in options.columns.iter().enumerate() {
            if let Some((_, columns)) = groups.iter_mut().find(|(name, _)| *name == column.group) {
                columns.push(c);
            } else {
                groups.push((column.group.clone(), vec![c]));
            }
        }
        let search_column = options.initial_search_column;
        let worker = query::Worker::new(store.clone(), ctx.clone(), options.query_threads)?;
        let colors = options
            .colors
            .unwrap_or_else(|| TableColors::from_visuals(&ctx.style_of(ctx.theme()).visuals));
        Ok(Self {
            id: Id::new(id),
            colors,
            options,
            groups,
            view: RowSet::All(store.schema.rows),
            cache: PageCache::new(store.clone(), ctx.clone())?,
            worker,
            source: store,

            visible,
            widths,
            column_query: String::new(),
            dock: None,
            cell_value: None,
            selection: Selection::Empty,
            copy_job: None,
            copy_status: String::new(),
            copy_rows: 0,
            filters_visible: true,
            filters: vec![String::new(); count],
            focus_filter: None,
            filter_pending: None,
            filtering: false,
            filter_progress: 0.0,
            filter_elapsed: Duration::ZERO,
            filter_id: 0,
            search_column,
            search_text: String::new(),
            search_pending: None,
            search_id: 0,
            searching: false,
            search_progress: 0.0,
            search_elapsed: Duration::ZERO,
            matches: RowSet::All(0),
            match_index: None,
            focus_search: false,
            scroll_row: 0.0,
            scroll_x: 0.0,
            selected: None,
            jump: String::new(),
            error: None,
            frame_ms: 0.0,
            painted_cells: 0,
        })
    }
    fn name(&self, column: usize) -> &str {
        &self.source.schema.columns[column]
    }
    fn column_count(&self) -> usize {
        self.visible.len()
    }
    fn widget_id(&self, salt: impl std::hash::Hash + std::fmt::Debug) -> Id {
        self.id.with(salt)
    }
}
impl Table {
    /// Embeds the table in the available Ui rectangle. No window or panel is
    /// created and no Context-wide style is changed. Call once per frame.
    pub fn show(&mut self, root: &mut egui::Ui) -> TableResponse {
        self.colors = self
            .options
            .colors
            .unwrap_or_else(|| TableColors::from_visuals(root.visuals()));
        let start = Instant::now();
        let ctx = root.ctx().clone();
        self.poll(&ctx);
        let owns_focus = ctx.memory(|m| {
            m.focused().is_some_and(|id| {
                id == self.widget_id("table-body")
                    || id == self.widget_id("find-text")
                    || (0..self.column_count()).any(|c| {
                        id == self.widget_id(("header", c))
                            || id == self.widget_id(("column-filter", c))
                    })
            })
        });
        if owns_focus
            && ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L)
            })
        {
            self.filters_visible = !self.filters_visible;
        }
        root.push_id(self.id, |ui| {
            self.header(ui);
            let height = (ui.available_height() - 84.0).max(120.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
            if let Some(error) = &self.error {
                ui.painter().rect_filled(rect, 8.0, self.colors.surface);
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(30.0)), |ui| {
                    ui.heading("Unable to load this view"); ui.label(error);
                    ui.label("The data source reported an error. Check the source implementation or reload the table.");
                });
            } else {
                let mut grid = rect;
                if self.dock.is_some() {
                    let panel = Rect::from_min_max(pos2(rect.max.x - 300.0, rect.min.y), rect.max);
                    grid.max.x = panel.min.x - 14.0;
                    ui.painter().rect_filled(panel, 8.0, self.colors.surface);
                    ui.scope_builder(egui::UiBuilder::new().max_rect(panel.shrink(12.0)), |ui| {
                        ui.set_clip_rect(panel.shrink(8.0));
                        self.dock_panel(ui);
                    });
                }
                self.table(ui, grid);
            }
            self.selection_shortcuts(ui);
            let readout = Rect::from_min_max(pos2(rect.min.x, rect.max.y + 5.0), pos2(rect.max.x, rect.max.y + 37.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(readout), |ui| self.selection_bar(ui));
            let footer = Rect::from_min_max(pos2(rect.min.x, rect.max.y + 44.0), pos2(rect.max.x, rect.max.y + 80.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(footer), |ui| self.footer(ui));
        });
        self.frame_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.response()
    }
}
fn number(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn arrow_button(ui: &mut egui::Ui, enabled: bool, up: bool) -> egui::Response {
    let response = ui.add_enabled(enabled, egui::Button::new("").min_size(vec2(32.0, 30.0)));
    let center = response.rect.center();
    let direction = if up { -1.0 } else { 1.0 };
    let tip = center + vec2(0.0, 5.0 * direction);
    let color = if enabled {
        ui.visuals().text_color()
    } else {
        Color32::from_rgb(189, 199, 212)
    };
    let p = ui.painter();
    p.line_segment(
        [center - vec2(0.0, 5.0 * direction), tip],
        Stroke::new(1.5, color),
    );
    p.line_segment(
        [tip - vec2(4.0, 4.0 * direction), tip],
        Stroke::new(1.5, color),
    );
    p.line_segment(
        [tip + vec2(4.0, -4.0 * direction), tip],
        Stroke::new(1.5, color),
    );
    response
}

impl Table {
    /// Draw only the grid and its column headers into an already allocated
    /// rectangle. Use this instead of `show` when supplying your own toolbar.
    /// Selection, inline filters, scrolling, and Ctrl/Cmd+C still work.
    pub fn show_grid(&mut self, ui: &mut egui::Ui, rect: Rect) -> TableResponse {
        let start = Instant::now();
        self.colors = self
            .options
            .colors
            .unwrap_or_else(|| TableColors::from_visuals(ui.visuals()));
        self.poll(ui.ctx());
        ui.push_id(self.id, |ui| {
            if let Some(error) = &self.error {
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    error,
                    FontId::proportional(14.0),
                    self.colors.ink,
                );
            } else {
                self.table(ui, rect);
            }
            self.selection_shortcuts(ui);
        });
        self.frame_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.response()
    }
    fn response(&self) -> TableResponse {
        TableResponse {
            selected_source_row: self.selected,
            visible_rows: self.view.len(),
            painted_cells: self.painted_cells,
            busy: self.filtering || self.searching || self.copy_job.is_some(),
            error: self.error.clone(),
        }
    }
    /// Replace selection using positions in the current filtered view.
    pub fn set_selection(&mut self, selection: Selection) -> anyhow::Result<()> {
        let valid = match &selection {
            Selection::Empty => true,
            Selection::Columns { columns, anchor } => {
                columns.len() == self.column_count() && *anchor < self.column_count()
            }
            Selection::Rows { anchor, end } => *anchor < self.view.len() && *end < self.view.len(),
            Selection::Cells { anchor, end } => {
                anchor.0 < self.view.len()
                    && end.0 < self.view.len()
                    && anchor.1 < self.column_count()
                    && end.1 < self.column_count()
            }
        };
        anyhow::ensure!(valid, "Selection out of range");
        self.selection = selection;
        self.copy_status.clear();
        Ok(())
    }
    pub fn copy_selection(&mut self, ctx: &egui::Context) {
        self.start_copy(ctx);
    }
    pub fn cancel_copy(&mut self) {
        self.copy_job = None;
        self.copy_status = "Copy cancelled".into();
    }
    pub fn set_filters_visible(&mut self, visible: bool) {
        self.filters_visible = visible;
    }
    pub fn is_filtering(&self) -> bool {
        self.filtering
    }
    pub fn is_searching(&self) -> bool {
        self.searching
    }
    pub fn match_count(&self) -> u32 {
        self.matches.len()
    }
    /// Zero-based match index, when a match is active.
    pub fn current_match(&self) -> Option<u32> {
        self.match_index
    }
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
    pub fn row_view(&self) -> &RowSet {
        &self.view
    }
    pub fn column_visible(&self, column: usize) -> Option<bool> {
        self.visible.get(column).copied()
    }
    pub fn set_column_visible(&mut self, column: usize, visible: bool) -> anyhow::Result<()> {
        anyhow::ensure!(column < self.column_count(), "Column out of range");
        anyhow::ensure!(
            visible || !self.visible[column] || self.visible.iter().filter(|&&v| v).count() > 1,
            "At least one column must remain visible"
        );
        self.visible[column] = visible;
        Ok(())
    }
    pub fn set_filter(&mut self, column: usize, text: impl Into<String>) -> anyhow::Result<()> {
        anyhow::ensure!(column < self.column_count(), "Column out of range");
        self.filters[column] = text.into();
        self.changed_filter();
        Ok(())
    }
    pub fn set_search(&mut self, column: usize, text: impl Into<String>) -> anyhow::Result<()> {
        anyhow::ensure!(column < self.column_count(), "Column out of range");
        self.search_column = column;
        self.search_text = text.into();
        self.reveal_column(column);
        self.changed_search();
        Ok(())
    }
    pub fn scroll_to_row(&mut self, view_row: u32) {
        self.scroll_row = view_row.min(self.view.len().saturating_sub(1)) as f64;
    }
    pub fn viewport_ready(&self) -> bool {
        self.cache.is_ready()
    }
}

mod controller;
mod controls;
mod grid;
mod headers;
mod options;
#[cfg(test)]
mod tests;
