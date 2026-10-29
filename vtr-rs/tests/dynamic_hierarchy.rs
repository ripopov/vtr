use vtr::*;

#[test]
fn late_scopes_sources_aliases_and_attributes_preserve_tree_order() {
    for mode in [ExecutionMode::Inline, ExecutionMode::Threaded] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hierarchy.vtr");
        let mut writer = Writer::create_with(
            &path,
            WriterOptions {
                mode,
                ..WriterOptions::default()
            },
        )
        .unwrap();
        let root = writer
            .scope_with(SourceOptions::new("root", None), ScopeType::Core, "CELL")
            .unwrap();
        let root_wire = writer
            .signal(
                SourceOptions::new("wire", Some(root)),
                SignalKind::Bits {
                    width: 8,
                    states: 2,
                },
                VarType::Wire,
                Direction::Input,
            )
            .unwrap();
        let mut children = vec![NodeId(root.0 + 1)];
        let mut scopes = Vec::new();
        let key = writer.intern("description").unwrap();
        for index in 0..180 {
            writer.advance_time(index).unwrap();
            writer.emit_u64(root_wire, index).unwrap();
            writer.flush().unwrap();
            let scope = writer.scope(format!("child{index}"), Some(root)).unwrap();
            writer
                .node_attribute(scope, key, Value::Text(format!("scope {index}")))
                .unwrap();
            children.push(scope);
            scopes.push(scope);
            writer
                .alias(
                    root_wire,
                    SourceOptions::new("alias", Some(scope)),
                    VarType::Wire,
                    Direction::Input,
                )
                .unwrap();
            let source = writer
                .transactions::<u32>(SourceOptions::new("tx", Some(scope)), "REQUEST")
                .unwrap();
            let tx = writer.begin(source, &(index as u32), None).unwrap();
            writer.end(tx, Status::Ok).unwrap();
        }
        writer.close().unwrap();
        let reader = Reader::open(path).unwrap();
        assert_eq!(reader.children(None), &[root]);
        assert_eq!(reader.children(Some(root)), children);
        for (index, scope) in scopes.into_iter().enumerate() {
            assert_eq!(
                reader.full_path(scope).unwrap(),
                format!("root.child{index}")
            );
            assert_eq!(reader.children(Some(scope)).len(), 2);
            assert!(reader.is_within(scope, Some(root)));
            assert_eq!(
                reader.find_signal(&format!("root.child{index}.alias")),
                Some(root_wire)
            );
            assert_eq!(
                reader.node(scope).unwrap().attributes[0].value,
                Value::Text(format!("scope {index}"))
            );
        }
        let (_, sizes) = reader.scope_sizes();
        assert_eq!(
            (sizes.signals(0), sizes.variables(0), sizes.scopes(0)),
            (1, 181, 181)
        );
        assert!(reader.children(Some(NodeId(u32::MAX))).is_empty());
        reader.validate().unwrap();
    }
}

#[test]
fn declarations_reject_non_scope_parents_and_foreign_handles() {
    let directory = tempfile::tempdir().unwrap();
    let options = WriterOptions {
        mode: ExecutionMode::Inline,
        ..WriterOptions::default()
    };
    let mut a = Writer::create_with(directory.path().join("a.vtr"), options.clone()).unwrap();
    let mut b = Writer::create_with(directory.path().join("b.vtr"), options).unwrap();
    let foreign = b
        .transactions::<u64>(SourceOptions::new("foreign", None), "REQUEST")
        .unwrap();
    assert!(a.begin(foreign, &1, None).is_err());
    let source = a
        .transactions::<()>(SourceOptions::new("source", None), "REQUEST")
        .unwrap();
    assert!(a.scope("bad", Some(NodeId(source.id()))).is_err());
    assert!(a.flush().is_err());
    a.seal().unwrap();
    b.close().unwrap();
}
