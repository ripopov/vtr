//! The variable list panel: GPUI rows over `VariableListModel`, with the
//! filter text box.

use gpui_kit::component::Disableable;
use gpui_kit::prelude::*;
use gpui_kit::{
    Context, CursorStyle, Focusable, IntoElement, KeyDownEvent, SharedString, Window, div, px,
    uniform_list,
};
use volna_core::app::Command;
use volna_core::sidebar::Key;
use volna_core::sidebar::variables::{direction_label, shape_icon};

use crate::app::{Workspace, to_modifiers};
use crate::theme::{ThemePx, theme};
use crate::ui::{Icon, IconName, icon_button, panel_header};

impl Workspace {
    fn variables_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .filter
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            return;
        }
        let ks = &ev.keystroke;
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
        let placeholder = vars.placeholder(self.app.doc.is_loaded());
        let header = panel_header("Variables", cx).child(
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
                        .disabled(count == 0)
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
                        let var = this.app.variables.rows[ix];
                        let v = &h.vars[var];
                        let selected = this.app.variables.selected.contains(&ix);
                        let colors = t.row(selected, false);
                        let hover = t.hover;
                        let dims: SharedString = v.shape.dims().into();
                        let name: SharedString = if show_scope {
                            h.full_name(var).into()
                        } else {
                            v.name.clone().into()
                        };
                        let dir = direction_label(v.direction);
                        let mut row = div()
                            .id(("var", ix))
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
                                        Command::AddVars(vec![var])
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
                        row.child(
                            Icon::new(shape_icon(v.shape))
                                .size(t.px(14.0))
                                .inherit_color(),
                        )
                        .when(show_direction, |row| {
                            row.child(
                                div()
                                    .w(t.px(24.0))
                                    .flex_none()
                                    .text_size(px(t.ui_size_small))
                                    .child(SharedString::from(dir)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(name),
                        )
                        .child(div().flex_none().child(dims))
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
            .child(div().flex_none().px_2().py_1().child(self.filter.clone()))
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
    }
}
