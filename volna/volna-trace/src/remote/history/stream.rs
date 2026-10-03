//! Incremental decoder for PackedHistory's fixed bincode layout. Both input
//! and validation steps are bounded; finishing never rescans the whole history.

use super::{PackedHistory, stride};
use crate::data::SignalShape;
use crate::remote::transport::DATA_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Shape,
    Width,
    TimesLen,
    Times,
    OffsetsLen,
    Offsets,
    DataLen,
    Data,
    Validate,
    Ready,
    Failed,
}

pub struct HistoryDecoder {
    history: PackedHistory,
    stage: Stage,
    scalar: [u8; 8],
    scalar_len: usize,
    remaining: usize,
    declared: u64,
    received: u64,
    value: usize,
    position: usize,
    utf8: Utf8,
}

impl HistoryDecoder {
    fn new(declared_bytes: u64, max_bytes: u64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            declared_bytes <= max_bytes && declared_bytes >= 28,
            "signal exceeds limit or has invalid size"
        );
        Ok(Self {
            history: PackedHistory {
                shape: SignalShape::Event,
                times: vec![],
                offsets: vec![],
                data: vec![],
                reservation: None,
            },
            stage: Stage::Shape,
            scalar: [0; 8],
            scalar_len: 0,
            remaining: 0,
            declared: declared_bytes,
            received: 0,
            value: 0,
            position: 0,
            utf8: Utf8::default(),
        })
    }

    /// Admit the complete decoded arrays before allocating them. The fixed
    /// struct allowance includes vector descriptors and the reservation itself;
    /// transport/host scratch must be reserved separately by the executor.
    pub fn with_budget(
        declared_bytes: u64,
        max_bytes: u64,
        budget: &crate::remote::memory::MemoryBudget,
    ) -> anyhow::Result<Self> {
        let bytes = declared_bytes
            .checked_add(std::mem::size_of::<PackedHistory>() as u64)
            .and_then(|n| n.checked_add(2 * std::mem::size_of::<usize>() as u64))
            .ok_or_else(|| anyhow::anyhow!("signal memory size overflow"))?;
        let reservation = budget.reserve(bytes)?;
        let mut decoder = Self::new(declared_bytes, max_bytes)?;
        decoder.history.reservation = Some(reservation);
        Ok(decoder)
    }

    /// Consume at most one transport data chunk. Errors permanently invalidate
    /// the builder; the caller must discard it and retry the complete object.
    pub fn feed(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let result = self.feed_inner(bytes);
        if result.is_err() {
            self.stage = Stage::Failed;
        }
        result
    }

    fn feed_inner(&mut self, mut bytes: &[u8]) -> anyhow::Result<()> {
        anyhow::ensure!(
            bytes.len() <= DATA_BYTES,
            "history input chunk exceeds limit"
        );
        anyhow::ensure!(
            !matches!(self.stage, Stage::Failed | Stage::Ready | Stage::Validate),
            "unexpected history bytes"
        );
        self.received = self
            .received
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("history size overflow"))?;
        anyhow::ensure!(
            self.received <= self.declared,
            "history exceeds declared bytes"
        );
        while !bytes.is_empty() {
            if self.stage == Stage::Data {
                let n = bytes.len().min(self.remaining);
                self.history.data.extend_from_slice(&bytes[..n]);
                self.remaining -= n;
                bytes = &bytes[n..];
                if self.remaining == 0 {
                    self.stage = Stage::Validate;
                }
                continue;
            }
            anyhow::ensure!(
                !matches!(self.stage, Stage::Validate | Stage::Ready),
                "trailing history bytes"
            );
            let width = if matches!(self.stage, Stage::Shape | Stage::Width) {
                4
            } else {
                8
            };
            let n = bytes.len().min(width - self.scalar_len);
            self.scalar[self.scalar_len..self.scalar_len + n].copy_from_slice(&bytes[..n]);
            self.scalar_len += n;
            bytes = &bytes[n..];
            if self.scalar_len != width {
                continue;
            }
            let value = if width == 4 {
                u32::from_le_bytes(self.scalar[..4].try_into()?) as u64
            } else {
                u64::from_le_bytes(self.scalar)
            };
            self.scalar_len = 0;
            self.scalar_value(value)?;
        }
        if self.stage == Stage::Validate {
            anyhow::ensure!(
                self.received == self.declared,
                "history shorter than declared size"
            );
        }
        Ok(())
    }

    fn scalar_value(&mut self, value: u64) -> anyhow::Result<()> {
        match self.stage {
            Stage::Shape => {
                self.history.shape = match value {
                    0 => SignalShape::Event,
                    1 => SignalShape::Bit,
                    2 => SignalShape::Vector { width: 2 },
                    3 => SignalShape::Real,
                    4 => SignalShape::Text,
                    _ => anyhow::bail!("unknown signal shape"),
                };
                self.stage = if value == 2 {
                    Stage::Width
                } else {
                    Stage::TimesLen
                };
            }
            Stage::Width => {
                anyhow::ensure!(value >= 2, "invalid vector width");
                self.history.shape = SignalShape::Vector {
                    width: value as u32,
                };
                self.stage = Stage::TimesLen;
            }
            Stage::TimesLen | Stage::OffsetsLen => {
                anyhow::ensure!(
                    value <= self.declared / 8,
                    "history array exceeds declared size"
                );
                let count = usize::try_from(value)?;
                if self.stage == Stage::TimesLen {
                    // Check all mandatory storage before reserving any array.
                    // Text needs two offset sentinels and at least one tag per
                    // value; fixed values include their padding in the stride.
                    let values = value.checked_add(1);
                    let minimum = value.checked_mul(8).and_then(|times| {
                        let storage = if let Some(stride) = stride(self.history.shape) {
                            values?.checked_mul(stride as u64)?
                        } else {
                            value.checked_add(2)?.checked_mul(8)?.checked_add(values?)?
                        };
                        times.checked_add(storage)?.checked_add(
                            28 + u64::from(matches!(
                                self.history.shape,
                                SignalShape::Vector { .. }
                            )) * 4,
                        )
                    });
                    anyhow::ensure!(
                        minimum.is_some_and(|size| size <= self.declared),
                        "history arrays exceed declared size"
                    );
                    self.history.times.try_reserve_exact(count)?;
                    self.stage = if count == 0 {
                        Stage::OffsetsLen
                    } else {
                        Stage::Times
                    };
                } else {
                    let expected = if self.history.shape == SignalShape::Text {
                        self.history
                            .times
                            .len()
                            .checked_add(2)
                            .ok_or_else(|| anyhow::anyhow!("offset count overflow"))?
                    } else {
                        0
                    };
                    anyhow::ensure!(count == expected, "incorrect history offset count");
                    self.history.offsets.try_reserve_exact(count)?;
                    self.stage = if count == 0 {
                        Stage::DataLen
                    } else {
                        Stage::Offsets
                    };
                }
                self.remaining = count;
            }
            Stage::Times => {
                anyhow::ensure!(
                    self.history.times.last().is_none_or(|&last| last <= value),
                    "unordered signal times"
                );
                self.history.times.push(value);
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.stage = Stage::OffsetsLen;
                }
            }
            Stage::Offsets => {
                anyhow::ensure!(value <= self.declared, "offset exceeds history size");
                if let Some(&last) = self.history.offsets.last() {
                    anyhow::ensure!(last < value, "invalid value offsets");
                } else {
                    anyhow::ensure!(value == 0, "invalid initial offset");
                }
                self.history.offsets.push(value);
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.stage = Stage::DataLen;
                }
            }
            Stage::DataLen => {
                let count = usize::try_from(value)?;
                let overhead = 28u64
                    + u64::from(matches!(self.history.shape, SignalShape::Vector { .. })) * 4
                    + (self.history.times.len() as u64 + self.history.offsets.len() as u64) * 8;
                anyhow::ensure!(
                    overhead.checked_add(value) == Some(self.declared),
                    "invalid history storage length"
                );
                if let Some(stride) = stride(self.history.shape) {
                    anyhow::ensure!(
                        self.history
                            .times
                            .len()
                            .checked_add(1)
                            .and_then(|n| n.checked_mul(stride))
                            == Some(count),
                        "invalid fixed history storage"
                    );
                } else {
                    anyhow::ensure!(
                        self.history.offsets.last() == Some(&value),
                        "invalid variable history storage"
                    );
                }
                anyhow::ensure!(count > 0, "missing initial history value");
                self.history.data.try_reserve_exact(count)?;
                self.remaining = count;
                self.stage = Stage::Data;
            }
            _ => anyhow::bail!("unexpected scalar in history"),
        }
        Ok(())
    }

    /// Validate at most DATA_BYTES of stored values. Call again after yielding
    /// when false is returned. This also handles a single very large text/bus.
    pub fn step(&mut self) -> anyhow::Result<bool> {
        let result = self.step_inner();
        if result.is_err() {
            self.stage = Stage::Failed;
        }
        result
    }

    fn step_inner(&mut self) -> anyhow::Result<bool> {
        if self.stage == Stage::Ready {
            return Ok(true);
        }
        anyhow::ensure!(
            self.stage == Stage::Validate,
            "history is not ready for validation"
        );
        let mut budget = DATA_BYTES;
        while self.value <= self.history.times.len() && budget > 0 {
            let data = self.history.raw(self.value.checked_sub(1));
            let tag = data[0];
            if self.position == 0 {
                match tag {
                    0 => {}
                    1 => anyhow::ensure!(
                        matches!(
                            self.history.shape,
                            SignalShape::Bit | SignalShape::Event | SignalShape::Vector { .. }
                        ),
                        "incompatible logic value"
                    ),
                    2 => anyhow::ensure!(
                        self.history.shape == SignalShape::Real,
                        "incompatible real value"
                    ),
                    3 | 4 => anyhow::ensure!(
                        self.history.shape == SignalShape::Text,
                        "incompatible variable value"
                    ),
                    _ => anyhow::bail!("unknown signal value tag"),
                }
                self.position = 1;
                budget -= 1;
            }
            let stop = data.len().min(self.position + budget);
            for (i, &byte) in data.iter().enumerate().take(stop).skip(self.position) {
                match tag {
                    0 => anyhow::ensure!(byte == 0, "invalid unavailable padding"),
                    1 => {
                        anyhow::ensure!(byte & 15 < 9, "invalid logic state");
                        if i * 2 > self.history.shape.width().max(1) as usize {
                            anyhow::ensure!(byte >> 4 == 0, "invalid logic padding");
                        } else {
                            anyhow::ensure!(byte >> 4 < 9, "invalid logic state");
                        }
                    }
                    3 => self.utf8.byte(byte)?,
                    _ => {}
                }
            }
            budget -= stop - self.position;
            self.position = stop;
            if stop == data.len() {
                anyhow::ensure!(self.utf8.remaining == 0, "truncated UTF-8 value");
                self.value += 1;
                self.position = 0;
            }
        }
        if self.value > self.history.times.len() {
            self.stage = Stage::Ready;
        }
        Ok(self.stage == Stage::Ready)
    }

    pub fn finish(self) -> anyhow::Result<PackedHistory> {
        anyhow::ensure!(
            self.stage == Stage::Ready,
            "incomplete history installation"
        );
        Ok(self.history)
    }
}

#[derive(Default)]
struct Utf8 {
    remaining: u8,
    low: u8,
    high: u8,
}
impl Utf8 {
    fn byte(&mut self, b: u8) -> anyhow::Result<()> {
        if self.remaining > 0 {
            anyhow::ensure!(
                (self.low..=self.high).contains(&b),
                "invalid UTF-8 continuation"
            );
            self.remaining -= 1;
            self.low = 0x80;
            self.high = 0xbf;
            return Ok(());
        }
        let (n, low, high) = match b {
            0..=0x7f => (0, 0, 0),
            0xc2..=0xdf => (1, 0x80, 0xbf),
            0xe0 => (2, 0xa0, 0xbf),
            0xe1..=0xec | 0xee..=0xef => (2, 0x80, 0xbf),
            0xed => (2, 0x80, 0x9f),
            0xf0 => (3, 0x90, 0xbf),
            0xf1..=0xf3 => (3, 0x80, 0xbf),
            0xf4 => (3, 0x80, 0x8f),
            _ => anyhow::bail!("invalid UTF-8 lead byte"),
        };
        self.remaining = n;
        self.low = low;
        self.high = high;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::history::VecHistory;
    use crate::data::{SignalHistory, WaveValue};
    use bincode::Options;
    #[test]
    fn chunked_installation_matches_serde_for_all_shapes_and_large_values() {
        for (shape, value) in [
            (SignalShape::Bit, WaveValue::Bits("z".into())),
            (
                SignalShape::Vector { width: 9 },
                WaveValue::Bits("01xzuwlh-".into()),
            ),
            (SignalShape::Real, WaveValue::Real(f64::INFINITY)),
            (SignalShape::Event, WaveValue::Bits("1".into())),
            (
                SignalShape::Text,
                WaveValue::Text("abcλ𐀀".repeat(DATA_BYTES / 8)),
            ),
        ] {
            let source = VecHistory {
                shape,
                times: vec![7, 7],
                values: vec![value.clone(), value],
                initial: WaveValue::Unavailable,
            };
            let packed = PackedHistory::from_history(&source).unwrap();
            let bytes = bincode::DefaultOptions::new()
                .with_fixint_encoding()
                .with_little_endian()
                .serialize(&packed)
                .unwrap();
            for chunk in [1, 7, DATA_BYTES] {
                let mut decoder =
                    HistoryDecoder::new(bytes.len() as u64, bytes.len() as u64).unwrap();
                for data in bytes.chunks(chunk) {
                    decoder.feed(data).unwrap();
                }
                let mut steps = 1;
                while !decoder.step().unwrap() {
                    steps += 1;
                }
                if packed.data.len() > DATA_BYTES {
                    assert!(steps > 1);
                }
                let result = decoder.finish().unwrap();
                result.validate().unwrap();
                assert_eq!(result.value(None), source.value(None));
                for i in 0..source.len() {
                    assert_eq!(result.value(Some(i)), source.value(Some(i)));
                }
            }
        }
    }
    #[test]
    fn utf8_rejects_overlong_surrogates_and_out_of_range() {
        for bytes in [
            &[0xc0, 0x80][..],
            &[0xed, 0xa0, 0x80],
            &[0xf4, 0x90, 0x80, 0x80],
            &[0x80],
        ] {
            let mut validator = Utf8::default();
            assert!(bytes.iter().try_for_each(|&b| validator.byte(b)).is_err());
        }
    }

    #[test]
    fn combined_storage_is_checked_before_allocation() {
        for shape in [1u32, 4] {
            let mut bytes = shape.to_le_bytes().to_vec();
            bytes.extend_from_slice(&100u64.to_le_bytes());
            let mut decoder = HistoryDecoder::new(850, 850).unwrap();
            assert!(decoder.feed(&bytes).is_err());
            assert_eq!(decoder.history.times.capacity(), 0);
            assert_eq!(decoder.history.offsets.capacity(), 0);
            assert!(decoder.feed(&[]).is_err());
            assert!(decoder.step().is_err());
            assert!(decoder.finish().is_err());
        }
    }

    #[test]
    fn incomplete_or_corrupt_objects_cannot_be_installed() {
        let source = VecHistory {
            shape: SignalShape::Bit,
            times: vec![7, 8],
            values: vec![WaveValue::Bits("0".into()), WaveValue::Bits("1".into())],
            initial: WaveValue::Unavailable,
        };
        let bytes = bincode::serialize(&PackedHistory::from_history(&source).unwrap()).unwrap();
        for cut in 0..bytes.len() {
            let mut decoder = HistoryDecoder::new(bytes.len() as u64, bytes.len() as u64).unwrap();
            decoder.feed(&bytes[..cut]).unwrap();
            assert!(decoder.step().is_err(), "accepted prefix of {cut} bytes");
            assert!(decoder.finish().is_err());
        }
        let mut corrupt = bytes.clone();
        *corrupt.last_mut().unwrap() = 9;
        let mut decoder = HistoryDecoder::new(bytes.len() as u64, bytes.len() as u64).unwrap();
        decoder.feed(&corrupt).unwrap();
        assert!(decoder.step().is_err());
        assert!(decoder.finish().is_err());

        let mut decoder = HistoryDecoder::new(bytes.len() as u64, bytes.len() as u64).unwrap();
        decoder.feed(&bytes).unwrap();
        // Even a complete payload stays private until validation finishes.
        assert!(decoder.finish().is_err());
    }
}
