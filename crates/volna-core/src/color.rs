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
}
