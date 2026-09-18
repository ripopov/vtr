//! The variable list: the variables of the selected scope (or a search over
//! every variable), filtered by a text box, with multi-selection.

use std::collections::BTreeSet;

use super::Key;
use crate::data::{Direction, Hierarchy, ScopeId, SignalShape, VarId};
use crate::geometry::Modifiers;
use crate::icons::IconName;
use crate::selection;

const MAX_SEARCH_ROWS: usize = 5000;

#[derive(Default)]
pub struct VariableListModel {
    pub scope: Option<ScopeId>,
    pub filter: String,
    pub rows: Vec<VarId>,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
}

/// What a key press asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VarKeyOutcome {
    pub changed: bool,
    /// Variables to add to the wave view.
    pub add: Option<Vec<VarId>>,
    /// Row to scroll into view.
    pub reveal: Option<usize>,
    /// Typing started filtering: the frontend should focus its filter box.
    pub focus_filter: bool,
}

impl VariableListModel {
    /// Start over for a new hierarchy.
    pub fn reset(&mut self, h: Option<&Hierarchy>) {
        self.scope = None;
        self.filter.clear();
        self.rebuild(h);
    }

    pub fn rebuild(&mut self, h: Option<&Hierarchy>) {
        self.rows.clear();
        self.selected.clear();
        self.anchor = None;
        let filter = self.filter.to_lowercase();
        if let Some(h) = h {
            let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
            match self.scope {
                Some(s) => {
                    self.rows.extend(
                        h.scopes[s]
                            .vars
                            .iter()
                            .copied()
                            .filter(|&v| matches(&h.vars[v].name)),
                    );
                }
                None if !filter.is_empty() => {
                    self.rows.extend(
                        (0..h.vars.len())
                            .filter(|&v| matches(&h.vars[v].name))
                            .take(MAX_SEARCH_ROWS),
                    );
                }
                None => {}
            }
        }
    }

    pub fn set_scope(&mut self, h: Option<&Hierarchy>, scope: Option<ScopeId>) {
        self.scope = scope;
        self.rebuild(h);
    }

    pub fn set_filter(&mut self, h: Option<&Hierarchy>, text: &str) {
        if self.filter != text {
            self.filter = text.to_owned();
            self.rebuild(h);
        }
    }

    fn is_searching(&self) -> bool {
        !self.filter.is_empty()
    }

    /// Rows show full paths while searching across the whole trace.
    pub fn show_scope(&self) -> bool {
        self.scope.is_none() && self.is_searching()
    }

    /// Whether any listed variable has a port direction worth a column.
    pub fn show_direction(&self, h: &Hierarchy) -> bool {
        self.rows
            .iter()
            .any(|&v| h.vars[v].direction != Direction::None)
    }

    /// The selected variables, or every listed one when nothing is selected.
    pub fn selected_or_all(&self) -> Vec<VarId> {
        if self.selected.is_empty() {
            self.rows.clone()
        } else {
            self.selected.iter().map(|&i| self.rows[i]).collect()
        }
    }

    pub fn select(&mut self, ix: usize, modifiers: Modifiers) {
        if ix < self.rows.len() {
            selection::select(&mut self.selected, &mut self.anchor, ix, modifiers);
        }
    }

    /// Placeholder text when there is nothing to list.
    pub fn placeholder(&self, loaded: bool) -> Option<&'static str> {
        if !loaded {
            Some("Open a trace to browse variables")
        } else if self.scope.is_none() && !self.is_searching() {
            Some("Select a scope, or type to search all variables")
        } else if self.rows.is_empty() {
            Some("No variables match")
        } else {
            None
        }
    }

    pub fn key(&mut self, key: &Key, modifiers: Modifiers) -> VarKeyOutcome {
        let mut out = VarKeyOutcome::default();
        match key {
            Key::Enter => {
                let vars = self.selected_or_all();
                if !vars.is_empty() {
                    out.add = Some(vars);
                }
            }
            Key::Down | Key::Up => {
                if self.rows.is_empty() {
                    return out;
                }
                let down = *key == Key::Down;
                let cur = self.anchor.unwrap_or(if down { usize::MAX } else { 0 });
                let next = if down {
                    cur.wrapping_add(1).min(self.rows.len() - 1)
                } else {
                    cur.saturating_sub(1)
                };
                let next = if cur == usize::MAX { 0 } else { next };
                if modifiers.shift {
                    self.selected.insert(next);
                } else {
                    self.selected.clear();
                    self.selected.insert(next);
                }
                self.anchor = Some(next);
                out.reveal = Some(next);
                out.changed = true;
            }
            Key::Escape => {
                out.changed = !self.selected.is_empty();
                self.selected.clear();
            }
            // Typing starts filtering: the frontend appends to its own text box
            // and reports the new filter back.
            Key::Char(text) if !text.is_empty() && !modifiers.platform && !modifiers.control => {
                out.focus_filter = true;
                out.changed = true;
            }
            _ => {}
        }
        out
    }
}

pub fn shape_icon(shape: SignalShape) -> IconName {
    match shape {
        SignalShape::Bit => IconName::Activity,
        SignalShape::Vector { .. } => IconName::Binary,
        SignalShape::Real => IconName::Sigma,
        SignalShape::Text | SignalShape::Event => IconName::Type,
    }
}

pub fn direction_label(d: Direction) -> &'static str {
    match d {
        Direction::None => "",
        Direction::Input => "in",
        Direction::Output => "out",
        Direction::InOut => "io",
    }
}
