mod app;
mod bench_query;
mod chrome;
mod storage;
mod theme;

use anyhow::{Context, Result, bail};
use std::{path::PathBuf, sync::Arc, time::Instant};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Atlas — large table viewer\n\n  cargo run --release                         Open/create 10M-row demo\n  cargo run --release -- generate [FILE] [ROWS]\n  cargo run --release -- view [FILE]\n  cargo run --release -- bench-ui [FILE]        Measure viewport rendering
  cargo run --release -- bench [FILE]\n\nDefault: data/atlas-10m.pb.zst. Generation is deterministic and never overwrites an existing file."
        );
        return Ok(());
    }
    let command = args.first().map(String::as_str).unwrap_or("view");
    let path = PathBuf::from(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("data/atlas-10m.pb.zst"),
    );
    match command {
        "generate" => {
            let rows = args
                .get(2)
                .map(|s| s.parse())
                .transpose()?
                .unwrap_or(10_000_000);
            let start = Instant::now();
            storage::generate(&path, rows, |done, total| {
                if done == total || done % 32 == 0 {
                    eprintln!("Generating {done}/{total} row groups");
                }
            })?;
            println!(
                "Created {} in {:.2}s ({:.1} MiB)",
                path.display(),
                start.elapsed().as_secs_f64(),
                std::fs::metadata(&path)?.len() as f64 / 1048576.0
            );
        }
        "bench-ui" => app::benchmark_ui(path)?,
        "bench" => bench_query::benchmark(Arc::new(storage::Store::open(&path)?))?,
        "view" => {
            let options = eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_title("Atlas · Table viewer")
                    .with_decorations(false)
                    .with_transparent(cfg!(target_os = "linux"))
                    .with_inner_size([1440.0, 900.0])
                    .with_min_inner_size([820.0, 560.0]),
                ..Default::default()
            };
            eframe::run_native(
                "Atlas",
                options,
                Box::new(move |cc| Ok(Box::new(app::Atlas::new(cc, path)))),
            )
            .map_err(|e| anyhow::anyhow!(e.to_string()))
            .context("opening desktop window")?;
        }
        _ => bail!("Unknown command {command:?}; use --help"),
    }
    Ok(())
}
