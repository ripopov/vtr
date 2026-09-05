use vtr_kdb::{Database, Debugger};
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: vtr-kdb check DESIGN.kdb.json TRACE.vtr [--prefix TOP]\n       vtr-kdb trace DESIGN.kdb.json TRACE.vtr SIGNAL --time T [--depth N] [--prefix TOP]\n       vtr-kdb netlist DESIGN.kdb.json TRACE.vtr INSTANCE --time T --output FILE.svg [--prefix TOP]\nExport: python tools/kdb/export.py --top MODULE -o DESIGN.kdb.json FILE.sv ...";
    if args.len() < 3 {
        return Err(usage.into());
    }
    let mut output = None;
    let mut time = None;
    let mut depth = 8;
    let mut prefix = String::new();
    let start = match args[0].as_str() {
        "check" => 3,
        "trace" | "netlist" if args.len() >= 4 => 4,
        _ => return Err(usage.into()),
    };
    let mut i = start;
    while i < args.len() {
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {}", args[i]))?;
        match args[i].as_str() {
            "--output" => output = Some(value.clone()),
            "--time" => time = Some(value.parse::<u64>().map_err(|e| e.to_string())?),
            "--depth" => depth = value.parse::<usize>().map_err(|e| e.to_string())?,
            "--prefix" => prefix = value.clone(),
            _ => return Err(format!("unknown option {}", args[i])),
        }
        i += 2;
    }
    let db = Database::open(&args[1])?;
    let reader = vtr::Reader::open(&args[2]).map_err(|e| e.to_string())?;
    let mut debug = Debugger::attach(&db, &reader, &prefix)?;
    for d in &debug.diagnostics {
        eprintln!("{d}");
    }
    if args[0] == "check" {
        println!(
            "KDB v{}: {} ({} symbols), design {}",
            db.version,
            db.top,
            db.symbols.len(),
            db.design_id
        );
        if debug
            .diagnostics
            .iter()
            .any(|d| d.starts_with("missing trace"))
        {
            return Err("incomplete trace mapping".into());
        }
    } else if args[0] == "netlist" {
        let graph = vtr_kdb::netlist::NetlistIndex::new(&db)?
            .module(&args[3])?
            .layout()?;
        let svg = graph.svg(&mut debug, time.ok_or("netlist requires --time")?)?;
        std::fs::write(output.ok_or("netlist requires --output FILE.svg")?, svg)
            .map_err(|e| e.to_string())?;
    } else {
        let tree = debug.trace(&args[3], time.ok_or("trace requires --time")?, depth)?;
        println!(
            "time unit: 1e{} s; timestamps are settled values, '-' means before event",
            reader.meta().timescale
        );
        print!("{}", tree.render());
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
}
