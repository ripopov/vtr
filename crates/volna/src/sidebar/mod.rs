//! Hierarchy browser: a scope tree and a separate, filterable variable list.

pub mod scopes;
pub mod variables;

pub use scopes::{ScopeTree, ScopeTreeEvent};
pub use variables::{VariableList, VariableListEvent};
