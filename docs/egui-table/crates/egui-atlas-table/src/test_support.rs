use crate::{ColumnBlock, DataSource, Schema, Source};
use std::sync::Arc;
pub struct MemoryData {
    rows: u32,
    columns: usize,
}
impl MemoryData {
    pub fn new(rows: u32, columns: usize) -> Self {
        Self { rows, columns }
    }
}
impl DataSource for MemoryData {
    fn schema(&self) -> Schema {
        let names = [
            "Record ID",
            "Name",
            "Company",
            "Country",
            "Status",
            "Revenue",
        ];
        Schema {
            rows: self.rows,
            group_rows: 16_384,
            columns: (0..self.columns)
                .map(|c| {
                    names
                        .get(c)
                        .map_or_else(|| format!("Column {c}"), |s| s.to_string())
                })
                .collect(),
        }
    }
    fn read_block(&self, group: u32, column: usize) -> anyhow::Result<ColumnBlock> {
        let start = group * 16_384;
        let n = 16_384.min(self.rows - start);
        if column == 0 {
            return ColumnBlock::new(
                (start..start + n)
                    .map(|r| format!("AT-{:08}", r + 1))
                    .collect(),
                (0..n).collect(),
            );
        }
        let values = match column {
            1 => vec!["Amelia Morgan", "Noah Chen", "Chloé Chen"],
            3 => vec!["United Kingdom", "Germany", "United States", "Japan"],
            4 => vec!["Active", "Paused", "Trial"],
            _ => vec!["Value", "Other"],
        };
        let codes = (start..start + n)
            .map(|r| (r as usize / (column % 5 + 1) % values.len()) as u32)
            .collect();
        ColumnBlock::new(values.into_iter().map(str::to_owned).collect(), codes)
    }
}
pub fn source(rows: u32, columns: usize) -> Source {
    Source::new(Arc::new(MemoryData::new(rows, columns))).unwrap()
}
