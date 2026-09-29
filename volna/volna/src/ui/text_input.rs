//! A minimal single-line text field (filter box, group name editor). Handles
//! printable input, backspace, word delete, escape and its own text undo:
//! its key context binds ⌘Z / Ctrl+Z, so undo in a field edits the text and
//! never the cockpit (`volna/volna/ARCHITECTURE.md`, "Undo and redo"). No IME or selection; enough
//! for a filter or a name.

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, CursorStyle, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, actions, div, px,
};

use super::icon::{Icon, IconName};
use crate::theme::{ThemePx, theme};

actions!(text_input, [UndoText, RedoText]);

const CONTEXT: &str = "TextInput";

/// Text undo and redo keys, on every platform: the innermost key context
/// wins, so these shadow the workspace's cockpit undo while a field has focus.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-z", UndoText, Some(CONTEXT)),
        KeyBinding::new("ctrl-z", UndoText, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-z", RedoText, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-z", RedoText, Some(CONTEXT)),
        KeyBinding::new("ctrl-y", RedoText, Some(CONTEXT)),
    ]);
}

/// What changed the text last: runs of typing or deleting undo together.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Type,
    Delete,
    Other,
}

pub enum TextInputEvent {
    Changed,
    /// Enter was pressed.
    Submit,
    /// Escape was pressed with empty text (any text, when plain).
    Cancel,
}

pub struct TextInput {
    text: String,
    placeholder: SharedString,
    focus_handle: FocusHandle,
    /// An editor rather than a filter: no search icon or clear button, it
    /// fills its box, and Escape cancels at once.
    plain: bool,
    /// The whole text is selected: typing replaces it, Backspace clears it,
    /// and any other key just drops the selection.
    all_selected: bool,
    /// The colour of what the field names (a marker's): a thicker outline
    /// in it, square on the left where the field meets its chip, and the
    /// selection tinted with it.
    accent: Option<Hsla>,
    /// Text before each undoable edit, and text undone.
    undo: Vec<String>,
    redo: Vec<String>,
    last_edit: Option<EditKind>,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        TextInput {
            text: String::new(),
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
            plain: false,
            all_selected: false,
            accent: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
        }
    }

    /// Select the whole text, as a rename starts: typing replaces it.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if !self.all_selected && !self.text.is_empty() {
            self.all_selected = true;
            cx.notify();
        }
    }

    /// An editor that fills its box (see the `plain` field).
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }

    /// Outline the field in `color` (see the `accent` field).
    pub fn accent(mut self, color: Hsla) -> Self {
        self.accent = Some(color);
        self
    }

    #[cfg(test)]
    pub fn accent_color(&self) -> Option<Hsla> {
        self.accent
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.text.is_empty() {
            self.edit(EditKind::Other, String::clear, cx);
        }
    }

    /// Replace the text without emitting `Changed` (the model already
    /// knows); the text undo starts over from it.
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        if self.text != text {
            self.text = text;
            self.undo.clear();
            self.redo.clear();
            self.last_edit = None;
            cx.notify();
        }
    }

    /// Append text (used when another view forwards a typed character).
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() || text.chars().any(|c| c.is_control()) {
            return;
        }
        self.edit(EditKind::Type, |t| t.push_str(text), cx);
    }

    /// Change the text as an undoable edit; a run of one kind of edit
    /// undoes at once.
    fn edit(&mut self, kind: EditKind, f: impl FnOnce(&mut String), cx: &mut Context<Self>) {
        let before = self.text.clone();
        f(&mut self.text);
        if self.text == before {
            return;
        }
        if kind == EditKind::Other || self.last_edit != Some(kind) {
            self.undo.push(before);
        }
        self.last_edit = Some(kind);
        self.redo.clear();
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn undo_text(&mut self, _: &UndoText, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.text, text));
            self.restored(cx);
        }
    }

    fn redo_text(&mut self, _: &RedoText, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.text, text));
            self.restored(cx);
        }
    }

    fn restored(&mut self, cx: &mut Context<Self>) {
        self.last_edit = None;
        self.all_selected = false;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if (ks.modifiers.platform || ks.modifiers.control) && ks.key == "a" {
            self.select_all(cx);
            return;
        }
        let selected = std::mem::take(&mut self.all_selected);
        if selected {
            cx.notify();
        }
        match ks.key.as_str() {
            "backspace" if selected => self.clear(cx),
            "backspace" => {
                let word = ks.modifiers.alt || ks.modifiers.platform;
                let kind = if word {
                    EditKind::Other
                } else {
                    EditKind::Delete
                };
                self.edit(
                    kind,
                    |text| {
                        if word {
                            let trimmed = text.trim_end().len();
                            let cut = text[..trimmed].rfind(' ').map(|i| i + 1).unwrap_or(0);
                            text.truncate(cut);
                        } else {
                            text.pop();
                        }
                    },
                    cx,
                );
            }
            "escape" => {
                if self.text.is_empty() || self.plain {
                    cx.emit(TextInputEvent::Cancel);
                } else {
                    self.clear(cx);
                }
            }
            "enter" => cx.emit(TextInputEvent::Submit),
            _ => {
                if ks.modifiers.platform || ks.modifiers.control {
                    return;
                }
                if let Some(c) = ks.key_char.as_deref()
                    && !c.chars().any(|ch| ch.is_control())
                {
                    let kind = if selected {
                        EditKind::Other
                    } else {
                        EditKind::Type
                    };
                    self.edit(
                        kind,
                        |text| {
                            if selected {
                                text.clear();
                            }
                            text.push_str(c);
                        },
                        cx,
                    );
                }
            }
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme(cx);
        let colors = t.input;
        let focused = self.focus_handle.is_focused(window);
        let empty = self.text.is_empty();
        let border = if focused {
            t.border_focused
        } else {
            t.input_border
        };
        let hover_border = t.border_focused;
        div()
            .id("text-input")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::undo_text))
            .on_action(cx.listener(Self::redo_text))
            .flex()
            .items_center()
            .gap_2()
            .when(self.plain, |el| el.size_full())
            .when(!self.plain, |el| el.h(t.px(24.0)))
            .px_2()
            .bg(t.input.bg)
            .map(|el| match self.accent {
                Some(accent) => el
                    .border_2()
                    .border_color(accent)
                    .rounded_r(t.px(4.0))
                    .ml(px(-1.0)),
                None => el
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .hover(move |s| s.border_color(hover_border)),
            })
            .cursor(CursorStyle::IBeam)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_click(cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)))
            .when(!self.plain, |el| {
                el.child(
                    Icon::new(IconName::Search)
                        .size(t.px(14.0))
                        .color(colors.icon_muted),
                )
            })
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(if empty {
                        div()
                            .text_color(colors.text_placeholder)
                            .child(self.placeholder.clone())
                    } else {
                        div()
                            .text_color(colors.text)
                            .when(self.all_selected, |el| {
                                el.bg(self.accent.map_or(t.selection.bg, |a| a.alpha(0.45)))
                            })
                            .child(SharedString::from(self.text.clone()))
                    })
                    .when(focused, |el| {
                        el.child(div().w(px(1.0)).h(t.px(14.0)).bg(colors.text))
                    }),
            )
            .when(!empty && !self.plain, |el| {
                el.child(
                    div()
                        .id("clear")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(t.px(16.0))
                        .rounded_sm()
                        .cursor(CursorStyle::PointingHand)
                        .hover(move |s| s.bg(colors.bg).border_1().border_color(hover_border))
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
                        .child(
                            Icon::new(IconName::X)
                                .size(t.px(12.0))
                                .color(colors.icon_muted),
                        ),
                )
            })
    }
}
