//! Headless tests of analog rows in the waveform panel: numeric readings of
//! the translators, turning plots on and off with their heights, the format
//! and signal menus, ranges and their easing, resizing rows by their edges,
//! workspaces, and the painted plot (exact and min/max columns, undefined
//! spans, the cursor dot and the hover readout).

use std::sync::Arc;

use web_time::{Duration, Instant};

use volna_core::Theme;
use volna_core::app::{Action, App, Command};
use volna_core::data::value_view::ValueView;
use volna_core::data::{SignalShape, Translators};
use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::panels::PanelId;
use volna_core::scene::{MonoMeasure, Prim, Scene};
use volna_core::session::{OpenSpec, Session};
use volna_core::wave::PointerEvent;
use volna_core::wave::analog::{self, AnalogDraw, AnalogRange};
use volna_core::wave::model::{MenuAction, MenuEntry, RowHeight};
use volna_core::wave::viewport::Viewport;
use volna_core::workspace::Workspace;

const TRACE: &str = "file:///tmp/analog.vtr";
const LOCATION: &str = "file:///tmp/analog.vtr.volna.json";
const END: u64 = 100_000;
const GLITCH: u64 = 42_010;
const BUS: usize = 0;
const REAL: usize = 1;
const BIT: usize = 2;

/// A 100 µs trace sampled every 10 ns: a 16-bit signed sine (X until
/// 100 ns) and a real sine, each with a one-sample glitch at 42.01 µs,
/// plus a slow bit.
fn trace() -> (tempfile::NamedTempFile, Arc<dyn Session>) {
    trace_until(END)
}

fn trace_until(end: u64) -> (tempfile::NamedTempFile, Arc<dyn Session>) {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    w.set_timescale(-9).unwrap();
    let (_, bus) = w
        .add_var(
            None,
            "bus",
            vtr::VarType::Wire,
            vtr::Direction::Output,
            vtr::SignalKind::Bits {
                width: 16,
                states: 4,
            },
        )
        .unwrap();
    let (_, real) = w
        .add_var(
            None,
            "level",
            vtr::VarType::Real,
            vtr::Direction::Output,
            vtr::SignalKind::Real,
        )
        .unwrap();
    let (_, bit) = w
        .add_var(
            None,
            "valid",
            vtr::VarType::Wire,
            vtr::Direction::Output,
            vtr::SignalKind::Bits {
                width: 1,
                states: 2,
            },
        )
        .unwrap();
    for t in (0..end).step_by(10) {
        w.set_time(t).unwrap();
        let phase = t as f64 * 2.0 * std::f64::consts::PI / 4000.0;
        if t < 100 {
            w.emit_logic_str(bus, b"xxxxxxxxxxxxxxxx").unwrap();
        } else {
            let v: i16 = if t == GLITCH {
                30_000
            } else {
                (phase.sin() * 10_000.0) as i16
            };
            w.emit_u64(bus, u64::from(v as u16)).unwrap();
        }
        w.emit_real(real, if t == GLITCH { 1.32 } else { 0.9 * phase.sin() })
            .unwrap();
        w.emit_bit(bit, u8::from((t / 1000) % 2 == 1)).unwrap();
    }
    w.close().unwrap();
    let session = OpenSpec::Path(file.path().into()).open().unwrap();
    (file, session)
}

fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for request in requests {
            app.deliver(request.perform());
        }
    }
}

fn app() -> (tempfile::NamedTempFile, App, PanelId) {
    let (file, session) = trace();
    let mut app = App::new();
    app.set_session(session);
    app.handle(Command::AddVars(vec![BUS, REAL, BIT]));
    pump(&mut app);
    let id = app.panels.focused_id();
    frame(&mut app, id);
    (file, app, id)
}

fn frame(app: &mut App, id: PanelId) -> &Scene {
    let theme = Theme::one_dark();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1400.0, 700.0), &theme)
        .unwrap();
    app.render_panel(id, &theme, &mut MonoMeasure)
}

fn view(app: &mut App, id: PanelId, start: f64, end: f64) {
    let App { panels, doc, .. } = app;
    panels
        .waves_mut(id)
        .unwrap()
        .nav
        .jump_to(doc, Viewport { start, end });
}

fn select(app: &mut App, id: PanelId, rows: &[usize]) {
    let w = app.panels.waves_mut(id).unwrap();
    w.selected = rows.iter().copied().collect();
    w.anchor = rows.first().copied();
}

fn heights(app: &App, id: PanelId) -> Vec<u8> {
    let w = app.panels.waves(id).unwrap();
    w.items.iter().map(|r| r.height().multiple()).collect()
}

fn draws(app: &App, id: PanelId) -> Vec<Option<AnalogDraw>> {
    let w = app.panels.waves(id).unwrap();
    w.items
        .iter()
        .map(|r| r.signal().unwrap().analog.as_ref().map(|a| a.draw))
        .collect()
}

fn set_format(app: &mut App, id: PanelId, row: usize, format: &str) {
    let w = app.panels.waves_mut(id).unwrap();
    let t = app.doc.translators.get(format).unwrap();
    w.items[row].signal_mut().unwrap().translator = t;
}

/// The waves-column rectangle of `row` at the last layout.
fn row_rect(app: &App, id: PanelId, row: usize) -> Rect {
    let l = app.panels.waves(id).unwrap().last_layout();
    Rect::from_xywh(
        l.waves.left(),
        l.row_y(row),
        l.waves.width(),
        l.row_height(row),
    )
}

/// Endpoints of the plot strokes inside `area`.
fn stroke_points(scene: &Scene, area: Rect) -> Vec<Point> {
    let color = Theme::one_dark().wave_signal;
    scene
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Lines {
                segments, color: c, ..
            } if *c == color => Some(segments),
            _ => None,
        })
        .flatten()
        .flat_map(|[a, b]| [*a, *b])
        .filter(|p| area.contains(*p))
        .collect()
}

fn x_of(app: &App, id: PanelId, t: f64) -> f32 {
    let w = app.panels.waves(id).unwrap();
    let l = w.last_layout();
    l.waves.left() + w.viewport(&app.doc).x_of(t, f64::from(l.waves.width())) as f32
}

#[test]
fn translators_read_numbers_like_their_values() {
    let t = Translators::builtin();
    let read = |id: &str, bits: &str| {
        t.get(id).unwrap().numeric(&ValueView::Logic(
            volna_core::data::value_view::LogicView::ascii(bits.as_bytes()),
        ))
    };
    assert_eq!(read("hex", "11111111"), Some(255.0));
    assert_eq!(read("udec", "0101"), Some(5.0));
    assert_eq!(read("sdec", "11111110"), Some(-2.0));
    assert_eq!(read("sdec", "01111111"), Some(127.0));
    assert_eq!(read("hex", "10x1"), None, "X is undefined, not a number");
    assert_eq!(
        read("float", &format!("{:032b}", 1.5f32.to_bits())),
        Some(1.5)
    );
    assert_eq!(read("text", "0101"), None);
    let real = t.get("real").unwrap();
    assert_eq!(real.numeric(&ValueView::Real(0.25)), Some(0.25));
    assert_eq!(real.numeric(&ValueView::Real(f64::NAN)), None);
    let v16 = SignalShape::Vector { width: 16 };
    assert_eq!(
        t.get("sdec").unwrap().limits(v16),
        Some((-32768.0, 32767.0))
    );
    assert_eq!(t.get("hex").unwrap().limits(v16), Some((0.0, 65535.0)));
    assert_eq!(real.limits(SignalShape::Real), None);
    // Labels go through the translator: the value that reads back as -2.
    let kind = t.get("sdec").unwrap().numeric_kind().unwrap();
    let value = kind
        .value_of(-2.0, SignalShape::Vector { width: 8 })
        .unwrap();
    assert_eq!(t.get("sdec").unwrap().translate(&value).text, "-2");
    assert_eq!(t.get("hex").unwrap().translate(&value).text, "fe");
}

#[test]
fn reals_open_as_plots_and_a_toggles_the_selection_with_its_height() {
    let (_file, mut app, id) = app();
    assert_eq!(draws(&app, id), [None, Some(AnalogDraw::Linear), None]);
    assert_eq!(heights(&app, id), [1, 3, 1]);

    // A on the bus and the bit: the bus becomes a 3× step plot, the bit stays.
    select(&mut app, id, &[BUS, BIT]);
    app.handle(Command::Action(Action::ToggleAnalog));
    assert_eq!(
        draws(&app, id),
        [Some(AnalogDraw::Step), Some(AnalogDraw::Linear), None]
    );
    assert_eq!(heights(&app, id), [3, 3, 1]);
    // Again: back to digital and 1×.
    app.handle(Command::Action(Action::ToggleAnalog));
    assert_eq!(draws(&app, id)[BUS], None);
    assert_eq!(heights(&app, id), [1, 3, 1]);

    // A resize in between is an explicit choice that turning analog off keeps.
    select(&mut app, id, &[BUS]);
    app.handle(Command::Action(Action::ToggleAnalog));
    app.handle(Command::Action(Action::IncreaseRowHeight));
    app.handle(Command::Action(Action::ToggleAnalog));
    assert_eq!(heights(&app, id)[BUS], 4);

    // A text format cannot plot: the row keeps digital drawing.
    select(&mut app, id, &[REAL]);
    app.handle(Command::Action(Action::ToggleAnalog));
    assert_eq!(draws(&app, id)[REAL], None);
    assert_eq!(heights(&app, id)[REAL], 1, "the real row got its 1× back");
}

#[test]
fn the_format_menu_draws_and_ranges_and_the_signal_menu_toggles() {
    let (_file, mut app, id) = app();
    let labels = |app: &App| -> Vec<String> {
        app.panels
            .waves(id)
            .unwrap()
            .menu
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Label(l) => Some(l.clone()),
                _ => None,
            })
            .collect()
    };
    let checked = |app: &App| -> Vec<MenuAction> {
        app.panels
            .waves(id)
            .unwrap()
            .menu
            .as_ref()
            .unwrap()
            .items()
            .filter(|i| i.checked)
            .map(|i| i.action.clone())
            .collect()
    };
    let open_format = |app: &mut App, row: usize| {
        let theme = Theme::one_dark();
        app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1400.0, 700.0), &theme);
        let w = app.panels.waves_mut(id).unwrap();
        let p = w
            .last_layout()
            .badges
            .iter()
            .find(|(r, _)| *r == row)
            .unwrap()
            .1
            .origin;
        let doc = &app.doc;
        w.open_format_menu(doc, row, p);
    };

    open_format(&mut app, BUS);
    assert_eq!(labels(&app), ["Draw"], "no Range section while digital");
    assert!(checked(&app).contains(&MenuAction::Draw(None)));
    app.handle(Command::MenuSelect(
        id,
        MenuAction::Draw(Some(AnalogDraw::Linear)),
    ));
    assert_eq!(draws(&app, id)[BUS], Some(AnalogDraw::Linear));

    open_format(&mut app, BUS);
    assert_eq!(labels(&app), ["Draw", "Range"]);
    let items: Vec<_> = app
        .panels
        .waves(id)
        .unwrap()
        .menu
        .as_ref()
        .unwrap()
        .items()
        .map(|i| i.action.clone())
        .collect();
    assert!(items.contains(&MenuAction::Range(AnalogRange::Type)));
    app.handle(Command::MenuSelect(
        id,
        MenuAction::Range(AnalogRange::Type),
    ));
    frame(&mut app, id);
    let a = app.panels.waves(id).unwrap().items[BUS]
        .signal()
        .unwrap()
        .analog
        .clone()
        .unwrap();
    assert_eq!(a.range, AnalogRange::Type);
    assert_eq!(a.target, Some((0.0, 65535.0)), "hex reads unsigned");

    // Reals have no type limits; bits have no Draw section.
    open_format(&mut app, REAL);
    let items: Vec<_> = app
        .panels
        .waves(id)
        .unwrap()
        .menu
        .as_ref()
        .unwrap()
        .items()
        .map(|i| i.action.clone())
        .collect();
    assert!(!items.contains(&MenuAction::Range(AnalogRange::Type)));
    open_format(&mut app, BIT);
    assert!(labels(&app).is_empty());
    app.handle(Command::MenuDismiss(id));

    // The signal menu toggles every plottable row of the selection.
    select(&mut app, id, &[BUS, REAL, BIT]);
    app.handle(Command::OpenSignalMenu(id));
    let toggle = app
        .panels
        .waves(id)
        .unwrap()
        .menu
        .as_ref()
        .unwrap()
        .items()
        .find(|i| i.action == MenuAction::ToggleAnalog)
        .cloned()
        .unwrap();
    assert!(toggle.checked);
    app.handle(Command::MenuSelect(id, MenuAction::ToggleAnalog));
    assert_eq!(draws(&app, id), [None, None, None]);
    assert_eq!(heights(&app, id), [1, 1, 1], "grown rows get their 1× back");
}

#[test]
fn ranges_fit_trace_window_and_type_and_the_window_range_eases() {
    let (_file, mut app, id) = app();
    select(&mut app, id, &[BUS]);
    app.handle(Command::Action(Action::ToggleAnalog));
    set_format(&mut app, id, BUS, "sdec");
    view(&mut app, id, 0.0, END as f64);
    frame(&mut app, id);
    let target = |app: &App| {
        app.panels.waves(id).unwrap().items[BUS]
            .signal()
            .unwrap()
            .analog
            .as_ref()
            .unwrap()
            .target
            .unwrap()
    };
    let shown = |app: &App| {
        app.panels.waves(id).unwrap().items[BUS]
            .signal()
            .unwrap()
            .analog
            .as_ref()
            .unwrap()
            .shown
            .unwrap()
    };
    assert_eq!(
        target(&app),
        (-10_000.0, 30_000.0),
        "the whole trace, glitch included"
    );
    assert_eq!(shown(&app), target(&app), "the first range shows at once");

    // A quiet window: the window range fits it and eases there.
    app.panels
        .waves_mut(id)
        .unwrap()
        .set_analog_range(&[BUS], AnalogRange::Window);
    view(&mut app, id, 10_000.0, 10_400.0);
    let t0 = Instant::now();
    assert!(app.tick(t0));
    let window = target(&app);
    assert!(window.1 - window.0 < 7_000.0, "{window:?}");
    assert!(app.is_animating());
    assert!(app.tick(t0 + Duration::from_millis(40)));
    let mid = shown(&app);
    assert!(
        mid.0 < window.0 && mid.0 > -10_000.0,
        "{mid:?} eases towards {window:?}"
    );
    for k in 2..40 {
        app.tick(t0 + Duration::from_millis(40 * k));
    }
    assert_eq!(shown(&app), window);
    assert!(!app.is_animating());
}

#[test]
fn zoomed_out_columns_keep_the_glitch_and_x_breaks_the_line() {
    let (_file, mut app, id) = app();
    select(&mut app, id, &[BUS]);
    app.handle(Command::Action(Action::ToggleAnalog));
    set_format(&mut app, id, BUS, "sdec");
    view(&mut app, id, 0.0, END as f64);
    frame(&mut app, id);
    let area = row_rect(&app, id, BUS);
    let w = app.panels.waves(id).unwrap();
    let h = w.items[BUS].signal().unwrap().history.clone().unwrap();
    let vp = w.viewport(&app.doc);
    assert_eq!(
        analog::draw_mode(h.as_ref(), &vp, area.width()),
        analog::DrawMode::Envelope
    );
    let gx = x_of(&app, id, GLITCH as f64);
    let scene = frame(&mut app, id);
    let peak = stroke_points(scene, area)
        .into_iter()
        .filter(|p| (p.x - gx).abs() <= 1.5)
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min);
    let top = area.top() + 5.0;
    assert!(
        (peak - top).abs() <= 1.0,
        "the glitch reaches the top ({peak} vs {top})"
    );
    // Its neighbours stay near the sine's amplitude, well below the top.
    let near = stroke_points(scene, area)
        .into_iter()
        .filter(|p| (p.x - (gx + 40.0)).abs() <= 0.5)
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min);
    assert!(near > top + 10.0, "{near}");
    // X before reset is an undefined span in the undefined colour.
    let undef = Theme::one_dark().wave_undef;
    assert!(scene.prims.iter().any(|p| matches!(p,
        Prim::Quad { rect, fill, .. } if *fill == undef && area.contains(rect.origin) && rect.left() <= area.left() + 1.0)));
}

#[test]
fn zoomed_in_plots_draw_every_sample_with_dots_and_the_cursor_dot() {
    let (_file, mut app, id) = app();
    view(&mut app, id, 41_900.0, 42_130.0);
    {
        let App { panels, doc, .. } = &mut app;
        panels.waves_mut(id).unwrap().set_cursor(doc, Some(GLITCH));
    }
    let scene = frame(&mut app, id).prims.clone();
    let area = row_rect(&app, id, REAL);
    let signal = Theme::one_dark().wave_signal;
    let dots: Vec<Rect> = scene
        .iter()
        .filter_map(|p| match p {
            Prim::Quad {
                rect,
                fill,
                radius,
                border_width,
                ..
            } if *fill == signal
                && *radius > 0.0
                && *border_width == 0.0
                && area.contains(rect.origin) =>
            {
                Some(*rect)
            }
            _ => None,
        })
        .collect();
    assert!(dots.len() >= 20, "one dot per sample: {}", dots.len());
    // The cursor dot sits on the glitch sample at the top of the plot.
    let cursor_dot = scene
        .iter()
        .find_map(|p| match p {
            Prim::Quad {
                rect,
                fill,
                border_width,
                ..
            } if *fill == signal && *border_width > 0.0 && area.contains(rect.origin) => {
                Some(*rect)
            }
            _ => None,
        })
        .expect("cursor dot");
    let cy = cursor_dot.top() + cursor_dot.height() / 2.0;
    assert!((cy - (area.top() + 5.0)).abs() < 1.0, "{cy}");
    // The value column reads the glitch.
    let s = Scene {
        prims: scene,
        ..Default::default()
    };
    assert!(s.texts().any(|t| t == "1.32"));
}

#[test]
fn hovering_a_plot_reads_a_sample_or_a_dense_column() {
    let (_file, mut app, id) = app();
    let hover = |app: &mut App, t: f64| -> Vec<String> {
        let x = x_of(app, id, t);
        let y = row_rect(app, id, REAL).top() + 20.0;
        app.handle(Command::Pointer(
            id,
            PointerEvent::Move {
                position: point(x, y),
            },
        ));
        frame(app, id).texts().map(str::to_owned).collect()
    };
    view(&mut app, id, 0.0, END as f64);
    frame(&mut app, id);
    let texts = hover(&mut app, 30_000.0);
    assert!(
        texts
            .iter()
            .any(|t| t.contains(" … ") && t.contains("changes")),
        "{texts:?}"
    );
    view(&mut app, id, 41_900.0, 42_130.0);
    frame(&mut app, id);
    let texts = hover(&mut app, GLITCH as f64 + 2.0);
    assert!(
        texts.iter().any(|t| t.starts_with("1.32  sample at")),
        "{texts:?}"
    );
}

#[test]
fn dragging_a_row_edge_resizes_to_presets_without_selecting() {
    let (_file, mut app, id) = app();
    select(&mut app, id, &[REAL]);
    let l = app.panels.waves(id).unwrap().last_layout().clone();
    let bottom = l.row_y(BUS) + l.row_height(BUS);
    let at = |y: f32| point(l.names.left() + 30.0, y);
    app.handle(Command::Pointer(
        id,
        PointerEvent::Down {
            position: at(bottom - 1.0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    assert_eq!(
        app.panels
            .waves(id)
            .unwrap()
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [REAL]
    );
    app.handle(Command::Pointer(
        id,
        PointerEvent::Move {
            position: at(bottom + 1.2 * l.row_h),
        },
    ));
    assert_eq!(heights(&app, id)[BUS], 2);
    app.handle(Command::Pointer(
        id,
        PointerEvent::Move {
            position: at(bottom + 2.1 * l.row_h),
        },
    ));
    assert_eq!(heights(&app, id)[BUS], 3);
    app.handle(Command::Pointer(
        id,
        PointerEvent::Move {
            position: at(bottom + 9.0 * l.row_h),
        },
    ));
    assert_eq!(heights(&app, id)[BUS], 8);
    app.handle(Command::Pointer(id, PointerEvent::Up));
    assert!(app.panels.waves(id).unwrap().drag.is_none());
    // A drag inside the selection resizes the whole selection.
    select(&mut app, id, &[BUS, BIT]);
    frame(&mut app, id);
    let l = app.panels.waves(id).unwrap().last_layout().clone();
    let bottom = l.row_y(BIT) + l.row_height(BIT);
    let at = |y: f32| point(l.names.left() + 30.0, y);
    app.handle(Command::Pointer(
        id,
        PointerEvent::Down {
            position: at(bottom),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(
        id,
        PointerEvent::Move {
            position: at(bottom + l.row_h),
        },
    ));
    app.handle(Command::Pointer(id, PointerEvent::Up));
    assert_eq!(heights(&app, id), [2, 3, 2]);
}

#[test]
fn analog_rows_round_trip_through_workspaces() {
    let (_file, mut app, id) = app();
    select(&mut app, id, &[BUS]);
    app.handle(Command::Action(Action::ToggleAnalog));
    app.panels
        .waves_mut(id)
        .unwrap()
        .set_analog_range(&[BUS], AnalogRange::Window);
    let saved =
        serde_json::to_value(Workspace::capture(&app, TRACE.into(), None).unwrap()).unwrap();
    let panel = &saved["panels"][0];
    assert_eq!(panel["version"], 3);
    assert_eq!(
        panel["rows"][0]["analog"],
        serde_json::json!({"draw": "step", "range": "window"})
    );
    assert_eq!(panel["rows"][0]["height"], 3);
    assert_eq!(
        panel["rows"][1]["analog"],
        serde_json::json!({"draw": "linear", "range": "trace"})
    );
    assert!(panel["rows"][2].get("analog").is_none());

    let (_file2, session) = trace();
    let mut restored = App::new();
    restored.set_session(session);
    Workspace::parse(&serde_json::to_vec(&saved).unwrap())
        .and_then(|ws| ws.prepare(&restored, TRACE, LOCATION))
        .unwrap()
        .commit(&mut restored)
        .unwrap();
    let rid = restored.panels.focused_id();
    assert_eq!(
        draws(&restored, rid),
        [Some(AnalogDraw::Step), Some(AnalogDraw::Linear), None]
    );
    assert_eq!(heights(&restored, rid), [3, 3, 1]);
    let again =
        serde_json::to_value(Workspace::capture(&restored, TRACE.into(), None).unwrap()).unwrap();
    assert_eq!(again["panels"], saved["panels"]);
    // Older wave panels are reported, not guessed at.
    let mut old = saved.clone();
    old["panels"][0]["version"] = 2.into();
    let mut other = App::new();
    other.set_session(trace().1);
    let plan = Workspace::parse(&serde_json::to_vec(&old).unwrap())
        .and_then(|ws| ws.prepare(&other, TRACE, LOCATION))
        .unwrap();
    assert!(
        plan.report()
            .notices
            .iter()
            .any(|r| r.contains("waves version 2")),
        "{:?}",
        plan.report().notices
    );
    let _ = RowHeight::ANALOG;
}

#[test]
fn long_histories_summarize_on_the_worker_and_release_with_the_plot() {
    use volna_core::document::SummaryLoad;
    use volna_core::session::LoadRequest;
    // 40 000 changes per signal: above the summary threshold.
    let (_file, session) = trace_until(400_000);
    let mut app = App::new();
    app.set_session(session);
    app.handle(Command::AddVars(vec![BUS]));
    pump(&mut app);
    let id = app.panels.focused_id();
    let used = |app: &App| app.status().memory.unwrap().used;
    let digital = used(&app);
    select(&mut app, id, &[BUS]);
    app.handle(Command::Action(Action::ToggleAnalog));
    let requests = app.take_requests();
    assert_eq!(requests.len(), 1);
    assert!(matches!(requests[0], LoadRequest::Summary { .. }));
    let signal = app.panels.waves(id).unwrap().items[BUS]
        .signal_ref()
        .unwrap();
    let kind = volna_core::data::NumericKind::Unsigned;
    assert!(matches!(
        app.doc.analog_summary(signal, kind),
        Some(SummaryLoad::Building { .. })
    ));

    // Zoomed out, the plot waits for the summary instead of scanning.
    view(&mut app, id, 0.0, 400_000.0);
    assert!(frame(&mut app, id).texts().any(|t| t == "Summarizing…"));
    // Zoomed in, it draws at once.
    view(&mut app, id, 1000.0, 1200.0);
    assert!(!frame(&mut app, id).texts().any(|t| t == "Summarizing…"));

    for request in requests {
        app.deliver(request.perform());
    }
    assert!(matches!(
        app.doc.analog_summary(signal, kind),
        Some(SummaryLoad::Ready(_))
    ));
    assert!(used(&app) > digital, "the summary is charged to the budget");
    view(&mut app, id, 0.0, 400_000.0);
    let scene = frame(&mut app, id);
    assert!(!scene.texts().any(|t| t == "Summarizing…"));
    let area = row_rect(&app, id, BUS);
    let scene = frame(&mut app, id);
    assert!(stroke_points(scene, area).len() > 100, "columns are drawn");

    // Another reading needs its own summary; the old one is released.
    set_format(&mut app, id, BUS, "sdec");
    app.handle(Command::Action(Action::ClearSelection));
    let signed = volna_core::data::NumericKind::Signed;
    assert!(app.doc.analog_summary(signal, kind).is_none());
    assert!(matches!(
        app.doc.analog_summary(signal, signed),
        Some(SummaryLoad::Building { .. })
    ));
    pump(&mut app);
    // Back to digital: released with its memory.
    select(&mut app, id, &[BUS]);
    app.handle(Command::Action(Action::ToggleAnalog));
    assert!(app.doc.analog_summary(signal, signed).is_none());
    assert_eq!(used(&app), digital);
}
