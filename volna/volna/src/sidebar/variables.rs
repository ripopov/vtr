//! The variable list panel: GPUI rows over `MemberListModel`, with the
//! filter text box.

use gpui_kit::component::Disableable;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, CursorStyle, Focusable, IntoElement, KeyDownEvent, SharedString, Window, div, px,
    uniform_list,
};
use volna_core::app::Command;
use volna_core::sidebar::Key;
use volna_core::sidebar::icons::{direction_icon, member_icon};
use volna_core::sidebar::members::{describe, log_site, member_detail};

use crate::app::{Workspace, to_modifiers};
use crate::theme::{ThemePx, theme};
use crate::ui::{Icon, IconName, icon_button, panel_header};

impl Workspace {
    fn variables_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.key == "tab" {
            window.focus(&self.scopes_focus, cx);
            cx.stop_propagation();
            return;
        }
        if self
            .filter
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            if ev.keystroke.key == "down" {
                window.focus(&self.variables_focus, cx);
                self.dispatch(
                    Command::VariablesKey(Key::Down, to_modifiers(ev.keystroke.modifiers)),
                    Some(window),
                    cx,
                );
                cx.stop_propagation();
            }
            return;
        }
        let ks = &ev.keystroke;
        if ks.key == "f10" && ks.modifiers.shift {
            let selected = self
                .app
                .variables
                .selected
                .iter()
                .filter_map(|&index| self.app.variables.rows.get(index).copied())
                .collect();
            self.dispatch(
                Command::OpenTable {
                    selected,
                    clicked: None,
                },
                Some(window),
                cx,
            );
            cx.stop_propagation();
            return;
        }
        let key = match ks.key.as_str() {
            "enter" => Key::Enter,
            "down" => Key::Down,
            "up" => Key::Up,
            "escape" => Key::Escape,
            _ => {
                // Typing starts filtering: hand the character to the text box.
                if let Some(c) = ks.key_char.as_deref().filter(|c| !c.is_empty())
                    && !ks.modifiers.platform
                    && !ks.modifiers.control
                {
                    let handle = self.filter.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                    let c = c.to_owned();
                    self.filter.update(cx, |f, cx| f.insert(&c, cx));
                    cx.stop_propagation();
                }
                return;
            }
        };
        self.dispatch(
            Command::VariablesKey(key, to_modifiers(ks.modifiers)),
            Some(window),
            cx,
        );
        cx.stop_propagation();
    }

    pub(crate) fn render_variables(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.panel;
        let focused = self.variables_focus.is_focused(window);
        let vars = &self.app.variables;
        let count = vars.rows.len();
        let show_scope = vars.show_scope();
        let show_direction = self
            .app
            .doc
            .hierarchy()
            .is_some_and(|h| vars.show_direction(h));
        let placeholder = vars.placeholder(self.app.doc.hierarchy());
        let breadcrumb = self
            .app
            .doc
            .hierarchy()
            .map(|h| vars.breadcrumb(h))
            .unwrap_or_default();
        let everywhere = vars.search_everywhere;
        let truncated = vars.truncated;
        let header = panel_header(vars.title(self.app.doc.hierarchy()), cx).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .font_family(t.mono_font)
                        .text_size(px(t.ui_size_small))
                        .text_color(colors.text_placeholder)
                        .child(SharedString::from(count.to_string())),
                )
                .child(
                    icon_button("add-all", IconName::Plus, t.panel, t.hover, cx)
                        .disabled(!vars.rows.iter().any(|m| m.var().is_some()))
                        .tooltip("Add all listed variables (⏎)")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.dispatch(Command::AddAllVars, Some(window), cx)
                        })),
                ),
        );

        let list = uniform_list(
            "variable-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let t = *theme(cx);
                let Some(h) = this.app.doc.hierarchy() else {
                    return Vec::new();
                };
                range
                    .map(|ix| {
                        let owner = cx.entity().downgrade();
                        let member = this.app.variables.rows[ix];
                        let selected = this.app.variables.selected.contains(&ix);
                        let colors = t.row(selected, false);
                        let hover = t.hover;
                        let dims: SharedString = member_detail(h, member).into();
                        let name: SharedString = if show_scope {
                            h.member_path(member).into()
                        } else {
                            h.member_name(member).to_owned().into()
                        };
                        let dir = member
                            .var()
                            .and_then(|id| direction_icon(h.vars[id].direction));
                        let tooltip = describe(h, member);
                        let severity = log_site(h, member).map(|s| s.severity);
                        let mut row = div()
                            .id(("var", ix))
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .h(px(t.row_height))
                            .px_2()
                            .gap_2()
                            .cursor(CursorStyle::PointingHand)
                            .font_family(t.mono_font)
                            .text_size(px(t.mono_size))
                            .text_color(colors.text)
                            .on_click(cx.listener(
                                move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                                    window.focus(&this.variables_focus, cx);
                                    let command = if ev.click_count() == 2 {
                                        Command::ActivateMembers(vec![member])
                                    } else {
                                        Command::SelectVar {
                                            ix,
                                            modifiers: to_modifiers(ev.modifiers()),
                                        }
                                    };
                                    this.dispatch(command, Some(window), cx);
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
                        row.tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
                            .child(
                                Icon::new(member_icon(h, member))
                                    .size(t.px(14.0))
                                    .inherit_color(),
                            )
                            .when(show_direction, |row| {
                                row.child(
                                    div()
                                        .w(t.px(24.0))
                                        .flex_none()
                                        .text_size(px(t.ui_size_small))
                                        .when_some(dir, |el, (icon, tint)| {
                                            el.child(
                                                Icon::new(icon)
                                                    .size(t.px(14.0))
                                                    .color(tint.color(&t)),
                                            )
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .when_some(severity, |row, severity| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px_1()
                                        .rounded_sm()
                                        .bg(t.badge.bg)
                                        .text_color(t.badge.text)
                                        .text_size(px(t.ui_size_small))
                                        .child(SharedString::from(severity)),
                                )
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(t.px(140.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(colors.text_muted)
                                    .child(dims),
                            )
                            .context_menu(move |menu, _, _cx| {
                                let owner = owner.clone();
                                menu.item(PopupMenuItem::new("Open in table").on_click(
                                    move |_, window, cx| {
                                        _ = owner.update(cx, |workspace, cx| {
                                            let selected = workspace
                                                .app
                                                .variables
                                                .selected
                                                .iter()
                                                .filter_map(|&index| {
                                                    workspace.app.variables.rows.get(index).copied()
                                                })
                                                .collect();
                                            workspace.dispatch(
                                                Command::OpenTable {
                                                    selected,
                                                    clicked: Some(member),
                                                },
                                                Some(window),
                                                cx,
                                            );
                                        });
                                    },
                                ))
                            })
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.variables_scroll)
        .flex_1()
        .size_full();

        div()
            .id("variables-panel")
            .track_focus(&self.variables_focus)
            .on_key_down(cx.listener(Self::variables_key))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.panel.bg)
            .child(header)
            .when(!breadcrumb.is_empty(), |el| {
                el.child(
                    div()
                        .px_2()
                        .py_1()
                        .font_family(t.mono_font)
                        .text_size(px(t.ui_size_small))
                        .text_color(colors.text_muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(breadcrumb)),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.filter.clone()))
                    .child(
                        icon_button(
                            "search-everywhere",
                            IconName::Search,
                            if everywhere { t.selection } else { t.panel },
                            t.hover,
                            cx,
                        )
                        .tooltip("Search everywhere: variables, generators and streams")
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.dispatch(
                                    Command::SetSearchEverywhere(!everywhere),
                                    Some(window),
                                    cx,
                                );
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(focused, |el| el.border_1().border_color(t.border_focused))
                    .child(match placeholder {
                        Some(text) => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_4()
                            .text_align(gpui_kit::TextAlign::Center)
                            .text_size(px(t.ui_size_small))
                            .text_color(colors.text_placeholder)
                            .child(text)
                            .into_any_element(),
                        None => list.into_any_element(),
                    }),
            )
            .when(truncated, |el| {
                el.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(t.ui_size_small))
                        .text_color(colors.text_muted)
                        .child("First 5,000 matches · refine your search"),
                )
            })
    }
}
