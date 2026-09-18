//! Resolved design tokens. Host colours are resolved once, never while painting a row.
//!
//! [`Theme`] is generic over its colour type so a frontend can map the resolved
//! palette into its own colour representation once (`theme.map(...)`) and keep
//! reading the same field names. All resolution logic runs on the core
//! [`Color`] type; metrics are logical pixels.
pub mod builtin;
mod palette;
pub use builtin::{BUILTIN, Builtin};
pub use palette::{Appearance, ColorPair, HostPalette};
mod color;
pub use color::parse_css_color;
pub mod vscode;

use crate::color::Color;
use crate::data::ValueKind;
use crate::scene::FontRole;

/// A resolved surface or interaction state. Cheap to copy into closures.
#[derive(Clone, Copy, Debug)]
pub struct Surface<C = Color> {
    pub bg: C,
    pub text: C,
    pub text_muted: C,
    pub text_placeholder: C,
    pub icon: C,
    pub icon_muted: C,
    pub icon_accent: C,
    pub error: C,
    values: [C; 5],
}

impl<C: Copy> Surface<C> {
    pub fn value_color(self, kind: ValueKind) -> C {
        self.values[value_index(kind)]
    }

    pub fn map<D: Copy>(self, f: &impl Fn(C) -> D) -> Surface<D> {
        Surface {
            bg: f(self.bg),
            text: f(self.text),
            text_muted: f(self.text_muted),
            text_placeholder: f(self.text_placeholder),
            icon: f(self.icon),
            icon_muted: f(self.icon_muted),
            icon_accent: f(self.icon_accent),
            error: f(self.error),
            values: self.values.map(f),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MarkerColors<C = Color> {
    pub stroke: C,
    pub background: C,
    pub hover: C,
    pub text: C,
    pub hover_text: C,
}

impl<C: Copy> MarkerColors<C> {
    pub fn map<D: Copy>(self, f: &impl Fn(C) -> D) -> MarkerColors<D> {
        MarkerColors {
            stroke: f(self.stroke),
            background: f(self.background),
            hover: f(self.hover),
            text: f(self.text),
            hover_text: f(self.hover_text),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme<C = Color> {
    pub appearance: Appearance,
    pub editor: Surface<C>,
    pub panel: Surface<C>,
    pub bar: Surface<C>,
    pub bar_hover: Surface<C>,
    pub elevated: Surface<C>,
    pub tooltip: Surface<C>,
    pub input: Surface<C>,
    pub selection: Surface<C>,
    pub hover: Surface<C>,
    pub menu_hover: Surface<C>,
    pub button: Surface<C>,
    pub button_hover: Surface<C>,
    pub badge: Surface<C>,
    pub badge_hover: Surface<C>,
    pub border: C,
    pub border_variant: C,
    pub border_focused: C,
    pub input_border: C,
    pub scrollbar_thumb: C,
    pub scrollbar_thumb_hover: C,
    // Waveforms
    pub wave_signal: C,
    pub wave_high_fill: C,
    pub wave_undef: C,
    pub wave_highimp: C,
    pub wave_dontcare: C,
    pub wave_weak: C,
    pub wave_dense: C,
    /// Arrow colour when multiple event occurrences share a pixel.
    pub wave_event_coalesced: C,
    pub wave_bus_text: C,
    pub wave_tick: C,
    pub wave_tick_text: C,
    pub wave_row_selected: C,
    pub wave_row_hover: C,
    pub wave_cursor: C,
    pub wave_cursor_inactive: C,
    pub wave_cursor_text: C,
    pub markers: [MarkerColors<C>; 6],

    // Typography (font families are the bundled faces, see `icons::FONTS`)
    pub ui_font: &'static str,
    pub mono_font: &'static str,
    pub ui_size: f32,
    pub ui_size_small: f32,
    pub mono_size: f32,

    // Metrics in logical pixels (multiples of the 4px grid)
    pub row_height: f32,
    pub header_height: f32,
    pub titlebar_height: f32,
    pub statusbar_height: f32,
    pub timeline_height: f32,
    pub icon_size: f32,
    pub splitter_grab: f32,
    /// The interface zoom the metrics above already include (1.0 = the
    /// design sizes). Painters multiply their own pixel constants by it.
    pub zoom: f32,
}

fn c(hex: u32) -> Color {
    Color::rgb(hex)
}
fn ca(hex: u32, a: f32) -> Color {
    c(hex).with_alpha(a)
}

impl<C: Copy> Theme<C> {
    pub fn row(&self, selected: bool, hovered: bool) -> Surface<C> {
        if selected {
            self.selection
        } else if hovered {
            self.hover
        } else {
            self.panel
        }
    }

    pub fn value_color(&self, kind: ValueKind) -> C {
        [
            self.wave_signal,
            self.wave_undef,
            self.wave_highimp,
            self.wave_dontcare,
            self.wave_weak,
        ][value_index(kind)]
    }

    pub fn marker(&self, index: usize) -> MarkerColors<C> {
        self.markers[index % self.markers.len()]
    }

    /// Family name for a font role.
    pub fn font_family(&self, role: FontRole) -> &'static str {
        match role {
            FontRole::Mono => self.mono_font,
            _ => self.ui_font,
        }
    }

    /// Convert every colour, keeping metrics and fonts.
    pub fn map<D: Copy>(&self, f: impl Fn(C) -> D) -> Theme<D> {
        let f = &f;
        Theme {
            appearance: self.appearance,
            editor: self.editor.map(f),
            panel: self.panel.map(f),
            bar: self.bar.map(f),
            bar_hover: self.bar_hover.map(f),
            elevated: self.elevated.map(f),
            tooltip: self.tooltip.map(f),
            input: self.input.map(f),
            selection: self.selection.map(f),
            hover: self.hover.map(f),
            menu_hover: self.menu_hover.map(f),
            button: self.button.map(f),
            button_hover: self.button_hover.map(f),
            badge: self.badge.map(f),
            badge_hover: self.badge_hover.map(f),
            border: f(self.border),
            border_variant: f(self.border_variant),
            border_focused: f(self.border_focused),
            input_border: f(self.input_border),
            scrollbar_thumb: f(self.scrollbar_thumb),
            scrollbar_thumb_hover: f(self.scrollbar_thumb_hover),
            wave_signal: f(self.wave_signal),
            wave_high_fill: f(self.wave_high_fill),
            wave_undef: f(self.wave_undef),
            wave_highimp: f(self.wave_highimp),
            wave_dontcare: f(self.wave_dontcare),
            wave_weak: f(self.wave_weak),
            wave_dense: f(self.wave_dense),
            wave_event_coalesced: f(self.wave_event_coalesced),
            wave_bus_text: f(self.wave_bus_text),
            wave_tick: f(self.wave_tick),
            wave_tick_text: f(self.wave_tick_text),
            wave_row_selected: f(self.wave_row_selected),
            wave_row_hover: f(self.wave_row_hover),
            wave_cursor: f(self.wave_cursor),
            wave_cursor_inactive: f(self.wave_cursor_inactive),
            wave_cursor_text: f(self.wave_cursor_text),
            markers: self.markers.map(|m| m.map(f)),
            ui_font: self.ui_font,
            mono_font: self.mono_font,
            ui_size: self.ui_size,
            ui_size_small: self.ui_size_small,
            mono_size: self.mono_size,
            row_height: self.row_height,
            header_height: self.header_height,
            titlebar_height: self.titlebar_height,
            statusbar_height: self.statusbar_height,
            timeline_height: self.timeline_height,
            icon_size: self.icon_size,
            splitter_grab: self.splitter_grab,
            zoom: self.zoom,
        }
    }

    /// The same theme at interface zoom `factor`: every font size and metric
    /// is the design size times `factor`, whatever zoom `self` already has.
    pub fn zoomed(&self, factor: f32) -> Self
    where
        C: Copy,
    {
        let factor = if factor.is_finite() && factor > 0.0 {
            factor
        } else {
            1.0
        };
        let f = factor / self.zoom;
        let mut t = self.map(|c| c);
        t.ui_size *= f;
        t.ui_size_small *= f;
        t.mono_size *= f;
        t.row_height *= f;
        t.header_height *= f;
        t.titlebar_height *= f;
        t.statusbar_height *= f;
        t.timeline_height *= f;
        t.icon_size *= f;
        t.splitter_grab *= f;
        t.zoom = factor;
        t
    }

    /// A design-time pixel size at this theme's zoom.
    pub fn scale(&self, px: f32) -> f32 {
        px * self.zoom
    }
}

impl Theme<Color> {
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
            wave_event_coalesced: c(0xe5c07b),
            wave_bus_text: c(0xdce0e5),
            wave_tick: ca(0xc8ccd4, 0.08),
            wave_tick_text: c(0xa9afbc),
            wave_row_selected: ca(0x74ade8, 0.10),
            wave_row_hover: ca(0xc8ccd4, 0.04),
            wave_cursor: c(0x74ade8),
            wave_cursor_inactive: c(0x74ade8).with_alpha(0.45),
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
            ui_size: 13.0,
            ui_size_small: 11.0,
            mono_size: 12.0,

            row_height: 24.0,
            header_height: 32.0,
            titlebar_height: 32.0,
            statusbar_height: 24.0,
            timeline_height: 32.0,
            icon_size: 16.0,
            splitter_grab: 8.0,
            zoom: 1.0,
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
        let charts: [Color; 6] = std::array::from_fn(|i| opaque(p.charts[i], c(fallback[i])));
        let resolve = |pair, bg, fg| Surface::resolve(pair, bg, fg, p, accent, charts);
        t.appearance = p.appearance;
        t.editor = resolve(p.editor, bg, fg);
        t.panel = resolve(p.panel, t.editor.bg, t.editor.text);
        t.bar = resolve(p.bar, t.panel.bg, t.panel.text);
        t.bar_hover = resolve(
            ColorPair::default(),
            over(t.bar.text.with_alpha(0.1), t.bar.bg),
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
            with_bg(p.selection, over(accent.with_alpha(0.25), t.panel.bg)),
            t.panel.bg,
            t.panel.text,
        );
        t.hover = resolve(
            with_bg(p.hover, over(t.panel.text.with_alpha(0.08), t.panel.bg)),
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
            over(t.input.text.with_alpha(0.15), t.input.bg),
            t.input.text,
        );
        t.border = p
            .border
            .unwrap_or(over(t.panel.text.with_alpha(0.25), t.panel.bg));
        t.border_variant = p.panel_border.unwrap_or(t.border);
        t.border_focused = p.focus.unwrap_or(accent);
        t.input_border = p.input_border.unwrap_or(t.border);
        if hc {
            t.border = stroke(p.contrast_border.unwrap_or(t.border), &[t.panel.bg]);
            t.border_variant = t.border;
            t.border_focused = stroke(t.border_focused, &[t.panel.bg]);
            t.input_border = stroke(p.contrast_border.unwrap_or(t.input_border), &[t.input.bg]);
        }
        t.scrollbar_thumb = p.scrollbar.unwrap_or(t.panel.text.with_alpha(0.4));
        t.scrollbar_thumb_hover = p.scrollbar_hover.unwrap_or(t.panel.text.with_alpha(0.7));
        if hc {
            t.scrollbar_thumb = stroke(over(t.scrollbar_thumb, t.panel.bg), &[t.panel.bg]);
            t.scrollbar_thumb_hover =
                stroke(over(t.scrollbar_thumb_hover, t.panel.bg), &[t.panel.bg]);
        }
        t.wave_row_selected = accent.with_alpha(if hc { 0.08 } else { 0.10 });
        t.wave_row_hover = t.editor.text.with_alpha(0.04);
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
        t.wave_high_fill = t.wave_signal.with_alpha(0.10);
        t.wave_dense = t.wave_signal.with_alpha(if hc { 0.8 } else { 0.55 });
        t.wave_event_coalesced = stroke(charts[3], &backgrounds);
        let hue_gap = (t.wave_event_coalesced.h - t.wave_signal.h).abs();
        if hue_gap.min(1.0 - hue_gap) < 0.08
            && (t.wave_event_coalesced.l - t.wave_signal.l).abs() < 0.15
        {
            t.wave_event_coalesced = stroke(
                Color {
                    h: (t.wave_signal.h + 0.5) % 1.0,
                    s: 0.75,
                    l: 0.5,
                    a: 1.0,
                },
                &backgrounds,
            );
        }
        t.wave_bus_text = t.editor.text;
        t.wave_tick = t.editor.text.with_alpha(if hc { 0.4 } else { 0.12 });
        t.wave_tick_text = t.panel.text_muted;
        t.wave_cursor = stroke(opaque(p.cursor, accent), &backgrounds);
        t.wave_cursor_inactive = t.wave_cursor.with_alpha(0.45);
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
}

impl Surface<Color> {
    fn resolve(
        pair: ColorPair,
        parent: Color,
        fallback: Color,
        p: &HostPalette,
        accent: Color,
        charts: [Color; 6],
    ) -> Self {
        let bg = over(pair.background.unwrap_or(parent), parent);
        let text = foreground(pair.foreground, fallback, bg);
        // Shared adornment colours are not a supplied foreground/background pair.
        // Fall back to the surface text if they disappear on a different surface.
        let secondary = |color: Option<Color>, fallback| {
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

fn value_index(kind: ValueKind) -> usize {
    use ValueKind::*;
    match kind {
        Normal => 0,
        Undef => 1,
        HighImp => 2,
        DontCare => 3,
        Weak => 4,
    }
}
fn opaque(color: Option<Color>, fallback: Color) -> Color {
    color
        .filter(|c| c.a > 0.0)
        .unwrap_or(fallback)
        .with_alpha(1.0)
}
// Respect the host's chosen contrast (including subdued text). Only invisible
// supplied foregrounds and missing tokens need fallback repair.
fn foreground(color: Option<Color>, fallback: Color, bg: Color) -> Color {
    color
        .filter(|c| contrast(over(*c, bg), bg) > 1.05)
        .unwrap_or_else(|| fallback_text(fallback, bg))
}
fn fallback_text(color: Color, bg: Color) -> Color {
    if contrast(over(color, bg), bg) >= 4.5 {
        return color;
    }
    if contrast(c(0), bg) > contrast(c(0xffffff), bg) {
        c(0)
    } else {
        c(0xffffff)
    }
}

/// Composite `fg` over an opaque `bg`.
pub fn over(fg: Color, bg: Color) -> Color {
    if fg.a == 1.0 {
        return fg;
    }
    let f = fg.to_rgba();
    let b = bg.to_rgba();
    Color::from_rgba(
        f[0] * f[3] + b[0] * (1.0 - f[3]),
        f[1] * f[3] + b[1] * (1.0 - f[3]),
        f[2] * f[3] + b[2] * (1.0 - f[3]),
        1.0,
    )
}
pub(crate) fn luminance(color: Color) -> f32 {
    let c = color.to_rgba();
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}
/// WCAG contrast ratio of two opaque colours.
pub fn contrast(a: Color, b: Color) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
/// Only thin graphics need a contrast floor. Change HSL lightness alone, preserving
/// hue/saturation, and choose the nearest passing lightness. Called at resolution.
fn stroke(color: Color, backgrounds: &[Color]) -> Color {
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
