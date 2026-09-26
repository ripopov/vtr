//! `PanelCanvas`: the GPUI element that hosts a core-painted panel (waves or
//! pipeline). It asks the core for the frame's layout (for hitboxes), paints
//! the display list the core produces, and forwards pointer input as commands.

use std::collections::HashMap;

use gpui_kit::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity, Font,
    FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    PinchEvent, Pixels, ScrollDelta, ScrollWheelEvent, ShapedLine, SharedString, Style, TextAlign,
    TextRun, Window, fill, point, px, quad, size,
};
use volna_core::FontRole;
use volna_core::app::{Command, PanelLayout};
use volna_core::geometry::{CursorIcon, Point as CPoint, Rect as CRect};
use volna_core::scene::{Prim, TextMeasure};
use volna_core::wave::PointerEvent;

use crate::app::{TextKey, Workspace, to_modifiers};
use crate::theme::{Theme, core_theme, hsla, theme};

pub struct PanelCanvas {
    ws: Entity<Workspace>,
    panel: volna_core::panels::PanelId,
    generation: u64,
    table_focus: Option<gpui_kit::FocusHandle>,
    /// A waveform panel: its rows are exposed as a tree.
    waves: bool,
}

impl PanelCanvas {
    pub fn new(ws: Entity<Workspace>, panel: volna_core::panels::PanelId, generation: u64) -> Self {
        PanelCanvas {
            ws,
            panel,
            generation,
            table_focus: None,
            waves: false,
        }
    }

    pub fn waves(mut self, waves: bool) -> Self {
        self.waves = waves;
        self
    }

    pub fn table(mut self, focus: gpui_kit::FocusHandle) -> Self {
        self.table_focus = Some(focus);
        self
    }
}

pub struct CanvasPrepaint {
    hitbox: Hitbox,
    /// Small interactive rectangles, so GPUI resolves pointer shapes per position.
    regions: Vec<(CRect, Hitbox)>,
    row_h: f32,
    accessible: Vec<(
        volna_core::table::AccessibleRow,
        Option<gpui_kit::accesskit::NodeId>,
    )>,
    table_status: Option<String>,
    /// Wave rows on screen, while assistive technology is active.
    wave_rows: Vec<volna_core::wave::model::AccessibleRow>,
    scale: f64,
}

impl IntoElement for PanelCanvas {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn gbounds(r: CRect) -> Bounds<Pixels> {
    Bounds::new(
        point(px(r.origin.x), px(r.origin.y)),
        size(px(r.size.width), px(r.size.height)),
    )
}

fn cbounds(b: Bounds<Pixels>) -> CRect {
    CRect::from_xywh(
        f32::from(b.origin.x),
        f32::from(b.origin.y),
        f32::from(b.size.width),
        f32::from(b.size.height),
    )
}

fn cpoint(p: gpui_kit::Point<Pixels>) -> CPoint {
    CPoint {
        x: f32::from(p.x),
        y: f32::from(p.y),
    }
}

fn font(t: &Theme, role: FontRole) -> Font {
    let weight = match role {
        FontRole::Ui | FontRole::Mono => FontWeight::NORMAL,
        FontRole::UiMedium => FontWeight::MEDIUM,
        FontRole::UiSemibold => FontWeight::SEMIBOLD,
    };
    Font {
        family: t.font_family(role).into(),
        features: Default::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

fn shape(
    window: &Window,
    t: &Theme,
    text: &str,
    role: FontRole,
    size: f32,
    color: Hsla,
) -> ShapedLine {
    let text: SharedString = text.to_owned().into();
    let run = TextRun {
        len: text.len(),
        font: font(t, role),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, px(size), &[run], None)
}

/// A wave row as a tree item: its name, level, selection and, for a group,
/// whether it is expanded.
pub(crate) fn wave_row_node(
    row: &volna_core::wave::model::AccessibleRow,
    scale: f64,
) -> gpui_kit::accesskit::Node {
    use gpui_kit::accesskit::{Node, Rect, Role};
    let mut node = Node::new(Role::TreeItem);
    node.set_label(row.label.clone());
    node.set_level(row.level);
    node.set_selected(row.selected);
    if let Some(expanded) = row.expanded {
        node.set_expanded(expanded);
    }
    node.set_bounds(Rect::new(
        row.bounds.left() as f64 * scale,
        row.bounds.top() as f64 * scale,
        row.bounds.right() as f64 * scale,
        row.bounds.bottom() as f64 * scale,
    ));
    node
}

/// Width measurement through GPUI's text system.
struct GpuiMeasure<'a> {
    window: &'a Window,
    theme: &'a Theme,
}

impl TextMeasure for GpuiMeasure<'_> {
    fn text_width(&mut self, text: &str, font: FontRole, size: f32) -> f32 {
        f32::from(shape(self.window, self.theme, text, font, size, Hsla::default()).width())
    }
}

const SHAPED_CACHE_LIMIT: usize = 4096;

fn cursor_style(icon: CursorIcon) -> CursorStyle {
    match icon {
        CursorIcon::Default => CursorStyle::Arrow,
        CursorIcon::PointingHand => CursorStyle::PointingHand,
        CursorIcon::ResizeLeftRight => CursorStyle::ResizeLeftRight,
        CursorIcon::ResizeUpDown => CursorStyle::ResizeUpDown,
        CursorIcon::Grabbing => CursorStyle::ClosedHand,
    }
}

/// Paint `prims[*i..]` until the matching `PopClip`, nesting clips as content masks.
fn paint_prims(
    prims: &[Prim],
    i: &mut usize,
    t: &Theme,
    shaped: &mut HashMap<TextKey, ShapedLine>,
    window: &mut Window,
    cx: &mut App,
) {
    while *i < prims.len() {
        let prim = &prims[*i];
        *i += 1;
        match prim {
            Prim::Quad {
                rect,
                fill: color,
                radius,
                border_width,
                border_color,
            } => {
                let b = gbounds(*rect);
                if *radius == 0.0 && *border_width == 0.0 {
                    window.paint_quad(fill(b, hsla(*color)));
                } else {
                    window.paint_quad(quad(
                        b,
                        px(*radius),
                        hsla(*color),
                        px(*border_width),
                        hsla(*border_color),
                        gpui_kit::BorderStyle::default(),
                    ));
                }
            }
            Prim::Lines {
                segments,
                color,
                width,
            } => {
                let mut builder = PathBuilder::stroke(px(*width));
                for [a, b] in segments {
                    builder.move_to(point(px(a.x), px(a.y)));
                    builder.line_to(point(px(b.x), px(b.y)));
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, hsla(*color));
                }
            }
            Prim::Text {
                origin,
                height,
                text,
                font: role,
                size,
                color,
            } => {
                let color = hsla(*color);
                let key = TextKey {
                    text: text.clone(),
                    font: *role,
                    size: size.to_bits(),
                    color: [color.h, color.s, color.l, color.a].map(f32::to_bits),
                };
                if shaped.len() >= SHAPED_CACHE_LIMIT {
                    shaped.clear();
                }
                let line = shaped
                    .entry(key)
                    .or_insert_with(|| shape(window, t, text, *role, *size, color));
                line.paint(
                    point(px(origin.x), px(origin.y)),
                    px(*height),
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
            Prim::Icon { name, rect, color } => {
                window
                    .paint_svg(
                        gbounds(*rect),
                        name.path().into(),
                        None,
                        gpui_kit::TransformationMatrix::unit(),
                        hsla(*color),
                        cx,
                    )
                    .ok();
            }
            Prim::PushClip(rect) => {
                let mask = ContentMask {
                    bounds: gbounds(*rect),
                };
                window.with_content_mask(Some(mask), |window| {
                    paint_prims(prims, i, t, shaped, window, cx)
                });
            }
            Prim::PopClip => return,
        }
    }
}

impl Element for PanelCanvas {
    type RequestLayoutState = ();
    type PrepaintState = CanvasPrepaint;

    fn id(&self) -> Option<ElementId> {
        if self.table_focus.is_some() {
            Some(ElementId::Name("table-rows".into()))
        } else {
            self.waves.then(|| ElementId::Name("wave-rows".into()))
        }
    }

    fn a11y_role(&self) -> Option<gpui_kit::Role> {
        if self.table_focus.is_some() {
            Some(gpui_kit::Role::ListBox)
        } else {
            self.waves.then_some(gpui_kit::Role::Tree)
        }
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut CanvasPrepaint,
        builder: &mut gpui_kit::A11ySubtreeBuilder,
    ) {
        use gpui_kit::accesskit::{Action, Node, Rect, Role};
        if self.waves {
            builder.parent_node().set_label("Waveform rows");
            builder
                .parent_node()
                .set_description("Groups fold with Left and Right. Only visible rows are exposed.");
            for row in &prepaint.wave_rows {
                let node_id = builder.synthetic_node_id((self.generation, self.panel.0, row.entry));
                builder.push_child(node_id, wave_row_node(row, prepaint.scale));
            }
            return;
        }
        builder
            .parent_node()
            .set_label(prepaint.table_status.as_deref().unwrap_or("Table rows"));
        builder.parent_node().set_description(
            "Arrow keys navigate rows. Enter opens record details. Only visible rows are exposed.",
        );
        for (row, id) in &mut prepaint.accessible {
            let node_id = builder.synthetic_node_id((self.generation, self.panel.0, row.ordinal));
            let mut node = Node::new(Role::ListBoxOption);
            node.set_label(row.label.clone());
            node.set_selected(row.selected);
            node.set_row_index_text(row.ordinal.saturating_add(1).to_string());
            node.set_bounds(Rect::new(
                row.bounds.left() as f64 * prepaint.scale,
                row.bounds.top() as f64 * prepaint.scale,
                row.bounds.right() as f64 * prepaint.scale,
                row.bounds.bottom() as f64 * prepaint.scale,
            ));
            node.add_action(Action::Click);
            node.add_action(Action::Focus);
            if row.selected {
                builder.parent_node().set_active_descendant(node_id);
            }
            builder.push_child(node_id, node);
            *id = Some(node_id);
        }
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui_kit::relative(1.0).into();
        style.size.height = gpui_kit::relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> CanvasPrepaint {
        let t = *core_theme(cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let rects = self.ws.update(cx, |ws, _| {
            match ws.app.layout_panel(self.panel, cbounds(bounds), &t) {
                Some(PanelLayout::Waves(layout)) => {
                    let mut rects = vec![layout.names_split, layout.values_split];
                    rects.extend(layout.badges.iter().map(|(_, b)| *b));
                    rects.extend(layout.marker_chips.iter().map(|(_, b)| *b));
                    rects
                }
                Some(PanelLayout::Pipeline(layout)) => {
                    let mut rects = vec![layout.label_split];
                    rects.extend(layout.marker_chips.iter().map(|(_, b)| *b));
                    rects.extend(layout.retry);
                    rects.extend(layout.activity_controls.iter().map(|(_, rect, _)| *rect));
                    rects
                }
                Some(PanelLayout::Table(_)) => Vec::new(),
                None => Vec::new(),
            }
        });
        let regions = rects
            .into_iter()
            .map(|r| (r, window.insert_hitbox(gbounds(r), HitboxBehavior::Normal)))
            .collect();
        let table = window
            .is_a11y_active()
            .then(|| {
                self.ws
                    .read(cx)
                    .app
                    .panels
                    .get(self.panel)
                    .and_then(|panel| panel.kind.table())
            })
            .flatten();
        CanvasPrepaint {
            hitbox,
            regions,
            row_h: t.row_height,
            accessible: table
                .map(|table| table.accessible_rows().map(|row| (row, None)).collect())
                .unwrap_or_default(),
            table_status: table.map(|table| format!("Table rows. {}", table.status())),
            wave_rows: if self.waves && window.is_a11y_active() {
                self.ws
                    .read(cx)
                    .app
                    .panels
                    .waves(self.panel)
                    .map(|w| w.accessible_rows().collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            scale: window.scale_factor() as f64,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut CanvasPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(focus) = &self.table_focus {
            for (row, node_id) in &prepaint.accessible {
                let Some(node_id) = node_id else { continue };
                for action in [
                    gpui_kit::AccessibleAction::Click,
                    gpui_kit::AccessibleAction::Focus,
                ] {
                    let owner = self.ws.clone();
                    let focus = focus.clone();
                    let generation = self.generation;
                    let panel = self.panel;
                    let ordinal = row.ordinal;
                    window.on_a11y_action(*node_id, action, move |_, window, cx| {
                        owner.update(cx, |ws, cx| {
                            ws.dispatch_if_current(
                                generation,
                                Command::Table(
                                    panel,
                                    volna_core::table::TableCommand::Select(ordinal),
                                ),
                                Some(window),
                                cx,
                            )
                        });
                        window.focus(&focus, cx);
                    });
                }
            }
        }
        let t = *theme(cx);
        let core = *core_theme(cx);
        // Build the display list, then paint it. The scene buffer and the
        // shaped-text cache live on the workspace so they persist across frames.
        let (mut scene, mut shaped) = self.ws.update(cx, |ws, _| {
            let mut scene = std::mem::take(&mut ws.scene);
            let mut measure = GpuiMeasure { window, theme: &t };
            ws.app
                .render_panel_into(self.panel, &core, &mut measure, &mut scene);
            (scene, std::mem::take(&mut ws.shaped))
        });
        let mut i = 0;
        paint_prims(&scene.prims, &mut i, &t, &mut shaped, window, cx);
        // Pointer shapes.
        if let Some(icon) = scene.window_cursor {
            window.set_window_cursor_style(cursor_style(icon));
        } else {
            for (rect, icon) in &scene.cursors {
                if let Some((_, hitbox)) = prepaint.regions.iter().find(|(r, _)| r == rect) {
                    window.set_cursor_style(cursor_style(*icon), hitbox);
                }
            }
        }
        scene.clear();
        self.register_mouse_handlers(prepaint, window);
        self.ws.update(cx, |ws, _| {
            ws.scene = scene;
            ws.shaped = shaped;
            if let Some(panel) = ws.app.panels.get_mut(self.panel) {
                panel.record_paint();
            }
        });
    }
}

impl PanelCanvas {
    fn register_mouse_handlers(&self, prepaint: &CanvasPrepaint, window: &mut Window) {
        let ws = self.ws.clone();
        let hitbox = prepaint.hitbox.clone();
        let row_h = prepaint.row_h;
        let panel = self.panel;
        let generation = self.generation;
        let send = move |ws: &Entity<Workspace>,
                         ev: PointerEvent,
                         window: &mut Window,
                         cx: &mut App| {
            ws.update(cx, |ws, cx| {
                ws.dispatch_if_current(generation, Command::Pointer(panel, ev), Some(window), cx)
            });
        };

        window.on_mouse_event({
            let ws = ws.clone();
            let hitbox = hitbox.clone();
            move |ev: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let button = match ev.button {
                    MouseButton::Left => volna_core::geometry::MouseButton::Left,
                    MouseButton::Middle => volna_core::geometry::MouseButton::Middle,
                    MouseButton::Right => volna_core::geometry::MouseButton::Right,
                    _ => return,
                };
                if ws.read(cx).app.doc.generation() != generation {
                    return;
                }
                // A press anywhere else keeps the name typed so far.
                ws.update(cx, |ws, cx| ws.commit_rename(window, cx));
                let focus = ws
                    .read(cx)
                    .dock
                    .as_ref()
                    .and_then(|dock| dock.focus(panel, cx))
                    .unwrap_or_else(|| ws.read(cx).waves_focus.clone());
                window.focus(&focus, cx);
                send(
                    &ws,
                    PointerEvent::Down {
                        position: cpoint(ev.position),
                        button,
                        modifiers: to_modifiers(ev.modifiers),
                    },
                    window,
                    cx,
                );
                // A second click on a record-bearing panel opens what the
                // first click selected; the core decides where.
                let selects = ws.read(cx).app.panels.get(panel).is_some_and(|p| {
                    p.kind.pipeline().is_some()
                        || p.kind.table().is_some()
                        || p.kind.waves().is_some_and(|w| w.pressed_record)
                });
                // A second click on a group's name renames it.
                let group_name = ws
                    .read(cx)
                    .app
                    .panels
                    .waves(panel)
                    .and_then(|w| w.group_name_at(cpoint(ev.position)))
                    .is_some();
                if ev.click_count >= 2
                    && button == volna_core::geometry::MouseButton::Left
                    && group_name
                {
                    ws.update(cx, |ws, cx| {
                        ws.dispatch_if_current(
                            generation,
                            Command::Action(volna_core::app::Action::RenameGroup),
                            Some(window),
                            cx,
                        )
                    });
                } else if ev.click_count >= 2
                    && button == volna_core::geometry::MouseButton::Left
                    && selects
                {
                    ws.update(cx, |ws, cx| {
                        ws.dispatch_if_current(
                            generation,
                            Command::ShowTransaction { from: panel },
                            Some(window),
                            cx,
                        )
                    });
                }
            }
        });

        window.on_mouse_event({
            let ws = ws.clone();
            move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                send(
                    &ws,
                    PointerEvent::Move {
                        position: cpoint(ev.position),
                    },
                    window,
                    cx,
                );
            }
        });

        window.on_mouse_event({
            let ws = ws.clone();
            move |_: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                if ws
                    .read(cx)
                    .app
                    .panels
                    .get(panel)
                    .is_some_and(|p| p.dragging())
                {
                    send(&ws, PointerEvent::Up, window, cx);
                }
            }
        });

        // Trackpad pinch: zoom about the gesture centre.
        window.on_mouse_event({
            let ws = ws.clone();
            let hitbox = hitbox.clone();
            move |ev: &PinchEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                send(
                    &ws,
                    PointerEvent::Pinch {
                        position: cpoint(ev.position),
                        delta: ev.delta,
                    },
                    window,
                    cx,
                );
            }
        });

        // The core resolves wheel zoom, time pan and row scroll from the hit
        // region and from whether the deltas are a trackpad's exact pixels.
        window.on_mouse_event({
            move |ev: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let delta = ev.delta.pixel_delta(px(row_h));
                send(
                    &ws,
                    PointerEvent::Wheel {
                        position: cpoint(ev.position),
                        dx: f32::from(delta.x),
                        dy: f32::from(delta.y),
                        modifiers: to_modifiers(ev.modifiers),
                        precise: matches!(ev.delta, ScrollDelta::Pixels(_)),
                    },
                    window,
                    cx,
                );
            }
        });
    }
}
