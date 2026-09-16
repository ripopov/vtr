//! Immutable packed signal payload shared by the remote receiver and renderer.
//! Offsets include the initial value followed by every recorded change.

use crate::data::{Bit, SignalHistory, SignalShape, WaveValue};
use serde::{Deserialize, Serialize};

const LOGIC: &[u8; 9] = b"01xzuwlh-";
pub mod stream;

#[derive(Debug, Serialize)]
pub struct PackedHistory {
    shape: SignalShape,
    times: Vec<u64>,
    offsets: Vec<u64>,
    #[serde(serialize_with = "super::serialize_bytes")]
    data: Vec<u8>,
    #[serde(skip)]
    reservation: Option<super::memory::Reservation>,
}

impl<'de> Deserialize<'de> for PackedHistory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Storage {
            shape: SignalShape,
            times: Vec<u64>,
            offsets: Vec<u64>,
            data: Vec<u8>,
        }
        let Storage {
            shape,
            times,
            offsets,
            data,
        } = Storage::deserialize(deserializer)?;
        let result = Self {
            shape,
            times,
            offsets,
            data,
            reservation: None,
        };
        result.validate().map_err(serde::de::Error::custom)?;
        Ok(result)
    }
}

fn stride(shape: SignalShape) -> Option<usize> {
    match shape {
        SignalShape::Bit | SignalShape::Event => Some(2),
        SignalShape::Vector { width } => Some(1 + width.div_ceil(2) as usize),
        SignalShape::Real => Some(9),
        SignalShape::Text => None,
    }
}

impl PackedHistory {
    pub fn from_history(history: &dyn SignalHistory) -> anyhow::Result<Self> {
        Self::from_history_with_limit(history, u64::MAX)
    }

    pub fn from_history_with_limit(
        history: &dyn SignalHistory,
        limit: u64,
    ) -> anyhow::Result<Self> {
        let count = history.len() as u64;
        let shape_bytes = if matches!(history.shape(), SignalShape::Vector { .. }) {
            8u64
        } else {
            4
        };
        let offset_bytes = if history.shape() == SignalShape::Text {
            count.checked_add(2).and_then(|n| n.checked_mul(8))
        } else {
            Some(0)
        }
        .ok_or_else(|| anyhow::anyhow!("history size overflow"))?;
        let overhead = count
            .checked_mul(8)
            .and_then(|n| n.checked_add(offset_bytes))
            .and_then(|n| n.checked_add(shape_bytes + 24))
            .ok_or_else(|| anyhow::anyhow!("history size overflow"))?;
        let minimum_data = count
            .checked_add(1)
            .and_then(|n| n.checked_mul(stride(history.shape()).unwrap_or(1) as u64))
            .ok_or_else(|| anyhow::anyhow!("history size overflow"))?;
        anyhow::ensure!(
            overhead
                .checked_add(minimum_data)
                .is_some_and(|n| n <= limit),
            "signal exceeds transfer limit"
        );
        let data_limit = limit - overhead;
        let mut result = Self {
            reservation: None,
            shape: history.shape(),
            times: Vec::with_capacity(history.len()),
            offsets: Vec::with_capacity(if history.shape() == SignalShape::Text {
                history
                    .len()
                    .checked_add(2)
                    .ok_or_else(|| anyhow::anyhow!("history too large"))?
            } else {
                0
            }),
            data: vec![],
        };
        result.push_value(history.value(None), data_limit)?;
        for i in 0..history.len() {
            result.times.push(history.time(i));
            result.push_value(history.value(Some(i)), data_limit)?;
        }
        if result.shape == SignalShape::Text {
            result.offsets.push(result.data.len() as u64);
        }
        result.validate()?;
        Ok(result)
    }

    fn push_value(&mut self, value: WaveValue, limit: u64) -> anyhow::Result<()> {
        let length = stride(self.shape).unwrap_or_else(|| match &value {
            WaveValue::Text(value) => value.len().saturating_add(1),
            WaveValue::Bytes(value) => value.len().saturating_add(1),
            _ => 1,
        });
        anyhow::ensure!(
            (self.data.len() as u64)
                .checked_add(length as u64)
                .is_some_and(|n| n <= limit),
            "signal exceeds transfer limit"
        );
        let start = self.data.len();
        if self.shape == SignalShape::Text {
            self.offsets.push(start as u64);
        }
        match value {
            WaveValue::Unavailable => self.data.push(0),
            WaveValue::Bits(bits) => {
                anyhow::ensure!(
                    bits.len() as u64 == self.shape.width().max(1) as u64,
                    "logic width mismatch"
                );
                self.data.push(1);
                for pair in bits.as_bytes().chunks(2) {
                    let code = |byte: u8| {
                        LOGIC
                            .iter()
                            .position(|&b| b == byte.to_ascii_lowercase())
                            .map(|n| n as u8)
                            .ok_or_else(|| anyhow::anyhow!("invalid logic character"))
                    };
                    let low = code(pair[0])?;
                    let high = if pair.len() == 2 { code(pair[1])? } else { 0 };
                    self.data.push(low | (high << 4));
                }
            }
            WaveValue::Real(value) => {
                self.data.push(2);
                self.data.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            WaveValue::Text(value) => {
                self.data.push(3);
                self.data.extend_from_slice(value.as_bytes());
            }
            WaveValue::Bytes(value) => {
                self.data.push(4);
                self.data.extend(value);
            }
        }
        if let Some(stride) = stride(self.shape) {
            anyhow::ensure!(
                self.data.len() - start <= stride,
                "value exceeds fixed storage size"
            );
            self.data.resize(start + stride, 0);
        }
        Ok(())
    }

    /// Required before any deserialized payload is exposed to SignalHistory.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !matches!(self.shape, SignalShape::Vector { width: 0 | 1 }),
            "invalid vector width"
        );
        anyhow::ensure!(
            self.times.windows(2).all(|t| t[0] <= t[1]),
            "history times are not ordered"
        );
        if let Some(stride) = stride(self.shape) {
            let size = self
                .times
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_mul(stride));
            anyhow::ensure!(
                size == Some(self.data.len()) && self.offsets.is_empty(),
                "invalid fixed history storage"
            );
        } else {
            anyhow::ensure!(
                self.offsets.len().checked_sub(2) == Some(self.times.len()),
                "incorrect history offset count"
            );
            anyhow::ensure!(
                self.offsets.first() == Some(&0)
                    && self.offsets.last() == Some(&(self.data.len() as u64)),
                "invalid history storage bounds"
            );
            for bounds in self.offsets.windows(2) {
                anyhow::ensure!(
                    bounds[0] < bounds[1] && bounds[1] <= self.data.len() as u64,
                    "invalid value offsets"
                );
            }
        }
        for index in 0..=self.times.len() {
            let data = self.raw(index.checked_sub(1));
            match data[0] {
                0 => anyhow::ensure!(
                    data[1..].iter().all(|&b| b == 0),
                    "invalid unavailable value padding"
                ),
                1 => {
                    let width = self.shape.width().max(1) as u64;
                    anyhow::ensure!(
                        matches!(
                            self.shape,
                            SignalShape::Bit | SignalShape::Vector { .. } | SignalShape::Event
                        ),
                        "logic value in incompatible signal"
                    );
                    anyhow::ensure!(
                        width.div_ceil(2) == (data.len() - 1) as u64,
                        "incorrect packed logic length"
                    );
                    for (i, &b) in data[1..].iter().enumerate() {
                        anyhow::ensure!((b & 15) < 9, "invalid logic state");
                        if (i as u64 * 2 + 1) < width {
                            anyhow::ensure!(b >> 4 < 9, "invalid logic state");
                        } else {
                            anyhow::ensure!(b >> 4 == 0, "invalid logic padding");
                        }
                    }
                }
                2 => anyhow::ensure!(
                    self.shape == SignalShape::Real && data.len() == 9,
                    "invalid real value"
                ),
                3 => {
                    anyhow::ensure!(
                        self.shape == SignalShape::Text,
                        "text in incompatible signal"
                    );
                    std::str::from_utf8(&data[1..])?;
                }
                4 => anyhow::ensure!(
                    self.shape == SignalShape::Text,
                    "bytes in incompatible signal"
                ),
                _ => anyhow::bail!("unknown value tag"),
            }
        }
        Ok(())
    }

    pub fn storage_bytes(&self) -> usize {
        self.times.capacity() * 8 + self.offsets.capacity() * 8 + self.data.capacity()
    }

    fn raw(&self, i: Option<usize>) -> &[u8] {
        let value = i.map_or(0, |i| i + 1);
        if let Some(stride) = stride(self.shape) {
            return &self.data[value * stride..(value + 1) * stride];
        }
        &self.data[self.offsets[value] as usize..self.offsets[value + 1] as usize]
    }
}

impl SignalHistory for PackedHistory {
    fn shape(&self) -> SignalShape {
        self.shape
    }
    fn len(&self) -> usize {
        self.times.len()
    }
    fn time(&self, i: usize) -> u64 {
        self.times[i]
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        let data = self.raw(i);
        match data[0] {
            0 => WaveValue::Unavailable,
            1 => {
                let width = self.shape.width().max(1) as usize;
                let text: String = (0..width)
                    .map(|i| {
                        let byte = data[1 + i / 2];
                        LOGIC[if i % 2 == 0 { byte & 15 } else { byte >> 4 } as usize] as char
                    })
                    .collect();
                WaveValue::Bits(text)
            }
            2 => WaveValue::Real(f64::from_bits(u64::from_le_bytes(
                data[1..].try_into().expect("validated real"),
            ))),
            3 => WaveValue::Text(
                std::str::from_utf8(&data[1..])
                    .expect("validated UTF-8")
                    .into(),
            ),
            4 => WaveValue::Bytes(data[1..].to_vec()),
            _ => unreachable!("validated value tag"),
        }
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        let data = self.raw(i);
        match data[0] {
            0 => Bit::Unavailable,
            1 if self.shape == SignalShape::Bit => Bit::from_ascii(LOGIC[(data[1] & 15) as usize]),
            _ => Bit::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::history::VecHistory;
    #[test]
    fn packed_values_preserve_shapes_states_initial_and_same_time_changes() {
        for (shape, values) in [
            (
                SignalShape::Bit,
                LOGIC
                    .iter()
                    .map(|&b| WaveValue::Bits((b as char).to_string()))
                    .collect(),
            ),
            (
                SignalShape::Vector { width: 9 },
                vec![WaveValue::Bits("01xzuwlh-".into())],
            ),
            (
                SignalShape::Text,
                vec![
                    WaveValue::Text("héllo".into()),
                    WaveValue::Bytes(vec![0, 255]),
                    WaveValue::Bytes(vec![]),
                ],
            ),
            (
                SignalShape::Real,
                vec![WaveValue::Real(-0.0), WaveValue::Real(f64::INFINITY)],
            ),
            (
                SignalShape::Event,
                vec![WaveValue::Bits("1".into()), WaveValue::Bits("1".into())],
            ),
        ] {
            let source = VecHistory {
                shape,
                times: vec![7; values.len()],
                values,
                initial: WaveValue::Unavailable,
            };
            let packed = PackedHistory::from_history(&source).unwrap();
            assert_eq!(packed.value(None), source.value(None));
            assert_eq!(packed.index_at(6), None);
            assert_eq!(packed.index_at(7), Some(source.len() - 1));
            for i in 0..source.len() {
                assert_eq!(packed.value(Some(i)), source.value(Some(i)));
                if shape == SignalShape::Bit {
                    assert_eq!(packed.bit(Some(i)), source.bit(Some(i)));
                }
            }
        }
    }

    #[test]
    fn invalid_storage_never_reaches_renderer() {
        let source = VecHistory {
            shape: SignalShape::Bit,
            times: vec![1],
            values: vec![WaveValue::Bits("1".into())],
            initial: WaveValue::Unavailable,
        };
        let mut packed = PackedHistory::from_history(&source).unwrap();
        assert!(PackedHistory::from_history_with_limit(&source, 1).is_err());
        *packed.data.last_mut().unwrap() = 15;
        assert!(packed.validate().is_err());
        packed.offsets.push(u64::MAX);
        assert!(packed.validate().is_err());
    }
}
