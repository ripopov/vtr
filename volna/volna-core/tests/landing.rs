//! The landing-page demo (docs/volna-landing.html) restores its checked-in
//! workspace over its checked-in trace without a notice, shows every panel
//! kind with data, and saves back to the same bytes. Regenerate both with
//! `core/vtr/examples/landing.rs` and `examples/landing_workspace.rs`.

use std::path::PathBuf;

use volna_core::geometry::Rect;
use volna_core::panels::PanelId;
use volna_core::pipeline::Rows;
use volna_core::scene::MonoMeasure;
use volna_core::transaction::TxPanelState;
use volna_core::workspace::Workspace;
use volna_core::{App, Instant, Theme};
use volna_trace::session::OpenSpec;

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples")
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
}

/// The landing trace with its checked-in workspace restored.
fn landing() -> App {
    let trace = examples().join("landing.vtr").canonicalize().unwrap();
    let saved = std::fs::read(examples().join("landing.vtr.volna.json")).unwrap();
    let trace_uri = url::Url::from_file_path(&trace).unwrap().to_string();
    let location = format!("{trace_uri}.volna.json");
    let session = OpenSpec::Path(trace.clone()).open().unwrap();
    let mut app = App::new();
    app.open_resource(OpenSpec::Path(trace), trace_uri.clone());
    volna_core::testing::complete_open(&mut app, session);
    Workspace::parse(&saved)
        .unwrap()
        .prepare(&app, &trace_uri, &location)
        .unwrap()
        .commit(&mut app)
        .unwrap();
    pump(&mut app);
    app
}

/// docs/volna-theme.html, "Waveform panel rendering", over the whole landing
/// run in Volna Dark: clocks show periods, states get a tint, numbers keep
/// their last digits, and labels never shrink to a lone ellipsis. X bands
/// and gated clocks are covered by the painter's unit tests.
#[test]
fn the_whole_landing_run_follows_the_rendering_rules() {
    let mut app = landing();
    let theme = Theme::volna(true);
    let bounds = Rect::from_xywh(0.0, 0.0, 1200.0, 640.0);
    let (first, last) = app.doc.limits();
    let render = |app: &mut App| {
        app.layout_panel(PanelId(1), bounds, &theme).unwrap();
        app.render_panel(PanelId(1), &theme, &mut MonoMeasure);
        pump(app);
        app.layout_panel(PanelId(1), bounds, &theme).unwrap();
        let scene = app.render_panel(PanelId(1), &theme, &mut MonoMeasure);
        let texts: Vec<String> = scene.texts().map(str::to_owned).collect();
        let quads: Vec<_> = scene.quads().collect();
        (texts, quads)
    };
    app.doc.shared.viewport.value.start = first as f64;
    app.doc.shared.viewport.value.end = last as f64;
    let (texts, quads) = render(&mut app);
    assert!(
        texts.iter().any(|t| t.contains("Hz · ")),
        "a fast clock is a band labelled with its frequency and period: {texts:?}"
    );
    assert!(
        quads.iter().any(|(_, c)| (c.a - 0.22).abs() < 1e-3),
        "state rows are tinted"
    );
    assert!(
        texts.iter().any(|t| t.starts_with('…') && t.len() > 3),
        "a number keeps its last digits: {texts:?}"
    );
    for text in &texts {
        let shown = text.trim_matches('…').chars().count();
        assert!(!text.contains('…') || shown >= 2, "lone ellipsis {text:?}");
    }
    // The interrupt window: the fast clock is a square wave again.
    app.doc.shared.viewport.value.start = 717.0;
    app.doc.shared.viewport.value.end = 849.0;
    let (texts, _) = render(&mut app);
    assert!(
        texts
            .iter()
            .all(|t| !t.contains("Hz · ") || !t.starts_with("500")),
        "{texts:?}"
    );
}

#[test]
fn the_landing_workspace_restores_every_panel_and_saves_the_same_bytes() {
    let trace = examples().join("landing.vtr").canonicalize().unwrap();
    let saved = std::fs::read(examples().join("landing.vtr.volna.json")).unwrap();
    let trace_uri = url::Url::from_file_path(&trace).unwrap().to_string();
    let location = format!("{trace_uri}.volna.json");
    let session = OpenSpec::Path(trace.clone()).open().unwrap();
    let mut app = App::new();
    app.open_resource(OpenSpec::Path(trace), trace_uri.clone());
    volna_core::testing::complete_open(&mut app, session);

    let plan = Workspace::parse(&saved)
        .unwrap()
        .prepare(&app, &trace_uri, &location)
        .unwrap();
    assert_eq!(plan.report().notices, Vec::<String>::new());
    plan.commit(&mut app).unwrap();
    pump(&mut app);
    let theme = Theme::one_dark();
    let bounds = Rect::from_xywh(0.0, 0.0, 900.0, 420.0);
    for id in [1, 2, 4] {
        app.layout_panel(PanelId(id), bounds, &theme).unwrap();
        app.render_panel(PanelId(id), &theme, &mut MonoMeasure);
    }
    pump(&mut app);
    app.tick(Instant::now());

    let waves = app.panels.waves(PanelId(1)).unwrap();
    let signals = waves
        .items()
        .iter()
        .filter(|i| i.signal().is_some())
        .count();
    assert_eq!(signals, 16);
    assert_eq!(waves.loaded_count(), signals, "every signal row loaded");
    app.layout_panel(PanelId(1), bounds, &theme).unwrap();
    let texts: Vec<String> = app
        .render_panel(PanelId(1), &theme, &mut MonoMeasure)
        .texts()
        .map(str::to_owned)
        .collect();
    for text in ["cpu0", "l2 port", "perf", "read", "burst", "dram_clk"] {
        assert!(
            texts.iter().any(|t| t == text),
            "the waves panel shows {text:?}: {texts:?}"
        );
    }
    match app.panels.pipeline(PanelId(2)).unwrap().rows(&app.doc) {
        Rows::Ready(rows) => assert_eq!(rows.len(), 420),
        _ => panic!("the pipeline rows are not ready"),
    }
    let tx = app.panels.transaction(PanelId(3)).unwrap();
    assert!(tx.pinned());
    match tx.state(&app.doc) {
        TxPanelState::Ready(view) => {
            assert!(
                view.identity
                    .label
                    .as_deref()
                    .unwrap()
                    .ends_with("lw   t1, 64(gp)")
            );
            assert!(
                view.related.total > 0,
                "the device read has related records"
            );
        }
        _ => panic!("the pinned record is not ready"),
    }
    assert_eq!(
        app.panels
            .get(PanelId(4))
            .unwrap()
            .kind
            .table()
            .unwrap()
            .len(),
        30
    );
    let markers: Vec<_> = app
        .doc
        .markers()
        .iter()
        .map(|m| m.label.as_deref())
        .collect();
    assert_eq!(markers, [Some("irq"), Some("refill done")]);

    let again = Workspace::capture(&app, volna_core::testing::paths("landing.vtr"), None).unwrap();
    assert_eq!(
        String::from_utf8(again.to_bytes().unwrap()).unwrap(),
        String::from_utf8(saved).unwrap(),
        "rerun examples/landing_workspace.rs"
    );
}
