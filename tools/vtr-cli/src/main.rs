//! `vtr` command-line tool: inspect, query and convert VTR traces.

use vtr_cli::{fst, ftr, kanata, otlp, vcd};
use std::process::exit;
use vtr::{Codec, Compression, NodeData, NodeId, Reader, TxQuery, Value, Writer, WriterOptions};

const USAGE: &str = "\
vtr - Volna Trace Record tools

USAGE:
  vtr info <file.vtr>                         file summary (meta, counts, sections)
  vtr hier <file.vtr> [--depth N] [--vars]    print the hierarchy
  vtr hier <file.vtr> --sizes [--depth N]     scopes with the scopes, variables and distinct signals below them
  vtr value <file.vtr> <path> <time>          value of a signal at a time
  vtr changes <file.vtr> <path> [--from T] [--to T] [--max N]
  vtr dump <file.vtr> [--from T] [--to T]     all value changes in time order (VCD-like)
  vtr tx <file.vtr> [--stream NAME] [--from T] [--to T] [--max N] [--id ID[,ID...]]
  vtr log <file.vtr> [--stream NAME] [--severity LEVEL] [--from T] [--to T] [--max N] [--sites]
  vtr clocks <file.vtr>                       declared clocks: stretches, periods and stopped time
  vtr convert <input> <output.vtr> [--states 2|4|9] [--codec zstd|lz4|none] [--level L]
              [--group-size N] [--block-records N] [--no-background] [--no-dedup]
              input formats by extension: .fst .vcd .log/.kanata[.gz] .json (OTLP) .ftr
  vtr to-vcd <file.vtr> <out.vcd>             write the signals as VCD (also: vtr2vcd)
  vtr fst-to-vcd <file.fst> <out.vcd>         same for an FST file (through fst-reader)
  vtr vcd-compare <a.vcd> <b.vcd>             compare value-change counts, total and per signal (JSON)
  vtr recover <in.vtr> <out.vtr>              rewrite a file whose writer never closed it as a complete
                                              one (exit status 3 when bytes were dropped)
  vtr index <trace> [--threads N] [--memory MiB]
                                              build the activity index <trace>.index of a .vtr or .fst
                                              (in the user cache when the directory is read-only)
  vtr index <trace> --check                   whether a valid activity index exists (exit status 1 if not)
  vtr active <trace> <t0> <t1> [--scope PATH] [--depth N] [--signals] [--build]
                                              scopes with the signals that change in [t0, t1] over their
                                              signals (to depth N below PATH, default 1), or the changing
                                              signals; exact, from the activity index and the trace

Times are integers in the file's time unit; `vtr active` also takes a unit suffix (25000ns, 2.5us).
Paths use '.' as separator.";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(2)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = !matches!(a.as_str(), "--vars" | "--sizes" | "--no-background" | "--no-dedup" | "--progress" | "--no-checksums" | "--sites" | "--check" | "--signals" | "--build");
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn open(path: &str) -> Reader {
    Reader::open(path).unwrap_or_else(|e| die(format!("{path}: {e}")))
}

fn fmt_value(r: &Reader, v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::I64(i) => i.to_string(),
        Value::U64(u) => u.to_string(),
        Value::F64(f) => f.to_string(),
        Value::Str(s) => format!("{:?}", r.str(*s)),
        Value::Bytes(b) => format!("0x{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        Value::Bits { width, data } => vtr::SignalValue::Bits { width: *width, states: 2, data }.to_ascii(),
        Value::Logic { width, data } => vtr::SignalValue::Bits { width: *width, states: 4, data }.to_ascii(),
        Value::Logic9 { width, data } => vtr::SignalValue::Bits { width: *width, states: 9, data }.to_ascii(),
        Value::Time(t) => format!("{t}t"),
        Value::Enum { value, name } => format!("{}({value})", r.str(*name)),
        Value::Pointer(p) => format!("@{p:#x}"),
        Value::Fixed { raw, scale } => format!("{raw}*2^-{scale}"),
        Value::UFixed { raw, scale } => format!("{raw}*2^-{scale}"),
        Value::List(l) => format!("[{}]", l.iter().map(|x| fmt_value(r, x)).collect::<Vec<_>>().join(", ")),
        Value::Map(m) => format!("{{{}}}", m.iter().map(|(k, x)| format!("{}: {}", r.str(*k), fmt_value(r, x))).collect::<Vec<_>>().join(", ")),
        Value::Text(s) => format!("{s:?}"),
    }
}

/// `vtr recover`: every verified section, then a Recovered ending, a directory
/// and a trailer. Exits 3 when the scan dropped bytes, as `mcap recover` does.
fn cmd_recover(args: &[String]) {
    let p = positional(args);
    let (Some(input), Some(output)) = (p.first(), p.get(1)) else { die(USAGE) };
    if std::fs::canonicalize(input).ok().is_some_and(|a| std::fs::canonicalize(output).is_ok_and(|b| a == b)) {
        die("the output must be another file than the input");
    }
    match vtr::recover(input, output) {
        Ok(None) => println!("{input} is complete; copied to {output}"),
        Ok(Some(dropped)) => {
            let r = open(output);
            println!("recovered {} sections into {output}; {dropped} bytes after the last verified section were dropped", r.sections().len());
            if dropped > 0 {
                exit(3);
            }
        }
        Err(e) => die(format!("{input}: {e}")),
    }
}

/// Times in file units as a readable duration or range sharing one unit:
/// `102.4 ns`, `102.4–409.6 ns`, `8 ps–16.8 us`.
fn fmt_durations(a: u64, b: u64, timescale: i8) -> String {
    const UNITS: [(i32, &str); 6] = [(0, "s"), (-3, "ms"), (-6, "us"), (-9, "ns"), (-12, "ps"), (-15, "fs")];
    let secs = |u: u64| u as f64 * 10f64.powi(timescale as i32);
    let unit = |x: f64| UNITS.iter().copied().find(|&(e, _)| x >= 10f64.powi(e)).unwrap_or((-15, "fs"));
    let num = |x: f64, e: i32| {
        let s = format!("{:.3}", x / 10f64.powi(e));
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let (ua, ub) = (unit(secs(a)), unit(secs(b)));
    match (a == b, ua == ub) {
        (true, _) => format!("{} {}", num(secs(a), ua.0), ua.1),
        (false, true) => format!("{}–{} {}", num(secs(a), ua.0), num(secs(b), ua.0), ua.1),
        (false, false) => format!("{} {}–{} {}", num(secs(a), ua.0), ua.1, num(secs(b), ub.0), ub.1),
    }
}

fn fmt_mib(bytes: u64) -> String {
    let m = bytes as f64 / (1u64 << 20) as f64;
    if m >= 100.0 { format!("{m:.0} MiB") } else if m >= 0.1 { format!("{m:.1} MiB") } else { format!("{:.1} KiB", bytes as f64 / 1024.0) }
}

/// A VTR or an FST trace opened for its activity index.
enum Trace {
    Vtr(Box<Reader>),
    Fst(vtr_cli::fst::activity::FstTrace),
}

impl Trace {
    /// Opens `path` by its magic: VTR, else FST.
    fn open(path: &str) -> Trace {
        let mut magic = [0u8; 8];
        let _ = std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic));
        if magic == vtr::container::FILE_MAGIC {
            Trace::Vtr(Box::new(open(path)))
        } else {
            Trace::Fst(vtr_cli::fst::activity::FstTrace::open(path).unwrap_or_else(|e| die(format!("{path}: {e}"))))
        }
    }

    fn identity(&self, path: &str) -> vtr::activity::Identity {
        match self {
            Trace::Vtr(r) => vtr::activity::Identity::of(r).unwrap_or_else(|e| die(format!("{path}: {e}"))),
            Trace::Fst(f) => f.identity(),
        }
    }

    fn timescale(&self) -> i8 {
        match self {
            Trace::Vtr(r) => r.meta().timescale,
            Trace::Fst(f) => f.timescale(),
        }
    }

    fn signal_count(&self) -> u32 {
        match self {
            Trace::Vtr(r) => r.signal_count(),
            Trace::Fst(f) => f.signal_count(),
        }
    }

    fn build(&self, out: impl std::io::Write, opts: &vtr::activity::BuildOptions) -> vtr::Result<vtr::activity::Summary> {
        match self {
            Trace::Vtr(r) => vtr::activity::build(r, out, opts),
            Trace::Fst(f) => f.build(out, opts),
        }
    }

    fn resolve(&self, signals: &[vtr::SignalId], t0: u64, t1: u64) -> vtr::Result<Vec<vtr::SignalId>> {
        match self {
            Trace::Vtr(r) => vtr::activity::resolve(r, signals, t0, t1),
            Trace::Fst(f) => f.resolve(signals, t0, t1),
        }
    }

    /// The sidecar locations of the trace at `path`.
    fn sidecar(&self, path: &str) -> vtr::activity::Sidecar {
        vtr::activity::Sidecar::new(path.as_ref(), &self.identity(path), vtr::activity::default_cache_dir().as_deref())
    }
}

/// `vtr index`: builds or checks the activity index (docs/hierarchy-activity.html).
fn cmd_index(args: &[String]) {
    use vtr::activity::{BuildOptions, Index};
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let trace = Trace::open(path);
    let (id, sidecar) = (trace.identity(path), trace.sidecar(path));
    if has(args, "--check") {
        for at in sidecar.paths() {
            match Index::open(at, &id) {
                Ok(_) => {
                    println!("{}: valid for this trace", at.display());
                    return;
                }
                Err(vtr::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => println!("{}: not valid for this trace ({e})", at.display()),
            }
        }
        println!("{path}: no valid activity index; build one with `vtr index {path}`");
        exit(1);
    }
    let mut opts = BuildOptions::default();
    if let Some(t) = flag(args, "--threads") {
        opts.threads = t.parse().unwrap_or_else(|_| die("--threads takes a number"));
    }
    if let Some(m) = flag(args, "--memory") {
        opts.memory = m.parse::<u64>().unwrap_or_else(|_| die("--memory takes MiB")) << 20;
    }
    let (at, s) = sidecar.write(|w| trace.build(w, &opts)).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let delta = match s.delta {
        Some((a, b)) => format!("Δ {}", fmt_durations(a, b, trace.timescale())),
        None => "no value-change blocks".into(),
    };
    println!(
        "{}: {} blocks, {delta}, {} ({:.2}% of {})",
        at.display(),
        s.blocks,
        fmt_mib(s.bytes),
        100.0 * s.bytes as f64 / s.source_bytes.max(1) as f64,
        fmt_mib(s.source_bytes)
    );
}

/// A time given as file units (`250000`) or with a unit (`25000ns`, `2.5us`):
/// the first file time at or after it, or with `up` the last at or before.
fn parse_time_in(s: &str, timescale: i8, up: bool) -> u64 {
    const UNITS: [(&str, i32); 6] = [("fs", -15), ("ps", -12), ("ns", -9), ("us", -6), ("ms", -3), ("s", 0)];
    let Some((num, e)) = UNITS.iter().find_map(|&(u, e)| s.strip_suffix(u).map(|n| (n, e))) else { return parse_time(s) };
    let v: f64 = num.trim().parse().unwrap_or_else(|_| die(format!("bad time {s:?}")));
    let units = v * 10f64.powi(e - timescale as i32);
    // Within a millionth of a unit counts as exact, so 25000ns is 250000 units at 100 ps.
    let r = units.round();
    let t = if (units - r).abs() < 1e-6 { r } else if up { units.floor() } else { units.ceil() };
    if !(0.0..1.8e19).contains(&t) {
        die(format!("time {s:?} out of range"));
    }
    t as u64
}

/// A window in the file's own resolution: `25,000.0–25,500.0 ns` for 100 ps units.
fn fmt_window(t0: u64, t1: u64, timescale: i8) -> String {
    let ts = timescale as i32;
    let e = if ts >= 0 { 0 } else { -((-ts) / 3) * 3 };
    let unit = match e { 0 => "s", -3 => "ms", -6 => "us", -9 => "ns", -12 => "ps", _ => "fs" };
    let decimals = (e - ts).max(0) as u32;
    let one = |t: u64| {
        let (int, frac) = (t / 10u64.pow(decimals), t % 10u64.pow(decimals));
        let int = thousands64(int);
        if decimals == 0 { int } else { format!("{int}.{frac:0w$}", w = decimals as usize) }
    };
    let scale = if ts > 0 { 10u64.pow(ts as u32) } else { 1 };
    format!("{}–{} {unit}", one(t0.saturating_mul(scale)), one(t1.saturating_mul(scale)))
}

fn thousands64(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Scopes in preorder with their parents, and the variables of each scope
/// with their signals: the hierarchy `vtr active` prints, from either format.
struct ScopeTree {
    names: Vec<String>,
    parent: Vec<u32>,
    vars: Vec<Vec<(u32, String)>>,
}

impl ScopeTree {
    fn from_vtr(r: &Reader) -> ScopeTree {
        let h = r.hierarchy();
        let mut t = ScopeTree { names: Vec::new(), parent: Vec::new(), vars: Vec::new() };
        let mut stack: Vec<(NodeId, u32)> = h.roots().filter(|&n| h.kind(n) == vtr::NodeKind::Scope).map(|n| (n, u32::MAX)).collect();
        stack.reverse();
        while let Some((n, parent)) = stack.pop() {
            let me = t.names.len() as u32;
            t.names.push(r.name(n).to_string());
            t.parent.push(parent);
            let mut vars = Vec::new();
            let mut kids = Vec::new();
            for k in h.children(n) {
                match (h.kind(k), h.signal_of(k)) {
                    (vtr::NodeKind::Var, Some(s)) => vars.push((s.0, r.name(k).to_string())),
                    (vtr::NodeKind::Scope, _) => kids.push((k, me)),
                    _ => {}
                }
            }
            t.vars.push(vars);
            stack.extend(kids.into_iter().rev());
        }
        t
    }

    fn from_fst(path: &str) -> ScopeTree {
        use fst_reader::FstHierarchyEntry;
        let file = std::fs::File::open(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
        let mut fr = fst_reader::FstReader::open(std::io::BufReader::new(file)).unwrap_or_else(|e| die(format!("{path}: {e:?}")));
        let mut t = ScopeTree { names: Vec::new(), parent: Vec::new(), vars: Vec::new() };
        let mut open: Vec<u32> = Vec::new();
        fr.read_hierarchy(|e| match e {
            FstHierarchyEntry::Scope { name, .. } => {
                t.parent.push(open.last().copied().unwrap_or(u32::MAX));
                open.push(t.names.len() as u32);
                t.names.push(name);
                t.vars.push(Vec::new());
            }
            FstHierarchyEntry::UpScope => {
                open.pop();
            }
            FstHierarchyEntry::Var { name, handle, .. } => {
                if let Some(&s) = open.last() {
                    t.vars[s as usize].push((handle.get_index() as u32, name));
                }
            }
            _ => {}
        })
        .unwrap_or_else(|e| die(format!("{path}: {e:?}")));
        t
    }

    /// Distinct signals `keep` accepts at or below every scope.
    fn sizes(&self, keep: impl Fn(u32) -> bool) -> vtr::ScopeSizes {
        let mut c = vtr::Census::new();
        let mut open: Vec<u32> = Vec::new();
        for (i, vars) in self.vars.iter().enumerate() {
            while open.last().is_some_and(|&o| o != self.parent[i]) {
                open.pop();
                c.leave();
            }
            c.enter();
            open.push(i as u32);
            for (s, _) in vars.iter().filter(|v| keep(v.0)) {
                c.var(*s);
            }
        }
        c.finish()
    }

    /// The scope at a `.`-joined path from a root.
    fn find(&self, path: &str) -> Option<usize> {
        let mut at = u32::MAX;
        for part in path.split('.') {
            at = (0..self.names.len()).find(|&i| self.parent[i] == at && self.names[i] == part)? as u32;
        }
        Some(at as usize)
    }

    fn path(&self, mut i: u32) -> String {
        let mut parts = Vec::new();
        while i != u32::MAX {
            parts.push(self.names[i as usize].as_str());
            i = self.parent[i as usize];
        }
        parts.reverse();
        parts.join(".")
    }
}

/// `vtr active`: which scopes and signals change in a window, exactly. The
/// activity index classifies every signal; the few it leaves undecided in a
/// window narrower than its thresholds are read from the trace.
fn cmd_active(args: &[String]) {
    use vtr::activity::BuildOptions;
    let p = positional(args);
    if p.len() < 3 {
        die(USAGE);
    }
    let path = p[0].as_str();
    let trace = Trace::open(path);
    let ts = trace.timescale();
    let (t0, t1) = (parse_time_in(&p[1], ts, false), parse_time_in(&p[2], ts, true));
    if t0 > t1 {
        die(format!("the window {}..{} is empty", p[1], p[2]));
    }
    let (id, sidecar) = (trace.identity(path), trace.sidecar(path));
    let index = match sidecar.load(&id) {
        Some((_, index)) => index,
        None if has(args, "--build") => {
            let (at, _) = sidecar.write(|w| trace.build(w, &BuildOptions::default())).unwrap_or_else(|e| die(format!("{path}: {e}")));
            eprintln!("built {}", at.display());
            sidecar.load(&id).unwrap_or_else(|| die(format!("{}: the index just built does not load", at.display()))).1
        }
        None => die(format!("{path}: no valid activity index; build one with `vtr index {path}` or pass --build")),
    };
    let c = index.classify(t0, t1);
    let mut active = c.active;
    active.extend(trace.resolve(&c.undecided, t0, t1).unwrap_or_else(|e| die(format!("{path}: {e}"))));
    let mut changing = vec![false; trace.signal_count() as usize];
    for s in &active {
        changing[s.0 as usize] = true;
    }
    let tree = match &trace {
        Trace::Vtr(r) => ScopeTree::from_vtr(r),
        Trace::Fst(_) => ScopeTree::from_fst(path),
    };
    let top = flag(args, "--scope").map(|s| tree.find(&s).unwrap_or_else(|| die(format!("scope {s} not found"))));
    let how = match c.undecided.len() {
        0 => "exact".to_string(),
        n => format!("{} undecided read from the trace", thousands(n as u32)),
    };
    println!("{} · {} of {} signals change · {how}", fmt_window(t0, t1, ts), thousands(active.len() as u32), thousands(trace.signal_count()));
    // Depth of every scope below the top (or from the roots); None outside it.
    let mut level: Vec<Option<usize>> = vec![None; tree.names.len()];
    for i in 0..tree.names.len() {
        let parent = Some(tree.parent[i]).filter(|&p| p != u32::MAX).map(|p| level[p as usize]);
        level[i] = match (top, parent) {
            (Some(t), _) if t == i => Some(0),
            (Some(_), Some(Some(l))) | (None, Some(Some(l))) => Some(l + 1),
            (None, None) => Some(0),
            _ => None,
        };
    }
    if has(args, "--signals") {
        // Each changing signal once, by its first variable in preorder below the scope.
        let mut shown = vec![false; changing.len()];
        for i in (0..tree.names.len()).filter(|&i| level[i].is_some()) {
            for (s, name) in &tree.vars[i] {
                if changing[*s as usize] && !shown[*s as usize] {
                    shown[*s as usize] = true;
                    println!("{}.{name}", tree.path(i as u32));
                }
            }
        }
        return;
    }
    let depth: usize = flag(args, "--depth").map_or(1, |d| d.parse().unwrap_or_else(|_| die("--depth takes a number")));
    let (all, moving) = (tree.sizes(|_| true), tree.sizes(|s| changing[s as usize]));
    let rows: Vec<_> = (0..tree.names.len())
        .filter_map(|i| level[i].filter(|&l| l <= depth).map(|l| (i, l)))
        .map(|(i, l)| (format!("{}{}", "  ".repeat(l), tree.names[i]), thousands(moving.signals(i as u32)), thousands(all.signals(i as u32))))
        .collect();
    let w = rows.iter().fold([0; 3], |w, r| [w[0].max(r.0.chars().count()), w[1].max(r.1.len()), w[2].max(r.2.len())]);
    for (name, a, b) in rows {
        println!("{name:<0$}  {a:>1$} / {b:>2$}", w[0], w[1], w[2]);
    }
}

fn cmd_info(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    let m = r.meta();
    println!("file:        {path}");
    let recovered = match r.recovered() {
        Some(dropped) => format!(" (recovered, no directory; {dropped} bytes dropped)"),
        None => String::new(),
    };
    println!("version:     {}.{}{recovered}", r.version().0, r.version().1);
    println!("writer:      {}", m.writer);
    println!("date:        {}", m.date);
    println!("file type:   {:?}", m.file_type);
    println!("timescale:   1e{} s", m.timescale);
    println!("time zero:   {}", m.time_zero);
    match r.time_range() {
        Some((a, b)) => println!("time range:  {a} .. {b}"),
        None => println!("time range:  (empty)"),
    }
    match r.ending() {
        Ok((end, Some(t))) => println!("ended:       {end} at t={t}"),
        Ok((end, None)) => println!("ended:       {end}"),
        Err(e) => println!("ended:       unknown ({e})"),
    }
    if !m.comment.is_empty() {
        println!("comment:     {}", m.comment);
    }
    for (k, v) in &m.attrs {
        println!("attr:        {} = {}", r.str(*k), fmt_value(&r, v));
    }
    let h = r.hierarchy();
    let mut scopes = 0;
    let mut vars = 0;
    let mut streams = 0;
    let mut gens = 0;
    for n in h.ids() {
        match h.kind(n) {
            vtr::NodeKind::Scope => scopes += 1,
            vtr::NodeKind::Var => vars += 1,
            vtr::NodeKind::Stream => streams += 1,
            vtr::NodeKind::Generator => gens += 1,
            _ => {}
        }
    }
    println!("hierarchy:   {scopes} scopes, {vars} vars ({} signals), {streams} streams, {gens} generators", r.signal_count());
    println!("strings:     {}", r.strings().len());
    println!("signal blocks: {}", r.block_count());
    if let Ok(st) = r.run_stats() {
        let names = ["plain", "shuffle", "delta", "delta+shuffle", "dictionary"];
        let parts: Vec<String> = st.iter().zip(names).filter(|(s, _)| s.0 > 0).map(|(s, n)| format!("{n} {} ({} bytes)", s.0, s.1)).collect();
        println!("column runs: {}", parts.join(", "));
    }
    let (ntx, nrel) = r.tx_counts();
    println!("tx blocks:   {} ({} transactions, {nrel} relations)", r.tx_block_count(), ntx - r.log_count());
    println!("log blocks:  {} ({} records, {} sites)", r.log_block_count(), r.log_count(), r.log_sites().len());
    println!("blackout:    {} transitions", r.blackout().len());
    let mut by_kind: std::collections::BTreeMap<u32, (usize, u64)> = Default::default();
    for e in r.sections() {
        let x = by_kind.entry(e.kind).or_default();
        x.0 += 1;
        x.1 += e.len + 24;
    }
    for (k, (n, bytes)) in by_kind {
        let name = vtr::container::SectionKind::from_u32(k).map(|k| format!("{k:?}")).unwrap_or(format!("kind {k}"));
        println!("section {name:<12} x{n:<6} {bytes} bytes");
    }
}

fn print_node(r: &Reader, n: NodeId, depth: usize, max_depth: usize, vars: bool) {
    let node = r.hierarchy().node(n);
    let indent = "  ".repeat(depth);
    let name = r.name(n);
    match &node.data {
        NodeData::Scope { scope_type, component } => {
            let c = r.str(*component);
            println!("{indent}{name} [{}{}]", scope_type.name(), if c.is_empty() { String::new() } else { format!(" {c}") });
        }
        NodeData::Var { var_type, direction, signal, declares } => {
            if !vars {
                return;
            }
            let k = r.hierarchy().signal_kind(*signal).unwrap();
            let kd = match k {
                vtr::SignalKind::Bits { width, states } => format!("{width} bits, {states}-state"),
                vtr::SignalKind::Real => "real".into(),
                vtr::SignalKind::VarLen => "string".into(),
            };
            println!("{indent}{name} : {} {} {kd} sig#{}{}", var_type.name(), direction.name(), signal.0, if declares.is_none() { " (alias)" } else { "" });
        }
        NodeData::Stream { kind } => println!("{indent}{name} [stream {}]", r.str(*kind)),
        NodeData::Generator => println!("{indent}{name} [generator #{}]", n.0),
        NodeData::EnumTable { entries } => {
            println!("{indent}{name} [enum {} literals]", entries.len());
        }
    }
    for (k, v) in &node.attrs {
        println!("{indent}  @{} = {}", r.str(*k), fmt_value(r, v));
    }
    if depth + 1 < max_depth {
        for c in r.hierarchy().children(n) {
            print_node(r, c, depth + 1, max_depth, vars);
        }
    }
}

fn cmd_hier(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    let depth: usize = flag(args, "--depth").map(|d| d.parse().unwrap_or_else(|_| die("bad --depth"))).unwrap_or(usize::MAX);
    let vars = has(args, "--vars");
    if has(args, "--sizes") {
        print_sizes(&r, depth);
        return;
    }
    for root in r.hierarchy().roots() {
        print_node(&r, root, 0, depth, vars);
    }
}

/// Groups digits in threes: 16261 -> "16,261".
fn thousands(n: u32) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Scopes to `max_depth` with their totals: an aliased signal counts once per scope.
fn print_sizes(r: &Reader, max_depth: usize) {
    let (nodes, sizes) = r.hierarchy().scope_sizes();
    let mut depth = vec![0usize; nodes.len()];
    let mut rows = Vec::new();
    for i in 0..nodes.len() as u32 {
        if let Some(p) = sizes.parent(i) {
            depth[i as usize] = depth[p as usize] + 1;
        }
        if depth[i as usize] < max_depth {
            let name = format!("{}{}", "  ".repeat(depth[i as usize]), r.name(nodes[i as usize]));
            rows.push([name, thousands(sizes.scopes(i)), thousands(sizes.variables(i)), thousands(sizes.signals(i))]);
        }
    }
    let head = ["scope", "scopes", "variables", "signals"].map(String::from);
    let mut w = [0; 4];
    for row in std::iter::once(&head).chain(&rows) {
        for (k, c) in row.iter().enumerate() {
            w[k] = w[k].max(c.chars().count());
        }
    }
    for row in std::iter::once(&head).chain(&rows) {
        println!("{:<a$}  {:>b$}  {:>c$}  {:>d$}", row[0], row[1], row[2], row[3], a = w[0], b = w[1], c = w[2], d = w[3]);
    }
}

fn parse_time(s: &str) -> u64 {
    s.parse().unwrap_or_else(|_| die(format!("bad time {s:?}")))
}

fn cmd_value(args: &[String]) {
    let p = positional(args);
    if p.len() < 3 {
        die(USAGE);
    }
    let r = open(&p[0]);
    let sig = r.find_signal(&p[1], '.').unwrap_or_else(|| die(format!("signal {} not found", p[1])));
    let t = parse_time(&p[2]);
    let v = r.value_at(sig, t).unwrap_or_else(|e| die(e));
    println!("{}", v.to_ascii());
}

fn cmd_changes(args: &[String]) {
    let p = positional(args);
    if p.len() < 2 {
        die(USAGE);
    }
    let r = open(&p[0]);
    let sig = r.find_signal(&p[1], '.').unwrap_or_else(|| die(format!("signal {} not found", p[1])));
    let t0 = flag(args, "--from").map(|s| parse_time(&s)).unwrap_or(0);
    let t1 = flag(args, "--to").map(|s| parse_time(&s)).unwrap_or(u64::MAX);
    let max: usize = flag(args, "--max").map(|s| s.parse().unwrap_or_else(|_| die("bad --max"))).unwrap_or(usize::MAX);
    let c = r.changes(sig, t0, t1).unwrap_or_else(|e| die(e));
    for (t, v) in c.iter().take(max) {
        println!("{t}\t{}", v.to_ascii());
    }
    if c.len() > max {
        println!("... {} more", c.len() - max);
    }
}

fn cmd_dump(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    let t0 = flag(args, "--from").map(|s| parse_time(&s)).unwrap_or(0);
    let t1 = flag(args, "--to").map(|s| parse_time(&s)).unwrap_or(u64::MAX);
    let h = r.hierarchy();
    let mut names: Vec<String> = vec![String::new(); h.signals.len()];
    for (i, v) in h.signal_var.iter().enumerate() {
        names[i] = r.full_path(*v, ".");
    }
    let mut last = u64::MAX;
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    use std::io::Write;
    r.for_each_change(t0, t1, |t, s, v| {
        if t != last {
            let _ = writeln!(out, "#{t}");
            last = t;
        }
        let _ = writeln!(out, "{} {}", v.to_ascii(), names[s.0 as usize]);
    })
    .unwrap_or_else(|e| die(e));
}

fn cmd_to_vcd(args: &[String], fst: bool) {
    let p = positional(args);
    if p.len() < 2 {
        die(USAGE);
    }
    let t = std::time::Instant::now();
    let n = if fst { vtr_cli::vcdout::fst_to_vcd(&p[0], &p[1]) } else { vtr_cli::vcdout::vtr_to_vcd(&p[0], &p[1]) }.unwrap_or_else(|e| die(e));
    println!("{{\"changes\": {n}, \"wall_s\": {}}}", t.elapsed().as_secs_f64());
}

fn cmd_vcd_compare(args: &[String]) {
    let p = positional(args);
    if p.len() < 2 {
        die(USAGE);
    }
    let a = vtr_cli::vcdout::vcd_stats(&p[0]).unwrap_or_else(|e| die(format!("{}: {e}", p[0])));
    let b = vtr_cli::vcdout::vcd_stats(&p[1]).unwrap_or_else(|e| die(format!("{}: {e}", p[1])));
    let c = vtr_cli::vcdout::compare(&a, &b);
    let examples: Vec<_> = c.mismatches.iter().take(10).map(|(n, x, y)| serde_json::json!({"signal": n, "a": x, "b": y})).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "changes_a": c.changes_a, "changes_b": c.changes_b, "signals_a": c.signals_a, "signals_b": c.signals_b,
            "mismatched_signals": c.mismatches.len(), "examples": examples, "identical": c.identical(),
        }))
        .unwrap()
    );
    if !c.identical() {
        exit(1);
    }
}

fn cmd_tx(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    let max: usize = flag(args, "--max").map(|s| s.parse().unwrap_or_else(|_| die("bad --max"))).unwrap_or(100);
    let mut q = TxQuery::default();
    if let (Some(a), Some(b)) = (flag(args, "--from"), flag(args, "--to")) {
        q.window = Some((parse_time(&a), parse_time(&b)));
    }
    if let Some(name) = flag(args, "--stream") {
        q.stream = r.streams().find(|&s| r.name(s) == name || r.full_path(s, ".") == name);
        if q.stream.is_none() {
            die(format!("stream {name} not found"));
        }
    }
    let show = |tx: &vtr::Transaction| {
        let gen = r.name(tx.generator);
        let stream = r.generator_stream(tx.generator).map(|s| r.full_path(s, ".")).unwrap_or_default();
        println!("tx {} {stream}/{gen} [{} .. {}] status={} kind={:?}{}", tx.id, tx.begin, tx.end, tx.status.name(), tx.kind, tx.parent.map(|p| format!(" parent={p}")).unwrap_or_default());
        for a in &tx.attrs {
            println!("    {} = {}", r.str(a.key), fmt_value(&r, &a.value));
        }
        for e in &tx.events {
            println!("    event @{} {} {}", e.time, r.str(e.name), fmt_value(&r, &Value::Map(e.attrs.clone())));
        }
        for s in &tx.stages {
            println!("    stage {} lane={} [{} .. {}] {}", r.str(s.name), r.str(s.lane), s.begin, s.end.map(|e| e.to_string()).unwrap_or("open".into()), fmt_value(&r, &Value::Map(s.attrs.clone())));
        }
    };
    if let Some(ids) = flag(args, "--id") {
        for id in ids.split(',') {
            let id: u64 = id.parse().unwrap_or_else(|_| die("bad --id"));
            match r.transaction(id).unwrap_or_else(|e| die(e)) {
                Some(tx) => {
                    show(&tx);
                    for rel in r.relations_from(id).unwrap_or_else(|e| die(e)) {
                        println!("    -> {} {} {}", r.str(rel.kind), rel.to, fmt_value(&r, &Value::Map(rel.attrs.clone())));
                    }
                    for rel in r.relations_to(id).unwrap_or_else(|e| die(e)) {
                        println!("    <- {} {} {}", r.str(rel.kind), rel.from, fmt_value(&r, &Value::Map(rel.attrs.clone())));
                    }
                }
                None => die(format!("transaction {id} not found")),
            }
        }
        return;
    }
    let mut n = 0;
    r.visit_transactions(&q, |tx| {
        n += 1;
        if n <= max {
            show(tx);
        }
        true
    })
    .unwrap_or_else(|e| die(e));
    if n > max {
        println!("... {} more", n - max);
    }
}

fn cmd_log(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    if has(args, "--sites") {
        for (i, s) in r.log_sites().iter().enumerate() {
            let loc = match (s.file, s.line) {
                (Some(f), Some(l)) => format!(" ({}:{l})", r.str(f)),
                (Some(f), None) => format!(" ({})", r.str(f)),
                _ => String::new(),
            };
            let args: Vec<String> = s.args.iter().zip(s.names.iter()).map(|(t, n)| format!("{}:{}", r.str(*n), t.name())).collect();
            println!("site {i} {} {} {:?}{loc} [{}]", r.full_path(s.stream, "."), s.severity.name(), r.str(s.fmt), args.join(", "));
        }
        return;
    }
    let max: usize = flag(args, "--max").map(|s| s.parse().unwrap_or_else(|_| die("bad --max"))).unwrap_or(usize::MAX);
    let mut q = vtr::LogQuery::default();
    if let (Some(a), Some(b)) = (flag(args, "--from"), flag(args, "--to")) {
        q.window = Some((parse_time(&a), parse_time(&b)));
    }
    if let Some(s) = flag(args, "--severity") {
        q.min_severity = vtr::Severity::from_name(&s).unwrap_or_else(|| die(format!("bad severity {s}")));
    }
    if let Some(name) = flag(args, "--stream") {
        q.stream = r.streams().find(|&s| r.name(s) == name || r.full_path(s, ".") == name);
        if q.stream.is_none() {
            die(format!("stream {name} not found"));
        }
    }
    let mut n = 0usize;
    let mut line = String::new();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    use std::io::Write;
    r.visit_log(&q, |rec| {
        n += 1;
        if n > max {
            return false;
        }
        line.clear();
        rec.format_into(r.strings(), &mut line);
        let _ = writeln!(out, "{} {:<5} {}: {}", rec.time, rec.severity().name(), r.full_path(rec.site.stream, "."), line);
        true
    })
    .unwrap_or_else(|e| die(e));
    let _ = out.flush();
}

fn cmd_clocks(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    for c in r.clocks() {
        let tl = r.clock(c.id).unwrap_or_else(|e| die(e));
        let stopped: u64 = tl.stretches().windows(2).filter_map(|w| tl.cycle_at(w[0].end).filter(|a| a.stopped).map(|_| w[1].begin - w[0].end)).sum();
        println!("clock {} {}: {} edges, {} stretches, stopped {stopped}{}", c.id.0, c.path, tl.edge_count(), tl.stretches().len(), if tl.is_open() { ", running at close" } else { "" });
        for s in tl.stretches() {
            let period = if s.period == 0 { "single edge".to_string() } else { format!("period {}", s.period) };
            println!("    [{} .. {}] {period}, {} edges from cycle {}", s.begin, s.end, s.edges(), s.first_cycle);
        }
    }
}

fn cmd_convert(args: &[String]) {
    let p = positional(args);
    if p.len() < 2 {
        die(USAGE);
    }
    let (input, output) = (&p[0], &p[1]);
    let mut opts = WriterOptions::default();
    if let Some(c) = flag(args, "--codec") {
        opts.compression = match c.as_str() {
            "zstd" => Compression::ZSTD_DEFAULT,
            "lz4" => Compression::LZ4,
            "none" => Compression::NONE,
            _ => die("bad --codec"),
        };
    }
    if let Some(l) = flag(args, "--level") {
        opts.compression.level = l.parse().unwrap_or_else(|_| die("bad --level"));
        if opts.compression.codec == Codec::None {
            opts.compression.codec = Codec::Zstd;
        }
    }
    if let Some(g) = flag(args, "--group-size") {
        opts.group_size = g.parse().unwrap_or_else(|_| die("bad --group-size"));
    }
    if let Some(b) = flag(args, "--block-records") {
        opts.block_records = b.parse().unwrap_or_else(|_| die("bad --block-records"));
    }
    opts.background = !has(args, "--no-background");
    opts.dedup = !has(args, "--no-dedup");
    opts.checksums = !has(args, "--no-checksums");
    let states: Option<u8> = flag(args, "--states").map(|s| s.parse().unwrap_or_else(|_| die("bad --states")));
    let mut w = Writer::create_with(output, opts).unwrap_or_else(|e| die(format!("{output}: {e}")));
    let start = std::time::Instant::now();
    let lower = input.to_ascii_lowercase();
    let res: Result<(), Box<dyn std::error::Error>> = if lower.ends_with(".fst") {
        fst::convert_fst(input, &mut w, &fst::FstConvertOptions { states, progress: has(args, "--progress") })
    } else if lower.ends_with(".vcd") || lower.ends_with(".ghw") {
        vcd::convert_wellen(input, &mut w, states.unwrap_or(4))
    } else if lower.ends_with(".log") || lower.ends_with(".log.gz") || lower.ends_with(".kanata") || lower.ends_with(".kanata.gz") {
        kanata::convert_kanata(input, &mut w)
    } else if lower.ends_with(".json") {
        otlp::convert_otlp_json(input, &mut w)
    } else if lower.ends_with(".ftr") {
        ftr::convert_ftr(input, &mut w)
    } else {
        die(format!("unknown input format: {input}"))
    };
    if let Err(e) = res {
        die(format!("{input}: {e}"));
    }
    let stats = w.stats();
    w.close().unwrap_or_else(|e| die(format!("{output}: {e}")));
    let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "wrote {output}: {} bytes, {} signals, {} value changes, {} transactions, {} blocks in {:.2}s",
        size,
        stats.signals,
        stats.records,
        stats.transactions,
        stats.blocks,
        start.elapsed().as_secs_f64()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    match cmd {
        "info" => cmd_info(rest),
        "hier" => cmd_hier(rest),
        "value" => cmd_value(rest),
        "changes" => cmd_changes(rest),
        "dump" => cmd_dump(rest),
        "to-vcd" => cmd_to_vcd(rest, false),
        "fst-to-vcd" => cmd_to_vcd(rest, true),
        "vcd-compare" => cmd_vcd_compare(rest),
        "tx" => cmd_tx(rest),
        "log" => cmd_log(rest),
        "clocks" => cmd_clocks(rest),
        "convert" => cmd_convert(rest),
        "recover" => cmd_recover(rest),
        "index" => cmd_index(rest),
        "active" => cmd_active(rest),
        "--version" | "-V" => println!("vtr {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("{USAGE}");
            exit(if cmd.is_empty() || cmd == "--help" || cmd == "-h" { 0 } else { 2 });
        }
    }
}
