use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    Context, CursorStyle, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    Render, SharedString, UniformListScrollHandle, Window, div, px, uniform_list,
};

use crate::data::{Direction, ScopeId, SignalShape, VarId, WaveSource};
use crate::theme::theme;
use crate::ui::{
    Icon, IconButton, IconName, TextInput, Tooltip, panel_header, text_input::TextInputEvent,
};

pub enum VariableListEvent {
    Add(Vec<VarId>),
}

pub struct VariableList {
    source: Option<Arc<dyn WaveSource>>,
    scope: Option<ScopeId>,
    filter: Entity<TextInput>,
    rows: Vec<VarId>,
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<VariableListEvent> for VariableList {}

impl Focusable for VariableList {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

const MAX_SEARCH_ROWS: usize = 5000;

impl VariableList {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextInput::new("Filter variables", cx));
        cx.subscribe(&filter, |this, _, event, cx| match event {
            TextInputEvent::Changed => this.rebuild(cx),
            TextInputEvent::Submit => this.add_selected_or_all(cx),
            TextInputEvent::Cancel => {}
        })
        .detach();
        VariableList {
            source: None,
            scope: None,
            filter,
            rows: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_source(&mut self, source: Option<Arc<dyn WaveSource>>, cx: &mut Context<Self>) {
        self.source = source;
        self.scope = None;
        self.filter.update(cx, |f, cx| f.clear(cx));
        self.rebuild(cx);
    }

    pub fn set_scope(&mut self, scope: Option<ScopeId>, cx: &mut Context<Self>) {
        self.scope = scope;
        self.rebuild(cx);
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.rows.clear();
        self.selected.clear();
        self.anchor = None;
        let filter = self.filter.read(cx).text().to_lowercase();
        if let Some(src) = &self.source {
            let h = src.hierarchy();
            let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
            match self.scope {
                Some(s) => {
                    self.rows.extend(
                        h.scopes[s]
                            .vars
                            .iter()
                            .copied()
                            .filter(|&v| matches(&h.vars[v].name)),
                    );
                }
                None if !filter.is_empty() => {
                    self.rows.extend(
                        (0..h.vars.len())
                            .filter(|&v| matches(&h.vars[v].name))
                            .take(MAX_SEARCH_ROWS),
                    );
                }
                None => {}
            }
        }
        cx.notify();
    }

    fn add_selected_or_all(&mut self, cx: &mut Context<Self>) {
        let vars: Vec<VarId> = if self.selected.is_empty() {
            self.rows.clone()
        } else {
            self.selected.iter().map(|&i| self.rows[i]).collect()
        };
        if !vars.is_empty() {
            cx.emit(VariableListEvent::Add(vars));
        }
    }

    fn add_all(&mut self, cx: &mut Context<Self>) {
        if !self.rows.is_empty() {
            cx.emit(VariableListEvent::Add(self.rows.clone()));
        }
    }

    fn select(&mut self, ix: usize, modifiers: gpui::Modifiers, cx: &mut Context<Self>) {
        crate::ui::selection::select(&mut self.selected, &mut self.anchor, ix, modifiers);
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "enter" => self.add_selected_or_all(cx),
            "down" | "up" => {
                if self.rows.is_empty() {
                    return;
                }
                let cur = self
                    .anchor
                    .unwrap_or(if ks.key == "down" { usize::MAX } else { 0 });
                let next = if ks.key == "down" {
                    cur.wrapping_add(1).min(self.rows.len() - 1)
                } else {
                    cur.saturating_sub(1)
                };
                let next = if cur == usize::MAX { 0 } else { next };
                if ks.modifiers.shift {
                    self.selected.insert(next);
                } else {
                    self.selected.clear();
                    self.selected.insert(next);
                }
                self.anchor = Some(next);
                self.scroll
                    .scroll_to_item(next, gpui::ScrollStrategy::Nearest);
                cx.notify();
            }
            "escape" => {
                self.selected.clear();
                cx.notify();
            }
            _ => {
                // Typing starts filtering.
                if ks.key_char.as_deref().is_some_and(|c| !c.is_empty())
                    && !ks.modifiers.platform
                    && !ks.modifiers.control
                {
                    let handle = self.filter.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                    self.filter.update(cx, |f, cx| {
                        // Re-dispatch the character to the input.
                        f.insert(ks.key_char.as_deref().unwrap_or(""), cx);
                    });
                }
            }
        }
    }
}

fn shape_icon(shape: SignalShape) -> IconName {
    match shape {
        SignalShape::Bit => IconName::Activity,
        SignalShape::Vector { .. } => IconName::Binary,
        SignalShape::Real => IconName::Sigma,
        SignalShape::Text => IconName::Type,
    }
}

fn direction_label(d: Direction) -> &'static str {
    match d {
        Direction::None => "",
        Direction::Input => "in",
        Direction::Output => "out",
        Direction::InOut => "io",
    }
}

impl Render for VariableList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let focused = self.focus_handle.is_focused(window);
        let count = self.rows.len();
        let searching = !self.filter.read(cx).text().is_empty();
        let show_scope = self.scope.is_none() && searching;
        let show_direction = self.source.as_ref().is_some_and(|s| {
            self.rows
                .iter()
                .any(|&v| s.hierarchy().vars[v].direction != Direction::None)
        });
        let header = panel_header("Variables", cx).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .font_family(t.mono_font.clone())
                        .text_size(t.ui_size_small)
                        .text_color(t.text_placeholder)
                        .child(SharedString::from(count.to_string())),
                )
                .child(
                    IconButton::new("add-all", IconName::Plus)
                        .disabled(count == 0)
                        .tooltip(Tooltip::with_shortcut("Add all listed variables", "⏎"))
                        .on_click(cx.listener(|this, _, _, cx| this.add_all(cx))),
                ),
        );

        let list = uniform_list(
            "variable-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let t = theme(cx).clone();
                let Some(src) = this.source.clone() else {
                    return Vec::new();
                };
                let h = src.hierarchy();
                range
                    .map(|ix| {
                        let var = this.rows[ix];
                        let v = &h.vars[var];
                        let selected = this.selected.contains(&ix);
                        let hover = t.element_hover;
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
                            .h(t.row_height)
                            .px_2()
                            .gap_2()
                            .cursor(CursorStyle::PointingHand)
                            .font_family(t.mono_font.clone())
                            .text_size(t.mono_size)
                            .text_color(t.text)
                            .on_click(cx.listener(
                                move |this, ev: &gpui::ClickEvent, window, cx| {
                                    window.focus(&this.focus_handle, cx);
                                    if ev.click_count() == 2 {
                                        cx.emit(VariableListEvent::Add(vec![var]));
                                    } else {
                                        this.select(ix, ev.modifiers(), cx);
                                    }
                                },
                            ));
                        if selected {
                            row = row.bg(t.element_selected);
                        } else {
                            row = row.hover(move |s| s.bg(hover));
                        }
                        row.child(Icon::new(shape_icon(v.shape)).size(px(14.0)).color(
                            if selected {
                                t.icon_accent
                            } else {
                                t.icon_muted
                            },
                        ))
                        .when(show_direction, |row| {
                            row.child(
                                div()
                                    .w(px(24.0))
                                    .flex_none()
                                    .text_size(t.ui_size_small)
                                    .text_color(t.text_placeholder)
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
                        .child(div().flex_none().text_color(t.text_placeholder).child(dims))
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .size_full();

        let placeholder: Option<&str> = if self.source.is_none() {
            Some("Open a trace to browse variables")
        } else if self.scope.is_none() && !searching {
            Some("Select a scope, or type to search all variables")
        } else if count == 0 {
            Some("No variables match")
        } else {
            None
        };

        div()
            .id("variables-panel")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg_panel)
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
                            .text_align(gpui::TextAlign::Center)
                            .text_size(t.ui_size_small)
                            .text_color(t.text_placeholder)
                            .child(text)
                            .into_any_element(),
                        None => list.into_any_element(),
                    }),
            )
    }
}
