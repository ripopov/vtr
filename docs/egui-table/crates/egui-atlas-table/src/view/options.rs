use super::*;
/// Optional application-specific cell painting. Called only for visible,
/// loaded cells; the painter is clipped to the cell. Must not block or do I/O.
pub trait CellPainter {
    fn paint(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        source_row: u32,
        column: usize,
        value: &str,
        selected: bool,
    );
}
#[derive(Clone, Default)]
pub struct CellStyle {
    pub right_aligned: bool,
    pub monospace: bool,
    pub muted: bool,
}
#[derive(Clone)]
pub struct ColumnOptions {
    pub width: f32,
    pub group: String,
    pub style: CellStyle,
}
impl Default for ColumnOptions {
    fn default() -> Self {
        Self {
            width: 180.0,
            group: String::new(),
            style: CellStyle::default(),
        }
    }
}
pub struct TableOptions {
    pub title: String,
    pub description: String,
    pub initial_visible: usize,
    pub initial_search_column: usize,
    pub columns: Vec<ColumnOptions>,
    /// Size of the query CPU pool for this instance (default: up to six).
    pub query_threads: usize,
    pub filter_debounce: Duration,
    pub search_debounce: Duration,
    pub cell_painter: Option<Arc<dyn CellPainter>>,
    /// None follows the embedding Ui visuals, including runtime theme changes.
    pub colors: Option<TableColors>,
}
impl Default for TableOptions {
    fn default() -> Self {
        Self {
            title: "Table".into(),
            description: String::new(),
            initial_visible: 6,
            initial_search_column: 0,
            columns: Vec::new(),
            query_threads: crate::source::worker_count(),
            filter_debounce: Duration::from_millis(200),
            search_debounce: Duration::from_millis(120),
            cell_painter: None,
            colors: None,
        }
    }
}
#[derive(Debug)]
pub struct TableResponse {
    pub selected_source_row: Option<u32>,
    pub visible_rows: u32,
    pub painted_cells: usize,
    pub busy: bool,
    pub error: Option<String>,
}

/// Colors for the grid's painted surfaces. Input/button styles come from egui.
#[derive(Clone, Copy, Debug)]
pub struct TableColors {
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub line: Color32,
    pub surface: Color32,
    pub header: Color32,
    pub row_alt: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub match_fill: Color32,
}
impl TableColors {
    pub fn light() -> Self {
        Self {
            ink: Color32::from_rgb(30, 43, 62),
            muted: Color32::from_rgb(112, 124, 141),
            accent: Color32::from_rgb(48, 99, 222),
            line: Color32::from_rgb(229, 234, 241),
            surface: Color32::WHITE,
            header: Color32::from_rgb(244, 247, 251),
            row_alt: Color32::from_rgb(251, 252, 254),
            hover: Color32::from_rgb(246, 249, 254),
            selected: Color32::from_rgb(215, 229, 255),
            match_fill: Color32::from_rgb(255, 244, 207),
        }
    }
    pub fn from_visuals(v: &egui::Visuals) -> Self {
        Self {
            ink: v.text_color(),
            muted: v.weak_text_color(),
            accent: v.selection.stroke.color,
            line: v.widgets.noninteractive.bg_stroke.color,
            surface: v.extreme_bg_color,
            header: v.faint_bg_color,
            row_alt: v.faint_bg_color,
            hover: v.widgets.hovered.weak_bg_fill,
            selected: v.selection.bg_fill,
            match_fill: if v.dark_mode {
                Color32::from_rgb(75, 61, 22)
            } else {
                Color32::from_rgb(255, 244, 207)
            },
        }
    }
}
