//! Position a GPUI Kit popup at the core's window-space menu coordinates.
use crate::theme::theme;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Entity, Focusable, IntoElement, ParentElement, Pixels, Point, Styled, Window, anchored,
    deferred, div, px,
};

pub fn popup_at(
    position: Point<Pixels>,
    menu: Entity<PopupMenu>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    // Focus after the trigger's mouse event, when the popup joins the view tree.
    let focus = menu.read(cx).focus_handle(cx);
    if !focus.contains_focused(window, cx) {
        window.focus(&focus, cx);
    }
    let t = theme(cx);
    deferred(
        anchored()
            .position(position)
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .rounded(px(4.0))
                    .when(t.appearance.is_high_contrast(), |el| {
                        el.border_1().border_color(t.border_focused)
                    })
                    .child(menu),
            ),
    )
    .with_priority(10)
}
