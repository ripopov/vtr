//! `Workspace`: the root view. Owns the trace state, the sidebar panels and
//! the waveform view, and lays them out with draggable splitters.

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "app_tests.rs"]
mod tests;

const SIDEBAR_FRACTION_MIN: f32 = 0.15;

use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, App, Context, CursorStyle, Entity, FocusHandle, Focusable,
    IntoElement, KeyBinding, Menu, MenuItem, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, SharedString, Styled, Transformation, Window, actions, div,
    percentage, point, px,
};

use crate::data::synth::SynthSource;
use crate::data::{WaveSource, vtr_source::VtrSource};
use crate::sidebar::{ScopeTree, ScopeTreeEvent, VariableList, VariableListEvent};
use crate::theme::theme;
use crate::ui::icon::icon_svg;
use crate::ui::menu::PopupMenuEvent;
use crate::ui::{
    Icon, IconButton, IconName, PopupMenu, PopupMenuItem, Splitter, SplitterAxis, TextButton,
    Tooltip,
};
use crate::wave::timeline::format_time;
use crate::wave::view::{self as wave_actions, WaveView, WaveViewEvent};

actions!(
    workspace,
    [OpenFile, ToggleSidebar, CloseTrace, OpenStressMenu, Quit]
);

pub enum TraceState {
    Empty,
    Loading { name: SharedString },
    Loaded(Arc<dyn WaveSource>),
    Error(SharedString),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WorkspaceDrag {
    Sidebar,
    ScopesSplit,
}

pub struct Workspace {
    state: TraceState,
    // A newer open, explicit source replacement, or close invalidates old results.
    load_generation: u64,
    scopes: Entity<ScopeTree>,
    variables: Entity<VariableList>,
    waves: Entity<WaveView>,
    sidebar_width: Pixels,
    sidebar_visible: bool,
    /// Height of the scope tree as a fraction of the sidebar.
    scopes_fraction: f32,
    drag: Option<WorkspaceDrag>,
    focus_handle: FocusHandle,
    stress_menu: Option<Entity<PopupMenu>>,
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
    use wave_actions::*;
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
                MenuItem::action("Zoom In", wave_actions::ZoomIn),
                MenuItem::action("Zoom Out", wave_actions::ZoomOut),
                MenuItem::action("Zoom to Fit", wave_actions::ZoomFit),
            ],
            disabled: false,
        },
    ]);
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scopes = cx.new(ScopeTree::new);
        let variables = cx.new(VariableList::new);
        let waves = cx.new(WaveView::new);

        cx.subscribe(&scopes, |this, _, event, cx| match event {
            ScopeTreeEvent::Selected(scope) => {
                let scope = *scope;
                this.variables.update(cx, |v, cx| v.set_scope(scope, cx));
            }
        })
        .detach();
        cx.subscribe(&variables, |this, _, event, cx| match event {
            VariableListEvent::Add(vars) => {
                let vars = vars.clone();
                this.waves.update(cx, |w, cx| w.add_vars(&vars, cx));
            }
        })
        .detach();
        cx.subscribe(&waves, |_, _, WaveViewEvent::Changed, cx| cx.notify())
            .detach();

        let focus_handle = cx.focus_handle();
        let wave_focus = waves.read(cx).focus_handle.clone();
        window.focus(&wave_focus, cx);

        Workspace {
            state: TraceState::Empty,
            load_generation: 0,
            scopes,
            variables,
            waves,
            sidebar_width: px(280.0),
            sidebar_visible: true,
            scopes_fraction: 0.42,
            drag: None,
            focus_handle,
            stress_menu: None,
            embedded: false,
        }
    }

    // -- trace loading ------------------------------------------------------------

    pub fn set_source(&mut self, source: Arc<dyn WaveSource>, cx: &mut Context<Self>) {
        self.load_generation += 1;
        self.state = TraceState::Loaded(source.clone());
        self.scopes
            .update(cx, |s, cx| s.set_source(Some(source.clone()), cx));
        self.variables
            .update(cx, |v, cx| v.set_source(Some(source.clone()), cx));
        self.waves
            .update(cx, |w, cx| w.set_source(Some(source), cx));
        cx.notify();
    }

    pub fn close_trace(&mut self, _: &CloseTrace, _w: &mut Window, cx: &mut Context<Self>) {
        self.load_generation += 1;
        self.state = TraceState::Empty;
        self.scopes.update(cx, |s, cx| s.set_source(None, cx));
        self.variables.update(cx, |v, cx| v.set_source(None, cx));
        self.waves.update(cx, |w, cx| w.set_source(None, cx));
        cx.notify();
    }

    fn load_source(
        &mut self,
        name: impl Into<SharedString>,
        load: impl std::future::Future<Output = anyhow::Result<Arc<dyn WaveSource>>> + Send + 'static,
        show_all: bool,
        cx: &mut Context<Self>,
    ) {
        self.load_generation += 1;
        let generation = self.load_generation;
        self.state = TraceState::Loading { name: name.into() };
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(load).await;
            this.update(cx, |this, cx| {
                if this.load_generation != generation {
                    return;
                }
                match result {
                    Ok(source) => {
                        let count = source.hierarchy().vars.len();
                        this.set_source(source, cx);
                        if show_all {
                            this.waves.update(cx, |w, cx| {
                                w.add_vars(&(0..count).collect::<Vec<_>>(), cx)
                            });
                        }
                    }
                    Err(e) => this.state = TraceState::Error(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Open a VTR file from disk (native).
    #[cfg(not(target_family = "wasm"))]
    pub fn open_path(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.load_source(
            name,
            async move { VtrSource::open(&path).map(|s| Arc::new(s) as Arc<dyn WaveSource>) },
            false,
            cx,
        );
    }

    /// Open a VTR image held in memory (used by the web bridge and drag-drop).
    pub fn open_bytes(&mut self, name: String, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.load_source(
            name.clone(),
            async move {
                VtrSource::from_bytes(name, bytes).map(|s| Arc::new(s) as Arc<dyn WaveSource>)
            },
            false,
            cx,
        );
    }

    pub fn open_synthetic(&mut self, transitions: usize, cx: &mut Context<Self>) {
        self.load_source(
            format!("synthetic {transitions}"),
            async move { Ok(Arc::new(SynthSource::new(transitions)) as Arc<dyn WaveSource>) },
            true,
            cx,
        );
    }

    fn open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _w: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
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

    /// One-line summary of the wave view state, for diagnostics.
    pub fn debug_state(&self, cx: &App) -> String {
        let w = self.waves.read(cx);
        format!(
            "items={} loaded={} selected={:?} anchor={:?} cursor={:?} markers={} viewport=({:.0},{:.0}) menu={} drag={:?} sidebar_w={:?} scopes_frac={:.2}",
            w.items.len(),
            w.items.iter().filter(|i| i.history.is_some()).count(),
            w.selected,
            w.anchor,
            w.cursor,
            w.markers.len(),
            w.viewport.start,
            w.viewport.end,
            w.menu.is_some(),
            self.drag,
            self.sidebar_width,
            self.scopes_fraction
        )
    }

    /// Smoothed paint time of the wave table, for diagnostics.
    pub fn waves_frame_ms(&self, cx: &App) -> f32 {
        self.waves.read(cx).frame_ms_avg
    }

    // -- rendering ----------------------------------------------------------------

    /// While a splitter is being dragged, a transparent surface covers the
    /// whole window so the pointer is tracked and released no matter which
    /// element is under it (the way VS Code's sash overlay works).
    fn render_drag_surface(&self, drag: WorkspaceDrag, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = match drag {
            WorkspaceDrag::Sidebar => CursorStyle::ResizeLeftRight,
            WorkspaceDrag::ScopesSplit => CursorStyle::ResizeUpDown,
        };
        gpui::deferred(
            div()
                .id("drag-surface")
                .absolute()
                .inset_0()
                .occlude()
                .cursor(cursor)
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, window, cx| {
                    match drag {
                        WorkspaceDrag::Sidebar => {
                            this.sidebar_width = px(f32::from(ev.position.x).clamp(160.0, 640.0));
                        }
                        WorkspaceDrag::ScopesSplit => {
                            let t = theme(cx);
                            let top = if this.embedded {
                                px(0.0)
                            } else {
                                t.titlebar_height
                            };
                            let total =
                                f32::from(window.viewport_size().height - top - t.statusbar_height)
                                    .max(1.0);
                            this.scopes_fraction = (f32::from(ev.position.y - top) / total)
                                .clamp(SIDEBAR_FRACTION_MIN, 1.0 - SIDEBAR_FRACTION_MIN);
                        }
                    }
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.drag = None;
                        cx.notify();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        this.drag = None;
                        cx.notify();
                    }),
                ),
        )
        .with_priority(100)
    }

    fn render_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.bar;
        let file: Option<SharedString> = match &self.state {
            TraceState::Loaded(s) => Some(s.info().name.clone().into()),
            TraceState::Loading { name } => Some(name.clone()),
            _ => None,
        };
        div()
            .id("titlebar")
            .flex()
            .flex_none()
            .items_center()
            .h(t.titlebar_height)
            .w_full()
            .pl(px(80.0))
            .pr_2()
            .gap_2()
            .bg(t.bar.bg)
            .border_b_1()
            .border_color(t.border)
            .font_family(t.ui_font)
            .text_size(t.ui_size)
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
                    .surface(t.bar)
                    .selected(self.sidebar_visible)
                    .tooltip(Tooltip::with_shortcut("Toggle sidebar", "⌘B"))
                    .on_click(
                        cx.listener(|this, _, w, cx| this.toggle_sidebar(&ToggleSidebar, w, cx)),
                    ),
            )
            .child(
                IconButton::new("open-file", IconName::FolderOpen)
                    .surface(t.bar)
                    .tooltip(Tooltip::with_shortcut("Open trace", "⌘O"))
                    .on_click(cx.listener(|this, _, w, cx| this.open_file(&OpenFile, w, cx))),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let frac = self
            .scopes_fraction
            .clamp(SIDEBAR_FRACTION_MIN, 1.0 - SIDEBAR_FRACTION_MIN);
        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .w(self.sidebar_width)
            .bg(t.panel.bg)
            .child(
                div()
                    .flex_none()
                    .h(gpui::relative(frac))
                    .min_h(px(96.0))
                    .overflow_hidden()
                    .child(self.scopes.clone()),
            )
            .child(
                Splitter::new("scopes-split", SplitterAxis::Horizontal)
                    .dragging(self.drag == Some(WorkspaceDrag::ScopesSplit))
                    .on_drag_start(cx.listener(|this, _, _, cx| {
                        this.drag = Some(WorkspaceDrag::ScopesSplit);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(96.0))
                    .overflow_hidden()
                    .child(self.variables.clone()),
            )
    }

    fn render_center(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme(cx);
        let colors = t.editor;
        match &self.state {
            TraceState::Loaded(_) => self.waves.clone().into_any_element(),
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
                        .text_size(t.ui_size)
                        .text_color(colors.text_muted)
                        .child(SharedString::from(format!("Loading {name}…"))),
                )
                .into_any_element(),
            TraceState::Empty | TraceState::Error(_) => self.render_empty(cx).into_any_element(),
        }
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.editor;
        let error = match &self.state {
            TraceState::Error(e) => Some(e.clone()),
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
                        .text_size(t.ui_size_small)
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
            .text_size(t.ui_size)
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
                    .child("Open a VTR waveform file to view its signals"),
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
                    .text_size(t.ui_size_small)
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
        let waves = self.waves.read(cx);
        let mono = |text: SharedString, color: gpui::Hsla| {
            div()
                .font_family(t.mono_font)
                .text_size(t.ui_size_small)
                .text_color(color)
                .child(text)
        };
        let mut left = div().flex().items_center().gap_3();
        let mut right = div().flex().items_center().gap_3();
        if let TraceState::Loaded(src) = &self.state {
            let info = src.info();
            let (a, b) = info.time_range;
            left = left
                .child(mono(
                    format!(
                        "{} – {}",
                        format_time(a as f64, info.timescale),
                        format_time(b as f64, info.timescale)
                    )
                    .into(),
                    colors.text_muted,
                ))
                .child(mono(
                    format!("{} signals", info.signal_count).into(),
                    colors.text_placeholder,
                ));
            if let Some(n) = info.change_count {
                left = left.child(mono(format!("{n} changes").into(), colors.text_placeholder));
            }
            let vp = waves.viewport;
            let px_per = vp.width() / f64::from(f32::from(waves.wave_width)).max(1.0);
            right = right.child(mono(
                format!("1 px = {}", format_time(px_per, info.timescale)).into(),
                colors.text_placeholder,
            ));
            if let Some(c) = waves.cursor {
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
                        .child(mono(
                            format_time(c as f64, info.timescale).into(),
                            colors.text,
                        )),
                );
            }
            if !waves.markers.is_empty() {
                left = left.child(mono(
                    format!("{} markers", waves.markers.len()).into(),
                    colors.text_placeholder,
                ));
            }
        }
        right = right.child(mono(
            format!("{:.1} ms", waves.frame_ms_avg).into(),
            colors.text_placeholder,
        ));
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
                .child(div().text_size(t.ui_size_small).child("Stress")),
        );
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(t.statusbar_height)
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.editor;
        let drag = self.drag;
        let sidebar_visible = self.sidebar_visible;
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
            .text_size(t.ui_size)
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
        root.child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .when(sidebar_visible, |el| {
                    el.child(self.render_sidebar(cx)).child(
                        Splitter::new("sidebar-split", SplitterAxis::Vertical)
                            .dragging(drag == Some(WorkspaceDrag::Sidebar))
                            .on_drag_start(cx.listener(|this, _, _, cx| {
                                this.drag = Some(WorkspaceDrag::Sidebar);
                                cx.notify();
                            })),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .child(self.render_center(cx)),
                ),
        )
        .child(self.render_statusbar(cx))
        .children(self.stress_menu.clone())
        .children(drag.map(|d| self.render_drag_surface(d, cx)))
    }
}
