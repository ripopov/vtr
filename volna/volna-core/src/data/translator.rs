//! Value translators turn a [`WaveValue`] into display text.
//!
//! Adding a translator means implementing [`Translator`] and registering it
//! with [`Translators::register`]; nothing else in the viewer changes.

use std::sync::Arc;

use super::value::{SignalShape, ValueKind, WaveValue, kind_of_bits};
use super::value_view::ValueView;

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
    /// How the translator reads a value as a number, for analog drawing.
    /// `None` for translators that do not show a number.
    fn numeric_kind(&self) -> Option<NumericKind> {
        None
    }
    /// The number this translator shows for `value`; `None` for undefined
    /// (X, Z, non-finite) values and non-numeric translators.
    fn numeric(&self, value: &ValueView<'_>) -> Option<f64> {
        self.numeric_kind()?.read(value)
    }
    /// The full range of numbers a signal of `shape` can show.
    fn limits(&self, shape: SignalShape) -> Option<(f64, f64)> {
        self.numeric_kind()?.limits(shape)
    }
}

/// How a numeric translator reads logic or real values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NumericKind {
    /// The bits as an unsigned integer (hexadecimal, binary, unsigned).
    Unsigned,
    /// The bits as a two's-complement integer.
    Signed,
    /// The bits as an IEEE 754 half, single or double.
    Float,
    /// An IEEE double signal.
    Real,
}

impl NumericKind {
    pub fn read(self, value: &ValueView<'_>) -> Option<f64> {
        let bits = match (self, value) {
            (Self::Real, ValueView::Real(v)) => return v.is_finite().then_some(*v),
            (Self::Real, _) => return None,
            (_, ValueView::Logic(bits)) if bits.width > 0 => bits,
            _ => return None,
        };
        let w = bits.width;
        let bit = |i: usize| match bits.bit(i) {
            b'0' => Some(0u64),
            b'1' => Some(1),
            _ => None,
        };
        if w > 64 {
            // Wide vectors lose precision beyond 53 bits, as any f64 plot does.
            let mut acc = 0.0f64;
            for i in 0..w {
                acc = acc * 2.0 + bit(i)? as f64;
            }
            return match self {
                Self::Unsigned => Some(acc),
                Self::Signed if bits.bit(0) == b'1' => Some(acc - 2f64.powi(w as i32)),
                Self::Signed => Some(acc),
                _ => None,
            };
        }
        let raw = bits.to_u64()?;
        let v = match self {
            Self::Unsigned => raw as f64,
            Self::Signed if w == 64 => raw as i64 as f64,
            Self::Signed if (raw >> (w - 1)) & 1 == 1 => raw as f64 - 2f64.powi(w as i32),
            Self::Signed => raw as f64,
            Self::Float => match w {
                16 => half_to_f64(raw as u16),
                32 => f32::from_bits(raw as u32) as f64,
                64 => f64::from_bits(raw),
                _ => return None,
            },
            Self::Real => unreachable!(),
        };
        v.is_finite().then_some(v)
    }

    /// The full range of an integer reading; floats have none.
    pub fn limits(self, shape: SignalShape) -> Option<(f64, f64)> {
        let width = match shape {
            SignalShape::Bit => 1,
            SignalShape::Vector { width } => width,
            _ => return None,
        };
        match self {
            Self::Unsigned => Some((0.0, 2f64.powi(width as i32) - 1.0)),
            Self::Signed => {
                let half = 2f64.powi(width as i32 - 1);
                Some((-half, half - 1.0))
            }
            Self::Float | Self::Real => None,
        }
    }

    /// The value that reads back as `v` (rounded to an integer for integer
    /// readings), so a translator can format axis labels like its values.
    pub fn value_of(self, v: f64, shape: SignalShape) -> Option<WaveValue> {
        match (self, shape) {
            (Self::Real, SignalShape::Real) => Some(WaveValue::Real(v)),
            (Self::Unsigned | Self::Signed, SignalShape::Vector { width }) if width <= 127 => {
                let n = v.round().clamp(i128::MIN as f64, i128::MAX as f64) as i128;
                Some(WaveValue::Bits(
                    (0..width)
                        .rev()
                        .map(|i| if (n >> i) & 1 == 1 { '1' } else { '0' })
                        .collect(),
                ))
            }
            (Self::Float, SignalShape::Vector { width: 32 }) => {
                Some(WaveValue::Bits(format!("{:032b}", (v as f32).to_bits())))
            }
            (Self::Float, SignalShape::Vector { width: 64 }) => {
                Some(WaveValue::Bits(format!("{:064b}", v.to_bits())))
            }
            _ => None,
        }
    }
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
    fn numeric_kind(&self) -> Option<NumericKind> {
        Some(NumericKind::Unsigned)
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
    fn numeric_kind(&self) -> Option<NumericKind> {
        Some(NumericKind::Unsigned)
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
    fn numeric_kind(&self) -> Option<NumericKind> {
        Some(NumericKind::Unsigned)
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
    fn numeric_kind(&self) -> Option<NumericKind> {
        Some(NumericKind::Signed)
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
    fn numeric_kind(&self) -> Option<NumericKind> {
        Some(NumericKind::Float)
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
    fn numeric_kind(&self) -> Option<NumericKind> {
        Some(NumericKind::Real)
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
    fn packed_logic_reads_as_numbers() {
        use crate::data::value_view::LogicView;
        // Bits above the width are ignored; bit 11 set makes it negative.
        let two = LogicView::packed_lsb(12, 2, &[0x34, 0xfa]);
        assert_eq!(two.to_u64(), Some(0xa34));
        assert_eq!(
            NumericKind::Signed.read(&ValueView::Logic(two)),
            Some(0xa34 as f64 - 4096.0)
        );
        // Four states: codes 0/1 in two-bit fields, LSB first; 2 is X.
        let four = LogicView::packed_lsb(4, 4, &[0b01_00_01_01]);
        assert_eq!(four.to_u64(), Some(0b1011));
        assert_eq!(LogicView::packed_lsb(4, 4, &[0b10_00_01_01]).to_u64(), None);
    }

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
