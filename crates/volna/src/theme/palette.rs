//! Host-independent input. Missing colours fall back independently; pairs stay together.
use gpui::Hsla;

#[cfg_attr(target_family = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    Light,
    #[default]
    Dark,
    HighContrastDark,
    HighContrastLight,
}

impl Appearance {
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark | Self::HighContrastDark)
    }
    pub fn is_high_contrast(self) -> bool {
        matches!(self, Self::HighContrastDark | Self::HighContrastLight)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ColorPair {
    pub background: Option<Hsla>,
    pub foreground: Option<Hsla>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HostPalette {
    pub appearance: Appearance,
    pub editor: ColorPair,
    pub panel: ColorPair,
    pub bar: ColorPair,
    pub elevated: ColorPair,
    pub tooltip: ColorPair,
    pub input: ColorPair,
    pub selection: ColorPair,
    pub hover: ColorPair,
    pub button: ColorPair,
    pub menu_hover: ColorPair,
    pub button_hover: Option<Hsla>,
    pub border: Option<Hsla>,
    pub panel_border: Option<Hsla>,
    pub input_border: Option<Hsla>,
    pub focus: Option<Hsla>,
    pub contrast_border: Option<Hsla>,
    pub muted: Option<Hsla>,
    pub placeholder: Option<Hsla>,
    pub icon: Option<Hsla>,
    pub accent: Option<Hsla>,
    pub error: Option<Hsla>,
    pub scrollbar: Option<Hsla>,
    pub scrollbar_hover: Option<Hsla>,
    /// Green, red, yellow, blue, orange, purple. Alpha becomes coverage, not a tint.
    pub charts: [Option<Hsla>; 6],
    pub cursor: Option<Hsla>,
}
