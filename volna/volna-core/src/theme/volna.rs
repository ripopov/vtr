//! Volna Dark and Volna Light, the default viewer themes. Their colours are
//! the viewer sheet of the design system, `docs/design-system/tokens/viewer.css`,
//! compiled in, so the website and the viewer read one source. Surfaces are
//! the Instrument neutrals; data colours are OKLCH values stored as sRGB; stage
//! fills are computed from the sheet's ladder numbers.

use std::collections::HashMap;

use super::{Appearance, MarkerColors, Surface, Theme, over, stroke};
use crate::color::Color;
use crate::pipeline::{StageStyle, StageSwatch};

/// The design system's viewer tokens (`--viewer-*`), dark block first.
pub const SHEET_CSS: &str = include_str!("../../../../docs/design-system/tokens/viewer.css");

/// How stage fills follow pipeline order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StageLadder {
    /// HSL hue from 250° to 0° at saturation 0.52 and lightness 0.58, with
    /// neighbours alternating to 0.70 beyond eight stages: One Dark and host
    /// palettes.
    Hsl,
    /// OKLCH hue from `hue_from` to `hue_to` in pipeline order at chroma `c`,
    /// lightness always alternating between `l` and `l_alt`; the edge is the
    /// fill with lightness `+ edge_dl`.
    Oklch {
        hue_from: f32,
        hue_to: f32,
        l: f32,
        l_alt: f32,
        c: f32,
        edge_dl: f32,
    },
}

/// Hues of enum and text value tints, clear of the X red and the Z amber.
const TINT_HUES: [f32; 6] = [265.0, 220.0, 175.0, 130.0, 300.0, 340.0];
/// Opacity of a value tint: too faint to compete with a state colour.
const TINT_ALPHA: f32 = 0.22;

/// One appearance block of the sheet: token name (without `--viewer-`) to value.
pub(crate) struct Sheet(HashMap<&'static str, &'static str>);

impl Sheet {
    /// The dark block (`:root,[data-theme="dark"]`) or the light one.
    pub(crate) fn new(dark: bool) -> Self {
        let selector = if dark {
            ":root,[data-theme=\"dark\"]{"
        } else {
            "[data-theme=\"light\"]{"
        };
        let start = SHEET_CSS
            .find(selector)
            .expect("viewer.css has both blocks")
            + selector.len();
        let body = &SHEET_CSS[start..];
        let body = &body[..body.find('}').expect("viewer.css blocks are closed")];
        Self(
            body.split(';')
                .filter_map(|decl| {
                    let (name, value) = decl.trim().split_once(':')?;
                    Some((name.trim().strip_prefix("--viewer-")?, value.trim()))
                })
                .collect(),
        )
    }

    #[cfg(test)]
    pub(crate) fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.keys().copied()
    }

    pub(crate) fn color(&self, name: &str) -> Color {
        let value = self
            .0
            .get(name)
            .unwrap_or_else(|| panic!("viewer.css lacks --viewer-{name}"));
        super::parse_css_color(value)
            .unwrap_or_else(|| panic!("--viewer-{name} is not a colour: {value}"))
    }

    pub(crate) fn number(&self, name: &str) -> f32 {
        let value = self
            .0
            .get(name)
            .unwrap_or_else(|| panic!("viewer.css lacks --viewer-{name}"));
        value
            .parse()
            .unwrap_or_else(|_| panic!("--viewer-{name} is not a number: {value}"))
    }
}

impl Theme<Color> {
    /// Volna Dark (`dark`) or Volna Light, from the design system's viewer sheet.
    pub fn volna(dark: bool) -> Self {
        let s = Sheet::new(dark);
        let col = |name: &str| s.color(name);
        let (text, muted, faint) = (col("text"), col("text-muted"), col("text-faint"));
        let accent = col("accent");
        let on_accent = col("on-accent");
        let (undef, highimp, dontcare, weak) =
            (col("undef"), col("highimp"), col("dontcare"), col("weak"));
        let surface = |bg: Color| Surface {
            bg,
            text,
            text_muted: muted,
            text_placeholder: faint,
            icon: muted,
            icon_muted: faint,
            icon_accent: accent,
            error: undef,
            warning: highimp,
            values: [text, undef, highimp, dontcare, weak],
        };
        let (canvas, panel, hover) = (col("canvas"), col("panel"), col("hover"));
        // A step of the accent toward the text, for a hovered button.
        let accent_hover = over(text.with_alpha(0.14), accent);
        let mut t = Self::one_dark();
        t.appearance = if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        t.editor = surface(canvas);
        t.panel = surface(panel);
        t.bar = surface(col("bar"));
        t.bar_hover = surface(hover);
        t.elevated = surface(col("elevated"));
        t.tooltip = surface(col("elevated"));
        t.input = surface(canvas);
        t.selection = surface(col("selection"));
        t.hover = surface(hover);
        t.menu_hover = surface(hover);
        t.button = Surface {
            text: on_accent,
            icon: on_accent,
            ..surface(accent)
        };
        t.button_hover = Surface {
            bg: accent_hover,
            ..t.button
        };
        t.badge = surface(canvas);
        t.badge_hover = surface(hover);
        t.border = col("border");
        t.border_variant = col("border-subtle");
        t.border_focused = accent;
        t.input_border = col("border");
        t.scrollbar_thumb = text.with_alpha(0.22);
        t.scrollbar_thumb_hover = text.with_alpha(0.4);
        t.wave_signal = col("signal");
        t.wave_high_fill = col("high-fill");
        t.wave_undef = undef;
        t.wave_undef_fill = col("undef-fill");
        t.wave_highimp = highimp;
        t.wave_dontcare = dontcare;
        t.wave_weak = weak;
        t.wave_dense = col("dense");
        t.wave_event_coalesced = col("event");
        t.wave_bus_text = text;
        t.wave_tick = col("grid");
        t.wave_tick_text = faint;
        t.wave_row_selected = col("row-selected");
        t.wave_row_hover = col("row-hover");
        t.wave_outside = col("outside");
        t.wave_cursor = accent;
        t.wave_cursor_inactive = accent.with_alpha(0.45);
        t.wave_cursor_text = on_accent;
        t.tx_relation_in = accent;
        t.tx_relation_out = col("relation-out");
        t.wave_flush = col("flush");
        t.stage_text = col("stage-text");
        t.stages = StageLadder::Oklch {
            hue_from: s.number("stage-hue-from"),
            hue_to: s.number("stage-hue-to"),
            l: s.number("stage-l"),
            l_alt: s.number("stage-l-alt"),
            c: s.number("stage-c"),
            edge_dl: s.number("stage-edge-dl"),
        };
        let marker_text = col("marker-text");
        t.markers = std::array::from_fn(|i| {
            let color = col(&format!("marker-{}", i + 1));
            MarkerColors {
                stroke: color,
                background: color,
                hover: over(text.with_alpha(0.12), color),
                text: marker_text,
                hover_text: marker_text,
            }
        });
        // The row colours take the signal's lightness and chroma in their own
        // hue (blue, cyan, violet, pink) and the grey of weak values, then meet
        // the stroke floor on every row surface.
        let [sl, sa, sb] = t.wave_signal.to_oklab();
        let chroma = sa.hypot(sb);
        t.resolve_tints([
            Color::oklch(sl, chroma, 250.0),
            Color::oklch(sl, chroma, 205.0),
            Color::oklch(sl, chroma, 300.0),
            Color::oklch(sl, chroma, 350.0),
            muted,
        ]);
        let tints = [t.wave_signal, t.tx_relation_out, accent];
        t.sidebar_tints = tints.map(|c| stroke(c, &[t.panel.bg, t.selection.bg, t.hover.bg]));
        t
    }

    /// The colours of a stage swatch.
    pub fn swatch(&self, swatch: StageSwatch) -> StageStyle {
        match swatch {
            StageSwatch::Step { rank, of } => self.stage_style(rank, of),
            StageSwatch::Fallback => self.stage_fallback(),
        }
    }

    /// Stage `k` of `n` in pipeline order.
    pub fn stage_style(&self, k: usize, n: usize) -> StageStyle {
        match self.stages {
            StageLadder::Hsl => {
                let steps = n.saturating_sub(1).max(1) as f32;
                let hue = (250.0 - k as f32 * (250.0 / steps)).rem_euclid(360.0) / 360.0;
                let l = if n > 8 && k % 2 == 1 { 0.70 } else { 0.58 };
                let hsl = |l| Color {
                    h: hue,
                    s: 0.52,
                    l,
                    a: 1.0,
                };
                StageStyle {
                    fill: hsl(l),
                    edge: hsl(l - 0.20),
                    text: self.stage_text,
                }
            }
            StageLadder::Oklch {
                hue_from,
                hue_to,
                l,
                l_alt,
                c,
                edge_dl,
            } => {
                let steps = n.saturating_sub(1).max(1) as f32;
                let hue = (hue_from - (hue_from - hue_to) * k as f32 / steps).rem_euclid(360.0);
                let l = if k % 2 == 1 { l_alt } else { l };
                StageStyle {
                    fill: Color::oklch(l, c, hue),
                    edge: Color::oklch(l + edge_dl, c, hue),
                    text: self.stage_text,
                }
            }
        }
    }

    /// Grey, for stage names outside the ladder and transactions without stages.
    pub fn stage_fallback(&self) -> StageStyle {
        match self.stages {
            StageLadder::Hsl => {
                let grey = |l| Color {
                    h: 0.0,
                    s: 0.0,
                    l,
                    a: 1.0,
                };
                StageStyle {
                    fill: grey(0.58),
                    edge: grey(0.38),
                    text: self.stage_text,
                }
            }
            StageLadder::Oklch { l_alt, edge_dl, .. } => StageStyle {
                fill: Color::oklch(l_alt, 0.0, 0.0),
                edge: Color::oklch(l_alt + edge_dl, 0.0, 0.0),
                text: self.stage_text,
            },
        }
    }

    /// A stable tint for an enum or text value: FNV-1a of its text picks one
    /// of six hues at the stage ladder's lightness and chroma.
    pub fn value_tint(&self, value: &str) -> Color {
        let mut h: u32 = 2_166_136_261;
        for ch in value.chars() {
            h ^= ch as u32;
            h = h.wrapping_mul(16_777_619);
        }
        let hue = TINT_HUES[h as usize % TINT_HUES.len()];
        let (l, c) = match self.stages {
            StageLadder::Oklch { l, c, .. } => (l, c),
            StageLadder::Hsl if self.appearance.is_dark() => (0.80, 0.10),
            StageLadder::Hsl => (0.90, 0.08),
        };
        Color::oklch(l, c, hue).with_alpha(TINT_ALPHA)
    }
}

#[cfg(test)]
mod tests;
