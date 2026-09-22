//! Reduced immutable table panel over one transaction generator or a fixed
//! set of signal histories. The source remains the row store: only a bounded
//! visible window (and, for multiple signals, one admitted timestamp axis) is
//! owned by the panel.

pub mod columns;
pub mod layout;
pub mod model;
pub mod paint;
pub mod source;

pub use layout::{ColumnRect, RowViewport, TableLayout};
pub use model::{
    AccessibleRow, PreparedCell, PreparedRow, PreparedWindow, RowIdentity, TableCommand,
    TableModel, TableState,
};
pub use source::{SignalSource, TableSource};
