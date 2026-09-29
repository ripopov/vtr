//! Navigation shared by every timed panel: the two link flags, the local
//! viewport and cursor a panel keeps while unlinked, and the commands that
//! move the time axis. A panel embeds a [`NavState`] and reads the shared
//! document values through it while linked.

pub mod tween;
pub use tween::{Lerp, Tween};

use web_time::Instant;

use crate::clock::ClockKey;
use crate::document::Document;
use crate::trace::{Rescale, Retime};
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
    /// Clock rulers, snapping clock and cycle origin. The rulers and the
    /// origin are journaled cockpit state: they change only through the
    /// methods below, which keep their value before the first change.
    clocks: crate::clock::ClockView,
    clocks_before: crate::history::Before<ClockChoice>,
    /// Where the last jump started, or where the last return left: the one
    /// place [`NavState::back`] swaps with the present.
    back: Option<Spot>,
}

/// A place on the time axis to come back to: the effective cursor and where
/// the effective viewport is headed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub cursor: Option<u64>,
    pub viewport: Viewport,
}

/// [`NavState::reveal`] treats this fraction of the viewport at each edge as
/// off screen, so a revealed time never lands flush against an edge.
const REVEAL_EDGE: f64 = 0.05;

/// A timed panel's ruler rows and cycle origin, as the undo journal swaps them.
pub(crate) type ClockChoice = (Option<Vec<ClockKey>>, Option<u64>);

/// Every time the panel holds: its own view and cursor, the cycle origin,
/// and the place a jump returns to.
impl Retime for NavState {
    fn retime(&mut self, by: Rescale) {
        self.local_viewport.retime(by);
        self.local_cursor.retime(by);
        self.clocks.origin.retime(by);
        if let Some(back) = &mut self.back {
            back.cursor.retime(by);
            back.viewport.retime(by);
        }
    }
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
            clocks: crate::clock::ClockView::default(),
            clocks_before: Default::default(),
            back: None,
        }
    }

    /// Copy the persistent navigation without the running animation.
    pub fn clone_view(&self) -> Self {
        Self {
            link: self.link,
            local_viewport: Tween::new(self.local_viewport.value),
            local_cursor: self.local_cursor,
            clocks: self.clocks.clone(),
            clocks_before: Default::default(),
            back: None,
        }
    }

    /// Forget the local cursor and stop any local animation; called when the
    /// document's session changes.
    pub fn reset(&mut self, limits: Option<(u64, u64)>) {
        self.local_cursor = None;
        self.back = None;
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

    /// The whole trace with a `margin_px` margin in a `width_px` wide area.
    pub fn zoom_fit(&mut self, doc: &mut Document, width_px: f64, margin_px: f64, now: Instant) {
        let target = Viewport::fit_px(doc.limits(), width_px, margin_px);
        self.animate_to(doc, target, now);
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

    // -- clocks ------------------------------------------------------------------

    /// The timeline of the clock clicks snap to and cycle steps follow.
    pub fn selected_clock(
        &self,
        doc: &Document,
    ) -> Option<std::sync::Arc<crate::clock::ClockTimeline>> {
        self.clocks.selected(&doc.clocks)?.timeline().cloned()
    }

    /// Step the cursor to the next (or previous) rising edge of the selected
    /// clock, from the cursor or the window's edge. Returns whether it moved.
    pub fn step_cycle(&mut self, doc: &mut Document, forward: bool, now: Instant) -> bool {
        let Some(timeline) = self.selected_clock(doc) else {
            return false;
        };
        let vp = self.viewport(doc);
        let to = match (self.cursor(doc), forward) {
            (Some(c), true) => timeline.next_edge(c),
            (Some(c), false) => timeline.prev_edge(c),
            (None, true) => timeline.next_edge(vp.start.max(0.0) as u64),
            (None, false) => timeline.prev_edge(vp.end.max(0.0) as u64 + 1),
        };
        let Some(to) = to else { return false };
        self.set_cursor(doc, Some(to));
        self.reveal_cursor(doc, now);
        true
    }

    /// Put the cursor on the edge of displayed cycle `shown` of the selected
    /// clock and centre it.
    pub fn go_to_cycle(
        &mut self,
        doc: &mut Document,
        shown: i64,
        now: Instant,
    ) -> Result<(), String> {
        let clock = self
            .clocks
            .selected(&doc.clocks)
            .ok_or("no clock to count cycles in")?;
        let name = clock.name.clone();
        let timeline = clock
            .timeline()
            .ok_or("the clock is still loading")?
            .clone();
        let time = self
            .clocks
            .absolute_cycle(&timeline, shown)
            .and_then(|c| timeline.edge(c))
            .ok_or_else(|| format!("{name} has no cycle {shown}"))?;
        self.set_cursor(doc, Some(time));
        self.go_to_cursor(doc, now);
        Ok(())
    }

    /// Number cycles from the cursor's cycle, or back from the first edge
    /// when the origin is already there.
    pub fn toggle_cycle_origin(&mut self, doc: &Document) {
        let cursor = self.cursor(doc);
        self.note_clocks();
        self.clocks.origin = if self.clocks.origin == cursor {
            None
        } else {
            cursor
        };
    }

    /// Select the clock clicks snap to and cycle steps follow (navigation).
    pub fn select_clock(&mut self, key: ClockKey) {
        self.clocks.selected = Some(key);
    }

    /// The panel's clock rulers, snapping clock and cycle origin.
    pub fn clocks(&self) -> &crate::clock::ClockView {
        &self.clocks
    }

    /// Show a ruler row (see [`crate::clock::ClockView::show_ruler`]).
    pub fn show_ruler(&mut self, clocks: &crate::clock::Clocks, key: &ClockKey) {
        self.note_clocks();
        self.clocks.show_ruler(clocks, key);
    }

    /// Hide a ruler row if it is shown.
    pub fn hide_ruler(&mut self, clocks: &crate::clock::Clocks, key: &ClockKey) {
        self.note_clocks();
        self.clocks.hide_ruler(clocks, key);
    }

    /// Show or hide a ruler row.
    pub fn toggle_ruler(&mut self, clocks: &crate::clock::Clocks, key: &ClockKey) {
        self.note_clocks();
        self.clocks.toggle_ruler(clocks, key);
    }

    /// Hide a closed trace's rulers as an edit, and stop snapping to its
    /// clock.
    pub(crate) fn forget_trace(&mut self, trace: crate::trace::TraceId) {
        let mut clocks = self.clocks.clone();
        if clocks.forget_trace(trace) {
            self.note_clocks();
        }
        self.clocks = clocks;
    }

    fn note_clocks(&mut self) {
        let clocks = &self.clocks;
        self.clocks_before
            .note(|| (clocks.rulers.clone(), clocks.origin));
    }

    /// Install clock choices that are not an edit (a restored workspace).
    pub(crate) fn restore_clocks(&mut self, view: crate::clock::ClockView) {
        self.clocks = view;
        self.clocks_before.take();
    }

    /// Install rulers and origin while undoing or redoing; returns the
    /// replaced ones.
    pub(crate) fn swap_clocks(&mut self, (rulers, origin): ClockChoice) -> ClockChoice {
        debug_assert!(!self.clocks_before.is_held(), "edits were not collected");
        let old = (
            std::mem::replace(&mut self.clocks.rulers, rulers),
            std::mem::replace(&mut self.clocks.origin, origin),
        );
        // A hidden ruler cannot stay the selected clock.
        if let Some(selected) = &self.clocks.selected
            && self
                .clocks
                .rulers
                .as_ref()
                .is_some_and(|r| !r.contains(selected))
        {
            self.clocks.selected = None;
        }
        old
    }

    /// Hand the rulers and origin before the edits since the last call to
    /// the undo journal, unless they cancelled out.
    pub(crate) fn take_edits(
        &mut self,
        panel: crate::panels::PanelId,
        clocks: &crate::clock::Clocks,
        history: &mut crate::history::History,
    ) {
        let Some((rulers, origin)) = self.clocks_before.take() else {
            return;
        };
        if (&rulers, &origin) == (&self.clocks.rulers, &self.clocks.origin) {
            return;
        }
        let label = clocks_label(clocks, &rulers, origin, &self.clocks);
        history.record(
            crate::history::Edit::Prop {
                panel,
                prop: crate::history::Prop::Clocks { rulers, origin },
            },
            Some(label),
        );
    }

    /// The effective cursor and the viewport's destination.
    pub fn spot(&self, doc: &Document) -> Spot {
        Spot {
            cursor: self.cursor(doc),
            viewport: self.viewport_state(doc).target(),
        }
    }

    /// The place [`NavState::back`] would return to.
    pub fn back_spot(&self) -> Option<Spot> {
        self.back
    }

    /// Move the cursor to `t` as a jump: the place before it becomes the one
    /// [`NavState::back`] returns to, and the view pans only as far as
    /// [`NavState::reveal`] needs. Returns whether the cursor moved; a jump
    /// to where the cursor already is keeps the earlier return place.
    pub fn jump_cursor(&mut self, doc: &mut Document, t: u64, now: Instant) -> bool {
        if self.cursor(doc) == Some(t) {
            self.reveal(doc, t, now);
            return false;
        }
        self.back = Some(self.spot(doc));
        self.set_cursor(doc, Some(t));
        self.reveal(doc, t, now);
        true
    }

    /// Return to the cursor and view from before the last jump; the place
    /// left becomes the return place, so doing it twice goes forward again.
    /// Returns false when no jump happened yet.
    pub fn back(&mut self, doc: &mut Document, now: Instant) -> bool {
        let Some(spot) = self.back.take() else {
            return false;
        };
        self.back = Some(self.spot(doc));
        self.set_cursor(doc, spot.cursor);
        self.animate_to(doc, spot.viewport, now);
        true
    }

    /// Centre the view on `t`, keeping its width, when `t` lies outside the
    /// middle 90% of where the view is headed. Returns whether it panned.
    pub fn reveal(&mut self, doc: &mut Document, t: u64, now: Instant) -> bool {
        let mut target = self.viewport_state(doc).target();
        let edge = target.width() * REVEAL_EDGE;
        let t = t as f64;
        if (target.start + edge..=target.end - edge).contains(&t) {
            return false;
        }
        target.center_on(t, doc.limits());
        self.animate_to(doc, target, now);
        true
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

/// "Show ruler core_clk", "Hide ruler bus_clk", "Set cycle origin".
fn clocks_label(
    clocks: &crate::clock::Clocks,
    rulers: &Option<Vec<ClockKey>>,
    origin: Option<u64>,
    now: &crate::clock::ClockView,
) -> String {
    let before = rulers.as_deref().unwrap_or(&clocks.defaults);
    let after = now.ruler_keys(clocks);
    let name = |key: &ClockKey| key.item.rsplit('.').next().unwrap_or(&key.item).to_owned();
    if let Some(shown) = after.iter().find(|p| !before.contains(p)) {
        return format!("Show ruler {}", name(shown));
    }
    if let Some(hidden) = before.iter().find(|p| !after.contains(p)) {
        return format!("Hide ruler {}", name(hidden));
    }
    match (origin, now.origin) {
        (_, Some(_)) if origin != now.origin => "Set cycle origin".into(),
        (Some(_), None) => "Clear cycle origin".into(),
        _ => "Clock rulers".into(),
    }
}
