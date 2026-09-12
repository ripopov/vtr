//! CSS colour parsing shared by host JSON and the VS Code adapter.
use crate::color::Color;

/// Parse the CSS colour syntaxes VS Code emits (and the common variants):
/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`, `rgba(r, g, b, a)`
/// with comma- or space-separated components (`/` before alpha), integer or
/// percentage channels, and `transparent`.
pub fn parse_css_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("transparent") {
        return Some(Color::TRANSPARENT);
    }
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = text.to_ascii_lowercase();
    let inner = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = if inner.contains(',') {
        inner.split(',').map(str::trim).collect()
    } else {
        let (rgb, alpha) = inner
            .split_once('/')
            .map_or((inner, None), |(rgb, a)| (rgb, Some(a.trim())));
        let mut parts: Vec<_> = rgb.split_whitespace().collect();
        if parts.len() != 3 {
            return None;
        }
        if let Some(alpha) = alpha {
            parts.push(alpha);
        }
        parts
    };
    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }
    let number = |s: &str, scale: f32| -> Option<f32> {
        let (s, scale) = s.strip_suffix('%').map_or((s, scale), |s| (s, 100.0));
        let value = s.parse::<f32>().ok()?;
        value.is_finite().then(|| (value / scale).clamp(0.0, 1.0))
    };
    Some(Color::from_rgba(
        number(parts[0], 255.0)?,
        number(parts[1], 255.0)?,
        number(parts[2], 255.0)?,
        parts.get(3).map_or(Some(1.0), |a| number(a, 1.0))?,
    ))
}

fn parse_hex(hex: &str) -> Option<Color> {
    let digits: Vec<u8> = hex
        .chars()
        .map(|ch| ch.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    let (r, g, b, a) = match digits.as_slice() {
        [r, g, b] => (r * 17, g * 17, b * 17, 255),
        [r, g, b, a] => (r * 17, g * 17, b * 17, a * 17),
        [r1, r2, g1, g2, b1, b2] => (r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, 255),
        [r1, r2, g1, g2, b1, b2, a1, a2] => {
            (r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, a1 * 16 + a2)
        }
        _ => return None,
    };
    Some(Color::from_rgba(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    ))
}
