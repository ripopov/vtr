#![recursion_limit = "256"]
//! `vtr-bench`: workload preparation and benchmark drivers.

mod activity;
mod scopes;
mod load;
mod logw;
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
  log-write <n> <out.vtr> [--no-background] [--codec ..] [--level L] [--rate R] [--commit-ms C]
                                                      synthetic simulator log through Writer::log (JSON);
                                                      --rate paces it to R records/s
  log-encode <n>                                      log block encoder micro-benchmark per codec (JSON)
  activity <in.vtr> [--eps E] [--mem M] [--windows N] [--threads T]   range-activity index measurements (JSON)
  activity-export <in.vtr> <t0> <t1> <out.bin> [--eps E] [--mem M]   demo data for docs/hierarchy-activity.html
  gen-bursty <out.vtr> [--scale S]                    synthetic ps trace with sleep phases and gated units
  scopes <in.vtr> [--children NAME]                   distinct signals per scope, three ways (JSON)
  gen-gates <in.vtr> <out.vtr> [--copies N]           gate-level hierarchy expanded from an RTL one
  stream <in.vtr>                                     count all changes via for_each_change
  digest <in.vtr> <t>                                 changes and log records at times <= t, with an
                                                      order-independent hash of each (JSON)
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
    if let Some(ms) = flag(args, "--commit-ms") {
        o.commit_interval = std::time::Duration::from_millis(ms.parse().unwrap());
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
                skip = !matches!(a.as_str(), "--no-background" | "--no-dedup" | "--as-tx");
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
        "log-write" => {
            let n: u64 = pos[1].parse().unwrap();
            let label = if has(&args, "--no-background") { "vtr-rust-inline" } else { "vtr-rust" };
            if has(&args, "--as-tx") {
                println!("{}", logw::run_write_as_tx(n, &pos[2], writer_opts(&args)));
            } else {
                let rate = flag(&args, "--rate").map(|r| r.parse().unwrap());
                println!("{}", logw::run_write(n, &pos[2], writer_opts(&args), label, rate));
            }
        }
        "log-encode" => {
            let n: u64 = pos[1].parse().unwrap();
            println!("{}", serde_json::to_string_pretty(&logw::run_encode(n)).unwrap());
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
        "activity" => {
            let eps = flag(&args, "--eps").map(|s| s.parse().unwrap()).unwrap_or(0.01);
            let mem = flag(&args, "--mem").map(|s| s.parse().unwrap()).unwrap_or(0.04);
            let windows = flag(&args, "--windows").map(|s| s.parse().unwrap()).unwrap_or(400);
            let threads = flag(&args, "--threads").map(|s| s.parse().unwrap()).unwrap_or(16);
            println!("{}", serde_json::to_string_pretty(&activity::run(&pos[1], eps, mem, windows, threads, seed)).unwrap());
        }
        "activity-export" => {
            let eps = flag(&args, "--eps").map(|s| s.parse().unwrap()).unwrap_or(0.01);
            let mem = flag(&args, "--mem").map(|s| s.parse().unwrap()).unwrap_or(0.04);
            activity::export(&pos[1], pos[2].parse().unwrap(), pos[3].parse().unwrap(), eps, mem, &pos[4]);
        }
        "scopes" => scopes::run(&pos[1], flag(&args, "--children").as_deref()),
        "gen-gates" => {
            let copies = flag(&args, "--copies").map(|s| s.parse().unwrap()).unwrap_or(1);
            scopes::gen_gates(&pos[1], &pos[2], copies, seed);
        }
        "gen-bursty" => {
            let scale = flag(&args, "--scale").map(|s| s.parse().unwrap()).unwrap_or(4);
            activity::gen_bursty(&pos[1], scale, seed);
        }
        "stream" => {
            let r = vtr::Reader::open(&pos[1]).unwrap();
            let t = std::time::Instant::now();
            let mut n = 0u64;
            r.for_each_change(0, u64::MAX, |_, _, _| n += 1).unwrap();
            println!("{{\"changes\": {n}, \"wall_s\": {}}}", t.elapsed().as_secs_f64());
        }
        "digest" => {
            // Two recordings of the same run agree up to `t` when their digests do.
            let r = vtr::Reader::open(&pos[1]).unwrap();
            let until: u64 = pos[2].parse().unwrap();
            let mix = |h: u64| {
                let mut x = h.wrapping_add(0x9E37_79B9_7F4A_7C15);
                x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                x ^ (x >> 31)
            };
            let bytes = |b: &[u8]| b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3));
            let (mut changes, mut change_hash) = (0u64, 0u64);
            r.for_each_change(0, until, |t, s, v| {
                changes += 1;
                change_hash = change_hash.wrapping_add(mix(t ^ mix(s.0 as u64 ^ mix(bytes(format!("{v:?}").as_bytes())))));
            })
            .unwrap();
            let (mut logs, mut log_hash) = (0u64, 0u64);
            r.visit_log(&vtr::LogQuery { window: Some((0, until)), ..Default::default() }, |rec| {
                if r.full_path(rec.site.stream, ".") != vtr::ending::STREAM {
                    logs += 1;
                    log_hash = log_hash.wrapping_add(mix(rec.time ^ mix(bytes(rec.format(r.strings()).as_bytes()))));
                }
                true
            })
            .unwrap();
            let (end, at) = r.ending().unwrap();
            println!(
                "{{\"changes\": {changes}, \"change_hash\": \"{change_hash:016x}\", \"logs\": {logs}, \"log_hash\": \"{log_hash:016x}\", \"recovered\": {}, \"ending\": \"{end}\", \"ending_time\": {}}}",
                r.recovered().is_some(),
                at.map_or("null".to_string(), |t| t.to_string())
            );
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
