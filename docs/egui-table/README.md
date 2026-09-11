# Atlas

A native, read-only data explorer built with **egui/eframe 0.36.1**. The demo stores **10,000,000 rows × 30 columns** in a seekable protobuf/zstd file and opens with the first six columns visible.

![Atlas viewer](docs/viewer.png)

[**Inside Atlas: interactive developer tutorial**](docs/developer/index.html) — a conceptual guide to the architecture and algorithms, with SVG diagrams. Open the HTML file directly in a browser; no server or build step is needed.

## Run

Requires Rust **1.95+**, a desktop display, and an OpenGL-capable driver. The lockfile is included.

```sh
cargo run --release
```

The first launch creates `data/atlas-10m.pb.zst` in a background thread, showing progress. Subsequent launches read its index and load the viewport. The generated file is about **262 MiB**; it is excluded from Git. Every cell is stored in the file—viewing does not regenerate values.

Explicit commands:

```sh
# Generate the full dataset without opening a window:
cargo run --release -- generate data/atlas-10m.pb.zst 10000000

# Open an existing Atlas dataset:
cargo run --release -- view data/atlas-10m.pb.zst

# Smaller fixture for development:
cargo run --release -- generate data/small.pb.zst 100000
cargo run --release -- view data/small.pb.zst

# Storage/query and headless rendering benchmarks:
cargo run --release -- bench data/atlas-10m.pb.zst
cargo run --release -- bench-ui data/atlas-10m.pb.zst
```

Generation refuses to overwrite files. It writes to a sibling `.partial` file and publishes the completed file atomically. A process killed during generation may leave that `.partial` file; remove that incomplete file before retrying. An existing invalid dataset produces an error instead of silently replacing data.

## Explore

- The demo's custom title bar includes a **theme picker** with Atlas Light/Dark, GitHub Light/Dark, Catppuccin Latte/Mocha, Monokai, and Dracula, alongside minimize, maximize/restore, and close controls. Drag anywhere in the compact header to move the window, including over buttons; a click still activates the button. Double-click the title area to maximize/restore, or drag an edge or corner to resize.
- On Linux, the demo rounds the window corners and updates native input/opaque regions to match. On Wayland, an input-transparent shadow subsurface sits outside the window geometry; maximized/fullscreen windows have square corners and no shadow. X11 shadows are supplied by the desktop compositor. This integration lives entirely in `src/chrome/` and the demo's dependencies; `egui-atlas-table` remains independent of native window decoration.
- **Columns** opens a searchable, grouped docked panel. It stays open while you work in the table. Check a box to show/hide a column; click its name to search it. Toggle any column, restore **Reset columns**, or **Show all**. At least one column remains visible. Drag header dividers to resize; scroll horizontally when needed.
- **Inline filters** sit directly below the column titles and are visible by default. Type in any column to narrow the view. Rules use case-insensitive, Unicode-aware fuzzy matching: `untd` matches “United Kingdom”; gaps between letters are allowed. Words and column rules combine with AND. This is subsequence matching, not edit-distance typo correction.
- Filters apply automatically after a 200 ms typing pause. A progress indicator replaces only the data rows; column titles and filter inputs remain available and keep keyboard focus. **Tab / Shift+Tab** move between column filters (revealing off-screen columns), **Enter** applies immediately, and **Esc** clears only the focused filter. Each input also has a clear button.
- **Filter row** in the toolbar, or **Ctrl/Cmd+Shift+L**, shows/hides the inputs without changing active filters. Active-filter chips stay visible: click the label to edit in its column header, or its × to remove the rule. Hidden-column chips are explicitly marked and reveal the column when edited. Use **Columns** to expose any of the 30 columns before adding a filter; hiding it afterward keeps the filter active. **Clear all** removes all rules.
- **Find in** shows the search column; click its name to open the column panel. Type to jump to the first fuzzy match in the current filtered view. Double-clicking a column title focuses search for that column; **Ctrl/Cmd+F** focuses the search field.
- **Down / Enter** advances to the next match; **Up / Shift+Enter** goes back. The buttons work too. Matches stay in source-row order. The first available result appears before the scan completes; a `+` indicates the count is still growing. Navigation stops at the currently known bounds rather than wrapping.
- **Esc** clears a focused search. With the table focused and search empty, **Up/Down**, **Page Up/Down**, and **Home/End** navigate rows.
- **Jump to** takes a one-based position in the current view. The gutter shows original record numbers, so filtered records keep their identity.
- Click a cell for its full value in the fixed readout below the table, with **Copy with headers**. The readout retains that explicitly labeled record/column until another cell is clicked. **Help & info** opens the docked guide; expand **Performance** for diagnostics. No hover bubbles or header overflow menus interrupt the table.

- **Select columns:** click a header; **Shift-click** selects the visible column range from the anchor; **Ctrl/Cmd-click** toggles individual columns. Whole-column selection includes all rows in the current filtered view, not just the viewport.
- **Select cells:** left-drag a rectangle in either direction; **Shift-click** extends it. Holding the pointer near an edge scrolls the selection vertically or horizontally. **Esc**, with the grid focused, clears selection.
- **Select whole rows:** with the grid focused, **Shift+Up/Down** extends or shrinks a row range from the active row, across all visible columns. It follows filtered row order, scrolls to keep the active row visible, and works while search is active. Reversing direction shrinks the range before extending past its anchor. Unmodified row navigation starts a fresh anchor.
- **Copy:** press **Ctrl/Cmd+C** with the grid or a selected header focused, or click **Copy with headers**. Paste the tab-separated text into an editor or spreadsheet. The first line contains column names; subsequent lines contain selected values in displayed row/column order. Tabs, newlines, and quotes inside values use quoted TSV fields. Hidden columns are excluded. Copy inside a focused search/filter input retains normal text-editing behavior.
- Copy captures a snapshot and reads missing blocks on a background worker, with progress and **Cancel**. The clipboard is updated only after completion. Text is limited to **128 MiB** and decoded blocks have a separate **128 MiB** budget; larger copies report an inline error and leave the clipboard unchanged, without truncation. Changing filters clears selection and cancels a pending copy. OS clipboard handoff still runs through the GUI integration.

All cells are read-only. Search reveals its selected column if it was hidden. Filters and search are independent: filters change the visible row set; search highlights and navigates within that set.

The named demo palettes adapt [GitHub Primer](https://github.com/primer/primitives), [Catppuccin](https://catppuccin.com/palette/), [VS Code Monokai](https://github.com/microsoft/vscode/blob/main/extensions/theme-monokai/themes/monokai-color-theme.json), and [Dracula](https://draculatheme.com/spec) colors to the table UI. Theme selection changes the header, panels, inputs, and grid together; the widget crate remains independent of these presets.

## How it handles 300 million cells

### Viewport rendering

The custom grid paints only rows and columns intersecting the viewport. There is no giant egui layout and no per-frame traversal of the dataset. Scrolling uses a **64-bit floating-point row position**; only local viewport offsets become `f32` coordinates. This preserves smooth single-row scrolling near row 10 million, where multiplying absolute row numbers by pixel height in `f32` would lose precision.

Filtered views use a compact bitset with a rank directory every 512 rows. Indexed rank/select maps visible positions to source rows without scanning earlier records. One 10-million-row mapping takes **1.27 MiB**; the unfiltered view needs no bitmap allocation. Individual lookups perform a binary search in the directory followed by at most eight 64-bit words.

An independent I/O worker loads visible cells. The render thread performs **no file reads, decompression, protobuf decoding, or fuzzy scans**. Cache misses display skeleton cells. The worker retains decoded blocks in a **64 MiB LRU**; the UI retains a bounded history of requested cell strings, so sparse views do not require entire decoded blocks per visible row on the render thread. Viewport requests are coalesced/cancelled and result queues are bounded.

### Background queries

Filtering uses a dedicated coordinator and a Rayon pool with up to six workers, leaving CPU headroom for rendering. It reads only predicate columns, matches dictionary entries once per block, and then tests integer codes. Predicates short-circuit empty candidate groups. Work is processed in bounded batches; memory does not scale with all decoded table cells.

Search scans the chosen column in source order, skips groups outside the filtered view, and reuses dictionary match results across identical dictionaries. It publishes the first matching group immediately and subsequent immutable snapshots at most every 150 ms. UI and worker share snapshots through `Arc`; large mappings are not copied by the UI.

Both operations use generation IDs and cancellation checks between blocks and every 256 dictionary entries. Typing cancels stale queries; outdated results cannot replace the current view. Bounded result channels prevent snapshot accumulation when the window is not rendering. Search waits for an active filter to finish before scanning its new view.

### File format

The schema is in [`proto/table.proto`](proto/table.proto); matching `prost` derives live in `src/storage.rs`, so building does not require `protoc`.

```text
ATLASPB1
  zstd(ColumnBlock)  row group 0, column 0
  zstd(ColumnBlock)  row group 0, column 1
  ...
  zstd(ColumnBlock)  row group N, column 29
  zstd(Index)
  index compressed size: little-endian u64
ATLASEND
```

A row group holds up to **16,384 rows**. Each column block contains a protobuf string dictionary and packed integer codes. The footer indexes every compressed frame. Frames use zstd checksums; loading validates version, dimensions, block bounds, decoded sizes, and dictionary codes.

This is a **seekable framed protobuf/zstd container**, not one monolithic protobuf message compressed as a single zstd stream. Independent frames enable random access without inflating the entire dataset. It opens files in this schema; it is not a generic viewer for arbitrary `.proto` files.

The deterministic synthetic data includes identifiers, names, organizations, locations, financial values, dates, and categorical fields. It intentionally includes repeated values and a unique ID column; compressibility and query timings on real high-cardinality data will differ.

## Measurements

Measured locally on an Intel Core Ultra 7 265K in release mode on the full 10-million-row file, with warm OS file cache and six filter workers:

| Operation | Measured time |
|---|---:|
| 18 random column-block reads + decompression | 3.94 ms |
| Country `untd` AND Status `actv` over 10M rows | 23.15 ms |
| Search Name `amorg` within the 834,205-row filtered view: first result | 0.85 ms |
| Same search: complete, 18,756 matches | 76.06 ms |
| 100,000 indexed lookups in the filtered view | 5.80 ms |
| Unique-ID fuzzy filter across all 10M distinct IDs | 122.10 ms |
| Warm frame + tessellation, top / middle / bottom | ~0.09–0.11 ms |

The UI benchmark uses a 1440×900 headless egui context and paints roughly 80 cells with six columns visible and the inline filter row enabled. It includes tessellation but **excludes GPU submission, presentation, and OS input latency**. Run the included benchmarks on your target hardware; these are reproducible examples, not universal performance guarantees.

## Validation

```sh
cargo test --workspace --locked
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo fmt --all --check
```

Tests cover docked-panel persistence, viewport resizing, full cell-value inspection, inline filter focus during loading, stable input geometry, keyboard traversal and clearing, hidden-column filter access, file round trips, checksum corruption, and truncation, final partial row groups, invalid accesses, dense/sparse/empty rank/select, Unicode fuzzy matching, combined filters, cancellation, search constrained to a filtered view, row/column viewport culling, last-row access, query generations, and first/second/third/previous match navigation. Native interaction checks were also performed on an isolated X11 display.

## Scope

This is a focused viewer demo, not a claim of QTableView feature parity. It does not provide editing, sorting, persistent layouts, or arbitrary protobuf schema discovery. UI rendering scales with the viewport; a cold filter/search remains a scan of the relevant compressed column data. Cold random jumps can briefly display loading cells while storage catches up.

## Filtering interaction design

![Inline column filters](docs/inline-filters.png)

The default uses a persistent row of inputs aligned with the data columns, following the quick-access patterns documented by [AG Grid floating filters](https://www.ag-grid.com/javascript-data-grid/floating-filters/) and [MUI header filters](https://mui.com/x/react-data-grid/filtering/header-filters/) (reviewed September 2026). This makes fuzzy filtering a direct action and lets several column conditions stay visible together. The summary strip keeps a constant height so adding a condition does not move the input being edited.

Categorical value pickers and numeric/date ranges could complement these inputs if typed operators are added later. The current dataset viewer retains its consistent fuzzy-text semantics for every column.

## Popup-free interaction design

![Docked column controls](docs/columns-panel.png)

Reviewed September 2026: [NN/g tooltip guidance](https://www.nngroup.com/articles/tooltip-guidelines/) discourages redundant tooltip text, hiding essential instructions, and obscuring related content. [Carbon data-table guidance](https://carbondesignsystem.com/components/data-table/usage/) places table-wide actions in the toolbar. These are established principles, not a claim that a particular pattern was invented in 2026.

Atlas applies these principles with direct header filters and search, a deliberately opened side panel for column configuration and help, and a fixed cell-value readout. The panel occupies its own space, has an explicit Close button, and remains open across table interactions and checkbox changes. Search-column selection shares the searchable column panel instead of a second dropdown. Help and diagnostics are available on demand without appearing under the pointer. Rendering remains limited to the reduced grid viewport while the panel is open.

## Selection and clipboard

![Rectangular cell selection](docs/selection.png)

Selection stores row-range or rectangle endpoints, or a dynamic column mask, independent of dataset size. Only visible cells are checked and painted. Copy snapshots the immutable filtered row map and loads one row group of the selected columns at a time. It does not depend on the viewport cache, so offscreen and previously unloaded cells are included. Only one copy job is active; cancellation is checked between rows and block reads.

Tests cover reversed rectangles, Shift/Ctrl column selection, hidden-column exclusion, native-style pointer events, clipboard commands and text-input focus, filtered TSV order across three row groups, escaping, size limits, and cancellation. Native X11 verification also checks the actual clipboard text for a dragged rectangle.

## Reuse in another egui app

The table is now the standalone [egui-atlas-table crate](crates/egui-atlas-table/README.md). The root binary only handles startup, the demo file adapter/generator, branding, and custom cell formatting. The library has no protobuf, zstd, filesystem, or eframe dependency.

```toml
[dependencies]
egui-atlas-table = { path = "/path/to/egui-table/crates/egui-atlas-table" }
```

Implement `DataSource`, construct `Table::new` once with a unique ID, and call `table.show(ui)` each frame. `show_grid(ui, rect)` supports application-owned controls. Schema sizes, initial visible columns, column groups/widths/formatting, colors, query threads, and debounce intervals are configurable. See the crate README for the source contract and a minimal embedding example.

Run `cargo run --release --example reuse` for two independent tables backed by ordinary Rust vectors. This example uses neither the demo file nor its generator.

[Refactor performance comparison and raw measurements](docs/refactor-performance.md).
