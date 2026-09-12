use gpui::{
    App, CursorStyle, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, RenderOnce, Styled, Window, deferred, div, px,
};

use crate::theme::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitterAxis {
    /// A vertical line separating left/right panes.
    Vertical,
    /// A horizontal line separating top/bottom panes.
    Horizontal,
}

/// A 1px divider with a wider grab zone that floats above its neighbours
/// (like VS Code's sash). It highlights on hover, shows a resize cursor, and
/// reports the start of a drag; the owner tracks the pointer afterwards with a
/// window-wide drag surface (see `Workspace`).
#[derive(IntoElement)]
pub struct Splitter {
    id: ElementId,
    axis: SplitterAxis,
    dragging: bool,
    on_drag_start: Option<Box<DragStartHandler>>,
}

impl Splitter {
    pub fn new(id: impl Into<ElementId>, axis: SplitterAxis) -> Self {
        Splitter {
            id: id.into(),
            axis,
            dragging: false,
            on_drag_start: None,
        }
    }

    pub fn dragging(mut self, dragging: bool) -> Self {
        self.dragging = dragging;
        self
    }

    pub fn on_drag_start(
        mut self,
        f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag_start = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Splitter {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);
        let grab = t.splitter_grab;
        let base_color = t.border;
        let active_color = t.border_focused;
        let cursor = match self.axis {
            SplitterAxis::Vertical => CursorStyle::ResizeLeftRight,
            SplitterAxis::Horizontal => CursorStyle::ResizeUpDown,
        };
        // The neutral 1px line occupies the layout slot. The grab zone is an
        // absolutely positioned, deferred child centred on it, so its hitbox
        // floats above both neighbouring panes (like VS Code's sash). Its own
        // centred 1px child recolours the line on hover and while dragging.
        let highlight = div().group_hover("splitter", move |s| s.bg(active_color));
        let highlight = if self.dragging {
            highlight.bg(active_color)
        } else {
            highlight
        };
        let highlight = match self.axis {
            SplitterAxis::Vertical => highlight.w(px(1.0)).h_full(),
            SplitterAxis::Horizontal => highlight.h(px(1.0)).w_full(),
        };
        let mut grab_zone = div()
            .id(self.id)
            .group("splitter")
            .absolute()
            .cursor(cursor)
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .child(highlight);
        grab_zone = match self.axis {
            SplitterAxis::Vertical => grab_zone.top_0().bottom_0().left(-(grab / 2.0)).w(grab),
            SplitterAxis::Horizontal => grab_zone.left_0().right_0().top(-(grab / 2.0)).h(grab),
        };
        if let Some(f) = self.on_drag_start {
            grab_zone = grab_zone.on_mouse_down(MouseButton::Left, f);
        }
        let line = div().bg(base_color);
        let line = match self.axis {
            SplitterAxis::Vertical => line.w(px(1.0)).h_full(),
            SplitterAxis::Horizontal => line.h(px(1.0)).w_full(),
        };
        let zone = div().relative().flex_none();
        let zone = match self.axis {
            SplitterAxis::Vertical => zone.w(px(1.0)).h_full(),
            SplitterAxis::Horizontal => zone.h(px(1.0)).w_full(),
        };
        zone.child(line).child(deferred(grab_zone).with_priority(1))
    }
}

type DragStartHandler = dyn Fn(&MouseDownEvent, &mut Window, &mut App);
