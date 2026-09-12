//! The display list a canvas view produces and a frontend paints.
//!
//! Primitives carry resolved colours (the core theme resolves them) and font
//! *roles*; a frontend maps roles to its own font handles. Text widths come back
//! through [`TextMeasure`], cached per string in [`TextCache`] so the core never
//! shapes the same label twice.

use std::collections::HashMap;

use crate::color::Color;
use crate::geometry::{CursorIcon, Point, Rect};
use crate::icons::IconName;

/// A typeface role. The theme names the family and size for each role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontRole {
    Ui,
    UiMedium,
    UiSemibold,
    Mono,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    /// Filled rectangle with optional rounded corners and a 1 px-style border.
    Quad {
        rect: Rect,
        fill: Color,
        radius: f32,
        border_width: f32,
        border_color: Color,
    },
    /// Stroked line segments of one colour.
    Lines {
        segments: Vec<[Point; 2]>,
        color: Color,
        width: f32,
    },
    /// One line of text, vertically centred in a box of `height` starting at `origin`.
    Text {
        origin: Point,
        height: f32,
        text: String,
        font: FontRole,
        size: f32,
        color: Color,
    },
    /// A monochrome icon fitted to `rect`.
    Icon {
        name: IconName,
        rect: Rect,
        color: Color,
    },
    /// Clip subsequent primitives until the matching [`Prim::PopClip`].
    PushClip(Rect),
    PopClip,
}

#[derive(Default, Debug)]
pub struct Scene {
    pub prims: Vec<Prim>,
    /// Regions that want a specific pointer shape when hovered.
    pub cursors: Vec<(Rect, CursorIcon)>,
    /// A pointer shape that overrides everything (active drags).
    pub window_cursor: Option<CursorIcon>,
}

impl Scene {
    pub fn clear(&mut self) {
        self.prims.clear();
        self.cursors.clear();
        self.window_cursor = None;
    }

    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.prims.push(Prim::Quad {
            rect,
            fill: color,
            radius: 0.0,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        });
    }

    pub fn quad(
        &mut self,
        rect: Rect,
        fill: Color,
        radius: f32,
        border_width: f32,
        border_color: Color,
    ) {
        self.prims.push(Prim::Quad {
            rect,
            fill,
            radius,
            border_width,
            border_color,
        });
    }

    pub fn text(
        &mut self,
        origin: Point,
        height: f32,
        text: impl Into<String>,
        font: FontRole,
        size: f32,
        color: Color,
    ) {
        self.prims.push(Prim::Text {
            origin,
            height,
            text: text.into(),
            font,
            size,
            color,
        });
    }

    pub fn icon(&mut self, name: IconName, rect: Rect, color: Color) {
        self.prims.push(Prim::Icon { name, rect, color });
    }

    pub fn lines(&mut self, segments: Vec<[Point; 2]>, color: Color, width: f32) {
        if !segments.is_empty() {
            self.prims.push(Prim::Lines {
                segments,
                color,
                width,
            });
        }
    }

    pub fn clipped(&mut self, rect: Rect, f: impl FnOnce(&mut Scene)) {
        self.prims.push(Prim::PushClip(rect));
        f(self);
        self.prims.push(Prim::PopClip);
    }

    /// Filled quads only, for tests.
    pub fn quads(&self) -> impl Iterator<Item = (Rect, Color)> + '_ {
        self.prims.iter().filter_map(|p| match p {
            Prim::Quad { rect, fill, .. } => Some((*rect, *fill)),
            _ => None,
        })
    }

    /// Text runs only, for tests.
    pub fn texts(&self) -> impl Iterator<Item = &str> + '_ {
        self.prims.iter().filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }
}

/// Implemented by a frontend with its text system.
pub trait TextMeasure {
    /// Advance width in logical pixels of `text` set in `font` at `size`.
    fn text_width(&mut self, text: &str, font: FontRole, size: f32) -> f32;
}

/// Width measurements keyed by string, role and size. Bounded: it is cleared
/// when it grows past a few thousand entries, which only happens with a
/// stream of unique labels (tick labels while panning), all cheap to remeasure.
#[derive(Default)]
pub struct TextCache {
    map: HashMap<(FontRole, u32, String), f32>,
}

const TEXT_CACHE_LIMIT: usize = 8192;

impl TextCache {
    pub fn width(
        &mut self,
        measure: &mut dyn TextMeasure,
        text: &str,
        font: FontRole,
        size: f32,
    ) -> f32 {
        let key = (font, size.to_bits(), text.to_owned());
        if let Some(w) = self.map.get(&key) {
            return *w;
        }
        if self.map.len() >= TEXT_CACHE_LIMIT {
            self.map.clear();
        }
        let w = measure.text_width(text, font, size);
        self.map.insert(key, w);
        w
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Fixed-advance measurement for tests and headless use: every character is
/// `0.6 × size` wide, which is close to a monospace face.
#[derive(Clone, Copy, Debug, Default)]
pub struct MonoMeasure;

impl TextMeasure for MonoMeasure {
    fn text_width(&mut self, text: &str, _font: FontRole, size: f32) -> f32 {
        text.chars().count() as f32 * size * 0.6
    }
}
