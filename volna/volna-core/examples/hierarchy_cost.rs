//! Headless hierarchy expansion and workspace-capture cost on a local trace.
//! cargo run --release -p volna-core --example hierarchy_cost -- TRACE
use std::time::Instant;
use volna_core::app::{App, Command};
use volna_core::session::OpenSpec;
use volna_core::workspace::Workspace;
use volna_core::workspace::persistence::Persistence;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("hierarchy_cost TRACE");
    let start = Instant::now();
    let session = OpenSpec::Path(path.clone().into()).open()?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.0;
    let scopes = session.hierarchy().scopes.len();
    let branches = session
        .hierarchy()
        .scopes
        .iter()
        .filter(|s| !s.children.is_empty())
        .count();
    let mut app = App::new();
    app.set_session(session);
    app.configure_persistence(Persistence::Auto);
    println!("open_ms={open_ms:.1} scopes={scopes} branches={branches}");
    let start = Instant::now();
    app.handle(Command::ExpandAllScopes(true));
    println!(
        "expand_ms={:.1} visible={} expanded={}",
        start.elapsed().as_secs_f64() * 1000.0,
        app.scopes.visible.len(),
        app.scopes.expanded().count()
    );
    let start = Instant::now();
    let snapshot = Workspace::capture(&app, |_| Ok(Some(path.clone())), None)?;
    println!("capture_ms={:.1}", start.elapsed().as_secs_f64() * 1000.0);
    let start = Instant::now();
    let bytes = snapshot.to_bytes()?;
    println!(
        "serialize_ms={:.1} workspace_bytes={}",
        start.elapsed().as_secs_f64() * 1000.0,
        bytes.len()
    );
    Ok(())
}
