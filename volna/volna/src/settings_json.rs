//! The JSON view of the Settings tab: gpui-kit's `Editor` over the document
//! text with completion, hover and diagnostics from the registry, a read-only
//! "default settings" document, and the apply button that hands the text
//! back to the core.

use std::rc::Rc;

use anyhow::Result;
use gpui_kit::base::input::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
    input::{CompletionProvider, Editor, EditorState, HoverProvider, InputEvent, Rope, RopeExt},
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, SharedString, Subscription, Task, WeakEntity,
    Window, div, px,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, Hover,
    HoverContents, MarkupContent, MarkupKind,
};
use volna_core::settings::{self, Host, Kind, Severity, Spec, spec};

use crate::app::Workspace;
use crate::settings_panel::SettingsPanelView;
use crate::theme::theme;

pub(crate) struct JsonView {
    editor: Entity<EditorState>,
    defaults: Entity<EditorState>,
    /// The core text last pushed into the editor.
    synced: String,
    show_defaults: bool,
    _subscriptions: Vec<Subscription>,
}

impl JsonView {
    pub(crate) fn new(
        ws: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<SettingsPanelView>,
    ) -> Self {
        let host = ws
            .upgrade()
            .map_or(Host::Native, |ws| ws.read(cx).app.settings.host());
        let provider = Rc::new(Registry { host });
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("json")
                .line_number(true)
                .placeholder("{}");
            state.lsp_mut().completion_provider = Some(provider.clone());
            state.lsp_mut().hover_provider = Some(provider.clone());
            state
        });
        let defaults = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .line_number(false)
                .default_value(settings::schema::default_document(host))
        });
        let ws_change = ws.clone();
        let subscription = cx.subscribe_in(&editor, window, move |this, editor, event, _, cx| {
            if let InputEvent::Change = event
                && let Some(json) = &this.json
                && let Some(ws) = ws_change.upgrade()
            {
                let text = editor.read(cx).value().to_string();
                let diagnostics = ws.read(cx).app.settings.diagnose(&text);
                json.publish(&diagnostics, cx);
                cx.notify();
            }
        });
        Self {
            editor,
            defaults,
            synced: String::new(),
            show_defaults: false,
            _subscriptions: vec![subscription],
        }
    }

    pub(crate) fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    /// Show the core's diagnostics as squiggles.
    fn publish(&self, diagnostics: &[settings::Diagnostic], cx: &mut App) {
        self.editor.update(cx, |state, cx| {
            let text = state.text().clone();
            if let Some(set) = state.diagnostics_mut() {
                set.clear();
                for d in diagnostics {
                    let (start, end) = if d.span.is_empty() {
                        let line = d.line.saturating_sub(1);
                        (text.line_start_offset(line), text.line_end_offset(line))
                    } else {
                        (d.span.start, d.span.end)
                    };
                    let start = text.offset_to_position(start.min(text.len()));
                    let end = text.offset_to_position(end.min(text.len()));
                    set.push(
                        Diagnostic::new(start..end, d.message.clone()).with_severity(
                            match d.severity {
                                Severity::Error => DiagnosticSeverity::Error,
                                Severity::Warning => DiagnosticSeverity::Warning,
                            },
                        ),
                    );
                }
            }
            cx.notify();
        });
    }

    pub(crate) fn render(
        &mut self,
        ws: &Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<SettingsPanelView>,
    ) -> AnyElement {
        let t = *theme(cx);
        let store = &ws.read(cx).app.settings;
        let core_text = store.text().to_owned();
        let diagnostics = store.diagnostics().to_vec();
        let editable = store.is_writable();
        let write_error: Option<SharedString> = store.last_error().map(|e| e.to_owned().into());
        // Follow the core's text unless the user has unsaved edits.
        if core_text != self.synced {
            let local = self.text(cx);
            self.synced = core_text.clone();
            if local == self.synced || local.is_empty() {
                let text = core_text.clone();
                self.editor
                    .update(cx, |state, cx| state.set_value(text, window, cx));
                self.publish(&diagnostics, cx);
            }
        }
        let dirty = self.text(cx) != core_text;
        let syntax: Option<SharedString> = diagnostics
            .iter()
            .find(|d| d.key.is_none())
            .map(|d| format!("Line {}: {}", d.line, d.message).into());
        let problems = diagnostics.iter().filter(|d| d.key.is_some()).count();
        let show_defaults = self.show_defaults;
        let editor = Editor::new(&self.editor)
            .h_full()
            .bordered(false)
            .readonly(!editable)
            .into_any_element();
        let defaults = show_defaults.then(|| {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_l_1()
                .border_color(t.border)
                .child(
                    Editor::new(&self.defaults)
                        .h_full()
                        .bordered(false)
                        .readonly(true),
                )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex_1()
                            .font_family(t.mono_font)
                            .text_size(px(t.ui_size_small))
                            .text_color(t.panel.text_muted)
                            .child(SharedString::from(format!(
                                "settings.json · {} changed{}",
                                ws.read(cx).app.settings.modified().len(),
                                if dirty { " · unsaved edits" } else { "" }
                            ))),
                    )
                    .when(problems > 0, |el| {
                        el.child(
                            div()
                                .text_size(px(t.ui_size_small))
                                .text_color(t.panel.error)
                                .child(SharedString::from(format!("{problems} problem(s)"))),
                        )
                    })
                    .child(
                        Button::new("settings-defaults")
                            .label("Default settings")
                            .ghost()
                            .xsmall()
                            .selected(show_defaults)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(json) = &mut this.json {
                                    json.show_defaults = !json.show_defaults;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("settings-apply")
                            .label("Apply")
                            .primary()
                            .xsmall()
                            .disabled(!dirty || !editable)
                            .tooltip("Write settings.json (⌘S)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_json(&crate::settings_panel::ApplySettingsJson, window, cx)
                            })),
                    ),
            )
            .when_some(syntax, |el, message| {
                el.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .bg(t.panel.bg)
                        .border_b_1()
                        .border_color(t.panel.error)
                        .text_size(px(t.ui_size_small))
                        .text_color(t.panel.error)
                        .child(SharedString::from(format!(
                            "{message}. The last good values are kept and the settings editor is read-only until it is fixed."
                        ))),
                )
            })
            .when_some(write_error, |el, message| {
                el.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .border_b_1()
                        .border_color(t.panel.error)
                        .text_size(px(t.ui_size_small))
                        .text_color(t.panel.error)
                        .child(message),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(div().flex_1().min_w_0().h_full().child(editor))
                    .children(defaults),
            )
            .into_any_element()
    }
}

/// Completion and hover over the registry, without a language server.
struct Registry {
    host: Host,
}

/// Where the caret is on its line: in a key, in a value of `key`, or elsewhere.
enum Place {
    Key,
    Value(&'static Spec),
    Other,
}

fn place(rope: &Rope, offset: usize) -> Place {
    let position = rope.offset_to_position(offset);
    let line_start = rope.line_start_offset(position.line as usize);
    let before: String = rope.slice(line_start..offset).to_string();
    let mut in_string = false;
    let mut colon = None;
    let mut quotes = Vec::new();
    let mut escaped = false;
    for (i, c) in before.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_string => escaped = true,
            '"' => {
                in_string = !in_string;
                quotes.push(i);
            }
            ':' if !in_string && colon.is_none() => colon = Some(i),
            _ => {}
        }
    }
    match colon {
        None => {
            if before.trim().is_empty() || (quotes.len() == 1 && in_string) {
                Place::Key
            } else {
                Place::Other
            }
        }
        Some(colon) => {
            let key = before[..colon].trim().trim_matches('"');
            match spec(key) {
                Some(spec) => Place::Value(spec),
                None => Place::Other,
            }
        }
    }
}

impl CompletionProvider for Registry {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let items: Vec<CompletionItem> = match place(rope, offset) {
            Place::Key => {
                let position = rope.offset_to_position(offset);
                let line_start = rope.line_start_offset(position.line as usize);
                let bare = rope.slice(line_start..offset).to_string().trim().is_empty();
                settings::REGISTRY
                    .iter()
                    .filter(|s| s.available(self.host))
                    .map(|s| CompletionItem {
                        label: s.id.to_owned(),
                        kind: Some(CompletionItemKind::PROPERTY),
                        detail: Some(s.title.to_owned()),
                        documentation: Some(lsp_types::Documentation::String(
                            s.description.to_owned(),
                        )),
                        insert_text: bare
                            .then(|| format!("\"{}\": {}", s.id, s.default.value().to_json_text())),
                        ..Default::default()
                    })
                    .collect()
            }
            Place::Value(spec) => {
                let quoted = |v: &str| format!("\"{v}\"");
                match spec.kind {
                    Kind::Bool => ["true", "false"]
                        .iter()
                        .map(|v| CompletionItem {
                            label: (*v).to_owned(),
                            kind: Some(CompletionItemKind::VALUE),
                            ..Default::default()
                        })
                        .collect(),
                    Kind::Enum(_) => spec
                        .choices(self.host)
                        .into_iter()
                        .map(|c| CompletionItem {
                            label: quoted(c.value),
                            kind: Some(CompletionItemKind::ENUM_MEMBER),
                            detail: Some(c.label.to_owned()),
                            ..Default::default()
                        })
                        .collect(),
                    Kind::Theme => volna_core::theme::builtin::choices()
                        .map(|(id, label)| CompletionItem {
                            label: quoted(id),
                            kind: Some(CompletionItemKind::ENUM_MEMBER),
                            detail: Some(label.into()),
                            ..Default::default()
                        })
                        .collect(),
                    _ => vec![CompletionItem {
                        label: spec.default.value().to_json_text(),
                        kind: Some(CompletionItemKind::VALUE),
                        detail: Some("default".into()),
                        ..Default::default()
                    }],
                }
            }
            Place::Other => Vec::new(),
        };
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text
            .chars()
            .all(|c| c == '"' || c.is_ascii_alphanumeric() || c == '.')
            && !new_text.is_empty()
    }
}

impl HoverProvider for Registry {
    fn hover(
        &self,
        rope: &Rope,
        offset: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let position = rope.offset_to_position(offset);
        let line = position.line as usize;
        let start = rope.line_start_offset(line);
        let end = rope.line_end_offset(line);
        let text = rope.slice(start..end).to_string();
        let hover = text.split('"').nth(1).and_then(spec).map(|spec| {
            let choices = match spec.kind {
                Kind::Enum(_) => spec
                    .choices(self.host)
                    .iter()
                    .map(|c| format!("`{}` {}", c.value, c.label))
                    .collect::<Vec<_>>()
                    .join(", "),
                Kind::Integer { min, max, .. } => format!("{min}–{max}"),
                Kind::Number { min, max, .. } => format!("{min}–{max}"),
                Kind::Bool => "true or false".into(),
                Kind::Text => "text".into(),
                Kind::Theme => {
                    volna_core::theme::builtin::choices()
                        .map(|(id, _)| format!("`{id}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                        + ", or a palette file name"
                }
            };
            Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!(
                        "**{}** `{}`\n\n{}\n\nAllowed: {}. Default: `{}`.{}",
                        spec.title,
                        spec.id,
                        spec.description,
                        choices,
                        spec.default.value().to_json_text(),
                        spec.apply
                            .badge()
                            .map(|b| format!(" ({b})"))
                            .unwrap_or_default()
                    ),
                }),
                range: Some(lsp_types::Range {
                    start: rope.offset_to_position(start),
                    end: rope.offset_to_position(end),
                }),
            }
        });
        Task::ready(Ok(hover))
    }
}
