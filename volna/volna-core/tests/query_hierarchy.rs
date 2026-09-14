#![cfg(not(target_family = "wasm"))]
use std::sync::Arc;
use volna_core::data::query_hierarchy::QueryHierarchy;
use vtr_query::{
    Budget, Cancellation, Error,
    metadata::{DeclarationData, Text},
    native_session::Session,
    session::{Delivery, Query, Reply},
    wave::Limits,
};
fn fixture() -> (tempfile::TempDir, vtr::Reader, u32, u32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hierarchy.vtr");
    let mut w = vtr::Writer::create(&path).unwrap();
    let root = w.begin_scope("literal.root", vtr::ScopeType::Module, "component");
    let (_, signal) = w.add_bits("same", 32, 4);
    w.add_alias("same", vtr::VarType::Reg, vtr::Direction::Input, signal)
        .unwrap();
    let nested = w.begin_scope("nested", vtr::ScopeType::Module, "");
    w.add_bits("child", 1, 2);
    w.end_scope().unwrap();
    w.add_bits(&"long界".repeat(2000), 1, 2);
    w.end_scope().unwrap();
    w.close().unwrap();
    (dir, vtr::Reader::open(path).unwrap(), root.0, nested.0)
}
fn begin(session: &mut Session<'_>, parent: Option<u32>) -> Arc<Delivery> {
    let cursor = session
        .start(
            Query::Children { parent },
            Limits {
                records: 1,
                bytes: 2048,
                work: 1,
            },
            Cancellation::default(),
        )
        .unwrap();
    session.advance(cursor).unwrap()
}
fn append_all(tree: &mut QueryHierarchy, session: &mut Session<'_>, mut page: Arc<Delivery>) {
    let first = page.request;
    loop {
        assert!(tree.append(page.clone()).unwrap());
        assert!(
            !tree.append(page.clone()).unwrap(),
            "retry must share the accepted page"
        );
        let Some(next) = page.next else { break };
        page = session.advance(next).unwrap();
    }
    session.release(first).unwrap();
}
#[test]
fn demanded_pages_preserve_raw_identity_aliases_and_reference_names_without_copying() {
    let (_dir, reader, root, nested) = fixture();
    let query_budget = Budget::new(1 << 20);
    let index_budget = Budget::new(16384);
    let mut session = Session::new(&reader, query_budget.clone(), 8).unwrap();
    let mut tree = QueryHierarchy::new(session.info(), 6, &index_budget).unwrap();
    assert!(tree.is_empty());
    assert_eq!(tree.state(None), None);
    let roots = begin(&mut session, None);
    let Reply::Children(page) = &roots.reply else {
        panic!("children")
    };
    let original_name = page.declarations()[0].name.as_str().unwrap().unwrap();
    tree.append(roots.clone()).unwrap();
    assert_eq!(tree.len(), 1, "loading roots must not fetch descendants");
    let name = tree
        .declaration(root)
        .unwrap()
        .name
        .as_str()
        .unwrap()
        .unwrap();
    assert_eq!(
        name.as_ptr(),
        original_name.as_ptr(),
        "raw text backing is shared"
    );
    session.release(roots.request).unwrap();
    drop(roots);
    let children = begin(&mut session, Some(root));
    append_all(&mut tree, &mut session, children);
    assert_eq!(tree.len(), 5);
    let state = tree.state(Some(root)).unwrap();
    assert!(state.complete);
    assert_eq!(state.received, 4);
    assert_eq!(
        tree.children(Some(nested)).count(),
        0,
        "collapsed scope remains unloaded"
    );
    assert!(tree.state(Some(nested)).is_none());
    let records: Vec<_> = tree.children(Some(root)).collect();
    assert_eq!(records[0].name.as_str().unwrap(), Some("same"));
    assert_eq!(records[1].name.as_str().unwrap(), Some("same"));
    match (&records[0].data, &records[1].data) {
        (
            DeclarationData::Variable {
                signal: a,
                alias: false,
                ..
            },
            DeclarationData::Variable {
                signal: b,
                alias: true,
                ..
            },
        ) => assert_eq!(a, b),
        _ => panic!("alias semantics must survive paging"),
    }
    assert!(matches!(records[3].name, Text::Reference { .. }));
    let nested_page = begin(&mut session, Some(nested));
    append_all(&mut tree, &mut session, nested_page);
    assert_eq!(tree.len(), 6);
    assert_eq!(
        tree.children(Some(nested))
            .next()
            .unwrap()
            .name
            .as_str()
            .unwrap(),
        Some("child")
    );
    drop(session);
    assert!(
        query_budget.used() > 0,
        "page payloads stay admitted while the tree retains them"
    );
    tree.clear();
    assert_eq!(query_budget.used(), 0);
    assert!(tree.is_empty());
    assert!(
        index_budget.used() > 0,
        "fixed arrays remain admitted for reuse"
    );
    drop(tree);
    assert_eq!(index_budget.used(), 0);
}
#[test]
fn missing_parents_foreign_snapshots_and_skipped_pages_do_not_change_the_tree() {
    let (_dir, reader, root, _) = fixture();
    let mut session = Session::new(&reader, Budget::new(1 << 20), 8).unwrap();
    let mut tree = QueryHierarchy::new(session.info(), 6, &Budget::new(16384)).unwrap();
    let first = begin(&mut session, Some(root));
    assert!(matches!(tree.append(first.clone()), Err(Error::Invalid(_))));
    assert!(tree.is_empty());
    let roots = begin(&mut session, None);
    append_all(&mut tree, &mut session, roots);
    let next = session.advance(first.next.unwrap()).unwrap();
    assert!(matches!(tree.append(next.clone()), Err(Error::Invalid(_))));
    assert_eq!(tree.len(), 1);
    tree.append(first.clone()).unwrap();
    let skipped = session.advance(next.next.unwrap()).unwrap();
    assert!(matches!(tree.append(skipped), Err(Error::Invalid(_))));
    assert_eq!(tree.state(Some(root)).unwrap().received, 1);
    tree.append(next).unwrap();
    let mut foreign = Session::new(&reader, Budget::new(1 << 20), 1).unwrap();
    let page = begin(&mut foreign, None);
    assert!(matches!(tree.append(page), Err(Error::Invalid(_))));
    assert_eq!(tree.len(), 3);
}
#[test]
fn capacity_refusal_preserves_the_prefix_and_its_continuation() {
    let (_dir, reader, root, _) = fixture();
    let mut session = Session::new(&reader, Budget::new(1 << 20), 4).unwrap();
    assert!(matches!(
        QueryHierarchy::new(session.info(), 6, &Budget::new(0)),
        Err(Error::ResourceLimit)
    ));
    let mut tree = QueryHierarchy::new(session.info(), 2, &Budget::new(16384)).unwrap();
    let roots = begin(&mut session, None);
    append_all(&mut tree, &mut session, roots);
    let first = begin(&mut session, Some(root));
    tree.append(first.clone()).unwrap();
    let before = tree.state(Some(root));
    let next = session.advance(first.next.unwrap()).unwrap();
    assert!(matches!(tree.append(next), Err(Error::ResourceLimit)));
    assert_eq!(tree.state(Some(root)), before);
    assert_eq!(tree.len(), 2);
}

#[test]
fn referenced_names_assemble_across_utf8_boundaries_under_a_byte_cap() {
    use volna_core::data::query_hierarchy::QueryText;
    let (_dir, reader, root, _) = fixture();
    let mut session = Session::new(&reader, Budget::new(1 << 20), 4).unwrap();
    let mut page = begin(&mut session, Some(root));
    let (id, bytes) = loop {
        let Reply::Children(children) = &page.reply else {
            panic!("children")
        };
        if let Some((id, bytes)) = children.declarations().iter().find_map(|node| {
            if let Text::Reference { id, bytes } = node.name {
                Some((id, bytes))
            } else {
                None
            }
        }) {
            break (id, bytes);
        }
        page = session.advance(page.next.unwrap()).unwrap();
    };
    session.release(page.request).unwrap();
    let budget = Budget::new(bytes as usize);
    assert!(matches!(
        QueryText::new(
            session.info().snapshot,
            id,
            bytes,
            bytes as usize - 1,
            &budget
        ),
        Err(Error::ResourceLimit)
    ));
    assert_eq!(budget.used(), 0);
    let mut text =
        QueryText::new(session.info().snapshot, id, bytes, bytes as usize, &budget).unwrap();
    assert_eq!(budget.used(), bytes as usize);
    let limits = Limits {
        records: 1,
        bytes: 2048,
        work: 32,
    };
    let make_part = |session: &mut Session<'_>, offset| {
        let cursor = session
            .start(
                Query::Text {
                    id,
                    offset,
                    length: 7,
                },
                limits,
                Cancellation::default(),
            )
            .unwrap();
        session.advance(cursor).unwrap()
    };
    let out_of_order = make_part(&mut session, 1);
    assert!(text.append(&out_of_order).is_err());
    session.release(out_of_order.request).unwrap();
    assert_eq!(text.offset(), 0);
    let mut other = Session::new(&reader, Budget::new(1 << 20), 1).unwrap();
    let foreign = make_part(&mut other, 0);
    assert!(text.append(&foreign).is_err());
    while text.remaining() > 0 {
        assert!(text.text().is_none());
        let part = make_part(&mut session, text.offset());
        assert!(text.append(&part).unwrap());
        assert!(!text.append(&part).unwrap());
        session.release(part.request).unwrap();
    }
    assert_eq!(text.text().unwrap(), "long界".repeat(2000));
    drop(text);
    assert_eq!(budget.used(), 0);
}

#[test]
fn scope_tree_expands_unloaded_scopes_and_refreshes_without_losing_selection() {
    use volna_core::sidebar::{Key, ScopeTreeModel, scopes::ScopeHierarchy};
    let (_dir, reader, root, nested) = fixture();
    let mut session = Session::new(&reader, Budget::new(1 << 20), 8).unwrap();
    let mut tree = QueryHierarchy::new(session.info(), 6, &Budget::new(16384)).unwrap();
    let roots = begin(&mut session, None);
    append_all(&mut tree, &mut session, roots);
    let mut model = ScopeTreeModel::new();
    model.reset(Some(&tree));
    assert_eq!(model.visible, vec![(root as usize, 0)]);
    assert!(tree.has_child_scopes(root as usize));
    model.key(&tree, &Key::Left);
    assert!(!model.is_expanded(root as usize));
    model.key(&tree, &Key::Right);
    assert!(
        model.is_expanded(root as usize),
        "unloaded children remain expandable"
    );
    let children = begin(&mut session, Some(root));
    append_all(&mut tree, &mut session, children);
    model.refresh(Some(&tree));
    assert_eq!(model.selected, Some(root as usize));
    assert_eq!(
        model.visible,
        vec![(root as usize, 0), (nested as usize, 1)]
    );
    model.key(&tree, &Key::Down);
    model.key(&tree, &Key::Right);
    assert_eq!(model.selected, Some(nested as usize));
    assert!(model.is_expanded(nested as usize));
    let child = begin(&mut session, Some(nested));
    append_all(&mut tree, &mut session, child);
    model.refresh(Some(&tree));
    assert!(
        !tree.has_child_scopes(nested as usize),
        "a completed variables-only scope has no scope children"
    );
    assert_eq!(model.selected, Some(nested as usize));
    model.key(&tree, &Key::Left);
    model.key(&tree, &Key::Left);
    assert_eq!(model.selected, Some(root as usize));
}

#[test]
fn variable_refresh_preserves_identity_when_earlier_declarations_arrive_late() {
    use volna_core::geometry::Modifiers;
    use volna_core::sidebar::VariableListModel;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out-of-order.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let left = writer.begin_scope("left", vtr::ScopeType::Module, "");
    let (a, _) = writer.add_bits("x_left", 1, 2);
    writer.end_scope().unwrap();
    let right = writer.begin_scope("right", vtr::ScopeType::Module, "");
    let (b, _) = writer.add_bits("x_right", 1, 2);
    writer.end_scope().unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let mut session = Session::new(&reader, Budget::new(1 << 20), 4).unwrap();
    let mut tree = QueryHierarchy::new(session.info(), 4, &Budget::new(16384)).unwrap();
    let roots = begin(&mut session, None);
    append_all(&mut tree, &mut session, roots);
    let right_page = begin(&mut session, Some(right.0));
    append_all(&mut tree, &mut session, right_page);
    let mut model = VariableListModel::new();
    model.set_filter(Some(&tree), "x_");
    assert_eq!(model.rows, vec![b.0 as usize]);
    assert!(
        !model.complete,
        "partial local search cannot claim complete results"
    );
    model.select(0, Modifiers::default());
    let left_page = begin(&mut session, Some(left.0));
    append_all(&mut tree, &mut session, left_page);
    model.refresh(Some(&tree));
    assert_eq!(model.rows, vec![a.0 as usize, b.0 as usize]);
    assert_eq!(model.selected_or_all(), vec![b.0 as usize]);
    assert_eq!(model.anchor, Some(1));
    assert!(model.complete);
}

#[test]
fn completing_a_referenced_name_refreshes_filter_results_and_keeps_its_byte_charge() {
    use volna_core::data::query_hierarchy::QueryText;
    use volna_core::sidebar::VariableListModel;
    let (_dir, reader, root, _) = fixture();
    let mut session = Session::new(&reader, Budget::new(1 << 20), 4).unwrap();
    let mut tree = QueryHierarchy::new(session.info(), 6, &Budget::new(16384)).unwrap();
    let roots = begin(&mut session, None);
    append_all(&mut tree, &mut session, roots);
    let children = begin(&mut session, Some(root));
    append_all(&mut tree, &mut session, children);
    let node = tree.children(Some(root)).last().unwrap();
    let var = node.id as usize;
    let Text::Reference { id, bytes } = node.name else {
        panic!("referenced name")
    };
    let text_budget = Budget::new(bytes as usize);
    let mut text = QueryText::new(
        session.info().snapshot,
        id,
        bytes,
        bytes as usize,
        &text_budget,
    )
    .unwrap();
    let mut model = VariableListModel::new();
    model.set_scope(Some(&tree), Some(root as usize));
    assert_eq!(
        model.rows.len(),
        3,
        "unfetched names still represent variables"
    );
    assert!(!model.complete);
    model.set_filter(Some(&tree), "long界");
    assert!(model.rows.is_empty());
    assert!(!model.complete);
    while text.remaining() > 0 {
        let cursor = session
            .start(
                Query::Text {
                    id,
                    offset: text.offset(),
                    length: 1023,
                },
                Limits {
                    records: 1,
                    bytes: 2048,
                    work: 1,
                },
                Cancellation::default(),
            )
            .unwrap();
        let part = session.advance(cursor).unwrap();
        text.append(&part).unwrap();
        session.release(cursor).unwrap();
    }
    tree.install_text(text).unwrap();
    assert!(
        tree.text(&Text::Reference {
            id,
            bytes: bytes + 1
        })
        .is_none()
    );
    model.refresh(Some(&tree));
    assert_eq!(model.rows, vec![var]);
    assert!(model.complete);
    assert_eq!(text_budget.used(), bytes as usize);
    tree.clear();
    assert_eq!(text_budget.used(), 0);
}

#[test]
fn deeply_expanded_scope_topology_does_not_use_the_call_stack() {
    use volna_core::sidebar::{ScopeTreeModel, scopes::ScopeHierarchy};
    struct Chain(usize);
    impl ScopeHierarchy for Chain {
        fn root_scopes(&self) -> impl Iterator<Item = usize> + '_ {
            std::iter::once(0)
        }
        fn child_scopes(&self, id: usize) -> impl Iterator<Item = usize> + '_ {
            (id + 1 < self.0).then_some(id + 1).into_iter()
        }
        fn scope_ids(&self) -> impl Iterator<Item = usize> + '_ {
            0..self.0
        }
        fn parent_scope(&self, id: usize) -> Option<usize> {
            id.checked_sub(1)
        }
        fn has_child_scopes(&self, id: usize) -> bool {
            id + 1 < self.0
        }
    }
    let hierarchy = Chain(20000);
    let mut model = ScopeTreeModel::new();
    model.reset(Some(&hierarchy));
    model.set_all(&hierarchy, true);
    assert_eq!(model.visible.len(), hierarchy.0);
    assert_eq!(model.visible.last(), Some(&(19999, 19999)));
}

#[test]
fn workspace_rows_and_scopes_resolve_as_pages_arrive() {
    use volna_core::{
        App, Command, data::SignalShape, wave::model::RowSource, workspace::Workspace,
    };
    let (_dir, reader, root, nested) = fixture();
    let budget = Budget::new(4 << 20);
    let mut session = Session::new(&reader, budget.clone(), 8).unwrap();
    let mut tree = QueryHierarchy::new(session.info(), 16, &budget).unwrap();
    for parent in [None, Some(root), Some(nested)] {
        let page = begin(&mut session, parent);
        append_all(&mut tree, &mut session, page);
    }
    let mut original = App::new();
    original
        .set_query_document("hierarchy.vtr".into(), session.info(), 16, &budget)
        .unwrap();
    assert!(
        original
            .doc
            .install_query_hierarchy(original.doc.generation(), tree)
    );
    original.refresh_query_metadata();
    let vars: Vec<_> = original
        .doc
        .query_hierarchy()
        .unwrap()
        .children(Some(root))
        .filter(|node| {
            matches!(node.data, DeclarationData::Variable { .. })
                && node.name.as_str().unwrap() == Some("same")
        })
        .map(|node| node.id as usize)
        .collect();
    original.handle(Command::SelectScope(nested as usize));
    original.handle(Command::AddVars(vars.clone()));
    let saved = Workspace::capture(&original, "hierarchy.vtr".into(), None).unwrap();
    let mut app = App::new();
    app.set_query_document("hierarchy.vtr".into(), session.info(), 16, &budget)
        .unwrap();
    saved
        .prepare(
            &app,
            "file:///tmp/hierarchy.vtr",
            "file:///tmp/hierarchy.vtr.volna.json",
        )
        .unwrap()
        .commit(&mut app)
        .unwrap();
    assert!(
        app.panels
            .focused_waves()
            .unwrap()
            .items
            .iter()
            .all(|row| matches!(row.source, RowSource::Unresolved { .. }))
    );
    let selection = app.panels.focused_waves().unwrap().selected.clone();
    let generation = app.doc.generation();
    // Refreshing empty metadata must not discard saved selected/expanded paths.
    app.refresh_query_metadata();
    for parent in [None, Some(root)] {
        let mut delivery = begin(&mut session, parent);
        let first = delivery.request;
        loop {
            app.doc
                .append_query_children(generation, delivery.clone())
                .unwrap();
            app.refresh_query_metadata();
            let Some(next) = delivery.next else { break };
            delivery = session.advance(next).unwrap();
        }
        session.release(first).unwrap();
    }
    assert_eq!(app.scopes.selected, Some(nested as usize));
    assert_eq!(app.variables.scope, Some(nested as usize));
    let waves = app.panels.focused_waves().unwrap();
    assert_eq!(waves.selected, selection);
    assert_eq!(waves.items.len(), 2);
    for (row, var) in waves.items.iter().zip(vars) {
        assert!(matches!(row.source, RowSource::Resolved { var: actual, .. } if actual == var));
        assert_eq!(row.shape, SignalShape::Vector { width: 32 });
        assert_eq!(row.name, "same");
        assert_eq!(row.scope, "literal.root");
        assert!(row.history.is_none());
    }
    assert_eq!(
        waves.items[0].source.signal(),
        waves.items[1].source.signal()
    );
    assert!(app.take_requests().is_empty());
    Workspace::capture(&app, "hierarchy.vtr".into(), None).unwrap();
}
