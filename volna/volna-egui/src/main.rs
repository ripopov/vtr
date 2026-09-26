//! Native entry point.
//!
//! ```text
//! volna-egui [FILE.vtr|FILE.fst]   open a trace
//! ```

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file: Option<std::path::PathBuf> = None;
    for a in &args {
        match a.as_str() {
            "-h" | "--help" => {
                println!("usage: volna-egui [FILE.vtr|FILE.fst]");
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
            }
            Ok(Box::new(app))
        }),
    )
}
