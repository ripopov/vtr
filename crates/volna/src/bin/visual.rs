//! Offscreen visual check and frame-time benchmark (macOS, Metal headless).
//!
//! ```text
//! cargo run -p volna --profile viewer --features visual-test --bin volna-visual -- [--out DIR]
//! ```
//! Renders the workspace with a real trace, exercises the interactions, saves
//! PNG screenshots and prints per-frame paint times for synthetic traces of
//! 10K, 1M and 100M transitions.

#[cfg(all(feature = "visual-test", not(target_family = "wasm")))]
fn main() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Instant;

    use gpui::{
        AppContext, HeadlessAppContext, Keystroke, Modifiers, MouseButton, Pixels, Point, px, size,
    };
    use volna::app::Workspace;
    use volna::assets::Assets;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = std::path::PathBuf::from("results");
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--out" {
            out = args[i + 1].clone().into();
            i += 1;
        }
        i += 1;
    }
    std::fs::create_dir_all(&out)?;

    let platform = gpui_platform::current_platform(true);
    let mut test = HeadlessAppContext::with_platform(
        platform.text_system(),
        Arc::new(Assets),
        gpui_platform::current_headless_renderer,
    );
    test.update(volna::init_app);

    let win_size = size(px(1440.0), px(900.0));
    let mut workspace: Option<gpui::Entity<Workspace>> = None;
    let handle = test.open_window(win_size, |window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        ws
    })?;
    let workspace = workspace.unwrap();
    let any = handle.into();

    let state = |test: &mut HeadlessAppContext, label: &str| {
        let s = test.update(|cx| workspace.read(cx).debug_state(cx));
        println!("STATE {label}: {s}");
    };
    let settle = |test: &mut HeadlessAppContext, frames: usize| {
        for _ in 0..frames {
            test.run_until_parked();
            test.update_window(any, |_, w, cx| {
                let _ = w.draw(cx);
            })
            .ok();
        }
    };
    let shot = |test: &mut HeadlessAppContext, name: &str| -> anyhow::Result<()> {
        settle(test, 2);
        let img = test.capture_screenshot(any)?;
        let path = out.join(format!("{name}.png"));
        img.save(&path)?;
        println!(
            "saved {} ({}x{})",
            path.display(),
            img.width(),
            img.height()
        );
        Ok(())
    };
    let click = |test: &mut HeadlessAppContext, p: Point<Pixels>, mods: Modifiers| {
        test.update_window(any, |_, w, cx| {
            w.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: p,
                    pressed_button: None,
                    modifiers: mods,
                }),
                cx,
            );
            w.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: MouseButton::Left,
                    position: p,
                    modifiers: mods,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            w.dispatch_event(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: MouseButton::Left,
                    position: p,
                    modifiers: mods,
                    click_count: 1,
                }),
                cx,
            );
        })
        .ok();
        test.run_until_parked();
    };
    let key = |test: &mut HeadlessAppContext, k: &str| {
        test.update_window(any, |_, w, cx| {
            w.dispatch_keystroke(Keystroke::parse(k).unwrap(), cx);
        })
        .ok();
        test.run_until_parked();
    };

    let drag = |test: &mut HeadlessAppContext, from: Point<Pixels>, to: Point<Pixels>| {
        test.update_window(any, |_, w, cx| {
            let mods = Modifiers::default();
            w.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: from,
                    pressed_button: None,
                    modifiers: mods,
                }),
                cx,
            );
            w.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    modifiers: mods,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
        })
        .ok();
        test.run_until_parked();
        let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
        for p in [mid, to] {
            test.update_window(any, |_, w, cx| {
                w.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: p,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                let _ = w.draw(cx);
            })
            .ok();
            test.run_until_parked();
        }
        test.update_window(any, |_, w, cx| {
            w.dispatch_event(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: MouseButton::Left,
                    position: to,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        })
        .ok();
        test.run_until_parked();
    };

    // 1. Empty state.
    shot(&mut test, "01-empty")?;
    // Splitter drag: grab the sidebar sash and release over the centre panel.
    drag(
        &mut test,
        Point::new(px(280.0), px(400.0)),
        Point::new(px(340.0), px(420.0)),
    );
    state(&mut test, "after sidebar drag");
    // Move the pointer afterwards; the panel must follow no further.
    click(
        &mut test,
        Point::new(px(700.0), px(500.0)),
        Modifiers::default(),
    );
    state(&mut test, "after click elsewhere");
    drag(
        &mut test,
        Point::new(px(340.0), px(340.0)),
        Point::new(px(280.0), px(520.0)),
    );
    state(&mut test, "sidebar drag back");
    // Horizontal sash between scopes and variables (at 42% of the sidebar height).
    let sash_y = 32.0 + (900.0 - 32.0 - 24.0) * 0.42;
    drag(
        &mut test,
        Point::new(px(150.0), px(sash_y)),
        Point::new(px(600.0), px(300.0)),
    );
    state(&mut test, "after scopes sash drag");
    drag(
        &mut test,
        Point::new(px(150.0), px(300.0)),
        Point::new(px(150.0), px(sash_y)),
    );
    state(&mut test, "scopes sash back");
    shot(&mut test, "01b-after-splitter-drag")?;

    // 2. Load the sample trace and add signals from the first scope.
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/picorv32.vtr");
    test.update(|cx| workspace.update(cx, |ws, cx| ws.open_path(example, cx)));
    settle(&mut test, 3);
    shot(&mut test, "02-loaded")?;
    // Click the second scope row (row height 24, header 32, titlebar 32, list padding 4).
    click(
        &mut test,
        Point::new(px(80.0), px(32.0 + 32.0 + 4.0 + 24.0 + 12.0)),
        Modifiers::default(),
    );
    settle(&mut test, 2);
    // Add all variables of the selected scope with the header "+" button.
    let sidebar_h = 900.0 - 32.0 - 24.0;
    let scopes_h = sidebar_h * 0.42;
    click(
        &mut test,
        Point::new(px(280.0 - 12.0 - 12.0), px(32.0 + scopes_h + 16.0)),
        Modifiers::default(),
    );
    settle(&mut test, 4);
    state(&mut test, "after add");
    shot(&mut test, "03-signals")?;

    // 3. Cursor click in the waves, zoom in twice, select a row, open the format menu.
    let waves_x = 280.0 + 220.0 + 120.0;
    click(
        &mut test,
        Point::new(px(waves_x + 300.0), px(32.0 + 32.0 + 24.0 * 3.0 + 12.0)),
        Modifiers::default(),
    );
    state(&mut test, "after wave click");
    // Select a row by clicking its name.
    click(
        &mut test,
        Point::new(px(400.0), px(32.0 + 32.0 + 24.0 * 5.0 + 12.0)),
        Modifiers::default(),
    );
    state(&mut test, "after name click");
    key(&mut test, "=");
    key(&mut test, "=");
    settle(&mut test, 12);
    key(&mut test, "m");
    key(&mut test, "shift-right");
    settle(&mut test, 12);
    shot(&mut test, "04-cursor-zoom")?;
    // Badge click (values column right edge).
    click(
        &mut test,
        Point::new(
            px(280.0 + 220.0 + 120.0 - 24.0),
            px(32.0 + 32.0 + 24.0 * 3.0 + 12.0),
        ),
        Modifiers::default(),
    );
    settle(&mut test, 2);
    state(&mut test, "after badge click");
    shot(&mut test, "05-format-menu")?;
    key(&mut test, "escape");
    key(&mut test, "f");
    settle(&mut test, 12);

    // 4. Frame-time benchmark: synthetic traces, identical viewport and window.
    for n in [10_000usize, 1_000_000, 100_000_000] {
        test.update(|cx| workspace.update(cx, |ws, cx| ws.open_synthetic(n, cx)));
        settle(&mut test, 4);
        key(&mut test, "f");
        settle(&mut test, 12);
        // Pan a little each frame so every frame is a real repaint.
        let mut samples = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            test.update_window(any, |_, w, cx| {
                w.dispatch_keystroke(Keystroke::parse("right").unwrap(), cx);
                let _ = w.draw(cx);
            })
            .ok();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = samples[samples.len() / 2];
        let p90 = samples[samples.len() * 9 / 10];
        let paint_ms = test.update(|cx| workspace.read(cx).waves_frame_ms(cx));
        println!(
            "PERF transitions={n} frame_median_ms={median:.2} frame_p90_ms={p90:.2} table_paint_ms={paint_ms:.2}"
        );
        // Zoomed-in view as well.
        for _ in 0..6 {
            key(&mut test, "=");
        }
        settle(&mut test, 12);
        let mut samples = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            test.update_window(any, |_, w, cx| {
                w.dispatch_keystroke(Keystroke::parse("right").unwrap(), cx);
                let _ = w.draw(cx);
            })
            .ok();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "PERF transitions={n} zoomed frame_median_ms={:.2} frame_p90_ms={:.2}",
            samples[samples.len() / 2],
            samples[samples.len() * 9 / 10]
        );
        if n == 100_000_000 {
            shot(&mut test, "06-synthetic-100m")?;
        }
    }
    Ok(())
}

#[cfg(not(all(feature = "visual-test", not(target_family = "wasm"))))]
fn main() {
    eprintln!("build with --features visual-test on macOS");
}
