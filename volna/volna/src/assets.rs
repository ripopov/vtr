//! GPUI asset source over the icons and fonts bundled in `volna_core`.

use std::borrow::Cow;

use gpui_kit::{App, AssetSource, Result, SharedString};
use volna_core::icons::{FONTS, IconName};

gpui_kit::assets::icon_assets!(
    ComponentAssets,
    [Check, Link, Unlink, Plus, X, Ellipsis, Maximize, Minimize]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match IconName::from_path(path) {
            Some(icon) => Ok(Some(Cow::Borrowed(icon.svg()))),
            None => ComponentAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ComponentAssets.list(path)?;
        paths.extend(
            IconName::ALL
                .iter()
                .map(|icon| icon.path())
                .filter(|p| p.starts_with(path))
                .map(SharedString::from),
        );
        Ok(paths)
    }
}

/// Register the bundled fonts with the text system.
pub(crate) fn load_fonts(cx: &App) -> anyhow::Result<()> {
    cx.text_system()
        .add_fonts(FONTS.iter().map(|f| Cow::Borrowed(f.bytes)).collect())
}
