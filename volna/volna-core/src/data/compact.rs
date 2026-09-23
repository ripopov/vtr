//! Compact immutable histories for loaders that decode values themselves
//! (FST): `u64` change times and fixed-stride packed values.
//!
//! Logic values use VTR's codes: 1, 2 or 4 bits per bit for 2, 4 or 9 states,
//! least significant bit first ([`vtr::signal::get_code`]). An entry of at most
//! 8 bits takes a power-of-two slot inside a byte, so a two-state clock costs
//! one bit per change; wider entries are byte-aligned so a [`ValueView`]
//! borrows them in place. A signal starts two-state and is repacked when a
//! value needs more states.

use anyhow::ensure;

use super::value_view::{LogicView, ValueView};
use super::{Bit, SignalHistory, SignalShape, WaveValue};
use vtr::signal::{CODE_ASCII, code_from_ascii, get_code, pack_ascii, states_for_code};

/// Every byte value, so a sub-byte entry can be viewed as a one-byte slice.
static BYTES: [u8; 256] = {
    let mut all = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        all[i] = i as u8;
        i += 1;
    }
    all
};

fn code_bits(states: u8) -> u32 {
    match states {
        2 => 1,
        4 => 2,
        _ => 4,
    }
}

/// Bits one entry occupies: a power of two up to 8, else whole bytes.
fn slot_bits(width: u32, states: u8) -> u32 {
    let bits = width * code_bits(states);
    if bits <= 8 {
        bits.next_power_of_two()
    } else {
        bits.div_ceil(8) * 8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Logic { width: u32, states: u8, slot: u32 },
    Real,
}

impl Layout {
    fn logic(width: u32, states: u8) -> Self {
        Self::Logic {
            width,
            states,
            slot: slot_bits(width, states),
        }
    }
}

/// Packed entry `i` of `data`: a view into it, or into [`BYTES`] for sub-byte slots.
fn entry(data: &[u8], slot: u32, i: usize) -> &[u8] {
    if slot <= 8 {
        let bit = i * slot as usize;
        let v = (data[bit / 8] >> (bit % 8)) & (((1u16 << slot) - 1) as u8);
        std::slice::from_ref(&BYTES[v as usize])
    } else {
        let n = slot as usize / 8;
        &data[i * n..(i + 1) * n]
    }
}

/// Append a packed entry (`bytes` holds at least the entry's bits).
fn push_entry(data: &mut Vec<u8>, slot: u32, len: usize, bytes: &[u8]) {
    if slot <= 8 {
        let bit = len * slot as usize;
        if bit.is_multiple_of(8) {
            data.push(0);
        }
        *data.last_mut().unwrap() |= bytes[0] << (bit % 8);
    } else {
        data.extend_from_slice(&bytes[..slot as usize / 8]);
    }
}

#[derive(Debug)]
pub struct CompactHistory {
    shape: SignalShape,
    layout: Layout,
    times: Vec<u64>,
    data: Vec<u8>,
}

impl CompactHistory {
    fn view(&self, i: usize) -> ValueView<'_> {
        match self.layout {
            Layout::Logic {
                width,
                states,
                slot,
            } => ValueView::Logic(LogicView::packed_lsb(
                width,
                states,
                entry(&self.data, slot, i),
            )),
            Layout::Real => ValueView::Real(f64::from_le_bytes(
                self.data[i * 8..i * 8 + 8].try_into().unwrap(),
            )),
        }
    }
}

impl SignalHistory for CompactHistory {
    fn resident_bytes(&self) -> u64 {
        (self.times.capacity() * std::mem::size_of::<u64>() + self.data.capacity()) as u64
    }
    fn shape(&self) -> SignalShape {
        self.shape
    }
    fn len(&self) -> usize {
        self.times.len()
    }
    fn time(&self, i: usize) -> u64 {
        self.times[i]
    }
    fn index_at(&self, t: u64) -> Option<usize> {
        self.times.partition_point(|&x| x <= t).checked_sub(1)
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        let Some(i) = i else {
            return WaveValue::Unavailable;
        };
        match self.layout {
            Layout::Logic {
                width,
                states,
                slot,
            } => {
                let data = entry(&self.data, slot, i);
                let text = (0..width as usize)
                    .rev()
                    .map(|k| CODE_ASCII[get_code(data, states, k) as usize] as char)
                    .collect();
                WaveValue::Bits(text)
            }
            Layout::Real => match self.view(i) {
                ValueView::Real(v) => WaveValue::Real(v),
                _ => unreachable!(),
            },
        }
    }
    fn value_view(&self, i: Option<usize>) -> ValueView<'_> {
        match i {
            None => ValueView::Unavailable,
            Some(i) => self.view(i),
        }
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        let Some(i) = i else {
            return Bit::Unavailable;
        };
        match self.layout {
            Layout::Logic {
                width: 1,
                states,
                slot,
            } => match get_code(entry(&self.data, slot, i), states, 0) {
                0 => Bit::Zero,
                1 => Bit::One,
                2 | 4 | 5 => Bit::X,
                3 => Bit::Z,
                _ => Bit::Other,
            },
            _ => Bit::Other,
        }
    }
}

/// Collects one signal's changes during a load.
pub(crate) struct CompactBuilder {
    shape: SignalShape,
    layout: Layout,
    times: Vec<u64>,
    data: Vec<u8>,
    scratch: Vec<u8>,
}

impl CompactBuilder {
    /// Logic and real shapes; text and events keep their own histories.
    pub(crate) fn new(shape: SignalShape) -> Option<Self> {
        let layout = match shape {
            SignalShape::Bit | SignalShape::Vector { .. } => Layout::logic(shape.width(), 2),
            SignalShape::Real => Layout::Real,
            SignalShape::Text | SignalShape::Event => return None,
        };
        Some(Self {
            shape,
            layout,
            times: Vec::new(),
            data: Vec::new(),
            scratch: Vec::new(),
        })
    }

    fn push_time(&mut self, t: u64) -> anyhow::Result<()> {
        ensure!(
            self.times.last().is_none_or(|&last| last <= t),
            "signal times are not ordered"
        );
        self.times.push(t);
        Ok(())
    }

    pub(crate) fn push_real(&mut self, t: u64, v: f64) -> anyhow::Result<()> {
        ensure!(self.layout == Layout::Real, "real value for a logic signal");
        self.push_time(t)?;
        self.data.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    /// Append an MSB-first ASCII logic value of exactly the declared width.
    pub(crate) fn push_logic(&mut self, t: u64, text: &[u8]) -> anyhow::Result<()> {
        let Layout::Logic { width, states, .. } = self.layout else {
            anyhow::bail!("logic value for a real signal");
        };
        ensure!(
            text.len() == width as usize,
            "logic value width differs from its declaration"
        );
        // Two-state fast path: the value as an integer, written little-endian.
        if states == 2 && width <= 64 {
            // Branch-free: '0' and '1' differ only in their low bit, and data
            // bits are unpredictable, so a per-character branch mispredicts.
            let mut acc = 0u64;
            let mut binary = true;
            for &c in text {
                acc = (acc << 1) | u64::from(c & 1);
                binary &= c | 1 == b'1';
            }
            if binary {
                self.push_time(t)?;
                let len = self.times.len() - 1;
                let Layout::Logic { slot, .. } = self.layout else {
                    unreachable!()
                };
                push_entry(&mut self.data, slot, len, &acc.to_le_bytes());
                return Ok(());
            }
        }
        let mut need = 2;
        for &c in text {
            ensure!(
                b"01xXzZuUwWlLhH-".contains(&c),
                "logic value contains an unsupported state"
            );
            need = need.max(states_for_code(code_from_ascii(c)));
        }
        if need > states {
            self.widen(need);
        }
        let Layout::Logic { states, slot, .. } = self.layout else {
            unreachable!()
        };
        self.scratch
            .resize(vtr::value::packed_len(width, states).max(1), 0);
        pack_ascii(text, width, states, &mut self.scratch);
        self.push_time(t)?;
        let len = self.times.len() - 1;
        push_entry(&mut self.data, slot, len, &self.scratch);
        Ok(())
    }

    /// Repack every entry with `to` states per bit.
    fn widen(&mut self, to: u8) {
        let Layout::Logic {
            width,
            states,
            slot,
        } = self.layout
        else {
            return;
        };
        let next = Layout::logic(width, to);
        let Layout::Logic {
            slot: next_slot, ..
        } = next
        else {
            unreachable!()
        };
        let mut data = Vec::with_capacity(self.times.len() * next_slot as usize / 8 + 1);
        let mut buf = vec![0u8; vtr::value::packed_len(width, to).max(1)];
        for i in 0..self.times.len() {
            let old = entry(&self.data, slot, i);
            buf.iter_mut().for_each(|b| *b = 0);
            for k in 0..width as usize {
                vtr::signal::set_code(&mut buf, to, k, get_code(old, states, k));
            }
            push_entry(&mut data, next_slot, i, &buf);
        }
        self.data = data;
        self.layout = next;
    }

    /// The finished history, its buffers trimmed to their contents.
    pub(crate) fn finish(mut self) -> CompactHistory {
        self.times.shrink_to_fit();
        self.data.shrink_to_fit();
        CompactHistory {
            shape: self.shape,
            layout: self.layout,
            times: self.times,
            data: self.data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(shape: SignalShape, values: &[&str]) -> CompactHistory {
        let mut b = CompactBuilder::new(shape).unwrap();
        for (t, v) in values.iter().enumerate() {
            b.push_logic(t as u64 * 10, v.as_bytes()).unwrap();
        }
        b.finish()
    }

    fn texts(h: &CompactHistory) -> Vec<String> {
        (0..h.len())
            .map(|i| match h.value(Some(i)) {
                WaveValue::Bits(s) => s,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn two_state_bits_pack_eight_per_byte() {
        let values: Vec<&str> = (0..20)
            .map(|i| if i % 3 == 0 { "1" } else { "0" })
            .collect();
        let h = build(SignalShape::Bit, &values);
        assert_eq!(texts(&h), values);
        assert_eq!(h.data.len(), 3, "20 changes in 3 bytes");
        assert_eq!(h.bit(Some(3)), Bit::One);
        assert_eq!(h.bit(Some(4)), Bit::Zero);
        assert_eq!(h.bit(None), Bit::Unavailable);
        assert_eq!(h.index_at(35), Some(3));
        assert_eq!(h.index_at(0), Some(0));
        assert_eq!(h.value(None), WaveValue::Unavailable);
    }

    #[test]
    fn values_repack_to_four_and_nine_states_in_order() {
        let values = ["1", "0", "x", "1", "z", "h", "0", "-"];
        let h = build(SignalShape::Bit, &values);
        assert_eq!(texts(&h), values);
        assert_eq!(h.bit(Some(2)), Bit::X);
        assert_eq!(h.bit(Some(4)), Bit::Z);
        assert_eq!(h.bit(Some(5)), Bit::Other);
        let wide = ["101", "011", "1x0", "zz1", "111", "01u"];
        assert_eq!(texts(&build(SignalShape::Vector { width: 3 }, &wide)), wide);
        let vec12 = [
            "101010101010",
            "0000000000x1",
            "111111111111",
            "01xzuwlh-010",
        ];
        assert_eq!(
            texts(&build(SignalShape::Vector { width: 12 }, &vec12)),
            vec12
        );
    }

    #[test]
    fn views_read_numbers_in_place_and_reals_round_trip() {
        use crate::data::NumericKind;
        let h = build(
            SignalShape::Vector { width: 16 },
            &["1111111111111110", "0000000000000101"],
        );
        assert_eq!(NumericKind::Signed.read(&h.value_view(Some(0))), Some(-2.0));
        assert_eq!(
            NumericKind::Unsigned.read(&h.value_view(Some(1))),
            Some(5.0)
        );
        let small = build(SignalShape::Vector { width: 3 }, &["101", "0x1"]);
        assert_eq!(
            NumericKind::Unsigned.read(&small.value_view(Some(0))),
            Some(5.0)
        );
        assert_eq!(NumericKind::Unsigned.read(&small.value_view(Some(1))), None);
        let mut r = CompactBuilder::new(SignalShape::Real).unwrap();
        r.push_real(1, -2.5).unwrap();
        r.push_real(3, 1e300).unwrap();
        let r = r.finish();
        assert_eq!(r.value(Some(0)), WaveValue::Real(-2.5));
        assert!(matches!(r.value_view(Some(1)), ValueView::Real(v) if v == 1e300));
        assert_eq!(r.resident_bytes(), 32);
    }

    #[test]
    fn malformed_values_are_rejected() {
        let mut b = CompactBuilder::new(SignalShape::Vector { width: 2 }).unwrap();
        assert!(b.push_logic(0, b"1").is_err());
        assert!(b.push_logic(0, b"1q").is_err());
        b.push_logic(5, b"10").unwrap();
        assert!(b.push_logic(4, b"10").is_err(), "time goes backwards");
        assert!(b.push_real(6, 1.0).is_err());
    }
}
