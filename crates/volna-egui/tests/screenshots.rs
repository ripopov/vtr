//! Drive the egui viewer headlessly through real egui input events, assert on
//! the core state after each step, and save PNGs to `results/egui/`.
//!
//! ```text
//! cargo test -p volna-egui --test screenshots
//! ```

use std::path::PathBuf;

use egui::{Key, Modifiers, PointerButton};
use volna_egui::VolnaApp;
use volna_egui::headless::Headless;

const W: usize = 1440;
const H: usize = 900;

fn results_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("results/egui");
    std::fs::create_dir_all(&dir).expect("results dir");
    dir
}

fn shot(h: &Headless, name: &str) {
    assert!(h.is_nonblank(), "blank capture: {name}");
    let path = results_dir().join(format!("{name}.png"));
    let img = image::RgbaImage::from_raw(W as u32, H as u32, h.rgba()).expect("image");
    img.save(&path).expect("save png");
    println!("saved {}", path.display());
}

/// Run frames until the trace and every requested history has arrived.
fn wait_loads(h: &mut Headless, app: &mut VolnaApp) {
    for _ in 0..2000 {
        h.settle(app, 1);
        if app.app.doc.is_loaded() && app.app.doc.pending_count() == 0 {
            h.settle(app, 2);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("loads did not finish: {}", app.app.debug_state());
}

fn expect(app: &VolnaApp, label: &str, needles: &[&str]) {
    let state = app.app.debug_state();
    println!("STATE {label}: {state}");
    for n in needles {
        assert!(state.contains(n), "{label}: expected {n:?}, got {state}");
    }
}

#[test]
fn fst_opens_and_loads_on_frontend_executor() {
    use volna_core::app::Command;
    let mut h = Headless::new(W, H);
    let mut app = VolnaApp::new(&h.ctx);
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ext/surfer/examples/verilator/features.fst");
    app.app.open_path(fixture);
    wait_loads(&mut h, &mut app);
    let count = app.app.doc.hierarchy().unwrap().vars.len();
    assert!(count > 0);
    app.app.handle(Command::AddVars((0..count).collect()));
    wait_loads(&mut h, &mut app);
    assert_eq!(app.app.waves.items.len(), count);
    assert!(app.app.waves.items.iter().all(|row| row.history.is_some()));
    assert!(h.is_nonblank());
}

#[test]
fn viewer_interactions() {
    let mut h = Headless::new(W, H);
    let mut app = VolnaApp::new(&h.ctx);
    h.settle(&mut app, 2);
    expect(
        &app,
        "empty",
        &["items=0 loaded=0", "cursor=None", "markers=0"],
    );
    shot(&h, "01-empty");

    // Open the bundled example and add the first scope's variables with "+".
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../volna/examples/picorv32.vtr");
    app.app.open_path(example);
    wait_loads(&mut h, &mut app);
    shot(&h, "02-loaded");
    let t = app.core_theme;
    let row = t.row_height;
    let top = t.titlebar_height;
    // Second scope row: header, then rows.
    h.click(&mut app, 80.0, top + t.header_height + row + row / 2.0);
    h.settle(&mut app, 2);
    let scope_row = app.app.scopes.visible[1].0;
    assert_eq!(app.app.scopes.selected, Some(scope_row));
    let sidebar_h = H as f32 - top - t.statusbar_height;
    let scopes_h = (sidebar_h * app.app.scopes_fraction).clamp(96.0, sidebar_h - 96.0);
    // The "+" button sits at the right of the variables header.
    let plus_y = top + scopes_h + t.header_height / 2.0;
    let plus_x = app.app.sidebar_width - 16.0;
    h.click(&mut app, plus_x, plus_y);
    wait_loads(&mut h, &mut app);
    expect(&app, "after add", &["items=29 loaded=29"]);
    shot(&h, "03-signals");

    // Cursor click in the waves, select a row by name, zoom in twice, marker,
    // next edge.
    let layout = app.app.waves.last_layout().clone();
    let waves_x = layout.waves.left();
    h.click(&mut app, waves_x + 300.0, layout.row_y(3) + row / 2.0);
    expect(&app, "after wave click", &["cursor=Some("]);
    h.click(
        &mut app,
        layout.names.left() + 40.0,
        layout.row_y(5) + row / 2.0,
    );
    expect(
        &app,
        "after name click",
        &["selected={5}", "anchor=Some(5)"],
    );
    let before_zoom = app.app.debug_state();
    h.key(&mut app, Key::Equals, Modifiers::NONE);
    h.key(&mut app, Key::Equals, Modifiers::NONE);
    std::thread::sleep(std::time::Duration::from_millis(200));
    h.settle(&mut app, 4);
    let viewport = |s: &str| {
        s.split("viewport=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned()
    };
    let after_zoom = app.app.debug_state();
    assert_ne!(
        viewport(&before_zoom),
        viewport(&after_zoom),
        "zoom must change the viewport"
    );
    h.key(&mut app, Key::M, Modifiers::NONE);
    h.key(&mut app, Key::ArrowRight, Modifiers::SHIFT);
    std::thread::sleep(std::time::Duration::from_millis(200));
    h.settle(&mut app, 4);
    expect(&app, "marker added", &["markers=1"]);
    shot(&h, "04-cursor-zoom");

    // Badge click opens the format menu; escape dismisses it.
    let layout = app.app.waves.last_layout().clone();
    let badge = layout.badges.iter().find(|(ix, _)| *ix == 3).unwrap().1;
    h.move_to(&mut app, badge.left() + 4.0, badge.top() + 4.0);
    h.click(&mut app, badge.left() + 4.0, badge.top() + 4.0);
    // egui fades new areas in over its animation time; let the popup settle.
    h.settle(&mut app, 12);
    expect(&app, "after badge click", &["menu=true"]);
    shot(&h, "05-format-menu");
    h.key(&mut app, Key::Escape, Modifiers::NONE);
    expect(&app, "menu dismissed", &["menu=false"]);

    // Pinch zoom out about the pointer, then fit.
    h.move_to(&mut app, waves_x + 200.0, layout.row_y(2) + row / 2.0);
    let before_pinch = app.app.debug_state();
    h.zoom(&mut app, 0.5);
    assert_ne!(viewport(&before_pinch), viewport(&app.app.debug_state()));
    h.key(&mut app, Key::F, Modifiers::NONE);
    std::thread::sleep(std::time::Duration::from_millis(200));
    h.settle(&mut app, 4);
    assert_eq!(viewport(&before_zoom), viewport(&app.app.debug_state()));

    // Drag the sidebar sash and the names divider.
    // The panel's resize handle sits on the sidebar side of the edge.
    let sash_x = app.app.sidebar_width - 2.0;
    h.drag(&mut app, (sash_x, 400.0), (sash_x + 60.0, 420.0));
    h.settle(&mut app, 2);
    expect(
        &app,
        "after sidebar drag",
        &["sidebar_w=338px", "drag=None"],
    );
    let layout = app.app.waves.last_layout().clone();
    let split_x = layout.names.right();
    h.drag(&mut app, (split_x, 500.0), (split_x + 40.0, 500.0));
    h.settle(&mut app, 2);
    assert!(
        (app.app.waves.names_width - 260.0).abs() < 1.5,
        "{}",
        app.app.waves.names_width
    );

    // Right-drag pans.
    let layout = app.app.waves.last_layout().clone();
    let before_pan = app.app.waves.viewport;
    h.key(&mut app, Key::Equals, Modifiers::NONE);
    std::thread::sleep(std::time::Duration::from_millis(200));
    h.settle(&mut app, 4);
    let zoomed = app.app.waves.viewport;
    assert!(zoomed.width() < before_pan.width());
    let y = layout.waves.top() + 100.0;
    let a = egui::Pos2::new(layout.waves.left() + 500.0, y);
    let b = egui::Pos2::new(layout.waves.left() + 400.0, y);
    h.frame(&mut app, vec![egui::Event::PointerMoved(a)]);
    h.frame(
        &mut app,
        vec![egui::Event::PointerButton {
            pos: a,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    h.frame(&mut app, vec![egui::Event::PointerMoved(b)]);
    h.frame(
        &mut app,
        vec![egui::Event::PointerButton {
            pos: b,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(
        app.app.waves.viewport.start > zoomed.start,
        "drag left pans right"
    );

    // Typing in the variable list filters; Enter adds the matches.
    h.click(
        &mut app,
        100.0,
        top + scopes_h + t.header_height + 40.0 + row * 2.0,
    );
    let vars_focus = app.app.variables.selected.len();
    assert_eq!(vars_focus, 1, "row click selects");
    h.text(&mut app, "clk");
    h.settle(&mut app, 2);
    assert_eq!(app.app.variables.filter, "clk");
    assert!(app.app.variables.rows.len() < 29);
    shot(&h, "06-filter");
}
