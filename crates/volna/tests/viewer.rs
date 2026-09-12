//! Whole-viewer interactions and optional screenshot capture (macOS, Metal headless).
//!
//! ```text
//! cargo test -p volna --features visual-test --test viewer
//! ```
//! Set VOLNA_SCREENSHOTS to an output directory to save PNGs. Timing measurements
//! are a separate ignored test; no machine-dependent performance thresholds.

#[cfg(target_os = "macos")]
mod theme_fixtures {
    use volna::theme::{Appearance, ColorPair, HostPalette};
    include!("fixtures/vscode/palettes.rs");
    pub fn all() -> [HostPalette; 4] {
        real_palettes()
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use libtest_mimic::{Arguments, Trial};
    let mut args = Arguments::from_args();
    // AppKit requires initialization on the process main thread.
    args.test_threads = Some(1);
    libtest_mimic::run(
        &args,
        vec![
            Trial::test("viewer_interactions", || {
                run(false).map_err(|e| format!("{e:#}").into())
            }),
            Trial::test("frame_times", || {
                run(true).map_err(|e| format!("{e:#}").into())
            })
            .with_ignored_flag(true),
        ],
    )
    .exit();
}

#[cfg(target_os = "macos")]
fn run(measure: bool) -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Instant;

    use gpui::{
        AppContext, HeadlessAppContext, Keystroke, Modifiers, MouseButton, Pixels, Point, px, size,
    };
    use volna::app::Workspace;
    use volna::assets::Assets;

    let out = std::env::var_os("VOLNA_SCREENSHOTS").map(std::path::PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out)?;
    }

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
        s
    };
    let expect = |test: &mut HeadlessAppContext, label: &str, expected: &[&str]| {
        let actual = state(test, label);
        for expected in expected {
            assert!(
                actual.contains(expected),
                "{label}: expected {expected:?}, got {actual}"
            );
        }
    };
    let settle = |test: &mut HeadlessAppContext, frames: usize| {
        for _ in 0..frames {
            test.run_until_parked();
            test.update_window(any, |_, w, cx| {
                let _ = w.draw(cx);
            })
            .expect("draw window");
        }
    };
    let shot = |test: &mut HeadlessAppContext, name: &str| -> anyhow::Result<()> {
        settle(test, 2);
        let img = test.capture_screenshot(any)?;
        assert!(img.width() > 0 && img.height() > 0, "empty capture: {name}");
        let first = img.get_pixel(0, 0);
        assert!(
            img.pixels().any(|pixel| pixel != first),
            "blank capture: {name}"
        );
        if let Some(out) = &out {
            let path = out.join(format!("{name}.png"));
            img.save(&path)?;
            println!("saved {}", path.display());
        }
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
        .expect("click window");
        test.run_until_parked();
    };
    let key = |test: &mut HeadlessAppContext, k: &str| {
        test.update_window(any, |_, w, cx| {
            w.dispatch_keystroke(Keystroke::parse(k).unwrap(), cx);
        })
        .expect("dispatch key");
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
        .expect("start drag");
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
            .expect("move drag");
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
        .expect("release drag");
        test.run_until_parked();
    };

    if !measure {
        // 1. Empty state.
        expect(
            &mut test,
            "empty",
            &["items=0 loaded=0", "cursor=None", "markers=0"],
        );
        shot(&mut test, "01-empty")?;
        // Splitter drag: grab the sidebar sash and release over the centre panel.
        drag(
            &mut test,
            Point::new(px(280.0), px(400.0)),
            Point::new(px(340.0), px(420.0)),
        );
        expect(
            &mut test,
            "after sidebar drag",
            &["sidebar_w=340px", "drag=None"],
        );
        // Move the pointer afterwards; the panel must follow no further.
        click(
            &mut test,
            Point::new(px(700.0), px(500.0)),
            Modifiers::default(),
        );
        expect(
            &mut test,
            "after click elsewhere",
            &["sidebar_w=340px", "drag=None"],
        );
        drag(
            &mut test,
            Point::new(px(340.0), px(340.0)),
            Point::new(px(280.0), px(520.0)),
        );
        expect(
            &mut test,
            "sidebar drag back",
            &["sidebar_w=280px", "drag=None"],
        );
        // Horizontal sash between scopes and variables (at 42% of the sidebar height).
        let sash_y = 32.0 + (900.0 - 32.0 - 24.0) * 0.42;
        drag(
            &mut test,
            Point::new(px(150.0), px(sash_y)),
            Point::new(px(600.0), px(300.0)),
        );
        expect(
            &mut test,
            "after scopes sash drag",
            &["scopes_frac=0.32", "drag=None"],
        );
        drag(
            &mut test,
            Point::new(px(150.0), px(300.0)),
            Point::new(px(150.0), px(sash_y)),
        );
        expect(
            &mut test,
            "scopes sash back",
            &["scopes_frac=0.42", "drag=None"],
        );
        shot(&mut test, "01b-after-splitter-drag")?;

        // 2. Load the sample trace and add signals from the first scope.
        let example =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/picorv32.vtr");
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
        expect(&mut test, "after add", &["items=29 loaded=29"]);
        shot(&mut test, "03-signals")?;

        // 3. Cursor click in the waves, zoom in twice, select a row, open the format menu.
        let waves_x = 280.0 + 220.0 + 120.0;
        click(
            &mut test,
            Point::new(px(waves_x + 300.0), px(32.0 + 32.0 + 24.0 * 3.0 + 12.0)),
            Modifiers::default(),
        );
        expect(&mut test, "after wave click", &["cursor=Some("]);
        // Select a row by clicking its name.
        click(
            &mut test,
            Point::new(px(400.0), px(32.0 + 32.0 + 24.0 * 5.0 + 12.0)),
            Modifiers::default(),
        );
        expect(
            &mut test,
            "after name click",
            &["selected={5}", "anchor=Some(5)"],
        );
        let before_zoom = state(&mut test, "before zoom");
        key(&mut test, "=");
        key(&mut test, "=");
        settle(&mut test, 12);
        let after_zoom = state(&mut test, "after zoom");
        let viewport = |s: &str| {
            s.split("viewport=")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        };
        assert_ne!(
            viewport(&before_zoom),
            viewport(&after_zoom),
            "zoom must change the viewport"
        );
        key(&mut test, "m");
        key(&mut test, "shift-right");
        settle(&mut test, 12);
        expect(&mut test, "marker added", &["markers=1"]);
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
        expect(&mut test, "after badge click", &["menu=true"]);
        shot(&mut test, "05-format-menu")?;
        // Replace only the theme while trace, selection, zoom, cursor, marker and
        // format menu are active. Cached GPUI children must repaint too.
        let before_theme = state(&mut test, "before themes");
        use volna::theme::{Appearance::*, ColorPair, HostPalette, Theme};
        for (name, appearance) in [
            ("06-light", Light),
            ("07-dark", Dark),
            ("08-hc-dark", HighContrastDark),
            ("09-hc-light", HighContrastLight),
            ("10-custom", Light),
        ] {
            let mut palette = HostPalette {
                appearance,
                ..Default::default()
            };
            if name == "10-custom" {
                let pair = |bg, fg| ColorPair {
                    background: Some(gpui::rgb(bg).into()),
                    foreground: Some(gpui::rgb(fg).into()),
                };
                palette.editor = pair(0xfff4e6, 0x321800);
                palette.panel = pair(0x203040, 0xf0f8ff);
                palette.bar = pair(0x005fb8, 0xffffff);
            }
            test.update(|cx| Theme::from_host(&palette).install(cx));
            shot(&mut test, name)?;
            assert_eq!(
                state(&mut test, name),
                before_theme,
                "theme must preserve viewer state"
            );
        }
        for (name, palette) in [
            "11-vscode-dark",
            "12-vscode-light",
            "13-vscode-hc-dark",
            "14-vscode-hc-light",
        ]
        .into_iter()
        .zip(theme_fixtures::all())
        {
            test.update(|cx| Theme::from_host(&palette).install(cx));
            shot(&mut test, name)?;
            assert_eq!(state(&mut test, name), before_theme);
        }
        test.update(|cx| volna::theme::Theme::one_dark().install(cx));
        key(&mut test, "escape");
        expect(&mut test, "menu dismissed", &["menu=false"]);
        key(&mut test, "f");
        settle(&mut test, 12);
        let fitted = state(&mut test, "fit");
        assert_eq!(
            viewport(&before_zoom),
            viewport(&fitted),
            "fit must restore full trace range"
        );
        return Ok(());
    }

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
            .expect("benchmark draw");
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
            .expect("benchmark draw");
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

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("viewer integration tests skipped: offscreen renderer requires macOS");
}
