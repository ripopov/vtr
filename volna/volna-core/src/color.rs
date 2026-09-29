//! A toolkit-neutral colour. Stored as HSLA (components in `0..=1`, straight
//! alpha) because the theme resolves contrast by moving lightness alone.
//! Conversions follow the same formulas as GPUI's `Hsla`/`Rgba`, so a frontend
//! that copies the four fields into its own type reproduces the theme exactly.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub h: f32,
    pub s: f32,
    pub l: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: 0.0,
    };

    /// Opaque colour from `0xRRGGBB`.
    pub fn rgb(hex: u32) -> Color {
        Color::rgba_u32((hex << 8) | 0xff)
    }

    /// Colour from `0xRRGGBBAA`.
    pub fn rgba_u32(hex: u32) -> Color {
        let r = ((hex >> 24) & 0xff) as f32 / 255.0;
        let g = ((hex >> 16) & 0xff) as f32 / 255.0;
        let b = ((hex >> 8) & 0xff) as f32 / 255.0;
        let a = (hex & 0xff) as f32 / 255.0;
        Color::from_rgba(r, g, b, a)
    }

    /// Colour from sRGB components in `0..=1`.
    pub fn from_rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        let max = r.max(g.max(b));
        let min = r.min(g.min(b));
        let delta = max - min;
        let l = (max + min) / 2.0;
        let s = if l == 0.0 || l == 1.0 {
            0.0
        } else if l < 0.5 {
            delta / (2.0 * l)
        } else {
            delta / (2.0 - 2.0 * l)
        };
        let h = if delta == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };
        Color { h, s, l, a }
    }

    /// sRGB components in `0..=1` plus straight alpha.
    pub fn to_rgba(self) -> [f32; 4] {
        let c = (1.0 - (2.0 * self.l - 1.0).abs()) * self.s;
        let x = c * (1.0 - ((self.h * 6.0) % 2.0 - 1.0).abs());
        let m = self.l - c / 2.0;
        let cm = c + m;
        let xm = x + m;
        let (r, g, b) = match (self.h * 6.0).floor() as i32 {
            0 | 6 => (cm, xm, m),
            1 => (xm, cm, m),
            2 => (m, cm, xm),
            3 => (m, xm, cm),
            4 => (xm, m, cm),
            _ => (cm, m, xm),
        };
        [r, g, b, self.a]
    }

    /// sRGB bytes plus straight alpha byte.
    pub fn to_rgba8(self) -> [u8; 4] {
        self.to_rgba()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    pub fn with_alpha(mut self, a: f32) -> Color {
        self.a = a;
        self
    }

    /// The sRGB colour for OKLCH lightness `l` (`0..=1`), chroma `c` and hue
    /// `h` in degrees. Out-of-gamut colours lose chroma until they fit,
    /// keeping lightness and hue, as CSS Color 4 maps them.
    pub fn oklch(l: f32, c: f32, h: f32) -> Color {
        let (sin, cos) = h.to_radians().sin_cos();
        let at = |c: f32| oklab_to_linear([l, c * cos, c * sin]);
        let fits = |v: [f32; 3]| v.iter().all(|x| (-1e-5..=1.0 + 1e-5).contains(x));
        let mut v = at(c);
        if !fits(v) {
            let (mut lo, mut hi) = (0.0, c);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                if fits(at(mid)) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            v = at(lo);
        }
        let [r, g, b] = v.map(|x| gamma(x.clamp(0.0, 1.0)));
        Color::from_rgba(r, g, b, 1.0)
    }

    /// OKLab `[L, a, b]` of the opaque colour.
    pub fn to_oklab(self) -> [f32; 3] {
        let [r, g, b, _] = self.to_rgba();
        let [r, g, b] = [r, g, b].map(linear);
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }
}

/// An sRGB channel in linear light.
pub(crate) fn linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear-light channel in sRGB.
pub(crate) fn gamma(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn oklab_to_linear([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn rgb_round_trips_through_hsla() {
        for hex in [
            0x000000, 0xffffff, 0x282c33, 0xa1c181, 0x74ade8, 0xd07277, 0x123456,
        ] {
            let c = Color::rgb(hex);
            let [r, g, b, a] = c.to_rgba();
            let back = ((r * 255.0).round() as u32) << 16
                | ((g * 255.0).round() as u32) << 8
                | (b * 255.0).round() as u32;
            assert_eq!(back, hex, "{hex:06x}");
            assert_eq!(a, 1.0);
        }
        assert_eq!(
            Color::rgba_u32(0x11223366).to_rgba8(),
            [0x11, 0x22, 0x33, 0x66]
        );
    }

    #[test]
    fn oklch_matches_the_design_sheet_and_round_trips() {
        // docs/design-system/tokens/viewer.css stores oklch(.80 .12 155) as #7ad59c
        // and oklch(.45 .19 25) as #a30018.
        assert_eq!(
            Color::oklch(0.80, 0.12, 155.0).to_rgba8(),
            [0x7a, 0xd5, 0x9c, 0xff]
        );
        assert_eq!(
            Color::oklch(0.45, 0.19, 25.0).to_rgba8(),
            [0xa3, 0x00, 0x18, 0xff]
        );
        let [l, a, b] = Color::oklch(0.68, 0.10, 200.0).to_oklab();
        assert!((l - 0.68).abs() < 1e-3);
        assert!((a.hypot(b) - 0.10).abs() < 2e-3);
        assert!((b.atan2(a).to_degrees().rem_euclid(360.0) - 200.0).abs() < 0.5);
    }
}
