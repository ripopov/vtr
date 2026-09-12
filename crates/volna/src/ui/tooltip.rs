use gpui::prelude::*;
use gpui::{
    AnyView, App, Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    px,
};

use crate::theme::theme;

/// A plain text tooltip with an optional keyboard shortcut hint.
pub struct Tooltip {
    text: SharedString,
    shortcut: Option<SharedString>,
}

impl Tooltip {
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        move |_, cx| {
            cx.new(|_| Tooltip {
                text: text.clone(),
                shortcut: None,
            })
            .into()
        }
    }

    pub fn with_shortcut(
        text: impl Into<SharedString>,
        shortcut: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        let shortcut = shortcut.into();
        move |_, cx| {
            cx.new(|_| Tooltip {
                text: text.clone(),
                shortcut: Some(shortcut.clone()),
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(24.0))
            .rounded_md()
            .bg(t.bg_elevated)
            .border_1()
            .border_color(t.border)
            .shadow_md()
            .font_family(t.ui_font.clone())
            .text_size(t.ui_size_small)
            .text_color(t.text)
            .child(self.text.clone())
            .when_some(self.shortcut.clone(), |el, s| {
                el.child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(t.element_active)
                        .text_color(t.text_muted)
                        .font_family(t.mono_font.clone())
                        .child(s),
                )
            })
    }
}
