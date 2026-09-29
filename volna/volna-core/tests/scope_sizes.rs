//! Scope sizes (docs/hierarchy-scope-sizes.html): the count over the
//! session hierarchy equals a brute-force set count on every fixture, VTR
//! and FST, and the scope tree reports it per row once the count after open
//! is delivered.

use std::collections::HashSet;
use std::path::PathBuf;
use volna_core::app::App;
use volna_core::data::{Hierarchy, ScopeId, ScopeSize, ScopeSizes};
use volna_core::session::{LoadRequest, OpenSpec};
use volna_core::sidebar::{ScopeTreeModel, TreeNode};
use volna_core::trace::{TraceId, Traced};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn brute(h: &Hierarchy, scope: ScopeId) -> ScopeSize {
    let (mut signals, mut variables, mut scopes) = (HashSet::new(), 0, 0);
    let mut stack = vec![scope];
    while let Some(s) = stack.pop() {
        scopes += 1;
        variables += h.scopes[s].vars.len() as u32;
        signals.extend(h.scopes[s].vars.iter().map(|&v| h.vars[v].signal));
        stack.extend(&h.scopes[s].children);
    }
    ScopeSize {
        signals: signals.len() as u32,
        variables,
        scopes,
    }
}

#[test]
fn counts_equal_brute_force_on_the_fixtures() {
    let mut aliased = 0;
    for rel in [
        "volna/volna/examples/counter.vtr",
        "volna/volna/examples/picorv32.vtr",
        "volna/volna/examples/feature_showcase.vtr",
        "volna/volna/examples/pipeline_showcase.vtr",
        "volna/volna/examples/landing.vtr",
        "volna/volna/examples/landing_dram.fst",
        "volna/volna-core/tests/fixtures/values.fst",
    ] {
        let session = OpenSpec::Path(root().join(rel)).open().unwrap();
        let h = session.hierarchy();
        let sizes = ScopeSizes::count(h);
        for s in 0..h.scopes.len() {
            let want = brute(h, s);
            assert_eq!(sizes.get(s), Some(want), "{rel}: {}", h.scopes[s].name);
            aliased += usize::from(want.variables > want.signals);
        }
    }
    assert!(aliased > 0, "some fixture scope names a signal twice");
}

#[test]
fn the_scope_tree_reports_sizes_once_counted() {
    let session = OpenSpec::Path(root().join("volna/volna/examples/picorv32.vtr"))
        .open()
        .unwrap();
    let mut app = App::new();
    app.set_session(session.clone());
    let h = session.hierarchy();
    let top = TreeNode::scope(Traced::new(TraceId::A, h.roots[0]));
    // Until the count finishes the column is empty rather than wrong.
    assert_eq!(ScopeTreeModel::size(app.doc.traces(), top), None);
    let requests = app.take_requests();
    let sizes = requests
        .iter()
        .filter(|r| matches!(r, LoadRequest::Sizes { .. }))
        .count();
    assert_eq!(sizes, 1, "one count per opened trace");
    for request in requests {
        app.deliver(request.perform());
    }
    assert_eq!(
        ScopeTreeModel::size(app.doc.traces(), top),
        Some(brute(h, h.roots[0]))
    );
    assert_eq!(
        ScopeTreeModel::size(app.doc.traces(), TreeNode::trace(TraceId::A)),
        None,
        "trace rows have no size"
    );
}

#[test]
fn a_count_for_an_older_generation_is_dropped() {
    let path = root().join("volna/volna/examples/counter.vtr");
    let mut app = App::new();
    app.set_session(OpenSpec::Path(path.clone()).open().unwrap());
    let stale = app.take_requests();
    // A new session replaces the trace before the count arrives.
    app.set_session(OpenSpec::Path(path).open().unwrap());
    for request in stale {
        app.deliver(request.perform());
    }
    let top = TreeNode::scope(Traced::new(TraceId::A, 0));
    assert_eq!(ScopeTreeModel::size(app.doc.traces(), top), None);
    for request in app.take_requests() {
        app.deliver(request.perform());
    }
    assert!(ScopeTreeModel::size(app.doc.traces(), top).is_some());
}

#[test]
fn a_row_labels_its_signals_and_details_the_rest() {
    let size = ScopeSize {
        signals: 6779,
        variables: 16261,
        scopes: 490,
    };
    assert_eq!(size.label(), "6,779");
    assert_eq!(
        size.detail(),
        "6,779 signals · 16,261 variables · 490 scopes"
    );
    let one = ScopeSize {
        signals: 1,
        variables: 1,
        scopes: 1,
    };
    assert_eq!(one.detail(), "1 signal · 1 variable · 1 scope");
    let zero = ScopeSize::default();
    assert_eq!(zero.label(), "0");
    assert_eq!(
        ScopeSize {
            signals: 1_234_567,
            ..zero
        }
        .label(),
        "1,234,567"
    );
}
