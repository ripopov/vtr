//! The ⌘K command palette: every registered action with its key hint, and
//! the settings the core matcher ranks for the query. Choosing a boolean
//! setting toggles it in place; any other setting opens the Settings tab
//! filtered to it. A query starting with `@` (or `'` in a timed panel)
//! switches to the marker navigator: the core's
//! [`volna_core::marker::navigator_rows`], where `↵` goes to a marker, `⇧↵`
//! measures from it, `F2` renames it and `Del` removes it.

use gpui_kit::component::{
    WindowExt,
    command::{Command as Palette, CommandGroup, CommandItem, CommandState},
};
use gpui_kit::prelude::*;
use gpui_kit::{Action, Context, Focusable, SharedString, WeakEntity, Window, div, px};
use volna_core::app::{ClockCommand, SettingsCommand};
use volna_core::marker::{LaneVerb, NavigatorRow};
use volna_core::settings::{self, Kind, Value};
use volna_core::{App as CoreApp, Command};

use crate::app::{self, Workspace};
use crate::settings_panel::ToggleSettingsJson;
use crate::theme::ThemePx;

/// The palette can only read this entity while the dialog renders, never
/// the workspace (which is mid-render then), so everything it shows is here.
pub(crate) struct PaletteModel {
    commands: Vec<(String, Box<dyn Action>)>,
    settings: Vec<(&'static settings::Spec, bool)>,
    /// The marker navigator's rows while the query starts with `@`.
    markers: Option<Vec<NavigatorRow>>,
    /// Outside `@` mode, whether *Find Marker…* is offered, and the first
    /// markers when a query word starts *markers* ([`PREVIEW`] at most).
    find_marker: bool,
    preview: Vec<NavigatorRow>,
}

impl PaletteModel {
    /// The Markers group comes first while it previews markers, so a query
    /// such as `mar` shows them above the many marker commands; otherwise
    /// the commands lead, and `remove all markers` runs with `↵`.
    fn markers_section(&self) -> usize {
        if self.preview.is_empty() { 1 } else { 0 }
    }
}

/// Markers an ordinary query shows before `@` lists them all.
const PREVIEW: usize = 8;
/// What *Find Marker…* is found by.
const FIND_MARKER: &str = "Find Marker… (@)";

gpui_kit::actions!(
    palette,
    [
        /// `⇧↵` in the marker navigator: measure from the marker.
        MeasureFromMarker,
        /// `F2` in the marker navigator: rename the marker.
        RenameMarker,
        /// `Del` in the marker navigator: remove the marker. Elsewhere the
        /// key falls through to the query field.
        RemoveMarker,
    ]
);

/// Keys of the marker navigator. `Del` is bound where the query field binds
/// it, and later, so it comes first; outside marker mode its handler
/// propagates and the field deletes a character.
pub(crate) fn key_bindings() -> Vec<gpui_kit::KeyBinding> {
    use gpui_kit::KeyBinding;
    vec![
        KeyBinding::new("shift-enter", MeasureFromMarker, Some("Command")),
        KeyBinding::new("f2", RenameMarker, Some("Command")),
        KeyBinding::new("delete", RemoveMarker, Some("Input")),
    ]
}

/// Every palette command: label and the action it dispatches. The open
/// trace's PIPELINE streams, the focused panel's clock choices and the
/// recent traces and workspaces follow the fixed commands; a number in the
/// query offers to go to that cycle.
pub(crate) fn commands(app: &CoreApp, query: &str) -> Vec<(String, Box<dyn Action>)> {
    let mut all: Vec<(String, Box<dyn Action>)> = vec![
        (
            app.undo_label()
                .map_or_else(|| "Undo".into(), |label| format!("Undo: {label}")),
            Box::new(app::Undo),
        ),
        (
            app.redo_label()
                .map_or_else(|| "Redo".into(), |label| format!("Redo: {label}")),
            Box::new(app::Redo),
        ),
    ];
    all.extend(
        fixed_commands()
            .into_iter()
            .map(|(label, action)| (label.to_owned(), action)),
    );
    if let Some(choices) = app.clock_choices() {
        let clock = |command| Box::new(app::ClockAction { command }) as Box<dyn Action>;
        if let Some(cycle) = query.split_whitespace().find_map(|w| w.parse::<i64>().ok()) {
            all.push((
                format!("Go to Cycle {cycle}"),
                clock(ClockCommand::GoToCycle(cycle)),
            ));
        }
        for (path, ruler) in &choices.clocks {
            let verb = if *ruler { "Hide" } else { "Show" };
            all.push((
                format!("{verb} Clock Ruler: {path}"),
                clock(ClockCommand::ToggleRuler(path.clone())),
            ));
            all.push((
                format!("Snap and Step to Clock: {path}"),
                clock(ClockCommand::Select(path.clone())),
            ));
        }
        all.push((
            if choices.origin {
                "Reset Cycle Origin".into()
            } else {
                "Set Cycle Origin at Cursor".into()
            },
            Box::new(app::ToggleCycleOrigin),
        ));
        all.push(("Next Cycle".into(), Box::new(app::NextCycle)));
        all.push(("Previous Cycle".into(), Box::new(app::PrevCycle)));
    }
    for (ix, label) in app::recent_labels(app).into_iter().flatten().enumerate() {
        all.push((
            format!("Open Recent: {label}"),
            Box::new(app::OpenRecent { ix }),
        ));
    }
    if app::recent_labels(app).is_some_and(|labels| !labels.is_empty()) {
        all.push(("Clear Recent".into(), Box::new(app::ClearRecent)));
    }
    for (path, track) in app::pipeline_streams(app) {
        all.push((
            format!("Open Pipeline: {path}"),
            Box::new(app::OpenPipelineTrack { track }),
        ));
    }
    all
}

fn fixed_commands() -> Vec<(&'static str, Box<dyn Action>)> {
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
        (
            "Developer: Cycle GPUI Frame Overlay",
            Box::new(app::CycleFrameOverlay),
        ),
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
        ("Show Transaction", Box::new(app::ShowTransaction)),
        ("Copy Signals", Box::new(app::CopySignals)),
        ("Cut Signals", Box::new(app::CutSignals)),
        ("Paste Signals", Box::new(app::PasteSignals)),
        ("Increase Row Height", Box::new(app::IncreaseRowHeight)),
        ("Decrease Row Height", Box::new(app::DecreaseRowHeight)),
        ("Reset Row Height", Box::new(app::ResetRowHeight)),
        ("Toggle Analog Drawing", Box::new(app::ToggleAnalog)),
        (
            "Add or Name Marker at Cursor",
            Box::new(app::AddOrRenameMarker),
        ),
        (
            "Remove Marker at Cursor",
            Box::new(app::RemoveMarkerAtCursor),
        ),
        ("Remove All Markers", Box::new(app::RemoveAllMarkers)),
        ("Next Marker", Box::new(app::NextMarker)),
        ("Previous Marker", Box::new(app::PrevMarker)),
        ("Return to Before the Last Jump", Box::new(app::JumpBack)),
        ("Measure from Cursor", Box::new(app::SetReference)),
        ("Clear Reference", Box::new(app::ClearReference)),
        ("Zoom to Measurement", Box::new(app::ZoomToMeasurement)),
    ]
}

const MAX_SETTINGS: usize = 8;

impl PaletteModel {
    fn compute(query: &str, app: &CoreApp) -> Self {
        if let Some(query) = volna_core::marker::navigator_query(query) {
            return Self {
                commands: Vec::new(),
                settings: Vec::new(),
                markers: Some(app.navigator_rows(query)),
                find_marker: false,
                preview: Vec::new(),
            };
        }
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let commands = commands(app, query)
            .into_iter()
            .filter(|(label, _)| {
                let lower = label.to_lowercase();
                // "go to 1500" and "1500" both find "Go to Cycle 1500".
                words.iter().all(|w| lower.contains(w))
            })
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
        // Typing "mar…" shows markers here and offers the navigator.
        let asked = volna_core::marker::palette_preview(query);
        let loaded = app.doc.is_loaded();
        let find = FIND_MARKER.to_lowercase();
        Self {
            commands,
            settings,
            markers: None,
            find_marker: loaded
                && (asked.is_some() || words.iter().all(|w| find.contains(w.as_str()))),
            preview: asked
                .filter(|_| loaded)
                .map(|rest| {
                    let mut rows = app.navigator_rows(&rest);
                    rows.truncate(PREVIEW);
                    rows
                })
                .unwrap_or_default(),
        }
    }
}

/// One navigator row: number, name, time, the step from the previous marker
/// and the distance from the reference, in fixed columns.
fn marker_row(row: &NavigatorRow, cx: &gpui_kit::App) -> gpui_kit::Div {
    let t = *crate::theme::theme(cx);
    let colors = t.editor;
    let mono = |text: String, color: gpui_kit::Hsla, w: f32| {
        div()
            .flex_none()
            .w(t.px(w))
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(t.mono_font)
            .text_size(px(t.ui_size_small))
            .text_color(color)
            .child(SharedString::from(text))
    };
    let marker = t.marker(row.id.palette_index());
    let number = if row.is_reference {
        format!("{} R", row.id)
    } else {
        row.id.to_string()
    };
    div()
        .debug_selector(move || format!("marker-row-{}", row.id))
        .flex()
        .items_center()
        .gap_2()
        .w_full()
        .child(mono(number, marker.stroke, 36.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(if row.name.is_some() {
                    colors.text
                } else {
                    colors.text_placeholder
                })
                .child(SharedString::from(
                    row.name.clone().unwrap_or_else(|| "No name".into()),
                )),
        )
        .child(mono(row.time.clone(), colors.text_muted, 84.0))
        .child(mono(
            row.step.clone().unwrap_or_default(),
            colors.text_muted,
            84.0,
        ))
        .child(mono(
            row.from_reference
                .clone()
                .map(|d| format!("R {d}"))
                .unwrap_or_default(),
            colors.text_muted,
            190.0,
        ))
}

impl Workspace {
    pub(crate) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette_with("", window, cx);
    }

    /// Open the palette with `query` typed, such as `@` for the markers.
    pub(crate) fn open_palette_with(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        let model = cx.new(|_| PaletteModel::compute(query, &self.app));
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
                    let navigator = read.markers.is_some();
                    let mut palette = Palette::new(&state)
                        .filterable(false)
                        .bordered(false)
                        .placeholder(if navigator {
                            "Find a marker by name or number…"
                        } else {
                            "Type a command, search settings, or @ for markers…"
                        })
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
                            let state = state.clone();
                            move |path, window, cx| {
                                // Sections: commands, markers, settings; or
                                // only markers in `@` mode.
                                let (find, marker) = {
                                    let read = model.read(cx);
                                    match &read.markers {
                                        Some(rows) => (false, rows.get(path.row).map(|r| r.id)),
                                        None if path.section == read.markers_section() => {
                                            let row = path.row;
                                            match (read.find_marker, row) {
                                                (true, 0) => (true, None),
                                                (true, row) => {
                                                    (false, read.preview.get(row - 1).map(|r| r.id))
                                                }
                                                (false, row) => {
                                                    (false, read.preview.get(row).map(|r| r.id))
                                                }
                                            }
                                        }
                                        None => (false, None),
                                    }
                                };
                                if find {
                                    // Stay open, listing every marker.
                                    state.update(cx, |state, cx| state.set_query("@", window, cx));
                                    return;
                                }
                                if let Some(id) = marker {
                                    window.close_dialog(cx);
                                    _ = ws.update(cx, |ws, cx| {
                                        ws.dispatch(
                                            Command::Lane(LaneVerb::GoTo(id)),
                                            Some(window),
                                            cx,
                                        );
                                    });
                                    return;
                                }
                                let chosen = {
                                    let read = model.read(cx);
                                    if path.section == 2 {
                                        // Settings come last either way.
                                        read.settings.get(path.row).copied()
                                    } else {
                                        None
                                    }
                                };
                                window.close_dialog(cx);
                                // Closing gave the keys back to the panel; a
                                // name field the command opened takes them.
                                _ = ws.update(cx, |ws, cx| ws.focus_rename(window, cx));
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
                    if let Some(rows) = &read.markers {
                        let mut group = CommandGroup::new().label("Markers");
                        for row in rows {
                            let label = match &row.name {
                                Some(name) => format!("{} {name}", row.id),
                                None => row.id.to_string(),
                            };
                            let row = row.clone();
                            group = group.item(
                                CommandItem::new()
                                    .label(label)
                                    .child(move |_, cx| marker_row(&row, cx)),
                            );
                        }
                        let empty = if read.markers.as_ref().is_some_and(|_| {
                            query_ws
                                .upgrade()
                                .is_some_and(|ws| ws.read(cx).app.doc.markers().is_empty())
                        }) {
                            "No markers yet · M marks the cursor"
                        } else {
                            "No marker matches"
                        };
                        let palette = palette
                            .group(group)
                            .empty(move |_, _, _| div().p_3().child(empty))
                            .footer(|_, _, cx| {
                                let t = *crate::theme::theme(cx);
                                div()
                                    .px_3()
                                    .py_1()
                                    .text_size(px(t.ui_size_small))
                                    .text_color(t.editor.text_muted)
                                    .child("↵ go to · ⇧↵ measure from · F2 rename · Del remove")
                            });
                        return content.child(navigator_keys(palette, &state, &model, &query_ws));
                    }
                    let mut commands = CommandGroup::new().label("Commands");
                    for (label, action) in &read.commands {
                        commands = commands.item(
                            CommandItem::new()
                                .label(label.clone())
                                .action(action.boxed_clone()),
                        );
                    }
                    let mut markers = CommandGroup::new().label("Markers");
                    if read.find_marker {
                        markers = markers.item(CommandItem::new().label(FIND_MARKER));
                    }
                    for row in &read.preview {
                        let label = match &row.name {
                            Some(name) => format!("{} {name}", row.id),
                            None => row.id.to_string(),
                        };
                        let row = row.clone();
                        markers = markers.item(
                            CommandItem::new()
                                .label(label)
                                .child(move |_, cx| marker_row(&row, cx)),
                        );
                    }
                    palette = if read.markers_section() == 0 {
                        palette.group(markers).group(commands)
                    } else {
                        palette.group(commands).group(markers)
                    };
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
        if !query.is_empty() {
            focus_state.update(cx, |state, cx| state.set_query(query, window, cx));
        }
        let focus = focus_state.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }
}

/// The navigator's own keys around `palette`: each acts on the highlighted
/// marker and closes the palette, except `Del`, which removes it and keeps
/// the list open. Outside marker mode they propagate.
fn navigator_keys(
    palette: Palette,
    state: &gpui_kit::Entity<CommandState>,
    model: &gpui_kit::Entity<PaletteModel>,
    ws: &WeakEntity<Workspace>,
) -> gpui_kit::Div {
    let selected = {
        let (state, model) = (state.clone(), model.clone());
        move |cx: &gpui_kit::App| {
            let row = state.read(cx).selected_index()?.row;
            model.read(cx).markers.as_ref()?.get(row).map(|r| r.id)
        }
    };
    let act = {
        let (ws, model, state) = (ws.clone(), model.clone(), state.clone());
        move |verb: LaneVerb, close: bool, window: &mut Window, cx: &mut gpui_kit::App| {
            if close {
                window.close_dialog(cx);
            }
            let Some(owner) = ws.upgrade() else { return };
            owner.update(cx, |ws, cx| {
                ws.dispatch(Command::Lane(verb), Some(window), cx)
            });
            if !close {
                let query = state.read(cx).query(cx).to_string();
                let next = PaletteModel::compute(&query, &owner.read(cx).app);
                model.update(cx, |model, cx| {
                    *model = next;
                    cx.notify();
                });
            }
        }
    };
    let (s1, s2, s3) = (selected.clone(), selected.clone(), selected);
    let (a1, a2, a3) = (act.clone(), act.clone(), act);
    div()
        .on_action(move |_: &MeasureFromMarker, window, cx| match s1(cx) {
            Some(id) => a1(LaneVerb::MeasureFrom(id), true, window, cx),
            None => cx.propagate(),
        })
        .on_action(move |_: &RenameMarker, window, cx| match s2(cx) {
            Some(id) => a2(LaneVerb::Rename(id), true, window, cx),
            None => cx.propagate(),
        })
        .on_action(move |_: &RemoveMarker, window, cx| match s3(cx) {
            Some(id) => a3(LaneVerb::Remove(id), false, window, cx),
            None => cx.propagate(),
        })
        .child(palette)
}
