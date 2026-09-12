//! Toolkit-independent model layer: signal values, change histories, value
//! translators and trace sources. Nothing in this module depends on GPUI, so
//! it can be unit-tested and reused by other front ends.

pub mod history;
pub mod source;
pub mod synth;
pub mod translator;
pub mod value;
pub mod vtr_source;

pub use history::SignalHistory;
pub use source::{
    Direction, Hierarchy, Scope, ScopeId, SignalRef, TraceInfo, VarId, Variable, WaveSource,
};
pub use translator::{Translated, Translator, Translators};
pub use value::{Bit, SignalShape, ValueKind, WaveValue};
