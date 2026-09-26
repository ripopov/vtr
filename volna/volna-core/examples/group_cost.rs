//! Folded-group cost: writes a trace with M signals under one scope (a
//! third 1-bit, the rest 16-bit buses, each changing about C times at
//! random ticks, with a one-tick X on every tenth), adds the scope as a
//! group, folds it, times the build of its activity summary, and times one
//! waveform frame with the folded row zoomed out, at 1/100 of the trace and
//! over 200 ticks. The same views with the group open (a screen of member
//! rows) are timed for comparison. Best of five runs.
//!
//! cargo run --release -p volna-core --example group_cost -- 1000 10000
use std::time::Instant;

use volna_core::Theme;
use volna_core::app::{Action, App, Command};
use volna_core::geometry::Rect;
use volna_core::scene::MonoMeasure;
use volna_core::session::OpenSpec;
use volna_core::wave::viewport::Viewport;

fn write_trace(path: &std::path::Path, members: usize, changes: u64) -> anyhow::Result<u64> {
    let mut w = vtr::Writer::create(path)?;
    w.set_timescale(-9)?;
    let scope = w.add_scope(None, "top", vtr::ScopeType::Module, "top")?;
    let mut signals = Vec::with_capacity(members);
    for k in 0..members {
        let width = if k % 3 == 0 { 1 } else { 16 };
        let (_, s) = w.add_var(
            Some(scope),
            &format!("s{k}"),
            vtr::VarType::Wire,
            vtr::Direction::Output,
            vtr::SignalKind::Bits { width, states: 4 },
        )?;
        signals.push((s, width));
    }
    // Every member changes on average once per `members` ticks, so the
    // trace is `changes * members` ticks long.
    let end = changes * members as u64;
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let x_at = end / 2;
    for t in 0..end {
        w.set_time(t)?;
        let k = (rnd() % members as u64) as usize;
        let (s, width) = signals[k];
        if t == 0 {
            for &(s, width) in &signals {
                if width == 1 {
                    w.emit_bit(s, 0)?;
                } else {
                    w.emit_u64(s, 0)?;
                }
            }
            continue;
        }
        if k.is_multiple_of(10) && t.abs_diff(x_at) < members as u64 {
            if width == 1 {
                w.emit_bit(s, 2)?;
            } else {
                w.emit_logic_str(s, b"xxxxxxxxxxxxxxxx")?;
            }
        } else if width == 1 {
            w.emit_bit(s, (rnd() % 2) as u8)?;
        } else {
            w.emit_u64(s, rnd() % 65536)?;
        }
    }
    w.close()?;
    Ok(end)
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

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1).map(|a| a.parse::<u64>());
    let members = args.next().transpose()?.unwrap_or(1000) as usize;
    let changes = args.next().transpose()?.unwrap_or(10_000);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("groups.vtr");
    let end = write_trace(&path, members, changes)?;
    let mut app = App::new();
    app.settings_loaded(r#"{"memory.budgetMiB": 16384, "memory.objectMiB": 8192}"#);
    app.set_session(OpenSpec::Path(path).open()?);
    let load = Instant::now();
    app.handle(Command::AddScopeAsGroup {
        scope: 0,
        recursive: false,
    });
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            break;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
    let load_ms = load.elapsed().as_secs_f64() * 1000.0;
    let waves = app.panels.focused_id();
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
    let end = end as f64;
    let mid = end / 2.0;
    let views = [
        ("full", 0.0, end),
        ("hundredth", mid, mid + end / 100.0),
        ("samples", mid, mid + 200.0),
    ];
    let mut out = serde_json::Map::new();
    out.insert("members".into(), members.into());
    out.insert("changes_per_member".into(), changes.into());
    out.insert("load_ms".into(), load_ms.into());
    for (name, a, b) in views {
        out.insert(format!("open_{name}_ms"), frame(&mut app, a, b).into());
    }
    // Fold it as ← does; the summary builds on the load worker, here
    // synchronously.
    {
        let w = app.panels.waves_mut(waves).unwrap();
        w.selected = [0].into();
        w.anchor = Some(0);
    }
    app.handle(Command::Action(Action::PanLeft));
    let build = Instant::now();
    let requests = app.take_requests();
    out.insert("summaries".into(), requests.len().into());
    for request in requests {
        app.deliver(request.perform());
    }
    out.insert(
        "summary_build_ms".into(),
        (build.elapsed().as_secs_f64() * 1000.0).into(),
    );
    for (name, a, b) in views {
        out.insert(format!("folded_{name}_ms"), frame(&mut app, a, b).into());
    }
    println!("{}", serde_json::Value::Object(out));
    Ok(())
}
