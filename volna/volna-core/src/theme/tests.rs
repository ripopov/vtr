use super::*;

include!("../../tests/fixtures/themes.rs");
use crate::color::Color;

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
            for tint in t.sidebar_tints {
                for bg in [t.panel.bg, t.selection.bg, t.hover.bg] {
                    assert!(contrast(tint, bg) >= 2.99);
                }
            }
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
                t.wave_event_coalesced,
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
    let p =
        HostPalette::from_json(include_str!("../../tests/fixtures/custom-palette.json")).unwrap();
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
    assert_eq!(t.wave_signal, color.with_alpha(1.0));
    for marker in t.markers {
        assert_eq!(marker.background, color.with_alpha(1.0));
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
    assert_eq!(t.markers[0].background, bright.with_alpha(1.0));
}

#[test]
fn native_defaults_remain_one_dark() {
    let t = Theme::one_dark();
    assert_eq!(t.editor.bg, c(0x282c33));
    assert_eq!(t.panel.text_muted, c(0xa9afbc));
    assert_eq!(t.wave_signal, c(0xa1c181));
    assert_eq!(t.marker(0).background.a, 0.85);
}

#[test]
fn css_syntax_is_shared_and_invalid_numbers_never_reach_frontends() {
    for (css, expected) in [
        ("#abc", 0xaabbccff),
        ("#abcd", 0xaabbccdd),
        (" #ABCDef ", 0xabcdefff),
        ("#12345678", 0x12345678),
        ("rgb(17, 34, 51)", 0x112233ff),
        ("rgba(17, 34, 51, 0.4)", 0x11223366),
        ("rgb(100% 0% 20% / 40%)", 0xff003366),
        ("transparent", 0),
        ("rgb(300, -10, 0)", 0xff0000ff),
    ] {
        let actual = parse_css_color(css).unwrap().to_rgba();
        let expected = Color::rgba_u32(expected).to_rgba();
        for (a, b) in actual.into_iter().zip(expected) {
            assert!((a - b).abs() < 0.0001, "{css}");
        }
    }
    for invalid in [
        "",
        "inherit",
        "#ggg",
        "#12",
        "rgb(NaN, 0, 0)",
        "rgb(inf, 0, 0)",
        "rgba(0, 0, 0, -inf)",
        "rgb(1,,2,3)",
        "rgb(1 2 3 / / .5)",
        "rgb(1 2 3 .5)",
        "#ééé",
        "rgb(1,2)",
    ] {
        assert!(parse_css_color(invalid).is_none(), "{invalid}");
    }
}

#[test]
fn json_interface_defaults_and_validation() {
    let p = HostPalette::from_json(r##"{"appearance":"high-contrast-light","editor":{"background":"#fff","foreground":null},"unknown":true}"##).unwrap();
    assert_eq!(p.appearance, Appearance::HighContrastLight);
    assert_eq!(p.editor.background, Some(c(0xffffff)));
    assert!(p.editor.foreground.is_none());
    assert_eq!(
        HostPalette::from_json("{}").unwrap().appearance,
        Appearance::Dark
    );
    for invalid in [
        "[]",
        "null",
        r#"{"appearance":"sepia"}"#,
        r#"{"editor":{"foreground":"rgb(NaN, 0, 0)"}}"#,
        r#"{"editor":{"foreground":123}}"#,
        r##"{"charts":["#fff"]}"##,
    ] {
        assert!(HostPalette::from_json(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn vscode_precedence_fallbacks_and_appearance_are_resolved_in_rust() {
    let p = vscode::host_palette(
        "kind=vscode-high-contrast-light\n--vscode-editor-background=#111\n--vscode-editor-foreground=#123456\n--vscode-foreground=#fff\n--vscode-charts-green=broken\n--vscode-terminal-ansiGreen=rgb(0 50% 0)\n--vscode-focusBorder=#00f\n--vscode-contrastActiveBorder=#f00",
    );
    assert_eq!(p.appearance, Appearance::HighContrastLight);
    assert_eq!(p.editor.foreground, Some(c(0x123456)));
    assert_eq!(p.panel.foreground, Some(c(0xffffff)));
    assert_eq!(p.charts[0], parse_css_color("rgb(0 50% 0)"));
    assert_eq!(p.focus, Some(c(0xff0000)));
    for (bg, expected) in [
        ("#fff", Appearance::Light),
        ("#111", Appearance::Dark),
        ("broken", Appearance::Dark),
    ] {
        assert_eq!(
            vscode::host_palette(&format!("--vscode-editor-background={bg}")).appearance,
            expected
        );
    }
    // A later snapshot starts fresh; omitted host roles cannot retain stale colours.
    assert!(vscode::host_palette("kind=vscode-light").charts[0].is_none());
}

#[test]
fn real_and_mixed_palettes_keep_surface_text_visible() {
    let custom =
        HostPalette::from_json(include_str!("../../tests/fixtures/custom-palette.json")).unwrap();
    for p in real_palettes().into_iter().chain([custom]) {
        let t = Theme::from_host(&p);
        for surface in [
            t.editor,
            t.panel,
            t.bar,
            t.bar_hover,
            t.elevated,
            t.tooltip,
            t.input,
            t.selection,
            t.hover,
            t.menu_hover,
            t.button,
            t.button_hover,
            t.badge,
            t.badge_hover,
        ] {
            // Preserve the host's choices (some supplied text pairs are below 4.5).
            assert!(contrast(over(surface.text, surface.bg), surface.bg) >= 3.0);
            for text in [
                surface.text_muted,
                surface.text_placeholder,
                surface.icon,
                surface.icon_accent,
                surface.error,
                surface.warning,
            ] {
                assert!(contrast(over(text, surface.bg), surface.bg) >= 2.99);
            }
        }
        for marker in t.markers {
            assert!(contrast(over(marker.text, marker.background), marker.background) >= 4.49);
        }
    }
}

#[test]
fn translucent_surfaces_and_foregrounds_use_actual_composited_contrast() {
    let p = HostPalette::from_json(r##"{"editor":{"background":"#000"},"panel":{"background":"#fff8","foreground":"#000"},"selection":{"background":"#fff8","foreground":"#fff0"}}"##).unwrap();
    let t = Theme::from_host(&p);
    assert_eq!(t.panel.bg, over(p.panel.background.unwrap(), t.editor.bg));
    assert_eq!(
        t.selection.bg,
        over(p.selection.background.unwrap(), t.panel.bg)
    );
    assert!(contrast(over(t.selection.text, t.selection.bg), t.selection.bg) >= 4.49);
}

#[test]
fn coalesced_events_remain_distinct_with_identical_host_chart_colors() {
    for appearance in [
        Appearance::Dark,
        Appearance::Light,
        Appearance::HighContrastDark,
        Appearance::HighContrastLight,
    ] {
        let palette = HostPalette {
            appearance,
            charts: [Some(c(0xa1c181)); 6],
            ..Default::default()
        };
        let theme = Theme::from_host(&palette);
        assert_ne!(theme.wave_signal, theme.wave_event_coalesced);
        assert!(contrast(theme.wave_event_coalesced, theme.editor.bg) >= 2.99);
    }
}

#[test]
fn zoomed_scales_every_metric_from_the_design_size_and_map_keeps_it() {
    let base = Theme::one_dark();
    assert_eq!(base.zoom, 1.0);
    let big = base.zoomed(2.0);
    assert_eq!(big.zoom, 2.0);
    assert_eq!(big.ui_size, base.ui_size * 2.0);
    assert_eq!(big.ui_size_small, base.ui_size_small * 2.0);
    assert_eq!(big.mono_size, base.mono_size * 2.0);
    assert_eq!(big.row_height, base.row_height * 2.0);
    assert_eq!(big.header_height, base.header_height * 2.0);
    assert_eq!(big.titlebar_height, base.titlebar_height * 2.0);
    assert_eq!(big.statusbar_height, base.statusbar_height * 2.0);
    assert_eq!(big.timeline_height, base.timeline_height * 2.0);
    assert_eq!(big.icon_size, base.icon_size * 2.0);
    assert_eq!(big.splitter_grab, base.splitter_grab * 2.0);
    assert_eq!(big.scale(12.0), 24.0);
    // Colours and fonts are untouched.
    assert_eq!(big.editor.bg, base.editor.bg);
    assert_eq!(big.ui_font, base.ui_font);
    // Zooming an already zoomed theme starts from the design size again.
    let half = big.zoomed(0.5);
    assert_eq!(half.row_height, base.row_height * 0.5);
    assert_eq!(half.zoomed(1.0).row_height, base.row_height);
    // Nonsense factors fall back to the design size.
    assert_eq!(base.zoomed(0.0).zoom, 1.0);
    assert_eq!(base.zoomed(f32::NAN).row_height, base.row_height);
    // Colour conversion keeps the zoom.
    let mapped = big.map(|c| c.with_alpha(0.5));
    assert_eq!(mapped.zoom, 2.0);
    assert_eq!(mapped.row_height, big.row_height);
}

/// Perceptual distance (OKLab ΔE × 100) between two opaque colours, with
/// Ottosson's published matrices.
#[allow(clippy::excessive_precision)]
fn delta_e(a: Color, b: Color) -> f32 {
    let lab = |c: Color| {
        let [r, g, b, _] = c.to_rgba();
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(r), lin(g), lin(b));
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        [
            0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        ]
    };
    let (a, b) = (lab(a), lab(b));
    100.0 * ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Every theme a user can get: Volna, One Dark, the bundled palettes, the real
/// host palettes and the fallbacks of an empty palette in each appearance.
fn every_theme() -> Vec<(String, Theme)> {
    let mut out = vec![
        ("volna-dark".to_owned(), Theme::volna(true)),
        ("volna-light".to_owned(), Theme::volna(false)),
        ("one-dark".to_owned(), Theme::one_dark()),
    ];
    for b in BUILTIN {
        out.push((b.id.to_owned(), Theme::builtin(b.id).unwrap()));
    }
    for (i, p) in real_palettes().into_iter().enumerate() {
        out.push((format!("host {i} {:?}", p.appearance), Theme::from_host(&p)));
    }
    for appearance in [
        Appearance::Dark,
        Appearance::Light,
        Appearance::HighContrastDark,
        Appearance::HighContrastLight,
    ] {
        let p = HostPalette {
            appearance,
            ..Default::default()
        };
        out.push((format!("fallback {appearance:?}"), Theme::from_host(&p)));
    }
    out
}

#[test]
fn row_colours_are_legible_everywhere_and_never_pass_for_x_or_z() {
    use crate::wave::Tint;
    let mut weakest = f32::MAX;
    let mut nearest = f32::MAX;
    for (name, t) in every_theme() {
        assert_eq!(t.ink(None), t.wave_signal);
        for tint in Tint::ALL {
            let ink = t.ink(Some(tint));
            assert_eq!(ink.a, 1.0, "{name} {tint:?}");
            for bg in t.tint_backgrounds() {
                let ratio = contrast(ink, bg);
                weakest = weakest.min(ratio);
                assert!(ratio >= 2.99, "{name} {tint:?}: {ratio:.2}:1");
            }
            for unknown in [t.wave_undef, t.wave_highimp] {
                let d = delta_e(ink, unknown);
                nearest = nearest.min(d);
                assert!(d >= 10.0, "{name} {tint:?} is {d:.1} from X/Z");
            }
        }
        // The six choices are told apart from each other too.
        let inks: Vec<Color> = std::iter::once(None)
            .chain(Tint::ALL.map(Some))
            .map(|tint| t.ink(tint))
            .collect();
        for (i, a) in inks.iter().enumerate() {
            for b in &inks[i + 1..] {
                assert!(delta_e(*a, *b) >= 5.0, "{name}: two colours alike");
            }
        }
    }
    eprintln!("weakest tint contrast {weakest:.2}:1, nearest to X/Z ΔE {nearest:.1}");
}

#[test]
fn an_inked_theme_changes_only_the_signal_ink() {
    use crate::wave::Tint;
    let t = Theme::one_dark();
    let pink = t.inked(Some(Tint::Pink));
    assert_eq!(pink.wave_signal, t.wave_tints[Tint::Pink.index()]);
    assert_eq!(pink.wave_high_fill.a, t.wave_high_fill.a);
    assert_eq!(pink.wave_dense.a, t.wave_dense.a);
    assert_eq!(pink.wave_high_fill.with_alpha(1.0), pink.wave_signal);
    for (a, b) in [
        (pink.wave_undef, t.wave_undef),
        (pink.wave_highimp, t.wave_highimp),
        (pink.wave_weak, t.wave_weak),
        (pink.wave_dontcare, t.wave_dontcare),
        (pink.wave_cursor, t.wave_cursor),
        (pink.wave_bus_text, t.wave_bus_text),
        (pink.markers[0].stroke, t.markers[0].stroke),
    ] {
        assert_eq!(a, b);
    }
    assert_eq!(t.inked(None).wave_signal, t.wave_signal);
}
