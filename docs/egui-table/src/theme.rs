//! Named palettes are a demo preference; the table follows ordinary egui visuals.
use eframe::egui::{self, Color32, FontId, RichText, Stroke, vec2};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    AtlasLight,
    GitHubLight,
    CatppuccinLatte,
    AtlasDark,
    GitHubDark,
    CatppuccinMocha,
    Monokai,
    Dracula,
}

impl Theme {
    pub const ALL: [Self; 8] = [
        Self::AtlasLight,
        Self::GitHubLight,
        Self::CatppuccinLatte,
        Self::AtlasDark,
        Self::GitHubDark,
        Self::CatppuccinMocha,
        Self::Monokai,
        Self::Dracula,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::AtlasLight => "Atlas Light",
            Self::AtlasDark => "Atlas Dark",
            Self::GitHubLight => "GitHub Light",
            Self::GitHubDark => "GitHub Dark",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::Monokai => "Monokai",
            Self::Dracula => "Dracula",
        }
    }

    pub fn mode(self) -> egui::Theme {
        match self {
            Self::AtlasLight | Self::GitHubLight | Self::CatppuccinLatte => egui::Theme::Light,
            _ => egui::Theme::Dark,
        }
    }

    fn colors(self) -> [Color32; 9] {
        // Ink, muted text, accent, canvas, surface, subtle surface, border,
        // hover, selection. UI adaptations of the upstream palettes:
        // https://github.com/primer/primitives
        // https://catppuccin.com/palette/
        // https://github.com/microsoft/vscode/blob/main/extensions/theme-monokai/themes/monokai-color-theme.json
        // https://draculatheme.com/spec
        let hex = match self {
            Self::AtlasLight => [
                0x1e2b3e, 0x5e6d82, 0x3063de, 0xf7f9fc, 0xffffff, 0xf3f6fb, 0xdbe3ee, 0xe9f0fd,
                0xd7e5ff,
            ],
            Self::AtlasDark => [
                0xe2eaf6, 0x97a8c0, 0x86b1ff, 0x0f1623, 0x172131, 0x1c283a, 0x314156, 0x24354d,
                0x2b446c,
            ],
            Self::GitHubLight => [
                0x1f2328, 0x59636e, 0x0969da, 0xf6f8fa, 0xffffff, 0xf6f8fa, 0xd1d9e0, 0xeaf2fb,
                0xddedff,
            ],
            Self::GitHubDark => [
                0xf0f6fc, 0x9198a1, 0x58a6ff, 0x010409, 0x0d1117, 0x151b23, 0x3d444d, 0x1c2d41,
                0x203b59,
            ],
            Self::CatppuccinLatte => [
                0x4c4f69, 0x5c5f77, 0x8839ef, 0xe6e9ef, 0xeff1f5, 0xe9ecf2, 0xbcc0cc, 0xe4def0,
                0xdccfee,
            ],
            Self::CatppuccinMocha => [
                0xcdd6f4, 0xbac2de, 0xcba6f7, 0x181825, 0x1e1e2e, 0x242437, 0x45475a, 0x313244,
                0x45405e,
            ],
            Self::Monokai => [
                0xf8f8f2, 0xb6b6a9, 0xa6e22e, 0x1e1f1c, 0x272822, 0x2e2f29, 0x49483e, 0x383a2e,
                0x454b32,
            ],
            Self::Dracula => [
                0xf8f8f2, 0xb8b9ce, 0xbd93f9, 0x21222c, 0x282a36, 0x303240, 0x494c62, 0x3a3c50,
                0x44475a,
            ],
        };
        hex.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8))
    }

    fn style(self) -> egui::Style {
        let mut style = egui::Style {
            visuals: match self.mode() {
                egui::Theme::Light => egui::Visuals::light(),
                egui::Theme::Dark => egui::Visuals::dark(),
            },
            ..Default::default()
        };
        let v = &mut style.visuals;
        let [
            ink,
            muted,
            accent,
            background,
            surface,
            subtle,
            line,
            hover,
            selected,
        ] = self.colors();
        v.override_text_color = Some(ink);
        v.weak_text_color = Some(muted);
        v.panel_fill = background;
        v.window_fill = surface;
        v.window_stroke = Stroke::new(1.0, line);
        v.extreme_bg_color = surface;
        v.text_edit_bg_color = Some(surface);
        v.faint_bg_color = subtle;
        v.code_bg_color = subtle;
        v.hyperlink_color = accent;
        v.selection.bg_fill = selected;
        v.selection.stroke = Stroke::new(1.0, accent);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, line);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, ink);
        for (widget, fill, border) in [
            (&mut v.widgets.inactive, surface, line),
            (&mut v.widgets.hovered, hover, accent),
            (&mut v.widgets.active, selected, accent),
            (&mut v.widgets.open, selected, accent),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::new(1.0, border);
            widget.fg_stroke = Stroke::new(1.0, ink);
            widget.corner_radius = 6.into();
            widget.expansion = 0.0;
        }
        style.spacing.item_spacing = vec2(10.0, 9.0);
        style.spacing.button_padding = vec2(12.0, 7.0);
        style.spacing.interact_size.y = 32.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
    }

    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_style_of(self.mode(), self.style());
        ctx.set_theme(self.mode());
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("demo-theme"), self));
        ctx.request_repaint();
    }
}

pub fn current(ctx: &egui::Context) -> Theme {
    ctx.data(|data| data.get_temp(egui::Id::new("demo-theme")))
        .unwrap_or_default()
}

pub fn initialize(ctx: &egui::Context) {
    ctx.set_style_of(egui::Theme::Dark, Theme::AtlasDark.style());
    Theme::default().apply(ctx);
}

pub fn selector(ui: &mut egui::Ui) {
    let previous = current(ui.ctx());
    let mut selected = previous;
    egui::ComboBox::from_id_salt("theme-selector")
        .width(190.0)
        .height(360.0)
        .selected_text(RichText::new(previous.name()).size(13.0))
        .show_ui(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for (mode, label) in [(egui::Theme::Light, "LIGHT"), (egui::Theme::Dark, "DARK")] {
                ui.label(RichText::new(label).size(10.0).weak());
                for theme in Theme::ALL.into_iter().filter(|theme| theme.mode() == mode) {
                    ui.selectable_value(&mut selected, theme, theme.name());
                }
            }
        })
        .response
        .on_hover_text("Choose a color theme");
    if selected != previous {
        selected.apply(ui.ctx());
    }
}
