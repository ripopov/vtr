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
use volna_core::panels::PanelsCommand;
use volna_core::scene::{MonoMeasure, Prim};
use volna_core::session::{LoadRequest, LoadResult, OpenSpec, Session};
use volna_core::sidebar::Key;
use volna_core::wave::{MenuEntry, PointerEvent, RowHeight, WaveMenuKind};
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
            .all(|row| row.signal().unwrap().history.is_none())
    );
    pump(&mut app);
    assert_eq!(app.doc.pending_count(), 0);
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| row.signal().unwrap().history.is_some())
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
            .signal()
            .unwrap()
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
            .signal()
            .unwrap()
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
    assert_eq!(app.panels.focused_waves().unwrap().items[1].name(), "alias");
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|i| Arc::ptr_eq(&history, i.signal().unwrap().history.as_ref().unwrap()))
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
            .signal()
            .unwrap()
            .history
            .is_none()
    );
    assert!(
        app.panels.focused_waves().unwrap().items[0]
            .signal()
            .unwrap()
            .error
            .is_none()
    );
    pump(&mut app);
    assert!(
        app.panels.focused_waves().unwrap().items[0]
            .signal()
            .unwrap()
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
        .signal()
        .unwrap()
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
            .items()
            .any(|item| item.action == MenuAction::RetryLoad)
    );
    app.handle(Command::MenuSelect(panel, MenuAction::RetryLoad));
    assert!(
        app.panels.waves(panel).unwrap().items.iter().all(|row| row
            .signal()
            .unwrap()
            .error
            .is_none())
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
        rows[0].signal().unwrap().history.as_ref().unwrap(),
        rows[1].signal().unwrap().history.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        rows[2].signal().unwrap().history.as_ref().unwrap(),
        &ready
    ));
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
            .items()
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
        app.panels.focused_waves().unwrap().items.iter().all(|i| i
            .signal()
            .unwrap()
            .error
            .is_some())
    );
    source.fail.store(false, SeqCst);
    app.handle(Command::AddVars(vec![0]));
    pump(&mut app);
    assert_eq!(source.loads.load(SeqCst), 2);
    assert!(
        app.panels.focused_waves().unwrap().items.iter().all(|i| i
            .signal()
            .unwrap()
            .error
            .is_none()
            && i.signal().unwrap().history.is_some())
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
    assert_eq!(menu.kind, WaveMenuKind::Format);
    assert!(menu.items().any(|i| i.checked));
    // Formats, then the Draw section of a numeric vector.
    assert!(menu.items().all(|item| matches!(
        item.action,
        volna_core::wave::model::MenuAction::Format(_)
            | volna_core::wave::model::MenuAction::Draw(_)
            | volna_core::wave::model::MenuAction::RetryLoad
    )));
    assert!(app.debug_state().contains("menu=true"));
    let before = app.panels.focused_waves().unwrap().items[0]
        .signal()
        .unwrap()
        .translator
        .id();
    let other = menu
        .items()
        .find(|i| !i.checked && matches!(i.action, volna_core::wave::model::MenuAction::Format(_)))
        .unwrap()
        .action
        .clone();
    let volna_core::wave::model::MenuAction::Format(ref format) = other else {
        panic!("format choice");
    };
    app.handle(Command::MenuSelect(app.panels.focused_id(), other.clone()));
    assert!(app.panels.focused_waves().unwrap().menu.is_none());
    assert_eq!(
        app.panels.focused_waves().unwrap().items[0]
            .signal()
            .unwrap()
            .translator
            .id(),
        format
    );
    assert_ne!(
        app.panels.focused_waves().unwrap().items[0]
            .signal()
            .unwrap()
            .translator
            .id(),
        before
    );
    app.handle(Command::Action(Action::CycleFormat));
    assert_ne!(
        app.panels.focused_waves().unwrap().items[0]
            .signal()
            .unwrap()
            .translator
            .id(),
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
fn signal_name_menu_opens_and_removes_the_selected_signal_group() {
    use volna_core::table::TableSource;
    use volna_core::wave::model::MenuAction;

    let (mut app, _) = loaded_app(100);
    app.handle(Command::AddVars(vec![0, 1, 2]));
    pump(&mut app);
    frame(&mut app, &Theme::one_dark());
    let waves = app.panels.focused_id();
    let position = {
        let layout = app.panels.waves(waves).unwrap().last_layout();
        point(
            layout.names.left() + 20.0,
            layout.row_y(1) + layout.row_h / 2.0,
        )
    };
    app.handle(Command::Pointer(
        waves,
        PointerEvent::Down {
            position,
            button: MouseButton::Right,
            modifiers: Modifiers::default(),
        },
    ));
    let panel = app.panels.waves(waves).unwrap();
    assert_eq!(
        panel.selected.iter().copied().collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let menu = panel.menu.as_ref().expect("signal menu opened");
    assert_eq!(menu.kind, WaveMenuKind::Signal);
    assert_eq!(
        menu.items()
            .map(|item| (&item.action, item.label.as_str()))
            .collect::<Vec<_>>(),
        [
            (&MenuAction::OpenTable, "Open in table"),
            (&MenuAction::CutSignals, "Cut"),
            (&MenuAction::CopySignals, "Copy"),
            (
                &MenuAction::RowHeight(RowHeight::PRESETS[0]),
                "1× (Default)"
            ),
            (&MenuAction::RowHeight(RowHeight::PRESETS[1]), "2×"),
            (&MenuAction::RowHeight(RowHeight::PRESETS[2]), "3×"),
            (&MenuAction::RowHeight(RowHeight::PRESETS[3]), "4×"),
            (&MenuAction::RowHeight(RowHeight::PRESETS[4]), "8×"),
            (&MenuAction::RemoveSignals, "Remove signal"),
        ]
    );
    assert!(matches!(
        &menu.entries[..],
        [
            MenuEntry::Item(_),
            MenuEntry::Separator,
            MenuEntry::Item(_),
            MenuEntry::Item(_),
            MenuEntry::Separator,
            MenuEntry::Submenu { label, .. },
            MenuEntry::Separator,
            MenuEntry::Item(_),
        ] if label == "Height"
    ));

    app.handle(Command::MenuSelect(waves, MenuAction::OpenTable));
    let table = app.panels.focused().kind.table().expect("table opened");
    let TableSource::Signals(signals) = &table.source else {
        panic!("signal table");
    };
    assert_eq!(signals.len(), 3);
    assert!(app.panels.waves(waves).unwrap().menu.is_none());

    app.handle(Command::Panels(PanelsCommand::Focus(waves)));
    app.handle(Command::OpenSignalMenu(waves));
    assert_eq!(
        app.panels.waves(waves).unwrap().menu.as_ref().unwrap().kind,
        WaveMenuKind::Signal
    );
    app.handle(Command::MenuSelect(waves, MenuAction::RemoveSignals));
    let panel = app.panels.waves(waves).unwrap();
    assert!(panel.items.is_empty());
    assert!(panel.selected.is_empty());
    assert!(panel.menu.is_none());
}

fn height_checks(app: &App) -> Vec<u8> {
    app.panels
        .focused_waves()
        .unwrap()
        .menu
        .as_ref()
        .unwrap()
        .items()
        .filter(|item| item.checked)
        .map(|item| match item.action {
            volna_core::wave::model::MenuAction::RowHeight(h) => h.multiple(),
            _ => panic!("only heights are checkable here"),
        })
        .collect()
}

fn heights(app: &App) -> Vec<u8> {
    app.panels
        .focused_waves()
        .unwrap()
        .items
        .iter()
        .map(|item| item.height().multiple())
        .collect()
}

#[test]
fn height_submenu_resizes_the_selection_and_rows_lay_out_paint_and_hit_test_tall() {
    use volna_core::wave::model::MenuAction;
    let theme = Theme::one_dark();
    let (mut app, _) = loaded_app(100);
    app.handle(Command::AddVars(vec![0, 1, 2, 3]));
    pump(&mut app);
    frame(&mut app, &theme);
    let waves = app.panels.focused_id();
    let row_h = app.panels.waves(waves).unwrap().last_layout().row_h;

    // A plain right-click on row 1 selects it alone; 1× is checked.
    app.handle(Command::Action(Action::ClearSelection));
    let layout = app.panels.waves(waves).unwrap().last_layout().clone();
    app.handle(Command::Pointer(
        waves,
        PointerEvent::Down {
            position: point(layout.names.left() + 20.0, layout.row_y(1) + row_h / 2.0),
            button: MouseButton::Right,
            modifiers: Modifiers::default(),
        },
    ));
    assert_eq!(height_checks(&app), [1]);
    app.handle(Command::MenuSelect(
        waves,
        MenuAction::RowHeight(RowHeight::try_from(4).unwrap()),
    ));
    assert!(app.panels.waves(waves).unwrap().menu.is_none());
    assert_eq!(heights(&app), [1, 4, 1, 1]);

    // Mixed heights check nothing; a choice applies to the whole group.
    app.handle(Command::Action(Action::SelectAll));
    app.handle(Command::OpenSignalMenu(waves));
    assert_eq!(height_checks(&app), Vec::<u8>::new());
    app.handle(Command::MenuSelect(
        waves,
        MenuAction::RowHeight(RowHeight::try_from(2).unwrap()),
    ));
    assert_eq!(heights(&app), [2, 2, 2, 2]);
    app.handle(Command::OpenSignalMenu(waves));
    assert_eq!(height_checks(&app), [2]);
    app.handle(Command::MenuDismiss(waves));

    // Only row 2 becomes 8×. Everything below it moves down, and a click
    // anywhere in its height lands on it.
    app.handle(Command::Action(Action::ClearSelection));
    frame(&mut app, &theme);
    let layout = app.panels.waves(waves).unwrap().last_layout().clone();
    app.handle(Command::Pointer(
        waves,
        PointerEvent::Down {
            position: point(layout.names.left() + 20.0, layout.row_y(2) + row_h),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    assert_eq!(
        app.panels
            .waves(waves)
            .unwrap()
            .selected
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [2]
    );
    app.handle(Command::OpenSignalMenu(waves));
    app.handle(Command::MenuSelect(
        waves,
        MenuAction::RowHeight(RowHeight::try_from(8).unwrap()),
    ));
    assert_eq!(heights(&app), [2, 2, 8, 2]);
    frame(&mut app, &theme);
    let layout = app.panels.waves(waves).unwrap().last_layout().clone();
    let top = layout.names.top();
    assert_eq!(
        (0..4)
            .map(|row| (layout.row_y(row) - top) / row_h)
            .collect::<Vec<_>>(),
        [0.0, 2.0, 4.0, 12.0]
    );
    assert_eq!(layout.row_height(2), 8.0 * row_h);
    assert_eq!(layout.row_at(layout.row_y(2) + 7.5 * row_h), Some(2));
    assert_eq!(layout.row_at(layout.row_y(3) + 0.5 * row_h), Some(3));
    app.handle(Command::Pointer(
        waves,
        PointerEvent::Down {
            position: point(layout.waves.left() + 50.0, layout.row_y(2) + 7.5 * row_h),
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
        },
    ));
    app.handle(Command::Pointer(waves, PointerEvent::Up));
    assert!(app.panels.waves(waves).unwrap().selected.contains(&2));

    // The selected row's highlight and its waveform span all eight lines;
    // its name stays on the first line.
    let name = app.panels.waves(waves).unwrap().items[2].name().to_owned();
    let scene = app.render_panel(waves, &theme, &mut MonoMeasure);
    let wave_row = Rect::new(
        point(layout.waves.left(), layout.row_y(2)),
        volna_core::geometry::size(layout.waves.width(), 8.0 * row_h),
    );
    assert!(scene.prims.iter().any(|prim| matches!(
        prim,
        Prim::Quad { rect, fill, .. } if *rect == wave_row && *fill == theme.wave_row_selected
    )));
    let name_line = scene.prims.iter().find_map(|prim| match prim {
        Prim::Text {
            origin,
            height,
            text,
            ..
        } if *text == name => Some((origin.y, *height)),
        _ => None,
    });
    assert_eq!(name_line, Some((layout.row_y(2), row_h)));
    let trace_bottom = scene
        .prims
        .iter()
        .filter_map(|prim| match prim {
            Prim::Quad { rect, .. }
                if rect.left() >= layout.waves.left()
                    && rect.top() >= wave_row.top()
                    && rect.bottom() <= wave_row.bottom() =>
            {
                Some(rect.bottom())
            }
            _ => None,
        })
        .fold(0.0f32, f32::max);
    assert!(
        trace_bottom > layout.row_y(2) + 6.0 * row_h,
        "the waveform fills the tall row"
    );
}

#[test]
fn row_height_actions_step_presets_and_keep_the_anchor_row_on_screen() {
    let theme = Theme::one_dark();
    let (mut app, _) = loaded_app(100);
    // Aliased rows of one variable are independent rows.
    app.handle(Command::AddVars(vec![0; 60]));
    pump(&mut app);
    frame(&mut app, &theme);
    let waves = app.panels.focused_id();
    let row_h = app.panels.waves(waves).unwrap().last_layout().row_h;
    {
        let w = app.panels.focused_waves_mut().unwrap();
        w.scroll_y = 20.0 * row_h;
        w.selected = [5, 30].into();
        w.anchor = Some(30);
    }
    frame(&mut app, &theme);
    let before = app.panels.waves(waves).unwrap().last_layout().row_y(30);
    for (action, expected) in [
        (Action::IncreaseRowHeight, 2),
        (Action::IncreaseRowHeight, 3),
        (Action::IncreaseRowHeight, 4),
        (Action::IncreaseRowHeight, 8),
        (Action::IncreaseRowHeight, 8),
        (Action::DecreaseRowHeight, 4),
        (Action::ResetRowHeight, 1),
        (Action::DecreaseRowHeight, 1),
        (Action::IncreaseRowHeight, 2),
    ] {
        app.handle(Command::Action(action));
        let h = heights(&app);
        assert_eq!((h[5], h[30], h[6]), (expected, expected, 1), "{action:?}");
        frame(&mut app, &theme);
        // Row 5 grew above the viewport; row 30 did not move on screen.
        assert_eq!(
            app.panels.waves(waves).unwrap().last_layout().row_y(30),
            before,
            "{action:?}"
        );
    }
}

fn row_names(app: &App, panel: volna_core::panels::PanelId) -> Vec<String> {
    app.panels
        .waves(panel)
        .unwrap()
        .items
        .iter()
        .map(|item| item.name().to_owned())
        .collect()
}

/// Point on the name cell of `row`, `frac` of the way down it.
fn name_point(app: &App, row: usize, frac: f32) -> volna_core::geometry::Point {
    let layout = app.panels.focused_waves().unwrap().last_layout();
    point(
        layout.names.left() + 20.0,
        layout.row_y(row) + layout.row_height(row) * frac,
    )
}

fn press(app: &mut App, position: volna_core::geometry::Point, modifiers: Modifiers) {
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Down {
            position,
            button: MouseButton::Left,
            modifiers,
        },
    ));
}

fn move_to(app: &mut App, position: volna_core::geometry::Point) {
    app.handle(Command::Pointer(
        app.panels.focused_id(),
        PointerEvent::Move { position },
    ));
}

fn press_row(app: &mut App, row: usize, frac: f32, modifiers: Modifiers) {
    press(app, name_point(app, row, frac), modifiers);
}

fn move_row(app: &mut App, row: usize, frac: f32) {
    move_to(app, name_point(app, row, frac));
}

fn release(app: &mut App) {
    app.handle(Command::Pointer(app.panels.focused_id(), PointerEvent::Up));
}

fn drop_line(app: &App, theme: &Theme) -> Option<f32> {
    let names = app.panels.focused_waves().unwrap().last_layout().names;
    app.scene().prims.iter().find_map(|p| match p {
        Prim::Quad { rect, fill, .. }
            if *fill == theme.border_focused && rect.width() > names.width() * 2.0 =>
        {
            Some(rect.top() + rect.height() / 2.0)
        }
        _ => None,
    })
}

#[test]
fn dragging_signal_names_reorders_rows_and_keeps_them_selected() {
    let theme = Theme::one_dark();
    let (mut app, _) = loaded_app(10);
    let waves = app.panels.focused_id();
    app.handle(Command::AddVars(vec![0, 1, 2, 3, 4, 5]));
    pump(&mut app);
    frame(&mut app, &theme);
    let names = row_names(&app, waves);
    let order = |ix: &[usize]| ix.iter().map(|&i| names[i].clone()).collect::<Vec<_>>();

    // A press that barely moves is a click: it selects without a drop line.
    press_row(&mut app, 4, 0.5, Modifiers::default());
    move_row(&mut app, 4, 0.6);
    frame(&mut app, &theme);
    assert_eq!(drop_line(&app, &theme), None);
    assert!(app.scene().window_cursor.is_none());
    release(&mut app);
    assert_eq!(row_names(&app, waves), names);
    let w = app.panels.focused_waves().unwrap();
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [4]);

    // Drag row 4 into the upper half of row 1: the line marks the gap above
    // row 1 and the drop inserts it there.
    press_row(&mut app, 4, 0.5, Modifiers::default());
    move_row(&mut app, 2, 0.5);
    move_row(&mut app, 1, 0.25);
    frame(&mut app, &theme);
    let gap_y = app.panels.focused_waves().unwrap().last_layout().row_y(1);
    assert_eq!(drop_line(&app, &theme), Some(gap_y));
    assert_eq!(
        app.scene().window_cursor,
        Some(volna_core::geometry::CursorIcon::Grabbing)
    );
    release(&mut app);
    assert_eq!(row_names(&app, waves), order(&[0, 4, 1, 2, 3, 5]));
    let w = app.panels.focused_waves().unwrap();
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [1]);
    assert_eq!(w.anchor, Some(1));
    assert!(w.drag.is_none());

    // A scattered group moves together, in order, to the end. Pressing on a
    // selected row keeps the group; the lower half of the last row is the end.
    press_row(&mut app, 0, 0.5, Modifiers::default());
    release(&mut app);
    let ctrl = Modifiers {
        control: !cfg!(target_os = "macos"),
        platform: cfg!(target_os = "macos"),
        ..Modifiers::default()
    };
    press_row(&mut app, 3, 0.5, ctrl);
    release(&mut app);
    frame(&mut app, &theme);
    press_row(&mut app, 3, 0.5, Modifiers::default());
    move_row(&mut app, 5, 0.9);
    frame(&mut app, &theme);
    let layout = app.panels.focused_waves().unwrap().last_layout().clone();
    assert_eq!(drop_line(&app, &theme), Some(layout.row_y(6)));
    release(&mut app);
    assert_eq!(row_names(&app, waves), order(&[4, 1, 3, 5, 0, 2]));
    let w = app.panels.focused_waves().unwrap();
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [4, 5]);

    // A drop that would not change the order shows no line and is a click:
    // the group narrows to the pressed row.
    frame(&mut app, &theme);
    press_row(&mut app, 5, 0.5, Modifiers::default());
    move_row(&mut app, 4, 0.3);
    frame(&mut app, &theme);
    assert_eq!(drop_line(&app, &theme), None);
    release(&mut app);
    assert_eq!(row_names(&app, waves), order(&[4, 1, 3, 5, 0, 2]));
    let w = app.panels.focused_waves().unwrap();
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [5]);

    // Escape cancels a drag in flight.
    frame(&mut app, &theme);
    press_row(&mut app, 5, 0.5, Modifiers::default());
    move_row(&mut app, 0, 0.1);
    app.handle(Command::Action(Action::ClearSelection));
    release(&mut app);
    assert_eq!(row_names(&app, waves), order(&[4, 1, 3, 5, 0, 2]));
}

#[test]
fn dragging_rows_past_an_edge_scrolls_and_keeps_heights_with_their_rows() {
    let theme = Theme::one_dark();
    let (mut app, _) = loaded_app(10);
    let waves = app.panels.focused_id();
    app.handle(Command::AddVars((0..40).map(|i| i % 8).collect()));
    pump(&mut app);
    // Row 0 is 4× tall; it keeps its height wherever it goes.
    frame(&mut app, &theme);
    press_row(&mut app, 0, 0.5, Modifiers::default());
    release(&mut app);
    for _ in 0..3 {
        app.handle(Command::Action(Action::IncreaseRowHeight));
    }
    frame(&mut app, &theme);
    let first = app.panels.focused_waves().unwrap().items[0]
        .name()
        .to_owned();

    press_row(&mut app, 0, 0.5, Modifiers::default());
    let bottom = app
        .panels
        .focused_waves()
        .unwrap()
        .last_layout()
        .names
        .bottom();
    move_to(&mut app, point(40.0, bottom + 30.0));
    assert!(app.is_animating(), "an edge drag auto-scrolls");
    let start = Instant::now();
    for ms in [0, 50, 100, 150, 200] {
        app.tick(start + Duration::from_millis(ms));
        frame(&mut app, &theme);
    }
    let w = app.panels.focused_waves().unwrap();
    assert!(w.scroll_y > 0.0, "scrolled down");
    let Some(volna_core::wave::Drag::Rows { gap: Some(gap), .. }) = w.drag else {
        panic!("dragging with a gap");
    };
    assert!(gap > 20, "gap follows the scroll: {gap}");
    // Leaving the edge zone stops scrolling.
    move_to(&mut app, point(40.0, bottom - 100.0));
    frame(&mut app, &theme);
    assert!(!app.is_animating());
    let gap = match app.panels.focused_waves().unwrap().drag {
        Some(volna_core::wave::Drag::Rows { gap: Some(gap), .. }) => gap,
        other => panic!("{other:?}"),
    };
    release(&mut app);
    let w = app.panels.focused_waves().unwrap();
    assert_eq!(w.items[gap - 1].name(), first);
    assert_eq!(w.items[gap - 1].height().multiple(), 4);
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [gap - 1]);
    assert_eq!(row_names(&app, waves).len(), 40);
}

#[test]
fn copied_rows_paste_as_duplicates_sharing_data_in_any_wave_panel() {
    use volna_core::wave::model::MenuAction;
    let (mut app, source) = loaded_app(10);
    let waves = app.panels.focused_id();
    app.handle(Command::AddVars(vec![0, 1, 2]));
    pump(&mut app);
    assert_eq!(source.loads.load(SeqCst), 3);
    let names = row_names(&app, waves);
    // Paste with an empty clipboard is a no-op.
    app.handle(Command::Action(Action::PasteSignals));
    assert_eq!(row_names(&app, waves), names);

    // Style the "clock" row, then duplicate it three times.
    app.panels.focused_waves_mut().unwrap().selected = [0].into();
    app.handle(Command::Action(Action::CycleFormat));
    app.handle(Command::Action(Action::IncreaseRowHeight));
    app.handle(Command::Action(Action::CopySignals));
    for _ in 0..3 {
        app.handle(Command::Action(Action::PasteSignals));
    }
    let w = app.panels.waves(waves).unwrap();
    assert_eq!(
        row_names(&app, waves),
        [
            names[0].as_str(),
            names[0].as_str(),
            names[0].as_str(),
            names[0].as_str(),
            names[1].as_str(),
            names[2].as_str()
        ]
    );
    assert_eq!(w.selected.iter().copied().collect::<Vec<_>>(), [3]);
    let clock = w.signal(0).unwrap();
    for copy in w.items[1..4].iter().map(|row| row.signal().unwrap()) {
        assert_eq!(copy.source, clock.source);
        assert_eq!(copy.format_id(), clock.format_id());
        assert_eq!(copy.height, clock.height);
        assert!(Arc::ptr_eq(
            copy.history.as_ref().unwrap(),
            clock.history.as_ref().unwrap()
        ));
    }
    assert!(
        app.take_requests().is_empty(),
        "duplicates reuse loaded data"
    );
    assert!(
        app.doc
            .copied_rows
            .iter()
            .all(|row| row.signal().unwrap().history.is_none()),
        "the clipboard does not hold trace data"
    );

    // Paste goes below the selection and works in another wave panel.
    app.panels.focused_waves_mut().unwrap().selected = [1, 2].into();
    app.handle(Command::Action(Action::CopySignals));
    app.handle(Command::Action(Action::NewPanel));
    let other = app.panels.focused_id();
    assert_ne!(other, waves);
    app.handle(Command::AddVars(vec![2]));
    app.handle(Command::Action(Action::PasteSignals));
    assert_eq!(
        row_names(&app, other),
        [names[2].as_str(), names[0].as_str(), names[0].as_str()]
    );
    assert!(app.take_requests().is_empty());

    // Cut and paste moves rows; data dropped meanwhile loads again.
    app.handle(Command::Action(Action::SelectAll));
    app.handle(Command::Action(Action::CutSignals));
    assert!(row_names(&app, other).is_empty());
    app.handle(Command::Panels(PanelsCommand::Focus(waves)));
    app.handle(Command::Action(Action::SelectAll));
    app.handle(Command::Action(Action::CutSignals));
    app.handle(Command::Panels(PanelsCommand::Focus(other)));
    app.handle(Command::Action(Action::PasteSignals));
    assert_eq!(
        row_names(&app, other),
        [
            names[0].as_str(),
            names[0].as_str(),
            names[0].as_str(),
            names[0].as_str(),
            names[1].as_str(),
            names[2].as_str()
        ]
    );
    pump(&mut app);
    assert_eq!(source.loads.load(SeqCst), 6);
    assert!(
        app.panels.waves(other).unwrap().items.iter().all(|row| row
            .signal()
            .unwrap()
            .history
            .is_some())
    );

    // The signal menu offers Paste only with rows on the clipboard.
    app.handle(Command::OpenSignalMenu(other));
    let has_paste = |app: &App| {
        app.panels
            .waves(other)
            .unwrap()
            .menu
            .as_ref()
            .unwrap()
            .items()
            .any(|item| item.action == MenuAction::PasteSignals)
    };
    assert!(has_paste(&app));
    app.handle(Command::MenuSelect(other, MenuAction::PasteSignals));
    assert_eq!(row_names(&app, other).len(), 12);
    assert!(app.panels.waves(other).unwrap().menu.is_none());

    app.close_trace();
    assert!(
        app.doc.copied_rows.is_empty(),
        "the clipboard belongs to the trace"
    );
}

#[test]
fn status_reports_memory_budget_use_as_signals_load_and_unload() {
    use volna_core::app::MemoryStatus;
    // A real VTR trace: procedural sessions own no resident data to account.
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut writer = vtr::Writer::create(file.path()).unwrap();
    let (_, bus) = writer
        .add_var(
            None,
            "bus",
            vtr::VarType::Wire,
            vtr::Direction::Output,
            vtr::SignalKind::Bits {
                width: 32,
                states: 2,
            },
        )
        .unwrap();
    for time in 0..10_000u64 {
        writer.set_time(time).unwrap();
        writer.emit_u64(bus, time * 7).unwrap();
    }
    writer.close().unwrap();

    let mut app = App::new();
    assert_eq!(app.status().memory, None, "no trace, no budget");
    app.set_session(OpenSpec::Path(file.path().into()).open().unwrap());
    let opened = app.status().memory.expect("an open trace has a budget");
    assert_eq!(opened.limit, 512 * 1024 * 1024);
    assert!(opened.used > 0, "the resident trace is accounted");
    app.handle(Command::AddVars(vec![0]));
    pump(&mut app);
    let loaded = app.status().memory.unwrap();
    assert!(loaded.used > opened.used, "{loaded:?} after {opened:?}");
    app.handle(Command::Action(Action::SelectAll));
    app.handle(Command::Action(Action::RemoveSelected));
    assert_eq!(app.status().memory.unwrap().used, opened.used);

    let mib = 1024 * 1024;
    let status = |used: u64, limit: u64| MemoryStatus { used, limit };
    assert_eq!(status(0, 512 * mib).label(), "0 / 512 MiB");
    assert_eq!(status(mib * 3 / 10, 512 * mib).label(), "0.3 / 512 MiB");
    assert_eq!(
        status(412 * mib + mib / 3, 512 * mib).label(),
        "412 / 512 MiB"
    );
    assert_eq!(status(1536 * mib, 4096 * mib).label(), "1.5 / 4 GiB");
    assert_eq!(
        status(20 * 1024 * mib, 64 * 1024 * mib).label(),
        "20 / 64 GiB"
    );
    assert!(!status(460 * mib, 512 * mib).nearly_full());
    assert!(status(461 * mib, 512 * mib).nearly_full());
    assert_eq!(status(1, 0).fraction(), 1.0);
    assert_eq!(
        status(384 * mib, 512 * mib).detail(),
        "Memory budget: 384.0 of 512 MiB used (75%) by loaded trace data. \
         Loads that would exceed the budget fail. Click to change the limits."
    );
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
