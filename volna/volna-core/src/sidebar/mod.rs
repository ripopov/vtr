//! Hierarchy browser models: a scope tree and a separate, filterable variable
//! list. Frontends render these rows with their own list widgets.

pub mod scopes;
pub mod variables;

pub use scopes::ScopeTreeModel;
pub use variables::VariableListModel;

/// A key the list models understand. Frontends translate their own key events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    Escape,
    /// A printable character (or several, for IME commits).
    Char(String),
}
