//! The scope tree panel: GPUI rows over `ScopeTreeModel`.

use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, CursorStyle, IntoElement, KeyDownEvent, SharedString, Window, div, px, uniform_list,
};
use volna_core::app::Command;
use volna_core::sidebar::Key;
use volna_core::sidebar::icons::{scope_icon, stream_tag};

use crate::app::Workspace;
use crate::theme::{ThemePx, theme};
use crate::ui::{Icon, IconName, icon_button, panel_header};

impl Workspace {
    fn scopes_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.key == "tab" {
            window.focus(&self.variables_focus, cx);
            cx.stop_propagation();
            return;
        }
        let key = match ev.keystroke.key.as_str() {
            "down" => Key::Down,
            "up" => Key::Up,
            "left" => Key::Left,
            "right" => Key::Right,
            "enter" => Key::Enter,
            "space" => Key::Space,
            _ => return,
        };
        self.dispatch(Command::ScopesKey(key), Some(window), cx);
        cx.stop_propagation();
    }

    pub(crate) fn render_scopes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.panel;
        let focused = self.scopes_focus.is_focused(window);
        let count = self.app.scopes.visible.len();
        let header = panel_header("Scopes", cx).child(
            div()
                .flex()
                .gap_1()
                .child(
                    icon_button("expand-all", IconName::ChevronsRight, t.panel, t.hover, cx)
                        .tooltip("Expand all")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.dispatch(Command::ExpandAllScopes(true), Some(window), cx)
                        })),
                )
                .child(
                    icon_button("collapse-all", IconName::ChevronsLeft, t.panel, t.hover, cx)
                        .tooltip("Collapse all")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.dispatch(Command::ExpandAllScopes(false), Some(window), cx)
                        })),
                ),
        );

        let list = uniform_list(
            "scope-tree",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let t = *theme(cx);
                let Some(h) = this.app.doc.hierarchy() else {
                    return Vec::new();
                };
                range
                    .map(|ix| {
                        let (id, depth) = this.app.scopes.visible[ix];
                        let scope = &h.scopes[id];
                        let has_children = !scope.children.is_empty();
                        let expanded = this.app.scopes.is_expanded(id);
                        let selected = this.app.scopes.selected == Some(id);
                        let colors = t.row(selected, false);
                        let hover = t.hover;
                        let name: SharedString = scope.name.clone().into();
                        let mut row = div()
                            .id(("scope", ix))
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .h(px(t.row_height))
                            .pl(t.px(8.0 + 12.0 * depth as f32))
                            .pr_2()
                            .gap_1()
                            .cursor(CursorStyle::PointingHand)
                            .font_family(t.ui_font)
                            .text_size(px(t.ui_size))
                            .text_color(colors.text)
                            .on_click(cx.listener(
                                move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                                    window.focus(&this.scopes_focus, cx);
                                    this.dispatch(Command::SelectScope(id), Some(window), cx);
                                    if ev.click_count() == 2 {
                                        this.dispatch(
                                            Command::ScopesKey(Key::Enter),
                                            Some(window),
                                            cx,
                                        );
                                    }
                                },
                            ));
                        if selected {
                            row = row
                                .bg(t.selection.bg)
                                .when(t.appearance.is_high_contrast(), |row| {
                                    row.border_1().border_color(t.border_focused)
                                });
                        } else {
                            row = row.hover(move |s| s.bg(hover.bg).text_color(hover.text));
                        }
                        let chevron = div()
                            .id(("chevron", ix))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(t.px(16.0))
                            .rounded_sm()
                            .when(has_children, |el| {
                                el.cursor(CursorStyle::PointingHand)
                                    .hover(move |s| s.bg(t.badge_hover.bg))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.dispatch(Command::ToggleScope(id), Some(window), cx)
                                    }))
                                    .child(
                                        Icon::new(if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(t.px(14.0))
                                        .inherit_color(),
                                    )
                            });
                        // Scopes with variables add as a group; child scopes
                        // with variables become folded subgroups unless only
                        // this scope is asked for.
                        let addable = h.has_vars(id);
                        let nested = scope.children.iter().any(|&c| h.has_vars(c));
                        let owner = cx.weak_entity();
                        let (icon, tint) = scope_icon(scope);
                        let tooltip = format!(
                            "{} — {} {}",
                            h.scope_path(id).join("."),
                            scope.kind,
                            scope.component
                        );
                        let scope_menu = move |menu: gpui_kit::component::menu::PopupMenu,
                                               _: &mut Window,
                                               _: &mut gpui_kit::Context<
                            gpui_kit::component::menu::PopupMenu,
                        >| {
                            if !addable {
                                return menu.item(
                                    PopupMenuItem::new("No variables to add").disabled(true),
                                );
                            }
                            let add = |label: &'static str, recursive: bool| {
                                let owner = owner.clone();
                                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                    _ = owner.update(cx, |ws, cx| {
                                        ws.dispatch(
                                            Command::AddScopeAsGroup {
                                                scope: id,
                                                recursive,
                                            },
                                            Some(window),
                                            cx,
                                        )
                                    });
                                })
                            };
                            let menu = menu.item(add("Add to Waves as Group", true));
                            if nested {
                                menu.item(add("Add to Waves as Group, This Scope Only", false))
                            } else {
                                menu
                            }
                        };
                        row.tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
                            .child(chevron)
                            .child(Icon::new(icon).size(t.px(14.0)).color(tint.color(&t)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .when_some(stream_tag(scope), |row, tag| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .max_w(t.px(90.0))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(t.badge.bg)
                                        .text_color(t.badge.text)
                                        .text_size(px(t.ui_size_small))
                                        .child(SharedString::from(tag.to_owned())),
                                )
                            })
                            .context_menu(scope_menu)
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scopes_scroll)
        .flex_1()
        .size_full();

        div()
            .id("scopes-panel")
            .track_focus(&self.scopes_focus)
            .on_key_down(cx.listener(Self::scopes_key))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.panel.bg)
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
                            .text_size(px(t.ui_size_small))
                            .text_color(colors.text_placeholder)
                            .child("No scopes")
                            .into_any_element()
                    } else {
                        list.into_any_element()
                    }),
            )
    }
}
