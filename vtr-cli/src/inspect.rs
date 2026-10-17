use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use vtr::{NodeData, NodeId, Reader, TxQuery, Value};
fn fmt_value(r: &Reader, v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::I64(i) => i.to_string(),
        Value::U64(u) => u.to_string(),
        Value::F64(f) => f.to_string(),
        Value::Str(s) => format!("{:?}", r.str(*s)),
        Value::Bytes(b) => {
            let mut text = String::from("0x");
            for byte in b {
                write!(text, "{byte:02x}").unwrap();
            }
            text
        }
        Value::Bits { width, data } => vtr::SignalValue::Bits {
            width: *width,
            states: 2,
            data,
        }
        .to_ascii(),
        Value::Logic { width, data } => vtr::SignalValue::Bits {
            width: *width,
            states: 4,
            data,
        }
        .to_ascii(),
        Value::Logic9 { width, data } => vtr::SignalValue::Bits {
            width: *width,
            states: 9,
            data,
        }
        .to_ascii(),
        Value::Time(t) => format!("{t}t"),
        Value::Enum { value, name } => format!("{}({value})", r.str(*name)),
        Value::Pointer(p) => format!("@{p:#x}"),
        Value::Fixed { raw, scale } => format!("{raw}*2^-{scale}"),
        Value::UFixed { raw, scale } => format!("{raw}*2^-{scale}"),
        Value::List(l) => format!(
            "[{}]",
            l.iter()
                .map(|x| fmt_value(r, x))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Map(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, x)| format!("{}: {}", r.str(*k), fmt_value(r, x)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Text(s) => format!("{s:?}"),
    }
}

pub fn info(path: &Path) -> Result<()> {
    let r = Reader::open(path)?;
    let path = path.display();
    let m = r.meta();
    println!("file:        {path}");
    let recovered = match r.recovered() {
        Some(dropped) => format!(" (recovered, no directory; {dropped} bytes dropped)"),
        None => String::new(),
    };
    println!(
        "version:     {}.{}{recovered}",
        r.version().0,
        r.version().1
    );
    println!("writer:      {}", m.writer);
    println!("date:        {}", m.date);
    println!("file type:   {:?}", m.file_type);
    println!("timescale:   1e{} s", m.timescale);
    println!("time zero:   {}", m.time_zero);
    match r.time_range() {
        Some((a, b)) => println!("time range:  {a} .. {b}"),
        None => println!("time range:  (empty)"),
    }
    match r.ending()? {
        (end, Some(t)) => println!("ended:       {end} at t={t}"),
        (end, None) => println!("ended:       {end}"),
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
    let mut gens = 0;
    for n in h.ids() {
        match h.kind(n) {
            vtr::NodeKind::Scope => scopes += 1,
            vtr::NodeKind::Var => vars += 1,
            vtr::NodeKind::Generator => gens += 1,
            vtr::NodeKind::EnumTable => {}
        }
    }
    println!(
        "hierarchy:   {scopes} scopes, {vars} vars ({} signals), {gens} generators",
        r.signal_count()
    );
    println!("strings:     {}", r.strings().len());
    println!("signal blocks: {}", r.block_count());
    {
        let st = r.run_stats()?;
        let names = ["plain", "shuffle", "delta", "delta+shuffle", "dictionary"];
        let parts: Vec<String> = st
            .iter()
            .zip(names)
            .filter(|(s, _)| s.0 > 0)
            .map(|(s, n)| format!("{n} {} ({} bytes)", s.0, s.1))
            .collect();
        println!("column runs: {}", parts.join(", "));
    }
    let (ntx, nrel) = r.tx_counts();
    println!(
        "tx blocks:   {} ({} transactions, {nrel} relations)",
        r.tx_block_count(),
        ntx - r.log_count()
    );
    println!(
        "log blocks:  {} ({} records, {} sites)",
        r.log_block_count(),
        r.log_count(),
        r.log_sites().len()
    );
    println!("blackout:    {} transitions", r.blackout().len());
    let mut by_kind: std::collections::BTreeMap<u32, (usize, u64)> = BTreeMap::default();
    for e in r.sections() {
        let x = by_kind.entry(e.kind).or_default();
        x.0 += 1;
        x.1 += e.len + 24;
    }
    for (k, (n, bytes)) in by_kind {
        let name = vtr::container::SectionKind::from_u32(k)
            .map(|k| format!("{k:?}"))
            .unwrap_or(format!("kind {k}"));
        println!("section {name:<12} x{n:<6} {bytes} bytes");
    }
    Ok(())
}

fn print_node(r: &Reader, n: NodeId, depth: usize, max_depth: usize, vars: bool) {
    if depth >= max_depth {
        return;
    }
    let node = r.hierarchy().node(n);
    let indent = "  ".repeat(depth);
    let name = r.name(n);
    match &node.data {
        NodeData::Scope {
            scope_type,
            component,
        } => {
            let c = r.str(*component);
            println!(
                "{indent}{name} [{}{}]",
                scope_type.name(),
                if c.is_empty() {
                    String::new()
                } else {
                    format!(" {c}")
                }
            );
        }
        NodeData::Var {
            var_type,
            direction,
            signal,
            declares,
        } => {
            if !vars {
                return;
            }
            let k = r.hierarchy().signal_kind(*signal).unwrap();
            let kd = match k {
                vtr::SignalKind::Bits { width, states } => format!("{width} bits, {states}-state"),
                vtr::SignalKind::Real => "real".into(),
                vtr::SignalKind::VarLen => "string".into(),
            };
            println!(
                "{indent}{name} : {} {} {kd} sig#{}{}",
                var_type.name(),
                direction.name(),
                signal.0,
                if declares.is_none() { " (alias)" } else { "" }
            );
        }
        NodeData::Generator { kind } => println!("{indent}{name} [generator {}]", r.str(*kind)),

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

pub fn hierarchy(path: &Path, depth: usize, vars: bool, sizes: bool) -> Result<()> {
    let r = Reader::open(path)?;
    if sizes {
        print_sizes(&r, depth);
    } else {
        for root in r.hierarchy().roots() {
            print_node(&r, root, 0, depth, vars);
        }
    }
    Ok(())
}

fn thousands(n: u32) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn print_sizes(r: &Reader, max_depth: usize) {
    let (nodes, sizes) = r.hierarchy().scope_sizes();
    let mut depth = vec![0usize; nodes.len()];
    let mut rows = Vec::new();
    for i in 0..nodes.len() as u32 {
        if let Some(p) = sizes.parent(i) {
            depth[i as usize] = depth[p as usize] + 1;
        }
        if depth[i as usize] < max_depth {
            let name = format!(
                "{}{}",
                "  ".repeat(depth[i as usize]),
                r.name(nodes[i as usize])
            );
            rows.push([
                name,
                thousands(sizes.scopes(i)),
                thousands(sizes.variables(i)),
                thousands(sizes.signals(i)),
            ]);
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
        println!(
            "{:<a$}  {:>b$}  {:>c$}  {:>d$}",
            row[0],
            row[1],
            row[2],
            row[3],
            a = w[0],
            b = w[1],
            c = w[2],
            d = w[3]
        );
    }
}

pub fn transactions(
    path: &Path,
    scope: Option<&str>,
    window: (u64, u64),
    max: usize,
    ids: &[u64],
) -> Result<()> {
    let r = Reader::open(path)?;
    let q = TxQuery {
        window: Some(window),
        scope: find_scope(&r, scope)?,
        ..Default::default()
    };
    let show = |tx: &vtr::Transaction| {
        let r#gen = r.name(tx.generator);
        let scope = r
            .hierarchy()
            .parent(tx.generator)
            .map(|s| r.full_path(s, "."))
            .unwrap_or_default();
        println!(
            "tx {} {scope}/{gen} [{} .. {}] status={} kind={:?}{}",
            tx.id,
            tx.begin,
            tx.end,
            tx.status.name(),
            tx.kind,
            tx.parent
                .map(|p| format!(" parent={p}"))
                .unwrap_or_default()
        );
        for a in &tx.attrs {
            println!("    {} = {}", r.str(a.key), fmt_value(&r, &a.value));
        }
        for e in &tx.events {
            println!(
                "    event @{} {} {}",
                e.time,
                r.str(e.name),
                fmt_value(&r, &Value::Map(e.attrs.clone()))
            );
        }
        for s in &tx.stages {
            println!(
                "    stage {} lane={} [{} .. {}] {}",
                r.str(s.name),
                r.str(s.lane),
                s.begin,
                s.end.map_or("open".into(), |e| e.to_string()),
                fmt_value(&r, &Value::Map(s.attrs.clone()))
            );
        }
    };
    if !ids.is_empty() {
        for &id in ids {
            match r.transaction(id)? {
                Some(tx) => {
                    show(&tx);
                    for rel in r.relations_from(id)? {
                        println!(
                            "    -> {} {} {}",
                            r.str(rel.kind),
                            rel.to,
                            fmt_value(&r, &Value::Map(rel.attrs.clone()))
                        );
                    }
                    for rel in r.relations_to(id)? {
                        println!(
                            "    <- {} {} {}",
                            r.str(rel.kind),
                            rel.from,
                            fmt_value(&r, &Value::Map(rel.attrs.clone()))
                        );
                    }
                }
                None => anyhow::bail!("transaction {id} not found"),
            }
        }
        return Ok(());
    }
    let mut n = 0;
    r.visit_transactions(&q, |tx| {
        n += 1;
        if n <= max {
            show(tx);
        }
        true
    })?;
    if n > max {
        println!("... {} more", n - max);
    }
    Ok(())
}

pub fn logs(
    path: &Path,
    scope: Option<&str>,
    window: (u64, u64),
    max: usize,
    severity: vtr::Severity,
    sites: bool,
) -> Result<()> {
    let r = Reader::open(path)?;
    if sites {
        for (i, s) in r.log_sites().iter().enumerate() {
            let loc = match (s.file, s.line) {
                (Some(f), Some(l)) => format!(" ({}:{l})", r.str(f)),
                (Some(f), None) => format!(" ({})", r.str(f)),
                _ => String::new(),
            };
            let args: Vec<String> = s
                .args
                .iter()
                .zip(s.names.iter())
                .map(|(t, n)| format!("{}:{}", r.str(*n), t.name()))
                .collect();
            println!(
                "site {i} {} {} {:?}{loc} [{}]",
                r.full_path(s.node, "."),
                s.severity.name(),
                r.str(s.fmt),
                args.join(", ")
            );
        }
        return Ok(());
    }
    let q = vtr::LogQuery {
        window: Some(window),
        scope: find_scope(&r, scope)?,
        min_severity: severity,
        ..Default::default()
    };
    let mut n = 0usize;
    let mut line = String::new();
    let mut output_error = None;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    use std::io::Write;
    r.visit_log(&q, |rec| {
        n += 1;
        if n > max {
            return false;
        }
        line.clear();
        rec.format_into(r.strings(), &mut line);
        if let Err(error) = writeln!(
            out,
            "{} {:<5} {}: {}",
            rec.time,
            rec.severity().name(),
            r.full_path(rec.site.node, "."),
            line
        ) {
            output_error = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = output_error {
        return Err(error.into());
    }
    out.flush()?;
    Ok(())
}

pub fn clocks(path: &Path) -> Result<()> {
    let r = Reader::open(path)?;
    for c in r.clocks() {
        let tl = r.clock(c.id)?;
        let stopped: u64 = tl
            .stretches()
            .windows(2)
            .filter_map(|w| {
                tl.cycle_at(w[0].end)
                    .filter(|a| a.stopped)
                    .map(|_| w[1].begin - w[0].end)
            })
            .sum();
        println!(
            "clock {} {}: {} edges, {} stretches, stopped {stopped}{}",
            c.id.0,
            c.path,
            tl.edge_count(),
            tl.stretches().len(),
            if tl.is_open() {
                ", running at close"
            } else {
                ""
            }
        );
        for s in tl.stretches() {
            let period = if s.period == 0 {
                "single edge".to_string()
            } else {
                format!("period {}", s.period)
            };
            println!(
                "    [{} .. {}] {period}, {} edges from cycle {}",
                s.begin,
                s.end,
                s.edges(),
                s.first_cycle
            );
        }
    }
    Ok(())
}

fn find_scope(r: &Reader, name: Option<&str>) -> Result<Option<NodeId>> {
    name.map(|name| {
        r.hierarchy()
            .nodes_of_kind(vtr::NodeKind::Scope)
            .find(|&s| r.full_path(s, ".") == name)
            .with_context(|| format!("scope {name:?} not found; use its full hierarchy path"))
    })
    .transpose()
}
