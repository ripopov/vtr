use super::*;
use crate::color::{gamma, linear};
use crate::theme::contrast;

/// Machado, Oliveira and Fernandes 2009 at full severity, in linear sRGB.
const CVD: [(&str, [[f32; 3]; 3]); 3] = [
    (
        "deutan",
        [
            [0.367_322, 0.860_646, -0.227_968],
            [0.280_085, 0.672_501, 0.047_413],
            [-0.011_820, 0.042_940, 0.968_881],
        ],
    ),
    (
        "protan",
        [
            [0.152_286, 1.052_583, -0.204_868],
            [0.114_503, 0.786_281, 0.099_216],
            [-0.003_882, -0.048_116, 1.051_998],
        ],
    ),
    (
        "tritan",
        [
            [1.255_528, -0.076_749, -0.178_779],
            [-0.078_411, 0.930_809, 0.147_602],
            [0.004_733, 0.691_367, 0.303_900],
        ],
    ),
];

fn simulate(c: Color, m: &[[f32; 3]; 3]) -> Color {
    let [r, g, b, _] = c.to_rgba();
    let v = [r, g, b].map(linear);
    let [r, g, b] =
        m.map(|row| gamma((row[0] * v[0] + row[1] * v[1] + row[2] * v[2]).clamp(0.0, 1.0)));
    Color::from_rgba(r, g, b, 1.0)
}

/// ΔE in OKLab, times 100.
fn delta_e(a: Color, b: Color) -> f32 {
    let (x, y) = (a.to_oklab(), b.to_oklab());
    100.0 * ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
}

fn both() -> [(&'static str, Theme); 2] {
    [
        ("Volna Dark", Theme::volna(true)),
        ("Volna Light", Theme::volna(false)),
    ]
}

#[test]
fn the_theme_is_the_design_sheet() {
    let dark = Sheet::new(true);
    let light = Sheet::new(false);
    let mut names: Vec<_> = dark.names().collect();
    let mut light_names: Vec<_> = light.names().collect();
    names.sort_unstable();
    light_names.sort_unstable();
    assert_eq!(names, light_names, "both blocks declare the same tokens");
    for (is_dark, (_, t)) in [true, false].into_iter().zip(both()) {
        let s = Sheet::new(is_dark);
        let pairs = [
            ("canvas", t.editor.bg),
            ("panel", t.panel.bg),
            ("bar", t.bar.bg),
            ("elevated", t.elevated.bg),
            ("border", t.border),
            ("border-subtle", t.border_variant),
            ("text", t.panel.text),
            ("text-muted", t.panel.text_muted),
            ("text-faint", t.panel.text_placeholder),
            ("selection", t.selection.bg),
            ("hover", t.hover.bg),
            ("accent", t.wave_cursor),
            ("on-accent", t.wave_cursor_text),
            ("signal", t.wave_signal),
            ("undef", t.wave_undef),
            ("highimp", t.wave_highimp),
            ("dontcare", t.wave_dontcare),
            ("weak", t.wave_weak),
            ("event", t.wave_event_coalesced),
            ("relation-out", t.tx_relation_out),
            ("high-fill", t.wave_high_fill),
            ("undef-fill", t.wave_undef_fill),
            ("dense", t.wave_dense),
            ("grid", t.wave_tick),
            ("row-selected", t.wave_row_selected),
            ("row-hover", t.wave_row_hover),
            ("outside", t.wave_outside),
            ("flush", t.wave_flush),
            ("marker-text", t.markers[0].text),
            ("stage-text", t.stage_text),
        ];
        for (name, value) in pairs {
            assert_eq!(value, s.color(name), "--viewer-{name}");
        }
        for (i, m) in t.markers.iter().enumerate() {
            assert_eq!(m.stroke, s.color(&format!("marker-{}", i + 1)));
        }
        let StageLadder::Oklch { hue_from, l, .. } = t.stages else {
            panic!("Volna computes its stage ladder in OKLCH");
        };
        assert_eq!(hue_from, s.number("stage-hue-from"));
        assert_eq!(l, s.number("stage-l"));
    }
}

#[test]
fn every_value_stroke_reaches_the_text_floor_on_the_canvas() {
    for (name, t) in both() {
        let canvas = t.editor.bg;
        let strokes = [
            ("signal", t.wave_signal),
            ("X", t.wave_undef),
            ("Z", t.wave_highimp),
            ("don't-care", t.wave_dontcare),
            ("weak", t.wave_weak),
            ("coalesced event", t.wave_event_coalesced),
            ("outgoing relation", t.tx_relation_out),
            ("cursor", t.wave_cursor),
            ("bus text", t.wave_bus_text),
        ];
        for (what, color) in strokes {
            let ratio = contrast(color, canvas);
            assert!(ratio >= 4.5, "{name}: {what} is {ratio:.2}:1 on the canvas");
        }
        // Quiet grid, loud signals: the grid is scaffolding at 1.3–1.4:1.
        let grid = contrast(over(t.wave_tick, canvas), canvas);
        assert!((1.25..1.5).contains(&grid), "{name}: grid {grid:.2}:1");
        // Marker chips carry readable labels.
        for m in &t.markers {
            assert!(
                contrast(m.text, m.background) >= 4.5,
                "{name}: marker label"
            );
            assert!(contrast(m.stroke, canvas) >= 3.0, "{name}: marker line");
        }
    }
}

#[test]
fn stage_names_read_on_every_fill_and_neighbours_survive_colour_blindness() {
    for (name, t) in both() {
        let mut weakest_text = f32::MAX;
        let mut closest = f32::MAX;
        // Every ladder length, not a few: an early try passed n = 8 and failed n = 3.
        for n in 2..=16 {
            let ladder: Vec<StageStyle> = (0..n).map(|k| t.stage_style(k, n)).collect();
            for style in &ladder {
                weakest_text = weakest_text.min(contrast(style.text, style.fill));
            }
            for pair in ladder.windows(2) {
                for (_, m) in &CVD {
                    closest = closest.min(delta_e(
                        simulate(pair[0].fill, m),
                        simulate(pair[1].fill, m),
                    ));
                }
                // Lightness alternates, so a hue shift one reader cannot see still shows.
                let (a, b) = (pair[0].fill.to_oklab()[0], pair[1].fill.to_oklab()[0]);
                assert!(
                    (a - b).abs() > 0.1,
                    "{name}: n={n} neighbours alternate lightness"
                );
            }
        }
        assert!(
            weakest_text >= 5.9,
            "{name}: stage text {weakest_text:.2}:1"
        );
        assert!(
            closest >= 11.0,
            "{name}: closest neighbours ΔE {closest:.1}"
        );
        let fallback = t.stage_fallback();
        assert!(contrast(fallback.text, fallback.fill) >= 4.5);
    }
}

#[test]
fn value_tints_are_stable_faint_and_clear_of_x_and_z() {
    for (name, t) in both() {
        let tint = t.value_tint("irq handler");
        assert_eq!(tint, t.value_tint("irq handler"), "{name}: stable");
        assert_eq!(tint.a, TINT_ALPHA);
        let states = [
            "idle",
            "fetch",
            "decode",
            "execute",
            "wait",
            "reset",
            "dot product loop",
        ];
        let hues: std::collections::HashSet<_> =
            states.iter().map(|s| t.value_tint(s).to_rgba8()).collect();
        assert!(hues.len() >= 4, "{name}: states spread over the hues");
        // A tint's hue stays at least 40° from the X red and the Z amber.
        let hue = |c: Color| {
            let [_, a, b] = c.to_oklab();
            b.atan2(a).to_degrees().rem_euclid(360.0)
        };
        let apart = |a: f32, b: f32| {
            let d = (a - b).abs();
            d.min(360.0 - d)
        };
        for s in states {
            let h = hue(t.value_tint(s));
            assert!(apart(h, hue(t.wave_undef)) >= 40.0, "{name}: {s} is not X");
            assert!(
                apart(h, hue(t.wave_highimp)) >= 40.0,
                "{name}: {s} is not Z"
            );
        }
    }
}
