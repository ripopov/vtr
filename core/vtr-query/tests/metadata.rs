#![cfg(all(feature = "native-engine", not(target_family = "wasm")))]
use vtr_query::{
    metadata::{DeclarationData, DeclarationPage, Text},
    native_metadata::{text_part, Children},
    wave::Limits,
    Budget, Cancellation, Error,
};

#[test]
fn child_pages_preserve_declarations_aliases_and_bounded_large_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hierarchy.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    let root = writer.begin_scope("literal.root", vtr::ScopeType::Module, "component");
    let (_, signal) = writer.add_bits("same", 32, 4);
    writer
        .add_alias("same", vtr::VarType::Reg, vtr::Direction::Input, signal)
        .unwrap();
    let long = "long界".repeat(2000);
    writer.add_bits(&long, 1, 2);
    writer.end_scope().unwrap();
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let budget = Budget::new(8192);
    let cancellation = Cancellation::default();
    let limits = Limits {
        records: 1,
        bytes: 2048,
        work: 1,
    };
    let mut roots =
        Children::new(&reader, None, limits, budget.clone(), cancellation.clone()).unwrap();
    let root_page = roots.next_page().unwrap();
    assert!(root_page.complete);
    assert_eq!(root_page.declarations().len(), 1);
    assert_eq!(
        root_page.declarations()[0].name.as_str().unwrap(),
        Some("literal.root")
    );
    assert_eq!(root_page.declarations()[0].children, 3);
    let mut children = Children::new(
        &reader,
        Some(root.0),
        limits,
        budget.clone(),
        cancellation.clone(),
    )
    .unwrap();
    for offset in 0..3 {
        let page = children.next_page().unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.declarations().len(), 1);
        assert_eq!(page.complete, offset == 2);
        let node = &page.declarations()[0];
        assert_eq!(node.parent, Some(root.0));
        if offset < 2 {
            assert_eq!(node.name.as_str().unwrap(), Some("same"));
            match node.data {
                DeclarationData::Variable {
                    signal: s, alias, ..
                } => {
                    assert_eq!(s, signal.0);
                    assert_eq!(alias, offset == 1);
                }
                _ => panic!("expected variable"),
            }
        } else {
            let Text::Reference { id, bytes } = node.name else {
                panic!("expected large string reference")
            };
            assert_eq!(bytes as usize, long.len());
            let mut assembled = Vec::new();
            while assembled.len() < long.len() {
                // Deliberately splits multibyte UTF-8 characters.
                let part = text_part(
                    &reader,
                    id,
                    assembled.len() as u64,
                    7,
                    &budget,
                    &cancellation,
                )
                .unwrap();
                assert!(part.bytes.as_slice().len() <= 7);
                assert_eq!(part.total_bytes, bytes);
                assembled.extend_from_slice(part.bytes.as_slice());
            }
            assert_eq!(assembled, long.as_bytes());
            assert!(text_part(&reader, id, bytes + 1, 7, &budget, &cancellation).is_err());
        }
    }
    assert!(children.next_page().unwrap().complete);
    assert!(Children::new(
        &reader,
        Some(u32::MAX),
        limits,
        budget.clone(),
        cancellation.clone()
    )
    .is_err());
    cancellation.cancel();
    assert!(matches!(children.next_page(), Err(Error::Cancelled)));
    drop(root_page);
    drop(roots);
    drop(children);
    assert_eq!(budget.used(), 0);
}

#[test]
fn metadata_cursor_does_not_advance_when_page_cannot_be_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata-budget.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    writer.add_bits("first", 1, 2);
    writer.add_bits("second", 1, 2);
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let row_bytes = std::mem::size_of::<DeclarationPage>()
        + std::mem::size_of::<vtr_query::metadata::Declaration>();
    let budget = Budget::new(row_bytes);
    let mut query = Children::new(
        &reader,
        None,
        Limits {
            records: 1,
            bytes: 4096,
            work: 1,
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    let first = query.next_page().unwrap();
    assert_eq!(first.offset, 0);
    assert!(matches!(
        first.declarations()[0].name,
        Text::Reference { .. }
    ));
    assert!(matches!(query.next_page(), Err(Error::ResourceLimit)));
    drop(first);
    let second = query.next_page().unwrap();
    assert_eq!(second.offset, 1);
    assert!(second.complete);
    drop(second);
    drop(query);
    assert_eq!(budget.used(), 0);
}

#[test]
fn search_resumes_unicode_matching_and_ancestry_at_single_work_units() {
    use vtr_query::native_metadata::Search;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("search.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    writer.add_bits("ABABAC-outside", 1, 2);
    let root = writer.begin_scope("scope", vtr::ScopeType::Module, "");
    let (ascii, _) = writer.add_bits("ababababAC", 1, 2);
    let (unicode, _) = writer.add_bits("prefix-İ-界", 1, 2);
    for _ in 0..30 {
        writer.begin_scope("nested", vtr::ScopeType::Module, "");
    }
    let (deep, _) = writer.add_bits("deep-ABABAC", 1, 2);
    for _ in 0..31 {
        writer.end_scope().unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    for (needle, expected) in [
        ("AbAbAc", vec![ascii.0, deep.0]),
        ("i\u{307}", vec![unicode.0]),
        ("界", vec![unicode.0]),
        ("absent", vec![]),
    ] {
        let budget = Budget::new(4096);
        let token = Cancellation::default();
        let mut search = Search::new(
            &reader,
            Some(root.0),
            needle,
            Limits {
                records: 1,
                bytes: 1024,
                work: 1,
            },
            budget.clone(),
            token.clone(),
        )
        .unwrap();
        let mut found = Vec::new();
        let mut done = false;
        let mut empty = 0;
        for _ in 0..20000 {
            let page = search.next_page().unwrap();
            assert!(page.declarations().len() <= 1);
            if page.declarations().is_empty() {
                empty += 1;
            }
            found.extend(page.declarations().iter().map(|node| node.id));
            if page.complete {
                done = true;
                break;
            }
        }
        assert!(done && empty > 0);
        assert_eq!(found, expected);
        token.cancel();
        assert!(matches!(search.next_page(), Err(Error::Cancelled)));
        drop(search);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn search_has_no_hidden_five_thousand_result_cutoff() {
    use vtr_query::native_metadata::Search;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("many.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    for i in 0..6001 {
        writer.add_bits(&format!("signal-{i}"), 1, 2);
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    for needle in ["SIGNAL", ""] {
        let budget = Budget::new(32768);
        let mut query = Search::new(
            &reader,
            None,
            needle,
            Limits {
                records: 17,
                bytes: 8192,
                work: 103,
            },
            budget.clone(),
            Cancellation::default(),
        )
        .unwrap();
        let mut next = 0;
        loop {
            let page = query.next_page().unwrap();
            for node in page.declarations() {
                assert_eq!(node.id, next);
                next += 1;
            }
            if page.complete {
                break;
            }
        }
        assert_eq!(next, 6001);
        drop(query);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn path_batches_preserve_literal_names_occurrences_and_scope_ambiguity() {
    use vtr_query::{
        metadata::{Path, Resolution},
        native_metadata::ResolvePaths,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paths.vtr");
    let mut writer = vtr::Writer::create(&path).unwrap();
    writer.begin_scope("literal.root", vtr::ScopeType::Module, "");
    let (first, signal) = writer.add_bits("\\signal.with.dots ", 1, 2);
    let second = writer
        .add_alias(
            "\\signal.with.dots ",
            vtr::VarType::Reg,
            vtr::Direction::Input,
            signal,
        )
        .unwrap();
    for _ in 0..2 {
        writer.begin_scope("duplicate", vtr::ScopeType::Module, "");
        writer.add_bits("s", 1, 2);
        writer.end_scope().unwrap();
    }
    for _ in 0..50 {
        writer.begin_scope("deep", vtr::ScopeType::Module, "");
    }
    let (deep, _) = writer.add_bits("leaf", 1, 2);
    for _ in 0..51 {
        writer.end_scope().unwrap();
    }
    writer.close().unwrap();
    let reader = vtr::Reader::open(path).unwrap();
    let make = |segments: Vec<String>, occurrence| Path {
        segments,
        occurrence,
        kind: Some(2),
    };
    let literal = vec!["literal.root".into(), "\\signal.with.dots ".into()];
    let mut deep_path = vec!["literal.root".into()];
    deep_path.extend((0..50).map(|_| "deep".into()));
    deep_path.push("leaf".into());
    let requests = vec![
        make(literal.clone(), None),
        make(literal.clone(), Some(0)),
        make(literal.clone(), Some(1)),
        make(literal, Some(2)),
        make(
            vec!["literal.root".into(), "duplicate".into(), "s".into()],
            Some(0),
        ),
        make(deep_path, None),
        make(vec![], None),
    ];
    let budget = Budget::new(32768);
    let mut resolver = ResolvePaths::new(
        &reader,
        requests,
        Limits {
            records: 2,
            bytes: 1024,
            work: 1,
        },
        budget.clone(),
        Cancellation::default(),
    )
    .unwrap();
    let mut actual = Vec::new();
    let mut done = false;
    for _ in 0..1000 {
        let page = resolver.next_page().unwrap();
        assert_eq!(page.offset, actual.len());
        actual.extend_from_slice(page.results());
        if page.complete {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(
        actual,
        [
            Resolution::Ambiguous,
            Resolution::Found(first.0),
            Resolution::Found(second.0),
            Resolution::Missing,
            Resolution::Ambiguous,
            Resolution::Found(deep.0),
            Resolution::Missing
        ]
    );
    drop(resolver);
    assert_eq!(budget.used(), 0);
}
