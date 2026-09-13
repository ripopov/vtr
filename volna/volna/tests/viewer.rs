//! Whole-viewer interactions and optional screenshot capture (macOS, Metal headless).
//!
//! ```text
//! cargo test -p volna --features visual-test --test viewer
//! ```
//! Set VOLNA_SCREENSHOTS to an output directory to save PNGs. Timing measurements
//! are a separate ignored test; no machine-dependent performance thresholds.

#[cfg(target_os = "macos")]
mod theme_fixtures {
    use volna::theme::{HostPalette, vscode};
    include!("../../volna-core/tests/fixtures/themes.rs");
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

    use gpui_kit::{
        AppContext, HeadlessAppContext, Keystroke, Modifiers, MouseButton, Pixels, Point, px, size,
    };
    use volna::app::Workspace;
    use volna::assets::Assets;

    let out = std::env::var_os("VOLNA_SCREENSHOTS").map(std::path::PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out)?;
    }

    let platform = gpui_kit::platform::current_platform(true);
    let mut test = HeadlessAppContext::with_platform(
        platform.text_system(),
        Arc::new(Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    test.update(volna::init_app);

    let win_size = size(px(1440.0), px(900.0));
    let mut workspace: Option<gpui_kit::Entity<Workspace>> = None;
    let handle = test.open_window(win_size, |window, cx| {
        let ws = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(ws.clone());
        cx.new(|cx| gpui_kit::component::Root::new(ws, window, cx))
    })?;
    let workspace = workspace.unwrap();
    let any = handle.into();

    let state = |test: &mut HeadlessAppContext, label: &str| {
        let s = test.update(|cx| workspace.read(cx).debug_state());
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
                gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                    position: p,
                    pressed_button: None,
                    modifiers: mods,
                }),
                cx,
            );
            w.dispatch_event(
                gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                    button: MouseButton::Left,
                    position: p,
                    modifiers: mods,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            w.dispatch_event(
                gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
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
                gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                    position: from,
                    pressed_button: None,
                    modifiers: mods,
                }),
                cx,
            );
            w.dispatch_event(
                gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
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
                    gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
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
                gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
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
        // A saved split can arrive before DockArea has ever measured bounds.
        // Verify the deferred install produces painted panels and live focus.
        test.update(|cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_session(Arc::new(volna_core::data::synth::SynthSource::new(100)), cx);
                ws.dispatch(volna_core::Command::AddVars(vec![0]), None, cx);
                ws.dispatch(
                    volna_core::Command::Action(volna_core::Action::SplitRight),
                    None,
                    cx,
                );
            });
        });
        settle(&mut test, 8);
        test.update(|cx| {
            for panel in workspace.read(cx).app.panels.iter() {
                let waves = panel.kind.waves().unwrap();
                assert!(
                    waves.frames_painted > 0,
                    "initial split must paint without input"
                );
                assert!(waves.last_layout().bounds.width() > 100.0);
            }
        });
        let initial_width =
            test.update(|cx| workspace.read(cx).app.doc.shared.viewport.target().width());
        key(&mut test, "=");
        settle(&mut test, 12);
        assert!(test.update(
            |cx| workspace.read(cx).app.doc.shared.viewport.target().width() < initial_width
        ));
        shot(&mut test, "dock-initial-split")?;
        test.update(|cx| {
            workspace.update(cx, |ws, cx| {
                ws.dispatch(volna_core::Command::CloseTrace, None, cx)
            })
        });
        settle(&mut test, 4);
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
        let custom = volna::theme::HostPalette::from_json(include_str!(
            "../../volna-core/tests/fixtures/custom-palette.json"
        ))?;
        test.update(|cx| volna::theme::install(volna::theme::CoreTheme::from_host(&custom), cx));
        shot(&mut test, "02b-custom-empty")?;
        test.update(|cx| volna::theme::install(volna::theme::CoreTheme::one_dark(), cx));
        // Click the second scope row (row height 24, header 32, titlebar 32, list padding 4).
        click(
            &mut test,
            Point::new(px(80.0), px(32.0 + 32.0 + 4.0 + 24.0 + 12.0)),
            Modifiers::default(),
        );
        settle(&mut test, 2);
        // Filtering remains custom on both native and web. Each character
        // must reach the core once, even with the variable-list key handler.
        let filter_y = 32.0 + (900.0 - 32.0 - 24.0) * 0.42 + 32.0 + 16.0;
        click(
            &mut test,
            Point::new(px(100.0), px(filter_y)),
            Modifiers::default(),
        );
        settle(&mut test, 2);
        for k in ["c", "l", "k"] {
            key(&mut test, k);
        }
        settle(&mut test, 2);
        assert_eq!(
            test.update(|cx| workspace.read(cx).app.variables.filter.clone()),
            "clk"
        );
        shot(&mut test, "02c-filter")?;
        key(&mut test, "escape");
        settle(&mut test, 2);
        assert_eq!(
            test.update(|cx| workspace.read(cx).app.variables.filter.clone()),
            ""
        );
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
        let before_zoom_image = test.capture_screenshot(any)?;
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
        let zoom_image = test.capture_screenshot(any)?;
        let scale = zoom_image.width() as f32 / 1440.0;
        assert!(
            (100..500).any(|y| (650..1400).any(|x| {
                let x = (x as f32 * scale) as u32;
                let y = (y as f32 * scale) as u32;
                before_zoom_image.get_pixel(x, y) != zoom_image.get_pixel(x, y)
            })),
            "zoom and marker edits must invalidate the dock's cached waveform canvas"
        );
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
        use volna::theme::{Appearance::*, CoreTheme, HostPalette, install};
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
                palette = HostPalette::from_json(include_str!(
                    "../../volna-core/tests/fixtures/custom-palette.json"
                ))?;
            }
            test.update(|cx| install(CoreTheme::from_host(&palette), cx));
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
            test.update(|cx| install(CoreTheme::from_host(&palette), cx));
            shot(&mut test, name)?;
            assert_eq!(state(&mut test, name), before_theme);
        }
        test.update(|cx| volna::theme::install(volna::theme::CoreTheme::one_dark(), cx));
        // Navigate and choose a translator through the component menu.
        let (menu_row, translator) = test.update(|cx| {
            let menu = workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .menu
                .as_ref()
                .unwrap();
            (menu.row, menu.items[1].id.clone())
        });
        key(&mut test, "down");
        key(&mut test, "down");
        key(&mut test, "enter");
        settle(&mut test, 2);
        expect(&mut test, "menu selection", &["menu=false"]);
        assert_eq!(
            test.update(
                |cx| workspace.read(cx).app.panels.focused_waves().unwrap().items[menu_row]
                    .translator
                    .id()
                    .to_owned()
            ),
            translator
        );
        click(
            &mut test,
            Point::new(px(596.0), px(64.0 + 24.0 * 3.0 + 12.0)),
            Modifiers::default(),
        );
        settle(&mut test, 2);
        expect(&mut test, "menu reopened", &["menu=true"]);
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
        // Dock geometry and keyboard actions use the production adapter.
        let single_bounds = test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .last_layout()
                .bounds
        });
        assert_eq!(
            single_bounds.top(),
            32.0,
            "one panel has no dock header height"
        );
        key(&mut test, "cmd-\\");
        settle(&mut test, 8);
        test.update(|cx| {
            let panels = &workspace.read(cx).app.panels;
            assert_eq!(panels.len(), 2);
            panels.validate().unwrap();
            let visible = panels.layout().visible();
            let a = panels.waves(visible[0]).unwrap().last_layout().bounds;
            let b = panels.waves(visible[1]).unwrap().last_layout().bounds;
            assert!(a.width() > 100.0 && b.width() > 100.0);
            assert!(a.right() <= b.left() + 2.0);
            assert!(
                a.top() > single_bounds.top(),
                "split panels expose a tab strip"
            );
        });
        shot(&mut test, "dock-split-right")?;
        key(&mut test, "l");
        assert!(!test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .link
                .viewport
        }));
        key(&mut test, "cmd-shift-\\");
        settle(&mut test, 8);
        assert_eq!(test.update(|cx| workspace.read(cx).app.panels.len()), 3);
        shot(&mut test, "dock-split-down")?;
        key(&mut test, "cmd-n");
        settle(&mut test, 8);
        assert_eq!(test.update(|cx| workspace.read(cx).app.panels.len()), 4);
        assert!(test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .items
                .is_empty()
        }));
        shot(&mut test, "dock-new-tab")?;
        let new_tab_bounds = test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .last_layout()
                .bounds
        });
        let menu_position = Point::new(
            px(new_tab_bounds.right() - 14.0),
            px(new_tab_bounds.top() - 16.0),
        );
        click(&mut test, menu_position, Modifiers::default());
        settle(&mut test, 3);
        click(
            &mut test,
            Point::new(menu_position.x - px(65.0), menu_position.y + px(138.0)),
            Modifiers::default(),
        );
        settle(&mut test, 3);
        for character in ["d", "e", "c", "o", "d", "e"] {
            key(&mut test, character);
        }
        shot(&mut test, "dock-rename-dialog")?;
        key(&mut test, "enter");
        settle(&mut test, 4);
        assert_eq!(
            test.update(|cx| workspace.read(cx).app.panels.focused().title.clone()),
            Some("decode".into())
        );
        let (before_sash, sash) = test.update(|cx| {
            let p = &workspace.read(cx).app.panels;
            let first = p
                .waves(p.layout().visible()[0])
                .unwrap()
                .last_layout()
                .bounds;
            (
                p.layout().clone(),
                Point::new(px(first.right() + 1.0), px(first.top() + 100.0)),
            )
        });
        drag(&mut test, sash, Point::new(sash.x + px(75.0), sash.y));
        settle(&mut test, 6);
        assert_ne!(
            test.update(|cx| workspace.read(cx).app.panels.layout().clone()),
            before_sash,
            "a dock sash drag updates core fractions"
        );
        let (layout_before_zoom, bounds_before_zoom) = test.update(|cx| {
            let p = &workspace.read(cx).app.panels;
            (
                p.layout().clone(),
                p.focused_waves().unwrap().last_layout().bounds,
            )
        });
        key(&mut test, "shift-escape");
        settle(&mut test, 6);
        test.update(|cx| {
            let p = &workspace.read(cx).app.panels;
            assert_eq!(*p.layout(), layout_before_zoom, "dock zoom is transient");
            assert!(
                p.focused_waves().unwrap().last_layout().bounds.width()
                    > bounds_before_zoom.width()
            );
        });
        shot(&mut test, "dock-zoom-group")?;
        key(&mut test, "shift-escape");
        settle(&mut test, 6);
        let previous_focus = test.update(|cx| workspace.read(cx).app.panels.focused_id());
        key(&mut test, "cmd-1");
        settle(&mut test, 4);
        test.update(|cx| {
            let p = &workspace.read(cx).app.panels;
            assert_eq!(p.focused_id(), p.layout().panels()[0]);
        });
        test.update(|cx| {
            workspace.update(cx, |ws, cx| {
                ws.dispatch(
                    volna_core::Command::Panels(volna_core::panels::PanelsCommand::Focus(
                        previous_focus,
                    )),
                    None,
                    cx,
                )
            })
        });
        settle(&mut test, 4);
        let (tab_from, drop_to, before_drop) = test.update(|cx| {
            let p = &workspace.read(cx).app.panels;
            let source = p.focused_waves().unwrap().last_layout().bounds;
            let first = p
                .waves(p.layout().visible()[0])
                .unwrap()
                .last_layout()
                .bounds;
            (
                Point::new(px(source.left() + 120.0), px(source.top() - 15.0)),
                Point::new(
                    px(first.left() + first.width() / 2.0),
                    px(first.bottom() - 15.0),
                ),
                p.layout().clone(),
            )
        });
        drag(&mut test, tab_from, drop_to);
        settle(&mut test, 8);
        test.update(|cx| {
            let p = &workspace.read(cx).app.panels;
            assert_eq!(p.len(), 4, "tab moves do not destroy panel content");
            assert_ne!(
                p.layout(),
                &before_drop,
                "dragging a tab to a panel edge must split there"
            );
            p.validate().unwrap();
        });
        shot(&mut test, "dock-drag-to-split")?;
        let before_resize = test.update(|cx| workspace.read(cx).app.panels.layout().clone());
        test.update_window(any, |_, w, _| w.resize(size(px(1680.0), px(1000.0))))?;
        settle(&mut test, 8);
        assert_eq!(
            test.update(|cx| workspace.read(cx).app.panels.layout().clone()),
            before_resize,
            "window resizing must not edit split fractions"
        );
        test.update_window(any, |_, w, _| w.resize(win_size))?;
        settle(&mut test, 8);
        let before_cycle = test.update(|cx| workspace.read(cx).app.panels.focused_id());
        key(&mut test, "ctrl-tab");
        settle(&mut test, 4);
        assert_ne!(
            test.update(|cx| workspace.read(cx).app.panels.focused_id()),
            before_cycle
        );
        for _ in 0..3 {
            key(&mut test, "cmd-w");
            settle(&mut test, 6);
        }
        test.update(|cx| {
            let panels = &workspace.read(cx).app.panels;
            assert_eq!(panels.len(), 1);
            panels.validate().unwrap();
            assert_eq!(
                panels.focused_waves().unwrap().last_layout().bounds,
                single_bounds
            );
        });
        shot(&mut test, "dock-single-again")?;
        // Exercise byte input through the GPUI executor (also used by web hosts).
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../ext/surfer/examples/verilator/features.fst");
        test.update(|cx| {
            workspace.update(cx, |ws, cx| {
                ws.open_bytes("features.fst".into(), std::fs::read(&fixture).unwrap(), cx);
            })
        });
        settle(&mut test, 4);
        let count = test.update(|cx| workspace.read(cx).app.doc.hierarchy().unwrap().vars.len());
        assert!(count > 0);
        // A newly loaded trace owns keyboard focus without requiring a click
        // into its replacement dock panel.
        let fitted_width = test.update(|cx| {
            let app = &workspace.read(cx).app;
            app.panels
                .focused_waves()
                .unwrap()
                .viewport(&app.doc)
                .width()
        });
        key(&mut test, "=");
        settle(&mut test, 20);
        test.update(|cx| {
            let app = &workspace.read(cx).app;
            assert!(
                app.panels
                    .focused_waves()
                    .unwrap()
                    .viewport_state(&app.doc)
                    .target()
                    .width()
                    < fitted_width
            );
        });
        test.update(|cx| {
            workspace.update(cx, |ws, cx| {
                ws.dispatch(
                    volna_core::app::Command::AddVars((0..count).collect()),
                    None,
                    cx,
                );
            })
        });
        settle(&mut test, 4);
        test.update(|cx| {
            let rows = &workspace.read(cx).app.panels.focused_waves().unwrap().items;
            assert_eq!(rows.len(), count);
            assert!(rows.iter().all(|row| row.history.is_some()));
        });
        shot(&mut test, "fst-bytes")?;
        return Ok(());
    }

    // 4. Frame-time benchmark: synthetic traces, identical viewport and window.
    let panel_count: usize = std::env::var("VOLNA_PERF_PANELS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    assert!(
        [1, 4].contains(&panel_count),
        "VOLNA_PERF_PANELS must be 1 or 4"
    );
    for n in [10_000usize, 1_000_000, 100_000_000] {
        test.update(|cx| workspace.update(cx, |ws, cx| ws.open_synthetic(n, cx)));
        settle(&mut test, 4);
        if panel_count == 4 {
            for command in [
                volna_core::Command::Action(volna_core::Action::SplitRight),
                volna_core::Command::Action(volna_core::Action::SplitDown),
                volna_core::Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(0)),
                volna_core::Command::Action(volna_core::Action::SplitDown),
            ] {
                test.update(|cx| workspace.update(cx, |ws, cx| ws.dispatch(command, None, cx)));
            }
            settle(&mut test, 4);
        }
        key(&mut test, "f");
        settle(&mut test, 12);
        // Pan a little each frame so every frame is a real repaint.
        let painted_before = test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .frames_painted
        });
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
        let painted = test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .frames_painted
        }) - painted_before;
        assert!(
            painted >= 30,
            "benchmark reused cached canvases: {painted} paints for 30 frames"
        );
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = samples[samples.len() / 2];
        let p90 = samples[samples.len() * 9 / 10];
        let paint_ms = test.update(|cx| workspace.read(cx).waves_frame_ms());
        println!(
            "PERF panels={panel_count} transitions={n} frame_median_ms={median:.2} frame_p90_ms={p90:.2} table_paint_ms={paint_ms:.2}"
        );
        // Zoomed-in view as well.
        for _ in 0..6 {
            key(&mut test, "=");
        }
        settle(&mut test, 12);
        let painted_before = test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .frames_painted
        });
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
        let painted = test.update(|cx| {
            workspace
                .read(cx)
                .app
                .panels
                .focused_waves()
                .unwrap()
                .frames_painted
        }) - painted_before;
        assert!(
            painted >= 30,
            "benchmark reused cached canvases: {painted} paints for 30 frames"
        );
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "PERF panels={panel_count} transitions={n} zoomed frame_median_ms={:.2} frame_p90_ms={:.2}",
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
