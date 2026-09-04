//! `vtr` command-line tool: inspect, query and convert VTR traces.

use vtr_cli::{fst, ftr, kanata, otlp, vcd};
use std::process::exit;
use vtr::{Codec, Compression, NodeData, NodeId, Reader, TxQuery, Value, Writer, WriterOptions};

const USAGE: &str = "\
vtr - Vibe Trace Record tools

USAGE:
  vtr info <file.vtr>                         file summary (meta, counts, sections)
  vtr hier <file.vtr> [--depth N] [--vars]    print the hierarchy
  vtr value <file.vtr> <path> <time>          value of a signal at a time
  vtr changes <file.vtr> <path> [--from T] [--to T] [--max N]
  vtr dump <file.vtr> [--from T] [--to T]     all value changes in time order (VCD-like)
  vtr tx <file.vtr> [--stream NAME] [--from T] [--to T] [--max N] [--id ID]
  vtr convert <input> <output.vtr> [--states 2|4|9] [--codec zstd|lz4|none] [--level L]
              [--group-size N] [--block-records N] [--no-background] [--no-dedup]
              input formats by extension: .fst .vcd .log/.kanata[.gz] .json (OTLP) .ftr

Times are integers in the file's time unit. Paths use '.' as separator.";

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
            skip = !matches!(a.as_str(), "--vars" | "--no-background" | "--no-dedup" | "--progress" | "--no-checksums");
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
    }
}

fn cmd_info(args: &[String]) {
    let p = positional(args);
    let path = p.first().unwrap_or_else(|| die(USAGE));
    let r = open(path);
    let m = r.meta();
    println!("file:        {path}");
    println!("version:     {}.{}{}", r.version().0, r.version().1, if r.recovered() { " (recovered, no directory)" } else { "" });
    println!("writer:      {}", m.writer);
    println!("date:        {}", m.date);
    println!("file type:   {:?}", m.file_type);
    println!("timescale:   1e{} s", m.timescale);
    println!("time zero:   {}", m.time_zero);
    match r.time_range() {
        Some((a, b)) => println!("time range:  {a} .. {b}"),
        None => println!("time range:  (empty)"),
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
    let (ntx, nrel) = r.tx_counts();
    println!("tx blocks:   {} ({ntx} transactions, {nrel} relations)", r.tx_block_count());
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
    for root in r.hierarchy().roots() {
        print_node(&r, root, 0, depth, vars);
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
            println!("    {:?} {} = {}", a.phase, r.str(a.key), fmt_value(&r, &a.value));
        }
        for e in &tx.events {
            println!("    event @{} {} {}", e.time, r.str(e.name), fmt_value(&r, &Value::Map(e.attrs.clone())));
        }
        for s in &tx.stages {
            println!("    stage {} lane={} [{} .. {}] {}", r.str(s.name), r.str(s.lane), s.begin, s.end.map(|e| e.to_string()).unwrap_or("open".into()), fmt_value(&r, &Value::Map(s.attrs.clone())));
        }
    };
    if let Some(id) = flag(args, "--id") {
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
        "tx" => cmd_tx(rest),
        "convert" => cmd_convert(rest),
        "--version" | "-V" => println!("vtr {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("{USAGE}");
            exit(if cmd.is_empty() || cmd == "--help" || cmd == "-h" { 0 } else { 2 });
        }
    }
}
