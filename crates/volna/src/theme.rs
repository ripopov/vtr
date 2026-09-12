//! GPUI's view of the core theme: the resolved palette mapped to `Hsla` once
//! when a theme is installed. All resolution (One Dark, host palettes, VS Code
//! snapshots) lives in `volna_core::theme`; this module only converts.

use gpui::{App, Global, Hsla};

pub use volna_core::theme::{Appearance, HostPalette, vscode};

/// A resolved surface with GPUI colours.
pub type Surface = volna_core::theme::Surface<Hsla>;
/// Marker colours with GPUI colours.
pub type MarkerColors = volna_core::theme::MarkerColors<Hsla>;

/// The core theme with GPUI colours. Field names match the core theme.
pub type Theme = volna_core::theme::Theme<Hsla>;
/// The toolkit-neutral theme the painter uses.
pub type CoreTheme = volna_core::theme::Theme;

struct ThemeGlobal {
    core: CoreTheme,
    gpui: Theme,
}

impl Global for ThemeGlobal {}

pub fn hsla(c: volna_core::Color) -> Hsla {
    Hsla {
        h: c.h,
        s: c.s,
        l: c.l,
        a: c.a,
    }
}

/// Make `core` the current theme without repainting (start-up).
pub fn set(core: CoreTheme, cx: &mut App) {
    cx.set_global(ThemeGlobal {
        gpui: core.map(hsla),
        core,
    });
}

/// Presentation only; invalidate cached children without rebuilding viewer state.
pub fn install(core: CoreTheme, cx: &mut App) {
    set(core, cx);
    cx.refresh_windows();
}

pub fn theme(cx: &App) -> &Theme {
    &cx.global::<ThemeGlobal>().gpui
}

pub fn core_theme(cx: &App) -> &CoreTheme {
    &cx.global::<ThemeGlobal>().core
}
