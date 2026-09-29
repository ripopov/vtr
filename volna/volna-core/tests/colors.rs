//! Headless tests of row colours (`docs/RATIONALE.md`, "Volna row colours"): the Color menu
//! and command, inheritance through groups, Default, ungrouping, the
//! clipboard, undo, workspaces and the painter.

use std::sync::Arc;

use volna_core::Theme;
use volna_core::app::{Action, App, Command};
use volna_core::geometry::{Point, Rect};
use volna_core::panels::PanelId;
use volna_core::scene::{MonoMeasure, Prim, Scene};
use volna_core::session::{OpenSpec, Session};
use volna_core::testing::a_all;
use volna_core::wave::model::{MenuAction, WaveModel};
use volna_core::wave::{MenuEntry, Tint};
use volna_core::workspace::Workspace;

const TRACE: &str = "file:///tmp/colors.vtr";
const LOCATION: &str = "file:///tmp/colors.vtr.volna.json";
const CLK: usize = 0;
const VALID: usize = 1;
const DATA: usize = 2;
const ERR: usize = 3;

/// `top.clk`, `top.axi.valid`, `top.axi.data[15:0]` (X over 400–600) and
/// `top.err`.
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
    let valid = var(axi, "valid", bit);
    let data = var(axi, "data", bus);
    let err = var(top, "err", bit);
    for t in (0..1000).step_by(10) {
        w.set_time(t).unwrap();
        w.emit_bit(clk, u8::from((t / 10) % 2 == 0)).unwrap();
        if t % 100 == 0 {
            w.emit_bit(valid, u8::from(t % 200 == 0)).unwrap();
            w.emit_bit(err, 0).unwrap();
            if (400..600).contains(&t) {
                w.emit_logic_str(data, b"xxxxxxxxxxxxxxxx").unwrap();
            } else {
                w.emit_u64(data, t).unwrap();
            }
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

/// The trace with its four variables in the waveform panel.
fn app() -> (tempfile::NamedTempFile, App, PanelId) {
    let (file, session) = trace();
    let mut app = App::new();
    app.set_session(session);
    app.handle(Command::AddVars(a_all(0..4)));
    pump(&mut app);
    let id = app.panels.focused_id();
    frame(&mut app, id);
    (file, app, id)
}

fn frame(app: &mut App, id: PanelId) -> &Scene {
    let theme = Theme::one_dark();
    app.layout_panel(id, Rect::from_xywh(0.0, 0.0, 1200.0, 500.0), &theme)
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

fn inks(app: &App, id: PanelId) -> Vec<Option<Tint>> {
    let w = waves(app, id);
    (0..w.items().len()).map(|i| w.ink(i)).collect()
}

fn own(app: &App, id: PanelId) -> Vec<Option<Tint>> {
    waves(app, id).items().iter().map(|e| e.tint()).collect()
}

/// The Color submenu of the open signal menu: each choice and its check.
fn color_menu(app: &App, id: PanelId) -> Vec<(Option<Tint>, bool)> {
    let menu = waves(app, id).menu.as_ref().expect("signal menu open");
    let items = menu
        .entries
        .iter()
        .find_map(|e| match e {
            MenuEntry::Submenu { label, items } if label == "Color" => Some(items),
            _ => None,
        })
        .expect("Color submenu");
    items
        .iter()
        .map(|item| match item.action {
            MenuAction::Tint(t) => (t, item.checked),
            _ => panic!("only colours in the Color submenu"),
        })
        .collect()
}

fn set_tint(app: &mut App, tint: Option<Tint>) {
    app.handle(Command::Action(Action::SetTint(tint)));
}

#[test]
fn the_menu_colours_the_selection_in_one_step_and_checks_only_shared_colours() {
    let (_f, mut app, id) = app();
    select(&mut app, id, &[VALID, DATA]);
    app.handle(Command::OpenSignalMenu(id));
    let choices = color_menu(&app, id);
    assert_eq!(
        choices,
        [
            (None, true),
            (Some(Tint::Blue), false),
            (Some(Tint::Cyan), false),
            (Some(Tint::Violet), false),
            (Some(Tint::Pink), false),
            (Some(Tint::Grey), false),
        ]
    );
    app.handle(Command::MenuSelect(id, MenuAction::Tint(Some(Tint::Blue))));
    assert_eq!(
        inks(&app, id),
        [None, Some(Tint::Blue), Some(Tint::Blue), None]
    );
    assert_eq!(app.undo_label(), Some("Color 2 signals Blue"));

    // Applying what is already there records nothing.
    set_tint(&mut app, Some(Tint::Blue));
    app.handle(Command::Undo);
    assert_eq!(inks(&app, id), [None; 4]);
    assert_eq!(app.undo_label(), Some("Add 4 signals"));
    app.handle(Command::Redo);
    assert_eq!(own(&app, id)[VALID], Some(Tint::Blue));

    // A mixed selection checks nothing; a shared colour is checked.
    select(&mut app, id, &[CLK, VALID]);
    app.handle(Command::OpenSignalMenu(id));
    assert!(color_menu(&app, id).iter().all(|(_, checked)| !checked));
    app.handle(Command::MenuDismiss(id));
    select(&mut app, id, &[VALID, DATA]);
    app.handle(Command::OpenSignalMenu(id));
    assert_eq!(
        color_menu(&app, id)
            .into_iter()
            .filter(|(_, c)| *c)
            .collect::<Vec<_>>(),
        [(Some(Tint::Blue), true)]
    );
    app.handle(Command::MenuDismiss(id));

    // One row names itself; Default clears it.
    select(&mut app, id, &[DATA]);
    set_tint(&mut app, None);
    assert_eq!(app.undo_label(), Some("Color data Default"));
    assert_eq!(inks(&app, id), [None, Some(Tint::Blue), None, None]);
}

#[test]
fn groups_pass_their_colour_down_and_ungrouping_keeps_it() {
    let (_f, mut app, id) = app();
    select(&mut app, id, &[VALID, DATA]);
    app.handle(Command::Action(Action::GroupSelection));
    app.handle(Command::CommitText(
        volna_core::app::EditTarget::Group {
            panel: id,
            entry: 1,
        },
        Some("axi".into()),
    ));
    // clk, [axi: valid, data], err
    select(&mut app, id, &[1]);
    set_tint(&mut app, Some(Tint::Cyan));
    assert_eq!(app.undo_label(), Some("Color axi Cyan"));
    select(&mut app, id, &[3]);
    set_tint(&mut app, Some(Tint::Pink));
    assert_eq!(
        inks(&app, id),
        [
            None,
            Some(Tint::Cyan),
            Some(Tint::Cyan),
            Some(Tint::Pink),
            None
        ]
    );
    assert_eq!(own(&app, id)[2], None, "valid inherits");

    // Default on a member hands it back to its group.
    set_tint(&mut app, None);
    assert_eq!(inks(&app, id)[3], Some(Tint::Cyan));
    app.handle(Command::Undo);

    // The group row was the colour source; its rows keep what they showed.
    select(&mut app, id, &[1]);
    app.handle(Command::Action(Action::Ungroup));
    assert_eq!(
        own(&app, id),
        [None, Some(Tint::Cyan), Some(Tint::Pink), None]
    );
    app.handle(Command::Undo);
    assert_eq!(
        own(&app, id),
        [None, Some(Tint::Cyan), None, Some(Tint::Pink), None]
    );

    // Copied rows keep the colour they are drawn in.
    select(&mut app, id, &[2]);
    app.handle(Command::Action(Action::CopySignals));
    select(&mut app, id, &[4]);
    app.handle(Command::Action(Action::PasteSignals));
    pump(&mut app);
    let w = waves(&app, id);
    assert_eq!(w.items()[5].name(), "valid");
    assert_eq!(w.items()[5].depth, 0);
    assert_eq!(w.ink(5), Some(Tint::Cyan));
}

fn restore(app: &mut App, value: &serde_json::Value) -> anyhow::Result<Vec<String>> {
    let plan = Workspace::parse(&serde_json::to_vec(value)?)?.prepare(app, TRACE, LOCATION)?;
    Ok(plan.commit(app)?.notices)
}

#[test]
fn workspaces_keep_colours_and_load_unknown_names_as_default() {
    let (_f, mut app, id) = app();
    select(&mut app, id, &[CLK]);
    set_tint(&mut app, Some(Tint::Violet));
    select(&mut app, id, &[ERR]);
    set_tint(&mut app, Some(Tint::Grey));
    let saved = serde_json::to_value(
        Workspace::capture(&app, volna_core::testing::paths(TRACE), None).unwrap(),
    )
    .unwrap();
    let panel = &saved["panels"][0];
    assert_eq!(panel["version"], 5);
    assert_eq!(panel["rows"][CLK]["tint"], "violet");
    assert!(panel["rows"][VALID].get("tint").is_none());
    assert_eq!(panel["rows"][ERR]["tint"], "grey");

    set_tint(&mut app, None);
    restore(&mut app, &saved).unwrap();
    let id = app.panels.focused_id();
    assert_eq!(
        own(&app, id),
        [Some(Tint::Violet), None, None, Some(Tint::Grey)]
    );

    let mut future = saved.clone();
    future["panels"][0]["rows"][CLK]["tint"] = "chartreuse".into();
    restore(&mut app, &future).unwrap();
    let id = app.panels.focused_id();
    assert_eq!(own(&app, id), [None, None, None, Some(Tint::Grey)]);
}

#[test]
fn named_commands_colour_the_selection() {
    for (name, tint) in [
        ("colorDefault", None),
        ("colorBlue", Some(Tint::Blue)),
        ("colorGrey", Some(Tint::Grey)),
    ] {
        assert_eq!(
            Command::named(name),
            Some(Command::Action(Action::SetTint(tint)))
        );
    }
    assert_eq!(Command::named("colorRed"), None);
}

/// Colours of the line strokes and fills inside `area`.
fn inks_in(scene: &Scene, area: Rect) -> Vec<volna_core::Color> {
    let inside = |p: Point| area.contains(p);
    scene
        .prims
        .iter()
        .filter_map(|prim| match prim {
            Prim::Lines {
                segments, color, ..
            } if segments.iter().any(|[a, b]| inside(*a) || inside(*b)) => Some(*color),
            Prim::Quad { rect, fill, .. } if inside(rect.origin) => Some(*fill),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tinted_row_draws_its_ink_and_keeps_x_red_and_a_name_stripe() {
    let (_f, mut app, id) = app();
    select(&mut app, id, &[DATA]);
    set_tint(&mut app, Some(Tint::Pink));
    app.handle(Command::Action(Action::ClearSelection));
    let theme = Theme::one_dark();
    let pink = theme.ink(Some(Tint::Pink));
    frame(&mut app, id);
    let scene = app.scene();
    let l = waves(&app, id).last_layout().clone();
    let row = |entry| {
        let (y, h) = l.entry_span(entry).unwrap();
        (y, h)
    };
    let (y, h) = row(DATA);
    let wave = Rect::from_xywh(l.waves.left(), y + 1.0, l.waves.width(), h - 2.0);
    let drawn = inks_in(scene, wave);
    assert!(drawn.contains(&pink), "the bus outline takes the colour");
    assert!(drawn.contains(&theme.wave_undef), "X stays red");
    assert!(!drawn.contains(&theme.wave_signal));

    // Untinted rows are unchanged, and only the tinted one has a stripe.
    let (y, h) = row(VALID);
    let other = Rect::from_xywh(l.waves.left(), y + 1.0, l.waves.width(), h - 2.0);
    assert!(inks_in(scene, other).contains(&theme.wave_signal));
    assert!(!inks_in(scene, other).contains(&pink));
    let stripes: Vec<Rect> = scene
        .quads()
        .filter(|(r, c)| *c == pink && r.left() == l.names.left())
        .map(|(r, _)| r)
        .collect();
    assert_eq!(stripes.len(), 1);
    assert_eq!(stripes[0].top(), row(DATA).0);
    assert_eq!(stripes[0].width(), 3.0);
}

#[test]
fn the_draw_walk_matches_the_ink_query_in_nested_groups() {
    let (_f, mut app, id) = app();
    select(&mut app, id, &[VALID, DATA]);
    app.handle(Command::Action(Action::GroupSelection));
    app.handle(Command::CommitText(
        volna_core::app::EditTarget::Group {
            panel: id,
            entry: 1,
        },
        Some("outer".into()),
    ));
    select(&mut app, id, &[3]);
    app.handle(Command::Action(Action::GroupSelection));
    app.handle(Command::CommitText(
        volna_core::app::EditTarget::Group {
            panel: id,
            entry: 3,
        },
        Some("inner".into()),
    ));
    // clk, [outer: valid, [inner: data]], err
    select(&mut app, id, &[1]);
    set_tint(&mut app, Some(Tint::Violet));
    assert_eq!(
        inks(&app, id),
        [
            None,
            Some(Tint::Violet),
            Some(Tint::Violet),
            Some(Tint::Violet),
            Some(Tint::Violet),
            None
        ]
    );
    // Painting from the middle of the tree reaches the same colours: the
    // data row inside both groups is violet on screen.
    let theme = Theme::one_dark();
    let violet = theme.ink(Some(Tint::Violet));
    frame(&mut app, id);
    let scene = app.scene();
    let l = waves(&app, id).last_layout().clone();
    let (y, h) = l.entry_span(4).unwrap();
    let wave = Rect::from_xywh(l.waves.left(), y + 1.0, l.waves.width(), h - 2.0);
    assert!(inks_in(scene, wave).contains(&violet));
    // Dissolving both groups at once keeps every row violet.
    select(&mut app, id, &[1, 3]);
    app.handle(Command::Action(Action::Ungroup));
    assert_eq!(
        inks(&app, id),
        [None, Some(Tint::Violet), Some(Tint::Violet), None]
    );
}
