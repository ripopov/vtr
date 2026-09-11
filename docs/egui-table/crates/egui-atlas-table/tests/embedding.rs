//! Public-API integration test: no access to the widget's private implementation.
use egui_atlas_table::{
    ColumnBlock, DataSource, Schema, Table, TableOptions, egui, selection::Selection,
};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
struct Data {
    ui_thread: thread::ThreadId,
}
impl DataSource for Data {
    fn schema(&self) -> Schema {
        Schema {
            rows: 4,
            group_rows: 2,
            columns: vec!["Item".into(), "Owner".into()],
        }
    }
    fn read_block(&self, group: u32, column: usize) -> anyhow::Result<ColumnBlock> {
        assert_ne!(
            thread::current().id(),
            self.ui_thread,
            "storage must not run on the render thread"
        );
        let dictionary = (group * 2..group * 2 + 2)
            .map(|row| format!("{column}:{row}"))
            .collect();
        ColumnBlock::new(dictionary, vec![0, 1])
    }
}
#[test]
fn grid_only_embedding_reads_off_thread_and_copies_with_headers() -> anyhow::Result<()> {
    let ctx = egui::Context::default();
    let mut table = Table::new(
        "embedded",
        Arc::new(Data {
            ui_thread: thread::current().id(),
        }),
        ctx.clone(),
        TableOptions {
            query_threads: 1,
            ..Default::default()
        },
    )?;
    table.set_selection(Selection::Rows { anchor: 1, end: 2 })?;
    table.copy_selection(&ctx);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 500.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.label("Application-owned toolbar");
                    let (rect, _) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                    let response = table.show_grid(ui, rect);
                    assert!(response.error.is_none());
                    assert_eq!(response.visible_rows, 4);
                });
            },
        );
        out.textures_delta.clear();
        if let Some(text) = out
            .platform_output
            .commands
            .into_iter()
            .find_map(|cmd| match cmd {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
        {
            assert_eq!(text, "Item\tOwner\n0:1\t1:1\n0:2\t1:2\n");
            break;
        }
        anyhow::ensure!(Instant::now() < deadline, "clipboard did not complete");
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
