//! `vtr-bench`: workload preparation and benchmark drivers.

mod load;
mod read;
mod replay;
mod tx;
mod util;
mod write;

use vtr::{Codec, Compression, WriterOptions};

const USAGE: &str = "\
vtr-bench commands:
  prepare-fst <in.fst> <out.rpl> [--replicate N]     FST -> replay workload
  gen <long_sparse|many_active|wide_bus> <scale> <out.rpl>
  write <in.rpl> <out.vtr> [--codec zstd|lz4|none] [--level L] [--no-background] [--group-size N] [--label S]
  read <in.fst> <in.vtr> [--seed S]                   read/navigation benchmarks vs wellen (JSON)
  load <in.vtr> <unique|requests> <count> [--seed S]   VTR history loads, without/with replacement (JSON)
  gen-kanata <out.log> <n_insn> [--seed S]
  gen-tlm <out.txr> <n_insn> [--seed S]
  tx-write <in.txr> <out.vtr> [--codec ..] [--no-background]
  kanata-write <in.log> <out.vtr> [--codec ..]
  tx-read <in.vtr> [--seed S]
  stream <in.vtr>                                     count all changes via for_each_change
  replay-info <in.rpl>";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn writer_opts(args: &[String]) -> WriterOptions {
    let mut o = WriterOptions::default();
    if let Some(c) = flag(args, "--codec") {
        o.compression = match c.as_str() {
            "zstd" => Compression::ZSTD_DEFAULT,
            "lz4" => Compression::LZ4,
            "none" => Compression::NONE,
            _ => panic!("bad codec"),
        };
    }
    if let Some(l) = flag(args, "--level") {
        o.compression.level = l.parse().unwrap();
        if o.compression.codec == Codec::None {
            o.compression.codec = Codec::Zstd;
        }
    }
    if let Some(g) = flag(args, "--group-size") {
        o.group_size = g.parse().unwrap();
    }
    if let Some(b) = flag(args, "--run-bytes") {
        o.run_bytes = b.parse().unwrap();
    }
    if let Some(b) = flag(args, "--block-records") {
        o.block_records = b.parse().unwrap();
    }
    o.background = !has(args, "--no-background");
    o.dedup = !has(args, "--no-dedup");
    o
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let pos: Vec<String> = {
        // strip flag values
        let mut out = Vec::new();
        let mut skip = false;
        for a in &args {
            if skip {
                skip = false;
                continue;
            }
            if a.starts_with("--") {
                skip = !matches!(a.as_str(), "--no-background" | "--no-dedup");
                continue;
            }
            out.push(a.clone());
        }
        let _ = pos;
        out
    };
    let cmd = pos.first().map(|s| s.as_str()).unwrap_or("");
    let seed: u64 = flag(&args, "--seed").map(|s| s.parse().unwrap_or_else(|e| {
        eprintln!("error: invalid --seed: {e}");
        std::process::exit(2);
    })).unwrap_or(42);
    match cmd {
        "prepare-fst" => {
            let rp = replay::Replay::from_fst(&pos[1]).expect("read fst");
            let rp = match flag(&args, "--replicate") {
                Some(n) => rp.replicate(n.parse().unwrap()),
                None => rp,
            };
            rp.save(&pos[2]).unwrap();
            eprintln!("{}: {} signals, {} changes, {} time steps, end {}", pos[2], rp.signals.len(), rp.recs.len(), rp.n_times(), rp.end_time);
        }
        "gen" => {
            let rp = replay::generate(&pos[1], pos[2].parse().unwrap());
            rp.save(&pos[3]).unwrap();
            eprintln!("{}: {} signals, {} changes, {} time steps", pos[3], rp.signals.len(), rp.recs.len(), rp.n_times());
        }
        "replay-info" => {
            let rp = replay::Replay::load(&pos[1]).unwrap();
            println!("{}", serde_json::json!({"signals": rp.signals.len(), "changes": rp.recs.len(), "time_steps": rp.n_times(), "end_time": rp.end_time}));
        }
        "write" => {
            let rp = replay::Replay::load(&pos[1]).unwrap();
            let label = flag(&args, "--label").unwrap_or_else(|| "vtr".into());
            let r = write::run(&rp, &pos[2], writer_opts(&args), &label);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
        "load" => {
            if pos.len() != 4 {
                eprintln!("usage: vtr-bench load <file.vtr> <unique|requests> <count> [--seed S]");
                std::process::exit(2);
            }
            let result = pos[3].parse::<usize>().map_err(|e| e.to_string())
                .and_then(|count| load::run(&pos[1], &pos[2], count, seed));
            match result {
                Ok(r) => println!("{}", serde_json::to_string_pretty(&r).unwrap()),
                Err(e) => { eprintln!("error: {e}"); std::process::exit(2); }
            }
        }
        "read" => {
            let r = read::run(&pos[1], &pos[2], seed);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
        "gen-kanata" => {
            tx::gen_kanata(&pos[1], pos[2].parse().unwrap(), seed).unwrap();
        }
        "gen-tlm" => {
            let rp = tx::gen_tlm(pos[2].parse().unwrap(), seed);
            rp.save(&pos[1]).unwrap();
            eprintln!("{}: {} ops", pos[1], rp.ops.len());
        }
        "tx-write" => {
            let rp = tx::TxReplay::load(&pos[1]).unwrap();
            let label = flag(&args, "--label").unwrap_or_else(|| "vtr".into());
            let r = tx::tx_write_vtr(&rp, &pos[2], writer_opts(&args), &label);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
        "kanata-write" => {
            let r = tx::kanata_write_vtr(&pos[1], &pos[2], writer_opts(&args));
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
        "plan" => {
            // Exports the read-benchmark query plan (signals, times) for the C harness.
            let r = vtr::Reader::open(&pos[1]).unwrap();
            let p = read::plan(&r, seed);
            println!("{}", p.0.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" "));
            println!("{}", p.1.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(" "));
        }
        "stream" => {
            let r = vtr::Reader::open(&pos[1]).unwrap();
            let t = std::time::Instant::now();
            let mut n = 0u64;
            r.for_each_change(0, u64::MAX, |_, _, _| n += 1).unwrap();
            println!("{{\"changes\": {n}, \"wall_s\": {}}}", t.elapsed().as_secs_f64());
        }
        "tx-read" => {
            let r = tx::tx_read(&pos[1], seed);
            println!("{}", serde_json::to_string_pretty(&r).unwrap());
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
