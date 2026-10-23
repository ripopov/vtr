//! File-local datatype declarations and padding-free canonical payloads.

use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
};

mod bindings;
mod values;
#[doc(hidden)]
pub use bindings::{binding_type, enum_bits, field_fixed_size, field_matches, field_type};
pub use values::{
    Address, Bits, Bytes, Fixed, Logic4, Logic9, PackedVector, StringId, TimeValue, UFixed,
};

use crate::{
    Error, Result,
    varint::{put_blob, put_u64},
    wire::Cursor,
};

pub type TypeId = u32;
pub const FIRST_DECLARED_TYPE: TypeId = 23;
pub const DEFAULT_PAYLOAD_LIMIT: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Primitive {
    Unit,
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
    Text,
    Bytes,
    Bits,
    Logic,
    Logic9,
    Time,
    Enum,
    Pointer,
    Fixed,
    Ufixed,
}

impl Primitive {
    #[must_use]
    pub const fn id(self) -> TypeId {
        self as TypeId
    }

    pub(crate) fn width(self) -> Option<usize> {
        Some(match self {
            Self::Unit => 0,
            Self::Bool | Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 | Self::Str => 4,
            Self::I64 | Self::U64 | Self::F64 | Self::Time | Self::Pointer => 8,
            Self::Fixed | Self::Ufixed => 16,
            _ => return None,
        })
    }

    pub(crate) fn signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }

    pub(crate) fn integer(self) -> bool {
        self.signed() || matches!(self, Self::U8 | Self::U16 | Self::U32 | Self::U64)
    }

    #[inline]
    pub(crate) fn packed_bytes(self, bits: u64) -> Result<usize> {
        let per_byte = match self {
            Self::Bits => 8,
            Self::Logic => 4,
            Self::Logic9 => 2,
            _ => return Err(Error::Corrupt("not a packed primitive")),
        };
        let bytes = bits / per_byte + u64::from(!bits.is_multiple_of(per_byte));
        if bytes > u64::from(u32::MAX) {
            return Err(Error::Corrupt("packed argument exceeds u32"));
        }
        Ok(bytes as usize)
    }

    #[inline]
    pub(crate) fn validate_packed(self, bytes: &[u8], bits: u64) -> Result<()> {
        let stride = match self {
            Self::Bits => 1,
            Self::Logic => 2,
            Self::Logic9 => 4,
            _ => return Err(Error::Corrupt("not a packed primitive")),
        };
        let remainder = (bits % (8 / stride)) * stride;
        if remainder != 0 && bytes.last().is_some_and(|b| b >> remainder != 0) {
            return Err(Error::Corrupt("nonzero packed padding"));
        }
        if self == Self::Logic9
            && bytes
                .iter()
                .enumerate()
                .any(|(i, b)| b & 15 > 8 || ((i as u64 * 2 + 1) < bits && b >> 4 > 8))
        {
            return Err(Error::Corrupt("invalid logic9 state"));
        }
        Ok(())
    }
}

const PRIMITIVES: [Primitive; 23] = [
    Primitive::Unit,
    Primitive::Bool,
    Primitive::I8,
    Primitive::I16,
    Primitive::I32,
    Primitive::I64,
    Primitive::U8,
    Primitive::U16,
    Primitive::U32,
    Primitive::U64,
    Primitive::F32,
    Primitive::F64,
    Primitive::Str,
    Primitive::Text,
    Primitive::Bytes,
    Primitive::Bits,
    Primitive::Logic,
    Primitive::Logic9,
    Primitive::Time,
    Primitive::Enum,
    Primitive::Pointer,
    Primitive::Fixed,
    Primitive::Ufixed,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub datatype: TypeId,
}

impl Field {
    pub fn new(name: impl Into<String>, datatype: TypeId) -> Self {
        Self {
            name: name.into(),
            datatype,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enumerator {
    /// Signed underlying values use their sign-extended two's-complement bits.
    pub value: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Scalar(Primitive),
    Struct(Vec<Field>),
    Array {
        element: TypeId,
        count: u32,
    },
    Sequence {
        element: TypeId,
    },
    Enum {
        underlying: TypeId,
        values: Vec<Enumerator>,
    },
}

#[derive(Clone, Debug)]
pub struct Datatype {
    pub name: String,
    pub kind: TypeKind,
    pub minimum_size: u32,
    pub fixed_size: Option<u32>,
    pub depth: u8,
    pub contains_text: bool,
    pub(crate) scalars: Option<Vec<Primitive>>,
    unsigned_words: Option<usize>,
    validates_by_size: bool,
}

impl Datatype {
    /// Eligible scalar pages contain at most 256 unsigned 64-bit leaves.
    #[must_use]
    pub fn unsigned_words(&self) -> Option<usize> {
        self.unsigned_words
    }

    fn compile_validation(mut self) -> Self {
        if self.fixed_size.is_some()
            && let Some(scalars) = &self.scalars
        {
            self.unsigned_words = scalars
                .iter()
                .all(|p| matches!(p, Primitive::U64 | Primitive::Time | Primitive::Pointer))
                .then_some(scalars.len());
            self.validates_by_size = scalars
                .iter()
                .all(|p| p.width().is_some() && !matches!(p, Primitive::Bool | Primitive::Str));
        }
        self
    }
}

#[derive(Clone, Debug)]
pub struct Types {
    types: Arc<Vec<Datatype>>,
    binding_depth: u8,
}

impl Default for Types {
    fn default() -> Self {
        static PRIMITIVE_TYPES: OnceLock<Arc<Vec<Datatype>>> = OnceLock::new();
        Self {
            binding_depth: 0,
            types: PRIMITIVE_TYPES
                .get_or_init(|| {
                    Arc::new(
                        PRIMITIVES
                            .into_iter()
                            .map(|primitive| {
                                let fixed_size = primitive.width().map(|n| n as u32);
                                Datatype {
                                    name: String::new(),
                                    kind: TypeKind::Scalar(primitive),
                                    minimum_size: fixed_size.unwrap_or(
                                        if matches!(primitive, Primitive::Text | Primitive::Bytes) {
                                            4
                                        } else {
                                            8
                                        },
                                    ),
                                    fixed_size,
                                    depth: 1,
                                    contains_text: primitive == Primitive::Text,
                                    unsigned_words: None,
                                    validates_by_size: false,
                                    scalars: Some(if primitive == Primitive::Unit {
                                        vec![]
                                    } else {
                                        vec![primitive]
                                    }),
                                }
                                .compile_validation()
                            })
                            .collect(),
                    )
                })
                .clone(),
        }
    }
}

impl Types {
    pub(crate) fn memory_charge(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.types.capacity() * std::mem::size_of::<Datatype>()
            + self
                .types
                .iter()
                .map(|datatype| {
                    datatype.name.capacity()
                        + datatype.scalars.as_ref().map_or(0, |leaves| {
                            leaves.capacity() * std::mem::size_of::<Primitive>()
                        })
                        + match &datatype.kind {
                            TypeKind::Struct(fields) => {
                                fields.capacity() * std::mem::size_of::<Field>()
                                    + fields
                                        .iter()
                                        .map(|field| field.name.capacity())
                                        .sum::<usize>()
                            }
                            TypeKind::Enum { values, .. } => {
                                values.capacity() * std::mem::size_of::<Enumerator>()
                                    + values
                                        .iter()
                                        .map(|value| value.name.capacity())
                                        .sum::<usize>()
                            }
                            _ => 0,
                        }
                })
                .sum::<usize>()
    }
    pub(crate) fn bind<T: Payload>(&mut self) -> Result<TypeId> {
        if self.binding_depth >= 64 {
            return Err(Error::Corrupt(
                "recursive datatype or binding depth exceeds 64",
            ));
        }
        self.binding_depth += 1;
        struct Binding<'a>(&'a mut Types);
        impl Drop for Binding<'_> {
            fn drop(&mut self) {
                self.0.binding_depth -= 1;
            }
        }
        let binding = Binding(self);
        T::bind(&mut *binding.0)
    }
    pub(crate) fn canonical_len(&self, datatype: TypeId, payload: &[u8]) -> Result<usize> {
        let mut cursor = Cursor::new(payload);
        self.walk(datatype, &mut cursor, &mut Validator(u64::MAX))?;
        Ok(payload.len() - cursor.remaining())
    }

    pub fn equivalent(
        &self,
        actual: TypeId,
        expected_types: &Self,
        expected: TypeId,
    ) -> Result<bool> {
        let actual = self.get(actual)?;
        let expected = expected_types.get(expected)?;
        Ok(match (&actual.kind, &expected.kind) {
            (TypeKind::Scalar(a), TypeKind::Scalar(b)) => a == b,
            (TypeKind::Struct(a), TypeKind::Struct(b)) => {
                actual.name == expected.name
                    && a.len() == b.len()
                    && a.iter().zip(b).all(|(a, b)| {
                        a.name == b.name
                            && self
                                .equivalent(a.datatype, expected_types, b.datatype)
                                .unwrap_or(false)
                    })
            }
            (
                TypeKind::Array {
                    element: a,
                    count: an,
                },
                TypeKind::Array {
                    element: b,
                    count: bn,
                },
            ) => an == bn && self.equivalent(*a, expected_types, *b)?,
            (TypeKind::Sequence { element: a }, TypeKind::Sequence { element: b }) => {
                self.equivalent(*a, expected_types, *b)?
            }
            (
                TypeKind::Enum {
                    underlying: a,
                    values: av,
                },
                TypeKind::Enum {
                    underlying: b,
                    values: bv,
                },
            ) => {
                actual.name == expected.name
                    && av == bv
                    && self.equivalent(*a, expected_types, *b)?
            }
            _ => false,
        })
    }
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.types.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    #[inline]
    pub fn get(&self, id: TypeId) -> Result<&Datatype> {
        self.types
            .get(id as usize)
            .filter(|_| id != Primitive::Enum.id())
            .ok_or_else(|| Error::corrupt("unknown payload datatype"))
    }

    pub fn structure(&mut self, name: impl Into<String>, fields: Vec<Field>) -> Result<TypeId> {
        self.declare(name.into(), TypeKind::Struct(fields))
    }

    pub fn array(
        &mut self,
        name: impl Into<String>,
        element: TypeId,
        count: u32,
    ) -> Result<TypeId> {
        self.declare(name.into(), TypeKind::Array { element, count })
    }

    pub fn sequence(&mut self, name: impl Into<String>, element: TypeId) -> Result<TypeId> {
        self.declare(name.into(), TypeKind::Sequence { element })
    }

    pub fn enumeration(
        &mut self,
        name: impl Into<String>,
        underlying: TypeId,
        values: Vec<Enumerator>,
    ) -> Result<TypeId> {
        self.declare(name.into(), TypeKind::Enum { underlying, values })
    }

    fn declare(&mut self, name: String, kind: TypeKind) -> Result<TypeId> {
        let mut datatype = Datatype {
            name,
            kind,
            minimum_size: 0,
            fixed_size: Some(0),
            depth: 1,
            contains_text: false,
            scalars: Some(vec![]),
            unsigned_words: None,
            validates_by_size: false,
        };
        let mut children = Vec::new();
        match &datatype.kind {
            TypeKind::Scalar(_) => return Err(Error::Corrupt("declared scalar datatype")),
            TypeKind::Struct(fields) => {
                unique_names(fields.iter().map(|f| f.name.as_str()))?;
                children.extend(fields.iter().map(|f| (f.datatype, 1)));
            }
            TypeKind::Array { element, count } => children.push((*element, *count)),
            TypeKind::Sequence { element } => children.push((*element, 0)),
            TypeKind::Enum { underlying, values } => {
                let TypeKind::Scalar(primitive) = self.get(*underlying)?.kind else {
                    return Err(Error::Corrupt("enum underlying type must be an integer"));
                };
                if !primitive.integer() {
                    return Err(Error::Corrupt("enum underlying type must be an integer"));
                }
                unique_names(values.iter().map(|v| v.name.as_str()))?;
                for value in values {
                    integer_range(primitive, value.value)?;
                }
                children.push((*underlying, 1));
            }
        }
        for (id, count) in children {
            let child = self.get(id)?;
            datatype.minimum_size = child
                .minimum_size
                .checked_mul(count)
                .and_then(|size| datatype.minimum_size.checked_add(size))
                .ok_or_else(|| Error::corrupt("datatype minimum size exceeds u32"))?;
            datatype.fixed_size = match (datatype.fixed_size, child.fixed_size) {
                (Some(total), Some(size)) => Some(
                    size.checked_mul(count)
                        .and_then(|size| total.checked_add(size))
                        .ok_or_else(|| Error::corrupt("fixed datatype size exceeds u32"))?,
                ),
                _ => None,
            };
            datatype.depth = datatype.depth.max(child.depth + 1);
            if datatype.depth > 64 {
                return Err(Error::Corrupt("datatype nesting exceeds 64"));
            }
            datatype.contains_text |= child.contains_text
                && (count != 0 || matches!(datatype.kind, TypeKind::Sequence { .. }));
            datatype.scalars = match (datatype.scalars.take(), child.scalars.as_ref()) {
                (Some(mut program), Some(child))
                    if child.is_empty()
                        || count as usize <= (256 - program.len()) / child.len() =>
                {
                    if !child.is_empty() {
                        for _ in 0..count {
                            program.extend_from_slice(child);
                        }
                    }
                    Some(program)
                }
                _ => None,
            };
        }
        if matches!(datatype.kind, TypeKind::Sequence { .. }) {
            datatype.minimum_size = 4;
            datatype.fixed_size = None;
            datatype.scalars = None;
        }
        let id = u32::try_from(self.types.len())
            .ok()
            .filter(|id| *id != u32::MAX)
            .ok_or_else(|| Error::corrupt("datatype identifier exhaustion"))?;
        Arc::make_mut(&mut self.types).push(datatype.compile_validation());
        Ok(id)
    }

    fn intern(&mut self, name: String, kind: TypeKind) -> Result<TypeId> {
        if let Some((id, _)) = self
            .types
            .iter()
            .enumerate()
            .skip(FIRST_DECLARED_TYPE as usize)
            .find(|(_, datatype)| datatype.name == name && datatype.kind == kind)
        {
            return Ok(id as TypeId);
        }
        self.declare(name, kind)
    }

    pub fn declarations(&self, first: TypeId) -> Result<Vec<u8>> {
        if first < FIRST_DECLARED_TYPE || first as usize > self.len() {
            return Err(Error::Corrupt("invalid datatype chunk start"));
        }
        let mut out = Vec::new();
        put_u64(&mut out, u64::from(first));
        put_u64(&mut out, (self.len() - first as usize) as u64);
        for datatype in &self.types[first as usize..] {
            out.push(match datatype.kind {
                TypeKind::Struct(_) => 1,
                TypeKind::Array { .. } => 2,
                TypeKind::Sequence { .. } => 3,
                TypeKind::Enum { .. } => 4,
                TypeKind::Scalar(_) => unreachable!(),
            });
            put_blob(&mut out, datatype.name.as_bytes());
            match &datatype.kind {
                TypeKind::Struct(fields) => {
                    put_u64(&mut out, fields.len() as u64);
                    for field in fields {
                        put_blob(&mut out, field.name.as_bytes());
                        put_u64(&mut out, u64::from(field.datatype));
                    }
                }
                TypeKind::Array { element, count } => {
                    put_u64(&mut out, u64::from(*element));
                    put_u64(&mut out, u64::from(*count));
                }
                TypeKind::Sequence { element } => put_u64(&mut out, u64::from(*element)),
                TypeKind::Enum { underlying, values } => {
                    put_u64(&mut out, u64::from(*underlying));
                    put_u64(&mut out, values.len() as u64);
                    for value in values {
                        out.extend_from_slice(&value.value.to_le_bytes());
                        put_blob(&mut out, value.name.as_bytes());
                    }
                }
                TypeKind::Scalar(_) => unreachable!(),
            }
        }
        Ok(out)
    }

    /// Invalid chunks leave all previously published declarations intact.
    pub fn read_declarations(&mut self, data: &[u8]) -> Result<()> {
        let mut cursor = Cursor::new(data);
        if cursor.id()? as usize != self.len() {
            return Err(Error::Corrupt("noncontiguous datatype chunk"));
        }
        let count = cursor.id()?;
        if count as usize > cursor.remaining() / 2 {
            return Err(Error::Corrupt("invalid datatype count"));
        }
        let mut next = self.clone();
        for _ in 0..count {
            let tag = cursor.byte()?;
            let name = cursor.text()?.to_owned();
            let kind = match tag {
                1 => {
                    let count = cursor.id()?;
                    if count as usize > cursor.remaining() / 2 {
                        return Err(Error::Corrupt("invalid datatype field count"));
                    }
                    let mut fields = Vec::new();
                    for _ in 0..count {
                        fields.push(Field::new(cursor.text()?, cursor.id()?));
                    }
                    TypeKind::Struct(fields)
                }
                2 => TypeKind::Array {
                    element: cursor.id()?,
                    count: cursor.id()?,
                },
                3 => TypeKind::Sequence {
                    element: cursor.id()?,
                },
                4 => {
                    let underlying = cursor.id()?;
                    let count = cursor.id()?;
                    if count as usize > cursor.remaining() / 9 {
                        return Err(Error::Corrupt("invalid enum literal count"));
                    }
                    let mut values = Vec::new();
                    for _ in 0..count {
                        values.push(Enumerator {
                            value: cursor.u64()?,
                            name: cursor.text()?.into(),
                        });
                    }
                    TypeKind::Enum { underlying, values }
                }
                _ => return Err(Error::Corrupt("invalid declared datatype kind")),
            };
            next.declare(name, kind)?;
        }
        cursor.done()?;
        *self = next;
        Ok(())
    }

    pub fn validate(&self, datatype: TypeId, payload: &[u8], string_count: u64) -> Result<()> {
        if payload.len() > u32::MAX as usize {
            return Err(Error::Corrupt("payload exceeds u32"));
        }
        let layout = self.get(datatype)?;
        if layout.validates_by_size {
            return if Some(payload.len() as u32) == layout.fixed_size {
                Ok(())
            } else {
                Err(Error::Corrupt("fixed typed payload size mismatch"))
            };
        }
        let mut cursor = Cursor::new(payload);
        self.walk(datatype, &mut cursor, &mut Validator(string_count))?;
        cursor.done()
    }

    pub(crate) fn walk(
        &self,
        id: TypeId,
        row: &mut Cursor<'_>,
        visitor: &mut impl PayloadVisitor,
    ) -> Result<()> {
        let datatype = self.get(id)?;
        if datatype.minimum_size as usize > row.remaining() {
            return Err(Error::Corrupt("truncated typed payload"));
        }
        if let Some(program) = &datatype.scalars {
            for primitive in program {
                visitor.scalar(*primitive, row)?;
            }
            return Ok(());
        }
        let (element, n) = match &datatype.kind {
            TypeKind::Struct(fields) => {
                for field in fields {
                    self.walk(field.datatype, row, visitor)?;
                }
                return Ok(());
            }
            TypeKind::Array { element, count } => (*element, *count),
            TypeKind::Sequence { element } => {
                let n = row.u32()?;
                visitor.count(n)?;
                (*element, n)
            }
            _ => return Err(Error::Corrupt("invalid datatype traversal")),
        };
        let child = self.get(element)?;
        if child.minimum_size != 0 && n as usize > row.remaining() / child.minimum_size as usize {
            return Err(Error::Corrupt("truncated array payload"));
        }
        if child.fixed_size != Some(0) {
            for _ in 0..n {
                self.walk(element, row, visitor)?;
            }
        }
        Ok(())
    }
}

pub(crate) trait PayloadVisitor {
    fn scalar(&mut self, primitive: Primitive, row: &mut Cursor<'_>) -> Result<()>;
    fn count(&mut self, count: u32) -> Result<()>;
}

struct Validator(u64);

impl PayloadVisitor for Validator {
    fn scalar(&mut self, primitive: Primitive, row: &mut Cursor<'_>) -> Result<()> {
        validate_scalar(primitive, row, self.0)
    }
    fn count(&mut self, _: u32) -> Result<()> {
        Ok(())
    }
}

fn unique_names<'a>(names: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut seen = HashSet::new();
    for name in names {
        if name.is_empty() || !seen.insert(name) {
            return Err(Error::Corrupt("duplicate or empty datatype name"));
        }
    }
    Ok(())
}

pub(crate) fn integer_range(primitive: Primitive, value: u64) -> Result<()> {
    let width = primitive
        .width()
        .ok_or_else(|| Error::corrupt("invalid integer primitive"))?;
    if width == 8 {
        return Ok(());
    }
    let shift = 64 - width * 8;
    let valid = if primitive.signed() {
        (((value << shift) as i64 >> shift) as u64) == value
    } else {
        value >> (width * 8) == 0
    };
    if valid {
        Ok(())
    } else {
        Err(Error::Corrupt("integer argument out of range"))
    }
}

pub(crate) fn validate_scalar(
    primitive: Primitive,
    row: &mut Cursor<'_>,
    strings: u64,
) -> Result<()> {
    match primitive {
        Primitive::Bool => {
            if row.byte()? > 1 {
                return Err(Error::Corrupt("invalid bool argument"));
            }
        }
        Primitive::Str => {
            if u64::from(row.u32()?) >= strings {
                return Err(Error::Corrupt("unknown argument string"));
            }
        }
        Primitive::Text | Primitive::Bytes => {
            let size = row.u32()? as usize;
            let data = row.take(size)?;
            if primitive == Primitive::Text && std::str::from_utf8(data).is_err() {
                return Err(Error::Corrupt("invalid UTF-8"));
            }
        }
        Primitive::Bits | Primitive::Logic | Primitive::Logic9 => {
            let bits = row.u64()?;
            let data = row.take(primitive.packed_bytes(bits)?)?;
            primitive.validate_packed(data, bits)?;
        }
        _ => {
            row.take(
                primitive
                    .width()
                    .ok_or_else(|| Error::corrupt("enum requires a declaration"))?,
            )?;
        }
    }
    Ok(())
}

/// A typed binding writes fields in declaration order, excluding native padding.
pub trait Payload: Sized {
    fn bind(types: &mut Types) -> Result<TypeId>;
    /// Encoded byte count when it is independent of the payload value.
    #[must_use]
    #[inline]
    fn fixed_size() -> Option<usize> {
        None
    }
    /// Compare against existing declarations without constructing a second schema.
    fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
        let mut expected = Types::new();
        let expected_id = Self::bind(&mut expected)?;
        types.equivalent(datatype, &expected, expected_id)
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()>;
    /// Append consecutive canonical values without an array or sequence prefix.
    #[inline]
    fn encode_slice(values: &[Self], out: &mut Vec<u8>) -> Result<()> {
        for value in values {
            value.encode(out)?;
        }
        Ok(())
    }
    fn decode(data: &mut PayloadReader<'_>) -> Result<Self>;
}

pub struct PayloadReader<'a> {
    cursor: Cursor<'a>,
    allocation_budget: usize,
}

impl<'a> PayloadReader<'a> {
    #[must_use]
    pub fn new(data: &'a [u8], allocation_budget: usize) -> Self {
        Self {
            cursor: Cursor::new(data),
            allocation_budget,
        }
    }

    #[inline]
    pub fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        self.cursor.take(size)
    }

    #[inline]
    pub fn spend(&mut self, bytes: usize) -> Result<()> {
        self.allocation_budget = self
            .allocation_budget
            .checked_sub(bytes)
            .ok_or_else(|| Error::corrupt("payload allocation budget exceeded"))?;
        Ok(())
    }

    #[inline]
    pub fn done(self) -> Result<()> {
        self.cursor.done()
    }
}

pub fn capture<T: Payload>(value: &T) -> Result<Vec<u8>> {
    let size = T::fixed_size().unwrap_or(0);
    if size > u32::MAX as usize {
        return Err(Error::invalid("payload exceeds u32"));
    }
    let mut out = Vec::with_capacity(size);
    value.encode(&mut out)?;
    if out.len() > u32::MAX as usize {
        return Err(Error::invalid("payload exceeds u32"));
    }
    Ok(out)
}

pub fn decode<T: Payload>(bytes: &[u8], allocation_budget: usize) -> Result<T> {
    let mut reader = PayloadReader::new(bytes, allocation_budget);
    let value = T::decode(&mut reader)?;
    reader.done()?;
    Ok(value)
}

macro_rules! scalar_payload {
    ($($ty:ty => $primitive:ident),* $(,)?) => { $(
        impl Payload for $ty {
            fn bind(_: &mut Types) -> Result<TypeId> { Ok(Primitive::$primitive.id()) }
            #[inline]
            fn fixed_size() -> Option<usize> { Some(size_of::<Self>()) }
            #[inline]
            fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
                Ok(matches!(types.get(datatype)?.kind, TypeKind::Scalar(Primitive::$primitive)))
            }
            #[inline]
            fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
                out.extend_from_slice(&self.to_le_bytes()); Ok(())
            }
            #[inline]
            fn encode_slice(values: &[Self], out: &mut Vec<u8>) -> Result<()> {
                #[cfg(target_endian = "little")]
                {
                    // SAFETY: Numeric primitives have no padding; on little-endian targets
                    // their initialized object bytes are the canonical wire bytes.
                    let bytes = unsafe { std::slice::from_raw_parts(
                        values.as_ptr().cast::<u8>(), std::mem::size_of_val(values),
                    ) };
                    out.extend_from_slice(bytes);
                }
                #[cfg(target_endian = "big")]
                for value in values { value.encode(out)?; }
                Ok(())
            }
            #[inline]
            fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
                Ok(Self::from_le_bytes(reader.take(size_of::<Self>())?.try_into().unwrap()))
            }
        }
    )* };
}
scalar_payload! { i8 => I8, i16 => I16, i32 => I32, i64 => I64,
u8 => U8, u16 => U16, u32 => U32, u64 => U64, f32 => F32, f64 => F64 }

impl Payload for () {
    fn bind(_: &mut Types) -> Result<TypeId> {
        Ok(Primitive::Unit.id())
    }
    #[inline]
    fn fixed_size() -> Option<usize> {
        Some(0)
    }
    fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
        Ok(matches!(
            types.get(datatype)?.kind,
            TypeKind::Scalar(Primitive::Unit)
        ))
    }
    fn encode(&self, _: &mut Vec<u8>) -> Result<()> {
        Ok(())
    }
    fn decode(_: &mut PayloadReader<'_>) -> Result<Self> {
        Ok(())
    }
}

impl Payload for bool {
    fn bind(_: &mut Types) -> Result<TypeId> {
        Ok(Primitive::Bool.id())
    }
    #[inline]
    fn fixed_size() -> Option<usize> {
        Some(1)
    }
    fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
        Ok(matches!(
            types.get(datatype)?.kind,
            TypeKind::Scalar(Primitive::Bool)
        ))
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        out.push(u8::from(*self));
        Ok(())
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        match reader.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Corrupt("invalid bool argument")),
        }
    }
}

impl Payload for String {
    fn bind(_: &mut Types) -> Result<TypeId> {
        Ok(Primitive::Text.id())
    }
    fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
        Ok(matches!(
            types.get(datatype)?.kind,
            TypeKind::Scalar(Primitive::Text)
        ))
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        encode_text(self, out)
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        let size = u32::decode(reader)? as usize;
        reader.spend(size)?;
        let text =
            std::str::from_utf8(reader.take(size)?).map_err(|_| Error::Corrupt("invalid UTF-8"))?;
        Ok(text.to_owned())
    }
}

impl<T: Payload> Payload for Vec<T> {
    fn bind(types: &mut Types) -> Result<TypeId> {
        let element = types.bind::<T>()?;
        types.intern(String::new(), TypeKind::Sequence { element })
    }
    fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
        match types.get(datatype)?.kind {
            TypeKind::Sequence { element } => T::matches(types, element),
            _ => Ok(false),
        }
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        encode_sequence(self, out)
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        let size = u32::decode(reader)? as usize;
        // Zero-sized members still require bounded iteration and owned capacity.
        reader.spend(
            size.checked_mul(size_of::<T>().max(1))
                .ok_or_else(|| Error::corrupt("payload allocation budget exceeded"))?,
        )?;
        let mut out = Vec::with_capacity(size);
        for _ in 0..size {
            out.push(T::decode(reader)?);
        }
        Ok(out)
    }
}

fn encode_text(text: &str, out: &mut Vec<u8>) -> Result<()> {
    u32::try_from(text.len())
        .map_err(|_| Error::invalid("text exceeds u32"))?
        .encode(out)?;
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn encode_sequence<T: Payload>(items: &[T], out: &mut Vec<u8>) -> Result<()> {
    u32::try_from(items.len())
        .map_err(|_| Error::invalid("sequence exceeds u32"))?
        .encode(out)?;
    T::encode_slice(items, out)
}

impl<T: Payload, const N: usize> Payload for [T; N] {
    fn bind(types: &mut Types) -> Result<TypeId> {
        let element = types.bind::<T>()?;
        let count = u32::try_from(N).map_err(|_| Error::invalid("array exceeds u32"))?;
        types.intern(String::new(), TypeKind::Array { element, count })
    }
    #[inline]
    fn fixed_size() -> Option<usize> {
        T::fixed_size()?.checked_mul(N)
    }
    #[inline]
    fn matches(types: &Types, datatype: TypeId) -> Result<bool> {
        match types.get(datatype)?.kind {
            TypeKind::Array { element, count } if count as usize == N => T::matches(types, element),
            _ => Ok(false),
        }
    }
    #[inline]
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        T::encode_slice(self, out)
    }
    #[inline]
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        reader.spend(
            N.checked_mul(size_of::<T>().max(1))
                .ok_or_else(|| Error::corrupt("payload allocation budget exceeded"))?,
        )?;
        let mut values = std::array::from_fn(|_| None);
        for value in &mut values {
            *value = Some(T::decode(reader)?);
        }
        Ok(values.map(Option::unwrap))
    }
}
