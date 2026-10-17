use vtr::*;

#[test]
fn typed_generators_own_records_at_root_and_in_nested_scopes() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = Writer::create(file.path()).unwrap();
    let root = w.add_generator(None, "requests", "MEMORY_BUS").unwrap();
    let module = w.add_scope(None, "cpu", ScopeType::Module, "core").unwrap();
    let direct = w
        .add_generator(Some(module), "instructions", "PIPELINE")
        .unwrap();
    let nested = w
        .add_scope(Some(module), "execution", ScopeType::Generic, "")
        .unwrap();
    let child = w.add_generator(Some(nested), "events", "CUSTOM").unwrap();
    w.node_attr(direct, "width", Value::U64(3)).unwrap();
    assert!(w.add_generator(Some(direct), "invalid", "TX").is_err());
    let mut expected = Vec::new();
    for generator in [root, direct, child, direct] {
        let id = w.begin_tx(generator, 10).unwrap();
        w.end_tx(id, 10, TxStatus::Ok).unwrap();
        expected.push(id);
    }
    w.close().unwrap();
    let r = Reader::open(file.path()).unwrap();
    for (node, kind) in [
        (root, "MEMORY_BUS"),
        (direct, "PIPELINE"),
        (child, "CUSTOM"),
    ] {
        let NodeData::Generator { kind: stored } = r.hierarchy().node_data(node) else {
            panic!("generator")
        };
        assert_eq!(r.str(stored), kind);
    }
    assert_eq!(r.hierarchy().parent(root), None);
    assert_eq!(r.hierarchy().parent(direct), Some(module));
    assert_eq!(r.hierarchy().attrs(direct)[0].1, Value::U64(3));
    let all = r.transactions(&TxQuery::default()).unwrap();
    assert_eq!(all.len(), 4);
    let selected = r
        .transactions(&TxQuery {
            scope: Some(module),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        selected.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected[1..]
    );
    let selected = r
        .transactions(&TxQuery {
            generator: Some(direct),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        selected.iter().map(|t| t.id).collect::<Vec<_>>(),
        [expected[1], expected[3]]
    );
}

#[test]
fn root_log_sites_and_clocks_are_typed_generators() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut w = Writer::create(file.path()).unwrap();
    let site = w
        .add_log_site(&LogSiteSpec::new(None, Severity::Info, "ready", &[]))
        .unwrap();
    let id = w.log(site, 5, &[]).unwrap();
    let clock = w.add_clock(None, "clk").unwrap();
    let clock_node = w.clock_generator(clock).unwrap();
    let work = w.add_generator(None, "work", "PIPELINE").unwrap();
    let link = w.intern("clk");
    w.node_attr(work, "vtr.clock", Value::Str(link)).unwrap();
    w.close().unwrap();
    let r = Reader::open(file.path()).unwrap();
    let site = &r.log_sites()[0];
    assert_eq!(site.scope, None);
    for (node, kind) in [(site.node, "LOG"), (clock_node, "CLOCK")] {
        let NodeData::Generator { kind: stored } = r.hierarchy().node_data(node) else {
            panic!("generator")
        };
        assert_eq!(r.str(stored), kind);
        assert_eq!(r.hierarchy().parent(node), None);
        assert_eq!(r.hierarchy().children(node).count(), 0);
    }
    assert_eq!(r.transaction(id).unwrap().unwrap().generator, site.node);
    assert_eq!(r.generator_clock(work), Some(clock));
    assert_eq!(r.generator_clock(NodeId(u32::MAX)), None);
}
