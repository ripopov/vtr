use vtr::{
    codec::Decompressor,
    declarations::{Declarations, Metadata, Node, NodeData},
    framing::Container,
    hierarchy::NodeId,
    schema::{FIRST_DECLARED_TYPE, Primitive},
};

#[test]
fn interning_after_incremental_reads_preserves_existing_ids_and_duplicate_entries() {
    use vtr::varint::{put_blob, put_u64};
    let chunk = |first, strings: &[&str]| {
        let mut out = Vec::new();
        put_u64(&mut out, first);
        put_u64(&mut out, strings.len() as u64);
        for text in strings {
            put_blob(&mut out, text.as_bytes());
        }
        out
    };
    let mut declarations = Declarations::default();
    declarations
        .read_strings(&chunk(0, &["", "a", "a"]))
        .unwrap();
    assert_eq!(declarations.intern("a").unwrap(), 1);
    assert_eq!(declarations.intern("b").unwrap(), 3);
    declarations.read_strings(&chunk(4, &["c", "b"])).unwrap();
    assert_eq!(declarations.intern("c").unwrap(), 4);
    assert_eq!(declarations.intern("b").unwrap(), 3);
    assert_eq!(declarations.intern("d").unwrap(), 6);
    assert_eq!(declarations.string(2).unwrap(), "a");
    assert_eq!(declarations.string(5).unwrap(), "b");
}

#[test]
fn cpp_declarations_round_trip_incremental_chunks_and_typed_sources() {
    let container =
        Container::from_bytes(include_bytes!("fixtures/typed.vtr").as_slice(), true).unwrap();
    let mut declarations = Declarations::default();
    for section in container.sections() {
        let payload = container.payload(section, true).unwrap();
        match section.kind {
            1 => {
                let metadata =
                    Metadata::decode(payload, declarations.strings.len() as u64).unwrap();
                assert_eq!(
                    metadata.encode(declarations.strings.len() as u64).unwrap(),
                    payload
                );
            }
            2 => {
                let raw = Decompressor::new().decompress(payload).unwrap();
                declarations.read_strings(&raw).unwrap();
                assert_eq!(
                    declarations.strings_chunk(section.aux0 as u32).unwrap(),
                    raw
                );
            }
            3 => {
                let raw = Decompressor::new().decompress(payload).unwrap();
                declarations.read_hierarchy(&raw).unwrap();
                assert_eq!(
                    declarations.hierarchy_chunk(section.aux0 as u32).unwrap(),
                    raw
                );
            }
            10 => {
                declarations.types.read_declarations(payload).unwrap();
                assert_eq!(
                    declarations
                        .types
                        .declarations(section.aux0 as u32)
                        .unwrap(),
                    payload
                );
            }
            _ => (),
        }
    }
    let (combined, _) = Declarations::from_container(&container).unwrap();
    assert_eq!(combined.nodes, declarations.nodes);
    assert_eq!(combined.strings, declarations.strings);
    assert!(declarations.types.len() > FIRST_DECLARED_TYPE as usize);
    for (index, node) in declarations.nodes.iter().enumerate() {
        if let NodeData::Generator { payload_type, .. } = node.data {
            assert_eq!(
                declarations
                    .generators
                    .get(index as u32)
                    .unwrap()
                    .payload_type,
                payload_type
            );
        }
    }
}

#[test]
fn invalid_chunks_leave_existing_declarations_intact() {
    let mut declarations = Declarations::new();
    declarations.intern("top").unwrap();
    let before = declarations.strings.clone();
    for chunk in [&[2, 1, 2, b'a'][..], &[3, 0], &[2, 1, 1, 255]] {
        assert!(declarations.read_strings(chunk).is_err());
        assert_eq!(declarations.strings, before);
    }
    declarations.nodes.push(Node {
        parent: None,
        name: 1,
        data: NodeData::Scope {
            scope_type: 0,
            component: 0,
        },
        attributes: vec![],
    });
    let source = declarations.hierarchy_chunk(0).unwrap();
    let mut target = Declarations::new();
    target.intern("top").unwrap();
    for end in 0..source.len() {
        assert!(target.read_hierarchy(&source[..end]).is_err());
        assert!(target.nodes.is_empty());
    }
    target.read_hierarchy(&source).unwrap();
    assert_eq!(target.nodes, declarations.nodes);
    assert_eq!(target.find("top"), Some(NodeId(0)));
    assert_eq!(target.find("missing"), None);
    let mut forward = declarations.clone();
    forward.nodes[0].parent = Some(NodeId(0));
    assert!(forward.hierarchy_chunk(0).is_err());
    let mut invalid = declarations.clone();
    invalid.nodes.push(Node {
        parent: Some(NodeId(0)),
        name: 1,
        data: NodeData::Generator {
            kind: 1,
            payload_type: Primitive::Enum.id(),
        },
        attributes: vec![],
    });
    assert!(invalid.hierarchy_chunk(0).is_err());
}
