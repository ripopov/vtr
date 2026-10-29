use std::{ops::Range, sync::Arc};

use crate::{
    Error, Result,
    attributes::Value,
    schema::{self, Payload, Primitive, TypeId, TypeKind, Types},
    wire::Cursor,
};

/// A file-local typed payload retained independently of its reader and caches.
#[derive(Clone, Debug)]
pub struct PayloadView {
    pub(super) types: Arc<super::RecordTypes>,
    pub(super) storage: Arc<super::RecordData>,
    pub(crate) datatype: TypeId,
    pub(crate) range: Range<usize>,
    pub(crate) allocation_limit: usize,
}

impl PayloadView {
    #[must_use]
    pub fn datatype(&self) -> TypeId {
        self.datatype
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.storage.raw.payload[self.range.clone()]
    }
    #[must_use]
    pub fn schema(&self) -> &Types {
        &self.types
    }

    pub fn decode<T: Payload>(&self) -> Result<T> {
        decode_payload(
            &self.types,
            self.datatype,
            self.bytes(),
            self.allocation_limit,
        )
    }

    fn child(&self, datatype: TypeId, offset: usize) -> Result<Self> {
        let tail = self
            .bytes()
            .get(offset..)
            .ok_or_else(|| Error::corrupt("payload field exceeds storage"))?;
        let length = self.types.canonical_len(datatype, tail)?;
        Ok(Self {
            datatype,
            range: self.range.start + offset..self.range.start + offset + length,
            ..self.clone()
        })
    }

    pub fn field(&self, name: &str) -> Result<Self> {
        let TypeKind::Struct(fields) = &self.types.get(self.datatype)?.kind else {
            return Err(Error::invalid("payload is not a structure"));
        };
        let mut offset = 0;
        for field in fields {
            if field.name == name {
                return self.child(field.datatype, offset);
            }
            offset += self
                .types
                .canonical_len(field.datatype, &self.bytes()[offset..])?;
        }
        Err(Error::invalid(format!("unknown payload field {name}")))
    }

    pub fn len(&self) -> Result<usize> {
        Ok(match &self.types.get(self.datatype)?.kind {
            TypeKind::Struct(fields) => fields.len(),
            TypeKind::Array { count, .. } => *count as usize,
            TypeKind::Sequence { .. } => Cursor::new(self.bytes()).u32()? as usize,
            _ => return Err(Error::invalid("payload is not a collection")),
        })
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.len().map(|length| length == 0)
    }

    /// Iterate structure fields or collection elements in declaration order.
    /// Earlier siblings are not rescanned, and each returned view retains its storage.
    pub fn children(&self) -> Result<impl ExactSizeIterator<Item = Result<Self>> + '_> {
        let kind = &self.types.get(self.datatype)?.kind;
        let (count, mut offset) = match kind {
            TypeKind::Struct(fields) => (fields.len(), 0),
            TypeKind::Array { count, .. } => (*count as usize, 0),
            TypeKind::Sequence { .. } => (Cursor::new(self.bytes()).u32()? as usize, 4),
            _ => return Err(Error::invalid("payload is not a collection")),
        };
        Ok((0..count).map(move |index| {
            let datatype = match kind {
                TypeKind::Struct(fields) => fields[index].datatype,
                TypeKind::Array { element, .. } | TypeKind::Sequence { element } => *element,
                _ => unreachable!(),
            };
            let child = self.child(datatype, offset)?;
            offset += child.bytes().len();
            Ok(child)
        }))
    }

    pub fn at(&self, index: usize) -> Result<Self> {
        if let TypeKind::Struct(fields) = &self.types.get(self.datatype)?.kind {
            let field = fields
                .get(index)
                .ok_or_else(|| Error::invalid("payload field index is out of range"))?;
            let mut offset = 0;
            for previous in &fields[..index] {
                offset += self
                    .types
                    .canonical_len(previous.datatype, &self.bytes()[offset..])?;
            }
            return self.child(field.datatype, offset);
        }
        let (element, count, mut offset) = match &self.types.get(self.datatype)?.kind {
            TypeKind::Array { element, count } => (*element, *count as usize, 0),
            TypeKind::Sequence { element } => {
                (*element, Cursor::new(self.bytes()).u32()? as usize, 4)
            }
            _ => return Err(Error::invalid("payload is not an array or sequence")),
        };
        if index >= count {
            return Err(Error::invalid("payload index is out of range"));
        }
        if let Some(size) = self.types.get(element)?.fixed_size {
            offset += index
                .checked_mul(size as usize)
                .ok_or_else(|| Error::corrupt("payload offset overflow"))?;
        } else {
            for _ in 0..index {
                offset += self.types.canonical_len(element, &self.bytes()[offset..])?;
            }
        }
        self.child(element, offset)
    }

    pub fn scalar(&self) -> Result<Value> {
        let datatype = self.types.get(self.datatype)?;
        let primitive = match datatype.kind {
            TypeKind::Scalar(primitive) => primitive,
            TypeKind::Enum { underlying, .. } => match self.types.get(underlying)?.kind {
                TypeKind::Scalar(primitive) => primitive,
                _ => return Err(Error::Corrupt("invalid enum base")),
            },
            _ => return Err(Error::invalid("payload is not a scalar")),
        };
        let mut cursor = Cursor::new(self.bytes());
        let value = match primitive {
            Primitive::Unit => Value::Null,
            Primitive::Bool => Value::Bool(cursor.byte()? != 0),
            Primitive::I8 => Value::I8(cursor.byte()? as i8),
            Primitive::I16 => Value::I16(i16::from_le_bytes(cursor.fixed()?)),
            Primitive::I32 => Value::I32(i32::from_le_bytes(cursor.fixed()?)),
            Primitive::I64 => Value::I64(i64::from_le_bytes(cursor.fixed()?)),
            Primitive::U8 => Value::U8(cursor.byte()?),
            Primitive::U16 => Value::U16(u16::from_le_bytes(cursor.fixed()?)),
            Primitive::U32 => Value::U32(cursor.u32()?),
            Primitive::U64 => Value::U64(cursor.u64()?),
            Primitive::F32 => Value::F32(f32::from_bits(cursor.u32()?)),
            Primitive::F64 => Value::F64(f64::from_bits(cursor.u64()?)),
            Primitive::Str => Value::Str(cursor.u32()?),
            Primitive::Time => Value::Time(cursor.u64()?),
            Primitive::Pointer => Value::Pointer(cursor.u64()?),
            Primitive::Fixed => Value::Fixed {
                raw: cursor.u64()? as i64,
                scale: cursor.u64()? as i64,
            },
            Primitive::Ufixed => Value::UFixed {
                raw: cursor.u64()?,
                scale: cursor.u64()? as i64,
            },
            Primitive::Text | Primitive::Bytes => {
                let length = cursor.u32()? as usize;
                if length > self.allocation_limit {
                    return Err(Error::Corrupt("payload allocation limit exceeded"));
                }
                let bytes = cursor.take(length)?;
                if primitive == Primitive::Text {
                    Value::Text(
                        std::str::from_utf8(bytes)
                            .map_err(|_| Error::Corrupt("invalid UTF-8"))?
                            .into(),
                    )
                } else {
                    Value::Bytes(bytes.to_vec())
                }
            }
            Primitive::Bits | Primitive::Logic | Primitive::Logic9 => {
                let width = cursor.u64()?;
                let length = primitive.packed_bytes(width)?;
                if length > self.allocation_limit {
                    return Err(Error::Corrupt("payload allocation limit exceeded"));
                }
                let data = cursor.take(length)?.to_vec();
                match primitive {
                    Primitive::Bits => Value::Bits { width, data },
                    Primitive::Logic => Value::Logic { width, data },
                    _ => Value::Logic9 { width, data },
                }
            }
            Primitive::Enum => return Err(Error::Corrupt("undeclared enum payload")),
        };
        cursor.done()?;
        Ok(value)
    }
}

pub(super) fn decode_payload<T: Payload>(
    types: &Types,
    datatype: TypeId,
    bytes: &[u8],
    allocation_limit: usize,
) -> Result<T> {
    if !T::matches(types, datatype)? {
        return Err(Error::invalid(
            "payload datatype does not match the requested Rust type",
        ));
    }
    schema::decode(bytes, allocation_limit)
}
