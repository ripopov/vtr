//! Model layer: signal values, change histories, value translators and the
//! hierarchy model. Sessions (`crate::session`) produce these.

pub(crate) mod fst_source;
pub mod history;
pub mod loaded_tracks;
pub mod source;
pub mod synth;
pub mod transactions;
pub mod translator;
pub mod value;
pub mod vtr_source;
mod vtr_transactions;

pub use history::SignalHistory;
pub use source::{Direction, Hierarchy, Scope, ScopeId, SignalRef, TraceInfo, VarId, Variable};
pub use source::{Generator, GeneratorId, Member, ScopeRole};
pub use translator::{Translated, Translator, Translators};
pub use value::{Bit, SignalShape, ValueKind, WaveValue};
