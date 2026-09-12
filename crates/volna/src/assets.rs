//! GPUI asset source over the icons and fonts bundled in `volna_core`.

use std::borrow::Cow;

use gpui::{App, AssetSource, Result, SharedString};
use volna_core::icons::{FONTS, IconName};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(IconName::from_path(path).map(|icon| Cow::Borrowed(icon.svg())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(IconName::ALL
            .iter()
            .map(|icon| icon.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

/// Register the bundled fonts with the text system.
pub fn load_fonts(cx: &App) -> anyhow::Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|f| Cow::Borrowed(f.bytes)).collect())
}
