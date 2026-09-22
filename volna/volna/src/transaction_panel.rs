//! The Transaction panel: everything recorded about one selected record.
//! Every decision (which record, what it contains, how a value reads, what it
//! is related to) is `volna_core::transaction`; this module only draws the
//! prepared view and forwards commands.

#[cfg(not(target_arch = "wasm32"))]
use gpui_kit::ClipboardItem;
use gpui_kit::base::dock as base;
use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
    dock::Panel,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement,
    tooltip::Tooltip,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, IntoElement,
    Render, SharedString, Subscription, WeakEntity, Window, div, px, relative,
};
use volna_core::Command;
use volna_core::panels::{PanelId, PanelsCommand};
use volna_core::transaction::view::FILTER_THRESHOLD;
use volna_core::transaction::{
    AttrRow, Chip, EventRow, RefRole, RefRow, Section, StageRow, TransactionCommand, TxPanelState,
    TxView,
};

use crate::app::Workspace;
use crate::theme::{ThemePx, hsla, theme};
use crate::ui::{Icon, IconName};

/// Height of one lifeline lane, in design pixels.
const LANE_PX: f32 = 18.0;

pub(crate) struct TransactionPanelView {
    ws: WeakEntity<Workspace>,
    id: PanelId,
    generation: u64,
    pub(crate) focus: FocusHandle,
    filter: Entity<InputState>,
    synced_filter: String,
    _subscriptions: Vec<Subscription>,
}

impl TransactionPanelView {
    pub(crate) fn new(
        ws: WeakEntity<Workspace>,
        id: PanelId,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        cx.on_focus_in(&focus, window, |view, window, cx| {
            view.panels(PanelsCommand::Focus(view.id), window, cx);
        })
        .detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter attributes"));
        let subscription = cx.subscribe_in(&filter, window, |view, input, event, window, cx| {
            if let InputEvent::Change = event {
                let text = input.read(cx).value().to_string();
                view.synced_filter = text.clone();
                view.dispatch(TransactionCommand::Filter(text), window, cx);
            }
        });
        Self {
            ws,
            id,
            generation,
            focus,
            filter,
            synced_filter: String::new(),
            _subscriptions: vec![subscription],
        }
    }

    fn dispatch(&self, command: TransactionCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.send(Command::Transaction(self.id, command), window, cx);
    }

    fn panels(&self, command: PanelsCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.send(Command::Panels(command), window, cx);
    }

    fn send(&self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        _ = self.ws.update(cx, |ws, cx| {
            ws.dispatch_if_current(self.generation, command, Some(window), cx)
        });
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let Some(owner) = self.ws.upgrade() else {
            return;
        };
        let result = {
            let ws = owner.read(cx);
            ws.app
                .panels
                .transaction(self.id)
                .map(|model| model.copy_tsv(&ws.app.doc))
        };
        match result {
            Some(Ok(text)) => {
                #[cfg(target_arch = "wasm32")]
                crate::table_clipboard::copy(&text);
                #[cfg(not(target_arch = "wasm32"))]
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Some(Err(error)) => {
                owner.update(cx, |ws, cx| {
                    ws.dispatch_if_current(
                        self.generation,
                        Command::Notice(error.to_string()),
                        None,
                        cx,
                    )
                });
            }
            None => {}
        }
    }

    /// Keep the filter box in step with the model (a split or a restore).
    fn sync_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let Some(text) = ws
            .read(cx)
            .app
            .panels
            .transaction(self.id)
            .map(|model| model.prefs.filter.clone())
        else {
            return;
        };
        if text != self.synced_filter {
            self.synced_filter = text.clone();
            self.filter
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }
}

impl EventEmitter<base::PanelEvent> for TransactionPanelView {}
impl Focusable for TransactionPanelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl base::Panel for TransactionPanelView {
    fn panel_name(&self) -> &'static str {
        "volna.transaction"
    }
    // Only core close commands remove a panel; the dock never does.
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn dump(&self, _: &App) -> base::PanelState {
        let mut state = base::PanelState::new("volna.transaction");
        state.info = base::PanelInfo::panel(serde_json::json!({"id": self.id.0}));
        state
    }
}
impl Panel for TransactionPanelView {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(
            self.ws
                .upgrade()?
                .read(cx)
                .app
                .panels
                .get(self.id)?
                .title()
                .into(),
        )
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tab_name(cx).unwrap_or_default()
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        use gpui_kit::assets::IconName as KitIcon;
        let owner = self.ws.upgrade()?;
        let model = owner.read(cx).app.panels.transaction(self.id)?;
        let (back, forward, pinned) = (model.can_go_back(), model.can_go_forward(), model.pinned);
        Some(vec![
            Button::new("tx-back")
                .icon(KitIcon::ChevronLeft)
                .ghost()
                .xsmall()
                .tooltip("Back (Alt+←)")
                .when(!back, |button| button.disabled(true))
                .on_click(cx.listener(|view, _, window, cx| {
                    view.dispatch(TransactionCommand::Back, window, cx)
                })),
            Button::new("tx-forward")
                .icon(KitIcon::ChevronRight)
                .ghost()
                .xsmall()
                .tooltip("Forward (Alt+→)")
                .when(!forward, |button| button.disabled(true))
                .on_click(cx.listener(|view, _, window, cx| {
                    view.dispatch(TransactionCommand::Forward, window, cx)
                })),
            Button::new("tx-pin")
                .icon(KitIcon::Pin)
                .ghost()
                .xsmall()
                .selected(pinned)
                .tooltip(if pinned {
                    "Pinned: this panel keeps its record"
                } else {
                    "Pin this record; the next one opens another panel"
                })
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.dispatch(TransactionCommand::Pin(!pinned), window, cx)
                })),
            Button::new("tx-close")
                .icon(KitIcon::X)
                .ghost()
                .xsmall()
                .tooltip("Close panel")
                .on_click(cx.listener(|view, _, window, cx| {
                    view.panels(PanelsCommand::Close(view.id), window, cx)
                })),
        ])
    }
}

impl Render for TransactionPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_filter(window, cx);
        let t = *theme(cx);
        let Some(owner) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        let (state, follows, revealable) = {
            let ws = owner.read(cx);
            let Some(model) = ws.app.panels.transaction(self.id) else {
                return div().into_any_element();
            };
            let follows = if model.pinned {
                Some("pinned".to_owned())
            } else {
                ws.app
                    .doc
                    .selection()
                    .and_then(|selection| ws.app.panels.get(selection.origin))
                    .map(|panel| format!("follows {}", panel.title()))
            };
            let revealable = model
                .shown()
                .and_then(|shown| Some((shown.track.track()?, shown.id)))
                .map(|(track, id)| {
                    ws.app
                        .panels
                        .pipeline_showing(&ws.app.doc, track, id)
                        .is_some()
                });
            (model.state(&ws.app.doc), follows, revealable)
        };
        let body = match state {
            TxPanelState::Ready(view) => self.render_view(*view, revealable, window, cx),
            TxPanelState::Empty => notice(
                &t,
                "No record selected",
                "Click a row in a pipeline or a table; the panel follows the selection.",
            ),
            TxPanelState::Loading => notice(&t, "Loading the record's track…", ""),
            TxPanelState::Missing => notice(
                &t,
                "Record not in this trace",
                "The saved track or record is not part of the open recording.",
            ),
            TxPanelState::Failed(error) => notice(&t, "Cannot load the record", &error),
            TxPanelState::Refused(error) => notice(&t, "Not admitted", &error),
        };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(t.px(28.0))
            .flex_none()
            .text_size(px(t.ui_size_small))
            .text_color(t.panel.text_muted)
            .when_some(follows, |element, text| {
                element.child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_sm()
                        .bg(t.badge.bg)
                        .text_color(t.badge.text)
                        .child(SharedString::from(text)),
                )
            });
        div()
            .id(("transaction-panel", self.id.0))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.editor.bg)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .text_color(t.editor.text)
            .track_focus(&self.focus)
            .key_context("Transaction")
            .on_key_down(
                cx.listener(|view, event: &gpui_kit::KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    let modifiers = crate::app::to_modifiers(event.keystroke.modifiers);
                    if event.keystroke.modifiers.alt && (key == "left" || key == "right") {
                        let command = if key == "left" {
                            TransactionCommand::Back
                        } else {
                            TransactionCommand::Forward
                        };
                        view.dispatch(command, window, cx);
                        cx.stop_propagation();
                    } else if key.eq_ignore_ascii_case("c") && modifiers.secondary() {
                        view.copy(cx);
                        cx.stop_propagation();
                    } else if key == "escape" {
                        // Focus returns to the panel that chose the record.
                        if let Some(origin) = view
                            .ws
                            .upgrade()
                            .and_then(|ws| ws.read(cx).app.doc.selection())
                            .map(|selection| selection.origin)
                        {
                            view.panels(PanelsCommand::Focus(origin), window, cx);
                        }
                        cx.stop_propagation();
                    }
                }),
            )
            .child(header)
            .child(body)
            .into_any_element()
    }
}

impl TransactionPanelView {
    fn render_view(
        &self,
        view: TxView,
        revealable: Option<bool>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *theme(cx);
        let muted = t.panel.text_muted;
        let identity = &view.identity;
        let mut breadcrumb = div()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(t.ui_size_small))
            .text_color(muted)
            .child(Icon::new(IconName::Workflow).size(t.px(12.0)).color(muted));
        if !identity.stream.is_empty() {
            breadcrumb = breadcrumb.child(SharedString::from(identity.stream.join(".")));
            breadcrumb = breadcrumb.child("›");
        }
        breadcrumb = breadcrumb
            .child(SharedString::from(identity.generator.clone()))
            .child(SharedString::from(format!("· row {}", identity.ordinal)));

        let mut title = div().flex().items_center().gap_2().child(
            div()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .bg(t.badge.bg)
                .font_family(t.mono_font)
                .text_size(px(t.ui_size_small))
                .text_color(t.badge.text)
                .child(SharedString::from(format!("#{}", identity.id.0))),
        );
        title = title.child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(t.mono_font)
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(SharedString::from(
                    identity
                        .label
                        .clone()
                        .unwrap_or_else(|| "(no label)".into()),
                )),
        );
        title = title.child(status_badge(&t, identity.status_text, identity.status));
        if let Some(kind) = identity.kind {
            title = title.child(
                div()
                    .text_size(px(t.ui_size_small))
                    .text_color(muted)
                    .child(kind),
            );
        }
        if let Some(parent) = identity.parent {
            title = title.child(
                div()
                    .text_size(px(t.ui_size_small))
                    .text_color(muted)
                    .child(SharedString::from(format!("child of #{}", parent.0))),
            );
        }

        let timing = &view.timing;
        let mut tiles = div().flex().gap_2();
        for (label, text, time) in [
            ("Begin", timing.begin_text.clone(), Some(timing.begin)),
            ("End", timing.end_text.clone(), timing.end),
            ("Duration", timing.duration_text.clone(), None),
        ] {
            tiles = tiles.child(self.tile(&t, label, text, time, cx));
        }

        let mut actions = div().flex().items_center().gap_1().flex_wrap();
        let reveal_label = match revealable {
            Some(true) => "Reveal in Pipeline",
            _ => "Open in Pipeline",
        };
        actions = actions
            .child(
                Button::new("tx-reveal")
                    .label(reveal_label)
                    .icon(gpui_kit::component::Icon::empty().path(IconName::Locate.path()))
                    .ghost()
                    .small()
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.send(Command::RevealTransaction { panel: view.id }, window, cx)
                    })),
            )
            .child(
                Button::new("tx-fit")
                    .label("Fit lifetime")
                    .ghost()
                    .small()
                    .tooltip("Fit the shared viewport to this record")
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.dispatch(TransactionCommand::FitLifetime, window, cx)
                    })),
            )
            .child(
                Button::new("tx-copy")
                    .label("Copy")
                    .ghost()
                    .small()
                    .tooltip("Copy the complete record as TSV")
                    .on_click(cx.listener(|view, _, _, cx| view.copy(cx))),
            );

        let mut body = div().flex_1().min_h_0().overflow_y_scrollbar().child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .child(breadcrumb)
                .child(title)
                .child(tiles)
                .child(self.lifeline(&t, &view, cx))
                .child(actions),
        );

        body = body.child(self.attributes(&t, &view, cx));
        body = body.child(self.section(&t, &view.stages, cx, stage_row));
        body = body.child(self.section(&t, &view.events, cx, event_row));
        body = body.child(self.related(&t, &view.related, cx));
        if view.truncated_bytes {
            body = body.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_size(px(t.ui_size_small))
                    .text_color(t.panel.text_muted)
                    .child("Values were cut at the admitted byte limit."),
            );
        }
        body.into_any_element()
    }

    /// A timing tile; Begin and End move the shared cursor.
    fn tile(
        &self,
        t: &crate::theme::Theme,
        label: &'static str,
        text: String,
        time: Option<u64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tile = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(t.panel.bg)
            .border_1()
            .border_color(t.border_variant)
            .child(
                div()
                    .text_size(px(t.ui_size_small))
                    .text_color(t.panel.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .font_family(t.mono_font)
                    .child(SharedString::from(text)),
            );
        match time {
            Some(time) => tile
                .id(SharedString::from(format!("tx-time-{label}")))
                .cursor_pointer()
                .hover(|style| style.bg(t.hover.bg))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.dispatch(TransactionCommand::Cursor(time), window, cx)
                }))
                .into_any_element(),
            None => tile.into_any_element(),
        }
    }

    /// The record's own Gantt: one row per lane, events as ticks, and the
    /// shared cursor when it falls inside the lifetime.
    fn lifeline(
        &self,
        t: &crate::theme::Theme,
        view: &TxView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut lanes = div().flex().flex_col().gap_0p5().relative();
        for (index, lane) in view.lifeline.iter().enumerate() {
            let mut row = div()
                .relative()
                .w_full()
                .h(t.px(LANE_PX))
                .bg(t.panel.bg)
                .rounded_sm();
            for (cell_ix, cell) in lane.cells.iter().enumerate() {
                let begin = cell.begin;
                row = row.child(
                    div()
                        .id(SharedString::from(format!("tx-cell-{index}-{cell_ix}")))
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(cell.start))
                        .w(relative(cell.width.max(0.004)))
                        .bg(hsla(cell.style.fill))
                        .border_1()
                        .border_color(hsla(cell.style.edge))
                        .cursor_pointer()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(t.ui_size_small))
                        .text_color(hsla(cell.style.text))
                        .font_family(t.mono_font)
                        .child(SharedString::from(cell.name.clone()))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.dispatch(TransactionCommand::Cursor(begin), window, cx)
                        })),
                );
            }
            lanes = lanes.child(row);
        }
        for (index, tick) in view.event_ticks.iter().enumerate() {
            let time = tick.time;
            lanes = lanes.child(
                div()
                    .id(("tx-event-tick", index))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(tick.at))
                    .w(t.px(2.0))
                    .bg(t.wave_event_coalesced)
                    .cursor_pointer()
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.dispatch(TransactionCommand::Cursor(time), window, cx)
                    })),
            );
        }
        if let Some(at) = view.cursor {
            lanes = lanes.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(at))
                    .w(px(1.0))
                    .bg(t.wave_cursor),
            );
        }
        lanes.into_any_element()
    }

    /// A collapsible heading whose count is the recorded total, cut or not.
    fn heading<T>(
        &self,
        t: &crate::theme::Theme,
        section: &Section<T>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = section.key;
        let collapsed = section.collapsed;
        div()
            .id(SharedString::from(format!("tx-section-{}", key.key())))
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_1()
            .cursor_pointer()
            .hover(|style| style.bg(t.hover.bg))
            .text_size(px(t.ui_size_small))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(t.panel.text_muted)
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(t.px(12.0))
                .color(t.panel.icon_muted),
            )
            .child(SharedString::from(format!(
                "{} · {}",
                key.title(),
                section.total
            )))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.dispatch(TransactionCommand::Collapse(key, !collapsed), window, cx)
            }))
            .into_any_element()
    }

    fn cut_note<T>(&self, t: &crate::theme::Theme, section: &Section<T>) -> Option<AnyElement> {
        section.cut().then(|| {
            div()
                .px_3()
                .py_1()
                .text_size(px(t.ui_size_small))
                .text_color(t.panel.text_muted)
                .child(SharedString::from(format!(
                    "Showing the first {} of {} · transaction.detailItems",
                    section.rows.len(),
                    section.matched
                )))
                .into_any_element()
        })
    }

    fn section<T>(
        &self,
        t: &crate::theme::Theme,
        section: &Section<T>,
        cx: &mut Context<Self>,
        row: impl Fn(&crate::theme::Theme, usize, &T, &mut Context<Self>) -> AnyElement,
    ) -> AnyElement {
        let mut element = div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.border_variant)
            .child(self.heading(t, section, cx));
        if !section.collapsed {
            for (index, entry) in section.rows.iter().enumerate() {
                element = element.child(row(t, index, entry, cx));
            }
            if let Some(note) = self.cut_note(t, section) {
                element = element.child(note);
            }
        }
        element.into_any_element()
    }

    fn attributes(
        &self,
        t: &crate::theme::Theme,
        view: &TxView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let section = &view.attributes;
        let mut element = div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.border_variant)
            .child(self.heading(t, section, cx));
        if section.collapsed {
            return element.into_any_element();
        }
        if section.total > FILTER_THRESHOLD {
            element = element.child(div().px_3().pb_1().child(Input::new(&self.filter).small()));
        }
        for row in &section.rows {
            element = element.child(self.attribute_row(t, row, cx));
        }
        if let Some(note) = self.cut_note(t, section) {
            element = element.child(note);
        }
        element.into_any_element()
    }

    fn attribute_row(
        &self,
        t: &crate::theme::Theme,
        row: &AttrRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = row.full_key.clone();
        let value = div()
            .flex_1()
            .min_w_0()
            .font_family(t.mono_font)
            .child(SharedString::from(row.value.clone()));
        // An integer cycles its radix on click, remembered per key.
        let value = match row.radix {
            Some(radix) => value
                .id(SharedString::from(format!("tx-attr-{key}")))
                .cursor_pointer()
                .hover(|style| style.bg(t.hover.bg))
                .tooltip({
                    let hint = format!("{} · click to cycle the radix", radix.name());
                    move |w, cx| Tooltip::new(hint.clone()).build(w, cx)
                })
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.dispatch(TransactionCommand::Radix(key.clone()), window, cx)
                }))
                .into_any_element(),
            None => value.into_any_element(),
        };
        div()
            .flex()
            .items_start()
            .gap_2()
            .px_3()
            .py_0p5()
            .child(
                div()
                    .w(t.px(150.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(t.panel.text_muted)
                    .child(SharedString::from(row.key.clone()))
                    .when_some(row.phase, |element, phase| element.child(tag(t, phase))),
            )
            .child(value)
            .into_any_element()
    }

    fn related(
        &self,
        t: &crate::theme::Theme,
        section: &Section<RefRow>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut element = div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.border_variant)
            .child(self.heading(t, section, cx));
        if section.collapsed {
            return element.into_any_element();
        }
        let mut group = String::new();
        for (index, row) in section.rows.iter().enumerate() {
            if row.group != group {
                group = row.group.clone();
                element = element.child(
                    div()
                        .px_3()
                        .pt_1()
                        .text_size(px(t.ui_size_small))
                        .text_color(t.panel.text_muted)
                        .child(SharedString::from(group.clone())),
                );
            }
            element = element.child(self.reference(t, index, row, cx));
        }
        if let Some(note) = self.cut_note(t, section) {
            element = element.child(note);
        }
        element.into_any_element()
    }

    fn reference(
        &self,
        t: &crate::theme::Theme,
        index: usize,
        row: &RefRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (track, id) = (row.target.generator, row.target.transaction);
        let color = match row.role {
            RefRole::Relation { outgoing: false } => t.tx_relation_in,
            RefRole::Relation { outgoing: true } => t.tx_relation_out,
            _ => t.panel.icon_muted,
        };
        let subtitle = match (&row.label, row.loaded) {
            (Some(label), _) => label.clone(),
            (None, true) => "(no label)".into(),
            (None, false) => "not loaded · loads on demand".into(),
        };
        div()
            .id(("tx-ref", index))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .cursor_pointer()
            .hover(|style| style.bg(t.hover.bg))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.dispatch(TransactionCommand::Jump { track, id }, window, cx)
            }))
            .child(
                div()
                    .w(t.px(52.0))
                    .flex_none()
                    .font_family(t.mono_font)
                    .text_color(color)
                    .child(SharedString::from(format!("#{}", id.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(SharedString::from(subtitle))
                    .when_some(row.track.clone(), |element, path| {
                        element.child(
                            div()
                                .text_size(px(t.ui_size_small))
                                .text_color(t.panel.text_muted)
                                .child(SharedString::from(path.join("."))),
                        )
                    })
                    .when_some(row.relation_label.clone(), |element, label| {
                        element.child(
                            div()
                                .text_size(px(t.ui_size_small))
                                .text_color(t.panel.text_muted)
                                .child(SharedString::from(label)),
                        )
                    }),
            )
            .child(div().text_color(t.panel.text_muted).child("›"))
            .into_any_element()
    }
}

fn notice(t: &crate::theme::Theme, title: &str, detail: &str) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .p_4()
        .child(
            div()
                .text_color(t.editor.text_muted)
                .child(SharedString::from(title.to_owned())),
        )
        .when(!detail.is_empty(), |element| {
            element.child(
                div()
                    .text_size(px(t.ui_size_small))
                    .text_color(t.editor.text_placeholder)
                    .child(SharedString::from(detail.to_owned())),
            )
        })
        .into_any_element()
}

fn tag(t: &crate::theme::Theme, text: &str) -> AnyElement {
    div()
        .px_1()
        .rounded_sm()
        .bg(t.badge.bg)
        .text_size(px(t.ui_size_small))
        .text_color(t.badge.text)
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

/// The word on the badge; the raw `TxStatus` name is its tooltip.
fn status_badge(
    t: &crate::theme::Theme,
    text: &str,
    status: volna_core::data::transactions::TxStatus,
) -> AnyElement {
    use volna_core::data::transactions::TxStatus;
    let color: Hsla = match status {
        TxStatus::Error | TxStatus::Aborted => t.editor.error,
        TxStatus::Open => t.wave_event_coalesced,
        _ => t.panel.text_muted,
    };
    let raw = status.name();
    div()
        .id("tx-status")
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .border_1()
        .border_color(color)
        .text_size(px(t.ui_size_small))
        .text_color(color)
        .when(status == TxStatus::Open, |element| element.border_dashed())
        .tooltip(move |w, cx| Tooltip::new(raw).build(w, cx))
        .child(SharedString::from(text.to_owned()))
        .into_any_element()
}

fn chips(t: &crate::theme::Theme, items: &[Chip], total: usize) -> AnyElement {
    let mut element = div().flex().flex_wrap().gap_1();
    for chip in items {
        element = element.child(
            div()
                .px_1()
                .rounded_sm()
                .bg(t.badge.bg)
                .font_family(t.mono_font)
                .text_size(px(t.ui_size_small))
                .text_color(t.badge.text)
                .child(SharedString::from(format!("{} {}", chip.key, chip.value))),
        );
    }
    if items.len() < total {
        element = element.child(
            div()
                .text_size(px(t.ui_size_small))
                .text_color(t.panel.text_muted)
                .child(SharedString::from(format!("+{}", total - items.len()))),
        );
    }
    element.into_any_element()
}

fn stage_row(
    t: &crate::theme::Theme,
    index: usize,
    row: &StageRow,
    cx: &mut Context<TransactionPanelView>,
) -> AnyElement {
    let begin = row.begin;
    div()
        .id(("tx-stage", index))
        .flex()
        .items_start()
        .gap_2()
        .px_3()
        .py_1()
        .cursor_pointer()
        .hover(|style| style.bg(t.hover.bg))
        .on_click(cx.listener(move |view, _, window, cx| {
            view.dispatch(TransactionCommand::Cursor(begin), window, cx)
        }))
        .child(
            div()
                .mt(t.px(4.0))
                .w(t.px(10.0))
                .h(t.px(10.0))
                .flex_none()
                .rounded_sm()
                .bg(hsla(row.style.fill)),
        )
        .child(
            div()
                .w(t.px(140.0))
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .font_family(t.mono_font)
                .child(SharedString::from(row.name.clone()))
                .when(!row.primary, |element| element.child(tag(t, &row.lane))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .w_full()
                        .h(t.px(4.0))
                        .rounded_sm()
                        .bg(t.panel.bg)
                        .child(
                            div()
                                .h_full()
                                .w(relative(row.share.max(0.01)))
                                .rounded_sm()
                                .bg(hsla(row.style.fill)),
                        ),
                )
                .child(chips(t, &row.attributes, row.attributes_total)),
        )
        .child(
            div()
                .w(t.px(150.0))
                .flex_none()
                .font_family(t.mono_font)
                .text_size(px(t.ui_size_small))
                .text_color(t.panel.text_muted)
                .child(SharedString::from(format!(
                    "{} → {} · {}",
                    row.begin_text, row.end_text, row.duration_text
                ))),
        )
        .into_any_element()
}

fn event_row(
    t: &crate::theme::Theme,
    index: usize,
    row: &EventRow,
    cx: &mut Context<TransactionPanelView>,
) -> AnyElement {
    let time = row.time;
    div()
        .id(("tx-event", index))
        .flex()
        .items_start()
        .gap_2()
        .px_3()
        .py_1()
        .cursor_pointer()
        .hover(|style| style.bg(t.hover.bg))
        .on_click(cx.listener(move |view, _, window, cx| {
            view.dispatch(TransactionCommand::Cursor(time), window, cx)
        }))
        .child(
            div()
                .w(t.px(150.0))
                .flex_none()
                .font_family(t.mono_font)
                .child(SharedString::from(row.name.clone())),
        )
        .child(
            div()
                .w(t.px(100.0))
                .flex_none()
                .font_family(t.mono_font)
                .text_size(px(t.ui_size_small))
                .text_color(t.panel.text_muted)
                .child(SharedString::from(row.time_text.clone())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(chips(t, &row.attributes, row.attributes_total)),
        )
        .into_any_element()
}
