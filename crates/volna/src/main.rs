//! Native entry point.
//!
//! ```text
//! volna [FILE.vtr|FILE.fst]   open a trace
//! volna --synthetic N         open a synthetic trace with N transitions
//! ```

#[cfg(not(target_family = "wasm"))]
fn main() {
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
                println!("usage: volna [FILE.vtr|FILE.fst] [--synthetic N]");
                return;
            }
            _ => file = Some(a.into()),
        }
    }
    volna::application().run(move |cx| {
        volna::init_app(cx);
        // Single-window tool: quit when the last window is closed.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        match volna::open_main_window(cx, false) {
            Ok(workspace) => {
                if let Some(path) = file {
                    workspace.update(cx, |ws, cx| ws.open_path(path, cx));
                } else if let Some(n) = synthetic {
                    workspace.update(cx, |ws, cx| ws.open_synthetic(n, cx));
                }
            }
            Err(e) => eprintln!("failed to open window: {e:#}"),
        }
        cx.activate(true);
    });
}

#[cfg(target_family = "wasm")]
fn main() {
    volna::web::start();
}
