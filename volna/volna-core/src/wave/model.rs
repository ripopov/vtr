//! `WaveModel`: the state of the waveform panel (displayed signals, selection,
//! viewport, scroll, hover, drags, the format menu) and every input rule that
//! mutates it. Navigation reads through the document while linked; markers
//! always belong to the document.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use web_time::Instant;

use super::layout::{LayoutInput, MIN_COLUMN, WaveLayout};

/// A ⌘-drag narrower than this (at zoom 1.0) is a click, not a zoom range.
pub const ZOOM_RANGE_MIN_PX: f32 = 4.0;
use super::viewport::{Viewport, ViewportState};
use crate::data::{SignalHistory, SignalRef, SignalShape, Translator, VarId};
use crate::document::Document;
use crate::geometry::{Modifiers, MouseButton, Point, point};
use crate::selection;
use crate::theme::Theme;

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

/// A row is either bound to this session or retains an unresolved durable locator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowSource {
    Resolved {
        var: VarId,
        signal: SignalRef,
    },
    Unresolved {
        path: Vec<String>,
        nth: Option<usize>,
        ambiguous: bool,
    },
}

impl RowSource {
    pub fn signal(&self) -> Option<SignalRef> {
        match self {
            Self::Resolved { signal, .. } => Some(*signal),
            _ => None,
        }
    }

    pub fn locator(&self, hierarchy: &crate::data::Hierarchy) -> (Vec<String>, Option<usize>) {
        match self {
            Self::Resolved { var, .. } => hierarchy.var_path(*var),
            Self::Unresolved { path, nth, .. } => (path.clone(), *nth),
        }
    }
}

#[derive(Clone)]
pub struct DisplayedSignal {
    pub source: RowSource,
    /// Unavailable translator requested by a workspace; cleared by an explicit format change.
    pub requested_format: Option<String>,
    pub name: String,
    pub scope: String,
    pub shape: SignalShape,
    pub translator: Arc<dyn Translator>,
    pub history: Option<Arc<dyn SignalHistory>>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Cursor,
    ZoomRange { start: Point, current: Point },
    Pan { last_x: f32 },
    NamesSplit,
    ValuesSplit,
    Scroll { grab: f32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Format(String),
    RetryLoad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub action: MenuAction,
    pub label: String,
    pub badge: Option<String>,
    pub checked: bool,
}

/// The row menu for value formats and failed-load retry. The frontend shows
/// its own popup widget at the panel position and reports the typed choice.
#[derive(Clone, Debug)]
pub struct FormatMenu {
    pub row: usize,
    pub position: Point,
    pub items: Vec<MenuItem>,
}

/// Pointer input over the wave panel, in the same coordinate space as the
/// bounds passed to [`WaveModel::layout`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Down {
        position: Point,
        button: MouseButton,
        modifiers: Modifiers,
    },
    Move {
        position: Point,
    },
    Up,
    /// The pointer left the panel (or the window).
    Leave,
    /// Wheel or trackpad scroll in logical pixels.
    Wheel {
        position: Point,
        dx: f32,
        dy: f32,
        modifiers: Modifiers,
    },
    /// Trackpad pinch; `delta` is the scale change of this event.
    Pinch {
        position: Point,
        delta: f32,
    },
}

pub struct WaveModel {
    pub items: Vec<DisplayedSignal>,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub link: Link,
    pub local_viewport: ViewportState,
    pub local_cursor: Option<u64>,
    pub scroll_y: f32,
    pub names_width: f32,
    pub values_width: f32,
    pub hover_row: Option<usize>,
    pub badge_hover: Option<usize>,
    /// Pointer is over a column divider / a marker chip (drives repaints).
    pub split_hover: bool,
    pub chip_hover: bool,
    pub drag: Option<Drag>,
    /// Width of the waves column at the last layout, for keyboard zoom.
    pub wave_width: f32,
    /// Duration of the last panel paint, and a smoothed average.
    pub frames_painted: u64,
    pub frame_ms: f32,
    pub frame_ms_avg: f32,
    pub menu: Option<FormatMenu>,
    /// Last known pointer position over the panel.
    pub pointer: Option<Point>,
    layout: WaveLayout,
}

impl Default for WaveModel {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveModel {
    pub fn viewport(&self, doc: &Document) -> Viewport {
        self.viewport_state(doc).viewport
    }

    pub fn viewport_state<'a>(&'a self, doc: &'a Document) -> &'a ViewportState {
        if self.link.viewport {
            &doc.shared.viewport
        } else {
            &self.local_viewport
        }
    }

    pub fn viewport_state_mut<'a>(&'a mut self, doc: &'a mut Document) -> &'a mut ViewportState {
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

    pub fn toggle_link(&mut self, doc: &Document, dim: LinkDim) {
        match dim {
            LinkDim::Viewport => {
                // Discard local animation on either edge. Unlink starts at
                // the exact shared frame, without inheriting an animation.
                self.local_viewport.set(doc.shared.viewport.viewport);
                self.link.viewport = !self.link.viewport;
            }
            LinkDim::Cursor => {
                self.local_cursor = doc.shared.cursor;
                self.link.cursor = !self.link.cursor;
            }
        }
    }

    /// Copy persistent view content, sharing immutable histories. Pixel
    /// layout, pointer capture, menus, and animation belong to the old view.
    pub fn clone_view(&self, with_rows: bool) -> Self {
        Self {
            items: if with_rows {
                self.items.clone()
            } else {
                Vec::new()
            },
            selected: if with_rows {
                self.selected.clone()
            } else {
                BTreeSet::new()
            },
            anchor: if with_rows { self.anchor } else { None },
            link: self.link,
            local_viewport: ViewportState::new(self.local_viewport.viewport),
            local_cursor: self.local_cursor,
            scroll_y: if with_rows { self.scroll_y } else { 0.0 },
            names_width: self.names_width,
            values_width: self.values_width,
            ..Self::new()
        }
    }

    pub fn new() -> Self {
        WaveModel {
            items: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            link: Link::default(),
            local_viewport: ViewportState::new(Viewport::fit((0, 1000))),
            local_cursor: None,
            scroll_y: 0.0,
            names_width: 220.0,
            values_width: 120.0,
            hover_row: None,
            badge_hover: None,
            split_hover: false,
            chip_hover: false,
            drag: None,
            wave_width: 800.0,
            frames_painted: 0,
            frame_ms: 0.0,
            frame_ms_avg: 0.0,
            menu: None,
            pointer: None,
            layout: WaveLayout::default(),
        }
    }

    /// Forget every row and interaction; called when the document's session changes.
    pub fn reset(&mut self, limits: Option<(u64, u64)>) {
        self.local_cursor = None;
        self.items.clear();
        self.selected.clear();
        self.anchor = None;
        self.scroll_y = 0.0;
        self.local_viewport.set(self.local_viewport.viewport);
        self.menu = None;
        self.drag = None;
        self.hover_row = None;
        self.badge_hover = None;
        if let Some(limits) = limits {
            self.local_viewport.set(Viewport::fit(limits));
        }
    }

    /// The layout of the last frame (see [`WaveModel::layout`]).
    pub fn last_layout(&self) -> &WaveLayout {
        &self.layout
    }

    pub fn is_animating(&self) -> bool {
        !self.link.viewport && self.local_viewport.is_animating()
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        !self.link.viewport && self.local_viewport.tick(now)
    }

    pub fn loaded_count(&self) -> usize {
        self.items.iter().filter(|i| i.history.is_some()).count()
    }

    // -- items ---------------------------------------------------------------

    /// Append rows for `vars`, sharing histories already loaded for the same
    /// signal and queuing a load for the rest. Returns the histories that must
    /// be loaded (deduplicated).
    pub fn add_vars(&mut self, doc: &mut Document, vars: &[VarId]) {
        let loaded = self
            .items
            .iter()
            .filter_map(|i| Some((i.source.signal()?, i.history.clone()?)))
            .collect();
        self.add_vars_with_histories(doc, vars, loaded);
    }

    pub(crate) fn add_vars_with_histories(
        &mut self,
        doc: &mut Document,
        vars: &[VarId],
        loaded: HashMap<SignalRef, Arc<dyn SignalHistory>>,
    ) {
        let Some(session) = doc.session().cloned() else {
            return;
        };
        let h = session.hierarchy();
        let first_new = self.items.len();
        for &var in vars {
            let Some(v) = h.vars.get(var) else { continue };
            let translator = doc.translators.default_for(v.shape);
            // Variable identity/format stay per row; aliases share immutable data.
            let history = loaded.get(&v.signal).cloned();
            let needs_load = history.is_none();
            self.items.push(DisplayedSignal {
                source: RowSource::Resolved {
                    var,
                    signal: v.signal,
                },
                requested_format: None,
                name: v.name.clone(),
                scope: h.scope_path(v.scope).join("."),
                shape: v.shape,
                translator,
                history,
                error: None,
            });
            if needs_load {
                doc.request_signal(v.signal);
            }
        }
        if self.items.len() > first_new {
            self.selected.clear();
            self.selected.extend(first_new..self.items.len());
            self.anchor = Some(first_new);
        }
    }

    /// A history load finished (the document already checked its generation).
    pub fn finish_signal(
        &mut self,
        signal: SignalRef,
        result: anyhow::Result<Arc<dyn SignalHistory>>,
    ) {
        let result = result.map_err(|e| e.to_string());
        for item in self
            .items
            .iter_mut()
            .filter(|i| i.source.signal() == Some(signal))
        {
            item.history = result.as_ref().ok().cloned();
            item.error = result.as_ref().err().cloned();
        }
    }

    pub fn remove_selected(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        let mut ix = 0;
        let selected = std::mem::take(&mut self.selected);
        self.items.retain(|_| {
            let keep = !selected.contains(&ix);
            ix += 1;
            keep
        });
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.items.len()).collect();
    }

    /// Escape: cancel a drag, close the menu, clear selection, then clear cursor.
    pub fn clear_selection(&mut self, doc: &mut Document) {
        if self.drag.is_some() {
            self.drag = None;
        } else if self.menu.is_some() {
            self.menu = None;
        } else if !self.selected.is_empty() {
            self.selected.clear();
        } else {
            self.set_cursor(doc, None);
        }
    }

    /// Click selection with platform conventions: plain = single, cmd/ctrl =
    /// toggle, shift = range from the anchor.
    pub fn select_row(&mut self, row: usize, modifiers: Modifiers) {
        if row >= self.items.len() {
            return;
        }
        selection::select(&mut self.selected, &mut self.anchor, row, modifiers);
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let current = self
            .anchor
            .or_else(|| self.selected.iter().next().copied())
            .unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, self.items.len() as isize - 1) as usize;
        self.selected.clear();
        self.selected.insert(next);
        self.anchor = Some(next);
    }

    // -- translators -----------------------------------------------------------

    pub fn set_translator(&mut self, doc: &Document, rows: &[usize], id: &str) {
        let Some(t) = doc.translators.get(id) else {
            return;
        };
        for &row in rows {
            if let Some(item) = self.items.get_mut(row)
                && t.applies(item.shape)
            {
                item.translator = t.clone();
                item.requested_format = None;
            }
        }
    }

    pub fn cycle_format(&mut self, doc: &Document) {
        let rows: Vec<usize> = self.selected.iter().copied().collect();
        for row in rows {
            let Some(item) = self.items.get(row) else {
                continue;
            };
            let options = doc.translators.applicable(item.shape);
            if options.is_empty() {
                continue;
            }
            let pos = options
                .iter()
                .position(|t| t.id() == item.translator.id())
                .unwrap_or(0);
            let next = options[(pos + 1) % options.len()].clone();
            self.items[row].translator = next;
            self.items[row].requested_format = None;
        }
    }

    /// Open the format menu for `row` at a panel position.
    pub fn open_format_menu(&mut self, doc: &Document, row: usize, position: Point) {
        let Some(item) = self.items.get(row) else {
            return;
        };
        let current = item.translator.id();
        let mut items: Vec<_> = doc
            .translators
            .applicable(item.shape)
            .into_iter()
            .map(|t| MenuItem {
                action: MenuAction::Format(t.id().into()),
                label: t.name().into(),
                badge: Some(t.badge().into()),
                checked: t.id() == current,
            })
            .collect();
        if item.error.is_some() && item.source.signal().is_some() {
            items.push(MenuItem {
                action: MenuAction::RetryLoad,
                label: "Retry loading".into(),
                badge: None,
                checked: false,
            });
        }
        self.menu = Some(FormatMenu {
            row,
            position,
            items,
        });
    }

    /// The frontend's popup reported a choice.
    /// Return a failed canonical signal to retry through the document owner.
    pub fn menu_select(&mut self, doc: &Document, action: &MenuAction) -> Option<SignalRef> {
        let menu = self.menu.take()?;
        let MenuAction::Format(id) = action else {
            let row = self.items.get(menu.row)?;
            return row.error.as_ref().and_then(|_| row.source.signal());
        };
        let rows: Vec<usize> = if self.selected.contains(&menu.row) {
            self.selected.iter().copied().collect()
        } else {
            vec![menu.row]
        };
        self.set_translator(doc, &rows, id);
        None
    }

    pub fn menu_dismiss(&mut self) {
        self.menu = None;
    }

    // -- navigation ------------------------------------------------------------

    fn wave_w(&self) -> f64 {
        f64::from(self.wave_width).max(1.0)
    }

    /// Immediate zoom (mouse wheel) around a pixel position in the waves column.
    pub fn zoom_at(&mut self, doc: &mut Document, x_px: f32, factor: f64) {
        let w = self.wave_w();
        let mut target = self.viewport(doc);
        target.zoom_about(f64::from(x_px), w, factor, doc.limits());
        self.viewport_state_mut(doc).set(target);
    }

    fn zoom_center(&mut self, doc: &mut Document, factor: f64, now: Instant) {
        let w = self.wave_w();
        let mut target = self.viewport_state(doc).target();
        let anchor_x = match self.cursor(doc) {
            Some(c) => {
                let x = target.x_of(c as f64, w);
                if (0.0..=w).contains(&x) { x } else { w / 2.0 }
            }
            None => w / 2.0,
        };
        target.zoom_about(anchor_x, w, factor, doc.limits());
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc)
            .animate_to(target, limits, now, mode);
    }

    pub fn zoom_in(&mut self, doc: &mut Document, now: Instant) {
        self.zoom_center(doc, 2.0, now);
    }

    pub fn zoom_out(&mut self, doc: &mut Document, now: Instant) {
        self.zoom_center(doc, 0.5, now);
    }

    pub fn zoom_fit(&mut self, doc: &mut Document, now: Instant) {
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc)
            .animate_to(Viewport::fit(limits), limits, now, mode);
    }

    pub fn zoom_to_cursor(&mut self, doc: &mut Document, now: Instant) {
        let Some(cursor) = self.cursor(doc) else {
            return;
        };
        let half_width = self.viewport_state(doc).target().width() / 4.0;
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc).animate_to(
            Viewport {
                start: cursor as f64 - half_width,
                end: cursor as f64 + half_width,
            },
            limits,
            now,
            mode,
        );
    }

    pub fn go_to_start(&mut self, doc: &mut Document, now: Instant) {
        let mut target = self.viewport_state(doc).target();
        target.go_to_start(doc.limits());
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc)
            .animate_to(target, limits, now, mode);
    }

    pub fn go_to_end(&mut self, doc: &mut Document, now: Instant) {
        let mut target = self.viewport_state(doc).target();
        target.go_to_end(doc.limits());
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc)
            .animate_to(target, limits, now, mode);
    }

    pub fn go_to_cursor(&mut self, doc: &mut Document, now: Instant) {
        let Some(c) = self.cursor(doc) else { return };
        let mut target = self.viewport_state(doc).target();
        target.center_on(c as f64, doc.limits());
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc)
            .animate_to(target, limits, now, mode);
    }

    pub fn pan_fraction(&mut self, doc: &mut Document, frac: f64, now: Instant) {
        let mut target = self.viewport_state(doc).target();
        let w = target.width();
        target.start += w * frac;
        target.end += w * frac;
        let limits = doc.limits();
        let mode = doc.navigation.animation;
        self.viewport_state_mut(doc)
            .animate_to(target, limits, now, mode);
    }

    /// Immediate pan by pixels (mouse drag / wheel).
    pub fn pan_px(&mut self, doc: &mut Document, dx: f32) {
        let w = self.wave_w();
        let mut target = self.viewport(doc);
        target.pan_px(f64::from(dx), w, doc.limits());
        self.viewport_state_mut(doc).set(target);
    }

    fn edge_history(&self) -> Option<Arc<dyn SignalHistory>> {
        let row = self
            .anchor
            .filter(|r| self.selected.contains(r))
            .or_else(|| self.selected.iter().next().copied())?;
        self.items.get(row)?.history.clone()
    }

    fn reveal_cursor(&mut self, doc: &mut Document, now: Instant) {
        let Some(c) = self.cursor(doc) else { return };
        let c = c as f64;
        if c < self.viewport(doc).start || c > self.viewport(doc).end {
            let mut target = self.viewport(doc);
            target.center_on(c, doc.limits());
            let limits = doc.limits();
            let mode = doc.navigation.animation;
            self.viewport_state_mut(doc)
                .animate_to(target, limits, now, mode);
        }
    }

    pub fn next_edge(&mut self, doc: &mut Document, now: Instant) {
        let Some(h) = self.edge_history() else { return };
        let from = self
            .cursor(doc)
            .unwrap_or(self.viewport(doc).start.max(0.0) as u64);
        if let Some(t) = h.next_change_after(from) {
            self.set_cursor(doc, Some(t));
            self.reveal_cursor(doc, now);
        }
    }

    pub fn prev_edge(&mut self, doc: &mut Document, now: Instant) {
        let Some(h) = self.edge_history() else { return };
        let from = self
            .cursor(doc)
            .unwrap_or(self.viewport(doc).end.max(0.0) as u64);
        if let Some(t) = h.prev_change_before(from) {
            self.set_cursor(doc, Some(t));
            self.reveal_cursor(doc, now);
        }
    }

    /// Record the paint time of the last frame.
    pub fn record_frame(&mut self, ms: f32) {
        self.frames_painted += 1;
        self.frame_ms = ms;
        self.frame_ms_avg = if self.frame_ms_avg == 0.0 {
            ms
        } else {
            self.frame_ms_avg * 0.9 + ms * 0.1
        };
    }

    // -- layout ------------------------------------------------------------------

    /// Lay the panel out in `bounds` for this frame. Clamps the scroll, records
    /// the waves width used by keyboard zoom, and derives hover state from the
    /// last pointer position. The result is kept for input handling.
    pub fn layout(
        &mut self,
        bounds: crate::geometry::Rect,
        doc: &Document,
        theme: &Theme,
    ) -> &WaveLayout {
        // Keep the same rows on screen when the interface zoom changes.
        if self.layout.row_h > 0.0 && self.layout.row_h != theme.row_height {
            self.scroll_y *= theme.row_height / self.layout.row_h;
        }
        let layout = WaveLayout::compute(LayoutInput {
            bounds,
            row_h: theme.row_height,
            header_h: theme.timeline_height,
            zoom: theme.zoom,
            names_width: self.names_width,
            values_width: self.values_width,
            item_count: self.items.len(),
            scroll_y: self.scroll_y,
            markers: &doc.markers,
            viewport: self.viewport(doc),
        });
        self.scroll_y = layout.scroll_y;
        self.wave_width = layout.waves.width();
        self.layout = layout;
        self.update_hover();
        &self.layout
    }

    fn update_hover(&mut self) {
        let (hover_row, badge_hover) = match self.pointer {
            Some(p) if self.layout.bounds.contains(p) && p.y >= self.layout.names.top() => {
                let row = self.layout.row_at(p.y).filter(|r| *r < self.items.len());
                let badge = self.layout.badge_at(p).map(|(ix, _)| ix);
                (row, badge)
            }
            _ => (None, None),
        };
        self.hover_row = hover_row;
        self.badge_hover = badge_hover;
    }

    // -- pointer input -------------------------------------------------------------

    /// Handle pointer input. Returns true when something visible changed.
    pub fn pointer(&mut self, doc: &mut Document, event: PointerEvent, now: Instant) -> bool {
        match event {
            PointerEvent::Down {
                position,
                button,
                modifiers,
            } => {
                self.pointer_down(doc, position, button, modifiers);
                true
            }
            PointerEvent::Move { position } => self.pointer_move(doc, position),
            PointerEvent::Up => {
                let had = self.drag.is_some();
                if let Some(Drag::ZoomRange { start, current }) = self.drag.take()
                    && (current.x - start.x).abs() >= ZOOM_RANGE_MIN_PX * self.layout.zoom
                {
                    let layout = &self.layout;
                    let viewport = self.viewport(doc);
                    let a =
                        viewport.time_at(f64::from(start.x - layout.waves.left()), self.wave_w());
                    let b =
                        viewport.time_at(f64::from(current.x - layout.waves.left()), self.wave_w());
                    let limits = doc.limits();
                    let mode = doc.navigation.animation;
                    self.viewport_state_mut(doc).animate_to(
                        Viewport {
                            start: a.min(b),
                            end: a.max(b),
                        },
                        limits,
                        now,
                        mode,
                    );
                }
                had
            }
            PointerEvent::Leave => {
                self.pointer = None;
                let had = self.hover_row.is_some()
                    || self.badge_hover.is_some()
                    || self.split_hover
                    || self.chip_hover;
                self.hover_row = None;
                self.badge_hover = None;
                self.split_hover = false;
                self.chip_hover = false;
                had
            }
            PointerEvent::Wheel {
                position,
                dx,
                dy,
                modifiers,
            } => {
                self.wheel(doc, position, dx, dy, modifiers, now);
                true
            }
            PointerEvent::Pinch { position, delta } => {
                let factor = f64::from(1.0 + delta).clamp(0.2, 5.0);
                let x = (position.x - self.layout.waves.left()).max(0.0);
                self.zoom_at(doc, x, factor);
                true
            }
        }
    }

    fn pointer_down(
        &mut self,
        doc: &mut Document,
        p: Point,
        button: MouseButton,
        modifiers: Modifiers,
    ) {
        let layout = self.layout.clone();
        self.pointer = Some(p);
        self.menu = None;
        if button == MouseButton::Left {
            if layout.names_split.contains(p) {
                self.drag = Some(Drag::NamesSplit);
                return;
            }
            if layout.values_split.contains(p) {
                self.drag = Some(Drag::ValuesSplit);
                return;
            }
            if let Some((track, thumb)) = layout.scrollbar {
                if thumb.contains(p) {
                    self.drag = Some(Drag::Scroll {
                        grab: p.y - thumb.top(),
                    });
                    return;
                }
                if track.contains(p) {
                    let travel = track.height() - thumb.height();
                    let frac =
                        ((p.y - track.top() - thumb.height() / 2.0) / travel).clamp(0.0, 1.0);
                    self.scroll_y = layout.max_scroll * frac;
                    self.drag = Some(Drag::Scroll {
                        grab: thumb.height() / 2.0,
                    });
                    return;
                }
            }
            if let Some(ix) = layout.chip_at(p) {
                if modifiers.shift {
                    doc.remove_marker(ix);
                } else {
                    let t = doc.markers[ix].time;
                    self.set_cursor(doc, Some(t));
                }
                return;
            }
        }
        let in_waves_x = p.x >= layout.waves.left() && p.x < layout.waves.right();
        let wave_wf = layout.wave_width_f64();
        let snap_px = doc.navigation.snap_px * f64::from(layout.zoom);
        if in_waves_x && button == MouseButton::Left && (modifiers.control || modifiers.platform) {
            let displayed = self.viewport(doc);
            self.viewport_state_mut(doc).set(displayed);
            self.drag = Some(Drag::ZoomRange {
                start: p,
                current: p,
            });
            return;
        }
        if layout.header.contains(p) {
            if in_waves_x && button == MouseButton::Left {
                let x = f64::from(p.x - layout.waves.left());
                let t = snapped_time(&self.viewport(doc), None, x, wave_wf, snap_px);
                self.set_cursor(doc, Some(t));
                self.drag = Some(Drag::Cursor);
            }
            return;
        }
        let row = layout.row_at(p.y).filter(|r| *r < self.items.len());
        if in_waves_x {
            match button {
                MouseButton::Left => {
                    let x = f64::from(p.x - layout.waves.left());
                    let hist = row.and_then(|r| self.items[r].history.clone());
                    let t = snapped_time(&self.viewport(doc), hist.as_deref(), x, wave_wf, snap_px);
                    self.set_cursor(doc, Some(t));
                    self.drag = Some(Drag::Cursor);
                    if let Some(r) = row
                        && (!self.selected.contains(&r) || modifiers.shift || modifiers.secondary())
                    {
                        self.select_row(r, modifiers);
                    }
                }
                MouseButton::Middle | MouseButton::Right => {
                    self.drag = Some(Drag::Pan { last_x: p.x });
                }
            }
            return;
        }
        // Names / values columns.
        if button != MouseButton::Left {
            return;
        }
        if let Some((ix, b)) = layout.badge_at(p) {
            let pos = point(b.left(), b.bottom() + 4.0 * layout.zoom);
            if !self.selected.contains(&ix) {
                self.select_row(ix, modifiers);
            }
            self.open_format_menu(doc, ix, pos);
            return;
        }
        match row {
            Some(r) => self.select_row(r, modifiers),
            None => self.selected.clear(),
        }
    }

    fn pointer_move(&mut self, doc: &mut Document, p: Point) -> bool {
        self.pointer = Some(p);
        let layout = &self.layout;
        let wave_wf = layout.wave_width_f64();
        match self.drag {
            Some(Drag::ZoomRange { start, .. }) => {
                self.drag = Some(Drag::ZoomRange { start, current: p });
                true
            }
            Some(Drag::Cursor) => {
                let x = f64::from(p.x - layout.waves.left()).clamp(0.0, wave_wf);
                let row = layout.row_at(p.y).filter(|r| *r < self.items.len());
                let hist = row.and_then(|r| self.items[r].history.clone());
                let t = snapped_time(
                    &self.viewport(doc),
                    hist.as_deref(),
                    x,
                    wave_wf,
                    doc.navigation.snap_px * f64::from(layout.zoom),
                );
                self.set_cursor(doc, Some(t));
                true
            }
            Some(Drag::Pan { last_x }) => {
                let dx = last_x - p.x;
                self.drag = Some(Drag::Pan { last_x: p.x });
                self.pan_px(doc, dx);
                true
            }
            // Column widths are kept at zoom 1.0 (they are saved in workspaces).
            Some(Drag::NamesSplit) => {
                self.names_width = ((p.x - layout.bounds.left()) / layout.zoom).max(MIN_COLUMN);
                true
            }
            Some(Drag::ValuesSplit) => {
                self.values_width = ((p.x - layout.names.right()) / layout.zoom).max(MIN_COLUMN);
                true
            }
            Some(Drag::Scroll { grab }) => {
                if let Some((track, thumb)) = layout.scrollbar {
                    let travel = track.height() - thumb.height();
                    if travel > 0.0 {
                        let frac = ((p.y - grab - track.top()) / travel).clamp(0.0, 1.0);
                        self.scroll_y = layout.max_scroll * frac;
                        return true;
                    }
                }
                false
            }
            None => {
                // Hover feedback only needs a repaint when the hovered row or
                // badge changes, when we enter/leave splitter zones, or when
                // the pointer leaves the table while something was hovered.
                let (prev_row, prev_badge, prev_split, prev_chip) = (
                    self.hover_row,
                    self.badge_hover,
                    self.split_hover,
                    self.chip_hover,
                );
                if layout.bounds.contains(p) {
                    let near_split = layout.near_split(p);
                    let chip = layout.chip_at(p).is_some();
                    self.split_hover = near_split;
                    self.chip_hover = chip;
                    self.update_hover();
                } else {
                    self.hover_row = None;
                    self.badge_hover = None;
                    self.split_hover = false;
                    self.chip_hover = false;
                }
                self.hover_row != prev_row
                    || self.badge_hover != prev_badge
                    || self.split_hover != prev_split
                    || self.chip_hover != prev_chip
            }
        }
    }

    /// Wheel over waves pans time; over names/values it scrolls rows.
    fn wheel(
        &mut self,
        doc: &mut Document,
        p: Point,
        dx: f32,
        dy: f32,
        modifiers: Modifiers,
        now: Instant,
    ) {
        if modifiers.control || modifiers.platform {
            let factor = 2f64.powf(f64::from(dy) / 120.0);
            let x = (p.x - self.layout.waves.left()).max(0.0);
            self.zoom_at(doc, x, factor);
        } else if modifiers.shift {
            // Some hosts translate Shift-wheel's vertical delta to horizontal.
            self.scroll_y = (self.scroll_y - dy - dx).clamp(0.0, self.layout.max_scroll);
        } else if p.x >= self.layout.waves.left() {
            self.pan_fraction(doc, -f64::from(dx + dy) / 1000.0, now);
        } else {
            if dx.abs() > 0.0 {
                self.pan_px(doc, -dx);
            }
            if dy.abs() > 0.0 {
                self.scroll_y = (self.scroll_y - dy).clamp(0.0, self.layout.max_scroll);
            }
        }
    }
}

/// Where a click on the waves at `x_px` lands after snapping to the nearest
/// transition of `history` within `snap_px` pixels (0 disables snapping).
pub fn snapped_time(
    vp: &Viewport,
    history: Option<&dyn SignalHistory>,
    x_px: f64,
    width_px: f64,
    snap_px: f64,
) -> u64 {
    let raw = vp.time_at(x_px, width_px).round().max(0.0);
    let Some(h) = history else { return raw as u64 };
    if snap_px <= 0.0 {
        return raw as u64;
    }
    let tol = snap_px / vp.px_per_unit(width_px);
    let t = raw as u64;
    let mut best: Option<(f64, u64)> = None;
    let mut consider = |cand: u64| {
        let d = (cand as f64 - raw).abs();
        if d <= tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, cand));
        }
    };
    match h.index_at(t) {
        Some(i) => {
            consider(h.time(i));
            if i + 1 < h.len() {
                consider(h.time(i + 1));
            }
        }
        None => {
            if !h.is_empty() {
                consider(h.time(0));
            }
        }
    }
    best.map(|(_, c)| c).unwrap_or(t)
}
