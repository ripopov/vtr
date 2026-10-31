//! Typed attribute values shared by metadata, records, and relations.

use std::collections::HashSet;

use crate::{
    Error, Result,
    schema::{Primitive, integer_range},
    varint::{put_blob, put_i64, put_u64, unzigzag},
    wire::Cursor,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Str(u32),
    Text(String),
    Bytes(Vec<u8>),
    Bits { width: u64, data: Vec<u8> },
    Logic { width: u64, data: Vec<u8> },
    Logic9 { width: u64, data: Vec<u8> },
    Time(u64),
    Enum { value: i64, name: u32 },
    Pointer(u64),
    Fixed { raw: i64, scale: i64 },
    UFixed { raw: u64, scale: i64 },
    List(Vec<Value>),
    Map(Vec<Attribute>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub key: u32,
    pub value: Value,
}

pub type Attributes = Vec<Attribute>;

impl Value {
    #[must_use]
    pub fn tag(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Bool(_) => 1,
            Self::I8(_) => 2,
            Self::I16(_) => 3,
            Self::I32(_) => 4,
            Self::I64(_) => 5,
            Self::U8(_) => 6,
            Self::U16(_) => 7,
            Self::U32(_) => 8,
            Self::U64(_) => 9,
            Self::F32(_) => 10,
            Self::F64(_) => 11,
            Self::Str(_) => 12,
            Self::Text(_) => 13,
            Self::Bytes(_) => 14,
            Self::Bits { .. } => 15,
            Self::Logic { .. } => 16,
            Self::Logic9 { .. } => 17,
            Self::Time(_) => 18,
            Self::Enum { .. } => 19,
            Self::Pointer(_) => 20,
            Self::Fixed { .. } => 21,
            Self::UFixed { .. } => 22,
            Self::List(_) => 23,
            Self::Map(_) => 24,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>, strings: u64) -> Result<()> {
        let length = out.len();
        if let Err(error) = self.encode_tagged(out, strings, 0) {
            out.truncate(length);
            return Err(error);
        }
        Ok(())
    }

    fn encode_tagged(&self, out: &mut Vec<u8>, strings: u64, depth: u8) -> Result<()> {
        out.push(self.tag());
        self.encode_payload(out, strings, depth)
    }

    fn encode_payload(&self, out: &mut Vec<u8>, strings: u64, depth: u8) -> Result<()> {
        if depth >= 64 {
            return Err(Error::invalid("value nesting exceeds 64"));
        }
        match self {
            Self::Null => (),
            Self::Bool(value) => out.push(u8::from(*value)),
            Self::I8(value) => put_i64(out, i64::from(*value)),
            Self::I16(value) => put_i64(out, i64::from(*value)),
            Self::I32(value) => put_i64(out, i64::from(*value)),
            Self::I64(value) => put_i64(out, *value),
            Self::U8(value) => put_u64(out, u64::from(*value)),
            Self::U16(value) => put_u64(out, u64::from(*value)),
            Self::U32(value) => put_u64(out, u64::from(*value)),
            Self::U64(value) | Self::Time(value) | Self::Pointer(value) => put_u64(out, *value),
            Self::F32(value) => out.extend_from_slice(&value.to_bits().to_le_bytes()),
            Self::F64(value) => out.extend_from_slice(&value.to_bits().to_le_bytes()),
            Self::Str(id) => {
                string_id(*id, strings)?;
                put_u64(out, u64::from(*id));
            }
            Self::Text(text) => put_blob(out, text.as_bytes()),
            Self::Bytes(bytes) => put_blob(out, bytes),
            Self::Bits { width, data }
            | Self::Logic { width, data }
            | Self::Logic9 { width, data } => {
                let primitive = packed(self.tag())?;
                if data.len() != primitive.packed_bytes(*width)? {
                    return Err(Error::invalid("packed value size mismatch"));
                }
                primitive.validate_packed(data, *width)?;
                put_u64(out, *width);
                out.extend_from_slice(data);
            }
            Self::Enum { value, name } => {
                string_id(*name, strings)?;
                put_i64(out, *value);
                put_u64(out, u64::from(*name));
            }
            Self::Fixed { raw, scale } => {
                put_i64(out, *raw);
                put_i64(out, *scale);
            }
            Self::UFixed { raw, scale } => {
                put_u64(out, *raw);
                put_i64(out, *scale);
            }
            Self::List(values) => {
                put_u64(out, values.len() as u64);
                for value in values {
                    value.encode_tagged(out, strings, depth + 1)?;
                }
            }
            Self::Map(attributes) => encode_attributes(out, attributes, strings, depth + 1)?,
        }
        Ok(())
    }

    pub fn decode(data: &[u8], strings: u64, allocation_budget: usize) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        let mut budget = allocation_budget;
        let value = Self::read_tagged::<true>(&mut cursor, strings, 0, &mut budget)?;
        cursor.done()?;
        Ok(value)
    }

    fn read_tagged<const RETAIN: bool>(
        cursor: &mut Cursor<'_>,
        strings: u64,
        depth: u8,
        budget: &mut usize,
    ) -> Result<Self> {
        let tag = cursor.byte()?;
        Self::read_payload::<RETAIN>(tag, cursor, strings, depth, budget)
    }

    fn read_payload<const RETAIN: bool>(
        tag: u8,
        cursor: &mut Cursor<'_>,
        strings: u64,
        depth: u8,
        budget: &mut usize,
    ) -> Result<Self> {
        if depth >= 64 {
            return Err(Error::Corrupt("value nesting exceeds 64"));
        }
        Ok(match tag {
            0 => Self::Null,
            1 => match cursor.byte()? {
                0 => Self::Bool(false),
                1 => Self::Bool(true),
                _ => return Err(Error::Corrupt("invalid bool attribute")),
            },
            2..=9 => {
                let raw = cursor.var()?;
                let raw = if tag <= 5 { unzigzag(raw) as u64 } else { raw };
                let primitive = [
                    Primitive::I8,
                    Primitive::I16,
                    Primitive::I32,
                    Primitive::I64,
                    Primitive::U8,
                    Primitive::U16,
                    Primitive::U32,
                    Primitive::U64,
                ][tag as usize - 2];
                integer_range(primitive, raw)?;
                match tag {
                    2 => Self::I8(raw as i8),
                    3 => Self::I16(raw as i16),
                    4 => Self::I32(raw as i32),
                    5 => Self::I64(raw as i64),
                    6 => Self::U8(raw as u8),
                    7 => Self::U16(raw as u16),
                    8 => Self::U32(raw as u32),
                    _ => Self::U64(raw),
                }
            }
            10 => Self::F32(f32::from_bits(cursor.u32()?)),
            11 => Self::F64(f64::from_bits(cursor.u64()?)),
            12 => {
                let id = cursor.id()?;
                string_id(id, strings)?;
                Self::Str(id)
            }
            13 | 14 => {
                let bytes = cursor.blob()?;
                spend(budget, bytes.len())?;
                if tag == 13 {
                    let text =
                        std::str::from_utf8(bytes).map_err(|_| Error::Corrupt("invalid UTF-8"))?;
                    if RETAIN {
                        Self::Text(text.into())
                    } else {
                        Self::Null
                    }
                } else if RETAIN {
                    Self::Bytes(bytes.to_vec())
                } else {
                    Self::Null
                }
            }
            15..=17 => {
                let width = cursor.var()?;
                let primitive = packed(tag)?;
                let bytes = cursor.take(primitive.packed_bytes(width)?)?;
                primitive.validate_packed(bytes, width)?;
                spend(budget, bytes.len())?;
                let data = if RETAIN { bytes.to_vec() } else { Vec::new() };
                match tag {
                    15 => Self::Bits { width, data },
                    16 => Self::Logic { width, data },
                    _ => Self::Logic9 { width, data },
                }
            }
            18 => Self::Time(cursor.var()?),
            19 => {
                let value = unzigzag(cursor.var()?);
                let name = cursor.id()?;
                string_id(name, strings)?;
                Self::Enum { value, name }
            }
            20 => Self::Pointer(cursor.var()?),
            21 => Self::Fixed {
                raw: unzigzag(cursor.var()?),
                scale: unzigzag(cursor.var()?),
            },
            22 => Self::UFixed {
                raw: cursor.var()?,
                scale: unzigzag(cursor.var()?),
            },
            23 => {
                let count = bounded_count(cursor, size_of::<Value>(), budget)?;
                let mut values = Vec::with_capacity(if RETAIN { count } else { 0 });
                for _ in 0..count {
                    let value = Self::read_tagged::<RETAIN>(cursor, strings, depth + 1, budget)?;
                    if RETAIN {
                        values.push(value);
                    }
                }
                Self::List(values)
            }
            24 => Self::Map(decode_attributes::<RETAIN>(
                cursor,
                strings,
                depth + 1,
                budget,
            )?),
            _ => return Err(Error::Corrupt("unknown value tag")),
        })
    }
}

fn packed(tag: u8) -> Result<Primitive> {
    match tag {
        15 => Ok(Primitive::Bits),
        16 => Ok(Primitive::Logic),
        17 => Ok(Primitive::Logic9),
        _ => Err(Error::Corrupt("invalid packed attribute")),
    }
}

fn string_id(id: u32, strings: u64) -> Result<()> {
    if u64::from(id) >= strings {
        Err(Error::Corrupt("unknown value string identifier"))
    } else {
        Ok(())
    }
}

fn spend(budget: &mut usize, size: usize) -> Result<()> {
    *budget = budget
        .checked_sub(size)
        .ok_or_else(|| Error::corrupt("attribute allocation budget exceeded"))?;
    Ok(())
}

fn bounded_count(cursor: &mut Cursor<'_>, size: usize, budget: &mut usize) -> Result<usize> {
    let count = usize::try_from(cursor.var()?)
        .map_err(|_| Error::Corrupt("attribute count exceeds usize"))?;
    if count > cursor.remaining() {
        return Err(Error::Corrupt("attribute count exceeds data"));
    }
    spend(
        budget,
        count
            .checked_mul(size)
            .ok_or_else(|| Error::corrupt("attribute allocation exceeds usize"))?,
    )?;
    Ok(count)
}

pub(crate) fn validate_attributes(attributes: &[Attribute], strings: u64) -> Result<()> {
    let mut keys = HashSet::new();
    for attribute in attributes {
        string_id(attribute.key, strings)?;
        if !keys.insert(attribute.key) {
            return Err(Error::Corrupt("duplicate attribute key"));
        }
        attribute.value.encode(&mut Vec::new(), strings)?;
    }
    Ok(())
}

pub(crate) fn encode_attributes(
    out: &mut Vec<u8>,
    attributes: &[Attribute],
    strings: u64,
    depth: u8,
) -> Result<()> {
    let mut keys = HashSet::new();
    put_u64(out, attributes.len() as u64);
    for attribute in attributes {
        string_id(attribute.key, strings)?;
        if !keys.insert(attribute.key) {
            return Err(Error::Corrupt("duplicate attribute key"));
        }
        put_u64(out, u64::from(attribute.key));
        attribute.value.encode_tagged(out, strings, depth)?;
    }
    Ok(())
}

pub(crate) fn read_attributes(
    cursor: &mut Cursor<'_>,
    strings: u64,
    depth: u8,
    budget: &mut usize,
) -> Result<Attributes> {
    decode_attributes::<true>(cursor, strings, depth, budget)
}

fn decode_attributes<const RETAIN: bool>(
    cursor: &mut Cursor<'_>,
    strings: u64,
    depth: u8,
    budget: &mut usize,
) -> Result<Attributes> {
    let count = bounded_count(cursor, size_of::<Attribute>(), budget)?;
    let mut out = Vec::with_capacity(if RETAIN { count } else { 0 });
    let mut keys = HashSet::new();
    for _ in 0..count {
        let key = cursor.id()?;
        string_id(key, strings)?;
        if !keys.insert(key) {
            return Err(Error::Corrupt("duplicate attribute key"));
        }
        let value = Value::read_tagged::<RETAIN>(cursor, strings, depth, budget)?;
        if RETAIN {
            out.push(Attribute { key, value });
        }
    }
    Ok(out)
}

#[derive(Default)]
pub(crate) struct AttributeColumns {
    pub(crate) columns: [Vec<u8>; 7],
}

impl AttributeColumns {
    pub(crate) fn append(&mut self, attributes: &[Attribute], strings: u64) -> Result<()> {
        validate_attributes(attributes, strings)?;
        put_u64(&mut self.columns[0], attributes.len() as u64);
        for attribute in attributes {
            put_u64(&mut self.columns[1], u64::from(attribute.key));
            let value = &attribute.value;
            let tag = value.tag();
            self.columns[2].push(tag);
            if let Value::Enum { value, name } = value {
                put_i64(&mut self.columns[3], *value);
                put_u64(&mut self.columns[5], u64::from(*name));
            } else {
                value.encode_payload(&mut self.columns[value_column(tag)], strings, 0)?;
            }
        }
        Ok(())
    }
}

fn value_column(tag: u8) -> usize {
    match tag {
        10 | 11 => 4,
        12 => 5,
        13..=17 | 23 | 24 => 6,
        _ => 3,
    }
}

pub(crate) struct AttributeReader<'a> {
    columns: [Cursor<'a>; 7],
    strings: u64,
    budget: usize,
}

impl<'a> AttributeReader<'a> {
    pub(crate) fn new(columns: [&'a [u8]; 7], strings: u64, budget: usize) -> Self {
        Self {
            columns: columns.map(Cursor::new),
            strings,
            budget,
        }
    }

    pub(crate) fn read(&mut self) -> Result<Attributes> {
        self.decode::<true>()
    }

    pub(crate) fn skip(&mut self) -> Result<()> {
        self.decode::<false>().map(|_| ())
    }

    fn decode<const RETAIN: bool>(&mut self) -> Result<Attributes> {
        let count = self.columns[0].var()?;
        if count > self.columns[2].remaining() as u64 {
            return Err(Error::Corrupt("invalid attribute count"));
        }
        let mut budget = self.budget;
        spend(
            &mut budget,
            (count as usize)
                .checked_mul(size_of::<Attribute>())
                .ok_or_else(|| Error::corrupt("attribute allocation exceeds usize"))?,
        )?;
        let mut out = Vec::with_capacity(if RETAIN { count as usize } else { 0 });
        let mut keys = HashSet::new();
        for _ in 0..count {
            let key = self.columns[1].id()?;
            string_id(key, self.strings)?;
            if !keys.insert(key) {
                return Err(Error::Corrupt("duplicate attribute key"));
            }
            let tag = self.columns[2].byte()?;
            let value = if tag == 19 {
                let value = unzigzag(self.columns[3].var()?);
                let name = self.columns[5].id()?;
                string_id(name, self.strings)?;
                Value::Enum { value, name }
            } else {
                Value::read_payload::<RETAIN>(
                    tag,
                    &mut self.columns[value_column(tag)],
                    self.strings,
                    0,
                    &mut budget,
                )?
            };
            if RETAIN {
                out.push(Attribute { key, value });
            }
        }
        Ok(out)
    }

    pub(crate) fn done(&self) -> Result<()> {
        for column in self.columns {
            column.done()?;
        }
        Ok(())
    }
}

pub(crate) fn value_charge(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.capacity(),
        Value::Bytes(value)
        | Value::Bits { data: value, .. }
        | Value::Logic { data: value, .. }
        | Value::Logic9 { data: value, .. } => value.capacity(),
        Value::List(values) => {
            values.capacity() * std::mem::size_of::<Value>()
                + values.iter().map(value_charge).sum::<usize>()
        }
        Value::Map(values) => memory_charge(values),
        _ => 0,
    }
}
pub(crate) fn memory_charge(attributes: &crate::attributes::Attributes) -> usize {
    // Spare slots move with the capture and remain part of its admission charge.
    attributes.capacity() * std::mem::size_of::<crate::attributes::Attribute>()
        + attributes
            .iter()
            .map(|attr| value_charge(&attr.value))
            .sum::<usize>()
}
