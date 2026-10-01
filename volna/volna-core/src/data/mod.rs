//! Model layer: signal values, change histories, value translators and the
//! hierarchy model. Sessions (`crate::session`) produce these.

pub mod activity;
pub mod compact;
pub(crate) mod fst_source;
pub mod history;
pub mod loaded_tracks;
pub mod sizes;
pub mod source;
pub mod text;
pub mod transactions;
pub mod translator;
pub mod value;
pub mod value_view;
pub mod vtr_source;
mod vtr_transactions;

pub use activity::{ActivityBuildInfo, ActivityCounter, ActivityCounts, ScopeActivity};
pub use history::SignalHistory;
pub use sizes::{ScopeSize, ScopeSizes};
pub use source::{
    Direction, Hierarchy, HierarchyBuilder, Scope, ScopeId, ScopeView, SignalRef, TraceInfo, VarId,
    Variable, VariableView,
};
pub use source::{Generator, GeneratorId, Member, ScopeRole};
pub use translator::{NumericKind, Translated, Translator, Translators};
pub use value::{Bit, SignalShape, ValueKind, WaveValue};

pub(crate) mod hierarchy_columns;
