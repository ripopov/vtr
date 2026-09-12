use gpui::{
    AnyView, App, ClickEvent, CursorStyle, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::icon::{Icon, IconName};
use crate::theme::{Surface, theme};

/// A square ghost icon button with hover, active, selected and disabled states.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    selected: bool,
    disabled: bool,
    color: Option<Hsla>,
    surfaces: Option<(Surface, Surface, Surface)>,
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
            surfaces: None,
            tooltip: None,
            on_click: None,
        }
    }

    pub fn surfaces(mut self, normal: Surface, hover: Surface, selected: Surface) -> Self {
        self.surfaces = Some((normal, hover, selected));
        self
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
        let t = theme(cx);
        let (colors, hover, selected) = self.surfaces.unwrap_or((t.panel, t.hover, t.selection));
        let icon_color = if self.disabled {
            colors.text_placeholder
        } else if self.selected {
            selected.icon_accent
        } else {
            self.color.unwrap_or(colors.icon_muted)
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
            .text_color(icon_color)
            .child(Icon::new(self.icon).inherit_color());
        if self.selected {
            el = el.bg(selected.bg);
        }
        if !self.disabled {
            let active = selected;

            el = el
                .cursor(CursorStyle::PointingHand)
                .hover(move |s| s.bg(hover.bg).text_color(hover.icon))
                .active(move |s| s.bg(active.bg).text_color(active.icon));
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
        let t = theme(cx);
        let normal = if self.primary { t.button } else { t.panel };
        let hover = if self.primary {
            t.button_hover
        } else {
            t.hover
        };
        let active = if self.primary { t.button } else { t.selection };
        let border = if self.primary && !t.appearance.is_high_contrast() {
            normal.bg
        } else if t.appearance.is_high_contrast() {
            t.border_focused
        } else {
            t.border
        };
        let mut el = div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.0))
            .px_3()
            .rounded_md()
            .bg(normal.bg)
            .border_1()
            .border_color(border)
            .font_family(t.ui_font)
            .text_size(t.ui_size)
            .text_color(normal.text)
            .cursor(CursorStyle::PointingHand)
            .hover(move |s| s.bg(hover.bg).text_color(hover.text))
            .active(move |s| s.bg(active.bg).text_color(active.text));
        if let Some(icon) = self.icon {
            el = el.child(Icon::new(icon).inherit_color());
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
