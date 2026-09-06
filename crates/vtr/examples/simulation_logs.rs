//! Reproducible simulation-log fixtures and million-message viewer workload.
//! cargo run --release -p vtr --example simulation_logs -- output.vtr [count]
use vtr::{LogArg, LogArgType, LogSiteSpec, Severity, Writer};
fn main() -> vtr::Result<()> {
    let path = std::env::args().nth(1).expect("output.vtr [count]");
    let count: u64 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "2000".into())
        .parse()
        .expect("count");
    let mut writer = Writer::create(path)?;
    writer.set_timescale(-9)?;
    let stream = writer.add_log_stream(None, "simulation_log");
    let sites: Vec<_> = [
        Severity::Trace,
        Severity::Debug,
        Severity::Info,
        Severity::Warn,
        Severity::Error,
        Severity::Fatal,
    ]
    .into_iter()
    .map(|level| {
        writer.add_log_site(
            &LogSiteSpec::new(stream, level, "{}", &[LogArgType::Text]).names(&["message"]),
        )
    })
    .collect();
    let other = writer.add_log_stream(None, "firmware_log");
    let firmware = writer.add_log_site(&LogSiteSpec::new(
        other,
        Severity::Info,
        "firmware checkpoint {}",
        &[LogArgType::U64],
    ));
    for n in 0..count {
        let (severity, message) = match n % 11 {
            0 => (2, format!("CPU0 retired instruction {n}: ADD x4, x2, x3")),
            1 => (
                1,
                format!(
                    "AXI read address=0x{:08x} outstanding=4",
                    0x80000000 + n * 64
                ),
            ),
            2 => (
                3,
                format!("DMA channel {} stalled: waiting for response", n % 4),
            ),
            3 => (2, format!("L2 cache refill complete: line {n}")),
            4 => (0, format!("Pipeline dispatch sequence={n}")),
            5 => (4, format!("AXI response SLVERR: transaction {n}")),
            6 => (2, "Reset released; simulation running".into()),
            7 => (1, format!("Unicode payload λ → receiver: packet {n}")),
            8 => (3, "Retry budget low\nBackpressure remains active".into()),
            9 => (2, format!("Scoreboard matched transaction {n}")),
            _ => (5, format!("Watchdog timeout on channel {}", n % 4)),
        };
        writer.log(sites[severity], n * 10, &[LogArg::Text(&message)])?;
        if n % 100 == 0 {
            writer.log(firmware, n * 10, &[LogArg::U64(n)])?;
        }
    }
    writer.close()
}
