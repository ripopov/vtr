use std::collections::HashSet;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    Context, CursorStyle, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    SharedString, UniformListScrollHandle, Window, div, px, uniform_list,
};

use crate::data::{ScopeId, WaveSource};
use crate::theme::theme;
use crate::ui::{Icon, IconButton, IconName, Tooltip, panel_header};

pub enum ScopeTreeEvent {
    Selected(Option<ScopeId>),
}

pub struct ScopeTree {
    source: Option<Arc<dyn WaveSource>>,
    expanded: HashSet<ScopeId>,
    selected: Option<ScopeId>,
    /// Flattened visible rows: (scope, depth).
    visible: Vec<(ScopeId, usize)>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<ScopeTreeEvent> for ScopeTree {}

impl Focusable for ScopeTree {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ScopeTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        ScopeTree {
            source: None,
            expanded: HashSet::new(),
            selected: None,
            visible: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_source(&mut self, source: Option<Arc<dyn WaveSource>>, cx: &mut Context<Self>) {
        self.source = source;
        self.expanded.clear();
        self.selected = None;
        if let Some(src) = &self.source {
            let h = src.hierarchy();
            // Expand the first two levels so the tree is not a bare list of roots.
            for &r in &h.roots {
                self.expanded.insert(r);
                for &c in &h.scopes[r].children {
                    self.expanded.insert(c);
                }
            }
            self.selected = h.roots.first().copied();
        }
        self.rebuild();
        cx.emit(ScopeTreeEvent::Selected(self.selected));
        cx.notify();
    }

    fn rebuild(&mut self) {
        self.visible.clear();
        let Some(src) = &self.source else { return };
        let h = src.hierarchy();
        fn walk(
            h: &crate::data::Hierarchy,
            id: ScopeId,
            depth: usize,
            expanded: &HashSet<ScopeId>,
            out: &mut Vec<(ScopeId, usize)>,
        ) {
            out.push((id, depth));
            if expanded.contains(&id) {
                for &c in &h.scopes[id].children {
                    walk(h, c, depth + 1, expanded, out);
                }
            }
        }
        for &r in &h.roots {
            walk(h, r, 0, &self.expanded, &mut self.visible);
        }
    }

    fn toggle(&mut self, id: ScopeId, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.rebuild();
        cx.notify();
    }

    fn select(&mut self, id: ScopeId, cx: &mut Context<Self>) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            cx.emit(ScopeTreeEvent::Selected(Some(id)));
        }
        cx.notify();
    }

    fn set_all(&mut self, expand: bool, cx: &mut Context<Self>) {
        self.expanded.clear();
        if expand && let Some(src) = &self.source {
            self.expanded.extend(0..src.hierarchy().scopes.len());
        }
        self.rebuild();
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(sel) = self.selected else { return };
        let Some(pos) = self.visible.iter().position(|(id, _)| *id == sel) else {
            return;
        };
        let has_children = self
            .source
            .as_ref()
            .is_some_and(|s| !s.hierarchy().scopes[sel].children.is_empty());
        match ev.keystroke.key.as_str() {
            "down" => {
                if pos + 1 < self.visible.len() {
                    let id = self.visible[pos + 1].0;
                    self.select(id, cx);
                    self.scroll
                        .scroll_to_item(pos + 1, gpui::ScrollStrategy::Nearest);
                }
            }
            "up" => {
                if pos > 0 {
                    let id = self.visible[pos - 1].0;
                    self.select(id, cx);
                    self.scroll
                        .scroll_to_item(pos - 1, gpui::ScrollStrategy::Nearest);
                }
            }
            "right" => {
                if has_children && !self.expanded.contains(&sel) {
                    self.toggle(sel, cx);
                }
            }
            "left" => {
                if self.expanded.contains(&sel) {
                    self.toggle(sel, cx);
                } else if let Some(p) = self
                    .source
                    .as_ref()
                    .and_then(|s| s.hierarchy().scopes[sel].parent)
                {
                    self.select(p, cx);
                }
            }
            "enter" | "space" if has_children => {
                self.toggle(sel, cx);
            }
            _ => {}
        }
    }
}

fn scope_icon(kind: &str) -> IconName {
    match kind {
        "module" | "sc_module" | "core" => IconName::Box,
        "struct" | "union" | "class" | "interface" | "vhdl_record" => IconName::Braces,
        "package" | "vhdl_package" => IconName::Folder,
        _ => IconName::Folder,
    }
}

impl Render for ScopeTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let focused = self.focus_handle.is_focused(window);
        let count = self.visible.len();
        let header = panel_header("Scopes", cx).child(
            div()
                .flex()
                .gap_1()
                .child(
                    IconButton::new("expand-all", IconName::ChevronsRight)
                        .tooltip(Tooltip::text("Expand all"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_all(true, cx))),
                )
                .child(
                    IconButton::new("collapse-all", IconName::ChevronsLeft)
                        .tooltip(Tooltip::text("Collapse all"))
                        .on_click(cx.listener(|this, _, _, cx| this.set_all(false, cx))),
                ),
        );

        let list = uniform_list(
            "scope-tree",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let t = theme(cx).clone();
                let Some(src) = this.source.clone() else {
                    return Vec::new();
                };
                let h = src.hierarchy();
                range
                    .map(|ix| {
                        let (id, depth) = this.visible[ix];
                        let scope = &h.scopes[id];
                        let has_children = !scope.children.is_empty();
                        let expanded = this.expanded.contains(&id);
                        let selected = this.selected == Some(id);
                        let hover = t.element_hover;
                        let name: SharedString = scope.name.clone().into();
                        let mut row = div()
                            .id(("scope", ix))
                            .flex()
                            .items_center()
                            .h(t.row_height)
                            .pl(px(8.0 + 12.0 * depth as f32))
                            .pr_2()
                            .gap_1()
                            .cursor(CursorStyle::PointingHand)
                            .font_family(t.ui_font.clone())
                            .text_size(t.ui_size)
                            .text_color(t.text)
                            .on_click(cx.listener(
                                move |this, ev: &gpui::ClickEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    this.select(id, cx);
                                    if ev.click_count() == 2 && has_children {
                                        this.toggle(id, cx);
                                    }
                                },
                            ));
                        if selected {
                            row = row.bg(t.element_selected);
                        } else {
                            row = row.hover(move |s| s.bg(hover));
                        }
                        let chevron = div()
                            .id(("chevron", ix))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(16.0))
                            .rounded_sm()
                            .when(has_children, |el| {
                                el.cursor(CursorStyle::PointingHand)
                                    .hover(move |s| s.bg(t.element_active))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.toggle(id, cx)),
                                    )
                                    .child(
                                        Icon::new(if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(px(14.0)),
                                    )
                            });
                        row.child(chevron)
                            .child(Icon::new(scope_icon(&scope.kind)).size(px(14.0)).color(
                                if selected {
                                    t.icon_accent
                                } else {
                                    t.icon_muted
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .size_full();

        div()
            .id("scopes-panel")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg_panel)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .py_1()
                    .when(focused, |el| el.border_1().border_color(t.border_focused))
                    .child(if count == 0 {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(t.ui_size_small)
                            .text_color(t.text_placeholder)
                            .child("No scopes")
                            .into_any_element()
                    } else {
                        list.into_any_element()
                    }),
            )
    }
}
