//! A minimal single-line text field (filter box). Handles printable input,
//! backspace, word delete and escape. No IME or selection; enough for a filter.

use gpui::prelude::*;
use gpui::{
    App, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};

use super::icon::{Icon, IconName};
use crate::theme::theme;

pub enum TextInputEvent {
    Changed,
    /// Enter was pressed.
    Submit,
    /// Escape was pressed with empty text.
    Cancel,
}

pub struct TextInput {
    text: String,
    placeholder: SharedString,
    focus_handle: FocusHandle,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        TextInput {
            text: String::new(),
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.text.is_empty() {
            self.text.clear();
            cx.emit(TextInputEvent::Changed);
            cx.notify();
        }
    }

    /// Append text (used when another view forwards a typed character).
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() || text.chars().any(|c| c.is_control()) {
            return;
        }
        self.text.push_str(text);
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "backspace" => {
                if ks.modifiers.alt || ks.modifiers.platform {
                    let trimmed = self.text.trim_end().len();
                    let cut = self.text[..trimmed].rfind(' ').map(|i| i + 1).unwrap_or(0);
                    self.text.truncate(cut);
                } else {
                    self.text.pop();
                }
                cx.emit(TextInputEvent::Changed);
                cx.notify();
            }
            "escape" => {
                if self.text.is_empty() {
                    cx.emit(TextInputEvent::Cancel);
                } else {
                    self.clear(cx);
                }
            }
            "enter" => cx.emit(TextInputEvent::Submit),
            _ => {
                if ks.modifiers.platform || ks.modifiers.control {
                    return;
                }
                if let Some(c) = ks.key_char.as_deref()
                    && !c.chars().any(|ch| ch.is_control())
                {
                    self.text.push_str(c);
                    cx.emit(TextInputEvent::Changed);
                    cx.notify();
                }
            }
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx).clone();
        let focused = self.focus_handle.is_focused(window);
        let empty = self.text.is_empty();
        let border = if focused { t.border_focused } else { t.border };
        let hover_border = t.border_focused;
        div()
            .id("text-input")
            .track_focus(&self.focus_handle)
            .flex()
            .items_center()
            .gap_2()
            .h(px(24.0))
            .px_2()
            .rounded_md()
            .bg(t.bg_editor)
            .border_1()
            .border_color(border)
            .hover(move |s| s.border_color(hover_border))
            .cursor(CursorStyle::IBeam)
            .font_family(t.ui_font.clone())
            .text_size(t.ui_size)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_click(cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)))
            .child(Icon::new(IconName::Search).size(px(14.0)))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(if empty {
                        div()
                            .text_color(t.text_placeholder)
                            .child(self.placeholder.clone())
                    } else {
                        div()
                            .text_color(t.text)
                            .child(SharedString::from(self.text.clone()))
                    })
                    .when(focused, |el| {
                        el.child(div().w(px(1.0)).h(px(14.0)).bg(t.text))
                    }),
            )
            .when(!empty, |el| {
                el.child(
                    div()
                        .id("clear")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(16.0))
                        .rounded_sm()
                        .cursor(CursorStyle::PointingHand)
                        .hover(move |s| s.bg(t.element_hover))
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
                        .child(Icon::new(IconName::X).size(px(12.0))),
                )
            })
    }
}
