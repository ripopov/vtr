//! Value translators turn a [`WaveValue`] into display text.
//!
//! Adding a translator means implementing [`Translator`] and registering it
//! with [`Translators::register`]; nothing else in the viewer changes.

use std::sync::Arc;

use super::value::{SignalShape, ValueKind, WaveValue, kind_of_bits};

#[derive(Clone, PartialEq, Debug)]
pub struct Translated {
    pub text: String,
    pub kind: ValueKind,
}

impl Translated {
    fn normal(text: impl Into<String>) -> Self {
        Translated {
            text: text.into(),
            kind: ValueKind::Normal,
        }
    }
}

pub trait Translator: Send + Sync {
    /// Stable identifier, stored in per-row settings (e.g. `"hex"`).
    fn id(&self) -> &'static str;
    /// Menu label.
    fn name(&self) -> &'static str;
    /// Short badge shown next to the signal name.
    fn badge(&self) -> &'static str;
    /// Whether the translator can display signals of this shape.
    fn applies(&self, shape: SignalShape) -> bool;
    fn translate(&self, value: &WaveValue) -> Translated;
}

/// Registry of translators. Order is the menu order.
#[derive(Clone)]
pub struct Translators {
    all: Vec<Arc<dyn Translator>>,
}

impl Translators {
    pub fn builtin() -> Self {
        let mut t = Translators { all: Vec::new() };
        t.register(Arc::new(BitTranslator));
        t.register(Arc::new(HexTranslator));
        t.register(Arc::new(BinaryTranslator));
        t.register(Arc::new(UnsignedTranslator));
        t.register(Arc::new(SignedTranslator));
        t.register(Arc::new(FloatTranslator));
        t.register(Arc::new(RealTranslator));
        t.register(Arc::new(TextTranslator));
        t
    }

    pub fn register(&mut self, translator: Arc<dyn Translator>) {
        self.all.retain(|t| t.id() != translator.id());
        self.all.push(translator);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Translator>> {
        self.all.iter().find(|t| t.id() == id).cloned()
    }

    pub fn applicable(&self, shape: SignalShape) -> Vec<Arc<dyn Translator>> {
        self.all
            .iter()
            .filter(|t| t.applies(shape))
            .cloned()
            .collect()
    }

    /// The translator a freshly added signal starts with.
    pub fn default_for(&self, shape: SignalShape) -> Arc<dyn Translator> {
        let id = match shape {
            SignalShape::Bit | SignalShape::Event => "bit",
            SignalShape::Vector { .. } => "hex",
            SignalShape::Real => "real",
            SignalShape::Text => "text",
        };
        self.get(id).expect("built-in translator")
    }
}

// ---------------------------------------------------------------------------

/// Whole-word rendering for values that are not plain 0/1, shared by the
/// numeric translators (like Surfer's numeric translators).
fn non_binary_word(bits: &[u8]) -> Option<Translated> {
    let kind = kind_of_bits(bits);
    let text = match kind {
        ValueKind::Normal => return None,
        ValueKind::Undef => "UNDEF",
        ValueKind::HighImp => "HIGHIMP",
        ValueKind::DontCare => "DON'T CARE",
        ValueKind::Weak => "WEAK",
    };
    Some(Translated {
        text: text.into(),
        kind,
    })
}

fn bits_of(value: &WaveValue) -> Option<&str> {
    match value {
        WaveValue::Bits(s) => Some(s.as_str()),
        _ => None,
    }
}

fn not_applicable() -> Translated {
    Translated {
        text: "?".into(),
        kind: ValueKind::Undef,
    }
}

struct BitTranslator;
impl Translator for BitTranslator {
    fn id(&self) -> &'static str {
        "bit"
    }
    fn name(&self) -> &'static str {
        "Bit"
    }
    fn badge(&self) -> &'static str {
        "bit"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        matches!(shape, SignalShape::Bit | SignalShape::Event)
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        raw_bits(value)
    }
}

fn raw_bits(value: &WaveValue) -> Translated {
    let Some(bits) = bits_of(value) else {
        return not_applicable();
    };
    Translated {
        text: bits.to_string(),
        kind: kind_of_bits(bits.as_bytes()),
    }
}

struct BinaryTranslator;
impl Translator for BinaryTranslator {
    fn id(&self) -> &'static str {
        "bin"
    }
    fn name(&self) -> &'static str {
        "Binary"
    }
    fn badge(&self) -> &'static str {
        "bin"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        shape.is_digital()
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        raw_bits(value)
    }
}

/// Digit-wise hexadecimal: a nibble containing `x`/`z`/... becomes that letter.
struct HexTranslator;
impl Translator for HexTranslator {
    fn id(&self) -> &'static str {
        "hex"
    }
    fn name(&self) -> &'static str {
        "Hexadecimal"
    }
    fn badge(&self) -> &'static str {
        "hex"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        shape.is_digital()
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        let Some(bits) = bits_of(value) else {
            return not_applicable();
        };
        let bytes = bits.as_bytes();
        let ndigits = bytes.len().div_ceil(4);
        let mut out = String::with_capacity(ndigits);
        for d in 0..ndigits {
            // Nibble `d` (MSB first) covers bit string indices [start, end).
            let end = bytes.len() - (ndigits - 1 - d) * 4;
            let start = end.saturating_sub(4);
            let nib = &bytes[start..end];
            let mut v = 0u8;
            let mut special: Option<u8> = None;
            for &c in nib {
                match c {
                    b'0' => v <<= 1,
                    b'1' => v = (v << 1) | 1,
                    other => {
                        // Priority: x over z over anything else.
                        let c = other.to_ascii_lowercase();
                        special = Some(match (special, c) {
                            (Some(b'x'), _) | (_, b'x') | (_, b'u') | (_, b'w') => b'x',
                            (Some(b'z'), _) | (_, b'z') => b'z',
                            (_, c) => c,
                        });
                    }
                }
            }
            match special {
                Some(c) => out.push(c as char),
                None => out.push(char::from_digit(v as u32, 16).unwrap()),
            }
        }
        Translated {
            text: out,
            kind: kind_of_bits(bytes),
        }
    }
}

/// Convert an MSB-first binary string of 0/1 to decimal, any width.
fn binary_to_decimal(bits: &[u8]) -> String {
    // Little-endian base-1e9 limbs.
    let mut limbs: Vec<u32> = vec![0];
    for &c in bits {
        let bit = (c == b'1') as u64;
        let mut carry = bit;
        for limb in limbs.iter_mut() {
            let v = (*limb as u64) * 2 + carry;
            *limb = (v % 1_000_000_000) as u32;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry as u32);
        }
    }
    let mut s = String::new();
    for (i, limb) in limbs.iter().rev().enumerate() {
        if i == 0 {
            s.push_str(&limb.to_string());
        } else {
            s.push_str(&format!("{limb:09}"));
        }
    }
    s
}

struct UnsignedTranslator;
impl Translator for UnsignedTranslator {
    fn id(&self) -> &'static str {
        "udec"
    }
    fn name(&self) -> &'static str {
        "Unsigned decimal"
    }
    fn badge(&self) -> &'static str {
        "dec"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        shape.is_digital()
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        let Some(bits) = bits_of(value) else {
            return not_applicable();
        };
        if let Some(t) = non_binary_word(bits.as_bytes()) {
            return t;
        }
        Translated::normal(binary_to_decimal(bits.as_bytes()))
    }
}

struct SignedTranslator;
impl Translator for SignedTranslator {
    fn id(&self) -> &'static str {
        "sdec"
    }
    fn name(&self) -> &'static str {
        "Signed decimal"
    }
    fn badge(&self) -> &'static str {
        "sdec"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        matches!(shape, SignalShape::Vector { .. })
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        let Some(bits) = bits_of(value) else {
            return not_applicable();
        };
        let bytes = bits.as_bytes();
        if let Some(t) = non_binary_word(bytes) {
            return t;
        }
        if bytes.first() != Some(&b'1') {
            return Translated::normal(binary_to_decimal(bytes));
        }
        // Two's complement: magnitude = (~bits) + 1.
        let mut inverted: Vec<u8> = bytes
            .iter()
            .map(|&c| if c == b'1' { b'0' } else { b'1' })
            .collect();
        for c in inverted.iter_mut().rev() {
            if *c == b'0' {
                *c = b'1';
                break;
            }
            *c = b'0';
        }
        Translated::normal(format!("-{}", binary_to_decimal(&inverted)))
    }
}

fn half_to_f64(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let frac = (h & 0x3ff) as f64;
    match exp {
        0 => sign * frac * 2f64.powi(-24),
        0x1f => {
            if frac == 0.0 {
                sign * f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => sign * (1.0 + frac / 1024.0) * 2f64.powi(exp - 15),
    }
}

fn format_float(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        }
    } else if v == 0.0 {
        "0.0".into()
    } else if v.abs() >= 1e-4 && v.abs() < 1e9 {
        let s = format!("{v:.6}");
        let s = s.trim_end_matches('0');
        if s.ends_with('.') {
            format!("{s}0")
        } else {
            s.to_string()
        }
    } else {
        format!("{v:.5e}")
    }
}

struct FloatTranslator;
impl Translator for FloatTranslator {
    fn id(&self) -> &'static str {
        "float"
    }
    fn name(&self) -> &'static str {
        "IEEE 754 float"
    }
    fn badge(&self) -> &'static str {
        "flt"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        matches!(
            shape,
            SignalShape::Vector {
                width: 16 | 32 | 64
            }
        )
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        let Some(bits) = bits_of(value) else {
            return not_applicable();
        };
        let bytes = bits.as_bytes();
        if let Some(t) = non_binary_word(bytes) {
            return t;
        }
        let raw = bytes
            .iter()
            .fold(0u64, |acc, &c| (acc << 1) | (c == b'1') as u64);
        let v = match bytes.len() {
            16 => half_to_f64(raw as u16),
            32 => f32::from_bits(raw as u32) as f64,
            64 => f64::from_bits(raw),
            _ => return not_applicable(),
        };
        Translated::normal(format_float(v))
    }
}

struct RealTranslator;
impl Translator for RealTranslator {
    fn id(&self) -> &'static str {
        "real"
    }
    fn name(&self) -> &'static str {
        "Real"
    }
    fn badge(&self) -> &'static str {
        "real"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        shape == SignalShape::Real
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        match value {
            WaveValue::Real(v) => Translated::normal(format_float(*v)),
            _ => not_applicable(),
        }
    }
}

struct TextTranslator;
impl Translator for TextTranslator {
    fn id(&self) -> &'static str {
        "text"
    }
    fn name(&self) -> &'static str {
        "Text"
    }
    fn badge(&self) -> &'static str {
        "str"
    }
    fn applies(&self, shape: SignalShape) -> bool {
        shape == SignalShape::Text
    }
    fn translate(&self, value: &WaveValue) -> Translated {
        match value {
            WaveValue::Text(s) => Translated::normal(s.clone()),
            WaveValue::Bytes(bytes) => Translated::normal(format!("\"{}\"", bytes.escape_ascii())),
            _ => not_applicable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_text_bytes_are_unambiguously_escaped() {
        let translated = Translators::builtin()
            .get("text")
            .unwrap()
            .translate(&WaveValue::Bytes(vec![b'a', 0, 255, b'\\', b'\n', b'"']));
        assert_eq!(translated.text, "\"a\\x00\\xff\\\\\\n\\\"\"");
    }

    fn tr(id: &str, bits: &str) -> String {
        Translators::builtin()
            .get(id)
            .unwrap()
            .translate(&WaveValue::Bits(bits.into()))
            .text
    }

    #[test]
    fn hex_digitwise() {
        assert_eq!(tr("hex", "00111111"), "3f");
        assert_eq!(tr("hex", "1111"), "f");
        assert_eq!(tr("hex", "101"), "5");
        assert_eq!(tr("hex", "0011x1111111"), "3xf");
        assert_eq!(tr("hex", "zzzz0000"), "z0");
        assert_eq!(tr("hex", "zxzz0000"), "x0");
    }

    #[test]
    fn decimal() {
        assert_eq!(tr("udec", "0"), "0");
        assert_eq!(tr("udec", "11111111"), "255");
        assert_eq!(tr("sdec", "11111111"), "-1");
        assert_eq!(tr("sdec", "10000000"), "-128");
        assert_eq!(tr("sdec", "01111111"), "127");
        let big = "1".repeat(80);
        assert_eq!(tr("udec", &big), "1208925819614629174706175");
        assert_eq!(tr("udec", "xx01"), "UNDEF");
        assert_eq!(tr("sdec", "zz01"), "HIGHIMP");
    }

    #[test]
    fn floats() {
        assert_eq!(tr("float", &format!("{:032b}", 1.5f32.to_bits())), "1.5");
        assert_eq!(
            tr("float", &format!("{:064b}", (-0.25f64).to_bits())),
            "-0.25"
        );
        assert_eq!(tr("float", &format!("{:016b}", 0x3c00u16)), "1.0");
        assert_eq!(
            tr("float", &format!("{:032b}", f32::INFINITY.to_bits())),
            "+Inf"
        );
        let t = Translators::builtin();
        assert!(
            t.get("float")
                .unwrap()
                .applies(SignalShape::Vector { width: 32 })
        );
        assert!(
            !t.get("float")
                .unwrap()
                .applies(SignalShape::Vector { width: 8 })
        );
    }

    #[test]
    fn defaults() {
        let t = Translators::builtin();
        assert_eq!(t.default_for(SignalShape::Bit).id(), "bit");
        assert_eq!(t.default_for(SignalShape::Vector { width: 8 }).id(), "hex");
        assert_eq!(t.applicable(SignalShape::Bit).len(), 4);
    }
}
