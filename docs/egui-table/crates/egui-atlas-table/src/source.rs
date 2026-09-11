//! Storage-independent dictionary blocks. The dynamic dispatch boundary is one
//! block read, never one cell. A source's schema and values are immutable for
//! its lifetime; replace the Table to display a different data revision.
use anyhow::{Result, ensure};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Schema {
    pub rows: u32,
    pub group_rows: u32,
    pub columns: Vec<String>,
}
/// Implementations may receive concurrent reads from viewport, query, and copy
/// workers. A block must contain exactly the group's row count, in source order.
/// Do not access egui or perform UI work here. Errors appear in the table.
pub trait DataSource: Send + Sync + 'static {
    fn schema(&self) -> Schema;
    fn read_block(&self, group: u32, column: usize) -> Result<ColumnBlock>;
}
#[derive(Clone)]
pub struct Source {
    pub(crate) schema: Schema,
    backend: Arc<dyn DataSource>,
}
impl Source {
    pub fn new(backend: Arc<dyn DataSource>) -> Result<Self> {
        let schema = backend.schema();
        ensure!(
            !schema.columns.is_empty(),
            "A table needs at least one column"
        );
        ensure!(
            schema.group_rows > 0 && schema.group_rows <= 65_536,
            "Row groups must contain 1..=65536 rows"
        );
        Ok(Self { schema, backend })
    }
    pub fn schema(&self) -> &Schema {
        &self.schema
    }
    pub fn groups(&self) -> u32 {
        self.schema.rows.div_ceil(self.schema.group_rows)
    }
    pub fn block_len(&self, group: u32) -> u32 {
        self.schema.group_rows.min(
            self.schema
                .rows
                .saturating_sub(group.saturating_mul(self.schema.group_rows)),
        )
    }
    pub fn read_block(&self, group: u32, column: usize) -> Result<ColumnBlock> {
        ensure!(
            group < self.groups() && column < self.schema.columns.len(),
            "Block out of range"
        );
        let block = self.backend.read_block(group, column)?;
        ensure!(
            block.codes.len() == self.block_len(group) as usize,
            "Wrong column row count"
        );
        Ok(block)
    }
}
/// Validated dictionary and codes, owned without copying the source's buffers.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnBlock {
    pub(crate) dictionary: Vec<String>,
    pub(crate) codes: Vec<u32>,
}
impl ColumnBlock {
    pub fn new(dictionary: Vec<String>, codes: Vec<u32>) -> Result<Self> {
        ensure!(
            codes.iter().all(|&c| (c as usize) < dictionary.len()),
            "Invalid dictionary code"
        );
        Ok(Self { dictionary, codes })
    }
    pub fn value(&self, offset: usize) -> &str {
        &self.dictionary[self.codes[offset] as usize]
    }
    pub fn dictionary(&self) -> &[String] {
        &self.dictionary
    }
    pub fn codes(&self) -> &[u32] {
        &self.codes
    }
    pub fn into_parts(self) -> (Vec<String>, Vec<u32>) {
        (self.dictionary, self.codes)
    }
    pub fn bytes(&self) -> usize {
        self.codes.capacity() * 4
            + self.dictionary.capacity() * std::mem::size_of::<String>()
            + self.dictionary.iter().map(|s| s.capacity()).sum::<usize>()
    }
}
pub fn worker_count() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(2).clamp(1, 6))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct BadSource;
    impl DataSource for BadSource {
        fn schema(&self) -> Schema {
            Schema {
                rows: 3,
                group_rows: 2,
                columns: vec!["A".into()],
            }
        }
        fn read_block(&self, _: u32, _: usize) -> Result<ColumnBlock> {
            ColumnBlock::new(vec!["x".into()], vec![0])
        }
    }
    #[test]
    fn validates_block_shape_and_codes() -> Result<()> {
        assert!(ColumnBlock::new(vec!["x".into()], vec![1]).is_err());
        let source = Source::new(Arc::new(BadSource))?;
        assert!(source.read_block(0, 0).is_err());
        assert!(source.read_block(1, 0).is_ok());
        assert!(source.read_block(2, 0).is_err());
        assert!(source.read_block(0, 1).is_err());
        Ok(())
    }
}
