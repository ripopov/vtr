//! Hierarchy browser models: a container tree and a separate, filterable member
//! list. Frontends render these rows with their own list widgets.

pub mod icons;
pub mod members;
pub mod scopes;

pub use members::MemberListModel;
pub use scopes::ScopeTreeModel;

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
