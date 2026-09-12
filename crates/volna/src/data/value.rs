//! Signal value representation shared by histories, translators and the renderer.

/// One logic bit, reduced to what the waveform renderer needs to draw it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Bit {
    Zero,
    One,
    /// Unknown (`x`, `u`, `w`).
    X,
    /// High impedance (`z`).
    Z,
    /// Any other VHDL-style code (`l`, `h`, `-`).
    Other,
}

impl Bit {
    /// Map a VCD/VHDL logic character to a [`Bit`].
    pub fn from_ascii(c: u8) -> Bit {
        match c {
            b'0' => Bit::Zero,
            b'1' => Bit::One,
            b'x' | b'X' | b'u' | b'U' | b'w' | b'W' => Bit::X,
            b'z' | b'Z' => Bit::Z,
            _ => Bit::Other,
        }
    }

    pub fn kind(self) -> ValueKind {
        match self {
            Bit::Zero | Bit::One => ValueKind::Normal,
            Bit::X => ValueKind::Undef,
            Bit::Z => ValueKind::HighImp,
            Bit::Other => ValueKind::Weak,
        }
    }
}

/// Classification of a value used to pick its colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ValueKind {
    Normal,
    Undef,
    HighImp,
    DontCare,
    Weak,
}

/// The storage shape of a signal, independent of the HDL variable type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum SignalShape {
    /// A single logic bit.
    Bit,
    /// A vector of `width` logic bits (`width >= 2`).
    Vector { width: u32 },
    /// An IEEE double.
    Real,
    /// A variable-length string.
    Text,
}

impl SignalShape {
    pub fn dims(self) -> String {
        match self {
            Self::Vector { width } => format!("[{}:0]", width - 1),
            Self::Real => "real".into(),
            Self::Text => "str".into(),
            Self::Bit => String::new(),
        }
    }

    pub fn width(self) -> u32 {
        match self {
            SignalShape::Bit => 1,
            SignalShape::Vector { width } => width,
            SignalShape::Real => 64,
            SignalShape::Text => 0,
        }
    }

    pub fn is_digital(self) -> bool {
        matches!(self, SignalShape::Bit | SignalShape::Vector { .. })
    }
}

/// An owned signal value handed to translators.
#[derive(Clone, PartialEq, Debug)]
pub enum WaveValue {
    /// MSB-first logic characters from the alphabet `01xzuwlh-`.
    Bits(String),
    Real(f64),
    Text(String),
}

impl WaveValue {
    pub fn kind(&self) -> ValueKind {
        match self {
            WaveValue::Bits(s) => kind_of_bits(s.as_bytes()),
            WaveValue::Real(_) | WaveValue::Text(_) => ValueKind::Normal,
        }
    }
}

/// Kind of a bit string, with the priority `x > z > - > weak`, like Surfer.
pub fn kind_of_bits(bits: &[u8]) -> ValueKind {
    let mut kind = ValueKind::Normal;
    for &c in bits {
        let k = match c {
            b'0' | b'1' => continue,
            b'x' | b'X' | b'u' | b'U' | b'w' | b'W' => return ValueKind::Undef,
            b'z' | b'Z' => ValueKind::HighImp,
            b'-' => ValueKind::DontCare,
            _ => ValueKind::Weak,
        };
        kind = match (kind, k) {
            (ValueKind::HighImp, _) => ValueKind::HighImp,
            (_, ValueKind::HighImp) => ValueKind::HighImp,
            (ValueKind::DontCare, _) => ValueKind::DontCare,
            (_, k) => k,
        };
    }
    kind
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_kinds() {
        assert_eq!(Bit::from_ascii(b'1'), Bit::One);
        assert_eq!(Bit::from_ascii(b'u'), Bit::X);
        assert_eq!(Bit::from_ascii(b'z').kind(), ValueKind::HighImp);
        assert_eq!(Bit::from_ascii(b'-'), Bit::Other);
    }

    #[test]
    fn kind_priority() {
        assert_eq!(kind_of_bits(b"0101"), ValueKind::Normal);
        assert_eq!(kind_of_bits(b"0z01"), ValueKind::HighImp);
        assert_eq!(kind_of_bits(b"xz01"), ValueKind::Undef);
        assert_eq!(kind_of_bits(b"zx01"), ValueKind::Undef);
        assert_eq!(kind_of_bits(b"-l01"), ValueKind::DontCare);
        assert_eq!(kind_of_bits(b"hl"), ValueKind::Weak);
    }
}
