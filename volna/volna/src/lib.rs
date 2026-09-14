//! Volna: the official VTR/VDB viewer, built with GPUI.
//! Provides VTR and FST waveforms; VDB integration is planned.
//!
//! This crate is the GPUI frontend; every viewer decision lives in
//! `volna_core`. Module map:
//! - `app`: the `Workspace` root view, key bindings, the core command loop
//! - `theme`, `assets`: the core theme mapped to GPUI colours; fonts and icons
//! - `ui`: component styling/placement, text input, splitter, icons and headers
//! - `sidebar`: scope tree and variable list rows
//! - `wave`: the `WaveTable` element that paints the core's display list

pub mod app;
pub mod assets;
mod dock;
#[cfg(not(target_family = "wasm"))]
pub mod native_workspace;
pub mod sidebar;
pub mod theme;
pub mod ui;
pub mod wave;

use gpui_kit::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px,
    size,
};

pub use app::Workspace;

/// Shared start-up: theme, fonts, key bindings.
pub fn init_app(cx: &mut App) {
    gpui_kit::init(cx);
    theme::set(theme::CoreTheme::one_dark(), cx);
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
        gpui_kit::application().with_assets(assets::Assets)
    }
    #[cfg(target_family = "wasm")]
    {
        // Keep the same bundle usable in webviews without shared memory.
        gpui_kit::platform::single_threaded_web().with_assets(assets::Assets)
    }
}

/// Open the main window and return the workspace entity.
pub fn open_main_window(
    cx: &mut App,
    embedded: bool,
) -> anyhow::Result<gpui_kit::Entity<Workspace>> {
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
        #[cfg(not(target_family = "wasm"))]
        {
            let weak = ws.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                weak.update(cx, |ws, cx| {
                    ws.app.persist_workspace(volna_core::Instant::now(), true);
                    ws.after(Some(window), cx);
                    let scheduler = &ws.app.workspace.scheduler;
                    !scheduler.enabled()
                        || scheduler.suspended()
                        || scheduler.target().is_none()
                        || (!scheduler.dirty() && scheduler.outstanding().is_none())
                })
                .unwrap_or(true)
            });
        }
        workspace = Some(ws.clone());
        cx.new(|cx| gpui_kit::component::Root::new(ws, window, cx))
    })?;
    workspace.ok_or_else(|| anyhow::anyhow!("window did not build a workspace"))
}

#[cfg(target_family = "wasm")]
pub mod web {
    //! Browser entry point and the JS bridge used by the VS Code extension.
    //!
    //! JS calls `open_trace(name, bytes)` to load a trace image; the app calls
    //! `window.volnaOpen()` (if the host defines it) to request a file dialog.

    use std::cell::RefCell;

    use futures::StreamExt;
    use futures::channel::mpsc;
    use wasm_bindgen::prelude::*;

    enum HostEvent {
        Open(String, Vec<u8>),
        Resource(String, Vec<u8>, Box<OpenMetadata>),
        Workspace(WorkspaceMessage),
        Theme(Box<crate::theme::CoreTheme>),
        Command(volna_core::app::Command),
        /// Log the viewer state to the console (browser-driven verification).
        DebugState,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OpenMetadata {
        trace_uri: String,
        candidates: Option<Candidates>,
        settings: Settings,
    }
    #[derive(serde::Deserialize)]
    struct Candidates {
        sidecar: volna_core::workspace::persistence::Candidate,
        fallback: volna_core::workspace::persistence::Candidate,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Settings {
        autosave: String,
        link_by_default: bool,
    }
    #[derive(serde::Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    enum WorkspaceMessage {
        Saved {
            ticket: volna_core::workspace::persistence::SaveTicket,
            error: Option<String>,
        },
        Workspace {
            candidate: volna_core::workspace::persistence::Candidate,
        },
        WorkspaceDestination {
            target: volna_core::workspace::persistence::Target,
        },
        RequestWorkspace,
        Candidates {
            trace_uri: String,
            candidates: Candidates,
        },
    }

    thread_local! {
        static CANDIDATES: RefCell<Option<(String, Candidates)>> = const { RefCell::new(None) };
        static PENDING_THEME: RefCell<Option<crate::theme::CoreTheme>> = const { RefCell::new(None) };
        static HOST_TX: RefCell<Option<mpsc::UnboundedSender<HostEvent>>> = const { RefCell::new(None) };
    }

    /// Load a VTR or FST file image after the host receives `volnaReady`.
    #[wasm_bindgen]
    pub fn open_trace(name: String, bytes: Vec<u8>) {
        HOST_TX.with(|tx| {
            if let Some(tx) = tx.borrow().as_ref() {
                tx.unbounded_send(HostEvent::Open(name, bytes)).ok();
            }
        });
    }

    fn enqueue(event: HostEvent) -> Result<(), JsValue> {
        HOST_TX.with(|tx| {
            tx.borrow()
                .as_ref()
                .ok_or_else(|| JsValue::from_str("viewer is not ready"))?
                .unbounded_send(event)
                .map_err(|_| JsValue::from_str("viewer is closed"))
        })
    }

    /// Open a durable resource with opaque workspace candidates supplied by its host.
    #[wasm_bindgen]
    pub fn open_resource(name: String, bytes: Vec<u8>, metadata: &str) -> Result<(), JsValue> {
        let metadata = serde_json::from_str(metadata)
            .map_err(|e| JsValue::from_str(&format!("invalid open metadata: {e}")))?;
        enqueue(HostEvent::Resource(name, bytes, Box::new(metadata)))
    }

    /// Workspace payloads are interpreted only by the Rust core.
    #[wasm_bindgen]
    pub fn workspace_message(json: &str) -> Result<(), JsValue> {
        let message = serde_json::from_str(json)
            .map_err(|e| JsValue::from_str(&format!("invalid workspace message: {e}")))?;
        enqueue(HostEvent::Workspace(message))
    }

    pub fn request_workspace_candidates(trace_uri: &str) {
        CANDIDATES.with(|slot| {
            if slot
                .borrow()
                .as_ref()
                .is_some_and(|(uri, _)| uri == trace_uri)
                && let Some((trace_uri, candidates)) = slot.borrow_mut().take()
            {
                enqueue(HostEvent::Workspace(WorkspaceMessage::Candidates {
                    trace_uri,
                    candidates,
                }))
                .ok();
            }
        });
    }

    fn post(message: serde_json::Value) {
        let global = js_sys::global();
        if let Ok(callback) = js_sys::Reflect::get(&global, &JsValue::from_str("volnaWorkspace"))
            && let Some(callback) = callback.dyn_ref::<js_sys::Function>()
        {
            callback
                .call1(&global, &JsValue::from_str(&message.to_string()))
                .ok();
        }
    }

    pub fn write_workspace(ticket: volna_core::workspace::persistence::SaveTicket, bytes: Vec<u8>) {
        post(
            serde_json::json!({"type":"workspace", "ticket":ticket, "json":String::from_utf8(bytes).expect("core emits UTF-8 JSON")}),
        );
    }
    pub fn workspace_dialog(save: bool) {
        post(serde_json::json!({"type": if save { "saveWorkspaceAs" } else { "openWorkspace" }}));
    }
    pub fn trace_closed(trace_uri: &str) {
        post(serde_json::json!({"type":"closeTrace", "traceUri":trace_uri}));
    }
    pub fn notice(text: &str) {
        post(serde_json::json!({"type":"notice", "text":text}));
    }

    /// Dispatch a named host command; unknown names are rejected before queuing.
    #[wasm_bindgen]
    pub fn dispatch_command(name: &str) -> Result<(), JsValue> {
        let command = volna_core::app::Command::named(name)
            .ok_or_else(|| JsValue::from_str("unknown Volna command"))?;
        HOST_TX.with(|tx| {
            let tx = tx.borrow();
            tx.as_ref()
                .ok_or_else(|| JsValue::from_str("viewer is not ready"))?
                .unbounded_send(HostEvent::Command(command))
                .map_err(|_| JsValue::from_str("viewer is closed"))
        })
    }

    /// Print the one-line viewer state (`Workspace::debug_state`) to the console.
    #[wasm_bindgen]
    pub fn debug_state() {
        HOST_TX.with(|tx| {
            if let Some(tx) = tx.borrow().as_ref() {
                tx.unbounded_send(HostEvent::DebugState).ok();
            }
        });
    }

    /// Host-neutral JSON palette; malformed input leaves the current theme intact.
    #[wasm_bindgen]
    pub fn set_theme(json: &str) -> Result<(), JsValue> {
        let palette = crate::theme::HostPalette::from_json(json)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        install_theme(crate::theme::CoreTheme::from_host(&palette));
        Ok(())
    }

    /// Resolved VS Code CSS snapshot, parsed entirely by shared Rust code.
    #[wasm_bindgen]
    pub fn set_vscode_theme(snapshot: &str) {
        install_theme(crate::theme::CoreTheme::from_host(
            &crate::theme::vscode::host_palette(snapshot),
        ));
    }

    // Keep the latest update while startup is pending; never re-enter GPUI.
    fn install_theme(theme: crate::theme::CoreTheme) {
        HOST_TX.with(|tx| {
            if let Some(tx) = tx.borrow().as_ref() {
                tx.unbounded_send(HostEvent::Theme(Box::new(theme))).ok();
            } else {
                PENDING_THEME.with(|slot| *slot.borrow_mut() = Some(theme));
            }
        });
    }

    /// Ask the host page for a file. The host defines `window.volnaOpen`.
    pub fn request_open_dialog() {
        let global = js_sys::global();
        if let Ok(f) = js_sys::Reflect::get(&global, &JsValue::from_str("volnaOpen"))
            && let Some(f) = f.dyn_ref::<js_sys::Function>()
        {
            f.call0(&global).ok();
            return;
        }
        log::warn!("volnaOpen is not defined by the host page");
    }

    fn host_snapshot(name: &str) -> Option<String> {
        let global = js_sys::global();
        js_sys::Reflect::get(&global, &name.into())
            .ok()?
            .dyn_ref::<js_sys::Function>()?
            .call0(&global)
            .ok()?
            .as_string()
    }

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        gpui_kit::web::init_logging();
        if let Some(json) = host_snapshot("volnaHostTheme") {
            if let Err(error) = set_theme(&json) {
                log::warn!("invalid initial host theme: {error:?}");
            }
        } else if let Some(snapshot) = host_snapshot("volnaVscodeTheme") {
            set_vscode_theme(&snapshot);
        }
        let (tx, mut rx) = mpsc::unbounded::<HostEvent>();
        // Embedded mode when the host page sets `window.volnaEmbedded = true`.
        let embedded = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("volnaEmbedded"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let handle = super::application().run_embedded(move |cx| {
            super::init_app(cx);
            PENDING_THEME.with(|slot| {
                if let Some(theme) = slot.borrow_mut().take() {
                    crate::theme::install(theme, cx);
                }
            });
            HOST_TX.with(|slot| *slot.borrow_mut() = Some(tx));
            match super::open_main_window(cx, embedded) {
                Ok(workspace) => {
                    cx.spawn(async move |cx| {
                        while let Some(event) = rx.next().await {
                            workspace.update(cx, |ws, cx| match event {
                                HostEvent::Open(name, bytes) => ws.open_bytes(name, bytes, cx),
                                HostEvent::Resource(name, bytes, metadata) => {
                                    use volna_core::workspace::persistence::Persistence;
                                    let policy = match metadata.settings.autosave.as_str() {
                                        "sidecar" => Persistence::Auto,
                                        "vscode" => Persistence::Storage,
                                        _ => Persistence::Disabled,
                                    };
                                    ws.app.configure_persistence(policy);
                                    ws.app.workspace.preferences.link_by_default =
                                        volna_core::wave::model::Link {
                                            viewport: metadata.settings.link_by_default,
                                            cursor: metadata.settings.link_by_default,
                                        };
                                    let OpenMetadata {
                                        trace_uri,
                                        candidates,
                                        ..
                                    } = *metadata;
                                    CANDIDATES.with(|slot| {
                                        *slot.borrow_mut() =
                                            candidates.map(|c| (trace_uri.clone(), c))
                                    });
                                    ws.app.open_resource(
                                        volna_core::session::OpenSpec::Bytes { name, bytes },
                                        trace_uri,
                                    );
                                    ws.after(None, cx);
                                }
                                HostEvent::Workspace(message) => {
                                    use volna_core::workspace::persistence::Content;
                                    match message {
                                        WorkspaceMessage::Saved { ticket, error } => {
                                            ws.app.workspace_saved(
                                                ticket,
                                                error,
                                                volna_core::Instant::now(),
                                            )
                                        }
                                        WorkspaceMessage::Candidates {
                                            trace_uri,
                                            candidates,
                                        } => ws.app.restore_candidates(
                                            &trace_uri,
                                            candidates.sidecar,
                                            candidates.fallback,
                                        ),
                                        WorkspaceMessage::Workspace { candidate } => {
                                            let result = match candidate.content {
                                                Content::Bytes(bytes) => {
                                                    ws.app.open_workspace(candidate.target, &bytes)
                                                }
                                                Content::Missing => Err(anyhow::anyhow!(
                                                    "workspace file does not exist"
                                                )),
                                                Content::Error(error) => {
                                                    Err(anyhow::anyhow!(error))
                                                }
                                            };
                                            if let Err(error) = result {
                                                ws.app.report_workspace_error(format!(
                                                    "Cannot open workspace: {error:#}"
                                                ));
                                            }
                                        }
                                        WorkspaceMessage::WorkspaceDestination { target } => {
                                            ws.app.save_workspace(Some(target))
                                        }
                                        WorkspaceMessage::RequestWorkspace => ws
                                            .app
                                            .persist_workspace(volna_core::Instant::now(), true),
                                    }
                                    ws.after(None, cx);
                                }
                                HostEvent::Theme(theme) => crate::theme::install(*theme, cx),
                                HostEvent::Command(command) => ws.dispatch(command, None, cx),
                                HostEvent::DebugState => log::info!("STATE {}", ws.debug_state()),
                            });
                        }
                    })
                    .detach();
                    // Tell the host we are ready to receive files.
                    if let Ok(f) =
                        js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("volnaReady"))
                        && let Some(f) = f.dyn_ref::<js_sys::Function>()
                    {
                        f.call0(&js_sys::global()).ok();
                    }
                }
                Err(e) => log::error!("failed to open window: {e:#}"),
            }
        });
        // The browser owns the run loop; keep the app alive for the page's lifetime.
        APP.with(|slot| *slot.borrow_mut() = Some(handle));
    }

    thread_local! {
        static APP: RefCell<Option<gpui_kit::ApplicationHandle>> = const { RefCell::new(None) };
    }
}

#[cfg(not(target_family = "wasm"))]
mod native_query;
