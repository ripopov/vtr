//! Paint a core [`Scene`] with an egui [`Painter`], and measure text through
//! egui's font system.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Stroke, StrokeKind};
use volna_core::geometry::{CursorIcon, Rect as CRect};
use volna_core::icons::IconName;
use volna_core::scene::{Prim, TextMeasure};
use volna_core::{Color, FontRole, Scene};

pub fn c32(c: Color) -> Color32 {
    let [r, g, b, a] = c.to_rgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub fn rect(r: CRect) -> Rect {
    Rect::from_min_size(
        Pos2::new(r.origin.x, r.origin.y),
        egui::vec2(r.size.width, r.size.height),
    )
}

/// The bundled families, as installed by [`crate::font_definitions`].
pub const UI_FAMILY: &str = "IBM Plex Sans";
pub const UI_SEMIBOLD_FAMILY: &str = "IBM Plex Sans SemiBold";
pub const MONO_FAMILY: &str = "Lilex";

pub fn font_id(role: FontRole, size: f32) -> FontId {
    let family = match role {
        FontRole::Ui => UI_FAMILY,
        FontRole::UiMedium | FontRole::UiSemibold => UI_SEMIBOLD_FAMILY,
        FontRole::Mono => MONO_FAMILY,
    };
    FontId::new(size, FontFamily::Name(Arc::from(family)))
}

pub fn cursor_icon(icon: CursorIcon) -> egui::CursorIcon {
    match icon {
        CursorIcon::Default => egui::CursorIcon::Default,
        CursorIcon::PointingHand => egui::CursorIcon::PointingHand,
        CursorIcon::ResizeLeftRight => egui::CursorIcon::ResizeHorizontal,
        CursorIcon::ResizeUpDown => egui::CursorIcon::ResizeVertical,
    }
}

/// Text measurement through the egui context's fonts.
pub struct EguiMeasure<'a>(pub &'a egui::Context);

impl TextMeasure for EguiMeasure<'_> {
    fn text_width(&mut self, text: &str, font: FontRole, size: f32) -> f32 {
        self.0.fonts_mut(|f| {
            f.layout_no_wrap(text.to_owned(), font_id(font, size), Color32::WHITE)
                .size()
                .x
        })
    }
}

/// Paint every primitive. Clips nest through painters with narrowed clip rects.
pub fn paint_scene(scene: &Scene, painter: &Painter) {
    let mut stack: Vec<Painter> = vec![painter.clone()];
    for prim in &scene.prims {
        let p = stack.last().expect("clip stack");
        match prim {
            Prim::Quad {
                rect: r,
                fill,
                radius,
                border_width,
                border_color,
            } => {
                let stroke = if *border_width > 0.0 {
                    Stroke::new(*border_width, c32(*border_color))
                } else {
                    Stroke::NONE
                };
                p.rect(
                    rect(*r),
                    CornerRadius::same(radius.round() as u8),
                    c32(*fill),
                    stroke,
                    StrokeKind::Inside,
                );
            }
            Prim::Lines {
                segments,
                color,
                width,
            } => {
                let stroke = Stroke::new(*width, c32(*color));
                for [a, b] in segments {
                    p.line_segment([Pos2::new(a.x, a.y), Pos2::new(b.x, b.y)], stroke);
                }
            }
            Prim::Text {
                origin,
                height,
                text,
                font,
                size,
                color,
            } => {
                let color = c32(*color);
                let galley = p.layout_no_wrap(text.clone(), font_id(*font, *size), color);
                let y = origin.y + (height - galley.size().y) / 2.0;
                p.galley(Pos2::new(origin.x, y.round()), galley, color);
            }
            Prim::Icon {
                name,
                rect: r,
                color,
            } => icon(p, *name, rect(*r), c32(*color)),
            Prim::PushClip(r) => {
                let next = p.with_clip_rect(rect(*r));
                stack.push(next);
            }
            Prim::PopClip => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
        }
    }
}

/// A disclosure triangle for tree rows.
pub fn chevron(p: &Painter, center: Pos2, expanded: bool, color: Color32) {
    let s = 3.5;
    let pts = if expanded {
        vec![
            Pos2::new(center.x - s, center.y - s / 2.0),
            Pos2::new(center.x + s, center.y - s / 2.0),
            Pos2::new(center.x, center.y + s / 2.0 + 1.0),
        ]
    } else {
        vec![
            Pos2::new(center.x - s / 2.0, center.y - s),
            Pos2::new(center.x + s / 2.0 + 1.0, center.y),
            Pos2::new(center.x - s / 2.0, center.y + s),
        ]
    };
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Icons are drawn as strokes from their Lucide geometry (24-unit view box),
/// so no font has to carry the glyphs.
pub(crate) fn icon(p: &Painter, name: IconName, r: Rect, color: Color32) {
    let s = r.width() / 24.0;
    let at = |x: f32, y: f32| Pos2::new(r.min.x + x * s, r.min.y + y * s);
    let stroke = Stroke::new((2.0 * s).max(1.0), color);
    let polyline = |pts: &[(f32, f32)]| {
        for w in pts.windows(2) {
            p.line_segment([at(w[0].0, w[0].1), at(w[1].0, w[1].1)], stroke);
        }
    };
    match name {
        IconName::Box => {
            // A cube: front square plus the top and side edges.
            polyline(&[
                (4.0, 8.0),
                (12.0, 4.0),
                (20.0, 8.0),
                (20.0, 16.0),
                (12.0, 20.0),
                (4.0, 16.0),
                (4.0, 8.0),
            ]);
            polyline(&[(4.0, 8.0), (12.0, 12.0), (20.0, 8.0)]);
            polyline(&[(12.0, 12.0), (12.0, 20.0)]);
        }
        IconName::Braces => {
            polyline(&[
                (9.0, 4.0),
                (7.0, 5.0),
                (7.0, 10.0),
                (5.0, 12.0),
                (7.0, 14.0),
                (7.0, 19.0),
                (9.0, 20.0),
            ]);
            polyline(&[
                (15.0, 4.0),
                (17.0, 5.0),
                (17.0, 10.0),
                (19.0, 12.0),
                (17.0, 14.0),
                (17.0, 19.0),
                (15.0, 20.0),
            ]);
        }
        IconName::Folder => {
            polyline(&[
                (3.0, 19.0),
                (3.0, 5.0),
                (9.0, 5.0),
                (11.0, 8.0),
                (21.0, 8.0),
                (21.0, 19.0),
                (3.0, 19.0),
            ]);
        }
        IconName::Activity => {
            polyline(&[
                (2.0, 12.0),
                (6.0, 12.0),
                (9.0, 4.0),
                (15.0, 20.0),
                (18.0, 12.0),
                (22.0, 12.0),
            ]);
        }
        IconName::Binary => {
            polyline(&[
                (6.0, 4.0),
                (10.0, 4.0),
                (10.0, 10.0),
                (6.0, 10.0),
                (6.0, 4.0),
            ]);
            polyline(&[
                (14.0, 14.0),
                (18.0, 14.0),
                (18.0, 20.0),
                (14.0, 20.0),
                (14.0, 14.0),
            ]);
            polyline(&[(16.0, 4.0), (16.0, 10.0)]);
            polyline(&[(8.0, 14.0), (8.0, 20.0)]);
        }
        IconName::Sigma => {
            polyline(&[
                (18.0, 5.0),
                (6.0, 5.0),
                (13.0, 12.0),
                (6.0, 19.0),
                (18.0, 19.0),
            ]);
        }
        IconName::Type => {
            polyline(&[(5.0, 6.0), (19.0, 6.0)]);
            polyline(&[(12.0, 6.0), (12.0, 19.0)]);
            polyline(&[(9.0, 19.0), (15.0, 19.0)]);
        }
        IconName::ListTree => {
            polyline(&[(13.0, 12.0), (21.0, 12.0)]);
            polyline(&[(8.0, 6.0), (21.0, 6.0)]);
            polyline(&[(13.0, 18.0), (21.0, 18.0)]);
            polyline(&[
                (3.0, 6.0),
                (3.0, 10.0),
                (3.6, 11.4),
                (5.0, 12.0),
                (8.0, 12.0),
            ]);
            polyline(&[
                (3.0, 10.0),
                (3.0, 16.0),
                (3.6, 17.4),
                (5.0, 18.0),
                (8.0, 18.0),
            ]);
        }
        _ => {
            // Other icons are not painted on the canvas today; draw a box so a
            // missing mapping is visible rather than silent.
            p.rect_stroke(
                r.shrink(2.0 * s),
                CornerRadius::ZERO,
                stroke,
                StrokeKind::Inside,
            );
        }
    }
}
