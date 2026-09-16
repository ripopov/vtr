//! Baseline-compatible native loading measurement (no transport or rendering).
//! Run each sample in a fresh process so Linux VmHWM has a useful scope.
use std::collections::HashSet;
use std::time::Instant;
use volna_core::data::transactions::TrackKind;
use volna_core::session::OpenSpec;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("load_cost TRACE SIGNAL_COUNT|tracks");
    let selection = args.next().expect("SIGNAL_COUNT|tracks");
    let start = Instant::now();
    let session = OpenSpec::Path(path.clone().into()).open()?;
    let open_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut histories = Vec::new();
    let mut tracks = Vec::new();
    let mut changes = 0;
    let load_start = Instant::now();
    if selection == "tracks" {
        for track in session
            .tracks()
            .iter()
            .filter(|track| matches!(track.kind, TrackKind::Stream { .. }))
        {
            tracks.push(session.load_track(track.id)?);
        }
    } else {
        let count = selection.parse::<usize>()?;
        let mut seen = HashSet::new();
        let ids: Vec<_> = session
            .hierarchy()
            .vars
            .iter()
            .map(|v| v.signal)
            .filter(|id| seen.insert(*id))
            .take(count)
            .collect();
        // Match the executor's modest signal batches; retain every result.
        for batch in ids.chunks(64) {
            for (_, result) in session.load_signals(batch) {
                let history = result?;
                changes += history.len();
                histories.push(history);
            }
        }
    }
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;
    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
    let rss = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok())
        });
    println!(
        "{}",
        serde_json::json!({
            "path": path, "selection": selection, "open_ms": open_ms,
            "load_ms": load_ms, "total_ms": total_ms, "peak_rss_kib": rss,
            "signals": histories.len(), "changes": changes, "transactions": tracks.iter().flat_map(|track| &track.generators).map(|g| g.transactions().len()).sum::<usize>()
        })
    );
    std::hint::black_box((&histories, &tracks));
    Ok(())
}
