use std::collections::HashSet;
use std::path::PathBuf;
use volna_trace::OpenSpec;
use volna_trace::data::{Hierarchy, ScopeId, ScopeSize, ScopeSizes};
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn brute(h: &Hierarchy, scope: ScopeId) -> ScopeSize {
    let (mut signals, mut variables, mut scopes) = (HashSet::new(), 0, 0);
    let mut stack = vec![scope];
    while let Some(s) = stack.pop() {
        scopes += 1;
        variables += h.scope(s).vars.len() as u32;
        signals.extend(h.scope(s).vars.iter().map(|v| h.var(v).signal));
        stack.extend(h.scope(s).children);
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
        "volna/volna-trace/tests/fixtures/values.fst",
    ] {
        let session = OpenSpec::Path(root().join(rel)).open().unwrap();
        let h = session.hierarchy();
        let sizes = ScopeSizes::count(h);
        for s in 0..h.scope_count() {
            let want = brute(h, s);
            assert_eq!(sizes.get(s), Some(want), "{rel}: {}", h.scope(s).name);
            aliased += usize::from(want.variables > want.signals);
        }
    }
    assert!(aliased > 0, "some fixture scope names a signal twice");
}
