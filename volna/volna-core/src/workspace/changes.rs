//! Compare only the persistent fields an input can touch. In particular pointer
//! motion never walks signal rows or serializes a workspace.
use crate::document::Marker;
use crate::panels::PanelId;
use crate::pipeline::RowView;
use crate::wave::{
    model::{DisplayedSignal, Link, PointerEvent},
    viewport::Viewport,
};
use crate::{Action, App, Command};
use std::collections::BTreeSet;

#[derive(PartialEq)]
pub(crate) struct Stamp {
    panel: PanelId,
    layout_revision: u64,
    shared_viewport: Viewport,
    shared_cursor: Option<u64>,
    markers: Vec<Marker>,
    sidebar: (bool, f32, f32),
    scope: Option<usize>,
    expanded: Option<BTreeSet<usize>>,
    unresolved_selected: Option<Vec<String>>,
    unresolved_expanded: Option<Vec<Vec<String>>>,
    filter: String,
    wave: Option<WaveStamp>,
    pipeline: Option<PipelineStamp>,
}
#[derive(PartialEq)]
struct PipelineStamp {
    follow: crate::pipeline::FollowActivity,
    link: Link,
    viewport: Option<Viewport>,
    cursor: Option<u64>,
    rows: RowView,
    label_width: f32,
}
#[derive(PartialEq)]
struct WaveStamp {
    link: Link,
    viewport: Option<Viewport>,
    cursor: Option<u64>,
    scroll: f32,
    columns: (f32, f32),
    rows: usize,
    selected: Option<BTreeSet<usize>>,
    formats: Option<Vec<String>>,
}
impl Stamp {
    pub(crate) fn capture(app: &App, command: &Command) -> Option<Self> {
        if !app.workspace.scheduler.enabled() || app.workspace.loading || !app.doc.is_loaded() {
            return None;
        }
        let pointer = match command {
            Command::Pointer(id, event) => Some((*id, event)),
            _ => None,
        };
        if let Some((id, event)) = pointer {
            match event {
                PointerEvent::Leave | PointerEvent::Up => return None,
                PointerEvent::Move { .. } if app.panels.get(id).is_none_or(|p| !p.dragging()) => {
                    return None;
                }
                _ => {}
            }
        }
        if matches!(
            command,
            Command::MenuDismiss(_)
                | Command::OpenSignalMenu(_)
                | Command::SelectVar { .. }
                | Command::ChromeDragStart(_)
                | Command::ChromeDragEnd
                | Command::RequestOpenDialog
                | Command::Open(_)
                | Command::CloseTrace
        ) {
            return None;
        }
        let panel = match command {
            Command::Pointer(id, _)
            | Command::PipelineActivity(id, _)
            | Command::MenuSelect(id, _)
            | Command::Panels(crate::panels::PanelsCommand::ToggleLink { panel: id, .. }) => *id,
            _ => app.panels.focused_id(),
        };
        let selection = pointer.is_none_or(|(_, event)| matches!(event, PointerEvent::Down { .. }));
        let formats = matches!(
            command,
            Command::MenuSelect(..) | Command::Action(Action::CycleFormat)
        );
        let scope = matches!(
            command,
            Command::ToggleScope(_) | Command::ExpandAllScopes(_) | Command::ScopesKey(_)
        );
        Some(Self {
            panel,
            layout_revision: app.panels.revision(),
            shared_viewport: app.doc.shared.viewport.target(),
            shared_cursor: app.doc.shared.cursor,
            markers: app.doc.markers.clone(),
            sidebar: (app.sidebar_visible, app.sidebar_width, app.scopes_fraction),
            scope: app.scopes.selected,
            expanded: scope.then(|| app.scopes.expanded().collect()),
            unresolved_selected: app.scopes.unresolved_selected.clone(),
            unresolved_expanded: scope.then(|| app.scopes.unresolved_expanded.clone()),
            filter: app.variables.filter.clone(),
            wave: app.panels.waves(panel).map(|w| WaveStamp {
                link: w.nav.link,
                viewport: (!w.nav.link.viewport).then(|| w.nav.local_viewport.target()),
                cursor: if w.nav.link.cursor {
                    None
                } else {
                    w.nav.local_cursor
                },
                scroll: w.scroll_y,
                columns: (w.names_width, w.values_width),
                rows: w.items.len(),
                selected: selection.then(|| w.selected.clone()),
                formats: formats.then(|| w.items.iter().map(DisplayedSignal::format_id).collect()),
            }),
            pipeline: app.panels.pipeline(panel).map(|p| PipelineStamp {
                follow: p.follow,
                link: p.nav.link,
                viewport: (!p.nav.link.viewport).then(|| p.nav.local_viewport.target()),
                cursor: if p.nav.link.cursor {
                    None
                } else {
                    p.nav.local_cursor
                },
                rows: p.rows.target(),
                label_width: p.label_width,
            }),
        })
    }
}
