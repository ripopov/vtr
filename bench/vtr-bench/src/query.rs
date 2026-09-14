//! Repeatable typed-session measurements, usable with the pre-cache revision.
use std::time::Instant;
use vtr_query::{native_session::Session, session::{Query, Reply},
    wave::{ChangeSearchResult, Direction, Limits, SignalTime}, Budget, Cancellation, Grid, Interval, TimeBound};

#[derive(Default, serde::Serialize)]
struct Measurement {
    wall_s: f64,
    pages: usize,
    progress_pages: usize,
    records: usize,
    changes: u64,
    edge: Option<u64>,
    timestamp_sum: u64,
}

fn execute(session: &mut Session<'_>, query: Query) -> Result<Measurement, String> {
    let start = Instant::now();
    let mut result = Measurement::default();
    let first = session.start(query, Limits { bytes: 1 << 20, records: 256, work: 4096 }, Cancellation::default())
        .map_err(|error| error.to_string())?;
    let mut cursor = first;
    let outcome: Result<(), String> = (|| {
        loop {
            let page = session.advance(cursor).map_err(|error| error.to_string())?;
            result.pages += 1;
            match &page.reply {
                Reply::Summary(summary) => {
                    result.records += summary.bins().len();
                    result.changes += summary.bins().iter().map(|bin| bin.changes).sum::<u64>();
                    result.progress_pages += usize::from(summary.bins().is_empty());
                }
                Reply::Window(window) => {
                    result.records += window.changes().len();
                    result.progress_pages += usize::from(window.changes().is_empty() && !window.complete);
                    for change in window.changes() {
                        result.timestamp_sum = result.timestamp_sum.wrapping_add(change.time);
                    }
                }
                Reply::Values(values) => result.records += values.samples().len(),
                Reply::FindChange(change) => match change.result {
                    ChangeSearchResult::Found(time) => { result.records += 1; result.edge = Some(time); },
                    ChangeSearchResult::Pending => result.progress_pages += 1,
                    ChangeSearchResult::Exhausted => {},
                },
                _ => return Err("unexpected query reply".into()),
            }
            std::hint::black_box(&page);
            match page.next { Some(next) => cursor = next, None => break }
        }
        Ok(())
    })();
    session.release(first).map_err(|error| error.to_string())?;
    outcome?;
    result.wall_s = start.elapsed().as_secs_f64();
    Ok(result)
}

pub fn run(path: &str, selector: &str) -> Result<serde_json::Value, String> {
    // Reference decoding is deliberately outside the measured reader/session.
    // Reject a zero-change selection rather than mistake it for a dense query.
    let reference = vtr::Reader::open(path).map_err(|error| error.to_string())?;
    let signal = selector.parse::<u32>().ok().map(vtr::SignalId)
        .or_else(|| reference.find_signal(selector, '.')).ok_or("unknown signal selector")?;
    let history = reference.load_signal(signal).map_err(|error| error.to_string())?;
    if history.is_empty() { return Err("selected signal has no changes; choose an active signal".into()); }
    let changes = history.len() as u64;
    let (start, end) = reference.time_range().ok_or("trace has no time range")?;
    let expected_edge = history.times().iter().rev().copied().find(|&time| time < end);
    // Pick the busiest 1% interval of this signal, outside the timer. A random
    // interval can contain no events even when the recording is large.
    let width = (u128::from(end - start) + 1).div_ceil(100);
    let times = history.times();
    let (mut left, mut best_left, mut best_right) = (0, 0, 0);
    for right in 0..times.len() {
        while u128::from(times[right] - times[left]) >= width { left += 1; }
        if right + 1 - left > best_right - best_left {
            (best_left, best_right) = (left, right + 1);
        }
    }
    let window_end = (u128::from(times[best_left]) + width).min(u128::from(end) + 1);
    let window = Interval::new(times[best_left], TimeBound::from_wide(window_end)
        .map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let window_records = best_right - best_left;
    let window_timestamp_sum = times[best_left..best_right].iter()
        .fold(0u64, |sum, &time| sum.wrapping_add(time));
    drop(history);
    drop(reference);
    let open = Instant::now();
    let reader = vtr::Reader::open(path).map_err(|error| error.to_string())?;
    let budget = Budget::new(512 << 20);
    let mut session = Session::new(&reader, budget.clone(), 4).map_err(|error| error.to_string())?;
    let open_s = open.elapsed().as_secs_f64();
    let grid = Grid::covering(session.info().time_range.unwrap(), 1024).map_err(|error| error.to_string())?.unwrap();
    let first = execute(&mut session, Query::Summary { signal: signal.0, grid })?;
    if first.changes != changes { return Err("first summary change count differs from reference".into()); }
    let mut repeated = Vec::new();
    let mut samples = Vec::new();
    let mut edges = Vec::new();
    let mut windows = Vec::new();
    for _ in 0..5 {
        let summary = execute(&mut session, Query::Summary { signal: signal.0, grid })?;
        if summary.changes != changes { return Err("repeated summary change count differs from reference".into()); }
        repeated.push(summary);
        let pairs = (0..1000).map(|i| SignalTime { signal: signal.0,
            time: start + ((u128::from(end - start) * i) / 999) as u64 }).collect();
        let points = execute(&mut session, Query::ValuesAt { pairs })?;
        if points.records != 1000 { return Err("missing exact samples".into()); }
        samples.push(points);
        let edge = execute(&mut session, Query::FindChange { signal: signal.0, from: end, direction: Direction::Previous })?;
        if edge.edge != expected_edge { return Err("edge timestamp differs from reference".into()); }
        edges.push(edge);
        let exact = execute(&mut session, Query::Window { signal: signal.0, interval: window })?;
        if exact.records != window_records || exact.timestamp_sum != window_timestamp_sum {
            return Err("exact window count or timestamp sum differs from reference".into());
        }
        windows.push(exact);
    }
    let reserved_query_bytes = budget.used();
    drop(session);
    if budget.used() != 0 { return Err("query reservations survive session destruction".into()); }
    Ok(serde_json::json!({ "file": path, "file_bytes": std::fs::metadata(path).map_err(|error| error.to_string())?.len(),
        "signal": signal.0, "changes": changes, "bins": grid.count(), "open_s": open_s,
        "first_summary": first, "repeated_summaries": repeated, "point_batches": samples,
        "previous_edges": edges, "exact_windows": windows,
        "window": { "start": window.start(), "end_exclusive": window.end().wide().to_string(),
            "records": window_records, "selection": "busiest 1% interval of selected signal" },
        "reserved_query_bytes": reserved_query_bytes,
        "method": "fresh reader caches, OS cache warmed by reference; one signal, typed session, 5 repeated trials; no network or frame timing" }))
}
