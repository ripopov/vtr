//! Semantic glyphs and tints. No toolkit or backend handles are involved.
use crate::data::{Direction, Hierarchy, Member, Scope, ScopeRole};
use crate::icons::IconName;
use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    None,
    Pipeline,
    Log,
    Stream,
    In,
    Out,
    InOut,
}

impl Tint {
    pub fn color<C: Copy>(self, theme: &Theme<C>) -> C {
        match self {
            Self::None => theme.panel.icon_muted,
            Self::Pipeline | Self::In => theme.sidebar_tints[0],
            Self::Log | Self::Out => theme.sidebar_tints[1],
            Self::Stream | Self::InOut => theme.sidebar_tints[2],
        }
    }
}

pub fn stream_icon(kind: &str) -> (IconName, Tint) {
    match kind {
        "PIPELINE" => (IconName::Workflow, Tint::Pipeline),
        "LOG" => (IconName::ScrollText, Tint::Log),
        _ => (IconName::ChartNoAxesGantt, Tint::Stream),
    }
}

pub fn scope_icon(scope: &Scope) -> (IconName, Tint) {
    if matches!(scope.role, ScopeRole::Stream { .. }) {
        return stream_icon(&scope.kind);
    }
    (scope_kind_icon(&scope.kind), Tint::None)
}

pub fn scope_kind_icon(kind: &str) -> IconName {
    use IconName::*;
    match kind {
        "module" | "vhdl_architecture" => Box,
        "sc_module" => Boxes,
        "core" => Cpu,
        "struct" | "union" | "vhdl_record" => Braces,
        "class" => Component,
        "interface" => Cable,
        "package" | "vhdl_package" => Package,
        "task" | "function" | "vhdl_procedure" | "vhdl_function" => SquareFunction,
        "generate" | "vhdl_generate" | "vhdl_for_generate" | "vhdl_if_generate" => GitFork,
        "begin" | "fork" | "vhdl_block" => Brackets,
        "vhdl_process" => Zap,
        "sv_array" => Layers,
        "program" => Terminal,
        "resource" => Server,
        "instrumentation_scope" => Library,
        _ => Folder,
    }
}

pub fn member_icon(h: &Hierarchy, member: Member) -> IconName {
    match member {
        Member::Var(id) => {
            let v = &h.vars[id];
            match v.var_type.as_str() {
                "parameter" => IconName::Pi,
                _ if v.enum_table.is_some() => IconName::Tags,
                "integer" => IconName::Hash,
                _ => super::members::shape_icon(v.shape),
            }
        }
        Member::Generator(_) if h.is_log(member) => IconName::MessageSquare,
        Member::Generator(_) => IconName::CircleDot,
        Member::Stream(id) => scope_icon(&h.scopes[id]).0,
    }
}

pub fn direction_icon(direction: Direction) -> Option<(IconName, Tint)> {
    match direction {
        Direction::Input => Some((IconName::LogIn, Tint::In)),
        Direction::Output => Some((IconName::LogOut, Tint::Out)),
        Direction::InOut => Some((IconName::ArrowLeftRight, Tint::InOut)),
        Direction::None => None,
    }
}

pub fn stream_tag(scope: &Scope) -> Option<&str> {
    (matches!(scope.role, ScopeRole::Stream { .. })
        && !matches!(scope.kind.as_str(), "PIPELINE" | "LOG"))
    .then_some(scope.kind.as_str())
}
