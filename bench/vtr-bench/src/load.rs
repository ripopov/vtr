//! Whole-history loads with explicitly unique or repeated signal requests.

use crate::{replay::Rng, util::best_of};
use std::collections::BTreeSet;
use vtr::{Reader, SignalId};

fn plan(n: u32, mode: &str, count: usize, seed: u64) -> Result<Vec<SignalId>, String> {
    if !matches!(mode, "unique" | "requests") {
        return Err("mode must be unique or requests".into());
    }
    let count = if mode == "unique" { count.min(n as usize) } else { count };
    if n == 0 && count != 0 {
        return Err("cannot select signals from an empty file".into());
    }
    let mut rng = Rng(seed);
    let mut ids = Vec::with_capacity(count);
    if mode == "unique" {
        // Floyd sampling needs exactly count draws, even with a degenerate seed
        // or when requesting every signal. Rejection sampling can fail to terminate.
        let mut selected = BTreeSet::new();
        for upper in n - count as u32..n {
            let candidate = rng.below(u64::from(upper) + 1) as u32;
            let id = if selected.insert(candidate) {
                candidate
            } else {
                selected.insert(upper);
                upper
            };
            ids.push(SignalId(id));
        }
    } else {
        ids.extend((0..count).map(|_| SignalId(rng.below(u64::from(n)) as u32)));
    }
    Ok(ids)
}

pub fn run(path: &str, mode: &str, count: usize, seed: u64) -> Result<serde_json::Value, String> {
    let n = Reader::open(path).map_err(|e| e.to_string())?.signal_count();
    let ids = plan(n, mode, count, seed)?;
    let unique = ids.iter().map(|s| s.0).collect::<BTreeSet<_>>().len();
    let (wall_s, changes) = best_of(3, || -> Result<usize, String> {
        let reader = Reader::open(path).map_err(|e| e.to_string())?;
        let loaded = reader.load_signals(&ids).map_err(|e| e.to_string())?;
        std::hint::black_box(&loaded);
        Ok(loaded.iter().map(|d| d.len()).sum())
    });
    Ok(serde_json::json!({"mode": mode, "requests": ids.len(), "unique_signals": unique, "changes": changes?, "wall_s": wall_s}))
}

#[cfg(test)]
mod tests {
    use super::plan;
    use std::collections::BTreeSet;

    #[test]
    fn unique_requests_terminate_and_cover_population_even_with_zero_seed() {
        for seed in [0, 42, u64::MAX] {
            for count in [0, 1, 7, 16, 1000] {
                let ids = plan(16, "unique", count, seed).unwrap();
                assert_eq!(ids.len(), count.min(16));
                assert_eq!(ids.iter().map(|s| s.0).collect::<BTreeSet<_>>().len(), ids.len());
                assert!(ids.iter().all(|s| s.0 < 16));
                assert_eq!(ids, plan(16, "unique", count, seed).unwrap());
            }
        }
    }

    #[test]
    fn repeated_and_empty_requests_keep_their_contract() {
        assert_eq!(plan(1, "requests", 20, 0).unwrap().len(), 20);
        assert!(plan(0, "requests", 1, 42).is_err());
        assert!(plan(0, "requests", 0, 42).unwrap().is_empty());
        assert!(plan(0, "unique", 100, 42).unwrap().is_empty());
        assert!(plan(1, "invalid", 1, 42).is_err());
    }
}
