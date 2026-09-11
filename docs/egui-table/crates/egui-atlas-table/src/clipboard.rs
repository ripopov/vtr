use crate::{rowset::RowSet, selection::CopyRange, source::Source};
use anyhow::{Result, ensure};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};
pub const CLIPBOARD_LIMIT: usize = 128 * 1024 * 1024;

pub struct CopyJob {
    pub rx: Receiver<Result<String>>,
    pub progress: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
}
impl CopyJob {
    pub fn start(
        store: Arc<Source>,
        view: RowSet,
        range: CopyRange,
        ctx: egui::Context,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(1);
        let progress = Arc::new(AtomicU32::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let p = progress.clone();
        let cancel = cancelled.clone();
        thread::Builder::new()
            .name("atlas-copy".into())
            .spawn(move || {
                let result = copy_tsv(&store, &view, &range, CLIPBOARD_LIMIT, &cancel, &p);
                if !cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(result);
                    ctx.request_repaint();
                }
            })?;
        Ok(Self {
            rx,
            progress,
            cancelled,
        })
    }
}
impl Drop for CopyJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn field(out: &mut String, value: &str, limit: usize) -> Result<()> {
    let quote = value.contains(['\t', '\n', '\r', '"']);
    let bytes = value.len()
        + if quote {
            2 + value.bytes().filter(|&b| b == b'"').count()
        } else {
            0
        };
    ensure!(
        out.len().saturating_add(bytes + 1) <= limit,
        "Selection exceeds the 128 MiB clipboard limit. Select fewer rows or columns; nothing was copied."
    );
    if quote {
        out.push('"');
        for c in value.chars() {
            out.push(c);
            if c == '"' {
                out.push('"');
            }
        }
        out.push('"');
    } else {
        out.push_str(value);
    }
    Ok(())
}
fn copy_tsv(
    store: &Source,
    view: &RowSet,
    range: &CopyRange,
    limit: usize,
    cancel: &AtomicBool,
    progress: &AtomicU32,
) -> Result<String> {
    ensure!(
        range.first <= range.last && range.last < view.len() && !range.columns.is_empty(),
        "Invalid selection"
    );
    let count = (range.last - range.first + 1) as usize;
    ensure!(
        count.saturating_mul(range.columns.len()) <= limit,
        "Selection exceeds the 128 MiB clipboard limit. Select fewer rows or columns; nothing was copied."
    );
    let mut out = String::new();
    for (i, &c) in range.columns.iter().enumerate() {
        ensure!(c < store.schema.columns.len(), "Invalid column");
        if i != 0 {
            out.push('\t');
        }
        field(&mut out, &store.schema.columns[c], limit)?;
    }
    out.push('\n');
    let mut group = None;
    let mut blocks = Vec::new();
    for index in range.first..=range.last {
        ensure!(!cancel.load(Ordering::Relaxed), "Copy cancelled");
        let source = view.select(index).unwrap();
        let next = source / store.schema.group_rows;
        if group != Some(next) {
            blocks.clear();
            let mut decoded = 0;
            for &c in &range.columns {
                ensure!(!cancel.load(Ordering::Relaxed), "Copy cancelled");
                let block = store.read_block(next, c)?;
                decoded += block.bytes();
                ensure!(
                    decoded <= CLIPBOARD_LIMIT,
                    "Selected column blocks exceed the copy memory budget; select fewer columns."
                );
                blocks.push(block);
            }
            group = Some(next);
        }
        for (i, block) in blocks.iter().enumerate() {
            if i != 0 {
                out.push('\t');
            }
            field(
                &mut out,
                block.value((source % store.schema.group_rows) as usize),
                limit,
            )?;
        }
        out.push('\n');
        progress.store(index - range.first + 1, Ordering::Relaxed);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tsv_filtered_order_headers_limits_and_cancellation() -> Result<()> {
        let store = crate::test_support::source(33_000, 30);
        let mut builder = crate::rowset::Builder::new(33_000);
        for r in [1, 16_384, 32_999] {
            builder.insert(r);
        }
        let view = builder.build();
        let range = CopyRange {
            first: 0,
            last: 2,
            columns: vec![0, 3],
        };
        let cancel = AtomicBool::new(false);
        let p = AtomicU32::new(0);
        let output = copy_tsv(&store, &view, &range, CLIPBOARD_LIMIT, &cancel, &p)?;
        let mut expected = String::from("Record ID\tCountry\n");
        for r in [1, 16_384, 32_999] {
            expected.push_str(&format!(
                "{}\t{}\n",
                store
                    .read_block(r / 16_384, 0)?
                    .value((r % 16_384) as usize),
                store
                    .read_block(r / 16_384, 3)?
                    .value((r % 16_384) as usize)
            ));
        }
        assert_eq!(output, expected);
        assert_eq!(p.load(Ordering::Relaxed), 3);
        assert!(copy_tsv(&store, &view, &range, 10, &cancel, &p).is_err());
        cancel.store(true, Ordering::Relaxed);
        assert!(copy_tsv(&store, &view, &range, CLIPBOARD_LIMIT, &cancel, &p).is_err());
        let mut escaped = String::new();
        field(&mut escaped, "a\tb\n\"c\"", 100)?;
        assert_eq!(escaped, "\"a\tb\n\"\"c\"\"\"");
        Ok(())
    }
}
