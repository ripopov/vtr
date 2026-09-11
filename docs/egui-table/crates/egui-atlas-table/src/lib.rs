#![doc = include_str!("../README.md")]
//! Read-only, virtualized tables for embedding in any egui application.
//!
//! Implement [`DataSource`] for immutable data, construct a [`Table`], then call
//! [`Table::show`] each frame. Storage, decompression, queries, and clipboard
//! preparation run off the UI thread. Multiple instances need distinct IDs.
mod clipboard;
mod pages;
pub mod query;
pub mod rowset;
pub mod selection;
pub mod source;
mod view;
pub use egui;
pub use source::{ColumnBlock, DataSource, Schema, Source};
pub use view::{
    CellPainter, CellStyle, ColumnOptions, Table, TableColors, TableOptions, TableResponse,
};
#[cfg(test)]
mod test_support;
