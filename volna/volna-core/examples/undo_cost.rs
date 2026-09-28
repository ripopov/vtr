//! Undo journal cost (`docs/undo-redo.html`): the size of a wave row, the
//! time to detach N rows from their histories (what the journal keeps for a
//! removed row or a closed panel), and the time and size of saving and
//! restoring an N-row wave panel through the workspace codec (what a
//! snapshot-per-step journal would pay). Then the journal itself: removing
//! all N rows of one panel and closing a second N-row panel, each undone and
//! redone while the other panel keeps the histories resident, with the
//! bytes the journal holds. Best of five runs.
//!
//! cargo run --release -p volna-core --example undo_cost -- 100000
use std::time::Instant;

use volna_core::app::{Action, App, Command};
use volna_core::panels::PanelsCommand;
use volna_core::session::OpenSpec;
use volna_core::wave::{Entry, model::WaveRow};
use volna_core::workspace::Workspace;

const TRACE: &str = "vscode-remote://ssh-remote+board/home/user/trace.vtr";
const LOCATION: &str = "vscode-remote://ssh-remote+board/home/user/trace.vtr.volna.json";

fn write_trace(path: &std::path::Path, vars: usize) -> anyhow::Result<()> {
    let mut w = vtr::Writer::create(path)?;
    w.set_timescale(-9)?;
    let top = w.add_scope(None, "top", vtr::ScopeType::Module, "top")?;
    let mut signals = Vec::new();
    let per = 1000;
    for u in 0..vars.div_ceil(per) {
        let unit = w.add_scope(
            Some(top),
            &format!("unit_{u}"),
            vtr::ScopeType::Module,
            "unit",
        )?;
        let core = w.add_scope(Some(unit), "core", vtr::ScopeType::Module, "core")?;
        for k in 0..per.min(vars - u * per) {
            let (_, s) = w.add_var(
                Some(core),
                &format!("sig_{k}"),
                vtr::VarType::Wire,
                vtr::Direction::Output,
                vtr::SignalKind::Bits {
                    width: 16,
                    states: 4,
                },
            )?;
            signals.push(s);
        }
    }
    for t in 0..4u64 {
        w.set_time(t * 10)?;
        for (i, &s) in signals.iter().enumerate() {
            w.emit_u64(s, (i as u64 * 7 + t) % 65536)?;
        }
    }
    w.close()?;
    Ok(())
}

fn best_ms(mut f: impl FnMut()) -> f64 {
    (0..5)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            break;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
}

fn time_ms(f: impl FnOnce()) -> f64 {
    let t = Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1000.0
}

/// Undo and redo the last step five times each; the best of each, and
/// whether any of them queued a load.
fn flip_ms(app: &mut App) -> (f64, f64, bool) {
    let (mut undo, mut redo, mut loads) = (f64::INFINITY, f64::INFINITY, false);
    for _ in 0..5 {
        undo = undo.min(time_ms(|| app.handle(Command::Undo)));
        loads |= !app.take_requests().is_empty();
        redo = redo.min(time_ms(|| app.handle(Command::Redo)));
        loads |= !app.take_requests().is_empty();
    }
    (undo, redo, loads)
}

fn main() -> anyhow::Result<()> {
    let vars: usize = std::env::args()
        .nth(1)
        .map(|a| a.parse())
        .transpose()?
        .unwrap_or(100_000);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("trace.vtr");
    write_trace(&path, vars)?;
    let mut app = App::new();
    app.settings_loaded(r#"{"memory.budgetMiB": 16384, "memory.objectMiB": 8192}"#);
    app.set_session(OpenSpec::Path(path).open()?);
    app.handle(Command::AddVars((0..vars).collect()));
    pump(&mut app);
    let id = app.panels.focused_id();
    let items = app.panels.waves(id).unwrap().items().to_vec();
    let heap: usize = items
        .iter()
        .map(|e| match &e.row {
            WaveRow::Signal(s) => s.name.capacity() + s.scope.capacity(),
            _ => 0,
        })
        .sum();
    let mut detached = Vec::new();
    let detach_ms = best_ms(|| detached = items.iter().map(Entry::detached).collect::<Vec<_>>());
    let mut bytes = Vec::new();
    let capture_ms = best_ms(|| {
        bytes = Workspace::capture(&app, "trace.vtr".into(), None)
            .unwrap()
            .to_bytes()
            .unwrap()
    });
    let restore_ms = best_ms(|| {
        let plan = Workspace::parse(&bytes)
            .and_then(|ws| ws.prepare(&app, TRACE, LOCATION))
            .unwrap();
        plan.commit(&mut app).unwrap();
    });
    pump(&mut app);

    // The journal: panel A loses its rows while panel B shares them, then
    // panel B closes while panel A shares them.
    let a = app.panels.focused_id();
    app.handle(Command::Action(Action::SplitRight));
    let b = app.panels.focused_id();
    app.handle(Command::Panels(PanelsCommand::Focus(a)));
    app.handle(Command::Action(Action::SelectAll));
    let remove_ms = time_ms(|| app.handle(Command::Action(Action::RemoveSelected)));
    let remove_journal_bytes = app.history.undo_steps().last().unwrap().bytes();
    let (undo_remove_ms, redo_remove_ms, remove_loads) = flip_ms(&mut app);
    app.handle(Command::Undo);
    app.handle(Command::Panels(PanelsCommand::Focus(b)));
    let close_ms = time_ms(|| app.handle(Command::Action(Action::ClosePanel)));
    let close_journal_bytes = app.history.undo_steps().last().unwrap().bytes();
    let (undo_close_ms, redo_close_ms, close_loads) = flip_ms(&mut app);
    println!(
        "{}",
        serde_json::json!({
            "rows": items.len(),
            "entry_bytes": std::mem::size_of::<Entry>(),
            "string_heap_bytes_per_row": heap as f64 / items.len() as f64,
            "detach_ms": detach_ms,
            "detached_rows": detached.len(),
            "workspace_bytes": bytes.len(),
            "workspace_bytes_per_row": bytes.len() as f64 / items.len() as f64,
            "capture_ms": capture_ms,
            "restore_ms": restore_ms,
            "remove_all_ms": remove_ms,
            "remove_journal_bytes": remove_journal_bytes,
            "undo_remove_ms": undo_remove_ms,
            "redo_remove_ms": redo_remove_ms,
            "close_panel_ms": close_ms,
            "close_journal_bytes": close_journal_bytes,
            "undo_close_ms": undo_close_ms,
            "redo_close_ms": redo_close_ms,
            "flips_queued_loads": remove_loads || close_loads,
        })
    );
    Ok(())
}
