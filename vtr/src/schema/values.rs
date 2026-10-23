//! Typed bindings for semantic scalars and borrowed collections.

use std::borrow::Cow;

use super::{Payload, PayloadReader, Primitive, TypeId, Types};
use crate::{Error, Result};

macro_rules! scalar {
    ($($name:ident($inner:ty) => $primitive:ident),* $(,)?) => { $(
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(pub $inner);

        impl Payload for $name {
            fn bind(_: &mut Types) -> Result<TypeId> { Ok(Primitive::$primitive.id()) }
            fn encode(&self, out: &mut Vec<u8>) -> Result<()> { self.0.encode(out) }
            fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
                <$inner>::decode(reader).map(Self)
            }
        }
    )* };
}

scalar! { StringId(u32) => Str, TimeValue(u64) => Time, Address(u64) => Pointer }

macro_rules! fixed {
    ($name:ident, $raw:ty, $primitive:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name {
            pub raw: $raw,
            pub scale: i64,
        }

        impl Payload for $name {
            fn bind(_: &mut Types) -> Result<TypeId> {
                Ok(Primitive::$primitive.id())
            }
            fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
                self.raw.encode(out)?;
                self.scale.encode(out)
            }
            fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
                Ok(Self {
                    raw: <$raw>::decode(reader)?,
                    scale: i64::decode(reader)?,
                })
            }
        }
    };
}

fixed!(Fixed, i64, Fixed);
fixed!(UFixed, u64, Ufixed);

/// A byte blob has a distinct datatype from a sequence of unsigned bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bytes<T = Vec<u8>>(pub T);

impl<T: AsRef<[u8]> + From<Vec<u8>>> Payload for Bytes<T> {
    fn bind(_: &mut Types) -> Result<TypeId> {
        Ok(Primitive::Bytes.id())
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let bytes = self.0.as_ref();
        u32::try_from(bytes.len())
            .map_err(|_| Error::invalid("byte blob exceeds u32"))?
            .encode(out)?;
        out.extend_from_slice(bytes);
        Ok(())
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        let size = u32::decode(reader)? as usize;
        reader.spend(size)?;
        Ok(Self(reader.take(size)?.to_vec().into()))
    }
}

/// Packed states in least-significant-first order, with zero high padding bits.
/// The supported alphabets contain 2, 4, or 9 states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedVector<const STATES: u8> {
    pub width: u64,
    pub packed: Vec<u8>,
}

pub type Bits = PackedVector<2>;
pub type Logic4 = PackedVector<4>;
pub type Logic9 = PackedVector<9>;

impl<const STATES: u8> PackedVector<STATES> {
    fn primitive() -> Result<Primitive> {
        match STATES {
            2 => Ok(Primitive::Bits),
            4 => Ok(Primitive::Logic),
            9 => Ok(Primitive::Logic9),
            _ => Err(Error::invalid(
                "packed alphabet must have 2, 4, or 9 states",
            )),
        }
    }
}

impl<const STATES: u8> Payload for PackedVector<STATES> {
    fn bind(_: &mut Types) -> Result<TypeId> {
        Self::primitive().map(Primitive::id)
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let primitive = Self::primitive()?;
        if self.packed.len() != primitive.packed_bytes(self.width)? {
            return Err(Error::invalid(
                "packed payload length does not match its width",
            ));
        }
        primitive.validate_packed(&self.packed, self.width)?;
        self.width.encode(out)?;
        out.extend_from_slice(&self.packed);
        Ok(())
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        let primitive = Self::primitive()?;
        let width = u64::decode(reader)?;
        let size = primitive.packed_bytes(width)?;
        reader.spend(size)?;
        let packed = reader.take(size)?;
        primitive.validate_packed(packed, width)?;
        Ok(Self {
            width,
            packed: packed.to_vec(),
        })
    }
}

/// Borrowed captures use the same datatype as `String`; decoding owns the text.
impl Payload for Cow<'_, str> {
    fn bind(types: &mut Types) -> Result<TypeId> {
        String::bind(types)
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        super::encode_text(self, out)
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        String::decode(reader).map(Cow::Owned)
    }
}

/// Borrowed captures use the same datatype as `Vec<T>`; decoding owns the items.
impl<T: Payload + Clone> Payload for Cow<'_, [T]> {
    fn bind(types: &mut Types) -> Result<TypeId> {
        Vec::<T>::bind(types)
    }
    fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        super::encode_sequence(self, out)
    }
    fn decode(reader: &mut PayloadReader<'_>) -> Result<Self> {
        Vec::<T>::decode(reader).map(Cow::Owned)
    }
}
