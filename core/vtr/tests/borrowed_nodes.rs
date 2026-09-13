use vtr::*;

#[test]
fn node_views_borrow_large_payloads_and_owned_conversion_is_explicit() {
    let mut hierarchy = Hierarchy::new();
    let id = hierarchy.push(Node { parent: None, name: StrId(0), data: NodeData::EnumTable { entries: (0..10000).map(|i| (StrId(i), StrId(i + 1))).collect() }, attrs: vec![(StrId(1), Value::Bytes(vec![7; 65536]))] });
    hierarchy.build_index();
    let view = hierarchy.node_ref(id);
    assert!(std::ptr::eq(view.attrs, hierarchy.attrs(id)));
    match view.data {
        NodeDataRef::EnumTable { entries } => assert!(std::ptr::eq(entries, hierarchy.enum_entries(id).unwrap())),
        _ => panic!("expected enum table"),
    }
    assert_eq!(view.to_owned(), hierarchy.node(id));
    assert_eq!(view.kind(), NodeKind::EnumTable);
}
