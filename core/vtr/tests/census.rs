//! Scope sizes (docs/hierarchy-scope-sizes.html): the streaming count equals
//! a brute-force set count on random trees with aliases, fed both in
//! declaration order and with variables after subscopes, and through a
//! written trace.

use rand::{rngs::StdRng, Rng, SeedableRng};
use std::collections::HashSet;
use vtr::*;

/// A random scope tree: parent per scope (preorder) and per-scope signals.
struct Tree {
    parent: Vec<Option<usize>>,
    vars: Vec<Vec<u32>>,
}

fn random_tree(rng: &mut StdRng) -> Tree {
    let n = rng.gen_range(1..60);
    let signals = rng.gen_range(1..40);
    let mut parent = vec![None];
    let mut depth = vec![0];
    for i in 1..n {
        // Mostly nested, sometimes another root.
        let p = if rng.gen_ratio(1, 12) { None } else { Some(rng.gen_range(0..i)) };
        depth.push(p.map_or(0, |p: usize| depth[p] + 1));
        parent.push(p);
    }
    // Preorder requires children after their parent and subtrees contiguous: renumber by DFS.
    let mut kids = vec![Vec::new(); n];
    let mut roots = Vec::new();
    for (i, p) in parent.iter().enumerate() {
        match p {
            Some(p) => kids[*p].push(i),
            None => roots.push(i),
        }
    }
    let mut order = Vec::new();
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(kids[i].iter().rev());
    }
    let mut new = vec![0; n];
    for (k, &i) in order.iter().enumerate() {
        new[i] = k;
    }
    let parent = order.iter().map(|&i| parent[i].map(|p| new[p])).collect();
    // Ports alias: a signal often reappears in nearby scopes.
    let vars = (0..n)
        .map(|_| (0..rng.gen_range(0..6)).map(|_| rng.gen_range(0..signals)).collect())
        .collect();
    Tree { parent, vars }
}

fn brute(t: &Tree) -> Vec<(u32, u32, u32)> {
    let n = t.parent.len();
    (0..n)
        .map(|s| {
            let below: Vec<usize> = (0..n)
                .filter(|&i| {
                    let mut i = i;
                    loop {
                        if i == s {
                            break true;
                        }
                        match t.parent[i] {
                            Some(p) => i = p,
                            None => break false,
                        }
                    }
                })
                .collect();
            let signals: HashSet<u32> = below.iter().flat_map(|&i| t.vars[i].iter().copied()).collect();
            let vars: usize = below.iter().map(|&i| t.vars[i].len()).sum();
            (signals.len() as u32, vars as u32, below.len() as u32)
        })
        .collect()
}

fn triples(s: &ScopeSizes) -> Vec<(u32, u32, u32)> {
    (0..s.len() as u32).map(|i| (s.signals(i), s.variables(i), s.scopes(i))).collect()
}

/// Feeds the tree in preorder; with `late`, half of each scope's variables come after its subscopes.
fn stream(t: &Tree, late: bool) -> ScopeSizes {
    let n = t.parent.len();
    let mut c = Census::new();
    let mut open: Vec<usize> = Vec::new();
    let split = |i: usize| if late { t.vars[i].len() / 2 } else { t.vars[i].len() };
    for i in 0..n {
        while open.last().is_some_and(|&o| Some(o) != t.parent[i]) {
            let o = open.pop().unwrap();
            t.vars[o][split(o)..].iter().for_each(|&g| c.var(g));
            c.leave();
        }
        assert_eq!(c.enter(), i as u32);
        t.vars[i][..split(i)].iter().for_each(|&g| c.var(g));
        open.push(i);
    }
    while let Some(o) = open.pop() {
        t.vars[o][split(o)..].iter().for_each(|&g| c.var(g));
        c.leave();
    }
    let s = c.finish();
    for (i, p) in t.parent.iter().enumerate() {
        assert_eq!(s.parent(i as u32), p.map(|p| p as u32));
    }
    s
}

#[test]
fn streaming_count_equals_brute_force() {
    let mut rng = StdRng::seed_from_u64(7);
    for _ in 0..500 {
        let t = random_tree(&mut rng);
        let want = brute(&t);
        assert_eq!(triples(&stream(&t, false)), want);
        assert_eq!(triples(&stream(&t, true)), want);
    }
}

#[test]
fn finish_closes_open_scopes() {
    let mut c = Census::new();
    let top = c.enter();
    c.var(3);
    let a = c.enter();
    c.var(3);
    c.var(9);
    let s = c.finish();
    assert_eq!((s.signals(top), s.variables(top), s.scopes(top)), (2, 3, 2));
    assert_eq!((s.signals(a), s.variables(a), s.scopes(a)), (2, 2, 1));
    assert!(Census::new().finish().is_empty());
}

#[test]
fn trace_hierarchy_counts_aliases_once() {
    let mut rng = StdRng::seed_from_u64(11);
    let dir = tempfile::tempdir().unwrap();
    for round in 0..20 {
        let t = random_tree(&mut rng);
        let path = dir.path().join(format!("t{round}.vtr"));
        let mut w = Writer::create(&path).unwrap();
        let one = SignalKind::Bits { width: 1, states: 2 };
        let mut ids: Vec<NodeId> = Vec::new();
        // The tree's signal g becomes the file's first variable of g; later ones alias it.
        let mut declared: Vec<Option<SignalId>> = vec![None; 64];
        for i in 0..t.parent.len() {
            let parent = t.parent[i].map(|p| ids[p]);
            let me = w.add_scope(parent, &format!("s{i}"), ScopeType::Module, "").unwrap();
            // Streams sit beside scopes and are not counted.
            if i % 5 == 0 {
                w.add_stream(Some(me), "tx", "TRANSACTOR").unwrap();
            }
            for (k, &g) in t.vars[i].iter().enumerate() {
                let name = format!("v{k}");
                match declared[g as usize] {
                    Some(s) => {
                        w.add_alias(Some(me), &name, VarType::Wire, Direction::Implicit, s).unwrap();
                    }
                    None => declared[g as usize] = Some(w.add_var(Some(me), &name, VarType::Wire, Direction::Implicit, one).unwrap().1),
                };
            }
            ids.push(me);
        }
        w.set_time(0).unwrap();
        w.close().unwrap();

        let r = Reader::open(&path).unwrap();
        let (nodes, sizes) = r.hierarchy().scope_sizes();
        assert_eq!(nodes.len(), t.parent.len());
        assert!(nodes.iter().enumerate().all(|(i, &n)| r.name(n) == format!("s{i}")));
        assert_eq!(triples(&sizes), brute(&t));
        // Counting a subset: the tree's even signals.
        let even: HashSet<u32> = declared.iter().enumerate().filter(|(g, _)| g % 2 == 0).filter_map(|(_, s)| s.map(|s| s.0)).collect();
        let (_, some) = r.hierarchy().scope_sizes_of(|s| even.contains(&s.0));
        let kept = Tree { parent: t.parent.clone(), vars: t.vars.iter().map(|v| v.iter().copied().filter(|g| g % 2 == 0).collect()).collect() };
        assert_eq!(triples(&some), brute(&kept));
    }
}
