//! GPUI chrome for the reduced table. Row decisions, bounded preparation and
//! exact identities stay in `volna-core`; this module hosts the controls and
//! the clipboard integration. The selected record is read in the Transaction
//! panel, which every selecting panel shares.

use crate::dock::CanvasPanelView;
use crate::theme::theme;
#[cfg(not(target_arch = "wasm32"))]
use gpui_kit::ClipboardItem;
use gpui_kit::component::{
    Disableable, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Empty, Focusable, Window, div, px};
use volna_core::Command;
use volna_core::icons::IconName;
use volna_core::table::columns::{ColumnSet, TransactionColumn};
use volna_core::table::{TableCommand, TableState};

impl CanvasPanelView {
    fn table_command(&self, command: TableCommand, window: &mut Window, cx: &mut Context<Self>) {
        _ = self.ws.update(cx, |ws, cx| {
            ws.dispatch_if_current(
                self.generation,
                Command::Table(self.id, command),
                Some(window),
                cx,
            )
        });
    }

    pub(crate) fn render_table(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(owner) = self.ws.upgrade() else {
            return Empty.into_any_element();
        };
        let Some(table) = owner
            .read(cx)
            .app
            .panels
            .get(self.id)
            .and_then(|panel| panel.kind.table())
        else {
            return Empty.into_any_element();
        };
        let t = *theme(cx);
        let has_selection = table.selected.is_some();
        let failed = matches!(table.state, TableState::Failed(_) | TableState::Refused(_));
        let loading = matches!(table.state, TableState::Loading);
        let column_items: Vec<(String, bool, TableCommand)> = match &table.columns {
            ColumnSet::Transactions(visible) => TransactionColumn::ALL
                .iter()
                .map(|&column| {
                    (
                        column.title().into(),
                        visible.contains(&column),
                        TableCommand::ToggleTransactionColumn(column),
                    )
                })
                .collect(),
            ColumnSet::Signals { time, visible } => {
                std::iter::once(("Time".into(), *time, TableCommand::ToggleSignalColumn(0)))
                    .chain(visible.iter().enumerate().map(|(index, &visible)| {
                        (
                            table.column_title(index + 1).to_owned(),
                            visible,
                            TableCommand::ToggleSignalColumn(index + 1),
                        )
                    }))
                    .collect()
            }
        };
        let id = self.id;
        let generation = self.generation;
        let columns_owner = owner.clone();
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(32.0 * t.zoom))
            .flex_none();
        for (name, icon, tooltip, command) in [
            (
                "table-first",
                IconName::TableFirst,
                "First row (Home)",
                TableCommand::First,
            ),
            (
                "table-previous",
                IconName::TablePrevious,
                "Previous row (Arrow Up)",
                TableCommand::Previous,
            ),
            (
                "table-next",
                IconName::TableNext,
                "Next row (Arrow Down)",
                TableCommand::Next,
            ),
            (
                "table-last",
                IconName::TableLast,
                "Last row (End)",
                TableCommand::Last,
            ),
        ] {
            let owner = owner.clone();
            toolbar = toolbar.child(
                Button::new(name)
                    .icon(gpui_kit::component::Icon::empty().path(icon.path()))
                    .tooltip(tooltip)
                    .ghost()
                    .small()
                    .on_click(move |_, window, cx| {
                        owner.update(cx, |ws, cx| {
                            ws.dispatch_if_current(
                                generation,
                                Command::Table(id, command.clone()),
                                Some(window),
                                cx,
                            )
                        })
                    }),
            );
        }
        toolbar = toolbar
            .child(
                Button::new("table-go-to")
                    .label("Go to…")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|view, _, window, cx| view.table_go_to(window, cx))),
            )
            .child(
                Button::new("table-columns")
                    .icon(gpui_kit::component::Icon::empty().path(IconName::TableColumns.path()))
                    .label("Columns")
                    .ghost()
                    .small()
                    .dropdown_menu_with_anchor(
                        gpui_kit::Anchor::TopLeft,
                        move |menu: PopupMenu, _, _| {
                            let menu = column_items.iter().fold(
                                menu,
                                |menu, (label, checked, command)| {
                                    let owner = columns_owner.clone();
                                    let command = command.clone();
                                    menu.item(
                                        PopupMenuItem::new(label.clone())
                                            .checked(*checked)
                                            .on_click(move |_, window, cx| {
                                                owner.update(cx, |ws, cx| {
                                                    ws.dispatch_if_current(
                                                        generation,
                                                        Command::Table(id, command.clone()),
                                                        Some(window),
                                                        cx,
                                                    )
                                                })
                                            }),
                                    )
                                },
                            );
                            let owner = columns_owner.clone();
                            menu.separator()
                                .item(PopupMenuItem::new("Reset to defaults").on_click(
                                    move |_, window, cx| {
                                        owner.update(cx, |ws, cx| {
                                            ws.dispatch_if_current(
                                                generation,
                                                Command::Table(id, TableCommand::ResetColumns),
                                                Some(window),
                                                cx,
                                            )
                                        })
                                    },
                                ))
                        },
                    ),
            )
            .child(
                Button::new("table-details")
                    .icon(gpui_kit::component::Icon::empty().path(IconName::TableDetails.path()))
                    .label("Details")
                    .ghost()
                    .small()
                    .disabled(!has_selection)
                    .tooltip("Show this record in a Transaction panel (Enter)")
                    .on_click(cx.listener(|view, _, window, cx| view.show_transaction(window, cx))),
            );
        if failed || loading {
            let command = if loading {
                TableCommand::Cancel
            } else {
                TableCommand::Retry
            };
            toolbar = toolbar.child(
                Button::new("table-retry")
                    .label(if loading { "Cancel" } else { "Retry" })
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.table_command(command.clone(), window, cx)
                    })),
            );
        }

        let canvas = div().flex_1().min_h_0().overflow_hidden().child(
            crate::canvas::PanelCanvas::new(owner.clone(), id, generation)
                .table(self.focus.clone()),
        );
        let body = div().flex().flex_1().min_h_0().child(canvas);

        div()
            .id(("table-panel", id.0))
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .key_context("Waves Table")
            .on_key_down(
                cx.listener(|view, event: &gpui_kit::KeyDownEvent, window, cx| {
                    let key = event.keystroke.key.as_str();
                    let modifiers = crate::app::to_modifiers(event.keystroke.modifiers);
                    let command = match key {
                        "home" => Some(TableCommand::First),
                        "end" => Some(TableCommand::Last),
                        "up" => Some(TableCommand::Previous),
                        "down" => Some(TableCommand::Next),
                        "pageup" => Some(TableCommand::Page(-1)),
                        "pagedown" => Some(TableCommand::Page(1)),
                        "escape" => Some(TableCommand::ClearSelection),
                        _ => None,
                    };
                    if key == "enter" {
                        view.show_transaction(window, cx);
                        cx.stop_propagation();
                    } else if let Some(command) = command {
                        view.table_command(command, window, cx);
                        cx.stop_propagation();
                    } else if key.eq_ignore_ascii_case("c") && modifiers.secondary() {
                        view.copy_table_row(cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(toolbar)
            .child(body)
            .into_any_element()
    }

    /// Enter, the Details button and a double-click all open the record in a
    /// Transaction panel: the first one that is not pinned, or a new one.
    fn show_transaction(&self, window: &mut Window, cx: &mut Context<Self>) {
        _ = self.ws.update(cx, |ws, cx| {
            ws.dispatch_if_current(
                self.generation,
                Command::ShowTransaction { from: self.id },
                Some(window),
                cx,
            )
        });
    }

    fn copy_table_row(&self, cx: &mut Context<Self>) {
        let Some(owner) = self.ws.upgrade() else {
            return;
        };
        match owner
            .read(cx)
            .app
            .panels
            .get(self.id)
            .and_then(|panel| panel.kind.table())
            .map(|table| table.copy_tsv())
        {
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

    fn table_go_to(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.ws.upgrade() else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("1-based row number"));
        let input_focus = input.read(cx).focus_handle(cx);
        let id = self.id;
        let generation = self.generation;
        let return_focus = self.focus.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let owner = owner.clone();
            let input = input.clone();
            let return_focus = return_focus.clone();
            dialog
                .title("Go to row")
                .child(Input::new(&input))
                .on_ok(move |_, window, cx| {
                    let value = input.read(cx).value().trim().parse::<u64>();
                    match value.ok().and_then(|row| row.checked_sub(1)) {
                        Some(row) => owner.update(cx, |ws, cx| {
                            let in_range = ws
                                .app
                                .panels
                                .get(id)
                                .and_then(|panel| panel.kind.table())
                                .is_some_and(|table| row < table.len());
                            let command = if in_range {
                                Command::Table(id, TableCommand::GoTo(row))
                            } else {
                                Command::Notice("Choose a row within this table.".into())
                            };
                            ws.dispatch_if_current(generation, command, Some(window), cx)
                        }),
                        None => owner.update(cx, |ws, cx| {
                            ws.dispatch_if_current(
                                generation,
                                Command::Notice("Choose a whole row number starting at 1.".into()),
                                Some(window),
                                cx,
                            )
                        }),
                    }
                    window.focus(&return_focus, cx);
                    true
                })
        });
        window.focus(&input_focus, cx);
    }
}
