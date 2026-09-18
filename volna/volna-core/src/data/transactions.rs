//! Raw transaction records and catalog. Identities are local to an open session;
//! names and attribute values are resolved, with no backend string handles.

use serde::{Deserialize, Serialize};
// Semantic enums, not reader handles or storage representations.
pub use vtr::{AttrPhase, TxKind, TxStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransactionRef(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackRef(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AttributeValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
    /// Packed LSB-first codes, with 2, 4 or 9 logic states per bit.
    Logic {
        width: u32,
        states: u8,
        data: Vec<u8>,
    },
    Time(u64),
    Enum {
        value: i64,
        name: String,
    },
    Pointer(u64),
    Fixed {
        raw: i64,
        scale: i32,
    },
    UFixed {
        raw: u64,
        scale: i32,
    },
    List(Vec<AttributeValue>),
    Map(Attributes),
}

pub type Attributes = Vec<(String, AttributeValue)>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrackKind {
    Stream { kind: String },
    Generator { stream: TrackRef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackRef,
    pub path: Vec<String>,
    pub kind: TrackKind,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionAttribute {
    pub key: String,
    #[serde(with = "phase_code")]
    pub phase: AttrPhase,
    pub value: AttributeValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionEvent {
    pub time: u64,
    pub name: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransactionStage {
    pub name: String,
    pub lane: String,
    pub begin: u64,
    pub end: Option<u64>,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: TransactionRef,
    pub generator: TrackRef,
    pub begin: u64,
    pub end: u64,
    #[serde(with = "status_code")]
    pub status: TxStatus,
    #[serde(with = "kind_code")]
    pub kind: TxKind,
    pub parent: Option<TransactionRef>,
    pub attributes: Vec<TransactionAttribute>,
    pub events: Vec<TransactionEvent>,
    pub stages: Vec<TransactionStage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub kind: String,
    pub from: TransactionRef,
    pub to: TransactionRef,
    pub attributes: Attributes,
}

// Keep wire serialization in the adapter; the VTR crate need not depend on
// serde. Reject unknown codes instead of using the reader enums' fallbacks.
macro_rules! wire_code {
    ($module:ident, $ty:ident, $max:expr) => {
        mod $module {
            use super::*;
            pub fn serialize<S: serde::Serializer>(
                value: &$ty,
                serializer: S,
            ) -> Result<S::Ok, S::Error> {
                serializer.serialize_u8(*value as u8)
            }
            pub fn deserialize<'de, D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<$ty, D::Error> {
                let value = u8::deserialize(deserializer)?;
                if value > $max {
                    return Err(serde::de::Error::custom("unknown transaction code"));
                }
                Ok($ty::from_u8(value))
            }
        }
    };
}
wire_code!(phase_code, AttrPhase, 2);
wire_code!(status_code, TxStatus, 4);
wire_code!(kind_code, TxKind, 5);
