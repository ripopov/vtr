//! `Workspace`: the GPUI root view, a thin adapter over [`volna_core::App`].
//! It hosts the chrome with GPUI widgets, forwards input as commands, drains
//! the core's events and runs the loads it asks for on the background executor.

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "app_tests.rs"]
mod tests;

use std::collections::HashMap;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
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
use crate::ui::menu::PopupMenuEvent;
use crate::ui::text_input::TextInputEvent;
use crate::ui::{
    Icon, IconButton, IconName, PopupMenu, PopupMenuItem, Splitter, SplitterAxis, TextButton,
    TextInput, Tooltip,
};

actions!(
    workspace,
    [OpenFile, ToggleSidebar, CloseTrace, OpenStressMenu, Quit]
);

actions!(
    waves,
    [
        ZoomIn,
        ZoomOut,
        ZoomFit,
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
    focus_handle: FocusHandle,
    pub(crate) waves_focus: FocusHandle,
    pub(crate) scopes_focus: FocusHandle,
    pub(crate) variables_focus: FocusHandle,
    pub(crate) filter: Entity<TextInput>,
    pub(crate) scopes_scroll: UniformListScrollHandle,
    pub(crate) variables_scroll: UniformListScrollHandle,
    stress_menu: Option<Entity<PopupMenu>>,
    /// Mirrors `app.waves.menu`: (row, popup).
    format_menu: Option<(usize, Entity<PopupMenu>)>,
    /// Display list buffer and shaped-text cache, reused across frames.
    pub(crate) scene: Scene,
    pub(crate) shaped: HashMap<TextKey, ShapedLine>,
    /// Hide the native-style title bar (used inside the VS Code webview).
    pub embedded: bool,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Register key bindings and the application menu.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-o", OpenFile, None),
        KeyBinding::new("ctrl-o", OpenFile, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("cmd-w", CloseTrace, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("=", ZoomIn, Some("Waves")),
        KeyBinding::new("shift-=", ZoomIn, Some("Waves")),
        KeyBinding::new("-", ZoomOut, Some("Waves")),
        KeyBinding::new("f", ZoomFit, Some("Waves")),
        KeyBinding::new("shift-f", ZoomFit, Some("Waves")),
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
            items: vec![MenuItem::action("Quit", Quit)],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Open…", OpenFile),
                MenuItem::action("Close Trace", CloseTrace),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
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

pub(crate) fn to_modifiers(m: gpui::Modifiers) -> volna_core::geometry::Modifiers {
    volna_core::geometry::Modifiers {
        shift: m.shift,
        control: m.control,
        alt: m.alt,
        platform: m.platform,
    }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        self.app.handle(command);
        self.after(window, cx);
    }

    fn after(&mut self, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        for event in self.app.take_events() {
            match event {
                Event::Changed => cx.notify(),
                Event::OpenFileDialog => self.open_file_dialog(cx),
                Event::RevealScopeRow(ix) => self
                    .scopes_scroll
                    .scroll_to_item(ix, gpui::ScrollStrategy::Nearest),
                Event::RevealVarRow(ix) => self
                    .variables_scroll
                    .scroll_to_item(ix, gpui::ScrollStrategy::Nearest),
                Event::FocusFilter => {
                    if let Some(window) = window.as_deref_mut() {
                        let handle = self.filter.read(cx).focus_handle(cx);
                        window.focus(&handle, cx);
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
        for request in self.app.take_requests() {
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
        match (&self.app.waves.menu, &self.format_menu) {
            (Some(m), current) if current.as_ref().map(|(row, _)| *row) != Some(m.row) => {
                let items = m
                    .items
                    .iter()
                    .map(|i| PopupMenuItem {
                        id: i.id.clone().into(),
                        label: i.label.clone().into(),
                        badge: i.badge.clone().map(Into::into),
                        checked: i.checked,
                    })
                    .collect();
                let position = point(px(m.position.x), px(m.position.y));
                let row = m.row;
                let menu = cx.new(|cx| PopupMenu::new(position, items, cx));
                if let Some(window) = window {
                    let handle = menu.read(cx).focus_handle().clone();
                    window.focus(&handle, cx);
                }
                cx.subscribe(&menu, |this, _, event, cx| {
                    let command = match event {
                        PopupMenuEvent::Selected(id) => Command::MenuSelect(id.to_string()),
                        PopupMenuEvent::Dismissed => Command::MenuDismiss,
                    };
                    this.dispatch(command, None, cx);
                })
                .detach();
                self.format_menu = Some((row, menu));
                cx.notify();
            }
            (None, Some(_)) => {
                self.format_menu = None;
                cx.notify();
            }
            _ => {}
        }
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
        self.app.open_path(path);
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
            let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
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

    fn open_stress_menu(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = [
            (10_000usize, "10 K transitions"),
            (1_000_000, "1 M transitions"),
            (100_000_000, "100 M transitions"),
        ]
        .iter()
        .map(|(n, label)| PopupMenuItem {
            id: n.to_string().into(),
            label: (*label).into(),
            badge: None,
            checked: false,
        })
        .collect();
        let menu = cx.new(|cx| PopupMenu::new(position, items, cx));
        let handle = menu.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.subscribe(&menu, |this, _, event, cx| {
            match event {
                PopupMenuEvent::Selected(id) => {
                    if let Ok(n) = id.parse::<usize>() {
                        this.open_synthetic(n, cx);
                    }
                }
                PopupMenuEvent::Dismissed => {}
            }
            this.stress_menu = None;
            cx.notify();
        })
        .detach();
        self.stress_menu = Some(menu);
        cx.notify();
    }

    /// One-line summary of the viewer state, for diagnostics.
    pub fn debug_state(&self) -> String {
        self.app.debug_state()
    }

    /// Smoothed paint time of the wave table, for diagnostics.
    pub fn waves_frame_ms(&self) -> f32 {
        self.app.waves.frame_ms_avg
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
        gpui::deferred(
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
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child("Volna"),
                    )
                    .when_some(file, |el, name| {
                        el.child(div().text_color(colors.text_placeholder).child("—"))
                            .child(div().text_color(colors.text_muted).child(name))
                    }),
            )
            .child(
                IconButton::new("toggle-sidebar", IconName::PanelLeft)
                    .surfaces(t.bar, t.bar_hover, t.bar_hover)
                    .selected(self.app.sidebar_visible)
                    .tooltip(Tooltip::with_shortcut("Toggle sidebar", "⌘B"))
                    .on_click(
                        cx.listener(|this, _, w, cx| this.toggle_sidebar(&ToggleSidebar, w, cx)),
                    ),
            )
            .child(
                IconButton::new("open-file", IconName::FolderOpen)
                    .surfaces(t.bar, t.bar_hover, t.bar_hover)
                    .tooltip(Tooltip::with_shortcut("Open trace", "⌘O"))
                    .on_click(cx.listener(|this, _, w, cx| this.open_file(&OpenFile, w, cx))),
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
                    .h(gpui::relative(frac))
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

    fn render_center(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme(cx);
        let colors = t.editor;
        match self.app.trace_state() {
            TraceState::Loaded(_) => self.render_waves(cx).into_any_element(),
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
    fn render_waves(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let el = div()
            .id("wave-view")
            .track_focus(&self.waves_focus)
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
        el.child(crate::wave::WaveTable::new(cx.entity()))
            .children(self.format_menu.as_ref().map(|(_, m)| m.clone()))
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
                    .font_weight(gpui::FontWeight::SEMIBOLD)
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
                    TextButton::new("open", "Open File…")
                        .icon(IconName::FolderOpen)
                        .primary(true)
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
        let mono = |text: String, color: gpui::Hsla| {
            div()
                .font_family(t.mono_font)
                .text_size(px(t.ui_size_small))
                .text_color(color)
                .child(SharedString::from(text))
        };
        let mut left = div().flex().items_center().gap_3();
        let mut right = div().flex().items_center().gap_3();
        if let Some(range) = status.time_range {
            left = left.child(mono(range, colors.text_muted));
        }
        if let Some(s) = status.signals {
            left = left.child(mono(s, colors.text_placeholder));
        }
        if let Some(s) = status.changes {
            left = left.child(mono(s, colors.text_placeholder));
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
                .tooltip(Tooltip::text("Open a synthetic stress trace"))
                .on_click(cx.listener(|this, ev: &gpui::ClickEvent, window, cx| {
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
        if self.app.tick(Instant::now()) {
            window.request_animation_frame();
        }
        let t = *theme(cx);
        let colors = t.editor;
        let drag = self.app.drag;
        let sidebar_visible = self.app.sidebar_visible;
        let mut root = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(t.editor.bg)
            .text_color(colors.text)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::close_trace));
        #[cfg(not(target_family = "wasm"))]
        {
            root = root.on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                if let Some(p) = paths.paths().first() {
                    this.open_path(p.clone(), cx);
                }
            }));
        }
        if !self.embedded {
            root = root.child(self.render_titlebar(cx));
        }
        let sidebar = sidebar_visible.then(|| self.render_sidebar(window, cx).into_any_element());
        let center = self.render_center(cx);
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
        .children(self.stress_menu.clone())
        .children(drag.map(|d| self.render_drag_surface(d, cx)))
    }
}
