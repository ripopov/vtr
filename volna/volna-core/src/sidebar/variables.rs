//! The variable list: the variables of the selected scope (or a search over
//! every variable), filtered by a text box, with multi-selection.

use std::collections::BTreeSet;

use super::Key;
use crate::data::{Direction, Hierarchy, ScopeId, SignalShape, VarId};
use crate::geometry::Modifiers;
use crate::icons::IconName;
use crate::selection;

/// Metadata access for resident and incrementally queried variable lists.
pub trait VariableHierarchy {
    fn variables(&self, scope: Option<ScopeId>) -> impl Iterator<Item = VarId> + '_;
    fn root_variables(&self) -> impl Iterator<Item = VarId> + '_;
    fn variable_name(&self, id: VarId) -> Option<&str>;
    fn variable_direction(&self, id: VarId) -> Direction;
    fn variables_complete(&self, scope: Option<ScopeId>, global: bool) -> bool;
}
impl VariableHierarchy for Hierarchy {
    fn variables(&self, scope: Option<ScopeId>) -> impl Iterator<Item = VarId> + '_ {
        (0..if scope.is_none() { self.vars.len() } else { 0 }).chain(
            scope
                .and_then(|scope| self.scopes.get(scope))
                .into_iter()
                .flat_map(|scope| scope.vars.iter().copied()),
        )
    }
    fn root_variables(&self) -> impl Iterator<Item = VarId> + '_ {
        std::iter::empty()
    }
    fn variable_name(&self, id: VarId) -> Option<&str> {
        self.vars.get(id).map(|var| var.name.as_str())
    }
    fn variable_direction(&self, id: VarId) -> Direction {
        self.vars
            .get(id)
            .map_or(Direction::None, |var| var.direction)
    }
    fn variables_complete(&self, _: Option<ScopeId>, _: bool) -> bool {
        true
    }
}
impl VariableHierarchy for crate::data::query_hierarchy::QueryHierarchy {
    fn variables(&self, scope: Option<ScopeId>) -> impl Iterator<Item = VarId> + '_ {
        self.declarations()
            .filter(move |node| {
                scope.is_none_or(|scope| node.parent.map(|id| id as ScopeId) == Some(scope))
            })
            .filter_map(variable_id)
    }
    fn root_variables(&self) -> impl Iterator<Item = VarId> + '_ {
        self.children(None).filter_map(variable_id)
    }
    fn variable_name(&self, id: VarId) -> Option<&str> {
        self.declaration(u32::try_from(id).ok()?)
            .and_then(|node| self.text(&node.name))
    }
    fn variable_direction(&self, id: VarId) -> Direction {
        let Some(node) = u32::try_from(id).ok().and_then(|id| self.declaration(id)) else {
            return Direction::None;
        };
        let vtr_query::metadata::DeclarationData::Variable { direction, .. } = node.data else {
            return Direction::None;
        };
        match vtr::Direction::from_u8(direction) {
            vtr::Direction::Input => Direction::Input,
            vtr::Direction::Output => Direction::Output,
            vtr::Direction::InOut => Direction::InOut,
            _ => Direction::None,
        }
    }
    fn variables_complete(&self, scope: Option<ScopeId>, global: bool) -> bool {
        let coverage = if global {
            self.is_fully_loaded()
        } else {
            match scope {
                Some(scope) => u32::try_from(scope).ok().is_some_and(|scope| {
                    self.declaration(scope)
                        .is_some_and(|node| node.children == 0)
                        || self.state(Some(scope)).is_some_and(|state| state.complete)
                }),
                None => self.state(None).is_some_and(|state| state.complete),
            }
        };
        coverage
            && if scope.is_none() && !global {
                self.root_variables()
                    .all(|id| self.variable_name(id).is_some())
            } else {
                self.variables(scope)
                    .all(|id| self.variable_name(id).is_some())
            }
    }
}
fn variable_id(node: &vtr_query::metadata::Declaration) -> Option<VarId> {
    matches!(
        node.data,
        vtr_query::metadata::DeclarationData::Variable { .. }
    )
    .then_some(node.id as VarId)
}

const MAX_SEARCH_ROWS: usize = 5000;

#[derive(Default)]
pub struct VariableListModel {
    pub scope: Option<ScopeId>,
    pub filter: String,
    pub rows: Vec<VarId>,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
    /// False while pages/names are missing or the global search row cap applies.
    pub complete: bool,
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Start over for a new hierarchy.
    pub fn reset(&mut self, h: Option<&impl VariableHierarchy>) {
        self.scope = None;
        self.filter.clear();
        self.rebuild(h);
    }

    pub fn rebuild(&mut self, h: Option<&impl VariableHierarchy>) {
        self.rows.clear();
        self.selected.clear();
        self.anchor = None;
        self.complete = false;
        let filter = self.filter.to_lowercase();
        if let Some(h) = h {
            let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
            self.complete =
                h.variables_complete(self.scope, self.scope.is_none() && !filter.is_empty());
            match self.scope {
                Some(s) => {
                    self.rows.extend(h.variables(Some(s)).filter(|&id| {
                        filter.is_empty() || h.variable_name(id).is_some_and(matches)
                    }))
                }
                None if !filter.is_empty() => {
                    self.rows.extend(
                        h.variables(None)
                            .filter(|&id| h.variable_name(id).is_some_and(matches))
                            .take(MAX_SEARCH_ROWS + 1),
                    );
                    if self.rows.len() > MAX_SEARCH_ROWS {
                        self.rows.truncate(MAX_SEARCH_ROWS);
                        self.complete = false;
                    }
                }
                None => self.rows.extend(h.root_variables()),
            }
        }
    }

    /// Incoming pages can change row positions; preserve selection by raw/local
    /// variable identity rather than keeping indices into the old row list.
    pub fn refresh(&mut self, h: Option<&impl VariableHierarchy>) {
        let selected: BTreeSet<_> = self
            .selected
            .iter()
            .filter_map(|&row| self.rows.get(row).copied())
            .collect();
        let anchor = self.anchor.and_then(|row| self.rows.get(row)).copied();
        self.rebuild(h);
        for (row, id) in self.rows.iter().enumerate() {
            if selected.contains(id) {
                self.selected.insert(row);
            }
            if anchor == Some(*id) {
                self.anchor = Some(row);
            }
        }
    }

    pub fn set_scope(&mut self, h: Option<&impl VariableHierarchy>, scope: Option<ScopeId>) {
        self.scope = scope;
        self.rebuild(h);
    }

    pub fn set_filter(&mut self, h: Option<&impl VariableHierarchy>, text: &str) {
        if self.filter != text {
            self.filter = text.to_owned();
            self.rebuild(h);
        }
    }

    pub fn is_searching(&self) -> bool {
        !self.filter.is_empty()
    }

    /// Rows show full paths while searching across the whole trace.
    pub fn show_scope(&self) -> bool {
        self.scope.is_none() && self.is_searching()
    }

    /// Whether any listed variable has a port direction worth a column.
    pub fn show_direction(&self, h: &impl VariableHierarchy) -> bool {
        self.rows
            .iter()
            .any(|&v| h.variable_direction(v) != Direction::None)
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
