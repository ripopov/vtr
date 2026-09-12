//! Assets compiled into the binary so native and wasm builds render identically.

use std::borrow::Cow;

use gpui::{App, AssetSource, Result, SharedString};

pub struct Assets;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $( (concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg"))), )*
        ];
    };
}

icons!(
    "activity",
    "audio-waveform",
    "binary",
    "box",
    "braces",
    "chevron-down",
    "chevron-right",
    "chevrons-left",
    "chevrons-right",
    "circle-dot",
    "folder",
    "folder-open",
    "loader-circle",
    "locate",
    "panel-left",
    "plus",
    "search",
    "sigma",
    "triangle-alert",
    "type",
    "x",
);

const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Lilex-Regular.ttf"),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// Register the bundled fonts with the text system.
pub fn load_fonts(cx: &App) -> anyhow::Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect())
}
