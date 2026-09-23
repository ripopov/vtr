//! Analog-row cost: writes a trace with a 16-bit signed bus and a real
//! signal that change every tick (N changes each), opens it through the
//! shared session backend and times one waveform frame with the bus drawn
//! digitally and as a plot, zoomed out (min/max columns), at 1/100 of the
//! trace and zoomed in (every sample), for each vertical range, after the
//! block min/max summary is built (timed separately). Best of five runs.
//!
//! cargo run --release -p volna-core --example analog_cost -- 10000000
use std::time::Instant;

use volna_core::app::{App, Command};
use volna_core::geometry::Rect;
use volna_core::scene::MonoMeasure;
use volna_core::session::OpenSpec;
use volna_core::wave::analog::{AnalogDraw, AnalogRange};
use volna_core::wave::viewport::Viewport;
use volna_core::{Action, Theme};

fn write_trace(path: &std::path::Path, n: u64) -> anyhow::Result<()> {
    let mut w = vtr::Writer::create(path)?;
    w.set_timescale(-9)?;
    let (_, bus) = w.add_var(
        "bus",
        vtr::VarType::Wire,
        vtr::Direction::Output,
        vtr::SignalKind::Bits {
            width: 16,
            states: 2,
        },
    );
    let (_, real) = w.add_var(
        "level",
        vtr::VarType::Real,
        vtr::Direction::Output,
        vtr::SignalKind::Real,
    );
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    for t in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let phase = t as f64 * std::f64::consts::TAU / 4000.0;
        let noise = (seed % 200) as f64 - 100.0;
        w.set_time(t)?;
        w.emit_u64(
            bus,
            u64::from((phase.sin() * 10_000.0 + noise) as i16 as u16),
        )?;
        w.emit_real(real, phase.sin() + noise / 10_000.0)?;
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

fn main() -> anyhow::Result<()> {
    let n: u64 = std::env::args()
        .nth(1)
        .map(|a| a.parse())
        .transpose()?
        .unwrap_or(1_000_000);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("analog.vtr");
    write_trace(&path, n)?;
    let mut app = App::new();
    app.settings_loaded(r#"{"memory.budgetMiB": 16384, "memory.objectMiB": 8192}"#);
    app.set_session(OpenSpec::Path(path).open()?);
    let load = Instant::now();
    app.handle(Command::AddVars(vec![0]));
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
    let set_view = |app: &mut App, start: f64, end: f64| {
        let App { panels, doc, .. } = app;
        panels
            .waves_mut(waves)
            .unwrap()
            .nav
            .jump_to(doc, Viewport { start, end });
    };
    let once = |app: &mut App| {
        let t = Instant::now();
        app.layout_panel(waves, bounds, &theme).unwrap();
        app.render_panel(waves, &theme, &mut MonoMeasure);
        t.elapsed().as_secs_f64() * 1000.0
    };
    let frame = |app: &mut App, start: f64, end: f64| {
        set_view(app, start, end);
        best_ms(|| {
            app.layout_panel(waves, bounds, &theme).unwrap();
            app.render_panel(waves, &theme, &mut MonoMeasure);
        })
    };
    let end = n as f64;
    let mid = end / 2.0;
    let views = [
        ("full", 0.0, end),
        ("hundredth", mid, mid + end / 100.0),
        ("samples", mid, mid + 200.0),
    ];
    let mut out = serde_json::Map::new();
    out.insert("changes".into(), n.into());
    out.insert("load_ms".into(), load_ms.into());
    for (name, a, b) in views {
        out.insert(format!("digital_{name}_ms"), frame(&mut app, a, b).into());
    }
    app.panels.waves_mut(waves).unwrap().selected = [0].into();
    // Signed decimal, then analog: the summary is built for that reading.
    while app.panels.waves(waves).unwrap().items[0]
        .signal()
        .unwrap()
        .translator
        .id()
        != "sdec"
    {
        app.handle(Command::Action(Action::CycleFormat));
    }
    app.handle(Command::Action(Action::ToggleAnalog));
    // The summary builds on the load worker; here, synchronously.
    let build = Instant::now();
    let requests = app.take_requests();
    let summaries = requests.len();
    for request in requests {
        app.deliver(request.perform());
    }
    out.insert("summaries".into(), summaries.into());
    out.insert(
        "summary_build_ms".into(),
        (build.elapsed().as_secs_f64() * 1000.0).into(),
    );
    set_view(&mut app, 0.0, end);
    out.insert("first_trace_frame_ms".into(), once(&mut app).into());
    for range in [AnalogRange::Trace, AnalogRange::Window, AnalogRange::Type] {
        app.panels
            .waves_mut(waves)
            .unwrap()
            .set_analog_range(&[0], range);
        for (name, a, b) in views {
            out.insert(
                format!("step_{}_{name}_ms", range.label().replace(' ', "_")),
                frame(&mut app, a, b).into(),
            );
        }
    }
    app.panels
        .waves_mut(waves)
        .unwrap()
        .set_analog(&[0], Some(AnalogDraw::Linear));
    app.panels
        .waves_mut(waves)
        .unwrap()
        .set_analog_range(&[0], AnalogRange::Trace);
    for (name, a, b) in views {
        out.insert(
            format!("linear_whole_trace_{name}_ms"),
            frame(&mut app, a, b).into(),
        );
    }
    println!("{}", serde_json::Value::Object(out));
    Ok(())
}
