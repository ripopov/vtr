use super::*;

include!("../../tests/fixtures/vscode/palettes.rs");

fn pair(bg: u32, fg: u32) -> ColorPair {
    ColorPair {
        background: Some(c(bg)),
        foreground: Some(c(fg)),
    }
}

#[test]
fn real_host_pairs_and_state_colors_are_preserved() {
    for p in real_palettes() {
        let t = Theme::from_host(&p);
        for (host, resolved) in [
            (p.editor, t.editor),
            (p.panel, t.panel),
            (p.bar, t.bar),
            (p.elevated, t.elevated),
            (p.tooltip, t.tooltip),
            (p.input, t.input),
            (p.selection, t.selection),
            (p.hover, t.hover),
            (p.button, t.button),
            (p.menu_hover, t.menu_hover),
        ] {
            if let Some(bg) = host.background.filter(|c| c.a == 1.0) {
                assert_eq!(resolved.bg, bg);
            }
            if let Some(fg) = host.foreground {
                assert_eq!(resolved.text, fg, "{:?}: supplied foreground", p.appearance);
            }
        }
        assert_eq!(t.row(true, true).bg, t.selection.bg);
        assert_eq!(t.row(false, true).text, t.hover.text);
        assert_eq!(t.row(false, false).text, t.panel.text);
        if p.appearance.is_high_contrast() {
            assert!(contrast(t.border, t.panel.bg) >= 2.99);
        }
        for (i, chart) in p.charts.into_iter().enumerate() {
            let color = opaque(chart, c(0));
            if contrast(color, t.editor.bg) >= 3.5 {
                let resolved = match i {
                    0 => t.wave_signal,
                    1 => t.wave_undef,
                    2 => t.wave_highimp,
                    3 => t.wave_dontcare,
                    _ => continue,
                };
                assert_eq!(
                    resolved, color,
                    "already-readable chart colours must not be altered"
                );
            }
        }
    }
}

#[test]
fn missing_and_invisible_values_fall_back_in_every_appearance() {
    for appearance in [
        Appearance::Dark,
        Appearance::Light,
        Appearance::HighContrastDark,
        Appearance::HighContrastLight,
    ] {
        for invisible in [None, Some(ca(0x808080, 0.0))] {
            let p = HostPalette {
                appearance,
                editor: ColorPair {
                    background: None,
                    foreground: invisible,
                },
                charts: [invisible; 6],
                ..Default::default()
            };
            let t = Theme::from_host(&p);
            for surface in [
                t.editor,
                t.panel,
                t.input,
                t.bar,
                t.elevated,
                t.tooltip,
                t.selection,
                t.hover,
                t.button,
                t.button_hover,
                t.badge,
                t.badge_hover,
            ] {
                assert!(contrast(over(surface.text, surface.bg), surface.bg) >= 4.49);
            }
            for wave in [
                t.wave_signal,
                t.wave_undef,
                t.wave_highimp,
                t.wave_dontcare,
                t.wave_weak,
                t.wave_cursor,
            ] {
                for bg in [
                    t.editor.bg,
                    over(t.wave_row_selected, t.editor.bg),
                    over(t.wave_row_hover, t.editor.bg),
                ] {
                    assert!(contrast(wave, bg) >= 2.99);
                }
            }
            for marker in t.markers {
                assert!(contrast(over(marker.text, marker.background), marker.background) >= 4.49);
            }
        }
    }
}

#[test]
fn each_surface_resolves_against_its_own_background() {
    let p = HostPalette {
        appearance: Appearance::Light,
        editor: pair(0xfff4e6, 0x321800),
        panel: pair(0x203040, 0xf0f8ff),
        bar: pair(0x005fb8, 0xffffff),
        input: pair(0xffffff, 0x333333),
        elevated: pair(0x502000, 0xffffff),
        tooltip: pair(0xe0ffe0, 0x002200),
        selection: pair(0x0060c0, 0xffffff),
        hover: pair(0xeeeeee, 0x333333),
        button: pair(0x0078d4, 0xffffff),
        menu_hover: pair(0xffff00, 0x000000),
        ..Default::default()
    };
    let t = Theme::from_host(&p);
    assert_eq!(t.panel.text, c(0xf0f8ff));
    assert_eq!(t.selection.text, c(0xffffff));
    assert_eq!(t.hover.text, c(0x333333));
    assert_eq!(t.bar.text, c(0xffffff));
    assert_eq!(t.button.text, c(0xffffff)); // Valid host choice below the old blanket 4.5 floor.
    assert_eq!(t.menu_hover.text, c(0));
    assert_eq!(t.tooltip.text, c(0x002200));
    assert_eq!(t.badge.text, t.input.text);
    assert!(contrast(t.bar_hover.text, t.bar_hover.bg) >= 4.49);
}

#[test]
fn translucent_chart_strokes_and_chips_keep_rgb_not_background_tint() {
    let color = ca(0x00b080, 0.2);
    let t = Theme::from_host(&HostPalette {
        charts: [Some(color); 6],
        ..Default::default()
    });
    assert_eq!(t.wave_signal, alpha(color, 1.0));
    for marker in t.markers {
        assert_eq!(marker.background, alpha(color, 1.0));
        assert_eq!(marker.hover, marker.background);
        assert!(contrast(over(marker.text, marker.background), marker.background) >= 4.49);
    }
    let bright = ca(0xffff00, 0.1);
    let t = Theme::from_host(&HostPalette {
        appearance: Appearance::Light,
        charts: [Some(bright); 6],
        ..Default::default()
    });
    assert_eq!(t.wave_signal.h, bright.h);
    assert_eq!(t.wave_signal.s, bright.s);
    assert!(t.wave_signal.l < bright.l);
    assert_eq!(t.markers[0].background, alpha(bright, 1.0));
}

#[test]
fn native_defaults_remain_one_dark() {
    let t = Theme::one_dark();
    assert_eq!(t.editor.bg, c(0x282c33));
    assert_eq!(t.panel.text_muted, c(0xa9afbc));
    assert_eq!(t.wave_signal, c(0xa1c181));
    assert_eq!(t.marker(0).background.a, 0.85);
}
