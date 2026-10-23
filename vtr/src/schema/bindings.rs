//! Declarative bindings for named structures and integer enum wrappers.

use super::{Payload, TypeId, TypeKind, Types};
use crate::Result;

#[doc(hidden)]
pub fn field_type<O, T: Payload>(types: &mut Types, _: fn(&O) -> &T) -> Result<TypeId> {
    types.bind::<T>()
}

#[doc(hidden)]
pub fn enum_bits<O, T: Into<i128>>(_: fn(&O) -> &T, value: T) -> u64 {
    value.into() as u64
}

#[doc(hidden)]
pub fn binding_type(types: &mut Types, name: impl Into<String>, kind: TypeKind) -> Result<TypeId> {
    types.intern(name.into(), kind)
}

/// Bind named fields in declaration order without repeating their Rust types.
/// An optional first argument overrides the file-local schema name.
///
/// ```
/// struct Packet { pc: u64, text: String }
/// vtr::impl_payload!(Packet { pc, text });
/// let packet = Packet { pc: 0x8000, text: "retired".into() };
/// let bytes = vtr::schema::capture(&packet)?;
/// let copy = vtr::schema::decode::<Packet>(&bytes, 1024)?;
/// assert_eq!(copy.text, "retired");
/// # Ok::<(), vtr::Error>(())
/// ```
#[macro_export]
macro_rules! impl_payload {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        $crate::impl_payload!(::core::stringify!($ty), $ty { $($field),* });
    };
    ($name:expr, $ty:ty { $($field:ident),* $(,)? }) => {
        impl $crate::Payload for $ty {
            fn bind(types: &mut $crate::Types) -> $crate::Result<$crate::TypeId> {
                let fields = ::std::vec![$(
                    $crate::schema::Field::new(
                        ::core::stringify!($field).strip_prefix("r#").unwrap_or(::core::stringify!($field)),
                        $crate::schema::field_type(types, |value: &Self| &value.$field)?,
                    )
                ),*];
                $crate::schema::binding_type(types, $name, $crate::schema::TypeKind::Struct(fields))
            }
            fn encode(&self, out: &mut ::std::vec::Vec<u8>) -> $crate::Result<()> {
                $($crate::Payload::encode(&self.$field, out)?;)*
                let _ = out;
                ::core::result::Result::Ok(())
            }
            fn decode(reader: &mut $crate::PayloadReader<'_>) -> $crate::Result<Self> {
                let value = Self { $($field: $crate::Payload::decode(reader)?),* };
                let _ = reader;
                ::core::result::Result::Ok(value)
            }
        }
    };
}

/// Bind a one-field integer tuple struct as an enum, preserving unknown values.
/// An optional first argument overrides the file-local schema name.
///
/// ```
/// #[derive(Debug, PartialEq)]
/// struct Operation(i16);
/// vtr::impl_payload_enum!(Operation { "load" => -3, "store" => 7 });
/// let bytes = vtr::schema::capture(&Operation(-17))?;
/// assert_eq!(vtr::schema::decode::<Operation>(&bytes, 0)?, Operation(-17));
/// # Ok::<(), vtr::Error>(())
/// ```
#[macro_export]
macro_rules! impl_payload_enum {
    ($ty:ty { $($label:expr => $value:expr),* $(,)? }) => {
        $crate::impl_payload_enum!(::core::stringify!($ty), $ty { $($label => $value),* });
    };
    ($name:expr, $ty:ty { $($label:expr => $value:expr),* $(,)? }) => {
        impl $crate::Payload for $ty {
            fn bind(types: &mut $crate::Types) -> $crate::Result<$crate::TypeId> {
                let underlying = $crate::schema::field_type(types, |value: &Self| &value.0)?;
                let values = ::std::vec![$(
                    $crate::schema::Enumerator {
                        name: ::std::convert::Into::into($label),
                        value: $crate::schema::enum_bits(|value: &Self| &value.0, $value),
                    }
                ),*];
                $crate::schema::binding_type(types, $name, $crate::schema::TypeKind::Enum { underlying, values })
            }
            fn encode(&self, out: &mut ::std::vec::Vec<u8>) -> $crate::Result<()> {
                $crate::Payload::encode(&self.0, out)
            }
            fn decode(reader: &mut $crate::PayloadReader<'_>) -> $crate::Result<Self> {
                ::core::result::Result::Ok(Self($crate::Payload::decode(reader)?))
            }
        }
    };
}
