//! Logical-pixel geometry and input types shared by every frontend.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

pub const fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

pub const fn size(width: f32, height: f32) -> Size {
    Size { width, height }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(origin: Point, size: Size) -> Rect {
        Rect { origin, size }
    }

    pub const fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }

    pub fn left(&self) -> f32 {
        self.origin.x
    }

    pub fn top(&self) -> f32 {
        self.origin.y
    }

    pub fn right(&self) -> f32 {
        self.origin.x + self.size.width
    }

    pub fn bottom(&self) -> f32 {
        self.origin.y + self.size.height
    }

    pub fn width(&self) -> f32 {
        self.size.width
    }

    pub fn height(&self) -> f32 {
        self.size.height
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.origin.x && p.x < self.right() && p.y >= self.origin.y && p.y < self.bottom()
    }

    pub fn is_empty(&self) -> bool {
        self.size.width <= 0.0 || self.size.height <= 0.0
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x0 = self.left().max(other.left());
        let y0 = self.top().max(other.top());
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        Rect::from_xywh(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
}

/// Keyboard modifiers. `platform` is Command on macOS and the Windows/Super
/// key elsewhere; [`Modifiers::secondary`] is the platform's multi-select key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub platform: bool,
}

impl Modifiers {
    /// Command on macOS, Control elsewhere.
    pub fn secondary(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.platform
        } else {
            self.control
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Pointer shape a frontend should show over a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorIcon {
    Default,
    PointingHand,
    ResizeLeftRight,
    ResizeUpDown,
}

/// Round to whole logical pixels so 1 px lines are crisp at 1x and 2x.
pub fn snap(v: f32) -> f32 {
    v.round()
}
