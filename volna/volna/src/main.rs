//! Native entry point; library and test workspaces never opt into persistence.
#[cfg(not(target_family = "wasm"))]
fn main() {
    use volna::native_workspace::{Options, Store};
    use volna_core::workspace::persistence::Persistence;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = match Options::parse(
        std::env::args().skip(1),
        std::env::var("VOLNA_WORKSPACE").ok(),
        std::env::var_os("VOLNA_CONFIG_DIR").map(Into::into),
    ) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
    };
    if options.help {
        println!(
            "usage: volna [FILE.vtr|FILE.fst|FILE.volna.json] [--synthetic N]\n  --workspace FILE   load and autosave an explicit workspace\n  --no-workspace     disable workspace reads and writes\n  --config-dir DIR   preferences directory\nEnvironment: VOLNA_WORKSPACE=off|FILE, VOLNA_CONFIG_DIR=DIR"
        );
        return;
    }
    let mut store = match Store::new(options.config_dir) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
    };
    let preferences = store.preferences().unwrap_or_else(|error| {
        eprintln!("preferences: {error:#}");
        Default::default()
    });
    let policy = if !preferences.autosave && options.policy == Persistence::Auto {
        Persistence::Disabled
    } else {
        options.policy
    };
    volna::application().run(move |cx| {
        volna::init_app(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        match volna::open_main_window(cx, false) {
            Ok(workspace) => workspace.update(cx, |ws, cx| {
                ws.enable_native_persistence(store, policy, preferences);
                if let Some(path) = options.file {
                    ws.open_path(path, cx);
                } else if let Some(n) = options.synthetic {
                    ws.open_synthetic(n, cx);
                } else if let Persistence::Explicit(target) = &ws.app.workspace.scheduler.policy
                    && let Ok(path) = volna::native_workspace::path_from_uri(target.location())
                {
                    ws.open_workspace_path(path, cx);
                }
            }),
            Err(error) => eprintln!("failed to open window: {error:#}"),
        }
        cx.activate(true);
    });
}
#[cfg(target_family = "wasm")]
fn main() {
    volna::web::start();
}
