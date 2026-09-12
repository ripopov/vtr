//! `WaveTable`: the GPUI element that hosts the core's wave panel. It asks the
//! core for the frame's layout (for hitboxes), paints the display list the
//! core produces, and forwards pointer input as commands.

use std::collections::HashMap;

use gpui::{
    App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId, Entity, Font,
    FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    PinchEvent, Pixels, ScrollWheelEvent, ShapedLine, SharedString, Style, TextAlign, TextRun,
    Window, fill, point, px, quad, size,
};
use volna_core::app::Command;
use volna_core::geometry::{CursorIcon, Point as CPoint, Rect as CRect};
use volna_core::scene::{Prim, TextMeasure};
use volna_core::wave::PointerEvent;
use volna_core::{FontRole, Instant, Scene};

use crate::app::{TextKey, Workspace, to_modifiers};
use crate::theme::{Theme, core_theme, hsla, theme};

pub struct WaveTable {
    ws: Entity<Workspace>,
}

impl WaveTable {
    pub fn new(ws: Entity<Workspace>) -> Self {
        WaveTable { ws }
    }
}

pub struct TablePrepaint {
    hitbox: Hitbox,
    /// Small interactive rectangles, so GPUI resolves pointer shapes per position.
    regions: Vec<(CRect, Hitbox)>,
    row_h: f32,
}

impl IntoElement for WaveTable {
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

fn cpoint(p: gpui::Point<Pixels>) -> CPoint {
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
                        gpui::BorderStyle::default(),
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
                        gpui::TransformationMatrix::unit(),
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

impl Element for WaveTable {
    type RequestLayoutState = ();
    type PrepaintState = TablePrepaint;

    fn id(&self) -> Option<ElementId> {
        None
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
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
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
    ) -> TablePrepaint {
        let t = *core_theme(cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let (rects, row_h) = self.ws.update(cx, |ws, _| {
            let layout = ws.app.layout_waves(cbounds(bounds), &t);
            let mut rects = vec![layout.names_split, layout.values_split];
            rects.extend(layout.badges.iter().map(|(_, b)| *b));
            rects.extend(layout.marker_chips.iter().map(|(_, b)| *b));
            (rects, layout.row_h)
        });
        let regions = rects
            .into_iter()
            .map(|r| (r, window.insert_hitbox(gbounds(r), HitboxBehavior::Normal)))
            .collect();
        TablePrepaint {
            hitbox,
            regions,
            row_h,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut TablePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let t = *theme(cx);
        let core = *core_theme(cx);
        // Build the display list, then paint it. The scene buffer and the
        // shaped-text cache live on the workspace so they persist across frames.
        let (mut scene, mut shaped) = self.ws.update(cx, |ws, _| {
            let mut scene = std::mem::take(&mut ws.scene);
            let mut measure = GpuiMeasure { window, theme: &t };
            ws.app.render_waves_into(&core, &mut measure, &mut scene);
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
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        self.ws.update(cx, |ws, _| {
            ws.scene = scene;
            ws.shaped = shaped;
            ws.app.waves.record_frame(ms);
        });
    }
}

impl WaveTable {
    fn register_mouse_handlers(&self, prepaint: &TablePrepaint, window: &mut Window) {
        let ws = self.ws.clone();
        let hitbox = prepaint.hitbox.clone();
        let row_h = prepaint.row_h;
        let send = |ws: &Entity<Workspace>, ev: PointerEvent, window: &mut Window, cx: &mut App| {
            ws.update(cx, |ws, cx| {
                ws.dispatch(Command::Pointer(ev), Some(window), cx)
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
                let focus = ws.read(cx).waves_focus.clone();
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
                if ws.read(cx).app.waves.drag.is_some() {
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

        // Wheel: zoom with cmd/ctrl, pan horizontally, scroll rows vertically.
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
                    },
                    window,
                    cx,
                );
            }
        });
    }
}

#[allow(dead_code)]
fn _scene_is_send(_: Scene) {}
