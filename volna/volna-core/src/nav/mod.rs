//! Navigation shared by every timed panel: the two link flags, the local
//! viewport and cursor a panel keeps while unlinked, and the commands that
//! move the time axis. A panel embeds a [`NavState`] and reads the shared
//! document values through it while linked.

pub mod tween;
pub use tween::{Lerp, Tween};

use web_time::Instant;

use crate::document::Document;
use crate::wave::viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Link {
    pub viewport: bool,
    pub cursor: bool,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            viewport: true,
            cursor: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkDim {
    Viewport,
    Cursor,
}

/// Which values a panel follows and what it keeps while it does not.
#[derive(Clone)]
pub struct NavState {
    pub link: Link,
    pub local_viewport: Tween<Viewport>,
    pub local_cursor: Option<u64>,
}

impl Default for NavState {
    fn default() -> Self {
        Self::new()
    }
}

impl NavState {
    pub fn new() -> Self {
        Self {
            link: Link::default(),
            local_viewport: Tween::new(Viewport::fit((0, 1000))),
            local_cursor: None,
        }
    }

    /// Copy the persistent navigation without the running animation.
    pub fn clone_view(&self) -> Self {
        Self {
            link: self.link,
            local_viewport: Tween::new(self.local_viewport.value),
            local_cursor: self.local_cursor,
        }
    }

    /// Forget the local cursor and stop any local animation; called when the
    /// document's session changes.
    pub fn reset(&mut self, limits: Option<(u64, u64)>) {
        self.local_cursor = None;
        let viewport = limits.map_or(self.local_viewport.value, Viewport::fit);
        self.local_viewport.set(viewport);
    }

    pub fn viewport(&self, doc: &Document) -> Viewport {
        self.viewport_state(doc).value
    }

    pub fn viewport_state<'a>(&'a self, doc: &'a Document) -> &'a Tween<Viewport> {
        if self.link.viewport {
            &doc.shared.viewport
        } else {
            &self.local_viewport
        }
    }

    pub fn viewport_state_mut<'a>(&'a mut self, doc: &'a mut Document) -> &'a mut Tween<Viewport> {
        if self.link.viewport {
            &mut doc.shared.viewport
        } else {
            &mut self.local_viewport
        }
    }

    pub fn cursor(&self, doc: &Document) -> Option<u64> {
        if self.link.cursor {
            doc.shared.cursor
        } else {
            self.local_cursor
        }
    }

    /// Returns whether the effective cursor changed.
    pub fn set_cursor(&mut self, doc: &mut Document, cursor: Option<u64>) -> bool {
        let current = if self.link.cursor {
            &mut doc.shared.cursor
        } else {
            &mut self.local_cursor
        };
        let changed = *current != cursor;
        *current = cursor;
        changed
    }

    /// Unlink snapshots the displayed shared value; relink adopts the shared
    /// value, discarding the local one. Either edge drops a local animation.
    pub fn toggle_link(&mut self, doc: &Document, dim: LinkDim) {
        match dim {
            LinkDim::Viewport => {
                self.local_viewport.set(doc.shared.viewport.value);
                self.link.viewport = !self.link.viewport;
            }
            LinkDim::Cursor => {
                self.local_cursor = doc.shared.cursor;
                self.link.cursor = !self.link.cursor;
            }
        }
    }

    /// The local animation only; the shared one is ticked by the document owner.
    pub fn tick(&mut self, now: Instant) -> bool {
        !self.link.viewport && self.local_viewport.tick(now)
    }

    pub fn is_animating(&self) -> bool {
        !self.link.viewport && self.local_viewport.is_animating()
    }

    // -- time-axis commands ------------------------------------------------------

    /// Animate the effective viewport to `target`, clamped to the trace.
    pub fn animate_to(&mut self, doc: &mut Document, mut target: Viewport, now: Instant) {
        target.clamp(doc.limits());
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc).animate_to(target, now, mode);
    }

    /// Jump the effective viewport to `target`, clamped to the trace.
    pub fn jump_to(&mut self, doc: &mut Document, mut target: Viewport) {
        target.clamp(doc.limits());
        self.viewport_state_mut(doc).set(target);
    }

    /// Immediate zoom (wheel, pinch) about a pixel position in a time area
    /// `width_px` wide.
    pub fn zoom_at(&mut self, doc: &mut Document, x_px: f64, width_px: f64, factor: f64) {
        let mut target = self.viewport(doc);
        target.zoom_about(x_px, width_px, factor, doc.limits());
        self.viewport_state_mut(doc).set(target);
    }

    /// Animated zoom about a pixel position, applied to the animation target
    /// so repeated events accumulate.
    pub fn zoom_target_at(
        &mut self,
        doc: &mut Document,
        x_px: f64,
        width_px: f64,
        factor: f64,
        now: Instant,
    ) {
        let mut target = self.viewport_state(doc).target();
        target.zoom_about(x_px, width_px, factor, doc.limits());
        self.animate_to(doc, target, now);
    }

    /// Keyboard zoom: about the cursor when it is visible, else the centre.
    pub fn zoom_center(&mut self, doc: &mut Document, width_px: f64, factor: f64, now: Instant) {
        let target = self.viewport_state(doc).target();
        let anchor_x = match self.cursor(doc) {
            Some(c) => {
                let x = target.x_of(c as f64, width_px);
                if (0.0..=width_px).contains(&x) {
                    x
                } else {
                    width_px / 2.0
                }
            }
            None => width_px / 2.0,
        };
        self.zoom_target_at(doc, anchor_x, width_px, factor, now);
    }

    pub fn zoom_fit(&mut self, doc: &mut Document, now: Instant) {
        self.animate_to(doc, Viewport::fit(doc.limits()), now);
    }

    pub fn zoom_to_cursor(&mut self, doc: &mut Document, now: Instant) {
        let Some(cursor) = self.cursor(doc) else {
            return;
        };
        let half_width = self.viewport_state(doc).target().width() / 4.0;
        let target = Viewport {
            start: cursor as f64 - half_width,
            end: cursor as f64 + half_width,
        };
        self.animate_to(doc, target, now);
    }

    pub fn go_to_start(&mut self, doc: &mut Document, now: Instant) {
        let mut target = self.viewport_state(doc).target();
        target.go_to_start(doc.limits());
        self.animate_to(doc, target, now);
    }

    pub fn go_to_end(&mut self, doc: &mut Document, now: Instant) {
        let mut target = self.viewport_state(doc).target();
        target.go_to_end(doc.limits());
        self.animate_to(doc, target, now);
    }

    pub fn go_to_cursor(&mut self, doc: &mut Document, now: Instant) {
        let Some(c) = self.cursor(doc) else { return };
        let mut target = self.viewport_state(doc).target();
        target.center_on(c as f64, doc.limits());
        self.animate_to(doc, target, now);
    }

    /// Animated pan by a fraction of the window width.
    pub fn pan_fraction(&mut self, doc: &mut Document, frac: f64, now: Instant) {
        let mut target = self.viewport_state(doc).target();
        let w = target.width();
        target.start += w * frac;
        target.end += w * frac;
        self.animate_to(doc, target, now);
    }

    /// Immediate pan by pixels (drag, trackpad scroll).
    pub fn pan_px(&mut self, doc: &mut Document, dx_px: f64, width_px: f64) {
        let mut target = self.viewport(doc);
        target.pan_px(dx_px, width_px, doc.limits());
        self.viewport_state_mut(doc).set(target);
    }

    /// Scroll the window so the cursor is visible, if it is not.
    pub fn reveal_cursor(&mut self, doc: &mut Document, now: Instant) {
        let Some(c) = self.cursor(doc) else { return };
        let c = c as f64;
        let mut target = self.viewport(doc);
        if c < target.start || c > target.end {
            target.center_on(c, doc.limits());
            self.animate_to(doc, target, now);
        }
    }
}
