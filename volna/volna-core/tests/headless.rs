//! Headless viewer tests: feed commands, assert on model state and on the
//! display list. No GPU, no window, every platform.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use volna_core::app::{Action, App, Command, Event};
use volna_core::data::synth::SynthSource;
use volna_core::data::{
    Bit, Hierarchy, SignalHistory, SignalRef, SignalShape, TraceInfo, WaveValue,
};
use volna_core::document::TraceState;
use volna_core::geometry::{Modifiers, MouseButton, Rect, point};
use volna_core::nav::Lerp;
use volna_core::scene::{MonoMeasure, Prim};
use volna_core::session::{LoadRequest, LoadResult, OpenSpec, Session};
use volna_core::sidebar::Key;
use volna_core::wave::PointerEvent;
use volna_core::{Instant, Theme};

/// A synthetic source that counts loads, can fail on demand, and has an
/// alias variable sharing signal 0.
struct Source {
    inner: SynthSource,
    hierarchy: Hierarchy,
    loads: AtomicUsize,
    fail: AtomicBool,
}

impl Source {
    fn new(n: usize) -> Arc<Self> {
        let inner = SynthSource::new(n);
        let mut hierarchy = inner.hierarchy().clone();
        let mut alias = hierarchy.vars[0].clone();
        alias.name = "alias".into();
        hierarchy.vars.push(alias);
        Arc::new(Self {
            inner,
            hierarchy,
            loads: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        })
    }
}

impl Session for Source {
    fn info(&self) -> &TraceInfo {
        self.inner.info()
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, signal: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        self.loads.fetch_add(1, SeqCst);
        anyhow::ensure!(!self.fail.load(SeqCst), "test failure");
        self.inner.load_signal(signal)
    }
}

/// Perform every queued load synchronously, like a frontend with an
/// executor that finishes before the next frame.
fn pump(app: &mut App) {
    loop {
        let requests = app.take_requests();
        if requests.is_empty() {
            return;
        }
        for r in requests {
            app.deliver(r.perform());
        }
    }
}

fn frame(app: &mut App, theme: &Theme) {
    app.layout_panel(
        app.panels.focused_id(),
        Rect::from_xywh(0.0, 0.0, 1200.0, 600.0),
        theme,
    );
    app.render_panel(app.panels.focused_id(), theme, &mut MonoMeasure);
}

/// A trace with an empty waveform panel where the start panel was.
fn loaded_app(n: usize) -> (App, Arc<Source>) {
    let source = Source::new(n);
    let mut app = App::new();
    app.set_session(source.clone());
    app.handle(Command::Action(Action::NewPanel));
    assert!(app.panels.focused_waves().is_some());
    (app, source)
}

#[test]
fn missing_initial_sample_is_not_painted_as_a_logic_level() {
    use volna_core::data::history::VecHistory;
    use volna_core::scene::Scene;
    use volna_core::wave::paint::paint_bit_row;
    use volna_core::wave::viewport::Viewport;
    let mut history = VecHistory {
        shape: SignalShape::Bit,
        times: vec![10],
        values: vec![WaveValue::Bits("1".into())],
        initial: WaveValue::Unavailable,
    };
    let area = Rect::from_xywh(0.0, 0.0, 100.0, 24.0);
    let viewport = Viewport {
        start: 0.0,
        end: 9.0,
    };
    let mut scene = Scene::default();
    paint_bit_row(&history, &viewport, area, &Theme::one_dark(), &mut scene);
    assert!(scene.prims.is_empty(), "no fabricated initial level");
    history.initial = WaveValue::Bits("x".into());
    paint_bit_row(&history, &viewport, area, &Theme::one_dark(), &mut scene);
    assert!(!scene.prims.is_empty(), "recorded X is still drawn");
}

#[test]
fn batch_loads_coalesce_and_stale_batches_preserve_new_pending() {
    let (mut app, source) = loaded_app(10);
    app.handle(Command::AddVars(vec![0, 1, 0]));
    let mut requests = app.take_requests();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    let LoadRequest::Signals { signals, .. } = &request else {
        panic!("expected batch")
    };
    assert_eq!(signals.len(), 2);
    let stale = request.perform();
    app.set_session(source);
    app.handle(Command::AddVars(vec![0, 1]));
    app.deliver(stale);
    assert_eq!(app.doc.pending_count(), 2);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| row.history.is_none())
    );
    pump(&mut app);
    assert_eq!(app.doc.pending_count(), 0);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| row.history.is_some())
    );
}

#[test]
fn removing_queued_signals_clears_demand_but_active_loads_survive_readd() {
    let (mut app, source) = loaded_app(10);
    let signal = source.hierarchy.vars[0].signal;
    app.handle(Command::AddVars(vec![0]));
    app.handle(Command::Action(Action::RemoveSelected));
    assert!(app.take_requests().is_empty());
    assert!(!app.doc.is_pending(signal));
    assert_eq!(source.loads.load(SeqCst), 0);

    app.handle(Command::AddVars(vec![0]));
    let active = app.take_requests().pop().unwrap();
    app.handle(Command::Action(Action::RemoveSelected));
    assert!(app.take_requests().is_empty());
    assert!(app.doc.is_pending(signal));
    app.handle(Command::AddVars(vec![0]));
    assert!(
        app.take_requests().is_empty(),
        "reuse the active immutable load"
    );
    app.deliver(active.perform());
    assert!(
        app.panels.focused_waves().unwrap().items[0]
            .history
            .is_some()
    );
    assert_eq!(source.loads.load(SeqCst), 1);
}

#[test]
fn removing_one_alias_keeps_the_other_alias_queued() {
    let (mut app, source) = loaded_app(10);
    app.handle(Command::AddVars(vec![0]));
    app.handle(Command::AddVars(vec![source.hierarchy.vars.len() - 1]));
    app.handle(Command::Action(Action::RemoveSelected));
    pump(&mut app);
    assert_eq!(source.loads.load(SeqCst), 1);
    assert_eq!(app.panels.focused_waves().unwrap().items.len(), 1);
    assert!(
        app.panels.focused_waves().unwrap().items[0]
            .history
            .is_some()
    );
}

#[test]
fn aliases_share_pending_and_loaded_histories() {
    let (mut app, source) = loaded_app(10);
    let alias = source.hierarchy.vars.len() - 1;
    app.handle(Command::AddVars(vec![0, alias, 0]));
    assert_eq!(app.doc.pending_count(), 1);
    assert_eq!(app.take_requests().len(), 1);
    // Rows share one history once it arrives.
    let signal = source.hierarchy.vars[0].signal;
    let history = source.inner.load_signal(signal).unwrap();
    app.deliver(LoadResult::Signals {
        generation: app.doc.generation(),
        results: vec![(signal, Ok(history.clone()))],
    });
    app.handle(Command::AddVars(vec![0]));
    assert!(
        app.take_requests().is_empty(),
        "loaded histories are reused"
    );
    assert_eq!(app.panels.focused_waves().unwrap().items[1].name, "alias");
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|i| Arc::ptr_eq(&history, i.history.as_ref().unwrap()))
    );
    let weak = Arc::downgrade(&history);
    let before = weak.strong_count();
    app.close_trace();
    assert_eq!(
        weak.strong_count(),
        before - 4,
        "rows released their histories"
    );
}

#[test]
fn stale_results_cannot_fill_rows_or_clear_new_pending_loads() {
    let (mut app, source) = loaded_app(10);
    app.handle(Command::AddVars(vec![0]));
    let old = app.doc.generation();
    let _stale = app.take_requests();
    // Reopening even the same session creates a new generation.
    app.set_session(source.clone());
    app.handle(Command::AddVars(vec![0]));
    let signal = source.hierarchy.vars[0].signal;
    app.deliver(LoadResult::Signals {
        generation: old,
        results: vec![(signal, source.inner.load_signal(signal))],
    });
    app.deliver(LoadResult::Signals {
        generation: old,
        results: vec![(signal, Err(anyhow::anyhow!("old failure")))],
    });
    assert!(app.doc.is_pending(signal));
    assert!(
        app.panels.focused_waves().unwrap().items[0]
            .history
            .is_none()
    );
    assert!(app.panels.focused_waves().unwrap().items[0].error.is_none());
    pump(&mut app);
    assert!(
        app.panels.focused_waves().unwrap().items[0]
            .history
            .is_some()
    );
}

#[test]
fn retry_menu_reloads_aliases_without_adding_rows_or_changing_ready_data() {
    use volna_core::wave::model::MenuAction;
    let (mut app, source) = loaded_app(10);
    source.fail.store(true, SeqCst);
    app.handle(Command::AddVars(vec![0, 0]));
    pump(&mut app);
    source.fail.store(false, SeqCst);
    app.handle(Command::AddVars(vec![1]));
    pump(&mut app);
    let panel = app.panels.focused_id();
    let ready = app.panels.waves(panel).unwrap().items[2]
        .history
        .clone()
        .unwrap();
    app.panels
        .waves_mut(panel)
        .unwrap()
        .open_format_menu(&app.doc, 0, point(100.0, 100.0));
    assert!(
        app.panels
            .waves(panel)
            .unwrap()
            .menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .any(|item| item.action == MenuAction::RetryLoad)
    );
    app.handle(Command::MenuSelect(panel, MenuAction::RetryLoad));
    assert!(
        app.panels
            .waves(panel)
            .unwrap()
            .items
            .iter()
            .all(|row| row.error.is_none())
    );
    assert_eq!(app.panels.waves(panel).unwrap().items.len(), 3);
    let mut requests = app.take_requests();
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();
    let LoadRequest::Signals { signals, .. } = &request else {
        panic!("signal request")
    };
    assert_eq!(signals, &[source.hierarchy.vars[0].signal]);
    app.deliver(request.perform());
    let rows = &app.panels.waves(panel).unwrap().items;
    assert!(Arc::ptr_eq(
        rows[0].history.as_ref().unwrap(),
        rows[1].history.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(rows[2].history.as_ref().unwrap(), &ready));
    assert_eq!(source.loads.load(SeqCst), 3);
    app.panels
        .waves_mut(panel)
        .unwrap()
        .open_format_menu(&app.doc, 0, point(100.0, 100.0));
    assert!(
        !app.panels
            .waves(panel)
            .unwrap()
            .menu
            .as_ref()
            .unwrap()
            .items
            .iter()
            .any(|item| item.action == MenuAction::RetryLoad)
    );
    app.handle(Command::MenuSelect(panel, MenuAction::RetryLoad));
    assert!(
        app.take_requests().is_empty(),
        "a stale retry cannot reload ready data"
    );
}

#[test]
fn failed_loads_can_retry_for_all_alias_rows() {
    let (mut app, source) = loaded_app(10);
    source.fail.store(true, SeqCst);
    app.handle(Command::AddVars(vec![0, 0]));
    pump(&mut app);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|i| i.error.is_some())
    );
    source.fail.store(false, SeqCst);
    app.handle(Command::AddVars(vec![0]));
    pump(&mut app);
    assert_eq!(source.loads.load(SeqCst), 2);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|i| i.error.is_none() && i.history.is_some())
    );
}

#[test]
fn latest_open_wins_and_stale_open_cannot_add_rows() {
    let mut app = App::new();
    app.open_synthetic(50);
    let slow = app.take_requests().pop().unwrap();
    let LoadRequest::Open {
        generation: slow_gen,
        ..
    } = slow
    else {
        panic!("expected an open request");
    };
    assert!(matches!(app.trace_state(), TraceState::Loading { .. }));
    let current: Arc<dyn Session> = Arc::new(SynthSource::new(7));
    app.handle(Command::Open(OpenSpec::Synthetic(7)));
    let new = app.take_requests().pop().unwrap();
    let LoadRequest::Open {
        generation: new_gen,
        ..
    } = new
    else {
        panic!("expected an open request");
    };
    app.deliver(LoadResult::Opened {
        generation: new_gen,
        result: Ok(current.clone()),
    });
    // The slow open completes later, with show-all semantics that must not apply.
    app.deliver(LoadResult::Opened {
        generation: slow_gen,
        result: Ok(Arc::new(SynthSource::new(100))),
    });
    assert!(matches!(app.trace_state(), TraceState::Loaded(s) if Arc::ptr_eq(s, &current)));
    assert!(app.panels.focused().kind.is_start(), "no rows were added");
}

#[test]
fn closing_invalidates_pending_success_and_error() {
    let mut app = App::new();
    for fail in [false, true] {
        app.handle(Command::Open(OpenSpec::Synthetic(10)));
        let LoadRequest::Open { generation, .. } = app.take_requests().pop().unwrap() else {
            panic!("expected an open request");
        };
        app.close_trace();
        let result = if fail {
            Err(anyhow::anyhow!("late error"))
        } else {
            Ok(Arc::new(SynthSource::new(10)) as Arc<dyn Session>)
        };
        app.deliver(LoadResult::Opened { generation, result });
        assert!(matches!(app.trace_state(), TraceState::Empty));
    }
}

#[test]
fn open_errors_are_reported_and_a_later_open_recovers() {
    let mut app = App::new();
    app.open_bytes("bad.vtr".into(), vec![0; 16]);
    pump(&mut app);
    assert!(matches!(app.trace_state(), TraceState::Error(e) if !e.is_empty()));
    app.open_synthetic(10);
    pump(&mut app);
    assert!(app.doc.is_loaded());
    assert_eq!(
        app.panels.focused_waves().unwrap().items.len(),
        app.doc.hierarchy().unwrap().vars.len()
    );
}

#[test]
fn synthetic_open_shows_all_signals_and_events_coalesce() {
    let mut app = App::new();
    app.open_synthetic(1000);
    pump(&mut app);
    let events = app.take_events();
    assert_eq!(
        events,
        vec![
            Event::Changed,
            Event::LayoutChanged {
                revision: app.panels.revision()
            }
        ]
    );
    assert!(app.panels.focused_waves().unwrap().loaded_count() > 0);
    assert_eq!(
        app.panels.focused_waves().unwrap().loaded_count(),
        app.panels.focused_waves().unwrap().items.len()
    );
}

#[test]
fn cursor_markers_and_selection_follow_the_document() {
    let (mut app, _) = loaded_app(100);
    let theme = Theme::one_dark();
    app.handle(Command::AddVars(vec![0, 1, 2]));
    pump(&mut app);
    frame(&mut app, &theme);
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    // Click in the waves of row 1 sets the cursor and selects the row.
    let p = point(layout.waves.left() + 300.0, layout.row_y(1) + 12.0);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(app.panels.focused_id(), PointerEvent::Up));
    let cursor = app.doc.shared.cursor.expect("cursor set");
    // Row 1 was already selected (new rows select themselves), so a plain
    // click keeps the multi-selection, as a drag start should.
    assert_eq!(
        app.panels
            .focused_waves()
            .unwrap()
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    app.handle(Command::Action(Action::ClearSelection));
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: p,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(app.panels.focused_id(), PointerEvent::Up));
    assert_eq!(
        app.panels
            .focused_waves()
            .unwrap()
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![1]
    );
    // Markers are document state.
    app.handle(Command::Action(Action::AddMarker));
    app.handle(Command::Action(Action::AddMarker));
    assert_eq!(app.doc.markers.len(), 1, "duplicate marker is ignored");
    assert_eq!(app.doc.markers[0].time, cursor);
    frame(&mut app, &theme);
    assert!(
        app.scene().texts().any(|t| t == "M1"),
        "marker chip is painted"
    );
    // Shift-click on the chip removes it; a plain click jumps the cursor.
    let chip = app
        .panels
        .focused_waves()
        .unwrap()
        .last_layout()
        .marker_chips[0]
        .1;
    let chip_p = point(chip.left() + 2.0, chip.top() + 2.0);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: chip_p,
            button: MouseButton::Left,
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
        },
    ));
    assert!(app.doc.markers.is_empty());
    // Escape: clear selection first, then cursor.
    app.handle(Command::Action(Action::ClearSelection));
    assert!(app.panels.focused_waves().unwrap().selected.is_empty());
    assert_eq!(app.doc.shared.cursor, Some(cursor));
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(app.doc.shared.cursor, None);
    // Name-column clicks select with modifiers.
    let name_p = |row: usize| point(layout.names.left() + 20.0, layout.row_y(row) + 12.0);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: name_p(0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: name_p(2),
            button: MouseButton::Left,
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
        },
    ));
    assert_eq!(
        app.panels
            .focused_waves()
            .unwrap()
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    app.handle(Command::Action(Action::RemoveSelected));
    assert!(app.panels.focused_waves().unwrap().items.is_empty());
}

#[test]
fn shift_wheel_scrolls_rows_without_panning_time() {
    let (mut app, _) = loaded_app(1000);
    app.handle(Command::AddVars(vec![0; 80]));
    pump(&mut app);
    frame(&mut app, &Theme::one_dark());
    let panel = app.panels.focused_id();
    let w = app.panels.focused_waves().unwrap();
    let before = w.viewport(&app.doc);
    let position = point(w.last_layout().waves.left() + 100.0, 180.0);
    assert!(w.last_layout().max_scroll > 100.0);
    for (dx, dy, expected) in [(0.0, -40.0, 40.0), (-40.0, 0.0, 80.0)] {
        app.handle(Command::Pointer(
            panel,
            PointerEvent::Wheel {
                position,
                dx,
                dy,
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                precise: false,
            },
        ));
        let w = app.panels.focused_waves().unwrap();
        assert_eq!(w.scroll_y, expected);
        assert_eq!(w.viewport(&app.doc), before);
        assert!(!app.is_animating());
    }
}

#[test]
fn command_drag_selects_time_even_with_vertical_drift() {
    let (mut app, _) = loaded_app(1000);
    frame(&mut app, &Theme::one_dark());
    let panel = app.panels.focused_id();
    let area = app.panels.focused_waves().unwrap().last_layout().waves;
    let now = Instant::now();
    app.handle_at(
        Command::Pointer(
            panel,
            PointerEvent::Down {
                position: point(area.left() + 50.0, area.top() + 100.0),
                button: MouseButton::Left,
                modifiers: Modifiers {
                    control: true,
                    platform: true,
                    ..Modifiers::default()
                },
            },
        ),
        now,
    );
    assert!(matches!(
        app.panels.focused_waves().unwrap().drag,
        Some(volna_core::wave::model::Drag::ZoomRange { .. })
    ));
    assert_eq!(app.panels.focused_waves().unwrap().cursor(&app.doc), None);
    app.handle_at(
        Command::Pointer(
            panel,
            PointerEvent::Move {
                position: point(area.left() + 150.0, area.top() + 250.0),
            },
        ),
        now,
    );
    app.handle_at(Command::Pointer(panel, PointerEvent::Up), now);
    assert!(app.is_animating());
    let full_width = app.doc.limits().1 as f64 - app.doc.limits().0 as f64;
    app.tick(now + Duration::from_secs(1));
    let selected = app.panels.focused_waves().unwrap().viewport(&app.doc);
    assert!((selected.width() - full_width * 100.0 / f64::from(area.width())).abs() < 1e-3);
}

#[test]
fn repeated_navigation_accumulates_before_animation_frames() {
    let (mut app, _) = loaded_app(1000);
    frame(&mut app, &Theme::one_dark());
    let now = Instant::now();
    let full = app.panels.focused_waves().unwrap().viewport(&app.doc);
    app.handle_at(Command::Action(Action::ZoomIn), now);
    app.handle_at(Command::Action(Action::ZoomIn), now);
    app.tick(now + Duration::from_secs(1));
    let zoomed = app.panels.focused_waves().unwrap().viewport(&app.doc);
    assert!((zoomed.width() - full.width() / 4.0).abs() < 1e-6);
    app.handle_at(
        Command::Action(Action::PanRight),
        now + Duration::from_secs(1),
    );
    app.handle_at(
        Command::Action(Action::PanRight),
        now + Duration::from_secs(1),
    );
    app.tick(now + Duration::from_secs(2));
    let panned = app.panels.focused_waves().unwrap().viewport(&app.doc);
    assert!((panned.start - zoomed.start - zoomed.width() / 2.0).abs() < 1e-6);
}

#[test]
fn area_gesture_previews_then_zooms_in_either_direction_and_escape_cancels() {
    for reverse in [false, true] {
        let (mut app, _) = loaded_app(1000);
        let theme = Theme::one_dark();
        frame(&mut app, &theme);
        let panel = app.panels.focused_id();
        let waves = app.panels.focused_waves().unwrap();
        let full = waves.viewport(&app.doc);
        let area = waves.last_layout().waves;
        let a = point(area.left() + area.width() * 0.25, area.top() + 40.0);
        let b = point(area.left() + area.width() * 0.75, a.y);
        let (a, b) = if reverse { (b, a) } else { (a, b) };
        let now = Instant::now();
        app.handle_at(
            Command::Pointer(
                panel,
                PointerEvent::Down {
                    position: a,
                    button: MouseButton::Left,
                    modifiers: Modifiers {
                        control: true,
                        ..Modifiers::default()
                    },
                },
            ),
            now,
        );
        app.handle_at(
            Command::Pointer(panel, PointerEvent::Move { position: b }),
            now,
        );
        assert_eq!(app.panels.focused_waves().unwrap().viewport(&app.doc), full);
        frame(&mut app, &theme);
        app.handle_at(Command::Pointer(panel, PointerEvent::Up), now);
        assert!(app.is_animating());
        app.tick(now + Duration::from_secs(1));
        let selected = app.panels.focused_waves().unwrap().viewport(&app.doc);
        assert!((selected.start - full.start - full.width() * 0.25).abs() < 1e-3);
        assert!((selected.width() - full.width() * 0.5).abs() < 1e-3);
        app.handle(Command::Pointer(
            panel,
            PointerEvent::Down {
                position: a,
                button: MouseButton::Left,
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
            },
        ));
        app.handle(Command::Pointer(panel, PointerEvent::Move { position: b }));
        app.handle(Command::Action(Action::ClearSelection));
        app.handle(Command::Pointer(panel, PointerEvent::Up));
        assert!(!app.is_animating());
        assert_eq!(
            app.panels.focused_waves().unwrap().viewport(&app.doc),
            selected
        );
    }
}

#[test]
fn zoom_pan_and_fit_are_deterministic_with_an_explicit_clock() {
    let (mut app, _) = loaded_app(1000);
    let theme = Theme::one_dark();
    app.handle(Command::AddVars(vec![0]));
    pump(&mut app);
    frame(&mut app, &theme);
    let full = app.panels.focused_waves().unwrap().viewport(&app.doc);
    let t0 = Instant::now();
    app.handle_at(Command::Action(Action::ZoomIn), t0);
    assert!(app.is_animating());
    assert!(app.tick(t0 + Duration::from_millis(50)));
    assert!(!app.tick(t0 + Duration::from_millis(500)));
    let zoomed = app.panels.focused_waves().unwrap().viewport(&app.doc);
    assert!(
        (zoomed.width() - full.width() / 2.0).abs() < 1.0,
        "{zoomed:?} vs {full:?}"
    );
    app.handle_at(Command::Action(Action::PanRight), t0);
    app.tick(t0 + Duration::from_secs(1));
    let panned = app.panels.focused_waves().unwrap().viewport(&app.doc);
    assert!(panned.start > zoomed.start);
    assert!((panned.width() - zoomed.width()).abs() < 1e-6);
    // Drag-pan with the right button.
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    let before = app.panels.focused_waves().unwrap().viewport(&app.doc).start;
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: point(layout.waves.left() + 400.0, layout.waves.top() + 12.0),
            button: MouseButton::Right,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Move {
            position: point(layout.waves.left() + 300.0, layout.waves.top() + 12.0),
        },
    ));
    app.handle(Command::Pointer(app.panels.focused_id(), PointerEvent::Up));
    assert!(
        app.panels.focused_waves().unwrap().viewport(&app.doc).start > before,
        "drag left pans right"
    );
    // Wheel with the secondary modifier zooms about the pointer, immediately.
    let dragged = app.panels.focused_waves().unwrap().viewport(&app.doc);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Wheel {
            position: point(layout.waves.left() + 100.0, layout.waves.top() + 10.0),
            dx: 0.0,
            dy: -120.0,
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            precise: false,
        },
    ));
    assert!(!app.is_animating());
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .viewport(&app.doc)
            .width()
            > dragged.width()
    );
    app.handle_at(Command::Action(Action::ZoomFit), t0);
    app.tick(t0 + Duration::from_secs(1));
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .viewport(&app.doc)
            .approx_eq(&full)
    );
    let status = app.status();
    assert!(status.px_per.as_deref().unwrap().starts_with("1 px = "));
}

/// A 1-bit history with a burst of changes in the middle of a quiet trace.
struct Burst;

impl SignalHistory for Burst {
    fn shape(&self) -> SignalShape {
        SignalShape::Bit
    }
    fn len(&self) -> usize {
        1002
    }
    fn time(&self, i: usize) -> u64 {
        match i {
            0 => 0,
            1001 => 100_000,
            i => 50_000 + i as u64, // 1000 changes within 1000 time units
        }
    }
    fn value(&self, i: Option<usize>) -> WaveValue {
        WaveValue::Bits(if i.is_some_and(|i| i % 2 == 1) {
            "1".into()
        } else {
            "0".into()
        })
    }
    fn bit(&self, i: Option<usize>) -> Bit {
        if i.is_some_and(|i| i % 2 == 1) {
            Bit::One
        } else {
            Bit::Zero
        }
    }
}

struct BurstSource {
    info: TraceInfo,
    hierarchy: Hierarchy,
}

impl BurstSource {
    fn new() -> Arc<Self> {
        let synth = SynthSource::new(10);
        let mut hierarchy = synth.hierarchy().clone();
        hierarchy.vars.truncate(1);
        hierarchy.vars[0].shape = SignalShape::Bit;
        hierarchy.scopes.iter_mut().for_each(|s| s.vars.truncate(1));
        Arc::new(Self {
            info: TraceInfo {
                design_id: None,
                name: "burst".into(),
                timescale: -9,
                time_range: (0, 100_000),
                signal_count: 1,
                change_count: Some(1002),
                time_unit: None,
            },
            hierarchy,
        })
    }
}

impl Session for BurstSource {
    fn info(&self) -> &TraceInfo {
        &self.info
    }
    fn hierarchy(&self) -> &Hierarchy {
        &self.hierarchy
    }
    fn load_signal(&self, _: SignalRef) -> anyhow::Result<Arc<dyn SignalHistory>> {
        Ok(Arc::new(Burst))
    }
}

#[test]
fn dense_columns_collapse_into_one_band_and_zooming_in_resolves_edges() {
    let theme = Theme::one_dark();
    let mut app = App::new();
    app.set_session(BurstSource::new());
    app.handle(Command::AddVars(vec![0]));
    pump(&mut app);
    frame(&mut app, &theme);
    let dense: Vec<_> = app
        .scene()
        .quads()
        .filter(|(_, c)| *c == theme.wave_dense)
        .collect();
    assert_eq!(dense.len(), 1, "one dense band for the burst: {dense:?}");
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    let expected_w = 1000.0 / 100_000.0 * f64::from(layout.waves.width());
    let (band, _) = dense[0];
    assert!(
        (f64::from(band.width()) - expected_w).abs() <= 2.0,
        "band {band:?} vs {expected_w}"
    );
    // Zoomed in far enough, every change is its own edge and nothing is dense.
    let t0 = Instant::now();
    app.doc.shared.viewport.value.start = 50_100.0;
    app.doc.shared.viewport.value.end = 50_120.0;
    let _ = t0;
    frame(&mut app, &theme);
    assert!(app.scene().quads().all(|(_, c)| c != theme.wave_dense));
    let edges = app
        .scene()
        .quads()
        .filter(|(r, c)| r.width() == 1.0 && r.height() > 2.0 && *c == theme.wave_signal)
        .count();
    assert_eq!(edges, 20, "one vertical edge per change in view");
}

#[test]
fn format_menu_and_translator_cycle() {
    let (mut app, _) = loaded_app(100);
    let theme = Theme::one_dark();
    let vector = app
        .doc
        .hierarchy()
        .unwrap()
        .vars
        .iter()
        .position(|v| matches!(v.shape, SignalShape::Vector { .. }))
        .expect("a vector variable");
    app.handle(Command::AddVars(vec![vector]));
    pump(&mut app);
    frame(&mut app, &theme);
    let badge = app.panels.focused_waves().unwrap().last_layout().badges[0].1;
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: point(badge.left() + 2.0, badge.top() + 2.0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    let menu = app
        .panels
        .focused_waves()
        .unwrap()
        .menu
        .clone()
        .expect("menu opened");
    assert!(menu.items.iter().any(|i| i.checked));
    assert!(app.debug_state().contains("menu=true"));
    let before = app.panels.focused_waves().unwrap().items[0].translator.id();
    let other = menu
        .items
        .iter()
        .find(|i| !i.checked)
        .unwrap()
        .action
        .clone();
    let volna_core::wave::model::MenuAction::Format(ref format) = other else {
        panic!("format choice");
    };
    app.handle(Command::MenuSelect(app.panels.focused_id(), other.clone()));
    assert!(app.panels.focused_waves().unwrap().menu.is_none());
    assert_eq!(
        app.panels.focused_waves().unwrap().items[0].translator.id(),
        format
    );
    assert_ne!(
        app.panels.focused_waves().unwrap().items[0].translator.id(),
        before
    );
    app.handle(Command::Action(Action::CycleFormat));
    assert_ne!(
        app.panels.focused_waves().unwrap().items[0].translator.id(),
        format
    );
    // Escape closes an open menu before touching the selection.
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: point(badge.left() + 2.0, badge.top() + 2.0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    assert!(app.panels.focused_waves().unwrap().menu.is_some());
    app.handle(Command::Action(Action::ClearSelection));
    assert!(app.panels.focused_waves().unwrap().menu.is_none());
    assert!(!app.panels.focused_waves().unwrap().selected.is_empty());
}

#[test]
fn sidebar_models_follow_scope_selection_and_keys() {
    let (mut app, _) = loaded_app(20);
    let h = app.doc.hierarchy().unwrap().clone();
    assert_eq!(app.scopes.selected, h.roots.first().copied());
    assert_eq!(app.variables.scope, app.scopes.selected);
    let first_rows = app.variables.rows.clone();
    assert!(!first_rows.is_empty());
    app.handle(Command::ScopesKey(Key::Down));
    let events = app.take_events();
    assert!(events.contains(&Event::RevealScopeRow(1)));
    assert_ne!(app.scopes.selected, h.roots.first().copied());
    assert_eq!(app.variables.scope, app.scopes.selected);
    // Filtering across the whole trace when no scope is selected.
    app.variables.set_scope(Some(&h), None);
    app.handle(Command::SetFilter(h.vars[0].name.clone()));
    assert!(
        app.variables
            .rows
            .contains(&volna_core::data::Member::Var(0))
    );
    assert!(app.variables.show_scope());
    // Enter adds the selection (or all).
    app.handle(Command::VariablesKey(Key::Down, Modifiers::default()));
    app.handle(Command::VariablesKey(Key::Enter, Modifiers::default()));
    assert_eq!(app.panels.focused_waves().unwrap().items.len(), 1);
    app.handle(Command::VariablesKey(
        Key::Char("x".into()),
        Modifiers::default(),
    ));
    assert!(app.take_events().contains(&Event::FocusFilter));
    app.handle(Command::ExpandAllScopes(false));
    assert_eq!(app.scopes.visible.len(), h.roots.len());
}

#[test]
fn layout_hit_regions_and_scene_cursors_agree() {
    let (mut app, _) = loaded_app(10);
    let theme = Theme::one_dark();
    app.handle(Command::AddVars((0..40).map(|i| i % 4).collect()));
    pump(&mut app);
    frame(&mut app, &theme);
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    assert!(layout.scrollbar.is_some(), "40 rows overflow 600 px");
    assert!(layout.max_scroll > 0.0);
    // Wheel scrolls rows; the layout clamps.
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Wheel {
            position: point(layout.names.left() + 10.0, layout.names.top() + 10.0),
            dx: 0.0,
            dy: -1e6,
            modifiers: Modifiers::default(),
            precise: false,
        },
    ));
    frame(&mut app, &theme);
    assert_eq!(
        app.panels.focused_waves().unwrap().scroll_y,
        app.panels.focused_waves().unwrap().last_layout().max_scroll
    );
    // Dragging the names divider resizes the column and pins the resize cursor.
    let split = app
        .panels
        .focused_waves()
        .unwrap()
        .last_layout()
        .names_split;
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position: point(split.left() + 4.0, split.top() + 100.0),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Move {
            position: point(split.left() + 64.0, split.top() + 100.0),
        },
    ));
    frame(&mut app, &theme);
    assert!(app.scene().window_cursor.is_some());
    assert!(
        (app.panels.focused_waves().unwrap().names_width - 280.0).abs() < 1.0,
        "{}",
        app.panels.focused_waves().unwrap().names_width
    );
    app.handle(Command::Pointer(app.panels.focused_id(), PointerEvent::Up));
    frame(&mut app, &theme);
    assert!(app.scene().window_cursor.is_none());
    assert!(
        app.scene().cursors.len()
            >= 2 + app
                .panels
                .focused_waves()
                .unwrap()
                .last_layout()
                .badges
                .len()
    );
    // Clip pushes and pops balance.
    let (mut depth, mut max_depth) = (0i32, 0);
    for p in &app.scene().prims {
        match p {
            Prim::PushClip(_) => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            Prim::PopClip => depth -= 1,
            _ => {}
        }
    }
    assert_eq!(depth, 0);
    assert!(max_depth >= 1);
}

#[test]
fn hover_is_derived_from_the_pointer_and_only_changes_request_repaints() {
    let (mut app, _) = loaded_app(10);
    let theme = Theme::one_dark();
    app.handle(Command::AddVars(vec![0, 1]));
    pump(&mut app);
    frame(&mut app, &theme);
    app.take_events();
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    let p = point(layout.names.left() + 10.0, layout.row_y(1) + 5.0);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Move { position: p },
    ));
    assert_eq!(app.panels.focused_waves().unwrap().hover_row, Some(1));
    assert_eq!(app.take_events(), vec![Event::Changed]);
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Move {
            position: point(p.x + 1.0, p.y),
        },
    ));
    assert!(app.take_events().is_empty(), "same row: no repaint");
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Leave,
    ));
    assert_eq!(app.panels.focused_waves().unwrap().hover_row, None);
    assert_eq!(app.take_events(), vec![Event::Changed]);
}

#[test]
fn debug_state_matches_the_documented_format() {
    let mut app = App::new();
    assert_eq!(
        app.debug_state(),
        "panel=1 start focused=true drag=None sidebar_w=280px scopes_frac=0.42"
    );
    app.handle(Command::Action(Action::NewPanel));
    assert_eq!(
        app.debug_state(),
        "panel=2 focused=true linked=(true,true) items=0 loaded=0 selected={} anchor=None cursor=None markers=0 viewport=(0,1000) menu=false drag=None sidebar_w=280px scopes_frac=0.42"
    );
    app.handle(Command::SetSidebarWidth(340.0));
    app.handle(Command::SetScopesFraction(0.05));
    assert!(
        app.debug_state()
            .contains("sidebar_w=340px scopes_frac=0.15")
    );
}
