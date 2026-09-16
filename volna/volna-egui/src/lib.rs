//! Volna's egui frontend: a native desktop viewer that is a thin adapter over
//! [`volna_core::App`]. It owns the window, paints the core's display list,
//! hosts egui widgets for the chrome, routes input as commands and runs the
//! core's loads on threads. Nothing here decides what the viewer shows.

pub mod headless;
pub mod paint;

use std::sync::Arc;
use std::sync::mpsc;

use egui::{
    Align, Align2, Area, CentralPanel, Color32, Context, FontData, FontDefinitions, FontFamily,
    Frame, Id, Key, Layout, Margin, Order, Panel, Pos2, Rect, RichText, ScrollArea, Sense, Stroke,
    TextEdit, Ui, Vec2, Visuals,
};
use volna_core::app::{Action, Command, Event};
use volna_core::document::TraceState;
use volna_core::geometry::{Modifiers, MouseButton, Rect as CRect};
use volna_core::icons::FONTS;
use volna_core::session::LoadResult;
use volna_core::sidebar::Key as ListKey;
use volna_core::sidebar::scopes::scope_icon;
use volna_core::sidebar::variables::{direction_label, shape_icon};
use volna_core::wave::PointerEvent;
use volna_core::{App, Instant, Scene, Theme};

use paint::{
    EguiMeasure, MONO_FAMILY, UI_FAMILY, UI_SEMIBOLD_FAMILY, c32, cursor_icon, font_id, paint_scene,
};

/// The bundled faces as egui font families, with egui's defaults as fallbacks.
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let monospace = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let mut regular = Vec::new();
    let mut semibold = Vec::new();
    let mut mono = Vec::new();
    for face in FONTS {
        let name = format!("{}-{}", face.family, face.weight);
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_static(face.bytes)));
        match (face.family, face.weight) {
            ("IBM Plex Sans", 600) => semibold.push(name),
            ("IBM Plex Sans", _) => regular.push(name),
            _ => mono.push(name),
        }
    }
    let with = |mut primary: Vec<String>, fallback: &[String]| {
        primary.extend(fallback.iter().cloned());
        primary
    };
    fonts.families.insert(
        FontFamily::Name(Arc::from(UI_FAMILY)),
        with(regular.clone(), &proportional),
    );
    fonts.families.insert(
        FontFamily::Name(Arc::from(UI_SEMIBOLD_FAMILY)),
        with(semibold, &proportional),
    );
    fonts.families.insert(
        FontFamily::Name(Arc::from(MONO_FAMILY)),
        with(mono.clone(), &monospace),
    );
    fonts
        .families
        .insert(FontFamily::Proportional, with(regular, &proportional));
    fonts
        .families
        .insert(FontFamily::Monospace, with(mono, &monospace));
    fonts
}

struct Ids {
    waves: Id,
    scopes: Id,
    variables: Id,
    filter: Id,
    format_menu: Id,
}

/// Which panel keyboard input goes to this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyTarget {
    Waves,
    Scopes,
    Variables,
    Other,
}

pub struct VolnaApp {
    pub app: App,
    pub core_theme: Theme,
    theme: volna_core::theme::Theme<Color32>,
    scene: Scene,
    tx: mpsc::Sender<LoadResult>,
    rx: mpsc::Receiver<LoadResult>,
    /// The filter box's text; the core owns the filter, this mirrors it.
    filter: String,
    focus_filter: bool,
    reveal_scope: Option<usize>,
    reveal_var: Option<usize>,
    pointer_inside: bool,
    ids: Ids,
}

impl VolnaApp {
    pub fn new(ctx: &Context) -> Self {
        let core_theme = Theme::one_dark();
        ctx.set_fonts(font_definitions());
        let app = VolnaApp {
            app: App::new(),
            theme: core_theme.map(c32),
            core_theme,
            scene: Scene::default(),
            tx: mpsc::channel().0,
            rx: mpsc::channel().1,
            filter: String::new(),
            focus_filter: false,
            reveal_scope: None,
            reveal_var: None,
            pointer_inside: false,
            ids: Ids {
                waves: Id::new("volna-waves"),
                scopes: Id::new("volna-scopes"),
                variables: Id::new("volna-variables"),
                filter: Id::new("volna-filter"),
                format_menu: Id::new("volna-format-menu"),
            },
        };
        let (tx, rx) = mpsc::channel();
        let mut app = VolnaApp { tx, rx, ..app };
        app.apply_visuals(ctx);
        app
    }

    fn apply_visuals(&mut self, ctx: &Context) {
        let t = &self.theme;
        let mut v = Visuals::dark();
        v.panel_fill = t.panel.bg;
        v.window_fill = t.elevated.bg;
        v.extreme_bg_color = t.input.bg;
        v.faint_bg_color = t.hover.bg;
        v.override_text_color = Some(t.panel.text);
        v.selection.bg_fill = t.selection.bg;
        v.selection.stroke = Stroke::new(1.0, t.border_focused);
        v.window_stroke = Stroke::new(1.0, t.border);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border_variant);
        v.widgets.noninteractive.fg_stroke.color = t.panel.text;
        v.widgets.inactive.bg_fill = t.badge.bg;
        v.widgets.inactive.weak_bg_fill = t.badge.bg;
        v.widgets.inactive.fg_stroke.color = t.panel.text;
        v.widgets.hovered.bg_fill = t.hover.bg;
        v.widgets.hovered.weak_bg_fill = t.hover.bg;
        v.widgets.hovered.fg_stroke.color = t.hover.text;
        v.widgets.active.bg_fill = t.selection.bg;
        v.widgets.active.weak_bg_fill = t.selection.bg;
        v.widgets.open.bg_fill = t.selection.bg;
        v.widgets.open.weak_bg_fill = t.selection.bg;
        let ui = font_id(volna_core::FontRole::Ui, self.core_theme.ui_size);
        let small = font_id(volna_core::FontRole::Ui, self.core_theme.ui_size_small);
        let mono = font_id(volna_core::FontRole::Mono, self.core_theme.mono_size);
        ctx.all_styles_mut(|style| {
            style.visuals = v.clone();
            style.text_styles.insert(egui::TextStyle::Body, ui.clone());
            style
                .text_styles
                .insert(egui::TextStyle::Button, ui.clone());
            style
                .text_styles
                .insert(egui::TextStyle::Small, small.clone());
            style
                .text_styles
                .insert(egui::TextStyle::Monospace, mono.clone());
        });
    }

    // -- the core loop -------------------------------------------------------------

    fn dispatch(&mut self, command: Command) {
        self.app.handle(command);
    }

    fn drain_results(&mut self) {
        while let Ok(result) = self.rx.try_recv() {
            self.app.deliver(result);
        }
    }

    /// Act on the core's events and run its loads on threads.
    fn after(&mut self, ctx: &Context) {
        for event in self.app.take_events() {
            match event {
                Event::LoadWorkspace { .. }
                | Event::PersistWorkspace { .. }
                | Event::OpenWorkspaceDialog
                | Event::SaveWorkspaceDialog
                | Event::TraceClosed { .. }
                | Event::Quit => {}
                Event::Changed | Event::LayoutChanged { .. } => ctx.request_repaint(),
                Event::Notice(text) => log::warn!("{text}"),
                Event::OpenFileDialog => self.open_file_dialog(),
                Event::RevealScopeRow(ix) => self.reveal_scope = Some(ix),
                Event::RevealVarRow(ix) => self.reveal_var = Some(ix),
                Event::FocusFilter => self.focus_filter = true,
            }
        }
        if self.filter != self.app.variables.filter {
            self.filter = self.app.variables.filter.clone();
        }
        for request in self.app.take_requests() {
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let result = request.perform();
                tx.send(result).ok();
                ctx.request_repaint();
            });
        }
    }

    fn open_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Waveform traces", &["vtr", "fst"])
            .pick_file()
        {
            self.app.open_path(path);
        }
    }

    /// One frame of the whole viewer in the root `Ui`. Public so headless
    /// tests can drive it.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        self.drain_results();
        let dropped: Vec<_> = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            self.app.open_path(file.path().to_path_buf());
        }
        if self.app.tick(Instant::now()) {
            ctx.request_repaint();
        }
        self.keyboard(ctx);
        self.toolbar(ui);
        self.statusbar(ui);
        if self.app.sidebar_visible {
            self.sidebar(ui);
        }
        self.center(ui);
        self.after(ctx);
    }

    // -- keyboard --------------------------------------------------------------------

    fn key_target(&self, ctx: &Context) -> KeyTarget {
        match ctx.memory(|m| m.focused()) {
            Some(id) if id == self.ids.scopes => KeyTarget::Scopes,
            Some(id) if id == self.ids.variables => KeyTarget::Variables,
            Some(id) if id == self.ids.waves => KeyTarget::Waves,
            Some(_) => KeyTarget::Other,
            None => KeyTarget::Waves,
        }
    }

    fn keyboard(&mut self, ctx: &Context) {
        let target = self.key_target(ctx);
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.command {
                        let global = match key {
                            Key::O => Some(Command::RequestOpenDialog),
                            Key::B => Some(Command::ToggleSidebar),
                            Key::W => Some(Command::CloseTrace),
                            _ => None,
                        };
                        if let Some(c) = global {
                            self.dispatch(c);
                            continue;
                        }
                    }
                    let mods = to_modifiers(modifiers);
                    match target {
                        KeyTarget::Waves => {
                            if let Some(a) = wave_action(key, modifiers) {
                                self.dispatch(Command::Action(a));
                            }
                        }
                        KeyTarget::Scopes => {
                            if let Some(k) = list_key(key) {
                                self.dispatch(Command::ScopesKey(k));
                            }
                        }
                        KeyTarget::Variables => {
                            if let Some(k) = list_key(key) {
                                self.dispatch(Command::VariablesKey(k, mods));
                            }
                        }
                        KeyTarget::Other => {}
                    }
                }
                egui::Event::Text(text) if target == KeyTarget::Variables => {
                    // Typing starts filtering.
                    self.filter.push_str(&text);
                    self.focus_filter = true;
                    let text = self.filter.clone();
                    self.dispatch(Command::SetFilter(text));
                }
                _ => {}
            }
        }
    }

    // -- chrome ---------------------------------------------------------------------

    fn toolbar(&mut self, root: &mut Ui) {
        let t = self.theme;
        let name = self.app.doc.name();
        let sidebar_visible = self.app.sidebar_visible;
        let mut command = None;
        Panel::top("volna-toolbar")
            .exact_size(self.core_theme.titlebar_height)
            .frame(
                Frame::NONE
                    .fill(t.bar.bg)
                    .inner_margin(Margin::symmetric(8, 4))
                    .stroke(Stroke::new(1.0, t.border)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("Volna").strong().color(t.bar.text));
                    if let Some(name) = name {
                        ui.label(RichText::new("—").color(t.bar.text_placeholder));
                        ui.label(RichText::new(name).color(t.bar.text_muted));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("Open…")
                            .on_hover_text("Open trace (⌘O)")
                            .clicked()
                        {
                            command = Some(Command::RequestOpenDialog);
                        }
                        if ui
                            .selectable_label(sidebar_visible, "Sidebar")
                            .on_hover_text("Toggle sidebar (⌘B)")
                            .clicked()
                        {
                            command = Some(Command::ToggleSidebar);
                        }
                    });
                });
            });
        if let Some(c) = command {
            self.dispatch(c);
        }
    }

    fn statusbar(&mut self, root: &mut Ui) {
        let t = self.theme;
        let status = self.app.status();
        let mut synthetic = None;
        Panel::bottom("volna-status")
            .exact_size(self.core_theme.statusbar_height)
            .frame(
                Frame::NONE
                    .fill(t.bar.bg)
                    .inner_margin(Margin::symmetric(8, 2))
                    .stroke(Stroke::new(1.0, t.border)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    let mono = |text: &str, color: Color32| {
                        RichText::new(text)
                            .font(font_id(
                                volna_core::FontRole::Mono,
                                self.core_theme.ui_size_small,
                            ))
                            .color(color)
                    };
                    if let Some(s) = &status.time_range {
                        ui.label(mono(s, t.bar.text_muted));
                    }
                    if let Some(s) = &status.signals {
                        ui.label(mono(s, t.bar.text_placeholder));
                    }
                    if let Some(s) = &status.changes {
                        ui.label(mono(s, t.bar.text_placeholder));
                    }
                    if let Some(s) = &status.cursor {
                        ui.label(mono(&format!("◎ {s}"), t.bar.text));
                    }
                    if let Some(s) = &status.markers {
                        ui.label(mono(s, t.bar.text_placeholder));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button("Stress", |ui| {
                            for (n, label) in [
                                (10_000usize, "10 K transitions"),
                                (1_000_000, "1 M transitions"),
                                (100_000_000, "100 M transitions"),
                            ] {
                                if ui.button(label).clicked() {
                                    synthetic = Some(n);
                                    ui.close();
                                }
                            }
                        });
                        ui.label(mono(&status.frame_ms, t.bar.text_placeholder));
                        if let Some(s) = &status.px_per {
                            ui.label(mono(s, t.bar.text_placeholder));
                        }
                    });
                });
            });
        if let Some(n) = synthetic {
            self.app.open_synthetic(n);
        }
    }

    fn sidebar(&mut self, root: &mut Ui) {
        let t = self.theme;
        let width = self.app.sidebar_width;
        let fraction = self.app.scopes_fraction;
        let mut scopes_h_seen = None;
        let response = Panel::left("volna-sidebar")
            .resizable(true)
            .default_size(width)
            .size_range(160.0..=640.0)
            .frame(Frame::NONE.fill(t.panel.bg))
            .show(root, |ui| {
                let total = ui.available_height();
                let scopes_h = (fraction * total).clamp(96.0, (total - 96.0).max(96.0));
                let top = Panel::top("volna-scopes")
                    .resizable(true)
                    .default_size(scopes_h)
                    .size_range(96.0..=(total - 96.0).max(96.0))
                    .frame(Frame::NONE.fill(t.panel.bg))
                    .show(ui, |ui| self.scopes_panel(ui));
                scopes_h_seen = Some(top.response.rect.height() / total.max(1.0));
                CentralPanel::default()
                    .frame(Frame::NONE.fill(t.panel.bg))
                    .show(ui, |ui| self.variables_panel(ui));
            });
        let w = response.response.rect.width();
        if (w - width).abs() > 0.5 {
            self.dispatch(Command::SetSidebarWidth(w));
        }
        if let Some(f) = scopes_h_seen
            && (f - fraction).abs() > 0.005
        {
            self.dispatch(Command::SetScopesFraction(f));
        }
    }

    fn panel_header(&self, ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
        let t = self.theme;
        let h = self.core_theme.header_height;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, t.panel.bg);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, t.border_variant),
        );
        ui.painter().text(
            Pos2::new(rect.min.x + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            title,
            font_id(
                volna_core::FontRole::UiSemibold,
                self.core_theme.ui_size_small,
            ),
            t.panel.text_muted,
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(Vec2::new(8.0, 4.0)))
                .layout(Layout::right_to_left(Align::Center)),
        );
        right(&mut child);
    }

    fn scopes_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.theme;
        let row_h = self.core_theme.row_height;
        let mut commands: Vec<Command> = Vec::new();
        self.panel_header(ui, "SCOPES", |ui| {
            if ui.small_button("«").on_hover_text("Collapse all").clicked() {
                commands.push(Command::ExpandAllScopes(false));
            }
            if ui.small_button("»").on_hover_text("Expand all").clicked() {
                commands.push(Command::ExpandAllScopes(true));
            }
        });
        let count = self.app.scopes.visible.len();
        let panel = ui.interact(
            ui.available_rect_before_wrap(),
            self.ids.scopes,
            Sense::click(),
        );
        if panel.clicked() {
            ui.memory_mut(|m| m.request_focus(self.ids.scopes));
        }
        if count == 0 {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("No scopes")
                        .small()
                        .color(t.panel.text_placeholder),
                );
            });
        } else {
            let focused = ui.memory(|m| m.has_focus(self.ids.scopes));
            let mut scroll = ScrollArea::vertical().id_salt("scopes").auto_shrink(false);
            if let Some(ix) = self.reveal_scope.take() {
                let offset = ix as f32 * row_h - ui.available_height() / 2.0;
                scroll = scroll.vertical_scroll_offset(offset.max(0.0));
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let visible = self.app.scopes.visible.clone();
            let Some(h) = self.app.doc.hierarchy() else {
                return;
            };
            scroll.show_rows(ui, row_h, count, |ui, range| {
                for ix in range {
                    let (id, depth) = visible[ix];
                    let scope = &h.scopes[id];
                    let has_children = !scope.children.is_empty();
                    let expanded = self.app.scopes.is_expanded(id);
                    let selected = self.app.scopes.selected == Some(id);
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_h),
                        Sense::click(),
                    );
                    let colors = if selected {
                        ui.painter().rect_filled(rect, 0.0, t.selection.bg);
                        t.selection
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, t.hover.bg);
                        t.hover
                    } else {
                        t.panel
                    };
                    if selected && focused {
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.0, t.border_focused),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let x = rect.min.x + 8.0 + 12.0 * depth as f32;
                    let chevron =
                        Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(16.0, row_h));
                    if has_children {
                        paint::chevron(ui.painter(), chevron.center(), expanded, colors.icon_muted);
                    }
                    paint::small_icon(
                        ui.painter(),
                        scope_icon(&scope.kind),
                        Rect::from_center_size(
                            Pos2::new(x + 27.0, rect.center().y),
                            Vec2::splat(14.0),
                        ),
                        colors.icon_muted,
                    );
                    ui.painter().text(
                        Pos2::new(x + 38.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &scope.name,
                        font_id(volna_core::FontRole::Ui, self.core_theme.ui_size),
                        colors.text,
                    );
                    if resp.clicked() {
                        ui.memory_mut(|m| m.request_focus(self.ids.scopes));
                        let on_chevron = resp
                            .interact_pointer_pos()
                            .is_some_and(|p| chevron.contains(p));
                        if on_chevron && has_children {
                            commands.push(Command::ToggleScope(id));
                        } else {
                            commands.push(Command::SelectScope(id));
                        }
                    }
                    if resp.double_clicked() && has_children {
                        commands.push(Command::ToggleScope(id));
                    }
                }
            });
        }
        for c in commands {
            self.dispatch(c);
        }
    }

    fn variables_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.theme;
        let row_h = self.core_theme.row_height;
        let mut commands: Vec<Command> = Vec::new();
        let count = self.app.variables.rows.len();
        self.panel_header(ui, "VARIABLES", |ui| {
            if ui
                .add_enabled(count > 0, egui::Button::new("+").small())
                .on_hover_text("Add all listed variables (⏎)")
                .clicked()
            {
                commands.push(Command::AddAllVars);
            }
            ui.label(
                RichText::new(count.to_string())
                    .font(font_id(
                        volna_core::FontRole::Mono,
                        self.core_theme.ui_size_small,
                    ))
                    .color(t.panel.text_placeholder),
            );
        });
        // Filter box.
        ui.add_space(4.0);
        let filter_resp = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0);
                ui.add_sized(
                    Vec2::new(ui.available_width() - 8.0, 24.0),
                    TextEdit::singleline(&mut self.filter)
                        .id(self.ids.filter)
                        .hint_text("Filter variables")
                        .font(font_id(volna_core::FontRole::Ui, self.core_theme.ui_size)),
                )
            })
            .inner;
        ui.add_space(4.0);
        if self.focus_filter {
            self.focus_filter = false;
            filter_resp.request_focus();
        }
        if filter_resp.changed() {
            commands.push(Command::SetFilter(self.filter.clone()));
        }
        if filter_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            commands.push(Command::AddSelectedOrAllVars);
        }
        let panel = ui.interact(
            ui.available_rect_before_wrap(),
            self.ids.variables,
            Sense::click(),
        );
        if panel.clicked() {
            ui.memory_mut(|m| m.request_focus(self.ids.variables));
        }
        let placeholder = self.app.variables.placeholder(self.app.doc.is_loaded());
        if let Some(text) = placeholder {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(text).small().color(t.panel.text_placeholder));
            });
        } else if let Some(h) = self.app.doc.hierarchy() {
            let focused = ui.memory(|m| m.has_focus(self.ids.variables));
            let show_scope = self.app.variables.show_scope();
            let show_direction = self.app.variables.show_direction(h);
            let mut scroll = ScrollArea::vertical()
                .id_salt("variables")
                .auto_shrink(false);
            if let Some(ix) = self.reveal_var.take() {
                let offset = ix as f32 * row_h - ui.available_height() / 2.0;
                scroll = scroll.vertical_scroll_offset(offset.max(0.0));
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let rows = self.app.variables.rows.clone();
            let mono = font_id(volna_core::FontRole::Mono, self.core_theme.mono_size);
            let small = font_id(volna_core::FontRole::Ui, self.core_theme.ui_size_small);
            scroll.show_rows(ui, row_h, count, |ui, range| {
                for ix in range {
                    let var = rows[ix];
                    let v = &h.vars[var];
                    let selected = self.app.variables.selected.contains(&ix);
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_h),
                        Sense::click(),
                    );
                    let colors = if selected {
                        ui.painter().rect_filled(rect, 0.0, t.selection.bg);
                        t.selection
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 0.0, t.hover.bg);
                        t.hover
                    } else {
                        t.panel
                    };
                    if selected && focused {
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            Stroke::new(1.0, t.border_focused),
                            egui::StrokeKind::Inside,
                        );
                    }
                    let mut x = rect.min.x + 8.0;
                    paint::small_icon(
                        ui.painter(),
                        shape_icon(v.shape),
                        Rect::from_center_size(
                            Pos2::new(x + 7.0, rect.center().y),
                            Vec2::splat(14.0),
                        ),
                        colors.icon_muted,
                    );
                    x += 22.0;
                    if show_direction {
                        ui.painter().text(
                            Pos2::new(x, rect.center().y),
                            Align2::LEFT_CENTER,
                            direction_label(v.direction),
                            small.clone(),
                            colors.text_muted,
                        );
                        x += 28.0;
                    }
                    let name = if show_scope {
                        h.full_name(var)
                    } else {
                        v.name.clone()
                    };
                    let dims = v.shape.dims();
                    let dims_w = if dims.is_empty() {
                        0.0
                    } else {
                        ui.painter()
                            .text(
                                Pos2::new(rect.max.x - 8.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                &dims,
                                mono.clone(),
                                colors.text_placeholder,
                            )
                            .width()
                            + 8.0
                    };
                    let name_clip = Rect::from_min_max(
                        Pos2::new(x, rect.min.y),
                        Pos2::new(rect.max.x - 8.0 - dims_w, rect.max.y),
                    );
                    ui.painter().with_clip_rect(name_clip).text(
                        Pos2::new(x, rect.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        mono.clone(),
                        colors.text,
                    );
                    if resp.double_clicked() {
                        commands.push(Command::AddVars(vec![var]));
                    } else if resp.clicked() {
                        ui.memory_mut(|m| m.request_focus(self.ids.variables));
                        let modifiers = to_modifiers(ui.input(|i| i.modifiers));
                        commands.push(Command::SelectVar { ix, modifiers });
                    }
                }
            });
        }
        for c in commands {
            self.dispatch(c);
        }
    }

    fn center(&mut self, root: &mut Ui) {
        let t = self.theme;
        let state = self.app.trace_state().clone();
        CentralPanel::default()
            .frame(Frame::NONE.fill(t.editor.bg))
            .show(root, |ui| match state {
                TraceState::Loaded(_) => self.wave_canvas(ui),
                TraceState::Loading { name } => {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new(format!("Loading {name}…")).color(t.editor.text_muted),
                        );
                    });
                }
                TraceState::Empty | TraceState::Error(_) => self.empty_state(ui, &state),
            });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, state: &TraceState) {
        let t = self.theme;
        let mut open = false;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(
                RichText::new("No trace open")
                    .size(16.0)
                    .strong()
                    .color(t.editor.text),
            );
            ui.label(
                RichText::new("Open a VTR or FST waveform file to view its signals")
                    .color(t.editor.text_muted),
            );
            if let TraceState::Error(e) = state {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("⚠ {e}")).color(t.panel.error));
            }
            ui.add_space(12.0);
            if ui.button("Open File…").clicked() {
                open = true;
            }
            ui.add_space(24.0);
            for (keys, label) in [
                ("⌘O", "Open a trace"),
                ("⏎", "Add selected variables"),
                ("= / -", "Zoom in / out"),
                ("F", "Zoom to fit"),
                ("M", "Add marker at cursor"),
                ("T", "Cycle value format"),
            ] {
                ui.label(
                    RichText::new(format!("{label}   {keys}"))
                        .small()
                        .color(t.editor.text_muted),
                );
            }
        });
        if open {
            self.dispatch(Command::RequestOpenDialog);
        }
    }

    fn wave_canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let rect = ui.available_rect_before_wrap();
        let _response = ui.allocate_rect(rect, Sense::click_and_drag());
        let bounds = CRect::from_xywh(rect.min.x, rect.min.y, rect.width(), rect.height());
        let panel = self.app.panels.focused_id();
        self.app.layout_waves(panel, bounds, &self.core_theme);

        // -- input ------------------------------------------------------------
        let menu_was_open = self.app.panels.focused_waves().unwrap().menu.is_some();
        let pos = ctx.input(|i| i.pointer.latest_pos());
        let inside = pos.is_some_and(|p| rect.contains(p));
        let modifiers = to_modifiers(ctx.input(|i| i.modifiers));
        if let Some(p) = pos {
            let position = volna_core::geometry::point(p.x, p.y);
            if inside || self.app.panels.focused_waves().unwrap().drag.is_some() {
                self.dispatch(Command::Pointer(panel, PointerEvent::Move { position }));
            }
            if inside {
                self.pointer_inside = true;
                for (eb, cb) in [
                    (egui::PointerButton::Primary, MouseButton::Left),
                    (egui::PointerButton::Secondary, MouseButton::Right),
                    (egui::PointerButton::Middle, MouseButton::Middle),
                ] {
                    if ctx.input(|i| i.pointer.button_pressed(eb)) {
                        ctx.memory_mut(|m| m.request_focus(self.ids.waves));
                        self.dispatch(Command::Pointer(
                            panel,
                            PointerEvent::Down {
                                position,
                                button: cb,
                                modifiers,
                            },
                        ));
                    }
                }
                let scroll = ctx.input(|i| i.smooth_scroll_delta);
                if scroll != Vec2::ZERO {
                    self.dispatch(Command::Pointer(
                        panel,
                        PointerEvent::Wheel {
                            position,
                            dx: scroll.x,
                            dy: scroll.y,
                            modifiers,
                        },
                    ));
                }
                let zoom = ctx.input(|i| i.zoom_delta());
                if zoom != 1.0 {
                    self.dispatch(Command::Pointer(
                        panel,
                        PointerEvent::Pinch {
                            position,
                            delta: zoom - 1.0,
                        },
                    ));
                }
            }
        }
        if !inside && self.pointer_inside {
            self.pointer_inside = false;
            self.dispatch(Command::Pointer(panel, PointerEvent::Leave));
        }
        if ctx.input(|i| i.pointer.any_released())
            && self.app.panels.focused_waves().unwrap().drag.is_some()
        {
            self.dispatch(Command::Pointer(panel, PointerEvent::Up));
        }

        // -- paint ------------------------------------------------------------
        let started = Instant::now();
        let mut scene = std::mem::take(&mut self.scene);
        let mut measure = EguiMeasure(&ctx);
        self.app
            .render_waves_into(panel, &self.core_theme, &mut measure, &mut scene);
        paint_scene(&scene, &ui.painter().with_clip_rect(rect));
        if let Some(icon) = scene.window_cursor {
            ctx.set_cursor_icon(cursor_icon(icon));
        } else if let Some(p) = pos
            && let Some((_, icon)) = scene
                .cursors
                .iter()
                .find(|(r, _)| paint::rect(*r).contains(p))
        {
            ctx.set_cursor_icon(cursor_icon(*icon));
        }
        self.scene = scene;
        self.app
            .panels
            .focused_waves_mut()
            .unwrap()
            .record_frame(started.elapsed().as_secs_f32() * 1000.0);

        // -- format menu ----------------------------------------------------------
        if let Some(menu) = self.app.panels.focused_waves().unwrap().menu.clone() {
            let t = self.theme;
            let mut chosen = None;
            let area = Area::new(self.ids.format_menu)
                .order(Order::Foreground)
                .fixed_pos(Pos2::new(menu.position.x, menu.position.y))
                .show(&ctx, |ui| {
                    Frame::popup(ui.style())
                        .fill(t.elevated.bg)
                        .stroke(Stroke::new(1.0, t.border))
                        .show(ui, |ui| {
                            ui.set_min_width(180.0);
                            for item in &menu.items {
                                let label = match &item.badge {
                                    Some(b) => format!("{}    {}", item.label, b),
                                    None => item.label.clone(),
                                };
                                if ui.selectable_label(item.checked, label).clicked() {
                                    chosen = Some(item.action.clone());
                                }
                            }
                        });
                });
            if let Some(id) = chosen {
                self.dispatch(Command::MenuSelect(panel, id));
            } else if menu_was_open
                && ctx.input(|i| i.pointer.any_pressed())
                && !area.response.contains_pointer()
            {
                self.dispatch(Command::MenuDismiss(panel));
            }
        }
    }
}

impl eframe::App for VolnaApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

pub fn to_modifiers(m: egui::Modifiers) -> Modifiers {
    Modifiers {
        shift: m.shift,
        control: m.ctrl,
        alt: m.alt,
        platform: m.mac_cmd,
    }
}

fn wave_action(key: Key, m: egui::Modifiers) -> Option<Action> {
    Some(match key {
        Key::Equals | Key::Plus => Action::ZoomIn,
        Key::Minus => Action::ZoomOut,
        Key::F => Action::ZoomFit,
        Key::Home | Key::S => Action::GoToStart,
        Key::End | Key::E => Action::GoToEnd,
        Key::C => Action::GoToCursor,
        Key::ArrowLeft if m.shift => Action::PrevEdge,
        Key::ArrowLeft => Action::PanLeft,
        Key::ArrowRight if m.shift => Action::NextEdge,
        Key::ArrowRight => Action::PanRight,
        Key::M if m.shift => Action::ClearMarkers,
        Key::M => Action::AddMarker,
        Key::Backspace | Key::Delete => Action::RemoveSelected,
        Key::A if m.command => Action::SelectAll,
        Key::Escape => Action::ClearSelection,
        Key::T => Action::CycleFormat,
        Key::ArrowUp => Action::MoveSelectionUp,
        Key::ArrowDown => Action::MoveSelectionDown,
        _ => return None,
    })
}

fn list_key(key: Key) -> Option<ListKey> {
    Some(match key {
        Key::ArrowUp => ListKey::Up,
        Key::ArrowDown => ListKey::Down,
        Key::ArrowLeft => ListKey::Left,
        Key::ArrowRight => ListKey::Right,
        Key::Enter => ListKey::Enter,
        Key::Space => ListKey::Space,
        Key::Escape => ListKey::Escape,
        _ => return None,
    })
}
