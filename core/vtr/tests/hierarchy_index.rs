//! Dense identities retain declaration order and borrow reader membership.
use vtr::{hierarchy_index::NodeIndex, *};

#[test]
fn rank_select_and_bidirectional_children_cross_bitmap_words() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.vtr");
    let mut w = Writer::create(&path).unwrap();
    let root = w
        .add_scope(None, "root", ScopeType::Module, "CELL")
        .unwrap();
    let mut vars = Vec::new();
    let mut scopes = vec![root];
    for i in 0..180 {
        if i % 3 == 0 {
            scopes.push(
                w.add_scope(Some(root), &format!("child{i}"), ScopeType::Module, "")
                    .unwrap(),
            );
        } else {
            vars.push(
                w.add_var(
                    Some(root),
                    &format!("v{i}"),
                    VarType::Wire,
                    Direction::Input,
                    SignalKind::Bits {
                        width: 1,
                        states: 2,
                    },
                )
                .unwrap()
                .0,
            );
        }
    }
    w.close().unwrap();
    let r = Reader::open(path).unwrap();
    let h = r.hierarchy();
    for (kinds, expected) in [
        (&[NodeKind::Scope][..], scopes),
        (&[NodeKind::Var][..], vars),
    ] {
        let index = NodeIndex::new(h, kinds);
        assert_eq!(index.len(), expected.len());
        for node in h.ids() {
            assert_eq!(
                index.ordinal(node),
                expected.iter().position(|&id| id == node)
            );
        }
        assert_eq!(index.ordinal(NodeId(u32::MAX)), None);
        for (id, &node) in expected.iter().enumerate() {
            assert_eq!(index.node(id), node);
        }
        let expected: Vec<_> = expected
            .iter()
            .enumerate()
            .filter_map(|(id, &node)| (h.parent(node) == Some(root)).then_some(id))
            .collect();
        let children = index.children(h, root);
        assert_eq!(children.len(), expected.len());
        assert_eq!(children.is_empty(), expected.is_empty());
        assert_eq!(children.iter().collect::<Vec<_>>(), expected);
        assert_eq!(
            children.iter().rev().collect::<Vec<_>>(),
            expected.iter().rev().copied().collect::<Vec<_>>()
        );
        let mut iter = children.iter();
        let mut deque = std::collections::VecDeque::from(expected);
        while !deque.is_empty() {
            assert_eq!(iter.next(), deque.pop_front());
            if !deque.is_empty() {
                assert_eq!(iter.next_back(), deque.pop_back());
            }
        }
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }
    let empty = NodeIndex::new(h, &[NodeKind::Generator]);
    assert!(empty.is_empty());
    assert_eq!(empty.resident_bytes(), 0);
    assert!(empty.children(h, root).is_empty());
    for node in h.ids() {
        assert_eq!(empty.ordinal(node), None);
        assert_eq!(
            format!("{:?}", h.node_data(node)),
            format!("{:?}", h.node(node).data)
        );
    }
}
