//! Resolved design tokens. Host colours are resolved once, never while painting a row.
mod palette;
pub use palette::{Appearance, ColorPair, HostPalette};
mod color;
pub use color::parse_css_color;
pub mod vscode;

use gpui::{App, Global, Hsla, Pixels, Rgba, px, rgb};

/// A resolved surface or interaction state. Cheap to copy into GPUI closures.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub bg: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_placeholder: Hsla,
    pub icon: Hsla,
    pub icon_muted: Hsla,
    pub icon_accent: Hsla,
    pub error: Hsla,
    values: [Hsla; 5],
}

impl Surface {
    pub fn value_color(self, kind: crate::data::ValueKind) -> Hsla {
        self.values[value_index(kind)]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MarkerColors {
    pub stroke: Hsla,
    pub background: Hsla,
    pub hover: Hsla,
    pub text: Hsla,
    pub hover_text: Hsla,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub appearance: Appearance,
    pub editor: Surface,
    pub panel: Surface,
    pub bar: Surface,
    pub bar_hover: Surface,
    pub elevated: Surface,
    pub tooltip: Surface,
    pub input: Surface,
    pub selection: Surface,
    pub hover: Surface,
    pub menu_hover: Surface,
    pub button: Surface,
    pub button_hover: Surface,
    pub badge: Surface,
    pub badge_hover: Surface,
    pub border: Hsla,
    pub border_variant: Hsla,
    pub border_focused: Hsla,
    pub input_border: Hsla,
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
    pub markers: [MarkerColors; 6],

    // Typography
    pub ui_font: &'static str,
    pub mono_font: &'static str,
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
fn ca(hex: u32, a: f32) -> Hsla {
    alpha(c(hex), a)
}
fn alpha(mut color: Hsla, a: f32) -> Hsla {
    color.a = a;
    color
}

impl Theme {
    /// Native and standalone-web defaults, unchanged by host theming.
    pub fn one_dark() -> Self {
        let surface = |bg| Surface {
            bg: c(bg),
            text: c(0xdce0e5),
            text_muted: c(0xa9afbc),
            text_placeholder: c(0x878a98),
            icon: c(0xdce0e5),
            icon_muted: c(0xa9afbc),
            icon_accent: c(0x74ade8),
            error: c(0xd07277),
            values: [
                c(0xdce0e5),
                c(0xd07277),
                c(0xdec184),
                c(0x74ade8),
                c(0x878a98),
            ],
        };
        Self {
            appearance: Appearance::Dark,
            editor: surface(0x282c33),
            panel: surface(0x2f343e),
            bar: surface(0x3b414d),
            bar_hover: surface(0x363c46),
            elevated: surface(0x2f343e),
            tooltip: surface(0x2f343e),
            input: surface(0x282c33),
            selection: surface(0x454a56),
            hover: surface(0x363c46),
            menu_hover: surface(0x363c46),
            button: Surface {
                text: c(0x282c33),
                ..surface(0x74ade8)
            },
            button_hover: Surface {
                text: c(0x282c33),
                ..surface(0x74ade8)
            },
            badge: surface(0x282c33),
            badge_hover: surface(0x454a56),
            border: c(0x464b57),
            border_variant: c(0x363c46),
            border_focused: c(0x47679e),
            input_border: c(0x464b57),
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
            markers: [0xbf956a, 0xb477cf, 0x6eb4bf, 0xd07277, 0xdec184, 0xa1c181].map(|hex| {
                MarkerColors {
                    stroke: c(hex),
                    background: ca(hex, 0.85),
                    hover: c(hex),
                    text: c(0x282c33),
                    hover_text: c(0x282c33),
                }
            }),
            ui_font: "IBM Plex Sans",
            mono_font: "Lilex",
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

    pub fn from_host(p: &HostPalette) -> Self {
        let mut t = Self::one_dark();
        let dark = p.appearance.is_dark();
        let hc = p.appearance.is_high_contrast();
        let bg = if dark {
            if hc { c(0) } else { c(0x1e1e1e) }
        } else {
            c(0xffffff)
        };
        let fg = if dark { c(0xcccccc) } else { c(0x333333) };
        let accent = p
            .accent
            .or(p.focus)
            .unwrap_or(if dark { c(0x75beff) } else { c(0x005fb8) });
        let fallback = if dark {
            [0x89d185, 0xf48771, 0xcca700, 0x75beff, 0xd18616, 0xb180d7]
        } else {
            [0x287b38, 0xa1260d, 0x895503, 0x005fb8, 0xb05b00, 0x8040a0]
        };
        // Charts are fills in the host. For thin strokes/chips use their RGB at full
        // coverage rather than flattening alpha into a desaturated background tint.
        // Fully transparent entries carry no visible colour and use the fallback.
        let charts: [Hsla; 6] = std::array::from_fn(|i| opaque(p.charts[i], c(fallback[i])));
        let resolve = |pair, bg, fg| Surface::resolve(pair, bg, fg, p, accent, charts);
        t.appearance = p.appearance;
        t.editor = resolve(p.editor, bg, fg);
        t.panel = resolve(p.panel, t.editor.bg, t.editor.text);
        t.bar = resolve(p.bar, t.panel.bg, t.panel.text);
        t.bar_hover = resolve(
            ColorPair::default(),
            over(alpha(t.bar.text, 0.1), t.bar.bg),
            t.bar.text,
        );
        t.elevated = resolve(p.elevated, t.panel.bg, t.panel.text);
        t.tooltip = resolve(p.tooltip, t.elevated.bg, t.elevated.text);
        let with_bg = |pair: ColorPair, fallback| ColorPair {
            background: Some(pair.background.unwrap_or(fallback)),
            ..pair
        };
        t.input = resolve(with_bg(p.input, t.editor.bg), t.panel.bg, t.editor.text);
        t.selection = resolve(
            with_bg(p.selection, over(alpha(accent, 0.25), t.panel.bg)),
            t.panel.bg,
            t.panel.text,
        );
        t.hover = resolve(
            with_bg(p.hover, over(alpha(t.panel.text, 0.08), t.panel.bg)),
            t.panel.bg,
            t.panel.text,
        );
        t.menu_hover = resolve(
            with_bg(p.menu_hover, t.hover.bg),
            t.elevated.bg,
            t.elevated.text,
        );
        t.button = resolve(with_bg(p.button, accent), t.panel.bg, c(0xffffff));
        t.button_hover = resolve(
            ColorPair {
                background: p.button_hover,
                foreground: p.button.foreground,
            },
            t.button.bg,
            t.button.text,
        );
        t.badge = t.input;
        t.badge_hover = resolve(
            ColorPair::default(),
            over(alpha(t.input.text, 0.15), t.input.bg),
            t.input.text,
        );
        t.border = p
            .border
            .unwrap_or(over(alpha(t.panel.text, 0.25), t.panel.bg));
        t.border_variant = p.panel_border.unwrap_or(t.border);
        t.border_focused = p.focus.unwrap_or(accent);
        t.input_border = p.input_border.unwrap_or(t.border);
        if hc {
            t.border = stroke(p.contrast_border.unwrap_or(t.border), &[t.panel.bg]);
            t.border_variant = t.border;
            t.border_focused = stroke(t.border_focused, &[t.panel.bg]);
            t.input_border = stroke(p.contrast_border.unwrap_or(t.input_border), &[t.input.bg]);
        }
        t.scrollbar_thumb = p.scrollbar.unwrap_or(alpha(t.panel.text, 0.4));
        t.scrollbar_thumb_hover = p.scrollbar_hover.unwrap_or(alpha(t.panel.text, 0.7));
        if hc {
            t.scrollbar_thumb = stroke(over(t.scrollbar_thumb, t.panel.bg), &[t.panel.bg]);
            t.scrollbar_thumb_hover =
                stroke(over(t.scrollbar_thumb_hover, t.panel.bg), &[t.panel.bg]);
        }
        t.wave_row_selected = alpha(accent, if hc { 0.08 } else { 0.10 });
        t.wave_row_hover = alpha(t.editor.text, 0.04);
        let backgrounds = [
            t.editor.bg,
            over(t.wave_row_selected, t.editor.bg),
            over(t.wave_row_hover, t.editor.bg),
        ];
        t.wave_signal = stroke(charts[0], &backgrounds);
        t.wave_undef = stroke(charts[1], &backgrounds);
        t.wave_highimp = stroke(charts[2], &backgrounds);
        t.wave_dontcare = stroke(charts[3], &backgrounds);
        t.wave_weak = stroke(opaque(p.muted, t.editor.text), &backgrounds);
        t.wave_high_fill = alpha(t.wave_signal, 0.10);
        t.wave_dense = alpha(t.wave_signal, if hc { 0.8 } else { 0.55 });
        t.wave_bus_text = t.editor.text;
        t.wave_tick = alpha(t.editor.text, if hc { 0.4 } else { 0.12 });
        t.wave_tick_text = t.panel.text_muted;
        t.wave_cursor = stroke(opaque(p.cursor, accent), &backgrounds);
        t.wave_cursor_text = fallback_text(t.editor.text, t.wave_cursor);
        t.markers = [4, 5, 3, 1, 2, 0].map(|i| MarkerColors {
            stroke: stroke(charts[i], &backgrounds),
            background: charts[i],
            hover: charts[i],
            text: fallback_text(t.panel.text, charts[i]),
            hover_text: fallback_text(t.panel.text, charts[i]),
        });
        t
    }

    /// Presentation only; invalidate cached children without rebuilding viewer state.
    pub fn install(self, cx: &mut App) {
        cx.set_global(self);
        cx.refresh_windows();
    }
    pub fn row(&self, selected: bool, hovered: bool) -> Surface {
        if selected {
            self.selection
        } else if hovered {
            self.hover
        } else {
            self.panel
        }
    }
    pub fn value_color(&self, kind: crate::data::ValueKind) -> Hsla {
        [
            self.wave_signal,
            self.wave_undef,
            self.wave_highimp,
            self.wave_dontcare,
            self.wave_weak,
        ][value_index(kind)]
    }
    pub fn marker(&self, index: usize) -> MarkerColors {
        self.markers[index % self.markers.len()]
    }
}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

impl Surface {
    fn resolve(
        pair: ColorPair,
        parent: Hsla,
        fallback: Hsla,
        p: &HostPalette,
        accent: Hsla,
        charts: [Hsla; 6],
    ) -> Self {
        let bg = over(pair.background.unwrap_or(parent), parent);
        let text = foreground(pair.foreground, fallback, bg);
        // Shared adornment colours are not a supplied foreground/background pair.
        // Fall back to the surface text if they disappear on a different surface.
        let secondary = |color: Option<Hsla>, fallback| {
            color
                .filter(|c| contrast(over(*c, bg), bg) >= 3.0)
                .unwrap_or(fallback)
        };
        // Keep muted labels on the main text's side of the surface: a blue bar
        // with white labels must not acquire dark muted text from a light editor.
        let muted = secondary(
            p.muted.filter(|c| {
                (luminance(over(*c, bg)) >= luminance(bg))
                    == (luminance(over(text, bg)) >= luminance(bg))
            }),
            text,
        );
        Self {
            bg,
            text,
            text_muted: muted,
            text_placeholder: secondary(p.placeholder, muted),
            icon: secondary(p.icon, text),
            icon_muted: muted,
            icon_accent: secondary(Some(accent), text),
            error: secondary(p.error, stroke(charts[1], &[bg])),
            values: [
                text,
                stroke(charts[1], &[bg]),
                stroke(charts[2], &[bg]),
                stroke(charts[3], &[bg]),
                foreground(p.muted, text, bg),
            ],
        }
    }
}

fn value_index(kind: crate::data::ValueKind) -> usize {
    use crate::data::ValueKind::*;
    match kind {
        Normal => 0,
        Undef => 1,
        HighImp => 2,
        DontCare => 3,
        Weak => 4,
    }
}
fn opaque(color: Option<Hsla>, fallback: Hsla) -> Hsla {
    alpha(color.filter(|c| c.a > 0.0).unwrap_or(fallback), 1.0)
}
// Respect the host's chosen contrast (including subdued text). Only invisible
// supplied foregrounds and missing tokens need fallback repair.
fn foreground(color: Option<Hsla>, fallback: Hsla, bg: Hsla) -> Hsla {
    color
        .filter(|c| contrast(over(*c, bg), bg) > 1.05)
        .unwrap_or_else(|| fallback_text(fallback, bg))
}
fn fallback_text(color: Hsla, bg: Hsla) -> Hsla {
    if contrast(over(color, bg), bg) >= 4.5 {
        return color;
    }
    if contrast(c(0), bg) > contrast(c(0xffffff), bg) {
        c(0)
    } else {
        c(0xffffff)
    }
}

fn over(fg: Hsla, bg: Hsla) -> Hsla {
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
/// Only thin graphics need a contrast floor. Change HSL lightness alone, preserving
/// hue/saturation, and choose the nearest passing lightness. Called at resolution.
fn stroke(color: Hsla, backgrounds: &[Hsla]) -> Hsla {
    let passes = |color| backgrounds.iter().all(|bg| contrast(color, *bg) >= 3.0);
    if passes(color) {
        return color;
    }
    [0.0, 1.0]
        .into_iter()
        .filter_map(|end| {
            let mut target = color;
            target.l = end;
            if !passes(target) {
                return None;
            }
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..16 {
                let mid = (lo + hi) / 2.0;
                target.l = color.l + (end - color.l) * mid;
                if passes(target) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            target.l = color.l + (end - color.l) * hi;
            Some(target)
        })
        .min_by(|a, b| (a.l - color.l).abs().total_cmp(&(b.l - color.l).abs()))
        .unwrap_or(color)
}

#[cfg(test)]
mod tests;
