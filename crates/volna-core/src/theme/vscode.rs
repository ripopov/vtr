//! The only VS Code colour mapping. Input is the webview's resolved CSS snapshot:
//! `kind=vscode-dark` followed by `--vscode-<id>=<CSS colour>` lines.
//! Real webviews and checked-in fixtures use the same parser and fallback rules.
use std::collections::HashMap;

use super::{Appearance, ColorPair, HostPalette, parse_css_color};

pub fn host_palette(snapshot: &str) -> HostPalette {
    let vars: HashMap<_, _> = snapshot
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.trim().trim_start_matches("--"), value.trim()))
        })
        .collect();
    let color = |ids: &[&str]| {
        ids.iter().find_map(|id| {
            let key = format!("vscode-{}", id.replace('.', "-"));
            parse_css_color(vars.get(key.as_str())?)
        })
    };
    let pair = |background: &[&str], foreground: &[&str]| ColorPair {
        background: color(background),
        foreground: color(foreground),
    };
    let appearance = match vars.get("kind").copied().unwrap_or_default() {
        "vscode-light" => Appearance::Light,
        "vscode-dark" => Appearance::Dark,
        "vscode-high-contrast-light" => Appearance::HighContrastLight,
        "vscode-high-contrast" => Appearance::HighContrastDark,
        _ => match color(&["editor.background"]) {
            Some(bg) if bg.a > 0.0 && super::luminance(bg) > 0.18 => Appearance::Light,
            _ => Appearance::Dark,
        },
    };
    HostPalette {
        appearance,
        editor: pair(&["editor.background"], &["editor.foreground", "foreground"]),
        panel: pair(
            &["sideBar.background"],
            &["sideBar.foreground", "foreground"],
        ),
        bar: pair(
            &["statusBar.background", "sideBar.background"],
            &["statusBar.foreground", "sideBar.foreground", "foreground"],
        ),
        elevated: pair(
            &["menu.background", "editorWidget.background"],
            &["menu.foreground", "editorWidget.foreground", "foreground"],
        ),
        tooltip: pair(
            &["editorHoverWidget.background", "editorWidget.background"],
            &[
                "editorHoverWidget.foreground",
                "editorWidget.foreground",
                "foreground",
            ],
        ),
        input: pair(&["input.background"], &["input.foreground", "foreground"]),
        selection: pair(
            &["list.activeSelectionBackground"],
            &["list.activeSelectionForeground"],
        ),
        hover: pair(&["list.hoverBackground"], &["list.hoverForeground"]),
        button: pair(&["button.background"], &["button.foreground"]),
        menu_hover: pair(&["menu.selectionBackground"], &["menu.selectionForeground"]),
        button_hover: color(&["button.hoverBackground"]),
        border: color(&["panel.border", "editorGroup.border", "editorWidget.border"]),
        panel_border: color(&["sideBar.border"]),
        input_border: color(&["input.border"]),
        focus: if appearance.is_high_contrast() {
            color(&["contrastActiveBorder", "focusBorder"])
        } else {
            color(&["focusBorder"])
        },
        contrast_border: color(&["contrastBorder"]),
        muted: color(&["descriptionForeground"]),
        placeholder: color(&["input.placeholderForeground", "disabledForeground"]),
        icon: color(&["icon.foreground"]),
        accent: color(&["textLink.foreground", "focusBorder", "button.background"]),
        error: color(&["errorForeground", "editorError.foreground"]),
        scrollbar: color(&["scrollbarSlider.background"]),
        scrollbar_hover: color(&["scrollbarSlider.hoverBackground"]),
        charts: [
            color(&["charts.green", "terminal.ansiGreen"]),
            color(&["charts.red", "editorError.foreground", "terminal.ansiRed"]),
            color(&[
                "charts.yellow",
                "editorWarning.foreground",
                "terminal.ansiYellow",
            ]),
            color(&["charts.blue", "editorInfo.foreground", "terminal.ansiBlue"]),
            color(&["charts.orange", "terminal.ansiBrightYellow"]),
            color(&["charts.purple", "terminal.ansiMagenta"]),
        ],
        cursor: color(&["editorCursor.foreground", "focusBorder"]),
    }
}
