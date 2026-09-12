//! Design tokens. Every colour, font and metric used by the viewer lives here,
//! Native/web defaults use One Dark; embedded hosts supply semantic colours.

use gpui::{App, Global, Hsla, Pixels, Rgba, SharedString, px, rgb, rgba};

#[derive(Clone)]
pub struct Theme {
    // Surfaces
    /// Waveform canvas and editor-like areas.
    pub bg_editor: Hsla,
    /// Side panels, headers, name/value columns.
    pub bg_panel: Hsla,
    /// Title bar and status bar.
    pub bg_bar: Hsla,
    /// Popovers, tooltips, menus.
    pub bg_elevated: Hsla,

    // Borders
    pub border: Hsla,
    pub border_variant: Hsla,
    pub border_focused: Hsla,

    // Interactive elements
    pub element_hover: Hsla,
    pub element_active: Hsla,
    pub element_selected: Hsla,
    pub ghost_element_hover: Hsla,

    // Text and icons
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub text_accent: Hsla,
    pub icon: Hsla,
    pub icon_muted: Hsla,
    pub icon_accent: Hsla,

    // Status
    pub accent: Hsla,
    pub error: Hsla,
    pub warning: Hsla,
    pub success: Hsla,

    // Scrollbars
    pub scrollbar_thumb: Hsla,
    pub scrollbar_thumb_hover: Hsla,

    // Waveforms
    pub wave_signal: Hsla,
    pub wave_high_fill: Hsla,
    pub wave_undef: Hsla,
    pub wave_highimp: Hsla,
    pub wave_dontcare: Hsla,
    pub wave_weak: Hsla,
    pub wave_dense: Hsla,
    pub wave_bus_text: Hsla,
    pub wave_tick: Hsla,
    pub wave_tick_text: Hsla,
    pub wave_row_selected: Hsla,
    pub wave_row_hover: Hsla,
    pub wave_cursor: Hsla,
    pub wave_cursor_text: Hsla,
    pub wave_marker_palette: [Hsla; 6],

    pub bg_input: Hsla,
    pub input_text: Hsla,
    pub input_border: Hsla,
    pub panel_text: Hsla,
    pub bar_text: Hsla,
    pub elevated_text: Hsla,
    pub selected_text: Hsla,
    pub button_bg: Hsla,
    pub button_text: Hsla,
    pub button_hover: Hsla,
    pub high_contrast: bool,
    host_colors: bool,

    // Typography
    pub ui_font: SharedString,
    pub mono_font: SharedString,
    pub ui_size: Pixels,
    pub ui_size_small: Pixels,
    pub mono_size: Pixels,

    // Metrics (multiples of the 4px grid)
    pub row_height: Pixels,
    pub header_height: Pixels,
    pub titlebar_height: Pixels,
    pub statusbar_height: Pixels,
    pub timeline_height: Pixels,
    pub icon_size: Pixels,
    pub splitter_grab: Pixels,
}

impl Global for Theme {}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h: Hsla = rgb(hex).into();
    h.a = alpha;
    h
}

#[allow(dead_code)]
fn c_rgba(hex_with_alpha: u32) -> Hsla {
    let v: Rgba = rgba(hex_with_alpha);
    v.into()
}

impl Theme {
    /// Zed "One Dark".
    pub fn one_dark() -> Self {
        Theme {
            bg_editor: c(0x282c33),
            bg_panel: c(0x2f343e),
            bg_bar: c(0x3b414d),
            bg_elevated: c(0x2f343e),

            border: c(0x464b57),
            border_variant: c(0x363c46),
            border_focused: c(0x47679e),

            element_hover: c(0x363c46),
            element_active: c(0x454a56),
            element_selected: c(0x454a56),
            ghost_element_hover: c(0x363c46),

            text: c(0xdce0e5),
            text_muted: c(0xa9afbc),
            text_placeholder: c(0x878a98),
            text_accent: c(0x74ade8),
            icon: c(0xdce0e5),
            icon_muted: c(0xa9afbc),
            icon_accent: c(0x74ade8),

            accent: c(0x74ade8),
            error: c(0xd07277),
            warning: c(0xdec184),
            success: c(0xa1c181),

            scrollbar_thumb: ca(0xc8ccd4, 0.30),
            scrollbar_thumb_hover: ca(0xc8ccd4, 0.50),

            wave_signal: c(0xa1c181),
            wave_high_fill: ca(0xa1c181, 0.10),
            wave_undef: c(0xd07277),
            wave_highimp: c(0xdec184),
            wave_dontcare: c(0x74ade8),
            wave_weak: c(0x878a98),
            wave_dense: ca(0xa1c181, 0.55),
            wave_bus_text: c(0xdce0e5),
            wave_tick: ca(0xc8ccd4, 0.08),
            wave_tick_text: c(0xa9afbc),
            wave_row_selected: ca(0x74ade8, 0.10),
            wave_row_hover: ca(0xc8ccd4, 0.04),
            wave_cursor: c(0x74ade8),
            wave_cursor_text: c(0x282c33),
            wave_marker_palette: [
                c(0xbf956a),
                c(0xb477cf),
                c(0x6eb4bf),
                c(0xd07277),
                c(0xdec184),
                c(0xa1c181),
            ],

            bg_input: c(0x282c33),
            input_text: c(0xdce0e5),
            input_border: c(0x464b57),
            panel_text: c(0xdce0e5),
            bar_text: c(0xdce0e5),
            elevated_text: c(0xdce0e5),
            selected_text: c(0xdce0e5),
            button_bg: c(0x74ade8),
            button_text: c(0x282c33),
            button_hover: c(0x74ade8),
            high_contrast: false,
            host_colors: false,

            ui_font: "IBM Plex Sans".into(),
            mono_font: "Lilex".into(),
            ui_size: px(13.0),
            ui_size_small: px(11.0),
            mono_size: px(12.0),

            row_height: px(24.0),
            header_height: px(32.0),
            titlebar_height: px(32.0),
            statusbar_height: px(24.0),
            timeline_height: px(32.0),
            icon_size: px(16.0),
            splitter_grab: px(8.0),
        }
    }

    /// Resolve semantic host colours, independent of any editor or transport.
    /// Packed colours are RGBA (including alpha); absent tokens use mode-aware fallbacks.
    pub fn from_host(dark: bool, high_contrast: bool, color: impl Fn(&str) -> Option<u32>) -> Self {
        let mut t = Self::one_dark();
        let bg = if dark {
            if high_contrast { c(0) } else { c(0x1e1e1e) }
        } else {
            c(0xffffff)
        };
        let fg = if dark { c(0xcccccc) } else { c(0x333333) };
        let get =
            |name: &str, fallback: Hsla| color(name).map(|v| rgba(v).into()).unwrap_or(fallback);
        t.high_contrast = high_contrast;
        t.host_colors = true;
        t.bg_editor = over(get("bg_editor", bg), bg);
        t.bg_panel = over(get("bg_panel", t.bg_editor), t.bg_editor);
        t.bg_bar = over(get("bg_bar", t.bg_panel), t.bg_panel);
        t.bg_elevated = over(get("bg_elevated", t.bg_panel), t.bg_panel);
        t.text = readable(get("text", fg), t.bg_editor, 4.5);
        t.panel_text = readable(get("panel_text", t.text), t.bg_panel, 4.5);
        t.bar_text = readable(get("bar_text", t.text), t.bg_bar, 4.5);
        t.elevated_text = readable(get("elevated_text", t.text), t.bg_elevated, 4.5);
        t.text_muted = readable(get("text_muted", t.text), t.bg_editor, 4.5);
        t.text_placeholder = readable(get("text_placeholder", t.text_muted), t.bg_editor, 4.5);
        let accent = if dark { c(0x75beff) } else { c(0x005fb8) };
        t.accent = readable(get("accent", accent), t.bg_editor, 3.0);
        t.text_accent = readable(get("text_accent", t.accent), t.bg_editor, 4.5);
        t.icon = readable(get("icon", t.panel_text), t.bg_panel, 3.0);
        t.icon_muted = readable(t.text_muted, t.bg_panel, 3.0);
        t.icon_accent = readable(t.accent, t.bg_panel, 3.0);
        t.border = get("border", over(alpha(t.text, 0.25), t.bg_panel));
        t.border_variant = get("border_variant", t.border);
        t.border_focused = get("border_focused", t.accent);
        if high_contrast {
            t.border = readable(get("contrast_border", t.border), t.bg_panel, 3.0);
            t.border_variant = t.border;
            t.border_focused = readable(t.border_focused, t.bg_panel, 3.0);
        }
        t.element_hover = over(get("element_hover", alpha(t.panel_text, 0.08)), t.bg_panel);
        t.element_active = over(get("element_active", alpha(t.panel_text, 0.15)), t.bg_panel);
        t.element_selected = over(get("element_selected", alpha(t.accent, 0.25)), t.bg_panel);
        t.selected_text = readable(get("selected_text", t.panel_text), t.element_selected, 4.5);
        t.ghost_element_hover = t.element_hover;
        t.bg_input = over(get("bg_input", t.bg_editor), t.bg_editor);
        t.input_text = readable(get("input_text", t.text), t.bg_input, 4.5);
        t.input_border = get("input_border", t.border);
        if high_contrast {
            t.input_border = readable(t.input_border, t.bg_input, 3.0);
        }
        t.button_bg = over(get("button_bg", t.accent), t.bg_panel);
        t.button_text = readable(get("button_text", c(0xffffff)), t.button_bg, 4.5);
        t.button_hover = over(get("button_hover", t.button_bg), t.bg_panel);
        t.error = readable(
            get("error", if dark { c(0xf48771) } else { c(0xa1260d) }),
            t.bg_editor,
            4.5,
        );
        t.warning = readable(
            get("warning", if dark { c(0xcca700) } else { c(0x895503) }),
            t.bg_editor,
            4.5,
        );
        t.success = readable(
            get("success", if dark { c(0x89d185) } else { c(0x287b38) }),
            t.bg_editor,
            4.5,
        );
        t.scrollbar_thumb = get("scrollbar_thumb", alpha(t.panel_text, 0.4));
        t.scrollbar_thumb_hover = get("scrollbar_thumb_hover", alpha(t.panel_text, 0.7));
        if high_contrast {
            t.scrollbar_thumb = readable(t.scrollbar_thumb, t.bg_panel, 3.0);
            t.scrollbar_thumb_hover = readable(t.scrollbar_thumb_hover, t.bg_panel, 3.0);
        }
        // Low-alpha row overlays keep wave colours readable on the actual canvas.
        t.wave_row_selected = alpha(t.accent, if high_contrast { 0.08 } else { 0.10 });
        t.wave_row_hover = alpha(t.text, 0.04);
        let wave_bg = over(t.wave_row_selected, t.bg_editor);
        let wave = |name, fallback| {
            readable(
                readable(get(name, fallback), t.bg_editor, 4.5),
                wave_bg,
                4.5,
            )
        };
        t.wave_signal = wave("wave_signal", t.success);
        t.wave_undef = wave("wave_undef", t.error);
        t.wave_highimp = wave("wave_highimp", t.warning);
        t.wave_dontcare = wave("wave_dontcare", t.accent);
        t.wave_weak = wave("wave_weak", t.text_muted);
        t.wave_high_fill = alpha(t.wave_signal, 0.10);
        t.wave_dense = alpha(t.wave_signal, if high_contrast { 0.8 } else { 0.55 });
        t.wave_bus_text = t.text;
        t.wave_tick = if high_contrast {
            alpha(t.text, 0.4)
        } else {
            alpha(t.text, 0.12)
        };
        t.wave_tick_text = readable(t.text_muted, t.bg_panel, 4.5);
        t.wave_cursor = wave("wave_cursor", t.accent);
        t.wave_cursor_text = readable(t.bg_editor, t.wave_cursor, 4.5);
        for (i, name) in [
            "marker_orange",
            "marker_purple",
            "marker_blue",
            "wave_undef",
            "wave_highimp",
            "wave_signal",
        ]
        .iter()
        .enumerate()
        {
            t.wave_marker_palette[i] = wave(name, t.wave_marker_palette[i]);
        }
        t
    }

    /// Replace presentation only and invalidate every view, including cached children.
    pub fn install(self, cx: &mut App) {
        cx.set_global(self);
        cx.refresh_windows();
    }

    pub fn input(&self) -> Self {
        self.on_surface(self.bg_input, self.input_text)
    }

    pub fn panel(&self) -> Self {
        self.on_surface(self.bg_panel, self.panel_text)
    }

    pub fn bar(&self) -> Self {
        self.on_surface(self.bg_bar, self.bar_text)
    }

    pub fn elevated(&self) -> Self {
        self.on_surface(self.bg_elevated, self.elevated_text)
    }

    pub fn row(&self, selected: bool, hovered: bool) -> Self {
        let bg = if selected {
            self.element_selected
        } else if hovered {
            self.element_hover
        } else {
            self.bg_panel
        };
        let fg = if selected {
            self.selected_text
        } else {
            self.panel_text
        };
        let mut t = self.on_surface(bg, fg);
        if self.host_colors {
            t.text = readable(fg, bg, 4.5);
            t.wave_undef = readable(t.wave_undef, bg, 4.5);
            t.wave_highimp = readable(t.wave_highimp, bg, 4.5);
            t.wave_dontcare = readable(t.wave_dontcare, bg, 4.5);
            t.wave_weak = readable(t.wave_weak, bg, 4.5);
            t.icon = readable(t.icon, bg, 3.0);
            t.icon_muted = readable(t.icon_muted, bg, 3.0);
            t.icon_accent = readable(t.icon_accent, bg, 3.0);
        }
        t
    }

    pub fn marker_text(&self, background: Hsla) -> Hsla {
        if self.host_colors {
            readable(self.wave_cursor_text, over(background, self.bg_panel), 4.5)
        } else {
            self.wave_cursor_text
        }
    }

    fn on_surface(&self, bg: Hsla, text: Hsla) -> Self {
        let mut t = self.clone();
        t.text = text;
        if self.host_colors {
            t.text_muted = readable(t.text_muted, bg, 4.5);
            t.text_placeholder = readable(t.text_placeholder, bg, 4.5);
            t.icon_accent = readable(t.icon_accent, bg, 3.0);
        }
        t
    }

    pub fn get(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }

    /// Colour for a value classification.
    pub fn value_color(&self, kind: crate::data::ValueKind) -> Hsla {
        use crate::data::ValueKind::*;
        match kind {
            Normal => self.wave_signal,
            Undef => self.wave_undef,
            HighImp => self.wave_highimp,
            DontCare => self.wave_dontcare,
            Weak => self.wave_weak,
        }
    }

    pub fn marker_color(&self, index: usize) -> Hsla {
        self.wave_marker_palette[index % self.wave_marker_palette.len()]
    }
}

/// Convenience accessor: `theme(cx).text`.
pub fn theme(cx: &App) -> &Theme {
    Theme::get(cx)
}

fn alpha(mut color: Hsla, a: f32) -> Hsla {
    color.a = a;
    color
}

/// Composite translucent tokens before evaluating contrast.
pub fn over(fg: Hsla, bg: Hsla) -> Hsla {
    if fg.a == 1.0 {
        return fg;
    }
    let f: Rgba = fg.into();
    let b: Rgba = bg.into();
    Rgba {
        r: f.r * f.a + b.r * (1.0 - f.a),
        g: f.g * f.a + b.g * (1.0 - f.a),
        b: f.b * f.a + b.b * (1.0 - f.a),
        a: 1.0,
    }
    .into()
}

fn luminance(color: Hsla) -> f32 {
    let c: Rgba = color.into();
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
}

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Keep host colours when legible; move toward black/white only as necessary.
pub fn readable(color: Hsla, bg: Hsla, ratio: f32) -> Hsla {
    let color = over(color, bg);
    if contrast(color, bg) >= ratio {
        return color;
    }
    let target = if contrast(c(0xffffff), bg) > contrast(c(0), bg) {
        c(0xffffff)
    } else {
        c(0)
    };
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if contrast(over(alpha(target, mid), color), bg) >= ratio {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    over(alpha(target, hi), color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_custom_colors_and_alpha_are_preserved() {
        let t = Theme::from_host(false, false, |name| match name {
            "bg_editor" => Some(0xfff4e6ff),
            "text" => Some(0x321800ff),
            "bg_panel" => Some(0x00000080),
            "scrollbar_thumb" => Some(0x12345678),
            _ => None,
        });
        assert_eq!(t.bg_editor, c(0xfff4e6));
        assert!(contrast(t.text, t.bg_editor) > 4.5);
        assert_eq!(t.text, c(0x321800));
        assert_eq!(t.bg_panel, over(rgba(0x00000080).into(), t.bg_editor));
        assert_eq!(t.scrollbar_thumb, Hsla::from(rgba(0x12345678)));
    }

    #[test]
    fn missing_and_low_contrast_colors_work_in_all_modes() {
        for dark in [false, true] {
            for hc in [false, true] {
                for supplied in [false, true] {
                    let t = Theme::from_host(dark, hc, |_| supplied.then_some(0x80808030));
                    for (fg, bg) in [
                        (t.text, t.bg_editor),
                        (t.panel_text, t.bg_panel),
                        (t.bar_text, t.bg_bar),
                        (t.elevated_text, t.bg_elevated),
                        (t.input_text, t.bg_input),
                        (t.selected_text, t.element_selected),
                        (t.button_text, t.button_bg),
                    ] {
                        assert!(contrast(fg, bg) >= 4.49);
                    }
                    for wave in [
                        t.wave_signal,
                        t.wave_undef,
                        t.wave_highimp,
                        t.wave_dontcare,
                        t.wave_weak,
                        t.wave_cursor,
                    ] {
                        for bg in [
                            t.bg_editor,
                            over(t.wave_row_selected, t.bg_editor),
                            over(t.wave_row_hover, t.bg_editor),
                        ] {
                            assert!(
                                contrast(wave, bg) >= 3.0,
                                "wave contrast in dark={dark} hc={hc}"
                            );
                        }
                    }
                    for marker in t.wave_marker_palette {
                        assert!(contrast(t.marker_text(marker), marker) >= 4.49);
                    }
                    if hc {
                        assert!(contrast(t.border, t.bg_panel) >= 2.99);
                    }
                }
            }
        }
    }
}
