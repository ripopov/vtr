//! Transaction-lane cost: writes a synthetic generator of N overlapping
//! reads (address and data stages, 1% failures), opens it through the
//! shared session backend, and times the generator index build (which
//! includes sub-row stacking) and one waveform frame with a lane in bars,
//! folded bars and density mode. Best of five runs per frame.
//!
//! cargo run --release -p volna-core --example lane_cost -- 1000000
use std::time::Instant;

use volna_core::app::{App, Command};
use volna_core::data::Member;
use volna_core::data::loaded_tracks::LoadedGenerator;
use volna_core::geometry::Rect;
use volna_core::scene::MonoMeasure;
use volna_core::session::OpenSpec;
use volna_core::wave::model::RowHeight;
use volna_core::wave::viewport::Viewport;
use volna_core::{Action, Theme};

fn write_trace(path: &std::path::Path, n: u64) -> anyhow::Result<()> {
    let mut w = vtr::Writer::create(path)?;
    w.set_timescale(-9)?;
    let stream = w.add_stream(None, "axi", "AXI4")?;
    let generator = w.add_generator(stream, "rd")?;
    let (addr, data, lane) = (w.intern("ADDR"), w.intern("DATA"), w.intern("0"));
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut open: Vec<(u64, vtr::TxId, bool)> = Vec::new();
    for i in 0..n {
        let begin = i * 4;
        // Close every read that ends by now, in end order.
        open.sort_by_key(|(end, ..)| std::cmp::Reverse(*end));
        while open.last().is_some_and(|(end, ..)| *end <= begin) {
            let (end, tx, failed) = open.pop().unwrap();
            w.set_time(end)?;
            let status = if failed {
                vtr::TxStatus::Error
            } else {
                vtr::TxStatus::Ok
            };
            w.end_tx(tx, end, status)?;
        }
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let life = 8 + seed % 33;
        w.set_time(begin)?;
        let tx = w.begin_tx(generator, begin)?;
        w.tx_stage(tx, addr, lane, begin, begin + 2, &[])?;
        w.tx_stage(tx, data, lane, begin + life - 4, begin + life, &[])?;
        open.push((begin + life, tx, seed.is_multiple_of(100)));
    }
    open.sort_by_key(|(end, ..)| *end);
    for (end, tx, _) in open {
        w.set_time(end)?;
        w.end_tx(tx, end, vtr::TxStatus::Ok)?;
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
    let path = dir.path().join("lanes.vtr");
    write_trace(&path, n)?;
    let session = OpenSpec::Path(path).open()?;
    let track = session.tracks()[1].id;
    let mut app = App::new();
    app.settings_loaded(r#"{"memory.budgetMiB": 16384, "memory.objectMiB": 8192}"#);
    app.set_session(session.clone());
    let load = Instant::now();
    app.handle(Command::AddToWaves(vec![Member::Generator(0)]));
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
    if let Some(volna_core::document::TrackLoadState::Failed(error)) = app.doc.track(track) {
        anyhow::bail!("load failed: {error}");
    }
    let records = app
        .doc
        .resident_generator(track)
        .expect("lane loaded")
        .transactions()
        .to_vec();
    let depth = app.doc.resident_generator(track).unwrap().depth();
    let index_ms = best_ms(|| {
        LoadedGenerator::new(
            track,
            records.clone(),
            Default::default(),
            Default::default(),
        )
        .unwrap();
    });
    let clone_ms = best_ms(|| drop(records.clone()));

    let theme = Theme::one_dark();
    let bounds = Rect::from_xywh(0.0, 0.0, 1400.0, 600.0);
    let frame = |app: &mut App, start: f64, end: f64| {
        let App { panels, doc, .. } = app;
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
    let end = app.doc.limits().1 as f64;
    let mid = end / 2.0;
    let bars_ms = frame(&mut app, mid, mid + 2000.0);
    let density_ms = frame(&mut app, 0.0, end);
    app.panels.waves_mut(waves).unwrap().selected = [0].into();
    app.handle(Command::Action(Action::ResetRowHeight));
    assert_eq!(
        app.panels.waves(waves).unwrap().items[0].height(),
        RowHeight::DEFAULT
    );
    let folded_ms = frame(&mut app, mid, mid + 2000.0);
    println!(
        "{}",
        serde_json::json!({
            "records": n, "depth": depth, "load_ms": load_ms,
            "index_ms": index_ms - clone_ms, "bars_frame_ms": bars_ms,
            "folded_frame_ms": folded_ms, "density_frame_ms": density_ms,
        })
    );
    Ok(())
}
