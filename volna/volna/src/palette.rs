//! The ⌘K command palette: every registered action with its key hint, and
//! the settings the core matcher ranks for the query. Choosing a boolean
//! setting toggles it in place; any other setting opens the Settings tab
//! filtered to it.

use gpui_kit::component::{
    WindowExt,
    command::{Command as Palette, CommandGroup, CommandItem, CommandState},
};
use gpui_kit::prelude::*;
use gpui_kit::{Action, Context, Focusable, WeakEntity, Window};
use volna_core::app::SettingsCommand;
use volna_core::settings::{self, Kind, Value};
use volna_core::{App as CoreApp, Command};

use crate::app::{self, Workspace};
use crate::settings_panel::ToggleSettingsJson;

/// The palette can only read this entity while the dialog renders, never
/// the workspace (which is mid-render then), so everything it shows is here.
pub(crate) struct PaletteModel {
    commands: Vec<usize>,
    settings: Vec<(&'static settings::Spec, bool)>,
}

/// Every palette command: label and the action it dispatches.
fn commands() -> Vec<(&'static str, Box<dyn Action>)> {
    vec![
        ("Open Trace…", Box::new(app::OpenFile)),
        ("Close Trace", Box::new(app::CloseTrace)),
        ("Open Workspace…", Box::new(app::OpenWorkspace)),
        ("Save Workspace", Box::new(app::SaveWorkspace)),
        ("Save Workspace As…", Box::new(app::SaveWorkspaceAs)),
        ("Preferences: Open Settings", Box::new(app::OpenSettings)),
        (
            "Preferences: Open Settings (JSON)",
            Box::new(ToggleSettingsJson),
        ),
        ("Interface: Zoom In", Box::new(app::UiZoomIn)),
        ("Interface: Zoom Out", Box::new(app::UiZoomOut)),
        ("Interface: Reset Zoom", Box::new(app::UiZoomReset)),
        ("Toggle Sidebar", Box::new(app::ToggleSidebar)),
        ("Split Right", Box::new(app::SplitRight)),
        ("Split Down", Box::new(app::SplitDown)),
        ("New Waveform Tab", Box::new(app::NewPanel)),
        ("Close Panel", Box::new(app::ClosePanel)),
        ("Focus Next Panel", Box::new(app::FocusNextPanel)),
        ("Focus Previous Panel", Box::new(app::FocusPrevPanel)),
        ("Follow Shared Viewport", Box::new(app::ToggleViewportLink)),
        ("Follow Shared Cursor", Box::new(app::ToggleCursorLink)),
        ("Zoom In", Box::new(app::ZoomIn)),
        ("Zoom Out", Box::new(app::ZoomOut)),
        ("Zoom to Fit", Box::new(app::ZoomFit)),
        ("Zoom to Cursor", Box::new(app::ZoomToCursor)),
        ("Go to Start", Box::new(app::GoToStart)),
        ("Go to End", Box::new(app::GoToEnd)),
        ("Add Marker at Cursor", Box::new(app::AddMarker)),
        ("Clear Markers", Box::new(app::ClearMarkers)),
    ]
}

const MAX_SETTINGS: usize = 8;

impl PaletteModel {
    fn compute(query: &str, app: &CoreApp) -> Self {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let commands = commands()
            .iter()
            .enumerate()
            .filter(|(_, (label, _))| {
                let lower = label.to_lowercase();
                words.iter().all(|w| lower.contains(w))
            })
            .map(|(ix, _)| ix)
            .collect();
        let store = &app.settings;
        let settings = if query.trim().is_empty() {
            Vec::new()
        } else {
            settings::search(query, store.host(), &|id| store.is_modified(id))
                .into_iter()
                .take(MAX_SETTINGS)
                .map(|hit| {
                    let on = matches!(hit.spec.kind, Kind::Bool)
                        && store
                            .value(hit.spec.id)
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    (hit.spec, on)
                })
                .collect()
        };
        Self { commands, settings }
    }
}

impl Workspace {
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        let model = cx.new(|_| PaletteModel::compute("", &self.app));
        let ws: WeakEntity<Workspace> = cx.weak_entity();
        let query_ws = ws.clone();
        let query_model = model.clone();
        let confirm_ws = ws.clone();
        let confirm_model = model.clone();
        let focus_state = state.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = state.clone();
            let model = model.clone();
            let query_ws = query_ws.clone();
            let query_model = query_model.clone();
            let confirm_ws = confirm_ws.clone();
            let confirm_model = confirm_model.clone();
            dialog
                .title("Command palette")
                .overlay_closable(true)
                .content(move |content, _, cx| {
                    let read = model.read(cx);
                    let all = commands();
                    let mut palette = Palette::new(&state)
                        .filterable(false)
                        .bordered(false)
                        .placeholder("Type a command or search settings…")
                        .on_query({
                            let ws = query_ws.clone();
                            let model = query_model.clone();
                            move |query, _, cx| {
                                let Some(ws) = ws.upgrade() else { return };
                                let next = PaletteModel::compute(query, &ws.read(cx).app);
                                model.update(cx, |model, cx| {
                                    *model = next;
                                    cx.notify();
                                });
                            }
                        })
                        .on_confirm({
                            let ws = confirm_ws.clone();
                            let model = confirm_model.clone();
                            move |path, window, cx| {
                                let chosen = {
                                    let read = model.read(cx);
                                    if path.section == 1 {
                                        read.settings.get(path.row).copied()
                                    } else {
                                        None
                                    }
                                };
                                window.close_dialog(cx);
                                let Some((spec, on)) = chosen else { return };
                                _ = ws.update(cx, |ws, cx| {
                                    let command = if matches!(spec.kind, Kind::Bool) {
                                        SettingsCommand::Set {
                                            id: spec.id.into(),
                                            value: Value::Bool(!on),
                                        }
                                    } else {
                                        SettingsCommand::Reveal { id: spec.id.into() }
                                    };
                                    ws.dispatch(Command::Settings(command), Some(window), cx);
                                });
                            }
                        })
                        .on_cancel(|window, cx| window.close_dialog(cx));
                    let mut group = CommandGroup::new().label("Commands");
                    for ix in &read.commands {
                        let (label, action) = &all[*ix];
                        group = group.item(
                            CommandItem::new()
                                .label(*label)
                                .action(action.boxed_clone()),
                        );
                    }
                    palette = palette.group(group);
                    let mut group = CommandGroup::new().label("Settings");
                    for (spec, on) in &read.settings {
                        group = group.item(
                            CommandItem::new()
                                .label(format!("{}: {}", spec.page.title(), spec.title))
                                .keywords(spec.keywords.iter().copied())
                                .checked(matches!(spec.kind, Kind::Bool) && *on),
                        );
                    }
                    palette = palette.group(group);
                    content.child(palette)
                })
        });
        let focus = focus_state.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }
}
