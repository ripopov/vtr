//! Host-independent input. Missing colours fall back independently; pairs stay together.
use gpui::Hsla;
use serde::{Deserialize, Deserializer};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ColorPair {
    #[serde(deserialize_with = "optional_color")]
    pub background: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub foreground: Option<Hsla>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
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
    #[serde(deserialize_with = "optional_color")]
    pub button_hover: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub border: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub panel_border: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub input_border: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub focus: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub contrast_border: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub muted: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub placeholder: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub icon: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub accent: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub error: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub scrollbar: Option<Hsla>,
    #[serde(deserialize_with = "optional_color")]
    pub scrollbar_hover: Option<Hsla>,
    /// Green, red, yellow, blue, orange, purple. Alpha becomes coverage, not a tint.
    #[serde(deserialize_with = "chart_colors")]
    pub charts: [Option<Hsla>; 6],
    #[serde(deserialize_with = "optional_color")]
    pub cursor: Option<Hsla>,
}

impl HostPalette {
    /// Host-neutral JSON: optional surface pairs and CSS colour strings.
    /// Unknown fields are ignored; malformed supplied colours are rejected.
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        if !value.is_object() {
            return Err(serde::de::Error::custom("theme must be a JSON object"));
        }
        serde_json::from_value(value)
    }
}

fn css_color<E: serde::de::Error>(value: Option<String>) -> Result<Option<Hsla>, E> {
    value
        .map(|s| {
            super::parse_css_color(&s)
                .ok_or_else(|| E::custom(format!("invalid CSS colour: {s:?}")))
        })
        .transpose()
}

fn optional_color<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Hsla>, D::Error> {
    css_color(Option::<String>::deserialize(d)?)
}

fn chart_colors<'de, D: Deserializer<'de>>(d: D) -> Result<[Option<Hsla>; 6], D::Error> {
    let values = <[Option<String>; 6]>::deserialize(d)?;
    let mut colors = [None; 6];
    for (slot, value) in colors.iter_mut().zip(values) {
        *slot = css_color(value)?;
    }
    Ok(colors)
}
