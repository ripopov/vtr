//! GPUI's view of the core theme: the resolved palette mapped to `Hsla` once
//! when a theme is installed. All resolution (One Dark, host palettes, VS Code
//! snapshots) lives in `volna_core::theme`; this module only converts.

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};
use gpui_kit::{App, Global, Hsla, px};

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
    let t = core.map(hsla);
    let kit = ComponentTheme::global_mut(cx);
    kit.mode = if t.appearance.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    kit.font_family = t.ui_font.into();
    kit.font_size = px(t.ui_size);
    kit.mono_font_family = t.mono_font.into();
    kit.mono_font_size = px(t.ui_size);
    kit.radius = px(4.0);
    kit.focus_ring = false;
    kit.background = t.panel.bg;
    kit.foreground = t.panel.text;
    kit.muted_foreground = t.panel.text_muted;
    kit.border = t.border;
    kit.ring = t.border_focused;
    kit.selection = t.selection.bg;
    kit.popover = t.elevated.bg;
    kit.popover_foreground = t.elevated.text;
    kit.accent = t.menu_hover.bg;
    kit.accent_foreground = t.menu_hover.text;
    kit.button_primary = t.button.bg;
    kit.button_primary_foreground = t.button.text;
    kit.button_primary_hover = t.button_hover.bg;
    kit.button_primary_active = t.button.bg;
    kit.button_secondary = t.button.bg;
    kit.button_secondary_foreground = t.button.text;
    kit.button_secondary_hover = t.button_hover.bg;
    kit.button_secondary_active = t.button.bg;
    kit.tab = t.bar.bg;
    kit.tab_foreground = t.bar.text_muted;
    kit.tab_active = t.editor.bg;
    kit.tab_active_foreground = t.editor.text;
    kit.tab_bar = t.bar.bg;
    kit.drag_border = t.border_focused;
    kit.tokens = kit.colors.into();
    ComponentTheme::sync_base(cx);
    cx.set_global(ThemeGlobal { gpui: t, core });
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
