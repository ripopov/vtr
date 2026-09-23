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
