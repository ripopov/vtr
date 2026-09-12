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
        let t = theme(cx).clone();
        let items = self.items.iter().enumerate().map(|(ix, item)| {
            let id = item.id.clone();
            let hover = t.element_hover;
            let active = t.element_active;
            div()
                .id(("menu-item", ix))
                .flex()
                .items_center()
                .gap_2()
                .h(px(24.0))
                .px_2()
                .mx_1()
                .rounded_sm()
                .cursor(CursorStyle::PointingHand)
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(active))
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
                                    .color(t.icon_accent),
                            )
                        }),
                )
                .child(div().flex_1().text_color(t.text).child(item.label.clone()))
                .when_some(item.badge.clone(), |el, b| {
                    el.child(
                        div()
                            .font_family(t.mono_font.clone())
                            .text_size(t.ui_size_small)
                            .text_color(t.text_muted)
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
                        .bg(t.bg_elevated)
                        .border_1()
                        .border_color(t.border)
                        .shadow_lg()
                        .font_family(t.ui_font.clone())
                        .text_size(t.ui_size)
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
