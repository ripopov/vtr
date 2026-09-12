use gpui::{
    AnyView, App, ClickEvent, CursorStyle, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::icon::{Icon, IconName};
use crate::theme::theme;

/// A square ghost icon button with hover, active, selected and disabled states.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    selected: bool,
    disabled: bool,
    color: Option<Hsla>,
    tooltip: Option<Box<TooltipBuilder>>,
    on_click: Option<Box<ClickHandler>>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        IconButton {
            id: id.into(),
            icon,
            selected: false,
            disabled: false,
            color: None,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx).panel();
        let icon_color = if self.disabled {
            t.text_placeholder
        } else if self.selected {
            t.icon_accent
        } else {
            self.color.unwrap_or(t.icon_muted)
        };
        let size = px(24.0);
        let mut el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .rounded_md()
            .child(Icon::new(self.icon).color(icon_color));
        if self.selected {
            el = el.bg(t.element_selected);
        }
        if !self.disabled {
            let hover = t.ghost_element_hover;
            let active = t.element_active;
            let text = t.icon;
            el = el
                .cursor(CursorStyle::PointingHand)
                .hover(move |s| s.bg(hover).text_color(text))
                .active(move |s| s.bg(active));
            if let Some(f) = self.on_click {
                el = el.on_click(f);
            }
            if let Some(tip) = self.tooltip {
                el = el.tooltip(tip);
            }
        }
        el
    }
}

/// A labelled button (filled when `primary`).
#[derive(IntoElement)]
pub struct TextButton {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    primary: bool,
    on_click: Option<Box<ClickHandler>>,
}

impl TextButton {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        TextButton {
            id: id.into(),
            label: label.into(),
            icon: None,
            primary: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn primary(mut self, primary: bool) -> Self {
        self.primary = primary;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for TextButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx).panel();
        let (bg, fg, hover, active, border) = if self.primary {
            (
                t.button_bg,
                t.button_text,
                t.button_hover,
                t.button_bg,
                if t.high_contrast {
                    t.border_focused
                } else {
                    t.button_bg
                },
            )
        } else {
            (
                t.bg_panel,
                t.text,
                t.element_hover,
                t.element_active,
                t.border,
            )
        };
        let mut el = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.0))
            .px_3()
            .rounded_md()
            .bg(bg)
            .border_1()
            .border_color(border)
            .font_family(t.ui_font.clone())
            .text_size(t.ui_size)
            .text_color(fg)
            .cursor(CursorStyle::PointingHand)
            .hover(move |s| {
                s.bg(hover)
                    .text_color(crate::theme::readable(fg, hover, 4.5))
            })
            .active(move |s| {
                s.bg(active)
                    .text_color(crate::theme::readable(fg, active, 4.5))
            });
        if let Some(icon) = self.icon {
            el = el.child(Icon::new(icon).color(fg));
        }
        el = el.child(self.label);
        if let Some(f) = self.on_click {
            el = el.on_click(f);
        }
        el
    }
}

type TooltipBuilder = dyn Fn(&mut Window, &mut App) -> AnyView;

type ClickHandler = dyn Fn(&ClickEvent, &mut Window, &mut App);
