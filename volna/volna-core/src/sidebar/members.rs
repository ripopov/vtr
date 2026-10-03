//! Container members and bounded whole-trace search, with shared selection,
//! presentation metadata and keyboard behavior.

use std::collections::BTreeSet;

use super::Key;
use crate::geometry::Modifiers;
use crate::icons::IconName;
use crate::selection;
use crate::trace::{TraceId, TraceSet, Traced};
use volna_trace::data::{Direction, Hierarchy, Member, ScopeId, ScopeRole, SignalShape, VarId};

const MAX_SEARCH_ROWS: usize = 5000;

/// A borrowed view of raw log attributes. Missing provenance stays missing.
pub struct LogSite<'a> {
    pub severity: String,
    pub file: Option<&'a str>,
    pub line: Option<u64>,
    pub function: Option<&'a str>,
}

pub fn log_site(h: &Hierarchy, member: Member) -> Option<LogSite<'_>> {
    use volna_trace::data::transactions::AttributeValue;
    let Member::Generator(id) = member else {
        return None;
    };
    if !h.is_log(member) {
        return None;
    }
    let attrs = &h.generators()[id].attributes;
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
            let v = h.var(id);
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
        Member::Stream(id) => format!("stream · {}", h.scope(id).kind),
    }
}

pub fn describe(h: &Hierarchy, member: Member) -> String {
    let detail = match member {
        Member::Var(id) => {
            let v = h.var(id);
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
    pub scope: Option<Traced<ScopeId>>,
    pub filter: String,
    pub rows: Vec<Traced<Member>>,
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
    pub activate: Option<Vec<Traced<Member>>>,
    /// Row to scroll into view.
    pub reveal: Option<usize>,
    /// Typing started filtering: the frontend should focus its filter box.
    pub focus_filter: bool,
}

/// Whether `name` lowercased contains `lower` (already lowercase). ASCII
/// names, the usual HDL case, are compared in place without allocating.
fn contains_folded(name: &str, lower: &str) -> bool {
    if lower.is_empty() {
        return true;
    }
    if name.is_ascii() && lower.is_ascii() {
        return name
            .as_bytes()
            .windows(lower.len())
            .any(|w| w.eq_ignore_ascii_case(lower.as_bytes()));
    }
    name.to_lowercase().contains(lower)
}

impl MemberListModel {
    /// Start over for a new set of traces.
    pub fn reset(&mut self, traces: &TraceSet) {
        self.scope = None;
        self.search_everywhere = false;
        self.filter.clear();
        self.rebuild(traces);
    }

    /// A trace left: stop listing its members.
    pub fn remove_trace(&mut self, traces: &TraceSet, trace: TraceId) {
        if self.scope.is_some_and(|s| s.trace == trace) {
            self.scope = None;
        }
        self.rebuild(traces);
    }

    pub fn rebuild(&mut self, traces: &TraceSet) {
        self.rows.clear();
        self.notice = None;
        self.selected.clear();
        self.anchor = None;
        self.truncated = false;
        let filter = self.filter.to_lowercase();
        let matches = |name: &str| contains_folded(name, &filter);
        match self.scope.filter(|_| !self.search_everywhere) {
            Some(scope) => {
                let Some(h) = traces.session(scope.trace).map(|s| s.hierarchy()) else {
                    return;
                };
                let s = h.scope(scope.item);
                self.rows.extend(
                    s.vars
                        .iter()
                        .filter(|&v| matches(h.var_name(v)))
                        .map(|v| scope.with(Member::Var(v))),
                );
                self.rows.extend(
                    s.generators
                        .iter()
                        .filter(|&g| matches(&h.generators()[g].name))
                        .map(|g| scope.with(Member::Generator(g))),
                );
            }
            // Searching looks through every open trace.
            None if !filter.is_empty() => {
                for (trace, session) in traces.loaded() {
                    let h = session.hierarchy();
                    let members = (0..h.var_count())
                        .map(Member::Var)
                        .chain((0..h.generators().len()).map(Member::Generator))
                        .chain(
                            h.scopes()
                                .enumerate()
                                .filter(|(_, s)| matches!(s.role, ScopeRole::Stream { .. }))
                                .map(|(id, _)| Member::Stream(id)),
                        );
                    let room = MAX_SEARCH_ROWS + 1 - self.rows.len();
                    self.rows.extend(
                        members
                            .filter(|&m| matches(h.member_name(m)))
                            .take(room)
                            .map(|m| Traced::new(trace, m)),
                    );
                    if self.rows.len() > MAX_SEARCH_ROWS {
                        break;
                    }
                }
                self.truncated = self.rows.len() > MAX_SEARCH_ROWS;
                self.rows.truncate(MAX_SEARCH_ROWS);
            }
            None => {}
        }
    }

    pub fn set_scope(&mut self, traces: &TraceSet, scope: Option<Traced<ScopeId>>) {
        self.scope = scope;
        self.search_everywhere = false;
        self.rebuild(traces);
    }

    pub fn set_filter(&mut self, traces: &TraceSet, text: &str) {
        if self.filter != text {
            self.filter = text.to_owned();
            self.rebuild(traces);
        }
    }

    fn is_searching(&self) -> bool {
        !self.filter.is_empty()
    }

    /// Rows show full paths while searching across the whole trace set.
    pub fn show_scope(&self) -> bool {
        (self.scope.is_none() || self.search_everywhere) && self.is_searching()
    }

    /// Whether any listed variable has a port direction worth a column.
    pub fn show_direction(&self, traces: &TraceSet) -> bool {
        self.rows.iter().any(|m| {
            m.item.var().is_some_and(|v| {
                traces
                    .session(m.trace)
                    .is_some_and(|s| s.hierarchy().var(v).direction != Direction::None)
            })
        })
    }

    /// The selected members, or every listed variable when nothing is
    /// selected.
    pub fn selected_or_all(&self) -> Vec<Traced<Member>> {
        if self.selected.is_empty() {
            self.rows
                .iter()
                .copied()
                .filter(|m| matches!(m.item, Member::Var(_)))
                .collect()
        } else {
            self.selected.iter().map(|&i| self.rows[i]).collect()
        }
    }

    pub fn listed_vars(&self) -> Vec<Traced<VarId>> {
        self.rows
            .iter()
            .filter_map(|m| Some(m.with(m.item.var()?)))
            .collect()
    }

    fn scope_of<'a>(&self, traces: &'a TraceSet) -> Option<(&'a Hierarchy, ScopeId)> {
        let scope = self.scope?;
        Some((traces.session(scope.trace)?.hierarchy(), scope.item))
    }

    pub fn title(&self, traces: &TraceSet) -> &'static str {
        if self.search_everywhere || self.show_scope() {
            return "Results";
        }
        match self.scope_of(traces).map(|(h, s)| h.scope(s)) {
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

    /// Where the listed members live: the scope's path, kind and component,
    /// after its trace's letter while several traces are open.
    pub fn breadcrumb(&self, traces: &TraceSet) -> String {
        if self.search_everywhere || self.show_scope() {
            let whole = if traces.is_combined() {
                "All traces"
            } else {
                "Whole trace"
            };
            return format!("{whole} · {}", self.filter);
        }
        let Some((h, id)) = self.scope_of(traces) else {
            return String::new();
        };
        let s = h.scope(id);
        let letter = match self.scope {
            Some(scope) if traces.is_combined() => format!("{} · ", scope.trace),
            _ => String::new(),
        };
        format!(
            "{letter}{} · {}{}",
            h.scope_path(id).join("."),
            s.kind,
            if s.component.is_empty() {
                String::new()
            } else {
                format!(" · {}", s.component)
            }
        )
    }

    pub fn select(&mut self, ix: usize, modifiers: Modifiers) {
        if ix < self.rows.len() {
            selection::select(&mut self.selected, &mut self.anchor, ix, modifiers);
        }
    }

    /// Placeholder text when there is nothing to list.
    pub fn placeholder(&self, traces: &TraceSet) -> Option<&'static str> {
        if traces.first_loaded().is_none() {
            Some("Open a trace to browse variables")
        } else if (self.scope.is_none() || self.search_everywhere) && !self.is_searching() {
            Some("Select a scope or stream, or type to search all members")
        } else if self.rows.is_empty() {
            if self.is_searching() {
                Some("No members match")
            } else if let Some((h, id)) = self.scope_of(traces) {
                let scope = h.scope(id);
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

#[cfg(test)]
mod tests {
    use super::contains_folded;

    #[test]
    fn folded_match_agrees_with_lowercasing() {
        for (name, filter) in [
            ("CPU_Core0", "core"),
            ("cpu", "cpux"),
            ("x", ""),
            ("Straße", "ße"),
            ("\u{212A}elvin", "kel"),
            ("ab", "abc"),
        ] {
            let lower = filter.to_lowercase();
            assert_eq!(
                contains_folded(name, &lower),
                name.to_lowercase().contains(&lower),
                "{name} / {filter}"
            );
        }
    }
}
