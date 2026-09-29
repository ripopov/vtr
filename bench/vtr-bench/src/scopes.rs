//! Scope-size measurements for docs/hierarchy-scope-sizes.html.
//!
//! `scopes <trace.vtr>` counts, for every scope, the distinct signals below it
//! (subscopes included; an aliased signal counts once) three ways and checks
//! that they agree:
//!
//! * variable sums: each scope's own variables summed up the tree, which is
//!   not a distinct count once ports alias nets;
//! * the stamp walk: every variable walks up until it meets a scope already
//!   stamped with its signal, O(variables × depth);
//! * the colour-set-size method (Hui, CPM 1992), streamed by
//!   `vtr::Census`: each signal's scopes in preorder add +1, the lowest
//!   common ancestor of each consecutive pair −1, and subtree sums give the
//!   counts. The LCAs come from Tarjan's offline algorithm in the same
//!   preorder pass, so the build is linear.
//!
//! It also times the per-frame form the activity meters need: the per-signal
//! scope and LCA lists are fixed, so a frame adds the entries of its active
//! signals and takes one prefix pass.
//!
//! `gen-gates <in.vtr> <out.vtr> [--copies N]` expands a real RTL hierarchy
//! into a gate-level one: every variable becomes one-bit nets, aliases keep
//! aliasing the same bits, and every net gets a driving cell scope whose ports
//! alias nets of its module.

use crate::util::peak_rss;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::json;
use std::time::Instant;
use vtr::{Direction, NodeData, NodeId, NodeKind, Reader, ScopeType, SignalId, SignalKind, VarType, Writer, WriterOptions};

fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

const NONE: u32 = u32::MAX;

/// Scopes in preorder with their parents, and every variable as (scope, signal) in the same order.
struct Tree {
    parent: Vec<u32>,
    depth: Vec<u32>,
    /// Variables per scope (CSR over preorder scopes).
    var_start: Vec<u32>,
    var_sig: Vec<u32>,
    signals: usize,
}

fn tree(r: &Reader) -> Tree {
    let h = r.hierarchy();
    let mut parent = Vec::new();
    let mut depth = Vec::new();
    let mut var_start = vec![0u32];
    let mut var_sig = Vec::new();
    // Iterative preorder over scopes; each scope's variables are listed with it.
    let mut stack: Vec<(NodeId, u32, u32)> = h.roots().filter(|&n| h.kind(n) == NodeKind::Scope).map(|n| (n, NONE, 0)).collect();
    stack.reverse();
    while let Some((n, p, d)) = stack.pop() {
        let me = parent.len() as u32;
        parent.push(p);
        depth.push(d);
        let mut kids = Vec::new();
        for c in h.children(n) {
            match h.kind(c) {
                NodeKind::Var => var_sig.push(h.signal_of(c).unwrap().0),
                NodeKind::Scope => kids.push(c),
                _ => {}
            }
        }
        var_start.push(var_sig.len() as u32);
        for &c in kids.iter().rev() {
            stack.push((c, me, d + 1));
        }
    }
    Tree { parent, depth, var_start, var_sig, signals: h.signals.len() }
}

fn subtree_sums(parent: &[u32], w: &mut [i64]) {
    for i in (1..parent.len()).rev() {
        let p = parent[i];
        if p != NONE {
            w[p as usize] += w[i];
        }
    }
}

fn find(uf: &mut [u32], mut x: u32) -> u32 {
    let mut r = x;
    while uf[r as usize] != r {
        r = uf[r as usize];
    }
    while uf[x as usize] != r {
        let n = uf[x as usize];
        uf[x as usize] = r;
        x = n;
    }
    r
}

/// Per signal, its distinct scopes in preorder and the LCA of each consecutive pair (CSR by signal).
struct Census {
    start: Vec<u32>,
    scope: Vec<u32>,
    lca: Vec<u32>,
}

/// One preorder pass: Tarjan's offline LCA answers (previous scope of this signal, this scope) on the spot.
fn census(t: &Tree) -> Census {
    let s = t.parent.len();
    let mut uf: Vec<u32> = (0..s as u32).collect();
    let mut anc: Vec<u32> = (0..s as u32).collect();
    let mut last = vec![NONE; t.signals];
    let mut stack: Vec<u32> = Vec::new();
    // Pairs in visiting order; bucketed by signal afterwards.
    let mut pairs: Vec<(u32, u32, u32)> = Vec::with_capacity(t.var_sig.len());
    for i in 0..s as u32 {
        let p = t.parent[i as usize];
        // Close every open scope that is not an ancestor of i.
        while let Some(&top) = stack.last() {
            if top == p {
                break;
            }
            stack.pop();
            let tp = t.parent[top as usize];
            let b = find(&mut uf, top);
            if tp == NONE {
                anc[b as usize] = NONE;
            } else {
                let a = find(&mut uf, tp);
                uf[b as usize] = a;
                anc[a as usize] = tp;
            }
        }
        stack.push(i);
        for k in t.var_start[i as usize]..t.var_start[i as usize + 1] {
            let g = t.var_sig[k as usize];
            let prev = last[g as usize];
            if prev == i {
                continue;
            }
            // No LCA before the signal's first scope or across roots.
            let l = if prev == NONE { NONE } else { anc[find(&mut uf, prev) as usize] };
            pairs.push((g, i, l));
            last[g as usize] = i;
        }
    }
    let mut start = vec![0u32; t.signals + 1];
    for &(g, _, _) in &pairs {
        start[g as usize + 1] += 1;
    }
    for g in 0..t.signals {
        start[g + 1] += start[g];
    }
    let mut fill = start.clone();
    let mut scope = vec![0u32; pairs.len()];
    let mut lca = vec![0u32; pairs.len()];
    for &(g, i, l) in &pairs {
        let at = fill[g as usize] as usize;
        scope[at] = i;
        lca[at] = l;
        fill[g as usize] += 1;
    }
    Census { start, scope, lca }
}

fn evaluate(c: &Census, parent: &[u32], active: impl Iterator<Item = usize>, w: &mut [i64]) {
    w.iter_mut().for_each(|x| *x = 0);
    for g in active {
        for k in c.start[g] as usize..c.start[g + 1] as usize {
            w[c.scope[k] as usize] += 1;
            let l = c.lca[k];
            if l != NONE {
                w[l as usize] -= 1;
            }
        }
    }
    subtree_sums(parent, w);
}

pub fn run(path: &str, children: Option<&str>) {
    let rss0 = rss();
    let t = Instant::now();
    let r = Reader::open(path).unwrap();
    let open_s = t.elapsed().as_secs_f64();
    let reader_bytes = rss().saturating_sub(rss0);
    let h = r.hierarchy();
    let nodes = h.len();

    let t = Instant::now();
    let tr = tree(&r);
    let tree_s = t.elapsed().as_secs_f64();
    let s = tr.parent.len();
    let v = tr.var_sig.len();
    let max_depth = *tr.depth.iter().max().unwrap_or(&0);
    let mut fanout = vec![0u32; s];
    for &p in &tr.parent {
        if p != NONE {
            fanout[p as usize] += 1;
        }
    }
    let max_children = *fanout.iter().max().unwrap_or(&0);
    drop(fanout);
    let mean_var_depth = (0..s).map(|i| (tr.var_start[i + 1] - tr.var_start[i]) as f64 * tr.depth[i] as f64).sum::<f64>() / v.max(1) as f64;

    // Variable sums.
    let t = Instant::now();
    let mut sums: Vec<i64> = (0..s).map(|i| (tr.var_start[i + 1] - tr.var_start[i]) as i64).collect();
    subtree_sums(&tr.parent, &mut sums);
    let sums_s = t.elapsed().as_secs_f64();

    // Stamp walk: signal -> scopes (CSR), then walk up until a stamped scope.
    let t = Instant::now();
    let mut sstart = vec![0u32; tr.signals + 1];
    for &g in &tr.var_sig {
        sstart[g as usize + 1] += 1;
    }
    for g in 0..tr.signals {
        sstart[g + 1] += sstart[g];
    }
    let mut fill = sstart.clone();
    let mut sscope = vec![0u32; v];
    for i in 0..s {
        for k in tr.var_start[i]..tr.var_start[i + 1] {
            let g = tr.var_sig[k as usize] as usize;
            sscope[fill[g] as usize] = i as u32;
            fill[g] += 1;
        }
    }
    drop(fill);
    let mut stamp = vec![NONE; s];
    let mut walk = vec![0i64; s];
    let mut steps = 0u64;
    for g in 0..tr.signals {
        for k in sstart[g]..sstart[g + 1] {
            let mut q = sscope[k as usize];
            while q != NONE && stamp[q as usize] != g as u32 {
                stamp[q as usize] = g as u32;
                walk[q as usize] += 1;
                steps += 1;
                q = tr.parent[q as usize];
            }
        }
    }
    let walk_s = t.elapsed().as_secs_f64();

    // Colour set size with offline LCA, sizes only: the library's streaming count.
    let t = Instant::now();
    let (_, sizes) = h.scope_sizes();
    let census_sizes_s = t.elapsed().as_secs_f64();
    assert!((0..s).all(|i| sizes.signals(i as u32) as i64 == walk[i]), "streaming colour-set-size counts differ from the stamp walk");
    drop(sizes);

    // Colour set size with offline LCA, keeping the per-signal lists.
    let t = Instant::now();
    let c = census(&tr);
    let census_build_s = t.elapsed().as_secs_f64();
    let mut w = vec![0i64; s];
    let t = Instant::now();
    evaluate(&c, &tr.parent, 0..tr.signals, &mut w);
    let census_eval_s = t.elapsed().as_secs_f64();
    assert_eq!(w, walk, "colour-set-size counts differ from the stamp walk");
    let census_bytes = (c.start.len() + c.scope.len() + c.lca.len()) * 4 + s * 4;

    // A frame of the activity meters: a random 30% of the signals active.
    let mut rng = StdRng::seed_from_u64(1);
    let active: Vec<usize> = (0..tr.signals).filter(|_| rng.gen_bool(0.3)).collect();
    let mut frames = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        evaluate(&c, &tr.parent, active.iter().copied(), &mut w);
        frames.push(t.elapsed().as_secs_f64());
    }
    frames.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut stamp2 = vec![NONE; s];
    let mut walk2 = vec![0i64; s];
    let t = Instant::now();
    for &g in &active {
        for k in sstart[g]..sstart[g + 1] {
            let mut q = sscope[k as usize];
            while q != NONE && stamp2[q as usize] != g as u32 {
                stamp2[q as usize] = g as u32;
                walk2[q as usize] += 1;
                q = tr.parent[q as usize];
            }
        }
    }
    let walk_frame_s = t.elapsed().as_secs_f64();
    assert_eq!(w, walk2, "per-frame counts differ");

    // A named scope and its children: name, scopes below, variables below, distinct signals, cell type.
    let mut listed = Vec::new();
    if let Some(name) = children {
        let mut scope_node = Vec::with_capacity(s);
        let mut stack: Vec<NodeId> = h.roots().filter(|&n| h.kind(n) == NodeKind::Scope).collect();
        stack.reverse();
        while let Some(n) = stack.pop() {
            scope_node.push(n);
            let kids: Vec<NodeId> = h.children(n).filter(|&c| h.kind(c) == NodeKind::Scope).collect();
            stack.extend(kids.into_iter().rev());
        }
        let mut below = vec![1i64; s];
        subtree_sums(&tr.parent, &mut below);
        if let Some(at) = (0..s).find(|&i| r.str(h.name(scope_node[i])) == name) {
            for i in std::iter::once(at).chain((at + 1..s).filter(|&i| tr.parent[i] == at as u32)) {
                let component = match h.node(scope_node[i]).data {
                    NodeData::Scope { component, .. } => r.str(component).to_string(),
                    _ => String::new(),
                };
                listed.push(json!([r.str(h.name(scope_node[i])), below[i], sums[i], walk[i], component]));
            }
        }
    }
    let root_distinct = (0..s).filter(|&i| tr.parent[i] == NONE).map(|i| walk[i]).sum::<i64>();
    let names_bytes: usize = (0..nodes as u32).map(|n| r.str(h.name(NodeId(n))).len()).sum();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "trace": path,
            "nodes": nodes, "scopes": s, "variables": v, "signals": tr.signals, "distinct_under_roots": root_distinct,
            "max_depth": max_depth, "max_children": max_children, "mean_var_depth": mean_var_depth,
            "open_s": open_s, "reader_rss_bytes": reader_bytes, "name_bytes": names_bytes,
            "preorder_s": tree_s,
            "variable_sums_s": sums_s,
            "stamp_walk_s": walk_s, "stamp_walk_steps": steps,
            "census_sizes_s": census_sizes_s, "census_build_s": census_build_s, "census_eval_s": census_eval_s, "census_bytes": census_bytes,
            "census_entries": c.scope.len(),
            "frame_active_signals": active.len(), "census_frame_s": frames[2], "stamp_walk_frame_s": walk_frame_s,
            "peak_rss_bytes": peak_rss(), "children": listed,
        }))
        .unwrap()
    );
}

/// Expands an RTL hierarchy into a gate-level one (see the module docs).
pub fn gen_gates(src: &str, out: &str, copies: usize, seed: u64) {
    let r = Reader::open(src).unwrap();
    let h = r.hierarchy();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut w = Writer::create_with(out, WriterOptions::default()).unwrap();
    w.set_timescale(r.meta().timescale).unwrap();
    let wire = VarType::Wire;
    let one = SignalKind::Bits { width: 1, states: 2 };
    let (mut scopes, mut vars, mut cells) = (0u64, 0u64, 0u64);
    for copy in 0..copies {
        let top = if copies > 1 { Some(w.add_scope(None, &format!("core{copy}"), ScopeType::Module, "").unwrap()) } else { None };
        // Old signal -> first of its new one-bit signals.
        let mut bits: Vec<u32> = vec![NONE; h.signals.len()];
        let mut stack: Vec<(NodeId, Option<NodeId>)> = h.roots().filter(|&n| h.kind(n) == NodeKind::Scope).map(|n| (n, top)).collect();
        stack.reverse();
        while let Some((n, parent)) = stack.pop() {
            let me = w.add_scope(parent, r.str(h.name(n)), ScopeType::Module, "").unwrap();
            scopes += 1;
            let mut nets: Vec<SignalId> = Vec::new();
            let mut kids = Vec::new();
            for c in h.children(n) {
                match h.node(c).data {
                    NodeData::Var { signal, declares, direction, .. } => {
                        let width = h.signals[signal.0 as usize].width().max(1);
                        let name = r.str(h.name(c)).to_string();
                        let bit_name = |i: u32| if width == 1 { name.clone() } else { format!("{name}[{i}]") };
                        if declares.is_some() || bits[signal.0 as usize] == NONE {
                            let mut first = NONE;
                            for i in 0..width {
                                let (_, s) = w.add_var(Some(me), &bit_name(i), wire, direction, one).unwrap();
                                if i == 0 {
                                    first = s.0;
                                }
                                nets.push(s);
                            }
                            bits[signal.0 as usize] = first;
                        } else {
                            let first = bits[signal.0 as usize];
                            for i in 0..width {
                                w.add_alias(Some(me), &bit_name(i), wire, direction, SignalId(first + i)).unwrap();
                            }
                        }
                        vars += width as u64;
                    }
                    NodeData::Scope { .. } => kids.push(c),
                    _ => {}
                }
            }
            // One driving cell per net declared here: a two-input gate or, for one net in five, a flop.
            for (k, &y) in nets.iter().enumerate() {
                let flop = rng.gen_ratio(1, 5);
                let cell = w.add_scope(Some(me), &format!("U{k}"), ScopeType::Module, if flop { "DFF_X1" } else { "NAND2_X1" }).unwrap();
                let pick = |rng: &mut StdRng| nets[rng.gen_range(0..nets.len())];
                let (a, b) = (pick(&mut rng), pick(&mut rng));
                let ports: [(&str, SignalId, Direction); 3] = if flop {
                    [("D", a, Direction::Input), ("CK", b, Direction::Input), ("Q", y, Direction::Output)]
                } else {
                    [("A", a, Direction::Input), ("B", b, Direction::Input), ("ZN", y, Direction::Output)]
                };
                for (name, s, d) in ports {
                    w.add_alias(Some(cell), name, wire, d, s).unwrap();
                }
                cells += 1;
                vars += 3;
            }
            for &c in kids.iter().rev() {
                stack.push((c, Some(me)));
            }
        }
    }
    w.set_time(0).unwrap();
    w.close().unwrap();
    eprintln!("{out}: {} scopes ({cells} cells), {vars} variables", scopes + cells + (copies > 1) as u64 * copies as u64);
}
