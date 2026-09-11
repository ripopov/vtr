use crate::{
    rowset::{Builder, RowSet},
    source::Source,
};
use anyhow::Result;
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use rayon::prelude::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Predicate {
    pub column: usize,
    pub text: String,
}
struct Fuzzy {
    pattern: Pattern,
    matcher: Matcher,
    buffer: Vec<char>,
    ascii_literal: Option<Vec<u8>>,
}
impl Fuzzy {
    fn new(text: &str) -> Self {
        Self {
            pattern: Pattern::new(
                text,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            ),
            matcher: Matcher::new(Config::DEFAULT),
            buffer: Vec::new(),
            ascii_literal: (!text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b)))
            .then(|| text.bytes().map(|b| b.to_ascii_lowercase()).collect()),
        }
    }
    fn matches(&mut self, text: &str) -> bool {
        // For one literal ASCII atom, boolean fuzzy matching is exactly a
        // case-insensitive subsequence test. Avoid allocating/converting UTF-32
        // and computing a score that the filter never uses. Non-ASCII values and
        // pattern operators retain nucleo's Unicode/normalization semantics.
        if let Some(needle) = &self.ascii_literal
            && text.is_ascii()
        {
            let mut matched = 0;
            for byte in text.bytes() {
                if byte.to_ascii_lowercase() == needle[matched] {
                    matched += 1;
                    if matched == needle.len() {
                        return true;
                    }
                }
            }
            return false;
        }
        self.pattern
            .score(Utf32Str::new(text, &mut self.buffer), &mut self.matcher)
            .is_some()
    }
}

pub enum Request {
    Filter {
        id: u64,
        predicates: Vec<Predicate>,
    },
    Search {
        id: u64,
        predicate: Predicate,
        view: RowSet,
    },
}
pub enum Event {
    Progress {
        id: u64,
        fraction: f32,
    },
    Filtered {
        id: u64,
        view: RowSet,
        elapsed: Duration,
    },
    Matches {
        id: u64,
        matches: RowSet,
        done: bool,
        fraction: f32,
        elapsed: Duration,
    },
    Error {
        id: u64,
        message: String,
    },
}
/// One coordinator, a bounded CPU pool, and a latest-generation token. No
/// per-keystroke thread creation. Events carry immutable rank/select snapshots.
pub struct Worker {
    tx: Sender<Request>,
    pub rx: Receiver<Event>,
    generation: Arc<AtomicU64>,
}
impl Worker {
    pub fn new(store: Arc<Source>, ctx: egui::Context, workers: usize) -> std::io::Result<Self> {
        let (tx, requests) = mpsc::channel();
        let (events, rx) = mpsc::sync_channel(2);
        let generation = Arc::new(AtomicU64::new(0));
        let token = generation.clone();
        thread::Builder::new()
            .name("atlas-query".into())
            .spawn(move || {
                let pool = match rayon::ThreadPoolBuilder::new()
                    .num_threads(workers.max(1))
                    .thread_name(|n| format!("atlas-scan-{n}"))
                    .build()
                {
                    Ok(pool) => pool,
                    Err(e) => {
                        let _ = events.send(Event::Error {
                            id: 0,
                            message: e.to_string(),
                        });
                        return;
                    }
                };
                while let Ok(mut request) = requests.recv() {
                    while let Ok(newer) = requests.try_recv() {
                        request = newer;
                    }
                    let id = match &request {
                        Request::Filter { id, .. } | Request::Search { id, .. } => *id,
                    };
                    let cancelled = || token.load(Ordering::Relaxed) != id;
                    if cancelled() {
                        continue;
                    }
                    let start = Instant::now();
                    let emit = |event| {
                        match event {
                            Event::Progress { .. } | Event::Matches { done: false, .. } => {
                                let _ = events.try_send(event);
                            }
                            _ => {
                                let _ = events.send(event);
                            }
                        }
                        ctx.request_repaint();
                    };
                    let result = match request {
                        Request::Filter { predicates, .. } => {
                            filter(&store, &predicates, &pool, &cancelled, |fraction| {
                                emit(Event::Progress { id, fraction })
                            })
                            .map(|view| {
                                if let Some(view) = view {
                                    emit(Event::Filtered {
                                        id,
                                        view,
                                        elapsed: start.elapsed(),
                                    });
                                }
                            })
                        }
                        Request::Search {
                            predicate, view, ..
                        } => search(
                            &store,
                            &predicate,
                            &view,
                            &cancelled,
                            |matches, done, fraction| {
                                emit(Event::Matches {
                                    id,
                                    matches,
                                    done,
                                    fraction,
                                    elapsed: start.elapsed(),
                                });
                            },
                        ),
                    };
                    if let Err(e) = result
                        && !cancelled()
                    {
                        emit(Event::Error {
                            id,
                            message: format!("{e:#}"),
                        });
                    }
                }
            })?;
        Ok(Self { tx, rx, generation })
    }
    pub fn cancel(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn submit(&self, request: Request) {
        let _ = self.tx.send(request);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn filter_group(
    store: &Source,
    group: u32,
    predicates: &[Predicate],
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<u32>> {
    let start = group * store.schema.group_rows;
    let mut candidates: Vec<u32> = (0..store.block_len(group)).collect();
    for predicate in predicates {
        if cancelled() || candidates.is_empty() {
            return Ok(Vec::new());
        }
        let block = store.read_block(group, predicate.column)?;
        let mut matcher = Fuzzy::new(&predicate.text);
        let mut accepted = Vec::with_capacity(block.dictionary.len());
        for (i, value) in block.dictionary.iter().enumerate() {
            if i % 256 == 0 && cancelled() {
                return Ok(Vec::new());
            }
            accepted.push(matcher.matches(value));
        }
        candidates.retain(|&r| accepted[block.codes[r as usize] as usize]);
    }
    for r in &mut candidates {
        *r += start;
    }
    Ok(candidates)
}
pub fn filter(
    store: &Source,
    predicates: &[Predicate],
    pool: &rayon::ThreadPool,
    cancelled: &(impl Fn() -> bool + Sync),
    mut progress: impl FnMut(f32),
) -> Result<Option<RowSet>> {
    if predicates.is_empty() {
        return Ok(Some(RowSet::All(store.schema.rows)));
    }
    let mut bits = Builder::new(store.schema.rows);
    let batch = pool.current_num_threads() as u32 * 2;
    for base in (0..store.groups()).step_by(batch as usize) {
        if cancelled() {
            return Ok(None);
        }
        let end = base.saturating_add(batch).min(store.groups());
        let chunks: Vec<Result<Vec<u32>>> = pool.install(|| {
            (base..end)
                .into_par_iter()
                .map(|g| filter_group(store, g, predicates, cancelled))
                .collect()
        });
        if cancelled() {
            return Ok(None);
        }
        for chunk in chunks {
            for row in chunk? {
                bits.insert(row);
            }
        }
        progress(end as f32 / store.groups() as f32);
    }
    Ok(Some(bits.build()))
}
pub fn search(
    store: &Source,
    predicate: &Predicate,
    view: &RowSet,
    cancelled: &impl Fn() -> bool,
    mut publish: impl FnMut(RowSet, bool, f32),
) -> Result<()> {
    let mut bits = Builder::new(store.schema.rows);
    let mut matcher = Fuzzy::new(&predicate.text);
    let mut previous_dictionary = Vec::new();
    let mut accepted = Vec::new();
    let mut last_publish = Instant::now();
    let mut first_published = false;
    for group in 0..store.groups() {
        if cancelled() {
            return Ok(());
        }
        let start = group * store.schema.group_rows;
        let end = start + store.block_len(group);
        if view.rank_before(end) > view.rank_before(start) {
            let block = store.read_block(group, predicate.column)?;
            if previous_dictionary != block.dictionary {
                accepted.clear();
                for (i, value) in block.dictionary.iter().enumerate() {
                    if i % 256 == 0 && cancelled() {
                        return Ok(());
                    }
                    accepted.push(matcher.matches(value));
                }
                previous_dictionary = block.dictionary;
            }
            let mut found = false;
            for (offset, &code) in block.codes.iter().enumerate() {
                let row = start + offset as u32;
                if accepted[code as usize] && view.contains(row) {
                    bits.insert(row);
                    found = true;
                }
            }
            if (found && !first_published) || last_publish.elapsed() >= Duration::from_millis(150) {
                publish(
                    bits.snapshot(),
                    false,
                    (group + 1) as f32 / store.groups() as f32,
                );
                first_published |= found;
                last_publish = Instant::now();
            }
        }
    }
    if !cancelled() {
        publish(bits.build(), true, 1.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_semantics() {
        assert!(Fuzzy::new("untd").matches("United Kingdom"));
        assert!(Fuzzy::new("AMORG").matches("Amelia Morgan"));
        assert!(!Fuzzy::new("xyz").matches("United Kingdom"));
        assert!(Fuzzy::new("chloe").matches("Chloé"));
    }
    #[test]
    fn ascii_fast_path_matches_nucleo() {
        for needle in [
            "AT-10000000",
            "untd",
            "AMORG",
            "a_b",
            "a.b",
            "a@b",
            "0",
            "",
            "a b",
            "^ab",
            "!ab",
            "é",
        ] {
            for text in [
                "",
                "AT-00000001",
                "AT-10000000",
                "United Kingdom",
                "Amelia Morgan",
                "A__B",
                "A.xB",
                "A@b",
                "Chloé",
                "abc",
                "a b",
            ] {
                let mut fuzzy = Fuzzy::new(needle);
                let actual = fuzzy.matches(text);
                fuzzy.ascii_literal = None;
                assert_eq!(actual, fuzzy.matches(text), "{needle:?} in {text:?}");
            }
        }
    }
    #[test]
    fn async_scan_mapping_and_cancellation() -> Result<()> {
        let store = crate::test_support::source(33_000, 30);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build()?;
        let p = vec![
            Predicate {
                column: 3,
                text: "untd".into(),
            },
            Predicate {
                column: 4,
                text: "actv".into(),
            },
        ];
        let view = filter(&store, &p, &pool, &|| false, |_| {})?.unwrap();
        let mut expected = Vec::new();
        for group in 0..store.groups() {
            let country = store.read_block(group, 3)?;
            let status = store.read_block(group, 4)?;
            for offset in 0..store.block_len(group) as usize {
                if country.value(offset).starts_with("United") && status.value(offset) == "Active" {
                    expected.push(group * store.schema.group_rows + offset as u32);
                }
            }
        }
        assert_eq!(
            (0..view.len())
                .map(|i| view.select(i).unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(filter(&store, &p, &pool, &|| true, |_| {})?.is_none());
        let mut seen = 0;
        search(
            &store,
            &Predicate {
                column: 0,
                text: "AT-000".into(),
            },
            &view,
            &|| false,
            |matches, done, _| {
                assert!(matches.len() >= seen);
                seen = matches.len();
                for i in 0..matches.len() {
                    assert!(view.contains(matches.select(i).unwrap()));
                }
                if done {
                    assert_eq!(matches.len(), view.len());
                }
            },
        )?;
        assert!(seen > 0);
        drop(store);
        Ok(())
    }
}
