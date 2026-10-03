//! Several traces in one window (`docs/multiple-traces.html`, stage 1): the
//! trace chips in the title bar and their menu, a trace's name field, and
//! adding traces by dialog, command line and drop. What a chip says and
//! what every choice does come from the core (`App::trace_chips`,
//! `Command::AddTrace` and its siblings).

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, CursorStyle, Entity, Focusable, IntoElement, MouseButton, Pixels, Point,
    SharedString, Subscription, Window, div, px,
};
use volna_core::app::{Command, TraceChip};
use volna_core::trace::TraceId;

use crate::app::Workspace;
use crate::theme::{ThemePx, core_theme, hsla, theme};
use crate::ui::TextInput;
use crate::ui::text_input::TextInputEvent;

/// The name field of a trace being renamed, hosted in its chip.
pub(crate) struct TraceRename {
    pub trace: TraceId,
    pub input: Entity<TextInput>,
    _subscription: Subscription,
}

/// A trace's letter on its colour: the same badge the wave rows' gutter
/// paints, for chips and the scope tree.
pub(crate) fn letter_badge(trace: TraceId, cx: &gpui_kit::App) -> impl IntoElement {
    let t = theme(cx);
    let colors = core_theme(cx).trace(trace);
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(t.px(16.0))
        .h(t.px(16.0))
        .rounded(t.px(3.0))
        .bg(hsla(colors.background))
        .text_color(hsla(colors.text))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_size(px(t.ui_size_small))
        .child(SharedString::from(trace.letter().to_string()))
}

impl Workspace {
    /// The traces as chips, each naming its letter, name, format and unit;
    /// a click selects its row in the scope tree, a right click opens its
    /// menu. Only while several traces are open: one trace is named plainly.
    pub(crate) fn render_trace_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let chips = self.app.trace_chips();
        if chips.len() < 2 {
            return None;
        }
        let row = div()
            .id("trace-chips")
            .flex()
            .min_w_0()
            .items_center()
            .gap_1()
            .children(chips.iter().map(|chip| self.trace_chip(chip, cx)));
        Some(row.into_any_element())
    }

    fn trace_chip(&self, chip: &TraceChip, cx: &mut Context<Self>) -> AnyElement {
        let t = *theme(cx);
        let trace = chip.trace;
        let renaming = self.trace_rename.as_ref().filter(|r| r.trace == trace);
        let tooltip = chip.tooltip();
        let name: SharedString = chip.name.clone().into();
        let detail: SharedString = chip.detail().into();
        let body = match renaming {
            Some(rename) => div()
                .w(t.px(140.0))
                .h(t.px(20.0))
                .child(rename.input.clone())
                .into_any_element(),
            None => div()
                .flex()
                .items_center()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .max_w(t.px(180.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(t.bar.text)
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(t.ui_size_small))
                        .text_color(t.bar.text_muted)
                        .child(detail),
                )
                .into_any_element(),
        };
        div()
            .id(("trace-chip", trace.index()))
            .debug_selector(move || format!("trace-chip-{trace}"))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(t.px(22.0))
            .pl(t.px(3.0))
            .pr(t.px(8.0))
            .rounded(t.px(4.0))
            .border_1()
            .border_color(t.border_variant)
            .bg(t.badge.bg)
            .cursor(CursorStyle::PointingHand)
            .hover(move |s| s.bg(t.badge_hover.bg))
            .when(chip.loading, |el| el.opacity(0.6))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(
                cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                    if ev.click_count() == 2 {
                        this.start_trace_rename(trace, window, cx);
                    } else {
                        this.dispatch(Command::RevealTrace(trace), Some(window), cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_trace_menu(trace, ev.position, window, cx);
                }),
            )
            .tooltip(move |w, cx| Tooltip::new(tooltip.clone()).build(w, cx))
            .child(letter_badge(trace, cx))
            .child(body)
            .into_any_element()
    }

    /// A trace's menu, from its chip or its row in the scope tree: rename,
    /// close it, or add another.
    pub(crate) fn open_trace_menu(
        &mut self,
        trace: TraceId,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chip) = self.app.trace_chip(trace) else {
            return;
        };
        let workspace = cx.entity().downgrade();
        let run = move |f: fn(&mut Workspace, TraceId, &mut Window, &mut Context<Workspace>)| {
            let workspace = workspace.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                workspace
                    .update(cx, |this, cx| {
                        this.status_menu = None;
                        f(this, trace, window, cx);
                    })
                    .ok();
            }
        };
        let focus = self.focus_handle.clone();
        let min_w = theme(cx).px(200.0);
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let close = if chip.closable {
                PopupMenuItem::new(format!("Close Trace {trace}")).on_click(run(
                    |this, trace, w, cx| this.dispatch(Command::RemoveTrace(trace), Some(w), cx),
                ))
            } else {
                PopupMenuItem::new("Close All Traces").on_click(run(|this, _, w, cx| {
                    this.dispatch(Command::CloseTrace, Some(w), cx)
                }))
            };
            let mut menu =
                menu.item(PopupMenuItem::label(chip.tooltip()))
                    .separator()
                    .item(PopupMenuItem::new("Rename Trace…").on_click(run(
                        |this, trace, w, cx| this.start_trace_rename(trace, w, cx),
                    )));
            if chip.renamed {
                menu = menu.item(PopupMenuItem::new("Use File Name").on_click(run(
                    |this, trace, w, cx| {
                        this.dispatch(Command::RenameTrace(trace, None), Some(w), cx)
                    },
                )));
            }
            menu.item(close)
                .separator()
                .item(
                    PopupMenuItem::new("Add Trace…").on_click(run(|this, _, w, cx| {
                        this.dispatch(Command::RequestAddTraceDialog, Some(w), cx)
                    })),
                )
                .min_w(min_w)
                .action_context(focus.clone())
        });
        cx.subscribe(&menu, move |this, _, _: &gpui_kit::DismissEvent, cx| {
            this.status_menu = None;
            cx.notify();
        })
        .detach();
        self.status_menu = Some((position, menu));
        cx.notify();
    }

    /// Open the name field in a trace's chip; Enter renames the trace (an
    /// undoable step), Escape keeps its name.
    pub(crate) fn start_trace_rename(
        &mut self,
        trace: TraceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chip) = self.app.trace_chip(trace) else {
            return;
        };
        let input = cx.new(|cx| {
            let mut input = TextInput::new(format!("Name of trace {trace}"), cx).plain();
            input.set_text(chip.name.clone(), cx);
            input.select_all(cx);
            input
        });
        let generation = self.app.doc.generation();
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &TextInputEvent, window, cx| {
                let name = match event {
                    TextInputEvent::Submit => Some(input.read(cx).text().to_owned()),
                    TextInputEvent::Cancel => None,
                    TextInputEvent::Changed => return,
                };
                this.trace_rename = None;
                if let Some(name) = name {
                    this.dispatch_if_current(
                        generation,
                        Command::RenameTrace(trace, Some(name)),
                        Some(window),
                        cx,
                    );
                }
                window.focus(&this.focus_handle, cx);
                cx.notify();
            },
        );
        window.focus(&input.read(cx).focus_handle(cx), cx);
        self.trace_rename = Some(TraceRename {
            trace,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Add a trace from disk beside the open ones (Add Trace…, the command
    /// line, a drop). With nothing open it is an ordinary open.
    #[cfg(not(target_family = "wasm"))]
    pub fn add_path(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if !self.app.doc.is_loaded() && self.app.doc.traces().is_empty() {
            self.open_path(path, cx);
            return;
        }
        match path
            .canonicalize()
            .map_err(anyhow::Error::from)
            .and_then(|path| Ok((crate::native_workspace::file_uri(&path)?, path)))
        {
            Ok((uri, path)) => self
                .app
                .add_resource(volna_trace::session::OpenSpec::Path(path), uri),
            Err(_) => self
                .app
                .handle(Command::AddTrace(volna_trace::session::OpenSpec::Path(
                    path,
                ))),
        }
        self.after(None, cx);
    }

    /// Open the first path and add the others: `volna A.vtr B.fst`.
    #[cfg(not(target_family = "wasm"))]
    pub fn open_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        let mut paths = paths.into_iter();
        if let Some(first) = paths.next() {
            self.open_path(first, cx);
        }
        for path in paths {
            self.add_path(path, cx);
        }
    }

    /// Ask for traces to add beside the open ones.
    pub(crate) fn add_trace_dialog(&mut self, cx: &mut Context<Self>) {
        #[cfg(not(target_family = "wasm"))]
        {
            let rx = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: Some("Add".into()),
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(paths))) = rx.await {
                    this.update(cx, |this, cx| {
                        for path in paths {
                            this.add_path(path, cx);
                        }
                    })
                    .ok();
                }
            })
            .detach();
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = cx;
            self.app.handle(Command::Notice(
                "Adding a trace is not available in this host yet.".into(),
            ));
        }
    }

    /// Files dropped on the window: with nothing open they open (the first)
    /// and join it (the rest); over open traces a menu at the drop point
    /// offers to replace them or to add the files beside them.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn drop_paths(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let workspaces = paths
            .iter()
            .any(|p| p.to_string_lossy().ends_with(".volna.json"));
        if self.app.doc.traces().is_empty() || workspaces {
            self.open_paths(paths, cx);
            return;
        }
        let name = |p: &std::path::Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let what = match paths.as_slice() {
            [one] => name(one),
            many => format!("{} traces", many.len()),
        };
        let add_label = match (paths.len(), self.app.doc.traces().free_id()) {
            (1, Some(letter)) => format!("Add {what} as {letter}"),
            _ => format!("Add {what}"),
        };
        let workspace = cx.entity().downgrade();
        let choose = move |add: bool, paths: Vec<std::path::PathBuf>| {
            let workspace = workspace.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| {
                let paths = paths.clone();
                workspace
                    .update(cx, |this, cx| {
                        this.status_menu = None;
                        if add {
                            for path in paths {
                                this.add_path(path, cx);
                            }
                        } else {
                            this.open_paths(paths, cx);
                        }
                    })
                    .ok();
            }
        };
        let focus = self.focus_handle.clone();
        let min_w = theme(cx).px(200.0);
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(PopupMenuItem::new(add_label.clone()).on_click(choose(true, paths.clone())))
                .item(
                    PopupMenuItem::new(format!("Open {what} Instead"))
                        .on_click(choose(false, paths.clone())),
                )
                .min_w(min_w)
                .action_context(focus.clone())
        });
        cx.subscribe(&menu, move |this, _, _: &gpui_kit::DismissEvent, cx| {
            this.status_menu = None;
            cx.notify();
        })
        .detach();
        self.status_menu = Some((position, menu));
        cx.notify();
    }
}
