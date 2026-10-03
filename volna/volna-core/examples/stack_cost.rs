//! Stacked-area cost (`docs/stacked-areas.html`): writes a trace with eight
//! one-bit issue valids under `issue`, each changing about N times at
//! random ticks, opens it through the shared session backend, stacks the
//! group, folds it (so only the stacked row paints) and times one waveform
//! frame zoomed out (one mean per layer and pixel column), at 1/100 of the
//! trace and zoomed in (every step), best of five. The whole-trace walk that
//! sets the scale and the layers' integral summaries (built for stacks of
//! more than 16,384 changes) are timed separately, as the load worker would
//! run them.
//!
//! cargo run --release -p volna-core --example stack_cost -- 1000000
use std::time::Instant;

use volna_core::app::{App, Command};
use volna_core::geometry::Rect;
use volna_core::scene::MonoMeasure;
use volna_core::wave::viewport::Viewport;
use volna_core::{Action, Theme};
use volna_trace::session::OpenSpec;

const MEMBERS: usize = 8;

fn write_trace(path: &std::path::Path, n: u64) -> anyhow::Result<()> {
    let mut w = vtr::Writer::create(path)?;
    w.set_timescale(-9)?;
    let issue = w.add_scope(None, "issue", vtr::ScopeType::Module, "issue")?;
    let valids = (0..MEMBERS)
        .map(|k| {
            w.add_var(
                Some(issue),
                &format!("vld{k}"),
                vtr::VarType::Wire,
                vtr::Direction::Output,
                vtr::SignalKind::Bits {
                    width: 1,
                    states: 2,
                },
            )
            .map(|(_, id)| id)
        })
        .collect::<vtr::Result<Vec<_>>>()?;
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    // A random bit per tick changes half the time: 2N ticks give N changes.
    for t in 0..2 * n {
        w.set_time(t)?;
        for (k, &id) in valids.iter().enumerate() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            w.emit_bit(id, ((seed >> (k * 7)) & 1) as u8)?;
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

fn main() -> anyhow::Result<()> {
    let n: u64 = std::env::args()
        .nth(1)
        .map(|a| a.parse())
        .transpose()?
        .unwrap_or(1_000_000);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("stack.vtr");
    write_trace(&path, n)?;
    let mut app = App::new();
    app.settings_loaded(r#"{"memory.budgetMiB": 16384, "memory.objectMiB": 8192}"#);
    app.set_session(OpenSpec::Path(path).open()?);
    pump(&mut app);
    let issue = match app
        .doc
        .hierarchy(volna_core::trace::TraceId::A)
        .unwrap()
        .find_scope(&["issue"])
    {
        volna_trace::data::source::Lookup::Found(s) => s,
        _ => anyhow::bail!("no issue scope"),
    };
    let load = Instant::now();
    app.handle(Command::AddScopeAsGroup {
        scope: volna_core::trace::Traced::new(volna_core::trace::TraceId::A, issue),
        recursive: false,
    });
    pump(&mut app);
    let load_ms = load.elapsed().as_secs_f64() * 1000.0;
    let waves = app.panels.focused_id();
    let changes: usize = app
        .panels
        .waves(waves)
        .unwrap()
        .group_histories(0)
        .iter()
        .map(|h| h.len())
        .sum();
    app.handle(Command::Action(Action::ToggleStack));
    // The whole-trace walk and the layers' integral summaries run on the
    // load worker; here, synchronously, each timed.
    let (mut walk_ms, mut integral_ms, mut integrals) = (0.0, 0.0, 0);
    for request in app.take_requests() {
        let integral = matches!(request, volna_core::session::LoadRequest::Integral { .. });
        let build = Instant::now();
        let result = request.perform();
        let ms = build.elapsed().as_secs_f64() * 1000.0;
        if integral {
            integral_ms += ms;
            integrals += 1;
        } else {
            walk_ms += ms;
        }
        app.deliver(result);
    }
    // Folded, only the stacked row paints.
    app.handle(Command::Action(Action::PanLeft));
    let theme = Theme::one_dark();
    let bounds = Rect::from_xywh(0.0, 0.0, 1400.0, 300.0);
    let frame = |app: &mut App, start: f64, end: f64| {
        let App { panels, doc, .. } = &mut *app;
        panels
            .waves_mut(waves)
            .unwrap()
            .nav
            .jump_to(doc, Viewport { start, end });
        best_ms(|| {
            app.layout_panel(waves, bounds, &theme).unwrap();
            app.render_panel(waves, &theme, &mut MonoMeasure);
        })
    };
    let end = 2.0 * n as f64;
    let mid = end / 2.0;
    let mut out = serde_json::Map::new();
    out.insert("changes_per_member".into(), (changes / MEMBERS).into());
    out.insert("load_ms".into(), load_ms.into());
    out.insert("walk_ms".into(), walk_ms.into());
    out.insert("integral_summaries".into(), integrals.into());
    out.insert("integral_build_ms".into(), integral_ms.into());
    for (name, a, b) in [
        ("full", 0.0, end),
        ("hundredth", mid, mid + end / 100.0),
        ("steps", mid, mid + 200.0),
    ] {
        out.insert(format!("stack_{name}_ms"), frame(&mut app, a, b).into());
    }
    // The same group folded as activity, for comparison.
    app.handle(Command::Action(Action::ToggleStack));
    pump(&mut app);
    out.insert("activity_full_ms".into(), frame(&mut app, 0.0, end).into());
    println!("{}", serde_json::Value::Object(out));
    Ok(())
}
