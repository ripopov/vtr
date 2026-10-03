#![cfg(unix)]
#[path = "../src/server.rs"]
mod server;
use std::sync::Arc;
use volna_trace::data::Hierarchy;
use volna_trace::remote::{
    ClientStep,
    hierarchy::PAGE_ENTRIES,
    memory::MemoryBudget,
    transport::{Body, Packet},
};
use volna_trace::session::{LoadResult, Session};
fn wide(count: usize) -> Arc<dyn Session> {
    let mut h = volna_trace::data::HierarchyBuilder::default();
    let root = h.push_scope("顶层.λ".into(), "module".into(), None);
    for i in 0..count {
        let scope = h.push_scope(format!("cell{i}"), "module".into(), Some(root));
        h.scopes[scope].component = "NAND2_X1".into();
        h.vars.push(volna_trace::data::Variable {
            name: "\\pin.λ".into(),
            scope,
            shape: volna_trace::data::SignalShape::Vector { width: 64 },
            var_type: "wire".into(),
            direction: volna_trace::data::Direction::Input,
            signal: volna_trace::data::SignalRef(0),
            enum_table: Some(42),
        });
    }
    volna_trace::testing::hierarchy_session(h)
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

#[cfg(unix)]
#[test]
fn production_in_process_transport_matches_local_over_multiple_pages() {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    use volna_trace::remote::client::RemoteClient;
    use volna_trace::remote::transport::{Command, read_packet, write_packet};
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
        server::serve(
            server_socket.try_clone().unwrap(),
            server_socket,
            31,
            || Ok(local),
            || Ok(()),
        )
    });
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let mut client = RemoteClient::new(0, 11, 64 * 1024 * 1024, budget.clone()).unwrap();
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
