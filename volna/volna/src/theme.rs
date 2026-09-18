//! GPUI's view of the core theme: the resolved palette mapped to `Hsla` once
//! when a theme is installed. All resolution (One Dark, host palettes, VS Code
//! snapshots) lives in `volna_core::theme`; this module only converts.

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Pixels, px};

pub use volna_core::theme::{Appearance, HostPalette, vscode};

/// A resolved surface with GPUI colours.
pub type Surface = volna_core::theme::Surface<Hsla>;

/// The core theme with GPUI colours. Field names match the core theme.
pub type Theme = volna_core::theme::Theme<Hsla>;
/// The toolkit-neutral theme the painter uses.
pub type CoreTheme = volna_core::theme::Theme;

struct ThemeGlobal {
    /// The installed palette at its design sizes (zoom 1.0).
    base: CoreTheme,
    /// `appearance.zoom`; `core` and `gpui` are `base` at this zoom.
    zoom: f32,
    core: CoreTheme,
    gpui: Theme,
}

impl Global for ThemeGlobal {}

/// Design-time pixel sizes at the current interface zoom, for the chrome
/// Volna lays out itself (`t.px(12.0)` instead of `px(12.0)`). The theme's
/// own metrics (`row_height`, `ui_size`, …) are already zoomed.
pub trait ThemePx {
    fn px(&self, design_px: f32) -> Pixels;
}

impl<C: Copy> ThemePx for volna_core::theme::Theme<C> {
    fn px(&self, design_px: f32) -> Pixels {
        px(self.scale(design_px))
    }
}

pub fn hsla(c: volna_core::Color) -> Hsla {
    Hsla {
        h: c.h,
        s: c.s,
        l: c.l,
        a: c.a,
    }
}

/// Make `core` the current theme without repainting (start-up). The
/// interface zoom already installed is kept.
pub fn set(core: CoreTheme, cx: &mut App) {
    let zoom = cx.try_global::<ThemeGlobal>().map_or(1.0, |g| g.zoom);
    set_zoomed(core, zoom, cx);
}

/// Install `base` scaled by `zoom`. gpui-kit's defaults are a light palette;
/// first adopt its own dark or light base for the appearance, then project
/// every token the chrome reads (buttons, tabs, sidebar, group boxes, inputs,
/// switches, lists, menus) from the core surfaces, so components Volna does
/// not restyle still match. gpui-kit's `Root` sets the window's rem size from
/// the component theme's `font_size`, so every rem-based size in its chrome
/// (the settings editor, inputs, menus, dialogs, the palette) follows the
/// zoomed UI font size; Volna's own chrome reads the zoomed metrics and
/// [`ThemePx`].
fn set_zoomed(base: CoreTheme, zoom: f32, cx: &mut App) {
    let core = base.zoomed(zoom);
    let zoom = core.zoom;
    let t = core.map(hsla);
    let mode = if t.appearance.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    ComponentTheme::change(mode, None, cx);
    let kit = ComponentTheme::global_mut(cx);
    kit.mode = mode;
    kit.font_family = t.ui_font.into();
    kit.font_size = px(t.ui_size);
    kit.mono_font_family = t.mono_font.into();
    kit.mono_font_size = px(t.ui_size);
    kit.radius = px(4.0);
    kit.focus_ring = false;
    // Surfaces
    kit.background = t.panel.bg;
    kit.foreground = t.panel.text;
    kit.muted = t.hover.bg;
    kit.muted_foreground = t.panel.text_muted;
    kit.secondary = t.badge_hover.bg;
    kit.secondary_foreground = t.panel.text;
    kit.secondary_hover = t.hover.bg;
    kit.secondary_active = t.selection.bg;
    kit.accent = t.menu_hover.bg;
    kit.accent_foreground = t.menu_hover.text;
    kit.primary = t.button.bg;
    kit.primary_foreground = t.button.text;
    kit.primary_hover = t.button_hover.bg;
    kit.primary_active = t.button.bg;
    kit.danger = t.panel.error;
    kit.border = t.border;
    kit.ring = t.border_focused;
    kit.selection = t.selection.bg;
    kit.caret = t.panel.text;
    kit.link = t.panel.icon_accent;
    kit.link_hover = t.panel.icon_accent;
    kit.link_active = t.panel.icon_accent;
    kit.popover = t.elevated.bg;
    kit.popover_foreground = t.elevated.text;
    kit.tiles = t.editor.bg;
    kit.window_border = t.border;
    kit.drag_border = t.border_focused;
    kit.drop_target = t.selection.bg;
    kit.skeleton = t.hover.bg;
    kit.scrollbar = gpui_kit::transparent_black();
    kit.scrollbar_thumb = t.scrollbar_thumb;
    kit.scrollbar_thumb_hover = t.scrollbar_thumb_hover;
    // Buttons
    kit.button = t.badge.bg;
    kit.button_foreground = t.panel.text;
    kit.button_hover = t.hover.bg;
    kit.button_active = t.selection.bg;
    kit.button_primary = t.button.bg;
    kit.button_primary_foreground = t.button.text;
    kit.button_primary_hover = t.button_hover.bg;
    kit.button_primary_active = t.button.bg;
    kit.button_secondary = t.badge_hover.bg;
    kit.button_secondary_foreground = t.panel.text;
    kit.button_secondary_hover = t.hover.bg;
    kit.button_secondary_active = t.selection.bg;
    // Bars and tabs
    kit.title_bar = t.bar.bg;
    kit.title_bar_border = t.border;
    kit.status_bar = t.bar.bg;
    kit.status_bar_border = t.border;
    kit.tab = t.bar.bg;
    kit.tab_foreground = t.bar.text_muted;
    kit.tab_active = t.editor.bg;
    kit.tab_active_foreground = t.editor.text;
    kit.tab_bar = t.bar.bg;
    kit.tab_bar_segmented = t.bar.bg;
    // Sidebar, group boxes, lists and tables (the settings editor)
    kit.sidebar = t.panel.bg;
    kit.sidebar_foreground = t.panel.text;
    kit.sidebar_border = t.border;
    kit.sidebar_accent = t.menu_hover.bg;
    kit.sidebar_accent_foreground = t.menu_hover.text;
    kit.sidebar_primary = t.button.bg;
    kit.sidebar_primary_foreground = t.button.text;
    kit.group_box = t.editor.bg;
    kit.group_box_foreground = t.editor.text;
    kit.accordion = t.panel.bg;
    kit.description_list_label = t.badge.bg;
    kit.description_list_label_foreground = t.panel.text_muted;
    kit.colors.list = t.panel.bg;
    kit.colors.list_even = t.panel.bg;
    kit.colors.list_head = t.bar.bg;
    kit.colors.list_hover = t.hover.bg;
    kit.colors.list_active = t.selection.bg;
    kit.colors.list_active_border = t.border_focused;
    kit.table = t.panel.bg;
    kit.table_even = t.panel.bg;
    kit.table_head = t.bar.bg;
    kit.table_head_foreground = t.bar.text_muted;
    kit.table_foot = t.bar.bg;
    kit.table_foot_foreground = t.bar.text_muted;
    kit.table_hover = t.hover.bg;
    kit.table_active = t.selection.bg;
    kit.table_active_border = t.border_focused;
    kit.table_row_border = t.border_variant;
    // Controls
    kit.input = t.input_border;
    kit.switch = t.border;
    kit.switch_thumb = t.panel.text;
    kit.slider_bar = t.button.bg;
    kit.slider_thumb = t.panel.text;
    kit.progress_bar = t.button.bg;
    kit.tokens = kit.colors.into();
    ComponentTheme::sync_base(cx);
    cx.set_global(ThemeGlobal {
        base,
        zoom,
        gpui: t,
        core,
    });
}

/// Presentation only; invalidate cached children without rebuilding viewer state.
pub fn install(core: CoreTheme, cx: &mut App) {
    set(core, cx);
    cx.refresh_windows();
}

/// Apply `appearance.zoom`: re-project the installed palette at `zoom` and
/// repaint. Like a theme change, viewer state is untouched.
pub fn set_zoom(zoom: f32, cx: &mut App) {
    let base = cx.global::<ThemeGlobal>().base;
    set_zoomed(base, zoom, cx);
    cx.refresh_windows();
}

pub fn theme(cx: &App) -> &Theme {
    &cx.global::<ThemeGlobal>().gpui
}

pub fn core_theme(cx: &App) -> &CoreTheme {
    &cx.global::<ThemeGlobal>().core
}
