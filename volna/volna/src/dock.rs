//! GPUI's dock is a mirror of the core layout. Only explicit core commands
//! remove content; widget detachment during a rebuild is not a close.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use gpui_kit::base::dock as base;
use gpui_kit::component::dock::{DockSkin, Panel, panel_handle};
use gpui_kit::component::{
    Selectable, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext, Axis as GAxis, Context, Div, Empty, Entity, EventEmitter,
    FocusHandle, Focusable, IntoElement, Render, Stateful, WeakEntity, Window, div, px,
};
use volna_core::panels::{Axis, Layout, PanelId, PanelsCommand};
use volna_core::{App as CoreApp, Command};

use crate::app::Workspace;
use crate::settings_panel::SettingsPanelView;

/// The GPUI view of one core panel, by kind: a core-painted canvas (waves,
/// pipeline, or an unsupported placeholder) or the settings tab.
pub(crate) enum PanelView {
    Canvas(Entity<CanvasPanelView>),
    Settings(Entity<SettingsPanelView>),
}

impl PanelView {
    fn handle(&self) -> Arc<dyn base::PanelView> {
        match self {
            Self::Canvas(view) => panel_handle(view.clone()),
            Self::Settings(view) => panel_handle(view.clone()),
        }
    }
    fn focus(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Canvas(view) => view.read(cx).focus.clone(),
            Self::Settings(view) => view.read(cx).focus_handle(cx),
        }
    }
    fn entity_id(&self) -> gpui_kit::EntityId {
        match self {
            Self::Canvas(view) => view.entity_id(),
            Self::Settings(view) => view.entity_id(),
        }
    }
}

pub(crate) struct DockHost {
    pub area: Entity<base::DockArea>,
    views: BTreeMap<PanelId, PanelView>,
    single: Rc<Cell<bool>>,
    revision: Option<u64>,
    installed: Option<Layout>,
    echo: Option<Layout>,
    generation: u64,
}

impl DockHost {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let single = Rc::new(Cell::new(true));
        let mut skin_handle = None;
        let area = cx.new(|cx| {
            let skin = DockSkin::new(cx);
            skin_handle = Some(skin.clone());
            base::DockArea::new("volna-dock", None, window, cx).with_renderer(Rc::new(
                DockRenderer {
                    skin,
                    single: single.clone(),
                },
            ))
        });
        skin_handle
            .unwrap()
            .set_panel_style(gpui_kit::component::dock::PanelStyle::TabBar, cx);
        cx.subscribe(&area, |ws, area, event, cx| {
            if !matches!(event, base::DockEvent::LayoutChanged) {
                return;
            }
            let candidate = from_dock(&area.read(cx).dump(cx).center);
            let Some(host) = &mut ws.dock else { return };
            match candidate {
                Ok(layout) if host.echo.as_ref() == Some(&layout) => {}
                Ok(layout) => {
                    let revision = host.revision.unwrap_or(0);
                    // The user already installed this tree in the dock. Do
                    // not detach/rebuild all its widgets when core accepts it.
                    host.installed = Some(layout.clone());
                    host.echo = Some(layout.clone());
                    ws.dispatch(
                        Command::Panels(PanelsCommand::SetLayout {
                            layout,
                            from_revision: revision,
                        }),
                        None,
                        cx,
                    );
                }
                Err(e) => log::warn!("invalid dock layout: {e:#}"),
            }
        })
        .detach();
        Self {
            area,
            views: BTreeMap::new(),
            single,
            revision: None,
            installed: None,
            echo: None,
            generation: 0,
        }
    }

    pub fn sync(
        &mut self,
        app: &CoreApp,
        ws: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.generation != app.doc.generation() {
            // The settings tab keeps its id and its view across traces.
            self.views
                .retain(|_, view| matches!(view, PanelView::Settings(_)));
            self.installed = None;
            self.echo = None;
            self.revision = None;
            self.generation = app.doc.generation();
        }
        self.single.set(app.panels.len() == 1);
        self.views.retain(|id, _| app.panels.get(*id).is_some());
        for panel in app.panels.iter() {
            self.views.entry(panel.id).or_insert_with(|| {
                if panel.kind.is_settings() {
                    PanelView::Settings(
                        cx.new(|cx| SettingsPanelView::new(ws.clone(), panel.id, window, cx)),
                    )
                } else {
                    let pipeline = panel.kind.pipeline().is_some();
                    PanelView::Canvas(cx.new(|cx| {
                        CanvasPanelView::new(
                            ws.clone(),
                            panel.id,
                            app.doc.generation(),
                            pipeline,
                            window,
                            cx,
                        )
                    }))
                }
            });
        }
        let layout = app.panels.layout();
        if self.installed.as_ref() != Some(layout) {
            let bounds = self.area.read(cx).bounds();
            // The first single-panel installation needs no dimensions. For
            // a restored split, wait until the area has measured its slot.
            if app.panels.len() > 1
                && (bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0))
            {
                // A frame request alone may reuse the cached Workspace and
                // never call sync again. Retry after the area measures its slot.
                window.on_next_frame(move |_, cx| {
                    _ = ws.update(cx, |_, cx| cx.notify());
                });
                return;
            }
            let tree = to_dock(
                layout,
                &self.views,
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                cx,
            );
            self.area
                .update(cx, |area, cx| area.set_center(tree, window, cx));
            self.echo = from_dock(&self.area.read(cx).dump(cx).center).ok();
            self.installed = Some(layout.clone());
        }
        if self.revision != Some(app.panels.revision()) {
            self.area.update(cx, |_, cx| cx.notify());
        }
        self.revision = Some(app.panels.revision());
    }

    pub fn focus(&self, panel: PanelId, cx: &App) -> Option<FocusHandle> {
        self.installed.as_ref()?;
        self.views.get(&panel).map(|view| view.focus(cx))
    }

    /// The settings tab's view, created on demand so it can also be shown
    /// without a trace (when the dock itself is not rendered).
    pub fn settings_view(
        &mut self,
        id: PanelId,
        ws: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<SettingsPanelView> {
        let view = self.views.entry(id).or_insert_with(|| {
            PanelView::Settings(cx.new(|cx| SettingsPanelView::new(ws, id, window, cx)))
        });
        match view {
            PanelView::Settings(view) => view.clone(),
            PanelView::Canvas(_) => unreachable!("panel {id:?} is not the settings tab"),
        }
    }

    pub fn invalidate_panels(&self, cx: &mut App) {
        for view in self.views.values() {
            // Focus/menu callbacks may already hold the panel entity. Mark
            // it dirty without borrowing it again.
            cx.notify(view.entity_id());
        }
    }
}

fn to_dock(
    layout: &Layout,
    views: &BTreeMap<PanelId, PanelView>,
    width: f32,
    height: f32,
    cx: &App,
) -> base::DockLayout {
    match layout {
        Layout::Tabs { tabs, active } => {
            let mut out = base::DockLayout::tabs();
            for id in tabs {
                out = out.panel_view(views[id].handle(), cx);
            }
            out.active_index(tabs.iter().position(|id| id == active).unwrap())
        }
        Layout::Split {
            split,
            sizes,
            children,
        } => {
            let horizontal = *split == Axis::Horizontal;
            let mut out = if horizontal {
                base::DockLayout::h_split()
            } else {
                base::DockLayout::v_split()
            };
            for (child, share) in children.iter().zip(sizes) {
                let w = if horizontal { width * share } else { width };
                let h = if horizontal { height } else { height * share };
                out = out.child(
                    to_dock(child, views, w, h, cx),
                    Some(px(if horizontal { w } else { h })),
                );
            }
            out
        }
    }
}

/// Decode the widget snapshot; only the core validates workspace membership.
fn from_dock(state: &base::PanelState) -> Result<Layout> {
    match &state.info {
        base::PanelInfo::Stack { sizes, axis } => {
            let children: Vec<_> = state
                .children
                .iter()
                .map(from_dock)
                .collect::<Result<_>>()?;
            if children.len() == 1 {
                return Ok(children.into_iter().next().unwrap());
            }
            ensure!(
                children.len() >= 2 && sizes.len() == children.len(),
                "invalid split"
            );
            let total: f32 = sizes.iter().map(|p| f32::from(*p)).sum();
            ensure!(total.is_finite() && total > 0.0, "invalid sizes");
            Ok(Layout::Split {
                split: if *axis == 0 {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                },
                sizes: sizes.iter().map(|p| f32::from(*p) / total).collect(),
                children,
            })
        }
        base::PanelInfo::Tabs { active_index } => {
            let tabs: Vec<_> = state.children.iter().map(panel_id).collect::<Result<_>>()?;
            let active = *tabs
                .get(*active_index)
                .ok_or_else(|| anyhow::anyhow!("invalid active tab"))?;
            Ok(Layout::Tabs { tabs, active })
        }
        base::PanelInfo::Panel(_) => Ok(Layout::single(panel_id(state)?)),
        _ => bail!("floating tiles are not supported"),
    }
}

fn panel_id(state: &base::PanelState) -> Result<PanelId> {
    ensure!(
        matches!(
            state.panel_name.as_ref(),
            "volna.waves" | "volna.pipeline" | "volna.settings"
        ),
        "unknown dock widget"
    );
    match &state.info {
        base::PanelInfo::Panel(value) => value
            .get("id")
            .and_then(|v| v.as_u64())
            .map(PanelId)
            .ok_or_else(|| anyhow::anyhow!("missing panel ID")),
        _ => bail!("expected panel payload"),
    }
}

pub(crate) struct CanvasPanelView {
    ws: WeakEntity<Workspace>,
    id: PanelId,
    generation: u64,
    /// The dock widget name records the core kind, for diagnostics only. A
    /// panel never changes kind, so it is fixed at creation: the dock dumps
    /// its state while the workspace is being rendered and cannot be read.
    name: &'static str,
    focus: FocusHandle,
}

impl CanvasPanelView {
    fn rename(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.ws.upgrade() else {
            return;
        };
        let Some(panel) = owner.read(cx).app.panels.get(self.id) else {
            return;
        };
        let title = panel.title.clone().unwrap_or_default();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(title)
                .placeholder("Default panel title")
        });
        let id = self.id;
        let generation = self.generation;
        let input_focus = input.read(cx).focus_handle(cx);
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let value = input.clone();
            dialog
                .title("Rename panel")
                .child(Input::new(&input))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel-rename")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-rename")
                                .label("Rename")
                                .primary()
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(gpui_kit::component::dialog::Confirm {
                                            secondary: false,
                                        }),
                                        cx,
                                    );
                                }),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let title = value.read(cx).value().to_string();
                    owner.update(cx, |ws, cx| {
                        ws.dispatch_if_current(
                            generation,
                            Command::Panels(PanelsCommand::Rename(id, Some(title))),
                            Some(window),
                            cx,
                        )
                    });
                    true
                })
        });
        window.focus(&input_focus, cx);
    }
    fn dispatch(&self, command: PanelsCommand, window: &mut Window, cx: &mut Context<Self>) {
        _ = self.ws.update(cx, |ws, cx| {
            ws.dispatch_if_current(self.generation, Command::Panels(command), Some(window), cx)
        });
    }
    fn new(
        ws: WeakEntity<Workspace>,
        id: PanelId,
        generation: u64,
        pipeline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        cx.on_focus_in(&focus, window, |view, window, cx| {
            view.dispatch(PanelsCommand::Focus(view.id), window, cx);
        })
        .detach();
        Self {
            ws,
            id,
            generation,
            name: if pipeline {
                "volna.pipeline"
            } else {
                "volna.waves"
            },
            focus,
        }
    }
}

impl EventEmitter<base::PanelEvent> for CanvasPanelView {}
impl Focusable for CanvasPanelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl base::Panel for CanvasPanelView {
    fn panel_name(&self) -> &'static str {
        self.name
    }
    // Core close commands preserve the last waveform panel. Never let the
    // dock delete content behind the core's back.
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn dump(&self, _: &App) -> base::PanelState {
        let mut state = base::PanelState::new(self.name);
        state.info = base::PanelInfo::panel(serde_json::json!({"id": self.id.0}));
        state
    }
}
impl Panel for CanvasPanelView {
    fn tab_name(&self, cx: &App) -> Option<gpui_kit::SharedString> {
        self.ws
            .upgrade()?
            .read(cx)
            .app
            .panels
            .get(self.id)
            .map(|p| p.title().into())
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tab_name(cx).unwrap_or_default()
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        use gpui_kit::assets::IconName;
        use volna_core::nav::LinkDim;
        let link = self
            .ws
            .upgrade()?
            .read(cx)
            .app
            .panels
            .get(self.id)?
            .kind
            .nav()?
            .link;
        let link_button = |name, linked, dim, tooltip: &'static str| {
            Button::new(name)
                .icon(if linked {
                    IconName::Link
                } else {
                    IconName::Unlink
                })
                .ghost()
                .xsmall()
                .selected(linked)
                .tooltip(tooltip)
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.dispatch(
                        PanelsCommand::ToggleLink {
                            panel: view.id,
                            dim,
                        },
                        window,
                        cx,
                    )
                }))
        };
        Some(vec![
            link_button(
                "link-view",
                link.viewport,
                LinkDim::Viewport,
                "Follow shared viewport (L)",
            ),
            link_button(
                "link-cursor",
                link.cursor,
                LinkDim::Cursor,
                "Follow shared cursor (Shift+L)",
            ),
            Button::new("new-tab")
                .icon(IconName::Plus)
                .ghost()
                .xsmall()
                .tooltip("New waveform tab")
                .on_click(cx.listener(|view, _, window, cx| {
                    view.dispatch(PanelsCommand::NewTab { group_of: view.id }, window, cx)
                })),
            Button::new("close-panel")
                .icon(IconName::X)
                .ghost()
                .xsmall()
                .tooltip("Close panel")
                .on_click(cx.listener(|view, _, window, cx| {
                    view.dispatch(PanelsCommand::Close(view.id), window, cx)
                })),
        ])
    }

    fn dropdown_menu(
        &mut self,
        mut menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        for (title, axis) in [
            ("Split Right", Axis::Horizontal),
            ("Split Down", Axis::Vertical),
        ] {
            menu = menu.item(PopupMenuItem::new(title).on_click(cx.listener(
                move |view, _, window, cx| {
                    view.dispatch(
                        PanelsCommand::Split {
                            panel: view.id,
                            axis,
                        },
                        window,
                        cx,
                    );
                },
            )));
        }
        menu = menu.item(PopupMenuItem::new("Close Panel").on_click(cx.listener(
            |view, _, window, cx| view.dispatch(PanelsCommand::Close(view.id), window, cx),
        )));
        menu = menu.item(
            PopupMenuItem::new("Close Other Panels").on_click(cx.listener(
                |view, _, window, cx| {
                    view.dispatch(PanelsCommand::CloseOthers(view.id), window, cx);
                },
            )),
        );
        menu.item(
            PopupMenuItem::new("Rename…")
                .on_click(cx.listener(|view, _, window, cx| view.rename(window, cx))),
        )
    }
}
impl Render for CanvasPanelView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return Empty.into_any_element();
        };
        let unsupported = ws
            .read(cx)
            .app
            .panels
            .get(self.id)
            .is_none_or(|p| !p.kind.is_canvas());
        div()
            .id(("wave-panel", self.id.0))
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context("Waves")
            .when(unsupported, |el| {
                el.flex()
                    .items_center()
                    .justify_center()
                    .child("Unsupported panel — saved content is preserved")
            })
            .when(!unsupported, |el| {
                el.child(crate::canvas::PanelCanvas::new(
                    ws,
                    self.id,
                    self.generation,
                ))
            })
            .into_any_element()
    }
}

/// Compose the stock skin, changing only the single-panel header geometry.
struct DockRenderer {
    skin: Rc<DockSkin>,
    single: Rc<Cell<bool>>,
}
impl base::DockAreaRenderer for DockRenderer {
    fn frame(&self, w: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(w, cx)
    }
    fn center_frame(&self, w: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.center_frame(w, cx)
    }
    fn split_frame(
        &self,
        node: base::NodeId,
        axis: GAxis,
        w: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.skin.split_frame(node, axis, w, cx)
    }
    fn render_split_handle(
        &self,
        handle: &gpui_kit::base::ResizeHandleContext,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_split_handle(handle, w, cx)
    }
    fn render_dock(
        &self,
        dock: &base::DockContext,
        content: AnyElement,
        w: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_dock(dock, content, w, cx)
    }
    fn build_placeholder(
        &self,
        state: &base::PanelState,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn base::PanelView>> {
        self.skin.build_placeholder(state, w, cx)
    }
    fn tab_group_renderer(&self) -> Rc<dyn base::TabGroupRenderer> {
        Rc::new(TabRenderer {
            skin: self.skin.tab_group_renderer(),
            single: self.single.clone(),
        })
    }
    fn tiles_renderer(&self) -> Rc<dyn base::TilesRenderer> {
        self.skin.tiles_renderer()
    }
}
struct TabRenderer {
    skin: Rc<dyn base::TabGroupRenderer>,
    single: Rc<Cell<bool>>,
}
impl base::TabGroupRenderer for TabRenderer {
    fn frame(&self, group: &base::TabGroupContext, w: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(group, w, cx)
    }
    fn content_frame(
        &self,
        group: &base::TabGroupContext,
        w: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.skin.content_frame(group, w, cx)
    }
    fn render_tab_bar(
        &self,
        group: &base::TabGroupContext,
        w: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if self.single.get() {
            Empty.into_any_element()
        } else {
            self.skin.render_tab_bar(group, w, cx)
        }
    }
    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &base::TabGroupContext,
        w: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_active_panel(panel, group, w, cx)
    }
    fn render_drop_indicator(
        &self,
        indicator: base::DropIndicator,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_drop_indicator(indicator, w, cx)
    }
    fn render_empty(
        &self,
        group: &base::TabGroupContext,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_empty(group, w, cx)
    }
}
