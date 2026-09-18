//! Container members and bounded whole-trace search, with shared selection,
//! presentation metadata and keyboard behavior.

use std::collections::BTreeSet;

use super::Key;
use crate::data::{Direction, Hierarchy, Member, ScopeId, ScopeRole, SignalShape, VarId};
use crate::geometry::Modifiers;
use crate::icons::IconName;
use crate::selection;

const MAX_SEARCH_ROWS: usize = 5000;

/// A borrowed view of raw log attributes. Missing provenance stays missing.
pub struct LogSite<'a> {
    pub severity: String,
    pub file: Option<&'a str>,
    pub line: Option<u64>,
    pub function: Option<&'a str>,
}

pub fn log_site(h: &Hierarchy, member: Member) -> Option<LogSite<'_>> {
    use crate::data::transactions::AttributeValue;
    let Member::Generator(id) = member else {
        return None;
    };
    if !h.is_log(member) {
        return None;
    }
    let attrs = &h.generators[id].attributes;
    let attr = |key| attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    let text = |key| match attr(key) {
        Some(AttributeValue::Text(s)) => Some(s.as_str()),
        _ => None,
    };
    let severity = match attr("log.severity") {
        Some(AttributeValue::U64(n)) => match u8::try_from(*n) {
            Ok(n @ 0..=5) => vtr::logblock::Severity::from_code(n).name().to_owned(),
            _ => n.to_string(),
        },
        Some(AttributeValue::Text(s)) => s.clone(),
        _ => "unknown".into(),
    };
    Some(LogSite {
        severity,
        file: text("log.file"),
        line: match attr("log.line") {
            Some(AttributeValue::U64(n)) => Some(*n),
            _ => None,
        },
        function: text("log.func"),
    })
}

pub fn member_detail(h: &Hierarchy, member: Member) -> String {
    match member {
        Member::Var(id) => {
            let v = &h.vars[id];
            format!(
                "{}{}",
                if v.var_type == "parameter" {
                    "param "
                } else {
                    ""
                },
                v.shape.dims()
            )
        }
        Member::Generator(_) => {
            if let Some(site) = log_site(h, member) {
                match (site.file, site.line) {
                    (Some(file), Some(line)) => format!("{file}:{line}"),
                    (Some(file), None) => file.into(),
                    _ => String::new(),
                }
            } else {
                "generator".into()
            }
        }
        Member::Stream(id) => format!("stream · {}", h.scopes[id].kind),
    }
}

pub fn describe(h: &Hierarchy, member: Member) -> String {
    let detail = match member {
        Member::Var(id) => {
            let v = &h.vars[id];
            format!(
                "{} {} · {}{}",
                v.var_type,
                v.shape.dims(),
                match v.direction {
                    Direction::Input => "input",
                    Direction::Output => "output",
                    Direction::InOut => "inout",
                    Direction::None => "internal",
                },
                if v.enum_table.is_some() {
                    " · enum"
                } else {
                    ""
                }
            )
        }
        Member::Generator(_) => {
            if let Some(site) = log_site(h, member) {
                format!(
                    "log site · {} · {} {}",
                    site.severity,
                    member_detail(h, member),
                    site.function.unwrap_or("")
                )
            } else {
                "generator".into()
            }
        }
        Member::Stream(_) => member_detail(h, member),
    };
    format!("{} — {}", h.member_path(member), detail)
}

#[derive(Default)]
pub struct MemberListModel {
    pub scope: Option<ScopeId>,
    pub filter: String,
    pub rows: Vec<Member>,
    pub search_everywhere: bool,
    pub truncated: bool,
    pub notice: Option<String>,
    pub selected: BTreeSet<usize>,
    pub anchor: Option<usize>,
}

/// What a key press asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberKeyOutcome {
    pub changed: bool,
    /// Members to activate through the application's shared command path.
    pub activate: Option<Vec<Member>>,
    /// Row to scroll into view.
    pub reveal: Option<usize>,
    /// Typing started filtering: the frontend should focus its filter box.
    pub focus_filter: bool,
}

impl MemberListModel {
    /// Start over for a new hierarchy.
    pub fn reset(&mut self, h: Option<&Hierarchy>) {
        self.scope = None;
        self.search_everywhere = false;
        self.filter.clear();
        self.rebuild(h);
    }

    pub fn rebuild(&mut self, h: Option<&Hierarchy>) {
        self.rows.clear();
        self.notice = None;
        self.selected.clear();
        self.anchor = None;
        self.truncated = false;
        let filter = self.filter.to_lowercase();
        if let Some(h) = h {
            let matches = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
            match self.scope.filter(|_| !self.search_everywhere) {
                Some(s) => {
                    self.rows.extend(
                        h.scopes[s]
                            .vars
                            .iter()
                            .copied()
                            .filter(|&v| matches(&h.vars[v].name))
                            .map(Member::Var),
                    );
                    self.rows.extend(
                        h.scopes[s]
                            .generators
                            .iter()
                            .copied()
                            .filter(|&g| matches(&h.generators[g].name))
                            .map(Member::Generator),
                    );
                }
                None if !filter.is_empty() => {
                    let members = (0..h.vars.len())
                        .map(Member::Var)
                        .chain((0..h.generators.len()).map(Member::Generator))
                        .chain(
                            h.scopes
                                .iter()
                                .enumerate()
                                .filter(|(_, s)| matches!(s.role, ScopeRole::Stream { .. }))
                                .map(|(id, _)| Member::Stream(id)),
                        );
                    self.rows.extend(
                        members
                            .filter(|&m| matches(h.member_name(m)))
                            .take(MAX_SEARCH_ROWS + 1),
                    );
                    self.truncated = self.rows.len() > MAX_SEARCH_ROWS;
                    self.rows.truncate(MAX_SEARCH_ROWS);
                }
                None => {}
            }
        }
    }

    pub fn set_scope(&mut self, h: Option<&Hierarchy>, scope: Option<ScopeId>) {
        self.scope = scope;
        self.search_everywhere = false;
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
        (self.scope.is_none() || self.search_everywhere) && self.is_searching()
    }

    /// Whether any listed variable has a port direction worth a column.
    pub fn show_direction(&self, h: &Hierarchy) -> bool {
        self.rows
            .iter()
            .filter_map(|m| m.var())
            .any(|v| h.vars[v].direction != Direction::None)
    }

    /// The selected variables, or every listed one when nothing is selected.
    pub fn selected_or_all(&self) -> Vec<Member> {
        if self.selected.is_empty() {
            self.rows
                .iter()
                .copied()
                .filter(|m| matches!(m, Member::Var(_)))
                .collect()
        } else {
            self.selected.iter().map(|&i| self.rows[i]).collect()
        }
    }

    pub fn listed_vars(&self) -> Vec<VarId> {
        self.rows.iter().filter_map(|m| m.var()).collect()
    }

    pub fn title(&self, h: Option<&Hierarchy>) -> &'static str {
        if self.search_everywhere || self.show_scope() {
            return "Results";
        }
        match h.and_then(|h| self.scope.map(|s| &h.scopes[s])) {
            Some(s) if matches!(s.role, ScopeRole::Stream { .. }) => {
                if s.kind == "LOG" {
                    "Log sites"
                } else {
                    "Generators"
                }
            }
            Some(_) => "Variables",
            None => "Members",
        }
    }

    pub fn breadcrumb(&self, h: &Hierarchy) -> String {
        if self.search_everywhere || self.show_scope() {
            return format!("Whole trace · {}", self.filter);
        }
        self.scope
            .map(|id| {
                let s = &h.scopes[id];
                format!(
                    "{} · {}{}",
                    h.scope_path(id).join("."),
                    s.kind,
                    if s.component.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", s.component)
                    }
                )
            })
            .unwrap_or_default()
    }

    pub fn select(&mut self, ix: usize, modifiers: Modifiers) {
        if ix < self.rows.len() {
            selection::select(&mut self.selected, &mut self.anchor, ix, modifiers);
        }
    }

    /// Placeholder text when there is nothing to list.
    pub fn placeholder(&self, h: Option<&Hierarchy>) -> Option<&'static str> {
        if h.is_none() {
            Some("Open a trace to browse variables")
        } else if (self.scope.is_none() || self.search_everywhere) && !self.is_searching() {
            Some("Select a scope or stream, or type to search all members")
        } else if self.rows.is_empty() {
            if self.is_searching() {
                Some("No members match")
            } else if let Some(scope) = self.scope.map(|id| &h.unwrap().scopes[id]) {
                Some(if matches!(scope.role, ScopeRole::Stream { .. }) {
                    "This stream declares no generators"
                } else if scope.children.is_empty() {
                    "No variables in this scope"
                } else {
                    "No variables here; expand the scope for its children"
                })
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn key(&mut self, key: &Key, modifiers: Modifiers) -> MemberKeyOutcome {
        let mut out = MemberKeyOutcome::default();
        match key {
            Key::Enter => {
                let vars = self.selected_or_all();
                if !vars.is_empty() {
                    out.activate = Some(vars);
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
        SignalShape::Text => IconName::Type,
        SignalShape::Event => IconName::Zap,
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
