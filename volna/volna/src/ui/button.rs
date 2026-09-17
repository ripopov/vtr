//! Volna's compact icon-button styling over GPUI Kit's interaction component.
use super::IconName;
use crate::theme::{Surface, ThemePx, theme};
use gpui_kit::component::{
    Sizable,
    button::{Button, ButtonCustomVariant, ButtonVariants},
};
use gpui_kit::{App, ElementId, Styled};

pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    normal: Surface,
    hover: Surface,
    cx: &App,
) -> Button {
    Button::new(id)
        .icon(gpui_kit::component::Icon::empty().path(icon.path()))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(gpui_kit::transparent_black())
                .foreground(normal.icon_muted)
                .hover(hover.bg)
                .active(hover.bg),
        )
        .xsmall()
        .size(theme(cx).px(24.0))
}
