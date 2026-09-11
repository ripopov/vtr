use crate::storage::{Store, worker_count};
use anyhow::Result;
use egui_atlas_table::{
    Source,
    query::{Predicate, filter, search},
};
use std::{sync::Arc, time::Instant};
pub fn benchmark(store: Arc<Store>) -> Result<()> {
    let source = Source::new(store.clone())?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count())
        .build()?;
    println!(
        "{} rows × 30 columns; {:.1} MiB file; {} scan workers",
        store.index.rows,
        store.file_bytes as f64 / 1048576.0,
        worker_count()
    );
    let start = Instant::now();
    let mut bytes = 0;
    for row in [0, store.index.rows / 2, store.index.rows - 1] {
        for c in 0..6 {
            bytes += store.read_block(row / store.index.group_rows, c)?.bytes();
        }
    }
    println!(
        "18 random column-block reads/decompressions: {:.2} ms, {:.1} MiB decoded",
        start.elapsed().as_secs_f64() * 1000.0,
        bytes as f64 / 1048576.0
    );
    let predicates = vec![
        Predicate {
            column: 3,
            text: "untd".into(),
        },
        Predicate {
            column: 4,
            text: "actv".into(),
        },
    ];
    let start = Instant::now();
    let view = filter(&source, &predicates, &pool, &|| false, |_| {})?.unwrap();
    println!(
        "Two-column fuzzy filter: {:.2} ms; {} rows; {:.2} MiB mapping",
        start.elapsed().as_secs_f64() * 1000.0,
        view.len(),
        view.bytes() as f64 / 1048576.0
    );
    let start = Instant::now();
    let mut first = true;
    search(
        &source,
        &Predicate {
            column: 1,
            text: "amorg".into(),
        },
        &view,
        &|| false,
        |matches, done, _| {
            if first && !matches.is_empty() {
                println!(
                    "Search first result: {:.2} ms",
                    start.elapsed().as_secs_f64() * 1000.0
                );
                first = false;
            }
            if done {
                println!(
                    "Search completed: {:.2} ms; {} matches",
                    start.elapsed().as_secs_f64() * 1000.0,
                    matches.len()
                );
            }
        },
    )?;
    let start = Instant::now();
    let mut checksum = 0u64;
    for i in 0..100_000u32 {
        if !view.is_empty() {
            checksum += view.select(i.wrapping_mul(7919) % view.len()).unwrap() as u64;
        }
    }
    println!(
        "100,000 random filtered row lookups: {:.2} ms (checksum {checksum})",
        start.elapsed().as_secs_f64() * 1000.0
    );
    let start = Instant::now();
    let unique = filter(
        &source,
        &[Predicate {
            column: 0,
            text: format!("AT-{:08}", store.index.rows),
        }],
        &pool,
        &|| false,
        |_| {},
    )?
    .unwrap();
    println!(
        "Unique-ID fuzzy filter: {:.2} ms; {} matches (higher-cardinality scan)",
        start.elapsed().as_secs_f64() * 1000.0,
        unique.len()
    );
    Ok(())
}
