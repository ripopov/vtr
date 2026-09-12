//! Raw transaction query contract. Identities are local to an open session;
//! names and attribute values are resolved, with no backend string handles.

// These are semantic enums, not reader handles or storage representations.
pub use vtr::{AttrPhase, TxKind, TxStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransactionRef(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrackRef(pub u32);

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub enum TrackKind {
    Stream { kind: String },
    Generator { stream: TrackRef },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: TrackRef,
    pub path: Vec<String>,
    pub kind: TrackKind,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransactionAttribute {
    pub key: String,
    pub phase: AttrPhase,
    pub value: AttributeValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransactionEvent {
    pub time: u64,
    pub name: String,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransactionStage {
    pub name: String,
    pub lane: String,
    pub begin: u64,
    pub end: Option<u64>,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    pub id: TransactionRef,
    pub generator: TrackRef,
    pub begin: u64,
    pub end: u64,
    pub status: TxStatus,
    pub kind: TxKind,
    pub parent: Option<TransactionRef>,
    pub attributes: Vec<TransactionAttribute>,
    pub events: Vec<TransactionEvent>,
    pub stages: Vec<TransactionStage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    pub kind: String,
    pub from: TransactionRef,
    pub to: TransactionRef,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, Default)]
pub struct TransactionQuery {
    pub generator: Option<TrackRef>,
    pub stream: Option<TrackRef>,
    /// Inclusive overlap [start, end], following VTR transaction semantics.
    /// Point transactions at either boundary are included.
    pub window: Option<(u64, u64)>,
}

/// Optional session facet. Absence means unsupported, whereas an empty track
/// list or zero callback invocations means supported but empty.
pub trait TransactionQueries: Send + Sync {
    /// Resident metadata; no trace decoding.
    fn tracks(&self) -> &[Track];
    /// Blocking query. Visit order is backend order, not chronological order.
    /// Return false to stop early. The callback borrows each resolved record.
    fn visit_transactions(
        &self,
        query: &TransactionQuery,
        visitor: &mut dyn FnMut(&Transaction) -> bool,
    ) -> anyhow::Result<()>;
    /// A missing identity returns None, not an unsupported-operation error.
    fn transaction(&self, id: TransactionRef) -> anyhow::Result<Option<Transaction>>;
}

/// Relations are separate capabilities; future sources can implement either
/// facet without manufacturing the other domain.
pub trait RelationQueries: Send + Sync {
    /// Blocking query. A missing endpoint returns an empty list.
    fn relations_from(&self, id: TransactionRef) -> anyhow::Result<Vec<Relation>>;
    fn relations_to(&self, id: TransactionRef) -> anyhow::Result<Vec<Relation>>;
}
