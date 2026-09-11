# egui-atlas-table

[**Visual developer tutorial**](../../docs/developer/index.html): an offline HTML guide to the architecture, algorithms, and integration boundaries.

A reusable, read-only table for egui 0.36. It renders only visible cells, supports asynchronous per-column fuzzy filtering and search, and copies rectangular, row, or column selections with headers. This is a local workspace crate; it has not been published to crates.io.

The library depends on egui, anyhow, rayon, and nucleo-matcher. It does **not** depend on eframe, a windowing backend, protobuf, zstd, or the Atlas dataset.

## Embed a table

Implement `DataSource` over your immutable data. Metadata is obtained once at construction; block reads occur on background workers. Each block represents one column in a consecutive group of rows. Repeated values can share dictionary entries; unique values can use one entry per row.

```rust,no_run
use egui_atlas_table::{ColumnBlock, DataSource, Schema, Table, TableOptions, egui};
use std::sync::Arc;

struct People;
impl DataSource for People {
    fn schema(&self) -> Schema {
        Schema { rows: 2, group_rows: 256, columns: vec!["Name".into()] }
    }
    fn read_block(&self, _group: u32, _column: usize) -> anyhow::Result<ColumnBlock> {
        ColumnBlock::new(vec!["Alice".into(), "Bob".into()], vec![0, 1])
    }
}

# fn example(ctx: egui::Context, ui: &mut egui::Ui) -> anyhow::Result<()> {
// Construct once and retain Table in your application's state.
let mut table = Table::new("people", Arc::new(People), ctx, TableOptions::default())?;
// Call each frame, inside your application's panel/layout:
let response = table.show(ui);
if let Some(error) = response.error {
    eprintln!("{error}");
}
# Ok(()) }
```

Use distinct IDs for multiple tables. The host owns its windows, layout, and global egui style. Grid colors follow the host's visuals unless `TableOptions::colors` overrides them. `CellPainter` supports application-specific badges or formatting and receives a clipped painter, the full cell rectangle, source row, column, value, and selection state. It is called only for visible loaded cells and must not block.

For a custom toolbar, allocate a rectangle and call `Table::show_grid` instead of `show`. Drive the controller with `set_filter`, `clear_filters`, `set_search`, `next_match`, `scroll_to_row`, `set_column_visible`, `set_selection`, `copy_selection`, and `cancel_copy`. `row_view`, `selection`, `current_match`, and `match_count` expose state without coupling your app to the built-in controls. Call exactly one of `show` or `show_grid` per instance per frame.

## Components and contracts

| Component | Responsibility |
|---|---|
| `DataSource`, `Schema`, `ColumnBlock` | Immutable data adapter; validated dictionary blocks |
| `Source` | Validated schema and block dimensions; dispatch per block |
| `query` | Fuzzy filter/search scans, bounded worker pool, cancellation, result snapshots |
| `rowset` | Compact immutable row mapping with indexed rank/select |
| `selection` | Column, row, and rectangular selection independent of rendering |
| Internal viewport cache | Background loading, bounded decoded-block cache, viewport coalescing |
| Internal clipboard worker | Selected-block reads, TSV encoding, progress and cancellation |
| `Table` | Retained controller and view state, isolated IDs, full or grid-only UI |

The data source must be thread-safe: cache, query, and copy workers can request blocks concurrently. A source's schema and values must remain stable for the table's lifetime. Create a new table for a different dataset or revision. The library validates dictionary codes once on construction and checks requested block dimensions; source errors are surfaced to the host. Empty row sets and arbitrary positive column counts are supported. Row indices are `u32`; groups contain at most 65,536 rows.

Filtering changes the visible row map. Selection ranges use **positions in that map**, while `TableResponse::selected_source_row` and cell-painter callbacks identify original source rows. Filtering clears selection and cancels pending copy. Hiding columns preserves their filters; copying excludes hidden columns.

Default query workers: available CPUs minus two, capped at six. Set `query_threads` per instance to control CPU use in apps with several tables. Filter/search debounce intervals are configurable. Each table has a 64 MiB decoded cache and an approximately 8,192-cell UI history. Clipboard output is limited to 128 MiB, with a separate 128 MiB decoded-block budget. Oversized copies fail without changing the clipboard. The final OS clipboard handoff belongs to the host integration and can still incur UI-thread work.

Rendering never scans the dataset. It checks visible rows/columns, uses an `f64` scroll position, and loads missing blocks asynchronously. Source dispatch is per block, not per cell. ASCII literal fuzzy predicates use a boolean subsequence fast path; Unicode, whitespace-separated atoms, and operator syntax use nucleo. Query cancellation is checked between blocks and periodically within dictionaries. Clipboard cancellation is checked between rows and reads. A blocking backend read itself cannot be interrupted.

## Examples and validation

From the workspace root:

```sh
cargo run --release --example reuse
cargo test --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
```

The `reuse` example hosts two independent tables backed by ordinary Rust vectors. The Atlas binary demonstrates a protobuf/zstd adapter and custom cell painting. Public-API integration tests exercise grid-only embedding, background-only reads, and clipboard output. Additional tests cover isolation, 97 columns, empty data, filters, search, selection, cancellation, and corrupt storage.
