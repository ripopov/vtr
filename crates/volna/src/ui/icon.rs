use gpui::{App, Hsla, IntoElement, Pixels, RenderOnce, Styled, Svg, Window, svg};

use crate::theme::theme;

/// The single icon set (Lucide). Adding an icon: drop the SVG in `assets/icons`,
/// list it in `assets.rs` and add a variant here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconName {
    Activity,
    AudioWaveform,
    Binary,
    Box,
    Braces,
    ChevronDown,
    ChevronRight,
    ChevronsLeft,
    ChevronsRight,
    CircleDot,
    Folder,
    FolderOpen,
    LoaderCircle,
    Locate,
    PanelLeft,
    Plus,
    Search,
    Sigma,
    TriangleAlert,
    Type,
    X,
}

impl IconName {
    pub fn path(self) -> &'static str {
        use IconName::*;
        match self {
            Activity => "icons/activity.svg",
            AudioWaveform => "icons/audio-waveform.svg",
            Binary => "icons/binary.svg",
            Box => "icons/box.svg",
            Braces => "icons/braces.svg",
            ChevronDown => "icons/chevron-down.svg",
            ChevronRight => "icons/chevron-right.svg",
            ChevronsLeft => "icons/chevrons-left.svg",
            ChevronsRight => "icons/chevrons-right.svg",
            CircleDot => "icons/circle-dot.svg",
            Folder => "icons/folder.svg",
            FolderOpen => "icons/folder-open.svg",
            LoaderCircle => "icons/loader-circle.svg",
            Locate => "icons/locate.svg",
            PanelLeft => "icons/panel-left.svg",
            Plus => "icons/plus.svg",
            Search => "icons/search.svg",
            Sigma => "icons/sigma.svg",
            TriangleAlert => "icons/triangle-alert.svg",
            Type => "icons/type.svg",
            X => "icons/x.svg",
        }
    }
}

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
        let size = self.size.unwrap_or(t.icon_size);
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
