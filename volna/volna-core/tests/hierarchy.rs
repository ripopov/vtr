use std::sync::Arc;
use volna_core::app::{App, Command, Event};
use volna_core::data::source::Lookup;
use volna_core::data::transactions::TrackRef;
use volna_core::data::{Hierarchy, Member, ScopeRole};
use volna_core::geometry::Modifiers;
use volna_core::icons::IconName;
use volna_core::remote::objects::Metadata;
use volna_core::session::{OpenSpec, Session};
use volna_core::sidebar::icons::{Tint, member_icon, scope_icon, scope_kind_icon, stream_tag};
use volna_core::sidebar::members::log_site;
use volna_core::sidebar::{Key, MemberListModel, ScopeTreeModel};

fn fixture() -> Arc<dyn Session> {
    let file = tempfile::Builder::new().suffix(".vtr").tempfile().unwrap();
    let mut w = vtr::Writer::create(file.path()).unwrap();
    let root = w.begin_scope("soc", vtr::ScopeType::Module, "soc_top");
    let (var, _) = w.add_var(
        "read_valid",
        vtr::VarType::Wire,
        vtr::Direction::Input,
        vtr::SignalKind::Bits {
            width: 1,
            states: 4,
        },
    );
    let table = w.add_enum_table("state_t", &[("IDLE", "0"), ("BUSY", "1")]);
    w.node_attr(var, "enum_table", vtr::Value::U64(table.0 as u64))
        .unwrap();
    let cpu = w.begin_scope("cpu", vtr::ScopeType::Core, "");
    let stream = w.add_stream(Some(cpu), "thread0", "PIPELINE");
    w.add_generator(stream, "instructions");
    w.end_scope().unwrap();
    let bus = w.add_stream(Some(root), "read_bus", "TRANSACTOR");
    w.add_generator(bus, "read_request");
    w.add_generator(bus, "write_request");
    let log = w.add_log_stream(Some(root), "log");
    let site = w.add_generator(log, "read_failed at pc=%x");
    w.node_attr(site, "log.severity", vtr::Value::U64(4))
        .unwrap();
    let filename = w.intern("soc.sv");
    w.node_attr(site, "log.file", vtr::Value::Str(filename))
        .unwrap();
    w.node_attr(site, "log.line", vtr::Value::U64(42)).unwrap();
    w.add_stream(Some(root), "empty", "otel.scope");
    w.end_scope().unwrap();
    w.close().unwrap();
    if let Some(path) = std::env::var_os("VOLNA_HIERARCHY_FIXTURE") {
        std::fs::copy(file.path(), path).unwrap();
    }
    OpenSpec::Path(file.path().into()).open().unwrap()
}

fn scope(h: &Hierarchy, path: &[&str]) -> usize {
    match h.find_scope(path) {
        Lookup::Found(id) => id,
        other => panic!("{other:?}"),
    }
}

#[test]
fn mixed_vtr_metadata_icons_and_log_provenance() {
    let session = fixture();
    let h = session.hierarchy();
    let pipeline = scope(h, &["soc", "cpu", "thread0"]);
    assert_eq!(
        scope_icon(&h.scopes[pipeline]),
        (IconName::Workflow, Tint::Pipeline)
    );
    assert_eq!(h.scopes[0].component, "soc_top");
    let mut list = MemberListModel::default();
    list.set_scope(Some(h), Some(pipeline));
    assert_eq!(list.title(Some(h)), "Generators");
    assert_eq!(list.rows, [Member::Generator(0)]);
    assert_eq!(member_icon(h, list.rows[0]), IconName::CircleDot);
    assert_eq!(member_icon(h, Member::Var(0)), IconName::Tags);
    assert_eq!(
        h.find_generator(&["soc", "cpu", "thread0", "instructions"]),
        Lookup::Found(0)
    );
    for member in h
        .generators
        .iter()
        .enumerate()
        .map(|(id, _)| Member::Generator(id))
    {
        let track = session
            .tracks()
            .iter()
            .find(|t| Some(t.id) == h.member_track(member))
            .unwrap();
        assert_eq!(track.path.join("."), h.member_path(member));
    }
    let bus = scope(h, &["soc", "read_bus"]);
    assert_eq!(stream_tag(&h.scopes[bus]), Some("TRANSACTOR"));
    let log = scope(h, &["soc", "log"]);
    list.set_scope(Some(h), Some(log));
    assert_eq!(list.title(Some(h)), "Log sites");
    let site = log_site(h, list.rows[0]).unwrap();
    assert_eq!(
        (site.severity.as_str(), site.file, site.line),
        ("error", Some("soc.sv"), Some(42))
    );
    list.set_scope(Some(h), Some(scope(h, &["soc", "empty"])));
    assert_eq!(
        list.placeholder(Some(h)),
        Some("This stream declares no generators")
    );
    Metadata::from_session(session.as_ref()).validate().unwrap();
}

#[test]
fn search_order_activation_keyboard_and_notices() {
    let session = fixture();
    let mut app = App::new();
    app.set_session(session.clone());
    app.handle(Command::SetSearchEverywhere(true));
    app.handle(Command::SetFilter("read_".into()));
    let rows = app.variables.rows.clone();
    assert!(matches!(
        rows.as_slice(),
        [
            Member::Var(_),
            Member::Generator(_),
            Member::Generator(_),
            Member::Stream(_)
        ]
    ));
    assert!(app.variables.show_scope());
    app.handle(Command::AddSelectedOrAllVars);
    assert_eq!(app.panels.focused_waves().unwrap().items.len(), 1);
    app.take_requests();
    app.take_events();
    // A generator opens a pipeline panel (one track load); a log site only
    // reports a notice.
    app.handle(Command::ActivateMembers(vec![rows[1], rows[2]]));
    let requests = app.take_requests();
    assert!(matches!(
        requests.as_slice(),
        [volna_core::session::LoadRequest::Track { .. }]
    ));
    assert_eq!(app.panels.len(), 2);
    assert!(app.panels.focused().kind.pipeline().is_some());
    assert!(app.status().sidebar_notice.unwrap().contains("Log sites"));
    assert!(
        app.take_events()
            .iter()
            .any(|e| matches!(e, Event::Notice(_)))
    );
    let pipeline = scope(session.hierarchy(), &["soc", "cpu", "thread0"]);
    app.handle(Command::SelectScope(pipeline));
    assert!(!app.variables.search_everywhere);
    app.handle(Command::SetFilter(String::new()));
    app.handle(Command::ScopesKey(Key::Enter));
    assert!(app.status().sidebar_notice.is_none());
    assert_eq!(app.panels.len(), 3);
    assert_eq!(
        app.panels.focused().kind.pipeline().unwrap().track.path(),
        ["soc", "cpu", "thread0"]
    );
    app.handle(Command::ScopesKey(Key::Left));
    assert_eq!(
        app.scopes.selected,
        Some(scope(session.hierarchy(), &["soc", "cpu"]))
    );
    app.handle(Command::SetFilter("read_".into()));
    app.handle(Command::VariablesKey(Key::Escape, Modifiers::default()));
    assert!(app.variables.filter.is_empty());
}

#[test]
fn search_cap_counts_all_kinds_and_deep_trees_are_iterative() {
    let session = fixture();
    let mut h = session.hierarchy().clone();
    let template = h.vars[0].clone();
    h.vars = vec![template; 5000];
    let mut list = MemberListModel::default();
    list.set_filter(Some(&h), "read_");
    assert_eq!(list.rows.len(), 5000);
    assert!(list.truncated);
    h.vars.truncate(4996);
    list.rebuild(Some(&h));
    assert_eq!(list.rows.len(), 4999);
    assert!(!list.truncated);
    let mut h = Hierarchy::default();
    let mut parent = None;
    for _ in 0..20000 {
        parent = Some(h.push_scope("nested".into(), "module".into(), parent));
    }
    let mut tree = ScopeTreeModel::default();
    tree.set_all(&h, true);
    assert_eq!(tree.visible.last(), Some(&(19999, 19999)));
}

#[test]
fn stream_workspace_paths_roundtrip_and_unresolved_paths_survive() {
    use volna_core::workspace::Workspace;
    let session = fixture();
    let mut app = App::new();
    app.set_session(session.clone());
    app.handle(Command::SelectScope(scope(
        session.hierarchy(),
        &["soc", "cpu", "thread0"],
    )));
    app.handle(Command::ExpandAllScopes(true));
    let capture = |app: &App| Workspace::capture(app, "trace.vtr".into(), None).unwrap();
    let saved = capture(&app);
    let before = serde_json::to_value(&saved).unwrap();
    app.handle(Command::SetSearchEverywhere(true));
    saved
        .prepare(
            &app,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    assert_eq!(serde_json::to_value(capture(&app)).unwrap(), before);
    assert!(!app.variables.search_everywhere);
    let mut saved = before;
    saved["sidebar"]["selected_scope"] = serde_json::json!(["missing", "stream"]);
    saved["sidebar"]["expanded"] = serde_json::json!([["missing", "stream"]]);
    Workspace::parse(&serde_json::to_vec(&saved).unwrap())
        .unwrap()
        .prepare(
            &app,
            "file:///tmp/trace.vtr",
            "file:///tmp/trace.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    assert_eq!(serde_json::to_value(capture(&app)).unwrap(), saved);
}

#[test]
fn icons_have_assets_and_malformed_track_references_are_rejected() {
    for code in 0..=255 {
        let icon = scope_kind_icon(vtr::ScopeType::from_code(code).name());
        assert!(!icon.svg().is_empty());
        assert_eq!(IconName::from_path(icon.path()), Some(icon));
    }
    for icon in IconName::ALL {
        assert!(!icon.svg().is_empty());
    }
    let session = fixture();
    let mut metadata = Metadata::from_session(session.as_ref());
    metadata.hierarchy.generators[0].stream = usize::MAX;
    assert!(metadata.validate().is_err());
    let mut metadata = Metadata::from_session(session.as_ref());
    metadata.hierarchy.scopes[2].role = ScopeRole::Stream {
        track: TrackRef(u32::MAX),
    };
    assert!(metadata.validate().is_err());
}
