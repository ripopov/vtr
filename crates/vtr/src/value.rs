//! Typed attribute values (`Value`) and their binary encoding.
//!
//! `Value` is the universal attribute type used on hierarchy nodes,
//! transactions, transaction events/phases and relations. It is a superset of
//! the FTR/SCV attribute types, the OpenTelemetry `AnyValue` model and the
//! FST attribute payloads.

use crate::error::{Error, Result};
use crate::strings::StrId;
use crate::varint::{self, Reader};

/// Type tags as written to the file (one byte).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ValueTag {
    Null = 0,
    Bool = 1,
    I64 = 2,
    U64 = 3,
    F64 = 4,
    Str = 5,
    Bytes = 6,
    /// 2-state bit vector.
    Bits = 7,
    /// 4-state logic vector (0,1,X,Z).
    Logic = 8,
    /// 9-state logic vector (IEEE 1164).
    Logic9 = 9,
    /// Time in file timescale units.
    Time = 10,
    /// Named enumeration literal: integer value plus its name.
    Enum = 11,
    /// Opaque pointer / handle.
    Pointer = 12,
    /// Signed fixed point: raw integer plus binary scale (value = raw * 2^-scale).
    Fixed = 13,
    /// Unsigned fixed point.
    UFixed = 14,
    /// Ordered list of values.
    List = 15,
    /// Ordered key/value map (keys are interned strings).
    Map = 16,
    /// Inline UTF-8 text (not interned; for one-off strings such as log arguments).
    Text = 17,
}

impl ValueTag {
    pub fn from_u8(v: u8) -> Result<ValueTag> {
        use ValueTag::*;
        Ok(match v {
            0 => Null,
            1 => Bool,
            2 => I64,
            3 => U64,
            4 => F64,
            5 => Str,
            6 => Bytes,
            7 => Bits,
            8 => Logic,
            9 => Logic9,
            10 => Time,
            11 => Enum,
            12 => Pointer,
            13 => Fixed,
            14 => UFixed,
            15 => List,
            16 => Map,
            17 => Text,
            _ => return Err(Error::Corrupt("unknown value tag")),
        })
    }
}

/// A typed attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(StrId),
    Bytes(Vec<u8>),
    /// 2-state vector, `width` bits, packed LSB-first (bit i at byte i/8, bit i%8).
    Bits { width: u32, data: Vec<u8> },
    /// 4-state vector, 2 bits per bit (codes: 0=0, 1=1, 2=X, 3=Z), LSB-first.
    Logic { width: u32, data: Vec<u8> },
    /// 9-state vector, 4 bits per bit (codes: 0,1,X,Z,U,W,L,H,-), LSB-first.
    Logic9 { width: u32, data: Vec<u8> },
    Time(u64),
    Enum { value: i64, name: StrId },
    Pointer(u64),
    Fixed { raw: i64, scale: i32 },
    UFixed { raw: u64, scale: i32 },
    List(Vec<Value>),
    Map(Vec<(StrId, Value)>),
    /// Inline UTF-8 text, stored verbatim (unlike `Str`, which is an interned id).
    Text(String),
}

impl Value {
    pub fn tag(&self) -> ValueTag {
        match self {
            Value::Null => ValueTag::Null,
            Value::Bool(_) => ValueTag::Bool,
            Value::I64(_) => ValueTag::I64,
            Value::U64(_) => ValueTag::U64,
            Value::F64(_) => ValueTag::F64,
            Value::Str(_) => ValueTag::Str,
            Value::Bytes(_) => ValueTag::Bytes,
            Value::Bits { .. } => ValueTag::Bits,
            Value::Logic { .. } => ValueTag::Logic,
            Value::Logic9 { .. } => ValueTag::Logic9,
            Value::Time(_) => ValueTag::Time,
            Value::Enum { .. } => ValueTag::Enum,
            Value::Pointer(_) => ValueTag::Pointer,
            Value::Fixed { .. } => ValueTag::Fixed,
            Value::UFixed { .. } => ValueTag::UFixed,
            Value::List(_) => ValueTag::List,
            Value::Map(_) => ValueTag::Map,
            Value::Text(_) => ValueTag::Text,
        }
    }

    /// Encodes `[tag][payload]`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.tag() as u8);
        self.encode_payload(out);
    }

    /// Encodes only the payload (the tag is stored elsewhere, e.g. in a column).
    pub fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Value::Null => {}
            Value::Bool(b) => out.push(*b as u8),
            Value::I64(v) => varint::put_i64(out, *v),
            Value::U64(v) | Value::Time(v) | Value::Pointer(v) => varint::put_u64(out, *v),
            Value::F64(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::Str(s) => varint::put_u64(out, s.0 as u64),
            Value::Bytes(b) => varint::put_blob(out, b),
            Value::Bits { width, data } | Value::Logic { width, data } | Value::Logic9 { width, data } => {
                varint::put_u64(out, *width as u64);
                out.extend_from_slice(data);
            }
            Value::Enum { value, name } => {
                varint::put_i64(out, *value);
                varint::put_u64(out, name.0 as u64);
            }
            Value::Fixed { raw, scale } => {
                varint::put_i64(out, *raw);
                varint::put_i64(out, *scale as i64);
            }
            Value::UFixed { raw, scale } => {
                varint::put_u64(out, *raw);
                varint::put_i64(out, *scale as i64);
            }
            Value::List(items) => {
                varint::put_u64(out, items.len() as u64);
                for v in items {
                    v.encode(out);
                }
            }
            Value::Map(items) => {
                varint::put_u64(out, items.len() as u64);
                for (k, v) in items {
                    varint::put_u64(out, k.0 as u64);
                    v.encode(out);
                }
            }
            Value::Text(s) => varint::put_blob(out, s.as_bytes()),
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Value> {
        let tag = ValueTag::from_u8(r.u8()?)?;
        Self::decode_payload(tag, r)
    }

    pub fn decode_payload(tag: ValueTag, r: &mut Reader) -> Result<Value> {
        Ok(match tag {
            ValueTag::Null => Value::Null,
            ValueTag::Bool => Value::Bool(r.u8()? != 0),
            ValueTag::I64 => Value::I64(r.i64()?),
            ValueTag::U64 => Value::U64(r.u64()?),
            ValueTag::F64 => Value::F64(r.f64()?),
            ValueTag::Str => Value::Str(StrId(r.u32()?)),
            ValueTag::Bytes => Value::Bytes(r.blob()?.to_vec()),
            ValueTag::Bits | ValueTag::Logic | ValueTag::Logic9 => {
                let width = r.u32()?;
                let bytes = packed_len(width, states_of(tag));
                let data = r.bytes(bytes)?.to_vec();
                match tag {
                    ValueTag::Bits => Value::Bits { width, data },
                    ValueTag::Logic => Value::Logic { width, data },
                    _ => Value::Logic9 { width, data },
                }
            }
            ValueTag::Time => Value::Time(r.u64()?),
            ValueTag::Enum => {
                let value = r.i64()?;
                let name = StrId(r.u32()?);
                Value::Enum { value, name }
            }
            ValueTag::Pointer => Value::Pointer(r.u64()?),
            ValueTag::Fixed => {
                let raw = r.i64()?;
                let scale = r.i64()? as i32;
                Value::Fixed { raw, scale }
            }
            ValueTag::UFixed => {
                let raw = r.u64()?;
                let scale = r.i64()? as i32;
                Value::UFixed { raw, scale }
            }
            ValueTag::List => {
                let n = r.usize()?;
                if n > r.remaining() {
                    return Err(Error::Corrupt("list length exceeds data"));
                }
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(Value::decode(r)?);
                }
                Value::List(items)
            }
            ValueTag::Map => {
                let n = r.usize()?;
                if n > r.remaining() {
                    return Err(Error::Corrupt("map length exceeds data"));
                }
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = StrId(r.u32()?);
                    items.push((k, Value::decode(r)?));
                }
                Value::Map(items)
            }
            ValueTag::Text => {
                let b = r.blob()?;
                Value::Text(std::str::from_utf8(b).map_err(|_| Error::Corrupt("text value is not UTF-8"))?.to_string())
            }
        })
    }
}

fn states_of(tag: ValueTag) -> u8 {
    match tag {
        ValueTag::Bits => 2,
        ValueTag::Logic => 4,
        _ => 9,
    }
}

/// Number of bytes needed to pack `width` bits of a `states`-state vector.
#[inline]
pub fn packed_len(width: u32, states: u8) -> usize {
    let w = width as usize;
    match states {
        2 => w.div_ceil(8),
        4 => w.div_ceil(4),
        _ => w.div_ceil(2),
    }
}

/// Attribute list encoding: `count` then `(key, value)` pairs.
pub fn encode_attrs(attrs: &[(StrId, Value)], out: &mut Vec<u8>) {
    varint::put_u64(out, attrs.len() as u64);
    for (k, v) in attrs {
        varint::put_u64(out, k.0 as u64);
        v.encode(out);
    }
}

pub fn decode_attrs(r: &mut Reader) -> Result<Vec<(StrId, Value)>> {
    let n = r.usize()?;
    if n > r.remaining() {
        return Err(Error::Corrupt("attribute count exceeds data"));
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let k = StrId(r.u32()?);
        v.push((k, Value::decode(r)?));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let v = Value::Map(vec![
            (StrId(1), Value::Null),
            (StrId(2), Value::Bool(true)),
            (StrId(3), Value::I64(-5)),
            (StrId(4), Value::U64(u64::MAX)),
            (StrId(5), Value::F64(3.5)),
            (StrId(6), Value::Str(StrId(9))),
            (StrId(7), Value::Bytes(vec![1, 2, 3])),
            (StrId(8), Value::Bits { width: 12, data: vec![0xff, 0x0f] }),
            (StrId(9), Value::Logic { width: 3, data: vec![0b00_10_01] }),
            (StrId(10), Value::Logic9 { width: 2, data: vec![0x84] }),
            (StrId(11), Value::Time(77)),
            (StrId(12), Value::Enum { value: 2, name: StrId(3) }),
            (StrId(13), Value::Pointer(0xdead)),
            (StrId(14), Value::Fixed { raw: -3, scale: 4 }),
            (StrId(15), Value::UFixed { raw: 3, scale: -4 }),
            (StrId(16), Value::List(vec![Value::I64(1), Value::List(vec![])])),
        ]);
        let mut out = Vec::new();
        v.encode(&mut out);
        let mut r = Reader::new(&out);
        assert_eq!(Value::decode(&mut r).unwrap(), v);
        assert!(r.is_empty());
    }
}
