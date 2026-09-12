//! Volna: the official VTR/VDB viewer, built with GPUI.
//! Currently provides VTR waveforms; VDB integration is planned.
//!
//! Module map:
//! - `data`: toolkit-independent model (values, histories, translators, sources)
//! - `theme`, `assets`: design tokens and embedded fonts/icons
//! - `ui`: reusable primitives (buttons, splitter, menu, scrollbar, text input)
//! - `sidebar`: scope tree and variable list
//! - `wave`: viewport math, timeline, and the `WaveTable` element
//! - `app`: the `Workspace` root view and key bindings

pub mod app;
pub mod assets;
pub mod data;
pub mod sidebar;
pub mod theme;
pub mod ui;
pub mod wave;

use std::sync::Arc;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px,
    size,
};

pub use app::Workspace;

/// Shared start-up: theme, fonts, key bindings.
pub fn init_app(cx: &mut App) {
    cx.set_global(theme::Theme::one_dark());
    if let Err(e) = assets::load_fonts(cx) {
        log::warn!("failed to load bundled fonts: {e:#}");
    }
    app::init(cx);
}

/// Options for the main window on desktop platforms.
pub fn window_options(cx: &mut App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Volna".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(10.0), px(10.0))),
        }),
        window_min_size: Some(size(px(800.0), px(500.0))),
        ..Default::default()
    }
}

/// Build the application object with bundled assets for the current platform.
pub fn application() -> Application {
    #[cfg(not(target_family = "wasm"))]
    {
        gpui_platform::application().with_assets(assets::Assets)
    }
    #[cfg(target_family = "wasm")]
    {
        let platform = std::rc::Rc::new(gpui_web::WebPlatform::new(false));
        Application::with_platform(platform).with_assets(assets::Assets)
    }
}

/// Open the main window and return the workspace entity.
pub fn open_main_window(cx: &mut App, embedded: bool) -> anyhow::Result<gpui::Entity<Workspace>> {
    let options = if embedded {
        WindowOptions::default()
    } else {
        window_options(cx)
    };
    let mut workspace = None;
    cx.open_window(options, |window, cx| {
        let ws = cx.new(|cx| {
            let mut w = Workspace::new(window, cx);
            w.embedded = embedded;
            w
        });
        workspace = Some(ws.clone());
        ws
    })?;
    workspace.ok_or_else(|| anyhow::anyhow!("window did not build a workspace"))
}

#[allow(dead_code)]
fn _assert_source_object_safe(_: Arc<dyn data::WaveSource>) {}

#[cfg(target_family = "wasm")]
pub mod web {
    //! Browser entry point and the JS bridge used by the VS Code extension.
    //!
    //! JS calls `open_trace(name, bytes)` to load a VTR image; the app calls
    //! `window.volnaOpen()` (if the host defines it) to request a file dialog.

    use std::cell::RefCell;

    use futures::StreamExt;
    use futures::channel::mpsc;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static OPEN_TX: RefCell<Option<mpsc::UnboundedSender<(String, Vec<u8>)>>> = const { RefCell::new(None) };
    }

    /// Load a VTR file image. Safe to call before the app has finished starting.
    #[wasm_bindgen]
    pub fn open_trace(name: String, bytes: Vec<u8>) {
        OPEN_TX.with(|tx| {
            if let Some(tx) = tx.borrow().as_ref() {
                tx.unbounded_send((name, bytes)).ok();
            }
        });
    }

    /// Ask the host page for a file. The host defines `window.volnaOpen`.
    pub fn request_open_dialog() {
        let global = js_sys::global();
        if let Ok(f) = js_sys::Reflect::get(&global, &JsValue::from_str("volnaOpen")) {
            if let Some(f) = f.dyn_ref::<js_sys::Function>() {
                f.call0(&global).ok();
                return;
            }
        }
        log::warn!("volnaOpen is not defined by the host page");
    }

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        gpui_web::init_logging();
        let (tx, mut rx) = mpsc::unbounded::<(String, Vec<u8>)>();
        OPEN_TX.with(|slot| *slot.borrow_mut() = Some(tx));
        // Embedded mode when the host page sets `window.volnaEmbedded = true`.
        let embedded = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("volnaEmbedded"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let handle = super::application().run_embedded(move |cx| {
            super::init_app(cx);
            match super::open_main_window(cx, embedded) {
                Ok(workspace) => {
                    cx.spawn(async move |cx| {
                        while let Some((name, bytes)) = rx.next().await {
                            let _ = workspace.update(cx, |ws, cx| ws.open_bytes(name, bytes, cx));
                        }
                    })
                    .detach();
                    // Tell the host we are ready to receive files.
                    if let Ok(f) =
                        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("volnaReady"))
                    {
                        if let Some(f) = f.dyn_ref::<js_sys::Function>() {
                            f.call0(&js_sys::global()).ok();
                        }
                    }
                }
                Err(e) => log::error!("failed to open window: {e:#}"),
            }
        });
        // The browser owns the run loop; keep the app alive for the page's lifetime.
        APP.with(|slot| *slot.borrow_mut() = Some(handle));
    }

    thread_local! {
        static APP: RefCell<Option<gpui::ApplicationHandle>> = const { RefCell::new(None) };
    }
}
