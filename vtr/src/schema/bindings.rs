//! Declarative bindings for named structures and integer enum wrappers.

use super::{Payload, TypeId, TypeKind, Types};
use crate::Result;

#[doc(hidden)]
pub fn field_type<O, T: Payload>(types: &mut Types, _: fn(&O) -> &T) -> Result<TypeId> {
    types.bind::<T>()
}

#[doc(hidden)]
#[inline]
#[must_use]
pub fn field_fixed_size<O, T: Payload>(_: fn(&O) -> &T) -> Option<usize> {
    T::fixed_size()
}

#[doc(hidden)]
pub fn field_matches<O, T: Payload>(
    types: &Types,
    datatype: TypeId,
    _: fn(&O) -> &T,
) -> Result<bool> {
    T::matches(types, datatype)
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
            fn fixed_size() -> Option<usize> {
                let mut size = 0usize;
                $(size = size.checked_add($crate::schema::field_fixed_size(|value: &Self| &value.$field)?)?;)*
                Some(size)
            }
            fn matches(types: &$crate::Types, datatype: $crate::TypeId) -> $crate::Result<bool> {
                let actual = types.get(datatype)?;
                if actual.name != $name { return Ok(false); }
                let $crate::schema::TypeKind::Struct(fields) = &actual.kind else { return Ok(false); };
                let mut fields = fields.iter();
                $(
                    let Some(field) = fields.next() else { return Ok(false); };
                    if field.name != ::core::stringify!($field).strip_prefix("r#").unwrap_or(::core::stringify!($field))
                        || !$crate::schema::field_matches(types, field.datatype, |value: &Self| &value.$field)? {
                        return Ok(false);
                    }
                )*
                Ok(fields.next().is_none())
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
            fn fixed_size() -> Option<usize> {
                $crate::schema::field_fixed_size(|value: &Self| &value.0)
            }
            fn matches(types: &$crate::Types, datatype: $crate::TypeId) -> $crate::Result<bool> {
                let actual = types.get(datatype)?;
                if actual.name != $name { return Ok(false); }
                let $crate::schema::TypeKind::Enum { underlying, values } = &actual.kind else { return Ok(false); };
                if !$crate::schema::field_matches(types, *underlying, |value: &Self| &value.0)? { return Ok(false); }
                let mut values = values.iter();
                $(
                    let Some(value) = values.next() else { return Ok(false); };
                    if value.name != $label || value.value != $crate::schema::enum_bits(|value: &Self| &value.0, $value) {
                        return Ok(false);
                    }
                )*
                Ok(values.next().is_none())
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
