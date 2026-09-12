//! Native entry point.
//!
//! ```text
//! volna-egui [FILE.vtr|FILE.fst]   open a trace
//! volna-egui --synthetic N         open a synthetic trace with N transitions
//! ```

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file: Option<std::path::PathBuf> = None;
    let mut synthetic: Option<usize> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--synthetic" => {
                synthetic = it
                    .next()
                    .and_then(|n| n.replace('_', "").parse().ok())
                    .or(Some(1_000_000))
            }
            "-h" | "--help" => {
                println!("usage: volna-egui [FILE.vtr|FILE.fst] [--synthetic N]");
                return Ok(());
            }
            _ => file = Some(a.into()),
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Volna")
            .with_app_id("volna-egui")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Volna",
        options,
        Box::new(move |cc| {
            let mut app = volna_egui::VolnaApp::new(&cc.egui_ctx);
            if let Some(path) = file {
                app.app.open_path(path);
            } else if let Some(n) = synthetic {
                app.app.open_synthetic(n);
            }
            Ok(Box::new(app))
        }),
    )
}
