//! `Workspace`: the GPUI root view, a thin adapter over [`volna_core::App`].
//! It hosts the chrome with GPUI widgets, forwards input as commands, drains
//! the core's events and runs the loads it asks for on the background executor.

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "app_tests.rs"]
mod tests;

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, App, Context, CursorStyle, Entity, FocusHandle, Focusable,
    IntoElement, KeyBinding, Menu, MenuItem, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, ShapedLine, SharedString, Styled, Transformation,
    UniformListScrollHandle, Window, actions, div, percentage, point, px,
};
use volna_core::app::{Action, ChromeDrag, Command, Event};
use volna_core::document::TraceState;
use volna_core::session::Session;
use volna_core::{App as CoreApp, FontRole, Instant, Scene};

use crate::theme::theme;
use crate::ui::icon::icon_svg;
use crate::ui::text_input::TextInputEvent;
use crate::ui::{Icon, IconName, Splitter, SplitterAxis, TextInput, icon_button, popup_at};
use gpui_kit::component::{
    Selectable, Sizable,
    button::{Button, ButtonVariants},
    menu::{PopupMenu, PopupMenuItem},
    tooltip::Tooltip,
};

actions!(
    workspace,
    [
        OpenFile,
        OpenWorkspace,
        SaveWorkspace,
        SaveWorkspaceAs,
        ToggleSidebar,
        CloseTrace,
        OpenStressMenu,
        Quit,
        OpenSettings,
        CommandPalette,
        FocusPanel1,
        FocusPanel2,
        FocusPanel3,
        FocusPanel4,
        FocusPanel5,
        FocusPanel6,
        FocusPanel7,
        FocusPanel8,
        FocusPanel9
    ]
);

actions!(
    waves,
    [
        SplitRight,
        SplitDown,
        NewPanel,
        ClosePanel,
        FocusNextPanel,
        FocusPrevPanel,
        ToggleViewportLink,
        ToggleCursorLink,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        ZoomToCursor,
        PanPageLeft,
        PanPageRight,
        GoToStart,
        GoToEnd,
        GoToCursor,
        PanLeft,
        PanRight,
        NextEdge,
        PrevEdge,
        AddMarker,
        ClearMarkers,
        RemoveSelected,
        SelectAll,
        ClearSelection,
        CycleFormat,
        MoveSelectionUp,
        MoveSelectionDown,
    ]
);

/// Key of the shaped-text cache used by the wave painter.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct TextKey {
    pub text: String,
    pub font: FontRole,
    pub size: u32,
    pub color: [u32; 4],
}

pub struct Workspace {
    pub app: CoreApp,
    #[cfg(target_family = "wasm")]
    pub(crate) remote: Option<crate::web_remote::Bridge>,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) native_store: Option<crate::native_workspace::Store>,
    pub(crate) dock: Option<crate::dock::DockHost>,
    panel_focus_pending: bool,
    focus_handle: FocusHandle,
    pub(crate) waves_focus: FocusHandle,
    pub(crate) scopes_focus: FocusHandle,
    pub(crate) variables_focus: FocusHandle,
    pub(crate) filter: Entity<TextInput>,
    pub(crate) scopes_scroll: UniformListScrollHandle,
    pub(crate) variables_scroll: UniformListScrollHandle,
    stress_menu: Option<(gpui_kit::Point<Pixels>, Entity<PopupMenu>)>,
    /// Mirrors `app.panels.focused_waves().unwrap().menu`: (row, position, popup).
    format_menu: Option<(
        volna_core::panels::PanelId,
        usize,
        gpui_kit::Point<Pixels>,
        Entity<PopupMenu>,
    )>,
    /// Display list buffer and shaped-text cache, reused across frames.
    pub(crate) scene: Scene,
    pub(crate) shaped: HashMap<TextKey, ShapedLine>,
    /// Hide the native-style title bar (used inside the VS Code webview).
    pub embedded: bool,
    /// The command line chose the workspace policy; `workspace.autosave` is ignored.
    pub(crate) cli_policy: bool,
    #[cfg(not(target_family = "wasm"))]
    pub(crate) config_watcher: Option<notify::RecommendedWatcher>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Register key bindings and the application menu.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-1", FocusPanel1, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-1", FocusPanel1, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-2", FocusPanel2, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-2", FocusPanel2, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-3", FocusPanel3, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-3", FocusPanel3, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-4", FocusPanel4, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-4", FocusPanel4, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-5", FocusPanel5, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-5", FocusPanel5, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-6", FocusPanel6, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-6", FocusPanel6, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-7", FocusPanel7, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-7", FocusPanel7, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-8", FocusPanel8, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-8", FocusPanel8, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-9", FocusPanel9, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-9", FocusPanel9, Some("Workspace && !Embedded")),
    ]);
    cx.bind_keys([
        KeyBinding::new("cmd-s", SaveWorkspace, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-s", SaveWorkspace, Some("Workspace && !Embedded")),
        KeyBinding::new(
            "cmd-shift-s",
            SaveWorkspaceAs,
            Some("Workspace && !Embedded"),
        ),
        KeyBinding::new(
            "ctrl-shift-s",
            SaveWorkspaceAs,
            Some("Workspace && !Embedded"),
        ),
        KeyBinding::new("cmd-o", OpenFile, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("cmd-w", ClosePanel, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-w", ClosePanel, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-\\", SplitRight, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-\\", SplitRight, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-shift-\\", SplitDown, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-shift-\\", SplitDown, Some("Workspace && !Embedded")),
        KeyBinding::new("cmd-n", NewPanel, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-n", NewPanel, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-tab", FocusNextPanel, Some("Workspace && !Embedded")),
        KeyBinding::new(
            "ctrl-shift-tab",
            FocusPrevPanel,
            Some("Workspace && !Embedded"),
        ),
        KeyBinding::new("l", ToggleViewportLink, Some("Waves")),
        KeyBinding::new("shift-l", ToggleCursorLink, Some("Waves")),
        KeyBinding::new(
            "shift-escape",
            gpui_kit::component::dock::ToggleZoom,
            Some("Waves"),
        ),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("cmd-k", CommandPalette, Some("Workspace && !Embedded")),
        KeyBinding::new("ctrl-k", CommandPalette, Some("Workspace && !Embedded")),
        KeyBinding::new(
            "cmd-shift-,",
            crate::settings_panel::ToggleSettingsJson,
            None,
        ),
        KeyBinding::new(
            "ctrl-shift-,",
            crate::settings_panel::ToggleSettingsJson,
            None,
        ),
        KeyBinding::new(
            "escape",
            crate::settings_panel::SettingsEscape,
            Some("Settings"),
        ),
        KeyBinding::new(
            "cmd-s",
            crate::settings_panel::ApplySettingsJson,
            Some("Settings"),
        ),
        KeyBinding::new(
            "ctrl-s",
            crate::settings_panel::ApplySettingsJson,
            Some("Settings"),
        ),
        KeyBinding::new("=", ZoomIn, Some("Waves")),
        KeyBinding::new("shift-=", ZoomIn, Some("Waves")),
        KeyBinding::new("-", ZoomOut, Some("Waves")),
        KeyBinding::new("f", ZoomFit, Some("Waves")),
        KeyBinding::new("shift-f", ZoomFit, Some("Waves")),
        KeyBinding::new("shift-z", ZoomToCursor, Some("Waves")),
        KeyBinding::new("pageup", PanPageRight, Some("Waves")),
        KeyBinding::new("pagedown", PanPageLeft, Some("Waves")),
        KeyBinding::new("home", GoToStart, Some("Waves")),
        KeyBinding::new("s", GoToStart, Some("Waves")),
        KeyBinding::new("end", GoToEnd, Some("Waves")),
        KeyBinding::new("e", GoToEnd, Some("Waves")),
        KeyBinding::new("c", GoToCursor, Some("Waves")),
        KeyBinding::new("left", PanLeft, Some("Waves")),
        KeyBinding::new("right", PanRight, Some("Waves")),
        KeyBinding::new("shift-right", NextEdge, Some("Waves")),
        KeyBinding::new("shift-left", PrevEdge, Some("Waves")),
        KeyBinding::new("m", AddMarker, Some("Waves")),
        KeyBinding::new("shift-m", ClearMarkers, Some("Waves")),
        KeyBinding::new("backspace", RemoveSelected, Some("Waves")),
        KeyBinding::new("delete", RemoveSelected, Some("Waves")),
        KeyBinding::new("cmd-a", SelectAll, Some("Waves")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Waves")),
        KeyBinding::new("escape", ClearSelection, Some("Waves")),
        KeyBinding::new("t", CycleFormat, Some("Waves")),
        KeyBinding::new("up", MoveSelectionUp, Some("Waves")),
        KeyBinding::new("down", MoveSelectionDown, Some("Waves")),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![
        Menu {
            name: "Volna".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open…", OpenFile),
                MenuItem::action("Close Trace", CloseTrace),
                MenuItem::separator(),
                MenuItem::action("Open Workspace…", OpenWorkspace),
                MenuItem::action("Save Workspace", SaveWorkspace),
                MenuItem::action("Save Workspace As…", SaveWorkspaceAs),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", CommandPalette),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Split Right", SplitRight),
                MenuItem::action("Split Down", SplitDown),
                MenuItem::action("New Waveform Tab", NewPanel),
                MenuItem::action("Close Panel", ClosePanel),
                MenuItem::action("Follow Shared Viewport", ToggleViewportLink),
                MenuItem::action("Follow Shared Cursor", ToggleCursorLink),
                MenuItem::separator(),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::action("Zoom to Fit", ZoomFit),
            ],
            disabled: false,
        },
    ]);
}

/// Wire every wave action to its core action on an element.
macro_rules! wave_actions {
    ($el:expr, $cx:expr, [$($name:ident),* $(,)?]) => {
        $el $(.on_action($cx.listener(|this: &mut Workspace, _: &$name, window, cx| {
            this.dispatch(Command::Action(Action::$name), Some(window), cx)
        })))*
    };
}

pub(crate) fn to_modifiers(m: gpui_kit::Modifiers) -> volna_core::geometry::Modifiers {
    volna_core::geometry::Modifiers {
        shift: m.shift,
        control: m.control,
        alt: m.alt,
        platform: m.platform,
    }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(not(target_family = "wasm"))]
        cx.on_app_quit(|this, cx| {
            this.app.persist_workspace(Instant::now(), true);
            this.app.flush_settings(Instant::now());
            this.after(None, cx);
            async {}
        })
        .detach();
        let filter = cx.new(|cx| TextInput::new("Filter variables", cx));
        cx.subscribe(&filter, |this, filter, event, cx| match event {
            TextInputEvent::Changed => {
                let text = filter.read(cx).text().to_owned();
                this.dispatch(Command::SetFilter(text), None, cx);
            }
            TextInputEvent::Submit => this.dispatch(Command::AddSelectedOrAllVars, None, cx),
            TextInputEvent::Cancel => {}
        })
        .detach();
        let waves_focus = cx.focus_handle();
        window.focus(&waves_focus, cx);
        Workspace {
            app: CoreApp::new(),
            #[cfg(target_family = "wasm")]
            remote: None,
            #[cfg(not(target_family = "wasm"))]
            native_store: None,
            dock: None,
            panel_focus_pending: false,
            focus_handle: cx.focus_handle(),
            waves_focus,
            scopes_focus: cx.focus_handle(),
            variables_focus: cx.focus_handle(),
            filter,
            scopes_scroll: UniformListScrollHandle::new(),
            variables_scroll: UniformListScrollHandle::new(),
            stress_menu: None,
            format_menu: None,
            scene: Scene::default(),
            shaped: HashMap::new(),
            embedded: false,
            cli_policy: false,
            #[cfg(not(target_family = "wasm"))]
            config_watcher: None,
        }
    }

    // -- the core loop ----------------------------------------------------------------

    /// Send a command to the core and act on what it asks for.
    pub fn dispatch(
        &mut self,
        command: Command,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_if_current(self.app.doc.generation(), command, window, cx);
    }

    pub(crate) fn dispatch_if_current(
        &mut self,
        generation: u64,
        command: Command,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let before = self.app.panels.focused_id();
        if !self.app.handle_if_current(generation, command) {
            return;
        }
        self.panel_focus_pending |= before != self.app.panels.focused_id();
        self.after(window, cx);
    }

    pub(crate) fn after(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        loop {
            let events = self.app.take_events();
            if events.is_empty() {
                break;
            }
            for event in events {
                match event {
                    Event::LoadWorkspace { trace_uri } => {
                        #[cfg(not(target_family = "wasm"))]
                        self.load_workspace_candidates(trace_uri);
                        #[cfg(target_family = "wasm")]
                        crate::web::request_workspace_candidates(&trace_uri);
                    }
                    Event::PersistWorkspace { ticket, bytes } => {
                        #[cfg(not(target_family = "wasm"))]
                        self.write_workspace(ticket, bytes);
                        #[cfg(target_family = "wasm")]
                        crate::web::write_workspace(ticket, bytes);
                    }
                    Event::OpenWorkspaceDialog | Event::SaveWorkspaceDialog => {
                        let save = matches!(event, Event::SaveWorkspaceDialog);
                        #[cfg(not(target_family = "wasm"))]
                        self.workspace_dialog(save, cx);
                        #[cfg(target_family = "wasm")]
                        crate::web::workspace_dialog(save);
                    }
                    Event::TraceClosed { trace_uri } => {
                        #[cfg(target_family = "wasm")]
                        crate::web::trace_closed(&trace_uri);
                        #[cfg(not(target_family = "wasm"))]
                        let _ = trace_uri;
                    }
                    Event::Quit => cx.quit(),
                    Event::Changed => {
                        if let Some(dock) = &self.dock {
                            dock.invalidate_panels(cx);
                        }
                        cx.notify();
                    }
                    Event::LayoutChanged { .. } => {
                        self.panel_focus_pending = true;
                        cx.notify();
                    }
                    Event::Notice(text) => {
                        log::warn!("{text}");
                        #[cfg(target_family = "wasm")]
                        crate::web::notice(&text);
                    }
                    Event::OpenFileDialog => self.open_file_dialog(cx),
                    Event::RevealScopeRow(ix) => self
                        .scopes_scroll
                        .scroll_to_item(ix, gpui_kit::ScrollStrategy::Nearest),
                    Event::RevealVarRow(ix) => self
                        .variables_scroll
                        .scroll_to_item(ix, gpui_kit::ScrollStrategy::Nearest),
                    Event::FocusFilter => {
                        if let Some(window) = window.as_deref_mut() {
                            let handle = self.filter.read(cx).focus_handle(cx);
                            window.focus(&handle, cx);
                        }
                    }
                    Event::WriteSettings { ticket, bytes } => {
                        #[cfg(not(target_family = "wasm"))]
                        self.write_settings(ticket, bytes);
                        #[cfg(target_family = "wasm")]
                        {
                            let error = crate::web::write_settings(&bytes).err();
                            self.app.settings_saved(ticket, error, Instant::now());
                        }
                    }
                    Event::SettingsChanged { keys } => self.settings_changed(&keys, cx),
                    Event::FocusSettingsSearch => {
                        if let (Some(window), Some(id)) =
                            (window.as_deref_mut(), self.app.panels.settings_id())
                        {
                            let view = self.settings_view(id, window, cx);
                            view.update(cx, |view, cx| view.focus_search(window, cx));
                        }
                    }
                }
            }
        }
        self.sync_format_menu(window, cx);
        self.sync_filter(cx);
        self.run_requests(cx);
    }

    /// Perform queued loads on the background executor and deliver the results.
    fn run_requests(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_family = "wasm")]
        self.sync_remote();
        for request in self.app.take_requests() {
            #[cfg(target_family = "wasm")]
            let Some(request) = self.route_remote(request, cx) else {
                continue;
            };
            cx.spawn(async move |this, cx| {
                let result = cx.background_spawn(async move { request.perform() }).await;
                this.update(cx, |this, cx| {
                    this.app.deliver(result);
                    this.after(None, cx);
                })
                .ok();
            })
            .detach();
        }
    }

    /// Keep the GPUI popup in step with the core's format menu.
    fn sync_format_menu(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let panel = self.app.panels.focused_id();
        let Some(m) = self
            .app
            .panels
            .focused_waves()
            .and_then(|w| w.menu.as_ref())
        else {
            self.format_menu = None;
            return;
        };
        if self
            .format_menu
            .as_ref()
            .is_some_and(|(id, row, _, _)| *id == panel && *row == m.row)
        {
            return;
        }
        let Some(window) = window else { return };
        let row = m.row;
        let generation = self.app.doc.generation();
        let position = point(px(m.position.x), px(m.position.y));
        let items: Vec<_> = m
            .items
            .iter()
            .map(|item| {
                let id = item.action.clone();
                let label = match &item.badge {
                    Some(badge) => format!("{} ({badge})", item.label),
                    None => item.label.clone(),
                };
                PopupMenuItem::new(label)
                    .checked(item.checked)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dispatch_if_current(
                            generation,
                            Command::MenuSelect(panel, id.clone()),
                            Some(window),
                            cx,
                        );
                    }))
            })
            .collect();
        let focus = self.waves_focus.clone();
        let menu = PopupMenu::build(window, cx, |mut menu, _, _| {
            for item in items {
                menu = menu.item(item);
            }
            menu.min_w(px(200.0)).action_context(focus)
        });
        cx.subscribe(&menu, move |this, _, _: &gpui_kit::DismissEvent, cx| {
            this.dispatch_if_current(generation, Command::MenuDismiss(panel), None, cx);
        })
        .detach();
        self.format_menu = Some((panel, row, position, menu));
        cx.notify();
    }

    /// The core owns the filter text; the text box shows it.
    fn sync_filter(&mut self, cx: &mut Context<Self>) {
        let text = self.app.variables.filter.clone();
        if self.filter.read(cx).text() != text {
            self.filter.update(cx, |f, cx| f.set_text(text, cx));
        }
    }

    // -- trace loading ------------------------------------------------------------

    /// Open a VTR or FST file from disk (native).
    #[cfg(not(target_family = "wasm"))]
    pub fn open_path(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if path.to_string_lossy().ends_with(".volna.json") {
            self.open_workspace_path(path, cx);
            return;
        }
        if self.app.workspace.scheduler.enabled() {
            match path
                .canonicalize()
                .map_err(anyhow::Error::from)
                .and_then(|path| Ok((crate::native_workspace::file_uri(&path)?, path)))
            {
                Ok((uri, path)) => self
                    .app
                    .open_resource(volna_core::session::OpenSpec::Path(path), uri),
                Err(error) => self
                    .app
                    .report_workspace_error(format!("Cannot open trace: {error:#}")),
            }
        } else {
            self.app.open_path(path);
        }
        self.after(None, cx);
    }

    /// Open a trace image held in memory (used by the web bridge and drag-drop).
    pub fn open_bytes(&mut self, name: String, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.app.open_bytes(name, bytes);
        self.after(None, cx);
    }

    pub fn open_synthetic(&mut self, transitions: usize, cx: &mut Context<Self>) {
        self.app.open_synthetic(transitions);
        self.after(None, cx);
    }

    /// Replace the session immediately (tests and hosts that already hold one).
    pub fn set_session(&mut self, session: std::sync::Arc<dyn Session>, cx: &mut Context<Self>) {
        self.app.set_session(session);
        self.after(None, cx);
    }

    /// Run a request the way the load loop would, for tests that need to
    /// control completion order.
    #[cfg(test)]
    pub(crate) fn queue(
        &mut self,
        request: volna_core::session::LoadRequest,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { request.perform() }).await;
            this.update(cx, |this, cx| {
                this.app.deliver(result);
                this.after(None, cx);
            })
            .ok();
        })
        .detach();
    }

    fn open_file_dialog(&mut self, cx: &mut Context<Self>) {
        #[cfg(not(target_family = "wasm"))]
        {
            let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Open".into()),
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(paths))) = rx.await
                    && let Some(path) = paths.into_iter().next()
                {
                    this.update(cx, |this, cx| this.open_path(path, cx)).ok();
                }
            })
            .detach();
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = cx;
            crate::web::request_open_dialog();
        }
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Command::RequestOpenDialog, Some(window), cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Command::ToggleSidebar, Some(window), cx);
    }

    fn close_trace(&mut self, _: &CloseTrace, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(Command::CloseTrace, Some(window), cx);
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.embedded {
            // VS Code owns the settings UI; open it filtered to this extension.
            #[cfg(target_family = "wasm")]
            crate::web::open_host_settings();
            return;
        }
        self.dispatch(
            Command::Settings(volna_core::app::SettingsCommand::Open),
            Some(window),
            cx,
        );
    }

    fn command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(window, cx);
    }

    /// The settings tab's view (created when needed).
    pub(crate) fn settings_view(
        &mut self,
        id: volna_core::panels::PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<crate::settings_panel::SettingsPanelView> {
        let mut dock = self
            .dock
            .take()
            .unwrap_or_else(|| crate::dock::DockHost::new(window, cx));
        let view = dock.settings_view(id, cx.weak_entity(), window, cx);
        self.dock = Some(dock);
        view
    }

    /// React to resolved settings: re-project the theme, follow the
    /// workspace policy unless the command line fixed it.
    fn settings_changed(&mut self, keys: &[&'static str], cx: &mut Context<Self>) {
        if keys.contains(&"appearance.theme") {
            self.apply_theme_setting(cx);
        }
        if keys.contains(&"workspace.autosave") && !self.embedded && !self.cli_policy {
            use volna_core::settings::Autosave;
            use volna_core::workspace::persistence::Persistence;
            let policy = match self.app.settings.resolved().workspace.autosave {
                Autosave::Off => Persistence::Disabled,
                _ => Persistence::Auto,
            };
            self.app.configure_persistence(policy);
        }
        cx.notify();
    }

    /// Install the theme `appearance.theme` names. Embedded hosts supply
    /// their own palette snapshot instead.
    pub(crate) fn apply_theme_setting(&mut self, cx: &mut Context<Self>) {
        if self.embedded {
            return;
        }
        let name = self.app.settings.resolved().appearance.theme.clone();
        log::debug!("applying theme setting {name:?}");
        #[cfg(not(target_family = "wasm"))]
        let theme = match &self.native_store {
            Some(store) => store.theme(&name),
            None => crate::theme::CoreTheme::builtin(&name)
                .ok_or_else(|| anyhow::anyhow!("no theme directory")),
        };
        #[cfg(target_family = "wasm")]
        let theme = crate::theme::CoreTheme::builtin(&name)
            .ok_or_else(|| anyhow::anyhow!("palette files are not available in the browser"));
        match theme {
            Ok(theme) => crate::theme::install(theme, cx),
            Err(error) => {
                self.app
                    .report_workspace_error(format!("Theme '{name}': {error:#}; using One Dark"));
                crate::theme::install(crate::theme::CoreTheme::one_dark(), cx);
            }
        }
    }

    fn open_stress_menu(
        &mut self,
        position: gpui_kit::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<_> = [
            (10_000usize, "10 K transitions"),
            (1_000_000, "1 M transitions"),
            (100_000_000, "100 M transitions"),
        ]
        .into_iter()
        .map(|(n, label)| {
            PopupMenuItem::new(label).on_click(cx.listener(move |this, _, _, cx| {
                this.open_synthetic(n, cx);
                this.stress_menu = None;
            }))
        })
        .collect();
        let focus = self.focus_handle.clone();
        let menu = PopupMenu::build(window, cx, |mut menu, _, _| {
            for item in items {
                menu = menu.item(item);
            }
            menu.min_w(px(200.0)).action_context(focus)
        });
        cx.subscribe(&menu, move |this, _, _: &gpui_kit::DismissEvent, cx| {
            this.stress_menu = None;
            cx.notify();
        })
        .detach();
        self.stress_menu = Some((position, menu));
        cx.notify();
    }

    /// One-line summary of the viewer state, for diagnostics.
    pub fn debug_state(&self) -> String {
        self.app.debug_state()
    }

    /// Smoothed paint time of the wave table, for diagnostics.
    pub fn waves_frame_ms(&self) -> f32 {
        self.app
            .panels
            .focused_waves()
            .map_or(0.0, |w| w.frame_ms_avg)
    }

    // -- rendering ----------------------------------------------------------------

    /// While a splitter is being dragged, a transparent surface covers the
    /// whole window so the pointer is tracked and released no matter which
    /// element is under it (the way VS Code's sash overlay works).
    fn render_drag_surface(&self, drag: ChromeDrag, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = match drag {
            ChromeDrag::Sidebar => CursorStyle::ResizeLeftRight,
            ChromeDrag::ScopesSplit => CursorStyle::ResizeUpDown,
        };
        gpui_kit::deferred(
            div()
                .id("drag-surface")
                .absolute()
                .inset_0()
                .occlude()
                .cursor(cursor)
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, window, cx| {
                    let command = match drag {
                        ChromeDrag::Sidebar => Command::SetSidebarWidth(f32::from(ev.position.x)),
                        ChromeDrag::ScopesSplit => {
                            let t = theme(cx);
                            let top = if this.embedded {
                                0.0
                            } else {
                                t.titlebar_height
                            };
                            let total = (f32::from(window.viewport_size().height)
                                - top
                                - t.statusbar_height)
                                .max(1.0);
                            Command::SetScopesFraction((f32::from(ev.position.y) - top) / total)
                        }
                    };
                    this.dispatch(command, Some(window), cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, window, cx| {
                        this.dispatch(Command::ChromeDragEnd, Some(window), cx);
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, window, cx| {
                        this.dispatch(Command::ChromeDragEnd, Some(window), cx);
                    }),
                ),
        )
        .with_priority(100)
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.bar;
        let file: Option<SharedString> = self.app.doc.name().map(Into::into);
        div()
            .id("titlebar")
            .flex()
            .flex_none()
            .items_center()
            .h(px(t.titlebar_height))
            .w_full()
            .pl(px(80.0))
            .pr_2()
            .gap_2()
            .bg(t.bar.bg)
            .border_b_1()
            .border_color(t.border)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .child(
                // Everything left of the buttons drags the window; double-click zooms it.
                div()
                    .id("titlebar-drag")
                    .flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |ev, window, _| {
                        if ev.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(Icon::new(IconName::AudioWaveform).color(colors.icon_accent))
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child("Volna"),
                    )
                    .when_some(file, |el, name| {
                        el.child(div().text_color(colors.text_placeholder).child("—"))
                            .child(div().text_color(colors.text_muted).child(name))
                    }),
            )
            .child(
                icon_button(
                    "toggle-sidebar",
                    IconName::PanelLeft,
                    t.bar,
                    t.bar_hover,
                    cx,
                )
                .selected(self.app.sidebar_visible)
                .tooltip("Toggle sidebar (⌘B)")
                .on_click(cx.listener(|this, _, w, cx| this.toggle_sidebar(&ToggleSidebar, w, cx))),
            )
            .child(
                icon_button("open-file", IconName::FolderOpen, t.bar, t.bar_hover, cx)
                    .tooltip("Open trace (⌘O)")
                    .on_click(cx.listener(|this, _, w, cx| this.open_file(&OpenFile, w, cx))),
            )
            .child(
                icon_button("open-settings", IconName::Settings, t.bar, t.bar_hover, cx)
                    .selected(self.app.panels.settings_id().is_some())
                    .tooltip("Settings (⌘,)")
                    .on_click(
                        cx.listener(|this, _, w, cx| this.open_settings(&OpenSettings, w, cx)),
                    ),
            )
    }

    fn render_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let frac = self.app.scopes_fraction;
        let scopes = self.render_scopes(window, cx).into_any_element();
        let variables = self.render_variables(window, cx).into_any_element();
        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .w(px(self.app.sidebar_width))
            .bg(t.panel.bg)
            .child(
                div()
                    .flex_none()
                    .h(gpui_kit::relative(frac))
                    .min_h(px(96.0))
                    .overflow_hidden()
                    .child(scopes),
            )
            .child(
                Splitter::new("scopes-split", SplitterAxis::Horizontal)
                    .dragging(self.app.drag == Some(ChromeDrag::ScopesSplit))
                    .on_drag_start(cx.listener(|this, _, window, cx| {
                        this.dispatch(
                            Command::ChromeDragStart(ChromeDrag::ScopesSplit),
                            Some(window),
                            cx,
                        );
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(96.0))
                    .overflow_hidden()
                    .child(variables),
            )
    }

    fn render_center(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let t = *theme(cx);
        let colors = t.editor;
        let settings_only = match self.app.trace_state() {
            TraceState::Loaded(_) | TraceState::Loading { .. } => None,
            _ => self.app.panels.settings_id(),
        };
        if let Some(id) = settings_only {
            let view = self.settings_view(id, window, cx);
            return div().size_full().child(view).into_any_element();
        }
        match self.app.trace_state() {
            TraceState::Loaded(_) => self.render_waves(window, cx).into_any_element(),
            TraceState::Loading { name } => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(t.editor.bg)
                .child(
                    icon_svg(IconName::LoaderCircle, px(28.0), colors.icon_accent).with_animation(
                        "spinner",
                        Animation::new(Duration::from_millis(900)).repeat(),
                        |svg, delta| {
                            svg.with_transformation(Transformation::rotate(percentage(delta)))
                        },
                    ),
                )
                .child(
                    div()
                        .font_family(t.ui_font)
                        .text_size(px(t.ui_size))
                        .text_color(colors.text_muted)
                        .child(SharedString::from(format!("Loading {name}…"))),
                )
                .into_any_element(),
            TraceState::Empty | TraceState::Error(_) => self.render_empty(cx).into_any_element(),
        }
    }

    /// The wave panel: the focusable, action-handling host of the `WaveTable` element.
    fn render_waves(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let el = div()
            .id("wave-view")
            .key_context("Waves")
            .size_full()
            .relative();
        let el = wave_actions!(
            el,
            cx,
            [
                ZoomIn,
                ZoomOut,
                ZoomFit,
                ZoomToCursor,
                PanPageLeft,
                PanPageRight,
                GoToStart,
                GoToEnd,
                GoToCursor,
                PanLeft,
                PanRight,
                NextEdge,
                PrevEdge,
                AddMarker,
                ClearMarkers,
                RemoveSelected,
                SelectAll,
                ClearSelection,
                CycleFormat,
                MoveSelectionUp,
                MoveSelectionDown,
            ]
        );
        let mut dock = self
            .dock
            .take()
            .unwrap_or_else(|| crate::dock::DockHost::new(window, cx));
        dock.sync(&self.app, cx.weak_entity(), window, cx);
        if let Some(focus) = dock.focus(self.app.panels.focused_id(), cx) {
            if self.panel_focus_pending {
                window.focus(&focus, cx);
                self.panel_focus_pending = false;
            }
            self.waves_focus = focus;
        }
        let area = dock.area.clone();
        self.dock = Some(dock);
        el.child(area)
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.editor;
        let error: Option<SharedString> = match self.app.trace_state() {
            TraceState::Error(e) => Some(e.clone().into()),
            _ => None,
        };
        let hint = |keys: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .w(px(260.0))
                .child(div().text_color(colors.text_muted).child(label))
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(t.badge_hover.bg)
                        .font_family(t.mono_font)
                        .text_size(px(t.ui_size_small))
                        .text_color(t.badge_hover.text)
                        .child(keys),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(t.editor.bg)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .child(
                Icon::new(IconName::AudioWaveform)
                    .size(px(40.0))
                    .color(colors.text_placeholder),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(16.0))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(colors.text)
                    .child("No trace open"),
            )
            .child(
                div()
                    .text_color(colors.text_muted)
                    .child("Open a VTR or FST waveform file to view its signals"),
            )
            .when_some(error, |el, e| {
                el.child(
                    div()
                        .mt_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(t.panel.bg)
                        .border_1()
                        .border_color(t.panel.error)
                        .text_color(t.panel.error)
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(14.0))
                                .color(t.panel.error),
                        )
                        .child(e),
                )
            })
            .child(
                div().mt_3().flex().gap_2().child(
                    Button::new("open")
                        .label("Open File…")
                        .icon(gpui_kit::component::Icon::empty().path(IconName::FolderOpen.path()))
                        .primary()
                        .on_click(cx.listener(|this, _, w, cx| this.open_file(&OpenFile, w, cx))),
                ),
            )
            .child(
                div()
                    .mt_6()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .text_size(px(t.ui_size_small))
                    .child(hint("⌘O", "Open a trace"))
                    .child(hint("⏎", "Add selected variables"))
                    .child(hint("= / -", "Zoom in / out"))
                    .child(hint("F", "Zoom to fit"))
                    .child(hint("M", "Add marker at cursor"))
                    .child(hint("T", "Cycle value format")),
            )
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.bar;
        let status = self.app.status();
        let mono = |text: String, color: gpui_kit::Hsla| {
            div()
                .font_family(t.mono_font)
                .text_size(px(t.ui_size_small))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let mut left = div().flex().items_center().gap_3();
        let mut right = div().flex().items_center().gap_3();
        if let Some(panel) = status.panel {
            left = left.child(mono(panel, colors.text));
        }
        if let Some(link) = status.links {
            let chip = |id, text: &'static str, linked, action| {
                let icon = if linked {
                    gpui_kit::assets::IconName::Link
                } else {
                    gpui_kit::assets::IconName::Unlink
                };
                let tooltip = format!(
                    "{} {} link",
                    if linked { "Disable" } else { "Enable" },
                    text.to_lowercase()
                );
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .h(px(18.0))
                    .rounded_sm()
                    .cursor(CursorStyle::PointingHand)
                    .text_size(px(t.ui_size_small))
                    .text_color(if linked {
                        colors.icon_accent
                    } else {
                        colors.text_muted
                    })
                    .hover(move |s| s.bg(t.bar_hover.bg).text_color(t.bar_hover.text))
                    .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
                    .child(gpui_kit::component::Icon::new(icon).with_size(px(12.0)))
                    .child(text)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dispatch(Command::Action(action), Some(window), cx)
                    }))
            };
            right = right
                .child(chip(
                    "status-view-link",
                    "View",
                    link.viewport,
                    Action::ToggleViewportLink,
                ))
                .child(chip(
                    "status-cursor-link",
                    "Cursor",
                    link.cursor,
                    Action::ToggleCursorLink,
                ));
        }
        if let Some(range) = status.time_range {
            left = left.child(mono(range, colors.text_muted));
        }
        if let Some(s) = status.signals {
            left = left.child(mono(s, colors.text_placeholder));
        }
        if let Some(s) = status.changes {
            left = left.child(mono(s, colors.text_placeholder));
        }
        if let Some(notice) = status.workspace_notice {
            let details = self.app.workspace.notices.clone();
            right = right.child(
                div()
                    .id("workspace-notice")
                    .cursor(CursorStyle::PointingHand)
                    .text_color(t.editor.error)
                    .text_size(px(t.ui_size_small))
                    .child("Workspace notice")
                    .tooltip(move |w, cx| Tooltip::new(notice.clone()).build(w, cx))
                    .on_click(move |_, window, cx| {
                        use gpui_kit::component::WindowExt;
                        let details = details.clone();
                        window.open_dialog(cx, move |dialog, _, _| {
                            let details = details.clone();
                            dialog
                                .title("Workspace details")
                                .footer(
                                    Button::new("close-workspace-details")
                                        .label("Close")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    gpui_kit::uniform_list(
                                        "workspace-details",
                                        details.len(),
                                        move |range, _, _| {
                                            range
                                                .map(|index| {
                                                    div()
                                                        .id(("workspace-detail", index))
                                                        .h(px(28.0))
                                                        .text_ellipsis()
                                                        .child(details[index].clone())
                                                        .tooltip({
                                                            let text = details[index].clone();
                                                            move |w, cx| {
                                                                Tooltip::new(text.clone())
                                                                    .build(w, cx)
                                                            }
                                                        })
                                                })
                                                .collect::<Vec<_>>()
                                        },
                                    )
                                    .h(px(280.0)),
                                )
                        });
                    }),
            );
        }
        if let Some(s) = status.px_per {
            right = right.child(mono(s, colors.text_placeholder));
        }
        if let Some(c) = status.cursor {
            left = left.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Icon::new(IconName::Locate)
                            .size(px(12.0))
                            .color(colors.icon_accent),
                    )
                    .child(mono(c, colors.text)),
            );
        }
        if let Some(m) = status.markers {
            left = left.child(mono(m, colors.text_placeholder));
        }
        right = right.child(mono(status.frame_ms, colors.text_placeholder));
        right = right.child(
            div()
                .id("stress")
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .h(px(18.0))
                .rounded_sm()
                .cursor(CursorStyle::PointingHand)
                .text_color(colors.text_muted)
                .hover(move |s| s.bg(t.bar_hover.bg).text_color(t.bar_hover.text))
                .tooltip(|w, cx| Tooltip::new("Open a synthetic stress trace").build(w, cx))
                .on_click(cx.listener(|this, ev: &gpui_kit::ClickEvent, window, cx| {
                    let p = ev.position();
                    this.open_stress_menu(point(p.x - px(160.0), p.y - px(120.0)), window, cx);
                }))
                .child(Icon::new(IconName::Activity).size(px(12.0)).inherit_color())
                .child(div().text_size(px(t.ui_size_small)).child("Stress")),
        );
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(t.statusbar_height))
            .w_full()
            .px_2()
            .bg(t.bar.bg)
            .border_t_1()
            .border_color(t.border)
            .font_family(t.ui_font)
            .child(left)
            .child(right)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_format_menu(Some(window), cx);
        let was_animating = self.app.is_animating();
        if self.app.tick(Instant::now()) {
            window.request_animation_frame();
        }
        self.after(Some(window), cx);
        if was_animating && let Some(dock) = &self.dock {
            dock.invalidate_panels(cx);
        }
        let t = *theme(cx);
        let colors = t.editor;
        let drag = self.app.drag;
        let sidebar_visible = self.app.sidebar_visible;
        let mut root = div()
            .id("workspace")
            .key_context(if self.embedded {
                "Workspace Embedded"
            } else {
                "Workspace"
            })
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(t.editor.bg)
            .text_color(colors.text)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(|this, _: &OpenWorkspace, window, cx| {
                this.dispatch(Command::RequestOpenWorkspace, Some(window), cx)
            }))
            .on_action(cx.listener(|this, _: &SaveWorkspace, window, cx| {
                this.dispatch(Command::SaveWorkspace, Some(window), cx)
            }))
            .on_action(cx.listener(|this, _: &SaveWorkspaceAs, window, cx| {
                this.dispatch(Command::RequestSaveWorkspaceAs, Some(window), cx)
            }))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.dispatch(Command::RequestQuit, Some(window), cx)
            }))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::close_trace))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::command_palette))
            .on_action(cx.listener(
                |this, _: &crate::settings_panel::ToggleSettingsJson, window, cx| {
                    use volna_core::app::SettingsCommand;
                    if this.embedded {
                        return;
                    }
                    if this.app.panels.settings_id().is_none() {
                        this.dispatch(Command::Settings(SettingsCommand::Open), Some(window), cx);
                    }
                    if !this.app.settings_view.json {
                        this.dispatch(
                            Command::Settings(SettingsCommand::ToggleJson),
                            Some(window),
                            cx,
                        );
                    }
                },
            ));
        root = root.on_action(cx.listener(|this, _: &FocusPanel1, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(0)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel2, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(1)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel3, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(2)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel4, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(3)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel5, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(4)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel6, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(5)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel7, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(6)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel8, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(7)),
                Some(window),
                cx,
            );
        }));
        root = root.on_action(cx.listener(|this, _: &FocusPanel9, window, cx| {
            this.dispatch(
                Command::Panels(volna_core::panels::PanelsCommand::FocusIndex(8)),
                Some(window),
                cx,
            );
        }));
        root = wave_actions!(
            root,
            cx,
            [
                SplitRight,
                SplitDown,
                NewPanel,
                ClosePanel,
                FocusNextPanel,
                FocusPrevPanel,
                ToggleViewportLink,
                ToggleCursorLink
            ]
        );
        #[cfg(not(target_family = "wasm"))]
        {
            root = root.on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, _, cx| {
                if let Some(p) = paths.paths().first() {
                    this.open_path(p.clone(), cx);
                }
            }));
        }
        if !self.embedded {
            root = root.child(self.render_titlebar(cx));
        }
        let sidebar = sidebar_visible.then(|| self.render_sidebar(window, cx).into_any_element());
        let center = self.render_center(window, cx);
        let dialogs = gpui_kit::component::Root::render_dialog_layer(window, cx);
        root.child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .when_some(sidebar, |el, sidebar| {
                    el.child(sidebar).child(
                        Splitter::new("sidebar-split", SplitterAxis::Vertical)
                            .dragging(drag == Some(ChromeDrag::Sidebar))
                            .on_drag_start(cx.listener(|this, _, window, cx| {
                                this.dispatch(
                                    Command::ChromeDragStart(ChromeDrag::Sidebar),
                                    Some(window),
                                    cx,
                                );
                            })),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(center),
                ),
        )
        .child(self.render_statusbar(cx))
        .children(
            self.format_menu
                .as_ref()
                .map(|(_, _, p, m)| popup_at(*p, m.clone(), window, cx)),
        )
        .children(
            self.stress_menu
                .as_ref()
                .map(|(p, m)| popup_at(*p, m.clone(), window, cx)),
        )
        .children(drag.map(|d| self.render_drag_surface(d, cx)))
        .children(dialogs)
    }
}
