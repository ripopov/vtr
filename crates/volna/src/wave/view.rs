//! `WaveView`: the state of the waveform panel (displayed signals, selection,
//! cursor, markers, viewport) and the actions that mutate it. Painting lives
//! in `table.rs`.

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "view_tests.rs"]
mod tests;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Modifiers, Pixels, Point,
    Render, SharedString, Window, actions, div, px,
};
use web_time::Instant;

use super::table::WaveTable;
use super::viewport::Viewport;
use crate::data::{
    SignalHistory, SignalRef, SignalShape, Translator, Translators, VarId, WaveSource,
};
use crate::ui::{PopupMenu, PopupMenuItem, menu::PopupMenuEvent};

actions!(
    waves,
    [
        ZoomIn,
        ZoomOut,
        ZoomFit,
        GoToStart,
        GoToEnd,
        GoToCursor,
        PanLeft,
        PanRight,
        NextEdge,
        PrevEdge,
        AddMarker,
        ClearMarkers,
        RemoveSelected,
        SelectAll,
        ClearSelection,
        CycleFormat,
        MoveSelectionUp,
        MoveSelectionDown,
    ]
);

pub struct DisplayedSignal {
    pub var: VarId,
    signal: SignalRef,
    pub name: SharedString,
    pub scope: SharedString,
    pub shape: SignalShape,
    pub translator: Arc<dyn Translator>,
    pub history: Option<Arc<dyn SignalHistory>>,
    pub error: Option<SharedString>,
}

#[derive(Clone, Copy, Debug)]
pub struct Marker {
    pub time: u64,
}

pub struct ViewportAnimation {
    from: Viewport,
    to: Viewport,
    start: Instant,
    duration_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    Cursor,
    Pan { last_x: Pixels },
    NamesSplit,
    ValuesSplit,
    Scroll { grab: Pixels },
}

pub enum WaveViewEvent {
    /// Cursor, viewport, selection or items changed; the status bar re-reads state.
    Changed,
}

pub struct WaveView {
    pub(super) source: Option<Arc<dyn WaveSource>>,
    // Generations isolate completions even when the same source is reopened.
    load_generation: u64,
    pending: HashSet<SignalRef>,
    pub items: Vec<DisplayedSignal>,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    pub cursor: Option<u64>,
    pub markers: Vec<Marker>,
    pub viewport: Viewport,
    pub anim: Option<ViewportAnimation>,
    pub scroll_y: Pixels,
    pub names_width: Pixels,
    pub values_width: Pixels,
    pub hover_row: Option<usize>,
    pub badge_hover: Option<usize>,
    /// Pointer is over a column divider / a marker chip (drives repaints).
    pub split_hover: bool,
    pub chip_hover: bool,
    pub drag: Option<Drag>,
    pub translators: Translators,
    pub focus_handle: FocusHandle,
    /// Width of the waves column at the last layout, for keyboard zoom.
    pub wave_width: Pixels,
    /// Duration of the last table paint, and a smoothed average.
    pub frame_ms: f32,
    pub frame_ms_avg: f32,
    pub menu: Option<Entity<PopupMenu>>,
    menu_row: Option<usize>,
}

impl EventEmitter<WaveViewEvent> for WaveView {}

impl Focusable for WaveView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl WaveView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        WaveView {
            source: None,
            load_generation: 0,
            pending: HashSet::new(),
            items: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
            markers: Vec::new(),
            viewport: Viewport::fit((0, 1000)),
            anim: None,
            scroll_y: px(0.0),
            names_width: px(220.0),
            values_width: px(120.0),
            hover_row: None,
            badge_hover: None,
            split_hover: false,
            chip_hover: false,
            drag: None,
            translators: Translators::builtin(),
            focus_handle: cx.focus_handle(),
            wave_width: px(800.0),
            frame_ms: 0.0,
            frame_ms_avg: 0.0,
            menu: None,
            menu_row: None,
        }
    }

    pub fn set_source(&mut self, source: Option<Arc<dyn WaveSource>>, cx: &mut Context<Self>) {
        self.load_generation += 1;
        self.pending.clear();
        self.items.clear();
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.markers.clear();
        self.scroll_y = px(0.0);
        self.anim = None;
        self.menu = None;
        self.menu_row = None;
        self.drag = None;
        self.hover_row = None;
        self.badge_hover = None;
        if let Some(src) = &source {
            self.viewport = Viewport::fit(src.info().time_range);
        }
        self.source = source;
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(WaveViewEvent::Changed);
        cx.notify();
    }

    pub fn limits(&self) -> (u64, u64) {
        self.source
            .as_ref()
            .map(|s| s.info().time_range)
            .unwrap_or((0, 1000))
    }

    pub fn timescale(&self) -> i8 {
        self.source
            .as_ref()
            .map(|s| s.info().timescale)
            .unwrap_or(-9)
    }

    // -- items ---------------------------------------------------------------

    pub fn add_vars(&mut self, vars: &[VarId], cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let h = source.hierarchy();
        let first_new = self.items.len();
        let loaded: HashMap<_, _> = self
            .items
            .iter()
            .filter_map(|i| i.history.as_ref().map(|h| (i.signal, h.clone())))
            .collect();
        for &var in vars {
            let v = &h.vars[var];
            let translator = self.translators.default_for(v.shape);
            // Variable identity/format stay per row; aliases share immutable data.
            let history = loaded.get(&v.signal).cloned();
            let needs_load = history.is_none();
            self.items.push(DisplayedSignal {
                var,
                signal: v.signal,
                name: v.name.clone().into(),
                scope: h.scope_path(v.scope).join(".").into(),
                shape: v.shape,
                translator,
                history,
                error: None,
            });
            let signal = v.signal;
            if !needs_load || !self.pending.insert(signal) {
                continue;
            }
            let generation = self.load_generation;
            let source = source.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn(async move { source.load_signal(signal) })
                    .await;
                this.update(cx, |this, cx| {
                    this.finish_signal(generation, signal, result, cx);
                })
                .ok();
            })
            .detach();
        }
        if !vars.is_empty() {
            self.selected.clear();
            self.selected.extend(first_new..self.items.len());
            self.anchor = Some(first_new);
        }
        self.changed(cx);
    }

    fn finish_signal(
        &mut self,
        generation: u64,
        signal: SignalRef,
        result: anyhow::Result<Arc<dyn SignalHistory>>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.load_generation {
            return;
        }
        self.pending.remove(&signal);
        let result = result.map_err(|e| SharedString::from(e.to_string()));
        for item in self.items.iter_mut().filter(|i| i.signal == signal) {
            item.history = result.as_ref().ok().cloned();
            item.error = result.as_ref().err().cloned();
        }
        self.changed(cx);
    }

    pub fn remove_selected(&mut self, _: &RemoveSelected, _w: &mut Window, cx: &mut Context<Self>) {
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
        self.changed(cx);
    }

    pub fn select_all(&mut self, _: &SelectAll, _w: &mut Window, cx: &mut Context<Self>) {
        self.selected = (0..self.items.len()).collect();
        self.changed(cx);
    }

    pub fn clear_selection(&mut self, _: &ClearSelection, _w: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.menu = None;
        } else if !self.selected.is_empty() {
            self.selected.clear();
        } else {
            self.cursor = None;
        }
        self.changed(cx);
    }

    /// Click selection with platform conventions: plain = single, cmd/ctrl =
    /// toggle, shift = range from the anchor.
    pub fn select_row(&mut self, row: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if row >= self.items.len() {
            return;
        }
        crate::ui::selection::select(&mut self.selected, &mut self.anchor, row, modifiers);
        self.changed(cx);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
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
        self.changed(cx);
    }

    pub fn move_selection_up(
        &mut self,
        _: &MoveSelectionUp,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(-1, cx);
    }

    pub fn move_selection_down(
        &mut self,
        _: &MoveSelectionDown,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(1, cx);
    }

    // -- translators -----------------------------------------------------------

    pub fn set_translator(&mut self, rows: &[usize], id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.translators.get(id) else {
            return;
        };
        for &row in rows {
            if let Some(item) = self.items.get_mut(row)
                && t.applies(item.shape)
            {
                item.translator = t.clone();
            }
        }
        self.changed(cx);
    }

    pub fn cycle_format(&mut self, _: &CycleFormat, _w: &mut Window, cx: &mut Context<Self>) {
        let rows: Vec<usize> = self.selected.iter().copied().collect();
        for row in rows {
            let Some(item) = self.items.get(row) else {
                continue;
            };
            let options = self.translators.applicable(item.shape);
            if options.is_empty() {
                continue;
            }
            let pos = options
                .iter()
                .position(|t| t.id() == item.translator.id())
                .unwrap_or(0);
            let next = options[(pos + 1) % options.len()].clone();
            self.items[row].translator = next;
        }
        self.changed(cx);
    }

    /// Open the format menu for `row` at a window position.
    pub fn open_format_menu(
        &mut self,
        row: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.items.get(row) else {
            return;
        };
        let current = item.translator.id();
        let items: Vec<PopupMenuItem> = self
            .translators
            .applicable(item.shape)
            .into_iter()
            .map(|t| PopupMenuItem {
                id: t.id().into(),
                label: t.name().into(),
                badge: Some(t.badge().into()),
                checked: t.id() == current,
            })
            .collect();
        let menu = cx.new(|cx| PopupMenu::new(position, items, cx));
        let handle = menu.read(cx).focus_handle().clone();
        window.focus(&handle, cx);
        cx.subscribe(&menu, |this, _, event, cx| match event {
            PopupMenuEvent::Selected(id) => {
                let rows: Vec<usize> = match this.menu_row {
                    Some(r) if !this.selected.contains(&r) => vec![r],
                    _ => this.selected.iter().copied().collect(),
                };
                this.set_translator(&rows, id, cx);
                this.menu = None;
                this.menu_row = None;
                cx.notify();
            }
            PopupMenuEvent::Dismissed => {
                this.menu = None;
                this.menu_row = None;
                cx.notify();
            }
        })
        .detach();
        self.menu = Some(menu);
        self.menu_row = Some(row);
        cx.notify();
    }

    // -- navigation ------------------------------------------------------------

    fn animate_to(&mut self, target: Viewport, cx: &mut Context<Self>) {
        let mut target = target;
        target.clamp(self.limits());
        if self.viewport.approx_eq(&target) {
            return;
        }
        self.anim = Some(ViewportAnimation {
            from: self.viewport,
            to: target,
            start: Instant::now(),
            duration_ms: 140.0,
        });
        self.changed(cx);
    }

    /// Advance the viewport animation; returns true while it is still running.
    pub fn tick_animation(&mut self) -> bool {
        let Some(anim) = &self.anim else { return false };
        let t = (anim.start.elapsed().as_secs_f64() * 1000.0 / anim.duration_ms).min(1.0);
        // ease-out cubic
        let e = 1.0 - (1.0 - t).powi(3);
        self.viewport = Viewport::lerp(&anim.from, &anim.to, e);
        if t >= 1.0 {
            self.viewport = anim.to;
            self.anim = None;
            return false;
        }
        true
    }

    /// Immediate zoom (mouse wheel) around a pixel position in the waves column.
    pub fn zoom_at(&mut self, x_px: Pixels, factor: f64, cx: &mut Context<Self>) {
        self.anim = None;
        let w = f64::from(f32::from(self.wave_width)).max(1.0);
        let limits = self.limits();
        self.viewport
            .zoom_about(f64::from(f32::from(x_px)), w, factor, limits);
        self.changed(cx);
    }

    fn zoom_center(&mut self, factor: f64, cx: &mut Context<Self>) {
        let w = f64::from(f32::from(self.wave_width)).max(1.0);
        let anchor_x = match self.cursor {
            Some(c) => {
                let x = self.viewport.x_of(c as f64, w);
                if (0.0..=w).contains(&x) { x } else { w / 2.0 }
            }
            None => w / 2.0,
        };
        let mut target = self.viewport;
        target.zoom_about(anchor_x, w, factor, self.limits());
        self.animate_to(target, cx);
    }

    pub fn zoom_in(&mut self, _: &ZoomIn, _w: &mut Window, cx: &mut Context<Self>) {
        self.zoom_center(2.0, cx);
    }

    pub fn zoom_out(&mut self, _: &ZoomOut, _w: &mut Window, cx: &mut Context<Self>) {
        self.zoom_center(0.5, cx);
    }

    pub fn zoom_fit(&mut self, _: &ZoomFit, _w: &mut Window, cx: &mut Context<Self>) {
        let target = Viewport::fit(self.limits());
        self.animate_to(target, cx);
    }

    pub fn go_to_start(&mut self, _: &GoToStart, _w: &mut Window, cx: &mut Context<Self>) {
        let mut target = self.viewport;
        target.go_to_start(self.limits());
        self.animate_to(target, cx);
    }

    pub fn go_to_end(&mut self, _: &GoToEnd, _w: &mut Window, cx: &mut Context<Self>) {
        let mut target = self.viewport;
        target.go_to_end(self.limits());
        self.animate_to(target, cx);
    }

    pub fn go_to_cursor(&mut self, _: &GoToCursor, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.cursor else { return };
        let mut target = self.viewport;
        target.center_on(c as f64, self.limits());
        self.animate_to(target, cx);
    }

    fn pan_fraction(&mut self, frac: f64, cx: &mut Context<Self>) {
        let mut target = self.viewport;
        let w = target.width();
        target.start += w * frac;
        target.end += w * frac;
        self.animate_to(target, cx);
    }

    pub fn pan_left(&mut self, _: &PanLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.pan_fraction(-0.25, cx);
    }

    pub fn pan_right(&mut self, _: &PanRight, _w: &mut Window, cx: &mut Context<Self>) {
        self.pan_fraction(0.25, cx);
    }

    /// Immediate pan by pixels (mouse drag / wheel).
    pub fn pan_px(&mut self, dx: Pixels, cx: &mut Context<Self>) {
        self.anim = None;
        let w = f64::from(f32::from(self.wave_width)).max(1.0);
        let limits = self.limits();
        self.viewport.pan_px(f64::from(f32::from(dx)), w, limits);
        self.changed(cx);
    }

    fn edge_history(&self) -> Option<Arc<dyn SignalHistory>> {
        let row = self
            .anchor
            .filter(|r| self.selected.contains(r))
            .or_else(|| self.selected.iter().next().copied())?;
        self.items.get(row)?.history.clone()
    }

    fn reveal_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.cursor else { return };
        let c = c as f64;
        if c < self.viewport.start || c > self.viewport.end {
            let mut target = self.viewport;
            target.center_on(c, self.limits());
            self.animate_to(target, cx);
        }
    }

    pub fn next_edge(&mut self, _: &NextEdge, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.edge_history() else { return };
        let from = self.cursor.unwrap_or(self.viewport.start.max(0.0) as u64);
        if let Some(t) = h.next_change_after(from) {
            self.cursor = Some(t);
            self.reveal_cursor(cx);
            self.changed(cx);
        }
    }

    pub fn prev_edge(&mut self, _: &PrevEdge, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.edge_history() else { return };
        let from = self.cursor.unwrap_or(self.viewport.end.max(0.0) as u64);
        if let Some(t) = h.prev_change_before(from) {
            self.cursor = Some(t);
            self.reveal_cursor(cx);
            self.changed(cx);
        }
    }

    // -- cursor and markers ----------------------------------------------------

    pub fn set_cursor(&mut self, t: Option<u64>, cx: &mut Context<Self>) {
        if self.cursor != t {
            self.cursor = t;
            self.changed(cx);
        }
    }

    pub fn add_marker(&mut self, _: &AddMarker, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.cursor else { return };
        if self.markers.iter().any(|m| m.time == c) {
            return;
        }
        self.markers.push(Marker { time: c });
        self.markers.sort_by_key(|m| m.time);
        self.changed(cx);
    }

    pub fn clear_markers(&mut self, _: &ClearMarkers, _w: &mut Window, cx: &mut Context<Self>) {
        self.markers.clear();
        self.changed(cx);
    }

    pub fn remove_marker(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.markers.len() {
            self.markers.remove(ix);
            self.changed(cx);
        }
    }

    /// Record the paint time of the last frame.
    pub fn record_frame(&mut self, ms: f32) {
        self.frame_ms = ms;
        self.frame_ms_avg = if self.frame_ms_avg == 0.0 {
            ms
        } else {
            self.frame_ms_avg * 0.9 + ms * 0.1
        };
    }
}

impl Render for WaveView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tick_animation() {
            window.request_animation_frame();
        }
        div()
            .id("wave-view")
            .track_focus(&self.focus_handle)
            .key_context("Waves")
            .size_full()
            .relative()
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_fit))
            .on_action(cx.listener(Self::go_to_start))
            .on_action(cx.listener(Self::go_to_end))
            .on_action(cx.listener(Self::go_to_cursor))
            .on_action(cx.listener(Self::pan_left))
            .on_action(cx.listener(Self::pan_right))
            .on_action(cx.listener(Self::next_edge))
            .on_action(cx.listener(Self::prev_edge))
            .on_action(cx.listener(Self::add_marker))
            .on_action(cx.listener(Self::clear_markers))
            .on_action(cx.listener(Self::remove_selected))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear_selection))
            .on_action(cx.listener(Self::cycle_format))
            .on_action(cx.listener(Self::move_selection_up))
            .on_action(cx.listener(Self::move_selection_down))
            .child(WaveTable::new(cx.entity()))
            .children(self.menu.clone())
    }
}
