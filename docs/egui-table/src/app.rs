use crate::storage::{self, Store};
use eframe::egui::{self, Align2, Color32, FontId, Rect, pos2, vec2};
use egui_atlas_table::{CellPainter, CellStyle, ColumnOptions, Table, TableColors, TableOptions};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};
enum Boot {
    Progress(f32),
    Ready(Arc<Store>),
    Error(String),
}
pub struct Atlas {
    #[cfg(target_os = "linux")]
    window_frame: crate::chrome::WindowFrame,
    boot: Receiver<Boot>,
    progress: f32,
    table: Option<Table>,
    painted_cells: usize,
    error: Option<String>,
}
fn options() -> TableOptions {
    let mut columns = vec![ColumnOptions::default(); 30];
    for (group, range) in storage::GROUPS {
        for c in range {
            columns[c].group = group.into();
        }
    }
    for (c, w) in [
        (0, 158.0),
        (1, 194.0),
        (2, 194.0),
        (3, 184.0),
        (4, 136.0),
        (5, 150.0),
        (6, 290.0),
        (29, 270.0),
    ] {
        columns[c].width = w;
    }
    for (c, column) in columns.iter_mut().enumerate() {
        column.style = CellStyle {
            right_aligned: [5, 16, 17, 18, 19].contains(&c),
            monospace: c == 0,
            muted: c == 0,
        };
    }
    TableOptions {
        title: "Customer directory".into(),
        description: "Explore every record. Keep only what matters.".into(),
        initial_search_column: 1,
        columns,
        cell_painter: Some(Arc::new(DemoCells)),
        colors: None,
        ..Default::default()
    }
}
struct DemoCells;
impl CellPainter for DemoCells {
    fn paint(
        &self,
        p: &egui::Painter,
        rect: Rect,
        _row: u32,
        c: usize,
        value: &str,
        _selected: bool,
    ) {
        let style = p.ctx().style_of(p.ctx().theme());
        let visuals = &style.visuals;
        let colors = TableColors::from_visuals(visuals);
        if c == 4 {
            let (fg, bg) = match (visuals.dark_mode, value) {
                (true, "Active") => (
                    Color32::from_rgb(112, 222, 176),
                    Color32::from_rgb(28, 66, 55),
                ),
                (true, "Trial") => (
                    Color32::from_rgb(157, 190, 255),
                    Color32::from_rgb(38, 56, 91),
                ),
                (true, "Paused") => (
                    Color32::from_rgb(242, 203, 120),
                    Color32::from_rgb(72, 57, 32),
                ),
                (false, "Active") => (
                    Color32::from_rgb(30, 112, 80),
                    Color32::from_rgb(231, 245, 237),
                ),
                (false, "Trial") => (
                    Color32::from_rgb(72, 104, 187),
                    Color32::from_rgb(236, 241, 253),
                ),
                (false, "Paused") => (
                    Color32::from_rgb(140, 94, 25),
                    Color32::from_rgb(252, 245, 225),
                ),
                _ => (colors.muted, visuals.faint_bg_color),
            };
            p.rect_filled(
                Rect::from_min_size(rect.min + vec2(10.0, 7.0), vec2(77.0, 21.0)),
                10.0,
                bg,
            );
            p.circle_filled(rect.min + vec2(21.0, 17.5), 2.5, fg);
            p.text(
                rect.min + vec2(29.0, 17.5),
                Align2::LEFT_CENTER,
                value,
                FontId::proportional(11.0),
                fg,
            );
        } else {
            let (align, x) = if [5, 16, 17, 18, 19].contains(&c) {
                (Align2::RIGHT_CENTER, rect.max.x - 14.0)
            } else {
                (Align2::LEFT_CENTER, rect.min.x + 13.0)
            };
            p.text(
                pos2(x, rect.center().y),
                align,
                value,
                if c == 0 {
                    FontId::monospace(12.0)
                } else {
                    FontId::proportional(13.0)
                },
                if c == 0 { colors.muted } else { colors.ink },
            );
        }
    }
}
impl Atlas {
    pub fn new(cc: &eframe::CreationContext<'_>, path: PathBuf) -> Self {
        let app = Self::with_context(cc.egui_ctx.clone(), path);
        #[cfg(target_os = "linux")]
        let app = Self {
            window_frame: crate::chrome::WindowFrame::new(cc),
            ..app
        };
        app
    }
    fn with_context(ctx: egui::Context, path: PathBuf) -> Self {
        crate::theme::initialize(&ctx);
        let (tx, boot) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<Store> {
                if !path.exists() {
                    storage::generate(&path, 10_000_000, |done, total| {
                        let _ = tx.send(Boot::Progress(done as f32 / total as f32));
                        ctx.request_repaint();
                    })?;
                }
                Store::open(&path)
            })();
            let _ = tx.send(match result {
                Ok(store) => Boot::Ready(Arc::new(store)),
                Err(e) => Boot::Error(format!("{e:#}")),
            });
            ctx.request_repaint();
        });
        Self {
            #[cfg(target_os = "linux")]
            window_frame: Default::default(),
            boot,
            progress: 0.0,
            table: None,
            painted_cells: 0,
            error: None,
        }
    }
    fn draw(&mut self, root: &mut egui::Ui) {
        while let Ok(event) = self.boot.try_recv() {
            match event {
                Boot::Progress(p) => self.progress = p,
                Boot::Error(e) => self.error = Some(e),
                Boot::Ready(store) => {
                    match Table::new("directory", store, root.ctx().clone(), options()) {
                        Ok(table) => self.table = Some(table),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            }
        }
        crate::chrome::header(root);
        let radius = crate::chrome::corner_radius(root.ctx());
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(root.visuals().panel_fill)
                    .corner_radius(egui::CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: radius,
                        se: radius,
                    })
                    .inner_margin(24),
            )
            .show(root, |ui| {
                if let Some(table) = &mut self.table {
                    self.painted_cells = table.show(ui).painted_cells;
                } else if let Some(e) = &self.error {
                    ui.heading("Unable to load this view");
                    ui.label(e);
                } else {
                    ui.spinner();
                    ui.label("Preparing your directory");
                    ui.add(egui::ProgressBar::new(self.progress).show_percentage());
                }
            });
        crate::chrome::resize_edges(root);
        crate::chrome::outline(root);
    }
}
impl eframe::App for Atlas {
    fn ui(&mut self, root: &mut egui::Ui, _: &mut eframe::Frame) {
        #[cfg(target_os = "linux")]
        self.window_frame.update(root.ctx());
        self.draw(root);
    }

    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }
}
fn frame(app: &mut Atlas, ctx: &egui::Context) -> egui::FullOutput {
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1440.0, 900.0))),
            ..Default::default()
        },
        |ui| app.draw(ui),
    );
    out.textures_delta.clear();
    out
}
fn ready(app: &mut Atlas, ctx: &egui::Context) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        frame(app, ctx);
        if let Some(e) = &app.error {
            anyhow::bail!("{e}");
        }
        if app.table.as_ref().is_some_and(Table::viewport_ready) {
            return Ok(());
        }
        anyhow::ensure!(Instant::now() < deadline, "Viewport did not load");
        thread::sleep(Duration::from_millis(1));
    }
}
pub fn benchmark_ui(path: PathBuf) -> anyhow::Result<()> {
    anyhow::ensure!(path.exists(), "Generate the dataset first");
    let ctx = egui::Context::default();
    let mut app = Atlas::with_context(ctx.clone(), path);
    ready(&mut app, &ctx)?;
    let rows = app.table.as_ref().unwrap().row_view().len();
    for (name, position) in [("Top", 0), ("Middle", rows / 2), ("Bottom", rows)] {
        app.table.as_mut().unwrap().scroll_to_row(position);
        ready(&mut app, &ctx)?;
        let mut times = Vec::new();
        for _ in 0..300 {
            let start = Instant::now();
            let out = frame(&mut app, &ctx);
            let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{name}: {} painted cells, frame+tessellation median {:.3} ms / p95 {:.3} ms",
            app.painted_cells, times[150], times[285]
        );
    }
    println!(
        "Headless 1440×900, 6 visible columns, warm cells/fonts. Excludes GPU submission/presentation."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_selects_every_palette_and_badges_have_readable_contrast() {
        let ctx = egui::Context::default();
        crate::theme::initialize(&ctx);
        let (_, boot) = mpsc::channel();
        let mut app = Atlas {
            #[cfg(target_os = "linux")]
            window_frame: Default::default(),
            boot,
            progress: 0.0,
            table: None,
            painted_cells: 0,
            error: None,
        };
        for theme in crate::theme::Theme::ALL.into_iter().cycle().skip(1).take(8) {
            let previous = crate::theme::current(&ctx);
            click_theme_text(&mut app, &ctx, previous.name(), false);
            click_theme_text(&mut app, &ctx, theme.name(), true);
            assert_eq!(crate::theme::current(&ctx), theme);
            assert_eq!(ctx.theme(), theme.mode());
            let style = ctx.style_of(ctx.theme());
            let colors = TableColors::from_visuals(&style.visuals);
            for (foreground, backgrounds) in [
                (
                    colors.ink,
                    vec![colors.surface, colors.header, colors.hover, colors.selected],
                ),
                (colors.muted, vec![colors.surface, colors.header]),
                (colors.accent, vec![colors.surface, colors.header]),
            ] {
                for background in backgrounds {
                    let a = luminance(background);
                    let b = luminance(foreground);
                    assert!(
                        (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                        "{theme:?} table text needs readable contrast"
                    );
                }
            }
            for status in ["Active", "Trial", "Paused", "Other"] {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    DemoCells.paint(
                        ui.painter(),
                        Rect::from_min_size(pos2(0.0, 0.0), vec2(136.0, 35.0)),
                        0,
                        4,
                        status,
                        false,
                    );
                });
                output.textures_delta.clear();
                let background = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Rect(rect) => Some(rect.fill),
                        _ => None,
                    })
                    .unwrap();
                let foreground = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) => Some(text.fallback_color),
                        _ => None,
                    })
                    .unwrap();
                let a = luminance(background);
                let b = luminance(foreground);
                assert!(
                    (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                    "{theme:?} {status} badge needs readable contrast"
                );
            }
        }
    }

    fn click_theme_text(app: &mut Atlas, ctx: &egui::Context, label: &str, popup: bool) {
        frame(app, ctx);
        let output = frame(app, ctx);
        let position = geometry(&output)
            .into_iter()
            .find(|(text, rect)| text == label && (rect.top() > 40.0) == popup)
            .unwrap_or_else(|| panic!("theme control {label} is visible, popup={popup}"))
            .1
            .center();
        for pressed in [true, false] {
            let output = hover_frame(
                app,
                ctx,
                vec2(1440.0, 900.0),
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            assert!(
                output.viewport_output.values().all(|v| v
                    .commands
                    .iter()
                    .all(|command| matches!(command, egui::ViewportCommand::SetTheme(_)))),
                "theme selection must not trigger window actions"
            );
        }
    }

    // Sweep real pointer input over painted text and checkbox boxes. Compare
    // geometry over multiple frames because egui styles use the previous response.
    #[test]
    fn hover_keeps_app_geometry_stable() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!("atlas-hover-{}.pb.zst", std::process::id()));
        storage::generate(&path, 100, |_, _| {})?;
        let ctx = egui::Context::default();
        let mut app = Atlas::with_context(ctx.clone(), path.clone());
        ready(&mut app, &ctx)?;
        std::fs::remove_file(path)?;
        for theme in crate::theme::Theme::ALL {
            theme.apply(&ctx);
            for size in [vec2(1440.0, 900.0), vec2(820.0, 560.0)] {
                for panel in ["Columns  6 / 30", "Help & info", "Close"] {
                    let output = hover_frame(&mut app, &ctx, size, vec![]);
                    let target = geometry(&output)
                        .into_iter()
                        .find(|(text, _)| text == panel)
                        .unwrap()
                        .1
                        .center();
                    for pressed in [true, false] {
                        hover_frame(
                            &mut app,
                            &ctx,
                            size,
                            vec![
                                egui::Event::PointerMoved(target),
                                egui::Event::PointerButton {
                                    pos: target,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                        );
                    }
                    for _ in 0..3 {
                        hover_frame(&mut app, &ctx, size, vec![egui::Event::PointerGone]);
                    }
                    let baseline = geometry(&hover_frame(&mut app, &ctx, size, vec![]));
                    for (label, rect) in &baseline {
                        for _ in 0..3 {
                            let output = hover_frame(
                                &mut app,
                                &ctx,
                                size,
                                vec![egui::Event::PointerMoved(rect.center())],
                            );
                            assert_eq!(
                                geometry(&output),
                                baseline,
                                "hovering {label:?} in {panel}, {theme:?}, {size:?}"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn hover_frame(
        app: &mut Atlas,
        ctx: &egui::Context,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| app.draw(ui),
        );
        output.textures_delta.clear();
        output
    }

    fn geometry(output: &egui::FullOutput) -> Vec<(String, Rect)> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if !text.galley.job.text.starts_with("UI build:") =>
                {
                    Some((
                        text.galley.job.text.clone(),
                        Rect::from_min_size(text.pos, text.galley.size()),
                    ))
                }
                egui::epaint::Shape::Rect(rect)
                    if rect.rect.width() == rect.rect.height()
                        && (10.0..=20.0).contains(&rect.rect.width()) =>
                {
                    Some(("checkbox".into(), rect.rect))
                }
                _ => None,
            })
            .collect()
    }

    fn luminance(color: Color32) -> f32 {
        let linear = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }
}
