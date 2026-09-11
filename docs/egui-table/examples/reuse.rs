//! Run `cargo run --release --example reuse` for two independent tables backed
//! by ordinary application data, without the Atlas file format or generator.
use egui_atlas_table::{ColumnBlock, DataSource, Schema, Table, TableOptions, egui};
use std::sync::Arc;
struct Records {
    columns: Vec<Vec<String>>,
    names: Vec<String>,
}
impl DataSource for Records {
    fn schema(&self) -> Schema {
        Schema {
            rows: self.columns[0].len() as u32,
            group_rows: 256,
            columns: self.names.clone(),
        }
    }
    fn read_block(&self, group: u32, column: usize) -> anyhow::Result<ColumnBlock> {
        let start = group as usize * 256;
        let end = (start + 256).min(self.columns[column].len());
        // Unique-value blocks work too; dictionary encoding is optional for an adapter.
        ColumnBlock::new(
            self.columns[column][start..end].to_vec(),
            (0..(end - start) as u32).collect(),
        )
    }
}
struct App {
    left: Table,
    right: Table,
}
impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(root, |ui| {
            ui.columns(2, |columns| {
                self.left.show(&mut columns[0]);
                self.right.show(&mut columns[1]);
            });
        });
    }
}
fn main() -> anyhow::Result<()> {
    let source = Arc::new(Records {
        names: vec!["Task".into(), "Owner".into(), "State".into()],
        columns: vec![
            (0..5000).map(|i| format!("Task {i}")).collect(),
            (0..5000).map(|i| format!("Owner {}", i % 20)).collect(),
            (0..5000)
                .map(|i| if i % 3 == 0 { "Complete" } else { "Open" }.into())
                .collect(),
        ],
    });
    eframe::run_native(
        "Reusable tables",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 900.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(App {
                left: Table::new(
                    "all-tasks",
                    source.clone(),
                    cc.egui_ctx.clone(),
                    TableOptions {
                        title: "All tasks".into(),
                        ..Default::default()
                    },
                )?,
                right: Table::new(
                    "other-view",
                    source,
                    cc.egui_ctx.clone(),
                    TableOptions {
                        title: "Independent view".into(),
                        ..Default::default()
                    },
                )?,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))
}
