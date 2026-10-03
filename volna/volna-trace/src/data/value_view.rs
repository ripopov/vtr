//! Borrowed signal values for bounded projection. Reading a preview never
//! formats or clones the complete resident value.

use std::borrow::Cow;

use super::WaveValue;

#[derive(Clone, Debug)]
pub struct LogicView<'a> {
    pub width: usize,
    storage: LogicStorage<'a>,
}

#[derive(Clone, Debug)]
enum LogicStorage<'a> {
    Ascii(Cow<'a, [u8]>),
    PackedLsb { states: u8, data: &'a [u8] },
    NibblesMsb(&'a [u8]),
}

impl<'a> LogicView<'a> {
    pub fn ascii(data: impl Into<Cow<'a, [u8]>>) -> Self {
        let data = data.into();
        Self {
            width: data.len(),
            storage: LogicStorage::Ascii(data),
        }
    }
    pub fn packed_lsb(width: u32, states: u8, data: &'a [u8]) -> Self {
        Self {
            width: width as usize,
            storage: LogicStorage::PackedLsb { states, data },
        }
    }
    pub(crate) fn nibbles_msb(width: usize, data: &'a [u8]) -> Self {
        Self {
            width,
            storage: LogicStorage::NibblesMsb(data),
        }
    }
    /// The bits as an unsigned integer, when every bit is 0 or 1 and the
    /// vector fits in 64 bits. Packed two-state storage reads whole bytes.
    pub fn to_u64(&self) -> Option<u64> {
        let w = self.width;
        if w == 0 || w > 64 {
            return None;
        }
        let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
        match &self.storage {
            LogicStorage::PackedLsb { states: 2, data } => {
                let raw = data
                    .iter()
                    .take(8)
                    .enumerate()
                    .fold(0u64, |acc, (i, b)| acc | (u64::from(*b) << (8 * i)));
                Some(raw & mask)
            }
            LogicStorage::PackedLsb { states: 4, data } => {
                let mut raw = 0u64;
                for i in 0..w {
                    match (data[i >> 2] >> ((i & 3) * 2)) & 3 {
                        0 => {}
                        1 => raw |= 1 << i,
                        _ => return None,
                    }
                }
                Some(raw)
            }
            _ => {
                let mut raw = 0u64;
                for i in 0..w {
                    raw = (raw << 1)
                        | match self.bit(i) {
                            b'0' => 0,
                            b'1' => 1,
                            _ => return None,
                        };
                }
                Some(raw)
            }
        }
    }

    /// The value's kind, with the priority of [`super::value::kind_of_bits`].
    /// Two-state storage is always normal.
    pub fn kind(&self) -> super::ValueKind {
        match &self.storage {
            LogicStorage::Ascii(data) => super::value::kind_of_bits(data),
            LogicStorage::PackedLsb { states: 2, .. } => super::ValueKind::Normal,
            // Codes 0 and 1 are the only normal ones: a packed value whose
            // slots never exceed 1 is normal without decoding a bit.
            LogicStorage::PackedLsb { states, data } => {
                let (per_byte, mask) = if *states == 4 { (4, 0xaa) } else { (2, 0xee) };
                let (full, rest) = (self.width / per_byte, self.width % per_byte);
                let tail = if rest == 0 {
                    0
                } else {
                    data[full] & mask & ((1u16 << (rest * 8 / per_byte)) - 1) as u8
                };
                if tail == 0 && data[..full].iter().all(|b| b & mask == 0) {
                    super::ValueKind::Normal
                } else {
                    super::value::kind_of_codes((0..self.width).map(|i| self.bit(i)))
                }
            }
            _ => super::value::kind_of_codes((0..self.width).map(|i| self.bit(i))),
        }
    }

    /// Append the value as logic codes of [`vtr::signal::CODE_ASCII`] in
    /// nibbles, most significant bit first and in the low nibble of each
    /// byte: the remote wire layout. Fails on a character outside the table.
    pub(crate) fn write_nibbles_msb(&self, out: &mut Vec<u8>) -> Option<()> {
        let w = self.width;
        out.reserve(w.div_ceil(2));
        match &self.storage {
            LogicStorage::Ascii(data) => {
                let code = |byte: u8| {
                    let byte = byte.to_ascii_lowercase();
                    vtr::signal::CODE_ASCII.iter().position(|&c| c == byte)
                };
                for pair in data.chunks(2) {
                    let low = code(pair[0])? as u8;
                    let high = match pair.get(1) {
                        Some(&b) => code(b)? as u8,
                        None => 0,
                    };
                    out.push(low | (high << 4));
                }
            }
            LogicStorage::PackedLsb { states, data } => {
                let code = |i: usize| vtr::signal::get_code(data, *states, w - 1 - i);
                for i in (0..w).step_by(2) {
                    let high = if i + 1 < w { code(i + 1) } else { 0 };
                    out.push(code(i) | (high << 4));
                }
            }
            LogicStorage::NibblesMsb(data) => {
                let bytes = &data[..w.div_ceil(2)];
                out.extend_from_slice(bytes);
                if w % 2 == 1 {
                    *out.last_mut().expect("a nibble") &= 0x0f;
                }
            }
        }
        Some(())
    }

    /// MSB-first ASCII logic code. Storage was validated by its owner.
    pub fn bit(&self, index: usize) -> u8 {
        assert!(index < self.width);
        match &self.storage {
            LogicStorage::Ascii(data) => data[index].to_ascii_lowercase(),
            LogicStorage::PackedLsb { states, data } => {
                vtr::signal::CODE_ASCII
                    [vtr::signal::get_code(data, *states, self.width - 1 - index) as usize]
            }
            LogicStorage::NibblesMsb(data) => {
                let byte = data[index / 2];
                vtr::signal::CODE_ASCII[if index.is_multiple_of(2) {
                    byte & 15
                } else {
                    byte >> 4
                } as usize]
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum ValueView<'a> {
    Unavailable,
    Logic(LogicView<'a>),
    Real(f64),
    Text(Cow<'a, str>),
    Bytes(Cow<'a, [u8]>),
}

impl<'a> ValueView<'a> {
    pub fn borrowed(value: &'a WaveValue) -> Self {
        match value {
            WaveValue::Unavailable => Self::Unavailable,
            WaveValue::Bits(value) => Self::Logic(LogicView::ascii(value.as_bytes())),
            WaveValue::Real(value) => Self::Real(*value),
            WaveValue::Text(value) => Self::Text(Cow::Borrowed(value)),
            WaveValue::Bytes(value) => Self::Bytes(Cow::Borrowed(value)),
        }
    }
    /// The kind of the value, as [`WaveValue::kind`] classifies it.
    pub fn kind(&self) -> super::ValueKind {
        match self {
            Self::Unavailable => super::ValueKind::Undef,
            Self::Logic(logic) => logic.kind(),
            Self::Real(_) | Self::Text(_) | Self::Bytes(_) => super::ValueKind::Normal,
        }
    }

    pub fn owned(value: WaveValue) -> Self {
        match value {
            WaveValue::Unavailable => Self::Unavailable,
            WaveValue::Bits(value) => Self::Logic(LogicView::ascii(value.into_bytes())),
            WaveValue::Real(value) => Self::Real(value),
            WaveValue::Text(value) => Self::Text(Cow::Owned(value)),
            WaveValue::Bytes(value) => Self::Bytes(Cow::Owned(value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ValueKind;

    #[test]
    fn packed_kinds_match_their_ascii_spelling() {
        for text in [
            "0101", "01x1", "z000", "10-1", "0l01", "x", "1", "01010", "0101x",
        ] {
            for states in [4u8, 9] {
                let mut data = vec![0u8; text.len()];
                for (i, c) in text.bytes().rev().enumerate() {
                    vtr::signal::set_code(&mut data, states, i, vtr::signal::code_from_ascii(c));
                }
                let packed = LogicView::packed_lsb(text.len() as u32, states, &data);
                let expected = crate::data::value::kind_of_bits(text.as_bytes());
                if states == 4 && !text.bytes().all(|c| b"01xz".contains(&c)) {
                    continue;
                }
                assert_eq!(packed.kind(), expected, "{text} in {states} states");
            }
        }
        assert_eq!(LogicView::ascii(b"0x".as_slice()).kind(), ValueKind::Undef);
    }

    #[test]
    fn every_storage_writes_the_same_wire_nibbles() {
        for text in [
            "0",
            "1",
            "01",
            "10110",
            "0101x",
            "z0h-",
            "uwl01",
            "1111000011",
        ] {
            let expected = {
                let mut out = Vec::new();
                LogicView::ascii(text.as_bytes())
                    .write_nibbles_msb(&mut out)
                    .unwrap();
                out
            };
            for states in [2u8, 4, 9] {
                let fits = match states {
                    2 => text.bytes().all(|c| b"01".contains(&c)),
                    4 => text.bytes().all(|c| b"01xz".contains(&c)),
                    _ => true,
                };
                if !fits {
                    continue;
                }
                let mut data = vec![0u8; text.len()];
                for (i, c) in text.bytes().rev().enumerate() {
                    vtr::signal::set_code(&mut data, states, i, vtr::signal::code_from_ascii(c));
                }
                let mut out = Vec::new();
                LogicView::packed_lsb(text.len() as u32, states, &data)
                    .write_nibbles_msb(&mut out)
                    .unwrap();
                assert_eq!(out, expected, "{text} in {states} states");
            }
            let mut out = Vec::new();
            LogicView::nibbles_msb(text.len(), &expected)
                .write_nibbles_msb(&mut out)
                .unwrap();
            assert_eq!(out, expected, "{text} as nibbles");
        }
        assert!(
            LogicView::ascii(b"0q".as_slice())
                .write_nibbles_msb(&mut Vec::new())
                .is_none()
        );
    }
}
