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
        CursorIcon::Grabbing => egui::CursorIcon::Grabbing,
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
        IconName::ChartNoAxesGantt => {
            polyline(&[(6., 5.), (18., 5.)]);
            polyline(&[(4., 12.), (14., 12.)]);
            polyline(&[(12., 19.), (20., 19.)]);
        }
        IconName::CircleDot => {
            p.circle_stroke(at(12., 12.), 10. * s, stroke);
            p.circle_filled(at(12., 12.), s, color);
        }
        IconName::Workflow | IconName::Boxes => {
            p.rect_stroke(
                Rect::from_min_max(at(3., 3.), at(11., 11.)),
                2. * s,
                stroke,
                StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_min_max(at(13., 13.), at(21., 21.)),
                2. * s,
                stroke,
                StrokeKind::Inside,
            );
            polyline(&[(7., 11.), (7., 17.), (13., 17.)]);
        }
        IconName::ScrollText => {
            polyline(&[
                (4., 3.),
                (19., 3.),
                (19., 17.),
                (22., 17.),
                (22., 21.),
                (6., 21.),
                (6., 3.),
                (2., 3.),
                (2., 7.),
                (6., 7.),
            ]);
            polyline(&[(10., 8.), (15., 8.)]);
            polyline(&[(10., 12.), (15., 12.)]);
        }
        IconName::MessageSquare => polyline(&[
            (2., 3.),
            (22., 3.),
            (22., 18.),
            (7., 18.),
            (2., 22.),
            (2., 3.),
        ]),
        IconName::Zap => polyline(&[
            (14., 2.),
            (4., 13.),
            (11., 13.),
            (9., 22.),
            (20., 10.),
            (13., 10.),
            (14., 2.),
        ]),
        IconName::Hash => {
            polyline(&[(4., 9.), (20., 9.)]);
            polyline(&[(4., 15.), (20., 15.)]);
            polyline(&[(10., 3.), (8., 21.)]);
            polyline(&[(16., 3.), (14., 21.)]);
        }
        IconName::Pi => {
            polyline(&[(4., 7.), (5., 4.), (20., 4.)]);
            polyline(&[(9., 4.), (9., 20.)]);
            polyline(&[(15., 4.), (15., 18.), (18., 20.)]);
        }
        IconName::Tags => {
            polyline(&[
                (6., 2.),
                (13., 2.),
                (22., 11.),
                (15., 18.),
                (6., 9.),
                (6., 2.),
            ]);
            polyline(&[(2., 7.), (2., 13.), (10., 21.), (13., 21.)]);
            p.circle_filled(at(10., 6.), s, color);
        }
        IconName::Cpu => {
            p.rect_stroke(
                Rect::from_min_max(at(4., 4.), at(20., 20.)),
                s,
                stroke,
                StrokeKind::Inside,
            );
            p.rect_stroke(
                Rect::from_min_max(at(8., 8.), at(16., 16.)),
                s,
                stroke,
                StrokeKind::Inside,
            );
            for v in [7., 12., 17.] {
                polyline(&[(v, 2.), (v, 4.)]);
                polyline(&[(v, 20.), (v, 22.)]);
                polyline(&[(2., v), (4., v)]);
                polyline(&[(20., v), (22., v)]);
            }
        }
        IconName::Component => {
            for (x, y) in [(12., 5.), (5., 12.), (19., 12.), (12., 19.)] {
                polyline(&[
                    (x, y - 3.),
                    (x + 3., y),
                    (x, y + 3.),
                    (x - 3., y),
                    (x, y - 3.),
                ]);
            }
        }
        IconName::Brackets => {
            polyline(&[(8., 3.), (4., 3.), (4., 21.), (8., 21.)]);
            polyline(&[(16., 3.), (20., 3.), (20., 21.), (16., 21.)]);
        }
        IconName::Terminal => {
            polyline(&[(4., 5.), (10., 11.), (4., 17.)]);
            polyline(&[(12., 19.), (20., 19.)]);
        }
        IconName::GitFork => {
            for (x, y) in [(6., 6.), (18., 6.), (12., 18.)] {
                p.circle_stroke(at(x, y), 3. * s, stroke);
            }
            polyline(&[(6., 9.), (6., 12.), (18., 12.), (18., 9.)]);
            polyline(&[(12., 12.), (12., 15.)]);
        }
        IconName::Layers => {
            polyline(&[(2., 6.), (12., 2.), (22., 6.), (12., 11.), (2., 6.)]);
            for y in [12., 17.] {
                polyline(&[(2., y), (12., y + 5.), (22., y)]);
            }
        }
        IconName::Library => {
            for (x, y) in [(4., 4.), (8., 8.), (12., 6.)] {
                polyline(&[(x, y), (x, 20.)]);
            }
            polyline(&[(16., 6.), (20., 20.)]);
        }
        IconName::Server => {
            for y in [2., 14.] {
                p.rect_stroke(
                    Rect::from_min_max(at(2., y), at(22., y + 8.)),
                    s,
                    stroke,
                    StrokeKind::Inside,
                );
                p.circle_filled(at(6., y + 4.), s, color);
            }
        }
        IconName::SquareFunction => {
            p.rect_stroke(
                Rect::from_min_max(at(3., 3.), at(21., 21.)),
                2. * s,
                stroke,
                StrokeKind::Inside,
            );
            polyline(&[(9., 17.), (11., 16.), (12., 9.), (13., 7.), (16., 7.)]);
            polyline(&[(9., 11.), (15., 11.)]);
        }
        IconName::Cable => {
            polyline(&[
                (5., 10.),
                (5., 19.),
                (8., 21.),
                (12., 18.),
                (12., 6.),
                (16., 3.),
                (19., 6.),
                (19., 14.),
            ]);
            for (x, y) in [(2., 5.), (16., 14.)] {
                p.rect_stroke(
                    Rect::from_min_max(at(x, y), at(x + 6., y + 5.)),
                    s,
                    stroke,
                    StrokeKind::Inside,
                );
            }
        }
        IconName::LogIn | IconName::LogOut => {
            let x = if name == IconName::LogIn { 15. } else { 21. };
            polyline(&[(x - 12., 12.), (x, 12.), (x - 5., 7.)]);
            polyline(&[(x, 12.), (x - 5., 17.)]);
            let x = if name == IconName::LogIn { 21. } else { 3. };
            polyline(&[(x, 3.), (x, 21.)]);
        }
        IconName::ArrowLeftRight => {
            polyline(&[(8., 3.), (4., 7.), (20., 7.)]);
            polyline(&[(4., 7.), (8., 11.)]);
            polyline(&[(16., 13.), (20., 17.), (4., 17.)]);
            polyline(&[(20., 17.), (16., 21.)]);
        }
        IconName::Package => {
            icon(p, IconName::Box, r, color);
            polyline(&[(7., 5.), (16., 10.), (16., 14.)]);
        }
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
