//! Compact hierarchy parity and protocol-v5 admission/lifecycle tests.
use std::sync::Arc;
use volna_core::data::{Hierarchy, ScopeSizes};
use volna_core::remote::{
    ClientStep,
    hierarchy::{Header, PAGE_ENTRIES, Page},
    memory::MemoryBudget,
    open::OpenTransfer,
    transport::{Body, DATA_BYTES, ObjectId, Packet},
};
use volna_core::session::{LoadResult, OpenSpec, Session};
use volna_core::trace::TraceId;

fn objects(session: &dyn Session) -> Vec<(ObjectId, Vec<u8>)> {
    let header = Header::from_session(session).unwrap();
    let sizes = ScopeSizes::count(session.hierarchy());
    let mut objects = vec![(ObjectId::Metadata, bincode::serialize(&header).unwrap())];
    for p in 0..header.scope_pages() {
        objects.push((
            ObjectId::Scopes(p),
            bincode::serialize(&Page::scopes(session.hierarchy(), &sizes, p)).unwrap(),
        ));
    }
    for p in 0..header.var_pages() {
        objects.push((
            ObjectId::Variables(p),
            bincode::serialize(&Page::vars(session.hierarchy(), p)).unwrap(),
        ));
    }
    objects
}
fn transfer(budget: &MemoryBudget) -> OpenTransfer {
    OpenTransfer::new(7, TraceId::A, 11, 64 * 1024 * 1024, budget.clone()).unwrap()
}
fn accept(
    transfer: &mut OpenTransfer,
    sequence: &mut u64,
    body: Body,
) -> anyhow::Result<Option<Arc<dyn Session>>> {
    let packet = Packet {
        session: 31,
        request: 7,
        sequence: *sequence,
        body,
    };
    *sequence += 1;
    let expected = volna_core::remote::transport::acknowledgement(&packet);
    let mut step = transfer.accept(packet)?;
    loop {
        match step {
            ClientStep::Yield => step = transfer.step()?,
            ClientStep::Ack(ack) => {
                assert_eq!(ack, expected);
                return Ok(None);
            }
            ClientStep::Complete {
                ack,
                result:
                    LoadResult::Opened {
                        generation, result, ..
                    },
            } => {
                assert_eq!(ack, expected);
                assert_eq!(generation, 11);
                return result.map(Some);
            }
            _ => panic!("Open result"),
        }
    }
}
fn send(
    transfer: &mut OpenTransfer,
    sequence: &mut u64,
    id: ObjectId,
    bytes: Vec<u8>,
    chunk: usize,
) -> anyhow::Result<Option<Arc<dyn Session>>> {
    assert!(
        accept(
            transfer,
            sequence,
            Body::Begin {
                object: id,
                decoded_bytes: bytes.len() as u64
            }
        )?
        .is_none()
    );
    for (i, bytes) in bytes.chunks(chunk).enumerate() {
        assert!(
            accept(
                transfer,
                sequence,
                Body::Data {
                    offset: (i * chunk) as u64,
                    bytes: bytes.into()
                }
            )?
            .is_none()
        );
    }
    accept(transfer, sequence, Body::End)
}
fn roundtrip(session: Arc<dyn Session>, chunk: usize, budget: &MemoryBudget) -> Arc<dyn Session> {
    let mut transfer = transfer(budget);
    let mut sequence = 0;
    let mut result = None;
    let objects = objects(session.as_ref());
    for (i, (id, bytes)) in objects.iter().enumerate() {
        result = send(&mut transfer, &mut sequence, *id, bytes.clone(), chunk).unwrap();
        assert_eq!(
            result.is_some(),
            i == objects.len() - 1,
            "publication requires the last End"
        );
    }
    transfer.finish().unwrap();
    result.unwrap()
}
fn equal(a: &Hierarchy, b: &Hierarchy) {
    assert_eq!(a.scope_count(), b.scope_count());
    assert_eq!(a.var_count(), b.var_count());
    assert_eq!(
        a.roots().iter().collect::<Vec<_>>(),
        b.roots().iter().collect::<Vec<_>>()
    );
    for id in 0..a.scope_count() {
        let (a, b) = (a.scope(id), b.scope(id));
        assert_eq!(
            (a.name, a.kind, a.component, a.parent, a.role),
            (b.name, b.kind, b.component, b.parent, b.role)
        );
        assert_eq!(
            a.children.iter().collect::<Vec<_>>(),
            b.children.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            a.vars.iter().collect::<Vec<_>>(),
            b.vars.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            a.generators.iter().collect::<Vec<_>>(),
            b.generators.iter().collect::<Vec<_>>()
        );
    }
    for id in 0..a.var_count() {
        let (a, b) = (a.var(id), b.var(id));
        assert_eq!(
            (
                a.name,
                a.scope,
                a.shape,
                a.var_type,
                a.direction,
                a.signal,
                a.enum_table
            ),
            (
                b.name,
                b.scope,
                b.shape,
                b.var_type,
                b.direction,
                b.signal,
                b.enum_table
            )
        );
    }
    assert_eq!(
        bincode::serialize(a.generators()).unwrap(),
        bincode::serialize(b.generators()).unwrap()
    );
}

#[test]
fn every_fixture_roundtrips_column_for_column_with_sizes_and_no_client_census() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [
        "../volna/examples/counter.vtr",
        "../volna/examples/picorv32.vtr",
        "../volna/examples/landing.vtr",
        "tests/fixtures/values.fst",
        "tests/fixtures/values-wrapped.fst",
    ] {
        let local = OpenSpec::Path(root.join(path)).open().unwrap();
        let budget = MemoryBudget::new(64 * 1024 * 1024);
        let remote = roundtrip(local.clone(), 7, &budget);
        equal(local.hierarchy(), remote.hierarchy());
        let sizes = ScopeSizes::count(local.hierarchy());
        let received = remote.scope_sizes().unwrap();
        for id in 0..local.hierarchy().scope_count() {
            assert_eq!(sizes.get(id), received.get(id));
        }
        let mut app = volna_core::app::App::new();
        app.set_session(remote.clone());
        assert!(
            app.take_requests()
                .iter()
                .all(|r| !matches!(r, volna_core::session::LoadRequest::Sizes { .. }))
        );
        drop(app);
        drop(remote);
        assert!(budget.used() > 0, "sizes retain their admission owner");
        drop(received);
        assert_eq!(budget.used(), 0);
    }
}

fn wide(count: usize) -> Arc<dyn Session> {
    let mut h = volna_core::data::HierarchyBuilder::default();
    let root = h.push_scope("顶层.λ".into(), "module".into(), None);
    for i in 0..count {
        let scope = h.push_scope(format!("cell{i}"), "module".into(), Some(root));
        h.scopes[scope].component = "NAND2_X1".into();
        h.vars.push(volna_core::data::Variable {
            name: "\\pin.λ".into(),
            scope,
            shape: volna_core::data::SignalShape::Vector { width: 64 },
            var_type: "wire".into(),
            direction: volna_core::data::Direction::Input,
            signal: volna_core::data::SignalRef(0),
            enum_table: Some(42),
        });
    }
    volna_core::testing::hierarchy_session(h)
}
#[test]
fn pages_cross_65536_entries_and_shared_hierarchy_outlives_the_session() {
    for count in [0, 1, PAGE_ENTRIES - 1, PAGE_ENTRIES, PAGE_ENTRIES + 1] {
        let local = wide(count);
        let budget = MemoryBudget::new(32 * 1024 * 1024);
        let remote = roundtrip(local.clone(), DATA_BYTES, &budget);
        equal(local.hierarchy(), remote.hierarchy());
        let hierarchy = remote.hierarchy().clone();
        drop(remote);
        assert!(budget.used() > 0);
        assert_eq!(hierarchy.scope_count(), count + 1);
        drop(hierarchy);
        assert_eq!(budget.used(), 0);
    }
}
#[test]
fn insufficient_page_budget_fails_at_begin_before_any_page_input() {
    let local = wide(100);
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let mut transfer = transfer(&budget);
    let mut sequence = 0;
    let mut objects = objects(local.as_ref()).into_iter();
    let (id, header) = objects.next().unwrap();
    assert!(
        send(&mut transfer, &mut sequence, id, header, DATA_BYTES)
            .unwrap()
            .is_none()
    );
    let before = budget.used();
    let (id, page) = objects.next().unwrap();
    // Scratch and the wire payload fit, but resident column headers do not.
    // Admission must still fail at Begin; no page bytes are fed.
    budget.set_limit(before + (DATA_BYTES + 8192) as u64 + page.len() as u64);
    let error = accept(
        &mut transfer,
        &mut sequence,
        Body::Begin {
            object: id,
            decoded_bytes: page.len() as u64,
        },
    )
    .err()
    .unwrap();
    assert!(format!("{error:#}").contains("memory.budgetMiB"));
    assert_eq!(
        budget.used(),
        0,
        "poisoning releases the private catalog as well"
    );
    assert!(transfer.finish().is_err());
}
#[test]
fn malformed_ids_offsets_shapes_and_page_order_release_all_private_storage() {
    let local = wide(3);
    let base = objects(local.as_ref());
    for case in 0..19 {
        let mut objects = base.clone();
        if case == 8 {
            objects.swap(1, 2);
        } else {
            let index = if case < 4 || matches!(case, 9 | 10 | 12 | 13 | 14 | 16) {
                1
            } else {
                2
            };
            let mut page: Page = bincode::deserialize(&objects[index].1).unwrap();
            match case {
                0 => page.columns[3][0] = 0, // root cannot parent itself
                1 => page.columns[0][0] = u32::MAX,
                2 => page.offsets[1] = u32::MAX,
                3 => page.sizes[0].scopes = 0,
                4 => page.columns[1][0] = u32::MAX,
                5 => page.columns[6][0] = u32::MAX,
                6 => page.columns[2][0] = 99,
                7 => {
                    page.columns[2][1] = 1;
                    page.columns[3][1] = 1;
                } // incompatible alias
                9 => page.ids[0] = u32::MAX,
                10 | 11 => page.ids[1] = page.ids[0],
                12 => page.names[0] = 0xff,
                13 => {
                    page.columns[0].pop();
                }
                14 => page.offsets[0] = 1,
                15 | 16 => {
                    page.ids.swap(0, 1);
                    for column in &mut page.columns {
                        column.swap(0, 1);
                    }
                    if !page.sizes.is_empty() {
                        page.sizes.swap(0, 1);
                    }
                }
                17 => page.columns[5][0] = 8, // unknown direction/enum flag
                18 => {
                    page.columns[5][0] &= !4;
                    page.columns[7][0] = 1;
                } // enum identity without presence flag
                _ => unreachable!(),
            }
            objects[index].1 = bincode::serialize(&page).unwrap();
        }
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let mut transfer = transfer(&budget);
        let mut sequence = 0;
        let mut failed = false;
        for (id, bytes) in objects {
            match send(&mut transfer, &mut sequence, id, bytes, DATA_BYTES) {
                Err(_) => {
                    failed = true;
                    break;
                }
                Ok(None) => {}
                Ok(Some(_)) => panic!("malformed page published, case {case}"),
            }
        }
        assert!(failed, "case {case}");
        assert_eq!(budget.used(), 0);
        assert!(transfer.finish().is_err());
    }
}
#[test]
fn missing_last_end_and_wrong_envelopes_never_publish_a_session() {
    let local = wide(2);
    for wrong in [false, true] {
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let mut transfer = transfer(&budget);
        let mut sequence = 0;
        let mut all = objects(local.as_ref());
        let (id, bytes) = all.pop().unwrap();
        for (id, bytes) in all {
            assert!(
                send(&mut transfer, &mut sequence, id, bytes, DATA_BYTES)
                    .unwrap()
                    .is_none()
            );
        }
        accept(
            &mut transfer,
            &mut sequence,
            Body::Begin {
                object: id,
                decoded_bytes: bytes.len() as u64,
            },
        )
        .unwrap();
        accept(
            &mut transfer,
            &mut sequence,
            Body::Data { offset: 0, bytes },
        )
        .unwrap();
        if wrong {
            let packet = Packet {
                session: 32,
                request: 7,
                sequence,
                body: Body::End,
            };
            assert!(transfer.accept(packet).is_err());
            assert_eq!(budget.used(), 0);
        }
        assert!(transfer.finish().is_err());
        assert_eq!(budget.used(), 0);
    }
}

#[cfg(unix)]
#[test]
fn production_in_process_transport_matches_local_over_multiple_pages() {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    use volna_core::remote::{
        client::RemoteClient,
        transport::{Command, read_packet, write_packet},
    };
    let local = wide(PAGE_ENTRIES + 1);
    let expected = local.clone();
    let (mut client_socket, server_socket) = UnixStream::pair().unwrap();
    for socket in [&client_socket, &server_socket] {
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(20)))
            .unwrap();
    }
    let worker = std::thread::spawn(move || {
        volna_core::remote::server::serve(
            server_socket.try_clone().unwrap(),
            server_socket,
            31,
            || Ok(local),
            || Ok(()),
        )
    });
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let mut client = RemoteClient::new(TraceId::A, 11, 64 * 1024 * 1024, budget.clone()).unwrap();
    write_packet(&mut client_socket, &client.take_command().unwrap().unwrap()).unwrap();
    let remote = 'opened: loop {
        let mut step = client
            .accept(read_packet(&mut client_socket).unwrap().unwrap())
            .unwrap();
        loop {
            match step {
                ClientStep::Yield => step = client.step().unwrap(),
                ClientStep::Ack(ack) => {
                    write_packet(&mut client_socket, &ack).unwrap();
                    break;
                }
                ClientStep::Complete {
                    ack,
                    result: LoadResult::Opened { result, .. },
                } => {
                    write_packet(&mut client_socket, &ack).unwrap();
                    break 'opened result.unwrap();
                }
                _ => panic!("Open"),
            }
        }
    };
    equal(expected.hierarchy(), remote.hierarchy());
    write_packet(
        &mut client_socket,
        &Packet {
            session: 31,
            request: 8,
            sequence: 0,
            body: Body::Command(Command::Close),
        },
    )
    .unwrap();
    drop(client_socket);
    worker.join().unwrap().unwrap();
    drop(remote);
    assert_eq!(budget.used(), 0);
}

#[test]
fn preorder_pages_preserve_declaration_ids_and_membership() {
    let mut b = volna_core::data::HierarchyBuilder::default();
    let a = b.push_scope("a".into(), "module".into(), None);
    let sibling = b.push_scope("b".into(), "module".into(), None);
    let child = b.push_scope("c".into(), "module".into(), Some(a));
    for scope in [sibling, a, child, a] {
        b.vars.push(volna_core::data::Variable {
            name: "same".into(),
            scope,
            shape: volna_core::data::SignalShape::Bit,
            var_type: "wire".into(),
            direction: volna_core::data::Direction::None,
            signal: volna_core::data::SignalRef(0),
            enum_table: None,
        });
    }
    let local = volna_core::testing::hierarchy_session(b);
    let sizes = ScopeSizes::count(local.hierarchy());
    assert_eq!(Page::scopes(local.hierarchy(), &sizes, 0).ids, [0, 2, 1]);
    assert_eq!(Page::vars(local.hierarchy(), 0).ids, [1, 3, 2, 0]);
    let budget = MemoryBudget::new(1024 * 1024);
    let remote = roundtrip(local.clone(), 1, &budget);
    equal(local.hierarchy(), remote.hierarchy());
    let clone = local.hierarchy().clone();
    assert_eq!(
        clone.var(0).name.as_ptr(),
        local.hierarchy().var(0).name.as_ptr()
    );
    assert_eq!(
        clone.var(1).name.as_ptr(),
        clone.var(0).name.as_ptr(),
        "repeated names are interned"
    );
    drop(remote);
    assert_eq!(budget.used(), 0);
}

#[test]
fn empty_forest_and_deep_hierarchy_roundtrip_without_recursion() {
    for depth in [0, 20_000] {
        let mut b = volna_core::data::HierarchyBuilder::default();
        let mut parent = None;
        for _ in 0..depth {
            parent = Some(b.push_scope("nested".into(), "module".into(), parent));
        }
        let local = volna_core::testing::hierarchy_session(b);
        let budget = MemoryBudget::new(16 * 1024 * 1024);
        let remote = roundtrip(local.clone(), DATA_BYTES, &budget);
        equal(local.hierarchy(), remote.hierarchy());
        if depth > 0 {
            assert!(!remote.hierarchy().has_vars(0));
        }
        drop(remote);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn vtr_views_equal_reader_declarations_node_by_node() {
    use vtr::{NodeData, NodeKind};
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for name in ["counter", "picorv32", "landing"] {
        let path = base.join(format!("../volna/examples/{name}.vtr"));
        let reader = vtr::Reader::open(&path).unwrap();
        let local = OpenSpec::Path(path).open().unwrap();
        let h = reader.hierarchy();
        let scopes: Vec<_> = h
            .ids()
            .filter(|&id| matches!(h.kind(id), NodeKind::Scope | NodeKind::Stream))
            .collect();
        let vars: Vec<_> = h.ids().filter(|&id| h.kind(id) == NodeKind::Var).collect();
        assert_eq!(scopes.len(), local.hierarchy().scope_count());
        assert_eq!(vars.len(), local.hierarchy().var_count());
        for (id, &node) in scopes.iter().enumerate() {
            let scope = local.hierarchy().scope(id);
            assert_eq!(scope.name, reader.str(h.name(node)));
            assert_eq!(
                scope.parent,
                h.parent(node)
                    .map(|p| scopes.iter().position(|&id| id == p).unwrap())
            );
            let children: Vec<_> = h
                .children(node)
                .filter_map(|n| scopes.iter().position(|&id| id == n))
                .collect();
            let members: Vec<_> = h
                .children(node)
                .filter_map(|n| vars.iter().position(|&id| id == n))
                .collect();
            assert_eq!(scope.children.iter().collect::<Vec<_>>(), children);
            assert_eq!(scope.vars.iter().collect::<Vec<_>>(), members);
            match h.node_data(node) {
                NodeData::Scope {
                    scope_type,
                    component,
                } => {
                    assert_eq!(scope.kind, scope_type.name());
                    assert_eq!(scope.component, reader.str(component));
                }
                NodeData::Stream { kind } => assert_eq!(scope.kind, reader.str(kind)),
                _ => unreachable!(),
            }
        }
        for (id, &node) in vars.iter().enumerate() {
            let var = local.hierarchy().var(id);
            assert_eq!(var.name, reader.str(h.name(node)));
            let NodeData::Var {
                var_type,
                direction,
                signal,
                ..
            } = h.node_data(node)
            else {
                unreachable!()
            };
            assert_eq!(var.var_type, var_type.name());
            assert_eq!(var.direction, direction.into());
            assert_eq!(var.signal.0, signal.0);
            assert_eq!(local.hierarchy().signal(id), var.signal);
        }
        let clone = local.hierarchy().clone();
        assert_eq!(
            clone.scope(0).name.as_ptr(),
            local.hierarchy().scope(0).name.as_ptr()
        );
        drop(local);
        assert_eq!(clone.scope_count(), scopes.len());
    }
}

#[test]
fn invalid_catalog_owners_and_capabilities_release_the_private_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("raw.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let stream = writer.add_stream(None, "raw", "TX").unwrap();
    writer.add_generator(stream, "site").unwrap();
    writer.close().unwrap();
    let local = OpenSpec::Path(path).open().unwrap();
    for case in 0..3 {
        let mut all = objects(local.as_ref());
        let mut header: Header = bincode::deserialize(&all[0].1).unwrap();
        match case {
            0 => header.generators[0].stream = usize::MAX,
            1 => header.generators[0].track.0 = u32::MAX,
            2 => header.capabilities.transactions = false,
            _ => unreachable!(),
        }
        all[0].1 = bincode::serialize(&header).unwrap();
        let budget = MemoryBudget::new(1024 * 1024);
        let mut transfer = transfer(&budget);
        let mut sequence = 0;
        let mut rejected = false;
        for (id, bytes) in all {
            match send(&mut transfer, &mut sequence, id, bytes, DATA_BYTES) {
                Err(_) => {
                    rejected = true;
                    break;
                }
                Ok(None) => {}
                Ok(Some(_)) => panic!("invalid catalog published"),
            }
        }
        assert!(rejected);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn root_variables_keep_synthetic_scope_declaration_order_and_later_top_is_distinct() {
    use vtr::*;
    for mode in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roots.vtr");
        let mut writer = Writer::create(&path).unwrap();
        let before = (mode != 0).then(|| {
            writer
                .add_scope(
                    None,
                    if mode == 2 { "(top)" } else { "before" },
                    ScopeType::Module,
                    "REAL",
                )
                .unwrap()
        });
        if let Some(parent) = before {
            writer
                .add_scope(Some(parent), "nested", ScopeType::Module, "")
                .unwrap();
        }
        let (_, signal) = writer
            .add_var(
                None,
                "root_var",
                VarType::Wire,
                Direction::Implicit,
                SignalKind::Bits {
                    width: 1,
                    states: 2,
                },
            )
            .unwrap();
        if let Some(parent) = before {
            writer
                .add_alias(
                    Some(parent),
                    "alias",
                    VarType::Wire,
                    Direction::Input,
                    signal,
                )
                .unwrap();
        }
        let after = writer
            .add_scope(
                None,
                if mode == 0 { "(top)" } else { "after" },
                ScopeType::Module,
                "LATER",
            )
            .unwrap();
        writer
            .add_scope(Some(after), "child", ScopeType::Module, "")
            .unwrap();
        let stream = writer.add_stream(Some(after), "TX", "raw").unwrap();
        writer.add_generator(stream, "site").unwrap();
        writer.close().unwrap();
        let local = OpenSpec::Path(path).open().unwrap();
        let h = local.hierarchy();
        let expected_top = if mode == 1 { 2 } else { 0 };
        assert_eq!(h.var(0).scope, expected_top);
        assert_eq!(h.scope(expected_top).name, "(top)");
        assert_eq!(
            h.scope(expected_top).component,
            if mode == 2 { "REAL" } else { "" }
        );
        let expected_roots = match mode {
            0 => vec![0, 1],
            1 => vec![0, 2, 3],
            _ => vec![0, 2],
        };
        assert_eq!(h.roots().iter().collect::<Vec<_>>(), expected_roots);
        let later_id = if mode == 1 {
            3
        } else if mode == 2 {
            2
        } else {
            1
        };
        assert_eq!(h.scope(later_id).component, "LATER");
        assert_eq!(
            h.scope(later_id).children.iter().collect::<Vec<_>>(),
            [later_id + 1, later_id + 2]
        );
        assert_eq!(h.generators()[0].stream, later_id + 2);
        assert_eq!(h.scope(later_id + 1).parent, Some(later_id));
        if mode == 0 {
            assert_eq!(
                h.find_scope(&["(top)"]),
                volna_core::data::source::Lookup::Ambiguous
            );
        }
        if mode == 2 {
            assert_eq!(h.scope(0).vars.iter().collect::<Vec<_>>(), [0, 1]);
        }
        let budget = MemoryBudget::new(1024 * 1024);
        let remote = roundtrip(local.clone(), 7, &budget);
        equal(h, remote.hierarchy());
    }
}

#[test]
fn enum_presence_preserves_the_complete_raw_identity_range() {
    use volna_core::data::*;
    let mut builder = HierarchyBuilder::default();
    builder.push_scope("top".into(), "module".into(), None);
    for (enum_table, direction) in [
        (None, Direction::None),
        (Some(0), Direction::Input),
        (Some(42), Direction::Output),
        (Some(u32::MAX), Direction::InOut),
    ] {
        builder.vars.push(Variable {
            name: "alias".into(),
            scope: 0,
            shape: SignalShape::Bit,
            var_type: "wire".into(),
            direction,
            signal: SignalRef(0),
            enum_table,
        });
    }
    let local = volna_core::testing::hierarchy_session(builder);
    let budget = MemoryBudget::new(1024 * 1024);
    let remote = roundtrip(local.clone(), 7, &budget);
    equal(local.hierarchy(), remote.hierarchy());
    assert_eq!(remote.hierarchy().var(3).enum_table, Some(u32::MAX));
    drop(remote);
    assert_eq!(budget.used(), 0);
}
