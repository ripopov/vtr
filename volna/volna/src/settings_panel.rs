//! The Settings tab: a dock panel that renders the registry through gpui-kit's
//! `Settings` pages, Volna's own search bar and ranked results view, modified
//! markers and per-item actions, and the JSON view. Every decision (values,
//! ranking, edits, diagnostics) comes from the core; this file only draws.

use gpui_kit::base::dock as base;
use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
    dock::Panel,
    input::{Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction},
    label::Label,
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    setting::{SettingGroup, SettingItem, SettingPage, Settings},
    switch::Switch,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Display, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, Render, SharedString, StyleRefinement, Subscription, WeakEntity, Window, div, px,
};
use volna_core::app::SettingsCommand;
use volna_core::panels::PanelId;
use volna_core::settings::{self, Host, Kind, Page, REGISTRY, Spec, Value, search::Hit};
use volna_core::{App as CoreApp, Command};

use crate::app::Workspace;
use crate::theme::theme;
use crate::ui::{Icon, IconName};

gpui_kit::actions!(
    settings,
    [
        SettingsEscape,
        ToggleSettingsJson,
        ApplySettingsJson,
        FocusSettingsSearch
    ]
);

pub(crate) struct SettingsPanelView {
    ws: WeakEntity<Workspace>,
    id: PanelId,
    focus: FocusHandle,
    search: Entity<InputState>,
    /// Mirrors the core query; set when the core changed it (palette reveal).
    synced_query: String,
    pub(crate) json: Option<crate::settings_json::JsonView>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPanelView {
    pub(crate) fn new(
        ws: WeakEntity<Workspace>,
        id: PanelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search settings (@modified, @page:waves, @id:waves.snap)")
        });
        let subscription = cx.subscribe_in(&search, window, |this, input, event, window, cx| {
            if let InputEvent::Change = event {
                let text = input.read(cx).value().to_string();
                this.synced_query = text.clone();
                this.dispatch(SettingsCommand::Query(text), window, cx);
            }
        });
        Self {
            ws,
            id,
            focus: cx.focus_handle(),
            search,
            synced_query: String::new(),
            json: None,
            _subscriptions: vec![subscription],
        }
    }

    pub(crate) fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.search.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn dispatch(&self, command: SettingsCommand, window: &mut Window, cx: &mut Context<Self>) {
        _ = self.ws.update(cx, |ws, cx| {
            ws.dispatch(Command::Settings(command), Some(window), cx)
        });
    }

    /// Keep the search box in step with the core's query.
    fn sync_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let query = ws.read(cx).app.settings_view.query.clone();
        if query != self.synced_query {
            self.synced_query = query.clone();
            self.search
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
    }

    fn escape(&mut self, _: &SettingsEscape, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let view = ws.read(cx).app.settings_view.clone();
        if view.json {
            self.dispatch(SettingsCommand::ToggleJson, window, cx);
        } else if !view.query.is_empty() {
            self.dispatch(SettingsCommand::Query(String::new()), window, cx);
        } else {
            self.dispatch(SettingsCommand::Close, window, cx);
        }
    }

    fn toggle_json(&mut self, _: &ToggleSettingsJson, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(SettingsCommand::ToggleJson, window, cx);
    }

    pub(crate) fn apply_json(
        &mut self,
        _: &ApplySettingsJson,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(json) = &self.json {
            let text = json.text(cx);
            self.dispatch(SettingsCommand::ReplaceText(text), window, cx);
        }
    }

    fn render_bar(&self, ws: &Entity<Workspace>, cx: &mut Context<Self>) -> AnyElement {
        let t = *theme(cx);
        let app = &ws.read(cx).app;
        let view = app.settings_view.clone();
        let host = app.settings.host();
        let modified = app.settings.modified().len();
        let hits = if view.query.trim().is_empty() {
            None
        } else {
            Some(hits(app).len())
        };
        let query_has_modified = view
            .query
            .split_whitespace()
            .any(|t| t.eq_ignore_ascii_case("@modified"));
        let available = REGISTRY.iter().filter(|s| s.available(host)).count();
        let count: SharedString = match hits {
            Some(n) => format!("{n} of {available}").into(),
            None => format!("{available} settings").into(),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(t.border)
            .bg(t.panel.bg)
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.search)
                        .prefix(Icon::new(IconName::Search).size(px(14.0)))
                        .small(),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(t.ui_size_small))
                    .text_color(t.panel.text_muted)
                    .child(count),
            )
            .child(
                Button::new("settings-modified")
                    .label(format!("@modified ({modified})"))
                    .ghost()
                    .xsmall()
                    .selected(query_has_modified)
                    .tooltip("Show only settings set in settings.json")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let query = if query_has_modified {
                            view.query
                                .split_whitespace()
                                .filter(|t| !t.eq_ignore_ascii_case("@modified"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        } else if view.query.trim().is_empty() {
                            "@modified".to_owned()
                        } else {
                            format!("@modified {}", view.query.trim())
                        };
                        this.dispatch(SettingsCommand::Query(query), window, cx);
                    })),
            )
            .child(
                Button::new("settings-json")
                    .label(if view.json {
                        "Back to settings"
                    } else {
                        "Open settings.json"
                    })
                    .icon(gpui_kit::component::Icon::empty().path(IconName::Braces.path()))
                    .outline()
                    .xsmall()
                    .tooltip("Toggle the JSON view (⌘⇧,)")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dispatch(SettingsCommand::ToggleJson, window, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_pages(&self, ws: &Entity<Workspace>, cx: &mut Context<Self>) -> AnyElement {
        let app = &ws.read(cx).app;
        let host = app.settings.host();
        let weak = self.ws.clone();
        let mut pages = Vec::new();
        for page in Page::ALL {
            let specs: Vec<&'static Spec> = REGISTRY
                .iter()
                .filter(|s| s.page == *page && s.available(host))
                .collect();
            if specs.is_empty() {
                continue;
            }
            let mut groups: Vec<(&'static str, Vec<&'static Spec>)> = Vec::new();
            for spec in specs {
                match groups.iter_mut().find(|(name, _)| *name == spec.group) {
                    Some((_, items)) => items.push(spec),
                    None => groups.push((spec.group, vec![spec])),
                }
            }
            let mut setting_page = SettingPage::new(page.title())
                .icon(gpui_kit::component::Icon::empty().path(page.icon().path()))
                .description(page.description());
            for (name, specs) in groups {
                let mut group = SettingGroup::new().title(name);
                for spec in specs {
                    let ws = weak.clone();
                    let reset_ws = weak.clone();
                    let dirty_ws = weak.clone();
                    group = group.item(
                        SettingItem::render(move |_, window, cx| {
                            render_row(&ws, spec, None, window, cx)
                        })
                        .keywords(spec.keywords.iter().copied())
                        .on_reset(
                            move |cx| {
                                dirty_ws
                                    .upgrade()
                                    .is_some_and(|ws| ws.read(cx).app.settings.is_modified(spec.id))
                            },
                            move |window, cx| {
                                _ = reset_ws.update(cx, |ws, cx| {
                                    ws.dispatch(
                                        Command::Settings(SettingsCommand::Reset {
                                            id: spec.id.into(),
                                        }),
                                        Some(window),
                                        cx,
                                    )
                                });
                            },
                        ),
                    );
                }
                setting_page = setting_page.group(group);
            }
            pages.push(setting_page);
        }
        // Volna's own search bar replaces the component's input.
        let hidden = StyleRefinement {
            display: Some(Display::None),
            ..Default::default()
        };
        Settings::new("volna-settings")
            .sidebar_width(px(200.0))
            .header_style(&hidden)
            .pages(pages)
            .into_any_element()
    }

    fn render_results(
        &self,
        ws: &Entity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *theme(cx);
        let app = &ws.read(cx).app;
        let hits = hits(app);
        let weak = self.ws.clone();
        if hits.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.panel.text_muted)
                .child("No settings match")
                .into_any_element();
        }
        let rows: Vec<AnyElement> = hits
            .iter()
            .enumerate()
            .map(|(ix, hit)| {
                div()
                    .id(("settings-hit", ix))
                    .w_full()
                    .py_3()
                    .border_b_1()
                    .border_color(t.border)
                    .child(render_row(&weak, hit.spec, Some(hit), window, cx))
                    .into_any_element()
            })
            .collect();
        div()
            .id("settings-results")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .children(rows)
            .into_any_element()
    }
}

/// Ranked hits for the current query on this host.
fn hits(app: &CoreApp) -> Vec<Hit> {
    let store = &app.settings;
    settings::search(&app.settings_view.query, store.host(), &|id| {
        store.is_modified(id)
    })
}

/// One setting row: modified bar, title with highlights, description, the
/// reason a search matched, the apply badge, the control and the actions menu.
fn render_row(
    ws: &WeakEntity<Workspace>,
    spec: &'static Spec,
    hit: Option<&Hit>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(entity) = ws.upgrade() else {
        return div().into_any_element();
    };
    let t = *theme(cx);
    let app = &entity.read(cx).app;
    let store = &app.settings;
    let modified = store.is_modified(spec.id);
    let overridden = store.is_overridden(spec.id);
    let editable = store.editable();
    let value = store
        .value(spec.id)
        .cloned()
        .unwrap_or_else(|| spec.default.value());
    let themes = store.themes().to_vec();
    let host = store.host();
    let mut title = Label::new(spec.title)
        .text_sm()
        .font_weight(gpui_kit::FontWeight::MEDIUM);
    if let Some(hit) = hit
        && !hit.title_ranges.is_empty()
    {
        title = title.highlights(highlight_text(spec.title, &hit.title_ranges));
    }
    let why: Option<SharedString> = hit.and_then(|hit| match &hit.matched {
        settings::Matched::Title => None,
        settings::Matched::Id => Some(spec.id.into()),
        settings::Matched::Keyword(kw) => Some(format!("keyword: {kw}").into()),
        settings::Matched::Description => Some("matched the description".into()),
    });
    let page_label: Option<SharedString> =
        hit.map(|_| format!("{} · {}", spec.page.title(), spec.group).into());
    let control = render_control(
        ws,
        spec,
        value.clone(),
        editable && !overridden,
        &themes,
        host,
        window,
        cx,
    );
    div()
        .flex()
        .w_full()
        .gap_3()
        .child(div().flex_none().w(px(3.0)).rounded_sm().bg(if modified {
            t.panel.icon_accent
        } else {
            gpui_kit::transparent_black()
        }))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .justify_between()
                .items_start()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .when_some(page_label, |el, label| {
                            el.child(
                                div()
                                    .text_size(px(t.ui_size_small))
                                    .text_color(t.panel.text_placeholder)
                                    .child(label),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(title)
                                .when_some(spec.apply.badge(), |el, badge| {
                                    el.child(
                                        div()
                                            .px_1p5()
                                            .rounded_sm()
                                            .bg(t.badge_hover.bg)
                                            .text_size(px(t.ui_size_small))
                                            .text_color(t.badge_hover.text)
                                            .child(badge),
                                    )
                                })
                                .when(overridden, |el| {
                                    el.child(
                                        div()
                                            .px_1p5()
                                            .rounded_sm()
                                            .bg(t.badge_hover.bg)
                                            .text_size(px(t.ui_size_small))
                                            .text_color(t.badge_hover.text)
                                            .child("set by the host"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(t.ui_size_small))
                                .text_color(t.panel.text_muted)
                                .child(SharedString::from(strip_markdown(spec.description))),
                        )
                        .when_some(why, |el, why| {
                            el.child(
                                div()
                                    .text_size(px(t.ui_size_small))
                                    .font_family(t.mono_font)
                                    .text_color(t.panel.text_placeholder)
                                    .child(why),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(control)
                        .child(render_actions(ws, spec, value, modified, editable)),
                ),
        )
        .into_any_element()
}

/// The text of the highlighted ranges, for `Label::highlights`.
fn highlight_text(text: &str, ranges: &[std::ops::Range<usize>]) -> String {
    ranges
        .iter()
        .filter_map(|r| text.get(r.clone()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Inline code marks are dropped; the row renders plain text.
fn strip_markdown(text: &str) -> String {
    text.replace('`', "")
}

fn set(
    ws: &WeakEntity<Workspace>,
    id: &'static str,
    value: Value,
    window: &mut Window,
    cx: &mut App,
) {
    _ = ws.update(cx, |ws, cx| {
        ws.dispatch(
            Command::Settings(SettingsCommand::Set {
                id: id.into(),
                value,
            }),
            Some(window),
            cx,
        )
    });
}

struct NumberState {
    input: Entity<InputState>,
    shown: i64,
    _subscriptions: Vec<Subscription>,
}

struct TextState {
    input: Entity<InputState>,
    shown: String,
    _subscriptions: Vec<Subscription>,
}

#[allow(clippy::too_many_arguments)]
fn render_control(
    ws: &WeakEntity<Workspace>,
    spec: &'static Spec,
    value: Value,
    enabled: bool,
    themes: &[String],
    host: Host,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = spec.id;
    match spec.kind {
        Kind::Bool => {
            let ws = ws.clone();
            Switch::new(SharedString::from(format!("setting-{id}")))
                .checked(value.as_bool().unwrap_or(false))
                .disabled(!enabled)
                .on_click(move |checked, window, cx| {
                    set(&ws, id, Value::Bool(*checked), window, cx);
                })
                .into_any_element()
        }
        Kind::Integer { min, max, step } => {
            let current = value.as_i64().unwrap_or_default();
            let state = window.use_keyed_state(
                SharedString::from(format!("setting-number-{id}")),
                cx,
                |window, cx| {
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(current.to_string())
                            .min(min as f64)
                            .max(max as f64)
                            .step(step as f64)
                    });
                    let ws_step = ws.clone();
                    let ws_change = ws.clone();
                    let subscriptions = vec![
                        cx.subscribe_in(
                            &input,
                            window,
                            move |state: &mut NumberState,
                                  input,
                                  event: &NumberInputEvent,
                                  window,
                                  cx| {
                                let NumberInputEvent::Step(action) = event;
                                let Ok(shown) = input.read(cx).value().parse::<i64>() else {
                                    return;
                                };
                                let next = match action {
                                    StepAction::Increment => shown + step,
                                    StepAction::Decrement => shown - step,
                                }
                                .clamp(min, max);
                                state.shown = next;
                                input.update(cx, |input, cx| {
                                    input.set_value(next.to_string(), window, cx)
                                });
                                set(&ws_step, id, Value::Integer(next), window, cx);
                            },
                        ),
                        cx.subscribe_in(
                            &input,
                            window,
                            move |state: &mut NumberState,
                                  input,
                                  event: &InputEvent,
                                  window,
                                  cx| {
                                match event {
                                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                                        let text = input.read(cx).value();
                                        let Ok(parsed) = text.trim().parse::<i64>() else {
                                            let shown = state.shown;
                                            input.update(cx, |input, cx| {
                                                input.set_value(shown.to_string(), window, cx)
                                            });
                                            return;
                                        };
                                        let next = parsed.clamp(min, max);
                                        if next != parsed {
                                            input.update(cx, |input, cx| {
                                                input.set_value(next.to_string(), window, cx)
                                            });
                                        }
                                        if next != state.shown {
                                            state.shown = next;
                                            set(&ws_change, id, Value::Integer(next), window, cx);
                                        }
                                    }
                                    _ => {}
                                }
                            },
                        ),
                    ];
                    NumberState {
                        input,
                        shown: current,
                        _subscriptions: subscriptions,
                    }
                },
            );
            state.update(cx, |state, cx| {
                if state.shown != current {
                    state.shown = current;
                    let input = state.input.clone();
                    input.update(cx, |input, cx| {
                        input.set_value(current.to_string(), window, cx)
                    });
                }
            });
            let input = state.read(cx).input.clone();
            NumberInput::new(&input)
                .small()
                .disabled(!enabled)
                .w(px(120.0))
                .into_any_element()
        }
        Kind::Number { .. } => {
            // No number-kind setting is registered yet; render the value read-only.
            div()
                .child(SharedString::from(value.to_json_text()))
                .into_any_element()
        }
        Kind::Text => {
            let current = value.as_str().unwrap_or_default().to_owned();
            let state = window.use_keyed_state(
                SharedString::from(format!("setting-text-{id}")),
                cx,
                |window, cx| {
                    let input =
                        cx.new(|cx| InputState::new(window, cx).default_value(current.clone()));
                    let ws = ws.clone();
                    let subscriptions = vec![cx.subscribe_in(
                        &input,
                        window,
                        move |state: &mut TextState, input, event: &InputEvent, window, cx| {
                            if let InputEvent::PressEnter { .. } | InputEvent::Blur = event {
                                let text = input.read(cx).value().to_string();
                                if text != state.shown {
                                    state.shown = text.clone();
                                    set(&ws, id, Value::Text(text), window, cx);
                                }
                            }
                        },
                    )];
                    TextState {
                        input,
                        shown: current.clone(),
                        _subscriptions: subscriptions,
                    }
                },
            );
            state.update(cx, |state, cx| {
                if state.shown != current {
                    state.shown = current.clone();
                    let input = state.input.clone();
                    input.update(cx, |input, cx| input.set_value(current.clone(), window, cx));
                }
            });
            let input = state.read(cx).input.clone();
            Input::new(&input)
                .small()
                .disabled(!enabled)
                .w(px(220.0))
                .into_any_element()
        }
        Kind::Enum(_) | Kind::Theme => {
            let choices: Vec<(String, String)> = match spec.kind {
                Kind::Theme => std::iter::once(("one-dark".to_owned(), "One Dark".to_owned()))
                    .chain(themes.iter().map(|t| (t.clone(), format!("{t} (palette)"))))
                    .collect(),
                _ => spec
                    .choices(host)
                    .into_iter()
                    .map(|c| (c.value.to_owned(), c.label.to_owned()))
                    .collect(),
            };
            let current = value.as_str().unwrap_or_default().to_owned();
            let label = choices
                .iter()
                .find(|(v, _)| *v == current)
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| current.clone());
            let ws = ws.clone();
            Button::new(SharedString::from(format!("setting-enum-{id}")))
                .label(label)
                .dropdown_caret(true)
                .outline()
                .small()
                .disabled(!enabled)
                .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    choices.iter().fold(menu, |menu, (value, label)| {
                        let ws = ws.clone();
                        let value = value.clone();
                        let checked = value == current;
                        menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                            move |_, window, cx| {
                                set(&ws, id, Value::Text(value.clone()), window, cx);
                            },
                        ))
                    })
                })
                .into_any_element()
        }
    }
}

fn render_actions(
    ws: &WeakEntity<Workspace>,
    spec: &'static Spec,
    value: Value,
    modified: bool,
    editable: bool,
) -> AnyElement {
    let ws = ws.clone();
    let id = spec.id;
    let json = format!("\"{}\": {}", id, value.to_json_text());
    Button::new(SharedString::from(format!("setting-actions-{id}")))
        .icon(gpui_kit::assets::IconName::Ellipsis)
        .ghost()
        .xsmall()
        .tooltip("Reset, copy id, copy as JSON")
        .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu: PopupMenu, _, _| {
            let ws_reset = ws.clone();
            let json = json.clone();
            menu.item(
                PopupMenuItem::new("Reset setting")
                    .disabled(!modified || !editable)
                    .on_click(move |_, window, cx| {
                        _ = ws_reset.update(cx, |ws, cx| {
                            ws.dispatch(
                                Command::Settings(SettingsCommand::Reset { id: id.into() }),
                                Some(window),
                                cx,
                            )
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Copy setting id").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(id.to_owned()));
                }),
            )
            .item(
                PopupMenuItem::new("Copy setting as JSON").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(json.clone()));
                }),
            )
        })
        .into_any_element()
}

impl EventEmitter<base::PanelEvent> for SettingsPanelView {}
impl Focusable for SettingsPanelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl base::Panel for SettingsPanelView {
    fn panel_name(&self) -> &'static str {
        "volna.settings"
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn dump(&self, _: &App) -> base::PanelState {
        let mut state = base::PanelState::new("volna.settings");
        state.info = base::PanelInfo::panel(serde_json::json!({"id": self.id.0}));
        state
    }
}
impl Panel for SettingsPanelView {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Settings".into())
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Settings"
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("close-settings")
                .icon(gpui_kit::assets::IconName::X)
                .ghost()
                .xsmall()
                .tooltip("Close settings")
                .on_click(cx.listener(|view, _, window, cx| {
                    view.dispatch(SettingsCommand::Close, window, cx)
                })),
        ])
    }
}

impl Render for SettingsPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        self.sync_search(window, cx);
        let t = *theme(cx);
        let view = ws.read(cx).app.settings_view.clone();
        let query_active = settings::search::Query::parse(&view.query).is_active();
        let body = if view.json {
            let json = self.json.get_or_insert_with(|| {
                crate::settings_json::JsonView::new(self.ws.clone(), window, cx)
            });
            json.render(&ws, window, cx)
        } else if query_active {
            self.render_results(&ws, window, cx)
        } else {
            self.render_pages(&ws, cx)
        };
        let bar = self.render_bar(&ws, cx).into_any_element();
        div()
            .id(("settings-panel", self.id.0))
            .key_context("Settings")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::toggle_json))
            .on_action(cx.listener(Self::apply_json))
            .on_action(cx.listener(|this, _: &FocusSettingsSearch, window, cx| {
                this.focus_search(window, cx)
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel.bg)
            .text_color(t.panel.text)
            .font_family(t.ui_font)
            .text_size(px(t.ui_size))
            .child(bar)
            .child(div().flex_1().min_h_0().w_full().child(body))
            .into_any_element()
    }
}
