use std::{
    thread,
    time::{Duration, Instant},
};
fn headless_frame(app: &mut Table, ctx: &egui::Context) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1440.0, 900.0))),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                app.show(ui);
            });
        },
    );
    // This benchmark has no GPU backend; explicitly acknowledge texture updates.
    output.textures_delta.clear();
    output
}
fn wait_ready(app: &mut Table, ctx: &egui::Context) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        headless_frame(app, ctx);
        if let Some(e) = &app.error {
            anyhow::bail!("{e}");
        }
        if app.viewport_ready() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Viewport did not load before deadline"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
use super::*;
fn fixture() -> anyhow::Result<(Table, egui::Context)> {
    let ctx = egui::Context::default();
    let options = TableOptions {
        initial_search_column: 1,
        ..Default::default()
    };
    let mut app = Table::new(
        "test-table",
        Arc::new(crate::test_support::MemoryData::new(33_000, 30)),
        ctx.clone(),
        options,
    )?;
    wait_ready(&mut app, &ctx)?;
    Ok((app, ctx))
}
fn input_frame(
    app: &mut Table,
    ctx: &egui::Context,
    mut events: Vec<egui::Event>,
) -> egui::FullOutput {
    let modifiers = events
        .iter()
        .rev()
        .find_map(|e| match e {
            egui::Event::PointerButton { modifiers, .. } | egui::Event::Key { modifiers, .. } => {
                Some(*modifiers)
            }
            _ => None,
        })
        .unwrap_or_default();
    events.insert(0, egui::Event::ModifiersChanged(modifiers));
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1440.0, 900.0))),
            events,
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                app.show(ui);
            });
        },
    );
    out.textures_delta.clear();
    out
}
fn key_event(key: Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
fn pointer(
    app: &mut Table,
    ctx: &egui::Context,
    p: egui::Pos2,
    pressed: bool,
    modifiers: egui::Modifiers,
) {
    input_frame(
        app,
        ctx,
        vec![
            egui::Event::PointerMoved(p),
            egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            },
        ],
    );
}
#[test]
fn resizing_keeps_header_separators_unclipped() -> anyhow::Result<()> {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let (mut app, ctx) = fixture()?;
            ctx.set_theme(theme);
            ctx.set_pixels_per_point(scale);
            app.selection.column(1, false, false, &app.visible);
            // Let zoom, layout, and the previous-frame hit regions settle.
            for _ in 0..4 {
                headless_frame(&mut app, &ctx);
            }
            let start = ctx
                .read_response(app.widget_id(("resize", 0usize)))
                .unwrap()
                .rect
                .center();
            let original_width = app.widths[0];
            input_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(start)]);
            pointer(&mut app, &ctx, start, true, egui::Modifiers::NONE);
            for step in 1..=24 {
                let p = start + vec2(step as f32 * 0.75, 0.0);
                let out = input_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
                let handle = ctx
                    .read_response(app.widget_id(("resize", 0usize)))
                    .unwrap()
                    .rect;
                let separator = out
                    .shapes
                    .iter()
                    .find(|shape| {
                        matches!(&shape.shape, egui::epaint::Shape::LineSegment { points, .. }
                        if (points[0].x - handle.center().x).abs() < 1.0
                            && points[0].x == points[1].x
                            && (points[0].y - (handle.top() + 11.0)).abs() < 0.1
                            && (points[1].y - (handle.bottom() - 11.0)).abs() < 0.1)
                    })
                    .expect("resizing must paint the separator every frame");
                let egui::epaint::Shape::LineSegment { points, stroke } = &separator.shape else {
                    unreachable!()
                };
                let stroke_bounds = Rect::from_two_pos(points[0], points[1])
                    .expand(stroke.width / 2.0 + 0.5 / scale);
                assert!(
                    separator.clip_rect.contains_rect(stroke_bounds),
                    "separator stroke must remain inside the clip at scale {scale}, step {step}"
                );
            }
            pointer(
                &mut app,
                &ctx,
                start + vec2(18.0, 0.0),
                false,
                egui::Modifiers::NONE,
            );
            assert!(
                app.widths[0] > original_width,
                "gesture must resize the column at scale {scale}: {original_width} -> {}",
                app.widths[0]
            );
        }
    }
    Ok(())
}

#[test]
fn shift_arrows_select_whole_filtered_rows() -> anyhow::Result<()> {
    let (mut app, ctx) = fixture()?;
    let mut rows = crate::rowset::Builder::new(33_000);
    for row in [1, 5, 20, 50] {
        rows.insert(row);
    }
    app.view = rows.build();
    app.visible[2] = false;
    app.selected = Some(5);
    app.search_text = "active search".into();
    app.matches = app.view.clone();
    app.match_index = Some(1);
    ctx.memory_mut(|m| m.request_focus(app.widget_id("table-body")));
    for (key, expected) in [
        (Key::ArrowDown, (1, 2)),
        (Key::ArrowUp, (1, 1)),
        (Key::ArrowUp, (0, 1)),
        (Key::ArrowUp, (0, 1)),
        (Key::ArrowDown, (1, 1)),
    ] {
        input_frame(&mut app, &ctx, vec![key_event(key, egui::Modifiers::SHIFT)]);
        let range = app
            .selection
            .snapshot(app.view.len(), &app.visible)
            .unwrap();
        assert_eq!((range.first, range.last), expected);
        assert_eq!(range.columns, [0, 1, 3, 4, 5]);
        assert_eq!(
            app.match_index,
            Some(1),
            "Shift arrows must not navigate search matches"
        );
    }
    input_frame(&mut app, &ctx, vec![egui::Event::Copy]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let out = input_frame(&mut app, &ctx, vec![]);
        if let Some(text) = out
            .platform_output
            .commands
            .into_iter()
            .find_map(|cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
        {
            assert!(text.starts_with("Record ID\tName\tCountry\tStatus\tRevenue\nAT-00000006\t"));
            assert_eq!(text.lines().count(), 2);
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "copy did not finish");
        thread::sleep(Duration::from_millis(1));
    }
    app.search_text.clear();
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::ArrowDown, egui::Modifiers::NONE)],
    );
    assert!(matches!(app.selection, Selection::Empty));
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::ArrowDown, egui::Modifiers::SHIFT)],
    );
    let range = app
        .selection
        .snapshot(app.view.len(), &app.visible)
        .unwrap();
    assert_eq!(
        (range.first, range.last),
        (2, 3),
        "unmodified navigation starts a new anchor"
    );
    app.view = RowSet::All(33_000);
    app.selected = Some(32_998);
    for _ in 0..2 {
        input_frame(
            &mut app,
            &ctx,
            vec![key_event(Key::ArrowDown, egui::Modifiers::SHIFT)],
        );
    }
    let range = app
        .selection
        .snapshot(app.view.len(), &app.visible)
        .unwrap();
    assert_eq!((range.first, range.last), (32_998, 32_999));
    assert!(app.scroll_row > 32_980.0);
    app.edit_filter(3);
    headless_frame(&mut app, &ctx);
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::ArrowUp, egui::Modifiers::SHIFT)],
    );
    let after = app
        .selection
        .snapshot(app.view.len(), &app.visible)
        .unwrap();
    assert_eq!((after.first, after.last), (range.first, range.last));
    Ok(())
}
#[test]
fn mouse_selection_and_clipboard_focus() -> anyhow::Result<()> {
    let (mut app, ctx) = fixture()?;
    for (column, mods) in [
        (1, egui::Modifiers::NONE),
        (3, egui::Modifiers::SHIFT),
        (2, egui::Modifiers::CTRL),
    ] {
        let pos = ctx
            .read_response(app.widget_id(("header", column)))
            .unwrap()
            .rect
            .center();
        pointer(&mut app, &ctx, pos, true, mods);
        pointer(&mut app, &ctx, pos, false, mods);
    }
    assert_eq!(
        app.selection
            .snapshot(app.view.len(), &app.visible)
            .unwrap()
            .columns,
        [1, 3]
    );
    assert!(!ctx.memory(|m| m.has_focus(app.widget_id("find-text"))));
    let body = ctx.read_response(app.widget_id("table-body")).unwrap().rect;
    let column_x = |c: usize| {
        ctx.read_response(app.widget_id(("header", c)))
            .unwrap()
            .rect
            .center()
            .x
    };
    let start = pos2(column_x(1), body.min.y + 16.0);
    let end = pos2(column_x(3), body.min.y + 2.0 * ROW as f32 + 16.0);
    pointer(&mut app, &ctx, start, true, egui::Modifiers::NONE);
    input_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(end)]);
    pointer(&mut app, &ctx, end, false, egui::Modifiers::NONE);
    let range = app
        .selection
        .snapshot(app.view.len(), &app.visible)
        .unwrap();
    assert_eq!(
        (range.first, range.last, range.columns),
        (0, 2, vec![1, 2, 3])
    );
    input_frame(&mut app, &ctx, vec![egui::Event::Copy]);
    let deadline = Instant::now() + Duration::from_secs(5);
    let copied = loop {
        let out = input_frame(&mut app, &ctx, vec![]);
        if let Some(text) = out
            .platform_output
            .commands
            .into_iter()
            .find_map(|cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
        {
            break text;
        }
        anyhow::ensure!(Instant::now() < deadline, "copy never completed");
        thread::sleep(Duration::from_millis(1));
    };
    assert!(copied.starts_with("Name\tCompany\tCountry\n"));
    assert_eq!(copied.lines().count(), 4);
    app.visible.fill(true);
    headless_frame(&mut app, &ctx);
    headless_frame(&mut app, &ctx);
    let body = ctx.read_response(app.widget_id("table-body")).unwrap().rect;
    let from = body.min + vec2(GUTTER + 30.0, 50.0);
    let outside = body.max + vec2(40.0, 40.0);
    pointer(&mut app, &ctx, from, true, egui::Modifiers::NONE);
    for _ in 0..45 {
        input_frame(&mut app, &ctx, vec![egui::Event::PointerMoved(outside)]);
    }
    assert!(
        app.scroll_row > 5.0 && app.scroll_x > 50.0,
        "dragging edges must scroll both axes"
    );
    assert!(
        app.painted_cells < 200,
        "selection must preserve viewport culling"
    );
    pointer(&mut app, &ctx, outside, false, egui::Modifiers::NONE);
    let extended = app
        .selection
        .snapshot(app.view.len(), &app.visible)
        .unwrap();
    assert!(extended.last > 15 && extended.columns.len() >= 6);
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(matches!(app.selection, Selection::Empty));
    app.edit_filter(3);
    headless_frame(&mut app, &ctx);
    input_frame(&mut app, &ctx, vec![egui::Event::Copy]);
    assert!(
        app.copy_job.is_none(),
        "text input copy must not copy the grid"
    );
    app.changed_filter();
    assert!(matches!(app.selection, Selection::Empty));
    Ok(())
}
#[test]
fn dock_preserves_table_interaction_and_cell_readout() -> anyhow::Result<()> {
    let (mut app, ctx) = fixture()?;
    headless_frame(&mut app, &ctx);
    let full = ctx.read_response(app.widget_id("table-body")).unwrap().rect;
    app.dock = Some(Dock::Columns);
    wait_ready(&mut app, &ctx)?;
    headless_frame(&mut app, &ctx);
    let docked = ctx.read_response(app.widget_id("table-body")).unwrap().rect;
    assert!(docked.width() < full.width() - 250.0);
    let p = docked.min + vec2(GUTTER + 20.0, 16.0);
    input_frame(
        &mut app,
        &ctx,
        vec![
            egui::Event::PointerMoved(p),
            egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    input_frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(
        app.dock,
        Some(Dock::Columns),
        "table clicks must not dismiss the panel"
    );
    let (row, col, value) = app
        .cell_value
        .as_ref()
        .expect("click should expose full value");
    assert_eq!((*row, *col), (0, 0));
    assert!(!value.is_empty());
    app.edit_filter(3);
    headless_frame(&mut app, &ctx);
    input_frame(&mut app, &ctx, vec![egui::Event::Text("untd".into())]);
    assert!(app.filtering);
    assert_eq!(app.dock, Some(Dock::Columns));
    assert_eq!(app.painted_cells, 0);
    app.dock = None;
    headless_frame(&mut app, &ctx);
    headless_frame(&mut app, &ctx);
    assert_eq!(
        ctx.read_response(app.widget_id("table-body")).unwrap().rect,
        full
    );
    Ok(())
}
#[test]
fn inline_filters_keep_focus_and_geometry_while_loading() -> anyhow::Result<()> {
    let (mut app, ctx) = fixture()?;
    let id = app.widget_id(("column-filter", 3usize));
    assert!(app.filters_visible);
    app.edit_filter(3);
    headless_frame(&mut app, &ctx);
    let before = ctx.read_response(id).unwrap().rect;
    assert!(ctx.memory(|m| m.has_focus(id)));
    input_frame(&mut app, &ctx, vec![egui::Event::Text("unt".into())]);
    assert_eq!(app.filters[3], "unt");
    assert!(app.filtering);
    headless_frame(&mut app, &ctx);
    assert_eq!(app.painted_cells, 0, "loading indicator must replace rows");
    assert!(
        ctx.memory(|m| m.has_focus(id)),
        "filter input must survive loading"
    );
    let during = ctx.read_response(id).unwrap().rect;
    assert!(
        (before.min.y - during.min.y).abs() < 1.0,
        "active chips must not move the filter row: {before:?} -> {during:?}"
    );
    input_frame(&mut app, &ctx, vec![egui::Event::Text("d".into())]);
    assert_eq!(app.filters[3], "untd");
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::Tab, egui::Modifiers::NONE)],
    );
    assert!(ctx.memory(|m| m.has_focus(app.widget_id(("column-filter", 4usize)))));
    input_frame(&mut app, &ctx, vec![egui::Event::Text("actv".into())]);
    assert_eq!(app.filters[4], "actv");
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(app.filters[4].is_empty());
    assert_eq!(
        app.filters[3], "untd",
        "Escape only clears the current column"
    );
    app.visible[3] = false;
    app.filters_visible = false;
    app.edit_filter(3);
    headless_frame(&mut app, &ctx);
    assert!(app.visible[3] && app.filters_visible);
    assert!(ctx.memory(|m| m.has_focus(id)));
    assert_eq!(app.filters[3], "untd");
    app.visible.fill(true);
    app.edit_filter(5);
    headless_frame(&mut app, &ctx);
    input_frame(
        &mut app,
        &ctx,
        vec![key_event(Key::Tab, egui::Modifiers::NONE)],
    );
    assert!(ctx.memory(|m| m.has_focus(app.widget_id(("column-filter", 6usize)))));
    assert!(app.scroll_x > 0.0, "Tab must reveal off-screen filters");
    drop(app);
    Ok(())
}
#[test]
fn viewport_query_generations_and_match_navigation() -> anyhow::Result<()> {
    let (mut app, ctx) = fixture()?;
    assert_eq!(app.visible.iter().filter(|&&v| v).count(), 6);
    let top_cells = app.painted_cells;
    assert!(top_cells > 0 && top_cells < 200);
    app.scroll_row = 33_000.0;
    wait_ready(&mut app, &ctx)?;
    assert!(app.scroll_row > 32_970.0);
    assert!(app.painted_cells.abs_diff(top_cells) <= 6);
    assert_eq!(app.cache.value(32_999, 0), Some("AT-00033000"));
    app.visible.fill(true);
    app.scroll_x = 100_000.0;
    wait_ready(&mut app, &ctx)?;
    assert!(app.painted_cells < 200, "off-screen columns must be culled");
    assert!(app.cache.value(32_999, 29).is_some());
    app.visible.fill(false);
    app.visible[..6].fill(true);
    app.scroll_x = 0.0;
    // A queued old result must not replace the current view.
    app.filters[3] = "untd".into();
    app.changed_filter();
    let old_generation = app.filter_id;
    app.filters[3] = "germ".into();
    app.changed_filter();
    assert!(app.filter_id > old_generation);
    app.filter_pending = Some(Instant::now() - Duration::from_secs(1));
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.filtering {
        headless_frame(&mut app, &ctx);
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!app.view.is_empty() && app.view.len() < 33_000);
    let store = &app.source;
    for i in [0, app.view.len() / 2, app.view.len() - 1] {
        let row = app.view.select(i).unwrap();
        assert_eq!(
            store
                .read_block(row / store.schema.group_rows, 3)?
                .value((row % store.schema.group_rows) as usize),
            "Germany"
        );
    }
    app.search_column = 1;
    app.search_text = "amorg".into();
    app.changed_search();
    app.search_pending = Some(Instant::now() - Duration::from_secs(1));
    while app.searching {
        headless_frame(&mut app, &ctx);
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert!(app.matches.len() >= 3);
    let first = app.selected.unwrap();
    app.next_match(false);
    let second = app.selected.unwrap();
    app.next_match(false);
    let third = app.selected.unwrap();
    assert!(first < second && second < third);
    app.next_match(true);
    app.next_match(true);
    assert_eq!(app.selected, Some(first));
    app.next_match(true);
    assert_eq!(app.selected, Some(first));
    app.search_text.clear();
    app.changed_search();
    assert_eq!(app.matches.len(), 0);
    assert!(!app.searching);
    app.clear_filters();
    app.filter_pending = Some(Instant::now() - Duration::from_secs(1));
    while app.filtering {
        headless_frame(&mut app, &ctx);
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(app.view.len(), 33_000);
    drop(app);
    Ok(())
}

#[test]
fn independent_instances_dynamic_columns_and_host_style() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    ctx.set_theme(egui::Theme::Dark);
    let style = ctx.style_of(egui::Theme::Dark);
    let mut left = Table::new(
        "left",
        Arc::new(crate::test_support::MemoryData::new(0, 1)),
        ctx.clone(),
        TableOptions::default(),
    )?;
    let mut right = Table::new(
        "right",
        Arc::new(crate::test_support::MemoryData::new(50, 97)),
        ctx.clone(),
        TableOptions::default(),
    )?;
    right.visible.fill(false);
    right.visible[1] = true;
    right.visible[96] = true;
    right.selection.column(96, false, false, &right.visible);
    let mut host = || {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1800.0, 900.0))),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.columns(2, |columns| {
                        left.show(&mut columns[0]);
                        right.show(&mut columns[1]);
                    });
                });
            },
        );
        out.textures_delta.clear();
    };
    host();
    host();
    assert_eq!(
        ctx.style_of(egui::Theme::Dark),
        style,
        "widget must not change Context style"
    );
    let a = ctx
        .read_response(left.widget_id("table-body"))
        .unwrap()
        .rect;
    let b = ctx
        .read_response(right.widget_id("table-body"))
        .unwrap()
        .rect;
    assert!(a.max.x <= b.min.x);
    assert!(left.selection.snapshot(0, &left.visible).is_none());
    assert_eq!(
        right
            .selection
            .snapshot(50, &right.visible)
            .unwrap()
            .columns,
        [96]
    );
    assert_ne!(
        left.widget_id(("column-filter", 0)),
        right.widget_id(("column-filter", 0))
    );
    assert!(right.set_filter(97, "invalid").is_err());
    assert!(left.set_column_visible(0, false).is_err());
    right.set_filter(96, "othr")?;
    right.filter_pending = Some(Instant::now() - Duration::from_secs(1));
    let deadline = Instant::now() + Duration::from_secs(5);
    while right.filtering {
        headless_frame(&mut right, &ctx);
        anyhow::ensure!(Instant::now() < deadline, "filter timed out");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!right.view.is_empty() && right.view.len() < 50);
    assert_eq!(left.view.len(), 0);
    Ok(())
}

#[test]
fn u32_row_boundary_keeps_last_row_renderable() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    let mut table = Table::new(
        "large",
        Arc::new(crate::test_support::MemoryData::new(u32::MAX, 1)),
        ctx.clone(),
        TableOptions {
            query_threads: 1,
            ..Default::default()
        },
    )?;
    table.scroll_to_row(u32::MAX);
    wait_ready(&mut table, &ctx)?;
    assert_eq!(table.cache.value(u32::MAX - 1, 0), Some("AT-4294967295"));
    assert!(table.painted_cells > 0 && table.painted_cells < 100);
    Ok(())
}
