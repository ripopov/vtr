//! Window chrome belongs to the demo, not the embeddable table.
use eframe::egui::{
    self, Align2, Color32, FontId, Rect, Sense, Stroke, ViewportCommand, pos2, vec2,
};

const HEIGHT: f32 = 40.0;
const CONTROL_HEIGHT: f32 = 26.0;
const EDGE: f32 = 5.0;
const CORNER: f32 = 14.0;

#[cfg(target_os = "linux")]
mod native;
#[cfg(target_os = "linux")]
pub use native::WindowFrame;

/// Radius in UI points. Maximized/fullscreen windows meet the screen edges.
pub fn corner_radius(ctx: &egui::Context) -> u8 {
    if cfg!(target_os = "linux")
        && !ctx.input(|i| {
            i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
        })
    {
        12
    } else {
        0
    }
}

pub fn outline(root: &egui::Ui) {
    let radius = corner_radius(root.ctx());
    if radius != 0 {
        root.painter().rect_stroke(
            root.max_rect().shrink(0.5),
            radius,
            root.visuals().widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
}

pub fn header(root: &mut egui::Ui) {
    let radius = corner_radius(root.ctx());
    egui::Panel::top("window-header")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(root.visuals().window_fill)
                .corner_radius(egui::CornerRadius {
                    nw: radius,
                    ne: radius,
                    sw: 0,
                    se: 0,
                }),
        )
        .show(root, |ui| {
            let rect = ui.max_rect();
            let controls_left = rect.right() - 328.0;
            // Register the drag target before the controls. Buttons keep click
            // priority, while a drag can begin anywhere in the header, including
            // branding, controls and the spaces around them. Edge resize targets
            // are registered later and retain priority at the window boundary.
            let drag = ui.interact(rect, egui::Id::new("window-drag"), Sense::click_and_drag());
            if drag.double_clicked() {
                toggle_maximized(ui.ctx());
            }
            // Track the gesture from its press origin, even if it leaves the
            // header or a child control changes its response between frames.
            let gesture = egui::Id::new("window-drag-requested");
            let request_drag = ui.input(|i| {
                let Some(origin) = i.pointer.press_origin() else {
                    return false;
                };
                let Some(position) = i.pointer.interact_pos() else {
                    return false;
                };
                let resizable = !i.viewport().maximized.unwrap_or(false)
                    && !i.viewport().fullscreen.unwrap_or(false);
                let on_resize_edge = resizable
                    && (origin.y < rect.top() + EDGE
                        || origin.x < rect.left() + EDGE
                        || origin.x > rect.right() - EDGE
                        || (origin.y < rect.top() + CORNER
                            && (origin.x < rect.left() + CORNER
                                || origin.x > rect.right() - CORNER)));
                i.pointer.primary_down()
                    && rect.contains(origin)
                    && !on_resize_edge
                    && origin.distance(position) > 6.0
            });
            let reset_drag = ui.input(|i| i.pointer.primary_pressed() || !i.pointer.primary_down());
            let send = ui.ctx().data_mut(|data| {
                if reset_drag {
                    data.remove::<bool>(gesture);
                }
                if request_drag && !data.get_temp::<bool>(gesture).unwrap_or(false) {
                    data.insert_temp(gesture, true);
                    true
                } else {
                    false
                }
            });
            if send {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }

            let p = ui.painter();
            let accent = ui.visuals().selection.stroke.color;
            let mark =
                Rect::from_center_size(pos2(rect.left() + 29.0, rect.center().y), vec2(24.0, 24.0));
            p.rect_filled(mark, 7.0, ui.visuals().selection.bg_fill);
            for (offset, height) in [(6.0, 8.0), (12.0, 15.0), (18.0, 11.0)] {
                p.line_segment(
                    [
                        pos2(mark.left() + offset, mark.bottom() - 6.0),
                        pos2(mark.left() + offset, mark.bottom() - 6.0 - height),
                    ],
                    Stroke::new(2.0, accent),
                );
            }
            p.text(
                pos2(rect.left() + 48.0, rect.center().y),
                Align2::LEFT_CENTER,
                "Atlas",
                FontId::proportional(18.0),
                ui.visuals().text_color(),
            );
            p.text(
                pos2(rect.left() + 100.0, rect.center().y + 1.0),
                Align2::LEFT_CENTER,
                "DATA EXPLORER",
                FontId::proportional(10.0),
                ui.visuals().weak_text_color(),
            );

            let controls = Rect::from_min_max(
                pos2(controls_left, rect.top() + 7.0),
                pos2(rect.right() - 12.0, rect.bottom() - 7.0),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(controls)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.spacing_mut().interact_size.y = CONTROL_HEIGHT;
                    ui.spacing_mut().button_padding = vec2(8.0, 4.0);
                    crate::theme::selector(ui);
                    ui.add_space(12.0);
                    window_button(ui, WindowButton::Minimize);
                    window_button(ui, WindowButton::Maximize);
                    window_button(ui, WindowButton::Close);
                },
            );
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
        });
}

fn toggle_maximized(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

#[derive(Clone, Copy)]
enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

fn window_button(ui: &mut egui::Ui, button: WindowButton) {
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let label = match button {
        WindowButton::Minimize => "Minimize",
        WindowButton::Maximize if maximized => "Restore",
        WindowButton::Maximize => "Maximize",
        WindowButton::Close => "Close window",
    };
    let (rect, _) = ui.allocate_exact_size(vec2(34.0, CONTROL_HEIGHT), Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::new(("window-control", label)),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let visuals = ui.style().interact(&response);
    let hovered = response.hovered() || response.has_focus();
    let close_hover = matches!(button, WindowButton::Close) && hovered;
    if hovered || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            6.0,
            if close_hover {
                Color32::from_rgb(196, 48, 64)
            } else {
                visuals.weak_bg_fill
            },
        );
    }
    let stroke = Stroke::new(
        1.3,
        if close_hover {
            Color32::WHITE
        } else {
            ui.visuals().text_color()
        },
    );
    let icon = Rect::from_center_size(rect.center(), vec2(10.0, 10.0));
    let p = ui.painter();
    match button {
        WindowButton::Minimize => {
            p.hline(icon.x_range(), icon.center().y, stroke);
        }
        WindowButton::Maximize => {
            if maximized {
                let back = icon.translate(vec2(2.0, -2.0));
                p.line_segment([back.left_top(), back.right_top()], stroke);
                p.line_segment([back.right_top(), back.right_bottom()], stroke);
            }
            p.rect_stroke(icon, 0.0, stroke, egui::StrokeKind::Inside);
        }
        WindowButton::Close => {
            p.line_segment([icon.left_top(), icon.right_bottom()], stroke);
            p.line_segment([icon.right_top(), icon.left_bottom()], stroke);
        }
    }
    if response.clicked() {
        match button {
            WindowButton::Minimize => ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true)),
            WindowButton::Maximize => toggle_maximized(ui.ctx()),
            WindowButton::Close => ui.ctx().send_viewport_cmd(ViewportCommand::Close),
        }
    }
    response.on_hover_text(label);
}

/// Explicit resize zones keep borderless windows resizable on X11 and Wayland.
pub fn resize_edges(root: &mut egui::Ui) {
    if root.input(|i| {
        i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
    }) {
        return;
    }
    let rect = root.max_rect();
    let (left, right, top, bottom) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    use egui::{CursorIcon as C, ResizeDirection as D};
    let zones = [
        (
            D::North,
            C::ResizeNorth,
            Rect::from_min_max(pos2(left + CORNER, top), pos2(right - CORNER, top + EDGE)),
        ),
        (
            D::South,
            C::ResizeSouth,
            Rect::from_min_max(
                pos2(left + CORNER, bottom - EDGE),
                pos2(right - CORNER, bottom),
            ),
        ),
        (
            D::West,
            C::ResizeWest,
            Rect::from_min_max(pos2(left, top + CORNER), pos2(left + EDGE, bottom - CORNER)),
        ),
        (
            D::East,
            C::ResizeEast,
            Rect::from_min_max(
                pos2(right - EDGE, top + CORNER),
                pos2(right, bottom - CORNER),
            ),
        ),
        (
            D::NorthWest,
            C::ResizeNorthWest,
            Rect::from_min_size(rect.left_top(), vec2(CORNER, CORNER)),
        ),
        (
            D::NorthEast,
            C::ResizeNorthEast,
            Rect::from_min_size(pos2(right - CORNER, top), vec2(CORNER, CORNER)),
        ),
        (
            D::SouthWest,
            C::ResizeSouthWest,
            Rect::from_min_size(pos2(left, bottom - CORNER), vec2(CORNER, CORNER)),
        ),
        (
            D::SouthEast,
            C::ResizeSouthEast,
            Rect::from_min_size(pos2(right - CORNER, bottom - CORNER), vec2(CORNER, CORNER)),
        ),
    ];
    for (index, (direction, cursor, rect)) in zones.into_iter().enumerate() {
        let response = root
            .interact(rect, egui::Id::new(("window-resize", index)), Sense::drag())
            .on_hover_cursor(cursor);
        if response.drag_started() {
            root.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        maximized: bool,
        events: Vec<egui::Event>,
    ) -> Vec<ViewportCommand> {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(820.0, 560.0))),
            events,
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .maximized = Some(maximized);
        let mut output = ctx.run_ui(input, |ui| {
            header(ui);
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("Content");
            });
            resize_edges(ui);
        });
        output.textures_delta.clear();
        output
            .viewport_output
            .remove(&egui::ViewportId::ROOT)
            .unwrap()
            .commands
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    #[test]
    fn window_controls_send_only_their_own_command() {
        for (label, maximized, expected) in [
            ("Minimize", false, ViewportCommand::Minimized(true)),
            ("Maximize", false, ViewportCommand::Maximized(true)),
            ("Restore", true, ViewportCommand::Maximized(false)),
            ("Close window", false, ViewportCommand::Close),
        ] {
            let ctx = egui::Context::default();
            frame(&ctx, maximized, vec![]);
            frame(&ctx, maximized, vec![]);
            let rect = ctx
                .read_response(egui::Id::new(("window-control", label)))
                .unwrap()
                .rect;
            assert!(rect.left() > 500.0 && rect.right() < 820.0);
            assert!(rect.top() >= EDGE && rect.bottom() <= HEIGHT);
            let mut commands = frame(&ctx, maximized, pointer(rect.center(), true));
            commands.extend(frame(&ctx, maximized, pointer(rect.center(), false)));
            assert_eq!(commands, vec![expected], "{label}");
        }
    }

    #[test]
    fn title_drags_and_double_click_toggles_maximize() {
        for maximized in [false, true] {
            let ctx = egui::Context::default();
            frame(&ctx, maximized, vec![]);
            frame(&ctx, maximized, vec![]);
            let pos = pos2(300.0, 28.0);
            let mut commands = Vec::new();
            for _ in 0..2 {
                commands.extend(frame(&ctx, maximized, pointer(pos, true)));
                commands.extend(frame(&ctx, maximized, pointer(pos, false)));
            }
            assert_eq!(commands, vec![ViewportCommand::Maximized(!maximized)]);
            // Use a fresh context for the drag so click history cannot affect it.
            let ctx = egui::Context::default();
            frame(&ctx, maximized, vec![]);
            frame(&ctx, maximized, pointer(pos, true));
            let commands = frame(
                &ctx,
                maximized,
                vec![egui::Event::PointerMoved(pos + vec2(30.0, 0.0))],
            );
            assert_eq!(commands, vec![ViewportCommand::StartDrag]);
        }
    }

    #[test]
    fn whole_header_drags_without_activating_buttons() {
        for maximized in [false, true] {
            for pos in [
                pos2(29.0, 20.0),  // logo
                pos2(65.0, 20.0),  // title
                pos2(160.0, 20.0), // subtitle
                pos2(400.0, 20.0), // empty title area
                pos2(600.0, 20.0), // theme selector
                pos2(650.0, 20.0), // theme selector
                pos2(690.0, 20.0), // gap beside theme controls
                pos2(715.0, 20.0), // minimize
                pos2(750.0, 20.0), // maximize
                pos2(790.0, 20.0), // close
                pos2(750.0, 6.0),  // above window controls
                pos2(750.0, 36.0), // below window controls
            ] {
                let ctx = egui::Context::default();
                frame(&ctx, maximized, vec![]);
                frame(&ctx, maximized, vec![]);
                let theme = ctx.theme();
                let mut commands = frame(&ctx, maximized, pointer(pos, true));
                let moved = pos + vec2(18.0, 45.0);
                commands.extend(frame(
                    &ctx,
                    maximized,
                    vec![egui::Event::PointerMoved(moved)],
                ));
                commands.extend(frame(
                    &ctx,
                    maximized,
                    vec![egui::Event::PointerMoved(moved + vec2(5.0, 5.0))],
                ));
                commands.extend(frame(&ctx, maximized, pointer(moved, false)));
                assert_eq!(
                    commands,
                    vec![ViewportCommand::StartDrag],
                    "drag at {pos:?}, maximized={maximized}"
                );
                assert_eq!(ctx.theme(), theme, "drag must not select a theme");
            }
        }
    }

    #[test]
    fn all_edges_resize_and_maximized_windows_have_no_resize_handles() {
        for (index, direction) in [
            egui::ResizeDirection::North,
            egui::ResizeDirection::South,
            egui::ResizeDirection::West,
            egui::ResizeDirection::East,
            egui::ResizeDirection::NorthWest,
            egui::ResizeDirection::NorthEast,
            egui::ResizeDirection::SouthWest,
            egui::ResizeDirection::SouthEast,
        ]
        .into_iter()
        .enumerate()
        {
            let ctx = egui::Context::default();
            frame(&ctx, false, vec![]);
            frame(&ctx, false, vec![]);
            let rect = ctx
                .read_response(egui::Id::new(("window-resize", index)))
                .unwrap()
                .rect;
            let mut commands = frame(&ctx, false, pointer(rect.center(), true));
            commands.extend(frame(
                &ctx,
                false,
                vec![egui::Event::PointerMoved(rect.center() + vec2(20.0, 20.0))],
            ));
            assert_eq!(commands, vec![ViewportCommand::BeginResize(direction)]);
        }
        let ctx = egui::Context::default();
        frame(&ctx, true, vec![]);
        frame(&ctx, true, vec![]);
        for index in 0..8usize {
            assert!(
                ctx.read_response(egui::Id::new(("window-resize", index)))
                    .is_none()
            );
        }
    }
}
