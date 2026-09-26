//! Headless tests of signal groups in the waveform panel
//! (`docs/wave_groups.html`): making, folding, renaming and dissolving
//! groups; the folded summary, its value cell, hover list and edge
//! stepping; commands on groups; dragging with a level; the clipboard;
//! adding a scope as a group; and workspaces.

use std::sync::Arc;

use web_time::Instant;

use volna_core::Theme;
use volna_core::app::{Action, App, Command};
use volna_core::data::ValueKind;
use volna_core::geometry::{Modifiers, MouseButton, Point, Rect, point};
use volna_core::panels::{PanelId, PanelKind};
use volna_core::scene::{MonoMeasure, Prim, Scene};
use volna_core::session::{OpenSpec, Session};
use volna_core::wave::layout::{CHEVRON_W, INDENT, indent_x};
use volna_core::wave::model::{GroupRow, MenuAction, WaveModel, WaveRow};
use volna_core::wave::tree::{self, Entry};
use volna_core::wave::viewport::Viewport;
use volna_core::wave::{Drag, PointerEvent};
use volna_core::workspace::Workspace;

const TRACE: &str = "file:///tmp/groups.vtr";
const LOCATION: &str = "file:///tmp/groups.vtr.volna.json";
const END: u64 = 1000;
/// `wdata` is X for one cycle from here.
const X_AT: u64 = 505;

/// A small SoC: `top.clk`, `top.rst_n`, an AXI write channel under
/// `top.axi` with a read channel below it, and `top.core.pc`.
fn trace() -> (tempfile::NamedTempFile, Arc<dyn Session>) {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    w.set_timescale(-9).unwrap();
    let bit = vtr::SignalKind::Bits {
        width: 1,
        states: 4,
    };
    let bus = vtr::SignalKind::Bits {
        width: 16,
        states: 4,
    };
    let top = w
        .add_scope(None, "top", vtr::ScopeType::Module, "top")
        .unwrap();
    let axi = w
        .add_scope(Some(top), "axi", vtr::ScopeType::Module, "axi")
        .unwrap();
    let read = w
        .add_scope(Some(axi), "read", vtr::ScopeType::Module, "read")
        .unwrap();
    let core = w
        .add_scope(Some(top), "core", vtr::ScopeType::Module, "core")
        .unwrap();
    let mut var = |scope, name: &str, kind| {
        w.add_var(
            Some(scope),
            name,
            vtr::VarType::Wire,
            vtr::Direction::Output,
            kind,
        )
        .unwrap()
        .1
    };
    let clk = var(top, "clk", bit);
    let rst = var(top, "rst_n", bit);
    let awvalid = var(axi, "awvalid", bit);
    let wdata = var(axi, "wdata", bus);
    let bvalid = var(axi, "bvalid", bit);
    let rvalid = var(read, "rvalid", bit);
    let rdata = var(read, "rdata", bus);
    let pc = var(core, "pc", bus);
    for t in (0..END).step_by(5) {
        w.set_time(t).unwrap();
        w.emit_bit(clk, u8::from((t / 5) % 2 == 0)).unwrap();
        if t == 0 || t == 40 {
            w.emit_bit(rst, u8::from(t == 40)).unwrap();
        }
        if t % 100 == 0 {
            w.emit_bit(awvalid, u8::from(t % 200 == 0)).unwrap();
            w.emit_bit(rvalid, u8::from(t % 300 == 0)).unwrap();
            w.emit_u64(rdata, t / 10).unwrap();
        }
        if t % 100 == 50 || t == 0 {
            w.emit_bit(bvalid, u8::from(t % 200 == 50)).unwrap();
        }
        if t == X_AT {
            w.emit_logic_str(wdata, b"xxxxxxxxxxxxxxxx").unwrap();
        } else if t == X_AT + 10 || t % 80 == 0 {
            w.emit_u64(wdata, t).unwrap();
        }
        if t % 20 == 0 {
            w.emit_u64(pc, 0x100 + t / 5).unwrap();
        }
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

/// The trace with every variable in the waveform panel, in declaration
/// order: clk, rst_n, awvalid, wdata, bvalid, rvalid, rdata, pc.
fn app() -> (tempfile::NamedTempFile, App, PanelId) {
    let (file, session) = trace();
    let mut app = App::new();
    app.set_session(session);
    let vars = (0..app.doc.hierarchy().unwrap().vars.len()).collect();
    app.handle(Command::AddVars(vars));
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

fn waves(app: &App, id: PanelId) -> &WaveModel {
    app.panels.waves(id).unwrap()
}

fn select(app: &mut App, id: PanelId, rows: &[usize]) {
    let w = app.panels.waves_mut(id).unwrap();
    w.selected = rows.iter().copied().collect();
    w.anchor = rows.first().copied();
}

/// Each entry as indentation plus name, a `+`/`-` before folded/open groups.
fn outline(app: &App, id: PanelId) -> Vec<String> {
    waves(app, id)
        .items
        .iter()
        .map(|e| {
            let mark = match &e.row {
                WaveRow::Group(g) if g.collapsed => "+",
                WaveRow::Group(_) => "-",
                _ => "",
            };
            format!("{}{mark}{}", "  ".repeat(usize::from(e.depth)), e.name())
        })
        .collect()
}

fn selected(app: &App, id: PanelId) -> Vec<usize> {
    waves(app, id).selected.iter().copied().collect()
}

/// A point on entry `entry`'s name cell, `x` pixels into the names column
/// and `frac` of the way down its first line.
fn name_at(app: &App, id: PanelId, entry: usize, x: f32, frac: f32) -> Point {
    let l = waves(app, id).last_layout();
    let (y, _) = l.entry_span(entry).expect("entry is visible");
    point(l.names.left() + x, y + l.row_h * frac)
}

fn chevron(app: &App, id: PanelId, entry: usize) -> Point {
    let w = waves(app, id);
    let l = w.last_layout();
    let (y, _) = l.entry_span(entry).unwrap();
    let x = indent_x(l.names.left(), w.items[entry].depth, l.zoom) + CHEVRON_W / 2.0;
    point(x, y + l.row_h / 2.0)
}

fn press(app: &mut App, id: PanelId, position: Point, button: MouseButton, modifiers: Modifiers) {
    app.handle(Command::Pointer(
        id,
        PointerEvent::Down {
            position,
            button,
            modifiers,
        },
    ));
}

fn click(app: &mut App, id: PanelId, position: Point) {
    press(app, id, position, MouseButton::Left, Modifiers::default());
    app.handle(Command::Pointer(id, PointerEvent::Up));
    frame(app, id);
}

/// Press on `from`, move through the drag threshold to `to`, release.
fn drag(app: &mut App, id: PanelId, from: Point, to: Point) -> Option<Drag> {
    press(app, id, from, MouseButton::Left, Modifiers::default());
    for p in [point(from.x, from.y + 6.0), to] {
        app.handle(Command::Pointer(id, PointerEvent::Move { position: p }));
        frame(app, id);
    }
    let shown = waves(app, id).drag;
    app.handle(Command::Pointer(id, PointerEvent::Up));
    frame(app, id);
    shown
}

fn texts(scene: &Scene) -> Vec<&str> {
    scene
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The same colour at any opacity.
fn same_hue(a: volna_core::color::Color, b: volna_core::color::Color) -> bool {
    a.a > 0.0 && (a.h, a.s, a.l) == (b.h, b.s, b.l)
}

fn view(app: &mut App, id: PanelId, start: f64, end: f64) {
    let App { panels, doc, .. } = app;
    panels
        .waves_mut(id)
        .unwrap()
        .nav
        .jump_to(doc, Viewport { start, end });
}

/// clk, rst_n, [AXI: awvalid, wdata, bvalid, [Read: rvalid, rdata]], pc.
fn grouped(app: &mut App, id: PanelId) {
    select(app, id, &[5, 6]);
    app.handle(Command::Action(Action::GroupSelection));
    app.handle(Command::RenameGroup(id, Some("Read".into())));
    select(app, id, &[2, 3, 4, 5]);
    app.handle(Command::Action(Action::GroupSelection));
    app.handle(Command::RenameGroup(id, Some("AXI".into())));
    frame(app, id);
    assert_eq!(
        outline(app, id),
        [
            "clk",
            "rst_n",
            "-AXI",
            "  awvalid",
            "  wdata",
            "  bvalid",
            "  -Read",
            "    rvalid",
            "    rdata",
            "pc"
        ]
    );
}

#[test]
fn g_groups_the_selection_in_place_and_starts_renaming_it() {
    let (_f, mut app, id) = app();
    // Rows from two places keep their order under a group at the first one.
    select(&mut app, id, &[3, 4, 7]);
    app.handle(Command::Action(Action::GroupSelection));
    assert_eq!(
        outline(&app, id),
        [
            "clk", "rst_n", "awvalid", "-Group 1", "  wdata", "  bvalid", "  pc", "rvalid", "rdata"
        ]
    );
    let w = waves(&app, id);
    assert_eq!(selected(&app, id), [3]);
    assert_eq!(w.rename, Some(3));
    frame(&mut app, id);
    let rect = waves(&app, id)
        .rename_rect()
        .expect("the editor has a place");
    let l = waves(&app, id).last_layout();
    assert_eq!(rect.left(), indent_x(l.names.left(), 0, 1.0) + CHEVRON_W);
    assert_eq!(rect.top(), l.entry_span(3).unwrap().0);
    // The group's name is left to the editor while it is open.
    assert!(!texts(frame(&mut app, id)).contains(&"Group 1"));

    // Enter commits, an empty name or Escape keeps the old one.
    app.handle(Command::RenameGroup(id, Some("  Write  ".into())));
    assert_eq!(waves(&app, id).items[3].name(), "Write");
    assert_eq!(waves(&app, id).rename, None);
    app.handle(Command::Action(Action::RenameGroup));
    app.handle(Command::RenameGroup(id, Some("   ".into())));
    assert_eq!(waves(&app, id).items[3].name(), "Write");
    app.handle(Command::Action(Action::RenameGroup));
    app.handle(Command::Action(Action::ClearSelection));
    assert_eq!(waves(&app, id).rename, None, "Escape cancels the rename");
    assert_eq!(selected(&app, id), [3], "and keeps the selection");

    // A second group gets the next free name.
    select(&mut app, id, &[0]);
    app.handle(Command::Action(Action::GroupSelection));
    assert_eq!(waves(&app, id).items[0].name(), "Group 1");
    select(&mut app, id, &[1]);
    app.handle(Command::Action(Action::GroupSelection));
    assert_eq!(waves(&app, id).items[1].name(), "Group 2");
    assert_eq!(
        outline(&app, id)[..3],
        ["-Group 1", "  -Group 2", "    clk"]
    );
}

#[test]
fn folding_hides_rows_and_moves_their_selection_to_the_group() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    let before = waves(&app, id).last_layout().max_scroll;
    let shown = waves(&app, id).last_layout().visible.len();
    let pc = waves(&app, id).last_layout().entry_span(9).unwrap().0;

    // A click on the chevron folds; a selected row inside goes to the group.
    select(&mut app, id, &[4, 9]);
    let at = chevron(&app, id, 2);
    click(&mut app, id, at);
    assert_eq!(outline(&app, id)[2], "+AXI");
    assert_eq!(selected(&app, id), [2, 9]);
    let l = waves(&app, id).last_layout();
    assert_eq!(l.visible.len(), shown - 6);
    assert_eq!(l.position(4), None);
    assert_eq!(l.entry_span(9).unwrap().0, pc - 6.0 * l.row_h);
    assert!(l.max_scroll <= before);
    let scene = frame(&mut app, id);
    assert!(
        !texts(scene).contains(&"awvalid"),
        "folded rows are not painted"
    );
    assert!(texts(scene).contains(&"AXI"));
    // The number of signals it holds follows the name.
    assert!(texts(scene).contains(&"5"));

    // → unfolds a selected group, ← folds it; Alt does the groups inside.
    select(&mut app, id, &[2]);
    let vp = waves(&app, id).viewport(&app.doc);
    app.handle(Command::Action(Action::PanRight));
    assert_eq!(outline(&app, id)[2], "-AXI");
    app.handle(Command::Action(Action::FoldGroupDeep));
    assert_eq!(outline(&app, id)[2], "+AXI");
    assert_eq!(outline(&app, id)[6], "  +Read");
    app.handle(Command::Action(Action::UnfoldGroupDeep));
    assert_eq!(outline(&app, id)[6], "  -Read");
    app.handle(Command::Action(Action::PanLeft));
    assert_eq!(outline(&app, id)[2], "+AXI");
    assert_eq!(
        waves(&app, id).viewport(&app.doc),
        vp,
        "fold keys do not pan"
    );
    // On any other row the arrows pan as before.
    select(&mut app, id, &[0]);
    app.handle(Command::Action(Action::PanRight));
    app.tick(Instant::now() + std::time::Duration::from_secs(2));
    assert!(waves(&app, id).viewport(&app.doc).start > vp.start);

    // ↓ walks visible rows only.
    select(&mut app, id, &[1]);
    app.handle(Command::Action(Action::MoveSelectionDown));
    app.handle(Command::Action(Action::MoveSelectionDown));
    assert_eq!(selected(&app, id), [9]);
    // Shift-click takes the visible rows between.
    frame(&mut app, id);
    let at = name_at(&app, id, 0, 60.0, 0.5);
    press(
        &mut app,
        id,
        at,
        MouseButton::Left,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    app.handle(Command::Pointer(id, PointerEvent::Up));
    assert_eq!(selected(&app, id), [0, 1, 2, 9]);
    // Fold all and unfold all come from the row menu.
    app.handle(Command::MenuSelect(id, MenuAction::FoldAll(false)));
    let at = name_at(&app, id, 0, 60.0, 0.5);
    press(&mut app, id, at, MouseButton::Right, Modifiers::default());
    app.handle(Command::MenuSelect(id, MenuAction::FoldAll(false)));
    assert!(outline(&app, id).iter().all(|row| !row.contains('+')));
}

#[test]
fn ungroup_keeps_rows_in_place_and_remove_takes_them() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    select(&mut app, id, &[2]);
    app.handle(Command::Action(Action::Ungroup));
    assert_eq!(
        outline(&app, id),
        [
            "clk", "rst_n", "awvalid", "wdata", "bvalid", "-Read", "  rvalid", "  rdata", "pc"
        ]
    );
    assert_eq!(selected(&app, id), [2, 3, 4, 5], "the former children");

    // The row menu names what Delete does to a group.
    select(&mut app, id, &[5]);
    frame(&mut app, id);
    let at = name_at(&app, id, 5, 60.0, 0.5);
    press(&mut app, id, at, MouseButton::Right, Modifiers::default());
    let menu = waves(&app, id).menu.clone().unwrap();
    let labels: Vec<&str> = menu.items().map(|i| i.label.as_str()).collect();
    for label in [
        "Group selection",
        "Ungroup",
        "Rename…",
        "Fold",
        "Fold all",
        "Unfold all",
        "Remove with contents",
    ] {
        assert!(labels.contains(&label), "{label} in {labels:?}");
    }
    app.handle(Command::MenuSelect(id, MenuAction::RemoveSignals));
    assert_eq!(
        outline(&app, id),
        ["clk", "rst_n", "awvalid", "wdata", "bvalid", "pc"]
    );
    assert!(app.doc.pending_count() == 0);
}

#[test]
fn row_commands_take_the_subtree_and_value_commands_reach_its_signals() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    select(&mut app, id, &[6]);
    // Format and analog apply to the signals in the group.
    let formats = |app: &App| -> Vec<String> {
        waves(app, id)
            .items
            .iter()
            .filter_map(|e| e.signal())
            .map(|s| s.format_id())
            .collect()
    };
    let before = formats(&app);
    app.handle(Command::Action(Action::CycleFormat));
    let after = formats(&app);
    let changed: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    assert_eq!(changed, [5, 6], "rvalid and rdata");
    app.handle(Command::Action(Action::ToggleAnalog));
    assert!(
        waves(&app, id).items[8].signal().unwrap().analog.is_some(),
        "rdata plots"
    );
    // Height is the group row's own.
    app.handle(Command::Action(Action::IncreaseRowHeight));
    let heights: Vec<u8> = waves(&app, id)
        .items
        .iter()
        .map(|e| e.height().multiple())
        .collect();
    assert_eq!(heights[6], 2);
    assert_eq!(heights[7], 1);
    // Open in table opens the group's signals.
    app.handle(Command::OpenTableFromPanel {
        panel: id,
        row: None,
    });
    let table = app
        .panels
        .iter()
        .find_map(|p| match &p.kind {
            PanelKind::Table(t) => Some(t),
            _ => None,
        })
        .expect("a table opened");
    let volna_core::table::TableSource::Signals(signals) = &table.source else {
        panic!("a signal table");
    };
    assert_eq!(signals.len(), 2);
}

#[test]
fn a_folded_group_summarises_its_signals_and_steps_through_their_changes() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    select(&mut app, id, &[2]);
    app.handle(Command::Action(Action::PanLeft));
    assert_eq!(outline(&app, id)[2], "+AXI");
    view(&mut app, id, 0.0, END as f64);
    let theme = Theme::one_dark();
    let x_color = theme.value_color(ValueKind::Undef);
    let (y, h) = {
        frame(&mut app, id);
        waves(&app, id).last_layout().entry_span(2).unwrap()
    };
    frame(&mut app, id);
    let l = waves(&app, id).last_layout().clone();
    let scene = frame(&mut app, id);
    let in_row =
        |r: &Rect| r.top() >= y - 0.5 && r.bottom() <= y + h + 0.5 && r.left() >= l.waves.left();
    // The one-cycle X in wdata is drawn in the X colour at 505–515 ns.
    let x_rects: Vec<Rect> = scene
        .prims
        .iter()
        .filter_map(|p| match p {
            Prim::Quad { rect, fill, .. } if in_row(rect) && same_hue(*fill, x_color) => {
                Some(*rect)
            }
            _ => None,
        })
        .collect();
    assert!(!x_rects.is_empty(), "the X stretch is drawn");
    let x_of = |t: f64| l.waves.left() + (t / END as f64 * f64::from(l.waves.width())) as f32;
    assert!(
        x_rects
            .iter()
            .all(|r| r.left() >= x_of(X_AT as f64) - 2.0
                && r.right() <= x_of((X_AT + 10) as f64) + 2.0),
        "{x_rects:?}"
    );

    // Zoomed far out, a one-cycle X still shows.
    view(&mut app, id, 0.0, 1.0e6);
    let scene = frame(&mut app, id);
    assert!(scene.prims.iter().any(|p| matches!(p,
        Prim::Quad { rect, fill, .. } if in_row(rect) && same_hue(*fill, x_color))));

    // Shift+→ steps through every change of any member.
    view(&mut app, id, 0.0, END as f64);
    let members: Vec<_> = waves(&app, id).group_histories(2);
    assert_eq!(members.len(), 5);
    let mut expected: Vec<u64> = members
        .iter()
        .flat_map(|h| (0..h.len()).map(|i| h.time(i)))
        .filter(|&t| t > 0)
        .collect();
    expected.sort_unstable();
    expected.dedup();
    let panel = app.panels.waves_mut(id).unwrap();
    panel.set_cursor(&mut app.doc, Some(0));
    let mut stepped = Vec::new();
    for _ in 0..expected.len() {
        app.handle(Command::Action(Action::NextEdge));
        stepped.push(waves(&app, id).cursor(&app.doc).unwrap());
    }
    assert_eq!(stepped, expected);
    app.handle(Command::Action(Action::PrevEdge));
    assert_eq!(
        waves(&app, id).cursor(&app.doc),
        Some(expected[expected.len() - 2])
    );

    // The value cell counts the members that change at the cursor.
    let panel = app.panels.waves_mut(id).unwrap();
    panel.set_cursor(&mut app.doc, Some(X_AT));
    let scene = frame(&mut app, id);
    assert!(
        texts(scene).contains(&"1 of 5 changed"),
        "{:?}",
        texts(scene)
    );
    assert!(texts(scene).contains(&" · 1 X"));
    let panel = app.panels.waves_mut(id).unwrap();
    panel.set_cursor(&mut app.doc, Some(X_AT + 1));
    assert!(texts(frame(&mut app, id)).contains(&"5 signals"));

    // Hovering the folded row lists the members' values there.
    let p = point(x_of((X_AT + 2) as f64), y + h / 2.0);
    app.handle(Command::Pointer(id, PointerEvent::Move { position: p }));
    let scene = frame(&mut app, id);
    let t = texts(scene);
    for name in ["awvalid", "wdata", "bvalid", "rvalid", "rdata"] {
        assert!(t.contains(&name), "{name} in {t:?}");
    }
    assert!(t.iter().any(|s| s.contains("xxxx")), "wdata reads X: {t:?}");
}

#[test]
fn dragging_names_picks_the_level_from_x_and_drops_into_folded_groups() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    // Below bvalid the rows can go into AXI (depth 1) or Read (depth 2);
    // drag clk there and let x choose the level.
    let row_h = waves(&app, id).last_layout().row_h;
    let depth_x = |d: f32| 12.0 + INDENT * d + 4.0;
    let from = name_at(&app, id, 0, 60.0, 0.5);
    let to = name_at(&app, id, 5, depth_x(1.0), 0.9);
    let shown = drag(&mut app, id, from, to);
    assert!(
        matches!(
            shown,
            Some(Drag::Rows {
                gap: Some(_),
                depth: 1,
                into: None,
                ..
            })
        ),
        "{shown:?}"
    );
    assert_eq!(
        outline(&app, id),
        [
            "rst_n",
            "-AXI",
            "  awvalid",
            "  wdata",
            "  bvalid",
            "  clk",
            "  -Read",
            "    rvalid",
            "    rdata",
            "pc"
        ]
    );
    assert_eq!(selected(&app, id), [5]);

    // The end of the list offers every level of the open groups above it;
    // x at depth 0 puts pc back at the top level, x at depth 2 into Read.
    let from = name_at(&app, id, 9, 60.0, 0.5);
    let to = name_at(&app, id, 8, depth_x(2.0), 0.9);
    let shown = drag(&mut app, id, from, to);
    assert!(
        matches!(shown, Some(Drag::Rows { depth: 2, .. })),
        "{shown:?}"
    );
    assert_eq!(outline(&app, id)[9], "    pc");

    // A group cannot go inside itself: no drop line.
    let from = name_at(&app, id, 1, 60.0, 0.5);
    let to = name_at(&app, id, 3, depth_x(1.0), 0.9);
    let shown = drag(&mut app, id, from, to);
    assert!(
        matches!(shown, Some(Drag::Rows { gap: None, .. })),
        "{shown:?}"
    );
    assert_eq!(outline(&app, id)[1], "-AXI");

    // The middle of a folded group's row takes the rows and stays folded.
    select(&mut app, id, &[6]);
    app.handle(Command::Action(Action::PanLeft));
    frame(&mut app, id);
    assert_eq!(outline(&app, id)[6], "  +Read");
    let from = name_at(&app, id, 0, 60.0, 0.5);
    let to = name_at(&app, id, 6, 80.0, 0.5);
    let shown = drag(&mut app, id, from, to);
    assert!(
        matches!(shown, Some(Drag::Rows { into: Some(_), .. })),
        "{shown:?}"
    );
    assert_eq!(
        outline(&app, id),
        [
            "-AXI",
            "  awvalid",
            "  wdata",
            "  bvalid",
            "  clk",
            "  +Read",
            "    rvalid",
            "    rdata",
            "    pc",
            "    rst_n"
        ]
    );
    assert_eq!(selected(&app, id), [9]);
    // Nothing selected is hidden: the moved row is revealed by its group
    // only when the group opens, so the selection moves to the group.
    frame(&mut app, id);
    let _ = row_h;
    assert!(tree::validate(&waves(&app, id).items).is_ok());
}

#[test]
fn the_clipboard_keeps_structure_and_pastes_under_the_last_selected_row() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    select(&mut app, id, &[6]);
    app.handle(Command::Action(Action::CopySignals));
    assert_eq!(app.doc.copied_rows.len(), 3);
    assert_eq!(
        app.doc.copied_rows[0].depth, 0,
        "normalised to the top level"
    );
    // Paste lands after awvalid, at its level.
    select(&mut app, id, &[3]);
    app.handle(Command::Action(Action::PasteSignals));
    pump(&mut app);
    assert_eq!(
        outline(&app, id)[3..7],
        ["  awvalid", "  -Read", "    rvalid", "    rdata"]
    );
    assert_eq!(selected(&app, id), [4]);
    // Cut removes the group with its rows; paste puts it back whole.
    app.handle(Command::Action(Action::CutSignals));
    assert_eq!(outline(&app, id).len(), 10);
    select(&mut app, id, &[9]);
    app.handle(Command::Action(Action::PasteSignals));
    assert_eq!(outline(&app, id)[10..], ["-Read", "  rvalid", "  rdata"]);
    // Pasted rows share the histories already loaded.
    let w = waves(&app, id);
    assert!(w.items[11].signal().unwrap().history.is_some());
}

#[test]
fn add_scope_as_group_nests_child_scopes_folded() {
    let (file, session) = trace();
    let _file = file;
    let mut app = App::new();
    app.set_session(session);
    app.handle(Command::Action(Action::NewPanel));
    let id = app.panels.focused_id();
    let h = app.doc.hierarchy().unwrap();
    let axi = match h.find_scope(&["top", "axi"]) {
        volna_core::data::source::Lookup::Found(s) => s,
        other => panic!("{other:?}"),
    };
    let top = match h.find_scope(&["top"]) {
        volna_core::data::source::Lookup::Found(s) => s,
        other => panic!("{other:?}"),
    };
    app.handle(Command::AddScopeAsGroup {
        scope: axi,
        recursive: false,
    });
    assert_eq!(
        outline(&app, id),
        ["-axi", "  awvalid", "  wdata", "  bvalid"]
    );
    app.handle(Command::AddScopeAsGroup {
        scope: top,
        recursive: true,
    });
    pump(&mut app);
    assert_eq!(
        outline(&app, id)[4..],
        [
            "-top",
            "  clk",
            "  rst_n",
            "  +axi",
            "    awvalid",
            "    wdata",
            "    bvalid",
            "    +read",
            "      rvalid",
            "      rdata",
            "  +core",
            "    pc"
        ]
    );
    assert_eq!(selected(&app, id), [4]);
    assert!(
        waves(&app, id)
            .items
            .iter()
            .filter_map(|e| e.signal())
            .all(|s| s.history.is_some())
    );
}

fn restore(app: &mut App, value: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    let plan = Workspace::parse(&serde_json::to_vec(value)?)?.prepare(app, TRACE, LOCATION)?;
    let report = plan.commit(app)?;
    Ok(report.notices)
}

#[test]
fn workspaces_store_groups_as_a_tree_and_refuse_deeper_nesting() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    select(&mut app, id, &[7]);
    app.handle(Command::Action(Action::IncreaseRowHeight));
    let panel = app.panels.waves_mut(id).unwrap();
    panel.set_folded(6, true, false);
    panel.items[2].row = WaveRow::Group(GroupRow {
        name: "AXI master".into(),
        collapsed: false,
        height: volna_core::wave::RowHeight::try_from(2).unwrap(),
    });
    let saved =
        serde_json::to_value(Workspace::capture(&app, TRACE.into(), None).unwrap()).unwrap();
    let panel = &saved["panels"][0];
    assert_eq!(panel["version"], 4);
    let rows = &panel["rows"];
    assert_eq!(rows.as_array().unwrap().len(), 4);
    assert_eq!(rows[2]["type"], "group");
    assert_eq!(rows[2]["name"], "AXI master");
    assert_eq!(rows[2]["height"], 2);
    assert!(rows[2].get("collapsed").is_none());
    let read = &rows[2]["rows"][3];
    assert_eq!(read["name"], "Read");
    assert_eq!(read["collapsed"], true);
    assert_eq!(
        read["rows"][0]["signal"],
        serde_json::json!(["top", "axi", "read", "rvalid"])
    );
    // The selected rvalid is hidden in folded Read, so Read was selected.
    assert_eq!(panel["selected"], serde_json::json!([6]));

    let before = outline(&app, id);
    app.panels.waves_mut(id).unwrap().items.clear();
    let report = restore(&mut app, &saved).unwrap();
    assert!(report.is_empty(), "{report:?}");
    let id = app
        .panels
        .iter()
        .find(|p| p.kind.waves().is_some())
        .unwrap()
        .id;
    assert_eq!(outline(&app, id), before);
    assert_eq!(waves(&app, id).items[2].height().multiple(), 2);
    pump(&mut app);
    assert!(
        waves(&app, id)
            .items
            .iter()
            .filter_map(|e| e.signal())
            .all(|s| s.history.is_some())
    );

    // Selected rows inside a folded group are selected through the group.
    let mut hidden = saved.clone();
    hidden["panels"][0]["selected"] = serde_json::json!([7]);
    restore(&mut app, &hidden).unwrap();
    assert_eq!(selected(&app, id), [6]);

    // A missing signal stays in its group as an unresolved row.
    let mut missing = saved.clone();
    missing["panels"][0]["rows"][2]["rows"][0]["signal"] =
        serde_json::json!(["top", "axi", "gone"]);
    let report = restore(&mut app, &missing).unwrap();
    assert!(report.iter().any(|l| l.contains("gone")), "{report:?}");
    assert_eq!(outline(&app, id)[3], "  gone");

    // Eight levels load; a ninth is refused.
    let nest = |levels: usize| {
        let mut row =
            serde_json::json!({"type": "signal", "signal": ["top", "clk"], "format": "bin"});
        for k in 0..levels {
            row = serde_json::json!({"type": "group", "name": format!("g{k}"), "rows": [row]});
        }
        let mut v = saved.clone();
        v["panels"][0]["rows"] = serde_json::json!([row]);
        v["panels"][0]["selected"] = serde_json::json!([]);
        v
    };
    restore(&mut app, &nest(7)).unwrap();
    assert_eq!(waves(&app, id).items.last().unwrap().depth, 7);
    let err = restore(&mut app, &nest(8)).unwrap_err();
    assert!(format!("{err:#}").contains("nested deeper"), "{err:#}");

    // A version 3 panel is reported and kept as it was.
    let mut old = saved.clone();
    old["panels"][0]["version"] = serde_json::json!(3);
    let report = restore(&mut app, &old).unwrap();
    assert!(
        report.iter().any(|l| l.contains("waves version 3")),
        "{report:?}"
    );
    let kept = app.panels.iter().find(|p| p.id == id).unwrap();
    assert!(matches!(kept.kind, PanelKind::Unsupported(_)));
}

#[test]
fn every_tree_edit_keeps_a_valid_tree_and_the_rows_it_had() {
    // Random trees from a fixed seed, then random edits.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut rnd = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let leaf = |k: usize| WaveRow::Clock(volna_core::wave::model::ClockRow::new(&format!("c{k}")));
    let names = |items: &[Entry]| -> Vec<String> {
        let mut v: Vec<String> = items
            .iter()
            .filter(|e| !e.is_group())
            .map(|e| e.name().to_owned())
            .collect();
        v.sort();
        v
    };
    for round in 0..400 {
        let mut items: Vec<Entry> = Vec::new();
        for k in 0..(4 + rnd(20)) {
            let limit = items
                .last()
                .map_or(0, |e: &Entry| e.depth as usize + usize::from(e.is_group()));
            let depth = rnd(limit + 1).min(usize::from(tree::MAX_DEPTH) - 1) as u8;
            let row = if rnd(3) == 0 && depth + 1 < tree::MAX_DEPTH {
                let mut g = GroupRow::new(format!("g{k}"));
                g.collapsed = rnd(3) == 0;
                WaveRow::Group(g)
            } else {
                leaf(k)
            };
            items.push(Entry::new(depth, row));
        }
        tree::validate(&items).unwrap();
        let all = names(&items);
        let sel: std::collections::BTreeSet<usize> =
            (0..items.len()).filter(|_| rnd(4) == 0).collect();

        // Group then ungroup restores the list.
        let mut g = items.clone();
        if let Some(at) = tree::group(&mut g, &sel, WaveRow::Group(GroupRow::new("new"))) {
            tree::validate(&g).unwrap_or_else(|e| panic!("round {round}: {e}"));
            assert_eq!(names(&g), all);
            let mut u = g.clone();
            tree::ungroup(&mut u, &std::collections::BTreeSet::from([at]));
            tree::validate(&u).unwrap();
            assert_eq!(names(&u), all);
            // Grouping keeps leaves in order when the selection is one block.
            let roots = tree::roots(&items, &sel);
            if roots.len() == 1 {
                assert_eq!(
                    u.iter()
                        .map(|e| (e.depth, e.name().to_owned()))
                        .collect::<Vec<_>>(),
                    items
                        .iter()
                        .map(|e| (e.depth, e.name().to_owned()))
                        .collect::<Vec<_>>(),
                    "round {round}"
                );
            }
        }

        // Every drop the gaps offer keeps the tree valid, and moving the
        // rows back where they were restores it.
        let visible = tree::visible(&items);
        for gap in 0..=visible.len() {
            let depths = tree::gap_depths(&items, &visible, gap);
            for depth in depths {
                let to = tree::Place::gap(&items, &visible, gap, depth);
                let Some(moved) = tree::plan_move(&items, &sel, to) else {
                    continue;
                };
                let mut m = items.clone();
                moved.apply(&mut m);
                tree::validate(&m)
                    .unwrap_or_else(|e| panic!("round {round} gap {gap} depth {depth}: {e}"));
                assert_eq!(names(&m), all);
                // A single moved subtree moved back restores the list.
                let roots = tree::roots(&items, &sel);
                if roots.len() == 1 {
                    let r = roots[0];
                    let key = |e: &Entry| e.name().to_owned();
                    let successor = items.get(tree::subtree_end(&items, r)).map(key);
                    let root = moved_root(&m, &key(&items[r]));
                    let at = successor.map_or(m.len(), |name| moved_root(&m, &name));
                    let back = tree::Place {
                        at,
                        depth: items[r].depth,
                    };
                    let plan = tree::plan_move(&m, &std::collections::BTreeSet::from([root]), back)
                        .unwrap_or_else(|| panic!("round {round}: no way back"));
                    plan.apply(&mut m);
                    assert_eq!(
                        m.iter()
                            .map(|e| (e.depth, e.name().to_owned()))
                            .collect::<Vec<_>>(),
                        items
                            .iter()
                            .map(|e| (e.depth, e.name().to_owned()))
                            .collect::<Vec<_>>(),
                        "round {round} gap {gap} depth {depth}"
                    );
                }
            }
        }

        // Remove takes whole subtrees; extract + insert round-trips them.
        let copied = tree::extract(&items, &sel);
        let mut r = items.clone();
        if tree::remove(&mut r, &sel).is_some() {
            tree::validate(&r).unwrap();
            let removed = names(&copied).len();
            assert_eq!(names(&r).len() + removed, all.len());
            let mut back = r.clone();
            let to = tree::Place {
                at: back.len(),
                depth: 0,
            };
            tree::insert(&mut back, to, copied).unwrap();
            tree::validate(&back).unwrap();
            assert_eq!(names(&back), all);
        }
    }
}

fn moved_root(items: &[Entry], name: &str) -> usize {
    items.iter().position(|e| e.name() == name).unwrap()
}

#[test]
fn rows_are_a_tree_for_assistive_technology_and_group_names_take_a_double_click() {
    let (_f, mut app, id) = app();
    grouped(&mut app, id);
    select(&mut app, id, &[6]);
    app.handle(Command::Action(Action::PanLeft));
    frame(&mut app, id);
    let rows: Vec<_> = waves(&app, id).accessible_rows().collect();
    let summary: Vec<(String, usize, Option<bool>, bool)> = rows
        .iter()
        .map(|r| (r.label.clone(), r.level, r.expanded, r.selected))
        .collect();
    assert_eq!(
        summary,
        [
            ("clk".into(), 1, None, false),
            ("rst_n".into(), 1, None, false),
            ("AXI, 5 rows".into(), 1, Some(true), false),
            ("awvalid".into(), 2, None, false),
            ("wdata".into(), 2, None, false),
            ("bvalid".into(), 2, None, false),
            ("Read, 2 rows".into(), 2, Some(false), true),
            ("pc".into(), 1, None, false),
        ]
    );
    // A double-click renames on the name, not on the chevron.
    let w = waves(&app, id);
    assert_eq!(w.group_name_at(name_at(&app, id, 2, 80.0, 0.5)), Some(2));
    assert_eq!(w.group_name_at(chevron(&app, id, 2)), None);
    assert_eq!(w.group_name_at(name_at(&app, id, 0, 80.0, 0.5)), None);
}
