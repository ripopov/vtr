use gpui::{App, Div, ParentElement, SharedString, Styled, div, px};

use crate::theme::theme;

/// A panel header: fixed height, uppercase small label, 1px bottom border.
/// Callers append trailing controls with `.child(...)`.
pub fn panel_header(title: impl Into<SharedString>, cx: &App) -> Div {
    let t = *theme(cx);
    let colors = t.panel;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .h(px(t.header_height))
        .px_3()
        .bg(t.panel.bg)
        .border_b_1()
        .border_color(t.border_variant)
        .child(
            div()
                .font_family(t.ui_font)
                .text_size(px(t.ui_size_small))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors.text_muted)
                .child(title.into().to_uppercase()),
        )
}
