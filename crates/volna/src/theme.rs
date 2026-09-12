//! Design tokens. Every colour, font and metric used by the viewer lives here,
//! derived from Zed's "One Dark" theme. Components never hard-code values.

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
