use gpui::prelude::*;
use gpui::{
    Context, CursorStyle, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, px,
};

use super::icon::{Icon, IconName};
use crate::theme::theme;

#[derive(Clone, Debug)]
pub struct PopupMenuItem {
    pub id: SharedString,
    pub label: SharedString,
    pub badge: Option<SharedString>,
    pub checked: bool,
}

pub enum PopupMenuEvent {
    Selected(SharedString),
    Dismissed,
}

/// A floating list of choices anchored at a window position.
pub struct PopupMenu {
    position: Point<Pixels>,
    items: Vec<PopupMenuItem>,
    focus_handle: FocusHandle,
}

impl EventEmitter<PopupMenuEvent> for PopupMenu {}

impl PopupMenu {
    pub fn new(position: Point<Pixels>, items: Vec<PopupMenuItem>, cx: &mut Context<Self>) -> Self {
        PopupMenu {
            position,
            items,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }
}

impl Render for PopupMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.elevated;
        let items = self.items.iter().enumerate().map(|(ix, item)| {
            let id = item.id.clone();
            let hover = t.menu_hover;
            let active = t.menu_hover;
            div()
                .id(("menu-item", ix))
                .flex()
                .items_center()
                .gap_2()
                .h(px(24.0))
                .px_2()
                .mx_1()
                .rounded_sm()
                .text_color(colors.text)
                .cursor(CursorStyle::PointingHand)
                .hover(move |s| s.bg(hover.bg).text_color(hover.text))
                .active(move |s| s.bg(active.bg).text_color(active.text))
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(PopupMenuEvent::Selected(id.clone()))),
                )
                .child(
                    div()
                        .w(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(item.checked, |el| {
                            el.child(
                                Icon::new(IconName::CircleDot)
                                    .size(px(12.0))
                                    .inherit_color(),
                            )
                        }),
                )
                .child(div().flex_1().child(item.label.clone()))
                .when_some(item.badge.clone(), |el, b| {
                    el.child(
                        div()
                            .font_family(t.mono_font)
                            .text_size(px(t.ui_size_small))
                            .child(b),
                    )
                })
        });
        deferred(
            anchored()
                .position(self.position)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .id("popup-menu")
                        .track_focus(&self.focus_handle)
                        .occlude()
                        .flex()
                        .flex_col()
                        .py_1()
                        .min_w(px(180.0))
                        .rounded_md()
                        .bg(t.elevated.bg)
                        .border_1()
                        .border_color(t.border)
                        .shadow_lg()
                        .font_family(t.ui_font)
                        .text_size(px(t.ui_size))
                        .on_mouse_down_out(
                            cx.listener(|_, _, _, cx| cx.emit(PopupMenuEvent::Dismissed)),
                        )
                        .on_key_down(cx.listener(|_, ev: &gpui::KeyDownEvent, _, cx| {
                            if ev.keystroke.key == "escape" {
                                cx.emit(PopupMenuEvent::Dismissed);
                            }
                        }))
                        .children(items),
                ),
        )
        .with_priority(10)
    }
}
