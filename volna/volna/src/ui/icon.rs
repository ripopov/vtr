use gpui::{App, Hsla, IntoElement, Pixels, RenderOnce, Styled, Svg, Window, svg};

pub use volna_core::icons::IconName;

use crate::theme::theme;

#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    color: Option<Hsla>,
    inherit: bool,
    size: Option<Pixels>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Icon {
            name,
            color: None,
            inherit: false,
            size: None,
        }
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn inherit_color(mut self) -> Self {
        self.inherit = true;
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = Some(size);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let size = self.size.unwrap_or(gpui::px(t.icon_size));
        // GPUI SVGs require an explicit colour; an unset SVG colour does not
        // inherit and suppresses painting entirely.
        let color = self.color.unwrap_or_else(|| {
            if self.inherit {
                window.text_style().color
            } else {
                t.panel.icon_muted
            }
        });
        svg()
            .path(self.name.path())
            .size(size)
            .flex_none()
            .text_color(color)
    }
}

/// Bare `svg()` for callers that need further styling (animations).
pub fn icon_svg(name: IconName, size: Pixels, color: Hsla) -> Svg {
    svg()
        .path(name.path())
        .size(size)
        .flex_none()
        .text_color(color)
}
