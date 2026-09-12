//! Logging demo: a toy simulator writes timestamped log messages next to a
//! transaction stream, then the file is read back three ways.
//!
//!     cargo run --release --example logging -- /tmp/demo_log.vtr
//!     vtr log /tmp/demo_log.vtr --severity warn

use vtr::{LogArgType, LogQuery, LogSiteSpec, Reader, ScopeType, Severity, TxQuery, TxStatus, Writer};

/// Registers a site with the calling location filled in.
macro_rules! log_site {
    ($w:expr, $stream:expr, $sev:expr, $fmt:literal, [$($t:ident),*]) => {
        $w.add_log_site(&LogSiteSpec::new($stream, $sev, $fmt, &[$(LogArgType::$t),*]).location(file!(), line!()))
    };
}

fn main() -> vtr::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "demo_log.vtr".into());
    let mut w = Writer::create(&path)?;
    w.set_timescale(-9)?; // ns

    // Hierarchy: a SoC with a CPU and a DMA engine; each component owns a LOG stream.
    let soc = w.begin_scope("soc", ScopeType::Generic, "");
    let cpu = w.begin_scope("cpu0", ScopeType::Core, "");
    let cpu_log = w.add_log_stream(Some(cpu), "log");
    w.end_scope()?;
    let dma = w.begin_scope("dma", ScopeType::Generic, "");
    let dma_log = w.add_log_stream(Some(dma), "log");
    let dma_tx = w.add_stream(Some(dma), "transfers", "TLM");
    let gen_xfer = w.add_generator(dma_tx, "transfer");
    w.end_scope()?;
    w.end_scope()?;
    let _ = soc;

    // Call sites: one generator each, registered once. Arguments are typed.
    let s_fetch = log_site!(w, cpu_log, Severity::Debug, "fetch pc={:#010x} inst={:#010x}", [U64, U64]);
    let s_retire = log_site!(w, cpu_log, Severity::Info, "retire pc={:#010x} rd=x{} value={:#x}", [U64, U64, U64]);
    let s_stall = log_site!(w, cpu_log, Severity::Warn, "{} stalled {} cycles waiting for {}", [Text, U64, Text]);
    let s_dma_start = log_site!(w, dma_log, Severity::Info, "channel {} start {} bytes {:#x} -> {:#x}", [U64, U64, U64, U64]);
    let s_dma_done = log_site!(w, dma_log, Severity::Info, "channel {} done in {:.2} us", [U64, F64]);
    let s_dma_err = log_site!(w, dma_log, Severity::Error, "channel {} bus error at {:#x} ({})", [U64, U64, Text]);

    let mut t = 0u64;
    let mut pc = 0x8000_0000u64;
    let mut open_xfer = None;
    for cycle in 0..20_000u64 {
        t += 10;
        w.log(s_fetch, t, &[pc.into(), (0x0040_0093 ^ cycle).into()])?;
        if cycle % 3 == 0 {
            w.log(s_retire, t + 2, &[pc.into(), (cycle % 32).into(), (cycle * 0x9e37).into()])?;
        }
        if cycle % 97 == 0 {
            let unit = if cycle % 2 == 0 { "lsu" } else { "fpu" };
            w.log(s_stall, t + 3, &[unit.into(), (cycle % 7 + 1).into(), "dcache".into()])?;
        }
        pc += 4;
        // A DMA transfer every 500 cycles: its messages are linked to the transaction.
        if cycle % 500 == 0 {
            let tx = w.begin_tx(gen_xfer, t)?;
            let ch = cycle / 500 % 4;
            w.log_with_parent(s_dma_start, t, Some(tx), &[ch.into(), 4096u64.into(), 0x1000_0000u64.into(), 0x2000_0000u64.into()])?;
            open_xfer = Some((tx, ch, t));
        }
        if let Some((tx, ch, t0)) = open_xfer {
            if t - t0 >= 3000 {
                if ch == 3 {
                    w.log_with_parent(s_dma_err, t, Some(tx), &[ch.into(), 0x1000_0800u64.into(), "SLVERR".into()])?;
                    w.end_tx(tx, t, TxStatus::Error)?;
                } else {
                    w.log_with_parent(s_dma_done, t, Some(tx), &[ch.into(), ((t - t0) as f64 / 1000.0).into()])?;
                    w.end_tx(tx, t, TxStatus::Ok)?;
                }
                open_xfer = None;
            }
        }
    }
    let stats = w.stats();
    w.close()?;
    println!("wrote {path}: {} log records, {} transactions, {} bytes", stats.log_records, stats.transactions, std::fs::metadata(&path)?.len());

    // 1. Warnings and errors from the whole SoC, formatted.
    let r = Reader::open(&path)?;
    println!("\n-- severity >= warn --");
    let mut n = 0;
    r.visit_log(&LogQuery { min_severity: Severity::Warn, ..Default::default() }, |rec| {
        n += 1;
        if n <= 5 {
            println!("{:>8} ns {:<5} {}: {}", rec.time, rec.severity().name(), r.full_path(rec.site.stream, "."), rec.format(r.strings()));
        }
        true
    })?;
    println!("({n} records)");

    // 2. The messages that belong to one DMA transaction (parent link).
    println!("\n-- log of the first DMA transfer --");
    let first = r.transactions(&TxQuery { stream: Some(dma_tx), ..Default::default() })?.into_iter().next().unwrap();
    r.visit_log(&LogQuery { stream: Some(dma_log), window: Some((first.begin, first.end)), ..Default::default() }, |rec| {
        if rec.parent == Some(first.id) {
            println!("{:>8} ns tx {} : {}", rec.time, first.id, rec.format(r.strings()));
        }
        true
    })?;

    // 3. Structured access: the arguments of one site without formatting.
    println!("\n-- longest stalls (structured query on site arguments) --");
    let stall_site = r.log_sites().iter().find(|s| r.str(s.fmt).starts_with("{} stalled")).unwrap();
    let mut worst = Vec::new();
    r.visit_log(&LogQuery { generator: Some(stall_site.node), ..Default::default() }, |rec| {
        if let Some(vtr::LogArg::U64(cycles)) = rec.arg(1) {
            worst.push((cycles, rec.time));
        }
        true
    })?;
    worst.sort_unstable_by(|a, b| b.cmp(a));
    for (cycles, time) in worst.iter().take(3) {
        println!("{cycles} cycles at {time} ns");
    }
    Ok(())
}
